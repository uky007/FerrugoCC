//! レジスタ割り当てパス（Chapter 20）
//!
//! Pseudo レジスタを含むアセンブリ命令列に対して、
//! グラフ彩色によるレジスタ割り当てを行う。
//!
//! # パイプライン
//! ```text
//! Asm(Pseudo) → [liveness] → [interference graph] → [coalescing] → [graph coloring]
//!             → [replace] → Asm(Reg+Stack) → [fixup] → Asm(valid) → [emit]
//! ```
//!
//! # アルゴリズム
//! 1. 生存解析（後方データフロー解析）で各命令時点の生存変数集合を計算
//! 2. 干渉グラフを構築（同時に生存する変数間に辺を張る、Mov 辺も収集）
//! 3. 保守的 Coalescing（Briggs/George 基準で Mov 辺のノードを合体、冗長 Mov 除去）
//! 4. Chaitin-Briggs アルゴリズムでグラフ彩色（レジスタ割り当て）
//! 5. Pseudo オペランドを Register または Stack(spill) に置換
//! 6. Fixup パスで無効なオペランド組み合わせを修正（Truncate 含む）+ プロローグ/エピローグ生成
//!
//! # スタックフレームレイアウト
//! ```text
//! push %rbp                    ← RBP 保存
//! movq %rsp, %rbp
//! push %rbx                   ← callee-saved (使用時のみ)
//! push %r12                   ← callee-saved (使用時のみ)
//! subq $N, %rsp               ← spill + ローカル変数領域
//! ──────────────────────
//!  -8(%rbp)  : callee-saved push 領域 (push %rbx 等)
//!  -(8+callee_bytes)(%rbp) 以降 : spill スロット・ローカル変数
//! ```
//! Fixup 時に全ての負オフセット Stack オペランドを callee-saved push 分だけ
//! 下方にシフトし、push 領域との重複を防ぐ（shift_stack_offsets）。

use std::collections::{HashMap, HashSet};

use super::asm_ast::*;
use crate::parse::ast::Type;

// ────────────────────────────────────────────
// 公開インターフェース
// ────────────────────────────────────────────

/// レジスタ割り当て結果。
///
/// - `instructions`: Pseudo が全て解決された命令列
/// - `spill_bytes`: spill スロットに必要なスタックサイズ（バイト）
/// - `callee_saved_used`: 使用した callee-saved レジスタ（push/pop が必要）
pub struct RegAllocResult {
    pub instructions: Vec<Instruction>,
    pub spill_bytes: usize,
    pub callee_saved_used: Vec<Reg>,
}

/// レジスタ割り当てを実行する。
///
/// 整数 Pseudo と XMM Pseudo を分離し、それぞれ独立にグラフ彩色を行う。
/// 整数は GP_ALLOCATABLE（12個）、XMM は XMM_ALLOCATABLE（15個）から割り当てる。
/// 割り当てられなかった変数は spill（スタック退避）となる。
pub fn allocate_registers(
    instructions: Vec<Instruction>,
    var_types: &HashMap<String, Type>,
) -> RegAllocResult {
    // 整数 Pseudo と XMM Pseudo を分類
    let (gp_pseudos, xmm_pseudos) = classify_pseudos(&instructions, var_types);

    // 生存解析
    let liveness = analyze_liveness(&instructions);

    // 整数グラフの彩色（coalescing 付き）
    let gp_coloring = if !gp_pseudos.is_empty() {
        let mut graph = build_interference_graph(&instructions, &liveness, &gp_pseudos, false);
        let merge_map = coalesce_graph(&mut graph, GP_ALLOCATABLE.len());
        let mut coloring = color_graph(graph, &GP_ALLOCATABLE, false);
        apply_merge_map(&mut coloring, &merge_map);
        coloring
    } else {
        ColoringResult {
            assignments: HashMap::new(),
        }
    };

    // XMM グラフの彩色（coalescing 付き）
    let xmm_coloring = if !xmm_pseudos.is_empty() {
        let mut graph = build_interference_graph(&instructions, &liveness, &xmm_pseudos, true);
        let merge_map = coalesce_graph(&mut graph, XMM_ALLOCATABLE.len());
        let mut coloring = color_graph(graph, &XMM_ALLOCATABLE, true);
        apply_merge_map(&mut coloring, &merge_map);
        coloring
    } else {
        ColoringResult {
            assignments: HashMap::new(),
        }
    };

    // Pseudo 置換
    let mut assignments: HashMap<String, Operand> = HashMap::new();
    let mut callee_saved_set: HashSet<Reg> = HashSet::new();

    // 強制スタック変数（配列、構造体等）の最も深いオフセットを取得し、
    // スピル変数がそれらと重ならないようにする。
    let mut next_spill_offset: i32 = scan_min_stack_offset(&instructions);
    for (name, assignment) in &gp_coloring.assignments {
        match assignment {
            Assignment::Register(reg) => {
                assignments.insert(name.clone(), Operand::Register(*reg));
                if GP_CALLEE_SAVED.contains(reg) {
                    callee_saved_set.insert(*reg);
                }
            }
            Assignment::Spill => {
                next_spill_offset -= 8;
                assignments.insert(name.clone(), Operand::Stack(next_spill_offset));
            }
        }
    }

    // XMM 割り当て結果をマージ
    for (name, assignment) in &xmm_coloring.assignments {
        match assignment {
            Assignment::Register(reg) => {
                assignments.insert(name.clone(), Operand::Register(*reg));
                // XMM はすべて caller-saved なので callee-saved に追加しない
            }
            Assignment::Spill => {
                next_spill_offset -= 8;
                assignments.insert(name.clone(), Operand::Stack(next_spill_offset));
            }
        }
    }

    // 命令列の Pseudo を置換
    let instructions = replace_pseudos(instructions, &assignments);

    // callee-saved の順序を固定（push/pop の一貫性のため）
    let mut callee_saved_used: Vec<Reg> = callee_saved_set.into_iter().collect();
    callee_saved_used.sort_by_key(|r| GP_CALLEE_SAVED.iter().position(|x| x == r).unwrap_or(99));

    RegAllocResult {
        instructions,
        spill_bytes: (-next_spill_offset) as usize,
        callee_saved_used,
    }
}

/// Fixup パス: 無効なオペランド組み合わせの修正 + プロローグ/エピローグ生成。
///
/// x86-64 では memory-memory 命令が不正なため、scratch レジスタ（R10/R11/XMM15）
/// 経由に展開する。また符号/ゼロ拡張命令（`movsbl`, `movslq`, `movzbl` 等）は
/// dst がメモリの場合も無効なため、レジスタ経由に展開する。
/// その後、spill サイズと使用した callee-saved レジスタに基づいて
/// プロローグ（push + subq）とエピローグ（addq + pop）を挿入する。
pub fn fixup_instructions(
    instructions: Vec<Instruction>,
    spill_bytes: usize,
    callee_saved_used: &[Reg],
) -> Vec<Instruction> {
    // 1. 無効なオペランド組み合わせを修正
    let mut fixed = Vec::new();
    for instr in instructions {
        fixup_instruction(instr, &mut fixed);
    }

    // 2. プロローグ/エピローグを挿入
    insert_prologue_epilogue(fixed, spill_bytes, callee_saved_used)
}

// ────────────────────────────────────────────
// 内部: Pseudo 分類
// ────────────────────────────────────────────

/// 命令列内の全 Pseudo を var_types に基づいて整数/XMM に分類する。
/// Double 型の変数は XMM グラフ、それ以外は整数グラフで彩色される。
fn classify_pseudos(
    instructions: &[Instruction],
    var_types: &HashMap<String, Type>,
) -> (HashSet<String>, HashSet<String>) {
    let mut gp = HashSet::new();
    let mut xmm = HashSet::new();

    for instr in instructions {
        for_each_operand(instr, |op| {
            if let Operand::Pseudo(name) = op {
                let ty = var_types.get(name);
                if ty.is_some_and(|t| t.is_double()) {
                    xmm.insert(name.clone());
                } else {
                    gp.insert(name.clone());
                }
            }
        });
    }

    (gp, xmm)
}

// ────────────────────────────────────────────
// 内部: 生存解析
// ────────────────────────────────────────────

/// 干渉グラフのノード
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum GraphNode {
    Pseudo(String),
    HardReg(Reg),
}

/// 生存解析の結果
struct LivenessInfo {
    live_after: Vec<HashSet<GraphNode>>,
}

/// 命令が使用(use)するオペランドを収集
fn instruction_uses(instr: &Instruction) -> Vec<GraphNode> {
    let mut uses = Vec::new();

    match instr {
        Instruction::Mov { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            // dst が Memory/MemoryOffset の場合、ベースレジスタはアドレス計算で読まれる
            add_memory_base_to_uses(dst, &mut uses);
        }
        Instruction::Unary { operand, .. } => {
            add_operand_nodes_for_use(operand, &mut uses);
        }
        Instruction::Cmp { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            add_operand_nodes_for_use(dst, &mut uses);
        }
        Instruction::SetCC { operand, .. } => {
            // dst が Memory/MemoryOffset の場合、ベースレジスタはアドレス計算で読まれる
            add_memory_base_to_uses(operand, &mut uses);
        }
        Instruction::Binary { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            add_operand_nodes_for_use(dst, &mut uses);
        }
        Instruction::Idiv { operand, .. } => {
            add_operand_nodes_for_use(operand, &mut uses);
            uses.push(GraphNode::HardReg(Reg::AX));
            uses.push(GraphNode::HardReg(Reg::DX));
        }
        Instruction::Div { operand, .. } => {
            add_operand_nodes_for_use(operand, &mut uses);
            uses.push(GraphNode::HardReg(Reg::AX));
            uses.push(GraphNode::HardReg(Reg::DX));
        }
        Instruction::SignExtend(_) => {
            uses.push(GraphNode::HardReg(Reg::AX));
        }
        Instruction::Movsx { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            add_memory_base_to_uses(dst, &mut uses);
        }
        Instruction::MovsxByte { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            add_memory_base_to_uses(dst, &mut uses);
        }
        Instruction::MovZeroExtend { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            add_memory_base_to_uses(dst, &mut uses);
        }
        Instruction::MovZeroExtendByte { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            add_memory_base_to_uses(dst, &mut uses);
        }
        Instruction::Truncate { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            add_memory_base_to_uses(dst, &mut uses);
        }
        Instruction::Push(op) => {
            add_operand_nodes_for_use(op, &mut uses);
        }
        Instruction::Pop(op) => {
            // dst が Memory/MemoryOffset の場合、ベースレジスタはアドレス計算で読まれる
            add_memory_base_to_uses(op, &mut uses);
        }
        Instruction::Jmp(_) | Instruction::JmpCC(_, _) | Instruction::Label(_) => {}
        Instruction::AllocateStack(_) | Instruction::DeallocateStack(_) => {}
        Instruction::Call(_) => {}
        Instruction::Ret => {
            uses.push(GraphNode::HardReg(Reg::AX));
        }
        Instruction::Cvtsi2sd { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            add_memory_base_to_uses(dst, &mut uses);
        }
        Instruction::Cvttsd2si { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            add_memory_base_to_uses(dst, &mut uses);
        }
        Instruction::Lea { src, dst, .. } => {
            add_operand_nodes_for_use(src, &mut uses);
            add_memory_base_to_uses(dst, &mut uses);
        }
        Instruction::JmpIndirect(op, _) => {
            add_operand_nodes_for_use(op, &mut uses);
        }
        Instruction::CallIndirect(op) => {
            add_operand_nodes_for_use(op, &mut uses);
        }
        Instruction::RawBytes(_) => {}
    }

    uses
}

/// 命令が定義(def)するオペランドを収集
fn instruction_defs(instr: &Instruction) -> Vec<GraphNode> {
    let mut defs = Vec::new();

    match instr {
        Instruction::Mov { dst, .. } => {
            add_operand_nodes_for_def(dst, &mut defs);
        }
        Instruction::Unary { operand, .. } => {
            add_operand_nodes_for_def(operand, &mut defs);
        }
        Instruction::Cmp { .. } => {}
        Instruction::SetCC { operand, .. } => {
            add_operand_nodes_for_def(operand, &mut defs);
        }
        Instruction::Binary { dst, .. } => {
            add_operand_nodes_for_def(dst, &mut defs);
        }
        Instruction::Idiv { .. } => {
            defs.push(GraphNode::HardReg(Reg::AX));
            defs.push(GraphNode::HardReg(Reg::DX));
        }
        Instruction::Div { .. } => {
            defs.push(GraphNode::HardReg(Reg::AX));
            defs.push(GraphNode::HardReg(Reg::DX));
        }
        Instruction::SignExtend(_) => {
            defs.push(GraphNode::HardReg(Reg::DX));
        }
        Instruction::Movsx { dst, .. } => {
            add_operand_nodes_for_def(dst, &mut defs);
        }
        Instruction::MovsxByte { dst, .. } => {
            add_operand_nodes_for_def(dst, &mut defs);
        }
        Instruction::MovZeroExtend { dst, .. } => {
            add_operand_nodes_for_def(dst, &mut defs);
        }
        Instruction::MovZeroExtendByte { dst, .. } => {
            add_operand_nodes_for_def(dst, &mut defs);
        }
        Instruction::Truncate { dst, .. } => {
            add_operand_nodes_for_def(dst, &mut defs);
        }
        Instruction::Push(_) => {}
        Instruction::Pop(op) => {
            add_operand_nodes_for_def(op, &mut defs);
        }
        Instruction::Jmp(_) | Instruction::JmpCC(_, _) | Instruction::Label(_) => {}
        Instruction::AllocateStack(_) | Instruction::DeallocateStack(_) => {}
        Instruction::Call(_) => {
            // Call destroys all caller-saved registers
            for &reg in &GP_CALLER_SAVED {
                defs.push(GraphNode::HardReg(reg));
            }
            for &reg in &XMM_ALL {
                defs.push(GraphNode::HardReg(reg));
            }
        }
        Instruction::Ret => {}
        Instruction::Cvtsi2sd { dst, .. } => {
            add_operand_nodes_for_def(dst, &mut defs);
        }
        Instruction::Cvttsd2si { dst, .. } => {
            add_operand_nodes_for_def(dst, &mut defs);
        }
        Instruction::Lea { dst, .. } => {
            add_operand_nodes_for_def(dst, &mut defs);
        }
        Instruction::JmpIndirect(_, _) => {}
        Instruction::CallIndirect(_) => {
            // Same as Call: destroys all caller-saved registers
            for &reg in &GP_CALLER_SAVED {
                defs.push(GraphNode::HardReg(reg));
            }
            for &reg in &XMM_ALL {
                defs.push(GraphNode::HardReg(reg));
            }
        }
        Instruction::RawBytes(_) => {}
    }

    defs
}

/// オペランドを「使用(use)」として追加する。
/// Memory(reg) / MemoryOffset(reg, _) のベースレジスタはアドレス計算で読まれるので use に含める。
fn add_operand_nodes_for_use(op: &Operand, nodes: &mut Vec<GraphNode>) {
    match op {
        Operand::Register(reg) => nodes.push(GraphNode::HardReg(*reg)),
        Operand::Pseudo(name) => nodes.push(GraphNode::Pseudo(name.clone())),
        Operand::Memory(reg) => nodes.push(GraphNode::HardReg(*reg)),
        Operand::MemoryOffset(reg, _) => nodes.push(GraphNode::HardReg(*reg)),
        Operand::Imm(_) | Operand::Stack(_) | Operand::Data(_) => {}
    }
}

/// オペランドを「定義(def)」として追加する。
/// Memory(reg) / MemoryOffset(reg, _) はメモリへの書き込みであり、
/// ベースレジスタ自体は書き換えられないので def に含めない。
fn add_operand_nodes_for_def(op: &Operand, nodes: &mut Vec<GraphNode>) {
    match op {
        Operand::Register(reg) => nodes.push(GraphNode::HardReg(*reg)),
        Operand::Pseudo(name) => nodes.push(GraphNode::Pseudo(name.clone())),
        Operand::Memory(_) | Operand::MemoryOffset(_, _) => {
            // メモリへの書き込みはベースレジスタの定義ではない
        }
        Operand::Imm(_) | Operand::Stack(_) | Operand::Data(_) => {}
    }
}

/// Memory/MemoryOffset のベースレジスタだけを use に追加する。
/// dst が Memory(reg) の場合、アドレス計算でレジスタが読まれるため。
fn add_memory_base_to_uses(op: &Operand, nodes: &mut Vec<GraphNode>) {
    match op {
        Operand::Memory(reg) => nodes.push(GraphNode::HardReg(*reg)),
        Operand::MemoryOffset(reg, _) => nodes.push(GraphNode::HardReg(*reg)),
        _ => {}
    }
}

/// 命令がジャンプ先を持つ場合、そのラベルを返す
fn jump_targets(instr: &Instruction) -> Vec<String> {
    match instr {
        Instruction::Jmp(label) => vec![label.clone()],
        Instruction::JmpCC(_, label) => vec![label.clone()],
        // JmpIndirect: ジャンプテーブルの全エントリを CFG 後続として返す
        Instruction::JmpIndirect(_, targets) => targets.clone(),
        _ => vec![],
    }
}

/// 後方データフロー解析で各命令の直後の生存集合を計算する。
///
/// ラベル/ジャンプから CFG（制御フロー・グラフ）を構築し、
/// 不動点反復で各命令時点の `live_in`/`live_after` を求める。
///
/// ```text
/// live_after[i] = ∪ { live_in[s] | s は i の後続命令 }
/// live_in[i]    = uses[i] ∪ (live_after[i] - defs[i])
/// ```
fn analyze_liveness(instructions: &[Instruction]) -> LivenessInfo {
    let n = instructions.len();
    if n == 0 {
        return LivenessInfo { live_after: vec![] };
    }

    // ラベル→命令インデックスのマッピング
    let mut label_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, instr) in instructions.iter().enumerate() {
        if let Instruction::Label(label) = instr {
            label_to_idx.insert(label.clone(), i);
        }
    }

    // 各命令の後続命令インデックス
    let mut successors: Vec<Vec<usize>> = vec![vec![]; n];
    for i in 0..n {
        let targets = jump_targets(&instructions[i]);
        for target in &targets {
            if let Some(&idx) = label_to_idx.get(target) {
                successors[i].push(idx);
            }
        }
        // フォールスルー（Jmp, JmpIndirect, Ret 以外）
        if !matches!(
            instructions[i],
            Instruction::Jmp(_) | Instruction::JmpIndirect(_, _) | Instruction::Ret
        ) && i + 1 < n
        {
            successors[i].push(i + 1);
        }
    }

    // 各命令の use/def を事前計算
    let uses: Vec<HashSet<GraphNode>> = instructions
        .iter()
        .map(|instr| instruction_uses(instr).into_iter().collect())
        .collect();
    let defs: Vec<HashSet<GraphNode>> = instructions
        .iter()
        .map(|instr| instruction_defs(instr).into_iter().collect())
        .collect();

    // live_in[i], live_after[i]
    let mut live_in: Vec<HashSet<GraphNode>> = vec![HashSet::new(); n];
    let mut live_after: Vec<HashSet<GraphNode>> = vec![HashSet::new(); n];

    // 不動点反復（後方から）
    let mut changed = true;
    while changed {
        changed = false;
        for i in (0..n).rev() {
            // live_after[i] = union of live_in[s] for s in successors[i]
            let mut new_after: HashSet<GraphNode> = HashSet::new();
            for &s in &successors[i] {
                for node in &live_in[s] {
                    new_after.insert(node.clone());
                }
            }

            // live_in[i] = uses[i] ∪ (live_after[i] - defs[i])
            let mut new_in: HashSet<GraphNode> = uses[i].clone();
            for node in &new_after {
                if !defs[i].contains(node) {
                    new_in.insert(node.clone());
                }
            }

            if new_in != live_in[i] || new_after != live_after[i] {
                changed = true;
                live_in[i] = new_in;
                live_after[i] = new_after;
            }
        }
    }

    LivenessInfo { live_after }
}

// ────────────────────────────────────────────
// 内部: 干渉グラフ
// ────────────────────────────────────────────

struct InterferenceGraph {
    /// 隣接リスト
    adj: HashMap<GraphNode, HashSet<GraphNode>>,
    /// Mov 辺: coalescing 候補の (src, dst) ペア
    mov_edges: Vec<(GraphNode, GraphNode)>,
}

impl InterferenceGraph {
    fn new() -> Self {
        InterferenceGraph {
            adj: HashMap::new(),
            mov_edges: Vec::new(),
        }
    }

    fn add_node(&mut self, node: GraphNode) {
        self.adj.entry(node).or_default();
    }

    fn add_edge(&mut self, a: &GraphNode, b: &GraphNode) {
        if a == b {
            return;
        }
        self.adj.entry(a.clone()).or_default().insert(b.clone());
        self.adj.entry(b.clone()).or_default().insert(a.clone());
    }
}

/// 干渉グラフを構築する。
///
/// 各命令の定義変数と、その命令直後に生存している変数の間に干渉辺を張る。
/// Mov 系命令では src-dst 間に辺を張らない（将来の coalescing 最適化のため）。
///
/// - `relevant_pseudos`: このグラフに含める Pseudo 集合（整数 or XMM）
/// - `is_xmm`: true なら XMM レジスタのみ追跡、false なら整数レジスタのみ追跡
fn build_interference_graph(
    instructions: &[Instruction],
    liveness: &LivenessInfo,
    relevant_pseudos: &HashSet<String>,
    is_xmm: bool,
) -> InterferenceGraph {
    let mut graph = InterferenceGraph::new();

    // 関連するハードレジスタ集合
    let relevant_hard_regs: HashSet<Reg> = if is_xmm {
        XMM_ALL.iter().copied().collect()
    } else {
        let mut s: HashSet<Reg> = GP_ALLOCATABLE.iter().copied().collect();
        s.insert(Reg::R10);
        s.insert(Reg::R11);
        s
    };

    // フィルター関数: このグラフに含めるべきノードかどうか
    let is_relevant = |node: &GraphNode| -> bool {
        match node {
            GraphNode::Pseudo(name) => relevant_pseudos.contains(name),
            GraphNode::HardReg(reg) => relevant_hard_regs.contains(reg),
        }
    };

    // 全 Pseudo をノードとして追加
    for name in relevant_pseudos {
        graph.add_node(GraphNode::Pseudo(name.clone()));
    }

    // 各命令について干渉辺を追加
    for (i, instr) in instructions.iter().enumerate() {
        let defined = instruction_defs(instr);
        let live_after = &liveness.live_after[i];

        let is_mov = matches!(
            instr,
            Instruction::Mov { .. }
                | Instruction::Movsx { .. }
                | Instruction::MovsxByte { .. }
                | Instruction::MovZeroExtend { .. }
                | Instruction::MovZeroExtendByte { .. }
                | Instruction::Truncate { .. }
        );

        // Mov のソースを取得（coalescing のため干渉辺を張らない）
        let mov_src: Option<GraphNode> = if is_mov {
            let uses = instruction_uses(instr);
            uses.into_iter().next()
        } else {
            None
        };

        for d in &defined {
            if !is_relevant(d) {
                continue;
            }
            graph.add_node(d.clone());

            for v in live_after {
                if !is_relevant(v) {
                    continue;
                }
                if v == d {
                    continue;
                }
                // Mov の場合、src と dst の間には辺を張らない
                if is_mov
                    && let Some(ref src_node) = mov_src
                    && v == src_node
                {
                    continue;
                }
                graph.add_edge(d, v);
            }
        }

        // Mov 辺を収集（coalescing 候補 — plain Mov のみ）
        // Movsx/Truncate 等の型変換命令は値が変わるため coalescing しない。
        if matches!(instr, Instruction::Mov { .. })
            && let Some(ref src_node) = mov_src
        {
            for d in &defined {
                if is_relevant(src_node) && is_relevant(d) && src_node != d {
                    graph.mov_edges.push((src_node.clone(), d.clone()));
                }
            }
        }
    }

    graph
}

// ────────────────────────────────────────────
// 内部: Coalescing（レジスタ合体）
// ────────────────────────────────────────────

/// merge_map のルートを辿って正規ノードを返す。
fn find_canonical(merge_map: &HashMap<GraphNode, GraphNode>, node: &GraphNode) -> GraphNode {
    let mut current = node.clone();
    while let Some(next) = merge_map.get(&current) {
        current = next.clone();
    }
    current
}

/// 保守的 coalescing（Briggs + George 基準）を実行する。
///
/// Mov 辺で結ばれた非干渉ノードペアを合体し、冗長な Mov を除去する。
/// - **Briggs 基準** (Pseudo-Pseudo): 合体ノードの degree ≥ k な隣接ノード数 < k なら安全
/// - **George 基準** (Pseudo-HardReg): Pseudo の全隣接ノード t が、
///   HardReg とも干渉するか degree < k なら安全
///
/// 戻り値の merge_map は「このノードはあのノードに合体された」を記録する。
fn coalesce_graph(graph: &mut InterferenceGraph, k: usize) -> HashMap<GraphNode, GraphNode> {
    let mut merge_map: HashMap<GraphNode, GraphNode> = HashMap::new();

    let mut changed = true;
    while changed {
        changed = false;
        let mov_edges_snapshot = graph.mov_edges.clone();

        for (raw_u, raw_v) in &mov_edges_snapshot {
            let u = find_canonical(&merge_map, raw_u);
            let v = find_canonical(&merge_map, raw_v);
            if u == v {
                continue;
            } // 既に合体済み

            // 干渉していたら合体不可
            if graph.adj.get(&u).is_some_and(|s| s.contains(&v)) {
                continue;
            }

            let can_coalesce = match (&u, &v) {
                // Pseudo-Pseudo: Briggs 基準
                (GraphNode::Pseudo(_), GraphNode::Pseudo(_)) => briggs_criterion(graph, &u, &v, k),
                // Pseudo-HardReg: George 基準（Pseudo を HardReg に合体）
                (GraphNode::Pseudo(_), GraphNode::HardReg(_)) => george_criterion(graph, &u, &v, k),
                (GraphNode::HardReg(_), GraphNode::Pseudo(_)) => george_criterion(graph, &v, &u, k),
                // HardReg-HardReg: 同じレジスタでない限り合体不可
                (GraphNode::HardReg(_), GraphNode::HardReg(_)) => false,
            };

            if can_coalesce {
                // HardReg は常に into 側（正規ノード、グラフに残る）
                let (from, into) = match (&u, &v) {
                    (GraphNode::HardReg(_), _) => (v.clone(), u.clone()),
                    (_, GraphNode::HardReg(_)) => (u.clone(), v.clone()),
                    _ => (u.clone(), v.clone()),
                };
                merge_nodes(graph, &from, &into);
                merge_map.insert(from, into);
                changed = true;
                break; // グラフが変更されたので再走査
            }
        }
    }

    merge_map
}

/// Briggs 基準: 合体後のノードが k 未満の高次隣接ノードを持つか判定。
fn briggs_criterion(graph: &InterferenceGraph, u: &GraphNode, v: &GraphNode, k: usize) -> bool {
    let u_neighbors = graph.adj.get(u).cloned().unwrap_or_default();
    let v_neighbors = graph.adj.get(v).cloned().unwrap_or_default();
    let mut merged_neighbors: HashSet<GraphNode> = u_neighbors;
    merged_neighbors.extend(v_neighbors);
    merged_neighbors.remove(u);
    merged_neighbors.remove(v);

    let high_degree_count = merged_neighbors
        .iter()
        .filter(|n| match n {
            GraphNode::HardReg(_) => true, // precolored は常に高次
            GraphNode::Pseudo(_) => graph.adj.get(n).map_or(0, |s| s.len()) >= k,
        })
        .count();

    high_degree_count < k
}

/// George 基準: u (Pseudo) の全隣接ノードが v (HardReg) とも干渉するか低次か判定。
fn george_criterion(
    graph: &InterferenceGraph,
    u: &GraphNode, // Pseudo
    v: &GraphNode, // HardReg
    k: usize,
) -> bool {
    let u_neighbors = graph.adj.get(u).cloned().unwrap_or_default();
    let v_neighbors = graph.adj.get(v).cloned().unwrap_or_default();

    for t in &u_neighbors {
        if t == v {
            continue;
        }
        if v_neighbors.contains(t) {
            continue;
        } // t は v とも干渉 → OK
        match t {
            GraphNode::HardReg(_) => return false, // HardReg が v と非干渉 → 不安全
            GraphNode::Pseudo(_) => {
                if graph.adj.get(t).map_or(0, |s| s.len()) >= k {
                    return false; // 高次 Pseudo が v と非干渉 → 不安全
                }
            }
        }
    }
    true
}

/// ノード `from` を `into` に合体する。隣接リストを更新。
fn merge_nodes(graph: &mut InterferenceGraph, from: &GraphNode, into: &GraphNode) {
    let from_neighbors = graph.adj.remove(from).unwrap_or_default();

    for n in &from_neighbors {
        if n == into {
            continue;
        }
        // n の隣接リストから from を削除し into を追加
        if let Some(s) = graph.adj.get_mut(n) {
            s.remove(from);
            s.insert(into.clone());
        }
        // into の隣接リストに n を追加
        graph.adj.entry(into.clone()).or_default().insert(n.clone());
    }

    // into の隣接リストから from を削除（self-loop 防止）
    if let Some(s) = graph.adj.get_mut(into) {
        s.remove(from);
    }
}

/// coalescing の merge_map を彩色結果に適用する。
///
/// merge_map 内の各 (from → into) エントリについて、
/// from の Pseudo に into の割り当て（色）をコピーする。
fn apply_merge_map(coloring: &mut ColoringResult, merge_map: &HashMap<GraphNode, GraphNode>) {
    for (from, into) in merge_map {
        if let GraphNode::Pseudo(from_name) = from {
            let canonical = find_canonical(merge_map, into);
            match &canonical {
                GraphNode::Pseudo(canonical_name) => {
                    if let Some(assignment) = coloring.assignments.get(canonical_name).cloned() {
                        coloring.assignments.insert(from_name.clone(), assignment);
                    }
                }
                GraphNode::HardReg(reg) => {
                    coloring
                        .assignments
                        .insert(from_name.clone(), Assignment::Register(*reg));
                }
            }
        }
    }
}

// ────────────────────────────────────────────
// 内部: グラフ彩色（Chaitin-Briggs）
// ────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Assignment {
    Register(Reg),
    Spill,
}

struct ColoringResult {
    assignments: HashMap<String, Assignment>,
}

/// Chaitin-Briggs アルゴリズムでグラフ彩色を行う。
///
/// 1. **Simplify**: degree < k のノードをスタックに push して除去
/// 2. **Potential Spill**: 全ノードが degree >= k なら、最大 degree のノードを
///    spill 候補としてマークし push
/// 3. **Select**: スタックから pop し、隣接ノードが使っていない色を割り当て。
///    spill 候補でも色が見つかれば割り当てる（楽観的彩色）。
///    色が見つからなければ実際に spill（スタック退避）。
fn color_graph(graph: InterferenceGraph, allocatable: &[Reg], _is_xmm: bool) -> ColoringResult {
    let k = allocatable.len();

    // オリジナルの隣接リストを保存
    let original_adj = graph.adj.clone();

    // Pseudo ノードだけをリストアップ
    let pseudo_nodes: Vec<String> = graph
        .adj
        .keys()
        .filter_map(|node| {
            if let GraphNode::Pseudo(name) = node {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect();

    if pseudo_nodes.is_empty() {
        return ColoringResult {
            assignments: HashMap::new(),
        };
    }

    // Simplify + Potential Spill（working copy で）
    let mut working_adj: HashMap<GraphNode, HashSet<GraphNode>> = graph.adj;
    let mut stack: Vec<(String, bool)> = Vec::new();
    let mut remaining: HashSet<String> = pseudo_nodes.into_iter().collect();

    let working_degree = |adj: &HashMap<GraphNode, HashSet<GraphNode>>, name: &str| -> usize {
        let node = GraphNode::Pseudo(name.to_string());
        adj.get(&node).map_or(0, |s| s.len())
    };

    let remove_from_working = |adj: &mut HashMap<GraphNode, HashSet<GraphNode>>, name: &str| {
        let node = GraphNode::Pseudo(name.to_string());
        if let Some(neighbors) = adj.remove(&node) {
            for n in &neighbors {
                if let Some(s) = adj.get_mut(n) {
                    s.remove(&node);
                }
            }
        }
    };

    while !remaining.is_empty() {
        // Phase 1: degree < k のノードを除去
        let mut progress = true;
        while progress {
            progress = false;
            let candidates: Vec<String> = remaining
                .iter()
                .filter(|name| working_degree(&working_adj, name) < k)
                .cloned()
                .collect();

            for name in candidates {
                remove_from_working(&mut working_adj, &name);
                stack.push((name.clone(), false));
                remaining.remove(&name);
                progress = true;
            }
        }

        // Phase 2: spill 候補
        if !remaining.is_empty() {
            let spill_name = remaining
                .iter()
                .max_by_key(|name| working_degree(&working_adj, name))
                .cloned()
                .unwrap();

            remove_from_working(&mut working_adj, &spill_name);
            stack.push((spill_name.clone(), true));
            remaining.remove(&spill_name);
        }
    }

    // Select: スタックから pop して色を割り当て
    let mut assignments: HashMap<String, Assignment> = HashMap::new();
    // HardReg の「色」は自分自身
    let reg_to_color: HashMap<Reg, usize> = allocatable
        .iter()
        .enumerate()
        .map(|(i, &reg)| (reg, i))
        .collect();

    while let Some((name, _is_spill_candidate)) = stack.pop() {
        let node = GraphNode::Pseudo(name.clone());
        let original_neighbors = original_adj.get(&node).cloned().unwrap_or_default();

        // 隣接ノードが使っている色を収集
        let mut used_colors: HashSet<usize> = HashSet::new();
        for neighbor in &original_neighbors {
            match neighbor {
                GraphNode::HardReg(reg) => {
                    if let Some(&color) = reg_to_color.get(reg) {
                        used_colors.insert(color);
                    }
                }
                GraphNode::Pseudo(n) => {
                    if let Some(Assignment::Register(reg)) = assignments.get(n)
                        && let Some(&color) = reg_to_color.get(reg)
                    {
                        used_colors.insert(color);
                    }
                }
            }
        }

        // 空いている色を探す
        let mut assigned = false;
        for (color, &reg) in allocatable.iter().enumerate() {
            if !used_colors.contains(&color) {
                assignments.insert(name.clone(), Assignment::Register(reg));
                assigned = true;
                break;
            }
        }

        if !assigned {
            // Spill
            assignments.insert(name.clone(), Assignment::Spill);
        }
    }

    ColoringResult { assignments }
}

// ────────────────────────────────────────────
// 内部: Pseudo 置換
// ────────────────────────────────────────────

/// 全命令の Pseudo オペランドを彩色結果に基づいて置換する。
/// Register が割り当てられた Pseudo → `Operand::Register(reg)`
/// Spill された Pseudo → `Operand::Stack(offset)`
fn replace_pseudos(
    instructions: Vec<Instruction>,
    assignments: &HashMap<String, Operand>,
) -> Vec<Instruction> {
    instructions
        .into_iter()
        .map(|instr| replace_in_instruction(instr, assignments))
        .collect()
}

fn replace_operand(op: Operand, assignments: &HashMap<String, Operand>) -> Operand {
    match op {
        Operand::Pseudo(ref name) => assignments.get(name).cloned().unwrap_or(op),
        _ => op,
    }
}

fn replace_in_instruction(
    instr: Instruction,
    assignments: &HashMap<String, Operand>,
) -> Instruction {
    match instr {
        Instruction::Mov { asm_type, src, dst } => Instruction::Mov {
            asm_type,
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::Unary {
            asm_type,
            op,
            operand,
        } => Instruction::Unary {
            asm_type,
            op,
            operand: replace_operand(operand, assignments),
        },
        Instruction::Cmp { asm_type, src, dst } => Instruction::Cmp {
            asm_type,
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::SetCC { condition, operand } => Instruction::SetCC {
            condition,
            operand: replace_operand(operand, assignments),
        },
        Instruction::Binary {
            asm_type,
            op,
            src,
            dst,
        } => Instruction::Binary {
            asm_type,
            op,
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::Idiv { asm_type, operand } => Instruction::Idiv {
            asm_type,
            operand: replace_operand(operand, assignments),
        },
        Instruction::Div { asm_type, operand } => Instruction::Div {
            asm_type,
            operand: replace_operand(operand, assignments),
        },
        Instruction::Movsx { src, dst } => Instruction::Movsx {
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::MovsxByte { asm_type, src, dst } => Instruction::MovsxByte {
            asm_type,
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::MovZeroExtend { src, dst } => Instruction::MovZeroExtend {
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::MovZeroExtendByte { asm_type, src, dst } => Instruction::MovZeroExtendByte {
            asm_type,
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::Truncate { src, dst } => Instruction::Truncate {
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::Push(op) => Instruction::Push(replace_operand(op, assignments)),
        Instruction::Pop(op) => Instruction::Pop(replace_operand(op, assignments)),
        Instruction::Cvtsi2sd { asm_type, src, dst } => Instruction::Cvtsi2sd {
            asm_type,
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::Cvttsd2si { asm_type, src, dst } => Instruction::Cvttsd2si {
            asm_type,
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::Lea { src, dst } => Instruction::Lea {
            src: replace_operand(src, assignments),
            dst: replace_operand(dst, assignments),
        },
        Instruction::JmpIndirect(op, ref targets) => {
            Instruction::JmpIndirect(replace_operand(op, assignments), targets.clone())
        }
        Instruction::CallIndirect(op) => {
            Instruction::CallIndirect(replace_operand(op, assignments))
        }
        // These don't have operands to replace
        instr @ (Instruction::SignExtend(_)
        | Instruction::Jmp(_)
        | Instruction::JmpCC(_, _)
        | Instruction::Label(_)
        | Instruction::AllocateStack(_)
        | Instruction::DeallocateStack(_)
        | Instruction::Call(_)
        | Instruction::Ret
        | Instruction::RawBytes(_)) => instr,
    }
}

// ────────────────────────────────────────────
// 内部: Fixup パス
// ────────────────────────────────────────────

fn is_memory_operand(op: &Operand) -> bool {
    matches!(
        op,
        Operand::Stack(_) | Operand::Data(_) | Operand::Memory(_) | Operand::MemoryOffset(_, _)
    )
}

fn is_imm(op: &Operand) -> bool {
    matches!(op, Operand::Imm(_))
}

fn is_large_imm(op: &Operand) -> bool {
    if let Operand::Imm(v) = op {
        *v > i32::MAX as i64 || *v < i32::MIN as i64
    } else {
        false
    }
}

fn fixup_instruction(instr: Instruction, out: &mut Vec<Instruction>) {
    match instr {
        // Mov: memory,memory → via scratch
        Instruction::Mov {
            asm_type,
            ref src,
            ref dst,
        } if is_memory_operand(src) && is_memory_operand(dst) => {
            if asm_type == AsmType::Double {
                out.push(Instruction::Mov {
                    asm_type,
                    src: src.clone(),
                    dst: Operand::Register(Reg::XMM15),
                });
                out.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Register(Reg::XMM15),
                    dst: dst.clone(),
                });
            } else {
                out.push(Instruction::Mov {
                    asm_type,
                    src: src.clone(),
                    dst: Operand::Register(Reg::R10),
                });
                out.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Register(Reg::R10),
                    dst: dst.clone(),
                });
            }
        }

        // Mov: large immediate to memory → via R10
        Instruction::Mov {
            asm_type,
            ref src,
            ref dst,
        } if is_large_imm(src) && is_memory_operand(dst) => {
            out.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: src.clone(),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Mov {
                asm_type,
                src: Operand::Register(Reg::R10),
                dst: dst.clone(),
            });
        }

        // Mov src,src (same register) → remove noop
        Instruction::Mov {
            ref src, ref dst, ..
        } if src == dst => {
            // Skip noop moves
        }

        // Binary: memory,memory → via scratch (non-double)
        Instruction::Binary {
            asm_type,
            op,
            ref src,
            ref dst,
        } if asm_type != AsmType::Double && is_memory_operand(src) && is_memory_operand(dst) => {
            // imul cannot have memory dst, so load dst into R11
            if matches!(op, AsmBinaryOp::Mult) {
                out.push(Instruction::Mov {
                    asm_type,
                    src: dst.clone(),
                    dst: Operand::Register(Reg::R11),
                });
                out.push(Instruction::Binary {
                    asm_type,
                    op,
                    src: src.clone(),
                    dst: Operand::Register(Reg::R11),
                });
                out.push(Instruction::Mov {
                    asm_type,
                    src: Operand::Register(Reg::R11),
                    dst: dst.clone(),
                });
            } else {
                out.push(Instruction::Mov {
                    asm_type,
                    src: src.clone(),
                    dst: Operand::Register(Reg::R10),
                });
                out.push(Instruction::Binary {
                    asm_type,
                    op,
                    src: Operand::Register(Reg::R10),
                    dst: dst.clone(),
                });
            }
        }

        // Binary: Quadword with large immediate (> 32-bit signed) → via R10
        Instruction::Binary {
            asm_type: AsmType::Quadword,
            op,
            src: Operand::Imm(v),
            ref dst,
        } if v > i32::MAX as i64 || v < i32::MIN as i64 => {
            out.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Imm(v),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Binary {
                asm_type: AsmType::Quadword,
                op,
                src: Operand::Register(Reg::R10),
                dst: dst.clone(),
            });
        }

        // Binary: double memory,memory → via XMM15
        Instruction::Binary {
            asm_type,
            op,
            ref src,
            ref dst,
        } if asm_type == AsmType::Double && is_memory_operand(src) && is_memory_operand(dst) => {
            out.push(Instruction::Mov {
                asm_type: AsmType::Double,
                src: src.clone(),
                dst: Operand::Register(Reg::XMM15),
            });
            out.push(Instruction::Binary {
                asm_type,
                op,
                src: Operand::Register(Reg::XMM15),
                dst: dst.clone(),
            });
        }

        // Binary: imul with memory dst → via R11
        Instruction::Binary {
            asm_type,
            op: AsmBinaryOp::Mult,
            ref src,
            ref dst,
        } if asm_type != AsmType::Double && is_memory_operand(dst) => {
            out.push(Instruction::Mov {
                asm_type,
                src: dst.clone(),
                dst: Operand::Register(Reg::R11),
            });
            out.push(Instruction::Binary {
                asm_type,
                op: AsmBinaryOp::Mult,
                src: src.clone(),
                dst: Operand::Register(Reg::R11),
            });
            out.push(Instruction::Mov {
                asm_type,
                src: Operand::Register(Reg::R11),
                dst: dst.clone(),
            });
        }

        // Cmp: memory,memory → via R10
        Instruction::Cmp {
            asm_type,
            ref src,
            ref dst,
        } if asm_type != AsmType::Double && is_memory_operand(src) && is_memory_operand(dst) => {
            out.push(Instruction::Mov {
                asm_type,
                src: dst.clone(),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Cmp {
                asm_type,
                src: src.clone(),
                dst: Operand::Register(Reg::R10),
            });
        }

        // Cmp: dst is immediate → load dst into R10
        Instruction::Cmp {
            asm_type,
            ref src,
            dst: Operand::Imm(v),
        } => {
            out.push(Instruction::Mov {
                asm_type,
                src: Operand::Imm(v),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Cmp {
                asm_type,
                src: src.clone(),
                dst: Operand::Register(Reg::R10),
            });
        }

        // Cmp: large immediate → via R10
        Instruction::Cmp {
            asm_type,
            ref src,
            ref dst,
        } if is_large_imm(src) => {
            out.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: src.clone(),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Cmp {
                asm_type,
                src: Operand::Register(Reg::R10),
                dst: dst.clone(),
            });
        }

        // Idiv/Div: immediate operand → via R10
        Instruction::Idiv {
            asm_type,
            ref operand,
        } if is_imm(operand) => {
            out.push(Instruction::Mov {
                asm_type,
                src: operand.clone(),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Idiv {
                asm_type,
                operand: Operand::Register(Reg::R10),
            });
        }
        Instruction::Div {
            asm_type,
            ref operand,
        } if is_imm(operand) => {
            out.push(Instruction::Mov {
                asm_type,
                src: operand.clone(),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Div {
                asm_type,
                operand: Operand::Register(Reg::R10),
            });
        }

        // Movsx: immediate → just a regular quadword mov (sign extension of a constant is a no-op)
        Instruction::Movsx { ref src, ref dst } if is_imm(src) => {
            out.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: src.clone(),
                dst: dst.clone(),
            });
        }

        // Movsx: dst is memory → via R11 (movslq requires register destination)
        Instruction::Movsx { ref src, ref dst } if is_memory_operand(dst) => {
            out.push(Instruction::Movsx {
                src: src.clone(),
                dst: Operand::Register(Reg::R11),
            });
            out.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::R11),
                dst: dst.clone(),
            });
        }

        // MovsxByte: immediate → just a regular mov
        Instruction::MovsxByte {
            asm_type,
            ref src,
            ref dst,
        } if is_imm(src) => {
            out.push(Instruction::Mov {
                asm_type,
                src: src.clone(),
                dst: dst.clone(),
            });
        }

        // MovsxByte: dst is memory → via R11 (movsbl/movsbq requires register destination)
        Instruction::MovsxByte {
            asm_type,
            ref src,
            ref dst,
        } if is_memory_operand(dst) => {
            out.push(Instruction::MovsxByte {
                asm_type,
                src: src.clone(),
                dst: Operand::Register(Reg::R11),
            });
            out.push(Instruction::Mov {
                asm_type,
                src: Operand::Register(Reg::R11),
                dst: dst.clone(),
            });
        }

        // MovZeroExtend: immediate → just a regular mov
        Instruction::MovZeroExtend { ref src, ref dst } if is_imm(src) => {
            out.push(Instruction::Mov {
                asm_type: AsmType::Longword,
                src: src.clone(),
                dst: dst.clone(),
            });
        }

        // MovZeroExtend: dst is memory → via R11 (requires register destination)
        Instruction::MovZeroExtend { ref src, ref dst } if is_memory_operand(dst) => {
            out.push(Instruction::MovZeroExtend {
                src: src.clone(),
                dst: Operand::Register(Reg::R11),
            });
            out.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::R11),
                dst: dst.clone(),
            });
        }

        // MovZeroExtendByte: immediate → just a regular mov
        Instruction::MovZeroExtendByte {
            asm_type,
            ref src,
            ref dst,
        } if is_imm(src) => {
            out.push(Instruction::Mov {
                asm_type,
                src: src.clone(),
                dst: dst.clone(),
            });
        }

        // MovZeroExtendByte: dst is memory → via R11 (movzbl/movzbq requires register destination)
        Instruction::MovZeroExtendByte {
            asm_type,
            ref src,
            ref dst,
        } if is_memory_operand(dst) => {
            out.push(Instruction::MovZeroExtendByte {
                asm_type,
                src: src.clone(),
                dst: Operand::Register(Reg::R11),
            });
            out.push(Instruction::Mov {
                asm_type,
                src: Operand::Register(Reg::R11),
                dst: dst.clone(),
            });
        }

        // Truncate: memory,memory → via R10
        Instruction::Truncate { ref src, ref dst }
            if is_memory_operand(src) && is_memory_operand(dst) =>
        {
            out.push(Instruction::Mov {
                asm_type: AsmType::Longword,
                src: src.clone(),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Mov {
                asm_type: AsmType::Longword,
                src: Operand::Register(Reg::R10),
                dst: dst.clone(),
            });
        }

        // Truncate: memory dst → via R10
        Instruction::Truncate { ref src, ref dst } if is_memory_operand(dst) => {
            out.push(Instruction::Truncate {
                src: src.clone(),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Mov {
                asm_type: AsmType::Longword,
                src: Operand::Register(Reg::R10),
                dst: dst.clone(),
            });
        }

        // Lea: memory,memory → via R10
        Instruction::Lea { ref src, ref dst } if is_memory_operand(dst) => {
            out.push(Instruction::Lea {
                src: src.clone(),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::R10),
                dst: dst.clone(),
            });
        }

        // Cvtsi2sd: immediate src → load to R10 first
        Instruction::Cvtsi2sd {
            asm_type,
            ref src,
            ref dst,
        } if is_imm(src) => {
            out.push(Instruction::Mov {
                asm_type,
                src: src.clone(),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::Cvtsi2sd {
                asm_type,
                src: Operand::Register(Reg::R10),
                dst: dst.clone(),
            });
        }

        // Cvtsi2sd: memory dst → via XMM15
        Instruction::Cvtsi2sd {
            asm_type,
            ref src,
            ref dst,
        } if is_memory_operand(dst) => {
            out.push(Instruction::Cvtsi2sd {
                asm_type,
                src: src.clone(),
                dst: Operand::Register(Reg::XMM15),
            });
            out.push(Instruction::Mov {
                asm_type: AsmType::Double,
                src: Operand::Register(Reg::XMM15),
                dst: dst.clone(),
            });
        }

        // Cvttsd2si: memory dst → via R11
        Instruction::Cvttsd2si {
            asm_type,
            ref src,
            ref dst,
        } if is_memory_operand(dst) => {
            out.push(Instruction::Cvttsd2si {
                asm_type,
                src: src.clone(),
                dst: Operand::Register(Reg::R11),
            });
            out.push(Instruction::Mov {
                asm_type,
                src: Operand::Register(Reg::R11),
                dst: dst.clone(),
            });
        }

        // JmpIndirect: memory operand → load via R10
        Instruction::JmpIndirect(ref op, ref targets) if is_memory_operand(op) => {
            out.push(Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: op.clone(),
                dst: Operand::Register(Reg::R10),
            });
            out.push(Instruction::JmpIndirect(
                Operand::Register(Reg::R10),
                targets.clone(),
            ));
        }

        // Default: no fixup needed
        other => out.push(other),
    }
}

// ────────────────────────────────────────────
// 内部: プロローグ/エピローグ
// ────────────────────────────────────────────

/// プロローグとエピローグを命令列に挿入する。
///
/// **プロローグ**（命令列の先頭に挿入）:
/// ```text
/// push %rbp
/// movq %rsp, %rbp
/// push <callee-saved>...     // 使用した callee-saved のみ
/// subq $N, %rsp              // spill スロット確保（16B アラインメント調整済み）
/// ```
///
/// **エピローグ**（各 `Ret` の前に挿入）:
/// ```text
/// addq $N, %rsp
/// pop <callee-saved>...      // push の逆順
/// pop %rbp
/// ret
/// ```
///
/// アラインメント: `call` の return address (8B) + `push %rbp` (8B) = 16B で整列。
/// その後 `callee_count * 8 + alloc_size` が 16 の倍数になるよう調整する。
fn insert_prologue_epilogue(
    instructions: Vec<Instruction>,
    spill_bytes: usize,
    callee_saved_used: &[Reg],
) -> Vec<Instruction> {
    let callee_count = callee_saved_used.len();

    // アラインメント計算
    // call main の return address (8B) + pushq %rbp (8B) で 16B（0 mod 16 に戻る）。
    // その後 callee-saved push が callee_count * 8 バイト。
    // RSP を 0 mod 16 に保つには callee_count * 8 + alloc_size が 16 の倍数である必要がある。
    let callee_bytes = callee_count * 8;

    // 全命令の Stack() オペランドをスキャンし、最も深い負オフセットを取得。
    // これはスピル変数と強制スタック変数の両方を含む実際のスタック使用量を反映する。
    let min_stack_offset = scan_min_stack_offset(&instructions);
    let stack_bytes_from_scan = if min_stack_offset < 0 {
        (-min_stack_offset) as usize
    } else {
        0
    };
    let needed_stack = std::cmp::max(spill_bytes, stack_bytes_from_scan);

    let total = callee_bytes + needed_stack;
    let aligned_total = (total + 15) & !15;
    let alloc_size = aligned_total - callee_bytes;

    // ── スタックオフセット補正 ──
    // Spill 変数と強制スタック変数は -8(%rbp), -16(%rbp), ... に割り当てられるが、
    // callee-saved レジスタの push も同じ領域を使う。
    // callee_bytes 分だけ下にシフトして衝突を回避する。
    let shift = -(callee_bytes as i32);
    let instructions: Vec<Instruction> = if callee_bytes > 0 {
        instructions
            .into_iter()
            .map(|instr| shift_stack_offsets(instr, shift))
            .collect()
    } else {
        instructions
    };

    let mut result = Vec::new();

    // プロローグ
    result.push(Instruction::Push(Operand::Register(Reg::BP)));
    result.push(Instruction::Mov {
        asm_type: AsmType::Quadword,
        src: Operand::Register(Reg::SP),
        dst: Operand::Register(Reg::BP),
    });
    for &reg in callee_saved_used {
        result.push(Instruction::Push(Operand::Register(reg)));
    }
    if alloc_size > 0 {
        result.push(Instruction::AllocateStack(alloc_size));
    }

    // 命令列（Ret をエピローグに置換）
    for instr in instructions {
        if matches!(instr, Instruction::Ret) {
            // エピローグ
            if alloc_size > 0 {
                result.push(Instruction::DeallocateStack(alloc_size));
            }
            for &reg in callee_saved_used.iter().rev() {
                result.push(Instruction::Pop(Operand::Register(reg)));
            }
            result.push(Instruction::Pop(Operand::Register(Reg::BP)));
            result.push(Instruction::Ret);
        } else {
            result.push(instr);
        }
    }

    result
}

/// Stack(offset) の負オフセットを shift 分だけずらす。
/// 正オフセット（スタック渡し引数）はそのまま維持する。
fn shift_stack_op(op: Operand, shift: i32) -> Operand {
    match op {
        Operand::Stack(offset) if offset < 0 => Operand::Stack(offset + shift),
        other => other,
    }
}

fn shift_stack_offsets(instr: Instruction, shift: i32) -> Instruction {
    match instr {
        Instruction::Mov { asm_type, src, dst } => Instruction::Mov {
            asm_type,
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::Unary {
            asm_type,
            op,
            operand,
        } => Instruction::Unary {
            asm_type,
            op,
            operand: shift_stack_op(operand, shift),
        },
        Instruction::Cmp { asm_type, src, dst } => Instruction::Cmp {
            asm_type,
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::SetCC { condition, operand } => Instruction::SetCC {
            condition,
            operand: shift_stack_op(operand, shift),
        },
        Instruction::Binary {
            asm_type,
            op,
            src,
            dst,
        } => Instruction::Binary {
            asm_type,
            op,
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::Idiv { asm_type, operand } => Instruction::Idiv {
            asm_type,
            operand: shift_stack_op(operand, shift),
        },
        Instruction::Div { asm_type, operand } => Instruction::Div {
            asm_type,
            operand: shift_stack_op(operand, shift),
        },
        Instruction::Movsx { src, dst } => Instruction::Movsx {
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::MovsxByte { asm_type, src, dst } => Instruction::MovsxByte {
            asm_type,
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::MovZeroExtend { src, dst } => Instruction::MovZeroExtend {
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::MovZeroExtendByte { asm_type, src, dst } => Instruction::MovZeroExtendByte {
            asm_type,
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::Truncate { src, dst } => Instruction::Truncate {
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::Push(op) => Instruction::Push(shift_stack_op(op, shift)),
        Instruction::Pop(op) => Instruction::Pop(shift_stack_op(op, shift)),
        Instruction::Cvtsi2sd { asm_type, src, dst } => Instruction::Cvtsi2sd {
            asm_type,
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::Cvttsd2si { asm_type, src, dst } => Instruction::Cvttsd2si {
            asm_type,
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::Lea { src, dst } => Instruction::Lea {
            src: shift_stack_op(src, shift),
            dst: shift_stack_op(dst, shift),
        },
        Instruction::JmpIndirect(op, targets) => {
            Instruction::JmpIndirect(shift_stack_op(op, shift), targets)
        }
        Instruction::CallIndirect(op) => Instruction::CallIndirect(shift_stack_op(op, shift)),
        instr @ (Instruction::SignExtend(_)
        | Instruction::Jmp(_)
        | Instruction::JmpCC(_, _)
        | Instruction::Label(_)
        | Instruction::AllocateStack(_)
        | Instruction::DeallocateStack(_)
        | Instruction::Call(_)
        | Instruction::Ret
        | Instruction::RawBytes(_)) => instr,
    }
}

// ────────────────────────────────────────────
// ユーティリティ
// ────────────────────────────────────────────

/// 全命令の Stack() オペランドをスキャンし、最も深い（最も負の）オフセットを返す。
/// Stack() オペランドがない場合は 0 を返す。
fn scan_min_stack_offset(instructions: &[Instruction]) -> i32 {
    let mut min_offset: i32 = 0;
    for instr in instructions {
        for_each_operand(instr, |op| {
            if let Operand::Stack(offset) = op
                && *offset < min_offset
            {
                min_offset = *offset;
            }
        });
    }
    min_offset
}

/// 命令の全オペランドに対してクロージャを適用
fn for_each_operand<F: FnMut(&Operand)>(instr: &Instruction, mut f: F) {
    match instr {
        Instruction::Mov { src, dst, .. } => {
            f(src);
            f(dst);
        }
        Instruction::Unary { operand, .. } => {
            f(operand);
        }
        Instruction::Cmp { src, dst, .. } => {
            f(src);
            f(dst);
        }
        Instruction::SetCC { operand, .. } => {
            f(operand);
        }
        Instruction::Binary { src, dst, .. } => {
            f(src);
            f(dst);
        }
        Instruction::Idiv { operand, .. } => {
            f(operand);
        }
        Instruction::Div { operand, .. } => {
            f(operand);
        }
        Instruction::Movsx { src, dst } => {
            f(src);
            f(dst);
        }
        Instruction::MovsxByte { src, dst, .. } => {
            f(src);
            f(dst);
        }
        Instruction::MovZeroExtend { src, dst } => {
            f(src);
            f(dst);
        }
        Instruction::MovZeroExtendByte { src, dst, .. } => {
            f(src);
            f(dst);
        }
        Instruction::Truncate { src, dst } => {
            f(src);
            f(dst);
        }
        Instruction::Push(op) | Instruction::Pop(op) => {
            f(op);
        }
        Instruction::Cvtsi2sd { src, dst, .. } => {
            f(src);
            f(dst);
        }
        Instruction::Cvttsd2si { src, dst, .. } => {
            f(src);
            f(dst);
        }
        Instruction::Lea { src, dst } => {
            f(src);
            f(dst);
        }
        Instruction::JmpIndirect(op, _) => {
            f(op);
        }
        Instruction::CallIndirect(op) => {
            f(op);
        }
        Instruction::SignExtend(_)
        | Instruction::Jmp(_)
        | Instruction::JmpCC(_, _)
        | Instruction::Label(_)
        | Instruction::AllocateStack(_)
        | Instruction::DeallocateStack(_)
        | Instruction::Call(_)
        | Instruction::Ret
        | Instruction::RawBytes(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// merge_nodes で Pseudo を HardReg にマージしたとき、
    /// 干渉グラフの隣接が対称 (adj[a]∋b ⇔ adj[b]∋a) を維持するか検証する。
    ///
    /// 再現シナリオ:
    ///   - Pseudo("x") と HardReg(DI) の間に Mov 辺（干渉辺なし）
    ///   - Pseudo("tmp") と Pseudo("x") の間に干渉辺あり
    ///   - x を DI にマージ → DI-tmp 間に干渉辺が対称に転写されること
    #[test]
    fn merge_nodes_keeps_adjacency_symmetric() {
        let mut graph = InterferenceGraph::new();

        let di = GraphNode::HardReg(Reg::DI);
        let x = GraphNode::Pseudo("x".into());
        let tmp = GraphNode::Pseudo("tmp".into());

        // ノード追加（Pseudo は add_node で初期化、HardReg は辺追加時のみ）
        graph.add_node(x.clone());
        graph.add_node(tmp.clone());
        // DI には add_node しない — 実際のビルドと同じ状況

        // 干渉辺: tmp ↔ x
        graph.add_edge(&tmp, &x);

        // 前提確認
        assert!(graph.adj.get(&tmp).unwrap().contains(&x));
        assert!(graph.adj.get(&x).unwrap().contains(&tmp));
        assert!(graph.adj.get(&di).is_none(), "DI should have no adj entry before merge");

        // x → DI にマージ
        merge_nodes(&mut graph, &x, &di);

        // マージ後: DI ↔ tmp が対称であること
        assert!(
            graph.adj.get(&di).is_some_and(|s| s.contains(&tmp)),
            "adj[DI] must contain tmp after merging x→DI"
        );
        assert!(
            graph.adj.get(&tmp).is_some_and(|s| s.contains(&di)),
            "adj[tmp] must contain DI after merging x→DI"
        );

        // x の旧エントリは削除されていること
        assert!(
            graph.adj.get(&x).is_none(),
            "adj[x] should be removed after merge"
        );
        // tmp の旧辺 (tmp→x) は消えていること
        assert!(
            !graph.adj.get(&tmp).unwrap().contains(&x),
            "adj[tmp] should no longer contain x after merge"
        );
    }

    /// マージ先 (into) に既存の隣接がある場合も対称性を維持するか検証。
    #[test]
    fn merge_nodes_symmetric_with_existing_edges() {
        let mut graph = InterferenceGraph::new();

        let di = GraphNode::HardReg(Reg::DI);
        let x = GraphNode::Pseudo("x".into());
        let tmp = GraphNode::Pseudo("tmp".into());
        let count = GraphNode::Pseudo("count".into());

        graph.add_node(x.clone());
        graph.add_node(tmp.clone());
        graph.add_node(count.clone());

        // 干渉辺: tmp ↔ x, count ↔ x
        graph.add_edge(&tmp, &x);
        graph.add_edge(&count, &x);

        // DI に既存辺を作る (count ↔ DI)
        graph.add_edge(&count, &di);

        // x → DI にマージ
        merge_nodes(&mut graph, &x, &di);

        // DI ↔ tmp が対称
        assert!(graph.adj.get(&di).unwrap().contains(&tmp));
        assert!(graph.adj.get(&tmp).unwrap().contains(&di));

        // DI ↔ count も維持
        assert!(graph.adj.get(&di).unwrap().contains(&count));
        assert!(graph.adj.get(&count).unwrap().contains(&di));

        // self-loop なし
        assert!(!graph.adj.get(&di).unwrap().contains(&di));
    }
}
