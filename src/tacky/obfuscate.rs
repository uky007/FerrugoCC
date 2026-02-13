//! TACKY IR 難読化パス
//!
//! TACKY → TACKY の変換を行う難読化パス（最適化の逆）。
//! `--fobfuscate` フラグで有効化される。
//!
//! # パス適用順序（TACKY IR レベル）
//! 1. **Constant Encoding**（定数の間接化）— 即値を `a * b + c` の実行時計算に置換
//! 2. **Junk Code Insertion**（ジャンクコード挿入）— 4命令ごとに dead computation を挿入
//! 3. **Opaque Predicates**（不透明述語）— 4パターンの常真条件分岐で値生成命令を囲む
//!    - パターン 0: `x*(x+1) % 2 == 0`（連続整数の積は偶数）
//!    - パターン 1: `!(x² + 1 > 0)`（x²+1 は常に正）
//!    - パターン 2: `(x+1)² - x² - 1 - 2x == 0`（代数恒等式）
//!    - パターン 3: `(x³ - x) % 3 == 0`（連続3整数の積は3の倍数）
//! 4. **Control Flow Flattening**（制御フロー平坦化）— 基本ブロックをジャンプテーブル
//!    + 状態エンコードの dispatch ループに変換。IDA 等の CFG 復元を破壊する。
//!    - ジャンプテーブル: `.data` セクションにブロックラベルの配列を配置し `jmp *%rax` で分岐
//!    - 状態エンコード: `encoded = index * 37 + 0xCAFE` のアフィン変換で状態変数を符号化
//! 5. **String Encryption**（文字列暗号化）— 文字列リテラルを加算暗号化し main() で復号
//!
//! Pass 5 は他のパスの後に適用する。復号コードが CFF 等で破壊されるのを防ぐため。
//!
//! # ASM レベル難読化（codegen/mod.rs で適用、レジスタ割り当て後）
//! - **Anti-Disassembly**: 無条件ジャンプ直後に `0xE8`（call opcode）を挿入し命令境界認識を破壊
//! - **Indirect Calls**: `call func` を `lea func(%rip), %r10; call *%r10` に変換

use super::tacky_ast::*;
use crate::parse::ast::Type;

/// 難読化コンテキスト — temp 変数とラベルのカウンタを管理
struct ObfCtx {
    tmp_counter: usize,
    label_counter: usize,
}

impl ObfCtx {
    fn new() -> Self {
        ObfCtx {
            tmp_counter: 0,
            label_counter: 0,
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
/// 各関数に対し Pass 1→4 を適用した後、プログラム全体に Pass 5（文字列暗号化）を適用する。
/// Pass 5 を最後にすることで、復号コードが他のパス（特に CFF）で破壊されるのを防ぐ。
pub fn obfuscate(program: TackyProgram) -> TackyProgram {
    let mut program = program;

    let mut ctx = ObfCtx::new();

    for func in &mut program.functions {

        // Pass 1: 定数の間接化
        func.body = constant_encoding(std::mem::take(&mut func.body), &mut ctx, &mut func.var_types);

        // Pass 2: ジャンクコード挿入
        func.body = junk_code_insertion(std::mem::take(&mut func.body), &mut ctx, &mut func.var_types);

        // Pass 3: 不透明述語（多様化パターン）
        func.body = opaque_predicates(std::mem::take(&mut func.body), &mut ctx, &mut func.var_types);

        // Pass 4: 制御フロー平坦化（ジャンプテーブル + 状態エンコード）
        func.body = control_flow_flattening(
            std::mem::take(&mut func.body),
            &mut ctx,
            &mut func.var_types,
            &mut program.static_vars,
        );
    }

    // Pass 5: 文字列暗号化（他のパスの後に適用 — 復号コードが CFF 等で壊されるのを防ぐ）
    string_encryption(&mut program, &mut ctx);

    program
}

// ─────────────────────────────────────────────────────────────
// Pass 5: String Encryption（文字列暗号化）
// ─────────────────────────────────────────────────────────────

/// 暗号化キー（加算ベース — XOR が TACKY IR にないため）
const STRING_ENCRYPT_KEY: u8 = 0x5A;

/// 文字列定数を暗号化し、main() の先頭に復号コードを挿入する。
///
/// 1. static_constants から StringInit を抽出し、バイト列を加算暗号化
/// 2. 暗号化バイト列を ByteArrayInit として static_vars に移動（.data、書き込み可能）
/// 3. main() の先頭にアンロール復号コードを挿入:
///    各バイトを Load → Subtract(key) → Store で復号
fn string_encryption(program: &mut TackyProgram, ctx: &mut ObfCtx) {
    // 暗号化対象の文字列定数を収集
    let mut encrypted_strings: Vec<(String, Vec<u8>, usize)> = Vec::new(); // (label, encrypted_bytes, original_len_with_null)

    program.static_constants.retain(|sc| {
        if let TackyStaticInit::StringInit(content, byte_len) = &sc.init {
            // 各バイトをキーで加算暗号化（null 終端含む）
            let mut encrypted: Vec<u8> = content.as_bytes().iter()
                .map(|b| b.wrapping_add(STRING_ENCRYPT_KEY))
                .collect();
            // null 終端も暗号化
            encrypted.push(0u8.wrapping_add(STRING_ENCRYPT_KEY));

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
                    right: TackyVal::Constant(TackyConst::Int(STRING_ENCRYPT_KEY as i32)),
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
// Pass 2: Junk Code Insertion（ジャンクコード挿入）
// ─────────────────────────────────────────────────────────────

/// 4命令ごとに dead computation（結果が使われない計算）を挿入する。
/// Label の直前には挿入しない。
fn junk_code_insertion(
    instrs: Vec<TackyInstruction>,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
) -> Vec<TackyInstruction> {
    let mut result = Vec::new();
    let mut count = 0;

    for (i, instr) in instrs.iter().enumerate() {
        // 4命令ごとにジャンクコードを挿入（ただし Label の直前は避ける）
        if count > 0 && count % 4 == 0 {
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
// Pass 3: Opaque Predicates（不透明述語）
// ─────────────────────────────────────────────────────────────

/// 5回に1回、値生成命令を常に真の条件分岐で囲む。
/// `x * (x + 1) % 2 == 0` は任意の整数 x で常に真（連続整数の積は偶数）。
fn opaque_predicates(
    instrs: Vec<TackyInstruction>,
    ctx: &mut ObfCtx,
    var_types: &mut std::collections::HashMap<String, Type>,
) -> Vec<TackyInstruction> {
    let mut result = Vec::new();
    let mut candidate_count = 0;

    for instr in instrs {
        if is_value_producing(&instr) {
            candidate_count += 1;
            if candidate_count % 5 == 0 {
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
// Pass 4: Control Flow Flattening（制御フロー平坦化）
// ─────────────────────────────────────────────────────────────

/// CFF 状態エンコード定数
/// エンコード: `encoded = index * CFF_A + CFF_B`
/// デコード: `index = (encoded - CFF_B) / CFF_A`
const CFF_A: i32 = 37;
const CFF_B: i32 = 0xCAFE_u16 as i32;

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
        (index as i32).wrapping_mul(CFF_A).wrapping_add(CFF_B)
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
        right: TackyVal::Constant(TackyConst::Int(CFF_B)),
        dst: TackyVal::Var(tmp_sub.clone()),
    });
    result.push(TackyInstruction::Binary {
        op: TackyBinaryOp::Divide,
        left: TackyVal::Var(tmp_sub),
        right: TackyVal::Constant(TackyConst::Int(CFF_A)),
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

        // ブロック内の命令を出力（ラベルは新しいブロックラベルに置き換え済みなのでスキップ）
        for instr in block {
            match instr {
                // 元のラベルはスキップ（ブロックラベルで代替）
                TackyInstruction::Label(_) => {}

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

        let result = junk_code_insertion(instrs, &mut ctx, &mut var_types);

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

        let result = opaque_predicates(instrs, &mut ctx, &mut var_types);

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

        let result = control_flow_flattening(instrs, &mut ctx, &mut var_types, &mut static_vars);

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
    fn test_state_encoding_consistency() {
        // Feature 4: エンコード/デコードの一貫性を検証
        for i in 0..20 {
            let encoded = (i as i32).wrapping_mul(CFF_A).wrapping_add(CFF_B);
            let decoded = (encoded.wrapping_sub(CFF_B)) / CFF_A;
            assert_eq!(decoded, i as i32, "encode/decode should be consistent for index {i}");
        }
    }
}
