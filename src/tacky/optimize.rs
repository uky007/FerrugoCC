//! TACKY IR 最適化パス
//!
//! TACKY → TACKY の変換を行う最適化パス。
//! 各パスは独立して適用でき、収束するまで繰り返す。
//!
//! 1. Constant Folding（定数畳み込み）
//! 2. Unreachable Code Elimination（到達不能コード除去）
//! 3. Copy Propagation（コピー伝播）
//! 4. Dead Store Elimination（無用コード除去）

use super::tacky_ast::*;

/// 最適化パスを収束するまで繰り返し適用する
pub fn optimize(program: TackyProgram) -> TackyProgram {
    let mut program = program;
    let max_iterations = 10;

    for _ in 0..max_iterations {
        let mut changed = false;

        for func in &mut program.functions {
            let old_len = func.body.len();
            func.body = constant_folding(std::mem::take(&mut func.body));
            func.body = unreachable_code_elimination(std::mem::take(&mut func.body));
            func.body = copy_propagation(std::mem::take(&mut func.body));
            func.body = dead_store_elimination(std::mem::take(&mut func.body), &func.var_types);
            if func.body.len() != old_len {
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    program
}

/// Constant Folding — 定数式をコンパイル時に計算する
fn constant_folding(instrs: Vec<TackyInstruction>) -> Vec<TackyInstruction> {
    instrs.into_iter().filter_map(|instr| {
        match &instr {
            TackyInstruction::Binary { op, left, right, dst } => {
                if let (TackyVal::Constant(lc), TackyVal::Constant(rc)) = (left, right) {
                    if let Some(result) = fold_binary(*op, lc, rc) {
                        return Some(TackyInstruction::Copy {
                            src: TackyVal::Constant(result),
                            dst: dst.clone(),
                        });
                    }
                }
                Some(instr)
            }
            TackyInstruction::Unary { op, src, dst } => {
                if let TackyVal::Constant(c) = src {
                    if let Some(result) = fold_unary(*op, c) {
                        return Some(TackyInstruction::Copy {
                            src: TackyVal::Constant(result),
                            dst: dst.clone(),
                        });
                    }
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
    }).collect()
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
        (TackyConst::Int(l), TackyConst::Int(r)) => fold_int_binary(op, *l as i64, *r as i64).map(|v| TackyConst::Int(v as i32)),
        (TackyConst::Long(l), TackyConst::Long(r)) => fold_int_binary(op, *l, *r).map(TackyConst::Long),
        (TackyConst::UInt(l), TackyConst::UInt(r)) => fold_uint_binary(op, *l as u64, *r as u64).map(|v| TackyConst::UInt(v as u32)),
        (TackyConst::ULong(l), TackyConst::ULong(r)) => fold_uint_binary(op, *l, *r).map(TackyConst::ULong),
        (TackyConst::Double(l), TackyConst::Double(r)) => fold_double_binary(op, *l, *r),
        _ => None,
    }
}

fn fold_int_binary(op: TackyBinaryOp, l: i64, r: i64) -> Option<i64> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Subtract => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Multiply => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Divide => if r != 0 { Some(l.wrapping_div(r)) } else { None },
        TackyBinaryOp::Remainder => if r != 0 { Some(l.wrapping_rem(r)) } else { None },
        TackyBinaryOp::Equal => Some(if l == r { 1 } else { 0 }),
        TackyBinaryOp::NotEqual => Some(if l != r { 1 } else { 0 }),
        TackyBinaryOp::LessThan => Some(if l < r { 1 } else { 0 }),
        TackyBinaryOp::LessOrEqual => Some(if l <= r { 1 } else { 0 }),
        TackyBinaryOp::GreaterThan => Some(if l > r { 1 } else { 0 }),
        TackyBinaryOp::GreaterOrEqual => Some(if l >= r { 1 } else { 0 }),
        _ => None,
    }
}

fn fold_uint_binary(op: TackyBinaryOp, l: u64, r: u64) -> Option<u64> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Subtract => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Multiply => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Divide => if r != 0 { Some(l.wrapping_div(r)) } else { None },
        TackyBinaryOp::Remainder => if r != 0 { Some(l.wrapping_rem(r)) } else { None },
        TackyBinaryOp::Equal => Some(if l == r { 1 } else { 0 }),
        TackyBinaryOp::NotEqual => Some(if l != r { 1 } else { 0 }),
        TackyBinaryOp::LessThan => Some(if l < r { 1 } else { 0 }),
        TackyBinaryOp::LessOrEqual => Some(if l <= r { 1 } else { 0 }),
        TackyBinaryOp::GreaterThan => Some(if l > r { 1 } else { 0 }),
        TackyBinaryOp::GreaterOrEqual => Some(if l >= r { 1 } else { 0 }),
        _ => None,
    }
}

fn fold_double_binary(op: TackyBinaryOp, l: f64, r: f64) -> Option<TackyConst> {
    match op {
        TackyBinaryOp::AddDouble => Some(TackyConst::Double(l + r)),
        TackyBinaryOp::SubDouble => Some(TackyConst::Double(l - r)),
        TackyBinaryOp::MulDouble => Some(TackyConst::Double(l * r)),
        TackyBinaryOp::DivDouble => if r != 0.0 { Some(TackyConst::Double(l / r)) } else { None },
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
        (TackyUnaryOp::Not, TackyConst::Int(v)) => Some(TackyConst::Int(if *v == 0 { 1 } else { 0 })),
        (TackyUnaryOp::Not, TackyConst::Long(v)) => Some(TackyConst::Int(if *v == 0 { 1 } else { 0 })),
        (TackyUnaryOp::Not, TackyConst::UInt(v)) => Some(TackyConst::Int(if *v == 0 { 1 } else { 0 })),
        (TackyUnaryOp::Not, TackyConst::ULong(v)) => Some(TackyConst::Int(if *v == 0 { 1 } else { 0 })),
        (TackyUnaryOp::Not, TackyConst::Double(v)) => Some(TackyConst::Int(if *v == 0.0 { 1 } else { 0 })),
        _ => None,
    }
}

/// Unreachable Code Elimination — 到達不能コードを除去する
fn unreachable_code_elimination(instrs: Vec<TackyInstruction>) -> Vec<TackyInstruction> {
    use std::collections::{HashSet, VecDeque};

    if instrs.is_empty() {
        return instrs;
    }

    // Build basic blocks
    let mut block_starts: Vec<usize> = vec![0];
    let mut label_to_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for (i, instr) in instrs.iter().enumerate() {
        match instr {
            TackyInstruction::Label(label) => {
                if i > 0 {
                    block_starts.push(i);
                }
                label_to_idx.insert(label.clone(), i);
            }
            TackyInstruction::Jump(_) | TackyInstruction::Return(_) | TackyInstruction::ReturnVoid => {
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

    // Determine block ranges
    let mut blocks: Vec<(usize, usize)> = Vec::new(); // (start, end) exclusive
    for i in 0..block_starts.len() {
        let start = block_starts[i];
        let end = if i + 1 < block_starts.len() { block_starts[i + 1] } else { instrs.len() };
        blocks.push((start, end));
    }

    // Map instruction index to block index
    let instr_to_block = |idx: usize| -> usize {
        for (bi, (start, end)) in blocks.iter().enumerate() {
            if idx >= *start && idx < *end {
                return bi;
            }
        }
        0
    };

    // Build successor edges
    let mut reachable: HashSet<usize> = HashSet::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    queue.push_back(0);
    reachable.insert(0);

    while let Some(bi) = queue.pop_front() {
        let (_, end) = blocks[bi];
        if end == 0 { continue; }
        let last_idx = end - 1;
        let last = &instrs[last_idx];

        match last {
            TackyInstruction::Jump(label) => {
                if let Some(&target_idx) = label_to_idx.get(label) {
                    let target_block = instr_to_block(target_idx);
                    if reachable.insert(target_block) {
                        queue.push_back(target_block);
                    }
                }
            }
            TackyInstruction::JumpIfZero { target, .. } | TackyInstruction::JumpIfNotZero { target, .. } => {
                // Fall-through
                if bi + 1 < blocks.len() && reachable.insert(bi + 1) {
                    queue.push_back(bi + 1);
                }
                // Branch target
                if let Some(&target_idx) = label_to_idx.get(target) {
                    let target_block = instr_to_block(target_idx);
                    if reachable.insert(target_block) {
                        queue.push_back(target_block);
                    }
                }
            }
            TackyInstruction::Return(_) | TackyInstruction::ReturnVoid => {
                // No successors
            }
            _ => {
                // Fall-through
                if bi + 1 < blocks.len() && reachable.insert(bi + 1) {
                    queue.push_back(bi + 1);
                }
            }
        }
    }

    // Collect reachable instructions
    let mut result = Vec::new();
    for bi in 0..blocks.len() {
        if reachable.contains(&bi) {
            let (start, end) = blocks[bi];
            for i in start..end {
                result.push(instrs[i].clone());
            }
        }
    }

    result
}

/// Copy Propagation — コピー伝播
fn copy_propagation(instrs: Vec<TackyInstruction>) -> Vec<TackyInstruction> {
    use std::collections::HashMap;

    // Simple single-pass copy propagation
    // Track active copies: dst -> src
    let mut copies: HashMap<String, TackyVal> = HashMap::new();

    instrs.into_iter().map(|instr| {
        let instr = replace_uses(instr, &copies);

        // Update copy map
        match &instr {
            TackyInstruction::Copy { src, dst: TackyVal::Var(dst_name) } => {
                // Invalidate any copies that reference dst_name
                copies.retain(|_, v| {
                    if let TackyVal::Var(vn) = v { vn != dst_name } else { true }
                });
                // Add new copy
                copies.insert(dst_name.clone(), src.clone());
            }
            _ => {
                // Invalidate copies whose dst is written by this instruction
                if let Some(dst_name) = get_written_var(&instr) {
                    copies.retain(|k, v| {
                        k != &dst_name && match v { TackyVal::Var(vn) => vn != &dst_name, _ => true }
                    });
                }
                // Labels and jumps invalidate all copies (conservative)
                match &instr {
                    TackyInstruction::Label(_) => { copies.clear(); }
                    TackyInstruction::FunCall { .. } => { copies.clear(); }
                    _ => {}
                }
            }
        }

        instr
    }).collect()
}

fn replace_uses(instr: TackyInstruction, copies: &std::collections::HashMap<String, TackyVal>) -> TackyInstruction {
    fn sub(val: &TackyVal, copies: &std::collections::HashMap<String, TackyVal>) -> TackyVal {
        if let TackyVal::Var(name) = val {
            if let Some(replacement) = copies.get(name) {
                return replacement.clone();
            }
        }
        val.clone()
    }

    match instr {
        TackyInstruction::Return(val) => TackyInstruction::Return(sub(&val, copies)),
        TackyInstruction::Unary { op, src, dst } => TackyInstruction::Unary { op, src: sub(&src, copies), dst },
        TackyInstruction::Binary { op, left, right, dst } => TackyInstruction::Binary { op, left: sub(&left, copies), right: sub(&right, copies), dst },
        TackyInstruction::Copy { src, dst } => TackyInstruction::Copy { src: sub(&src, copies), dst },
        TackyInstruction::JumpIfZero { condition, target } => TackyInstruction::JumpIfZero { condition: sub(&condition, copies), target },
        TackyInstruction::JumpIfNotZero { condition, target } => TackyInstruction::JumpIfNotZero { condition: sub(&condition, copies), target },
        TackyInstruction::Store { src, dst_ptr } => TackyInstruction::Store { src: sub(&src, copies), dst_ptr: sub(&dst_ptr, copies) },
        TackyInstruction::Load { src_ptr, dst } => TackyInstruction::Load { src_ptr: sub(&src_ptr, copies), dst },
        TackyInstruction::SignExtend { src, dst } => TackyInstruction::SignExtend { src: sub(&src, copies), dst },
        TackyInstruction::ZeroExtend { src, dst } => TackyInstruction::ZeroExtend { src: sub(&src, copies), dst },
        TackyInstruction::Truncate { src, dst } => TackyInstruction::Truncate { src: sub(&src, copies), dst },
        TackyInstruction::IntToDouble { src, dst } => TackyInstruction::IntToDouble { src: sub(&src, copies), dst },
        TackyInstruction::DoubleToInt { src, dst } => TackyInstruction::DoubleToInt { src: sub(&src, copies), dst },
        TackyInstruction::UIntToDouble { src, dst } => TackyInstruction::UIntToDouble { src: sub(&src, copies), dst },
        TackyInstruction::DoubleToUInt { src, dst } => TackyInstruction::DoubleToUInt { src: sub(&src, copies), dst },
        TackyInstruction::GetAddress { src, dst } => TackyInstruction::GetAddress { src, dst }, // Don't substitute address source
        TackyInstruction::AddPtr { ptr, index, scale, dst } => TackyInstruction::AddPtr { ptr: sub(&ptr, copies), index: sub(&index, copies), scale, dst },
        TackyInstruction::CopyToOffset { src, dst, offset } => TackyInstruction::CopyToOffset { src: sub(&src, copies), dst, offset },
        TackyInstruction::CopyFromOffset { src, offset, dst } => TackyInstruction::CopyFromOffset { src, offset, dst },
        TackyInstruction::CopyStruct { src, dst, size } => TackyInstruction::CopyStruct { src: sub(&src, copies), dst, size },
        TackyInstruction::FunCall { name, args, dst, dst_type } => {
            let args = args.iter().map(|a| sub(a, copies)).collect();
            TackyInstruction::FunCall { name, args, dst, dst_type }
        }
        other => other,
    }
}

fn get_written_var(instr: &TackyInstruction) -> Option<String> {
    match instr {
        TackyInstruction::Unary { dst: TackyVal::Var(n), .. }
        | TackyInstruction::Binary { dst: TackyVal::Var(n), .. }
        | TackyInstruction::Copy { dst: TackyVal::Var(n), .. }
        | TackyInstruction::SignExtend { dst: TackyVal::Var(n), .. }
        | TackyInstruction::ZeroExtend { dst: TackyVal::Var(n), .. }
        | TackyInstruction::Truncate { dst: TackyVal::Var(n), .. }
        | TackyInstruction::IntToDouble { dst: TackyVal::Var(n), .. }
        | TackyInstruction::DoubleToInt { dst: TackyVal::Var(n), .. }
        | TackyInstruction::UIntToDouble { dst: TackyVal::Var(n), .. }
        | TackyInstruction::DoubleToUInt { dst: TackyVal::Var(n), .. }
        | TackyInstruction::GetAddress { dst: TackyVal::Var(n), .. }
        | TackyInstruction::Load { dst: TackyVal::Var(n), .. }
        | TackyInstruction::AddPtr { dst: TackyVal::Var(n), .. }
        | TackyInstruction::CopyFromOffset { dst: TackyVal::Var(n), .. }
        | TackyInstruction::FunCall { dst: TackyVal::Var(n), .. } => Some(n.clone()),
        TackyInstruction::CopyToOffset { dst, .. } => Some(dst.clone()),
        TackyInstruction::Store { dst_ptr: TackyVal::Var(n), .. } => Some(n.clone()),
        TackyInstruction::CopyStruct { dst: TackyVal::Var(n), .. } => Some(n.clone()),
        _ => None,
    }
}

/// Dead Store Elimination — 無用コード除去
fn dead_store_elimination(instrs: Vec<TackyInstruction>, _var_types: &std::collections::HashMap<String, crate::parse::ast::Type>) -> Vec<TackyInstruction> {
    use std::collections::HashSet;

    // All non-temp variables are considered live (conservative)
    // Only eliminate temp variables that are never read
    let mut used: HashSet<String> = HashSet::new();
    let mut defined: HashSet<String> = HashSet::new();

    // Collect all uses and defs
    for instr in &instrs {
        collect_uses(instr, &mut used);
        if let Some(def) = get_written_var(instr) {
            defined.insert(def);
        }
    }

    // Dead temps: defined but never used (and only temp variables)
    let dead_temps: HashSet<String> = defined.iter()
        .filter(|d| (d.starts_with("tmp.") || d.starts_with("obf_tmp.")) && !used.contains(*d))
        .cloned()
        .collect();

    instrs.into_iter().filter(|instr| {
        // Don't eliminate instructions with side effects
        match instr {
            TackyInstruction::FunCall { .. } | TackyInstruction::Store { .. }
            | TackyInstruction::Return(_) | TackyInstruction::ReturnVoid
            | TackyInstruction::Jump(_) | TackyInstruction::JumpIfZero { .. }
            | TackyInstruction::JumpIfNotZero { .. } | TackyInstruction::Label(_)
            | TackyInstruction::CopyToOffset { .. } | TackyInstruction::CopyStruct { .. } => true,
            _ => {
                if let Some(def) = get_written_var(instr) {
                    !dead_temps.contains(&def)
                } else {
                    true
                }
            }
        }
    }).collect()
}

fn collect_uses(instr: &TackyInstruction, used: &mut std::collections::HashSet<String>) {
    fn add_val(val: &TackyVal, used: &mut std::collections::HashSet<String>) {
        if let TackyVal::Var(name) = val {
            used.insert(name.clone());
        }
    }

    match instr {
        TackyInstruction::Return(val) => add_val(val, used),
        TackyInstruction::Unary { src, .. } => add_val(src, used),
        TackyInstruction::Binary { left, right, .. } => { add_val(left, used); add_val(right, used); }
        TackyInstruction::Copy { src, .. } => add_val(src, used),
        TackyInstruction::JumpIfZero { condition, .. } | TackyInstruction::JumpIfNotZero { condition, .. } => add_val(condition, used),
        TackyInstruction::FunCall { args, .. } => { for a in args { add_val(a, used); } }
        TackyInstruction::SignExtend { src, .. } | TackyInstruction::ZeroExtend { src, .. }
        | TackyInstruction::Truncate { src, .. } | TackyInstruction::IntToDouble { src, .. }
        | TackyInstruction::DoubleToInt { src, .. } | TackyInstruction::UIntToDouble { src, .. }
        | TackyInstruction::DoubleToUInt { src, .. } => add_val(src, used),
        TackyInstruction::GetAddress { src, .. } => add_val(src, used),
        TackyInstruction::Load { src_ptr, .. } => add_val(src_ptr, used),
        TackyInstruction::Store { src, dst_ptr } => { add_val(src, used); add_val(dst_ptr, used); }
        TackyInstruction::AddPtr { ptr, index, .. } => { add_val(ptr, used); add_val(index, used); }
        TackyInstruction::CopyToOffset { src, dst, .. } => { add_val(src, used); used.insert(dst.clone()); }
        TackyInstruction::CopyFromOffset { src, .. } => { used.insert(src.clone()); }
        TackyInstruction::CopyStruct { src, dst, .. } => { add_val(src, used); add_val(dst, used); }
        _ => {}
    }
}
