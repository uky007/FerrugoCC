//! 難読化設定（ObfuscationConfig）
//!
//! 各難読化パスの有効/無効およびパラメータを管理する。
//! `--fobfuscate` と `--obf-level=N` で制御する。
//! 全16パス（TACKY IR 11パス + ASM 5パス）。

/// 難読化パスの設定。パイプライン全体で共有される。
#[derive(Debug, Clone)]
pub struct ObfuscationConfig {
    // ── パス有効/無効 ──
    /// Pass 1 (TACKY): 定数の間接化（即値を a*b+c に分解）
    pub constant_encoding: bool,
    /// Pass 2 (TACKY): 算術置換（Add/Subtract を多段計算に展開）
    pub arith_subst: bool,
    /// Pass 3 (TACKY): ジャンクコード挿入
    pub junk_code: bool,
    /// Pass 4 (TACKY): 不透明述語
    pub opaque_predicates: bool,
    /// Pass 5 (TACKY): 制御フロー平坦化
    pub cff: bool,
    /// Pass 6 (TACKY): 文字列暗号化
    pub string_encryption: bool,
    /// Pass 7 (ASM): 反逆アセンブリ（ゴミバイト挿入）
    pub anti_disassembly: bool,
    /// Pass 8 (ASM): 関数呼び出しの間接化
    pub indirect_calls: bool,
    /// Pass 9 (ASM): レジスタシャッフル（dead mov 挿入）
    pub reg_shuffle: bool,
    /// Pass 10 (ASM): スタックフレーム難読化（偽スタックスロット挿入）
    pub stack_frame_obf: bool,
    /// Pass 11 (ASM): 命令置換（同等命令列への置換）
    pub instr_subst: bool,
    /// Pass 12 (TACKY): 関数インライン展開
    pub func_inline: bool,
    /// Pass 13 (TACKY): 関数アウトライン化
    pub func_outline: bool,
    /// Pass 14 (TACKY): VM仮想化（コード仮想化）
    pub vm_virtualize: bool,
    /// Pass 15 (TACKY): ライブラリ関数難読化
    pub lib_obfuscate: bool,
    /// Pass 16 (TACKY): OPSEC 衛生化（シンボルリネーム）
    pub opsec: bool,
    /// OPSEC 文字列リーク警告
    pub opsec_warn: bool,

    // ── 頻度パラメータ ──
    /// ジャンクコード挿入頻度（N命令ごとに挿入）
    pub junk_freq: usize,
    /// 不透明述語頻度（N個の値生成命令ごとに適用）
    pub pred_freq: usize,

    // ── CFF パラメータ ──
    /// 状態エンコード乗数 (encoded = index * cff_a + cff_b)
    pub cff_a: i32,
    /// 状態エンコードバイアス
    pub cff_b: i32,

    /// 算術置換頻度（N回に1回適用）
    pub arith_freq: usize,
    /// レジスタシャッフル頻度（N命令ごとに挿入）
    pub reg_shuffle_freq: usize,
    /// 偽スタックスロット数
    pub stack_frame_padding: usize,
    /// 偽スタック操作の挿入頻度（N命令ごと）
    pub stack_frame_fake_freq: usize,
    /// 命令置換頻度（N命令ごと）
    pub instr_subst_freq: usize,
    /// インライン展開頻度（N回の適格呼び出しごとにインライン化）
    pub func_inline_freq: usize,
    /// アウトライン化の最小ブロックサイズ
    pub func_outline_min_block: usize,

    // ── 文字列暗号化パラメータ ──
    /// 暗号化キー（加算ベース）
    pub string_key: u8,
}

impl ObfuscationConfig {
    /// レベルに応じたプリセット設定を生成する。
    ///
    /// | Level | 概要 |
    /// |-------|------|
    /// | 1     | 軽量: 定数+ジャンク+述語のみ、低頻度 |
    /// | 2     | 標準: +CFF+算術置換 |
    /// | 3     | 強力: 全14パス有効（VM仮想化除く） |
    /// | 4     | 最大: 全15パス有効、高頻度 |
    pub fn from_level(level: u8) -> Self {
        match level {
            1 => ObfuscationConfig {
                constant_encoding: true,
                arith_subst: false,
                junk_code: true,
                opaque_predicates: true,
                cff: false,
                string_encryption: false,
                anti_disassembly: false,
                indirect_calls: false,
                reg_shuffle: false,
                stack_frame_obf: false,
                instr_subst: false,
                func_inline: false,
                func_outline: false,
                vm_virtualize: false,
                lib_obfuscate: true,
                opsec: false,
                opsec_warn: false,
                junk_freq: 8,
                pred_freq: 10,
                arith_freq: 3,
                reg_shuffle_freq: 5,
                stack_frame_padding: 4,
                stack_frame_fake_freq: 8,
                instr_subst_freq: 4,
                func_inline_freq: 3,
                func_outline_min_block: 4,
                cff_a: 37,
                cff_b: 0xCAFE,
                string_key: 0x5A,
            },
            2 => ObfuscationConfig {
                constant_encoding: true,
                arith_subst: true,
                junk_code: true,
                opaque_predicates: true,
                cff: true,
                string_encryption: false,
                anti_disassembly: false,
                indirect_calls: false,
                reg_shuffle: false,
                stack_frame_obf: false,
                instr_subst: false,
                func_inline: false,
                func_outline: false,
                vm_virtualize: false,
                lib_obfuscate: true,
                opsec: false,
                opsec_warn: true,
                junk_freq: 4,
                pred_freq: 5,
                arith_freq: 5,
                reg_shuffle_freq: 5,
                stack_frame_padding: 4,
                stack_frame_fake_freq: 8,
                instr_subst_freq: 4,
                func_inline_freq: 3,
                func_outline_min_block: 4,
                cff_a: 37,
                cff_b: 0xCAFE,
                string_key: 0x5A,
            },
            3 => ObfuscationConfig {
                constant_encoding: true,
                arith_subst: true,
                junk_code: true,
                opaque_predicates: true,
                cff: true,
                string_encryption: true,
                anti_disassembly: true,
                indirect_calls: true,
                reg_shuffle: true,
                stack_frame_obf: true,
                instr_subst: true,
                func_inline: true,
                func_outline: true,
                vm_virtualize: false,
                lib_obfuscate: true,
                opsec: true,
                opsec_warn: true,
                junk_freq: 4,
                pred_freq: 5,
                arith_freq: 3,
                reg_shuffle_freq: 5,
                stack_frame_padding: 4,
                stack_frame_fake_freq: 8,
                instr_subst_freq: 4,
                func_inline_freq: 3,
                func_outline_min_block: 4,
                cff_a: 37,
                cff_b: 0xCAFE,
                string_key: 0x5A,
            },
            // level >= 4: 最大
            _ => ObfuscationConfig {
                constant_encoding: true,
                arith_subst: true,
                junk_code: true,
                opaque_predicates: true,
                cff: true,
                string_encryption: true,
                anti_disassembly: true,
                indirect_calls: true,
                reg_shuffle: true,
                stack_frame_obf: true,
                instr_subst: true,
                func_inline: true,
                func_outline: true,
                vm_virtualize: true,
                lib_obfuscate: true,
                opsec: true,
                opsec_warn: true,
                junk_freq: 2,
                pred_freq: 2,
                arith_freq: 2,
                reg_shuffle_freq: 3,
                stack_frame_padding: 8,
                stack_frame_fake_freq: 4,
                instr_subst_freq: 2,
                func_inline_freq: 2,
                func_outline_min_block: 3,
                cff_a: 37,
                cff_b: 0xCAFE,
                string_key: 0x5A,
            },
        }
    }

    /// 現在のデフォルト設定（Level 3 相当 = 従来の --fobfuscate と同等）
    #[allow(dead_code)]
    pub fn default_all() -> Self {
        Self::from_level(3)
    }
}
