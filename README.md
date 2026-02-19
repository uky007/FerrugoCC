# FerrugoCC

A C compiler written in Rust, developed incrementally following [Writing a C Compiler](https://nostarch.com/writing-c-compiler) by Nora Sandler.

"Ferrugo" means "rust" in Latin, a nod to the implementation language.

## Build & Run

```bash
cargo build

# Full compilation (C source -> executable)
cargo run -- source.c

# Stop at each stage
cargo run -- --lex source.c      # Lexing only
cargo run -- --parse source.c    # Through parsing
cargo run -- --validate source.c # Through type checking
cargo run -- --codegen source.c  # Through code generation
cargo run -- -S source.c         # Assembly output (.s file)

# Obfuscation compilation (applies obfuscation passes instead of optimization)
cargo run -- --fobfuscate source.c
cargo run -- --fobfuscate -S source.c  # Obfuscated assembly output

# Obfuscation level (1=light, 2=standard, 3=full, 4=maximum)
cargo run -- --fobfuscate --obf-level=1 source.c  # Constant encoding + junk + predicates only
cargo run -- --fobfuscate --obf-level=4 source.c  # All passes + high frequency

# Per-pass control
cargo run -- --fobfuscate --obf-no-cff source.c              # Disable CFF
cargo run -- --fobfuscate --obf-no-strings source.c           # Disable string encryption
cargo run -- --fobfuscate --obf-no-anti-disasm source.c       # Disable anti-disassembly
cargo run -- --fobfuscate --obf-no-indirect-calls source.c    # Disable indirect calls
cargo run -- --fobfuscate --obf-no-arith-subst source.c      # Disable arithmetic substitution
cargo run -- --fobfuscate --obf-no-reg-shuffle source.c      # Disable register shuffle
cargo run -- --fobfuscate --obf-no-stack-frame source.c     # Disable stack frame obfuscation
cargo run -- --fobfuscate --obf-no-instr-subst source.c    # Disable instruction substitution
cargo run -- --fobfuscate --obf-no-func-inline source.c   # Disable function inlining
cargo run -- --fobfuscate --obf-no-func-outline source.c  # Disable function outlining
cargo run -- --fobfuscate --obf-no-vm-virtualize source.c # Disable VM virtualization

# Frequency parameters
cargo run -- --fobfuscate --obf-junk-freq=2 source.c          # Insert junk every 2 instructions
cargo run -- --fobfuscate --obf-pred-freq=3 source.c           # Apply opaque predicate every 3rd
cargo run -- --fobfuscate --obf-arith-freq=2 source.c          # Apply arithmetic substitution every 2nd
cargo run -- --fobfuscate --obf-reg-shuffle-freq=3 source.c    # Insert register shuffle every 3 instructions
cargo run -- --fobfuscate --obf-stack-padding=8 source.c       # 8 fake stack slots
cargo run -- --fobfuscate --obf-stack-fake-freq=4 source.c     # Insert fake stack ops every 4 instructions
cargo run -- --fobfuscate --obf-instr-subst-freq=2 source.c    # Attempt instruction substitution every 2
cargo run -- --fobfuscate --obf-inline-freq=2 source.c         # Inline every 2nd eligible call
cargo run -- --fobfuscate --obf-outline-min-block=3 source.c   # Minimum outlined block size of 3
```

Requires `gcc` on the system for assembling and linking.

## Tests

```bash
cargo test
```

## Benchmark Suite

A benchmark suite for quantitative evaluation of obfuscation effectiveness.
Compiles 10 C programs at Level 0 (normal) through Level 4 (maximum obfuscation),
verifying correct execution results and comparing binary sizes.

```bash
bash benchmark/generate.sh
```

Generates 50 binaries (10 programs x 5 levels) + 50 assembly files,
with automatic correctness verification via exit codes and size aggregation.

### Benchmark Programs

| # | File | Description | Expected exit code |
|---|------|-------------|:--:|
| 01 | `constant_return.c` | Constant return | 42 |
| 02 | `arithmetic.c` | Arithmetic + type conversions | 30 |
| 03 | `conditional.c` | if/else chains | 77 |
| 04 | `loop_sum.c` | for loop summation | 55 |
| 05 | `nested_loops.c` | Nested loops (bubble sort style) | 101 |
| 06 | `function_calls.c` | Multiple functions + recursion | 120 |
| 07 | `pointers.c` | Pointer arithmetic + arrays | 90 |
| 08 | `strings.c` | String literal operations | 44 |
| 09 | `structs.c` | Structs + pointers | 46 |
| 10 | `mixed_complex.c` | Combined features | 37 |

### Binary Sizes by Obfuscation Level

```
Level 0 (normal):       44,416 bytes  (1.0x)
Level 1 (light):        46,024 bytes  (1.04x)
Level 2 (standard):    118,240 bytes  (2.66x)
Level 3 (full):        141,000 bytes  (3.17x)
Level 4 (maximum):     797,936 bytes  (17.96x)
```

Output: `benchmark/output/level_N/<name>` (binaries), `benchmark/output/level_N/<name>.s` (assembly)

Intended for quantitative evaluation with deobfuscators (D-810, SATURN, etc.).

## Implementation Progress

| Chapter | Feature | Status |
|---------|---------|--------|
| 1 | Constant return (`return 42;`) | Done |
| 2 | Unary operators (`-`, `~`, `!`) | Done |
| 3 | Binary arithmetic (`+`, `-`, `*`, `/`, `%`) | Done |
| 4 | Relational, equality & logical operators (`<`, `<=`, `>`, `>=`, `==`, `!=`, `&&`, `\|\|`) | Done |
| 5 | Local variables & assignment (`int a = 5; a = 10;`) | Done |
| 6 | if/else, ternary, compound statements (`if/else`, `?:`, `{}`) | Done |
| 7 | Compound assignment, increment/decrement, comma operator | Done |
| 8 | Loop statements (`while`, `do-while`, `for`) with `break`/`continue` | Done |
| 9 | Functions (declaration, definition, calls, parameters, variadic `...`) | Done |
| 10 | File-scope variables & storage classes (`static`, `extern`) | Done |
| 11 | Long integers (`long`, type checking pass, implicit conversions) | Done |
| 12 | Unsigned integers (`unsigned int`, `unsigned long`, usual arithmetic conversions) | Done |
| 13 | Floating-point (`double`, SSE instructions, XMM registers) | Done |
| 14 | Pointers (`int *`, `&`, `*`, pointer comparison, null, casts) | Done |
| 15 | Arrays & pointer arithmetic (`int arr[10]`, `arr[i]`, `ptr + n`, `sizeof`) | Done |
| 16 | Characters & strings (`char`, `unsigned char`, char/string literals) | Done |
| 17 | void type & void pointers (`void`, `void *`, `malloc`/`free`) | Done |
| 18 | Structs (`struct`, member access, pointer-to-struct access) | Done |
| 19 | TACKY IR (three-address code intermediate representation, optimization pass infrastructure) | Done |
| 20 | Register allocation (graph coloring, liveness analysis, Chaitin-Briggs) | Done |

## Code Obfuscation (`--fobfuscate`)

After completing all 20 chapters, code obfuscation passes were implemented as an additional feature.
The `--fobfuscate` flag applies obfuscation passes instead of optimization.
Consists of 10 TACKY IR-level passes and 5 ASM-level passes (15 total).

### Obfuscation Levels

`--obf-level=N` controls obfuscation intensity:

| Level | Active Passes | VM | Use Case |
|-------|---------------|:--:|----------|
| 1 | Constant encoding, junk code, opaque predicates | No | Light: basic obfuscation |
| 2 | Level 1 + CFF, arithmetic substitution | No | Standard: adds control flow flattening |
| 3 | Level 2 + inlining, outlining, string encryption, anti-disasm, indirect calls, register shuffle, stack frame obf, instruction substitution | No | Full: all passes except VM (default) |
| 4 | All 15 passes (+ VM virtualization), high frequency | Yes | Maximum: VM virtualization + all passes at high frequency |

### TACKY IR Level (10 passes)

- **Pass 12 -- Function Inlining**: Embeds callee function bodies at call sites, destroying the call graph.
  Renames variables/labels with `_inline_{N}_{name}`, converts `Return` to `Copy + Jump`.
  Eligibility: body <= 50 instructions, non-recursive, non-main, non-Struct return, no GetAddress on parameters.
  `--obf-inline-freq=N` controls frequency (default: every 3rd eligible call)
- **Pass 1 -- Constant Encoding**: Replaces immediate values with runtime computations
  ```c
  // Before: x = 42;
  // After:  tmp_a = 6; tmp_b = 7; x = tmp_a * tmp_b;  // 6 * 7 = 42
  // Zero:   tmp = 7; x = tmp - tmp;                     // a - a = 0
  ```
- **Pass 2 -- Arithmetic Substitution**: Expands Add/Subtract into mathematically equivalent multi-step computations,
  making expression recovery by decompilers (Hex-Rays, Ghidra) difficult. 4 patterns rotated:
  | # | Target | Transform | Principle |
  |---|--------|-----------|-----------|
  | 0 | Add | `a+b` -> `(a+K)+(b-K)` | Affine transform |
  | 1 | Add | `a+b` -> `3(a+b)-2a-2b` | Coefficient expansion |
  | 2 | Sub | `a-b` -> `(a+K)-(b+K)` | Affine transform |
  | 3 | Sub | `a-b` -> `3a-3b-(2a-2b)` | Coefficient expansion |
- **Pass 3 -- Junk Code Insertion**: Inserts 3 dead computation instructions every N instructions (default 4)
- **Pass 4 -- Opaque Predicates**: Wraps value-producing instructions with always-true conditional branches every Nth time (default 5).
  Rotates 4 mathematical identities to prevent pattern-matching removal:
  | # | Identity | Principle |
  |---|----------|-----------|
  | 0 | `x*(x+1) % 2 == 0` | Product of consecutive integers is even |
  | 1 | `!(x^2 + 1 > 0)` -> 0 | x^2+1 is always positive |
  | 2 | `(x+1)^2 - x^2 - 1 - 2x == 0` | Algebraic identity |
  | 3 | `(x^3 - x) % 3 == 0` | Product of 3 consecutive integers divisible by 3 |
- **Pass 13 -- Function Outlining**: Extracts straight-line code blocks (Copy/Binary/Unary only)
  into new functions `_obf_outlined_{N}`, flooding the binary with decoy functions.
  Validates: inputs <= 6, no Double/Struct/Array I/O, intermediate variables unused outside the block (scans entire function body, including loop back-edges).
  `--obf-outline-min-block=N` sets minimum block size (default 4)
- **Pass 14 -- VM Virtualization (VM-Based Code Virtualization)**: Converts each TACKY instruction of eligible functions
  into individual handlers, with bytecode arrays and handler tables in `.data` section.
  Same category as VMProtect/Themida commercial protectors. Applied before CFF so the VM dispatch loop
  itself gets flattened, achieving double indirection.
  Eligibility: non-main, no Double types, no float conversions, no struct ops, body >= 2 instructions.
  `--obf-no-vm-virtualize` to disable (enabled only at Level 4)
  ```
  // Before: Copy(a, dst); Binary(Add, dst, b, result); Return(result);
  // After:
  //   .data: bytecode[] = {0,1,2,...}  handler_table[] = {&h0, &h1, &h2,...}
  //   dispatch: fetch bytecode[pc] -> load handler_table[idx] -> jmp *handler
  //   handler_0: Copy(a, dst); jmp dispatch
  //   handler_1: Binary(Add, dst, b, result); jmp dispatch
  //   handler_2: Return(result)  // direct return
  ```
- **Pass 15 -- Library Function Obfuscation**: Replaces calls to known library functions (`strlen`, `strcmp`,
  `strcpy`, `memcpy`, `memset`, `memcmp`, `strncmp`, `strncpy`, `strchr`, `strcat`) with equivalent custom
  TACKY IR implementations (`_obf_strlen`, etc.).
  Applied before all other passes so the custom implementations get fully obfuscated by the entire pipeline,
  defeating IDA Pro's FLIRT signature matching.
  `--obf-no-lib-obfuscate` to disable (enabled at all levels)
- **Pass 5 -- Control Flow Flattening (CFF)**: Transforms basic blocks into a jump-table + state-encoded
  dispatch loop, destroying CFG recovery in IDA Pro etc.
  - **Jump table**: Places block label array (`PointerArrayInit`) in `.data`, dispatches via `JumpIndirect` (`jmp *%rax`)
  - **State encoding**: Encodes state variable with affine transform (`encoded = index * A + B`, default A=37, B=0xCAFE).
    Decodes at dispatch (`index = (encoded - 0xCAFE) / 37`) before indexing the jump table
  ```asm
  # Generated dispatch loop
  subl $51966, %eax        # decoded = (state - 0xCAFE) / 37
  cdq
  movl $37, %r10d
  idivl %r10d
  leaq .Lobf_jt_N(%rip), %rbx  # Jump table base address
  imulq $8, %rax
  addq %rbx, %rax
  movq (%rax), %rax        # Load jump target address
  jmp *%rax                # Indirect jump
  ```
- **Pass 6 -- String Encryption**: Encrypts string literals with additive cipher (key=0x5A) and stores as `ByteArrayInit` in `.data`.
  Inserts unrolled decryption code (Load -> Subtract(key) -> Store) at the beginning of main()

### ASM Level (5 passes, applied after register allocation + fixup)

- **Pass 7 -- Anti-Disassembly**: Inserts `0xE8` (x86 `call rel32` opcode) as `.byte` after unconditional jumps.
  Linear sweep disassemblers interpret this as a 5-byte instruction, corrupting instruction boundary detection
  ```asm
  jmp .Lobf_6
  .byte 0xe8        # <- Disassembler tries to interpret as call rel32
  .Lobf_6:
  ```
- **Pass 8 -- Indirect Calls**: Converts `call func` to `lea func(%rip), %r10; call *%r10`, hindering static call graph recovery
- **Pass 9 -- Register Shuffle**: Inserts dead `movq` instructions every N instructions, creating false dependencies via R10/R11 scratch registers.
  3 patterns rotated: Dead copy, Copy chain, Round-trip
- **Pass 10 -- Stack Frame Obfuscation**: Extends the stack frame with fake slots and inserts fake store/load operations,
  causing decompilers to generate fake local variables and polluting data flow analysis
- **Pass 11 -- Instruction Substitution**: Replaces x86-64 instructions with semantically equivalent but pattern-different sequences.
  4 patterns: Add<->Sub immediate swap, Neg expansion (`not+add $1`), Mov immediate split (`mov (N+K); sub K`)

### Pass Ordering

TACKY IR pass ordering is intentionally designed:
1. **Library Function Obfuscation** first -- custom implementations get all subsequent passes applied
2. **Function Inlining** -- inlined code gets all subsequent passes applied
3. **Constant Encoding** -- constants added by later passes need not be encoded
4. **Arithmetic Substitution** -- further complicates expressions from constant encoding
5. **Junk Code** -- doesn't alter control flow, safe before CFF
6. **Opaque Predicates** -- adds branches that CFF will flatten
7. **Function Outlining** -- extracts already-obfuscated code into decoy functions
8. **VM Virtualization** -- converts functions to bytecode+VM interpreter; before CFF for double indirection
9. **CFF** -- flattens all functions including VM dispatch loops
10. **String Encryption** -- applied last so decryption code isn't destroyed by other passes

ASM-level passes are applied after register allocation (order: Stack Frame Obf -> Register Shuffle -> Instruction Substitution -> Anti-Disassembly -> Indirect Calls).

## Architecture

```
Source Code (.c)
    |
    v
+----------+
|  Lexer    |  src/lex/           Tokenize
+----+-----+
     v
+----------+
|  Parser   |  src/parse/         Build AST
+----+-----+
     v
+----------+
| Validate  |  src/typecheck/     Type checking & implicit cast insertion
+----+-----+
     v
+----------+
| TACKY Gen |  src/tacky/         C AST -> TACKY IR (three-address code)
+----+-----+
     v
+----------+
| Optimize  |  src/tacky/         TACKY IR optimization passes (default)
|    or     |  optimize.rs        Algebraic simplification, constant folding, unreachable code elimination,
|          |                      copy propagation, CSE, liveness-based DCE
| Obfuscate |  obfuscate.rs       TACKY obfuscation passes (--fobfuscate)
+----+-----+                      Inlining, constant encoding, arith subst, junk code, opaque predicates,
     v                            outlining, VM virtualization, CFF, string encryption
+----------+
| Codegen   |  src/codegen/       TACKY IR -> Asm(Pseudo)
|          |  generator.rs
+----+-----+
     v
+----------+
| RegAlloc  |  src/codegen/       Liveness analysis -> interference graph -> coalescing -> graph coloring
|          |  regalloc.rs         Pseudo -> Register/Stack(spill)
+----+-----+
     v
+----------+
| Fixup     |  src/codegen/       Fix invalid operand combinations + prologue/epilogue generation
|          |  regalloc.rs
+----+-----+
     v
+----------+
| ASM Obf   |  src/codegen/       ASM-level obfuscation (--fobfuscate)
|          |  mod.rs              Stack frame obf, register shuffle, instruction substitution, anti-disasm, indirect calls
+----+-----+
     v
+----------+
| Emitter   |  src/emit/          Assembly AST -> .s text output
+----+-----+
     v
+----------+
|  Driver   |  src/driver.rs      Invoke gcc: .s -> executable
+----------+
```

Target: x86-64 Linux/macOS (AT&T syntax).

---

## 詳細な実装ノート / Detailed Implementation Notes

以下は各章の詳細な実装ノート（日本語）。

### Chapter 20 の詳細

Chapter 20 では**グラフ彩色によるレジスタ割り当て**を実装した。
それまで全変数をスタックに配置し、毎回 load→operate→store の3命令パターンだったのを、
変数をレジスタに直接割り当てることで効率的なコードを生成する。

- **Pseudo レジスタ**: コード生成で変数を `Operand::Pseudo(name)` として出力し、後段で解決する
  ```
  // 旧: 3命令パターン（全変数スタック経由）
  movl -4(%rbp), %eax      // load left
  addl -8(%rbp), %eax      // operate
  movl %eax, -12(%rbp)     // store result

  // 新: 2命令パターン（変数がレジスタに割り当てられる）
  movl %ebx, %ecx          // left → dst
  addl %edx, %ecx          // right + dst
  ```
- **生存解析（Liveness Analysis）**: 後方データフロー解析で各命令時点の生存変数集合を計算
  - ラベル/ジャンプから CFG を構築し、不動点反復で `live_in`/`live_after` を求める
  - 暗黙的な use/def を追跡（`idiv` → AX,DX、`call` → 全 caller-saved レジスタ等）
- **干渉グラフ**: 同時に生存する変数間に辺を張る。整数グラフと XMM グラフを分離して独立に彩色
  - Mov 命令の src-dst 間には辺を張らない（coalescing で活用）
- **保守的 Coalescing**: Mov 辺で結ばれた非干渉ノードを合体し、冗長な Mov を除去
  - Briggs 基準: Pseudo-Pseudo 合体（合体後の高次隣接数 < k で安全判定）
  - George 基準: Pseudo-HardReg 合体（Pseudo の全隣接が HardReg とも干渉 or 低次で安全判定）
- **Chaitin-Briggs グラフ彩色**:
  - Simplify: degree < k のノードをスタックに push して除去
  - Potential Spill: degree が最大のノードを spill 候補としてマーク
  - Select: スタックから pop して、隣接ノードが使っていない色（レジスタ）を割り当て
  - 楽観的彩色: spill 候補でも色が見つかれば割り当て、見つからなければ実際に spill
- **レジスタ分類（System V AMD64 ABI）**:
  - 整数割り当て候補（12個）: AX, BX, CX, DX, SI, DI, R8, R9, R12, R13, R14, R15
  - Callee-saved（5個）: BX, R12, R13, R14, R15（関数呼び出しをまたいで保存が必要）
  - Scratch レジスタ: R10, R11（fixup 用）、XMM15（XMM の fixup 用）
  - XMM 割り当て候補（15個）: XMM0〜XMM14
- **Fixup パス**: レジスタ割り当て後に生じる無効なオペランド組み合わせを修正
  - `movl Stack, Stack` → scratch レジスタ（R10 or XMM15）経由の2命令に展開
  - `imul` のメモリ dst → R11 経由に展開
  - `Truncate` のメモリ-メモリ → R10 経由に展開（難読化で spill 増加時に発生）
  - `movslq $imm, reg` → `movq $imm, reg` に変換（即値の符号拡張は不要）
  - 同一レジスタ間の `mov` を除去（noop 最適化）
- **プロローグ/エピローグ自動生成**: 使用した callee-saved レジスタのみ push/pop する
  ```asm
  push %rbp              # フレームポインタ保存
  movq %rsp, %rbp
  push %rbx              # callee-saved（使用した場合のみ）
  push %r12              # callee-saved（使用した場合のみ）
  subq $N, %rsp          # spill スロット確保（16バイトアラインメント調整済み）
  ```
- **スタックオフセット補正**: callee-saved レジスタの push が `%rbp` 直下のスタック領域を使用するため、
  spill/ローカル変数のオフセットを callee-saved push 分だけ下方にシフトし、重複を防ぐ
- **スタックアラインメント**: `callee_saved_count * 8 + alloc_size ≡ 0 (mod 16)` を保証
  ```rust
  let callee_bytes = callee_count * 8;
  let total = callee_bytes + spill_bytes;
  let aligned_total = (total + 15) & !15;
  let alloc_size = aligned_total - callee_bytes;
  ```
- **アドレス取得対象の例外処理**: `&x` でアドレスを取られる変数、構造体、配列は
  レジスタ割り当ての対象外とし、従来通りスタックに配置する

### コード難読化の詳細

#### TACKY IR レベル（10パス）

- **Pass 12 — 関数インライン展開（Function Inlining）**: 呼び出し先の関数本体を呼び出し元に埋め込み、コールグラフを破壊する。
  変数・ラベルを `_inline_{N}_{name}` でリネームし、`Return` を `Copy + Jump` に変換。
  適格条件: 本体 ≤ 50 命令、非再帰、非 main、非 Struct 戻り値、パラメータの GetAddress なし。
  `--obf-inline-freq=N` で N 回の適格呼び出しごとにインライン化（デフォルト 3）
  ```c
  // 元: result = add(a, b);
  // 変換後: _inline_0_x = a; _inline_0_y = b;
  //         _inline_0_tmp = _inline_0_x + _inline_0_y;
  //         result = _inline_0_tmp;
  ```
- **Pass 1 — 定数の間接化（Constant Encoding）**: 即値をランタイム計算に置換
  ```c
  // 元: x = 42;
  // 変換後: tmp_a = 6; tmp_b = 7; x = tmp_a * tmp_b;  // 6 * 7 = 42
  // ゼロの場合: tmp = 7; x = tmp - tmp;                // a - a = 0
  ```
- **Pass 2 — 算術置換（Arithmetic Substitution）**: Add/Subtract を数学的に等価な多段計算に展開し、
  デコンパイラ（Hex-Rays, Ghidra）での式復元を困難にする。4パターンをローテーション:
  | # | 対象 | 変換 | 原理 |
  |---|------|------|------|
  | 0 | Add | `a+b` → `(a+K)+(b-K)` | アフィン変換 |
  | 1 | Add | `a+b` → `3(a+b)-2a-2b` | 係数展開 |
  | 2 | Sub | `a-b` → `(a+K)-(b+K)` | アフィン変換 |
  | 3 | Sub | `a-b` → `3a-3b-(2a-2b)` | 係数展開 |
- **Pass 3 — ジャンクコード挿入**: N命令ごと（デフォルト4）に結果が使われない dead computation を3命令挿入
- **Pass 4 — 不透明述語（Opaque Predicates）**: N回に1回（デフォルト5）、値生成命令を常に真の条件分岐で囲む。
  パターンマッチによる自動除去を防ぐため4種類の数学的恒等式をローテーション:
  | # | 恒等式 | 原理 |
  |---|--------|------|
  | 0 | `x*(x+1) % 2 == 0` | 連続整数の積は偶数 |
  | 1 | `!(x² + 1 > 0)` → 0 | x²+1 は常に正 |
  | 2 | `(x+1)² - x² - 1 - 2x == 0` | 展開すると恒等式 |
  | 3 | `(x³ - x) % 3 == 0` | 連続3整数の積は3の倍数 |
- **Pass 13 — 関数アウトライン化（Function Outlining）**: 関数内の直線コードブロック（Copy/Binary/Unary のみで構成）を
  新しい関数 `_obf_outlined_{N}` に切り出し、偽の関数を大量に出現させる。解析者が見る関数は意味不明な断片となる。
  入力変数 ≤ 6、Double/Struct/Array 型の入出力なし、中間変数がブロック外（関数全体）で未使用であることを検証。
  `--obf-outline-min-block=N` で最小ブロックサイズを設定（デフォルト 4）
  ```c
  // 元: tmp = a + b; result = tmp * c;
  // 変換後: result = _obf_outlined_0(a, b, c);
  // 新関数: int _obf_outlined_0(int p0, int p1, int p2) {
  //           t0 = p0 + p1; t1 = t0 * p2; return t1; }
  ```
- **Pass 14 — VM仮想化（VM-Based Code Virtualization）**: 適格な関数の各TACKY命令を個別の
  ハンドラに配置し、`.data` セクションにバイトコード配列とハンドラテーブルを生成する。
  VMProtect/Themida 等の商用プロテクタと同カテゴリの技術で、静的解析でのCFG復元を極めて困難にする。
  CFF の前に適用することで、VMディスパッチループ自体が CFF で平坦化され二重の間接化が実現される。
  適格条件: 非 main、Double 型なし、浮動小数点変換なし、構造体操作なし、本体 ≥ 2 命令。
  `--obf-no-vm-virtualize` で無効化可能（Level 4 のみデフォルト有効）
  ```
  // 変換前: Copy(a, dst); Binary(Add, dst, b, result); Return(result);
  // 変換後:
  //   .data: bytecode[] = {0,1,2,...}  handler_table[] = {&h0, &h1, &h2,...}
  //   dispatch: fetch bytecode[pc] → load handler_table[idx] → jmp *handler
  //   handler_0: Copy(a, dst); jmp dispatch
  //   handler_1: Binary(Add, dst, b, result); jmp dispatch
  //   handler_2: Return(result)  // 直接リターン
  ```
- **Pass 15 — ライブラリ関数難読化（Library Function Obfuscation）**: `strlen`, `strcmp`, `strcpy`,
  `memcpy`, `memset`, `memcmp`, `strncmp`, `strncpy`, `strchr`, `strcat` の既知ライブラリ関数の
  呼び出しを等価な自前実装（`_obf_strlen` 等）に差し替える。
  全パスの最初に適用することで、自前実装が後続の全パイプラインで難読化され、
  IDA Pro の FLIRT シグネチャマッチングを無効化する。
  `--obf-no-lib-obfuscate` で無効化可能（全レベルでデフォルト有効）
- **Pass 5 — 制御フロー平坦化（CFF）**: 関数内の基本ブロックをジャンプテーブル + 状態エンコードの
  dispatch ループに変換し、IDA Pro 等の CFG 復元を破壊する
  - **ジャンプテーブル**: `.data` セクションにブロックラベルの配列（`PointerArrayInit`）を配置し、
    `JumpIndirect`（`jmp *%rax`）で分岐。連続比較の `if (state == i) goto block_i` よりも
    静的解析での復元が困難
  - **状態エンコード**: state 変数をアフィン変換（`encoded = index * A + B`、デフォルト A=37, B=0xCAFE）で符号化。
    dispatch でデコード（`index = (encoded - 0xCAFE) / 37`）してからジャンプテーブルを索引。
    自動的なステートマシン復元を妨害する
  - **`JumpIndirect` の `possible_targets`**: 間接ジャンプは動的ターゲットだが、レジスタ割り当ての
    生存解析で正しい CFG 後続ブロック情報が必要。ジャンプテーブルの全エントリラベルを
    `possible_targets` として保持し、`jump_targets()` で返すことで解決
  ```asm
  # dispatch ループ（生成されるアセンブリ）
  subl $51966, %eax        # decoded = (state - 0xCAFE) / 37
  cdq
  movl $37, %r10d
  idivl %r10d
  leaq .Lobf_jt_N(%rip), %rbx  # ジャンプテーブルのベースアドレス
  imulq $8, %rax
  addq %rbx, %rax
  movq (%rax), %rax        # ジャンプ先アドレスをロード
  jmp *%rax                # 間接ジャンプ
  ```
- **Pass 6 — 文字列暗号化**: 文字列リテラルを加算暗号化（key=0x5A）して `.data` に `ByteArrayInit` として配置。
  main() の先頭にアンロール復号コード（Load → Subtract(key) → Store）を挿入
  - Pass 6 は Pass 1〜5 の後に適用する。復号コードが CFF 等で破壊されるのを防ぐため

#### ASM レベル（5パス、レジスタ割り当て+fixup 後に適用）

- **Pass 7 — 反逆アセンブリ（Anti-Disassembly）**: 無条件ジャンプ（`Jmp`, `JmpIndirect`）の直後に
  `0xE8`（x86 の `call rel32` オペコード）を `.byte` として挿入。リニアスイープ型逆アセンブラが
  5バイト命令として解釈しようとするため、後続命令の命令境界認識が破壊される
  ```asm
  jmp .Lobf_6
  .byte 0xe8        # ← 逆アセンブラはここから call rel32 として解釈を試みる
  .Lobf_6:
  ```
- **Pass 8 — 関数呼び出しの間接化（Indirect Calls）**: `call func` を
  `lea func(%rip), %r10; call *%r10` に変換。静的解析でのコールグラフ復元を妨害する
- **Pass 9 — レジスタシャッフル（Register Shuffle）**: dead な `movq` 命令を N 命令ごとに挿入し、
  R10/R11 scratch レジスタへの偽コピーでデータフローグラフに偽の依存関係を生成する。
  3パターンをローテーション: Dead copy（1命令）、Copy chain（2命令）、Round-trip（2命令）
  ```asm
  movq %rcx, %r10       # Dead copy: ライブなレジスタの値を scratch にコピー
  movq %rax, %r10       # Copy chain: レジスタ値を scratch1 に
  movq %r10, %r11       #   scratch1 → scratch2 に伝播
  movq %rdx, %r10       # Round-trip: scratch に退避
  movq %r10, %rdx       #   scratch から復元（noop）
  ```
- **Pass 10 — スタックフレーム難読化（Stack Frame Obfuscation）**: スタックフレームを拡張して偽のスタックスロットを追加し、
  偽の store/load 操作を N 命令ごとに挿入する。デコンパイラが偽スロットを「ローカル変数」として認識し、
  復元される変数数が増加する。偽の store/load パターンが実際の変数アクセスに紛れ、データフロー解析を妨害する
  ```asm
  subq $64, %rsp          # AllocateStack が拡張される（偽スロット分追加）
  movl %ecx, -48(%rbp)    # 偽の int store → デコンパイラが int 型の偽変数を生成
  movq %rax, -56(%rbp)    # 偽の quad store → long/pointer 型の偽変数を生成
  movl -48(%rbp), %r10d   # 偽の load → 偽変数の「使用」を生成
  ```
- **Pass 11 — 命令置換（Instruction Substitution）**: x86-64 命令を意味的に等価だがパターンの異なる命令列に置換し、
  デコンパイラ・逆アセンブラのパターンマッチングを妨害する。4パターンをローテーション:
  Add→Sub 即値スワップ、Sub→Add 即値スワップ、Neg 展開（`not+add $1`）、Mov 即値分割（`mov (N+K); sub K`）
  ```asm
  # Add → Sub 即値スワップ
  addl $42, %eax    →    subl $-42, %eax
  # Neg 展開（二の補数: -x = ~x + 1）
  negl %edx         →    notl %edx; addl $1, %edx
  # Mov 即値分割
  movl $100, %eax   →    movl $142, %eax; subl $42, %eax
  ```

#### パス適用順序の設計

TACKY IR パスの適用順序は意図的に設計されている:
1. **ライブラリ関数難読化**が最初 → 自前実装が後続の全パスで難読化される（FLIRT 対策）
2. **関数インライン展開** → インラインされたコードに後続の全パスが適用される
3. **定数の間接化** → 後続パスが追加する定数はエンコード不要
4. **算術置換** → 定数間接化で展開された式をさらに複雑にしつつ、後続パスでさらにノイズを加える
5. **ジャンクコード** → 制御フローを変えないので CFF の解析に影響しない
6. **不透明述語** → 分岐を追加。CFF がこれも含めて平坦化する
7. **関数アウトライン化** → Pass 1-4 で難読化済みのコードが切り出され、解析者が見る関数は意味不明な断片
8. **VM仮想化** → 適格な関数をバイトコード＋VMインタプリタに変換。CFF の前に適用することで二重間接化
9. **CFF** → VMディスパッチループを含む全関数に適用。コード＋データの相関解析が必要な二重の間接化
10. **文字列暗号化** → 復号コードが他のパスで壊されないよう最後に適用

ASM レベルパスはレジスタ割り当て後に適用する（適用順: スタックフレーム難読化 → レジスタシャッフル → 命令置換 → 反逆アセンブリ → 間接コール）:
- スタックフレーム難読化はフレーム構造を変更するため最初に適用。後続のレジスタシャッフルが挿入する dead mov が偽スタック操作の近傍に散在することで解析をさらに困難にする
- レジスタシャッフルは dead mov を挿入し R10/R11 scratch を使用。fixup シーケンスの中間を避けるため安全
- 命令置換はシャッフル後に適用。シャッフルで挿入された dead mov の近傍で命令置換が行われることでデータフロー解析をさらに困難にする
- 反逆アセンブリはジャンプ命令の位置を変えないため安全
- 間接コール変換は R10（caller-saved scratch）を使用し、Call の直前に挿入するため安全

### 可変長引数関数の定義サポートの詳細

`va_list`/`va_start`/`va_arg`/`va_end` によるユーザー定義可変長引数関数を実装した。
System V AMD64 ABI に完全準拠し、`#include <stdarg.h>` なしで使用可能（コンパイラ組み込み）。

#### 対応する引数型

- **整数型**: `int`, `long`, `unsigned int`, `unsigned long`（GP レジスタ経由）
- **ポインタ型**: `int *`, `char *` 等（GP レジスタ経由）
- **浮動小数点型**: `double`（XMM レジスタ経由）
- **7引数以上のスタック渡し**: レジスタを超過した引数は `overflow_arg_area` から取得

#### System V AMD64 ABI の va_list 構造体

```
va_list (24 bytes, align 8):
  [0..4]   gp_offset        次の GP レジスタ引数のオフセット (0〜48)
  [4..8]   fp_offset        次の XMM レジスタ引数のオフセット (48〜176)
  [8..16]  overflow_arg_area スタック渡し引数へのポインタ
  [16..24] reg_save_area    レジスタ保存領域へのポインタ
```

#### レジスタ保存領域 (176 bytes)

可変長関数の入口で、パラメータ受け取り前に全引数レジスタを保存:

```
offset 0-47:   GP レジスタ (rdi, rsi, rdx, rcx, r8, r9) × 8B = 48B
offset 48-175: XMM レジスタ (xmm0-xmm7) × 16B = 128B
```

#### va_arg の分岐ロジック

整数型:
```
if gp_offset < 48:
    val = reg_save_area[gp_offset]   // レジスタ保存領域から取得
    gp_offset += 8
else:
    val = *overflow_arg_area          // スタックから取得
    overflow_arg_area += 8
```

浮動小数点型:
```
if fp_offset < 176:
    val = reg_save_area[fp_offset]   // XMM 保存領域から取得
    fp_offset += 16
else:
    val = *overflow_arg_area          // スタックから取得
    overflow_arg_area += 8
```

#### 実装範囲

- **AST**: `Type::VaList`（24B, align 8）、`Expr::VaStart`/`VaArg`/`VaEnd`
- **パーサー**: `va_list` を型トークンとして認識（ブロック内宣言・for 初期化部・キャスト・sizeof で使用可能）。
  `va_start`/`va_arg`/`va_end` を特殊識別子としてパース
- **型チェッカー**: 引数が `va_list` 型であることを検証
- **TACKY IR**: `VaStart { gp_offset_init, fp_offset_init }`、`VaArg { arg_type }`、`VaEnd`（no-op）
- **コード生成**: va_arg の分岐ロジックをインライン展開（Cmp + JmpCC + Mov + Lea + Binary）
- **レジスタ割り当て**: `VaList` 型変数を強制スタック配置。`__va_reg_save` は `Array(UChar, 176)` として管理
- **スタックフレーム修正**: `insert_prologue_epilogue` で全 `Stack()` オペランドをスキャンし、
  強制スタック変数を含む正確なフレームサイズを計算（`scan_min_stack_offset`）

### typedef サポートの詳細

`typedef` による型エイリアス定義を実装した:

- **基本型エイリアス**: `typedef int myint;` — `myint` を `int` の別名として使用可能
- **ポインタ typedef**: `typedef int *pint;` — `pint` は `int *` のエイリアス
- **構造体 typedef**: `typedef struct point { int x; int y; } Point;` — 構造体タグと型名を同時に定義
- **配列 typedef**: `typedef int arr3[3];` — 固定長配列の型名を定義
- **ネスト typedef**: `typedef pint *ppint;` — typedef 名を使った型を再度 typedef 可能
- **unsigned 型**: `typedef unsigned long usize_t;` — 複合型指定子のエイリアス
- **グローバル/ブロックスコープ**: トップレベルおよび関数本体内の両方で typedef 宣言可能
- **関数パラメータ/戻り値**: typedef 名を関数シグネチャで使用可能
- **キャスト/sizeof**: `(myint)expr`, `sizeof(myint)` で typedef 名を認識
- **カンマ区切り**: `typedef int myint, *pint;` で複数の型名を一度に定義

実装方式:
- パーサーに `typedef_names: HashMap<String, Type>` テーブルを追加（`struct_tags` と同様のフラット方式）
- `parse_typedef()` が `typedef` キーワードを消費し、ベース型 + 宣言子をパースして型名テーブルに登録
- `parse_type_specifier()` で識別子が typedef 名テーブルに存在すればその型として解決
- AST には `TopLevelDecl::Typedef` / `BlockItem::Typedef` ノードを追加（型チェッカー・コード生成ではスキップ）
- キャスト式・sizeof 式の先読みで typedef 名を型キーワードとして判定

### Chapter 19 の詳細

Chapter 19 では TACKY IR（三番地コード中間表現）を導入した:

- **TACKY IR**: C の AST と x86-64 アセンブリの中間に位置する三番地コード形式の IR
  ```
  // C: int r = a + b * c;
  // TACKY:
  tmp.0 = b * c
  r = a + tmp.0
  ```
- **コンパイルパイプライン変更**: `C AST → TACKY IR → Assembly AST` の2段階に分離
- **最適化パス基盤**: TACKY IR 上で6パス構成の最適化パイプライン（代数的簡略化・定数畳み込み・到達不能コード除去・コピー伝播・共通部分式除去・生存解析ベース死コード除去）を実行可能にする

### Chapter 18 の詳細

Chapter 18 では構造体を実装した:

- **構造体型**: `struct` の定義とメンバアクセス
  ```c
  struct Point { int x; int y; };
  struct Point p;
  p.x = 10;
  p.y = 20;
  return p.x + p.y;  // 30
  ```
- **ポインタ経由のメンバアクセス**: `ptr->member`（`(*ptr).member` の糖衣構文）
- **`MemoryOffset(Reg, i32)` オペランド**: レジスタ+オフセット間接アドレッシングで構造体メンバにアクセス
- **`CopyToOffset`/`CopyFromOffset`**: 構造体コピーのための TACKY 命令

### Chapter 17 の詳細

Chapter 17 では以下の機能を追加した:

- **`void` 型**: 関数の戻り値型・キャスト先として使用可能な不完全型
  ```c
  void do_nothing(void) { return; }
  int main(void) { do_nothing(); return 0; }
  ```
- **`void *` (void ポインタ)**: 任意のポインタ型と暗黙的に相互変換可能な汎用ポインタ
  ```c
  void *malloc(unsigned long size);
  void free(void *ptr);
  int main(void) {
      int *arr = malloc(5 * sizeof(int));  // void* → int* 暗黙変換
      arr[0] = 42;
      int result = arr[0];
      free(arr);                            // int* → void* 暗黙変換
      return result;  // 42
  }
  ```
- **`(void)expr` キャスト**: 式の値を捨てる（副作用のみ実行）
- **void ポインタの暗黙変換**: 代入・関数引数・return・三項演算子で `void *` ↔ 任意のポインタ型
- **不完全型チェック**: `void x;`, `sizeof(void)`, void ポインタ算術等をコンパイルエラーとして検出

### Chapter 15 の詳細

Chapter 15 では以下の機能を追加した:

- **配列型**: `int arr[10]`, `long arr[5]` 等の固定長配列宣言
- **配列添字**: `arr[i]` はパーサーで `*(arr + i)` に脱糖（desugaring）
- **配列→ポインタ減衰（decay）**: 式中の配列は自動的にポインタに変換
- **ポインタ算術**: ポインタ加減算は要素サイズ分スケーリングされる
- **ポインタ減算**: 同じ型のポインタ同士の差分は要素数で返される
- **ポインタ比較の拡張**: `<`, `<=`, `>`, `>=` を同じポインタ型同士で許可
- **ポインタ増分/減分**: `++`, `--`, `+=`, `-=` が要素サイズ分移動
- **`sizeof` 演算子**: 型チェック時に定数（`unsigned long`）に解決
- **配列パラメータ**: 関数パラメータの `int arr[]` は `int *arr` に変換
- **グローバル配列**: `.bss` セクションにゼロ初期化で配置
- **配列初期化子リスト**: `int arr[3] = {1, 2, 3};` 形式の初期化に対応
  - ローカル配列: `CopyToOffset` 命令列で各要素を書き込み
  - グローバル配列・`static` ローカル配列: `.data` セクションに静的初期値を出力
  - 部分初期化: 明示要素数 < 配列サイズの場合、残りをゼロ初期化
  - 式初期化子: ローカル配列では `int x = 5; int arr[2] = {x, x+1};` が可能
- **カンマ区切り複数宣言**: `int a = 1, b = 2, *p = &a;` 形式のカンマ区切り宣言に対応
  - ローカル変数・グローバル変数・for 初期化部すべてで使用可能
  - ポインタ・配列混在: `int x = 1, *p, arr[3] = {2, 3, 4};` — 各宣言子に同じベース型を渡し、`parse_declarator()` がポインタ/配列を個別に付加
  - `parse_declaration()` が `Vec<Declaration>` を返し、`parse_block_item()` が `Vec<BlockItem>` に展開

### Chapter 14 の詳細

Chapter 14 では以下の機能を追加した:

- **ポインタ型**: `int *`, `double *`, `int **` 等の多段ポインタに対応
- **アドレス演算子 (`&`)**: 変数のアドレスを取得
- **間接参照演算子 (`*`)**: ポインタ経由の読み書き
- **ポインタの関数引数・戻り値**: ポインタを関数間で受け渡し
- **ポインタ比較**: `==` / `!=` で同じポインタ型同士を比較
- **null ポインタ**: 整数定数 0 をポインタに代入可能。条件式で真偽判定
- **ポインタ ↔ 整数キャスト**: 明示的キャストで相互変換
- **宣言子パーサー**: `int *x`, `int **pp` 等のポインタ宣言構文を解析
- **左辺値の一般化**: `*ptr = val` 形式の代入に対応

### Chapter 13 の詳細

Chapter 13 では以下の機能を追加した:

- **`double` 型**: IEEE 754 倍精度浮動小数点数に対応
- **浮動小数点リテラル**: 小数点・指数表記に対応
- **SSE 命令による浮動小数点演算**: `addsd`, `subsd`, `mulsd`, `divsd`
- **単項演算子**: 符号反転（`xorpd`）、論理否定（`comisd`）
- **比較演算**: `comisd` 命令 + unsigned 条件コード
- **型変換**: 整数 ↔ `double` の暗黙変換（`cvtsi2sd`, `cvttsd2si`）
- **混合引数の関数呼出規約**: 整数レジスタと XMM レジスタを独立カウント
- **`double` 定数プール**: `.rodata` セクションにビットパターンを配置

### Chapter 12 の詳細

Chapter 12 では以下の機能を追加した:

- **`unsigned int`/`unsigned long` 型**: 符号なし整数型に対応
- **符号なしリテラル**: `U`/`u`, `UL`/`ul`/`LU`/`lu` サフィックス
- **通常算術変換**: 4 型の暗黙変換
- **符号なし除算/剰余**: `div` 命令（`idiv` と区別）
- **符号なし比較**: `seta`/`setae`/`setb`/`setbe` 条件コード
- **ゼロ拡張**: `unsigned int` → `unsigned long` に `movl`（上位32ビット自動クリア）

### Chapter 11 の詳細

- **`long` 型**: 64ビット符号付き整数に対応
- **型検査パス（Validate）の導入**: パースとコード生成の間に型検査を挿入
- **暗黙的型変換**: `int` ↔ `long` の自動変換（`Cast` ノード挿入）
- **型に応じたコード生成**: `movl`/`addl` (32bit) vs `movq`/`addq` (64bit) の選択
- **符号拡張/切り詰め命令**: `movslq` (int→long), `movl` (long→int truncate)

### Chapter 10 の詳細

- **グローバル変数**: ファイルスコープの変数宣言に対応（`.data`/`.bss` セクションに配置）
- **`static` ローカル変数**: 関数呼び出し間で値を保持する
- **`static` 関数**: 内部リンケージ（`.globl` を出力しない）
- **`extern` 宣言**: 外部リンケージの変数参照

### Chapter 9 の詳細

- **関数定義・呼び出し**: 複数関数のプログラムに対応
- **関数プロトタイプ（前方宣言）**: 定義前に宣言できる
- **再帰**: 関数の再帰呼び出しに対応
- **最大6引数のレジスタ渡し**: System V AMD64 ABI 準拠
- **7引数以上のスタック渡し**: 16バイトアラインメント対応
- **可変長引数関数の呼び出し**: `printf`, `sprintf` 等の可変長引数関数を呼び出せる
  - `...` トークンのレキシング、パラメータリスト末尾の `, ...` パース
  - 可変長関数呼び出し時に `%al` に XMM レジスタ引数数をセット（System V ABI 準拠）
  - デフォルト引数昇格（`char` → `int`）
  ```c
  int printf(char *fmt, ...);
  int main(void) {
      printf("%d %.2f %s\n", 42, 3.14, "hello");
      return 0;
  }
  ```
- **可変長引数関数の定義**: `va_list`/`va_start`/`va_arg`/`va_end` によるユーザー定義の可変長引数関数
  - System V AMD64 ABI 準拠の `va_list` 構造体（24バイト: `gp_offset`, `fp_offset`, `overflow_arg_area`, `reg_save_area`）
  - 関数入口で全引数レジスタ（GP 6個 + XMM 8個）を 176バイトのレジスタ保存領域に退避
  - `va_arg` で整数型（`int`, `long`, ポインタ）と浮動小数点型（`double`）の両方に対応
  - レジスタ渡し引数（GP: `gp_offset < 48`、XMM: `fp_offset < 176`）とスタック渡し引数（`overflow_arg_area`）の自動切り替え
  - `#include <stdarg.h>` 不要（コンパイラ組み込み）
  ```c
  int my_sum(int count, ...) {
      va_list ap;
      va_start(ap, count);
      int sum = 0;
      for (int i = 0; i < count; i = i + 1) {
          sum = sum + va_arg(ap, int);
      }
      va_end(ap);
      return sum;
  }
  int main(void) { return my_sum(3, 10, 20, 12); }  // 42
  ```

### Chapter 8 の詳細

- **while / do-while / for ループ**
- **break / continue**

### Chapter 7 の詳細

- **複合代入演算子**: `+=`, `-=`, `*=`, `/=`, `%=`
- **前置/後置インクリメント/デクリメント**: `++a`, `a++`, `--a`, `a--`
- **カンマ演算子**: `expr1, expr2`

## Future Work

書籍 "Writing a C Compiler" の全20章を完了。今後の改善候補:

### 最適化

- [x] **Coalescing（コピー合体）**: `Mov a, b` で a と b が干渉しなければ合体し、Mov を除去する。Briggs 基準（Pseudo-Pseudo）と George 基準（Pseudo-HardReg）で安全性を判定
- [x] **TACKY IR 上の最適化パス強化**: 6パス構成の最適化パイプライン（収束まで最大10回反復）
  - **代数的簡略化（Algebraic Simplifications）**: `x+0→x`, `x*1→x`, `x*0→0`, `x-x→0`, `x/1→x`, `x%1→0` 等の恒等式を簡略化
  - **定数畳み込み（Constant Folding）**: コンパイル時に定数式を評価
  - **到達不能コード除去（Unreachable Code Elimination）**: CFG の BFS で到達不能な基本ブロックを除去
  - **コピー伝播（Copy Propagation）**: `dst = src` のコピーを追跡し、後続の `dst` の使用を `src` に置換
  - **共通部分式除去（CSE）**: 基本ブロック内で同一計算を検出し、Copy に置換
  - **生存解析ベース死コード除去（Liveness DCE）**: 逆方向データフロー解析で生存していない変数への書き込みを除去
- [x] **ポインタ経由の書き込みを考慮したコピー伝播**: Store 時にアドレス取得変数のコピーを無効化

### 言語機能の拡充

- [x] **可変長引数関数の呼び出し**: `printf(fmt, ...)` 等の外部可変長引数関数を呼び出し可能に
- [x] **可変長引数関数の定義**: `va_list`/`va_start`/`va_arg`/`va_end`（System V AMD64 ABI 準拠、int/long/double/ポインタ対応）
- [x] **配列初期化子リスト**: `int arr[3] = {1, 2, 3};`
- [x] **カンマ区切り複数宣言**: `int a = 1, b = 2, c = 3;`
- [x] **switch 文**: `switch`/`case`/`default`（fall-through、ネスト、enum 定数 case ラベル対応）
- [x] **enum 型**: `enum Color { RED, GREEN, BLUE };`（明示値、負値、typedef enum、無名 enum 対応）
- [x] **typedef**: 型エイリアスの定義（`typedef int myint;`, `typedef struct { ... } Point;`, ポインタ・配列・ネスト対応）
- [ ] **プリプロセッサ**: `#include`, `#define`, `#ifdef` 等

### コード難読化（Anti-Reverse Engineering）

コンパイラレベルでの難読化変換。`--fobfuscate` フラグで有効化し、TACKY IR + ASM レベルの計15パスを適用する。

TACKY IR レベル（10パス）:
- [x] **定数の間接化（Constant Encoding）**
- [x] **算術置換（Arithmetic Substitution）**
- [x] **ジャンクコード挿入**
- [x] **不透明述語（Opaque Predicates）多様化**
- [x] **制御フロー平坦化（CFF）+ ジャンプテーブル + 状態エンコード**
- [x] **文字列暗号化**
- [x] **VM仮想化（VM-Based Code Virtualization）**
- [x] **ライブラリ関数難読化（Library Function Obfuscation）**

ASM レベル（5パス）:
- [x] **反逆アセンブリ（Anti-Disassembly）**
- [x] **関数呼び出しの間接化（Indirect Calls）**
- [x] **レジスタシャッフル（Register Shuffle）**
- [x] **スタックフレーム難読化（Stack Frame Obfuscation）**
- [x] **命令置換（Instruction Substitution）**

関数境界攪乱（2パス）:
- [x] **関数インライン展開（Function Inlining）**
- [x] **関数アウトライン化（Function Outlining）**

パラメータ化:
- [x] **難易度レベル制御（`--obf-level=1..4`）**
- [x] **個別パス制御**: `--obf-no-cff`, `--obf-no-strings`, `--obf-no-vm-virtualize` 等で各パスを個別に無効化
- [x] **頻度パラメータ**: `--obf-junk-freq=N`, `--obf-pred-freq=N` 等で頻度を調整

### ベンチマーク・評価

- [x] **難読化ベンチマークスイート**: 10本のCプログラム × 5難読化レベル = 50バイナリの自動生成・検証
- [ ] **デオブフスケーター定量評価**: D-810, SATURN 等での復元成功率測定

### コード品質

- [x] **コンパイラ警告の解消**: 未使用変数・インポートを整理し0件に（19件 → 0件）
- [ ] **E2E テストスイートの充実**: 各章の機能を網羅する統合テストの追加

## License

MIT License. See [LICENSE](LICENSE).
