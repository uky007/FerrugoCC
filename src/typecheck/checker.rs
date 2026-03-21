//! 型検査ロジック（Chapter 11, 12, 13, 14, 15, 16, 17, 18）
//!
//! AST を in-place で変換し、以下を行う:
//! 1. シンボルテーブル構築: 変数と関数の型情報を記録
//! 2. 式の型推論: 各式の型を決定
//! 3. 暗黙的型変換の明示化: `Cast` ノードを挿入
//! 4. 定数の型解決: `Constant(v)` で v が i32 範囲外 → `ConstantLong(v)` に変換
//!
//! # Chapter 12: 符号なし整数
//! - `ConstantUInt`/`ConstantULong` の型推論（u32 範囲外 → ULong に昇格）
//! - 通常算術変換（usual arithmetic conversions）で 4 型を統一
//! - `Cast` に `source_type` を追加し、符号拡張/ゼロ拡張を区別
//!
//! # Chapter 13: 浮動小数点数
//! - `ConstantDouble` → `Type::Double`
//! - `common_type()` で `Double` が最上位（どちらかが `Double` なら `Double`）
//! - ビット反転 (`~`) / 剰余 (`%`) を `Double` に対してエラー
//!
//! # Chapter 14: ポインタ
//! - `Dereref(inner)`: inner が `Pointer(target)` でなければエラー → `target` 型
//! - `AddrOf(inner)`: inner が左辺値でなければエラー → `Pointer(inner の型)`
//! - 左辺値チェック: `Var` / `Dereref` のみが左辺値（代入・`++`/`--` の対象）
//! - null ポインタ定数: 整数定数 0 は任意のポインタ型に暗黙変換可能
//! - ポインタ比較: `==` / `!=` のみ（同じポインタ型同士）
//! - 禁止操作: `~ptr`, `-ptr`, `ptr % n` → TypeError
//! - 禁止キャスト: `double` ↔ ポインタ → TypeError
//!
//! # Chapter 15: 配列とポインタ算術
//! - 配列→ポインタ減衰（decay）: `Var` が配列型ならポインタ型に自動変換
//! - ポインタ算術: `ptr + int`（スケーリング）、`ptr - ptr`（要素数差分）
//! - ポインタ比較拡張: `<`, `<=`, `>`, `>=` を同じポインタ型同士で許可
//! - `sizeof` 解決: `SizeOfType`/`SizeOfExpr` を `ConstantULong(size)` に置換
//! - 配列への代入・初期化禁止
//! - `AddrOf` 配列特別処理: `&arr` → 先頭要素へのポインタ
//!
//! # Chapter 17: void 型と void ポインタ
//! - `void` 変数宣言の禁止（不完全型）
//! - `sizeof(void)` / `sizeof(void式)` の禁止
//! - void 関数の return チェック: `return;` のみ許可、`return expr;` はエラー
//! - 非 void 関数の `return;` はエラー
//! - `void *` ↔ 任意のポインタ型の暗黙変換（代入・引数・return・ternary）
//! - `void *` と他ポインタの `==`/`!=` 比較を許可
//! - void 式の制約: 条件式・算術・論理演算のオペランドに使用禁止
//! - void ポインタ算術の禁止（`void *p + 1` 等）
//! - void ポインタの間接参照禁止（`*void_ptr`）
//! - `(void)expr` キャスト: 式の値を捨てる
//! - ternary: 両辺 void → void、片方のみ void → エラー
//!
//! # Chapter 18: 構造体
//! - `Dot(inner, member)`: inner が `Struct` でなければエラー → メンバの型を返す
//! - `CompoundInit`: 式コンテキストではエラー（宣言時のみ使用可能）
//! - 左辺値拡張: `Dot(inner, _)` は inner が左辺値なら左辺値
//! - 構造体代入: 同じタグ名の構造体同士のみ代入可能（バイト単位コピー）
//! - 複合初期化子: メンバ数一致・各メンバの型互換チェック（暗黙キャスト挿入）
//! - 不完全型チェック: 前方宣言のみの構造体の変数宣言・sizeof を禁止
//! - 構造体への算術・比較演算は全てエラー

use std::collections::HashMap;

use crate::error::{CompileError, Result};
use crate::parse::ast::{
    BinaryOp, BlockItem, Declaration, Expr, ForInit, FunctionDecl, Program, Statement,
    TopLevelDecl, Type, UnaryOp, struct_member_offset_ex,
};

/// シンボルの型情報。
#[derive(Debug, Clone)]
enum SymbolType {
    Variable(Type),
    Function {
        return_type: Type,
        /// `None` = 引数未指定 `()` — 任意の引数で呼び出し可能
        /// `Some(vec![])` = 引数ゼロ `(void)` — 引数なし
        /// `Some(vec![...])` = 具体的なプロトタイプ
        param_types: Option<Vec<Type>>,
        is_variadic: bool,
    },
}

/// 型検査のエントリポイント。AST を in-place で変換する。
pub fn typecheck(program: &mut Program) -> Result<()> {
    let mut symbols: HashMap<String, SymbolType> = HashMap::new();

    for decl in &mut program.declarations {
        match decl {
            TopLevelDecl::Function(func_decl) => {
                typecheck_function_decl(func_decl, &mut symbols)?;
            }
            TopLevelDecl::Variable(var_decl) => {
                typecheck_file_scope_var(var_decl, &mut symbols)?;
            }
            TopLevelDecl::Typedef { .. } => {
                // typedef はパース時に解決済み。型チェック不要。
            }
        }
    }

    Ok(())
}

/// `char s[] = "hello"` / `char s[10] = "hello"` — 文字列リテラルを char 配列初期化子に変換。
/// サイズ 0 の場合は文字列長+1（null終端含む）から推論する。
fn convert_string_to_char_array(decl: &mut Declaration) -> Result<()> {
    if let Some(Expr::StringLiteral(s)) = &decl.init
        && let Type::Array(ref elem_type, count) = decl.var_type
        && matches!(**elem_type, Type::Char | Type::UChar)
    {
        let bytes: Vec<u8> = s.bytes().collect();
        let str_len = bytes.len() + 1; // null terminator 含む
        let array_size = if count == 0 { str_len } else { count };
        if count > 0 && bytes.len() > count {
            return Err(CompileError::TypeError(format!(
                "initializer-string for '{}' is too long ({} chars for array of {})",
                decl.name,
                bytes.len(),
                count
            )));
        }
        decl.var_type = Type::Array(elem_type.clone(), array_size);
        let mut chars: Vec<Expr> = bytes
            .iter()
            .take(array_size)
            .map(|&b| Expr::Constant(b as i64))
            .collect();
        while chars.len() < array_size {
            chars.push(Expr::Constant(0));
        }
        decl.init = Some(Expr::CompoundInit(chars));
    }
    Ok(())
}

/// ファイルスコープ変数の型検査。
fn typecheck_file_scope_var(
    decl: &mut Declaration,
    symbols: &mut HashMap<String, SymbolType>,
) -> Result<()> {
    // 構造体定義のみ（変数名なし）の場合はスキップ（Chapter 18）
    if decl.name.is_empty() {
        return Ok(());
    }

    // 非オブジェクト型の変数宣言は禁止（void, 関数型, 不完全構造体）
    if !decl.var_type.is_object_type() {
        return Err(CompileError::TypeError(format!(
            "variable '{}' has non-object type '{:?}'",
            decl.name, decl.var_type
        )));
    }

    // char s[] = "hello" / char s[10] = "hello": 文字列→char配列変換
    convert_string_to_char_array(decl)?;

    symbols.insert(
        decl.name.clone(),
        SymbolType::Variable(decl.var_type.clone()),
    );

    if let Some(init) = &mut decl.init {
        resolve_constant(init);
    }

    Ok(())
}

/// 関数宣言/定義の型検査。
fn typecheck_function_decl(
    func: &mut FunctionDecl,
    symbols: &mut HashMap<String, SymbolType>,
) -> Result<()> {
    let param_types_vec: Vec<Type> = func.params.iter().map(|(t, _)| t.clone()).collect();
    let param_types: Option<Vec<Type>> = if func.has_prototype {
        Some(param_types_vec)
    } else {
        None
    };

    // 既存の宣言との互換性チェック
    // 一方が引数未指定 (None) の場合はプロトタイプ側で上書きを許容
    if let Some(existing) = symbols.get(&func.name)
        && let SymbolType::Function {
            return_type,
            param_types: existing_params,
            is_variadic,
        } = existing
    {
        let conflict = *return_type != func.return_type
            || *is_variadic != func.is_variadic
            || match (existing_params, &param_types) {
                (None, _) | (_, &None) => false, // 一方が未指定なら互換
                (Some(a), Some(b)) => a != b,
            };
        if conflict {
            return Err(CompileError::TypeError(format!(
                "conflicting types for function '{}'",
                func.name
            )));
        }
    }

    // プロトタイプ情報がある方で上書き（None → Some への昇格）
    let should_update = match symbols.get(&func.name) {
        Some(SymbolType::Function {
            param_types: existing_params,
            ..
        }) => existing_params.is_none() && param_types.is_some(),
        _ => true,
    };
    if should_update {
        symbols.insert(
            func.name.clone(),
            SymbolType::Function {
                return_type: func.return_type.clone(),
                param_types: param_types.clone(),
                is_variadic: func.is_variadic,
            },
        );
    }

    if let Some(body) = &mut func.body {
        // パラメータをローカルシンボルに追加
        let mut local_symbols = symbols.clone();
        for (param_type, param_name) in &func.params {
            local_symbols.insert(param_name.clone(), SymbolType::Variable(param_type.clone()));
        }

        for item in body.iter_mut() {
            typecheck_block_item(item, &mut local_symbols, &func.return_type)?;
        }
    }

    Ok(())
}

/// ブロック要素の型検査。
fn typecheck_block_item(
    item: &mut BlockItem,
    symbols: &mut HashMap<String, SymbolType>,
    return_type: &Type,
) -> Result<()> {
    match item {
        BlockItem::Statement(stmt) => typecheck_statement(stmt, symbols, return_type),
        BlockItem::Declaration(decl) => typecheck_local_declaration(decl, symbols),
        BlockItem::Typedef { .. } => Ok(()), // パース時に解決済み
    }
}

/// ローカル変数宣言の型検査。
fn typecheck_local_declaration(
    decl: &mut Declaration,
    symbols: &mut HashMap<String, SymbolType>,
) -> Result<()> {
    // 構造体定義のみ（変数名なし）の場合はスキップ（Chapter 18）
    if decl.name.is_empty() {
        return Ok(());
    }

    // 非オブジェクト型の変数宣言は禁止（void, 関数型, 不完全構造体）
    if !decl.var_type.is_object_type() {
        return Err(CompileError::TypeError(format!(
            "variable '{}' has non-object type '{:?}'",
            decl.name, decl.var_type
        )));
    }

    // char s[] = "hello" / char s[10] = "hello": 文字列→char配列変換
    convert_string_to_char_array(decl)?;

    symbols.insert(
        decl.name.clone(),
        SymbolType::Variable(decl.var_type.clone()),
    );

    if let Some(init) = &mut decl.init {
        // Chapter 18: 複合初期化子の検査
        if let Expr::CompoundInit(inits) = init {
            if let Type::Struct { ref members, .. } = decl.var_type {
                if inits.len() != members.len() {
                    return Err(CompileError::TypeError(format!(
                        "wrong number of initializers for struct (expected {}, got {})",
                        members.len(),
                        inits.len()
                    )));
                }
                for (init_expr, member) in inits.iter_mut().zip(members.iter()) {
                    let init_type = typecheck_expr(init_expr, symbols)?;
                    if init_type != member.member_type {
                        let old = std::mem::replace(init_expr, Expr::Constant(0));
                        *init_expr = Expr::Cast {
                            target_type: member.member_type.clone(),
                            source_type: init_type,
                            expr: Box::new(old),
                        };
                    }
                }
                return Ok(());
            } else if let Type::Array(_, count) = &decl.var_type {
                // 暗黙の配列サイズ: int arr[] = {1,2,3} → arr[3]
                let count = *count;
                if count == 0
                    && !inits.is_empty()
                    && let Type::Array(ref elem_type, _) = decl.var_type
                {
                    let et = (**elem_type).clone();
                    decl.var_type = Type::Array(Box::new(et), inits.len());
                }
                let (elem_type, count) = match &decl.var_type {
                    Type::Array(e, c) => (e.clone(), *c),
                    _ => unreachable!(),
                };
                // 配列初期化子リスト: 要素数チェック + 各要素の型チェック
                if inits.len() > count {
                    return Err(CompileError::TypeError(format!(
                        "too many initializers for array (expected at most {}, got {})",
                        count,
                        inits.len()
                    )));
                }
                for init_expr in inits.iter_mut() {
                    // struct 配列の要素が CompoundInit なら struct 初期化として検査
                    if let Expr::CompoundInit(sub_inits) = init_expr
                        && let Type::Struct { ref members, .. } = *elem_type
                    {
                        for (sub_init, member) in sub_inits.iter_mut().zip(members.iter()) {
                            let sub_type = typecheck_expr(sub_init, symbols)?;
                            if sub_type != member.member_type {
                                let old = std::mem::replace(sub_init, Expr::Constant(0));
                                *sub_init = Expr::Cast {
                                    target_type: member.member_type.clone(),
                                    source_type: sub_type,
                                    expr: Box::new(old),
                                };
                            }
                        }
                        continue;
                    }
                    let init_type = typecheck_expr(init_expr, symbols)?;
                    if init_type != *elem_type {
                        let old = std::mem::replace(init_expr, Expr::Constant(0));
                        *init_expr = Expr::Cast {
                            target_type: (*elem_type).clone(),
                            source_type: init_type,
                            expr: Box::new(old),
                        };
                    }
                }
                return Ok(());
            } else {
                return Err(CompileError::TypeError(
                    "compound initializer used with non-struct/non-array type".to_string(),
                ));
            }
        }

        let init_type = typecheck_expr(init, symbols)?;

        // 構造体同士の代入: 同じタグ名の構造体のみ
        if decl.var_type.is_struct() && init_type.is_struct() {
            if decl.var_type != init_type {
                return Err(CompileError::TypeError(
                    "incompatible struct types in initialization".to_string(),
                ));
            }
            return Ok(());
        }

        // 右辺の型が左辺と異なる場合、キャストを挿入
        if init_type != decl.var_type {
            // 構造体への非構造体代入は禁止
            if decl.var_type.is_struct() || init_type.is_struct() {
                return Err(CompileError::TypeError(
                    "incompatible types in initialization".to_string(),
                ));
            }
            // void ポインタの暗黙変換: void* ↔ 任意のポインタ型
            if decl.var_type.is_pointer()
                && !init_type.is_pointer()
                && !is_null_pointer_constant(init)
            {
                return Err(CompileError::TypeError(
                    "cannot initialize pointer with non-pointer non-null value".to_string(),
                ));
            }
            // ポインタ同士だが型が異なる場合: void* が関与する場合のみ許可
            if decl.var_type.is_pointer()
                && init_type.is_pointer()
                && decl.var_type != init_type
                && !is_void_pointer(&decl.var_type)
                && !is_void_pointer(&init_type)
            {
                return Err(CompileError::TypeError(
                    "incompatible pointer types in initialization".to_string(),
                ));
            }
            let old_init = std::mem::replace(init, Expr::Constant(0)); // placeholder
            *init = Expr::Cast {
                target_type: decl.var_type.clone(),
                source_type: init_type,
                expr: Box::new(old_init),
            };
        }
    }

    Ok(())
}

/// 文の型検査。
fn typecheck_statement(
    stmt: &mut Statement,
    symbols: &mut HashMap<String, SymbolType>,
    return_type: &Type,
) -> Result<()> {
    match stmt {
        Statement::Return(opt_expr) => {
            match opt_expr {
                None => {
                    // `return;` — only allowed in void functions
                    if !return_type.is_void() {
                        return Err(CompileError::TypeError(
                            "return with no value in non-void function".to_string(),
                        ));
                    }
                    Ok(())
                }
                Some(expr) => {
                    // `return expr;` — not allowed in void functions
                    if return_type.is_void() {
                        return Err(CompileError::TypeError(
                            "return with a value in void function".to_string(),
                        ));
                    }
                    let expr_type = typecheck_expr(expr, symbols)?;
                    if expr_type != *return_type {
                        // void ポインタ暗黙変換チェック
                        if return_type.is_pointer()
                            && expr_type.is_pointer()
                            && *return_type != expr_type
                            && !is_void_pointer(return_type)
                            && !is_void_pointer(&expr_type)
                        {
                            return Err(CompileError::TypeError(
                                "incompatible pointer types in return".to_string(),
                            ));
                        }
                        let old_expr = std::mem::replace(expr, Expr::Constant(0));
                        *expr = Expr::Cast {
                            target_type: return_type.clone(),
                            source_type: expr_type,
                            expr: Box::new(old_expr),
                        };
                    }
                    Ok(())
                }
            }
        }
        Statement::Expression(expr) => {
            typecheck_expr(expr, symbols)?;
            Ok(())
        }
        Statement::Null => Ok(()),
        Statement::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let cond_type = typecheck_expr(condition, symbols)?;
            if cond_type.is_void() {
                return Err(CompileError::TypeError(
                    "void expression used as condition".to_string(),
                ));
            }
            typecheck_statement(then_branch, symbols, return_type)?;
            if let Some(else_stmt) = else_branch {
                typecheck_statement(else_stmt, symbols, return_type)?;
            }
            Ok(())
        }
        Statement::Compound(items) => {
            let mut inner_symbols = symbols.clone();
            for item in items.iter_mut() {
                typecheck_block_item(item, &mut inner_symbols, return_type)?;
            }
            Ok(())
        }
        Statement::While { condition, body } => {
            let cond_type = typecheck_expr(condition, symbols)?;
            if cond_type.is_void() {
                return Err(CompileError::TypeError(
                    "void expression used as condition".to_string(),
                ));
            }
            typecheck_statement(body, symbols, return_type)
        }
        Statement::DoWhile { body, condition } => {
            typecheck_statement(body, symbols, return_type)?;
            let cond_type = typecheck_expr(condition, symbols)?;
            if cond_type.is_void() {
                return Err(CompileError::TypeError(
                    "void expression used as condition".to_string(),
                ));
            }
            Ok(())
        }
        Statement::For {
            init,
            condition,
            post,
            body,
        } => {
            let mut inner_symbols = symbols.clone();
            match init.as_mut() {
                ForInit::Declaration(decls) => {
                    for decl in decls.iter_mut() {
                        typecheck_local_declaration(decl, &mut inner_symbols)?;
                    }
                }
                ForInit::Expression(Some(expr)) => {
                    typecheck_expr(expr, &inner_symbols)?;
                }
                ForInit::Expression(None) => {}
            }
            if let Some(cond) = condition {
                let cond_type = typecheck_expr(cond, &inner_symbols)?;
                if cond_type.is_void() {
                    return Err(CompileError::TypeError(
                        "void expression used as condition".to_string(),
                    ));
                }
            }
            if let Some(post_expr) = post {
                typecheck_expr(post_expr, &inner_symbols)?;
            }
            typecheck_statement(body, &mut inner_symbols, return_type)
        }
        Statement::Break | Statement::Continue => Ok(()),
        Statement::Switch { expr, body } => {
            let expr_type = typecheck_expr(expr, symbols)?;
            // switch 式は整数型のみ（void/struct/double/pointer は不可）
            if expr_type.is_void()
                || expr_type.is_struct()
                || expr_type.is_double()
                || expr_type.is_pointer()
            {
                return Err(CompileError::TypeError(
                    "switch expression must have integer type".to_string(),
                ));
            }
            typecheck_statement(body, symbols, return_type)
        }
        Statement::Case { body, .. } => typecheck_statement(body, symbols, return_type),
        Statement::Default(body) => typecheck_statement(body, symbols, return_type),
        Statement::Goto(_) => Ok(()),
        Statement::Label { body, .. } => typecheck_statement(body, symbols, return_type),
    }
}

/// 式の型検査。型を推論し、必要に応じて Cast ノードを挿入する。
/// 返り値は式の結果の型。
fn typecheck_expr(expr: &mut Expr, symbols: &HashMap<String, SymbolType>) -> Result<Type> {
    match expr {
        Expr::Constant(v) => {
            // i32 に収まらなければ ConstantLong に変換
            if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                let val = *v;
                *expr = Expr::ConstantLong(val);
                Ok(Type::Long)
            } else {
                Ok(Type::Int)
            }
        }
        Expr::ConstantLong(_) => Ok(Type::Long),

        Expr::ConstantUInt(v) => {
            // u32 範囲外なら ULong に昇格
            if *v > u32::MAX as u64 {
                let val = *v;
                *expr = Expr::ConstantULong(val);
                Ok(Type::ULong)
            } else {
                Ok(Type::UInt)
            }
        }
        Expr::ConstantULong(_) => Ok(Type::ULong),

        Expr::ConstantDouble(_) => Ok(Type::Double),

        Expr::Cast {
            target_type,
            source_type,
            expr: inner,
        } => {
            let actual_source = typecheck_expr(inner, symbols)?;
            *source_type = actual_source.clone();
            // (void)expr — 値を捨てるキャスト（常に許可）
            if target_type.is_void() {
                return Ok(Type::Void);
            }
            // void 式からのキャストは禁止（void → int 等）
            if actual_source.is_void() {
                return Err(CompileError::TypeError(
                    "cannot cast void expression to non-void type".to_string(),
                ));
            }
            // ポインタ ↔ double のキャストは禁止
            if target_type.is_pointer() && actual_source == Type::Double {
                return Err(CompileError::TypeError(
                    "cannot cast 'double' to pointer type".to_string(),
                ));
            }
            if target_type.is_double() && actual_source.is_pointer() {
                return Err(CompileError::TypeError(
                    "cannot cast pointer type to 'double'".to_string(),
                ));
            }
            Ok(target_type.clone())
        }

        Expr::Var(name) => {
            match symbols.get(name) {
                Some(SymbolType::Variable(t)) => {
                    // 配列→ポインタ減衰（Chapter 15）
                    Ok(array_decay(t.clone()))
                }
                Some(SymbolType::Function {
                    return_type,
                    param_types,
                    is_variadic,
                }) => {
                    // Function name used as value → decays to function pointer with prototype
                    Ok(Type::Pointer(Box::new(Type::Function {
                        return_type: Box::new(return_type.clone()),
                        param_types: param_types.clone(),
                        is_variadic: *is_variadic,
                    })))
                }
                None if name == "__func__" || name == "__FUNCTION__" => {
                    // C99 predefined identifier: const char[]
                    Ok(Type::Pointer(Box::new(Type::Char)))
                }
                None => Err(CompileError::TypeError(format!(
                    "undeclared variable '{}'",
                    name
                ))),
            }
        }

        Expr::Assign(lhs, rhs) => {
            let lhs_type = typecheck_expr(lhs, symbols)?;
            if !is_lvalue(lhs) {
                return Err(CompileError::TypeError(
                    "left side of assignment must be an lvalue".to_string(),
                ));
            }
            // 配列への代入は禁止（Chapter 15）
            if is_array_var(lhs, symbols) {
                return Err(CompileError::TypeError(
                    "cannot assign to array variable".to_string(),
                ));
            }
            let rhs_type = typecheck_expr(rhs, symbols)?;
            // void 式を代入の右辺に使用禁止
            if rhs_type.is_void() {
                return Err(CompileError::TypeError(
                    "void expression used as right-hand side of assignment".to_string(),
                ));
            }
            // 構造体同士の代入: 同じタグ名のみ（Chapter 18）
            if lhs_type.is_struct() || rhs_type.is_struct() {
                if lhs_type != rhs_type {
                    return Err(CompileError::TypeError(
                        "incompatible struct types in assignment".to_string(),
                    ));
                }
                return Ok(lhs_type);
            }
            if rhs_type != lhs_type {
                // void ポインタ暗黙変換チェック
                if lhs_type.is_pointer()
                    && rhs_type.is_pointer()
                    && lhs_type != rhs_type
                    && !is_void_pointer(&lhs_type)
                    && !is_void_pointer(&rhs_type)
                {
                    return Err(CompileError::TypeError(
                        "incompatible pointer types in assignment".to_string(),
                    ));
                }
                let old_rhs = std::mem::replace(rhs.as_mut(), Expr::Constant(0));
                **rhs = Expr::Cast {
                    target_type: lhs_type.clone(),
                    source_type: rhs_type,
                    expr: Box::new(old_rhs),
                };
            }
            Ok(lhs_type)
        }

        Expr::CompoundAssign(op, lhs, rhs) => {
            let lhs_type = typecheck_expr(lhs, symbols)?;
            if !is_lvalue(lhs) {
                return Err(CompileError::TypeError(
                    "left side of compound assignment must be an lvalue".to_string(),
                ));
            }
            // 配列への複合代入は禁止（Chapter 15）
            if is_array_var(lhs, symbols) {
                return Err(CompileError::TypeError(
                    "cannot assign to array variable".to_string(),
                ));
            }
            let rhs_type = typecheck_expr(rhs, symbols)?;
            // ポインタ算術の複合代入: ptr += int, ptr -= int（Chapter 15）
            // void ポインタ算術は禁止
            if lhs_type.is_pointer()
                && is_void_pointer(&lhs_type)
                && matches!(op, BinaryOp::Add | BinaryOp::Subtract)
            {
                return Err(CompileError::TypeError(
                    "pointer arithmetic on void pointer".to_string(),
                ));
            }
            if lhs_type.is_pointer() && matches!(op, BinaryOp::Add | BinaryOp::Subtract) {
                // 右辺を Long にキャスト
                if rhs_type != Type::Long {
                    let old_rhs = std::mem::replace(rhs.as_mut(), Expr::Constant(0));
                    **rhs = Expr::Cast {
                        target_type: Type::Long,
                        source_type: rhs_type,
                        expr: Box::new(old_rhs),
                    };
                }
                Ok(lhs_type)
            } else if lhs_type.is_pointer() {
                Err(CompileError::TypeError(format!(
                    "compound assignment '{:?}' cannot be applied to pointer types",
                    op
                )))
            } else {
                let _common = common_type(&lhs_type, &rhs_type);
                Ok(lhs_type)
            }
        }

        Expr::PostfixIncrement(inner) | Expr::PostfixDecrement(inner) => {
            let inner_type = typecheck_expr(inner, symbols)?;
            if !is_lvalue(inner) {
                return Err(CompileError::TypeError(
                    "operand of postfix increment/decrement must be an lvalue".to_string(),
                ));
            }
            // 配列への ++/-- は禁止（Chapter 15）
            if is_array_var(inner, symbols) {
                return Err(CompileError::TypeError(
                    "cannot increment/decrement array variable".to_string(),
                ));
            }
            Ok(inner_type)
        }

        Expr::Unary(op, inner) => {
            let inner_type = typecheck_expr(inner, symbols)?;
            match op {
                UnaryOp::Not => {
                    // void 式に `!` は禁止
                    if inner_type.is_void() {
                        return Err(CompileError::TypeError(
                            "logical not '!' cannot be applied to void expression".to_string(),
                        ));
                    }
                    // `!` は常に Int を返す（ポインタも許可: null なら 1、非 null なら 0）
                    Ok(Type::Int)
                }
                UnaryOp::Complement => {
                    if inner_type.is_void() {
                        return Err(CompileError::TypeError(
                            "bitwise complement '~' cannot be applied to void expression"
                                .to_string(),
                        ));
                    }
                    if inner_type == Type::Double {
                        return Err(CompileError::TypeError(
                            "bitwise complement '~' cannot be applied to double".to_string(),
                        ));
                    }
                    if inner_type.is_pointer() {
                        return Err(CompileError::TypeError(
                            "bitwise complement '~' cannot be applied to pointer type".to_string(),
                        ));
                    }
                    // Integer promotion: Char/UChar → Int（Chapter 16）
                    if inner_type.is_character() {
                        let old = std::mem::replace(inner.as_mut(), Expr::Constant(0));
                        **inner = Expr::Cast {
                            target_type: Type::Int,
                            source_type: inner_type,
                            expr: Box::new(old),
                        };
                        Ok(Type::Int)
                    } else {
                        Ok(inner_type)
                    }
                }
                UnaryOp::Negate => {
                    if inner_type.is_void() {
                        return Err(CompileError::TypeError(
                            "unary negation '-' cannot be applied to void expression".to_string(),
                        ));
                    }
                    if inner_type.is_pointer() {
                        return Err(CompileError::TypeError(
                            "unary negation '-' cannot be applied to pointer type".to_string(),
                        ));
                    }
                    if inner_type.is_struct() {
                        return Err(CompileError::TypeError(
                            "unary negation '-' cannot be applied to struct type".to_string(),
                        ));
                    }
                    // Integer promotion: Char/UChar → Int（Chapter 16）
                    if inner_type.is_character() {
                        let old = std::mem::replace(inner.as_mut(), Expr::Constant(0));
                        **inner = Expr::Cast {
                            target_type: Type::Int,
                            source_type: inner_type,
                            expr: Box::new(old),
                        };
                        Ok(Type::Int)
                    } else {
                        Ok(inner_type)
                    }
                }
                UnaryOp::PreIncrement | UnaryOp::PreDecrement => {
                    if !is_lvalue(inner) {
                        return Err(CompileError::TypeError(
                            "operand of prefix increment/decrement must be an lvalue".to_string(),
                        ));
                    }
                    // 配列への ++/-- は禁止（Chapter 15）
                    if is_array_var(inner, symbols) {
                        return Err(CompileError::TypeError(
                            "cannot increment/decrement array variable".to_string(),
                        ));
                    }
                    Ok(inner_type)
                }
            }
        }

        Expr::Binary(op, left, right) => {
            let left_type = typecheck_expr(left, symbols)?;
            let right_type = typecheck_expr(right, symbols)?;

            match op {
                // 論理演算子: オペランドは共通型に昇格するが結果は Int
                // ポインタも使用可能（非 null = 真）
                // void 式は禁止
                BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
                    if left_type.is_void() || right_type.is_void() {
                        return Err(CompileError::TypeError(
                            "void expression used as operand of logical operator".to_string(),
                        ));
                    }
                    Ok(Type::Int)
                }

                // 比較演算子: ポインタ同士の比較も対応（Chapter 15 で全比較演算子に拡張）
                BinaryOp::LessThan
                | BinaryOp::LessEqual
                | BinaryOp::GreaterThan
                | BinaryOp::GreaterEqual
                | BinaryOp::Equal
                | BinaryOp::NotEqual => {
                    if left_type.is_void() || right_type.is_void() {
                        return Err(CompileError::TypeError(
                            "void expression used as operand of comparison".to_string(),
                        ));
                    }
                    if left_type.is_pointer() || right_type.is_pointer() {
                        // 両方とも同じポインタ型であること
                        // ただし null ポインタ定数（整数 0）は任意のポインタ型と比較可能
                        // void* と他のポインタ型の == / != 比較も許可
                        if left_type.is_pointer() && right_type.is_pointer() {
                            if left_type != right_type {
                                // void* が関与する場合は == / != のみ許可
                                if is_void_pointer(&left_type) || is_void_pointer(&right_type) {
                                    if !matches!(op, BinaryOp::Equal | BinaryOp::NotEqual) {
                                        return Err(CompileError::TypeError(
                                            "ordered comparison of void pointer with other pointer type".to_string()
                                        ));
                                    }
                                    // void* 比較 OK — キャストして揃える
                                    // 共通型は void*
                                    if !is_void_pointer(&left_type) {
                                        convert_operand(left, &left_type, &right_type);
                                    } else if !is_void_pointer(&right_type) {
                                        convert_operand(right, &right_type, &left_type);
                                    }
                                } else {
                                    return Err(CompileError::TypeError(
                                        "comparison between incompatible pointer types".to_string(),
                                    ));
                                }
                            }
                        } else if left_type.is_pointer() && !right_type.is_pointer() {
                            // null ポインタ定数 (0) を許可
                            if !is_null_pointer_constant(right) {
                                return Err(CompileError::TypeError(
                                    "comparison between pointer and non-zero integer".to_string(),
                                ));
                            }
                            // 整数 0 をポインタ型にキャスト
                            convert_operand(right, &right_type, &left_type);
                        } else {
                            // right is pointer, left is not
                            if !is_null_pointer_constant(left) {
                                return Err(CompileError::TypeError(
                                    "comparison between pointer and non-zero integer".to_string(),
                                ));
                            }
                            convert_operand(left, &left_type, &right_type);
                        }
                        Ok(Type::Int)
                    } else {
                        let common = common_type(&left_type, &right_type);
                        convert_operand(left, &left_type, &common);
                        convert_operand(right, &right_type, &common);
                        Ok(Type::Int)
                    }
                }

                // 加算: ポインタ算術対応（Chapter 15）
                BinaryOp::Add => {
                    if left_type.is_void() || right_type.is_void() {
                        return Err(CompileError::TypeError(
                            "void expression used as operand of addition".to_string(),
                        ));
                    }
                    if left_type.is_pointer() && right_type.is_pointer() {
                        return Err(CompileError::TypeError(
                            "cannot add two pointers".to_string(),
                        ));
                    }
                    if left_type.is_pointer() && !right_type.is_pointer() {
                        // void ポインタ算術は禁止
                        if is_void_pointer(&left_type) {
                            return Err(CompileError::TypeError(
                                "pointer arithmetic on void pointer".to_string(),
                            ));
                        }
                        // ptr + int: 右辺を Long にキャスト
                        if right_type != Type::Long {
                            convert_operand(right, &right_type, &Type::Long);
                        }
                        Ok(left_type)
                    } else if !left_type.is_pointer() && right_type.is_pointer() {
                        // void ポインタ算術は禁止
                        if is_void_pointer(&right_type) {
                            return Err(CompileError::TypeError(
                                "pointer arithmetic on void pointer".to_string(),
                            ));
                        }
                        // int + ptr: operand をスワップして正規化
                        // AST ノードを入れ替え: left と right をスワップ
                        std::mem::swap(left, right);
                        // left が今ポインタ、right が今整数
                        let int_type = left_type.clone(); // 元の left（今の right）の型
                        if int_type != Type::Long {
                            convert_operand(right, &int_type, &Type::Long);
                        }
                        Ok(right_type) // 元の right（今の left）の型 = ポインタ型
                    } else {
                        // 通常の算術加算
                        let common = common_type(&left_type, &right_type);
                        convert_operand(left, &left_type, &common);
                        convert_operand(right, &right_type, &common);
                        Ok(common)
                    }
                }

                // 減算: ポインタ算術対応（Chapter 15）
                BinaryOp::Subtract => {
                    if left_type.is_void() || right_type.is_void() {
                        return Err(CompileError::TypeError(
                            "void expression used as operand of subtraction".to_string(),
                        ));
                    }
                    if left_type.is_pointer() && right_type.is_pointer() {
                        // void ポインタ同士の減算は禁止
                        if is_void_pointer(&left_type) || is_void_pointer(&right_type) {
                            return Err(CompileError::TypeError(
                                "pointer arithmetic on void pointer".to_string(),
                            ));
                        }
                        // ptr - ptr: 同じポインタ型でなければエラー
                        if left_type != right_type {
                            return Err(CompileError::TypeError(
                                "subtraction between incompatible pointer types".to_string(),
                            ));
                        }
                        Ok(Type::Long) // 結果は要素数差分（Long）
                    } else if left_type.is_pointer() && !right_type.is_pointer() {
                        // void ポインタ算術は禁止
                        if is_void_pointer(&left_type) {
                            return Err(CompileError::TypeError(
                                "pointer arithmetic on void pointer".to_string(),
                            ));
                        }
                        // ptr - int: 右辺を Long にキャスト
                        if right_type != Type::Long {
                            convert_operand(right, &right_type, &Type::Long);
                        }
                        Ok(left_type)
                    } else if !left_type.is_pointer() && right_type.is_pointer() {
                        // int - ptr: エラー
                        Err(CompileError::TypeError(
                            "cannot subtract pointer from integer".to_string(),
                        ))
                    } else {
                        // 通常の算術減算
                        let common = common_type(&left_type, &right_type);
                        convert_operand(left, &left_type, &common);
                        convert_operand(right, &right_type, &common);
                        Ok(common)
                    }
                }

                // 乗算・除算・剰余: ポインタ禁止、void 禁止
                BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Remainder => {
                    if left_type.is_void() || right_type.is_void() {
                        return Err(CompileError::TypeError(
                            "void expression used as operand of arithmetic".to_string(),
                        ));
                    }
                    if left_type.is_pointer() || right_type.is_pointer() {
                        return Err(CompileError::TypeError(format!(
                            "arithmetic operator '{:?}' cannot be applied to pointer types",
                            op
                        )));
                    }
                    let common = common_type(&left_type, &right_type);
                    if matches!(op, BinaryOp::Remainder) && common == Type::Double {
                        return Err(CompileError::TypeError(
                            "remainder '%' cannot be applied to double".to_string(),
                        ));
                    }
                    convert_operand(left, &left_type, &common);
                    convert_operand(right, &right_type, &common);
                    Ok(common)
                }

                // ビット演算 (& | ^): ポインタ禁止、void 禁止、double 禁止
                BinaryOp::BitwiseAnd | BinaryOp::BitwiseOr | BinaryOp::BitwiseXor => {
                    if left_type.is_void() || right_type.is_void() {
                        return Err(CompileError::TypeError(
                            "void expression used as operand of bitwise operation".to_string(),
                        ));
                    }
                    if left_type.is_pointer() || right_type.is_pointer() {
                        return Err(CompileError::TypeError(format!(
                            "bitwise operator '{:?}' cannot be applied to pointer types",
                            op
                        )));
                    }
                    if left_type == Type::Double || right_type == Type::Double {
                        return Err(CompileError::TypeError(
                            "bitwise operator cannot be applied to double".to_string(),
                        ));
                    }
                    let common = common_type(&left_type, &right_type);
                    convert_operand(left, &left_type, &common);
                    convert_operand(right, &right_type, &common);
                    Ok(common)
                }

                // シフト演算 (<< >>): 各オペランドを独立に整数昇格、
                // 結果型は昇格後の左辺型（C 標準 6.5.7）
                BinaryOp::ShiftLeft | BinaryOp::ShiftRight => {
                    if left_type.is_void() || right_type.is_void() {
                        return Err(CompileError::TypeError(
                            "void expression used as operand of shift operation".to_string(),
                        ));
                    }
                    if left_type.is_pointer() || right_type.is_pointer() {
                        return Err(CompileError::TypeError(
                            "shift operator cannot be applied to pointer types".to_string(),
                        ));
                    }
                    if left_type == Type::Double || right_type == Type::Double {
                        return Err(CompileError::TypeError(
                            "shift operator cannot be applied to double".to_string(),
                        ));
                    }
                    // 各オペランドを独立に整数昇格
                    let promoted_left = integer_promote(&left_type);
                    let promoted_right = integer_promote(&right_type);
                    convert_operand(left, &left_type, &promoted_left);
                    convert_operand(right, &right_type, &promoted_right);
                    Ok(promoted_left)
                }

                // カンマ演算子: 右辺の型を返す
                BinaryOp::Comma => Ok(right_type),
            }
        }

        Expr::Conditional {
            condition,
            then_expr,
            else_expr,
        } => {
            let cond_type = typecheck_expr(condition, symbols)?;
            // 条件式に void は使用禁止
            if cond_type.is_void() {
                return Err(CompileError::TypeError(
                    "void expression used as condition".to_string(),
                ));
            }
            let then_type = typecheck_expr(then_expr, symbols)?;
            let else_type = typecheck_expr(else_expr, symbols)?;
            // 両方 void の場合は void を返す
            if then_type.is_void() && else_type.is_void() {
                return Ok(Type::Void);
            }
            // 片方だけ void はエラー
            if then_type.is_void() || else_type.is_void() {
                return Err(CompileError::TypeError(
                    "incompatible types in conditional expression (void and non-void)".to_string(),
                ));
            }
            // ポインタ型の場合は特別な処理
            if then_type.is_pointer() || else_type.is_pointer() {
                if then_type == else_type {
                    Ok(then_type)
                } else if then_type.is_pointer() && is_null_pointer_constant(else_expr) {
                    convert_operand(else_expr, &else_type, &then_type);
                    Ok(then_type)
                } else if else_type.is_pointer() && is_null_pointer_constant(then_expr) {
                    convert_operand(then_expr, &then_type, &else_type);
                    Ok(else_type)
                } else if then_type.is_pointer() && else_type.is_pointer() {
                    // void* と他のポインタ型 → 共通型は void*
                    if is_void_pointer(&then_type) && !is_void_pointer(&else_type) {
                        convert_operand(else_expr, &else_type, &then_type);
                        Ok(then_type)
                    } else if is_void_pointer(&else_type) && !is_void_pointer(&then_type) {
                        convert_operand(then_expr, &then_type, &else_type);
                        Ok(else_type)
                    } else {
                        Err(CompileError::TypeError(
                            "incompatible types in conditional expression".to_string(),
                        ))
                    }
                } else {
                    Err(CompileError::TypeError(
                        "incompatible types in conditional expression".to_string(),
                    ))
                }
            } else {
                let common = common_type(&then_type, &else_type);
                convert_operand(then_expr, &then_type, &common);
                convert_operand(else_expr, &else_type, &common);
                Ok(common)
            }
        }

        Expr::FunctionCall(name, args) => {
            // GCC builtin: __builtin_expect(expr, expected) → returns expr type
            if name == "__builtin_expect" {
                if args.len() != 2 {
                    return Err(CompileError::TypeError(
                        "__builtin_expect requires 2 arguments".to_string(),
                    ));
                }
                let first_type = typecheck_expr(&mut args[0], symbols)?;
                typecheck_expr(&mut args[1], symbols)?;
                return Ok(first_type);
            }

            // GCC builtins (whitelist): evaluate args, return first arg's type.
            // Only builtins with pass-through semantics are supported here.
            if matches!(
                name.as_str(),
                "__builtin_bswap16"
                    | "__builtin_bswap32"
                    | "__builtin_bswap64"
                    | "__builtin_object_size"
                    | "__builtin_ctz"
                    | "__builtin_ctzl"
                    | "__builtin_clz"
                    | "__builtin_clzl"
                    | "__builtin_popcount"
                    | "__builtin_popcountl"
                    | "__builtin_abs"
                    | "__builtin_labs"
            ) {
                for arg in args.iter_mut() {
                    typecheck_expr(arg, symbols)?;
                }
                if !args.is_empty() {
                    return typecheck_expr(&mut args[0], symbols);
                }
                return Ok(Type::Int);
            }

            // 関数シグネチャを解決: 直接呼び出し / 関数ポインタ呼び出し
            let call_sig = match symbols.get(name) {
                Some(SymbolType::Function {
                    return_type,
                    param_types,
                    is_variadic,
                }) => CallSig {
                    return_type: return_type.clone(),
                    param_types: param_types.clone(),
                    is_variadic: *is_variadic,
                },
                Some(SymbolType::Variable(Type::Pointer(inner))) => match inner.as_ref() {
                    // 関数ポインタ: Pointer(Function { ... })
                    Type::Function {
                        return_type,
                        param_types,
                        is_variadic,
                    } => CallSig {
                        return_type: *return_type.clone(),
                        param_types: param_types.clone(),
                        is_variadic: *is_variadic,
                    },
                    // Pointer(Void) — 型情報なし（system header tolerance 用フォールバック）
                    // パーサーが関数ポインタのパラメータリストをパースできなかった場合に残る。
                    // 引数の型チェックのみ行い、戻り値は Int と仮定する。
                    // TODO: tacky_gen で戻り値型を使う段階で、このパスが実コードで
                    // 踏まれるケースを調査し、必要なら Pointer(Void) を排除する。
                    Type::Void => {
                        for arg in args.iter_mut() {
                            typecheck_expr(arg, symbols)?;
                        }
                        return Ok(Type::Int);
                    }
                    _ => {
                        return Err(CompileError::TypeError(format!(
                            "called object '{}' is not a function or function pointer",
                            name
                        )));
                    }
                },
                _ => {
                    return Err(CompileError::TypeError(format!(
                        "undeclared function '{}'",
                        name
                    )));
                }
            };

            typecheck_call_args(name, args, &call_sig, symbols)?;
            Ok(call_sig.return_type)
        }

        Expr::CallExpr(callee, args) => {
            let callee_type = typecheck_expr(callee, symbols)?;
            let call_sig = match &callee_type {
                Type::Pointer(inner) => match inner.as_ref() {
                    Type::Function {
                        return_type,
                        param_types,
                        is_variadic,
                    } => CallSig {
                        return_type: *return_type.clone(),
                        param_types: param_types.clone(),
                        is_variadic: *is_variadic,
                    },
                    Type::Void => {
                        for arg in args.iter_mut() {
                            typecheck_expr(arg, symbols)?;
                        }
                        return Ok(Type::Int);
                    }
                    _ => {
                        return Err(CompileError::TypeError(
                            "called expression is not a function pointer".to_string(),
                        ));
                    }
                },
                _ => {
                    return Err(CompileError::TypeError(
                        "called expression is not a function pointer".to_string(),
                    ));
                }
            };
            typecheck_call_args("<expr>", args, &call_sig, symbols)?;
            Ok(call_sig.return_type)
        }

        Expr::Dereref(inner) => {
            let inner_type = typecheck_expr(inner, symbols)?;
            match inner_type {
                Type::Pointer(ref target) if target.is_void() => Err(CompileError::TypeError(
                    "dereference of void pointer".to_string(),
                )),
                Type::Pointer(target) => {
                    // 配列→ポインタ減衰: *(ptr_to_array) → Pointer(elem)
                    // 例: int (*p)[3]; *p → Array(Int,3) → decay → Pointer(Int)
                    Ok(array_decay(*target))
                }
                _ => Err(CompileError::TypeError(
                    "dereference of non-pointer type".to_string(),
                )),
            }
        }

        Expr::AddrOf(inner) => {
            // 配列変数の場合は特別処理（Chapter 15）
            // &arr → Pointer(Array(elem, size))（配列全体へのポインタ）
            if let Expr::Var(name) = inner.as_ref()
                && let Some(SymbolType::Variable(Type::Array(elem, size))) = symbols.get(name)
            {
                // typecheck_expr は既に decay してしまうので、直接配列型から計算
                return Ok(Type::Pointer(Box::new(Type::Array(elem.clone(), *size))));
            }
            let inner_type = typecheck_expr(inner, symbols)?;
            // &func_name: 関数はすでに Pointer(Function) に decay しているので
            // 二重ポインタにならないようそのまま返す（C 標準: &f == f）
            if matches!(inner_type, Type::Pointer(ref t) if matches!(t.as_ref(), Type::Function { .. }))
            {
                return Ok(inner_type);
            }
            if !is_lvalue(inner) {
                return Err(CompileError::TypeError(
                    "cannot take address of non-lvalue expression".to_string(),
                ));
            }
            Ok(Type::Pointer(Box::new(inner_type)))
        }

        // 文字列リテラル（Chapter 16）: char * として扱う
        Expr::StringLiteral(_) => Ok(Type::Pointer(Box::new(Type::Char))),

        // sizeof（Chapter 15）: 型チェック時に定数に解決
        Expr::SizeOfType(ty) => {
            if ty.is_incomplete() {
                return Err(CompileError::TypeError(
                    "sizeof applied to incomplete type".to_string(),
                ));
            }
            let size = ty.size() as u64;
            *expr = Expr::ConstantULong(size);
            Ok(Type::ULong)
        }

        Expr::SizeOfExpr(inner) => {
            // Var の場合は配列 decay 前の型（配列サイズを保持）をシンボルテーブルから取得
            let actual_type = if let Expr::Var(name) = inner.as_ref() {
                match symbols.get(name) {
                    Some(SymbolType::Variable(t)) => t.clone(),
                    _ => typecheck_expr(inner, symbols)?,
                }
            } else {
                typecheck_expr(inner, symbols)?
            };
            if actual_type.is_incomplete() {
                return Err(CompileError::TypeError(
                    "sizeof applied to expression of incomplete type".to_string(),
                ));
            }
            let size = actual_type.size() as u64;
            *expr = Expr::ConstantULong(size);
            Ok(Type::ULong)
        }

        // Chapter 18: メンバアクセス
        Expr::Dot(inner, member_name) => {
            let inner_type = typecheck_expr(inner, symbols)?;
            match &inner_type {
                Type::Struct {
                    members,
                    tag,
                    is_union,
                } => match struct_member_offset_ex(members, member_name, *is_union) {
                    Some((_, member_type)) => Ok(array_decay(member_type)),
                    None => Err(CompileError::TypeError(format!(
                        "struct '{}' has no member '{}'",
                        tag, member_name
                    ))),
                },
                _ => Err(CompileError::TypeError(
                    "member access on non-struct type".to_string(),
                )),
            }
        }

        // Chapter 18: 複合初期化子（宣言コンテキスト外で使われた場合）
        Expr::CompoundInit(_) => Err(CompileError::TypeError(
            "compound initializer not allowed in expression context".to_string(),
        )),

        Expr::VaStart(ap) => {
            let ap_type = typecheck_expr(ap, symbols)?;
            if ap_type != Type::VaList {
                return Err(CompileError::TypeError(
                    "va_start requires va_list argument".into(),
                ));
            }
            Ok(Type::Void)
        }

        Expr::VaArg { ap, arg_type } => {
            let ap_type = typecheck_expr(ap, symbols)?;
            if ap_type != Type::VaList {
                return Err(CompileError::TypeError(
                    "va_arg requires va_list argument".into(),
                ));
            }
            Ok(arg_type.clone())
        }

        Expr::VaEnd(ap) => {
            let ap_type = typecheck_expr(ap, symbols)?;
            if ap_type != Type::VaList {
                return Err(CompileError::TypeError(
                    "va_end requires va_list argument".into(),
                ));
            }
            Ok(Type::Void)
        }

        Expr::VaCopy(dst, src) => {
            typecheck_expr(dst, symbols)?;
            typecheck_expr(src, symbols)?;
            Ok(Type::Void)
        }
    }
}

/// 定数リテラルの型を解決する。i32 に収まらない Constant は ConstantLong に変換。
/// u32 に収まらない ConstantUInt は ConstantULong に変換。
fn resolve_constant(expr: &mut Expr) {
    match expr {
        Expr::Constant(v) => {
            if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                let val = *v;
                *expr = Expr::ConstantLong(val);
            }
        }
        Expr::ConstantUInt(v) => {
            if *v > u32::MAX as u64 {
                let val = *v;
                *expr = Expr::ConstantULong(val);
            }
        }
        Expr::ConstantDouble(_) => {
            // Double 定数はそのまま
        }
        _ => {}
    }
}

/// 関数呼び出しのシグネチャ情報。直接呼び出しと間接呼び出しの両方で共用。
struct CallSig {
    return_type: Type,
    /// `None` = 引数未指定 `()` — 引数チェックをスキップ（K&R 互換）
    /// `Some(vec![])` = 引数ゼロ `(void)` — 引数があればエラー
    /// `Some(vec![...])` = 具体的なプロトタイプ
    param_types: Option<Vec<Type>>,
    is_variadic: bool,
}

/// 関数呼び出しの引数を型チェックする（直接/間接呼び出し共通）。
fn typecheck_call_args(
    name: &str,
    args: &mut [Expr],
    sig: &CallSig,
    symbols: &HashMap<String, SymbolType>,
) -> Result<()> {
    let param_types = match &sig.param_types {
        Some(pt) => pt,
        None => {
            // 引数未指定 — 型チェックのみ行い、引数数・型の整合性はスキップ
            for arg in args.iter_mut() {
                let arg_type = typecheck_expr(arg, symbols)?;
                // デフォルト引数昇格: char → int
                if arg_type.is_character() {
                    let old_arg = std::mem::replace(arg, Expr::Constant(0));
                    *arg = Expr::Cast {
                        target_type: Type::Int,
                        source_type: arg_type,
                        expr: Box::new(old_arg),
                    };
                }
            }
            return Ok(());
        }
    };

    // 引数数チェック
    if sig.is_variadic {
        if args.len() < param_types.len() {
            return Err(CompileError::TypeError(format!(
                "function '{}' requires at least {} arguments, got {}",
                name,
                param_types.len(),
                args.len()
            )));
        }
    } else if args.len() != param_types.len() {
        return Err(CompileError::TypeError(format!(
            "function '{}' expects {} arguments, got {}",
            name,
            param_types.len(),
            args.len()
        )));
    }

    // 名前付きパラメータの型チェック+キャスト
    for (arg, expected_type) in args.iter_mut().zip(param_types.iter()) {
        let arg_type = typecheck_expr(arg, symbols)?;
        if arg_type != *expected_type {
            // void ポインタ暗黙変換チェック: 一方が void* なら許可
            // 関数ポインタ同士は互換として許可（bsearch/qsort comparator 等）
            let is_fn_ptr = |t: &Type| matches!(t, Type::Pointer(inner) if matches!(inner.as_ref(), Type::Function { .. }));
            if expected_type.is_pointer()
                && arg_type.is_pointer()
                && *expected_type != arg_type
                && !is_void_pointer(expected_type)
                && !is_void_pointer(&arg_type)
                && !(is_fn_ptr(expected_type) && is_fn_ptr(&arg_type))
            {
                return Err(CompileError::TypeError(format!(
                    "incompatible pointer types in argument to function '{}'",
                    name
                )));
            }
            let old_arg = std::mem::replace(arg, Expr::Constant(0));
            *arg = Expr::Cast {
                target_type: expected_type.clone(),
                source_type: arg_type,
                expr: Box::new(old_arg),
            };
        }
    }

    // 可変長引数部分: デフォルト引数昇格（char → int）
    if sig.is_variadic {
        for arg in args.iter_mut().skip(param_types.len()) {
            let arg_type = typecheck_expr(arg, symbols)?;
            if arg_type.is_character() {
                let old_arg = std::mem::replace(arg, Expr::Constant(0));
                *arg = Expr::Cast {
                    target_type: Type::Int,
                    source_type: arg_type,
                    expr: Box::new(old_arg),
                };
            }
        }
    }

    Ok(())
}

/// 配列/関数→ポインタ減衰。
///
/// - 配列型 → ポインタ型（Chapter 15）
/// - 関数型 → 関数ポインタ型（typedef `handler_t` → `handler_t *` 相当）
/// - その他 → そのまま返す
fn array_decay(ty: Type) -> Type {
    match ty {
        Type::Array(elem, _) => Type::Pointer(elem),
        Type::Function { .. } => Type::Pointer(Box::new(ty)),
        other => other,
    }
}

/// 式が配列変数かどうかを判定する（Chapter 15）。
///
/// シンボルテーブル上の型が配列であるか確認する（decay 前の型）。
fn is_array_var(expr: &Expr, symbols: &HashMap<String, SymbolType>) -> bool {
    if let Expr::Var(name) = expr
        && let Some(SymbolType::Variable(t)) = symbols.get(name)
    {
        return t.is_array();
    }
    false
}

/// 左辺値（lvalue）判定（Chapter 14, 18）。
///
/// 左辺値は代入の左辺に置ける式。変数参照、ポインタ間接参照、構造体メンバアクセスが該当する。
fn is_lvalue(expr: &Expr) -> bool {
    match expr {
        Expr::Var(_) | Expr::Dereref(_) => true,
        // Chapter 18: Dot(inner, _) は inner が lvalue なら lvalue
        Expr::Dot(inner, _) => is_lvalue(inner),
        _ => false,
    }
}

/// void ポインタの判定。
fn is_void_pointer(ty: &Type) -> bool {
    matches!(ty, Type::Pointer(inner) if inner.is_void())
}

/// null ポインタ定数の判定（Chapter 14）。
///
/// 整数定数の 0 は任意のポインタ型に暗黙変換可能な null ポインタ定数。
fn is_null_pointer_constant(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Constant(0) | Expr::ConstantLong(0) | Expr::ConstantUInt(0) | Expr::ConstantULong(0)
    )
}

/// 2つの型の共通型を求める（通常算術変換 / usual arithmetic conversions）。
///
/// | a \ b    | Int  | Long | UInt  | ULong |
/// |----------|------|------|-------|-------|
/// | Int      | Int  | Long | UInt  | ULong |
/// | Long     | Long | Long | Long  | ULong |
/// | UInt     | UInt | Long | UInt  | ULong |
/// | ULong    | ULong| ULong| ULong | ULong |
/// 整数昇格 (integer promotion): Char/UChar/Int → Int, それ以外はそのまま。
fn integer_promote(t: &Type) -> Type {
    if t.is_character() {
        Type::Int
    } else {
        t.clone()
    }
}

fn common_type(a: &Type, b: &Type) -> Type {
    // Integer promotion: Char/UChar → Int（Chapter 16）
    let a = if a.is_character() { &Type::Int } else { a };
    let b = if b.is_character() { &Type::Int } else { b };

    if a == b {
        return a.clone();
    }
    // Double is the highest rank (Chapter 13)
    if *a == Type::Double || *b == Type::Double {
        return Type::Double;
    }
    // If either is ULong, result is ULong
    if *a == Type::ULong || *b == Type::ULong {
        return Type::ULong;
    }
    // If either is Long
    if *a == Type::Long || *b == Type::Long {
        // Long + UInt → Long (Long can represent all UInt values)
        // Long + Int → Long
        return Type::Long;
    }
    // If either is UInt
    if *a == Type::UInt || *b == Type::UInt {
        return Type::UInt;
    }
    Type::Int
}

/// 必要に応じて Cast ノードを挿入する。
fn convert_operand(expr: &mut Box<Expr>, from: &Type, to: &Type) {
    if from != to {
        let old = std::mem::replace(expr.as_mut(), Expr::Constant(0));
        **expr = Expr::Cast {
            target_type: to.clone(),
            source_type: from.clone(),
            expr: Box::new(old),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::ast::*;

    #[test]
    fn typecheck_constant_int() {
        let mut program = Program {
            declarations: vec![TopLevelDecl::Function(FunctionDecl {
                name: "main".to_string(),
                return_type: Type::Int,
                params: vec![],
                body: Some(vec![BlockItem::Statement(Statement::Return(Some(
                    Expr::Constant(42),
                )))]),
                storage_class: None,
                is_variadic: false,
                has_prototype: true,
            })],
        };
        typecheck(&mut program).unwrap();
        // 42 fits in i32, should remain Constant
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        if let BlockItem::Statement(Statement::Return(Some(expr))) = &func.body.as_ref().unwrap()[0]
        {
            assert!(matches!(expr, Expr::Constant(42)));
        } else {
            panic!("expected return");
        }
    }

    #[test]
    fn typecheck_constant_too_large_for_int() {
        let mut program = Program {
            declarations: vec![TopLevelDecl::Function(FunctionDecl {
                name: "main".to_string(),
                return_type: Type::Long,
                params: vec![],
                body: Some(vec![
                    BlockItem::Statement(Statement::Return(Some(Expr::Constant(8589934592)))), // 2^33
                ]),
                storage_class: None,
                is_variadic: false,
                has_prototype: true,
            })],
        };
        typecheck(&mut program).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        if let BlockItem::Statement(Statement::Return(Some(expr))) = &func.body.as_ref().unwrap()[0]
        {
            assert!(matches!(expr, Expr::ConstantLong(8589934592)));
        } else {
            panic!("expected return");
        }
    }

    #[test]
    fn typecheck_cast_on_return_type_mismatch() {
        let mut program = Program {
            declarations: vec![TopLevelDecl::Function(FunctionDecl {
                name: "main".to_string(),
                return_type: Type::Int,
                params: vec![],
                body: Some(vec![BlockItem::Statement(Statement::Return(Some(
                    Expr::ConstantLong(42),
                )))]),
                storage_class: None,
                is_variadic: false,
                has_prototype: true,
            })],
        };
        typecheck(&mut program).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        if let BlockItem::Statement(Statement::Return(Some(expr))) = &func.body.as_ref().unwrap()[0]
        {
            assert!(matches!(
                expr,
                Expr::Cast {
                    target_type: Type::Int,
                    ..
                }
            ));
        } else {
            panic!("expected return with cast");
        }
    }

    #[test]
    fn typecheck_binary_promotion() {
        // int + long → both promoted to long
        let mut program = Program {
            declarations: vec![TopLevelDecl::Function(FunctionDecl {
                name: "main".to_string(),
                return_type: Type::Long,
                params: vec![],
                body: Some(vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        var_type: Type::Int,
                        init: Some(Expr::Constant(1)),
                        storage_class: None,
                    }),
                    BlockItem::Declaration(Declaration {
                        name: "b".to_string(),
                        var_type: Type::Long,
                        init: Some(Expr::ConstantLong(2)),
                        storage_class: None,
                    }),
                    BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                        BinaryOp::Add,
                        Box::new(Expr::Var("a".to_string())),
                        Box::new(Expr::Var("b".to_string())),
                    )))),
                ]),
                storage_class: None,
                is_variadic: false,
                has_prototype: true,
            })],
        };
        typecheck(&mut program).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        if let BlockItem::Statement(Statement::Return(Some(Expr::Binary(_, left, _)))) =
            &func.body.as_ref().unwrap()[2]
        {
            // left (int) should be cast to long
            assert!(matches!(
                left.as_ref(),
                Expr::Cast {
                    target_type: Type::Long,
                    ..
                }
            ));
        } else {
            panic!("expected return with binary");
        }
    }
}
