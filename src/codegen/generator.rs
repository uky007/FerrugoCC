//! コード生成器（Code Generator）
//!
//! TACKY IR を走査し、対応する x86-64 アセンブリ命令列に変換する。
//! 各 TACKY 命令を機械的にアセンブリ命令にマッピングする。
//!
//! # Chapter 20: Pseudo レジスタ出力
//! 変数は `Operand::Pseudo(name)` として出力され、レジスタ割り当てパスで
//! `Register` または `Stack` に置換される。
//! アドレス取得対象・構造体・配列変数のみ `Stack(offset)` に事前割り当てする。

use super::asm_ast::{
    AsmBinaryOp, AsmFunction, AsmStaticConstant, AsmStaticVar, AsmType, AsmUnaryOp, CondCode,
    Instruction, Operand, Reg, StaticInit,
};
use crate::error::{CompileError, Result};
use crate::parse::ast::Type;
use crate::tacky::tacky_ast::*;
use std::collections::{HashMap, HashSet};

/// 引数レジスタの順序（System V AMD64 ABI）
const ARG_REGISTERS: [Reg; 6] = [Reg::DI, Reg::SI, Reg::DX, Reg::CX, Reg::R8, Reg::R9];

/// XMM 引数レジスタの順序（System V AMD64 ABI）
const XMM_ARG_REGISTERS: [Reg; 8] = [
    Reg::XMM0,
    Reg::XMM1,
    Reg::XMM2,
    Reg::XMM3,
    Reg::XMM4,
    Reg::XMM5,
    Reg::XMM6,
    Reg::XMM7,
];

/// Type → AsmType 変換。
fn type_to_asm(t: &Type) -> AsmType {
    match t {
        Type::Void => unreachable!("void has no assembly representation"),
        Type::Char | Type::UChar => AsmType::Byte,
        Type::Short | Type::UShort => AsmType::Word,
        Type::Int | Type::UInt => AsmType::Longword,
        Type::Long | Type::ULong => AsmType::Quadword,
        Type::Float => AsmType::Float,
        Type::Double => AsmType::Double,
        Type::Pointer(_) => AsmType::Quadword,
        Type::Array(_, _) => AsmType::Quadword,
        Type::Struct { .. } => unreachable!("struct has no single AsmType"),
        Type::Function { .. } => unreachable!("function type has no assembly representation"),
        Type::VaList => AsmType::Quadword,
    }
}

/// AsmType のバイトサイズ。
fn asm_type_size(t: AsmType) -> i32 {
    match t {
        AsmType::Byte => 1,
        AsmType::Word => 2,
        AsmType::Longword | AsmType::Float => 4,
        AsmType::Quadword | AsmType::Double => 8,
    }
}

/// safe_asm_type: 構造体型にはプレースホルダーを使う
fn safe_asm_type(t: &Type) -> AsmType {
    if t.is_struct() {
        AsmType::Quadword
    } else {
        type_to_asm(t)
    }
}

/// コード生成の結果（各関数の命令列 + var_types）
pub struct CodegenFunctionResult {
    pub func: AsmFunction,
    pub var_types: HashMap<String, Type>,
}

/// TACKY プログラムをアセンブリ AST に変換する。
///
/// Chapter 20: 各関数の var_types も返す（レジスタ割り当てで使用）。
pub fn generate(
    program: &TackyProgram,
) -> Result<(
    Vec<CodegenFunctionResult>,
    Vec<AsmStaticVar>,
    Vec<AsmStaticConstant>,
)> {
    let mut results = Vec::new();

    // 静的変数のセット（Pseudo にしない変数）
    let mut static_vars_set: HashMap<String, String> = HashMap::new();
    for sv in &program.static_vars {
        static_vars_set.insert(sv.name.clone(), sv.name.clone());
    }
    for sc in &program.static_constants {
        if let TackyStaticInit::StringInit(_, _) = &sc.init {
            static_vars_set.insert(sc.name.clone(), sc.name.clone());
        }
    }

    // double/float 定数プール: TACKY の static_constants から構築
    // value = (label, alignment, is_float)
    let mut double_constants: HashMap<u64, (String, usize, bool)> = HashMap::new();
    for sc in &program.static_constants {
        if let TackyStaticInit::DoubleInit(v) = sc.init {
            double_constants.insert(v.to_bits(), (sc.name.clone(), sc.alignment, false));
        }
    }

    // extern 変数（__stderrp 等）も static_vars_set に追加
    for func in &program.functions {
        for name in &func.static_var_names {
            static_vars_set
                .entry(name.clone())
                .or_insert_with(|| name.clone());
        }
    }

    for func in &program.functions {
        results.push(generate_function(
            func,
            &static_vars_set,
            &mut double_constants,
        )?);
    }

    // 静的変数を AsmStaticVar に変換
    let static_vars: Vec<AsmStaticVar> = program
        .static_vars
        .iter()
        .map(|sv| {
            let elem_type = sv.var_type.target_type();
            let is_struct_array =
                sv.var_type.is_array() && elem_type.is_some_and(|t| t.is_struct());
            let asm_type = if sv.var_type.is_struct() || is_struct_array {
                AsmType::Quadword
            } else if sv.var_type.is_array() {
                // 配列の asm_type は末端要素型を使用（多次元配列対応）
                {
                    let mut t = elem_type.unwrap();
                    while let Type::Array(inner, _) = t {
                        t = inner;
                    }
                    type_to_asm(t)
                }
            } else {
                type_to_asm(&sv.var_type)
            };
            AsmStaticVar {
                name: sv.name.clone(),
                global: sv.global,
                init: convert_static_init(&sv.init),
                asm_type,
                var_type: if is_struct_array || sv.var_type.is_struct() {
                    Some(sv.var_type.clone())
                } else {
                    None
                },
            }
        })
        .collect();

    // 静的定数を AsmStaticConstant に変換
    let mut static_constants: Vec<AsmStaticConstant> = program
        .static_constants
        .iter()
        .filter(|sc| !matches!(sc.init, TackyStaticInit::DoubleInit(_)))
        .map(|sc| AsmStaticConstant {
            name: sc.name.clone(),
            alignment: sc.alignment,
            init: convert_static_init(&sc.init),
        })
        .collect();

    // ソートして決定的な順序で出力
    let mut sorted_doubles: Vec<_> = double_constants.iter().collect();
    sorted_doubles.sort_by_key(|(bits, _)| *bits);
    for (bits, (name, alignment, is_float)) in sorted_doubles {
        let init = if *is_float {
            // Float constant (key has 0x1_0000_0000 marker bit)
            StaticInit::FloatInit(f32::from_bits((*bits & 0xFFFF_FFFF) as u32))
        } else {
            StaticInit::DoubleInit(f64::from_bits(*bits))
        };
        static_constants.push(AsmStaticConstant {
            name: name.clone(),
            alignment: *alignment,
            init,
        });
    }

    Ok((results, static_vars, static_constants))
}

fn convert_static_init(init: &TackyStaticInit) -> StaticInit {
    match init {
        TackyStaticInit::IntInit(v) => StaticInit::IntInit(*v),
        TackyStaticInit::FloatInit(v) => StaticInit::FloatInit(*v),
        TackyStaticInit::DoubleInit(v) => StaticInit::DoubleInit(*v),
        TackyStaticInit::ZeroInit(n) => StaticInit::ZeroInit(*n),
        TackyStaticInit::StringInit(s, n) => StaticInit::StringInit(s.clone(), *n),
        TackyStaticInit::ByteArrayInit(bytes) => StaticInit::ByteArrayInit(bytes.clone()),
        TackyStaticInit::PointerArrayInit(labels) => StaticInit::PointerArrayInit(labels.clone()),
        TackyStaticInit::ArrayInit(elems) => {
            StaticInit::ArrayInit(elems.iter().map(convert_static_init).collect())
        }
    }
}

/// パラメータの分類
struct ParamClassification {
    locations: Vec<(Type, ParamLocation)>,
    stack_count: usize,
}

enum ParamLocation {
    IntReg(usize),
    XmmReg(usize),
    Stack(i32),
}

fn classify_parameters(params: &[(Type, String)]) -> ParamClassification {
    let mut int_idx = 0;
    let mut xmm_idx = 0;
    let mut stack_idx = 0;
    let mut locations = Vec::new();

    for (param_type, _) in params {
        if param_type.is_floating() {
            if xmm_idx < 8 {
                locations.push((param_type.clone(), ParamLocation::XmmReg(xmm_idx)));
                xmm_idx += 1;
            } else {
                let offset = 16 + stack_idx * 8;
                locations.push((param_type.clone(), ParamLocation::Stack(offset as i32)));
                stack_idx += 1;
            }
        } else if int_idx < 6 {
            locations.push((param_type.clone(), ParamLocation::IntReg(int_idx)));
            int_idx += 1;
        } else {
            let offset = 16 + stack_idx * 8;
            locations.push((param_type.clone(), ParamLocation::Stack(offset as i32)));
            stack_idx += 1;
        }
    }

    ParamClassification {
        locations,
        stack_count: stack_idx,
    }
}

/// Pseudo にしてはいけない変数を収集する（アドレス取得対象・構造体・配列）
fn collect_forced_stack_vars(
    body: &[TackyInstruction],
    var_types: &HashMap<String, Type>,
) -> HashSet<String> {
    let mut forced = HashSet::new();

    for instr in body {
        match instr {
            TackyInstruction::GetAddress {
                src: TackyVal::Var(name),
                ..
            } => {
                forced.insert(name.clone());
            }
            TackyInstruction::CopyToOffset { dst, .. } => {
                forced.insert(dst.clone());
            }
            TackyInstruction::CopyFromOffset { src, .. } => {
                forced.insert(src.clone());
            }
            _ => {}
        }
    }

    // 構造体/配列型/va_list型の変数もスタックに強制配置
    for (name, ty) in var_types {
        if ty.is_struct() || ty.is_array() || ty.is_va_list() {
            forced.insert(name.clone());
        }
    }

    forced
}

fn generate_function(
    func: &TackyFunction,
    static_vars: &HashMap<String, String>,
    double_constants: &mut HashMap<u64, (String, usize, bool)>,
) -> Result<CodegenFunctionResult> {
    let mut instructions = Vec::new();

    // スタックに強制配置する変数のオフセットを計算
    let mut stack_vars: HashMap<String, i32> = HashMap::new();
    let mut next_offset: i32 = 0;
    let mut var_types = func.var_types.clone();

    // va_list パラメータは「ポインタ」として受け取る（呼び出し元が &va_list を渡す）。
    // ローカル va_list (va_start) は 24B struct だが、パラメータは 8B ポインタ。
    // 型を Pointer(VaList) に変更して、引数渡し時に Lea ではなく Mov を使わせる。
    for param_name in &func.params {
        if let Some(ty) = var_types.get(param_name)
            && matches!(ty, Type::VaList)
        {
            var_types.insert(param_name.clone(), Type::Pointer(Box::new(Type::VaList)));
        }
    }

    // 可変長関数: レジスタ保存領域（176B）をスタックに確保
    if func.is_variadic {
        // __va_reg_save: 176B (6 GP×8B + 8 XMM×16B), align 16
        next_offset -= 176;
        if next_offset % 16 != 0 {
            next_offset -= 16 + (next_offset % 16);
        }
        stack_vars.insert("__va_reg_save".to_string(), next_offset);
        var_types.insert(
            "__va_reg_save".to_string(),
            Type::Array(Box::new(Type::UChar), 176),
        );
    }

    let forced_stack = collect_forced_stack_vars(&func.body, &var_types);

    // 1. 強制スタック変数を割り当て（ソート済みキーで決定的に）
    let mut sorted_var_types: Vec<(&String, &Type)> = var_types.iter().collect();
    sorted_var_types.sort_by_key(|(name, _)| (*name).clone());
    for (name, ty) in sorted_var_types {
        if static_vars.contains_key(name) {
            continue;
        }
        if !forced_stack.contains(name) {
            continue;
        }
        if ty.is_void() {
            continue;
        }
        let (size, align) = if ty.is_struct() || ty.is_array() || ty.is_va_list() {
            (ty.size() as i32, ty.alignment() as i32)
        } else {
            let at = safe_asm_type(ty);
            let s = asm_type_size(at);
            (s, s)
        };
        next_offset -= size;
        if align > 0 && next_offset % align != 0 {
            next_offset -= align + (next_offset % align);
        }
        stack_vars.insert(name.clone(), next_offset);
    }

    // 可変長関数: レジスタ保存領域に全引数レジスタを保存（パラメータ受け取り前に）
    if func.is_variadic {
        let reg_save_offset = *stack_vars.get("__va_reg_save").unwrap();
        // GP レジスタ保存 (6個 × 8B = 48B)
        for (i, &reg) in ARG_REGISTERS.iter().enumerate() {
            instructions.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(reg),
                dst: Operand::Stack(reg_save_offset + (i * 8) as i32),
            });
        }
        // XMM レジスタ保存 (8個 × 16B = 128B, offset 48 以降)
        for (i, &reg) in XMM_ARG_REGISTERS.iter().enumerate() {
            instructions.push(Instruction::Mov {
                asm_type: AsmType::Double,
                src: Operand::Register(reg),
                dst: Operand::Stack(reg_save_offset + 48 + (i * 16) as i32),
            });
        }
    }

    // 2. パラメータの処理（Pseudo or Stack）
    let param_types_names: Vec<(Type, String)> = func
        .params
        .iter()
        .map(|name| {
            let ty = func.var_types.get(name).cloned().unwrap_or(Type::Int);
            (ty, name.clone())
        })
        .collect();
    let classification = classify_parameters(&param_types_names);

    for (i, param_name) in func.params.iter().enumerate() {
        let param_type = func.var_types.get(param_name).cloned().unwrap_or(Type::Int);
        let asm_type = safe_asm_type(&param_type);
        let dst_op = resolve_var_operand(param_name, static_vars, &stack_vars);

        let (_, ref loc) = classification.locations[i];
        match loc {
            ParamLocation::IntReg(idx) => {
                // 可変長関数の場合、レジスタは既にreg_save_areaに保存済み。
                // パラメータはreg_save_areaからロードする。
                if func.is_variadic {
                    let reg_save_offset = *stack_vars.get("__va_reg_save").unwrap();
                    instructions.push(Instruction::Mov {
                        asm_type: AsmType::Quadword,
                        src: Operand::Stack(reg_save_offset + (*idx * 8) as i32),
                        dst: Operand::Register(Reg::R10),
                    });
                    instructions.push(Instruction::Mov {
                        asm_type,
                        src: Operand::Register(Reg::R10),
                        dst: dst_op,
                    });
                } else {
                    instructions.push(Instruction::Mov {
                        asm_type,
                        src: Operand::Register(ARG_REGISTERS[*idx]),
                        dst: dst_op,
                    });
                }
            }
            ParamLocation::XmmReg(idx) => {
                if func.is_variadic {
                    let reg_save_offset = *stack_vars.get("__va_reg_save").unwrap();
                    instructions.push(Instruction::Mov {
                        asm_type,
                        src: Operand::Stack(reg_save_offset + 48 + (*idx * 16) as i32),
                        dst: Operand::Register(Reg::XMM15),
                    });
                    instructions.push(Instruction::Mov {
                        asm_type,
                        src: Operand::Register(Reg::XMM15),
                        dst: dst_op,
                    });
                } else {
                    instructions.push(Instruction::Mov {
                        asm_type,
                        src: Operand::Register(XMM_ARG_REGISTERS[*idx]),
                        dst: dst_op,
                    });
                }
            }
            ParamLocation::Stack(stack_offset) => {
                // Stack-passed parameters: need a scratch register to copy
                if param_type.is_floating() {
                    instructions.push(Instruction::Mov {
                        asm_type,
                        src: Operand::Stack(*stack_offset),
                        dst: Operand::Register(Reg::XMM0),
                    });
                    instructions.push(Instruction::Mov {
                        asm_type,
                        src: Operand::Register(Reg::XMM0),
                        dst: dst_op,
                    });
                } else {
                    instructions.push(Instruction::Mov {
                        asm_type: AsmType::Quadword,
                        src: Operand::Stack(*stack_offset),
                        dst: Operand::Register(Reg::AX),
                    });
                    instructions.push(Instruction::Mov {
                        asm_type,
                        src: Operand::Register(Reg::AX),
                        dst: dst_op,
                    });
                }
            }
        }
    }

    // 3. TACKY 命令を変換
    let mut va_label_counter: usize = 0;
    for instr in &func.body {
        generate_instruction(
            instr,
            static_vars,
            &stack_vars,
            &mut instructions,
            double_constants,
            &var_types,
            &mut va_label_counter,
            &func.return_type,
        )?;
    }

    Ok(CodegenFunctionResult {
        func: AsmFunction {
            name: func.name.clone(),
            instructions,
            global: func.global,
        },
        var_types,
    })
}

/// 変数名からオペランドを解決する（Chapter 20: Pseudo 対応）
fn resolve_var_operand(
    name: &str,
    static_vars: &HashMap<String, String>,
    stack_vars: &HashMap<String, i32>,
) -> Operand {
    if let Some(label) = static_vars.get(name) {
        Operand::Data(label.clone())
    } else if let Some(offset) = stack_vars.get(name) {
        Operand::Stack(*offset)
    } else {
        Operand::Pseudo(name.to_string())
    }
}

/// TackyVal → Operand に変換する
fn val_to_operand(
    val: &TackyVal,
    static_vars: &HashMap<String, String>,
    stack_vars: &HashMap<String, i32>,
) -> Result<Operand> {
    match val {
        TackyVal::Constant(c) => match c {
            TackyConst::Int(v) => Ok(Operand::Imm(*v as i64)),
            TackyConst::Long(v) => Ok(Operand::Imm(*v)),
            TackyConst::UInt(v) => Ok(Operand::Imm(*v as i64)),
            TackyConst::ULong(v) => Ok(Operand::Imm(*v as i64)),
            TackyConst::Char(v) => Ok(Operand::Imm(*v as i64)),
            TackyConst::UChar(v) => Ok(Operand::Imm(*v as i64)),
            TackyConst::Float(_) => Err(CompileError::CodegenError(
                "float constant should be loaded from memory".to_string(),
            )),
            TackyConst::Double(_) => Err(CompileError::CodegenError(
                "double constant should be loaded from memory".to_string(),
            )),
        },
        TackyVal::Var(name) => Ok(resolve_var_operand(name, static_vars, stack_vars)),
    }
}

/// TackyVal の型を取得する
fn val_type(val: &TackyVal, var_types: &HashMap<String, Type>) -> Type {
    match val {
        TackyVal::Constant(c) => match c {
            TackyConst::Int(_) => Type::Int,
            TackyConst::Long(_) => Type::Long,
            TackyConst::UInt(_) => Type::UInt,
            TackyConst::ULong(_) => Type::ULong,
            TackyConst::Float(_) => Type::Float,
            TackyConst::Double(_) => Type::Double,
            TackyConst::Char(_) => Type::Char,
            TackyConst::UChar(_) => Type::UChar,
        },
        TackyVal::Var(name) => var_types.get(name).cloned().unwrap_or(Type::Int),
    }
}

/// 浮動小数点定数をオペランドにロードする（double / float 共用）
fn load_double_val(
    val: &TackyVal,
    static_vars: &HashMap<String, String>,
    stack_vars: &HashMap<String, i32>,
    _instrs: &mut Vec<Instruction>,
    double_constants: &mut HashMap<u64, (String, usize, bool)>,
    const_label_counter: &mut usize,
) -> Operand {
    match val {
        TackyVal::Constant(TackyConst::Double(v)) => {
            let bits = v.to_bits();
            let label = if let Some((l, _, _)) = double_constants.get(&bits) {
                l.clone()
            } else {
                let l = format!(".Lconst_{}", *const_label_counter);
                *const_label_counter += 1;
                double_constants.insert(bits, (l.clone(), 8, false));
                l
            };
            Operand::Data(label)
        }
        TackyVal::Constant(TackyConst::Float(v)) => {
            // float は 4 バイト定数。bit パターンを u64 に拡張してキーとして使う
            let bits = v.to_bits() as u64 | 0x1_0000_0000; // 上位ビットで double と区別
            let label = if let Some((l, _, _)) = double_constants.get(&bits) {
                l.clone()
            } else {
                let l = format!(".Lconst_{}", *const_label_counter);
                *const_label_counter += 1;
                double_constants.insert(bits, (l.clone(), 4, true));
                l
            };
            Operand::Data(label)
        }
        TackyVal::Var(name) => resolve_var_operand(name, static_vars, stack_vars),
        _ => unreachable!("expected floating-point val"),
    }
}

/// 単一の TACKY 命令をアセンブリ命令列に変換する
#[allow(clippy::too_many_arguments)]
fn generate_instruction(
    instr: &TackyInstruction,
    static_vars: &HashMap<String, String>,
    stack_vars: &HashMap<String, i32>,
    instrs: &mut Vec<Instruction>,
    double_constants: &mut HashMap<u64, (String, usize, bool)>,
    var_types: &HashMap<String, Type>,
    _va_label_counter: &mut usize,
    func_return_type: &Type,
) -> Result<()> {
    let mut const_counter: usize = double_constants.len();

    match instr {
        TackyInstruction::Return(val) => {
            let ty = val_type(val, var_types);
            if ty.is_floating() {
                let src = load_double_val(
                    val,
                    static_vars,
                    stack_vars,
                    instrs,
                    double_constants,
                    &mut const_counter,
                );
                let fp_asm = if ty.is_float() {
                    AsmType::Float
                } else {
                    AsmType::Double
                };
                instrs.push(Instruction::Mov {
                    asm_type: fp_asm,
                    src,
                    dst: Operand::Register(Reg::XMM0),
                });
            } else if func_return_type.is_struct() && func_return_type.size() <= 16 {
                // System V ABI: struct ≤ 16 bytes returned in RAX (+ RDX)
                // The TACKY value is a Pointer(Struct) (address of local struct).
                let src = val_to_operand(val, static_vars, stack_vars)?;
                let struct_size = func_return_type.size();
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Quadword,
                    src: src.clone(),
                    dst: Operand::Register(Reg::CX),
                });
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Quadword,
                    src: Operand::Memory(Reg::CX),
                    dst: Operand::Register(Reg::AX),
                });
                if struct_size > 8 {
                    instrs.push(Instruction::Mov {
                        asm_type: AsmType::Quadword,
                        src: Operand::MemoryOffset(Reg::CX, 8),
                        dst: Operand::Register(Reg::DX),
                    });
                }
            } else {
                let src = val_to_operand(val, static_vars, stack_vars)?;
                let asm_type = safe_asm_type(&ty);
                instrs.push(Instruction::Mov {
                    asm_type,
                    src,
                    dst: Operand::Register(Reg::AX),
                });
            }
            instrs.push(Instruction::Ret);
        }

        TackyInstruction::ReturnVoid => {
            // sret 関数: sret pointer を %rax に返す (System V ABI)
            if func_return_type.is_struct() && func_return_type.size() > 16 {
                let sret_op = val_to_operand(
                    &TackyVal::Var("__sret_ptr".to_string()),
                    static_vars,
                    stack_vars,
                )?;
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Quadword,
                    src: sret_op,
                    dst: Operand::Register(Reg::AX),
                });
            }
            instrs.push(Instruction::Ret);
        }

        TackyInstruction::Copy { src, dst } => {
            let dst_type = val_type(dst, var_types);
            if dst_type.is_floating() {
                let src_op = load_double_val(
                    src,
                    static_vars,
                    stack_vars,
                    instrs,
                    double_constants,
                    &mut const_counter,
                );
                let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
                let fp_asm = if dst_type.is_float() {
                    AsmType::Float
                } else {
                    AsmType::Double
                };
                // Chapter 20: direct mov (fixup will handle if both are memory)
                instrs.push(Instruction::Mov {
                    asm_type: fp_asm,
                    src: src_op,
                    dst: dst_op,
                });
            } else {
                let src_op = val_to_operand(src, static_vars, stack_vars)?;
                let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
                let asm_type = safe_asm_type(&dst_type);
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: src_op,
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::Unary { op, src, dst } => {
            let dst_type = val_type(dst, var_types);
            let src_type = val_type(src, var_types);

            match op {
                TackyUnaryOp::Negate => {
                    if dst_type.is_floating() {
                        let src_op = load_double_val(
                            src,
                            static_vars,
                            stack_vars,
                            instrs,
                            double_constants,
                            &mut const_counter,
                        );
                        let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
                        let fp_asm = if dst_type.is_float() {
                            AsmType::Float
                        } else {
                            AsmType::Double
                        };
                        instrs.push(Instruction::Mov {
                            asm_type: fp_asm,
                            src: src_op,
                            dst: Operand::Register(Reg::XMM0),
                        });
                        let (neg_zero_bits, neg_align) = if dst_type.is_float() {
                            (0x1_8000_0000u64, 16) // marker bit + sign mask, 16-byte aligned for xorps
                        } else {
                            (0x8000000000000000u64, 16)
                        };
                        let neg_label = get_or_add_double_constant(
                            neg_zero_bits,
                            neg_align,
                            dst_type.is_float(),
                            double_constants,
                            &mut const_counter,
                        );
                        instrs.push(Instruction::Binary {
                            asm_type: fp_asm,
                            op: AsmBinaryOp::Xor,
                            src: Operand::Data(neg_label),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: fp_asm,
                            src: Operand::Register(Reg::XMM0),
                            dst: dst_op,
                        });
                    } else {
                        let asm_type = safe_asm_type(&dst_type);
                        let src_op = val_to_operand(src, static_vars, stack_vars)?;
                        let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
                        // Chapter 20: Mov src→dst; Neg dst
                        instrs.push(Instruction::Mov {
                            asm_type,
                            src: src_op,
                            dst: dst_op.clone(),
                        });
                        instrs.push(Instruction::Unary {
                            asm_type,
                            op: AsmUnaryOp::Neg,
                            operand: dst_op,
                        });
                    }
                }
                TackyUnaryOp::Complement => {
                    let asm_type = safe_asm_type(&dst_type);
                    let src_op = val_to_operand(src, static_vars, stack_vars)?;
                    let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
                    // Chapter 20: Mov src→dst; Not dst
                    instrs.push(Instruction::Mov {
                        asm_type,
                        src: src_op,
                        dst: dst_op.clone(),
                    });
                    instrs.push(Instruction::Unary {
                        asm_type,
                        op: AsmUnaryOp::Not,
                        operand: dst_op,
                    });
                }
                TackyUnaryOp::Not => {
                    let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
                    if src_type.is_floating() {
                        let src_op = load_double_val(
                            src,
                            static_vars,
                            stack_vars,
                            instrs,
                            double_constants,
                            &mut const_counter,
                        );
                        let fp_asm = if src_type.is_float() {
                            AsmType::Float
                        } else {
                            AsmType::Double
                        };
                        instrs.push(Instruction::Mov {
                            asm_type: fp_asm,
                            src: src_op,
                            dst: Operand::Register(Reg::XMM0),
                        });
                        instrs.push(Instruction::Binary {
                            asm_type: fp_asm,
                            op: AsmBinaryOp::Xor,
                            src: Operand::Register(Reg::XMM15),
                            dst: Operand::Register(Reg::XMM15),
                        });
                        instrs.push(Instruction::Cmp {
                            asm_type: fp_asm,
                            src: Operand::Register(Reg::XMM15),
                            dst: Operand::Register(Reg::XMM0),
                        });
                    } else {
                        let src_asm = safe_asm_type(&src_type);
                        let src_op = val_to_operand(src, static_vars, stack_vars)?;
                        // Chapter 20: Cmp $0, src (directly)
                        instrs.push(Instruction::Cmp {
                            asm_type: src_asm,
                            src: Operand::Imm(0),
                            dst: src_op,
                        });
                    }
                    instrs.push(Instruction::Mov {
                        asm_type: AsmType::Longword,
                        src: Operand::Imm(0),
                        dst: dst_op.clone(),
                    });
                    instrs.push(Instruction::SetCC {
                        condition: CondCode::E,
                        operand: dst_op,
                    });
                }
            }
        }

        TackyInstruction::Binary {
            op,
            left,
            right,
            dst,
        } => {
            generate_binary_instruction(
                *op,
                left,
                right,
                dst,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                &mut const_counter,
                var_types,
            )?;
        }

        TackyInstruction::Jump(label) => {
            instrs.push(Instruction::Jmp(label.clone()));
        }

        TackyInstruction::JumpIfZero { condition, target } => {
            let cond_type = val_type(condition, var_types);
            let asm_type = safe_asm_type(&cond_type);
            let cond_op = val_to_operand(condition, static_vars, stack_vars)?;
            // Chapter 20: Cmp directly on operand
            instrs.push(Instruction::Cmp {
                asm_type,
                src: Operand::Imm(0),
                dst: cond_op,
            });
            instrs.push(Instruction::JmpCC(CondCode::E, target.clone()));
        }

        TackyInstruction::JumpIfNotZero { condition, target } => {
            let cond_type = val_type(condition, var_types);
            let asm_type = safe_asm_type(&cond_type);
            let cond_op = val_to_operand(condition, static_vars, stack_vars)?;
            instrs.push(Instruction::Cmp {
                asm_type,
                src: Operand::Imm(0),
                dst: cond_op,
            });
            instrs.push(Instruction::JmpCC(CondCode::NE, target.clone()));
        }

        TackyInstruction::Label(label) => {
            instrs.push(Instruction::Label(label.clone()));
        }

        TackyInstruction::FunCall {
            name,
            args,
            dst,
            dst_type,
            is_variadic,
        } => {
            generate_function_call(
                name,
                args,
                dst,
                dst_type,
                *is_variadic,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                &mut const_counter,
                var_types,
            )?;
        }

        TackyInstruction::SignExtend { src, dst } => {
            let src_type = val_type(src, var_types);
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            let src_size = src_type.size();

            if src_size == 1 {
                let dst_type = val_type(dst, var_types);
                let dst_asm = safe_asm_type(&dst_type);
                instrs.push(Instruction::MovsxByte {
                    asm_type: dst_asm,
                    src: src_op,
                    dst: dst_op,
                });
            } else if src_size == 2 {
                let dst_type = val_type(dst, var_types);
                let dst_asm = safe_asm_type(&dst_type);
                instrs.push(Instruction::MovsxWord {
                    asm_type: dst_asm,
                    src: src_op,
                    dst: dst_op,
                });
            } else {
                instrs.push(Instruction::Movsx {
                    src: src_op,
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::ZeroExtend { src, dst } => {
            let src_type = val_type(src, var_types);
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            let src_size = src_type.size();

            if src_size == 1 {
                let dst_type = val_type(dst, var_types);
                let dst_asm = safe_asm_type(&dst_type);
                instrs.push(Instruction::MovZeroExtendByte {
                    asm_type: dst_asm,
                    src: src_op,
                    dst: dst_op,
                });
            } else if src_size == 2 {
                let dst_type = val_type(dst, var_types);
                let dst_asm = safe_asm_type(&dst_type);
                instrs.push(Instruction::MovZeroExtendWord {
                    asm_type: dst_asm,
                    src: src_op,
                    dst: dst_op,
                });
            } else {
                instrs.push(Instruction::MovZeroExtend {
                    src: src_op,
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::Truncate { src, dst } => {
            let dst_type = val_type(dst, var_types);
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            let dst_asm = safe_asm_type(&dst_type);
            // 16-bit/8-bit への truncate は正しいサイズの Mov を使用
            // (Truncate は movl 固定で 32-bit 書き込みのため、short/char dst では隣接メモリを破壊する)
            if matches!(dst_asm, AsmType::Word | AsmType::Byte) {
                instrs.push(Instruction::Mov {
                    asm_type: dst_asm,
                    src: src_op,
                    dst: dst_op,
                });
            } else {
                instrs.push(Instruction::Truncate {
                    src: src_op,
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::IntToDouble { src, dst } => {
            let src_type = val_type(src, var_types);
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            let asm_t = safe_asm_type(&src_type);

            // x86-64 の cvtsi2sd は 32-bit/64-bit のみ。16-bit/8-bit は 32-bit に sign-extend してから変換
            if asm_t == AsmType::Word {
                instrs.push(Instruction::MovsxWord {
                    asm_type: AsmType::Longword,
                    src: src_op,
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Cvtsi2sd {
                    asm_type: AsmType::Longword,
                    src: Operand::Register(Reg::AX),
                    dst: dst_op,
                });
            } else if asm_t == AsmType::Byte {
                instrs.push(Instruction::MovsxByte {
                    asm_type: AsmType::Longword,
                    src: src_op,
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Cvtsi2sd {
                    asm_type: AsmType::Longword,
                    src: Operand::Register(Reg::AX),
                    dst: dst_op,
                });
            } else {
                instrs.push(Instruction::Cvtsi2sd {
                    asm_type: asm_t,
                    src: src_op,
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::DoubleToInt { src, dst } => {
            let dst_type = val_type(dst, var_types);
            let src_op = load_double_val(
                src,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                &mut const_counter,
            );
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            let asm_t = safe_asm_type(&dst_type);

            // x86-64 の cvttsd2si は 32-bit/64-bit のみ。16-bit/8-bit は 32-bit 経由で truncate
            if matches!(asm_t, AsmType::Word | AsmType::Byte) {
                instrs.push(Instruction::Cvttsd2si {
                    asm_type: AsmType::Longword,
                    src: src_op,
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Mov {
                    asm_type: asm_t,
                    src: Operand::Register(Reg::AX),
                    dst: dst_op,
                });
            } else {
                instrs.push(Instruction::Cvttsd2si {
                    asm_type: asm_t,
                    src: src_op,
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::UIntToDouble { src, dst } => {
            // unsigned long → double: conditional algorithm (uses hardcoded regs)
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

            let large_label = format!(".Lul2d_large{}", const_counter);
            let end_label = format!(".Lul2d_end{}", const_counter);
            #[allow(unused_assignments)]
            {
                const_counter += 1;
            }

            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: src_op,
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::Cmp {
                asm_type: AsmType::Quadword,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::JmpCC(CondCode::L, large_label.clone()));
            instrs.push(Instruction::Cvtsi2sd {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::AX),
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::Jmp(end_label.clone()));
            instrs.push(Instruction::Label(large_label));
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
            instrs.push(Instruction::Binary {
                asm_type: AsmType::Double,
                op: AsmBinaryOp::Add,
                src: Operand::Register(Reg::XMM0),
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::Label(end_label));
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Double,
                src: Operand::Register(Reg::XMM0),
                dst: dst_op,
            });
        }

        TackyInstruction::DoubleToUInt { src, dst } => {
            // double → unsigned long: conditional algorithm
            let src_op = load_double_val(
                src,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                &mut const_counter,
            );
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

            let out_of_range_label = format!(".Ld2ul_oor{}", const_counter);
            let end_label = format!(".Ld2ul_end{}", const_counter);
            const_counter += 1;

            let bound_label = get_or_add_double_constant(
                9223372036854775808.0f64.to_bits(),
                8,
                false,
                double_constants,
                &mut const_counter,
            );

            instrs.push(Instruction::Mov {
                asm_type: AsmType::Double,
                src: src_op,
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::Cmp {
                asm_type: AsmType::Double,
                src: Operand::Data(bound_label.clone()),
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::JmpCC(CondCode::AE, out_of_range_label.clone()));
            instrs.push(Instruction::Cvttsd2si {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::XMM0),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::Jmp(end_label.clone()));
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
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Imm(i64::MIN),
                dst: Operand::Register(Reg::CX),
            });
            instrs.push(Instruction::Binary {
                asm_type: AsmType::Quadword,
                op: AsmBinaryOp::Add,
                src: Operand::Register(Reg::CX),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::Label(end_label));
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::AX),
                dst: dst_op,
            });
        }

        // Float conversion instructions — for now, delegate to double equivalents
        TackyInstruction::FloatToDouble { src, dst } => {
            let src_op = load_double_val(
                src,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                &mut const_counter,
            );
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            instrs.push(Instruction::Cvtss2sd {
                src: src_op,
                dst: dst_op,
            });
        }

        TackyInstruction::DoubleToFloat { src, dst } => {
            let src_op = load_double_val(
                src,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                &mut const_counter,
            );
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            instrs.push(Instruction::Cvtsd2ss {
                src: src_op,
                dst: dst_op,
            });
        }

        TackyInstruction::IntToFloat { src, dst } => {
            let src_type = val_type(src, var_types);
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            let asm_t = safe_asm_type(&src_type);

            // x86-64 の cvtsi2ss は 32-bit/64-bit のみ。16-bit/8-bit は 32-bit に sign-extend してから変換
            if asm_t == AsmType::Word {
                instrs.push(Instruction::MovsxWord {
                    asm_type: AsmType::Longword,
                    src: src_op,
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Cvtsi2ss {
                    asm_type: AsmType::Longword,
                    src: Operand::Register(Reg::AX),
                    dst: dst_op,
                });
            } else if asm_t == AsmType::Byte {
                instrs.push(Instruction::MovsxByte {
                    asm_type: AsmType::Longword,
                    src: src_op,
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Cvtsi2ss {
                    asm_type: AsmType::Longword,
                    src: Operand::Register(Reg::AX),
                    dst: dst_op,
                });
            } else {
                instrs.push(Instruction::Cvtsi2ss {
                    asm_type: asm_t,
                    src: src_op,
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::FloatToInt { src, dst } => {
            let dst_type = val_type(dst, var_types);
            let src_op = load_double_val(
                src,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                &mut const_counter,
            );
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            let asm_t = safe_asm_type(&dst_type);

            // x86-64 の cvttss2si は 32-bit/64-bit のみ。16-bit/8-bit は 32-bit 経由で truncate
            if matches!(asm_t, AsmType::Word | AsmType::Byte) {
                instrs.push(Instruction::Cvttss2si {
                    asm_type: AsmType::Longword,
                    src: src_op,
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Mov {
                    asm_type: asm_t,
                    src: Operand::Register(Reg::AX),
                    dst: dst_op,
                });
            } else {
                instrs.push(Instruction::Cvttss2si {
                    asm_type: asm_t,
                    src: src_op,
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::UIntToFloat { src, dst } => {
            // unsigned long → float: 既存の unsigned→double 変換 + cvtsd2ss で narrowing
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

            let large_label = format!(".Lul2f_large{}", const_counter);
            let end_label = format!(".Lul2f_end{}", const_counter);
            #[allow(unused_assignments)]
            {
                const_counter += 1;
            }

            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: src_op,
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::Cmp {
                asm_type: AsmType::Quadword,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::JmpCC(CondCode::L, large_label.clone()));
            instrs.push(Instruction::Cvtsi2sd {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::AX),
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::Jmp(end_label.clone()));
            instrs.push(Instruction::Label(large_label));
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
            instrs.push(Instruction::Binary {
                asm_type: AsmType::Double,
                op: AsmBinaryOp::Add,
                src: Operand::Register(Reg::XMM0),
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::Label(end_label));
            // double → float に narrowing してから store
            instrs.push(Instruction::Cvtsd2ss {
                src: Operand::Register(Reg::XMM0),
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Float,
                src: Operand::Register(Reg::XMM0),
                dst: dst_op,
            });
        }

        TackyInstruction::FloatToUInt { src, dst } => {
            // float → unsigned long: float を double に拡張してから double→unsigned パスを使用
            let src_op = load_double_val(
                src,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                &mut const_counter,
            );
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

            let out_of_range_label = format!(".Lf2ul_oor{}", const_counter);
            let end_label = format!(".Lf2ul_end{}", const_counter);
            const_counter += 1;

            let bound_label = get_or_add_double_constant(
                9223372036854775808.0f64.to_bits(),
                8,
                false,
                double_constants,
                &mut const_counter,
            );

            // float → double 拡張
            instrs.push(Instruction::Cvtss2sd {
                src: src_op,
                dst: Operand::Register(Reg::XMM0),
            });
            // 以降は double → unsigned long と同じロジック
            instrs.push(Instruction::Cmp {
                asm_type: AsmType::Double,
                src: Operand::Data(bound_label.clone()),
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::JmpCC(CondCode::AE, out_of_range_label.clone()));
            instrs.push(Instruction::Cvttsd2si {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::XMM0),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::Jmp(end_label.clone()));
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
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Imm(i64::MIN),
                dst: Operand::Register(Reg::CX),
            });
            instrs.push(Instruction::Binary {
                asm_type: AsmType::Quadword,
                op: AsmBinaryOp::Add,
                src: Operand::Register(Reg::CX),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::Label(end_label));
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::AX),
                dst: dst_op,
            });
        }

        TackyInstruction::GetAddress { src, dst } => {
            let src_op = if let TackyVal::Var(name) = src {
                // If not a local/static variable, treat as global symbol (e.g. function name)
                if !stack_vars.contains_key(name) && !static_vars.contains_key(name) {
                    Operand::Data(name.clone())
                } else {
                    val_to_operand(src, static_vars, stack_vars)?
                }
            } else {
                val_to_operand(src, static_vars, stack_vars)?
            };
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            instrs.push(Instruction::Lea {
                src: src_op,
                dst: dst_op,
            });
        }

        TackyInstruction::Load { src_ptr, dst } => {
            let ptr_op = val_to_operand(src_ptr, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            let dst_type = val_type(dst, var_types);
            let asm_type = safe_asm_type(&dst_type);

            // Load ptr into AX, then deref
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: ptr_op,
                dst: Operand::Register(Reg::AX),
            });

            if dst_type.is_floating() {
                let fp_asm = if dst_type.is_float() {
                    AsmType::Float
                } else {
                    AsmType::Double
                };
                instrs.push(Instruction::Mov {
                    asm_type: fp_asm,
                    src: Operand::Memory(Reg::AX),
                    dst: dst_op,
                });
            } else {
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Memory(Reg::AX),
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::Store { src, dst_ptr } => {
            let src_type = val_type(src, var_types);
            let ptr_op = val_to_operand(dst_ptr, static_vars, stack_vars)?;

            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: ptr_op,
                dst: Operand::Register(Reg::CX),
            });

            if src_type.is_floating() {
                let src_op = load_double_val(
                    src,
                    static_vars,
                    stack_vars,
                    instrs,
                    double_constants,
                    &mut const_counter,
                );
                let fp_asm = if src_type.is_float() {
                    AsmType::Float
                } else {
                    AsmType::Double
                };
                instrs.push(Instruction::Mov {
                    asm_type: fp_asm,
                    src: src_op,
                    dst: Operand::Register(Reg::XMM0),
                });
                instrs.push(Instruction::Mov {
                    asm_type: fp_asm,
                    src: Operand::Register(Reg::XMM0),
                    dst: Operand::Memory(Reg::CX),
                });
            } else {
                let asm_type = safe_asm_type(&src_type);
                let src_op = val_to_operand(src, static_vars, stack_vars)?;
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: src_op,
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Register(Reg::AX),
                    dst: Operand::Memory(Reg::CX),
                });
            }
        }

        TackyInstruction::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } => {
            let ptr_op = val_to_operand(ptr, static_vars, stack_vars)?;
            let idx_op = val_to_operand(index, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: idx_op,
                dst: Operand::Register(Reg::AX),
            });
            if *scale != 1 {
                instrs.push(Instruction::Binary {
                    asm_type: AsmType::Quadword,
                    op: AsmBinaryOp::Mult,
                    src: Operand::Imm(*scale as i64),
                    dst: Operand::Register(Reg::AX),
                });
            }
            instrs.push(Instruction::Binary {
                asm_type: AsmType::Quadword,
                op: AsmBinaryOp::Add,
                src: ptr_op,
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::AX),
                dst: dst_op,
            });
        }

        TackyInstruction::CopyToOffset { src, dst, offset } => {
            let src_type = val_type(src, var_types);
            let dst_var_op = resolve_var_operand(dst, static_vars, stack_vars);

            instrs.push(Instruction::Lea {
                src: dst_var_op,
                dst: Operand::Register(Reg::CX),
            });

            if src_type.is_floating() {
                let src_op = load_double_val(
                    src,
                    static_vars,
                    stack_vars,
                    instrs,
                    double_constants,
                    &mut const_counter,
                );
                let fp_asm = if src_type.is_float() {
                    AsmType::Float
                } else {
                    AsmType::Double
                };
                instrs.push(Instruction::Mov {
                    asm_type: fp_asm,
                    src: src_op,
                    dst: Operand::Register(Reg::XMM0),
                });
                instrs.push(Instruction::Mov {
                    asm_type: fp_asm,
                    src: Operand::Register(Reg::XMM0),
                    dst: Operand::MemoryOffset(Reg::CX, *offset as i32),
                });
            } else {
                let asm_type = safe_asm_type(&src_type);
                let src_op = val_to_operand(src, static_vars, stack_vars)?;
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: src_op,
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Register(Reg::AX),
                    dst: Operand::MemoryOffset(Reg::CX, *offset as i32),
                });
            }
        }

        TackyInstruction::CopyFromOffset { src, offset, dst } => {
            let dst_type = val_type(dst, var_types);
            let src_var_op = resolve_var_operand(src, static_vars, stack_vars);
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

            instrs.push(Instruction::Lea {
                src: src_var_op,
                dst: Operand::Register(Reg::AX),
            });

            if dst_type.is_floating() {
                let fp_asm = if dst_type.is_float() {
                    AsmType::Float
                } else {
                    AsmType::Double
                };
                instrs.push(Instruction::Mov {
                    asm_type: fp_asm,
                    src: Operand::MemoryOffset(Reg::AX, *offset as i32),
                    dst: dst_op,
                });
            } else {
                let asm_type = safe_asm_type(&dst_type);
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: Operand::MemoryOffset(Reg::AX, *offset as i32),
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::JumpIndirect {
            target,
            possible_targets,
        } => {
            let target_op = val_to_operand(target, static_vars, stack_vars)?;
            instrs.push(Instruction::JmpIndirect(
                target_op,
                possible_targets.clone(),
            ));
        }

        TackyInstruction::CopyStruct { src, dst, size } => {
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            let src_type = val_type(src, var_types);
            let dst_type = val_type(dst, var_types);

            // struct と VaList はスタック割り当てオブジェクトなので Lea でアドレス取得。
            // それ以外（ポインタ等）は値がアドレスなので Mov。
            if src_type.is_struct() || matches!(src_type, Type::VaList) {
                instrs.push(Instruction::Lea {
                    src: src_op,
                    dst: Operand::Register(Reg::CX),
                });
            } else {
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Quadword,
                    src: src_op,
                    dst: Operand::Register(Reg::CX),
                });
            }
            if dst_type.is_struct() || matches!(dst_type, Type::VaList) {
                instrs.push(Instruction::Lea {
                    src: dst_op,
                    dst: Operand::Register(Reg::DI),
                });
            } else {
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Quadword,
                    src: dst_op,
                    dst: Operand::Register(Reg::DI),
                });
            }

            let mut copied = 0;
            let total_size = *size;
            while copied + 8 <= total_size {
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Quadword,
                    src: Operand::MemoryOffset(Reg::CX, copied as i32),
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Quadword,
                    src: Operand::Register(Reg::AX),
                    dst: Operand::MemoryOffset(Reg::DI, copied as i32),
                });
                copied += 8;
            }
            if copied + 4 <= total_size {
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Longword,
                    src: Operand::MemoryOffset(Reg::CX, copied as i32),
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Longword,
                    src: Operand::Register(Reg::AX),
                    dst: Operand::MemoryOffset(Reg::DI, copied as i32),
                });
                copied += 4;
            }
            while copied < total_size {
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Byte,
                    src: Operand::MemoryOffset(Reg::CX, copied as i32),
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Byte,
                    src: Operand::Register(Reg::AX),
                    dst: Operand::MemoryOffset(Reg::DI, copied as i32),
                });
                copied += 1;
            }
        }

        TackyInstruction::VaStart {
            ap,
            gp_offset_init,
            fp_offset_init,
        } => {
            let ap_name = match ap {
                TackyVal::Var(n) => n.as_str(),
                _ => unreachable!(),
            };
            let ap_offset = *stack_vars.get(ap_name).expect("va_list must be on stack");

            // gp_offset (offset 0, 4 bytes)
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Longword,
                src: Operand::Imm(*gp_offset_init as i64),
                dst: Operand::Stack(ap_offset),
            });

            // fp_offset (offset 4, 4 bytes)
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Longword,
                src: Operand::Imm(*fp_offset_init as i64),
                dst: Operand::Stack(ap_offset + 4),
            });

            // overflow_arg_area (offset 8, 8 bytes) = RBP + 16
            // Stack(16) is the first stack-passed argument location (RBP + 16 after fixup)
            instrs.push(Instruction::Lea {
                src: Operand::Stack(16),
                dst: Operand::Register(Reg::R10),
            });
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::R10),
                dst: Operand::Stack(ap_offset + 8),
            });

            // reg_save_area (offset 16, 8 bytes) = &__va_reg_save
            let reg_save_offset = *stack_vars
                .get("__va_reg_save")
                .expect("__va_reg_save must be on stack");
            instrs.push(Instruction::Lea {
                src: Operand::Stack(reg_save_offset),
                dst: Operand::Register(Reg::R10),
            });
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::R10),
                dst: Operand::Stack(ap_offset + 16),
            });
        }

        TackyInstruction::VaArg { .. } => {
            unreachable!("VaArg should be lowered at TACKY level in tacky_gen.rs")
        }

        TackyInstruction::VaEnd => {
            // No-op
        }
    }

    Ok(())
}

fn get_or_add_double_constant(
    bits: u64,
    alignment: usize,
    is_float: bool,
    double_constants: &mut HashMap<u64, (String, usize, bool)>,
    const_counter: &mut usize,
) -> String {
    if let Some((label, existing_align, _)) = double_constants.get_mut(&bits) {
        if alignment > *existing_align {
            *existing_align = alignment;
        }
        label.clone()
    } else {
        let label = format!(".Lconst_{}", *const_counter);
        *const_counter += 1;
        double_constants.insert(bits, (label.clone(), alignment, is_float));
        label
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_binary_instruction(
    op: TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    static_vars: &HashMap<String, String>,
    stack_vars: &HashMap<String, i32>,
    instrs: &mut Vec<Instruction>,
    double_constants: &mut HashMap<u64, (String, usize, bool)>,
    const_counter: &mut usize,
    var_types: &HashMap<String, Type>,
) -> Result<()> {
    let left_type = val_type(left, var_types);
    let dst_type = val_type(dst, var_types);
    let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

    match op {
        TackyBinaryOp::AddDouble
        | TackyBinaryOp::SubDouble
        | TackyBinaryOp::MulDouble
        | TackyBinaryOp::DivDouble => {
            let left_op = load_double_val(
                left,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                const_counter,
            );
            let right_op = load_double_val(
                right,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                const_counter,
            );
            let asm_op = match op {
                TackyBinaryOp::AddDouble => AsmBinaryOp::Add,
                TackyBinaryOp::SubDouble => AsmBinaryOp::Sub,
                TackyBinaryOp::MulDouble => AsmBinaryOp::Mult,
                TackyBinaryOp::DivDouble => AsmBinaryOp::DivDouble,
                _ => unreachable!(),
            };
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Double,
                src: left_op,
                dst: dst_op.clone(),
            });
            instrs.push(Instruction::Binary {
                asm_type: AsmType::Double,
                op: asm_op,
                src: right_op,
                dst: dst_op,
            });
        }

        TackyBinaryOp::AddFloat
        | TackyBinaryOp::SubFloat
        | TackyBinaryOp::MulFloat
        | TackyBinaryOp::DivFloat => {
            let left_op = load_double_val(
                left,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                const_counter,
            );
            let right_op = load_double_val(
                right,
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                const_counter,
            );
            let asm_op = match op {
                TackyBinaryOp::AddFloat => AsmBinaryOp::Add,
                TackyBinaryOp::SubFloat => AsmBinaryOp::Sub,
                TackyBinaryOp::MulFloat => AsmBinaryOp::Mult,
                TackyBinaryOp::DivFloat => AsmBinaryOp::DivDouble,
                _ => unreachable!(),
            };
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Float,
                src: left_op,
                dst: dst_op.clone(),
            });
            instrs.push(Instruction::Binary {
                asm_type: AsmType::Float,
                op: asm_op,
                src: right_op,
                dst: dst_op,
            });
        }

        TackyBinaryOp::Equal
        | TackyBinaryOp::NotEqual
        | TackyBinaryOp::LessThan
        | TackyBinaryOp::LessOrEqual
        | TackyBinaryOp::GreaterThan
        | TackyBinaryOp::GreaterOrEqual => {
            let is_fp = left_type.is_floating();
            let is_unsigned = left_type.is_unsigned() || left_type.is_pointer();

            if is_fp {
                let fp_asm_type = if left_type.is_float() {
                    AsmType::Float
                } else {
                    AsmType::Double
                };
                let left_op = load_double_val(
                    left,
                    static_vars,
                    stack_vars,
                    instrs,
                    double_constants,
                    const_counter,
                );
                let right_op = load_double_val(
                    right,
                    static_vars,
                    stack_vars,
                    instrs,
                    double_constants,
                    const_counter,
                );
                // NaN 対応: < と <= ではオペランドを入れ替えて A/AE を使う
                // comiss/comisd の unordered (NaN) は CF=1,ZF=1,PF=1
                // A (CF=0,ZF=0) と AE (CF=0) は NaN で false → C セマンティクスに一致
                let swap = matches!(op, TackyBinaryOp::LessThan | TackyBinaryOp::LessOrEqual);
                let (cmp_left, cmp_right) = if swap {
                    (right_op, left_op)
                } else {
                    (left_op, right_op)
                };
                instrs.push(Instruction::Mov {
                    asm_type: fp_asm_type,
                    src: cmp_left,
                    dst: Operand::Register(Reg::XMM14),
                });
                instrs.push(Instruction::Mov {
                    asm_type: fp_asm_type,
                    src: cmp_right,
                    dst: Operand::Register(Reg::XMM0),
                });
                instrs.push(Instruction::Cmp {
                    asm_type: fp_asm_type,
                    src: Operand::Register(Reg::XMM0),
                    dst: Operand::Register(Reg::XMM14),
                });
            } else {
                let asm_type = safe_asm_type(&left_type);
                let left_op = val_to_operand(left, static_vars, stack_vars)?;
                let right_op = val_to_operand(right, static_vars, stack_vars)?;
                // Chapter 20: Cmp right, left (directly on pseudos)
                instrs.push(Instruction::Cmp {
                    asm_type,
                    src: right_op,
                    dst: left_op,
                });
            }

            instrs.push(Instruction::Mov {
                asm_type: AsmType::Longword,
                src: Operand::Imm(0),
                dst: dst_op.clone(),
            });

            let cc = if is_fp {
                // FP: < と <= はオペランド入れ替え済み → A/AE を使用
                match op {
                    TackyBinaryOp::LessThan => CondCode::A,     // swapped
                    TackyBinaryOp::LessOrEqual => CondCode::AE, // swapped
                    TackyBinaryOp::GreaterThan => CondCode::A,
                    TackyBinaryOp::GreaterOrEqual => CondCode::AE,
                    TackyBinaryOp::Equal => CondCode::E,
                    TackyBinaryOp::NotEqual => CondCode::NE,
                    _ => unreachable!(),
                }
            } else if is_unsigned {
                match op {
                    TackyBinaryOp::LessThan => CondCode::B,
                    TackyBinaryOp::LessOrEqual => CondCode::BE,
                    TackyBinaryOp::GreaterThan => CondCode::A,
                    TackyBinaryOp::GreaterOrEqual => CondCode::AE,
                    TackyBinaryOp::Equal => CondCode::E,
                    TackyBinaryOp::NotEqual => CondCode::NE,
                    _ => unreachable!(),
                }
            } else {
                match op {
                    TackyBinaryOp::LessThan => CondCode::L,
                    TackyBinaryOp::LessOrEqual => CondCode::LE,
                    TackyBinaryOp::GreaterThan => CondCode::G,
                    TackyBinaryOp::GreaterOrEqual => CondCode::GE,
                    TackyBinaryOp::Equal => CondCode::E,
                    TackyBinaryOp::NotEqual => CondCode::NE,
                    _ => unreachable!(),
                }
            };

            instrs.push(Instruction::SetCC {
                condition: cc,
                operand: dst_op.clone(),
            });

            // FP の == と != は NaN (unordered) を追加チェック
            if is_fp {
                match op {
                    TackyBinaryOp::Equal => {
                        // sete dst; setnp %al; and %al, dst → NaN で false
                        instrs.push(Instruction::SetCC {
                            condition: CondCode::NP,
                            operand: Operand::Register(Reg::AX),
                        });
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Byte,
                            op: AsmBinaryOp::And,
                            src: Operand::Register(Reg::AX),
                            dst: dst_op,
                        });
                    }
                    TackyBinaryOp::NotEqual => {
                        // setne dst; setp %al; or %al, dst → NaN で true
                        instrs.push(Instruction::SetCC {
                            condition: CondCode::P,
                            operand: Operand::Register(Reg::AX),
                        });
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Byte,
                            op: AsmBinaryOp::Or,
                            src: Operand::Register(Reg::AX),
                            dst: dst_op,
                        });
                    }
                    _ => {}
                }
            }
        }

        TackyBinaryOp::Add | TackyBinaryOp::Subtract | TackyBinaryOp::Multiply => {
            let asm_type = safe_asm_type(&dst_type);
            let left_op = val_to_operand(left, static_vars, stack_vars)?;
            let right_op = val_to_operand(right, static_vars, stack_vars)?;

            let asm_op = match op {
                TackyBinaryOp::Add => AsmBinaryOp::Add,
                TackyBinaryOp::Subtract => AsmBinaryOp::Sub,
                TackyBinaryOp::Multiply => AsmBinaryOp::Mult,
                _ => unreachable!(),
            };
            // Chapter 20: Mov left→dst; Binary right,dst
            instrs.push(Instruction::Mov {
                asm_type,
                src: left_op,
                dst: dst_op.clone(),
            });
            instrs.push(Instruction::Binary {
                asm_type,
                op: asm_op,
                src: right_op,
                dst: dst_op,
            });
        }

        TackyBinaryOp::BitwiseAnd | TackyBinaryOp::BitwiseOr | TackyBinaryOp::BitwiseXor => {
            let asm_type = safe_asm_type(&dst_type);
            let left_op = val_to_operand(left, static_vars, stack_vars)?;
            let right_op = val_to_operand(right, static_vars, stack_vars)?;
            let asm_op = match op {
                TackyBinaryOp::BitwiseAnd => AsmBinaryOp::And,
                TackyBinaryOp::BitwiseOr => AsmBinaryOp::Or,
                TackyBinaryOp::BitwiseXor => AsmBinaryOp::BitXor,
                _ => unreachable!(),
            };
            instrs.push(Instruction::Mov {
                asm_type,
                src: left_op,
                dst: dst_op.clone(),
            });
            instrs.push(Instruction::Binary {
                asm_type,
                op: asm_op,
                src: right_op,
                dst: dst_op,
            });
        }

        TackyBinaryOp::ShiftLeft | TackyBinaryOp::ShiftRight => {
            let asm_type = safe_asm_type(&dst_type);
            let left_op = val_to_operand(left, static_vars, stack_vars)?;
            let right_op = val_to_operand(right, static_vars, stack_vars)?;
            let asm_op = if matches!(op, TackyBinaryOp::ShiftLeft) {
                AsmBinaryOp::Sal
            } else if dst_type.is_unsigned() {
                AsmBinaryOp::Shr
            } else {
                AsmBinaryOp::Sar
            };
            // Move left operand to dst
            instrs.push(Instruction::Mov {
                asm_type,
                src: left_op,
                dst: dst_op.clone(),
            });
            // Move shift amount to CX (shift uses CL)
            instrs.push(Instruction::Mov {
                asm_type,
                src: right_op,
                dst: Operand::Register(Reg::CX),
            });
            // Shift dst by CL
            instrs.push(Instruction::Binary {
                asm_type,
                op: asm_op,
                src: Operand::Register(Reg::CX),
                dst: dst_op,
            });
        }

        TackyBinaryOp::Divide | TackyBinaryOp::Remainder => {
            let asm_type = safe_asm_type(&dst_type);
            let left_op = val_to_operand(left, static_vars, stack_vars)?;
            let right_op = val_to_operand(right, static_vars, stack_vars)?;

            // Division must use hardcoded AX/DX
            instrs.push(Instruction::Mov {
                asm_type,
                src: left_op,
                dst: Operand::Register(Reg::AX),
            });

            if dst_type.is_unsigned() {
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::DX),
                });
                instrs.push(Instruction::Div {
                    asm_type,
                    operand: right_op,
                });
            } else {
                instrs.push(Instruction::SignExtend(asm_type));
                instrs.push(Instruction::Idiv {
                    asm_type,
                    operand: right_op,
                });
            }

            let result_reg = if matches!(op, TackyBinaryOp::Remainder) {
                Reg::DX
            } else {
                Reg::AX
            };
            instrs.push(Instruction::Mov {
                asm_type,
                src: Operand::Register(result_reg),
                dst: dst_op,
            });
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn generate_function_call(
    name: &str,
    args: &[TackyVal],
    dst: &TackyVal,
    dst_type: &Type,
    is_variadic: bool,
    static_vars: &HashMap<String, String>,
    stack_vars: &HashMap<String, i32>,
    instrs: &mut Vec<Instruction>,
    double_constants: &mut HashMap<u64, (String, usize, bool)>,
    const_counter: &mut usize,
    var_types: &HashMap<String, Type>,
) -> Result<()> {
    let arg_types: Vec<Type> = args.iter().map(|a| val_type(a, var_types)).collect();
    let typed_params: Vec<(Type, String)> = arg_types
        .iter()
        .map(|t| (t.clone(), String::new()))
        .collect();
    let call_class = classify_parameters(&typed_params);

    let stack_count = call_class.stack_count;
    let padding = if !stack_count.is_multiple_of(2) { 8 } else { 0 };

    if padding > 0 {
        instrs.push(Instruction::AllocateStack(padding));
    }

    let mut stack_arg_indices: Vec<usize> = Vec::new();
    let mut int_reg_args: Vec<(usize, usize)> = Vec::new();
    let mut xmm_reg_args: Vec<(usize, usize)> = Vec::new();
    for (i, (_, loc)) in call_class.locations.iter().enumerate() {
        match loc {
            ParamLocation::Stack(_) => stack_arg_indices.push(i),
            ParamLocation::IntReg(idx) => int_reg_args.push((i, *idx)),
            ParamLocation::XmmReg(idx) => xmm_reg_args.push((i, *idx)),
        }
    }

    // Push stack args in reverse order
    for &i in stack_arg_indices.iter().rev() {
        if arg_types[i].is_floating() {
            let src = load_double_val(
                &args[i],
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                const_counter,
            );
            let fp_asm = if arg_types[i].is_float() {
                AsmType::Float
            } else {
                AsmType::Double
            };
            instrs.push(Instruction::Mov {
                asm_type: fp_asm,
                src,
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::Push(Operand::Register(Reg::XMM0)));
        } else if matches!(arg_types[i], Type::VaList) {
            let src = val_to_operand(&args[i], static_vars, stack_vars)?;
            instrs.push(Instruction::Lea {
                src,
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
        } else {
            let src = val_to_operand(&args[i], static_vars, stack_vars)?;
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src,
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
        }
    }

    // Int register args
    for &(arg_idx, _) in int_reg_args.iter().rev() {
        let src = val_to_operand(&args[arg_idx], static_vars, stack_vars)?;
        // VaList はスタック割り当て構造体 — アドレスを渡す (Lea)
        if matches!(arg_types[arg_idx], Type::VaList) {
            instrs.push(Instruction::Lea {
                src,
                dst: Operand::Register(Reg::AX),
            });
        } else {
            let asm_type = safe_asm_type(&arg_types[arg_idx]);
            instrs.push(Instruction::Mov {
                asm_type,
                src,
                dst: Operand::Register(Reg::AX),
            });
        }
        instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
    }
    for &(_, reg_idx) in &int_reg_args {
        instrs.push(Instruction::Pop(Operand::Register(ARG_REGISTERS[reg_idx])));
    }

    // XMM register args
    if xmm_reg_args.len() == 1 {
        let (arg_idx, reg_idx) = xmm_reg_args[0];
        let src = load_double_val(
            &args[arg_idx],
            static_vars,
            stack_vars,
            instrs,
            double_constants,
            const_counter,
        );
        let fp_asm = if arg_types[arg_idx].is_float() {
            AsmType::Float
        } else {
            AsmType::Double
        };
        instrs.push(Instruction::Mov {
            asm_type: fp_asm,
            src,
            dst: Operand::Register(Reg::XMM0),
        });
        if reg_idx != 0 {
            instrs.push(Instruction::Mov {
                asm_type: fp_asm,
                src: Operand::Register(Reg::XMM0),
                dst: Operand::Register(XMM_ARG_REGISTERS[reg_idx]),
            });
        }
    } else if xmm_reg_args.len() > 1 {
        for &(arg_idx, _) in xmm_reg_args.iter().rev() {
            let src = load_double_val(
                &args[arg_idx],
                static_vars,
                stack_vars,
                instrs,
                double_constants,
                const_counter,
            );
            let fp_asm = if arg_types[arg_idx].is_float() {
                AsmType::Float
            } else {
                AsmType::Double
            };
            instrs.push(Instruction::Mov {
                asm_type: fp_asm,
                src,
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::Push(Operand::Register(Reg::XMM0)));
        }
        for &(_, reg_idx) in &xmm_reg_args {
            instrs.push(Instruction::Pop(Operand::Register(
                XMM_ARG_REGISTERS[reg_idx],
            )));
        }
    }

    // System V ABI: 可変長関数では %al に XMM レジスタ引数の数をセット
    if is_variadic {
        let xmm_count = xmm_reg_args.len() as i64;
        instrs.push(Instruction::Mov {
            asm_type: AsmType::Longword,
            src: Operand::Imm(xmm_count),
            dst: Operand::Register(Reg::AX),
        });
    }

    // Indirect call if name refers to a function pointer variable.
    // Check both existence AND that the type is actually a function pointer
    // (avoids false hits from local variables that shadow function names).
    let is_fn_ptr_var = var_types.get(name).is_some_and(
        |ty| matches!(ty, Type::Pointer(inner) if matches!(inner.as_ref(), Type::Function { .. })),
    );
    if is_fn_ptr_var {
        let fn_ptr_op = val_to_operand(&TackyVal::Var(name.to_string()), static_vars, stack_vars)?;
        instrs.push(Instruction::Mov {
            asm_type: AsmType::Quadword,
            src: fn_ptr_op,
            dst: Operand::Register(Reg::R10),
        });
        instrs.push(Instruction::CallIndirect(Operand::Register(Reg::R10)));
    } else {
        instrs.push(Instruction::Call(name.to_string()));
    }

    let dealloc = stack_count * 8 + padding;
    if dealloc > 0 {
        instrs.push(Instruction::DeallocateStack(dealloc));
    }

    // Store result
    if !dst_type.is_void() {
        let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
        if dst_type.is_floating() {
            let fp_asm = if dst_type.is_float() {
                AsmType::Float
            } else {
                AsmType::Double
            };
            instrs.push(Instruction::Mov {
                asm_type: fp_asm,
                src: Operand::Register(Reg::XMM0),
                dst: dst_op,
            });
        } else if dst_type.is_struct() && dst_type.size() <= 16 {
            // System V ABI: struct ≤ 16 bytes returned in RAX (+ RDX)
            instrs.push(Instruction::Lea {
                src: dst_op.clone(),
                dst: Operand::Register(Reg::CX),
            });
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::AX),
                dst: Operand::Memory(Reg::CX),
            });
            if dst_type.size() > 8 {
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Quadword,
                    src: Operand::Register(Reg::DX),
                    dst: Operand::MemoryOffset(Reg::CX, 8),
                });
            }
        } else {
            let asm_type = safe_asm_type(dst_type);
            instrs.push(Instruction::Mov {
                asm_type,
                src: Operand::Register(Reg::AX),
                dst: dst_op,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    // Tests will be updated after full integration
}
