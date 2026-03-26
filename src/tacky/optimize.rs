//! TACKY IR 最適化パス
//!
//! TACKY → TACKY の変換を行う最適化パス。
//! 各パスは独立して適用でき、収束するまで繰り返す。
//!
//! 1. Algebraic Simplifications（代数的簡略化）
//! 2. Constant Folding（定数畳み込み）
//! 3. Unreachable Code Elimination（到達不能コード除去）
//! 4. Copy Propagation（コピー伝播）
//! 5. Common Subexpression Elimination（共通部分式除去）
//! 6. Liveness Dead Code Elimination（生存解析ベース死コード除去）

use super::tacky_ast::*;
use crate::parse::ast::Type;
use std::collections::{HashMap, HashSet};

// ────────────────────────────────────────────
// CFG（制御フローグラフ）
// ────────────────────────────────────────────

#[allow(dead_code)]
struct Cfg {
    blocks: Vec<(usize, usize)>, // (start, end) exclusive
    successors: Vec<Vec<usize>>,
    predecessors: Vec<Vec<usize>>,
    label_to_block: HashMap<String, usize>,
}

fn build_cfg(instrs: &[TackyInstruction]) -> Cfg {
    if instrs.is_empty() {
        return Cfg {
            blocks: vec![],
            successors: vec![],
            predecessors: vec![],
            label_to_block: HashMap::new(),
        };
    }

    // Identify block start indices
    let mut block_starts: Vec<usize> = vec![0];
    let mut label_to_idx: HashMap<String, usize> = HashMap::new();

    for (i, instr) in instrs.iter().enumerate() {
        match instr {
            TackyInstruction::Label(label) => {
                if i > 0 {
                    block_starts.push(i);
                }
                label_to_idx.insert(label.clone(), i);
            }
            TackyInstruction::Jump(_)
            | TackyInstruction::Return(_)
            | TackyInstruction::ReturnVoid
            | TackyInstruction::JumpIndirect { .. } => {
                if i + 1 < instrs.len() {
                    block_starts.push(i + 1);
                }
            }
            TackyInstruction::JumpIfZero { .. } | TackyInstruction::JumpIfNotZero { .. } => {
                if i + 1 < instrs.len() {
                    block_starts.push(i + 1);
                }
            }
            _ => {}
        }
    }

    block_starts.sort();
    block_starts.dedup();

    // Build block ranges
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for i in 0..block_starts.len() {
        let start = block_starts[i];
        let end = if i + 1 < block_starts.len() {
            block_starts[i + 1]
        } else {
            instrs.len()
        };
        blocks.push((start, end));
    }

    let num_blocks = blocks.len();

    // Map instruction index → block index (binary search on sorted block_starts)
    let instr_to_block = |idx: usize| -> usize {
        match block_starts.binary_search(&idx) {
            Ok(bi) => bi,
            Err(bi) => bi - 1,
        }
    };

    // Build label → block mapping
    let mut label_to_block: HashMap<String, usize> = HashMap::new();
    for (label, &idx) in &label_to_idx {
        label_to_block.insert(label.clone(), instr_to_block(idx));
    }

    // Build successors
    let mut successors: Vec<Vec<usize>> = vec![vec![]; num_blocks];
    for bi in 0..num_blocks {
        let (_, end) = blocks[bi];
        if end == 0 {
            continue;
        }
        let last = &instrs[end - 1];

        match last {
            TackyInstruction::Jump(label) => {
                if let Some(&target) = label_to_block.get(label) {
                    successors[bi].push(target);
                }
            }
            TackyInstruction::JumpIfZero { target, .. }
            | TackyInstruction::JumpIfNotZero { target, .. } => {
                if bi + 1 < num_blocks {
                    successors[bi].push(bi + 1);
                }
                if let Some(&target_block) = label_to_block.get(target) {
                    successors[bi].push(target_block);
                }
            }
            TackyInstruction::JumpIndirect {
                possible_targets, ..
            } => {
                for label in possible_targets {
                    if let Some(&target_block) = label_to_block.get(label) {
                        successors[bi].push(target_block);
                    }
                }
            }
            TackyInstruction::Return(_) | TackyInstruction::ReturnVoid => {
                // No successors
            }
            _ => {
                if bi + 1 < num_blocks {
                    successors[bi].push(bi + 1);
                }
            }
        }
    }

    // Build predecessors from successors
    let mut predecessors: Vec<Vec<usize>> = vec![vec![]; num_blocks];
    for (bi, succs) in successors.iter().enumerate() {
        for &s in succs {
            predecessors[s].push(bi);
        }
    }

    Cfg {
        blocks,
        successors,
        predecessors,
        label_to_block,
    }
}

// ────────────────────────────────────────────
// optimize() — メインエントリポイント
// ────────────────────────────────────────────

/// 最適化パスを収束するまで繰り返し適用する
pub fn optimize(program: TackyProgram) -> TackyProgram {
    let mut program = program;
    let max_iterations = 10;

    for _ in 0..max_iterations {
        let mut changed = false;

        for func in &mut program.functions {
            let old_body = func.body.clone();
            func.body = algebraic_simplifications(std::mem::take(&mut func.body), &func.var_types);
            func.body = constant_folding(std::mem::take(&mut func.body));
            func.body = unreachable_code_elimination(std::mem::take(&mut func.body));
            func.body = copy_propagation(std::mem::take(&mut func.body));
            func.body = common_subexpression_elimination(std::mem::take(&mut func.body));
            func.body = liveness_dead_code_elimination(
                std::mem::take(&mut func.body),
                &func.var_types,
                &func.static_var_names,
            );
            if func.body != old_body {
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    program
}

// ────────────────────────────────────────────
// Pass 1: Algebraic Simplifications（代数的簡略化）
// ────────────────────────────────────────────

fn algebraic_simplifications(
    instrs: Vec<TackyInstruction>,
    var_types: &HashMap<String, Type>,
) -> Vec<TackyInstruction> {
    instrs
        .into_iter()
        .map(|instr| match &instr {
            TackyInstruction::Binary {
                op,
                left,
                right,
                dst,
            } => {
                if let Some(simplified) = simplify_binary(*op, left, right, dst, var_types) {
                    return simplified;
                }
                instr
            }
            _ => instr,
        })
        .collect()
}

fn simplify_binary(
    op: TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    var_types: &HashMap<String, Type>,
) -> Option<TackyInstruction> {
    let left_zero = is_zero_val(left);
    let right_zero = is_zero_val(right);
    let left_one = is_one(left);
    let right_one = is_one(right);
    let same_var = is_same_var(left, right);

    match op {
        // Integer Add: x+0→x, 0+x→x
        TackyBinaryOp::Add => {
            if right_zero {
                return Some(make_copy(left, dst));
            }
            if left_zero {
                return Some(make_copy(right, dst));
            }
            None
        }
        // Integer Subtract: x-0→x, x-x→0
        TackyBinaryOp::Subtract => {
            if right_zero {
                return Some(make_copy(left, dst));
            }
            if same_var {
                return Some(make_copy_zero(left, dst, var_types));
            }
            None
        }
        // Integer Multiply: x*1→x, 1*x→x, x*0→0, 0*x→0
        TackyBinaryOp::Multiply => {
            if right_one {
                return Some(make_copy(left, dst));
            }
            if left_one {
                return Some(make_copy(right, dst));
            }
            if right_zero {
                return Some(make_copy_zero(left, dst, var_types));
            }
            if left_zero {
                return Some(make_copy_zero(right, dst, var_types));
            }
            None
        }
        // Integer Divide: x/1→x
        TackyBinaryOp::Divide => {
            if right_one {
                return Some(make_copy(left, dst));
            }
            None
        }
        // Integer Remainder: x%1→0
        TackyBinaryOp::Remainder => {
            if right_one {
                return Some(make_copy_zero(left, dst, var_types));
            }
            None
        }
        // Double Multiply: x*1.0→x, 1.0*x→x
        TackyBinaryOp::MulDouble => {
            if is_one_double(right) {
                return Some(make_copy(left, dst));
            }
            if is_one_double(left) {
                return Some(make_copy(right, dst));
            }
            None
        }
        // Double Divide: x/1.0→x
        TackyBinaryOp::DivDouble => {
            if is_one_double(right) {
                return Some(make_copy(left, dst));
            }
            None
        }
        // Self-comparisons (integer only, not double)
        TackyBinaryOp::Equal => {
            if same_var && !is_double_var_val(left, var_types) {
                return Some(TackyInstruction::Copy {
                    src: TackyVal::Constant(TackyConst::Int(1)),
                    dst: dst.clone(),
                });
            }
            None
        }
        TackyBinaryOp::NotEqual => {
            if same_var && !is_double_var_val(left, var_types) {
                return Some(TackyInstruction::Copy {
                    src: TackyVal::Constant(TackyConst::Int(0)),
                    dst: dst.clone(),
                });
            }
            None
        }
        TackyBinaryOp::LessOrEqual | TackyBinaryOp::GreaterOrEqual => {
            if same_var && !is_double_var_val(left, var_types) {
                return Some(TackyInstruction::Copy {
                    src: TackyVal::Constant(TackyConst::Int(1)),
                    dst: dst.clone(),
                });
            }
            None
        }
        TackyBinaryOp::LessThan | TackyBinaryOp::GreaterThan => {
            if same_var && !is_double_var_val(left, var_types) {
                return Some(TackyInstruction::Copy {
                    src: TackyVal::Constant(TackyConst::Int(0)),
                    dst: dst.clone(),
                });
            }
            None
        }
        // Bitwise AND: x & 0 = 0
        TackyBinaryOp::BitwiseAnd => {
            if right_zero {
                return Some(make_copy_zero(left, dst, var_types));
            }
            if left_zero {
                return Some(make_copy_zero(right, dst, var_types));
            }
            None
        }
        // Bitwise OR: x | 0 = x, 0 | x = x
        TackyBinaryOp::BitwiseOr => {
            if right_zero {
                return Some(make_copy(left, dst));
            }
            if left_zero {
                return Some(make_copy(right, dst));
            }
            None
        }
        // Bitwise XOR: x ^ 0 = x, 0 ^ x = x
        TackyBinaryOp::BitwiseXor => {
            if right_zero {
                return Some(make_copy(left, dst));
            }
            if left_zero {
                return Some(make_copy(right, dst));
            }
            None
        }
        // Shift left: x << 0 = x
        TackyBinaryOp::ShiftLeft => {
            if right_zero {
                return Some(make_copy(left, dst));
            }
            None
        }
        // Shift right: x >> 0 = x
        TackyBinaryOp::ShiftRight => {
            if right_zero {
                return Some(make_copy(left, dst));
            }
            None
        }
        _ => None,
    }
}

fn is_zero_val(val: &TackyVal) -> bool {
    match val {
        TackyVal::Constant(c) => is_zero_const(c),
        _ => false,
    }
}

fn is_one(val: &TackyVal) -> bool {
    matches!(
        val,
        TackyVal::Constant(TackyConst::Int(1))
            | TackyVal::Constant(TackyConst::Long(1))
            | TackyVal::Constant(TackyConst::UInt(1))
            | TackyVal::Constant(TackyConst::ULong(1))
            | TackyVal::Constant(TackyConst::Char(1))
            | TackyVal::Constant(TackyConst::UChar(1))
    )
}

fn is_one_double(val: &TackyVal) -> bool {
    matches!(val, TackyVal::Constant(TackyConst::Double(v)) if *v == 1.0)
}

fn is_same_var(a: &TackyVal, b: &TackyVal) -> bool {
    match (a, b) {
        (TackyVal::Var(na), TackyVal::Var(nb)) => na == nb,
        _ => false,
    }
}

fn is_double_var_val(val: &TackyVal, var_types: &HashMap<String, Type>) -> bool {
    match val {
        TackyVal::Var(name) => matches!(var_types.get(name), Some(Type::Double)),
        _ => false,
    }
}

fn make_copy(src: &TackyVal, dst: &TackyVal) -> TackyInstruction {
    TackyInstruction::Copy {
        src: src.clone(),
        dst: dst.clone(),
    }
}

fn make_zero_typed(val: &TackyVal, var_types: &HashMap<String, Type>) -> TackyConst {
    match val {
        TackyVal::Constant(c) => match c {
            TackyConst::Int(_) => TackyConst::Int(0),
            TackyConst::Long(_) => TackyConst::Long(0),
            TackyConst::UInt(_) => TackyConst::UInt(0),
            TackyConst::ULong(_) => TackyConst::ULong(0),
            TackyConst::Double(_) => TackyConst::Double(0.0),
            TackyConst::Char(_) => TackyConst::Char(0),
            TackyConst::UChar(_) => TackyConst::UChar(0),
        },
        TackyVal::Var(name) => match var_types.get(name) {
            Some(Type::Int) => TackyConst::Int(0),
            Some(Type::Long) => TackyConst::Long(0),
            Some(Type::UInt) => TackyConst::UInt(0),
            Some(Type::ULong) => TackyConst::ULong(0),
            Some(Type::Double) => TackyConst::Double(0.0),
            Some(Type::Char) => TackyConst::Char(0),
            Some(Type::UChar) => TackyConst::UChar(0),
            _ => TackyConst::Int(0),
        },
    }
}

fn make_copy_zero(
    operand: &TackyVal,
    dst: &TackyVal,
    var_types: &HashMap<String, Type>,
) -> TackyInstruction {
    TackyInstruction::Copy {
        src: TackyVal::Constant(make_zero_typed(operand, var_types)),
        dst: dst.clone(),
    }
}

// ────────────────────────────────────────────
// Pass 2: Constant Folding（定数畳み込み）
// ────────────────────────────────────────────

/// Constant Folding — 定数式をコンパイル時に計算する
fn constant_folding(instrs: Vec<TackyInstruction>) -> Vec<TackyInstruction> {
    instrs
        .into_iter()
        .filter_map(|instr| {
            match &instr {
                TackyInstruction::Binary {
                    op,
                    left,
                    right,
                    dst,
                } => {
                    if let (TackyVal::Constant(lc), TackyVal::Constant(rc)) = (left, right)
                        && let Some(result) = fold_binary(*op, lc, rc)
                    {
                        return Some(TackyInstruction::Copy {
                            src: TackyVal::Constant(result),
                            dst: dst.clone(),
                        });
                    }
                    Some(instr)
                }
                TackyInstruction::Unary { op, src, dst } => {
                    if let TackyVal::Constant(c) = src
                        && let Some(result) = fold_unary(*op, c)
                    {
                        return Some(TackyInstruction::Copy {
                            src: TackyVal::Constant(result),
                            dst: dst.clone(),
                        });
                    }
                    Some(instr)
                }
                TackyInstruction::JumpIfZero { condition, target } => {
                    if let TackyVal::Constant(c) = condition {
                        if is_zero_const(c) {
                            return Some(TackyInstruction::Jump(target.clone()));
                        } else {
                            return None; // Remove dead branch
                        }
                    }
                    Some(instr)
                }
                TackyInstruction::JumpIfNotZero { condition, target } => {
                    if let TackyVal::Constant(c) = condition {
                        if !is_zero_const(c) {
                            return Some(TackyInstruction::Jump(target.clone()));
                        } else {
                            return None; // Remove dead branch
                        }
                    }
                    Some(instr)
                }
                _ => Some(instr),
            }
        })
        .collect()
}

fn is_zero_const(c: &TackyConst) -> bool {
    match c {
        TackyConst::Int(0) => true,
        TackyConst::Long(0) => true,
        TackyConst::UInt(0) => true,
        TackyConst::ULong(0) => true,
        TackyConst::Double(v) => *v == 0.0,
        TackyConst::Char(0) => true,
        TackyConst::UChar(0) => true,
        _ => false,
    }
}

fn fold_binary(op: TackyBinaryOp, left: &TackyConst, right: &TackyConst) -> Option<TackyConst> {
    match (left, right) {
        (TackyConst::Int(l), TackyConst::Int(r)) => {
            fold_int_binary(op, *l as i64, *r as i64).map(|v| TackyConst::Int(v as i32))
        }
        (TackyConst::Long(l), TackyConst::Long(r)) => {
            fold_int_binary(op, *l, *r).map(TackyConst::Long)
        }
        (TackyConst::UInt(l), TackyConst::UInt(r)) => {
            fold_uint_binary(op, *l as u64, *r as u64).map(|v| TackyConst::UInt(v as u32))
        }
        (TackyConst::ULong(l), TackyConst::ULong(r)) => {
            fold_uint_binary(op, *l, *r).map(TackyConst::ULong)
        }
        (TackyConst::Double(l), TackyConst::Double(r)) => fold_double_binary(op, *l, *r),
        _ => None,
    }
}

fn fold_int_binary(op: TackyBinaryOp, l: i64, r: i64) -> Option<i64> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Subtract => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Multiply => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Divide => {
            if r != 0 {
                Some(l.wrapping_div(r))
            } else {
                None
            }
        }
        TackyBinaryOp::Remainder => {
            if r != 0 {
                Some(l.wrapping_rem(r))
            } else {
                None
            }
        }
        TackyBinaryOp::Equal => Some(if l == r { 1 } else { 0 }),
        TackyBinaryOp::NotEqual => Some(if l != r { 1 } else { 0 }),
        TackyBinaryOp::LessThan => Some(if l < r { 1 } else { 0 }),
        TackyBinaryOp::LessOrEqual => Some(if l <= r { 1 } else { 0 }),
        TackyBinaryOp::GreaterThan => Some(if l > r { 1 } else { 0 }),
        TackyBinaryOp::GreaterOrEqual => Some(if l >= r { 1 } else { 0 }),
        TackyBinaryOp::BitwiseAnd => Some(l & r),
        TackyBinaryOp::BitwiseOr => Some(l | r),
        TackyBinaryOp::BitwiseXor => Some(l ^ r),
        TackyBinaryOp::ShiftLeft => Some(l.wrapping_shl(r as u32)),
        TackyBinaryOp::ShiftRight => Some(l.wrapping_shr(r as u32)),
        _ => None,
    }
}

fn fold_uint_binary(op: TackyBinaryOp, l: u64, r: u64) -> Option<u64> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Subtract => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Multiply => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Divide => {
            if r != 0 {
                Some(l.wrapping_div(r))
            } else {
                None
            }
        }
        TackyBinaryOp::Remainder => {
            if r != 0 {
                Some(l.wrapping_rem(r))
            } else {
                None
            }
        }
        TackyBinaryOp::Equal => Some(if l == r { 1 } else { 0 }),
        TackyBinaryOp::NotEqual => Some(if l != r { 1 } else { 0 }),
        TackyBinaryOp::LessThan => Some(if l < r { 1 } else { 0 }),
        TackyBinaryOp::LessOrEqual => Some(if l <= r { 1 } else { 0 }),
        TackyBinaryOp::GreaterThan => Some(if l > r { 1 } else { 0 }),
        TackyBinaryOp::GreaterOrEqual => Some(if l >= r { 1 } else { 0 }),
        TackyBinaryOp::BitwiseAnd => Some(l & r),
        TackyBinaryOp::BitwiseOr => Some(l | r),
        TackyBinaryOp::BitwiseXor => Some(l ^ r),
        TackyBinaryOp::ShiftLeft => Some(l.wrapping_shl(r as u32)),
        TackyBinaryOp::ShiftRight => Some(l.wrapping_shr(r as u32)),
        _ => None,
    }
}

fn fold_double_binary(op: TackyBinaryOp, l: f64, r: f64) -> Option<TackyConst> {
    match op {
        TackyBinaryOp::AddDouble => Some(TackyConst::Double(l + r)),
        TackyBinaryOp::SubDouble => Some(TackyConst::Double(l - r)),
        TackyBinaryOp::MulDouble => Some(TackyConst::Double(l * r)),
        TackyBinaryOp::DivDouble => {
            if r != 0.0 {
                Some(TackyConst::Double(l / r))
            } else {
                None
            }
        }
        TackyBinaryOp::Equal => Some(TackyConst::Int(if l == r { 1 } else { 0 })),
        TackyBinaryOp::NotEqual => Some(TackyConst::Int(if l != r { 1 } else { 0 })),
        TackyBinaryOp::LessThan => Some(TackyConst::Int(if l < r { 1 } else { 0 })),
        TackyBinaryOp::LessOrEqual => Some(TackyConst::Int(if l <= r { 1 } else { 0 })),
        TackyBinaryOp::GreaterThan => Some(TackyConst::Int(if l > r { 1 } else { 0 })),
        TackyBinaryOp::GreaterOrEqual => Some(TackyConst::Int(if l >= r { 1 } else { 0 })),
        _ => None,
    }
}

fn fold_unary(op: TackyUnaryOp, c: &TackyConst) -> Option<TackyConst> {
    match (op, c) {
        (TackyUnaryOp::Negate, TackyConst::Int(v)) => Some(TackyConst::Int(v.wrapping_neg())),
        (TackyUnaryOp::Negate, TackyConst::Long(v)) => Some(TackyConst::Long(v.wrapping_neg())),
        (TackyUnaryOp::Negate, TackyConst::Double(v)) => Some(TackyConst::Double(-v)),
        (TackyUnaryOp::Complement, TackyConst::Int(v)) => Some(TackyConst::Int(!v)),
        (TackyUnaryOp::Complement, TackyConst::Long(v)) => Some(TackyConst::Long(!v)),
        (TackyUnaryOp::Complement, TackyConst::UInt(v)) => Some(TackyConst::UInt(!v)),
        (TackyUnaryOp::Complement, TackyConst::ULong(v)) => Some(TackyConst::ULong(!v)),
        (TackyUnaryOp::Not, TackyConst::Int(v)) => {
            Some(TackyConst::Int(if *v == 0 { 1 } else { 0 }))
        }
        (TackyUnaryOp::Not, TackyConst::Long(v)) => {
            Some(TackyConst::Int(if *v == 0 { 1 } else { 0 }))
        }
        (TackyUnaryOp::Not, TackyConst::UInt(v)) => {
            Some(TackyConst::Int(if *v == 0 { 1 } else { 0 }))
        }
        (TackyUnaryOp::Not, TackyConst::ULong(v)) => {
            Some(TackyConst::Int(if *v == 0 { 1 } else { 0 }))
        }
        (TackyUnaryOp::Not, TackyConst::Double(v)) => {
            Some(TackyConst::Int(if *v == 0.0 { 1 } else { 0 }))
        }
        _ => None,
    }
}

// ────────────────────────────────────────────
// Pass 3: Unreachable Code Elimination（到達不能コード除去）
// ────────────────────────────────────────────

fn unreachable_code_elimination(instrs: Vec<TackyInstruction>) -> Vec<TackyInstruction> {
    use std::collections::VecDeque;

    if instrs.is_empty() {
        return instrs;
    }

    let cfg = build_cfg(&instrs);

    // BFS from block 0
    let mut reachable: HashSet<usize> = HashSet::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    queue.push_back(0);
    reachable.insert(0);

    while let Some(bi) = queue.pop_front() {
        for &succ in &cfg.successors[bi] {
            if reachable.insert(succ) {
                queue.push_back(succ);
            }
        }
    }

    // Collect reachable instructions
    let mut result = Vec::new();
    for (bi, &(start, end)) in cfg.blocks.iter().enumerate() {
        if reachable.contains(&bi) {
            for instr in &instrs[start..end] {
                result.push(instr.clone());
            }
        }
    }

    result
}

// ────────────────────────────────────────────
// Pass 4: Copy Propagation（コピー伝播）
// ────────────────────────────────────────────

fn copy_propagation(instrs: Vec<TackyInstruction>) -> Vec<TackyInstruction> {
    // Simple single-pass copy propagation
    // Track active copies: dst -> src
    let address_taken = collect_address_taken(&instrs);
    let mut copies: HashMap<String, TackyVal> = HashMap::new();

    instrs
        .into_iter()
        .map(|instr| {
            let instr = replace_uses(instr, &copies);

            // Update copy map
            match &instr {
                TackyInstruction::Copy {
                    src,
                    dst: TackyVal::Var(dst_name),
                } => {
                    // Invalidate any copies that reference dst_name
                    copies.retain(|_, v| {
                        if let TackyVal::Var(vn) = v {
                            vn != dst_name
                        } else {
                            true
                        }
                    });
                    // Add new copy
                    copies.insert(dst_name.clone(), src.clone());
                }
                _ => {
                    // Invalidate copies whose dst is written by this instruction
                    if let Some(dst_name) = get_written_var(&instr) {
                        copies.retain(|k, v| {
                            k != &dst_name
                                && match v {
                                    TackyVal::Var(vn) => vn != &dst_name,
                                    _ => true,
                                }
                        });
                    }
                    // Labels and function calls invalidate all copies (conservative)
                    // Store invalidates copies for address-taken variables
                    match &instr {
                        TackyInstruction::Label(_) => {
                            copies.clear();
                        }
                        TackyInstruction::FunCall { .. } => {
                            copies.clear();
                        }
                        TackyInstruction::Store { .. } => {
                            copies.retain(|k, v| {
                                !address_taken.contains(k)
                                    && match v {
                                        TackyVal::Var(vn) => !address_taken.contains(vn),
                                        _ => true,
                                    }
                            });
                        }
                        _ => {}
                    }
                }
            }

            instr
        })
        .collect()
}

fn replace_uses(instr: TackyInstruction, copies: &HashMap<String, TackyVal>) -> TackyInstruction {
    fn sub(val: &TackyVal, copies: &HashMap<String, TackyVal>) -> TackyVal {
        if let TackyVal::Var(name) = val
            && let Some(replacement) = copies.get(name)
        {
            return replacement.clone();
        }
        val.clone()
    }

    match instr {
        TackyInstruction::Return(val) => TackyInstruction::Return(sub(&val, copies)),
        TackyInstruction::Unary { op, src, dst } => TackyInstruction::Unary {
            op,
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::Binary {
            op,
            left,
            right,
            dst,
        } => TackyInstruction::Binary {
            op,
            left: sub(&left, copies),
            right: sub(&right, copies),
            dst,
        },
        TackyInstruction::Copy { src, dst } => TackyInstruction::Copy {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::JumpIfZero { condition, target } => TackyInstruction::JumpIfZero {
            condition: sub(&condition, copies),
            target,
        },
        TackyInstruction::JumpIfNotZero { condition, target } => TackyInstruction::JumpIfNotZero {
            condition: sub(&condition, copies),
            target,
        },
        TackyInstruction::Store { src, dst_ptr } => TackyInstruction::Store {
            src: sub(&src, copies),
            dst_ptr: sub(&dst_ptr, copies),
        },
        TackyInstruction::Load { src_ptr, dst } => TackyInstruction::Load {
            src_ptr: sub(&src_ptr, copies),
            dst,
        },
        TackyInstruction::SignExtend { src, dst } => TackyInstruction::SignExtend {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::ZeroExtend { src, dst } => TackyInstruction::ZeroExtend {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::Truncate { src, dst } => TackyInstruction::Truncate {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::IntToDouble { src, dst } => TackyInstruction::IntToDouble {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::DoubleToInt { src, dst } => TackyInstruction::DoubleToInt {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::UIntToDouble { src, dst } => TackyInstruction::UIntToDouble {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::DoubleToUInt { src, dst } => TackyInstruction::DoubleToUInt {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::FloatToDouble { src, dst } => TackyInstruction::FloatToDouble {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::DoubleToFloat { src, dst } => TackyInstruction::DoubleToFloat {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::IntToFloat { src, dst } => TackyInstruction::IntToFloat {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::FloatToInt { src, dst } => TackyInstruction::FloatToInt {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::UIntToFloat { src, dst } => TackyInstruction::UIntToFloat {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::FloatToUInt { src, dst } => TackyInstruction::FloatToUInt {
            src: sub(&src, copies),
            dst,
        },
        TackyInstruction::GetAddress { src, dst } => TackyInstruction::GetAddress { src, dst }, // Don't substitute address source
        TackyInstruction::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } => TackyInstruction::AddPtr {
            ptr: sub(&ptr, copies),
            index: sub(&index, copies),
            scale,
            dst,
        },
        TackyInstruction::CopyToOffset { src, dst, offset } => TackyInstruction::CopyToOffset {
            src: sub(&src, copies),
            dst,
            offset,
        },
        TackyInstruction::CopyFromOffset { src, offset, dst } => {
            TackyInstruction::CopyFromOffset { src, offset, dst }
        }
        TackyInstruction::CopyStruct { src, dst, size } => TackyInstruction::CopyStruct {
            src: sub(&src, copies),
            dst,
            size,
        },
        TackyInstruction::FunCall {
            name,
            args,
            dst,
            dst_type,
            is_variadic,
        } => {
            let args = args.iter().map(|a| sub(a, copies)).collect();
            TackyInstruction::FunCall {
                name,
                args,
                dst,
                dst_type,
                is_variadic,
            }
        }
        TackyInstruction::JumpIndirect {
            target,
            possible_targets,
        } => TackyInstruction::JumpIndirect {
            target: sub(&target, copies),
            possible_targets,
        },
        other => other,
    }
}

fn get_written_var(instr: &TackyInstruction) -> Option<String> {
    match instr {
        TackyInstruction::Unary {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::Binary {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::Copy {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::SignExtend {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::ZeroExtend {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::Truncate {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::IntToDouble {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::DoubleToInt {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::UIntToDouble {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::DoubleToUInt {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::FloatToDouble {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::DoubleToFloat {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::IntToFloat {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::FloatToInt {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::UIntToFloat {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::FloatToUInt {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::GetAddress {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::Load {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::AddPtr {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::CopyFromOffset {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstruction::FunCall {
            dst: TackyVal::Var(n),
            ..
        } => Some(n.clone()),
        TackyInstruction::CopyToOffset { dst, .. } => Some(dst.clone()),
        TackyInstruction::Store {
            dst_ptr: TackyVal::Var(n),
            ..
        } => Some(n.clone()),
        TackyInstruction::CopyStruct {
            dst: TackyVal::Var(n),
            ..
        } => Some(n.clone()),
        _ => None,
    }
}

// ────────────────────────────────────────────
// Pass 5: Common Subexpression Elimination（共通部分式除去）
// ────────────────────────────────────────────

/// CSE 用の値表現（f64 を Hash 可能にするラッパー）
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum CseVal {
    Var(String),
    Int(i32),
    Long(i64),
    UInt(u32),
    ULong(u64),
    DoubleBits(u64),
    Char(i8),
    UChar(u8),
}

/// CSE 用の式表現
#[derive(Clone, PartialEq, Eq, Hash)]
enum CseExpr {
    Binary(TackyBinaryOp, CseVal, CseVal),
    Unary(TackyUnaryOp, CseVal),
    SignExtend(CseVal),
    ZeroExtend(CseVal),
    Truncate(CseVal),
    IntToDouble(CseVal),
    DoubleToInt(CseVal),
    UIntToDouble(CseVal),
    DoubleToUInt(CseVal),
    FloatToDouble(CseVal),
    DoubleToFloat(CseVal),
    IntToFloat(CseVal),
    FloatToInt(CseVal),
    UIntToFloat(CseVal),
    FloatToUInt(CseVal),
}

fn tacky_val_to_cse(val: &TackyVal) -> CseVal {
    match val {
        TackyVal::Var(name) => CseVal::Var(name.clone()),
        TackyVal::Constant(c) => match c {
            TackyConst::Int(v) => CseVal::Int(*v),
            TackyConst::Long(v) => CseVal::Long(*v),
            TackyConst::UInt(v) => CseVal::UInt(*v),
            TackyConst::ULong(v) => CseVal::ULong(*v),
            TackyConst::Double(v) => CseVal::DoubleBits(v.to_bits()),
            TackyConst::Char(v) => CseVal::Char(*v),
            TackyConst::UChar(v) => CseVal::UChar(*v),
        },
    }
}

fn is_commutative(op: TackyBinaryOp) -> bool {
    matches!(
        op,
        TackyBinaryOp::Add
            | TackyBinaryOp::Multiply
            | TackyBinaryOp::Equal
            | TackyBinaryOp::NotEqual
            | TackyBinaryOp::AddDouble
            | TackyBinaryOp::MulDouble
            | TackyBinaryOp::BitwiseAnd
            | TackyBinaryOp::BitwiseOr
            | TackyBinaryOp::BitwiseXor
    )
}

fn common_subexpression_elimination(instrs: Vec<TackyInstruction>) -> Vec<TackyInstruction> {
    let mut available: HashMap<CseExpr, String> = HashMap::new();
    let mut result = Vec::with_capacity(instrs.len());

    for instr in instrs {
        // Try to build a CSE key for this instruction
        let cse_info = match &instr {
            TackyInstruction::Binary {
                op,
                left,
                right,
                dst,
            } => {
                let (l, r) = if is_commutative(*op) {
                    let lv = tacky_val_to_cse(left);
                    let rv = tacky_val_to_cse(right);
                    if lv <= rv { (lv, rv) } else { (rv, lv) }
                } else {
                    (tacky_val_to_cse(left), tacky_val_to_cse(right))
                };
                Some((CseExpr::Binary(*op, l, r), dst))
            }
            TackyInstruction::Unary { op, src, dst } => {
                Some((CseExpr::Unary(*op, tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::SignExtend { src, dst } => {
                Some((CseExpr::SignExtend(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::ZeroExtend { src, dst } => {
                Some((CseExpr::ZeroExtend(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::Truncate { src, dst } => {
                Some((CseExpr::Truncate(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::IntToDouble { src, dst } => {
                Some((CseExpr::IntToDouble(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::DoubleToInt { src, dst } => {
                Some((CseExpr::DoubleToInt(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::UIntToDouble { src, dst } => {
                Some((CseExpr::UIntToDouble(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::DoubleToUInt { src, dst } => {
                Some((CseExpr::DoubleToUInt(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::FloatToDouble { src, dst } => {
                Some((CseExpr::FloatToDouble(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::DoubleToFloat { src, dst } => {
                Some((CseExpr::DoubleToFloat(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::IntToFloat { src, dst } => {
                Some((CseExpr::IntToFloat(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::FloatToInt { src, dst } => {
                Some((CseExpr::FloatToInt(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::UIntToFloat { src, dst } => {
                Some((CseExpr::UIntToFloat(tacky_val_to_cse(src)), dst))
            }
            TackyInstruction::FloatToUInt { src, dst } => {
                Some((CseExpr::FloatToUInt(tacky_val_to_cse(src)), dst))
            }
            _ => None,
        };

        if let Some((key, dst)) = cse_info {
            if let Some(cached_var) = available.get(&key) {
                // Replace with Copy from cached result
                result.push(TackyInstruction::Copy {
                    src: TackyVal::Var(cached_var.clone()),
                    dst: dst.clone(),
                });
                // Invalidate entries referencing the dst variable
                if let TackyVal::Var(dst_name) = dst {
                    invalidate_cse_var(&mut available, dst_name);
                }
                continue;
            }
            // Register in available map
            if let TackyVal::Var(dst_name) = dst {
                // Invalidate entries referencing the dst variable before registering
                invalidate_cse_var(&mut available, dst_name);
                available.insert(key, dst_name.clone());
            }
            result.push(instr);
            continue;
        }

        // Non-CSE instruction: invalidate if it writes a variable
        if let Some(written) = get_written_var(&instr) {
            invalidate_cse_var(&mut available, &written);
        }

        // Invalidation triggers: labels, jumps, calls, stores, memory ops
        match &instr {
            TackyInstruction::Label(_)
            | TackyInstruction::FunCall { .. }
            | TackyInstruction::Store { .. }
            | TackyInstruction::CopyToOffset { .. }
            | TackyInstruction::CopyStruct { .. }
            | TackyInstruction::Jump(_)
            | TackyInstruction::JumpIfZero { .. }
            | TackyInstruction::JumpIfNotZero { .. }
            | TackyInstruction::JumpIndirect { .. }
            | TackyInstruction::Return(_)
            | TackyInstruction::ReturnVoid => {
                available.clear();
            }
            _ => {}
        }

        result.push(instr);
    }

    result
}

/// CSE available マップから、指定変数を参照するエントリを全て削除
fn invalidate_cse_var(available: &mut HashMap<CseExpr, String>, var_name: &str) {
    available.retain(|key, val| {
        // Remove if the cached result variable matches
        if val == var_name {
            return false;
        }
        // Remove if any operand in the expression references var_name
        match key {
            CseExpr::Binary(_, l, r) => {
                !cse_val_is_var(l, var_name) && !cse_val_is_var(r, var_name)
            }
            CseExpr::Unary(_, v)
            | CseExpr::SignExtend(v)
            | CseExpr::ZeroExtend(v)
            | CseExpr::Truncate(v)
            | CseExpr::IntToDouble(v)
            | CseExpr::DoubleToInt(v)
            | CseExpr::UIntToDouble(v)
            | CseExpr::DoubleToUInt(v)
            | CseExpr::FloatToDouble(v)
            | CseExpr::DoubleToFloat(v)
            | CseExpr::IntToFloat(v)
            | CseExpr::FloatToInt(v)
            | CseExpr::UIntToFloat(v)
            | CseExpr::FloatToUInt(v) => !cse_val_is_var(v, var_name),
        }
    });
}

fn cse_val_is_var(val: &CseVal, name: &str) -> bool {
    matches!(val, CseVal::Var(n) if n == name)
}

// ────────────────────────────────────────────
// Pass 6: Liveness Dead Code Elimination（生存解析ベース死コード除去）
// ────────────────────────────────────────────

fn liveness_dead_code_elimination(
    instrs: Vec<TackyInstruction>,
    _var_types: &HashMap<String, Type>,
    static_var_names: &HashSet<String>,
) -> Vec<TackyInstruction> {
    if instrs.is_empty() {
        return instrs;
    }

    let cfg = build_cfg(&instrs);
    if cfg.blocks.is_empty() {
        return instrs;
    }

    // Step 1: Collect address-taken variables (always considered live)
    let address_taken = collect_address_taken(&instrs);

    // Step 2: Compute block-level use/def sets
    let num_blocks = cfg.blocks.len();
    let mut block_use: Vec<HashSet<String>> = vec![HashSet::new(); num_blocks];
    let mut block_def: Vec<HashSet<String>> = vec![HashSet::new(); num_blocks];

    for (bi, &(start, end)) in cfg.blocks.iter().enumerate() {
        // Process instructions in forward order for block-level use/def
        for instr in &instrs[start..end] {
            // Collect uses that are NOT already defined in this block
            let mut instr_uses = HashSet::new();
            collect_uses(instr, &mut instr_uses);
            for u in instr_uses {
                if !block_def[bi].contains(&u) {
                    block_use[bi].insert(u);
                }
            }
            // Collect def
            if let Some(def) = get_written_var(instr) {
                block_def[bi].insert(def);
            }
        }
    }

    // Step 3: Fixed-point iteration for live_in / live_out
    let mut live_in: Vec<HashSet<String>> = vec![HashSet::new(); num_blocks];
    let mut live_out: Vec<HashSet<String>> = vec![HashSet::new(); num_blocks];

    let mut changed = true;
    while changed {
        changed = false;
        for bi in (0..num_blocks).rev() {
            // live_out[b] = union of live_in[s] for s in successors[b]
            let mut new_out: HashSet<String> = HashSet::new();
            for &s in &cfg.successors[bi] {
                for v in &live_in[s] {
                    new_out.insert(v.clone());
                }
            }

            // live_in[b] = use[b] ∪ (live_out[b] \ def[b])
            let mut new_in: HashSet<String> = block_use[bi].clone();
            for v in &new_out {
                if !block_def[bi].contains(v) {
                    new_in.insert(v.clone());
                }
            }

            if new_in != live_in[bi] || new_out != live_out[bi] {
                changed = true;
                live_in[bi] = new_in;
                live_out[bi] = new_out;
            }
        }
    }

    // Step 4: Instruction-level elimination (backward scan within each block)
    let mut keep = vec![true; instrs.len()];

    for (bi, &(start, end)) in cfg.blocks.iter().enumerate() {
        let mut live = live_out[bi].clone();

        for i in (start..end).rev() {
            let instr = &instrs[i];

            if has_side_effect(instr) {
                // Cannot remove; update liveness
                if let Some(def) = get_written_var(instr) {
                    live.remove(&def);
                }
                collect_uses_into(instr, &mut live);
            } else if let Some(def) = get_written_var(instr) {
                if !live.contains(&def)
                    && !address_taken.contains(&def)
                    && !static_var_names.contains(&def)
                {
                    // Dead instruction — mark for removal
                    keep[i] = false;
                } else {
                    // Live instruction — update liveness
                    live.remove(&def);
                    collect_uses_into(instr, &mut live);
                }
            } else {
                // No def, no side effect (shouldn't happen much)
                collect_uses_into(instr, &mut live);
            }
        }
    }

    instrs
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, instr)| instr)
        .collect()
}

fn collect_address_taken(instrs: &[TackyInstruction]) -> HashSet<String> {
    let mut taken = HashSet::new();
    for instr in instrs {
        match instr {
            TackyInstruction::GetAddress {
                src: TackyVal::Var(name),
                ..
            } => {
                taken.insert(name.clone());
            }
            TackyInstruction::CopyToOffset { dst, .. } => {
                taken.insert(dst.clone());
            }
            TackyInstruction::CopyFromOffset { src, .. } => {
                taken.insert(src.clone());
            }
            _ => {}
        }
    }
    taken
}

fn has_side_effect(instr: &TackyInstruction) -> bool {
    matches!(
        instr,
        TackyInstruction::Return(_)
            | TackyInstruction::ReturnVoid
            | TackyInstruction::Jump(_)
            | TackyInstruction::JumpIfZero { .. }
            | TackyInstruction::JumpIfNotZero { .. }
            | TackyInstruction::JumpIndirect { .. }
            | TackyInstruction::Label(_)
            | TackyInstruction::FunCall { .. }
            | TackyInstruction::Store { .. }
            | TackyInstruction::CopyToOffset { .. }
            | TackyInstruction::CopyStruct { .. }
    )
}

fn collect_uses(instr: &TackyInstruction, used: &mut HashSet<String>) {
    fn add_val(val: &TackyVal, used: &mut HashSet<String>) {
        if let TackyVal::Var(name) = val {
            used.insert(name.clone());
        }
    }

    match instr {
        TackyInstruction::Return(val) => add_val(val, used),
        TackyInstruction::Unary { src, .. } => add_val(src, used),
        TackyInstruction::Binary { left, right, .. } => {
            add_val(left, used);
            add_val(right, used);
        }
        TackyInstruction::Copy { src, .. } => add_val(src, used),
        TackyInstruction::JumpIfZero { condition, .. }
        | TackyInstruction::JumpIfNotZero { condition, .. } => add_val(condition, used),
        TackyInstruction::FunCall { name, args, .. } => {
            // name may be a function pointer variable — treat as use
            used.insert(name.clone());
            for a in args {
                add_val(a, used);
            }
        }
        TackyInstruction::SignExtend { src, .. }
        | TackyInstruction::ZeroExtend { src, .. }
        | TackyInstruction::Truncate { src, .. }
        | TackyInstruction::IntToDouble { src, .. }
        | TackyInstruction::DoubleToInt { src, .. }
        | TackyInstruction::UIntToDouble { src, .. }
        | TackyInstruction::DoubleToUInt { src, .. }
        | TackyInstruction::FloatToDouble { src, .. }
        | TackyInstruction::DoubleToFloat { src, .. }
        | TackyInstruction::IntToFloat { src, .. }
        | TackyInstruction::FloatToInt { src, .. }
        | TackyInstruction::UIntToFloat { src, .. }
        | TackyInstruction::FloatToUInt { src, .. } => add_val(src, used),
        TackyInstruction::GetAddress { src, .. } => add_val(src, used),
        TackyInstruction::Load { src_ptr, .. } => add_val(src_ptr, used),
        TackyInstruction::Store { src, dst_ptr } => {
            add_val(src, used);
            add_val(dst_ptr, used);
        }
        TackyInstruction::AddPtr { ptr, index, .. } => {
            add_val(ptr, used);
            add_val(index, used);
        }
        TackyInstruction::CopyToOffset { src, dst, .. } => {
            add_val(src, used);
            used.insert(dst.clone());
        }
        TackyInstruction::CopyFromOffset { src, .. } => {
            used.insert(src.clone());
        }
        TackyInstruction::CopyStruct { src, dst, .. } => {
            add_val(src, used);
            add_val(dst, used);
        }
        TackyInstruction::JumpIndirect { target, .. } => add_val(target, used),
        _ => {}
    }
}

/// collect_uses と同じだが live セットに直接追加する（liveness DCE 用）
fn collect_uses_into(instr: &TackyInstruction, live: &mut HashSet<String>) {
    collect_uses(instr, live);
}
