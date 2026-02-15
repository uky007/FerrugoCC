//! FerrugoCC — Rust製Cコンパイラ
//!
//! "Writing a C Compiler" (Nora Sandler) に沿って開発する学習用Cコンパイラ。
//!
//! # パイプライン
//! ```text
//! source.c → [Lex] → [Parse] → [Validate] → [TackyGen] → [Optimize/Obfuscate]
//!          → [Codegen] → [RegAlloc+Coalescing] → [Fixup] → [Emit] → source.s → [gcc] → binary
//! ```
//!
//! `--fobfuscate` 指定時は最適化の代わりに難読化パス（15パス）を適用する。
//! `--obf-level=N` で難読化強度を段階的に制御可能（1=軽量〜4=最大）。
//!
//! # 使い方
//! ```text
//! ferrugocc <source.c>                                  # フルコンパイル（実行ファイル生成）
//! ferrugocc --lex <source.c>                            # 字句解析のみ
//! ferrugocc --parse <source.c>                          # 構文解析まで
//! ferrugocc --validate <source.c>                       # 型検査まで
//! ferrugocc --tacky <source.c>                          # TACKY IR 生成まで
//! ferrugocc --codegen <source.c>                        # コード生成まで（Asm AST 構築）
//! ferrugocc -S <source.c>                               # アセンブリ出力まで（.s ファイル生成）
//! ferrugocc --fobfuscate <source.c>                     # 難読化コンパイル（Level 3）
//! ferrugocc --fobfuscate --obf-level=1 <source.c>       # 軽量難読化
//! ferrugocc --fobfuscate --obf-level=4 <source.c>       # 最大難読化
//! ferrugocc --fobfuscate --obf-no-cff <source.c>        # CFF 無効化
//! ferrugocc --fobfuscate --obf-no-reg-shuffle <source.c> # レジスタシャッフル無効化
//! ferrugocc --fobfuscate --obf-no-stack-frame <source.c> # スタックフレーム難読化無効化
//! ferrugocc --fobfuscate --obf-no-instr-subst <source.c> # 命令置換無効化
//! ferrugocc --fobfuscate --obf-no-func-inline <source.c> # 関数インライン展開無効化
//! ferrugocc --fobfuscate --obf-no-func-outline <source.c> # 関数アウトライン化無効化
//! ferrugocc --fobfuscate --obf-no-lib-obfuscate <source.c> # ライブラリ関数難読化無効化
//! ferrugocc --fobfuscate --obf-junk-freq=2 <source.c>   # ジャンク頻度変更
//! ferrugocc --fobfuscate --obf-reg-shuffle-freq=3 <source.c> # シャッフル頻度変更
//! ferrugocc --fobfuscate --obf-inline-freq=2 <source.c>  # インライン頻度変更
//! ferrugocc --fobfuscate --obf-outline-min-block=3 <source.c> # アウトライン最小ブロック変更
//! ```

mod error;
mod lex;
mod parse;
mod typecheck;
mod tacky;
mod codegen;
mod emit;
mod driver;
mod obfuscation;

use std::path::PathBuf;
use std::process;

use clap::Parser;

use driver::Stage;
use obfuscation::ObfuscationConfig;

/// FerrugoCC のコマンドライン引数定義。
///
/// `clap` の derive マクロにより、構造体のフィールドから
/// 自動的にコマンドライン引数のパーサーが生成される。
#[derive(Parser)]
#[command(name = "ferrugocc", about = "A C compiler written in Rust")]
struct Cli {
    /// Run the lexer only
    #[arg(long)]
    lex: bool,

    /// Run through parsing
    #[arg(long)]
    parse: bool,

    /// Run through type checking (validate)
    #[arg(long)]
    validate: bool,

    /// Run through TACKY IR generation
    #[arg(long)]
    tacky: bool,

    /// Run through code generation
    #[arg(long)]
    codegen: bool,

    /// Emit assembly (.s) only
    #[arg(short = 'S')]
    emit_asm: bool,

    /// 難読化コンパイル（最適化の代わりに難読化パスを適用）
    #[arg(long = "fobfuscate")]
    obfuscate: bool,

    /// 難読化レベル (1=軽量, 2=標準, 3=全パス有効, 4=最大)
    #[arg(long = "obf-level", default_value_t = 3)]
    obf_level: u8,

    /// CFF（制御フロー平坦化）を無効化
    #[arg(long = "obf-no-cff")]
    obf_no_cff: bool,

    /// 文字列暗号化を無効化
    #[arg(long = "obf-no-strings")]
    obf_no_strings: bool,

    /// 反逆アセンブリを無効化
    #[arg(long = "obf-no-anti-disasm")]
    obf_no_anti_disasm: bool,

    /// 間接呼び出し変換を無効化
    #[arg(long = "obf-no-indirect-calls")]
    obf_no_indirect_calls: bool,

    /// ジャンクコード挿入頻度（N命令ごと）
    #[arg(long = "obf-junk-freq")]
    obf_junk_freq: Option<usize>,

    /// 不透明述語頻度（N個の値生成命令ごと）
    #[arg(long = "obf-pred-freq")]
    obf_pred_freq: Option<usize>,

    /// 算術置換を無効化
    #[arg(long = "obf-no-arith-subst")]
    obf_no_arith_subst: bool,

    /// レジスタシャッフルを無効化
    #[arg(long = "obf-no-reg-shuffle")]
    obf_no_reg_shuffle: bool,

    /// スタックフレーム難読化を無効化
    #[arg(long = "obf-no-stack-frame")]
    obf_no_stack_frame: bool,

    /// 命令置換を無効化
    #[arg(long = "obf-no-instr-subst")]
    obf_no_instr_subst: bool,

    /// 関数インライン展開を無効化
    #[arg(long = "obf-no-func-inline")]
    obf_no_func_inline: bool,

    /// 関数アウトライン化を無効化
    #[arg(long = "obf-no-func-outline")]
    obf_no_func_outline: bool,

    /// VM仮想化を無効化
    #[arg(long = "obf-no-vm-virtualize")]
    obf_no_vm_virtualize: bool,

    /// ライブラリ関数難読化を無効化
    #[arg(long = "obf-no-lib-obfuscate")]
    obf_no_lib_obfuscate: bool,

    /// 算術置換頻度（N回に1回適用）
    #[arg(long = "obf-arith-freq")]
    obf_arith_freq: Option<usize>,

    /// レジスタシャッフル頻度（N命令ごとに挿入）
    #[arg(long = "obf-reg-shuffle-freq")]
    obf_reg_shuffle_freq: Option<usize>,

    /// 偽スタックスロット数
    #[arg(long = "obf-stack-padding")]
    obf_stack_padding: Option<usize>,

    /// 偽スタック操作の挿入頻度（N命令ごと）
    #[arg(long = "obf-stack-fake-freq")]
    obf_stack_fake_freq: Option<usize>,

    /// 命令置換頻度（N命令ごと）
    #[arg(long = "obf-instr-subst-freq")]
    obf_instr_subst_freq: Option<usize>,

    /// インライン展開頻度（N回の適格呼び出しごとにインライン化）
    #[arg(long = "obf-inline-freq")]
    obf_inline_freq: Option<usize>,

    /// アウトライン最小ブロックサイズ
    #[arg(long = "obf-outline-min-block")]
    obf_outline_min_block: Option<usize>,

    /// Input C source file
    source: PathBuf,
}

fn main() {
    let cli = Cli::parse();

    // フラグに応じて停止ステージを決定（フラグなしならフルコンパイル）
    let stage = if cli.lex {
        Stage::Lex
    } else if cli.parse {
        Stage::Parse
    } else if cli.validate {
        Stage::Validate
    } else if cli.tacky {
        Stage::Tacky
    } else if cli.codegen {
        Stage::Codegen
    } else if cli.emit_asm {
        Stage::EmitAsm
    } else {
        Stage::Full
    };

    // 難読化設定の構築
    let obf_config = if cli.obfuscate {
        let mut config = ObfuscationConfig::from_level(cli.obf_level);

        // 個別パス無効化
        if cli.obf_no_cff { config.cff = false; }
        if cli.obf_no_strings { config.string_encryption = false; }
        if cli.obf_no_anti_disasm { config.anti_disassembly = false; }
        if cli.obf_no_indirect_calls { config.indirect_calls = false; }
        if cli.obf_no_arith_subst { config.arith_subst = false; }
        if cli.obf_no_reg_shuffle { config.reg_shuffle = false; }
        if cli.obf_no_stack_frame { config.stack_frame_obf = false; }
        if cli.obf_no_instr_subst { config.instr_subst = false; }
        if cli.obf_no_func_inline { config.func_inline = false; }
        if cli.obf_no_func_outline { config.func_outline = false; }
        if cli.obf_no_vm_virtualize { config.vm_virtualize = false; }
        if cli.obf_no_lib_obfuscate { config.lib_obfuscate = false; }

        // 頻度オーバーライド
        if let Some(freq) = cli.obf_junk_freq { config.junk_freq = freq; }
        if let Some(freq) = cli.obf_pred_freq { config.pred_freq = freq; }
        if let Some(freq) = cli.obf_arith_freq { config.arith_freq = freq; }
        if let Some(freq) = cli.obf_reg_shuffle_freq { config.reg_shuffle_freq = freq; }
        if let Some(n) = cli.obf_stack_padding { config.stack_frame_padding = n; }
        if let Some(freq) = cli.obf_stack_fake_freq { config.stack_frame_fake_freq = freq; }
        if let Some(freq) = cli.obf_instr_subst_freq { config.instr_subst_freq = freq; }
        if let Some(freq) = cli.obf_inline_freq { config.func_inline_freq = freq; }
        if let Some(n) = cli.obf_outline_min_block { config.func_outline_min_block = n; }

        Some(config)
    } else {
        None
    };

    if let Err(e) = driver::run(&cli.source, stage, obf_config) {
        eprintln!("ferrugocc: {e}");
        process::exit(1);
    }
}
