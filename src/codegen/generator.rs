//! コード生成器（Code Generator）
//!
//! C の AST を走査し、対応する x86-64 アセンブリ命令列に変換する。
//!
//! # Chapter 11: 型対応コード生成
//! `Type::Int` → `AsmType::Longword` (32bit), `Type::Long` → `AsmType::Quadword` (64bit)
//! `Cast` ノードに応じて `Movsx` (int→long) や `Truncate` (long→int) を生成する。
//!
//! # Chapter 12: 符号なし整数対応
//! - `VarLocation` に `Type` を保持し、符号情報をコード生成まで伝搬
//! - `Cast`: `source_type` の符号で `Movsx` (符号拡張) / `MovZeroExtend` (ゼロ拡張) を選択
//! - 除算/剰余: 符号なしなら `Div`（DX=0）、符号付きなら `SignExtend` + `Idiv`
//! - 比較: 符号なしなら `B`/`BE`/`A`/`AE`、符号付きなら `L`/`LE`/`G`/`GE`
//!
//! # Chapter 13: 浮動小数点数 (`double`)
//! - `Type::Double` → `AsmType::Double`、結果は XMM0 レジスタに保持
//! - 定数プール: `ConstantDouble` → `.rodata` の `AsmStaticConstant` 経由で `movsd` ロード
//! - SSE 算術: `addsd`/`subsd`/`mulsd`/`divsd`、比較は `comisd` + unsigned 条件コード
//! - 単項 `-` は `xorpd` で符号ビット反転（`-0.0` 定数とXOR）
//! - 型変換: `Cvtsi2sd` (整数→double), `Cvttsd2si` (double→整数, 切り捨て)
//! - 関数呼出規約: 整数引数 (DI,SI,DX,CX,R8,R9) と double 引数 (XMM0〜XMM7) を独立カウント
//!
//! # Chapter 14: ポインタ
//! - `Dereref` (間接参照): アドレスを生成し `Mov { src: Memory(AX), dst: AX }` で値を読み出す
//! - `AddrOf` (アドレス取得): `generate_lvalue_addr()` で左辺値のアドレスを AX に配置
//! - `generate_lvalue_addr()`: `Var` → `Lea`、`Dereref` → 内部式を評価（値がそのままアドレス）
//! - ポインタ経由の代入: LHS アドレスを Push → RHS 評価 → Pop CX → `Mov { src: AX, dst: Memory(CX) }`
//! - ポインタ ↔ 整数キャスト: 同サイズ (64bit) → no-op、小→大 → 符号/ゼロ拡張、大→小 → 切り詰め
//!
//! # Chapter 15: 配列とポインタ算術
//! - 配列変数: `Var` が配列型なら `Lea`（アドレスロード）で decay → ポインタとして扱う
//! - 配列スタック割り当て: `elem.size() * count` バイトを確保
//! - グローバル配列: `ZeroInit(size)` で `.bss` セクションにゼロ初期化
//! - ポインタ加算 (`ptr + int`): 右辺を `elem_size` で `imulq` スケーリング後に `addq`
//! - ポインタ減算 (`ptr - ptr`): バイト差分を `idivq elem_size` で要素数に変換
//! - ポインタ増減 (`++`/`--`/`+=`/`-=`): 増分を `sizeof(*ptr)` にスケーリング
//! - `sizeof`: 型チェッカーで `ConstantULong` に解決済み（コード生成では到達しない）
//!
//! # Chapter 17: void 型と void ポインタ
//! - `Type::Void` → `unreachable!`（void 値のコード生成は型チェッカーが防止）
//! - void 関数の `return;`: `Ret` のみ（式の評価なし）
//! - void 関数の暗黙 return: 値を返さず `Ret` のみ
//! - `(void)expr` キャスト: 内部式を評価し結果を破棄
//! - `void *` ↔ 他のポインタ型: 同サイズ (8 バイト) で no-op
//! - `expr_type()`: void 関数呼び出し → `Type::Void`、void キャスト → `Type::Void`

use std::collections::{HashMap, HashSet};
use crate::error::{CompileError, Result};
use crate::parse::ast::{
    Program, FunctionDecl, BlockItem, Declaration, Statement, Expr,
    UnaryOp, BinaryOp, ForInit, StorageClass, TopLevelDecl, Type,
};
use super::asm_ast::{
    AsmProgram, AsmFunction, AsmStaticVar, AsmStaticConstant, StaticInit,
    Instruction, Operand, Reg,
    AsmUnaryOp, AsmBinaryOp, CondCode, AsmType,
};

/// ループ内の break/continue ジャンプ先ラベル（Chapter 8）。
struct LoopLabels {
    break_label: String,
    continue_label: String,
}

/// 関数シンボルテーブル用の情報（Chapter 9, 11）。
struct FunctionInfo {
    param_count: usize,
    defined: bool,
    return_type: Type,
    param_types: Vec<Type>,
}

/// 変数の格納場所（Chapter 10, 11, 12）。
/// Type を保持して符号情報を保存する。
#[derive(Debug, Clone)]
enum VarLocation {
    Stack(i32, Type),
    Static(String, Type),
}

/// 引数レジスタの順序（System V AMD64 ABI）
const ARG_REGISTERS: [Reg; 6] = [Reg::DI, Reg::SI, Reg::DX, Reg::CX, Reg::R8, Reg::R9];

/// XMM 引数レジスタの順序（System V AMD64 ABI）
const XMM_ARG_REGISTERS: [Reg; 8] = [
    Reg::XMM0, Reg::XMM1, Reg::XMM2, Reg::XMM3,
    Reg::XMM4, Reg::XMM5, Reg::XMM6, Reg::XMM7,
];

/// Type → AsmType 変換。
fn type_to_asm(t: &Type) -> AsmType {
    match t {
        Type::Void => unreachable!("void has no assembly representation"),
        Type::Char | Type::UChar => AsmType::Byte,
        Type::Int | Type::UInt => AsmType::Longword,
        Type::Long | Type::ULong => AsmType::Quadword,
        Type::Double => AsmType::Double,
        Type::Pointer(_) => AsmType::Quadword,
        // 配列は decay 後ポインタとして扱う（Chapter 15）
        Type::Array(_, _) => AsmType::Quadword,
    }
}

/// AsmType のバイトサイズ。
fn asm_type_size(t: AsmType) -> i32 {
    match t {
        AsmType::Byte => 1,
        AsmType::Longword => 4,
        AsmType::Quadword | AsmType::Double => 8,
    }
}

/// 変数名からオペランド、AsmType、Type を解決する。
fn resolve_var(
    var_map: &HashMap<String, VarLocation>,
    global_var_map: &HashMap<String, VarLocation>,
    name: &str,
) -> Result<(Operand, AsmType, Type)> {
    if let Some(loc) = var_map.get(name) {
        match loc {
            VarLocation::Stack(offset, var_type) => Ok((Operand::Stack(*offset), type_to_asm(var_type), var_type.clone())),
            VarLocation::Static(label, var_type) => Ok((Operand::Data(label.clone()), type_to_asm(var_type), var_type.clone())),
        }
    } else if let Some(loc) = global_var_map.get(name) {
        match loc {
            VarLocation::Stack(_, _) => unreachable!("global var should not be on stack"),
            VarLocation::Static(label, var_type) => Ok((Operand::Data(label.clone()), type_to_asm(var_type), var_type.clone())),
        }
    } else {
        Err(CompileError::CodegenError(format!(
            "undeclared variable '{}'", name
        )))
    }
}

/// 静的定数初期値を解決する。
fn resolve_static_init(init_expr: &Expr) -> std::result::Result<StaticInit, String> {
    match init_expr {
        Expr::Constant(v) => Ok(StaticInit::IntInit(*v)),
        Expr::ConstantLong(v) => Ok(StaticInit::IntInit(*v)),
        Expr::ConstantUInt(v) => Ok(StaticInit::IntInit(*v as i64)),
        Expr::ConstantULong(v) => Ok(StaticInit::IntInit(*v as i64)),
        Expr::ConstantDouble(v) => Ok(StaticInit::DoubleInit(*v)),
        _ => Err("must be initialized with a constant expression".to_string()),
    }
}

/// C の AST をアセンブリ AST に変換する。
pub fn generate(program: &Program) -> Result<AsmProgram> {
    let mut label_counter = 0;
    let mut static_label_counter: usize = 0;

    let mut func_table: HashMap<String, FunctionInfo> = HashMap::new();
    let mut global_var_map: HashMap<String, VarLocation> = HashMap::new();
    let mut static_vars: Vec<AsmStaticVar> = Vec::new();
    // double 定数プール: ビットパターン → (ラベル, アラインメント)
    let mut double_constants: HashMap<u64, (String, usize)> = HashMap::new();
    let mut const_label_counter: usize = 0;
    // 文字列定数プール: (ラベル, 内容)（Chapter 16）
    let mut string_constants: Vec<(String, String)> = Vec::new();
    let mut string_label_counter: usize = 0;

    for decl in &program.declarations {
        match decl {
            TopLevelDecl::Function(func_decl) => {
                let has_body = func_decl.body.is_some();
                let param_types: Vec<Type> = func_decl.params.iter().map(|(t, _)| t.clone()).collect();
                if let Some(existing) = func_table.get(&func_decl.name) {
                    if existing.param_count != func_decl.params.len() {
                        return Err(CompileError::CodegenError(format!(
                            "conflicting parameter count for function '{}'", func_decl.name
                        )));
                    }
                    if has_body && existing.defined {
                        return Err(CompileError::CodegenError(format!(
                            "function '{}' defined multiple times", func_decl.name
                        )));
                    }
                }
                let entry = func_table.entry(func_decl.name.clone()).or_insert(FunctionInfo {
                    param_count: func_decl.params.len(),
                    defined: false,
                    return_type: func_decl.return_type.clone(),
                    param_types: param_types.clone(),
                });
                if has_body {
                    entry.defined = true;
                }
            }
            TopLevelDecl::Variable(var_decl) => {
                let sc = var_decl.storage_class;
                let asm_type = type_to_asm(&var_decl.var_type);

                if sc == Some(StorageClass::Extern) && var_decl.init.is_some() {
                    return Err(CompileError::CodegenError(format!(
                        "extern variable '{}' cannot have initializer", var_decl.name
                    )));
                }

                let init_val = if let Some(init_expr) = &var_decl.init {
                    resolve_static_init(init_expr).map_err(|msg| {
                        CompileError::CodegenError(format!(
                            "file-scope variable '{}' {}", var_decl.name, msg
                        ))
                    })?
                } else if var_decl.var_type.is_array() {
                    StaticInit::ZeroInit(var_decl.var_type.size())
                } else {
                    if var_decl.var_type == Type::Double {
                        StaticInit::DoubleInit(0.0)
                    } else {
                        StaticInit::IntInit(0)
                    }
                };

                global_var_map.insert(
                    var_decl.name.clone(),
                    VarLocation::Static(var_decl.name.clone(), var_decl.var_type.clone()),
                );

                match sc {
                    Some(StorageClass::Extern) => {}
                    Some(StorageClass::Static) => {
                        static_vars.push(AsmStaticVar {
                            name: var_decl.name.clone(),
                            global: false,
                            init: init_val,
                            asm_type,
                        });
                    }
                    None => {
                        static_vars.push(AsmStaticVar {
                            name: var_decl.name.clone(),
                            global: true,
                            init: init_val,
                            asm_type,
                        });
                    }
                }
            }
        }
    }

    let mut functions = Vec::new();
    for decl in &program.declarations {
        if let TopLevelDecl::Function(func_decl) = decl {
            if func_decl.body.is_some() {
                let global = func_decl.storage_class != Some(StorageClass::Static);
                functions.push(generate_function(
                    func_decl,
                    &mut label_counter,
                    &func_table,
                    &global_var_map,
                    &mut static_vars,
                    &mut static_label_counter,
                    global,
                    &mut double_constants,
                    &mut const_label_counter,
                    &mut string_constants,
                    &mut string_label_counter,
                )?);
            }
        }
    }

    // 定数プールを AsmStaticConstant に変換
    let mut static_constants: Vec<AsmStaticConstant> = double_constants
        .into_iter()
        .map(|(bits, (name, alignment))| {
            AsmStaticConstant {
                name,
                alignment,
                init: StaticInit::DoubleInit(f64::from_bits(bits)),
            }
        })
        .collect();
    // 文字列定数を追加（Chapter 16）
    for (label, content) in string_constants {
        let byte_len = content.len() + 1; // +1 for null terminator
        static_constants.push(AsmStaticConstant {
            name: label,
            alignment: 1,
            init: StaticInit::StringInit(content, byte_len),
        });
    }

    // ラベル名でソート（決定論的な出力のため）
    static_constants.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(AsmProgram { functions, static_vars, static_constants })
}

/// パラメータを分類する: (int_reg_idx, xmm_reg_idx, stack_offset)
struct ParamClassification {
    /// 各パラメータの受け取り先: Register(reg) or Stack(offset)
    locations: Vec<(Type, ParamLocation)>,
    /// スタック引数の数
    stack_count: usize,
}

enum ParamLocation {
    IntReg(usize),  // ARG_REGISTERS のインデックス
    XmmReg(usize),  // XMM_ARG_REGISTERS のインデックス
    Stack(i32),     // 16(%rbp) + N*8
}

fn classify_parameters(params: &[(Type, String)]) -> ParamClassification {
    let mut int_idx = 0;
    let mut xmm_idx = 0;
    let mut stack_idx = 0;
    let mut locations = Vec::new();

    for (param_type, _) in params {
        if param_type.is_double() {
            if xmm_idx < 8 {
                locations.push((param_type.clone(), ParamLocation::XmmReg(xmm_idx)));
                xmm_idx += 1;
            } else {
                let offset = 16 + stack_idx * 8;
                locations.push((param_type.clone(), ParamLocation::Stack(offset as i32)));
                stack_idx += 1;
            }
        } else {
            if int_idx < 6 {
                locations.push((param_type.clone(), ParamLocation::IntReg(int_idx)));
                int_idx += 1;
            } else {
                let offset = 16 + stack_idx * 8;
                locations.push((param_type.clone(), ParamLocation::Stack(offset as i32)));
                stack_idx += 1;
            }
        }
    }

    ParamClassification { locations, stack_count: stack_idx }
}

fn generate_function(
    func: &FunctionDecl,
    label_counter: &mut usize,
    func_table: &HashMap<String, FunctionInfo>,
    global_var_map: &HashMap<String, VarLocation>,
    static_vars: &mut Vec<AsmStaticVar>,
    static_label_counter: &mut usize,
    global: bool,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_label_counter: &mut usize,
    string_constants: &mut Vec<(String, String)>,
    string_label_counter: &mut usize,
) -> Result<AsmFunction> {
    let mut var_map: HashMap<String, VarLocation> = HashMap::new();
    let mut next_offset: i32 = 0;
    let mut instructions = Vec::new();

    // パラメータを分類して割り当て
    let classification = classify_parameters(&func.params);

    for (i, (param_type, param_name)) in func.params.iter().enumerate() {
        let asm_type = type_to_asm(param_type);
        let size = asm_type_size(asm_type);
        // アラインメント考慮
        next_offset -= size;
        if next_offset % size != 0 {
            next_offset -= size + (next_offset % size);
        }
        let offset = next_offset;
        var_map.insert(param_name.clone(), VarLocation::Stack(offset, param_type.clone()));

        let (_, ref loc) = classification.locations[i];
        match loc {
            ParamLocation::IntReg(idx) => {
                instructions.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Register(ARG_REGISTERS[*idx]),
                    dst: Operand::Stack(offset),
                });
            }
            ParamLocation::XmmReg(idx) => {
                instructions.push(Instruction::Mov {
                    asm_type: AsmType::Double,
                    src: Operand::Register(XMM_ARG_REGISTERS[*idx]),
                    dst: Operand::Stack(offset),
                });
            }
            ParamLocation::Stack(stack_offset) => {
                if param_type.is_double() {
                    // Load from stack arg into XMM then to local
                    instructions.push(Instruction::Mov {
                        asm_type: AsmType::Double,
                        src: Operand::Stack(*stack_offset),
                        dst: Operand::Register(Reg::XMM0),
                    });
                    instructions.push(Instruction::Mov {
                        asm_type: AsmType::Double,
                        src: Operand::Register(Reg::XMM0),
                        dst: Operand::Stack(offset),
                    });
                } else {
                    instructions.push(Instruction::Mov {
                        asm_type: AsmType::Quadword,  // スタック引数は常に8バイト
                        src: Operand::Stack(*stack_offset),
                        dst: Operand::Register(Reg::AX),
                    });
                    instructions.push(Instruction::Mov {
                        asm_type,
                        src: Operand::Register(Reg::AX),
                        dst: Operand::Stack(offset),
                    });
                }
            }
        }
    }

    let body = func.body.as_ref().unwrap();
    let mut scope_decls: HashSet<String> = HashSet::new();
    for (_, param_name) in &func.params {
        scope_decls.insert(param_name.clone());
    }
    for item in body {
        let instrs = generate_block_item(
            item, &mut var_map, &mut next_offset, label_counter,
            Some(&mut scope_decls), None, func_table, global_var_map,
            static_vars, static_label_counter, double_constants, const_label_counter,
            string_constants, string_label_counter,
        )?;
        instructions.extend(instrs);
    }

    // 暗黙の return (void 関数は値なし、non-void は return 0)
    if func.return_type.is_void() {
        // void 関数: 値を返さない
    } else if func.return_type == Type::Double {
        // xorpd %xmm0, %xmm0 で 0.0 を返す
        instructions.push(Instruction::Binary {
            asm_type: AsmType::Double,
            op: AsmBinaryOp::Xor,
            src: Operand::Register(Reg::XMM0),
            dst: Operand::Register(Reg::XMM0),
        });
    } else {
        let ret_asm_type = type_to_asm(&func.return_type);
        instructions.push(Instruction::Mov {
            asm_type: ret_asm_type,
            src: Operand::Imm(0),
            dst: Operand::Register(Reg::AX),
        });
    }
    instructions.push(Instruction::Ret);

    // AllocateStack（16バイトアラインメント）
    let total_bytes = (-next_offset) as usize;
    if total_bytes > 0 {
        let aligned = (total_bytes + 15) & !15;
        instructions.insert(0, Instruction::AllocateStack(aligned));
    }

    Ok(AsmFunction {
        name: func.name.clone(),
        instructions,
        global,
    })
}

fn generate_block_item(
    item: &BlockItem,
    var_map: &mut HashMap<String, VarLocation>,
    next_offset: &mut i32,
    label_counter: &mut usize,
    scope_decls: Option<&mut HashSet<String>>,
    loop_labels: Option<&LoopLabels>,
    func_table: &HashMap<String, FunctionInfo>,
    global_var_map: &HashMap<String, VarLocation>,
    static_vars: &mut Vec<AsmStaticVar>,
    static_label_counter: &mut usize,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_label_counter: &mut usize,
    string_constants: &mut Vec<(String, String)>,
    string_label_counter: &mut usize,
) -> Result<Vec<Instruction>> {
    match item {
        BlockItem::Statement(stmt) => generate_statement(
            stmt, var_map, next_offset, label_counter, loop_labels,
            func_table, global_var_map, static_vars, static_label_counter,
            double_constants, const_label_counter,
            string_constants, string_label_counter,
        ),
        BlockItem::Declaration(decl) => generate_declaration(
            decl, var_map, next_offset, label_counter, scope_decls,
            func_table, global_var_map, static_vars, static_label_counter,
            double_constants, const_label_counter,
            string_constants, string_label_counter,
        ),
    }
}

fn generate_declaration(
    decl: &Declaration,
    var_map: &mut HashMap<String, VarLocation>,
    next_offset: &mut i32,
    label_counter: &mut usize,
    scope_decls: Option<&mut HashSet<String>>,
    func_table: &HashMap<String, FunctionInfo>,
    global_var_map: &HashMap<String, VarLocation>,
    static_vars: &mut Vec<AsmStaticVar>,
    static_label_counter: &mut usize,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_label_counter: &mut usize,
    string_constants: &mut Vec<(String, String)>,
    string_label_counter: &mut usize,
) -> Result<Vec<Instruction>> {
    let sc = decl.storage_class;
    let asm_type = type_to_asm(&decl.var_type);

    if sc == Some(StorageClass::Extern) {
        if decl.init.is_some() {
            return Err(CompileError::CodegenError(format!(
                "extern variable '{}' cannot have initializer", decl.name
            )));
        }
        var_map.insert(decl.name.clone(), VarLocation::Static(decl.name.clone(), decl.var_type.clone()));
        return Ok(Vec::new());
    }

    if sc == Some(StorageClass::Static) {
        let init_val = if let Some(init_expr) = &decl.init {
            resolve_static_init(init_expr).map_err(|msg| {
                CompileError::CodegenError(format!(
                    "static variable '{}' {}", decl.name, msg
                ))
            })?
        } else if decl.var_type.is_array() {
            StaticInit::ZeroInit(decl.var_type.size())
        } else {
            if decl.var_type == Type::Double {
                StaticInit::DoubleInit(0.0)
            } else {
                StaticInit::IntInit(0)
            }
        };

        let unique_label = format!("{}.{}", decl.name, *static_label_counter);
        *static_label_counter += 1;

        static_vars.push(AsmStaticVar {
            name: unique_label.clone(),
            global: false,
            init: init_val,
            asm_type,
        });

        var_map.insert(decl.name.clone(), VarLocation::Static(unique_label, decl.var_type.clone()));
        return Ok(Vec::new());
    }

    // 通常のローカル変数
    if let Some(decls) = scope_decls {
        if !decls.insert(decl.name.clone()) {
            return Err(CompileError::CodegenError(format!(
                "variable '{}' already declared in this scope", decl.name
            )));
        }
    }

    // 配列はフルサイズを確保、スカラーは AsmType サイズ（Chapter 15）
    let size = if decl.var_type.is_array() {
        decl.var_type.size() as i32
    } else {
        asm_type_size(asm_type)
    };
    *next_offset -= size;
    // アラインメント（配列は要素サイズに合わせる）
    let align = if decl.var_type.is_array() {
        // 配列のアラインメントは要素型のサイズ
        decl.var_type.target_type().map(|t| t.size() as i32).unwrap_or(4)
    } else {
        asm_type_size(asm_type)
    };
    if *next_offset % align != 0 {
        *next_offset -= align + (*next_offset % align);
    }
    let offset = *next_offset;
    var_map.insert(decl.name.clone(), VarLocation::Stack(offset, decl.var_type.clone()));

    let mut instrs = Vec::new();
    if let Some(init) = &decl.init {
        instrs.extend(generate_expr(
            init, var_map, label_counter, func_table, global_var_map,
            double_constants, const_label_counter,
            string_constants, string_label_counter,
        )?);
        let result_reg = if decl.var_type.is_double() { Reg::XMM0 } else { Reg::AX };
        instrs.push(Instruction::Mov {
            asm_type,
            src: Operand::Register(result_reg),
            dst: Operand::Stack(offset),
        });
    }

    Ok(instrs)
}

/// 条件チェック用のゼロ比較命令列を生成する（integer: cmp $0; double: comisd）
fn emit_condition_check(
    instrs: &mut Vec<Instruction>,
    cond_type: AsmType,
    _double_constants: &mut HashMap<u64, (String, usize)>,
    _const_label_counter: &mut usize,
) {
    if cond_type == AsmType::Double {
        // xorpd で 0.0 を XMM15 に作り、comisd で比較
        instrs.push(Instruction::Binary {
            asm_type: AsmType::Double,
            op: AsmBinaryOp::Xor,
            src: Operand::Register(Reg::XMM15),
            dst: Operand::Register(Reg::XMM15),
        });
        instrs.push(Instruction::Cmp {
            asm_type: AsmType::Double,
            src: Operand::Register(Reg::XMM15),
            dst: Operand::Register(Reg::XMM0),
        });
    } else {
        instrs.push(Instruction::Cmp {
            asm_type: cond_type,
            src: Operand::Imm(0),
            dst: Operand::Register(Reg::AX),
        });
    }
}

fn generate_statement(
    stmt: &Statement,
    var_map: &mut HashMap<String, VarLocation>,
    next_offset: &mut i32,
    label_counter: &mut usize,
    loop_labels: Option<&LoopLabels>,
    func_table: &HashMap<String, FunctionInfo>,
    global_var_map: &HashMap<String, VarLocation>,
    static_vars: &mut Vec<AsmStaticVar>,
    static_label_counter: &mut usize,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_label_counter: &mut usize,
    string_constants: &mut Vec<(String, String)>,
    string_label_counter: &mut usize,
) -> Result<Vec<Instruction>> {
    match stmt {
        Statement::Return(opt_expr) => {
            match opt_expr {
                Some(expr) => {
                    let mut instrs = generate_expr(
                        expr, var_map, label_counter, func_table, global_var_map,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?;
                    instrs.push(Instruction::Ret);
                    Ok(instrs)
                }
                None => {
                    // `return;` — void function, no value to return
                    Ok(vec![Instruction::Ret])
                }
            }
        }
        Statement::Expression(expr) => {
            generate_expr(
                expr, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )
        }
        Statement::Null => Ok(Vec::new()),
        Statement::If { condition, then_branch, else_branch } => {
            let n = *label_counter;
            *label_counter += 1;

            let cond_type = expr_asm_type(condition, var_map, global_var_map, func_table);
            let mut instrs = generate_expr(
                condition, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?;
            emit_condition_check(&mut instrs, cond_type, double_constants, const_label_counter);

            if let Some(else_stmt) = else_branch {
                let else_label = format!(".Lif_else{n}");
                let end_label = format!(".Lif_end{n}");
                instrs.push(Instruction::JmpCC(CondCode::E, else_label.clone()));
                instrs.extend(generate_statement(
                    then_branch, var_map, next_offset, label_counter, loop_labels,
                    func_table, global_var_map, static_vars, static_label_counter,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?);
                instrs.push(Instruction::Jmp(end_label.clone()));
                instrs.push(Instruction::Label(else_label));
                instrs.extend(generate_statement(
                    else_stmt, var_map, next_offset, label_counter, loop_labels,
                    func_table, global_var_map, static_vars, static_label_counter,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?);
                instrs.push(Instruction::Label(end_label));
            } else {
                let end_label = format!(".Lif_end{n}");
                instrs.push(Instruction::JmpCC(CondCode::E, end_label.clone()));
                instrs.extend(generate_statement(
                    then_branch, var_map, next_offset, label_counter, loop_labels,
                    func_table, global_var_map, static_vars, static_label_counter,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?);
                instrs.push(Instruction::Label(end_label));
            }
            Ok(instrs)
        }
        Statement::Compound(items) => {
            let mut inner_map = var_map.clone();
            let mut scope_decls: HashSet<String> = HashSet::new();
            let mut instrs = Vec::new();
            for item in items {
                instrs.extend(generate_block_item(
                    item, &mut inner_map, next_offset, label_counter,
                    Some(&mut scope_decls), loop_labels, func_table, global_var_map,
                    static_vars, static_label_counter, double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?);
            }
            Ok(instrs)
        }
        Statement::While { condition, body } => {
            let n = *label_counter;
            *label_counter += 1;
            let start_label = format!(".Lwhile_start{n}");
            let end_label = format!(".Lwhile_end{n}");
            let labels = LoopLabels {
                break_label: end_label.clone(),
                continue_label: start_label.clone(),
            };

            let cond_type = expr_asm_type(condition, var_map, global_var_map, func_table);
            let mut instrs = vec![Instruction::Label(start_label.clone())];
            instrs.extend(generate_expr(
                condition, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?);
            emit_condition_check(&mut instrs, cond_type, double_constants, const_label_counter);
            instrs.push(Instruction::JmpCC(CondCode::E, end_label.clone()));
            instrs.extend(generate_statement(
                body, var_map, next_offset, label_counter, Some(&labels),
                func_table, global_var_map, static_vars, static_label_counter,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?);
            instrs.push(Instruction::Jmp(start_label));
            instrs.push(Instruction::Label(end_label));
            Ok(instrs)
        }
        Statement::DoWhile { body, condition } => {
            let n = *label_counter;
            *label_counter += 1;
            let start_label = format!(".Ldo_start{n}");
            let continue_label = format!(".Ldo_continue{n}");
            let end_label = format!(".Ldo_end{n}");
            let labels = LoopLabels {
                break_label: end_label.clone(),
                continue_label: continue_label.clone(),
            };

            let cond_type = expr_asm_type(condition, var_map, global_var_map, func_table);
            let mut instrs = vec![Instruction::Label(start_label.clone())];
            instrs.extend(generate_statement(
                body, var_map, next_offset, label_counter, Some(&labels),
                func_table, global_var_map, static_vars, static_label_counter,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?);
            instrs.push(Instruction::Label(continue_label));
            instrs.extend(generate_expr(
                condition, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?);
            emit_condition_check(&mut instrs, cond_type, double_constants, const_label_counter);
            instrs.push(Instruction::JmpCC(CondCode::NE, start_label));
            instrs.push(Instruction::Label(end_label));
            Ok(instrs)
        }
        Statement::For { init, condition, post, body } => {
            let n = *label_counter;
            *label_counter += 1;
            let start_label = format!(".Lfor_start{n}");
            let continue_label = format!(".Lfor_continue{n}");
            let end_label = format!(".Lfor_end{n}");
            let labels = LoopLabels {
                break_label: end_label.clone(),
                continue_label: continue_label.clone(),
            };

            let mut inner_map = var_map.clone();
            let map_ref: &mut HashMap<String, VarLocation> = match init {
                ForInit::Declaration(_) => &mut inner_map,
                ForInit::Expression(_) => var_map,
            };

            let mut instrs = Vec::new();
            match init {
                ForInit::Declaration(decl) => {
                    instrs.extend(generate_declaration(
                        decl, map_ref, next_offset, label_counter, None,
                        func_table, global_var_map, static_vars, static_label_counter,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?);
                }
                ForInit::Expression(Some(expr)) => {
                    instrs.extend(generate_expr(
                        expr, map_ref, label_counter, func_table, global_var_map,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?);
                }
                ForInit::Expression(None) => {}
            }

            instrs.push(Instruction::Label(start_label.clone()));

            if let Some(cond) = condition {
                let cond_type = expr_asm_type(cond, map_ref, global_var_map, func_table);
                instrs.extend(generate_expr(
                    cond, map_ref, label_counter, func_table, global_var_map,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?);
                emit_condition_check(&mut instrs, cond_type, double_constants, const_label_counter);
                instrs.push(Instruction::JmpCC(CondCode::E, end_label.clone()));
            }

            instrs.extend(generate_statement(
                body, map_ref, next_offset, label_counter, Some(&labels),
                func_table, global_var_map, static_vars, static_label_counter,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?);

            instrs.push(Instruction::Label(continue_label));
            if let Some(post_expr) = post {
                instrs.extend(generate_expr(
                    post_expr, map_ref, label_counter, func_table, global_var_map,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?);
            }

            instrs.push(Instruction::Jmp(start_label));
            instrs.push(Instruction::Label(end_label));
            Ok(instrs)
        }
        Statement::Break => {
            let labels = loop_labels.ok_or_else(|| {
                CompileError::CodegenError("break outside loop".to_string())
            })?;
            Ok(vec![Instruction::Jmp(labels.break_label.clone())])
        }
        Statement::Continue => {
            let labels = loop_labels.ok_or_else(|| {
                CompileError::CodegenError("continue outside loop".to_string())
            })?;
            Ok(vec![Instruction::Jmp(labels.continue_label.clone())])
        }
    }
}

/// 式の Type を推定する（コード生成なし）。型検査後のASTでは型が確定している。
fn expr_type(
    expr: &Expr,
    var_map: &HashMap<String, VarLocation>,
    global_var_map: &HashMap<String, VarLocation>,
    func_table: &HashMap<String, FunctionInfo>,
) -> Type {
    match expr {
        Expr::Constant(_) => Type::Int,
        Expr::ConstantLong(_) => Type::Long,
        Expr::ConstantUInt(_) => Type::UInt,
        Expr::ConstantULong(_) => Type::ULong,
        Expr::ConstantDouble(_) => Type::Double,
        Expr::StringLiteral(_) => Type::Pointer(Box::new(Type::Char)),
        Expr::Cast { target_type, .. } => target_type.clone(),
        Expr::Var(name) => {
            if let Ok((_, _, var_type)) = resolve_var(var_map, global_var_map, name) {
                // 配列→ポインタ減衰（Chapter 15）
                match var_type {
                    Type::Array(elem, _) => Type::Pointer(elem),
                    other => other,
                }
            } else {
                Type::Int
            }
        }
        Expr::Assign(lhs, _) | Expr::CompoundAssign(_, lhs, _) => {
            expr_type(lhs, var_map, global_var_map, func_table)
        }
        Expr::PostfixIncrement(inner) | Expr::PostfixDecrement(inner) => {
            expr_type(inner, var_map, global_var_map, func_table)
        }
        Expr::Unary(op, inner) => {
            match op {
                UnaryOp::Not => Type::Int, // ! always returns int
                _ => expr_type(inner, var_map, global_var_map, func_table),
            }
        }
        Expr::Binary(op, left, _right) => {
            match op {
                BinaryOp::LogicalAnd | BinaryOp::LogicalOr
                | BinaryOp::LessThan | BinaryOp::LessEqual
                | BinaryOp::GreaterThan | BinaryOp::GreaterEqual
                | BinaryOp::Equal | BinaryOp::NotEqual => Type::Int,
                BinaryOp::Comma => expr_type(_right, var_map, global_var_map, func_table),
                _ => {
                    // After type-checking, operands are already promoted to common type
                    expr_type(left, var_map, global_var_map, func_table)
                }
            }
        }
        Expr::Conditional { then_expr, .. } => {
            // After type-checking, both branches have same type
            expr_type(then_expr, var_map, global_var_map, func_table)
        }
        Expr::FunctionCall(name, _) => {
            if let Some(info) = func_table.get(name) {
                info.return_type.clone()
            } else {
                Type::Int
            }
        }
        Expr::Dereref(inner) => {
            // Chapter 14: ポインタ間接参照 — inner が Pointer(target) なら target を返す
            let inner_t = expr_type(inner, var_map, global_var_map, func_table);
            match inner_t {
                Type::Pointer(target) => *target,
                _ => Type::Int, // fallback (should not happen after type check)
            }
        }
        Expr::AddrOf(inner) => {
            // Chapter 14: アドレス取得 — Pointer(inner の型) を返す
            let inner_t = expr_type(inner, var_map, global_var_map, func_table);
            Type::Pointer(Box::new(inner_t))
        }
        // sizeof は型チェッカーで ConstantULong に置換済み（到達しないはず）
        Expr::SizeOfType(_) | Expr::SizeOfExpr(_) => Type::ULong,
    }
}

/// 式のAsmTypeを推定する。expr_type() の結果から導出。
fn expr_asm_type(
    expr: &Expr,
    var_map: &HashMap<String, VarLocation>,
    global_var_map: &HashMap<String, VarLocation>,
    func_table: &HashMap<String, FunctionInfo>,
) -> AsmType {
    let t = expr_type(expr, var_map, global_var_map, func_table);
    type_to_asm(&t)
}

/// 左辺値のアドレスを %rax に生成するヘルパー（Chapter 14）。
///
/// - `Var(name)` → `leaq offset(%rbp), %rax` or `leaq label(%rip), %rax`
/// - `Dereref(inner)` → inner の値を評価（inner の値がそのままアドレス）
fn generate_lvalue_addr(
    expr: &Expr,
    var_map: &HashMap<String, VarLocation>,
    label_counter: &mut usize,
    func_table: &HashMap<String, FunctionInfo>,
    global_var_map: &HashMap<String, VarLocation>,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_label_counter: &mut usize,
    string_constants: &mut Vec<(String, String)>,
    string_label_counter: &mut usize,
) -> Result<Vec<Instruction>> {
    match expr {
        Expr::Var(name) => {
            let (operand, _, _) = resolve_var(var_map, global_var_map, name)?;
            Ok(vec![
                Instruction::Lea { src: operand, dst: Operand::Register(Reg::AX) },
            ])
        }
        Expr::Dereref(inner) => {
            // inner の値がアドレスそのもの
            generate_expr(
                inner, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )
        }
        _ => Err(CompileError::CodegenError(
            "cannot take address of non-lvalue expression".to_string()
        )),
    }
}

/// 符号なし除算の命令列を生成するヘルパー。
/// DX を 0 にクリアしてから div を実行。
fn emit_unsigned_div(instrs: &mut Vec<Instruction>, asm_type: AsmType, divisor: Operand) {
    instrs.push(Instruction::Mov {
        asm_type,
        src: Operand::Imm(0),
        dst: Operand::Register(Reg::DX),
    });
    instrs.push(Instruction::Div { asm_type, operand: divisor });
}

/// 符号付き除算の命令列を生成するヘルパー。
fn emit_signed_div(instrs: &mut Vec<Instruction>, asm_type: AsmType, divisor: Operand) {
    instrs.push(Instruction::SignExtend(asm_type));
    instrs.push(Instruction::Idiv { asm_type, operand: divisor });
}

/// double 定数をプールに追加し、ラベルを返す。
fn add_double_constant(
    value: f64,
    alignment: usize,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_label_counter: &mut usize,
) -> String {
    let bits = value.to_bits();
    if let Some((label, existing_align)) = double_constants.get_mut(&bits) {
        // 既存のエントリがあれば、より大きいアラインメントを採用
        if alignment > *existing_align {
            *existing_align = alignment;
        }
        label.clone()
    } else {
        let label = format!(".Lconst_{}", *const_label_counter);
        *const_label_counter += 1;
        double_constants.insert(bits, (label.clone(), alignment));
        label
    }
}

/// 式の変換。整数式は結果が `%eax/%rax` に、double 式は結果が `%xmm0` に入る。
fn generate_expr(
    expr: &Expr,
    var_map: &HashMap<String, VarLocation>,
    label_counter: &mut usize,
    func_table: &HashMap<String, FunctionInfo>,
    global_var_map: &HashMap<String, VarLocation>,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_label_counter: &mut usize,
    string_constants: &mut Vec<(String, String)>,
    string_label_counter: &mut usize,
) -> Result<Vec<Instruction>> {
    match expr {
        Expr::Constant(value) => {
            Ok(vec![Instruction::Mov {
                asm_type: AsmType::Longword,
                src: Operand::Imm(*value),
                dst: Operand::Register(Reg::AX),
            }])
        }

        Expr::ConstantLong(value) => {
            Ok(vec![Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Imm(*value),
                dst: Operand::Register(Reg::AX),
            }])
        }

        Expr::ConstantUInt(value) => {
            Ok(vec![Instruction::Mov {
                asm_type: AsmType::Longword,
                src: Operand::Imm(*value as i64),
                dst: Operand::Register(Reg::AX),
            }])
        }

        Expr::ConstantULong(value) => {
            Ok(vec![Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Imm(*value as i64),
                dst: Operand::Register(Reg::AX),
            }])
        }

        Expr::ConstantDouble(value) => {
            let label = add_double_constant(*value, 8, double_constants, const_label_counter);
            Ok(vec![Instruction::Mov {
                asm_type: AsmType::Double,
                src: Operand::Data(label),
                dst: Operand::Register(Reg::XMM0),
            }])
        }

        Expr::StringLiteral(content) => {
            let label = format!(".Lstr_{}", *string_label_counter);
            *string_label_counter += 1;
            string_constants.push((label.clone(), content.clone()));
            Ok(vec![Instruction::Lea {
                src: Operand::Data(label),
                dst: Operand::Register(Reg::AX),
            }])
        }

        Expr::Cast { target_type, source_type, expr: inner } => {
            let mut instrs = generate_expr(
                inner, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?;

            // Chapter 17: (void)expr — evaluate and discard
            if target_type.is_void() {
                return Ok(instrs);
            }

            // Double ↔ Integer conversions
            if source_type.is_double() && !target_type.is_double() {
                // Double → Integer
                match target_type {
                    Type::Int | Type::Long => {
                        // cvttsd2si → AX
                        let dst_asm = type_to_asm(target_type);
                        instrs.push(Instruction::Cvttsd2si {
                            asm_type: dst_asm,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::AX),
                        });
                    }
                    Type::UInt => {
                        // cvttsd2si (Quadword) → AX, then truncate
                        instrs.push(Instruction::Cvttsd2si {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::AX),
                        });
                        instrs.push(Instruction::Truncate {
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::AX),
                        });
                    }
                    Type::ULong => {
                        // double → unsigned long: 条件分岐アルゴリズム
                        let n = *label_counter;
                        *label_counter += 1;
                        let out_of_range_label = format!(".Ld2ul_oor{n}");
                        let end_label = format!(".Ld2ul_end{n}");
                        // 2^63 を定数プールに追加
                        let bound_label = add_double_constant(
                            9223372036854775808.0, // 2^63 as f64
                            8, double_constants, const_label_counter,
                        );
                        // comisd bound, xmm0 (xmm0 < bound → below)
                        instrs.push(Instruction::Cmp {
                            asm_type: AsmType::Double,
                            src: Operand::Data(bound_label.clone()),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        instrs.push(Instruction::JmpCC(CondCode::AE, out_of_range_label.clone()));
                        // In range: simple cvttsd2si
                        instrs.push(Instruction::Cvttsd2si {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::AX),
                        });
                        instrs.push(Instruction::Jmp(end_label.clone()));
                        // Out of range: subtract 2^63, convert, add 2^63 back
                        instrs.push(Instruction::Label(out_of_range_label));
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Double,
                            op: AsmBinaryOp::Sub,
                            src: Operand::Data(bound_label),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.push(Instruction::Cvttsd2si {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::XMM14),
                            dst: Operand::Register(Reg::AX),
                        });
                        // Add 2^63 (as integer: 0x8000000000000000)
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Quadword,
                            src: Operand::Imm(i64::MIN), // 0x8000000000000000
                            dst: Operand::Register(Reg::CX),
                        });
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Quadword,
                            op: AsmBinaryOp::Add,
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                        instrs.push(Instruction::Label(end_label));
                    }
                    Type::Char | Type::UChar => {
                        // Double → Char/UChar: convert to int first, then truncate
                        instrs.push(Instruction::Cvttsd2si {
                            asm_type: AsmType::Longword,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::AX),
                        });
                        // Truncate handled: value already in AL
                    }
                    Type::Void | Type::Double | Type::Pointer(_) | Type::Array(_, _) => unreachable!(),
                }
            } else if !source_type.is_double() && target_type.is_double() {
                // Integer → Double
                match source_type {
                    Type::Char => {
                        // Sign-extend byte to longword, then cvtsi2sd
                        instrs.push(Instruction::MovsxByte {
                            asm_type: AsmType::Longword,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::AX),
                        });
                        instrs.push(Instruction::Cvtsi2sd {
                            asm_type: AsmType::Longword,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::XMM0),
                        });
                    }
                    Type::UChar => {
                        // Zero-extend byte to longword, then cvtsi2sd
                        instrs.push(Instruction::MovZeroExtendByte {
                            asm_type: AsmType::Longword,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::AX),
                        });
                        instrs.push(Instruction::Cvtsi2sd {
                            asm_type: AsmType::Longword,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::XMM0),
                        });
                    }
                    Type::Int | Type::Long => {
                        let src_asm = type_to_asm(source_type);
                        instrs.push(Instruction::Cvtsi2sd {
                            asm_type: src_asm,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::XMM0),
                        });
                    }
                    Type::UInt => {
                        // Zero-extend to 64-bit, then cvtsi2sd with Quadword
                        instrs.push(Instruction::MovZeroExtend {
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::AX),
                        });
                        instrs.push(Instruction::Cvtsi2sd {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::XMM0),
                        });
                    }
                    Type::ULong => {
                        // unsigned long → double: 条件分岐アルゴリズム
                        let n = *label_counter;
                        *label_counter += 1;
                        let large_label = format!(".Lul2d_large{n}");
                        let end_label = format!(".Lul2d_end{n}");
                        // Test if value >= 2^63 (i.e. sign bit set)
                        instrs.push(Instruction::Cmp {
                            asm_type: AsmType::Quadword,
                            src: Operand::Imm(0),
                            dst: Operand::Register(Reg::AX),
                        });
                        instrs.push(Instruction::JmpCC(CondCode::L, large_label.clone()));
                        // Small: simple cvtsi2sd
                        instrs.push(Instruction::Cvtsi2sd {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        instrs.push(Instruction::Jmp(end_label.clone()));
                        // Large: shift right by 1, OR in low bit, convert, multiply by 2
                        instrs.push(Instruction::Label(large_label));
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::DX),
                        });
                        // shr $1, %rax — done via unsigned div by 2 equivalent: we emit shift via specific instructions
                        // We can use: mov to DX, shr AX by 1, and low bit, or, convert, add
                        // Actually, use Binary to manipulate: we need a right shift.
                        // Since we don't have a shift instruction, we'll use an alternative approach:
                        // DX = AX & 1; AX = AX >> 1; AX = AX | DX; cvtsi2sd; addsd result,result
                        // We need Mov+Binary for this. Let's keep it simpler.
                        // Actually, on x86-64 we can: use DX for the low bit, shift AX right.
                        // But we don't have a shift instruction in our AST.
                        // The book's approach: shr $1, rdi; and $1, rsi; or rsi, rdi; cvtsi2sd; addsd
                        // Let's emit raw: We can represent SHR as "shrq $1, %rax".
                        // For now, let's use an Imm+Div approach: divide by 2 unsigned.
                        // DX = AX & 1 (save low bit)
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::CX),
                        });
                        // CX = AX & 1
                        // We don't have AND instruction, so use: MovZeroExtend(CX&1) — actually
                        // use mov CX, DX; Binary(AsmBinaryOp::Sub) trick.
                        // Simplest: use unsigned div by 2
                        // DX = 0, div $2
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Quadword,
                            src: Operand::Imm(0),
                            dst: Operand::Register(Reg::DX),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Quadword,
                            src: Operand::Imm(2),
                            dst: Operand::Register(Reg::R8),
                        });
                        instrs.push(Instruction::Div {
                            asm_type: AsmType::Quadword,
                            operand: Operand::Register(Reg::R8),
                        });
                        // AX = AX / 2, DX = AX % 2 (low bit)
                        // OR the low bit back: AX | DX
                        // We don't have OR, but we can ADD since at most one of them has the bit
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Quadword,
                            op: AsmBinaryOp::Add,
                            src: Operand::Register(Reg::DX),
                            dst: Operand::Register(Reg::AX),
                        });
                        instrs.push(Instruction::Cvtsi2sd {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        // Multiply by 2: addsd xmm0, xmm0
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Double,
                            op: AsmBinaryOp::Add,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        instrs.push(Instruction::Label(end_label));
                    }
                    Type::Void | Type::Double | Type::Pointer(_) | Type::Array(_, _) => unreachable!(),
                }
            } else if !source_type.is_double() && !target_type.is_double() {
                // Integer ↔ Integer (existing logic + Chapter 16: Byte)
                let src_size = source_type.size();
                let dst_size = target_type.size();
                if src_size == dst_size {
                    // Same size: no-op
                } else if src_size < dst_size {
                    if src_size == 1 {
                        // Byte → Longword/Quadword (Chapter 16)
                        let dst_asm = type_to_asm(target_type);
                        if source_type.is_unsigned() {
                            instrs.push(Instruction::MovZeroExtendByte {
                                asm_type: dst_asm,
                                src: Operand::Register(Reg::AX),
                                dst: Operand::Register(Reg::AX),
                            });
                        } else {
                            instrs.push(Instruction::MovsxByte {
                                asm_type: dst_asm,
                                src: Operand::Register(Reg::AX),
                                dst: Operand::Register(Reg::AX),
                            });
                        }
                    } else if source_type.is_unsigned() {
                        instrs.push(Instruction::MovZeroExtend {
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::AX),
                        });
                    } else {
                        instrs.push(Instruction::Movsx {
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::AX),
                        });
                    }
                } else {
                    // dst_size < src_size: truncate
                    if dst_size == 1 {
                        // Truncate to byte: just use the low byte (movb %al, %al is a no-op)
                        // We still emit a Truncate for clarity; emitter will handle it
                        instrs.push(Instruction::Truncate {
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::AX),
                        });
                    } else {
                        instrs.push(Instruction::Truncate {
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::AX),
                        });
                    }
                }
            }
            // Double → Double: no-op
            Ok(instrs)
        }

        Expr::Var(name) => {
            let (operand, asm_type, var_type) = resolve_var(var_map, global_var_map, name)?;
            if var_type.is_array() {
                // 配列→ポインタ decay: アドレスをロード（Chapter 15）
                Ok(vec![Instruction::Lea {
                    src: operand,
                    dst: Operand::Register(Reg::AX),
                }])
            } else {
                let dst_reg = if asm_type == AsmType::Double { Reg::XMM0 } else { Reg::AX };
                Ok(vec![Instruction::Mov {
                    asm_type,
                    src: operand,
                    dst: Operand::Register(dst_reg),
                }])
            }
        }

        Expr::Assign(lhs, rhs) => {
            let lhs_type = expr_type(lhs, var_map, global_var_map, func_table);
            let asm_type = type_to_asm(&lhs_type);

            if let Expr::Var(name) = lhs.as_ref() {
                // 変数への直接代入（高速パス）
                let (dst_operand, _, _) = resolve_var(var_map, global_var_map, name)?;
                let mut instrs = generate_expr(
                    rhs, var_map, label_counter, func_table, global_var_map,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?;
                let src_reg = if asm_type == AsmType::Double { Reg::XMM0 } else { Reg::AX };
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Register(src_reg),
                    dst: dst_operand,
                });
                Ok(instrs)
            } else {
                // 一般的な左辺値（*ptr など）への代入
                // 1. LHS のアドレスを計算 → push
                let mut instrs = generate_lvalue_addr(
                    lhs, var_map, label_counter, func_table, global_var_map,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?;
                instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                // 2. RHS を評価 → AX/XMM0
                instrs.extend(generate_expr(
                    rhs, var_map, label_counter, func_table, global_var_map,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?);
                // 3. アドレスを復元 → CX
                instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                // 4. *(CX) = AX/XMM0
                let src_reg = if asm_type == AsmType::Double { Reg::XMM0 } else { Reg::AX };
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Register(src_reg),
                    dst: Operand::Memory(Reg::CX),
                });
                Ok(instrs)
            }
        }

        Expr::Unary(op, inner) => {
            match op {
                UnaryOp::PreIncrement | UnaryOp::PreDecrement => {
                    if let Expr::Var(name) = inner.as_ref() {
                        let (operand, asm_type, var_type) = resolve_var(var_map, global_var_map, name)?;
                        let is_double = asm_type == AsmType::Double;
                        let result_reg = if is_double { Reg::XMM0 } else { Reg::AX };
                        // ポインタ型の場合は要素サイズ分増減（Chapter 15）
                        let increment = if var_type.is_pointer() {
                            var_type.target_type().unwrap().size() as i64
                        } else {
                            1
                        };
                        let mut instrs = vec![
                            Instruction::Mov {
                                asm_type,
                                src: operand.clone(),
                                dst: Operand::Register(result_reg),
                            },
                        ];
                        if is_double {
                            // Load 1.0 from constant pool
                            let one_label = add_double_constant(1.0, 8, double_constants, const_label_counter);
                            let asm_op = if matches!(op, UnaryOp::PreIncrement) { AsmBinaryOp::Add } else { AsmBinaryOp::Sub };
                            instrs.push(Instruction::Binary {
                                asm_type: AsmType::Double,
                                op: asm_op,
                                src: Operand::Data(one_label),
                                dst: Operand::Register(Reg::XMM0),
                            });
                        } else {
                            if matches!(op, UnaryOp::PreIncrement) {
                                instrs.push(Instruction::Binary {
                                    asm_type,
                                    op: AsmBinaryOp::Add,
                                    src: Operand::Imm(increment),
                                    dst: Operand::Register(Reg::AX),
                                });
                            } else {
                                instrs.push(Instruction::Binary {
                                    asm_type,
                                    op: AsmBinaryOp::Sub,
                                    src: Operand::Imm(increment),
                                    dst: Operand::Register(Reg::AX),
                                });
                            }
                        }
                        instrs.push(Instruction::Mov {
                            asm_type,
                            src: Operand::Register(result_reg),
                            dst: operand,
                        });
                        Ok(instrs)
                    } else {
                        Err(CompileError::CodegenError(
                            "lvalue required for prefix increment/decrement".to_string()
                        ))
                    }
                }
                _ => {
                    let asm_type = expr_asm_type(expr, var_map, global_var_map, func_table);
                    let inner_type = expr_asm_type(inner, var_map, global_var_map, func_table);
                    let mut instrs = generate_expr(
                        inner, var_map, label_counter, func_table, global_var_map,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?;
                    match op {
                        UnaryOp::Negate => {
                            if asm_type == AsmType::Double {
                                // xorpd with -0.0 to flip sign bit
                                let neg_zero_label = add_double_constant(
                                    f64::from_bits(0x8000000000000000), // -0.0
                                    16, // alignment 16 for xorpd
                                    double_constants, const_label_counter,
                                );
                                instrs.push(Instruction::Binary {
                                    asm_type: AsmType::Double,
                                    op: AsmBinaryOp::Xor,
                                    src: Operand::Data(neg_zero_label),
                                    dst: Operand::Register(Reg::XMM0),
                                });
                            } else {
                                instrs.push(Instruction::Unary {
                                    asm_type,
                                    op: AsmUnaryOp::Neg,
                                    operand: Operand::Register(Reg::AX),
                                });
                            }
                        }
                        UnaryOp::Complement => {
                            // Double complement is rejected by type checker
                            instrs.push(Instruction::Unary {
                                asm_type,
                                op: AsmUnaryOp::Not,
                                operand: Operand::Register(Reg::AX),
                            });
                        }
                        UnaryOp::Not => {
                            if inner_type == AsmType::Double {
                                // Compare with 0.0 using comisd
                                instrs.push(Instruction::Binary {
                                    asm_type: AsmType::Double,
                                    op: AsmBinaryOp::Xor,
                                    src: Operand::Register(Reg::XMM15),
                                    dst: Operand::Register(Reg::XMM15),
                                });
                                instrs.push(Instruction::Cmp {
                                    asm_type: AsmType::Double,
                                    src: Operand::Register(Reg::XMM15),
                                    dst: Operand::Register(Reg::XMM0),
                                });
                            } else {
                                instrs.push(Instruction::Cmp {
                                    asm_type: inner_type,
                                    src: Operand::Imm(0),
                                    dst: Operand::Register(Reg::AX),
                                });
                            }
                            instrs.push(Instruction::Mov {
                                asm_type: AsmType::Longword,
                                src: Operand::Imm(0),
                                dst: Operand::Register(Reg::AX),
                            });
                            instrs.push(Instruction::SetCC {
                                condition: CondCode::E,
                                operand: Operand::Register(Reg::AX),
                            });
                        }
                        UnaryOp::PreIncrement | UnaryOp::PreDecrement => unreachable!(),
                    }
                    Ok(instrs)
                }
            }
        }

        Expr::Conditional { condition, then_expr, else_expr } => {
            let n = *label_counter;
            *label_counter += 1;
            let else_label = format!(".Ltern_else{n}");
            let end_label = format!(".Ltern_end{n}");

            let cond_type = expr_asm_type(condition, var_map, global_var_map, func_table);
            let mut instrs = generate_expr(
                condition, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?;
            emit_condition_check(&mut instrs, cond_type, double_constants, const_label_counter);
            instrs.push(Instruction::JmpCC(CondCode::E, else_label.clone()));
            instrs.extend(generate_expr(
                then_expr, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?);
            instrs.push(Instruction::Jmp(end_label.clone()));
            instrs.push(Instruction::Label(else_label));
            instrs.extend(generate_expr(
                else_expr, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?);
            instrs.push(Instruction::Label(end_label));
            Ok(instrs)
        }

        Expr::CompoundAssign(op, lhs, rhs) => {
            if let Expr::Var(name) = lhs.as_ref() {
                let (var_operand, asm_type, var_type) = resolve_var(var_map, global_var_map, name)?;
                let is_double = asm_type == AsmType::Double;

                match op {
                BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply => {
                    if var_type.is_pointer() && matches!(op, BinaryOp::Add | BinaryOp::Subtract) {
                        // ポインタ複合代入: ptr += int, ptr -= int（Chapter 15）
                        let elem_size = var_type.target_type().unwrap().size() as i64;
                        let mut instrs = generate_expr(
                            rhs, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        // AX = rhs (integer), scale by elem_size
                        if elem_size != 1 {
                            instrs.push(Instruction::Binary {
                                asm_type: AsmType::Quadword,
                                op: AsmBinaryOp::Mult,
                                src: Operand::Imm(elem_size),
                                dst: Operand::Register(Reg::AX),
                            });
                        }
                        // Load pointer value
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Quadword,
                            src: var_operand.clone(),
                            dst: Operand::Register(Reg::CX),
                        });
                        let asm_op = if matches!(op, BinaryOp::Add) { AsmBinaryOp::Add } else { AsmBinaryOp::Sub };
                        if matches!(op, BinaryOp::Subtract) {
                            // CX - AX
                            instrs.push(Instruction::Binary {
                                asm_type: AsmType::Quadword,
                                op: asm_op,
                                src: Operand::Register(Reg::AX),
                                dst: Operand::Register(Reg::CX),
                            });
                            instrs.push(Instruction::Mov {
                                asm_type: AsmType::Quadword,
                                src: Operand::Register(Reg::CX),
                                dst: Operand::Register(Reg::AX),
                            });
                        } else {
                            // AX + CX
                            instrs.push(Instruction::Binary {
                                asm_type: AsmType::Quadword,
                                op: asm_op,
                                src: Operand::Register(Reg::CX),
                                dst: Operand::Register(Reg::AX),
                            });
                        }
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::AX),
                            dst: var_operand,
                        });
                        Ok(instrs)
                    } else if is_double {
                        // Double: load var to XMM14, eval rhs to XMM0, operate, store back
                        let mut instrs = vec![
                            Instruction::Mov {
                                asm_type: AsmType::Double,
                                src: var_operand.clone(),
                                dst: Operand::Register(Reg::XMM14),
                            },
                        ];
                        instrs.extend(generate_expr(
                            rhs, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        // XMM0 = rhs, XMM14 = var
                        let asm_op = match op {
                            BinaryOp::Add => AsmBinaryOp::Add,
                            BinaryOp::Subtract => AsmBinaryOp::Sub,
                            BinaryOp::Multiply => AsmBinaryOp::Mult,
                            _ => unreachable!(),
                        };
                        if matches!(op, BinaryOp::Subtract) {
                            // var - rhs: XMM14 - XMM0 → sub XMM0, XMM14; mov XMM14 → XMM0
                            instrs.push(Instruction::Binary {
                                asm_type: AsmType::Double,
                                op: asm_op,
                                src: Operand::Register(Reg::XMM0),
                                dst: Operand::Register(Reg::XMM14),
                            });
                            instrs.push(Instruction::Mov {
                                asm_type: AsmType::Double,
                                src: Operand::Register(Reg::XMM14),
                                dst: Operand::Register(Reg::XMM0),
                            });
                        } else {
                            // add/mul: commutative, XMM14 + XMM0 → add XMM14, XMM0
                            instrs.push(Instruction::Binary {
                                asm_type: AsmType::Double,
                                op: asm_op,
                                src: Operand::Register(Reg::XMM14),
                                dst: Operand::Register(Reg::XMM0),
                            });
                        }
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM0),
                            dst: var_operand,
                        });
                        Ok(instrs)
                    } else {
                        let mut instrs = vec![
                            Instruction::Mov {
                                asm_type,
                                src: var_operand.clone(),
                                dst: Operand::Register(Reg::AX),
                            },
                            Instruction::Push(Operand::Register(Reg::AX)),
                        ];
                        instrs.extend(generate_expr(
                            rhs, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                        let asm_op = match op {
                            BinaryOp::Add => AsmBinaryOp::Add,
                            BinaryOp::Subtract => AsmBinaryOp::Sub,
                            BinaryOp::Multiply => AsmBinaryOp::Mult,
                            _ => unreachable!(),
                        };
                        if matches!(op, BinaryOp::Subtract) {
                            instrs.push(Instruction::Binary {
                                asm_type,
                                op: asm_op,
                                src: Operand::Register(Reg::AX),
                                dst: Operand::Register(Reg::CX),
                            });
                            instrs.push(Instruction::Mov {
                                asm_type,
                                src: Operand::Register(Reg::CX),
                                dst: Operand::Register(Reg::AX),
                            });
                        } else {
                            instrs.push(Instruction::Binary {
                                asm_type,
                                op: asm_op,
                                src: Operand::Register(Reg::CX),
                                dst: Operand::Register(Reg::AX),
                            });
                        }
                        instrs.push(Instruction::Mov {
                            asm_type,
                            src: Operand::Register(Reg::AX),
                            dst: var_operand,
                        });
                        Ok(instrs)
                    }
                }
                BinaryOp::Divide | BinaryOp::Remainder => {
                    if is_double {
                        // Double /= : load var to XMM14, eval rhs to XMM0, divsd XMM0,XMM14, result in XMM14
                        // Remainder is rejected by type checker for double
                        let mut instrs = vec![
                            Instruction::Mov {
                                asm_type: AsmType::Double,
                                src: var_operand.clone(),
                                dst: Operand::Register(Reg::XMM14),
                            },
                        ];
                        instrs.extend(generate_expr(
                            rhs, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        // XMM14 / XMM0
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Double,
                            op: AsmBinaryOp::DivDouble,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM14),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM0),
                            dst: var_operand,
                        });
                        Ok(instrs)
                    } else {
                        let mut instrs = generate_expr(
                            rhs, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Mov {
                            asm_type,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::CX),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type,
                            src: var_operand.clone(),
                            dst: Operand::Register(Reg::AX),
                        });
                        if var_type.is_unsigned() {
                            emit_unsigned_div(&mut instrs, asm_type, Operand::Register(Reg::CX));
                        } else {
                            emit_signed_div(&mut instrs, asm_type, Operand::Register(Reg::CX));
                        }
                        if matches!(op, BinaryOp::Remainder) {
                            instrs.push(Instruction::Mov {
                                asm_type,
                                src: Operand::Register(Reg::DX),
                                dst: Operand::Register(Reg::AX),
                            });
                        }
                        instrs.push(Instruction::Mov {
                            asm_type,
                            src: Operand::Register(Reg::AX),
                            dst: var_operand,
                        });
                        Ok(instrs)
                    }
                }
                    _ => Err(CompileError::CodegenError(format!(
                        "unsupported compound assignment operator: {:?}", op
                    ))),
                }
            } else {
                Err(CompileError::CodegenError("unsupported lvalue in compound assignment".to_string()))
            }
        }

        Expr::PostfixIncrement(inner) => {
            if let Expr::Var(name) = inner.as_ref() {
                let (operand, asm_type, var_type) = resolve_var(var_map, global_var_map, name)?;
                // ポインタ型の場合は要素サイズ分増減（Chapter 15）
                let increment = if var_type.is_pointer() {
                    var_type.target_type().unwrap().size() as i64
                } else {
                    1
                };
                if asm_type == AsmType::Double {
                    let one_label = add_double_constant(1.0, 8, double_constants, const_label_counter);
                    Ok(vec![
                        // Load current value to XMM0 (return value)
                        Instruction::Mov { asm_type: AsmType::Double, src: operand.clone(), dst: Operand::Register(Reg::XMM0) },
                        // Copy to XMM14 for incrementing
                        Instruction::Mov { asm_type: AsmType::Double, src: Operand::Register(Reg::XMM0), dst: Operand::Register(Reg::XMM14) },
                        Instruction::Binary { asm_type: AsmType::Double, op: AsmBinaryOp::Add, src: Operand::Data(one_label), dst: Operand::Register(Reg::XMM14) },
                        Instruction::Mov { asm_type: AsmType::Double, src: Operand::Register(Reg::XMM14), dst: operand },
                    ])
                } else {
                    Ok(vec![
                        Instruction::Mov { asm_type, src: operand.clone(), dst: Operand::Register(Reg::AX) },
                        Instruction::Mov { asm_type, src: Operand::Register(Reg::AX), dst: Operand::Register(Reg::CX) },
                        Instruction::Binary { asm_type, op: AsmBinaryOp::Add, src: Operand::Imm(increment), dst: Operand::Register(Reg::CX) },
                        Instruction::Mov { asm_type, src: Operand::Register(Reg::CX), dst: operand },
                    ])
                }
            } else {
                Err(CompileError::CodegenError("unsupported lvalue in postfix increment".to_string()))
            }
        }

        Expr::PostfixDecrement(inner) => {
            if let Expr::Var(name) = inner.as_ref() {
                let (operand, asm_type, var_type) = resolve_var(var_map, global_var_map, name)?;
                let increment = if var_type.is_pointer() {
                    var_type.target_type().unwrap().size() as i64
                } else {
                    1
                };
                if asm_type == AsmType::Double {
                    let one_label = add_double_constant(1.0, 8, double_constants, const_label_counter);
                    Ok(vec![
                        Instruction::Mov { asm_type: AsmType::Double, src: operand.clone(), dst: Operand::Register(Reg::XMM0) },
                        Instruction::Mov { asm_type: AsmType::Double, src: Operand::Register(Reg::XMM0), dst: Operand::Register(Reg::XMM14) },
                        Instruction::Binary { asm_type: AsmType::Double, op: AsmBinaryOp::Sub, src: Operand::Data(one_label), dst: Operand::Register(Reg::XMM14) },
                        Instruction::Mov { asm_type: AsmType::Double, src: Operand::Register(Reg::XMM14), dst: operand },
                    ])
                } else {
                    Ok(vec![
                        Instruction::Mov { asm_type, src: operand.clone(), dst: Operand::Register(Reg::AX) },
                        Instruction::Mov { asm_type, src: Operand::Register(Reg::AX), dst: Operand::Register(Reg::CX) },
                        Instruction::Binary { asm_type, op: AsmBinaryOp::Sub, src: Operand::Imm(increment), dst: Operand::Register(Reg::CX) },
                        Instruction::Mov { asm_type, src: Operand::Register(Reg::CX), dst: operand },
                    ])
                }
            } else {
                Err(CompileError::CodegenError("unsupported lvalue in postfix decrement".to_string()))
            }
        }

        Expr::FunctionCall(name, args) => {
            if let Some(info) = func_table.get(name) {
                if info.param_count != args.len() {
                    return Err(CompileError::CodegenError(format!(
                        "function '{}' expects {} arguments, got {}",
                        name, info.param_count, args.len()
                    )));
                }
            }

            // Classify arguments by type
            let param_types: Vec<Type> = if let Some(info) = func_table.get(name) {
                info.param_types.clone()
            } else {
                // Unknown function: assume all int
                args.iter().map(|_| Type::Int).collect()
            };

            // Build (type, paramname) tuples for classify_parameters
            let typed_params: Vec<(Type, String)> = param_types.iter()
                .map(|t| (t.clone(), String::new()))
                .collect();
            let call_class = classify_parameters(&typed_params);

            // Count stack args and compute padding for 16-byte alignment
            let stack_count = call_class.stack_count;
            let padding = if stack_count % 2 != 0 { 8 } else { 0 };

            let mut instrs = Vec::new();
            if padding > 0 {
                instrs.push(Instruction::AllocateStack(padding));
            }

            // Collect args by location type
            let mut stack_arg_indices: Vec<usize> = Vec::new();
            let mut int_reg_args: Vec<(usize, usize)> = Vec::new(); // (arg_idx, reg_idx)
            let mut xmm_reg_args: Vec<(usize, usize)> = Vec::new();
            for (i, (_, loc)) in call_class.locations.iter().enumerate() {
                match loc {
                    ParamLocation::Stack(_) => stack_arg_indices.push(i),
                    ParamLocation::IntReg(idx) => int_reg_args.push((i, *idx)),
                    ParamLocation::XmmReg(idx) => xmm_reg_args.push((i, *idx)),
                }
            }

            // Push stack args in reverse order
            // For double: move XMM0 bits to AX via Cvttsd2si workaround won't work.
            // Instead, use Push(AX) trick: movsd to stack temp, movq to AX, push AX.
            // Actually the simplest: use Cvttsd2si? No, that changes the value.
            // Correct approach: movq %xmm0, %rax (raw bit move). We don't have this instruction.
            // Workaround: AllocateStack(8) acts as "push" by decrementing rsp.
            // Then we need movsd %xmm0, (%rsp). But we don't have rsp-relative addressing.
            // Best workaround: evaluate double arg, store to known rbp-offset temp, load to AX, push.
            // OR: just push 8 bytes of zeros then write over with movsd (%rsp) - but we need rsp.
            // Simplest correct approach: for each double stack arg, push a dummy 8 bytes (push $0),
            // then the function body code won't be correct without rsp-relative writes.
            //
            // Let's use a different strategy: evaluate ALL args first, save to temp stack slots
            // (rbp-relative), then set up the call frame from the saved values.
            //
            // For simplicity with few stack args: evaluate and push. For double, store to
            // temp slot on rbp-stack, load to AX (as raw bits via Longword pair), push.
            // This gets complex. Let's just use the fact that we can save/restore via XMM14/15.
            //
            // Simplest approach for now (most function calls will have <6 int + <8 double args):
            // Evaluate all args in order, saving int results via push and double results via
            // saving to XMM scratch registers or re-evaluating.

            // Strategy: Evaluate all args sequentially. Each result goes to AX or XMM0.
            // For register args: push int results to stack, collect XMM results differently.
            // For stack args: push immediately (for doubles, we push AX placeholder).

            // Push stack args in reverse order
            for &i in stack_arg_indices.iter().rev() {
                instrs.extend(generate_expr(
                    &args[i], var_map, label_counter, func_table, global_var_map,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?);
                if param_types[i].is_double() {
                    // For double stack args: save XMM0 to a temp slot, load to AX, push AX
                    // We use Mov(Double, XMM0, Stack(temp)) + Mov(Quad, Stack(temp), AX) + Push(AX)
                    // But we need a temp slot. Use DeallocateStack trick:
                    // push $0 to make room, then movsd %xmm0, (%rsp)
                    // Since we can't address rsp, use Push then overwrite via the callee.
                    // Actually... the callee reads this from 16(%rbp)+offset anyway.
                    // Let's just use: subq $8, %rsp (AllocateStack) - this makes room.
                    // The callee expects the arg at the right stack position.
                    // We CANNOT use movsd to (%rsp) with our AST.
                    //
                    // Correct and simple: Push a GPR that holds the double bits.
                    // To get double bits into GPR: use movq instruction (SSE2).
                    // We don't have movq (XMM->GPR) in our AST. But we can round-trip through memory:
                    // movsd %xmm0, -N(%rbp)   [to a temp slot]
                    // movq -N(%rbp), %rax      [load raw bits]
                    // pushq %rax
                    //
                    // We need to use an existing temp slot. Since next_offset is not accessible here,
                    // we'll use the XMM14 register as a temporary and use a stack-save approach.
                    //
                    // Actually, the simplest approach: we CAN use movsd %xmm0, -TEMP(%rbp)
                    // and movq -TEMP(%rbp), %rax, push %rax. We'll use a known temp slot
                    // at the bottom of the stack frame (below all variables).
                    // But we don't know next_offset here.
                    //
                    // OK, the most practical approach that works with our AST:
                    // 1. Push a dummy value to allocate space: Push(Imm(0))
                    // Wait - we don't have Push(Imm). We have Push(Operand).
                    // Let's check - Push takes an Operand. Imm is an Operand variant.
                    // pushq $0 then overwrite... but we still can't address (%rsp).
                    //
                    // Final approach: use the trick of Mov(Quadword, XMM0, AX) via two movl:
                    // No, that doesn't work for XMM.
                    //
                    // Let me just Push(Imm(0)) and note that this is a placeholder.
                    // For correct behavior, we need to add movq (XMM→GPR) or use (%rsp).
                    // For now, use AllocateStack + re-eval approach, which won't be used often.
                    //
                    // PRACTICAL SOLUTION: Most real programs won't overflow 8 XMM args.
                    // For the rare case of double stack args, we'll add a new instruction type
                    // or handle it specially. For now, let's just push 0 as a TODO.
                    // Actually, simplest correct fix: we can extend Push to handle XMM by
                    // emitting subq $8, %rsp; movsd %xmm0, (%rsp) in the emitter.
                    // Let's do that - Push(XMM0) will be handled specially in the emitter.
                    instrs.push(Instruction::Push(Operand::Register(Reg::XMM0)));
                } else {
                    instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                }
            }

            // Evaluate int register args, push results to save them
            for &(arg_idx, _) in int_reg_args.iter().rev() {
                instrs.extend(generate_expr(
                    &args[arg_idx], var_map, label_counter, func_table, global_var_map,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?);
                instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
            }
            // Pop into int arg registers in forward order
            for &(_, reg_idx) in &int_reg_args {
                instrs.push(Instruction::Pop(Operand::Register(ARG_REGISTERS[reg_idx])));
            }

            // Evaluate XMM register args
            // If only 1 XMM arg, just evaluate directly to XMM target reg
            // If multiple, evaluate in reverse, push (as above), pop to XMM regs
            if xmm_reg_args.len() == 1 {
                let (arg_idx, reg_idx) = xmm_reg_args[0];
                instrs.extend(generate_expr(
                    &args[arg_idx], var_map, label_counter, func_table, global_var_map,
                    double_constants, const_label_counter,
                    string_constants, string_label_counter,
                )?);
                if reg_idx != 0 {
                    // Move from XMM0 to target XMM reg
                    instrs.push(Instruction::Mov {
                        asm_type: AsmType::Double,
                        src: Operand::Register(Reg::XMM0),
                        dst: Operand::Register(XMM_ARG_REGISTERS[reg_idx]),
                    });
                }
            } else if xmm_reg_args.len() > 1 {
                // Evaluate in reverse, push results
                for &(arg_idx, _) in xmm_reg_args.iter().rev() {
                    instrs.extend(generate_expr(
                        &args[arg_idx], var_map, label_counter, func_table, global_var_map,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?);
                    instrs.push(Instruction::Push(Operand::Register(Reg::XMM0)));
                }
                // Pop to target XMM regs in forward order
                for &(_, reg_idx) in &xmm_reg_args {
                    instrs.push(Instruction::Pop(Operand::Register(XMM_ARG_REGISTERS[reg_idx])));
                }
            }

            instrs.push(Instruction::Call(name.clone()));

            let dealloc = stack_count * 8 + padding;
            if dealloc > 0 {
                instrs.push(Instruction::DeallocateStack(dealloc));
            }

            Ok(instrs)
        }

        Expr::Binary(op, left, right) => {
            let result_type = expr_type(expr, var_map, global_var_map, func_table);
            let asm_type = type_to_asm(&result_type);
            let operand_ctype = expr_type(left, var_map, global_var_map, func_table);
            let operand_asm_type = type_to_asm(&operand_ctype);
            let is_double_operand = operand_asm_type == AsmType::Double;

            match op {
                BinaryOp::LogicalAnd => {
                    let n = *label_counter;
                    *label_counter += 1;
                    let false_label = format!(".Land_false{n}");
                    let end_label = format!(".Land_end{n}");

                    let left_type = expr_asm_type(left, var_map, global_var_map, func_table);
                    let mut instrs = generate_expr(
                        left, var_map, label_counter, func_table, global_var_map,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?;
                    emit_condition_check(&mut instrs, left_type, double_constants, const_label_counter);
                    instrs.push(Instruction::JmpCC(CondCode::E, false_label.clone()));

                    let right_type = expr_asm_type(right, var_map, global_var_map, func_table);
                    instrs.extend(generate_expr(
                        right, var_map, label_counter, func_table, global_var_map,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?);
                    emit_condition_check(&mut instrs, right_type, double_constants, const_label_counter);
                    instrs.push(Instruction::JmpCC(CondCode::E, false_label.clone()));

                    instrs.push(Instruction::Mov { asm_type: AsmType::Longword, src: Operand::Imm(1), dst: Operand::Register(Reg::AX) });
                    instrs.push(Instruction::Jmp(end_label.clone()));
                    instrs.push(Instruction::Label(false_label));
                    instrs.push(Instruction::Mov { asm_type: AsmType::Longword, src: Operand::Imm(0), dst: Operand::Register(Reg::AX) });
                    instrs.push(Instruction::Label(end_label));
                    Ok(instrs)
                }
                BinaryOp::LogicalOr => {
                    let n = *label_counter;
                    *label_counter += 1;
                    let true_label = format!(".Lor_true{n}");
                    let end_label = format!(".Lor_end{n}");

                    let left_type = expr_asm_type(left, var_map, global_var_map, func_table);
                    let mut instrs = generate_expr(
                        left, var_map, label_counter, func_table, global_var_map,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?;
                    emit_condition_check(&mut instrs, left_type, double_constants, const_label_counter);
                    instrs.push(Instruction::JmpCC(CondCode::NE, true_label.clone()));

                    let right_type = expr_asm_type(right, var_map, global_var_map, func_table);
                    instrs.extend(generate_expr(
                        right, var_map, label_counter, func_table, global_var_map,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?);
                    emit_condition_check(&mut instrs, right_type, double_constants, const_label_counter);
                    instrs.push(Instruction::JmpCC(CondCode::NE, true_label.clone()));

                    instrs.push(Instruction::Mov { asm_type: AsmType::Longword, src: Operand::Imm(0), dst: Operand::Register(Reg::AX) });
                    instrs.push(Instruction::Jmp(end_label.clone()));
                    instrs.push(Instruction::Label(true_label));
                    instrs.push(Instruction::Mov { asm_type: AsmType::Longword, src: Operand::Imm(1), dst: Operand::Register(Reg::AX) });
                    instrs.push(Instruction::Label(end_label));
                    Ok(instrs)
                }

                BinaryOp::LessThan | BinaryOp::LessEqual
                | BinaryOp::GreaterThan | BinaryOp::GreaterEqual
                | BinaryOp::Equal | BinaryOp::NotEqual => {
                    let is_unsigned = operand_ctype.is_unsigned();
                    if is_double_operand {
                        // Double comparison: eval left to XMM0, save to XMM14, eval right to XMM0
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        // comisd XMM0, XMM14 (compare left (XMM14) with right (XMM0))
                        instrs.push(Instruction::Cmp {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Longword,
                            src: Operand::Imm(0),
                            dst: Operand::Register(Reg::AX),
                        });
                        // comisd sets unsigned flags (CF, ZF)
                        let cc = match op {
                            BinaryOp::LessThan => CondCode::B,
                            BinaryOp::LessEqual => CondCode::BE,
                            BinaryOp::GreaterThan => CondCode::A,
                            BinaryOp::GreaterEqual => CondCode::AE,
                            BinaryOp::Equal => CondCode::E,
                            BinaryOp::NotEqual => CondCode::NE,
                            _ => unreachable!(),
                        };
                        instrs.push(Instruction::SetCC {
                            condition: cc,
                            operand: Operand::Register(Reg::AX),
                        });
                        Ok(instrs)
                    } else {
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                        instrs.push(Instruction::Cmp {
                            asm_type: operand_asm_type,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::CX),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Longword,
                            src: Operand::Imm(0),
                            dst: Operand::Register(Reg::AX),
                        });
                        let cc = if is_unsigned {
                            match op {
                                BinaryOp::LessThan => CondCode::B,
                                BinaryOp::LessEqual => CondCode::BE,
                                BinaryOp::GreaterThan => CondCode::A,
                                BinaryOp::GreaterEqual => CondCode::AE,
                                BinaryOp::Equal => CondCode::E,
                                BinaryOp::NotEqual => CondCode::NE,
                                _ => unreachable!(),
                            }
                        } else {
                            match op {
                                BinaryOp::LessThan => CondCode::L,
                                BinaryOp::LessEqual => CondCode::LE,
                                BinaryOp::GreaterThan => CondCode::G,
                                BinaryOp::GreaterEqual => CondCode::GE,
                                BinaryOp::Equal => CondCode::E,
                                BinaryOp::NotEqual => CondCode::NE,
                                _ => unreachable!(),
                            }
                        };
                        instrs.push(Instruction::SetCC {
                            condition: cc,
                            operand: Operand::Register(Reg::AX),
                        });
                        Ok(instrs)
                    }
                }

                BinaryOp::Add => {
                    let left_type = expr_type(left, var_map, global_var_map, func_table);

                    if left_type.is_pointer() {
                        // ポインタ算術: ptr + int（Chapter 15）
                        // 型チェッカーが int + ptr → ptr + int にスワップ済み
                        let elem_size = left_type.target_type().unwrap().size() as i64;
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        // AX = right (integer), scale by elem_size
                        if elem_size != 1 {
                            instrs.push(Instruction::Binary {
                                asm_type: AsmType::Quadword,
                                op: AsmBinaryOp::Mult,
                                src: Operand::Imm(elem_size),
                                dst: Operand::Register(Reg::AX),
                            });
                        }
                        instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                        // CX = ptr, AX = scaled offset
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Quadword,
                            op: AsmBinaryOp::Add,
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                        Ok(instrs)
                    } else if is_double_operand {
                        // Double 加算
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Double,
                            op: AsmBinaryOp::Add,
                            src: Operand::Register(Reg::XMM14),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        Ok(instrs)
                    } else {
                        // 通常の整数加算
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                        instrs.push(Instruction::Binary {
                            asm_type,
                            op: AsmBinaryOp::Add,
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                        Ok(instrs)
                    }
                }

                BinaryOp::Subtract => {
                    let left_type = expr_type(left, var_map, global_var_map, func_table);
                    let right_type = expr_type(right, var_map, global_var_map, func_table);

                    if left_type.is_pointer() && right_type.is_pointer() {
                        // ptr - ptr: 要素数差分を返す（Chapter 15）
                        let elem_size = left_type.target_type().unwrap().size() as i64;
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                        // CX = left ptr, AX = right ptr → CX - AX
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Quadword,
                            op: AsmBinaryOp::Sub,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::CX),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                        // AX = byte difference → divide by elem_size
                        if elem_size != 1 {
                            // 符号付き除算で要素数差分を取得
                            instrs.push(Instruction::Mov {
                                asm_type: AsmType::Quadword,
                                src: Operand::Imm(elem_size),
                                dst: Operand::Register(Reg::CX),
                            });
                            instrs.push(Instruction::SignExtend(AsmType::Quadword));
                            instrs.push(Instruction::Idiv {
                                asm_type: AsmType::Quadword,
                                operand: Operand::Register(Reg::CX),
                            });
                        }
                        Ok(instrs)
                    } else if left_type.is_pointer() && !right_type.is_pointer() {
                        // ptr - int: スケーリング付き減算（Chapter 15）
                        let elem_size = left_type.target_type().unwrap().size() as i64;
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        // AX = right (integer), scale by elem_size
                        if elem_size != 1 {
                            instrs.push(Instruction::Binary {
                                asm_type: AsmType::Quadword,
                                op: AsmBinaryOp::Mult,
                                src: Operand::Imm(elem_size),
                                dst: Operand::Register(Reg::AX),
                            });
                        }
                        instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                        // CX = ptr, AX = scaled offset → CX - AX
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Quadword,
                            op: AsmBinaryOp::Sub,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::CX),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Quadword,
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                        Ok(instrs)
                    } else if is_double_operand {
                        // Double 減算
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Double,
                            op: AsmBinaryOp::Sub,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM14),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        Ok(instrs)
                    } else {
                        // 通常の整数減算
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                        instrs.push(Instruction::Binary {
                            asm_type,
                            op: AsmBinaryOp::Sub,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::CX),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type,
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                        Ok(instrs)
                    }
                }

                BinaryOp::Multiply => {
                    if is_double_operand {
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Double,
                            op: AsmBinaryOp::Mult,
                            src: Operand::Register(Reg::XMM14),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        Ok(instrs)
                    } else {
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                        instrs.push(Instruction::Binary {
                            asm_type,
                            op: AsmBinaryOp::Mult,
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                        Ok(instrs)
                    }
                }

                BinaryOp::Comma => {
                    let mut instrs = generate_expr(
                        left, var_map, label_counter, func_table, global_var_map,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?;
                    instrs.extend(generate_expr(
                        right, var_map, label_counter, func_table, global_var_map,
                        double_constants, const_label_counter,
                        string_constants, string_label_counter,
                    )?);
                    Ok(instrs)
                }

                BinaryOp::Divide | BinaryOp::Remainder => {
                    if is_double_operand {
                        // Double division: left→XMM0→XMM14, right→XMM0, divsd XMM0,XMM14
                        // Remainder is rejected by type checker for double
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        // XMM14 / XMM0
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Double,
                            op: AsmBinaryOp::DivDouble,
                            src: Operand::Register(Reg::XMM0),
                            dst: Operand::Register(Reg::XMM14),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: Operand::Register(Reg::XMM14),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        Ok(instrs)
                    } else {
                        let mut instrs = generate_expr(
                            left, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?;
                        instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                        instrs.extend(generate_expr(
                            right, var_map, label_counter, func_table, global_var_map,
                            double_constants, const_label_counter,
                            string_constants, string_label_counter,
                        )?);
                        instrs.push(Instruction::Mov {
                            asm_type,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::CX),
                        });
                        instrs.push(Instruction::Pop(Operand::Register(Reg::AX)));
                        if result_type.is_unsigned() {
                            emit_unsigned_div(&mut instrs, asm_type, Operand::Register(Reg::CX));
                        } else {
                            emit_signed_div(&mut instrs, asm_type, Operand::Register(Reg::CX));
                        }
                        if matches!(op, BinaryOp::Remainder) {
                            instrs.push(Instruction::Mov {
                                asm_type,
                                src: Operand::Register(Reg::DX),
                                dst: Operand::Register(Reg::AX),
                            });
                        }
                        Ok(instrs)
                    }
                }
            }
        }

        // sizeof は型チェッカーで ConstantULong に解決済み（到達しない）
        Expr::SizeOfType(_) | Expr::SizeOfExpr(_) => {
            unreachable!("sizeof should be resolved by type checker")
        }

        // ── Chapter 14: ポインタ ──
        Expr::AddrOf(inner) => {
            // &expr → inner のアドレスを %rax に
            generate_lvalue_addr(
                inner, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )
        }

        Expr::Dereref(inner) => {
            // *expr → inner を評価してアドレスを取得し、そのアドレスの値をロード
            let target_type = expr_type(expr, var_map, global_var_map, func_table);
            let asm_type = type_to_asm(&target_type);
            let mut instrs = generate_expr(
                inner, var_map, label_counter, func_table, global_var_map,
                double_constants, const_label_counter,
                string_constants, string_label_counter,
            )?;
            // %rax にアドレスが入っている → (%rax) から値をロード
            if asm_type == AsmType::Double {
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Memory(Reg::AX),
                    dst: Operand::Register(Reg::XMM0),
                });
            } else {
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Memory(Reg::AX),
                    dst: Operand::Register(Reg::AX),
                });
            }
            Ok(instrs)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::ast::{FunctionDecl, StorageClass, TopLevelDecl};

    fn func_decl(name: &str, params: Vec<&str>, body: Option<Vec<BlockItem>>) -> TopLevelDecl {
        TopLevelDecl::Function(FunctionDecl {
            name: name.to_string(),
            return_type: Type::Int,
            params: params.into_iter().map(|s| (Type::Int, String::from(s))).collect(),
            body,
            storage_class: None,
        })
    }

    fn func_decl_with_sc(
        name: &str,
        params: Vec<&str>,
        body: Option<Vec<BlockItem>>,
        sc: Option<StorageClass>,
    ) -> TopLevelDecl {
        TopLevelDecl::Function(FunctionDecl {
            name: name.to_string(),
            return_type: Type::Int,
            params: params.into_iter().map(|s| (Type::Int, String::from(s))).collect(),
            body,
            storage_class: sc,
        })
    }

    fn var_decl(name: &str, init: Option<Expr>) -> Declaration {
        Declaration {
            name: name.to_string(),
            var_type: Type::Int,
            init,
            storage_class: None,
        }
    }

    fn var_decl_with_sc(name: &str, init: Option<Expr>, sc: Option<StorageClass>) -> Declaration {
        Declaration {
            name: name.to_string(),
            var_type: Type::Int,
            init,
            storage_class: sc,
        }
    }

    /// Helper: use AsmType::Longword for int operations
    const LW: AsmType = AsmType::Longword;

    #[test]
    fn generate_return_constant() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Some(Expr::Constant(2)))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.functions[0].name, "main");
        assert_eq!(asm.functions[0].global, true);
        assert!(asm.functions[0].instructions.contains(
            &Instruction::Mov { asm_type: LW, src: Operand::Imm(2), dst: Operand::Register(Reg::AX) }
        ));
    }

    #[test]
    fn generate_negation() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Some(Expr::Unary(UnaryOp::Negate, Box::new(Expr::Constant(5)))))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert!(asm.functions[0].instructions.contains(
            &Instruction::Unary { asm_type: LW, op: AsmUnaryOp::Neg, operand: Operand::Register(Reg::AX) }
        ));
    }

    #[test]
    fn generate_complement() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Some(Expr::Unary(UnaryOp::Complement, Box::new(Expr::Constant(0)))))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert!(asm.functions[0].instructions.contains(
            &Instruction::Unary { asm_type: LW, op: AsmUnaryOp::Not, operand: Operand::Register(Reg::AX) }
        ));
    }

    #[test]
    fn generate_logical_not() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Some(Expr::Unary(UnaryOp::Not, Box::new(Expr::Constant(1)))))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert!(asm.functions[0].instructions.contains(
            &Instruction::SetCC { condition: CondCode::E, operand: Operand::Register(Reg::AX) }
        ));
    }

    #[test]
    fn generate_addition() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                    BinaryOp::Add, Box::new(Expr::Constant(1)), Box::new(Expr::Constant(2)),
                )))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert!(asm.functions[0].instructions.contains(
            &Instruction::Binary { asm_type: LW, op: AsmBinaryOp::Add, src: Operand::Register(Reg::CX), dst: Operand::Register(Reg::AX) }
        ));
    }

    #[test]
    fn generate_division() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                    BinaryOp::Divide, Box::new(Expr::Constant(7)), Box::new(Expr::Constant(2)),
                )))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert!(asm.functions[0].instructions.contains(
            &Instruction::Idiv { asm_type: LW, operand: Operand::Register(Reg::CX) }
        ));
    }

    #[test]
    fn generate_less_than() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                    BinaryOp::LessThan, Box::new(Expr::Constant(1)), Box::new(Expr::Constant(2)),
                )))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert!(asm.functions[0].instructions.contains(
            &Instruction::SetCC { condition: CondCode::L, operand: Operand::Register(Reg::AX) }
        ));
    }

    #[test]
    fn generate_var_declaration_and_return() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(5)))),
                BlockItem::Statement(Statement::Return(Some(Expr::Var("a".to_string())))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert!(!asm.functions[0].instructions.is_empty());
    }

    #[test]
    fn generate_duplicate_declaration_error() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(1)))),
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(2)))),
            ]))],
        };
        assert!(generate(&program).is_err());
    }

    #[test]
    fn generate_undeclared_variable_error() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Some(Expr::Var("x".to_string())))),
            ]))],
        };
        assert!(generate(&program).is_err());
    }

    #[test]
    fn generate_break_outside_loop_error() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Break),
            ]))],
        };
        assert!(generate(&program).is_err());
    }

    #[test]
    fn generate_continue_outside_loop_error() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Continue),
            ]))],
        };
        assert!(generate(&program).is_err());
    }

    #[test]
    fn generate_while_loop() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(0)))),
                BlockItem::Statement(Statement::While {
                    condition: Expr::Binary(BinaryOp::LessThan, Box::new(Expr::Var("a".to_string())), Box::new(Expr::Constant(5))),
                    body: Box::new(Statement::Expression(
                        Expr::Assign(Box::new(Expr::Var("a".to_string())), Box::new(Expr::Binary(BinaryOp::Add, Box::new(Expr::Var("a".to_string())), Box::new(Expr::Constant(1)))))
                    )),
                }),
                BlockItem::Statement(Statement::Return(Some(Expr::Var("a".to_string())))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert!(asm.functions[0].instructions.contains(&Instruction::Label(".Lwhile_start0".to_string())));
        assert!(asm.functions[0].instructions.contains(&Instruction::Label(".Lwhile_end0".to_string())));
    }

    #[test]
    fn generate_global_variable() {
        let program = Program {
            declarations: vec![
                TopLevelDecl::Variable(var_decl("x", Some(Expr::Constant(5)))),
                func_decl("main", vec![], Some(vec![
                    BlockItem::Statement(Statement::Return(Some(Expr::Var("x".to_string())))),
                ])),
            ],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars[0].name, "x");
        assert_eq!(asm.static_vars[0].global, true);
        assert_eq!(asm.static_vars[0].init, StaticInit::IntInit(5));
        assert_eq!(asm.static_vars[0].asm_type, AsmType::Longword);
    }

    #[test]
    fn generate_static_local_variable() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl_with_sc("c", Some(Expr::Constant(0)), Some(StorageClass::Static))),
                BlockItem::Statement(Statement::Return(Some(Expr::Var("c".to_string())))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars[0].name, "c.0");
        assert_eq!(asm.static_vars[0].global, false);
        assert_eq!(asm.static_vars[0].asm_type, AsmType::Longword);
    }

    #[test]
    fn generate_extern_with_initializer_error() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl_with_sc("x", Some(Expr::Constant(5)), Some(StorageClass::Extern))),
            ]))],
        };
        assert!(generate(&program).is_err());
    }

    #[test]
    fn generate_static_function() {
        let program = Program {
            declarations: vec![
                func_decl_with_sc("helper", vec![], Some(vec![
                    BlockItem::Statement(Statement::Return(Some(Expr::Constant(42)))),
                ]), Some(StorageClass::Static)),
                func_decl("main", vec![], Some(vec![
                    BlockItem::Statement(Statement::Return(Some(Expr::FunctionCall("helper".to_string(), vec![])))),
                ])),
            ],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.functions[0].global, false);
        assert_eq!(asm.functions[1].global, true);
    }
}
