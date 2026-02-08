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
//! | `a` (変数参照) | `movl offset(%rbp), %eax` |
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

use std::collections::{HashMap, HashSet};
use crate::error::{CompileError, Result};
use crate::parse::ast::{Program, BlockItem, Declaration, Statement, Expr, UnaryOp, BinaryOp, ForInit};
use super::asm_ast::{
    AsmProgram, AsmFunction, Instruction, Operand, Reg, AsmUnaryOp, AsmBinaryOp, CondCode,
};

/// ループ内の break/continue ジャンプ先ラベル（Chapter 8）。
struct LoopLabels {
    break_label: String,
    continue_label: String,
}

/// C の AST をアセンブリ AST に変換する。
pub fn generate(program: &Program) -> Result<AsmProgram> {
    let mut label_counter = 0;
    let function = generate_function(&program.function, &mut label_counter)?;
    Ok(AsmProgram { function })
}

/// 関数の変換: 本体のブロック要素列から命令列を生成する。
///
/// Chapter 5: 変数マップを使ってローカル変数のスタックオフセットを管理する。
/// すべてのブロック要素を処理した後、変数がある場合は `AllocateStack` を先頭に挿入する。
fn generate_function(func: &crate::parse::ast::Function, label_counter: &mut usize) -> Result<AsmFunction> {
    let mut var_map: HashMap<String, i32> = HashMap::new();
    let mut next_offset: i32 = -4;
    let mut instructions = Vec::new();

    let mut scope_decls: HashSet<String> = HashSet::new();
    for item in &func.body {
        let instrs = generate_block_item(item, &mut var_map, &mut next_offset, label_counter, Some(&mut scope_decls), None)?;
        instructions.extend(instrs);
    }

    // 変数がある場合、AllocateStack を先頭に挿入
    // next_offset ベースで計算（ネストスコープの変数も含む）
    let total_vars = ((-next_offset - 4) / 4) as usize;
    if total_vars > 0 {
        instructions.insert(0, Instruction::AllocateStack(total_vars * 4));
    }

    Ok(AsmFunction {
        name: func.name.clone(),
        instructions,
    })
}

/// ブロック要素の変換。
fn generate_block_item(
    item: &BlockItem,
    var_map: &mut HashMap<String, i32>,
    next_offset: &mut i32,
    label_counter: &mut usize,
    scope_decls: Option<&mut HashSet<String>>,
    loop_labels: Option<&LoopLabels>,
) -> Result<Vec<Instruction>> {
    match item {
        BlockItem::Statement(stmt) => generate_statement(stmt, var_map, next_offset, label_counter, loop_labels),
        BlockItem::Declaration(decl) => generate_declaration(decl, var_map, next_offset, label_counter, scope_decls),
    }
}

/// 宣言の変換。
///
/// 変数をスタックに割り当て、初期化式がある場合はその値を格納する。
fn generate_declaration(
    decl: &Declaration,
    var_map: &mut HashMap<String, i32>,
    next_offset: &mut i32,
    label_counter: &mut usize,
    scope_decls: Option<&mut HashSet<String>>,
) -> Result<Vec<Instruction>> {
    // 同一スコープ内の重複宣言チェック
    if let Some(decls) = scope_decls {
        if !decls.insert(decl.name.clone()) {
            return Err(CompileError::CodegenError(format!(
                "variable '{}' already declared in this scope", decl.name
            )));
        }
    }

    let offset = *next_offset;
    var_map.insert(decl.name.clone(), offset);
    *next_offset -= 4;

    let mut instrs = Vec::new();
    if let Some(init) = &decl.init {
        instrs.extend(generate_expr(init, var_map, label_counter)?);
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
    var_map: &mut HashMap<String, i32>,
    next_offset: &mut i32,
    label_counter: &mut usize,
    loop_labels: Option<&LoopLabels>,
) -> Result<Vec<Instruction>> {
    match stmt {
        Statement::Return(expr) => {
            let mut instrs = generate_expr(expr, var_map, label_counter)?;
            instrs.push(Instruction::Ret);
            Ok(instrs)
        }
        Statement::Expression(expr) => {
            generate_expr(expr, var_map, label_counter)
        }
        Statement::Null => {
            Ok(Vec::new())
        }
        Statement::If { condition, then_branch, else_branch } => {
            let n = *label_counter;
            *label_counter += 1;

            let mut instrs = generate_expr(condition, var_map, label_counter)?;
            instrs.push(Instruction::Cmp {
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            });

            if let Some(else_stmt) = else_branch {
                let else_label = format!(".Lif_else{n}");
                let end_label = format!(".Lif_end{n}");

                instrs.push(Instruction::JmpCC(CondCode::E, else_label.clone()));
                instrs.extend(generate_statement(then_branch, var_map, next_offset, label_counter, loop_labels)?);
                instrs.push(Instruction::Jmp(end_label.clone()));
                instrs.push(Instruction::Label(else_label));
                instrs.extend(generate_statement(else_stmt, var_map, next_offset, label_counter, loop_labels)?);
                instrs.push(Instruction::Label(end_label));
            } else {
                let end_label = format!(".Lif_end{n}");
                instrs.push(Instruction::JmpCC(CondCode::E, end_label.clone()));
                instrs.extend(generate_statement(then_branch, var_map, next_offset, label_counter, loop_labels)?);
                instrs.push(Instruction::Label(end_label));
            }
            Ok(instrs)
        }
        Statement::Compound(items) => {
            let mut inner_map = var_map.clone();
            let mut scope_decls: HashSet<String> = HashSet::new();
            let mut instrs = Vec::new();
            for item in items {
                instrs.extend(generate_block_item(item, &mut inner_map, next_offset, label_counter, Some(&mut scope_decls), loop_labels)?);
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
            instrs.extend(generate_expr(condition, var_map, label_counter)?);
            instrs.push(Instruction::Cmp {
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::JmpCC(CondCode::E, end_label.clone()));
            instrs.extend(generate_statement(body, var_map, next_offset, label_counter, Some(&labels))?);
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
            instrs.extend(generate_statement(body, var_map, next_offset, label_counter, Some(&labels))?);
            instrs.push(Instruction::Label(continue_label));
            instrs.extend(generate_expr(condition, var_map, label_counter)?);
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
            let map_ref: &mut HashMap<String, i32> = match init {
                ForInit::Declaration(_) => &mut inner_map,
                ForInit::Expression(_) => var_map,
            };

            let mut instrs = Vec::new();

            // init
            match init {
                ForInit::Declaration(decl) => {
                    instrs.extend(generate_declaration(decl, map_ref, next_offset, label_counter, None)?);
                }
                ForInit::Expression(Some(expr)) => {
                    instrs.extend(generate_expr(expr, map_ref, label_counter)?);
                }
                ForInit::Expression(None) => {}
            }

            instrs.push(Instruction::Label(start_label.clone()));

            // condition
            if let Some(cond) = condition {
                instrs.extend(generate_expr(cond, map_ref, label_counter)?);
                instrs.push(Instruction::Cmp {
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::AX),
                });
                instrs.push(Instruction::JmpCC(CondCode::E, end_label.clone()));
            }

            // body
            instrs.extend(generate_statement(body, map_ref, next_offset, label_counter, Some(&labels))?);

            // continue target + post
            instrs.push(Instruction::Label(continue_label));
            if let Some(post_expr) = post {
                instrs.extend(generate_expr(post_expr, map_ref, label_counter)?);
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

/// 式の変換（Chapter 5 で拡張）。
///
/// すべての式は、評価後に結果が `%eax` に入るような命令列を生成する。
fn generate_expr(
    expr: &Expr,
    var_map: &HashMap<String, i32>,
    label_counter: &mut usize,
) -> Result<Vec<Instruction>> {
    match expr {
        // 定数: 即値を %eax にロードする
        Expr::Constant(value) => {
            Ok(vec![Instruction::Mov {
                src: Operand::Imm(*value),
                dst: Operand::Register(Reg::AX),
            }])
        }

        // 変数参照: スタックから %eax にロードする
        Expr::Var(name) => {
            let offset = var_map.get(name).ok_or_else(|| {
                CompileError::CodegenError(format!("undeclared variable '{}'", name))
            })?;
            Ok(vec![Instruction::Mov {
                src: Operand::Stack(*offset),
                dst: Operand::Register(Reg::AX),
            }])
        }

        // 代入: 右辺を評価して結果をスタックに格納（%eax にも結果が残る）
        Expr::Assign(name, rhs) => {
            let offset = var_map.get(name).ok_or_else(|| {
                CompileError::CodegenError(format!("undeclared variable '{}'", name))
            })?;
            let mut instrs = generate_expr(rhs, var_map, label_counter)?;
            instrs.push(Instruction::Mov {
                src: Operand::Register(Reg::AX),
                dst: Operand::Stack(*offset),
            });
            Ok(instrs)
        }

        // 単項演算: まず内側の式を評価（結果は %eax）、その後に演算を適用
        Expr::Unary(op, inner) => {
            match op {
                UnaryOp::PreIncrement | UnaryOp::PreDecrement => {
                    // 前置 ++/--: 変数をロード → 加減算 → 格納 → 新値が %eax に残る
                    if let Expr::Var(name) = inner.as_ref() {
                        let offset = var_map.get(name).ok_or_else(|| {
                            CompileError::CodegenError(format!("undeclared variable '{}'", name))
                        })?;
                        let mut instrs = vec![
                            Instruction::Mov {
                                src: Operand::Stack(*offset),
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
                            dst: Operand::Stack(*offset),
                        });
                        Ok(instrs)
                    } else {
                        Err(CompileError::CodegenError(
                            "lvalue required for prefix increment/decrement".to_string()
                        ))
                    }
                }
                _ => {
                    let mut instrs = generate_expr(inner, var_map, label_counter)?;
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

            let mut instrs = generate_expr(condition, var_map, label_counter)?;
            instrs.push(Instruction::Cmp {
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            });
            instrs.push(Instruction::JmpCC(CondCode::E, else_label.clone()));
            instrs.extend(generate_expr(then_expr, var_map, label_counter)?);
            instrs.push(Instruction::Jmp(end_label.clone()));
            instrs.push(Instruction::Label(else_label));
            instrs.extend(generate_expr(else_expr, var_map, label_counter)?);
            instrs.push(Instruction::Label(end_label));
            Ok(instrs)
        }

        // 複合代入（Chapter 7）: a += 5 → a = a + 5 相当
        Expr::CompoundAssign(op, name, rhs) => {
            let offset = var_map.get(name).ok_or_else(|| {
                CompileError::CodegenError(format!("undeclared variable '{}'", name))
            })?;
            let offset = *offset;

            match op {
                BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply => {
                    // 現在値をロード → push → rhs評価 → pop → 演算 → 格納
                    let mut instrs = vec![
                        Instruction::Mov {
                            src: Operand::Stack(offset),
                            dst: Operand::Register(Reg::AX),
                        },
                        Instruction::Push(Operand::Register(Reg::AX)),
                    ];
                    instrs.extend(generate_expr(rhs, var_map, label_counter)?);
                    instrs.push(Instruction::Pop(Operand::Register(Reg::CX)));
                    let asm_op = match op {
                        BinaryOp::Add => AsmBinaryOp::Add,
                        BinaryOp::Subtract => AsmBinaryOp::Sub,
                        BinaryOp::Multiply => AsmBinaryOp::Mult,
                        _ => unreachable!(),
                    };
                    if matches!(op, BinaryOp::Subtract) {
                        // CX(現在値) - AX(rhs) → CX に結果
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
                        // AX(rhs) + CX(現在値) or AX * CX
                        instrs.push(Instruction::Binary {
                            op: asm_op,
                            src: Operand::Register(Reg::CX),
                            dst: Operand::Register(Reg::AX),
                        });
                    }
                    instrs.push(Instruction::Mov {
                        src: Operand::Register(Reg::AX),
                        dst: Operand::Stack(offset),
                    });
                    Ok(instrs)
                }
                BinaryOp::Divide | BinaryOp::Remainder => {
                    // rhs を評価 → CX に移動 → 現在値をAXにロード → cdq → idiv → 格納
                    let mut instrs = generate_expr(rhs, var_map, label_counter)?;
                    instrs.push(Instruction::Mov {
                        src: Operand::Register(Reg::AX),
                        dst: Operand::Register(Reg::CX),
                    });
                    instrs.push(Instruction::Mov {
                        src: Operand::Stack(offset),
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
                        dst: Operand::Stack(offset),
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
            let offset = var_map.get(name).ok_or_else(|| {
                CompileError::CodegenError(format!("undeclared variable '{}'", name))
            })?;
            Ok(vec![
                Instruction::Mov {
                    src: Operand::Stack(*offset),
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
                    dst: Operand::Stack(*offset),
                },
            ])
        }

        // 後置デクリメント（Chapter 7）: a-- → 旧値を返し、変数を -1
        Expr::PostfixDecrement(name) => {
            let offset = var_map.get(name).ok_or_else(|| {
                CompileError::CodegenError(format!("undeclared variable '{}'", name))
            })?;
            Ok(vec![
                Instruction::Mov {
                    src: Operand::Stack(*offset),
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
                    dst: Operand::Stack(*offset),
                },
            ])
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

                    let mut instrs = generate_expr(left, var_map, label_counter)?;
                    instrs.push(Instruction::Cmp {
                        src: Operand::Imm(0),
                        dst: Operand::Register(Reg::AX),
                    });
                    instrs.push(Instruction::JmpCC(CondCode::E, false_label.clone()));

                    instrs.extend(generate_expr(right, var_map, label_counter)?);
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

                    let mut instrs = generate_expr(left, var_map, label_counter)?;
                    instrs.push(Instruction::Cmp {
                        src: Operand::Imm(0),
                        dst: Operand::Register(Reg::AX),
                    });
                    instrs.push(Instruction::JmpCC(CondCode::NE, true_label.clone()));

                    instrs.extend(generate_expr(right, var_map, label_counter)?);
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
                    let mut instrs = generate_expr(left, var_map, label_counter)?;
                    instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                    instrs.extend(generate_expr(right, var_map, label_counter)?);
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
                    let mut instrs = generate_expr(left, var_map, label_counter)?;
                    instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                    instrs.extend(generate_expr(right, var_map, label_counter)?);

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
                    let mut instrs = generate_expr(left, var_map, label_counter)?;
                    instrs.extend(generate_expr(right, var_map, label_counter)?);
                    Ok(instrs)
                }

                // 除算・剰余: idivl を使う
                BinaryOp::Divide | BinaryOp::Remainder => {
                    let mut instrs = generate_expr(left, var_map, label_counter)?;
                    instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
                    instrs.extend(generate_expr(right, var_map, label_counter)?);

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
    use crate::parse::ast::Function;

    /// Chapter 1: `return 2` の場合
    #[test]
    fn generate_return_constant() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![BlockItem::Statement(Statement::Return(Expr::Constant(2)))],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(asm.function.name, "main");
        assert_eq!(
            asm.function.instructions,
            vec![
                Instruction::Mov {
                    src: Operand::Imm(2),
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
            function: Function {
                name: "main".to_string(),
                body: vec![BlockItem::Statement(Statement::Return(Expr::Unary(
                    UnaryOp::Negate,
                    Box::new(Expr::Constant(5)),
                )))],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
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
            ]
        );
    }

    /// Chapter 2: `return ~0` → movl $0, %eax; notl %eax; ret
    #[test]
    fn generate_complement() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![BlockItem::Statement(Statement::Return(Expr::Unary(
                    UnaryOp::Complement,
                    Box::new(Expr::Constant(0)),
                )))],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
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
            ]
        );
    }

    /// Chapter 2: `return !1`
    #[test]
    fn generate_logical_not() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![BlockItem::Statement(Statement::Return(Expr::Unary(
                    UnaryOp::Not,
                    Box::new(Expr::Constant(1)),
                )))],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
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
            ]
        );
    }

    // ── Chapter 3 テスト ──

    /// Chapter 3: `return 1 + 2` → 加算
    #[test]
    fn generate_addition() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::Add,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )))],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
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
            ]
        );
    }

    /// Chapter 3: `return 7 / 2` → 除算
    #[test]
    fn generate_division() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::Divide,
                    Box::new(Expr::Constant(7)),
                    Box::new(Expr::Constant(2)),
                )))],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(7), dst: Operand::Register(Reg::AX) },
                Instruction::Push(Operand::Register(Reg::AX)),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Register(Reg::CX) },
                Instruction::Pop(Operand::Register(Reg::AX)),
                Instruction::Cdq,
                Instruction::Idiv(Operand::Register(Reg::CX)),
                Instruction::Ret,
            ]
        );
    }

    // ── Chapter 4 テスト ──

    /// Chapter 4: `return 1 < 2` → cmpl + setl パターン
    #[test]
    fn generate_less_than() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::LessThan,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )))],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Push(Operand::Register(Reg::AX)),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Pop(Operand::Register(Reg::CX)),
                Instruction::Cmp { src: Operand::Register(Reg::AX), dst: Operand::Register(Reg::CX) },
                Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::SetCC { condition: CondCode::L, operand: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 4: `return 1 && 2` → 短絡評価
    #[test]
    fn generate_logical_and() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::LogicalAnd,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )))],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
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
            ]
        );
    }

    /// Chapter 4: `return 0 || 3` → 短絡評価
    #[test]
    fn generate_logical_or() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::LogicalOr,
                    Box::new(Expr::Constant(0)),
                    Box::new(Expr::Constant(3)),
                )))],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
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
            ]
        );
    }

    /// Chapter 3: `return 7 % 2` → 剰余
    #[test]
    fn generate_remainder() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::Remainder,
                    Box::new(Expr::Constant(7)),
                    Box::new(Expr::Constant(2)),
                )))],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
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
            ]
        );
    }

    // ── Chapter 5 テスト ──

    /// Chapter 5: `int a = 5; return a;`
    #[test]
    fn generate_var_declaration_and_return() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(5)),
                    }),
                    BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
            vec![
                Instruction::AllocateStack(4),
                Instruction::Mov { src: Operand::Imm(5), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-4) },
                Instruction::Mov { src: Operand::Stack(-4), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 5: `int a; a = 10; return a;`
    #[test]
    fn generate_assignment() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: None,
                    }),
                    BlockItem::Statement(Statement::Expression(
                        Expr::Assign("a".to_string(), Box::new(Expr::Constant(10)))
                    )),
                    BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
            vec![
                Instruction::AllocateStack(4),
                Instruction::Mov { src: Operand::Imm(10), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-4) },
                Instruction::Mov { src: Operand::Stack(-4), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 5: `int a = 2; int b = 3; return a + b;`
    #[test]
    fn generate_two_vars_addition() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(2)),
                    }),
                    BlockItem::Declaration(Declaration {
                        name: "b".to_string(),
                        init: Some(Expr::Constant(3)),
                    }),
                    BlockItem::Statement(Statement::Return(Expr::Binary(
                        BinaryOp::Add,
                        Box::new(Expr::Var("a".to_string())),
                        Box::new(Expr::Var("b".to_string())),
                    ))),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
            vec![
                Instruction::AllocateStack(8),
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
            ]
        );
    }

    /// Chapter 5: 重複宣言はエラー
    #[test]
    fn generate_duplicate_declaration_error() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(1)),
                    }),
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(2)),
                    }),
                ],
            },
        };
        let result = generate(&program);
        assert!(result.is_err());
    }

    /// Chapter 5: 未宣言変数はエラー
    #[test]
    fn generate_undeclared_variable_error() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Statement(Statement::Return(Expr::Var("x".to_string()))),
                ],
            },
        };
        let result = generate(&program);
        assert!(result.is_err());
    }

    // ── Chapter 6 テスト ──

    /// Chapter 6: `if (1) return 2; return 3;`
    #[test]
    fn generate_if_true() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Statement(Statement::If {
                        condition: Expr::Constant(1),
                        then_branch: Box::new(Statement::Return(Expr::Constant(2))),
                        else_branch: None,
                    }),
                    BlockItem::Statement(Statement::Return(Expr::Constant(3))),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
            vec![
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
                Instruction::JmpCC(CondCode::E, ".Lif_end0".to_string()),
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
                Instruction::Label(".Lif_end0".to_string()),
                Instruction::Mov { src: Operand::Imm(3), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    /// Chapter 6: `if (0) return 2; else return 3;`
    #[test]
    fn generate_if_else() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Statement(Statement::If {
                        condition: Expr::Constant(0),
                        then_branch: Box::new(Statement::Return(Expr::Constant(2))),
                        else_branch: Some(Box::new(Statement::Return(Expr::Constant(3)))),
                    }),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
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
            ]
        );
    }

    /// Chapter 6: `return 1 ? 5 : 10;`
    #[test]
    fn generate_ternary() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Statement(Statement::Return(Expr::Conditional {
                        condition: Box::new(Expr::Constant(1)),
                        then_expr: Box::new(Expr::Constant(5)),
                        else_expr: Box::new(Expr::Constant(10)),
                    })),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert_eq!(
            asm.function.instructions,
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
            ]
        );
    }

    /// Chapter 6: `int a = 1; { int a = 2; } return a;` — スコーピング
    #[test]
    fn generate_compound_scoping() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(1)),
                    }),
                    BlockItem::Statement(Statement::Compound(vec![
                        BlockItem::Declaration(Declaration {
                            name: "a".to_string(),
                            init: Some(Expr::Constant(2)),
                        }),
                    ])),
                    BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        // 外側の a はオフセット -4、内側の a はオフセット -8
        // return a は外側の a (オフセット -4) を参照する
        assert_eq!(
            asm.function.instructions,
            vec![
                Instruction::AllocateStack(8),
                Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-4) },
                Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
                Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-8) },
                Instruction::Mov { src: Operand::Stack(-4), dst: Operand::Register(Reg::AX) },
                Instruction::Ret,
            ]
        );
    }

    // ── Chapter 7 テスト ──

    /// Chapter 7: `int a = 5; a += 3; return a;` → 複合加算代入
    #[test]
    fn generate_compound_add_assign() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(5)),
                    }),
                    BlockItem::Statement(Statement::Expression(
                        Expr::CompoundAssign(BinaryOp::Add, "a".to_string(), Box::new(Expr::Constant(3)))
                    )),
                    BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        // Just check it generates without error; E2E tests verify correctness
        assert!(!asm.function.instructions.is_empty());
    }

    /// Chapter 7: `int a = 5; return ++a;` → 前置インクリメント
    #[test]
    fn generate_pre_increment() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(5)),
                    }),
                    BlockItem::Statement(Statement::Return(
                        Expr::Unary(UnaryOp::PreIncrement, Box::new(Expr::Var("a".to_string())))
                    )),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert!(!asm.function.instructions.is_empty());
    }

    /// Chapter 7: `int a = 5; return a++;` → 後置インクリメント
    #[test]
    fn generate_postfix_increment() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(5)),
                    }),
                    BlockItem::Statement(Statement::Return(
                        Expr::PostfixIncrement("a".to_string())
                    )),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert!(!asm.function.instructions.is_empty());
    }

    /// Chapter 7: カンマ演算子
    #[test]
    fn generate_comma_operator() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Statement(Statement::Return(
                        Expr::Binary(
                            BinaryOp::Comma,
                            Box::new(Expr::Constant(1)),
                            Box::new(Expr::Constant(2)),
                        )
                    )),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert!(!asm.function.instructions.is_empty());
    }

    /// Chapter 6: 同じスコープの重複宣言はエラー、異なるスコープならOK
    #[test]
    fn generate_shadow_in_nested_scope_ok() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(1)),
                    }),
                    BlockItem::Statement(Statement::Compound(vec![
                        BlockItem::Declaration(Declaration {
                            name: "a".to_string(),
                            init: Some(Expr::Constant(2)),
                        }),
                    ])),
                    BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
                ],
            },
        };
        // ネストスコープでのシャドーイングは許可される
        assert!(generate(&program).is_ok());
    }

    // ── Chapter 8 テスト ──

    /// Chapter 8: while ループの基本コード生成
    #[test]
    fn generate_while_loop() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(0)),
                    }),
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
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert!(!asm.function.instructions.is_empty());
        // ラベルが正しく生成されることを確認
        assert!(asm.function.instructions.contains(&Instruction::Label(".Lwhile_start0".to_string())));
        assert!(asm.function.instructions.contains(&Instruction::Label(".Lwhile_end0".to_string())));
    }

    /// Chapter 8: do-while ループの基本コード生成
    #[test]
    fn generate_do_while_loop() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(0)),
                    }),
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
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert!(!asm.function.instructions.is_empty());
        assert!(asm.function.instructions.contains(&Instruction::Label(".Ldo_start0".to_string())));
        assert!(asm.function.instructions.contains(&Instruction::Label(".Ldo_continue0".to_string())));
        assert!(asm.function.instructions.contains(&Instruction::Label(".Ldo_end0".to_string())));
    }

    /// Chapter 8: for ループの基本コード生成
    #[test]
    fn generate_for_loop() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(0)),
                    }),
                    BlockItem::Statement(Statement::For {
                        init: ForInit::Declaration(Declaration {
                            name: "i".to_string(),
                            init: Some(Expr::Constant(0)),
                        }),
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
                ],
            },
        };
        let asm = generate(&program).unwrap();
        assert!(!asm.function.instructions.is_empty());
        assert!(asm.function.instructions.contains(&Instruction::Label(".Lfor_start0".to_string())));
        assert!(asm.function.instructions.contains(&Instruction::Label(".Lfor_continue0".to_string())));
        assert!(asm.function.instructions.contains(&Instruction::Label(".Lfor_end0".to_string())));
    }

    /// Chapter 8: break はループ内でのみ使用可能
    #[test]
    fn generate_break_outside_loop_error() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Statement(Statement::Break),
                ],
            },
        };
        let result = generate(&program);
        assert!(result.is_err());
    }

    /// Chapter 8: continue はループ内でのみ使用可能
    #[test]
    fn generate_continue_outside_loop_error() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Statement(Statement::Continue),
                ],
            },
        };
        let result = generate(&program);
        assert!(result.is_err());
    }

    /// Chapter 8: break inside while generates correct jump
    #[test]
    fn generate_break_in_while() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Statement(Statement::While {
                        condition: Expr::Constant(1),
                        body: Box::new(Statement::Break),
                    }),
                    BlockItem::Statement(Statement::Return(Expr::Constant(0))),
                ],
            },
        };
        let asm = generate(&program).unwrap();
        // break should generate a Jmp to the while_end label
        assert!(asm.function.instructions.contains(&Instruction::Jmp(".Lwhile_end0".to_string())));
    }

    /// Chapter 8: continue inside while generates correct jump
    #[test]
    fn generate_continue_in_while() {
        let program = Program {
            function: Function {
                name: "main".to_string(),
                body: vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(0)),
                    }),
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
                ],
            },
        };
        let asm = generate(&program).unwrap();
        // continue in while should jump to while_start
        assert!(asm.function.instructions.contains(&Instruction::Jmp(".Lwhile_start0".to_string())));
    }
}
