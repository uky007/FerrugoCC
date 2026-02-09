//! コード生成器（Code Generator）
//!
//! C の AST を走査し、対応する x86-64 アセンブリ命令列に変換する。
//!
//! # 変換規則
//!
//! ## 式の評価（結果は常に `%eax` に格納される）
//!
//! | C の式 | 生成される命令列 |
//! |--------|-----------------|
//! | `42` (定数) | `movl $42, %eax` |
//! | `-expr` (否定) | `<expr の命令列>` → `negl %eax` |
//! | `~expr` (ビット反転) | `<expr の命令列>` → `notl %eax` |
//! | `!expr` (論理否定) | `<expr の命令列>` → `cmpl $0, %eax` → `movl $0, %eax` → `sete %al` |
//! | `a` (変数参照) | `movl offset(%rbp), %eax` または `movl name(%rip), %eax` |
//! | `a = expr` (代入) | `<expr の命令列>` → `movl %eax, offset(%rbp)` |
//! | `a += expr` (複合代入) | 現在値ロード → rhs評価 → 演算 → 格納 (Chapter 7) |
//! | `++a` (前置++) | `movl offset(%rbp), %eax` → `addl $1, %eax` → `movl %eax, offset(%rbp)` |
//! | `a++` (後置++) | 旧値を %eax に保持し、+1 した新値を変数に格納 (Chapter 7) |
//! | `a, b` (カンマ) | 左辺を評価（捨てる）→ 右辺を評価（結果が %eax に残る） (Chapter 7) |
//!
//! ## 論理否定 `!` の仕組み
//! `!x` は「x が 0 なら 1、非0 なら 0」を返す。アセンブリでは:
//! 1. `cmpl $0, %eax` — EAX と 0 を比較（結果はフラグレジスタに格納）
//! 2. `movl $0, %eax` — EAX をゼロクリア（フラグに影響しない）
//! 3. `sete %al` — ZF=1（つまり EAX が 0 だった）なら AL=1、そうでなければ AL=0
//!
//! ポイント: `movl` でフラグを壊さないことがこのパターンの鍵。
//! `xorl %eax, %eax` のほうが速いが、フラグを変更してしまうので使えない。
//!
//! ## Chapter 10: ファイルスコープ変数と static/extern
//!
//! ファイルスコープの変数はデータセクション（`.data` / `.bss`）に配置される。
//! `static` 変数は内部リンケージ（`global: false`）、通常の変数は外部リンケージ（`global: true`）。
//! ブロックスコープの `static` 変数はユニークラベルで静的領域に配置される。
//! `extern` 変数は定義が別の翻訳単位にあることを示し、初期化子を持てない。

use std::collections::{HashMap, HashSet};
use crate::error::{CompileError, Result};
use crate::parse::ast::{Program, FunctionDecl, BlockItem, Declaration, Statement, Expr, UnaryOp, BinaryOp, ForInit, StorageClass, TopLevelDecl};
use super::asm_ast::{
    AsmProgram, AsmFunction, AsmStaticVar, Instruction, Operand, Reg, AsmUnaryOp, AsmBinaryOp, CondCode,
};

/// ループ内の break/continue ジャンプ先ラベル（Chapter 8）。
struct LoopLabels {
    break_label: String,
    continue_label: String,
}

/// 関数シンボルテーブル用の情報（Chapter 9）。
struct FunctionInfo {
    param_count: usize,
    defined: bool,
}

/// 変数の格納場所（Chapter 10）。
///
/// ローカル変数はスタック上（`%rbp` 相対オフセット）、
/// グローバル変数や static 変数はデータセクション（RIP 相対ラベル）に配置される。
#[derive(Debug, Clone)]
enum VarLocation {
    Stack(i32),
    Static(String),
}

/// 引数レジスタの順序（System V AMD64 ABI）: %edi, %esi, %edx, %ecx, %r8d, %r9d
const ARG_REGISTERS: [Reg; 6] = [Reg::DI, Reg::SI, Reg::DX, Reg::CX, Reg::R8, Reg::R9];

/// 変数名からオペランドを解決するヘルパー関数（Chapter 10）。
///
/// ローカル変数マップ → グローバル変数マップの順に検索し、
/// `Stack` なら `Operand::Stack`、`Static` なら `Operand::Data` を返す。
fn resolve_var(
    var_map: &HashMap<String, VarLocation>,
    global_var_map: &HashMap<String, VarLocation>,
    name: &str,
) -> Result<Operand> {
    if let Some(loc) = var_map.get(name) {
        match loc {
            VarLocation::Stack(offset) => Ok(Operand::Stack(*offset)),
            VarLocation::Static(label) => Ok(Operand::Data(label.clone())),
        }
    } else if let Some(loc) = global_var_map.get(name) {
        match loc {
            VarLocation::Stack(_) => unreachable!("global var should not be on stack"),
            VarLocation::Static(label) => Ok(Operand::Data(label.clone())),
        }
    } else {
        Err(CompileError::CodegenError(format!(
            "undeclared variable '{}'", name
        )))
    }
}

/// C の AST をアセンブリ AST に変換する（Chapter 10: ファイルスコープ変数対応）。
pub fn generate(program: &Program) -> Result<AsmProgram> {
    let mut label_counter = 0;
    let mut static_label_counter: usize = 0;

    // バリデーション: 関数シンボルテーブルを構築
    let mut func_table: HashMap<String, FunctionInfo> = HashMap::new();
    // グローバル変数マップ: 名前 → VarLocation::Static(ラベル)
    let mut global_var_map: HashMap<String, VarLocation> = HashMap::new();
    // 静的変数リスト
    let mut static_vars: Vec<AsmStaticVar> = Vec::new();

    for decl in &program.declarations {
        match decl {
            TopLevelDecl::Function(func_decl) => {
                let has_body = func_decl.body.is_some();
                if let Some(existing) = func_table.get(&func_decl.name) {
                    // パラメータ数の一貫性チェック
                    if existing.param_count != func_decl.params.len() {
                        return Err(CompileError::CodegenError(format!(
                            "conflicting parameter count for function '{}'", func_decl.name
                        )));
                    }
                    // 重複定義チェック
                    if has_body && existing.defined {
                        return Err(CompileError::CodegenError(format!(
                            "function '{}' defined multiple times", func_decl.name
                        )));
                    }
                }
                let entry = func_table.entry(func_decl.name.clone()).or_insert(FunctionInfo {
                    param_count: func_decl.params.len(),
                    defined: false,
                });
                if has_body {
                    entry.defined = true;
                }
            }
            TopLevelDecl::Variable(var_decl) => {
                // ファイルスコープ変数の処理
                let sc = var_decl.storage_class;

                // extern 変数は初期化子を持てない
                if sc == Some(StorageClass::Extern) && var_decl.init.is_some() {
                    return Err(CompileError::CodegenError(format!(
                        "extern variable '{}' cannot have initializer", var_decl.name
                    )));
                }

                // 初期化値: 定数式のみ許可
                let init_val = if let Some(init_expr) = &var_decl.init {
                    match init_expr {
                        Expr::Constant(v) => *v,
                        _ => {
                            return Err(CompileError::CodegenError(format!(
                                "file-scope variable '{}' must be initialized with a constant expression",
                                var_decl.name
                            )));
                        }
                    }
                } else {
                    0 // デフォルト初期化
                };

                // グローバル変数マップに登録
                global_var_map.insert(
                    var_decl.name.clone(),
                    VarLocation::Static(var_decl.name.clone()),
                );

                match sc {
                    Some(StorageClass::Extern) => {
                        // extern: 定義は別の翻訳単位。
                        // static_vars に未登録なら追加しない（定義が別にある前提）。
                        // ただし、他の宣言でまだ登録されていなければ登録だけ行う。
                        // → VarLocation::Static は上で登録済み。static_vars には追加しない。
                    }
                    Some(StorageClass::Static) => {
                        // static: 内部リンケージ（global: false）
                        static_vars.push(AsmStaticVar {
                            name: var_decl.name.clone(),
                            global: false,
                            init: init_val,
                        });
                    }
                    None => {
                        // 通常の変数: 外部リンケージ（global: true）
                        static_vars.push(AsmStaticVar {
                            name: var_decl.name.clone(),
                            global: true,
                            init: init_val,
                        });
                    }
                }
            }
        }
    }

    // body がある関数のみコード生成
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
                )?);
            }
        }
    }

    Ok(AsmProgram { functions, static_vars })
}

/// 関数の変換: 本体のブロック要素列から命令列を生成する。
///
/// Chapter 5: 変数マップを使ってローカル変数のスタックオフセットを管理する。
/// Chapter 9: パラメータをレジスタからスタックに保存し、暗黙の return 0 を追加する。
/// Chapter 10: グローバル変数マップと静的変数リストを受け取る。
fn generate_function(
    func: &FunctionDecl,
    label_counter: &mut usize,
    func_table: &HashMap<String, FunctionInfo>,
    global_var_map: &HashMap<String, VarLocation>,
    static_vars: &mut Vec<AsmStaticVar>,
    static_label_counter: &mut usize,
    global: bool,
) -> Result<AsmFunction> {
    let mut var_map: HashMap<String, VarLocation> = HashMap::new();
    let mut next_offset: i32 = -4;
    let mut instructions = Vec::new();

    // パラメータをスタックに割り当て
    for (i, param_name) in func.params.iter().enumerate() {
        let offset = next_offset;
        var_map.insert(param_name.clone(), VarLocation::Stack(offset));
        next_offset -= 4;

        if i < 6 {
            // レジスタからスタックに保存
            instructions.push(Instruction::Mov {
                src: Operand::Register(ARG_REGISTERS[i]),
                dst: Operand::Stack(offset),
            });
        } else {
            // スタック引数: 16(%rbp) + (i - 6) * 8 からロード
            let stack_arg_offset = 16 + ((i - 6) as i32) * 8;
            instructions.push(Instruction::Mov {
                src: Operand::Stack(stack_arg_offset),
                dst: Operand::Register(Reg::AX),
            });
            instructions.push(Instruction::Mov {
                src: Operand::Register(Reg::AX),
                dst: Operand::Stack(offset),
            });
        }
    }

    let body = func.body.as_ref().unwrap();
    let mut scope_decls: HashSet<String> = HashSet::new();
    // パラメータ名をスコープ宣言に追加（重複宣言チェック用）
    for param_name in &func.params {
        scope_decls.insert(param_name.clone());
    }
    for item in body {
        let instrs = generate_block_item(
            item, &mut var_map, &mut next_offset, label_counter,
            Some(&mut scope_decls), None, func_table, global_var_map,
            static_vars, static_label_counter,
        )?;
        instructions.extend(instrs);
    }

    // 暗黙の return 0 を追加（未定義動作の回避）
    instructions.push(Instruction::Mov {
        src: Operand::Imm(0),
        dst: Operand::Register(Reg::AX),
    });
    instructions.push(Instruction::Ret);

    // AllocateStack を先頭に挿入（16バイトアラインメント）
    let total_bytes = ((-next_offset - 4) / 4) as usize * 4;
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

/// ブロック要素の変換。
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
) -> Result<Vec<Instruction>> {
    match item {
        BlockItem::Statement(stmt) => generate_statement(
            stmt, var_map, next_offset, label_counter, loop_labels,
            func_table, global_var_map, static_vars, static_label_counter,
        ),
        BlockItem::Declaration(decl) => generate_declaration(
            decl, var_map, next_offset, label_counter, scope_decls,
            func_table, global_var_map, static_vars, static_label_counter,
        ),
    }
}

/// 宣言の変換（Chapter 10: static/extern 対応）。
///
/// 変数をスタックに割り当て、初期化式がある場合はその値を格納する。
/// `static` 変数はユニークラベルで静的領域に配置される。
/// `extern` 変数は定義が別の翻訳単位にあることを宣言する。
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
) -> Result<Vec<Instruction>> {
    let sc = decl.storage_class;

    // ブロックスコープの extern 変数
    if sc == Some(StorageClass::Extern) {
        // extern は初期化子を持てない
        if decl.init.is_some() {
            return Err(CompileError::CodegenError(format!(
                "extern variable '{}' cannot have initializer", decl.name
            )));
        }
        // var_map に Static(name) として登録（定義は別の翻訳単位）
        var_map.insert(decl.name.clone(), VarLocation::Static(decl.name.clone()));
        return Ok(Vec::new());
    }

    // ブロックスコープの static 変数
    if sc == Some(StorageClass::Static) {
        // 初期化値: 定数式のみ許可
        let init_val = if let Some(init_expr) = &decl.init {
            match init_expr {
                Expr::Constant(v) => *v,
                _ => {
                    return Err(CompileError::CodegenError(format!(
                        "static variable '{}' must be initialized with a constant expression",
                        decl.name
                    )));
                }
            }
        } else {
            0 // デフォルト初期化
        };

        // ユニークラベルを生成
        let unique_label = format!("{}.{}", decl.name, *static_label_counter);
        *static_label_counter += 1;

        // 静的変数リストに追加（ブロックスコープ static は内部リンケージ）
        static_vars.push(AsmStaticVar {
            name: unique_label.clone(),
            global: false,
            init: init_val,
        });

        // var_map に Static(unique_label) として登録
        var_map.insert(decl.name.clone(), VarLocation::Static(unique_label));

        // ランタイム初期化コードは不要
        return Ok(Vec::new());
    }

    // 通常のローカル変数（スタック割り当て）

    // 同一スコープ内の重複宣言チェック
    if let Some(decls) = scope_decls {
        if !decls.insert(decl.name.clone()) {
            return Err(CompileError::CodegenError(format!(
                "variable '{}' already declared in this scope", decl.name
            )));
        }
    }

    let offset = *next_offset;
    var_map.insert(decl.name.clone(), VarLocation::Stack(offset));
    *next_offset -= 4;

    let mut instrs = Vec::new();
    if let Some(init) = &decl.init {
        instrs.extend(generate_expr(init, var_map, label_counter, func_table, global_var_map)?);
        instrs.push(Instruction::Mov {
            src: Operand::Register(Reg::AX),
            dst: Operand::Stack(offset),
        });
    }

    Ok(instrs)
}

/// 文の変換。
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
) -> Result<Vec<Instruction>> {
    match stmt {
        Statement::Return(expr) => {
            let mut instrs = generate_expr(expr, var_map, label_counter, func_table, global_var_map)?;
            instrs.push(Instruction::Ret);
            Ok(instrs)
        }
        Statement::Expression(expr) => {
            generate_expr(expr, var_map, label_counter, func_table, global_var_map)
        }
        Statement::Null => {
            Ok(Vec::new())
        }
        Statement::If { condition, then_branch, else_branch } => {
            let n = *label_counter;
            *label_counter += 1;

            let mut instrs = generate_expr(condition, var_map, label_counter, func_table, global_var_map)?;
            instrs.push(Instruction::Cmp {
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            });

            if let Some(else_stmt) = else_branch {
                let else_label = format!(".Lif_else{n}");
                let end_label = format!(".Lif_end{n}");

                instrs.push(Instruction::JmpCC(CondCode::E, else_label.clone()));
                instrs.extend(generate_statement(
                    then_branch, var_map, next_offset, label_counter, loop_labels,
                    func_table, global_var_map, static_vars, static_label_counter,
                )?);
                instrs.push(Instruction::Jmp(end_label.clone()));
                instrs.push(Instruction::Label(else_label));
                instrs.extend(generate_statement(
                    else_stmt, var_map, next_offset, label_counter, loop_labels,
                    func_table, global_var_map, static_vars, static_label_counter,
                )?);
                instrs.push(Instruction::Label(end_label));
            } else {
                let end_label = format!(".Lif_end{n}");
                instrs.push(Instruction::JmpCC(CondCode::E, end_label.clone()));
                instrs.extend(generate_statement(
                    then_branch, var_map, next_offset, label_counter, loop_labels,
                    func_table, global_var_map, static_vars, static_label_counter,
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
                    static_vars, static_label_counter,
                )?);
            }
            Ok(instrs)
        }
        // Chapter 8: while ループ
        Statement::While { condition, body } => {
            let n = *label_counter;
            *label_counter += 1;
            let start_label = format!(".Lwhile_start{n}");
            let end_label = format!(".Lwhile_end{n}");

            let labels = LoopLabels {
                break_label: end_label.clone(),
                continue_label: start_label.clone(),
            };

            let mut instrs = vec![Instruction::Label(start_label.clone())];
            instrs.extend(generate_expr(condition, var_map, label_counter, func_table, global_var_map)?);
            instrs.push(Instruction::Cmp {
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::JmpCC(CondCode::E, end_label.clone()));
            instrs.extend(generate_statement(
                body, var_map, next_offset, label_counter, Some(&labels),
                func_table, global_var_map, static_vars, static_label_counter,
            )?);
            instrs.push(Instruction::Jmp(start_label));
            instrs.push(Instruction::Label(end_label));
            Ok(instrs)
        }
        // Chapter 8: do-while ループ
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

            let mut instrs = vec![Instruction::Label(start_label.clone())];
            instrs.extend(generate_statement(
                body, var_map, next_offset, label_counter, Some(&labels),
                func_table, global_var_map, static_vars, static_label_counter,
            )?);
            instrs.push(Instruction::Label(continue_label));
            instrs.extend(generate_expr(condition, var_map, label_counter, func_table, global_var_map)?);
            instrs.push(Instruction::Cmp {
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::JmpCC(CondCode::NE, start_label));
            instrs.push(Instruction::Label(end_label));
            Ok(instrs)
        }
        // Chapter 8: for ループ
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

            // for の init が宣言の場合、新しいスコープが必要
            let mut inner_map = var_map.clone();
            let map_ref: &mut HashMap<String, VarLocation> = match init {
                ForInit::Declaration(_) => &mut inner_map,
                ForInit::Expression(_) => var_map,
            };

            let mut instrs = Vec::new();

            // init
            match init {
                ForInit::Declaration(decl) => {
                    instrs.extend(generate_declaration(
                        decl, map_ref, next_offset, label_counter, None,
                        func_table, global_var_map, static_vars, static_label_counter,
                    )?);
                }
                ForInit::Expression(Some(expr)) => {
                    instrs.extend(generate_expr(expr, map_ref, label_counter, func_table, global_var_map)?);
                }
                ForInit::Expression(None) => {}
            }

            instrs.push(Instruction::Label(start_label.clone()));

            // condition
            if let Some(cond) = condition {
                instrs.extend(generate_expr(cond, map_ref, label_counter, func_table, global_var_map)?);
                instrs.push(Instruction::Cmp {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::JmpCC(CondCode::E, end_label.clone()));
            }

            // body
            instrs.extend(generate_statement(
                body, map_ref, next_offset, label_counter, Some(&labels),
                func_table, global_var_map, static_vars, static_label_counter,
            )?);

            // continue target + post
            instrs.push(Instruction::Label(continue_label));
            if let Some(post_expr) = post {
                instrs.extend(generate_expr(post_expr, map_ref, label_counter, func_table, global_var_map)?);
            }

            instrs.push(Instruction::Jmp(start_label));
            instrs.push(Instruction::Label(end_label));
            Ok(instrs)
        }
        // Chapter 8: break
        Statement::Break => {
            let labels = loop_labels.ok_or_else(|| {
                CompileError::CodegenError("break outside loop".to_string())
            })?;
            Ok(vec![Instruction::Jmp(labels.break_label.clone())])
        }
        // Chapter 8: continue
        Statement::Continue => {
            let labels = loop_labels.ok_or_else(|| {
                CompileError::CodegenError("continue outside loop".to_string())
            })?;
            Ok(vec![Instruction::Jmp(labels.continue_label.clone())])
        }
    }
}

/// 式の変換（Chapter 5, 9, 10 で拡張）。
///
/// すべての式は、評価後に結果が `%eax` に入るような命令列を生成する。
fn generate_expr(
    expr: &Expr,
    var_map: &HashMap<String, VarLocation>,
    label_counter: &mut usize,
    func_table: &HashMap<String, FunctionInfo>,
    global_var_map: &HashMap<String, VarLocation>,
) -> Result<Vec<Instruction>> {
    match expr {
        // 定数: 即値を %eax にロードする
        Expr::Constant(value) => {
            Ok(vec![Instruction::Mov {
                src: Operand::Imm(*value),
                dst: Operand::Register(Reg::AX),
            }])
        }

        // 変数参照: スタックまたはデータセクションから %eax にロードする
        Expr::Var(name) => {
            let operand = resolve_var(var_map, global_var_map, name)?;
            Ok(vec![Instruction::Mov {
                src: operand,
                dst: Operand::Register(Reg::AX),
            }])
        }

        // 代入: 右辺を評価して結果を格納（%eax にも結果が残る）
        Expr::Assign(name, rhs) => {
            let dst_operand = resolve_var(var_map, global_var_map, name)?;
            let mut instrs = generate_expr(rhs, var_map, label_counter, func_table, global_var_map)?;
            instrs.push(Instruction::Mov {
                src: Operand::Register(Reg::AX),
                dst: dst_operand,
            });
            Ok(instrs)
        }

        // 単項演算: まず内側の式を評価（結果は %eax）、その後に演算を適用
        Expr::Unary(op, inner) => {
            match op {
                UnaryOp::PreIncrement | UnaryOp::PreDecrement => {
                    // 前置 ++/--: 変数をロード → 加減算 → 格納 → 新値が %eax に残る
                    if let Expr::Var(name) = inner.as_ref() {
                        let operand = resolve_var(var_map, global_var_map, name)?;
                        let mut instrs = vec![
                            Instruction::Mov {
                                src: operand.clone(),
                                dst: Operand::Register(Reg::AX),
                            },
                        ];
                        if matches!(op, UnaryOp::PreIncrement) {
                            instrs.push(Instruction::Binary {
                                op: AsmBinaryOp::Add,
                                src: Operand::Imm(1),
                                dst: Operand::Register(Reg::AX),
                            });
                        } else {
                            instrs.push(Instruction::Binary {
                                op: AsmBinaryOp::Sub,
                                src: Operand::Imm(1),
                                dst: Operand::Register(Reg::AX),
                            });
                        }
                        instrs.push(Instruction::Mov {
                            src: Operand::Register(Reg::AX),
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
                    let mut instrs = generate_expr(inner, var_map, label_counter, func_table, global_var_map)?;
                    match op {
                        UnaryOp::Negate => {
                            instrs.push(Instruction::Unary {
                                op: AsmUnaryOp::Neg,
                                operand: Operand::Register(Reg::AX),
                            });
                        }
                        UnaryOp::Complement => {
                            instrs.push(Instruction::Unary {
                                op: AsmUnaryOp::Not,
                                operand: Operand::Register(Reg::AX),
                            });
                        }
                        UnaryOp::Not => {
                            instrs.push(Instruction::Cmp {
                                src: Operand::Imm(0),
                                dst: Operand::Register(Reg::AX),
                            });
                            instrs.push(Instruction::Mov {
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

        // 三項演算子（Chapter 6）
        Expr::Conditional { condition, then_expr, else_expr } => {
            let n = *label_counter;
            *label_counter += 1;
            let else_label = format!(".Ltern_else{n}");
            let end_label = format!(".Ltern_end{n}");

            let mut instrs = generate_expr(condition, var_map, label_counter, func_table, global_var_map)?;
            instrs.push(Instruction::Cmp {
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::JmpCC(CondCode::E, else_label.clone()));
            instrs.extend(generate_expr(then_expr, var_map, label_counter, func_table, global_var_map)?);
            instrs.push(Instruction::Jmp(end_label.clone()));
            instrs.push(Instruction::Label(else_label));
            instrs.extend(generate_expr(else_expr, var_map, label_counter, func_table, global_var_map)?);
            instrs.push(Instruction::Label(end_label));
            Ok(instrs)
        }

        // 複合代入（Chapter 7）: a += 5 → a = a + 5 相当
        Expr::CompoundAssign(op, name, rhs) => {
            let var_operand = resolve_var(var_map, global_var_map, name)?;

            match op {
                BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply => {
                    let mut instrs = vec![
                        Instruction::Mov {
                            src: var_operand.clone(),
                            dst: Operand::Register(Reg::AX),
                        },
                        Instruction::Push(Operand::Register(Reg::AX)),
                    ];
                    instrs.extend(generate_expr(rhs, var_map, label_counter, func_table, global_var_map)?);
                    instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                    let asm_op = match op {
                        BinaryOp::Add => AsmBinaryOp::Add,
                        BinaryOp::Subtract => AsmBinaryOp::Sub,
                        BinaryOp::Multiply => AsmBinaryOp::Mult,
                        _ => unreachable!(),
                    };
                    if matches!(op, BinaryOp::Subtract) {
                        instrs.push(Instruction::Binary {
                            op: asm_op,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::CX),
                        });
                        instrs.push(Instruction::Mov {
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                    } else {
                        instrs.push(Instruction::Binary {
                            op: asm_op,
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                    }
                    instrs.push(Instruction::Mov {
                        src: Operand::Register(Reg::AX),
                        dst: var_operand,
                    });
                    Ok(instrs)
                }
                BinaryOp::Divide | BinaryOp::Remainder => {
                    let mut instrs = generate_expr(rhs, var_map, label_counter, func_table, global_var_map)?;
                    instrs.push(Instruction::Mov {
                        src: Operand::Register(Reg::AX),
                        dst: Operand::Register(Reg::CX),
                    });
                    instrs.push(Instruction::Mov {
                        src: var_operand.clone(),
                        dst: Operand::Register(Reg::AX),
                    });
                    instrs.push(Instruction::Cdq);
                    instrs.push(Instruction::Idiv(Operand::Register(Reg::CX)));
                    if matches!(op, BinaryOp::Remainder) {
                        instrs.push(Instruction::Mov {
                            src: Operand::Register(Reg::DX),
                            dst: Operand::Register(Reg::AX),
                        });
                    }
                    instrs.push(Instruction::Mov {
                        src: Operand::Register(Reg::AX),
                        dst: var_operand,
                    });
                    Ok(instrs)
                }
                _ => Err(CompileError::CodegenError(format!(
                    "unsupported compound assignment operator: {:?}", op
                ))),
            }
        }

        // 後置インクリメント（Chapter 7）: a++ → 旧値を返し、変数を +1
        Expr::PostfixIncrement(name) => {
            let operand = resolve_var(var_map, global_var_map, name)?;
            Ok(vec![
                Instruction::Mov {
                    src: operand.clone(),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Mov {
                    src: Operand::Register(Reg::AX),
                    dst: Operand::Register(Reg::CX),
                },
                Instruction::Binary {
                    op: AsmBinaryOp::Add,
                    src: Operand::Imm(1),
                    dst: Operand::Register(Reg::CX),
                },
                Instruction::Mov {
                    src: Operand::Register(Reg::CX),
                    dst: operand,
                },
            ])
        }

        // 後置デクリメント（Chapter 7）: a-- → 旧値を返し、変数を -1
        Expr::PostfixDecrement(name) => {
            let operand = resolve_var(var_map, global_var_map, name)?;
            Ok(vec![
                Instruction::Mov {
                    src: operand.clone(),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Mov {
                    src: Operand::Register(Reg::AX),
                    dst: Operand::Register(Reg::CX),
                },
                Instruction::Binary {
                    op: AsmBinaryOp::Sub,
                    src: Operand::Imm(1),
                    dst: Operand::Register(Reg::CX),
                },
                Instruction::Mov {
                    src: Operand::Register(Reg::CX),
                    dst: operand,
                },
            ])
        }

        // 関数呼び出し（Chapter 9）
        Expr::FunctionCall(name, args) => {
            // 引数数チェック（既知の関数の場合）
            if let Some(info) = func_table.get(name) {
                if info.param_count != args.len() {
                    return Err(CompileError::CodegenError(format!(
                        "function '{}' expects {} arguments, got {}",
                        name, info.param_count, args.len()
                    )));
                }
            }

            let arg_count = args.len();
            let stack_args = if arg_count > 6 { arg_count - 6 } else { 0 };
            let padding = if stack_args % 2 != 0 { 8 } else { 0 };

            let mut instrs = Vec::new();

            // 16バイトアラインメント用パディング
            if padding > 0 {
                instrs.push(Instruction::AllocateStack(padding));
            }

            // 全引数を右から左に評価し、各結果をスタックにプッシュ
            for arg in args.iter().rev() {
                instrs.extend(generate_expr(arg, var_map, label_counter, func_table, global_var_map)?);
                instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
            }

            // 先頭 min(N, 6) 個をレジスタにポップ
            let reg_args = std::cmp::min(arg_count, 6);
            for i in 0..reg_args {
                instrs.push(Instruction::Pop(Operand::Register(ARG_REGISTERS[i])));
            }

            // call
            instrs.push(Instruction::Call(name.clone()));

            // スタック引数の解放
            let dealloc = stack_args * 8 + padding;
            if dealloc > 0 {
                instrs.push(Instruction::DeallocateStack(dealloc));
            }

            Ok(instrs)
        }

        // 二項演算（Chapter 3-4, 7）
        Expr::Binary(op, left, right) => {
            match op {
                // ── 論理AND（短絡評価）──
                BinaryOp::LogicalAnd => {
                    let n = *label_counter;
                    *label_counter += 1;
                    let false_label = format!(".Land_false{n}");
                    let end_label = format!(".Land_end{n}");

                    let mut instrs = generate_expr(left, var_map, label_counter, func_table, global_var_map)?;
                    instrs.push(Instruction::Cmp {
                        src: Operand::Imm(0),
                        dst: Operand::Register(Reg::AX),
                    });
                    instrs.push(Instruction::JmpCC(CondCode::E, false_label.clone()));

                    instrs.extend(generate_expr(right, var_map, label_counter, func_table, global_var_map)?);
                    instrs.push(Instruction::Cmp {
                        src: Operand::Imm(0),
                        dst: Operand::Register(Reg::AX),
                    });
                    instrs.push(Instruction::JmpCC(CondCode::E, false_label.clone()));

                    instrs.push(Instruction::Mov {
                        src: Operand::Imm(1),
                        dst: Operand::Register(Reg::AX),
                    });
                    instrs.push(Instruction::Jmp(end_label.clone()));

                    instrs.push(Instruction::Label(false_label));
                    instrs.push(Instruction::Mov {
                        src: Operand::Imm(0),
                        dst: Operand::Register(Reg::AX),
                    });

                    instrs.push(Instruction::Label(end_label));
                    Ok(instrs)
                }

                // ── 論理OR（短絡評価）──
                BinaryOp::LogicalOr => {
                    let n = *label_counter;
                    *label_counter += 1;
                    let true_label = format!(".Lor_true{n}");
                    let end_label = format!(".Lor_end{n}");

                    let mut instrs = generate_expr(left, var_map, label_counter, func_table, global_var_map)?;
                    instrs.push(Instruction::Cmp {
                        src: Operand::Imm(0),
                        dst: Operand::Register(Reg::AX),
                    });
                    instrs.push(Instruction::JmpCC(CondCode::NE, true_label.clone()));

                    instrs.extend(generate_expr(right, var_map, label_counter, func_table, global_var_map)?);
                    instrs.push(Instruction::Cmp {
                        src: Operand::Imm(0),
                        dst: Operand::Register(Reg::AX),
                    });
                    instrs.push(Instruction::JmpCC(CondCode::NE, true_label.clone()));

                    instrs.push(Instruction::Mov {
                        src: Operand::Imm(0),
                        dst: Operand::Register(Reg::AX),
                    });
                    instrs.push(Instruction::Jmp(end_label.clone()));

                    instrs.push(Instruction::Label(true_label));
                    instrs.push(Instruction::Mov {
                        src: Operand::Imm(1),
                        dst: Operand::Register(Reg::AX),
                    });

                    instrs.push(Instruction::Label(end_label));
                    Ok(instrs)
                }

                // ── 関係・等価演算子（cmpl + setCC パターン）──
                BinaryOp::LessThan | BinaryOp::LessEqual
                | BinaryOp::GreaterThan | BinaryOp::GreaterEqual
                | BinaryOp::Equal | BinaryOp::NotEqual => {
                    let mut instrs = generate_expr(left, var_map, label_counter, func_table, global_var_map)?;
                    instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                    instrs.extend(generate_expr(right, var_map, label_counter, func_table, global_var_map)?);
                    instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                    instrs.push(Instruction::Cmp {
                        src: Operand::Register(Reg::AX),
                        dst: Operand::Register(Reg::CX),
                    });
                    instrs.push(Instruction::Mov {
                        src: Operand::Imm(0),
                        dst: Operand::Register(Reg::AX),
                    });
                    let cc = match op {
                        BinaryOp::LessThan => CondCode::L,
                        BinaryOp::LessEqual => CondCode::LE,
                        BinaryOp::GreaterThan => CondCode::G,
                        BinaryOp::GreaterEqual => CondCode::GE,
                        BinaryOp::Equal => CondCode::E,
                        BinaryOp::NotEqual => CondCode::NE,
                        _ => unreachable!(),
                    };
                    instrs.push(Instruction::SetCC {
                        condition: cc,
                        operand: Operand::Register(Reg::AX),
                    });
                    Ok(instrs)
                }

                // ── 算術演算子（Chapter 3）──
                BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply => {
                    let mut instrs = generate_expr(left, var_map, label_counter, func_table, global_var_map)?;
                    instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                    instrs.extend(generate_expr(right, var_map, label_counter, func_table, global_var_map)?);

                    instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                    let asm_op = match op {
                        BinaryOp::Add => AsmBinaryOp::Add,
                        BinaryOp::Subtract => AsmBinaryOp::Sub,
                        BinaryOp::Multiply => AsmBinaryOp::Mult,
                        _ => unreachable!(),
                    };
                    if matches!(op, BinaryOp::Subtract) {
                        instrs.push(Instruction::Binary {
                            op: asm_op,
                            src: Operand::Register(Reg::AX),
                            dst: Operand::Register(Reg::CX),
                        });
                        instrs.push(Instruction::Mov {
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                    } else {
                        instrs.push(Instruction::Binary {
                            op: asm_op,
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                    }
                    Ok(instrs)
                }

                // カンマ演算子（Chapter 7）: 左辺を評価して捨て、右辺の値を返す
                BinaryOp::Comma => {
                    let mut instrs = generate_expr(left, var_map, label_counter, func_table, global_var_map)?;
                    instrs.extend(generate_expr(right, var_map, label_counter, func_table, global_var_map)?);
                    Ok(instrs)
                }

                // 除算・剰余: idivl を使う
                BinaryOp::Divide | BinaryOp::Remainder => {
                    let mut instrs = generate_expr(left, var_map, label_counter, func_table, global_var_map)?;
                    instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                    instrs.extend(generate_expr(right, var_map, label_counter, func_table, global_var_map)?);

                    instrs.push(Instruction::Mov {
                        src: Operand::Register(Reg::AX),
                        dst: Operand::Register(Reg::CX),
                    });
                    instrs.push(Instruction::Pop(Operand::Register(Reg::AX)));
                    instrs.push(Instruction::Cdq);
                    instrs.push(Instruction::Idiv(Operand::Register(Reg::CX)));

                    if matches!(op, BinaryOp::Remainder) {
                        instrs.push(Instruction::Mov {
                            src: Operand::Register(Reg::DX),
                            dst: Operand::Register(Reg::AX),
                        });
                    }
                    Ok(instrs)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::ast::{FunctionDecl, StorageClass, TopLevelDecl};

    /// ヘルパー: 単純な関数宣言（storage_class: None）を TopLevelDecl::Function として包む
    fn func_decl(name: &str, params: Vec<&str>, body: Option<Vec<BlockItem>>) -> TopLevelDecl {
        TopLevelDecl::Function(FunctionDecl {
            name: name.to_string(),
            params: params.into_iter().map(String::from).collect(),
            body,
            storage_class: None,
        })
    }

    /// ヘルパー: ストレージクラス付き関数宣言を TopLevelDecl::Function として包む
    fn func_decl_with_sc(
        name: &str,
        params: Vec<&str>,
        body: Option<Vec<BlockItem>>,
        sc: Option<StorageClass>,
    ) -> TopLevelDecl {
        TopLevelDecl::Function(FunctionDecl {
            name: name.to_string(),
            params: params.into_iter().map(String::from).collect(),
            body,
            storage_class: sc,
        })
    }

    /// ヘルパー: 通常の変数宣言（storage_class: None）
    fn var_decl(name: &str, init: Option<Expr>) -> Declaration {
        Declaration {
            name: name.to_string(),
            init,
            storage_class: None,
        }
    }

    /// ヘルパー: ストレージクラス付き変数宣言
    fn var_decl_with_sc(name: &str, init: Option<Expr>, sc: Option<StorageClass>) -> Declaration {
        Declaration {
            name: name.to_string(),
            init,
            storage_class: sc,
        }
    }

    /// Chapter 1: `return 2` の場合
    #[test]
    fn generate_return_constant() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Constant(2))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.functions[0].name, "main");
        assert_eq!(asm.functions[0].global, true);
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov {
                    src: Operand::Imm(2),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 2: `return -5` → movl $5, %eax; negl %eax; ret
    #[test]
    fn generate_negation() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Unary(
                    UnaryOp::Negate,
                    Box::new(Expr::Constant(5)),
                ))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov {
                    src: Operand::Imm(5),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Unary {
                    op: AsmUnaryOp::Neg,
                    operand: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 2: `return ~0` → movl $0, %eax; notl %eax; ret
    #[test]
    fn generate_complement() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Unary(
                    UnaryOp::Complement,
                    Box::new(Expr::Constant(0)),
                ))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Unary {
                    op: AsmUnaryOp::Not,
                    operand: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 2: `return !1`
    #[test]
    fn generate_logical_not() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Unary(
                    UnaryOp::Not,
                    Box::new(Expr::Constant(1)),
                ))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov {
                    src: Operand::Imm(1),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Cmp {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Mov {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::SetCC {
                    condition: CondCode::E,
                    operand: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
            ]
        );
    }

    // ── Chapter 3 テスト ──

    /// Chapter 3: `return 1 + 2` → 加算
    #[test]
    fn generate_addition() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::Add,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                ))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Push(Operand::Register(Reg::AX)),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Pop(Operand::Register(Reg::CX)),
                Instruction::Binary {
                    op: AsmBinaryOp::Add,
                    src: Operand::Register(Reg::CX),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 3: `return 7 / 2` → 除算
    #[test]
    fn generate_division() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::Divide,
                    Box::new(Expr::Constant(7)),
                    Box::new(Expr::Constant(2)),
                ))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(7), dst: Operand::Register(Reg::AX) },
                Instruction::Push(Operand::Register(Reg::AX)),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Register(Reg::CX) },
                Instruction::Pop(Operand::Register(Reg::AX)),
                Instruction::Cdq,
                Instruction::Idiv(Operand::Register(Reg::CX)),
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
            ]
        );
    }

    // ── Chapter 4 テスト ──

    /// Chapter 4: `return 1 < 2` → cmpl + setl パターン
    #[test]
    fn generate_less_than() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::LessThan,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                ))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Push(Operand::Register(Reg::AX)),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Pop(Operand::Register(Reg::CX)),
                Instruction::Cmp { src: Operand::Register(Reg::AX), dst: Operand::Register(Reg::CX) },
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::SetCC { condition: CondCode::L, operand: Operand::Register(Reg::AX) },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 4: `return 1 && 2` → 短絡評価
    #[test]
    fn generate_logical_and() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::LogicalAnd,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                ))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::JmpCC(CondCode::E, ".Land_false0".to_string()),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::JmpCC(CondCode::E, ".Land_false0".to_string()),
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Jmp(".Land_end0".to_string()),
                Instruction::Label(".Land_false0".to_string()),
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Label(".Land_end0".to_string()),
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 4: `return 0 || 3` → 短絡評価
    #[test]
    fn generate_logical_or() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::LogicalOr,
                    Box::new(Expr::Constant(0)),
                    Box::new(Expr::Constant(3)),
                ))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::JmpCC(CondCode::NE, ".Lor_true0".to_string()),
                Instruction::Mov { src: Operand::Imm(3), dst: Operand::Register(Reg::AX) },
                Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::JmpCC(CondCode::NE, ".Lor_true0".to_string()),
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Jmp(".Lor_end0".to_string()),
                Instruction::Label(".Lor_true0".to_string()),
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Label(".Lor_end0".to_string()),
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 3: `return 7 % 2` → 剰余
    #[test]
    fn generate_remainder() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::Remainder,
                    Box::new(Expr::Constant(7)),
                    Box::new(Expr::Constant(2)),
                ))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(7), dst: Operand::Register(Reg::AX) },
                Instruction::Push(Operand::Register(Reg::AX)),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Register(Reg::CX) },
                Instruction::Pop(Operand::Register(Reg::AX)),
                Instruction::Cdq,
                Instruction::Idiv(Operand::Register(Reg::CX)),
                Instruction::Mov { src: Operand::Register(Reg::DX), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    // ── Chapter 5 テスト ──

    /// Chapter 5: `int a = 5; return a;`
    #[test]
    fn generate_var_declaration_and_return() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(5)))),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::AllocateStack(16),
                Instruction::Mov { src: Operand::Imm(5), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-4) },
                Instruction::Mov { src: Operand::Stack(-4), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 5: `int a; a = 10; return a;`
    #[test]
    fn generate_assignment() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", None)),
                BlockItem::Statement(Statement::Expression(
                    Expr::Assign("a".to_string(), Box::new(Expr::Constant(10)))
                )),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::AllocateStack(16),
                Instruction::Mov { src: Operand::Imm(10), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-4) },
                Instruction::Mov { src: Operand::Stack(-4), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 5: `int a = 2; int b = 3; return a + b;`
    #[test]
    fn generate_two_vars_addition() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(2)))),
                BlockItem::Declaration(var_decl("b", Some(Expr::Constant(3)))),
                BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::Add,
                    Box::new(Expr::Var("a".to_string())),
                    Box::new(Expr::Var("b".to_string())),
                ))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::AllocateStack(16),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-4) },
                Instruction::Mov { src: Operand::Imm(3), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-8) },
                Instruction::Mov { src: Operand::Stack(-4), dst: Operand::Register(Reg::AX) },
                Instruction::Push(Operand::Register(Reg::AX)),
                Instruction::Mov { src: Operand::Stack(-8), dst: Operand::Register(Reg::AX) },
                Instruction::Pop(Operand::Register(Reg::CX)),
                Instruction::Binary {
                    op: AsmBinaryOp::Add,
                    src: Operand::Register(Reg::CX),
                    dst: Operand::Register(Reg::AX),
                },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 5: 重複宣言はエラー
    #[test]
    fn generate_duplicate_declaration_error() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(1)))),
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(2)))),
            ]))],
        };
        let result = generate(&program);
        assert!(result.is_err());
    }

    /// Chapter 5: 未宣言変数はエラー
    #[test]
    fn generate_undeclared_variable_error() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Var("x".to_string()))),
            ]))],
        };
        let result = generate(&program);
        assert!(result.is_err());
    }

    // ── Chapter 6 テスト ──

    /// Chapter 6: `if (1) return 2; return 3;`
    #[test]
    fn generate_if_true() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::If {
                    condition: Expr::Constant(1),
                    then_branch: Box::new(Statement::Return(Expr::Constant(2))),
                    else_branch: None,
                }),
                BlockItem::Statement(Statement::Return(Expr::Constant(3))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::JmpCC(CondCode::E, ".Lif_end0".to_string()),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
                Instruction::Label(".Lif_end0".to_string()),
                Instruction::Mov { src: Operand::Imm(3), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 6: `if (0) return 2; else return 3;`
    #[test]
    fn generate_if_else() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::If {
                    condition: Expr::Constant(0),
                    then_branch: Box::new(Statement::Return(Expr::Constant(2))),
                    else_branch: Some(Box::new(Statement::Return(Expr::Constant(3)))),
                }),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::JmpCC(CondCode::E, ".Lif_else0".to_string()),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
                Instruction::Jmp(".Lif_end0".to_string()),
                Instruction::Label(".Lif_else0".to_string()),
                Instruction::Mov { src: Operand::Imm(3), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
                Instruction::Label(".Lif_end0".to_string()),
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 6: `return 1 ? 5 : 10;`
    #[test]
    fn generate_ternary() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(Expr::Conditional {
                    condition: Box::new(Expr::Constant(1)),
                    then_expr: Box::new(Expr::Constant(5)),
                    else_expr: Box::new(Expr::Constant(10)),
                })),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::JmpCC(CondCode::E, ".Ltern_else0".to_string()),
                Instruction::Mov { src: Operand::Imm(5), dst: Operand::Register(Reg::AX) },
                Instruction::Jmp(".Ltern_end0".to_string()),
                Instruction::Label(".Ltern_else0".to_string()),
                Instruction::Mov { src: Operand::Imm(10), dst: Operand::Register(Reg::AX) },
                Instruction::Label(".Ltern_end0".to_string()),
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 6: `int a = 1; { int a = 2; } return a;` — スコーピング
    #[test]
    fn generate_compound_scoping() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(1)))),
                BlockItem::Statement(Statement::Compound(vec![
                    BlockItem::Declaration(var_decl("a", Some(Expr::Constant(2)))),
                ])),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        // 外側の a はオフセット -4、内側の a はオフセット -8
        // return a は外側の a (オフセット -4) を参照する
        assert_eq!(
            asm.functions[0].instructions,
            vec![
                Instruction::AllocateStack(16),
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-4) },
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-8) },
                Instruction::Mov { src: Operand::Stack(-4), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
                // 暗黙の return 0
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    // ── Chapter 7 テスト ──

    /// Chapter 7: `int a = 5; a += 3; return a;` → 複合加算代入
    #[test]
    fn generate_compound_add_assign() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(5)))),
                BlockItem::Statement(Statement::Expression(
                    Expr::CompoundAssign(BinaryOp::Add, "a".to_string(), Box::new(Expr::Constant(3)))
                )),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        // Just check it generates without error; E2E tests verify correctness
        assert!(!asm.functions[0].instructions.is_empty());
    }

    /// Chapter 7: `int a = 5; return ++a;` → 前置インクリメント
    #[test]
    fn generate_pre_increment() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(5)))),
                BlockItem::Statement(Statement::Return(
                    Expr::Unary(UnaryOp::PreIncrement, Box::new(Expr::Var("a".to_string())))
                )),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert!(!asm.functions[0].instructions.is_empty());
    }

    /// Chapter 7: `int a = 5; return a++;` → 後置インクリメント
    #[test]
    fn generate_postfix_increment() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(5)))),
                BlockItem::Statement(Statement::Return(
                    Expr::PostfixIncrement("a".to_string())
                )),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert!(!asm.functions[0].instructions.is_empty());
    }

    /// Chapter 7: カンマ演算子
    #[test]
    fn generate_comma_operator() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Return(
                    Expr::Binary(
                        BinaryOp::Comma,
                        Box::new(Expr::Constant(1)),
                        Box::new(Expr::Constant(2)),
                    )
                )),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert!(!asm.functions[0].instructions.is_empty());
    }

    /// Chapter 6: 同じスコープの重複宣言はエラー、異なるスコープならOK
    #[test]
    fn generate_shadow_in_nested_scope_ok() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(1)))),
                BlockItem::Statement(Statement::Compound(vec![
                    BlockItem::Declaration(var_decl("a", Some(Expr::Constant(2)))),
                ])),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]))],
        };
        // ネストスコープでのシャドーイングは許可される
        assert!(generate(&program).is_ok());
    }

    // ── Chapter 8 テスト ──

    /// Chapter 8: while ループの基本コード生成
    #[test]
    fn generate_while_loop() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(0)))),
                BlockItem::Statement(Statement::While {
                    condition: Expr::Binary(
                        BinaryOp::LessThan,
                        Box::new(Expr::Var("a".to_string())),
                        Box::new(Expr::Constant(5)),
                    ),
                    body: Box::new(Statement::Expression(
                        Expr::Assign("a".to_string(), Box::new(Expr::Binary(
                            BinaryOp::Add,
                            Box::new(Expr::Var("a".to_string())),
                            Box::new(Expr::Constant(1)),
                        )))
                    )),
                }),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert!(!asm.functions[0].instructions.is_empty());
        // ラベルが正しく生成されることを確認
        assert!(asm.functions[0].instructions.contains(&Instruction::Label(".Lwhile_start0".to_string())));
        assert!(asm.functions[0].instructions.contains(&Instruction::Label(".Lwhile_end0".to_string())));
    }

    /// Chapter 8: do-while ループの基本コード生成
    #[test]
    fn generate_do_while_loop() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(0)))),
                BlockItem::Statement(Statement::DoWhile {
                    body: Box::new(Statement::Expression(
                        Expr::Assign("a".to_string(), Box::new(Expr::Binary(
                            BinaryOp::Add,
                            Box::new(Expr::Var("a".to_string())),
                            Box::new(Expr::Constant(1)),
                        )))
                    )),
                    condition: Expr::Binary(
                        BinaryOp::LessThan,
                        Box::new(Expr::Var("a".to_string())),
                        Box::new(Expr::Constant(5)),
                    ),
                }),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert!(!asm.functions[0].instructions.is_empty());
        assert!(asm.functions[0].instructions.contains(&Instruction::Label(".Ldo_start0".to_string())));
        assert!(asm.functions[0].instructions.contains(&Instruction::Label(".Ldo_continue0".to_string())));
        assert!(asm.functions[0].instructions.contains(&Instruction::Label(".Ldo_end0".to_string())));
    }

    /// Chapter 8: for ループの基本コード生成
    #[test]
    fn generate_for_loop() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(0)))),
                BlockItem::Statement(Statement::For {
                    init: ForInit::Declaration(var_decl("i", Some(Expr::Constant(0)))),
                    condition: Some(Expr::Binary(
                        BinaryOp::LessThan,
                        Box::new(Expr::Var("i".to_string())),
                        Box::new(Expr::Constant(5)),
                    )),
                    post: Some(Expr::Assign("i".to_string(), Box::new(Expr::Binary(
                        BinaryOp::Add,
                        Box::new(Expr::Var("i".to_string())),
                        Box::new(Expr::Constant(1)),
                    )))),
                    body: Box::new(Statement::Expression(
                        Expr::Assign("a".to_string(), Box::new(Expr::Binary(
                            BinaryOp::Add,
                            Box::new(Expr::Var("a".to_string())),
                            Box::new(Expr::Constant(1)),
                        )))
                    )),
                }),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        assert!(!asm.functions[0].instructions.is_empty());
        assert!(asm.functions[0].instructions.contains(&Instruction::Label(".Lfor_start0".to_string())));
        assert!(asm.functions[0].instructions.contains(&Instruction::Label(".Lfor_continue0".to_string())));
        assert!(asm.functions[0].instructions.contains(&Instruction::Label(".Lfor_end0".to_string())));
    }

    /// Chapter 8: break はループ内でのみ使用可能
    #[test]
    fn generate_break_outside_loop_error() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Break),
            ]))],
        };
        let result = generate(&program);
        assert!(result.is_err());
    }

    /// Chapter 8: continue はループ内でのみ使用可能
    #[test]
    fn generate_continue_outside_loop_error() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::Continue),
            ]))],
        };
        let result = generate(&program);
        assert!(result.is_err());
    }

    /// Chapter 8: break inside while generates correct jump
    #[test]
    fn generate_break_in_while() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Statement(Statement::While {
                    condition: Expr::Constant(1),
                    body: Box::new(Statement::Break),
                }),
                BlockItem::Statement(Statement::Return(Expr::Constant(0))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        // break should generate a Jmp to the while_end label
        assert!(asm.functions[0].instructions.contains(&Instruction::Jmp(".Lwhile_end0".to_string())));
    }

    /// Chapter 8: continue inside while generates correct jump
    #[test]
    fn generate_continue_in_while() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl("a", Some(Expr::Constant(0)))),
                BlockItem::Statement(Statement::While {
                    condition: Expr::Binary(
                        BinaryOp::LessThan,
                        Box::new(Expr::Var("a".to_string())),
                        Box::new(Expr::Constant(5)),
                    ),
                    body: Box::new(Statement::Compound(vec![
                        BlockItem::Statement(Statement::Expression(
                            Expr::PostfixIncrement("a".to_string())
                        )),
                        BlockItem::Statement(Statement::Continue),
                    ])),
                }),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]))],
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.static_vars, vec![]);
        // continue in while should jump to while_start
        assert!(asm.functions[0].instructions.contains(&Instruction::Jmp(".Lwhile_start0".to_string())));
    }

    // ── Chapter 10 テスト ──

    /// Chapter 10: グローバル変数 `int x = 5; int main(void) { return x; }`
    ///
    /// ファイルスコープの変数は Data オペランド（RIP相対）でアクセスされる。
    #[test]
    fn generate_global_variable() {
        let program = Program {
            declarations: vec![
                TopLevelDecl::Variable(var_decl("x", Some(Expr::Constant(5)))),
                func_decl("main", vec![], Some(vec![
                    BlockItem::Statement(Statement::Return(Expr::Var("x".to_string()))),
                ])),
            ],
        };
        let asm = generate(&program).unwrap();

        // static_vars に x が含まれる（global: true, init: 5）
        assert_eq!(asm.static_vars, vec![
            AsmStaticVar { name: "x".to_string(), global: true, init: 5 },
        ]);

        // main 関数は global: true
        assert_eq!(asm.functions[0].global, true);

        // return x は Data("x") からロードする
        assert!(asm.functions[0].instructions.contains(
            &Instruction::Mov {
                src: Operand::Data("x".to_string()),
                dst: Operand::Register(Reg::AX),
            }
        ));
    }

    /// Chapter 10: static ローカル変数のユニークラベル生成
    ///
    /// `int main(void) { static int c = 0; c = c + 1; return c; }`
    /// static ローカル変数はユニークラベル（例: `c.0`）でデータセクションに配置される。
    #[test]
    fn generate_static_local_variable() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl_with_sc("c", Some(Expr::Constant(0)), Some(StorageClass::Static))),
                BlockItem::Statement(Statement::Expression(
                    Expr::Assign("c".to_string(), Box::new(Expr::Binary(
                        BinaryOp::Add,
                        Box::new(Expr::Var("c".to_string())),
                        Box::new(Expr::Constant(1)),
                    )))
                )),
                BlockItem::Statement(Statement::Return(Expr::Var("c".to_string()))),
            ]))],
        };
        let asm = generate(&program).unwrap();

        // static_vars にユニークラベル付きで含まれる
        assert_eq!(asm.static_vars, vec![
            AsmStaticVar { name: "c.0".to_string(), global: false, init: 0 },
        ]);

        // c の参照は Data("c.0") になる
        assert!(asm.functions[0].instructions.contains(
            &Instruction::Mov {
                src: Operand::Data("c.0".to_string()),
                dst: Operand::Register(Reg::AX),
            }
        ));
    }

    /// Chapter 10: 関数内の extern 変数宣言
    ///
    /// `extern int x;` は初期化子を持てず、Data オペランドで参照される。
    #[test]
    fn generate_extern_variable_in_function() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl_with_sc("x", None, Some(StorageClass::Extern))),
                BlockItem::Statement(Statement::Return(Expr::Var("x".to_string()))),
            ]))],
        };
        let asm = generate(&program).unwrap();

        // extern 変数は static_vars に追加されない
        assert_eq!(asm.static_vars, vec![]);

        // x の参照は Data("x") になる
        assert!(asm.functions[0].instructions.contains(
            &Instruction::Mov {
                src: Operand::Data("x".to_string()),
                dst: Operand::Register(Reg::AX),
            }
        ));
    }

    /// Chapter 10: extern 変数に初期化子があるとエラー
    #[test]
    fn generate_extern_with_initializer_error() {
        let program = Program {
            declarations: vec![func_decl("main", vec![], Some(vec![
                BlockItem::Declaration(var_decl_with_sc("x", Some(Expr::Constant(5)), Some(StorageClass::Extern))),
            ]))],
        };
        let result = generate(&program);
        assert!(result.is_err());
    }

    /// Chapter 10: static 関数は global: false
    ///
    /// `static int helper(void) { return 42; }` は global: false で生成される。
    #[test]
    fn generate_static_function() {
        let program = Program {
            declarations: vec![
                func_decl_with_sc("helper", vec![], Some(vec![
                    BlockItem::Statement(Statement::Return(Expr::Constant(42))),
                ]), Some(StorageClass::Static)),
                func_decl("main", vec![], Some(vec![
                    BlockItem::Statement(Statement::Return(
                        Expr::FunctionCall("helper".to_string(), vec![])
                    )),
                ])),
            ],
        };
        let asm = generate(&program).unwrap();

        // helper は global: false
        assert_eq!(asm.functions[0].name, "helper");
        assert_eq!(asm.functions[0].global, false);

        // main は global: true
        assert_eq!(asm.functions[1].name, "main");
        assert_eq!(asm.functions[1].global, true);
    }

    /// Chapter 10: ファイルスコープの static 変数は global: false
    #[test]
    fn generate_file_scope_static_variable() {
        let program = Program {
            declarations: vec![
                TopLevelDecl::Variable(var_decl_with_sc("counter", Some(Expr::Constant(10)), Some(StorageClass::Static))),
                func_decl("main", vec![], Some(vec![
                    BlockItem::Statement(Statement::Return(Expr::Var("counter".to_string()))),
                ])),
            ],
        };
        let asm = generate(&program).unwrap();

        // static 変数は global: false
        assert_eq!(asm.static_vars, vec![
            AsmStaticVar { name: "counter".to_string(), global: false, init: 10 },
        ]);

        // counter の参照は Data("counter") になる
        assert!(asm.functions[0].instructions.contains(
            &Instruction::Mov {
                src: Operand::Data("counter".to_string()),
                dst: Operand::Register(Reg::AX),
            }
        ));
    }

    /// Chapter 10: ファイルスコープの extern 変数は static_vars に追加されない
    #[test]
    fn generate_file_scope_extern_variable() {
        let program = Program {
            declarations: vec![
                TopLevelDecl::Variable(var_decl_with_sc("ext_var", None, Some(StorageClass::Extern))),
                func_decl("main", vec![], Some(vec![
                    BlockItem::Statement(Statement::Return(Expr::Var("ext_var".to_string()))),
                ])),
            ],
        };
        let asm = generate(&program).unwrap();

        // extern 変数は static_vars に追加されない
        assert_eq!(asm.static_vars, vec![]);

        // ext_var の参照は Data("ext_var") になる
        assert!(asm.functions[0].instructions.contains(
            &Instruction::Mov {
                src: Operand::Data("ext_var".to_string()),
                dst: Operand::Register(Reg::AX),
            }
        ));
    }

    /// Chapter 10: ファイルスコープの extern 変数に初期化子があるとエラー
    #[test]
    fn generate_file_scope_extern_with_initializer_error() {
        let program = Program {
            declarations: vec![
                TopLevelDecl::Variable(var_decl_with_sc("ext_var", Some(Expr::Constant(5)), Some(StorageClass::Extern))),
            ],
        };
        let result = generate(&program);
        assert!(result.is_err());
    }

    /// Chapter 10: ファイルスコープ変数の非定数初期化子はエラー
    #[test]
    fn generate_file_scope_non_constant_init_error() {
        let program = Program {
            declarations: vec![
                TopLevelDecl::Variable(Declaration {
                    name: "x".to_string(),
                    init: Some(Expr::Binary(BinaryOp::Add, Box::new(Expr::Constant(1)), Box::new(Expr::Constant(2)))),
                    storage_class: None,
                }),
            ],
        };
        let result = generate(&program);
        assert!(result.is_err());
    }
}
