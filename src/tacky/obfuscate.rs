//! TACKY IR 難読化パス
//!
//! TACKY → TACKY の変換を行う難読化パス（最適化の逆）。
//! `--fobfuscate` フラグで有効化される。
//!
//! # パス適用順序（TACKY IR レベル）
//! 1. **Library Function Obfuscation**（ライブラリ関数難読化）— `strlen`, `strcmp`, `strcpy`,
//!    `memcpy`, `memset`, `memcmp`, `strncmp`, `strncpy`, `strchr`, `strcat` の既知ライブラリ関数を
//!    等価な自前実装に差し替え、FLIRT シグネチャマッチングを無効化する
//! 2. **Function Inlining**（関数インライン展開）— 呼び出し先の本体を呼び出し元に埋め込む
//! 3. **Constant Encoding**（定数の間接化）— 即値を `a * b + c` の実行時計算に置換
//! 4. **Arithmetic Substitution**（算術置換）— Add/Subtract を多段計算に展開
//! 5. **Junk Code Insertion**（ジャンクコード挿入）— 4命令ごとに dead computation を挿入
//! 6. **Opaque Predicates**（不透明述語）— 4パターンの常真条件分岐で値生成命令を囲む
//!    - パターン 0: `x*(x+1) % 2 == 0`（連続整数の積は偶数）
//!    - パターン 1: `!(x² + 1 > 0)`（x²+1 は常に正）
//!    - パターン 2: `(x+1)² - x² - 1 - 2x == 0`（代数恒等式）
//!    - パターン 3: `(x³ - x) % 3 == 0`（連続3整数の積は3の倍数）
//! 7. **Function Outlining**（関数アウトライン化）— コード断片を新しい関数に切り出す
//! 8. **VM Virtualization**（VM仮想化）— 適格な関数をバイトコード＋VMインタプリタに変換。
//!    `.data` にバイトコード配列とハンドラテーブルを配置し、ディスパッチループで間接実行
//! 9. **Control Flow Flattening**（制御フロー平坦化）— 基本ブロックをジャンプテーブル
//!    + 状態エンコードの dispatch ループに変換。IDA 等の CFG 復元を破壊する。
//!    - ジャンプテーブル: `.data` セクションにブロックラベルの配列を配置し `jmp *%rax` で分岐
//!    - 状態エンコード: `encoded = index * 37 + 0xCAFE` のアフィン変換で状態変数を符号化
//! 10. **String Encryption**（文字列暗号化）— 文字列リテラルを加算暗号化し main() で復号
//!
//! Pass 10 は他のパスの後に適用する。復号コードが CFF 等で破壊されるのを防ぐため。
//!
//! # ASM レベル難読化（codegen/mod.rs で適用、レジスタ割り当て後）
//! - **Stack Frame Obfuscation**: 偽のスタックスロットと偽の read/write 操作を挿入し偽ローカル変数を生成
//! - **Register Shuffle**: dead な `movq` を挿入し偽のレジスタ間依存関係を生成（R10/R11 使用）
//! - **Instruction Substitution**: 命令を意味的に等価な別の命令列に置換しパターンマッチングを妨害
//! - **Anti-Disassembly**: 無条件ジャンプ直後に `0xE8`（call opcode）を挿入し命令境界認識を破壊
//! - **Indirect Calls**: `call func` を `lea func(%rip), %r10; call *%r10` に変換

use std::collections::{HashMap, HashSet};

use super::tacky_ast::*;
use crate::parse::ast::Type;
use crate::obfuscation::ObfuscationConfig;

/// 難読化コンテキスト — temp 変数とラベルのカウンタを管理
struct ObfCtx {
    tmp_counter: usize,
    label_counter: usize,
    inline_counter: usize,
    outline_counter: usize,
    vm_counter: usize,
}

impl ObfCtx {
    fn new() -> Self {
        ObfCtx {
            tmp_counter: 0,
            label_counter: 0,
            inline_counter: 0,
            outline_counter: 0,
            vm_counter: 0,
        }
    }

    /// 新しい一時変数名を生成（`obf_tmp.N`）
    fn fresh_tmp(&mut self) -> String {
        let name = format!("obf_tmp.{}", self.tmp_counter);
        self.tmp_counter += 1;
        name
    }

    /// 新しいラベル名を生成（`.Lobf_N`）
    fn fresh_label(&mut self) -> String {
        let name = format!(".Lobf_{}", self.label_counter);
        self.label_counter += 1;
        name
    }
}

/// 難読化パスのエントリポイント
///
/// パス適用順序:
/// 1. Pass 15: ライブラリ関数難読化（自前実装が後続の全パスで難読化される）
/// 2. Pass 12: 関数インライン展開（インラインされたコードが後続パスで難読化される）
/// 3. Pass 1-4: 定数間接化・算術置換・ジャンクコード・不透明述語
/// 4. Pass 13: 関数アウトライン化（難読化済みコードが関数に切り出される）
/// 5. Pass 14: VM仮想化（適格な関数をバイトコード＋VMインタプリタに変換）
/// 6. Pass 5: CFF（VMディスパッチループを含む全関数に適用 → 二重間接化）
/// 7. Pass 6: 文字列暗号化（復号コードが CFF 等で破壊されるのを防ぐ）
pub fn obfuscate(program: TackyProgram, config: &ObfuscationConfig) -> TackyProgram {
    let mut program = program;

    let mut ctx = ObfCtx::new();

    // Pass 15: ライブラリ関数難読化（全パスの前 → 自前実装が後続の全パスで難読化される）
    if config.lib_obfuscate {
        replace_library_functions(&mut program, &mut ctx);
    }

    // Pass 12: 関数インライン展開（全パスの前 → インラインされたコードが後続で難読化される）
    if config.func_inline {
        inline_functions(&mut program, &mut ctx, config.func_inline_freq);
    }

    // Pass 1-4: 関数ごとの変換
    for func in &mut program.functions {

        // Pass 1: 定数の間接化
        if config.constant_encoding {
            func.body = constant_encoding(std::mem::take(&mut func.body), &mut ctx, &mut func.var_types);
        }

        // Pass 2: 算術置換（Add/Subtract を多段計算に展開）
        if config.arith_subst {
            func.body = arithmetic_substitution(std::mem::take(&mut func.body), &mut ctx, &mut func.var_types, config.arith_freq);
        }

        // Pass 3: ジャンクコード挿入
        if config.junk_code {
            func.body = junk_code_insertion(std::mem::take(&mut func.body), &mut ctx, &mut func.var_types, config.junk_freq);
        }

        // Pass 4: 不透明述語（多様化パターン）
        if config.opaque_predicates {
            func.body = opaque_predicates(std::mem::take(&mut func.body), &mut ctx, &mut func.var_types, config.pred_freq);
        }
    }

    // Pass 13: 関数アウトライン化（Pass 1-4 の後、CFF の前）
    if config.func_outline {
        outline_functions(&mut program, &mut ctx, config.func_outline_min_block);
    }

    // Pass 14: VM仮想化（適格な関数をバイトコード＋VMインタプリタに変換）
    if config.vm_virtualize {
        vm_virtualize(&mut program, &mut ctx);
    }

    // Pass 5: CFF（VMディスパッチループを含む全関数に適用 → 二重間接化）
    for func in &mut program.functions {
        if config.cff {
            func.body = control_flow_flattening(
                std::mem::take(&mut func.body),
                &mut ctx,
                &mut func.var_types,
                &mut program.static_vars,
                config.cff_a,
                config.cff_b,
            );
        }
    }

    // Pass 6: 文字列暗号化（他のパスの後に適用 — 復号コードが CFF 等で壊されるのを防ぐ）
    if config.string_encryption {
        string_encryption(&mut program, &mut ctx, config.string_key);
    }

    program
}

// ─────────────────────────────────────────────────────────────
// Pass 15: Library Function Obfuscation（ライブラリ関数難読化）
// ─────────────────────────────────────────────────────────────

/// ライブラリ関数の呼び出しを自前実装に差し替える。
///
/// FLIRT シグネチャ対策: `strlen` 等の既知ライブラリ関数を等価な
/// TACKY IR 実装に置換し、後続の難読化パスで認識不能にする。
fn replace_library_functions(program: &mut TackyProgram, ctx: &mut ObfCtx) {
    /// 差し替え対象のライブラリ関数名
    const TARGET_FUNCTIONS: &[&str] = &[
        "strlen", "strcmp", "strcpy", "memcpy", "memset",
        "memcmp", "strncmp", "strncpy", "strchr", "strcat",
    ];

    // 全 FunCall を走査し、対象関数名を収集
    let mut needed: HashSet<String> = HashSet::new();
    for func in &program.functions {
        for instr in &func.body {
            if let TackyInstruction::FunCall { name, .. } = instr {
                if TARGET_FUNCTIONS.contains(&name.as_str()) {
                    needed.insert(name.clone());
                }
            }
        }
    }

    if needed.is_empty() {
        return;
    }

    // 対象ごとに自前実装を生成
    let mut generated: HashMap<String, String> = HashMap::new(); // original -> obf name
    let mut new_functions: Vec<TackyFunction> = Vec::new();

    for name in &needed {
        match name.as_str() {
            "strlen" => {
                let obf_name = "_obf_strlen".to_string();
                new_functions.push(generate_strlen(ctx, &obf_name));
                generated.insert(name.clone(), obf_name);
            }
            "strcmp" => {
                let obf_name = "_obf_strcmp".to_string();
                new_functions.push(generate_strcmp(ctx, &obf_name));
                generated.insert(name.clone(), obf_name);
            }
            "strcpy" => {
                let obf_name = "_obf_strcpy".to_string();
                new_functions.push(generate_strcpy(ctx, &obf_name));
                generated.insert(name.clone(), obf_name);
            }
            "memcpy" => {
                let obf_name = "_obf_memcpy".to_string();
                new_functions.push(generate_memcpy(ctx, &obf_name));
                generated.insert(name.clone(), obf_name);
            }
            "memset" => {
                let obf_name = "_obf_memset".to_string();
                new_functions.push(generate_memset(ctx, &obf_name));
                generated.insert(name.clone(), obf_name);
            }
            "memcmp" => {
                let obf_name = "_obf_memcmp".to_string();
                new_functions.push(generate_memcmp(ctx, &obf_name));
                generated.insert(name.clone(), obf_name);
            }
            "strncmp" => {
                let obf_name = "_obf_strncmp".to_string();
                new_functions.push(generate_strncmp(ctx, &obf_name));
                generated.insert(name.clone(), obf_name);
            }
            "strncpy" => {
                let obf_name = "_obf_strncpy".to_string();
                new_functions.push(generate_strncpy(ctx, &obf_name));
                generated.insert(name.clone(), obf_name);
            }
            "strchr" => {
                let obf_name = "_obf_strchr".to_string();
                new_functions.push(generate_strchr(ctx, &obf_name));
                generated.insert(name.clone(), obf_name);
            }
            "strcat" => {
                let obf_name = "_obf_strcat".to_string();
                new_functions.push(generate_strcat(ctx, &obf_name));
                generated.insert(name.clone(), obf_name);
            }
            _ => {}
        }
    }

    // FunCall のターゲットを差し替え
    for func in &mut program.functions {
        for instr in &mut func.body {
            if let TackyInstruction::FunCall { name, .. } = instr {
                if let Some(obf_name) = generated.get(name) {
                    *name = obf_name.clone();
                }
            }
        }
    }

    // 生成した関数を追加
    program.functions.extend(new_functions);
}

/// `strlen` の等価な TACKY IR 実装を生成する。
///
/// ```c
/// long _obf_strlen(const char *s) {
///     long len = 0;
///     while (s[len] != '\0')
///         len = len + 1;
///     return len;
/// }
/// ```
fn generate_strlen(ctx: &mut ObfCtx, name: &str) -> TackyFunction {
    let p = "p".to_string();
    let len = ctx.fresh_tmp();    // loop counter (Long)
    let ptr = ctx.fresh_tmp();    // ptr = s + len (Pointer(Char))
    let ch = ctx.fresh_tmp();     // *ptr (Char)
    let ci = ctx.fresh_tmp();     // zero-extended to Int

    let loop_start = ctx.fresh_label();
    let loop_end = ctx.fresh_label();

    let mut var_types = HashMap::new();
    var_types.insert(p.clone(), Type::Pointer(Box::new(Type::Char)));
    var_types.insert(len.clone(), Type::Long);
    var_types.insert(ptr.clone(), Type::Pointer(Box::new(Type::Char)));
    var_types.insert(ch.clone(), Type::Char);
    var_types.insert(ci.clone(), Type::Int);

    let body = vec![
        // len = 0
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(len.clone()),
        },
        // loop_start:
        TackyInstruction::Label(loop_start.clone()),
        // ptr = s + len
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(p.clone()),
            index: TackyVal::Var(len.clone()),
            scale: 1,
            dst: TackyVal::Var(ptr.clone()),
        },
        // ch = *ptr
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(ptr.clone()),
            dst: TackyVal::Var(ch.clone()),
        },
        // ci = (int)ch
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch.clone()),
            dst: TackyVal::Var(ci.clone()),
        },
        // if ci == 0, break
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(ci.clone()),
            target: loop_end.clone(),
        },
        // len = len + 1
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(len.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(len.clone()),
        },
        // goto loop_start
        TackyInstruction::Jump(loop_start),
        // loop_end:
        TackyInstruction::Label(loop_end),
        // return len
        TackyInstruction::Return(TackyVal::Var(len)),
    ];

    TackyFunction {
        name: name.to_string(),
        global: true,
        params: vec![p],
        body,
        return_type: Type::Long,
        var_types,
        is_variadic: false,
    }
}

/// `strcmp` の等価な TACKY IR 実装を生成する。
///
/// ```c
/// int _obf_strcmp(const char *s1, const char *s2) {
///     long i = 0;
///     for (;;) {
///         char c1 = s1[i], c2 = s2[i];
///         int d = (int)c1 - (int)c2;
///         if (d != 0) return d;
///         if (c1 == '\0') return 0;
///         i = i + 1;
///     }
/// }
/// ```
fn generate_strcmp(ctx: &mut ObfCtx, name: &str) -> TackyFunction {
    let p1 = "p1".to_string();
    let p2 = "p2".to_string();
    let idx = ctx.fresh_tmp();     // loop index (Long)
    let ptr1 = ctx.fresh_tmp();    // s1 + idx
    let ptr2 = ctx.fresh_tmp();    // s2 + idx
    let ch1 = ctx.fresh_tmp();     // *ptr1 (Char)
    let ch2 = ctx.fresh_tmp();     // *ptr2 (Char)
    let ci1 = ctx.fresh_tmp();     // ZeroExtend ch1 → Int
    let ci2 = ctx.fresh_tmp();     // ZeroExtend ch2 → Int
    let diff = ctx.fresh_tmp();    // ci1 - ci2 (Int)

    let loop_start = ctx.fresh_label();
    let loop_end = ctx.fresh_label();
    let ret_diff = ctx.fresh_label();

    let ptr_char = Type::Pointer(Box::new(Type::Char));
    let mut var_types = HashMap::new();
    var_types.insert(p1.clone(), ptr_char.clone());
    var_types.insert(p2.clone(), ptr_char.clone());
    var_types.insert(idx.clone(), Type::Long);
    var_types.insert(ptr1.clone(), ptr_char.clone());
    var_types.insert(ptr2.clone(), ptr_char);
    var_types.insert(ch1.clone(), Type::Char);
    var_types.insert(ch2.clone(), Type::Char);
    var_types.insert(ci1.clone(), Type::Int);
    var_types.insert(ci2.clone(), Type::Int);
    var_types.insert(diff.clone(), Type::Int);

    let body = vec![
        // idx = 0
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(idx.clone()),
        },
        // loop_start:
        TackyInstruction::Label(loop_start.clone()),
        // ptr1 = s1 + idx
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(p1.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(ptr1.clone()),
        },
        // ptr2 = s2 + idx
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(p2.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(ptr2.clone()),
        },
        // ch1 = *ptr1
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(ptr1.clone()),
            dst: TackyVal::Var(ch1.clone()),
        },
        // ch2 = *ptr2
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(ptr2.clone()),
            dst: TackyVal::Var(ch2.clone()),
        },
        // ci1 = (int)ch1
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch1.clone()),
            dst: TackyVal::Var(ci1.clone()),
        },
        // ci2 = (int)ch2
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch2.clone()),
            dst: TackyVal::Var(ci2.clone()),
        },
        // diff = ci1 - ci2
        TackyInstruction::Binary {
            op: TackyBinaryOp::Subtract,
            left: TackyVal::Var(ci1.clone()),
            right: TackyVal::Var(ci2.clone()),
            dst: TackyVal::Var(diff.clone()),
        },
        // if diff != 0, return diff
        TackyInstruction::JumpIfNotZero {
            condition: TackyVal::Var(diff.clone()),
            target: ret_diff.clone(),
        },
        // if ch1 == 0 (null terminator), return 0
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(ci1.clone()),
            target: loop_end.clone(),
        },
        // idx++
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Jump(loop_start),
        // ret_diff: return diff
        TackyInstruction::Label(ret_diff),
        TackyInstruction::Return(TackyVal::Var(diff)),
        // loop_end: return 0
        TackyInstruction::Label(loop_end),
        TackyInstruction::Return(TackyVal::Constant(TackyConst::Int(0))),
    ];

    TackyFunction {
        name: name.to_string(),
        global: true,
        params: vec![p1, p2],
        body,
        return_type: Type::Int,
        var_types,
        is_variadic: false,
    }
}

/// `strcpy` の等価な TACKY IR 実装を生成する。
///
/// ```c
/// char *_obf_strcpy(char *dst, const char *src) {
///     long i = 0;
///     for (;;) {
///         char ch = src[i];
///         dst[i] = ch;
///         if (ch == '\0') break;
///         i = i + 1;
///     }
///     return dst;
/// }
/// ```
fn generate_strcpy(ctx: &mut ObfCtx, name: &str) -> TackyFunction {
    let dst = "p_dst".to_string();
    let src = "p_src".to_string();
    let idx = ctx.fresh_tmp();      // loop index (Long)
    let src_ptr = ctx.fresh_tmp();  // src + idx
    let dst_ptr = ctx.fresh_tmp();  // dst + idx
    let ch = ctx.fresh_tmp();       // *src_ptr (Char)
    let ci = ctx.fresh_tmp();       // ZeroExtend ch → Int

    let loop_start = ctx.fresh_label();
    let loop_end = ctx.fresh_label();

    let ptr_char = Type::Pointer(Box::new(Type::Char));
    let mut var_types = HashMap::new();
    var_types.insert(dst.clone(), ptr_char.clone());
    var_types.insert(src.clone(), ptr_char.clone());
    var_types.insert(idx.clone(), Type::Long);
    var_types.insert(src_ptr.clone(), ptr_char.clone());
    var_types.insert(dst_ptr.clone(), ptr_char);
    var_types.insert(ch.clone(), Type::Char);
    var_types.insert(ci.clone(), Type::Int);

    let body = vec![
        // idx = 0
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(idx.clone()),
        },
        // loop_start:
        TackyInstruction::Label(loop_start.clone()),
        // src_ptr = src + idx
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(src.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(src_ptr.clone()),
        },
        // ch = *src_ptr
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(src_ptr.clone()),
            dst: TackyVal::Var(ch.clone()),
        },
        // dst_ptr = dst + idx
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(dst.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(dst_ptr.clone()),
        },
        // *dst_ptr = ch
        TackyInstruction::Store {
            src: TackyVal::Var(ch.clone()),
            dst_ptr: TackyVal::Var(dst_ptr.clone()),
        },
        // ci = (int)ch
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch.clone()),
            dst: TackyVal::Var(ci.clone()),
        },
        // if ci == 0, break
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(ci.clone()),
            target: loop_end.clone(),
        },
        // idx++
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Jump(loop_start),
        // loop_end:
        TackyInstruction::Label(loop_end),
        // return dst
        TackyInstruction::Return(TackyVal::Var(dst.clone())),
    ];

    TackyFunction {
        name: name.to_string(),
        global: true,
        params: vec![dst, src],
        body,
        return_type: Type::Pointer(Box::new(Type::Char)),
        var_types,
        is_variadic: false,
    }
}

/// `memcpy` の等価な TACKY IR 実装を生成する。
///
/// ```c
/// void *_obf_memcpy(void *dst, void *src, long n) {
///     long i = 0;
///     while (i < n) {
///         ((char *)dst)[i] = ((char *)src)[i];
///         i = i + 1;
///     }
///     return dst;
/// }
/// ```
fn generate_memcpy(ctx: &mut ObfCtx, name: &str) -> TackyFunction {
    let dst = "p_dst".to_string();
    let src = "p_src".to_string();
    let n = "p_n".to_string();
    let idx = ctx.fresh_tmp();      // loop index (Long)
    let src_ptr = ctx.fresh_tmp();  // src + idx
    let dst_ptr = ctx.fresh_tmp();  // dst + idx
    let byte = ctx.fresh_tmp();     // loaded byte (Char)
    let cmp = ctx.fresh_tmp();      // idx < n (Int)

    let loop_start = ctx.fresh_label();
    let loop_end = ctx.fresh_label();

    let ptr_char = Type::Pointer(Box::new(Type::Char));
    let mut var_types = HashMap::new();
    var_types.insert(dst.clone(), ptr_char.clone());
    var_types.insert(src.clone(), ptr_char.clone());
    var_types.insert(n.clone(), Type::Long);
    var_types.insert(idx.clone(), Type::Long);
    var_types.insert(src_ptr.clone(), ptr_char.clone());
    var_types.insert(dst_ptr.clone(), ptr_char);
    var_types.insert(byte.clone(), Type::Char);
    var_types.insert(cmp.clone(), Type::Int);

    let body = vec![
        // idx = 0
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(idx.clone()),
        },
        // loop_start:
        TackyInstruction::Label(loop_start.clone()),
        // cmp = (idx < n)
        TackyInstruction::Binary {
            op: TackyBinaryOp::LessThan,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Var(n.clone()),
            dst: TackyVal::Var(cmp.clone()),
        },
        // if cmp == 0, break
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(cmp.clone()),
            target: loop_end.clone(),
        },
        // src_ptr = src + idx
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(src.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(src_ptr.clone()),
        },
        // byte = *src_ptr
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(src_ptr.clone()),
            dst: TackyVal::Var(byte.clone()),
        },
        // dst_ptr = dst + idx
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(dst.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(dst_ptr.clone()),
        },
        // *dst_ptr = byte
        TackyInstruction::Store {
            src: TackyVal::Var(byte.clone()),
            dst_ptr: TackyVal::Var(dst_ptr.clone()),
        },
        // idx++
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Jump(loop_start),
        // loop_end:
        TackyInstruction::Label(loop_end),
        // return dst
        TackyInstruction::Return(TackyVal::Var(dst.clone())),
    ];

    TackyFunction {
        name: name.to_string(),
        global: true,
        params: vec![dst, src, n],
        body,
        return_type: Type::Pointer(Box::new(Type::Char)),
        var_types,
        is_variadic: false,
    }
}

/// `memset` の等価な TACKY IR 実装を生成する。
///
/// ```c
/// void *_obf_memset(void *s, int c, long n) {
///     char b = (char)c;
///     long i = 0;
///     while (i < n) {
///         ((char *)s)[i] = b;
///         i = i + 1;
///     }
///     return s;
/// }
/// ```
fn generate_memset(ctx: &mut ObfCtx, name: &str) -> TackyFunction {
    let s = "p_s".to_string();
    let c = "p_c".to_string();
    let n = "p_n".to_string();
    let b = ctx.fresh_tmp();        // truncated byte (Char)
    let idx = ctx.fresh_tmp();      // loop index (Long)
    let dst_ptr = ctx.fresh_tmp();  // s + idx
    let cmp = ctx.fresh_tmp();      // idx < n (Int)

    let loop_start = ctx.fresh_label();
    let loop_end = ctx.fresh_label();

    let ptr_char = Type::Pointer(Box::new(Type::Char));
    let mut var_types = HashMap::new();
    var_types.insert(s.clone(), ptr_char.clone());
    var_types.insert(c.clone(), Type::Int);
    var_types.insert(n.clone(), Type::Long);
    var_types.insert(b.clone(), Type::Char);
    var_types.insert(idx.clone(), Type::Long);
    var_types.insert(dst_ptr.clone(), ptr_char);
    var_types.insert(cmp.clone(), Type::Int);

    let body = vec![
        // b = (char)c
        TackyInstruction::Truncate {
            src: TackyVal::Var(c.clone()),
            dst: TackyVal::Var(b.clone()),
        },
        // idx = 0
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(idx.clone()),
        },
        // loop_start:
        TackyInstruction::Label(loop_start.clone()),
        // cmp = (idx < n)
        TackyInstruction::Binary {
            op: TackyBinaryOp::LessThan,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Var(n.clone()),
            dst: TackyVal::Var(cmp.clone()),
        },
        // if cmp == 0, break
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(cmp.clone()),
            target: loop_end.clone(),
        },
        // dst_ptr = s + idx
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(s.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(dst_ptr.clone()),
        },
        // *dst_ptr = b
        TackyInstruction::Store {
            src: TackyVal::Var(b.clone()),
            dst_ptr: TackyVal::Var(dst_ptr.clone()),
        },
        // idx++
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Jump(loop_start),
        // loop_end:
        TackyInstruction::Label(loop_end),
        // return s
        TackyInstruction::Return(TackyVal::Var(s.clone())),
    ];

    TackyFunction {
        name: name.to_string(),
        global: true,
        params: vec![s, c, n],
        body,
        return_type: Type::Pointer(Box::new(Type::Char)),
        var_types,
        is_variadic: false,
    }
}

/// `memcmp` の等価な TACKY IR 実装を生成する。
///
/// ```c
/// int _obf_memcmp(const char *s1, const char *s2, long n) {
///     long i = 0;
///     while (i < n) {
///         int d = (int)s1[i] - (int)s2[i];
///         if (d != 0) return d;
///         i = i + 1;
///     }
///     return 0;
/// }
/// ```
fn generate_memcmp(ctx: &mut ObfCtx, name: &str) -> TackyFunction {
    let p1 = "p1".to_string();
    let p2 = "p2".to_string();
    let n = "p_n".to_string();
    let idx = ctx.fresh_tmp();
    let cmp = ctx.fresh_tmp();
    let ptr1 = ctx.fresh_tmp();
    let ptr2 = ctx.fresh_tmp();
    let ch1 = ctx.fresh_tmp();
    let ch2 = ctx.fresh_tmp();
    let ci1 = ctx.fresh_tmp();
    let ci2 = ctx.fresh_tmp();
    let diff = ctx.fresh_tmp();

    let loop_start = ctx.fresh_label();
    let loop_end = ctx.fresh_label();
    let ret_diff = ctx.fresh_label();

    let ptr_char = Type::Pointer(Box::new(Type::Char));
    let mut var_types = HashMap::new();
    var_types.insert(p1.clone(), ptr_char.clone());
    var_types.insert(p2.clone(), ptr_char.clone());
    var_types.insert(n.clone(), Type::Long);
    var_types.insert(idx.clone(), Type::Long);
    var_types.insert(cmp.clone(), Type::Int);
    var_types.insert(ptr1.clone(), ptr_char.clone());
    var_types.insert(ptr2.clone(), ptr_char);
    var_types.insert(ch1.clone(), Type::Char);
    var_types.insert(ch2.clone(), Type::Char);
    var_types.insert(ci1.clone(), Type::Int);
    var_types.insert(ci2.clone(), Type::Int);
    var_types.insert(diff.clone(), Type::Int);

    let body = vec![
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Label(loop_start.clone()),
        TackyInstruction::Binary {
            op: TackyBinaryOp::LessThan,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Var(n.clone()),
            dst: TackyVal::Var(cmp.clone()),
        },
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(cmp.clone()),
            target: loop_end.clone(),
        },
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(p1.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(ptr1.clone()),
        },
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(p2.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(ptr2.clone()),
        },
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(ptr1.clone()),
            dst: TackyVal::Var(ch1.clone()),
        },
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(ptr2.clone()),
            dst: TackyVal::Var(ch2.clone()),
        },
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch1.clone()),
            dst: TackyVal::Var(ci1.clone()),
        },
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch2.clone()),
            dst: TackyVal::Var(ci2.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Subtract,
            left: TackyVal::Var(ci1.clone()),
            right: TackyVal::Var(ci2.clone()),
            dst: TackyVal::Var(diff.clone()),
        },
        TackyInstruction::JumpIfNotZero {
            condition: TackyVal::Var(diff.clone()),
            target: ret_diff.clone(),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Jump(loop_start),
        TackyInstruction::Label(ret_diff),
        TackyInstruction::Return(TackyVal::Var(diff)),
        TackyInstruction::Label(loop_end),
        TackyInstruction::Return(TackyVal::Constant(TackyConst::Int(0))),
    ];

    TackyFunction {
        name: name.to_string(),
        global: true,
        params: vec![p1, p2, n],
        body,
        return_type: Type::Int,
        var_types,
        is_variadic: false,
    }
}

/// `strncmp` の等価な TACKY IR 実装を生成する。
///
/// ```c
/// int _obf_strncmp(const char *s1, const char *s2, long n) {
///     long i = 0;
///     while (i < n) {
///         int d = (int)s1[i] - (int)s2[i];
///         if (d != 0) return d;
///         if (s1[i] == '\0') return 0;
///         i = i + 1;
///     }
///     return 0;
/// }
/// ```
fn generate_strncmp(ctx: &mut ObfCtx, name: &str) -> TackyFunction {
    let p1 = "p1".to_string();
    let p2 = "p2".to_string();
    let n = "p_n".to_string();
    let idx = ctx.fresh_tmp();
    let cmp = ctx.fresh_tmp();
    let ptr1 = ctx.fresh_tmp();
    let ptr2 = ctx.fresh_tmp();
    let ch1 = ctx.fresh_tmp();
    let ch2 = ctx.fresh_tmp();
    let ci1 = ctx.fresh_tmp();
    let ci2 = ctx.fresh_tmp();
    let diff = ctx.fresh_tmp();

    let loop_start = ctx.fresh_label();
    let loop_end = ctx.fresh_label();
    let ret_diff = ctx.fresh_label();

    let ptr_char = Type::Pointer(Box::new(Type::Char));
    let mut var_types = HashMap::new();
    var_types.insert(p1.clone(), ptr_char.clone());
    var_types.insert(p2.clone(), ptr_char.clone());
    var_types.insert(n.clone(), Type::Long);
    var_types.insert(idx.clone(), Type::Long);
    var_types.insert(cmp.clone(), Type::Int);
    var_types.insert(ptr1.clone(), ptr_char.clone());
    var_types.insert(ptr2.clone(), ptr_char);
    var_types.insert(ch1.clone(), Type::Char);
    var_types.insert(ch2.clone(), Type::Char);
    var_types.insert(ci1.clone(), Type::Int);
    var_types.insert(ci2.clone(), Type::Int);
    var_types.insert(diff.clone(), Type::Int);

    let body = vec![
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Label(loop_start.clone()),
        // if i >= n, return 0
        TackyInstruction::Binary {
            op: TackyBinaryOp::LessThan,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Var(n.clone()),
            dst: TackyVal::Var(cmp.clone()),
        },
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(cmp.clone()),
            target: loop_end.clone(),
        },
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(p1.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(ptr1.clone()),
        },
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(p2.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(ptr2.clone()),
        },
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(ptr1.clone()),
            dst: TackyVal::Var(ch1.clone()),
        },
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(ptr2.clone()),
            dst: TackyVal::Var(ch2.clone()),
        },
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch1.clone()),
            dst: TackyVal::Var(ci1.clone()),
        },
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch2.clone()),
            dst: TackyVal::Var(ci2.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Subtract,
            left: TackyVal::Var(ci1.clone()),
            right: TackyVal::Var(ci2.clone()),
            dst: TackyVal::Var(diff.clone()),
        },
        TackyInstruction::JumpIfNotZero {
            condition: TackyVal::Var(diff.clone()),
            target: ret_diff.clone(),
        },
        // if s1[i] == '\0', both are equal up to null
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(ci1.clone()),
            target: loop_end.clone(),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Jump(loop_start),
        TackyInstruction::Label(ret_diff),
        TackyInstruction::Return(TackyVal::Var(diff)),
        TackyInstruction::Label(loop_end),
        TackyInstruction::Return(TackyVal::Constant(TackyConst::Int(0))),
    ];

    TackyFunction {
        name: name.to_string(),
        global: true,
        params: vec![p1, p2, n],
        body,
        return_type: Type::Int,
        var_types,
        is_variadic: false,
    }
}

/// `strncpy` の等価な TACKY IR 実装を生成する。
///
/// ```c
/// char *_obf_strncpy(char *dst, const char *src, long n) {
///     long i = 0;
///     while (i < n) {
///         char ch = src[i];
///         dst[i] = ch;
///         if (ch == '\0') break;
///         i = i + 1;
///     }
///     // 残りをゼロ埋め
///     while (i < n) {
///         dst[i] = '\0';
///         i = i + 1;
///     }
///     return dst;
/// }
/// ```
fn generate_strncpy(ctx: &mut ObfCtx, name: &str) -> TackyFunction {
    let dst = "p_dst".to_string();
    let src = "p_src".to_string();
    let n = "p_n".to_string();
    let idx = ctx.fresh_tmp();
    let cmp = ctx.fresh_tmp();
    let src_ptr = ctx.fresh_tmp();
    let dst_ptr = ctx.fresh_tmp();
    let ch = ctx.fresh_tmp();
    let ci = ctx.fresh_tmp();

    let loop1_start = ctx.fresh_label();
    let loop1_end = ctx.fresh_label();
    let loop2_start = ctx.fresh_label();
    let loop2_end = ctx.fresh_label();

    let ptr_char = Type::Pointer(Box::new(Type::Char));
    let mut var_types = HashMap::new();
    var_types.insert(dst.clone(), ptr_char.clone());
    var_types.insert(src.clone(), ptr_char.clone());
    var_types.insert(n.clone(), Type::Long);
    var_types.insert(idx.clone(), Type::Long);
    var_types.insert(cmp.clone(), Type::Int);
    var_types.insert(src_ptr.clone(), ptr_char.clone());
    var_types.insert(dst_ptr.clone(), ptr_char);
    var_types.insert(ch.clone(), Type::Char);
    var_types.insert(ci.clone(), Type::Int);

    let body = vec![
        // idx = 0
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(idx.clone()),
        },
        // ── loop 1: copy src chars ──
        TackyInstruction::Label(loop1_start.clone()),
        TackyInstruction::Binary {
            op: TackyBinaryOp::LessThan,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Var(n.clone()),
            dst: TackyVal::Var(cmp.clone()),
        },
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(cmp.clone()),
            target: loop2_end.clone(), // n reached, skip pad loop too
        },
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(src.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(src_ptr.clone()),
        },
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(src_ptr.clone()),
            dst: TackyVal::Var(ch.clone()),
        },
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(dst.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(dst_ptr.clone()),
        },
        TackyInstruction::Store {
            src: TackyVal::Var(ch.clone()),
            dst_ptr: TackyVal::Var(dst_ptr.clone()),
        },
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch.clone()),
            dst: TackyVal::Var(ci.clone()),
        },
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(ci.clone()),
            target: loop1_end.clone(), // null found, go to pad loop
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Jump(loop1_start),
        // ── null hit; idx already incremented past null ──
        TackyInstruction::Label(loop1_end.clone()),
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(idx.clone()),
        },
        // ── loop 2: zero-pad remaining ──
        TackyInstruction::Label(loop2_start.clone()),
        TackyInstruction::Binary {
            op: TackyBinaryOp::LessThan,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Var(n.clone()),
            dst: TackyVal::Var(cmp.clone()),
        },
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(cmp.clone()),
            target: loop2_end.clone(),
        },
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(dst.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(dst_ptr.clone()),
        },
        TackyInstruction::Store {
            src: TackyVal::Constant(TackyConst::Char(0)),
            dst_ptr: TackyVal::Var(dst_ptr.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Jump(loop2_start),
        // ── done ──
        TackyInstruction::Label(loop2_end),
        TackyInstruction::Return(TackyVal::Var(dst.clone())),
    ];

    TackyFunction {
        name: name.to_string(),
        global: true,
        params: vec![dst, src, n],
        body,
        return_type: Type::Pointer(Box::new(Type::Char)),
        var_types,
        is_variadic: false,
    }
}

/// `strchr` の等価な TACKY IR 実装を生成する。
///
/// ```c
/// char *_obf_strchr(const char *s, int c) {
///     char target = (char)c;
///     long i = 0;
///     for (;;) {
///         char ch = s[i];
///         if (ch == target) return s + i;
///         if (ch == '\0') return (char *)0;
///         i = i + 1;
///     }
/// }
/// ```
fn generate_strchr(ctx: &mut ObfCtx, name: &str) -> TackyFunction {
    let s = "p_s".to_string();
    let c = "p_c".to_string();
    let target = ctx.fresh_tmp();   // Truncate(c) → Char
    let idx = ctx.fresh_tmp();      // Long
    let ptr = ctx.fresh_tmp();      // s + idx
    let ch = ctx.fresh_tmp();       // Char
    let ci = ctx.fresh_tmp();       // ZeroExtend(ch) → Int
    let ti = ctx.fresh_tmp();       // ZeroExtend(target) → Int
    let eq = ctx.fresh_tmp();       // ci == ti (Int)

    let loop_start = ctx.fresh_label();
    let ret_found = ctx.fresh_label();
    let ret_null = ctx.fresh_label();

    let ptr_char = Type::Pointer(Box::new(Type::Char));
    let mut var_types = HashMap::new();
    var_types.insert(s.clone(), ptr_char.clone());
    var_types.insert(c.clone(), Type::Int);
    var_types.insert(target.clone(), Type::Char);
    var_types.insert(idx.clone(), Type::Long);
    var_types.insert(ptr.clone(), ptr_char);
    var_types.insert(ch.clone(), Type::Char);
    var_types.insert(ci.clone(), Type::Int);
    var_types.insert(ti.clone(), Type::Int);
    var_types.insert(eq.clone(), Type::Int);

    let body = vec![
        // target = (char)c
        TackyInstruction::Truncate {
            src: TackyVal::Var(c.clone()),
            dst: TackyVal::Var(target.clone()),
        },
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(target.clone()),
            dst: TackyVal::Var(ti.clone()),
        },
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Label(loop_start.clone()),
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(s.clone()),
            index: TackyVal::Var(idx.clone()),
            scale: 1,
            dst: TackyVal::Var(ptr.clone()),
        },
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(ptr.clone()),
            dst: TackyVal::Var(ch.clone()),
        },
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch.clone()),
            dst: TackyVal::Var(ci.clone()),
        },
        // if ch == target → return ptr
        TackyInstruction::Binary {
            op: TackyBinaryOp::Equal,
            left: TackyVal::Var(ci.clone()),
            right: TackyVal::Var(ti.clone()),
            dst: TackyVal::Var(eq.clone()),
        },
        TackyInstruction::JumpIfNotZero {
            condition: TackyVal::Var(eq.clone()),
            target: ret_found.clone(),
        },
        // if ch == '\0' → return NULL
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(ci.clone()),
            target: ret_null.clone(),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(idx.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(idx.clone()),
        },
        TackyInstruction::Jump(loop_start),
        // ret_found: return s + idx (= ptr)
        TackyInstruction::Label(ret_found),
        TackyInstruction::Return(TackyVal::Var(ptr.clone())),
        // ret_null: return 0 (NULL)
        TackyInstruction::Label(ret_null),
        TackyInstruction::Return(TackyVal::Constant(TackyConst::Long(0))),
    ];

    TackyFunction {
        name: name.to_string(),
        global: true,
        params: vec![s, c],
        body,
        return_type: Type::Pointer(Box::new(Type::Char)),
        var_types,
        is_variadic: false,
    }
}

/// `strcat` の等価な TACKY IR 実装を生成する。
///
/// ```c
/// char *_obf_strcat(char *dst, const char *src) {
///     // Phase 1: dst の末尾を探す
///     long di = 0;
///     while (dst[di] != '\0') di = di + 1;
///     // Phase 2: src をコピー
///     long si = 0;
///     for (;;) {
///         char ch = src[si];
///         dst[di] = ch;
///         if (ch == '\0') break;
///         di = di + 1; si = si + 1;
///     }
///     return dst;
/// }
/// ```
fn generate_strcat(ctx: &mut ObfCtx, name: &str) -> TackyFunction {
    let dst = "p_dst".to_string();
    let src = "p_src".to_string();
    let di = ctx.fresh_tmp();       // dst index (Long)
    let si = ctx.fresh_tmp();       // src index (Long)
    let dptr = ctx.fresh_tmp();     // dst + di
    let sptr = ctx.fresh_tmp();     // src + si
    let ch = ctx.fresh_tmp();       // Char
    let ci = ctx.fresh_tmp();       // Int

    let find_start = ctx.fresh_label();
    let find_end = ctx.fresh_label();
    let copy_start = ctx.fresh_label();
    let copy_end = ctx.fresh_label();

    let ptr_char = Type::Pointer(Box::new(Type::Char));
    let mut var_types = HashMap::new();
    var_types.insert(dst.clone(), ptr_char.clone());
    var_types.insert(src.clone(), ptr_char.clone());
    var_types.insert(di.clone(), Type::Long);
    var_types.insert(si.clone(), Type::Long);
    var_types.insert(dptr.clone(), ptr_char.clone());
    var_types.insert(sptr.clone(), ptr_char);
    var_types.insert(ch.clone(), Type::Char);
    var_types.insert(ci.clone(), Type::Int);

    let body = vec![
        // ── Phase 1: find end of dst ──
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(di.clone()),
        },
        TackyInstruction::Label(find_start.clone()),
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(dst.clone()),
            index: TackyVal::Var(di.clone()),
            scale: 1,
            dst: TackyVal::Var(dptr.clone()),
        },
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(dptr.clone()),
            dst: TackyVal::Var(ch.clone()),
        },
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch.clone()),
            dst: TackyVal::Var(ci.clone()),
        },
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(ci.clone()),
            target: find_end.clone(),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(di.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(di.clone()),
        },
        TackyInstruction::Jump(find_start),
        TackyInstruction::Label(find_end.clone()),
        // ── Phase 2: copy src to dst+di ──
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(si.clone()),
        },
        TackyInstruction::Label(copy_start.clone()),
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(src.clone()),
            index: TackyVal::Var(si.clone()),
            scale: 1,
            dst: TackyVal::Var(sptr.clone()),
        },
        TackyInstruction::Load {
            src_ptr: TackyVal::Var(sptr.clone()),
            dst: TackyVal::Var(ch.clone()),
        },
        TackyInstruction::AddPtr {
            ptr: TackyVal::Var(dst.clone()),
            index: TackyVal::Var(di.clone()),
            scale: 1,
            dst: TackyVal::Var(dptr.clone()),
        },
        TackyInstruction::Store {
            src: TackyVal::Var(ch.clone()),
            dst_ptr: TackyVal::Var(dptr.clone()),
        },
        TackyInstruction::ZeroExtend {
            src: TackyVal::Var(ch.clone()),
            dst: TackyVal::Var(ci.clone()),
        },
        TackyInstruction::JumpIfZero {
            condition: TackyVal::Var(ci.clone()),
            target: copy_end.clone(),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(di.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(di.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(si.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(si.clone()),
        },
        TackyInstruction::Jump(copy_start),
        TackyInstruction::Label(copy_end),
        TackyInstruction::Return(TackyVal::Var(dst.clone())),
    ];

    TackyFunction {
        name: name.to_string(),
        global: true,
        params: vec![dst, src],
        body,
        return_type: Type::Pointer(Box::new(Type::Char)),
        var_types,
        is_variadic: false,
    }
}

// ─────────────────────────────────────────────────────────────
// Pass 12: Function Inlining（関数インライン展開）
// ─────────────────────────────────────────────────────────────

/// 関数呼び出しを呼び出し先の関数本体で置換する。
/// コールグラフを破壊し、元の関数構造の復元を困難にする。
fn inline_functions(program: &mut TackyProgram, ctx: &mut ObfCtx, freq: usize) {
    // 静的変数・静的定数の名前を収集（リネーム対象外）
    let static_names: HashSet<String> = program.static_vars.iter().map(|v| v.name.clone())
        .chain(program.static_constants.iter().map(|c| c.name.clone()))
        .collect();

    // 全関数を clone して callee_map を構築（可変借用と不変借用の衝突を回避）
    let callee_map: HashMap<String, TackyFunction> = program.functions.iter()
        .map(|f| (f.name.clone(), f.clone()))
        .collect();

    for func in &mut program.functions {
        let mut new_body = Vec::new();
        let mut eligible_count = 0usize;

        for instr in std::mem::take(&mut func.body) {
            if let TackyInstruction::FunCall { ref name, ref args, ref dst, ref dst_type, is_variadic: _ } = instr {
                if let Some(callee) = callee_map.get(name) {
                    if is_inline_eligible(callee, name, dst_type, &static_names) {
                        eligible_count += 1;
                        if freq > 0 && eligible_count % freq == 0 {
                            // インライン展開を実行
                            let prefix = format!("_inline_{}", ctx.inline_counter);
                            ctx.inline_counter += 1;
                            let end_label = format!("{}_end", prefix);

                            // 引数→リネームされたパラメータへの Copy
                            for (param, arg) in callee.params.iter().zip(args.iter()) {
                                let renamed_param = format!("{}_{}", prefix, param);
                                new_body.push(TackyInstruction::Copy {
                                    src: arg.clone(),
                                    dst: TackyVal::Var(renamed_param),
                                });
                            }

                            // リネームされた本体を挿入
                            for callee_instr in &callee.body {
                                match callee_instr {
                                    TackyInstruction::Return(val) => {
                                        if !matches!(callee.return_type, Type::Void) {
                                            new_body.push(TackyInstruction::Copy {
                                                src: rename_val(val, &prefix, &static_names),
                                                dst: dst.clone(),
                                            });
                                        }
                                        new_body.push(TackyInstruction::Jump(end_label.clone()));
                                    }
                                    TackyInstruction::ReturnVoid => {
                                        new_body.push(TackyInstruction::Jump(end_label.clone()));
                                    }
                                    _ => {
                                        new_body.push(rename_instruction(
                                            callee_instr, &prefix, &static_names,
                                            dst, &end_label, &callee.return_type,
                                        ));
                                    }
                                }
                            }

                            // end ラベル
                            new_body.push(TackyInstruction::Label(end_label.clone()));

                            // リネームされた変数を呼び出し元の var_types に追加
                            for (var_name, var_type) in &callee.var_types {
                                if !static_names.contains(var_name) {
                                    let renamed = format!("{}_{}", prefix, var_name);
                                    func.var_types.insert(renamed, var_type.clone());
                                }
                            }

                            continue;
                        }
                    }
                }
            }
            new_body.push(instr);
        }

        func.body = new_body;
    }
}

/// インライン適格条件を判定する
fn is_inline_eligible(
    callee: &TackyFunction,
    callee_name: &str,
    dst_type: &Type,
    static_names: &HashSet<String>,
) -> bool {
    // 1. main() でない
    if callee_name == "main" {
        return false;
    }
    // 2. 本体が空でない
    if callee.body.is_empty() {
        return false;
    }
    // 3. 本体が ≤ 50 命令
    if callee.body.len() > 50 {
        return false;
    }
    // 4. 戻り値型が Struct でない
    if matches!(dst_type, Type::Struct { .. }) {
        return false;
    }
    // 5. 直接再帰でない
    if is_directly_recursive(callee) {
        return false;
    }
    // 6. パラメータの GetAddress を含まない
    if has_param_address_taken(callee, static_names) {
        return false;
    }
    true
}

/// 関数本体に自身への FunCall があるか判定する
fn is_directly_recursive(func: &TackyFunction) -> bool {
    func.body.iter().any(|instr| {
        matches!(instr, TackyInstruction::FunCall { name, .. } if name == &func.name)
    })
}

/// GetAddress の src がパラメータか判定する
fn has_param_address_taken(func: &TackyFunction, static_names: &HashSet<String>) -> bool {
    let params: HashSet<&str> = func.params.iter().map(|s| s.as_str()).collect();
    func.body.iter().any(|instr| {
        if let TackyInstruction::GetAddress { src: TackyVal::Var(name), .. } = instr {
            // 静的変数はパラメータではない
            !static_names.contains(name) && params.contains(name.as_str())
        } else {
            false
        }
    })
}

/// TackyVal のリネーム。Var をリネームし Constant はそのまま。
fn rename_val(val: &TackyVal, prefix: &str, static_names: &HashSet<String>) -> TackyVal {
    match val {
        TackyVal::Var(name) => {
            if static_names.contains(name) {
                val.clone()
            } else {
                TackyVal::Var(format!("{}_{}", prefix, name))
            }
        }
        TackyVal::Constant(_) => val.clone(),
    }
}

/// ラベル名のリネーム
fn rename_label(label: &str, prefix: &str) -> String {
    format!("{}_{}", prefix, label)
}

/// 命令全体のリネーム。全 TackyInstruction バリアントの変数・ラベルをリネームする。
/// Return / ReturnVoid は call_dst への Copy + Jump(end_label) に変換する。
fn rename_instruction(
    instr: &TackyInstruction,
    prefix: &str,
    static_names: &HashSet<String>,
    call_dst: &TackyVal,
    end_label: &str,
    return_type: &Type,
) -> TackyInstruction {
    let rv = |v: &TackyVal| rename_val(v, prefix, static_names);
    let rl = |l: &str| rename_label(l, prefix);

    // Return を特別処理するためマクロ的に書かない
    match instr {
        // Return(val) → Copy { src: rename(val), dst: call_dst } + Jump(end_label)
        // ただしここでは1命令しか返せないので、呼び出し側で特別処理が必要。
        // 実際には rename_instruction は inline_functions 内のループで呼ばれるので、
        // Return は呼び出し側で展開する。ここでは Copy に変換する。
        TackyInstruction::Return(val) => {
            if matches!(return_type, Type::Void) {
                TackyInstruction::Jump(end_label.to_string())
            } else {
                // Return は2命令に展開する必要があるが、1命令しか返せないので
                // Copy として返し、呼び出し側で Jump を追加する設計にする。
                // → 実際には inline_functions 内で直接展開するべき。
                // ここでは placeholder として Copy + Jump の最初の命令を返す。
                TackyInstruction::Copy {
                    src: rv(val),
                    dst: call_dst.clone(),
                }
            }
        }
        TackyInstruction::ReturnVoid => {
            TackyInstruction::Jump(end_label.to_string())
        }

        TackyInstruction::Unary { op, src, dst } => TackyInstruction::Unary {
            op: *op, src: rv(src), dst: rv(dst),
        },
        TackyInstruction::Binary { op, left, right, dst } => TackyInstruction::Binary {
            op: *op, left: rv(left), right: rv(right), dst: rv(dst),
        },
        TackyInstruction::Copy { src, dst } => TackyInstruction::Copy {
            src: rv(src), dst: rv(dst),
        },
        TackyInstruction::Jump(target) => TackyInstruction::Jump(rl(target)),
        TackyInstruction::JumpIfZero { condition, target } => TackyInstruction::JumpIfZero {
            condition: rv(condition), target: rl(target),
        },
        TackyInstruction::JumpIfNotZero { condition, target } => TackyInstruction::JumpIfNotZero {
            condition: rv(condition), target: rl(target),
        },
        TackyInstruction::Label(name) => TackyInstruction::Label(rl(name)),
        TackyInstruction::FunCall { name, args, dst, dst_type, is_variadic } => TackyInstruction::FunCall {
            name: name.clone(), // 関数名はリネームしない
            args: args.iter().map(|a| rv(a)).collect(),
            dst: rv(dst),
            dst_type: dst_type.clone(),
            is_variadic: *is_variadic,
        },
        TackyInstruction::SignExtend { src, dst } => TackyInstruction::SignExtend {
            src: rv(src), dst: rv(dst),
        },
        TackyInstruction::ZeroExtend { src, dst } => TackyInstruction::ZeroExtend {
            src: rv(src), dst: rv(dst),
        },
        TackyInstruction::Truncate { src, dst } => TackyInstruction::Truncate {
            src: rv(src), dst: rv(dst),
        },
        TackyInstruction::IntToDouble { src, dst } => TackyInstruction::IntToDouble {
            src: rv(src), dst: rv(dst),
        },
        TackyInstruction::DoubleToInt { src, dst } => TackyInstruction::DoubleToInt {
            src: rv(src), dst: rv(dst),
        },
        TackyInstruction::UIntToDouble { src, dst } => TackyInstruction::UIntToDouble {
            src: rv(src), dst: rv(dst),
        },
        TackyInstruction::DoubleToUInt { src, dst } => TackyInstruction::DoubleToUInt {
            src: rv(src), dst: rv(dst),
        },
        TackyInstruction::GetAddress { src, dst } => TackyInstruction::GetAddress {
            src: rv(src), dst: rv(dst),
        },
        TackyInstruction::Load { src_ptr, dst } => TackyInstruction::Load {
            src_ptr: rv(src_ptr), dst: rv(dst),
        },
        TackyInstruction::Store { src, dst_ptr } => TackyInstruction::Store {
            src: rv(src), dst_ptr: rv(dst_ptr),
        },
        TackyInstruction::AddPtr { ptr, index, scale, dst } => TackyInstruction::AddPtr {
            ptr: rv(ptr), index: rv(index), scale: *scale, dst: rv(dst),
        },
        TackyInstruction::CopyToOffset { src, dst, offset } => TackyInstruction::CopyToOffset {
            src: rv(src),
            dst: if static_names.contains(dst) { dst.clone() } else { format!("{}_{}", prefix, dst) },
            offset: *offset,
        },
        TackyInstruction::CopyFromOffset { src, offset, dst } => TackyInstruction::CopyFromOffset {
            src: if static_names.contains(src) { src.clone() } else { format!("{}_{}", prefix, src) },
            offset: *offset,
            dst: rv(dst),
        },
        TackyInstruction::CopyStruct { src, dst, size } => TackyInstruction::CopyStruct {
            src: rv(src), dst: rv(dst), size: *size,
        },
        TackyInstruction::JumpIndirect { target, possible_targets } => TackyInstruction::JumpIndirect {
            target: rv(target),
            possible_targets: possible_targets.iter().map(|l| rl(l)).collect(),
        },
        TackyInstruction::VaStart { ap, gp_offset_init, fp_offset_init } => TackyInstruction::VaStart {
            ap: rv(ap), gp_offset_init: *gp_offset_init, fp_offset_init: *fp_offset_init,
        },
        TackyInstruction::VaArg { ap, dst, arg_type } => TackyInstruction::VaArg {
            ap: rv(ap), dst: rv(dst), arg_type: arg_type.clone(),
        },
        TackyInstruction::VaEnd => TackyInstruction::VaEnd,
    }
}

// ─────────────────────────────────────────────────────────────
// Pass 13: Function Outlining（関数アウトライン化）
// ─────────────────────────────────────────────────────────────

/// コード断片を新しい関数に切り出す。
/// 偽の関数が大量に出現し、元の関数構造の復元を困難にする。
fn outline_functions(program: &mut TackyProgram, ctx: &mut ObfCtx, min_block_size: usize) {
    let mut new_functions: Vec<TackyFunction> = Vec::new();
    // 1関数あたりの最大アウトライン数（CFF の実行時オーバーヘッドを抑制）
    const MAX_OUTLINES_PER_FUNC: usize = 30;

    for func in &mut program.functions {
        let mut new_body: Vec<TackyInstruction> = Vec::new();
        let body = std::mem::take(&mut func.body);
        let mut i = 0;
        let mut outline_count = 0usize;

        while i < body.len() {
            // アウトライン候補ブロックを検索（上限チェック付き）
            if outline_count < MAX_OUTLINES_PER_FUNC {
                if let Some(block_len) = find_outline_candidate(&body, i, min_block_size) {
                    let block = &body[i..i + block_len];

                    if let Some((inputs, output_name, intermediates)) =
                        analyze_block(block, &body, i, &func.var_types)
                    {
                        // 入力変数 ≤ 6（整数レジスタ呼出規約の上限）
                        if inputs.len() <= 6 {
                            // Double / Struct / Array 型の入出力を除外
                            let output_type = func.var_types.get(&output_name)
                                .cloned().unwrap_or(Type::Int);
                            let has_bad_type = matches!(output_type,
                                Type::Double | Type::Struct { .. } | Type::Array(_, _))
                                || inputs.iter().any(|name| {
                                    let ty = func.var_types.get(name).unwrap_or(&Type::Int);
                                    matches!(ty, Type::Double | Type::Struct { .. } | Type::Array(_, _))
                                });

                            if !has_bad_type {
                                // 新関数を構築
                                let outlined_func = build_outlined_function(
                                    ctx, block, &inputs, &output_name, &intermediates,
                                    &output_type, &func.var_types,
                                );
                                let func_name = outlined_func.name.clone();

                                // 元の位置に FunCall を挿入
                                let call_args: Vec<TackyVal> = inputs.iter()
                                    .map(|name| TackyVal::Var(name.clone()))
                                    .collect();
                                new_body.push(TackyInstruction::FunCall {
                                    name: func_name,
                                    args: call_args,
                                    dst: TackyVal::Var(output_name),
                                    dst_type: output_type,
                                    is_variadic: false,
                                });

                                new_functions.push(outlined_func);
                                outline_count += 1;
                                i += block_len;
                                continue;
                            }
                        }
                    }
                }
            }

            new_body.push(body[i].clone());
            i += 1;
        }

        func.body = new_body;
    }

    // 新しく生成された関数をプログラムに追加
    program.functions.extend(new_functions);
}

/// アウトライン候補ブロックを検出する。
/// pos から始まる連続する Copy / Binary / Unary 命令のブロックを探す。
fn find_outline_candidate(body: &[TackyInstruction], pos: usize, min_size: usize) -> Option<usize> {
    let mut len = 0;
    for instr in &body[pos..] {
        match instr {
            TackyInstruction::Copy { .. }
            | TackyInstruction::Binary { .. }
            | TackyInstruction::Unary { .. } => {
                len += 1;
            }
            _ => break,
        }
    }
    if len >= min_size {
        Some(len)
    } else {
        None
    }
}

/// ブロックの入出力を解析する。
/// 成功時: (入力変数名リスト, 出力変数名, 中間変数名集合) を返す。
/// 安全でない場合は None を返す。
///
/// `full_body` は関数本体全体、`block_start` はブロックの開始位置。
/// 安全性チェックではブロック外の全命令（ブロック前+ブロック後）を走査する。
/// これによりループの後方ジャンプで使われる変数も正しく検出される。
fn analyze_block(
    block: &[TackyInstruction],
    full_body: &[TackyInstruction],
    block_start: usize,
    var_types: &HashMap<String, Type>,
) -> Option<(Vec<String>, String, HashSet<String>)> {
    let mut inputs: Vec<String> = Vec::new();
    let mut input_set: HashSet<String> = HashSet::new();
    let mut written: HashSet<String> = HashSet::new();

    for instr in block {
        // ソースオペランドを収集
        for src_val in instruction_sources(instr) {
            if let TackyVal::Var(name) = src_val {
                if !written.contains(name) && !input_set.contains(name) {
                    inputs.push(name.clone());
                    input_set.insert(name.clone());
                }
            }
        }
        // dst を written に追加
        if let Some(dst_name) = instruction_dst_name(instr) {
            written.insert(dst_name);
        }
    }

    // 出力 = 最後の命令の dst
    let output = instruction_dst_name(block.last()?)?;

    // 中間変数 = written - {output}
    let mut intermediates = written;
    intermediates.remove(&output);

    // 安全性チェック: 中間変数がブロック外（前方+後方）で使われていないか
    // ループの後方ジャンプで参照される変数を見逃さないよう全体を走査する
    if !intermediates.is_empty() {
        let block_end = block_start + block.len();
        for (idx, instr) in full_body.iter().enumerate() {
            // ブロック内の命令はスキップ
            if idx >= block_start && idx < block_end {
                continue;
            }
            for operand in instruction_all_operands(instr) {
                if let TackyVal::Var(name) = operand {
                    if intermediates.contains(name) {
                        return None; // 中間変数がブロック外で使われている
                    }
                }
            }
        }
    }

    // 入力変数の型チェック（Double / Struct / Array を除外）
    let _ = var_types; // 型チェックは呼び出し側で行う

    Some((inputs, output, intermediates))
}

/// 命令のソースオペランド（読まれる TackyVal）を返す
fn instruction_sources(instr: &TackyInstruction) -> Vec<&TackyVal> {
    match instr {
        TackyInstruction::Copy { src, .. } => vec![src],
        TackyInstruction::Unary { src, .. } => vec![src],
        TackyInstruction::Binary { left, right, .. } => vec![left, right],
        _ => vec![],
    }
}

/// 命令の dst 変数名を返す
fn instruction_dst_name(instr: &TackyInstruction) -> Option<String> {
    match instr {
        TackyInstruction::Copy { dst: TackyVal::Var(name), .. }
        | TackyInstruction::Unary { dst: TackyVal::Var(name), .. }
        | TackyInstruction::Binary { dst: TackyVal::Var(name), .. } => Some(name.clone()),
        _ => None,
    }
}

/// 命令の全オペランド（ソース+dst）を返す（中間変数の使用チェック用）
fn instruction_all_operands(instr: &TackyInstruction) -> Vec<&TackyVal> {
    match instr {
        TackyInstruction::Return(val) => vec![val],
        TackyInstruction::ReturnVoid => vec![],
        TackyInstruction::Unary { src, dst, .. } => vec![src, dst],
        TackyInstruction::Binary { left, right, dst, .. } => vec![left, right, dst],
        TackyInstruction::Copy { src, dst } => vec![src, dst],
        TackyInstruction::Jump(_) => vec![],
        TackyInstruction::JumpIfZero { condition, .. }
        | TackyInstruction::JumpIfNotZero { condition, .. } => vec![condition],
        TackyInstruction::Label(_) => vec![],
        TackyInstruction::FunCall { args, dst, .. } => {
            let mut v: Vec<&TackyVal> = args.iter().collect();
            v.push(dst);
            v
        }
        TackyInstruction::SignExtend { src, dst }
        | TackyInstruction::ZeroExtend { src, dst }
        | TackyInstruction::Truncate { src, dst }
        | TackyInstruction::IntToDouble { src, dst }
        | TackyInstruction::DoubleToInt { src, dst }
        | TackyInstruction::UIntToDouble { src, dst }
        | TackyInstruction::DoubleToUInt { src, dst } => vec![src, dst],
        TackyInstruction::GetAddress { src, dst } => vec![src, dst],
        TackyInstruction::Load { src_ptr, dst } => vec![src_ptr, dst],
        TackyInstruction::Store { src, dst_ptr } => vec![src, dst_ptr],
        TackyInstruction::AddPtr { ptr, index, dst, .. } => vec![ptr, index, dst],
        TackyInstruction::CopyToOffset { src, .. } => vec![src],
        TackyInstruction::CopyFromOffset { dst, .. } => vec![dst],
        TackyInstruction::CopyStruct { src, dst, .. } => vec![src, dst],
        TackyInstruction::JumpIndirect { target, .. } => vec![target],
        TackyInstruction::VaStart { ap, .. } => vec![ap],
        TackyInstruction::VaArg { ap, dst, .. } => vec![ap, dst],
        TackyInstruction::VaEnd => vec![],
    }
}

/// 新関数を構築する
fn build_outlined_function(
    ctx: &mut ObfCtx,
    block: &[TackyInstruction],
    inputs: &[String],
    output_name: &str,
    intermediates: &HashSet<String>,
    output_type: &Type,
    caller_var_types: &HashMap<String, Type>,
) -> TackyFunction {
    let func_name = format!("_obf_outlined_{}", ctx.outline_counter);
    ctx.outline_counter += 1;

    // 入力変数→パラメータ名のマッピング
    let mut input_to_param: HashMap<String, String> = HashMap::new();
    let mut params: Vec<String> = Vec::new();
    let mut var_types: HashMap<String, Type> = HashMap::new();

    for input_name in inputs {
        let param_name = ctx.fresh_tmp();
        let ty = caller_var_types.get(input_name).cloned().unwrap_or(Type::Int);
        var_types.insert(param_name.clone(), ty);
        input_to_param.insert(input_name.clone(), param_name.clone());
        params.push(param_name);
    }

    // 中間変数→新名前のマッピング
    let mut intermediate_to_new: HashMap<String, String> = HashMap::new();
    for name in intermediates {
        let new_name = ctx.fresh_tmp();
        let ty = caller_var_types.get(name).cloned().unwrap_or(Type::Int);
        var_types.insert(new_name.clone(), ty);
        intermediate_to_new.insert(name.clone(), new_name);
    }

    // 出力変数→新名前
    let output_new = ctx.fresh_tmp();
    var_types.insert(output_new.clone(), output_type.clone());

    // 命令のリネーム
    let rename = |val: &TackyVal| -> TackyVal {
        match val {
            TackyVal::Var(name) => {
                if let Some(param) = input_to_param.get(name) {
                    TackyVal::Var(param.clone())
                } else if let Some(new_name) = intermediate_to_new.get(name) {
                    TackyVal::Var(new_name.clone())
                } else if name == output_name {
                    TackyVal::Var(output_new.clone())
                } else {
                    val.clone()
                }
            }
            TackyVal::Constant(_) => val.clone(),
        }
    };

    let mut body: Vec<TackyInstruction> = Vec::new();
    for instr in block {
        let new_instr = match instr {
            TackyInstruction::Copy { src, dst } => TackyInstruction::Copy {
                src: rename(src), dst: rename(dst),
            },
            TackyInstruction::Unary { op, src, dst } => TackyInstruction::Unary {
                op: *op, src: rename(src), dst: rename(dst),
            },
            TackyInstruction::Binary { op, left, right, dst } => TackyInstruction::Binary {
                op: *op, left: rename(left), right: rename(right), dst: rename(dst),
            },
            _ => instr.clone(),
        };
        body.push(new_instr);
    }

    // Return(output)
    body.push(TackyInstruction::Return(TackyVal::Var(output_new)));

    TackyFunction {
        name: func_name,
        global: false,
        params,
        body,
        return_type: output_type.clone(),
        var_types,
        is_variadic: false,
    }
}

// ─────────────────────────────────────────────────────────────
// Pass 6: String Encryption（文字列暗号化）
// ─────────────────────────────────────────────────────────────

/// 文字列定数を暗号化し、main() の先頭に復号コードを挿入する。
///
/// 1. static_constants から StringInit を抽出し、バイト列を加算暗号化
/// 2. 暗号化バイト列を ByteArrayInit として static_vars に移動（.data、書き込み可能）
/// 3. main() の先頭にアンロール復号コードを挿入:
///    各バイトを Load → Subtract(key) → Store で復号
fn string_encryption(program: &mut TackyProgram, ctx: &mut ObfCtx, key: u8) {
    // 暗号化対象の文字列定数を収集
    let mut encrypted_strings: Vec<(String, Vec<u8>, usize)> = Vec::new(); // (label, encrypted_bytes, original_len_with_null)

    program.static_constants.retain(|sc| {
        if let TackyStaticInit::StringInit(content, byte_len) = &sc.init {
            // 各バイトをキーで加算暗号化（null 終端含む）
            let mut encrypted: Vec<u8> = content.as_bytes().iter()
                .map(|b| b.wrapping_add(key))
                .collect();
            // null 終端も暗号化
            encrypted.push(0u8.wrapping_add(key));

            encrypted_strings.push((sc.name.clone(), encrypted, *byte_len));
            false // static_constants から除去
        } else {
            true // StringInit 以外はそのまま残す
        }
    });

    if encrypted_strings.is_empty() {
        return;
    }

    // 暗号化バイト列を static_vars に追加（.data セクション、書き込み可能）
    for (label, encrypted_bytes, byte_len) in &encrypted_strings {
        program.static_vars.push(TackyStaticVar {
            name: label.clone(),
            global: false,
            var_type: Type::Array(Box::new(Type::Char), *byte_len),
            init: TackyStaticInit::ByteArrayInit(encrypted_bytes.clone()),
        });
    }

    // main() を探して先頭に復号コードを挿入
    if let Some(main_func) = program.functions.iter_mut().find(|f| f.name == "main") {
        let mut decrypt_instrs = Vec::new();

        for (label, _, byte_len) in &encrypted_strings {
            // base_ptr = &encrypted_string
            let base_ptr = ctx.fresh_tmp();
            main_func.var_types.insert(base_ptr.clone(), Type::Pointer(Box::new(Type::Char)));

            decrypt_instrs.push(TackyInstruction::GetAddress {
                src: TackyVal::Var(label.clone()),
                dst: TackyVal::Var(base_ptr.clone()),
            });

            // 各バイトを復号（アンロール）
            for i in 0..*byte_len {
                let byte_ptr = ctx.fresh_tmp();
                let enc_byte = ctx.fresh_tmp();
                let enc_int = ctx.fresh_tmp();
                let dec_int = ctx.fresh_tmp();
                let dec_byte = ctx.fresh_tmp();
                main_func.var_types.insert(byte_ptr.clone(), Type::Pointer(Box::new(Type::Char)));
                main_func.var_types.insert(enc_byte.clone(), Type::Char);
                main_func.var_types.insert(enc_int.clone(), Type::Int);
                main_func.var_types.insert(dec_int.clone(), Type::Int);
                main_func.var_types.insert(dec_byte.clone(), Type::Char);

                // byte_ptr = base_ptr + i
                decrypt_instrs.push(TackyInstruction::AddPtr {
                    ptr: TackyVal::Var(base_ptr.clone()),
                    index: TackyVal::Constant(TackyConst::Int(i as i32)),
                    scale: 1,
                    dst: TackyVal::Var(byte_ptr.clone()),
                });

                // enc_byte = *byte_ptr
                decrypt_instrs.push(TackyInstruction::Load {
                    src_ptr: TackyVal::Var(byte_ptr.clone()),
                    dst: TackyVal::Var(enc_byte.clone()),
                });

                // enc_int = sign_extend(enc_byte)
                decrypt_instrs.push(TackyInstruction::SignExtend {
                    src: TackyVal::Var(enc_byte),
                    dst: TackyVal::Var(enc_int.clone()),
                });

                // dec_int = enc_int - KEY
                decrypt_instrs.push(TackyInstruction::Binary {
                    op: TackyBinaryOp::Subtract,
                    left: TackyVal::Var(enc_int),
                    right: TackyVal::Constant(TackyConst::Int(key as i32)),
                    dst: TackyVal::Var(dec_int.clone()),
                });

                // dec_byte = truncate(dec_int)
                decrypt_instrs.push(TackyInstruction::Truncate {
                    src: TackyVal::Var(dec_int),
                    dst: TackyVal::Var(dec_byte.clone()),
                });

                // *byte_ptr = dec_byte
                decrypt_instrs.push(TackyInstruction::Store {
                    src: TackyVal::Var(dec_byte),
                    dst_ptr: TackyVal::Var(byte_ptr),
                });
            }
        }

        // main() の先頭に復号コードを挿入
        decrypt_instrs.append(&mut main_func.body);
        main_func.body = decrypt_instrs;
    }
}

// ─────────────────────────────────────────────────────────────
// Pass 1: Constant Encoding（定数の間接化）
// ─────────────────────────────────────────────────────────────

/// 定数 Copy を実行時計算に置換する。
/// `Copy { src: Constant(42), dst }` → `a * b + c` の演算に分解。
/// Double は精度問題があるためスキップ。
fn constant_encoding(
    instrs: Vec<TackyInstruction>,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
) -> Vec<TackyInstruction> {
    let mut result = Vec::new();

    for instr in instrs {
        match &instr {
            TackyInstruction::Copy { src: TackyVal::Constant(c), dst } => {
                if let Some(encoded) = encode_constant(c, dst, ctx, var_types) {
                    result.extend(encoded);
                    continue;
                }
                result.push(instr);
            }
            _ => result.push(instr),
        }
    }

    result
}

/// 定数を `a * b + c` の形に分解する命令列を生成する。
fn encode_constant(
    c: &TackyConst,
    dst: &TackyVal,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
) -> Option<Vec<TackyInstruction>> {
    match c {
        TackyConst::Int(v) => Some(encode_int_constant(*v as i64, Type::Int, dst, ctx, var_types,
            |x| TackyConst::Int(x as i32))),
        TackyConst::Long(v) => Some(encode_int_constant(*v, Type::Long, dst, ctx, var_types,
            |x| TackyConst::Long(x))),
        TackyConst::UInt(v) => Some(encode_int_constant(*v as i64, Type::UInt, dst, ctx, var_types,
            |x| TackyConst::UInt(x as u32))),
        TackyConst::ULong(v) => Some(encode_int_constant(*v as i64, Type::ULong, dst, ctx, var_types,
            |x| TackyConst::ULong(x as u64))),
        TackyConst::Char(v) => Some(encode_int_constant(*v as i64, Type::Char, dst, ctx, var_types,
            |x| TackyConst::Char(x as i8))),
        TackyConst::UChar(v) => Some(encode_int_constant(*v as i64, Type::UChar, dst, ctx, var_types,
            |x| TackyConst::UChar(x as u8))),
        // Double は精度問題があるためスキップ
        TackyConst::Double(_) => None,
    }
}

/// 整数値を `a * b + c == value` に分解する命令列を生成する。
fn encode_int_constant<F>(
    value: i64,
    ty: Type,
    dst: &TackyVal,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
    make_const: F,
) -> Vec<TackyInstruction>
where
    F: Fn(i64) -> TackyConst,
{
    let mut instrs = Vec::new();

    if value == 0 {
        // 0 → a - a パターン
        let tmp_a = ctx.fresh_tmp();
        var_types.insert(tmp_a.clone(), ty.clone());

        instrs.push(TackyInstruction::Copy {
            src: TackyVal::Constant(make_const(7)),
            dst: TackyVal::Var(tmp_a.clone()),
        });
        instrs.push(TackyInstruction::Binary {
            op: TackyBinaryOp::Subtract,
            left: TackyVal::Var(tmp_a.clone()),
            right: TackyVal::Var(tmp_a),
            dst: dst.clone(),
        });
    } else {
        // value → a * b + c
        // 因数を見つける（簡易: 小さな因数で割る）
        let (a, b, c) = decompose_value(value);

        let tmp_a = ctx.fresh_tmp();
        let tmp_b = ctx.fresh_tmp();
        let tmp_mul = ctx.fresh_tmp();
        var_types.insert(tmp_a.clone(), ty.clone());
        var_types.insert(tmp_b.clone(), ty.clone());
        var_types.insert(tmp_mul.clone(), ty.clone());

        instrs.push(TackyInstruction::Copy {
            src: TackyVal::Constant(make_const(a)),
            dst: TackyVal::Var(tmp_a.clone()),
        });
        instrs.push(TackyInstruction::Copy {
            src: TackyVal::Constant(make_const(b)),
            dst: TackyVal::Var(tmp_b.clone()),
        });
        instrs.push(TackyInstruction::Binary {
            op: TackyBinaryOp::Multiply,
            left: TackyVal::Var(tmp_a),
            right: TackyVal::Var(tmp_b),
            dst: TackyVal::Var(tmp_mul.clone()),
        });

        if c == 0 {
            instrs.push(TackyInstruction::Copy {
                src: TackyVal::Var(tmp_mul),
                dst: dst.clone(),
            });
        } else {
            let tmp_c = ctx.fresh_tmp();
            var_types.insert(tmp_c.clone(), ty);

            instrs.push(TackyInstruction::Copy {
                src: TackyVal::Constant(make_const(c)),
                dst: TackyVal::Var(tmp_c.clone()),
            });
            instrs.push(TackyInstruction::Binary {
                op: TackyBinaryOp::Add,
                left: TackyVal::Var(tmp_mul),
                right: TackyVal::Var(tmp_c),
                dst: dst.clone(),
            });
        }
    }

    instrs
}

/// 値を `a * b + c` に分解する。a, b は小さめの因数。
fn decompose_value(value: i64) -> (i64, i64, i64) {
    let factors = [7, 5, 3, 11, 13, 6, 9];
    for &f in &factors {
        if value % f == 0 && value / f != 1 && value / f != 0 {
            return (f, value / f, 0);
        }
    }
    // 割り切れない場合: value = f * (value / f) + (value % f)
    let f = 7i64;
    let q = value / f;
    let r = value - f * q; // use value - f*q to handle negative values correctly
    if q != 0 {
        (f, q, r)
    } else {
        // 非常に小さな値（-6..6）: 3 * 1 + (value - 3) など
        (3, 1, value - 3)
    }
}

// ─────────────────────────────────────────────────────────────
// Pass 2: Arithmetic Substitution（算術置換）
// ─────────────────────────────────────────────────────────────

/// Add/Subtract を数学的に等価な多段計算に置換する。
/// デコンパイラでの式復元を困難にする。
///
/// - Add → パターン0（アフィン変換）or パターン1（係数展開）をローテーション
/// - Subtract → パターン2（アフィン変換）or パターン3（係数展開）をローテーション
/// - Multiply, Divide, Double系 → スキップ（オーバーフロー・精度問題）
/// - `obf_tmp.*` 変数への操作 → スキップ（定数間接化との無限展開防止）
fn arithmetic_substitution(
    instrs: Vec<TackyInstruction>,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
    freq: usize,
) -> Vec<TackyInstruction> {
    let mut result = Vec::new();
    let mut candidate_count = 0;

    for instr in instrs {
        match &instr {
            TackyInstruction::Binary { op, left, right, dst } => {
                // obf_tmp.* 変数への操作はスキップ（カスケード防止）
                let dst_is_obf = if let TackyVal::Var(name) = dst {
                    name.starts_with("obf_tmp.")
                } else {
                    false
                };

                if !dst_is_obf {
                    match op {
                        TackyBinaryOp::Add => {
                            candidate_count += 1;
                            if candidate_count % freq == 0 {
                                // パターン0/1 をローテーション
                                let pattern = ctx.label_counter % 2;
                                ctx.label_counter += 1;
                                match pattern {
                                    0 => result.extend(arith_add_affine(left, right, dst, ctx, var_types)),
                                    _ => result.extend(arith_add_coeff(left, right, dst, ctx, var_types)),
                                }
                                continue;
                            }
                        }
                        TackyBinaryOp::Subtract => {
                            candidate_count += 1;
                            if candidate_count % freq == 0 {
                                // パターン2/3 をローテーション
                                let pattern = ctx.label_counter % 2;
                                ctx.label_counter += 1;
                                match pattern {
                                    0 => result.extend(arith_sub_affine(left, right, dst, ctx, var_types)),
                                    _ => result.extend(arith_sub_coeff(left, right, dst, ctx, var_types)),
                                }
                                continue;
                            }
                        }
                        // Multiply, Divide, Double系はスキップ
                        _ => {}
                    }
                }

                result.push(instr);
            }
            _ => result.push(instr),
        }
    }

    result
}

/// dst の型を var_types から取得する。見つからなければ Int を返す。
fn get_dst_type(dst: &TackyVal, var_types: &std::collections::HashMap<String, Type>) -> Type {
    if let TackyVal::Var(name) = dst {
        var_types.get(name).cloned().unwrap_or(Type::Int)
    } else {
        Type::Int
    }
}

/// パターン0 — アフィン変換（Add）:
/// `dst = a + b` → `tmp1 = a + K; tmp2 = b - K; dst = tmp1 + tmp2`
fn arith_add_affine(
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
) -> Vec<TackyInstruction> {
    let ty = get_dst_type(dst, var_types);
    let k = ((ctx.label_counter as i64).wrapping_mul(0x9E37) ^ 0x1F2D) & 0x7FFF;
    let k_const = make_typed_const(&ty, k);
    let k_const2 = make_typed_const(&ty, k);

    let tmp1 = ctx.fresh_tmp();
    let tmp2 = ctx.fresh_tmp();
    var_types.insert(tmp1.clone(), ty.clone());
    var_types.insert(tmp2.clone(), ty);

    vec![
        // tmp1 = a + K
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: left.clone(),
            right: TackyVal::Constant(k_const),
            dst: TackyVal::Var(tmp1.clone()),
        },
        // tmp2 = b - K
        TackyInstruction::Binary {
            op: TackyBinaryOp::Subtract,
            left: right.clone(),
            right: TackyVal::Constant(k_const2),
            dst: TackyVal::Var(tmp2.clone()),
        },
        // dst = tmp1 + tmp2
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(tmp1),
            right: TackyVal::Var(tmp2),
            dst: dst.clone(),
        },
    ]
}

/// パターン1 — 係数展開（Add）:
/// `dst = a + b` → `dst = 3(a+b) - 2a - 2b = a + b`
fn arith_add_coeff(
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
) -> Vec<TackyInstruction> {
    let ty = get_dst_type(dst, var_types);
    let three = make_typed_const(&ty, 3);
    let three2 = make_typed_const(&ty, 3);
    let two = make_typed_const(&ty, 2);
    let two2 = make_typed_const(&ty, 2);

    let tmp1 = ctx.fresh_tmp(); // a * 3
    let tmp2 = ctx.fresh_tmp(); // b * 3
    let tmp3 = ctx.fresh_tmp(); // 3a + 3b
    let tmp4 = ctx.fresh_tmp(); // a * 2
    let tmp5 = ctx.fresh_tmp(); // b * 2
    let tmp6 = ctx.fresh_tmp(); // 2a + 2b
    for t in [&tmp1, &tmp2, &tmp3, &tmp4, &tmp5, &tmp6] {
        var_types.insert(t.clone(), ty.clone());
    }

    vec![
        TackyInstruction::Binary {
            op: TackyBinaryOp::Multiply,
            left: left.clone(),
            right: TackyVal::Constant(three),
            dst: TackyVal::Var(tmp1.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Multiply,
            left: right.clone(),
            right: TackyVal::Constant(three2),
            dst: TackyVal::Var(tmp2.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(tmp1),
            right: TackyVal::Var(tmp2),
            dst: TackyVal::Var(tmp3.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Multiply,
            left: left.clone(),
            right: TackyVal::Constant(two),
            dst: TackyVal::Var(tmp4.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Multiply,
            left: right.clone(),
            right: TackyVal::Constant(two2),
            dst: TackyVal::Var(tmp5.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(tmp4),
            right: TackyVal::Var(tmp5),
            dst: TackyVal::Var(tmp6.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Subtract,
            left: TackyVal::Var(tmp3),
            right: TackyVal::Var(tmp6),
            dst: dst.clone(),
        },
    ]
}

/// パターン2 — アフィン変換（Subtract）:
/// `dst = a - b` → `tmp1 = a + K; tmp2 = b + K; dst = tmp1 - tmp2`
fn arith_sub_affine(
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
) -> Vec<TackyInstruction> {
    let ty = get_dst_type(dst, var_types);
    let k = ((ctx.label_counter as i64).wrapping_mul(0xA3B7) ^ 0x2E4C) & 0x7FFF;
    let k_const = make_typed_const(&ty, k);
    let k_const2 = make_typed_const(&ty, k);

    let tmp1 = ctx.fresh_tmp();
    let tmp2 = ctx.fresh_tmp();
    var_types.insert(tmp1.clone(), ty.clone());
    var_types.insert(tmp2.clone(), ty);

    vec![
        // tmp1 = a + K
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: left.clone(),
            right: TackyVal::Constant(k_const),
            dst: TackyVal::Var(tmp1.clone()),
        },
        // tmp2 = b + K
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: right.clone(),
            right: TackyVal::Constant(k_const2),
            dst: TackyVal::Var(tmp2.clone()),
        },
        // dst = tmp1 - tmp2
        TackyInstruction::Binary {
            op: TackyBinaryOp::Subtract,
            left: TackyVal::Var(tmp1),
            right: TackyVal::Var(tmp2),
            dst: dst.clone(),
        },
    ]
}

/// パターン3 — 係数展開（Subtract）:
/// `dst = a - b` → `dst = 3a - 3b - (2a - 2b)`
fn arith_sub_coeff(
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
) -> Vec<TackyInstruction> {
    let ty = get_dst_type(dst, var_types);
    let three = make_typed_const(&ty, 3);
    let three2 = make_typed_const(&ty, 3);
    let two = make_typed_const(&ty, 2);
    let two2 = make_typed_const(&ty, 2);

    let tmp1 = ctx.fresh_tmp(); // a * 3
    let tmp2 = ctx.fresh_tmp(); // b * 3
    let tmp3 = ctx.fresh_tmp(); // 3a - 3b
    let tmp4 = ctx.fresh_tmp(); // a * 2
    let tmp5 = ctx.fresh_tmp(); // b * 2
    let tmp6 = ctx.fresh_tmp(); // 2a - 2b
    for t in [&tmp1, &tmp2, &tmp3, &tmp4, &tmp5, &tmp6] {
        var_types.insert(t.clone(), ty.clone());
    }

    vec![
        TackyInstruction::Binary {
            op: TackyBinaryOp::Multiply,
            left: left.clone(),
            right: TackyVal::Constant(three),
            dst: TackyVal::Var(tmp1.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Multiply,
            left: right.clone(),
            right: TackyVal::Constant(three2),
            dst: TackyVal::Var(tmp2.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Subtract,
            left: TackyVal::Var(tmp1),
            right: TackyVal::Var(tmp2),
            dst: TackyVal::Var(tmp3.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Multiply,
            left: left.clone(),
            right: TackyVal::Constant(two),
            dst: TackyVal::Var(tmp4.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Multiply,
            left: right.clone(),
            right: TackyVal::Constant(two2),
            dst: TackyVal::Var(tmp5.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Subtract,
            left: TackyVal::Var(tmp4),
            right: TackyVal::Var(tmp5),
            dst: TackyVal::Var(tmp6.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Subtract,
            left: TackyVal::Var(tmp3),
            right: TackyVal::Var(tmp6),
            dst: dst.clone(),
        },
    ]
}

/// 型に応じた定数を生成する。
fn make_typed_const(ty: &Type, value: i64) -> TackyConst {
    match ty {
        Type::Int => TackyConst::Int(value as i32),
        Type::Long => TackyConst::Long(value),
        Type::UInt => TackyConst::UInt(value as u32),
        Type::ULong => TackyConst::ULong(value as u64),
        Type::Char => TackyConst::Char(value as i8),
        Type::UChar => TackyConst::UChar(value as u8),
        _ => TackyConst::Int(value as i32),
    }
}

// ─────────────────────────────────────────────────────────────
// Pass 3: Junk Code Insertion（ジャンクコード挿入）
// ─────────────────────────────────────────────────────────────

/// N命令ごとに dead computation（結果が使われない計算）を挿入する。
/// Label の直前には挿入しない。
fn junk_code_insertion(
    instrs: Vec<TackyInstruction>,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
    freq: usize,
) -> Vec<TackyInstruction> {
    let mut result = Vec::new();
    let mut count = 0;

    for (i, instr) in instrs.iter().enumerate() {
        // N命令ごとにジャンクコードを挿入（ただし Label の直前は避ける）
        if count > 0 && count % freq == 0 {
            let next_is_label = instrs.get(i).map_or(false, |next| matches!(next, TackyInstruction::Label(_)));
            if !next_is_label {
                result.extend(generate_junk(ctx, var_types));
            }
        }

        result.push(instr.clone());
        count += 1;
    }

    result
}

/// ジャンクコード（dead computation）を3命令生成する。
fn generate_junk(
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
) -> Vec<TackyInstruction> {
    let tmp_x = ctx.fresh_tmp();
    let tmp_y = ctx.fresh_tmp();
    let tmp_z = ctx.fresh_tmp();
    var_types.insert(tmp_x.clone(), Type::Int);
    var_types.insert(tmp_y.clone(), Type::Int);
    var_types.insert(tmp_z.clone(), Type::Int);

    vec![
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Int(0x1234)),
            dst: TackyVal::Var(tmp_x.clone()),
        },
        TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Int(0x5678)),
            dst: TackyVal::Var(tmp_y.clone()),
        },
        TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(tmp_x),
            right: TackyVal::Var(tmp_y),
            dst: TackyVal::Var(tmp_z),
        },
    ]
}

// ─────────────────────────────────────────────────────────────
// Pass 4: Opaque Predicates（不透明述語）
// ─────────────────────────────────────────────────────────────

/// N回に1回、値生成命令を常に真の条件分岐で囲む。
/// `x * (x + 1) % 2 == 0` は任意の整数 x で常に真（連続整数の積は偶数）。
fn opaque_predicates(
    instrs: Vec<TackyInstruction>,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
    freq: usize,
) -> Vec<TackyInstruction> {
    let mut result = Vec::new();
    let mut candidate_count = 0;

    for instr in instrs {
        if is_value_producing(&instr) {
            candidate_count += 1;
            if candidate_count % freq == 0 {
                result.extend(wrap_with_opaque_predicate(instr, ctx, var_types));
                continue;
            }
        }
        result.push(instr);
    }

    result
}

/// 値を生成する命令かどうか判定する。
/// 副作用のある命令（FunCall, Store, Return）や制御フロー命令（Jump, Label）は除外。
fn is_value_producing(instr: &TackyInstruction) -> bool {
    matches!(instr,
        TackyInstruction::Copy { .. }
        | TackyInstruction::Unary { .. }
        | TackyInstruction::Binary { .. }
        | TackyInstruction::SignExtend { .. }
        | TackyInstruction::ZeroExtend { .. }
        | TackyInstruction::Truncate { .. }
    )
}

/// 不透明述語で命令を囲む（Feature 5: 多様化パターン）。
///
/// カウンタの mod 4 で使用するパターンを選択し、パターンマッチによる自動除去を防ぐ。
///
/// ```text
/// <predicate computation>   // pred == 0（常に真）
/// JumpIfZero(pred, .Lobf_real)
/// <偽コード>
/// Jump(.Lobf_end)
/// .Lobf_real:
/// <本物の命令>
/// .Lobf_end:
/// ```
fn wrap_with_opaque_predicate(
    real_instr: TackyInstruction,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
) -> Vec<TackyInstruction> {
    let mut instrs = Vec::new();

    let label_real = ctx.fresh_label();
    let label_end = ctx.fresh_label();

    // パターン選択（label_counter をローテーション）
    let pattern = ctx.label_counter % 4;

    let pred_var = match pattern {
        0 => generate_predicate_0(ctx, var_types, &mut instrs),
        1 => generate_predicate_1(ctx, var_types, &mut instrs),
        2 => generate_predicate_2(ctx, var_types, &mut instrs),
        3 => generate_predicate_3(ctx, var_types, &mut instrs),
        _ => unreachable!(),
    };

    // if pred == 0 goto real (always taken — all predicates produce 0)
    instrs.push(TackyInstruction::JumpIfZero {
        condition: TackyVal::Var(pred_var),
        target: label_real.clone(),
    });

    // 偽コード（到達不能）— ジャンク代入
    let tmp_fake = ctx.fresh_tmp();
    var_types.insert(tmp_fake.clone(), Type::Int);
    instrs.push(TackyInstruction::Copy {
        src: TackyVal::Constant(TackyConst::Int(0xDEAD)),
        dst: TackyVal::Var(tmp_fake),
    });
    instrs.push(TackyInstruction::Jump(label_end.clone()));

    // .Lobf_real:
    instrs.push(TackyInstruction::Label(label_real));

    // 本物の命令
    instrs.push(real_instr);

    // .Lobf_end:
    instrs.push(TackyInstruction::Label(label_end));

    instrs
}

/// パターン 0: `x*(x+1) % 2 == 0`（連続整数の積は偶数）
fn generate_predicate_0(
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
    instrs: &mut Vec<TackyInstruction>,
) -> String {
    let tmp_x = ctx.fresh_tmp();
    let tmp_x_plus_1 = ctx.fresh_tmp();
    let tmp_prod = ctx.fresh_tmp();
    let tmp_pred = ctx.fresh_tmp();
    var_types.insert(tmp_x.clone(), Type::Int);
    var_types.insert(tmp_x_plus_1.clone(), Type::Int);
    var_types.insert(tmp_prod.clone(), Type::Int);
    var_types.insert(tmp_pred.clone(), Type::Int);

    instrs.push(TackyInstruction::Copy {
        src: TackyVal::Constant(TackyConst::Int(42)),
        dst: TackyVal::Var(tmp_x.clone()),
    });
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Add,
        left: TackyVal::Var(tmp_x.clone()),
        right: TackyVal::Constant(TackyConst::Int(1)),
        dst: TackyVal::Var(tmp_x_plus_1.clone()),
    });
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Multiply,
        left: TackyVal::Var(tmp_x),
        right: TackyVal::Var(tmp_x_plus_1),
        dst: TackyVal::Var(tmp_prod.clone()),
    });
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Remainder,
        left: TackyVal::Var(tmp_prod),
        right: TackyVal::Constant(TackyConst::Int(2)),
        dst: TackyVal::Var(tmp_pred.clone()),
    });

    tmp_pred
}

/// パターン 1: `x*x + 1 > 0` を `!(x*x + 1 > 0)` で表現 → 常に 0
/// x²≥0 なので x²+1≥1 > 0 は常に真。`!(true)` = 0。
fn generate_predicate_1(
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
    instrs: &mut Vec<TackyInstruction>,
) -> String {
    let tmp_x = ctx.fresh_tmp();
    let tmp_sq = ctx.fresh_tmp();
    let tmp_sq_plus_1 = ctx.fresh_tmp();
    let tmp_gt = ctx.fresh_tmp();
    let tmp_pred = ctx.fresh_tmp();
    var_types.insert(tmp_x.clone(), Type::Int);
    var_types.insert(tmp_sq.clone(), Type::Int);
    var_types.insert(tmp_sq_plus_1.clone(), Type::Int);
    var_types.insert(tmp_gt.clone(), Type::Int);
    var_types.insert(tmp_pred.clone(), Type::Int);

    // x = 17
    instrs.push(TackyInstruction::Copy {
        src: TackyVal::Constant(TackyConst::Int(17)),
        dst: TackyVal::Var(tmp_x.clone()),
    });
    // sq = x * x
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Multiply,
        left: TackyVal::Var(tmp_x.clone()),
        right: TackyVal::Var(tmp_x),
        dst: TackyVal::Var(tmp_sq.clone()),
    });
    // sq_plus_1 = sq + 1
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Add,
        left: TackyVal::Var(tmp_sq),
        right: TackyVal::Constant(TackyConst::Int(1)),
        dst: TackyVal::Var(tmp_sq_plus_1.clone()),
    });
    // gt = sq_plus_1 > 0  (always 1)
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::GreaterThan,
        left: TackyVal::Var(tmp_sq_plus_1),
        right: TackyVal::Constant(TackyConst::Int(0)),
        dst: TackyVal::Var(tmp_gt.clone()),
    });
    // pred = !gt  (always 0)
    instrs.push(TackyInstruction::Unary {
        op: TackyUnaryOp::Not,
        src: TackyVal::Var(tmp_gt),
        dst: TackyVal::Var(tmp_pred.clone()),
    });

    tmp_pred
}

/// パターン 2: `(x+1)² - x² - 1 == 2*x` — 展開すると恒等式
/// `(x+1)² - x² - 1 - 2*x` = `x² + 2x + 1 - x² - 1 - 2x` = 0
fn generate_predicate_2(
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
    instrs: &mut Vec<TackyInstruction>,
) -> String {
    let tmp_x = ctx.fresh_tmp();
    let tmp_x1 = ctx.fresh_tmp();
    let tmp_x1_sq = ctx.fresh_tmp();
    let tmp_x_sq = ctx.fresh_tmp();
    let tmp_sub1 = ctx.fresh_tmp();
    let tmp_sub2 = ctx.fresh_tmp();
    let tmp_2x = ctx.fresh_tmp();
    let tmp_pred = ctx.fresh_tmp();
    var_types.insert(tmp_x.clone(), Type::Int);
    var_types.insert(tmp_x1.clone(), Type::Int);
    var_types.insert(tmp_x1_sq.clone(), Type::Int);
    var_types.insert(tmp_x_sq.clone(), Type::Int);
    var_types.insert(tmp_sub1.clone(), Type::Int);
    var_types.insert(tmp_sub2.clone(), Type::Int);
    var_types.insert(tmp_2x.clone(), Type::Int);
    var_types.insert(tmp_pred.clone(), Type::Int);

    // x = 13
    instrs.push(TackyInstruction::Copy {
        src: TackyVal::Constant(TackyConst::Int(13)),
        dst: TackyVal::Var(tmp_x.clone()),
    });
    // x1 = x + 1
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Add,
        left: TackyVal::Var(tmp_x.clone()),
        right: TackyVal::Constant(TackyConst::Int(1)),
        dst: TackyVal::Var(tmp_x1.clone()),
    });
    // x1_sq = x1 * x1
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Multiply,
        left: TackyVal::Var(tmp_x1.clone()),
        right: TackyVal::Var(tmp_x1),
        dst: TackyVal::Var(tmp_x1_sq.clone()),
    });
    // x_sq = x * x
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Multiply,
        left: TackyVal::Var(tmp_x.clone()),
        right: TackyVal::Var(tmp_x.clone()),
        dst: TackyVal::Var(tmp_x_sq.clone()),
    });
    // sub1 = x1_sq - x_sq
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Subtract,
        left: TackyVal::Var(tmp_x1_sq),
        right: TackyVal::Var(tmp_x_sq),
        dst: TackyVal::Var(tmp_sub1.clone()),
    });
    // sub2 = sub1 - 1
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Subtract,
        left: TackyVal::Var(tmp_sub1),
        right: TackyVal::Constant(TackyConst::Int(1)),
        dst: TackyVal::Var(tmp_sub2.clone()),
    });
    // 2x = 2 * x
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Multiply,
        left: TackyVal::Constant(TackyConst::Int(2)),
        right: TackyVal::Var(tmp_x),
        dst: TackyVal::Var(tmp_2x.clone()),
    });
    // pred = sub2 - 2x  (always 0)
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Subtract,
        left: TackyVal::Var(tmp_sub2),
        right: TackyVal::Var(tmp_2x),
        dst: TackyVal::Var(tmp_pred.clone()),
    });

    tmp_pred
}

/// パターン 3: `(x³ - x) % 3 == 0`（連続3整数の積は3の倍数）
/// x*(x-1)*(x+1) = x³ - x は 3 の倍数。
fn generate_predicate_3(
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
    instrs: &mut Vec<TackyInstruction>,
) -> String {
    let tmp_x = ctx.fresh_tmp();
    let tmp_x_sq = ctx.fresh_tmp();
    let tmp_x_cubed = ctx.fresh_tmp();
    let tmp_diff = ctx.fresh_tmp();
    let tmp_pred = ctx.fresh_tmp();
    var_types.insert(tmp_x.clone(), Type::Int);
    var_types.insert(tmp_x_sq.clone(), Type::Int);
    var_types.insert(tmp_x_cubed.clone(), Type::Int);
    var_types.insert(tmp_diff.clone(), Type::Int);
    var_types.insert(tmp_pred.clone(), Type::Int);

    // x = 7
    instrs.push(TackyInstruction::Copy {
        src: TackyVal::Constant(TackyConst::Int(7)),
        dst: TackyVal::Var(tmp_x.clone()),
    });
    // x_sq = x * x
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Multiply,
        left: TackyVal::Var(tmp_x.clone()),
        right: TackyVal::Var(tmp_x.clone()),
        dst: TackyVal::Var(tmp_x_sq.clone()),
    });
    // x_cubed = x_sq * x
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Multiply,
        left: TackyVal::Var(tmp_x_sq),
        right: TackyVal::Var(tmp_x.clone()),
        dst: TackyVal::Var(tmp_x_cubed.clone()),
    });
    // diff = x_cubed - x
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Subtract,
        left: TackyVal::Var(tmp_x_cubed),
        right: TackyVal::Var(tmp_x),
        dst: TackyVal::Var(tmp_diff.clone()),
    });
    // pred = diff % 3  (always 0)
    instrs.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Remainder,
        left: TackyVal::Var(tmp_diff),
        right: TackyVal::Constant(TackyConst::Int(3)),
        dst: TackyVal::Var(tmp_pred.clone()),
    });

    tmp_pred
}

// ─────────────────────────────────────────────────────────────
// Pass 14: VM Virtualization（VM仮想化 — コード仮想化）
// ─────────────────────────────────────────────────────────────

/// VM仮想化の適格性を判定する。
///
/// 以下の条件をすべて満たす関数が適格:
/// - `main` でない（文字列暗号化の復号コードとの干渉を回避）
/// - `Double` 型の変数がない
/// - 浮動小数点変換命令がない（IntToDouble, DoubleToInt, UIntToDouble, DoubleToUInt）
/// - 構造体操作命令がない（CopyToOffset, CopyFromOffset, CopyStruct）
/// - 本体が 2 命令以上
fn is_vm_eligible(func: &TackyFunction) -> bool {
    if func.name == "main" {
        return false;
    }
    if func.body.len() < 2 {
        return false;
    }
    if func.var_types.values().any(|t| matches!(t, Type::Double)) {
        return false;
    }
    for instr in &func.body {
        match instr {
            TackyInstruction::IntToDouble { .. }
            | TackyInstruction::DoubleToInt { .. }
            | TackyInstruction::UIntToDouble { .. }
            | TackyInstruction::DoubleToUInt { .. }
            | TackyInstruction::CopyToOffset { .. }
            | TackyInstruction::CopyFromOffset { .. }
            | TackyInstruction::CopyStruct { .. } => return false,
            _ => {}
        }
    }
    true
}

/// 適格な関数をバイトコード＋VMインタプリタに変換する。
///
/// 各TACKY命令を個別のハンドラに配置し、バイトコード配列とハンドラテーブルを
/// `.data` セクションに配置する。ディスパッチループが `bytecode[PC]` をフェッチし
/// ハンドラテーブルから間接ジャンプすることで元の命令列を実行する。
///
/// 元のTACKY変数・型はそのまま保持し、命令単位の細粒度ディスパッチにより
/// 静的解析でのCFG復元を極めて困難にする。
fn vm_virtualize(program: &mut TackyProgram, ctx: &mut ObfCtx) {
    let func_count = program.functions.len();
    for fi in 0..func_count {
        if !is_vm_eligible(&program.functions[fi]) {
            continue;
        }

        let func = &program.functions[fi];
        let original_body = func.body.clone();
        let n = original_body.len();
        if n == 0 {
            continue;
        }

        // Step 1: ラベル → PC マッピング構築
        let mut label_to_pc: HashMap<String, usize> = HashMap::new();
        for (i, instr) in original_body.iter().enumerate() {
            if let TackyInstruction::Label(name) = instr {
                label_to_pc.insert(name.clone(), i);
            }
        }

        // Step 2: ハンドララベル生成
        let dispatch_label = ctx.fresh_label();
        let handler_labels: Vec<String> = (0..n).map(|_| ctx.fresh_label()).collect();

        // Step 3: バイトコード配列（ByteArrayInit）
        // 各命令のハンドラインデックスを u32 LE で格納（初期状態: 命令 i → ハンドラ i）
        let mut bc_bytes: Vec<u8> = Vec::new();
        for i in 0..n {
            bc_bytes.extend_from_slice(&(i as u32).to_le_bytes());
        }
        let bc_name = format!(".Lobf_vm_bc_{}", ctx.vm_counter);
        program.static_vars.push(TackyStaticVar {
            name: bc_name.clone(),
            global: false,
            var_type: Type::Array(Box::new(Type::UChar), bc_bytes.len()),
            init: TackyStaticInit::ByteArrayInit(bc_bytes),
        });

        // Step 4: ハンドラテーブル（PointerArrayInit）
        let jt_name = format!(".Lobf_vm_jt_{}", ctx.vm_counter);
        program.static_vars.push(TackyStaticVar {
            name: jt_name.clone(),
            global: false,
            var_type: Type::Array(Box::new(Type::Long), n),
            init: TackyStaticInit::PointerArrayInit(handler_labels.clone()),
        });

        ctx.vm_counter += 1;

        // Step 5: 新しい関数本体を生成
        let var_types = &mut program.functions[fi].var_types;
        let mut new_body: Vec<TackyInstruction> = Vec::new();

        // VM ローカル変数を登録
        // NOTE: pc_var は Long（64ビット）にする。AddPtr の codegen が index を
        // 常に Quadword (movq) で読み込むため、Int (32ビット) だとスタック上の
        // 隣接データをゴミとして読み込んでしまう。
        let pc_var = ctx.fresh_tmp();
        let bc_ptr_var = ctx.fresh_tmp();
        let jt_ptr_var = ctx.fresh_tmp();
        var_types.insert(pc_var.clone(), Type::Long);
        var_types.insert(bc_ptr_var.clone(), Type::Pointer(Box::new(Type::UChar)));
        var_types.insert(jt_ptr_var.clone(), Type::Pointer(Box::new(Type::Long)));

        // ── 初期化 ──
        // _vm_pc = 0
        new_body.push(TackyInstruction::Copy {
            src: TackyVal::Constant(TackyConst::Long(0)),
            dst: TackyVal::Var(pc_var.clone()),
        });
        // _vm_bc_ptr = &bytecode_data
        new_body.push(TackyInstruction::GetAddress {
            src: TackyVal::Var(bc_name),
            dst: TackyVal::Var(bc_ptr_var.clone()),
        });
        // _vm_jt_ptr = &handler_table
        new_body.push(TackyInstruction::GetAddress {
            src: TackyVal::Var(jt_name),
            dst: TackyVal::Var(jt_ptr_var.clone()),
        });
        // Jump(dispatch)
        new_body.push(TackyInstruction::Jump(dispatch_label.clone()));

        // ── ディスパッチループ ──
        new_body.push(TackyInstruction::Label(dispatch_label.clone()));

        // ディスパッチ用一時変数
        let fetch_ptr_var = ctx.fresh_tmp();
        let handler_idx_var = ctx.fresh_tmp();
        let handler_addr_ptr_var = ctx.fresh_tmp();
        let handler_addr_var = ctx.fresh_tmp();
        var_types.insert(fetch_ptr_var.clone(), Type::Pointer(Box::new(Type::Int)));
        var_types.insert(handler_idx_var.clone(), Type::Int);
        var_types.insert(handler_addr_ptr_var.clone(), Type::Pointer(Box::new(Type::Long)));
        var_types.insert(handler_addr_var.clone(), Type::Long);

        // fetch_ptr = AddPtr(bc_ptr, pc, scale=4)  — bytecode[pc] のアドレス
        new_body.push(TackyInstruction::AddPtr {
            ptr: TackyVal::Var(bc_ptr_var.clone()),
            index: TackyVal::Var(pc_var.clone()),
            scale: 4,
            dst: TackyVal::Var(fetch_ptr_var.clone()),
        });

        // handler_idx = Load(fetch_ptr)  — u32 ハンドラインデックス
        new_body.push(TackyInstruction::Load {
            src_ptr: TackyVal::Var(fetch_ptr_var),
            dst: TackyVal::Var(handler_idx_var.clone()),
        });

        // pc = pc + 1
        new_body.push(TackyInstruction::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Var(pc_var.clone()),
            right: TackyVal::Constant(TackyConst::Long(1)),
            dst: TackyVal::Var(pc_var.clone()),
        });

        // handler_idx は Int (32ビット) なので AddPtr の前に Long に拡張する
        let handler_idx_long_var = ctx.fresh_tmp();
        var_types.insert(handler_idx_long_var.clone(), Type::Long);
        new_body.push(TackyInstruction::SignExtend {
            src: TackyVal::Var(handler_idx_var),
            dst: TackyVal::Var(handler_idx_long_var.clone()),
        });

        // handler_addr_ptr = AddPtr(jt_ptr, handler_idx_long, scale=8)
        new_body.push(TackyInstruction::AddPtr {
            ptr: TackyVal::Var(jt_ptr_var.clone()),
            index: TackyVal::Var(handler_idx_long_var),
            scale: 8,
            dst: TackyVal::Var(handler_addr_ptr_var.clone()),
        });

        // handler_addr = Load(handler_addr_ptr)
        new_body.push(TackyInstruction::Load {
            src_ptr: TackyVal::Var(handler_addr_ptr_var),
            dst: TackyVal::Var(handler_addr_var.clone()),
        });

        // JumpIndirect(handler_addr, all_handler_labels)
        new_body.push(TackyInstruction::JumpIndirect {
            target: TackyVal::Var(handler_addr_var),
            possible_targets: handler_labels.clone(),
        });

        // ── ハンドラ群（元の各命令に対応） ──
        for (i, instr) in original_body.iter().enumerate() {
            new_body.push(TackyInstruction::Label(handler_labels[i].clone()));

            match instr {
                // Label: ノーオペ → dispatch に戻る
                TackyInstruction::Label(_) => {
                    new_body.push(TackyInstruction::Jump(dispatch_label.clone()));
                }

                // Jump(target): PC を即値で設定 → dispatch に戻る
                TackyInstruction::Jump(target) => {
                    if let Some(&target_pc) = label_to_pc.get(target) {
                        new_body.push(TackyInstruction::Copy {
                            src: TackyVal::Constant(TackyConst::Long(target_pc as i64)),
                            dst: TackyVal::Var(pc_var.clone()),
                        });
                    }
                    new_body.push(TackyInstruction::Jump(dispatch_label.clone()));
                }

                // JumpIfZero { condition, target }:
                // condition==0 で target_pc に設定、非ゼロなら PC そのまま
                TackyInstruction::JumpIfZero { condition, target } => {
                    if let Some(&target_pc) = label_to_pc.get(target) {
                        let skip = ctx.fresh_label();
                        new_body.push(TackyInstruction::JumpIfNotZero {
                            condition: condition.clone(),
                            target: skip.clone(),
                        });
                        // ゼロ → PC を target_pc に設定
                        new_body.push(TackyInstruction::Copy {
                            src: TackyVal::Constant(TackyConst::Long(target_pc as i64)),
                            dst: TackyVal::Var(pc_var.clone()),
                        });
                        new_body.push(TackyInstruction::Label(skip));
                    }
                    new_body.push(TackyInstruction::Jump(dispatch_label.clone()));
                }

                // JumpIfNotZero { condition, target }:
                // condition!=0 で target_pc に設定、ゼロなら PC そのまま
                TackyInstruction::JumpIfNotZero { condition, target } => {
                    if let Some(&target_pc) = label_to_pc.get(target) {
                        let skip = ctx.fresh_label();
                        new_body.push(TackyInstruction::JumpIfZero {
                            condition: condition.clone(),
                            target: skip.clone(),
                        });
                        // 非ゼロ → PC を target_pc に設定
                        new_body.push(TackyInstruction::Copy {
                            src: TackyVal::Constant(TackyConst::Long(target_pc as i64)),
                            dst: TackyVal::Var(pc_var.clone()),
                        });
                        new_body.push(TackyInstruction::Label(skip));
                    }
                    new_body.push(TackyInstruction::Jump(dispatch_label.clone()));
                }

                // Return / ReturnVoid: そのまま出力（dispatch に戻らない）
                TackyInstruction::Return(_) | TackyInstruction::ReturnVoid => {
                    new_body.push(instr.clone());
                }

                // その他全命令: そのまま出力 + dispatch に戻る
                _ => {
                    new_body.push(instr.clone());
                    new_body.push(TackyInstruction::Jump(dispatch_label.clone()));
                }
            }
        }

        program.functions[fi].body = new_body;
    }
}

// ─────────────────────────────────────────────────────────────
// Pass 5: Control Flow Flattening（制御フロー平坦化）
// ─────────────────────────────────────────────────────────────

/// 関数本体を基本ブロックに分割し、ジャンプテーブル + 状態エンコードの dispatch ループに変換する。
///
/// Feature 1: ジャンプテーブルによる間接ジャンプ（IDA の CFG 復元を破壊）
/// Feature 4: 状態変数の算術エンコード（ステートマシン復元を妨害）
///
/// ```text
/// obf_state = 0 * A + B     // encoded initial state
/// .Lobf_dispatch:
///   decoded = (obf_state - B) / A
///   ptr = jt_base + decoded * 8
///   JumpIndirect(Load(ptr))
/// block_0: <元のコード> obf_state = next_encoded; goto dispatch
/// block_1: ...
/// ```
fn control_flow_flattening(
    instrs: Vec<TackyInstruction>,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
    static_vars: &mut Vec<TackyStaticVar>,
    cff_a: i32,
    cff_b: i32,
) -> Vec<TackyInstruction> {
    if instrs.is_empty() {
        return instrs;
    }

    // 基本ブロックに分割
    let blocks = split_into_blocks(&instrs);

    // 単一ブロック関数はスキップ
    if blocks.len() <= 1 {
        return instrs;
    }

    // ラベル → ブロックインデックスのマッピングを構築
    let mut label_to_block: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, block) in blocks.iter().enumerate() {
        if let Some(TackyInstruction::Label(label)) = block.first() {
            label_to_block.insert(label.clone(), i);
        }
    }

    let state_var = ctx.fresh_tmp();
    var_types.insert(state_var.clone(), Type::Int);

    let dispatch_label = ctx.fresh_label();
    let exit_label = ctx.fresh_label();

    let mut result = Vec::new();

    // 各ブロック用のラベルを生成
    let block_labels: Vec<String> = (0..blocks.len())
        .map(|_| ctx.fresh_label())
        .collect();

    // ── ジャンプテーブルを静的変数として登録（Feature 1）──
    let jt_name = format!(".Lobf_jt_{}", ctx.label_counter);
    ctx.label_counter += 1;

    static_vars.push(TackyStaticVar {
        name: jt_name.clone(),
        global: false,
        var_type: Type::Array(Box::new(Type::Long), blocks.len()),
        init: TackyStaticInit::PointerArrayInit(block_labels.clone()),
    });

    // ── エンコード関数: index → index * A + B ──
    let encode = |index: usize| -> i32 {
        (index as i32).wrapping_mul(cff_a).wrapping_add(cff_b)
    };

    // obf_state = encode(0) = 0 * A + B = B
    result.push(TackyInstruction::Copy {
        src: TackyVal::Constant(TackyConst::Int(encode(0))),
        dst: TackyVal::Var(state_var.clone()),
    });

    // goto dispatch
    result.push(TackyInstruction::Jump(dispatch_label.clone()));

    // ── Dispatch: デコード + ジャンプテーブル間接ジャンプ ──
    result.push(TackyInstruction::Label(dispatch_label.clone()));

    // decoded = (state - B) / A
    let tmp_sub = ctx.fresh_tmp();
    let decoded_index = ctx.fresh_tmp();
    var_types.insert(tmp_sub.clone(), Type::Int);
    var_types.insert(decoded_index.clone(), Type::Int);

    result.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Subtract,
        left: TackyVal::Var(state_var.clone()),
        right: TackyVal::Constant(TackyConst::Int(cff_b)),
        dst: TackyVal::Var(tmp_sub.clone()),
    });
    result.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Divide,
        left: TackyVal::Var(tmp_sub),
        right: TackyVal::Constant(TackyConst::Int(cff_a)),
        dst: TackyVal::Var(decoded_index.clone()),
    });

    // base = &jump_table
    let jt_base = ctx.fresh_tmp();
    var_types.insert(jt_base.clone(), Type::Pointer(Box::new(Type::Long)));

    result.push(TackyInstruction::GetAddress {
        src: TackyVal::Var(jt_name),
        dst: TackyVal::Var(jt_base.clone()),
    });

    // ptr = base + decoded_index * 8
    let jt_ptr = ctx.fresh_tmp();
    var_types.insert(jt_ptr.clone(), Type::Pointer(Box::new(Type::Long)));

    result.push(TackyInstruction::AddPtr {
        ptr: TackyVal::Var(jt_base),
        index: TackyVal::Var(decoded_index),
        scale: 8,
        dst: TackyVal::Var(jt_ptr.clone()),
    });

    // addr = *ptr
    let jt_addr = ctx.fresh_tmp();
    var_types.insert(jt_addr.clone(), Type::Long);

    result.push(TackyInstruction::Load {
        src_ptr: TackyVal::Var(jt_ptr),
        dst: TackyVal::Var(jt_addr.clone()),
    });

    // JumpIndirect(addr) — possible_targets で生存解析に正しい CFG 後続を通知
    result.push(TackyInstruction::JumpIndirect {
        target: TackyVal::Var(jt_addr),
        possible_targets: block_labels.clone(),
    });

    // ── Block bodies ──
    for (i, block) in blocks.iter().enumerate() {
        result.push(TackyInstruction::Label(block_labels[i].clone()));

        // ブロック内の命令を出力
        for instr in block {
            match instr {
                // 元のラベルは保持（CFF ブロックラベルに加えて残す。
                // VM仮想化のハンドラテーブル等 .data セクションから参照される可能性がある）
                TackyInstruction::Label(_) => {
                    result.push(instr.clone());
                }

                // Return はそのまま出力（関数から直接脱出）
                TackyInstruction::Return(_) | TackyInstruction::ReturnVoid => {
                    result.push(instr.clone());
                }

                // Jump → encoded state 設定 + dispatch へ戻る
                TackyInstruction::Jump(target) => {
                    if let Some(&target_block) = label_to_block.get(target) {
                        result.push(TackyInstruction::Copy {
                            src: TackyVal::Constant(TackyConst::Int(encode(target_block))),
                            dst: TackyVal::Var(state_var.clone()),
                        });
                        result.push(TackyInstruction::Jump(dispatch_label.clone()));
                    } else {
                        // ターゲットが見つからない場合はそのまま
                        result.push(instr.clone());
                    }
                }

                // JumpIfZero → 条件付き encoded state 設定
                TackyInstruction::JumpIfZero { condition, target } => {
                    if let Some(&target_block) = label_to_block.get(target) {
                        let fallthrough_block = i + 1;
                        let tmp_is_zero = ctx.fresh_tmp();
                        var_types.insert(tmp_is_zero.clone(), Type::Int);

                        result.push(TackyInstruction::Binary {
                            op: TackyBinaryOp::Equal,
                            left: condition.clone(),
                            right: TackyVal::Constant(TackyConst::Int(0)),
                            dst: TackyVal::Var(tmp_is_zero.clone()),
                        });
                        result.push(TackyInstruction::JumpIfNotZero {
                            condition: TackyVal::Var(tmp_is_zero),
                            target: format!("{}_taken", block_labels[i]),
                        });

                        // Not taken: state = encode(fallthrough)
                        if fallthrough_block < blocks.len() {
                            result.push(TackyInstruction::Copy {
                                src: TackyVal::Constant(TackyConst::Int(encode(fallthrough_block))),
                                dst: TackyVal::Var(state_var.clone()),
                            });
                        }
                        result.push(TackyInstruction::Jump(dispatch_label.clone()));

                        // Taken: state = encode(target)
                        result.push(TackyInstruction::Label(format!("{}_taken", block_labels[i])));
                        result.push(TackyInstruction::Copy {
                            src: TackyVal::Constant(TackyConst::Int(encode(target_block))),
                            dst: TackyVal::Var(state_var.clone()),
                        });
                        result.push(TackyInstruction::Jump(dispatch_label.clone()));
                    } else {
                        result.push(instr.clone());
                    }
                }

                // JumpIfNotZero → 条件付き encoded state 設定
                TackyInstruction::JumpIfNotZero { condition, target } => {
                    if let Some(&target_block) = label_to_block.get(target) {
                        let fallthrough_block = i + 1;

                        result.push(TackyInstruction::JumpIfNotZero {
                            condition: condition.clone(),
                            target: format!("{}_taken", block_labels[i]),
                        });

                        // Not taken: state = encode(fallthrough)
                        if fallthrough_block < blocks.len() {
                            result.push(TackyInstruction::Copy {
                                src: TackyVal::Constant(TackyConst::Int(encode(fallthrough_block))),
                                dst: TackyVal::Var(state_var.clone()),
                            });
                        }
                        result.push(TackyInstruction::Jump(dispatch_label.clone()));

                        // Taken: state = encode(target)
                        result.push(TackyInstruction::Label(format!("{}_taken", block_labels[i])));
                        result.push(TackyInstruction::Copy {
                            src: TackyVal::Constant(TackyConst::Int(encode(target_block))),
                            dst: TackyVal::Var(state_var.clone()),
                        });
                        result.push(TackyInstruction::Jump(dispatch_label.clone()));
                    } else {
                        result.push(instr.clone());
                    }
                }

                // その他の命令はそのまま
                _ => {
                    result.push(instr.clone());
                }
            }
        }

        // ブロックの最後が制御フロー命令でない場合、次のブロックへフォールスルー
        let last = block.last();
        let is_terminator = last.map_or(false, |l| matches!(l,
            TackyInstruction::Jump(_) | TackyInstruction::JumpIfZero { .. }
            | TackyInstruction::JumpIfNotZero { .. }
            | TackyInstruction::Return(_) | TackyInstruction::ReturnVoid
        ));

        if !is_terminator {
            let next_block = i + 1;
            if next_block < blocks.len() {
                result.push(TackyInstruction::Copy {
                    src: TackyVal::Constant(TackyConst::Int(encode(next_block))),
                    dst: TackyVal::Var(state_var.clone()),
                });
                result.push(TackyInstruction::Jump(dispatch_label.clone()));
            }
        }
    }

    // ── Exit label ──
    result.push(TackyInstruction::Label(exit_label));

    result
}

/// 命令列を基本ブロックに分割する。
///
/// ブロック境界の定義:
/// - **ブロックの先頭**: 関数の先頭、ラベル命令、ジャンプ/リターンの直後
/// - **ブロックの末尾**: ジャンプ/リターン命令、次のラベルの直前
fn split_into_blocks(instrs: &[TackyInstruction]) -> Vec<Vec<TackyInstruction>> {
    if instrs.is_empty() {
        return vec![];
    }

    let mut blocks: Vec<Vec<TackyInstruction>> = Vec::new();
    let mut current_block: Vec<TackyInstruction> = Vec::new();

    for instr in instrs {
        match instr {
            TackyInstruction::Label(_) => {
                // ラベルは新しいブロックの先頭
                if !current_block.is_empty() {
                    blocks.push(std::mem::take(&mut current_block));
                }
                current_block.push(instr.clone());
            }
            TackyInstruction::Jump(_)
            | TackyInstruction::JumpIfZero { .. }
            | TackyInstruction::JumpIfNotZero { .. }
            | TackyInstruction::Return(_)
            | TackyInstruction::ReturnVoid => {
                current_block.push(instr.clone());
                blocks.push(std::mem::take(&mut current_block));
            }
            _ => {
                current_block.push(instr.clone());
            }
        }
    }

    if !current_block.is_empty() {
        blocks.push(current_block);
    }

    blocks
}

// ─────────────────────────────────────────────────────────────
// ユニットテスト
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_int_var(name: &str) -> TackyVal {
        TackyVal::Var(name.to_string())
    }

    fn make_var_types(names: &[&str]) -> HashMap<String, Type> {
        names.iter().map(|n| (n.to_string(), Type::Int)).collect()
    }

    #[test]
    fn test_constant_encoding_replaces_int_constants() {
        let instrs = vec![
            TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Int(42)),
                dst: make_int_var("x"),
            },
            TackyInstruction::Return(make_int_var("x")),
        ];
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["x"]);

        let result = constant_encoding(instrs, &mut ctx, &mut var_types);

        // 定数 42 が直接 Copy されていないこと
        let has_direct_42 = result.iter().any(|i| matches!(i,
            TackyInstruction::Copy { src: TackyVal::Constant(TackyConst::Int(42)), .. }
            if !matches!(i, TackyInstruction::Copy { dst: TackyVal::Var(n), .. } if n.starts_with("obf_tmp."))
        ));
        assert!(!has_direct_42, "constant 42 should be encoded");

        // Binary 演算（Multiply）が含まれていること
        let has_multiply = result.iter().any(|i| matches!(i,
            TackyInstruction::Binary { op: TackyBinaryOp::Multiply, .. }));
        assert!(has_multiply, "should contain a multiply operation");
    }

    #[test]
    fn test_constant_encoding_zero_uses_subtract() {
        let instrs = vec![
            TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Int(0)),
                dst: make_int_var("x"),
            },
        ];
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["x"]);

        let result = constant_encoding(instrs, &mut ctx, &mut var_types);

        let has_subtract = result.iter().any(|i| matches!(i,
            TackyInstruction::Binary { op: TackyBinaryOp::Subtract, .. }));
        assert!(has_subtract, "zero should use a-a subtract pattern");
    }

    #[test]
    fn test_constant_encoding_skips_double() {
        let instrs = vec![
            TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Double(3.14)),
                dst: make_int_var("x"),
            },
        ];
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["x"]);

        let result = constant_encoding(instrs, &mut ctx, &mut var_types);

        // Double はそのまま残る
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0],
            TackyInstruction::Copy { src: TackyVal::Constant(TackyConst::Double(_)), .. }));
    }

    #[test]
    fn test_junk_code_increases_instruction_count() {
        let instrs = vec![
            TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Int(1)),
                dst: make_int_var("a"),
            },
            TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Int(2)),
                dst: make_int_var("b"),
            },
            TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Int(3)),
                dst: make_int_var("c"),
            },
            TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Int(4)),
                dst: make_int_var("d"),
            },
            TackyInstruction::Return(make_int_var("d")),
        ];
        let original_len = instrs.len();
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["a", "b", "c", "d"]);

        let result = junk_code_insertion(instrs, &mut ctx, &mut var_types, 4);

        assert!(result.len() > original_len, "junk code should increase instruction count");
    }

    #[test]
    fn test_opaque_predicates_inserts_branches() {
        // 5つ以上の値生成命令があると、不透明述語が挿入される
        let instrs = vec![
            TackyInstruction::Copy { src: TackyVal::Constant(TackyConst::Int(1)), dst: make_int_var("a") },
            TackyInstruction::Copy { src: TackyVal::Constant(TackyConst::Int(2)), dst: make_int_var("b") },
            TackyInstruction::Copy { src: TackyVal::Constant(TackyConst::Int(3)), dst: make_int_var("c") },
            TackyInstruction::Copy { src: TackyVal::Constant(TackyConst::Int(4)), dst: make_int_var("d") },
            TackyInstruction::Copy { src: TackyVal::Constant(TackyConst::Int(5)), dst: make_int_var("e") },
            TackyInstruction::Return(make_int_var("e")),
        ];
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["a", "b", "c", "d", "e"]);

        let result = opaque_predicates(instrs, &mut ctx, &mut var_types, 5);

        // JumpIfZero（不透明述語の分岐）が挿入されていること
        let has_jump_if_zero = result.iter().any(|i| matches!(i, TackyInstruction::JumpIfZero { .. }));
        assert!(has_jump_if_zero, "opaque predicates should insert conditional branches");
    }

    #[test]
    fn test_control_flow_flattening_adds_dispatch() {
        let instrs = vec![
            TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Int(1)),
                dst: make_int_var("x"),
            },
            TackyInstruction::JumpIfZero {
                condition: make_int_var("x"),
                target: ".L_else".to_string(),
            },
            TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Int(10)),
                dst: make_int_var("r"),
            },
            TackyInstruction::Jump(".L_end".to_string()),
            TackyInstruction::Label(".L_else".to_string()),
            TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Int(20)),
                dst: make_int_var("r"),
            },
            TackyInstruction::Label(".L_end".to_string()),
            TackyInstruction::Return(make_int_var("r")),
        ];
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["x", "r"]);
        let mut static_vars = Vec::new();

        let result = control_flow_flattening(instrs, &mut ctx, &mut var_types, &mut static_vars, 37, 0xCAFE);

        // dispatch ラベル（.Lobf_）が存在すること
        let has_dispatch_label = result.iter().any(|i| {
            if let TackyInstruction::Label(l) = i {
                l.starts_with(".Lobf_")
            } else {
                false
            }
        });
        assert!(has_dispatch_label, "CFF should add dispatch labels");

        // state 変数（obf_tmp.）が使用されていること
        let has_state_var = result.iter().any(|i| {
            if let TackyInstruction::Copy { dst: TackyVal::Var(n), .. } = i {
                n.starts_with("obf_tmp.")
            } else {
                false
            }
        });
        assert!(has_state_var, "CFF should use obf_tmp state variable");

        // ジャンプテーブルが static_vars に追加されていること（Feature 1）
        assert!(!static_vars.is_empty(), "CFF should create jump table static var");
        let jt_var = &static_vars[0];
        assert!(matches!(&jt_var.init, TackyStaticInit::PointerArrayInit(_)),
            "jump table should use PointerArrayInit");

        // JumpIndirect が存在すること（Feature 1）
        let has_jump_indirect = result.iter().any(|i| matches!(i, TackyInstruction::JumpIndirect { .. }));
        assert!(has_jump_indirect, "CFF should use JumpIndirect for dispatch");

        // 状態エンコード定数（CFF_B = 0xCAFE）が使用されていること（Feature 4）
        let has_cafe = result.iter().any(|i| {
            if let TackyInstruction::Copy { src: TackyVal::Constant(TackyConst::Int(v)), .. } = i {
                *v == 0xCAFE_u16 as i32
            } else {
                false
            }
        });
        assert!(has_cafe, "CFF should use encoded state values (0xCAFE)");
    }

    #[test]
    fn test_opaque_predicate_diversification() {
        // Feature 5: 4つの異なるパターンが使用されることを確認
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&[]);

        // 各パターンが使われることを確認（label_counter % 4 で選択）
        for i in 0..4 {
            ctx.label_counter = i;
            let instr = TackyInstruction::Copy {
                src: TackyVal::Constant(TackyConst::Int(42)),
                dst: make_int_var("test_dst"),
            };
            let result = wrap_with_opaque_predicate(instr, &mut ctx, &mut var_types);

            // JumpIfZero が必ず含まれること
            let has_jump = result.iter().any(|i| matches!(i, TackyInstruction::JumpIfZero { .. }));
            assert!(has_jump, "pattern {i} should generate JumpIfZero");
        }
    }

    #[test]
    fn test_arith_subst_expands_add() {
        // freq=1 で全ての Add が置換されることを確認
        let instrs = vec![
            TackyInstruction::Binary {
                op: TackyBinaryOp::Add,
                left: make_int_var("a"),
                right: make_int_var("b"),
                dst: make_int_var("c"),
            },
        ];
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["a", "b", "c"]);

        let result = arithmetic_substitution(instrs, &mut ctx, &mut var_types, 1);

        // 元の1命令が3命令以上に展開されること
        assert!(result.len() >= 3, "Add should be expanded to 3+ instructions, got {}", result.len());
        // 元のAdd命令がそのまま残っていないこと
        let has_original = result.len() == 1;
        assert!(!has_original, "original Add should be replaced");
    }

    #[test]
    fn test_arith_subst_expands_subtract() {
        let instrs = vec![
            TackyInstruction::Binary {
                op: TackyBinaryOp::Subtract,
                left: make_int_var("a"),
                right: make_int_var("b"),
                dst: make_int_var("c"),
            },
        ];
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["a", "b", "c"]);

        let result = arithmetic_substitution(instrs, &mut ctx, &mut var_types, 1);

        assert!(result.len() >= 3, "Subtract should be expanded to 3+ instructions, got {}", result.len());
    }

    #[test]
    fn test_arith_subst_skips_multiply() {
        let instrs = vec![
            TackyInstruction::Binary {
                op: TackyBinaryOp::Multiply,
                left: make_int_var("a"),
                right: make_int_var("b"),
                dst: make_int_var("c"),
            },
        ];
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["a", "b", "c"]);

        let result = arithmetic_substitution(instrs, &mut ctx, &mut var_types, 1);

        // Multiply はそのまま残る
        assert_eq!(result.len(), 1, "Multiply should not be expanded");
    }

    #[test]
    fn test_arith_subst_skips_obf_tmp() {
        // obf_tmp.* への操作はカスケード防止でスキップされる
        let instrs = vec![
            TackyInstruction::Binary {
                op: TackyBinaryOp::Add,
                left: make_int_var("a"),
                right: make_int_var("b"),
                dst: TackyVal::Var("obf_tmp.0".to_string()),
            },
        ];
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["a", "b"]);
        var_types.insert("obf_tmp.0".to_string(), Type::Int);

        let result = arithmetic_substitution(instrs, &mut ctx, &mut var_types, 1);

        // obf_tmp への Add はスキップされる
        assert_eq!(result.len(), 1, "Add to obf_tmp should not be expanded");
    }

    #[test]
    fn test_arith_subst_respects_frequency() {
        // freq=2 だと 2回目の Add のみ置換される
        let instrs = vec![
            TackyInstruction::Binary {
                op: TackyBinaryOp::Add,
                left: make_int_var("a"),
                right: make_int_var("b"),
                dst: make_int_var("c"),
            },
            TackyInstruction::Binary {
                op: TackyBinaryOp::Add,
                left: make_int_var("c"),
                right: make_int_var("a"),
                dst: make_int_var("d"),
            },
        ];
        let mut ctx = ObfCtx::new();
        let mut var_types = make_var_types(&["a", "b", "c", "d"]);

        let result = arithmetic_substitution(instrs, &mut ctx, &mut var_types, 2);

        // 最初の Add はそのまま、2番目が展開される → 1 + 3+ = 4+ 命令
        assert!(result.len() >= 4, "only every 2nd Add should be expanded, got {} instructions", result.len());
        // 最初の命令は元の Add のままであること
        assert!(matches!(&result[0], TackyInstruction::Binary { op: TackyBinaryOp::Add, .. }),
            "first Add should remain unchanged");
    }

    #[test]
    fn test_state_encoding_consistency() {
        // Feature 4: エンコード/デコードの一貫性を検証
        let cff_a: i32 = 37;
        let cff_b: i32 = 0xCAFE;
        for i in 0..20 {
            let encoded = (i as i32).wrapping_mul(cff_a).wrapping_add(cff_b);
            let decoded = (encoded.wrapping_sub(cff_b)) / cff_a;
            assert_eq!(decoded, i as i32, "encode/decode should be consistent for index {i}");
        }
    }
}
