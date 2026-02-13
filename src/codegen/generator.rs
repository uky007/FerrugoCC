//! コード生成器（Code Generator）
//!
//! TACKY IR を走査し、対応する x86-64 アセンブリ命令列に変換する。
//! 各 TACKY 命令を機械的にアセンブリ命令にマッピングする。
//!
//! # Chapter 20: Pseudo レジスタ出力
//! 変数は `Operand::Pseudo(name)` として出力され、レジスタ割り当てパスで
//! `Register` または `Stack` に置換される。
//! アドレス取得対象・構造体・配列変数のみ `Stack(offset)` に事前割り当てする。

use std::collections::{HashMap, HashSet};
use crate::error::{CompileError, Result};
use crate::parse::ast::Type;
use crate::tacky::tacky_ast::*;
use super::asm_ast::{
    AsmFunction, AsmStaticVar, AsmStaticConstant, StaticInit,
    Instruction, Operand, Reg,
    AsmUnaryOp, AsmBinaryOp, CondCode, AsmType,
};

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
        Type::Array(_, _) => AsmType::Quadword,
        Type::Struct { .. } => unreachable!("struct has no single AsmType"),
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

/// safe_asm_type: 構造体型にはプレースホルダーを使う
fn safe_asm_type(t: &Type) -> AsmType {
    if t.is_struct() { AsmType::Quadword } else { type_to_asm(t) }
}

/// コード生成の結果（各関数の命令列 + var_types）
pub struct CodegenFunctionResult {
    pub func: AsmFunction,
    pub var_types: HashMap<String, Type>,
}

/// TACKY プログラムをアセンブリ AST に変換する。
///
/// Chapter 20: 各関数の var_types も返す（レジスタ割り当てで使用）。
pub fn generate(program: &TackyProgram) -> Result<(Vec<CodegenFunctionResult>, Vec<AsmStaticVar>, Vec<AsmStaticConstant>)> {
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

    // double 定数プール: TACKY の static_constants から構築
    let mut double_constants: HashMap<u64, (String, usize)> = HashMap::new();
    for sc in &program.static_constants {
        if let TackyStaticInit::DoubleInit(v) = sc.init {
            double_constants.insert(v.to_bits(), (sc.name.clone(), sc.alignment));
        }
    }

    for func in &program.functions {
        results.push(generate_function(func, &static_vars_set, &mut double_constants)?);
    }

    // 静的変数を AsmStaticVar に変換
    let static_vars: Vec<AsmStaticVar> = program.static_vars.iter().map(|sv| {
        let asm_type = if sv.var_type.is_struct() || sv.var_type.is_array() {
            AsmType::Quadword
        } else {
            type_to_asm(&sv.var_type)
        };
        AsmStaticVar {
            name: sv.name.clone(),
            global: sv.global,
            init: convert_static_init(&sv.init),
            asm_type,
        }
    }).collect();

    // 静的定数を AsmStaticConstant に変換
    let mut static_constants: Vec<AsmStaticConstant> = program.static_constants.iter()
        .filter(|sc| !matches!(sc.init, TackyStaticInit::DoubleInit(_)))
        .map(|sc| {
            AsmStaticConstant {
                name: sc.name.clone(),
                alignment: sc.alignment,
                init: convert_static_init(&sc.init),
            }
        }).collect();

    for (bits, (name, alignment)) in &double_constants {
        static_constants.push(AsmStaticConstant {
            name: name.clone(),
            alignment: *alignment,
            init: StaticInit::DoubleInit(f64::from_bits(*bits)),
        });
    }

    Ok((results, static_vars, static_constants))
}

fn convert_static_init(init: &TackyStaticInit) -> StaticInit {
    match init {
        TackyStaticInit::IntInit(v) => StaticInit::IntInit(*v),
        TackyStaticInit::DoubleInit(v) => StaticInit::DoubleInit(*v),
        TackyStaticInit::ZeroInit(n) => StaticInit::ZeroInit(*n),
        TackyStaticInit::StringInit(s, n) => StaticInit::StringInit(s.clone(), *n),
        TackyStaticInit::ByteArrayInit(bytes) => StaticInit::ByteArrayInit(bytes.clone()),
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

/// Pseudo にしてはいけない変数を収集する（アドレス取得対象・構造体・配列）
fn collect_forced_stack_vars(func: &TackyFunction) -> HashSet<String> {
    let mut forced = HashSet::new();

    for instr in &func.body {
        match instr {
            TackyInstruction::GetAddress { src: TackyVal::Var(name), .. } => {
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

    // 構造体/配列型の変数もスタックに強制配置
    for (name, ty) in &func.var_types {
        if ty.is_struct() || ty.is_array() {
            forced.insert(name.clone());
        }
    }

    forced
}

fn generate_function(
    func: &TackyFunction,
    static_vars: &HashMap<String, String>,
    double_constants: &mut HashMap<u64, (String, usize)>,
) -> Result<CodegenFunctionResult> {
    let mut instructions = Vec::new();
    let forced_stack = collect_forced_stack_vars(func);

    // スタックに強制配置する変数のオフセットを計算
    let mut stack_vars: HashMap<String, i32> = HashMap::new();
    let mut next_offset: i32 = 0;

    // 1. 強制スタック変数を割り当て（パラメータ含む）
    for (name, ty) in &func.var_types {
        if static_vars.contains_key(name) {
            continue;
        }
        if !forced_stack.contains(name) {
            continue;
        }
        if ty.is_void() {
            continue;
        }
        let (size, align) = if ty.is_struct() || ty.is_array() {
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

    // 2. パラメータの処理（Pseudo or Stack）
    let param_types_names: Vec<(Type, String)> = func.params.iter().map(|name| {
        let ty = func.var_types.get(name).cloned().unwrap_or(Type::Int);
        (ty, name.clone())
    }).collect();
    let classification = classify_parameters(&param_types_names);

    for (i, param_name) in func.params.iter().enumerate() {
        let param_type = func.var_types.get(param_name).cloned().unwrap_or(Type::Int);
        let asm_type = safe_asm_type(&param_type);
        let dst_op = resolve_var_operand(param_name, static_vars, &stack_vars);

        let (_, ref loc) = classification.locations[i];
        match loc {
            ParamLocation::IntReg(idx) => {
                instructions.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Register(ARG_REGISTERS[*idx]),
                    dst: dst_op,
                });
            }
            ParamLocation::XmmReg(idx) => {
                instructions.push(Instruction::Mov {
                    asm_type: AsmType::Double,
                    src: Operand::Register(XMM_ARG_REGISTERS[*idx]),
                    dst: dst_op,
                });
            }
            ParamLocation::Stack(stack_offset) => {
                // Stack-passed parameters: need a scratch register to copy
                if param_type.is_double() {
                    instructions.push(Instruction::Mov {
                        asm_type: AsmType::Double,
                        src: Operand::Stack(*stack_offset),
                        dst: Operand::Register(Reg::XMM0),
                    });
                    instructions.push(Instruction::Mov {
                        asm_type: AsmType::Double,
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
    for instr in &func.body {
        generate_instruction(instr, static_vars, &stack_vars, &mut instructions, double_constants, &func.var_types)?;
    }

    Ok(CodegenFunctionResult {
        func: AsmFunction {
            name: func.name.clone(),
            instructions,
            global: func.global,
        },
        var_types: func.var_types.clone(),
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
        TackyVal::Constant(c) => {
            match c {
                TackyConst::Int(v) => Ok(Operand::Imm(*v as i64)),
                TackyConst::Long(v) => Ok(Operand::Imm(*v)),
                TackyConst::UInt(v) => Ok(Operand::Imm(*v as i64)),
                TackyConst::ULong(v) => Ok(Operand::Imm(*v as i64)),
                TackyConst::Char(v) => Ok(Operand::Imm(*v as i64)),
                TackyConst::UChar(v) => Ok(Operand::Imm(*v as i64)),
                TackyConst::Double(_) => {
                    Err(CompileError::CodegenError("double constant should be loaded from memory".to_string()))
                }
            }
        }
        TackyVal::Var(name) => {
            Ok(resolve_var_operand(name, static_vars, stack_vars))
        }
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
            TackyConst::Double(_) => Type::Double,
            TackyConst::Char(_) => Type::Char,
            TackyConst::UChar(_) => Type::UChar,
        },
        TackyVal::Var(name) => {
            var_types.get(name).cloned().unwrap_or(Type::Int)
        }
    }
}

/// double 定数をオペランドにロードする
fn load_double_val(
    val: &TackyVal,
    static_vars: &HashMap<String, String>,
    stack_vars: &HashMap<String, i32>,
    _instrs: &mut Vec<Instruction>,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_label_counter: &mut usize,
) -> Operand {
    match val {
        TackyVal::Constant(TackyConst::Double(v)) => {
            let bits = v.to_bits();
            let label = if let Some((l, _)) = double_constants.get(&bits) {
                l.clone()
            } else {
                let l = format!(".Lconst_{}", *const_label_counter);
                *const_label_counter += 1;
                double_constants.insert(bits, (l.clone(), 8));
                l
            };
            Operand::Data(label)
        }
        TackyVal::Var(name) => {
            resolve_var_operand(name, static_vars, stack_vars)
        }
        _ => unreachable!("expected double val"),
    }
}

/// 単一の TACKY 命令をアセンブリ命令列に変換する
fn generate_instruction(
    instr: &TackyInstruction,
    static_vars: &HashMap<String, String>,
    stack_vars: &HashMap<String, i32>,
    instrs: &mut Vec<Instruction>,
    double_constants: &mut HashMap<u64, (String, usize)>,
    var_types: &HashMap<String, Type>,
) -> Result<()> {
    let mut const_counter: usize = double_constants.len();

    match instr {
        TackyInstruction::Return(val) => {
            let ty = val_type(val, var_types);
            if ty.is_double() {
                let src = load_double_val(val, static_vars, stack_vars, instrs, double_constants, &mut const_counter);
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Double,
                    src,
                    dst: Operand::Register(Reg::XMM0),
                });
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
            instrs.push(Instruction::Ret);
        }

        TackyInstruction::Copy { src, dst } => {
            let dst_type = val_type(dst, var_types);
            if dst_type.is_double() {
                let src_op = load_double_val(src, static_vars, stack_vars, instrs, double_constants, &mut const_counter);
                let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
                // Chapter 20: direct mov (fixup will handle if both are memory)
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Double,
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
                    if dst_type.is_double() {
                        let src_op = load_double_val(src, static_vars, stack_vars, instrs, double_constants, &mut const_counter);
                        let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: src_op,
                            dst: Operand::Register(Reg::XMM0),
                        });
                        let neg_zero_bits = 0x8000000000000000u64;
                        let neg_label = get_or_add_double_constant(neg_zero_bits, 16, double_constants, &mut const_counter);
                        instrs.push(Instruction::Binary {
                            asm_type: AsmType::Double,
                            op: AsmBinaryOp::Xor,
                            src: Operand::Data(neg_label),
                            dst: Operand::Register(Reg::XMM0),
                        });
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
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
                    if src_type.is_double() {
                        let src_op = load_double_val(src, static_vars, stack_vars, instrs, double_constants, &mut const_counter);
                        instrs.push(Instruction::Mov {
                            asm_type: AsmType::Double,
                            src: src_op,
                            dst: Operand::Register(Reg::XMM0),
                        });
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

        TackyInstruction::Binary { op, left, right, dst } => {
            generate_binary_instruction(*op, left, right, dst, static_vars, stack_vars, instrs, double_constants, &mut const_counter, var_types)?;
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

        TackyInstruction::FunCall { name, args, dst, dst_type } => {
            generate_function_call(name, args, dst, dst_type, static_vars, stack_vars, instrs, double_constants, &mut const_counter, var_types)?;
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
            } else {
                instrs.push(Instruction::MovZeroExtend {
                    src: src_op,
                    dst: dst_op,
                });
            }
        }

        TackyInstruction::Truncate { src, dst } => {
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            instrs.push(Instruction::Truncate {
                src: src_op,
                dst: dst_op,
            });
        }

        TackyInstruction::IntToDouble { src, dst } => {
            let src_type = val_type(src, var_types);
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

            instrs.push(Instruction::Cvtsi2sd {
                asm_type: safe_asm_type(&src_type),
                src: src_op,
                dst: dst_op,
            });
        }

        TackyInstruction::DoubleToInt { src, dst } => {
            let dst_type = val_type(dst, var_types);
            let src_op = load_double_val(src, static_vars, stack_vars, instrs, double_constants, &mut const_counter);
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

            instrs.push(Instruction::Cvttsd2si {
                asm_type: safe_asm_type(&dst_type),
                src: src_op,
                dst: dst_op,
            });
        }

        TackyInstruction::UIntToDouble { src, dst } => {
            // unsigned long → double: conditional algorithm (uses hardcoded regs)
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

            let large_label = format!(".Lul2d_large{}", const_counter);
            let end_label = format!(".Lul2d_end{}", const_counter);
            #[allow(unused_assignments)]
            { const_counter += 1; }

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
            let src_op = load_double_val(src, static_vars, stack_vars, instrs, double_constants, &mut const_counter);
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

            let out_of_range_label = format!(".Ld2ul_oor{}", const_counter);
            let end_label = format!(".Ld2ul_end{}", const_counter);
            const_counter += 1;

            let bound_label = get_or_add_double_constant(
                9223372036854775808.0f64.to_bits(), 8, double_constants, &mut const_counter
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

        TackyInstruction::GetAddress { src, dst } => {
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
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

            if dst_type.is_double() {
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Double,
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

            if src_type.is_double() {
                let src_op = load_double_val(src, static_vars, stack_vars, instrs, double_constants, &mut const_counter);
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Double,
                    src: src_op,
                    dst: Operand::Register(Reg::XMM0),
                });
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Double,
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

        TackyInstruction::AddPtr { ptr, index, scale, dst } => {
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

            if src_type.is_double() {
                let src_op = load_double_val(src, static_vars, stack_vars, instrs, double_constants, &mut const_counter);
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Double,
                    src: src_op,
                    dst: Operand::Register(Reg::XMM0),
                });
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Double,
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

            if dst_type.is_double() {
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Double,
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

        TackyInstruction::CopyStruct { src, dst, size } => {
            let src_op = val_to_operand(src, static_vars, stack_vars)?;
            let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
            let src_type = val_type(src, var_types);
            let dst_type = val_type(dst, var_types);

            if src_type.is_struct() {
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
            if dst_type.is_struct() {
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
    }

    Ok(())
}

fn get_or_add_double_constant(
    bits: u64,
    alignment: usize,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_counter: &mut usize,
) -> String {
    if let Some((label, existing_align)) = double_constants.get_mut(&bits) {
        if alignment > *existing_align {
            *existing_align = alignment;
        }
        label.clone()
    } else {
        let label = format!(".Lconst_{}", *const_counter);
        *const_counter += 1;
        double_constants.insert(bits, (label.clone(), alignment));
        label
    }
}

fn generate_binary_instruction(
    op: TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    static_vars: &HashMap<String, String>,
    stack_vars: &HashMap<String, i32>,
    instrs: &mut Vec<Instruction>,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_counter: &mut usize,
    var_types: &HashMap<String, Type>,
) -> Result<()> {
    let left_type = val_type(left, var_types);
    let dst_type = val_type(dst, var_types);
    let dst_op = val_to_operand(dst, static_vars, stack_vars)?;

    match op {
        TackyBinaryOp::AddDouble | TackyBinaryOp::SubDouble
        | TackyBinaryOp::MulDouble | TackyBinaryOp::DivDouble => {
            let left_op = load_double_val(left, static_vars, stack_vars, instrs, double_constants, const_counter);
            let right_op = load_double_val(right, static_vars, stack_vars, instrs, double_constants, const_counter);
            let asm_op = match op {
                TackyBinaryOp::AddDouble => AsmBinaryOp::Add,
                TackyBinaryOp::SubDouble => AsmBinaryOp::Sub,
                TackyBinaryOp::MulDouble => AsmBinaryOp::Mult,
                TackyBinaryOp::DivDouble => AsmBinaryOp::DivDouble,
                _ => unreachable!(),
            };
            // Chapter 20: Mov left→dst; Binary right,dst
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

        TackyBinaryOp::Equal | TackyBinaryOp::NotEqual
        | TackyBinaryOp::LessThan | TackyBinaryOp::LessOrEqual
        | TackyBinaryOp::GreaterThan | TackyBinaryOp::GreaterOrEqual => {
            let is_double = left_type.is_double();
            let is_unsigned = left_type.is_unsigned() || left_type.is_pointer();

            if is_double {
                let left_op = load_double_val(left, static_vars, stack_vars, instrs, double_constants, const_counter);
                let right_op = load_double_val(right, static_vars, stack_vars, instrs, double_constants, const_counter);
                // comisd needs register operands for both, keep using XMM regs
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Double,
                    src: left_op,
                    dst: Operand::Register(Reg::XMM14),
                });
                instrs.push(Instruction::Mov {
                    asm_type: AsmType::Double,
                    src: right_op,
                    dst: Operand::Register(Reg::XMM0),
                });
                instrs.push(Instruction::Cmp {
                    asm_type: AsmType::Double,
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

            let cc = if is_double || is_unsigned {
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
                operand: dst_op,
            });
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
            instrs.push(Instruction::Mov { asm_type, src: left_op, dst: dst_op.clone() });
            instrs.push(Instruction::Binary {
                asm_type,
                op: asm_op,
                src: right_op,
                dst: dst_op,
            });
        }

        TackyBinaryOp::Divide | TackyBinaryOp::Remainder => {
            let asm_type = safe_asm_type(&dst_type);
            let left_op = val_to_operand(left, static_vars, stack_vars)?;
            let right_op = val_to_operand(right, static_vars, stack_vars)?;

            // Division must use hardcoded AX/DX
            instrs.push(Instruction::Mov { asm_type, src: left_op, dst: Operand::Register(Reg::AX) });

            if dst_type.is_unsigned() {
                instrs.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Imm(0),
                    dst: Operand::Register(Reg::DX),
                });
                instrs.push(Instruction::Div { asm_type, operand: right_op });
            } else {
                instrs.push(Instruction::SignExtend(asm_type));
                instrs.push(Instruction::Idiv { asm_type, operand: right_op });
            }

            let result_reg = if matches!(op, TackyBinaryOp::Remainder) {
                Reg::DX
            } else {
                Reg::AX
            };
            instrs.push(Instruction::Mov { asm_type, src: Operand::Register(result_reg), dst: dst_op });
        }
    }

    Ok(())
}

fn generate_function_call(
    name: &str,
    args: &[TackyVal],
    dst: &TackyVal,
    dst_type: &Type,
    static_vars: &HashMap<String, String>,
    stack_vars: &HashMap<String, i32>,
    instrs: &mut Vec<Instruction>,
    double_constants: &mut HashMap<u64, (String, usize)>,
    const_counter: &mut usize,
    var_types: &HashMap<String, Type>,
) -> Result<()> {
    let arg_types: Vec<Type> = args.iter().map(|a| val_type(a, var_types)).collect();
    let typed_params: Vec<(Type, String)> = arg_types.iter()
        .map(|t| (t.clone(), String::new()))
        .collect();
    let call_class = classify_parameters(&typed_params);

    let stack_count = call_class.stack_count;
    let padding = if stack_count % 2 != 0 { 8 } else { 0 };

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
        if arg_types[i].is_double() {
            let src = load_double_val(&args[i], static_vars, stack_vars, instrs, double_constants, const_counter);
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Double,
                src,
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::Push(Operand::Register(Reg::XMM0)));
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
        let asm_type = safe_asm_type(&arg_types[arg_idx]);
        instrs.push(Instruction::Mov {
            asm_type,
            src,
            dst: Operand::Register(Reg::AX),
        });
        instrs.push(Instruction::Push(Operand::Register(Reg::AX)));
    }
    for &(_, reg_idx) in &int_reg_args {
        instrs.push(Instruction::Pop(Operand::Register(ARG_REGISTERS[reg_idx])));
    }

    // XMM register args
    if xmm_reg_args.len() == 1 {
        let (arg_idx, reg_idx) = xmm_reg_args[0];
        let src = load_double_val(&args[arg_idx], static_vars, stack_vars, instrs, double_constants, const_counter);
        instrs.push(Instruction::Mov {
            asm_type: AsmType::Double,
            src,
            dst: Operand::Register(Reg::XMM0),
        });
        if reg_idx != 0 {
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Double,
                src: Operand::Register(Reg::XMM0),
                dst: Operand::Register(XMM_ARG_REGISTERS[reg_idx]),
            });
        }
    } else if xmm_reg_args.len() > 1 {
        for &(arg_idx, _) in xmm_reg_args.iter().rev() {
            let src = load_double_val(&args[arg_idx], static_vars, stack_vars, instrs, double_constants, const_counter);
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Double,
                src,
                dst: Operand::Register(Reg::XMM0),
            });
            instrs.push(Instruction::Push(Operand::Register(Reg::XMM0)));
        }
        for &(_, reg_idx) in &xmm_reg_args {
            instrs.push(Instruction::Pop(Operand::Register(XMM_ARG_REGISTERS[reg_idx])));
        }
    }

    instrs.push(Instruction::Call(name.to_string()));

    let dealloc = stack_count * 8 + padding;
    if dealloc > 0 {
        instrs.push(Instruction::DeallocateStack(dealloc));
    }

    // Store result
    if !dst_type.is_void() {
        let dst_op = val_to_operand(dst, static_vars, stack_vars)?;
        if dst_type.is_double() {
            instrs.push(Instruction::Mov {
                asm_type: AsmType::Double,
                src: Operand::Register(Reg::XMM0),
                dst: dst_op,
            });
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
