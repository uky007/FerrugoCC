//! 型検査ロジック（Chapter 11, 12, 13, 14, 15）
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

use std::collections::HashMap;

use crate::error::{CompileError, Result};
use crate::parse::ast::{
    Program, TopLevelDecl, FunctionDecl, BlockItem, Declaration, Statement, Expr,
    UnaryOp, BinaryOp, ForInit, StorageClass, Type,
};

/// シンボルの型情報。
#[derive(Debug, Clone)]
enum SymbolType {
    Variable(Type),
    Function { return_type: Type, param_types: Vec<Type> },
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
        }
    }

    Ok(())
}

/// ファイルスコープ変数の型検査。
fn typecheck_file_scope_var(
    decl: &mut Declaration,
    symbols: &mut HashMap<String, SymbolType>,
) -> Result<()> {
    symbols.insert(decl.name.clone(), SymbolType::Variable(decl.var_type.clone()));

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
    let param_types: Vec<Type> = func.params.iter().map(|(t, _)| t.clone()).collect();

    // 既存の宣言との互換性チェック
    if let Some(existing) = symbols.get(&func.name) {
        if let SymbolType::Function { return_type, param_types: existing_params } = existing {
            if *return_type != func.return_type || *existing_params != param_types {
                return Err(CompileError::TypeError(format!(
                    "conflicting types for function '{}'", func.name
                )));
            }
        }
    }

    symbols.insert(func.name.clone(), SymbolType::Function {
        return_type: func.return_type.clone(),
        param_types: param_types.clone(),
    });

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
    }
}

/// ローカル変数宣言の型検査。
fn typecheck_local_declaration(
    decl: &mut Declaration,
    symbols: &mut HashMap<String, SymbolType>,
) -> Result<()> {
    symbols.insert(decl.name.clone(), SymbolType::Variable(decl.var_type.clone()));

    // 配列型への初期化子は禁止（初期化子リストは未対応）
    if decl.var_type.is_array() && decl.init.is_some() {
        return Err(CompileError::TypeError(format!(
            "cannot initialize array variable '{}' with scalar initializer", decl.name
        )));
    }

    if let Some(init) = &mut decl.init {
        let init_type = typecheck_expr(init, symbols)?;
        // 右辺の型が左辺と異なる場合、キャストを挿入
        if init_type != decl.var_type {
            // ポインタ型への null ポインタ定数の代入は許可
            if decl.var_type.is_pointer() && !init_type.is_pointer() && !is_null_pointer_constant(init) {
                return Err(CompileError::TypeError(format!(
                    "cannot initialize pointer with non-pointer non-null value"
                )));
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
        Statement::Return(expr) => {
            let expr_type = typecheck_expr(expr, symbols)?;
            if expr_type != *return_type {
                let old_expr = std::mem::replace(expr, Expr::Constant(0));
                *expr = Expr::Cast {
                    target_type: return_type.clone(),
                    source_type: expr_type,
                    expr: Box::new(old_expr),
                };
            }
            Ok(())
        }
        Statement::Expression(expr) => {
            typecheck_expr(expr, symbols)?;
            Ok(())
        }
        Statement::Null => Ok(()),
        Statement::If { condition, then_branch, else_branch } => {
            typecheck_expr(condition, symbols)?;
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
            typecheck_expr(condition, symbols)?;
            typecheck_statement(body, symbols, return_type)
        }
        Statement::DoWhile { body, condition } => {
            typecheck_statement(body, symbols, return_type)?;
            typecheck_expr(condition, symbols)?;
            Ok(())
        }
        Statement::For { init, condition, post, body } => {
            let mut inner_symbols = symbols.clone();
            match init {
                ForInit::Declaration(decl) => {
                    typecheck_local_declaration(decl, &mut inner_symbols)?;
                }
                ForInit::Expression(Some(expr)) => {
                    typecheck_expr(expr, &mut inner_symbols)?;
                }
                ForInit::Expression(None) => {}
            }
            if let Some(cond) = condition {
                typecheck_expr(cond, &inner_symbols)?;
            }
            if let Some(post_expr) = post {
                typecheck_expr(post_expr, &inner_symbols)?;
            }
            typecheck_statement(body, &mut inner_symbols, return_type)
        }
        Statement::Break | Statement::Continue => Ok(()),
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

        Expr::Cast { target_type, source_type, expr: inner } => {
            let actual_source = typecheck_expr(inner, symbols)?;
            *source_type = actual_source.clone();
            // ポインタ ↔ double のキャストは禁止
            if target_type.is_pointer() && actual_source == Type::Double {
                return Err(CompileError::TypeError(
                    "cannot cast 'double' to pointer type".to_string()
                ));
            }
            if target_type.is_double() && actual_source.is_pointer() {
                return Err(CompileError::TypeError(
                    "cannot cast pointer type to 'double'".to_string()
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
                Some(SymbolType::Function { .. }) => {
                    Err(CompileError::TypeError(format!(
                        "function '{}' used as variable", name
                    )))
                }
                None => {
                    Err(CompileError::TypeError(format!(
                        "undeclared variable '{}'", name
                    )))
                }
            }
        }

        Expr::Assign(lhs, rhs) => {
            let lhs_type = typecheck_expr(lhs, symbols)?;
            if !is_lvalue(lhs) {
                return Err(CompileError::TypeError(
                    "left side of assignment must be an lvalue".to_string()
                ));
            }
            // 配列への代入は禁止（Chapter 15）
            if is_array_var(lhs, symbols) {
                return Err(CompileError::TypeError(
                    "cannot assign to array variable".to_string()
                ));
            }
            let rhs_type = typecheck_expr(rhs, symbols)?;
            if rhs_type != lhs_type {
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
                    "left side of compound assignment must be an lvalue".to_string()
                ));
            }
            // 配列への複合代入は禁止（Chapter 15）
            if is_array_var(lhs, symbols) {
                return Err(CompileError::TypeError(
                    "cannot assign to array variable".to_string()
                ));
            }
            let rhs_type = typecheck_expr(rhs, symbols)?;
            // ポインタ算術の複合代入: ptr += int, ptr -= int（Chapter 15）
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
                    "compound assignment '{:?}' cannot be applied to pointer types", op
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
                    "operand of postfix increment/decrement must be an lvalue".to_string()
                ));
            }
            // 配列への ++/-- は禁止（Chapter 15）
            if is_array_var(inner, symbols) {
                return Err(CompileError::TypeError(
                    "cannot increment/decrement array variable".to_string()
                ));
            }
            Ok(inner_type)
        }

        Expr::Unary(op, inner) => {
            let inner_type = typecheck_expr(inner, symbols)?;
            match op {
                UnaryOp::Not => {
                    // `!` は常に Int を返す（ポインタも許可: null なら 1、非 null なら 0）
                    Ok(Type::Int)
                }
                UnaryOp::Complement => {
                    if inner_type == Type::Double {
                        return Err(CompileError::TypeError(
                            "bitwise complement '~' cannot be applied to double".to_string()
                        ));
                    }
                    if inner_type.is_pointer() {
                        return Err(CompileError::TypeError(
                            "bitwise complement '~' cannot be applied to pointer type".to_string()
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
                    if inner_type.is_pointer() {
                        return Err(CompileError::TypeError(
                            "unary negation '-' cannot be applied to pointer type".to_string()
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
                            "operand of prefix increment/decrement must be an lvalue".to_string()
                        ));
                    }
                    // 配列への ++/-- は禁止（Chapter 15）
                    if is_array_var(inner, symbols) {
                        return Err(CompileError::TypeError(
                            "cannot increment/decrement array variable".to_string()
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
                BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
                    Ok(Type::Int)
                }

                // 比較演算子: ポインタ同士の比較も対応（Chapter 15 で全比較演算子に拡張）
                BinaryOp::LessThan | BinaryOp::LessEqual
                | BinaryOp::GreaterThan | BinaryOp::GreaterEqual
                | BinaryOp::Equal | BinaryOp::NotEqual => {
                    if left_type.is_pointer() || right_type.is_pointer() {
                        // 両方とも同じポインタ型であること
                        // ただし null ポインタ定数（整数 0）は任意のポインタ型と比較可能
                        if left_type.is_pointer() && right_type.is_pointer() {
                            if left_type != right_type {
                                return Err(CompileError::TypeError(
                                    "comparison between incompatible pointer types".to_string()
                                ));
                            }
                        } else if left_type.is_pointer() && !right_type.is_pointer() {
                            // null ポインタ定数 (0) を許可
                            if !is_null_pointer_constant(right) {
                                return Err(CompileError::TypeError(
                                    "comparison between pointer and non-zero integer".to_string()
                                ));
                            }
                            // 整数 0 をポインタ型にキャスト
                            convert_operand(right, &right_type, &left_type);
                        } else {
                            // right is pointer, left is not
                            if !is_null_pointer_constant(left) {
                                return Err(CompileError::TypeError(
                                    "comparison between pointer and non-zero integer".to_string()
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
                    if left_type.is_pointer() && right_type.is_pointer() {
                        return Err(CompileError::TypeError(
                            "cannot add two pointers".to_string()
                        ));
                    }
                    if left_type.is_pointer() && !right_type.is_pointer() {
                        // ptr + int: 右辺を Long にキャスト
                        if right_type != Type::Long {
                            convert_operand(right, &right_type, &Type::Long);
                        }
                        Ok(left_type)
                    } else if !left_type.is_pointer() && right_type.is_pointer() {
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
                    if left_type.is_pointer() && right_type.is_pointer() {
                        // ptr - ptr: 同じポインタ型でなければエラー
                        if left_type != right_type {
                            return Err(CompileError::TypeError(
                                "subtraction between incompatible pointer types".to_string()
                            ));
                        }
                        Ok(Type::Long) // 結果は要素数差分（Long）
                    } else if left_type.is_pointer() && !right_type.is_pointer() {
                        // ptr - int: 右辺を Long にキャスト
                        if right_type != Type::Long {
                            convert_operand(right, &right_type, &Type::Long);
                        }
                        Ok(left_type)
                    } else if !left_type.is_pointer() && right_type.is_pointer() {
                        // int - ptr: エラー
                        Err(CompileError::TypeError(
                            "cannot subtract pointer from integer".to_string()
                        ))
                    } else {
                        // 通常の算術減算
                        let common = common_type(&left_type, &right_type);
                        convert_operand(left, &left_type, &common);
                        convert_operand(right, &right_type, &common);
                        Ok(common)
                    }
                }

                // 乗算・除算・剰余: ポインタ禁止
                BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Remainder => {
                    if left_type.is_pointer() || right_type.is_pointer() {
                        return Err(CompileError::TypeError(format!(
                            "arithmetic operator '{:?}' cannot be applied to pointer types", op
                        )));
                    }
                    let common = common_type(&left_type, &right_type);
                    if matches!(op, BinaryOp::Remainder) && common == Type::Double {
                        return Err(CompileError::TypeError(
                            "remainder '%' cannot be applied to double".to_string()
                        ));
                    }
                    convert_operand(left, &left_type, &common);
                    convert_operand(right, &right_type, &common);
                    Ok(common)
                }

                // カンマ演算子: 右辺の型を返す
                BinaryOp::Comma => {
                    Ok(right_type)
                }
            }
        }

        Expr::Conditional { condition, then_expr, else_expr } => {
            typecheck_expr(condition, symbols)?;
            let then_type = typecheck_expr(then_expr, symbols)?;
            let else_type = typecheck_expr(else_expr, symbols)?;
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
                } else {
                    Err(CompileError::TypeError(
                        "incompatible types in conditional expression".to_string()
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
            let (return_type, param_types) = match symbols.get(name) {
                Some(SymbolType::Function { return_type, param_types }) => {
                    (return_type.clone(), param_types.clone())
                }
                _ => return Err(CompileError::TypeError(format!(
                    "undeclared function '{}'", name
                ))),
            };

            if args.len() != param_types.len() {
                return Err(CompileError::TypeError(format!(
                    "function '{}' expects {} arguments, got {}",
                    name, param_types.len(), args.len()
                )));
            }

            for (arg, expected_type) in args.iter_mut().zip(param_types.iter()) {
                let arg_type = typecheck_expr(arg, symbols)?;
                if arg_type != *expected_type {
                    let old_arg = std::mem::replace(arg, Expr::Constant(0));
                    *arg = Expr::Cast {
                        target_type: expected_type.clone(),
                        source_type: arg_type,
                        expr: Box::new(old_arg),
                    };
                }
            }

            Ok(return_type)
        }

        Expr::Dereref(inner) => {
            let inner_type = typecheck_expr(inner, symbols)?;
            match inner_type {
                Type::Pointer(target) => Ok(*target),
                _ => Err(CompileError::TypeError(
                    "dereference of non-pointer type".to_string()
                )),
            }
        }

        Expr::AddrOf(inner) => {
            // 配列変数の場合は特別処理（Chapter 15）
            // &arr は先頭要素へのポインタ（decay 前の型から Pointer(elem) を生成）
            if let Expr::Var(name) = inner.as_ref() {
                if let Some(SymbolType::Variable(var_type)) = symbols.get(name) {
                    if let Type::Array(elem, _) = var_type {
                        // typecheck_expr は既に decay してしまうので、直接配列型から計算
                        return Ok(Type::Pointer(elem.clone()));
                    }
                }
            }
            let inner_type = typecheck_expr(inner, symbols)?;
            if !is_lvalue(inner) {
                return Err(CompileError::TypeError(
                    "cannot take address of non-lvalue expression".to_string()
                ));
            }
            Ok(Type::Pointer(Box::new(inner_type)))
        }

        // 文字列リテラル（Chapter 16）: char * として扱う
        Expr::StringLiteral(_) => {
            Ok(Type::Pointer(Box::new(Type::Char)))
        }

        // sizeof（Chapter 15）: 型チェック時に定数に解決
        Expr::SizeOfType(ty) => {
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
            let size = actual_type.size() as u64;
            *expr = Expr::ConstantULong(size);
            Ok(Type::ULong)
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

/// 配列→ポインタ減衰（Chapter 15）。
///
/// 配列型を対応するポインタ型に変換する。非配列型はそのまま返す。
fn array_decay(ty: Type) -> Type {
    match ty {
        Type::Array(elem, _) => Type::Pointer(elem),
        other => other,
    }
}

/// 式が配列変数かどうかを判定する（Chapter 15）。
///
/// シンボルテーブル上の型が配列であるか確認する（decay 前の型）。
fn is_array_var(expr: &Expr, symbols: &HashMap<String, SymbolType>) -> bool {
    if let Expr::Var(name) = expr {
        if let Some(SymbolType::Variable(t)) = symbols.get(name) {
            return t.is_array();
        }
    }
    false
}

/// 左辺値（lvalue）判定（Chapter 14）。
///
/// 左辺値は代入の左辺に置ける式。変数参照とポインタ間接参照が該当する。
fn is_lvalue(expr: &Expr) -> bool {
    matches!(expr, Expr::Var(_) | Expr::Dereref(_))
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
                body: Some(vec![
                    BlockItem::Statement(Statement::Return(Expr::Constant(42))),
                ]),
                storage_class: None,
            })],
        };
        typecheck(&mut program).unwrap();
        // 42 fits in i32, should remain Constant
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        if let BlockItem::Statement(Statement::Return(expr)) = &func.body.as_ref().unwrap()[0] {
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
                    BlockItem::Statement(Statement::Return(Expr::Constant(8589934592))), // 2^33
                ]),
                storage_class: None,
            })],
        };
        typecheck(&mut program).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        if let BlockItem::Statement(Statement::Return(expr)) = &func.body.as_ref().unwrap()[0] {
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
                body: Some(vec![
                    BlockItem::Statement(Statement::Return(Expr::ConstantLong(42))),
                ]),
                storage_class: None,
            })],
        };
        typecheck(&mut program).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        if let BlockItem::Statement(Statement::Return(expr)) = &func.body.as_ref().unwrap()[0] {
            assert!(matches!(expr, Expr::Cast { target_type: Type::Int, .. }));
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
                    BlockItem::Statement(Statement::Return(Expr::Binary(
                        BinaryOp::Add,
                        Box::new(Expr::Var("a".to_string())),
                        Box::new(Expr::Var("b".to_string())),
                    ))),
                ]),
                storage_class: None,
            })],
        };
        typecheck(&mut program).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        if let BlockItem::Statement(Statement::Return(Expr::Binary(_, left, _))) = &func.body.as_ref().unwrap()[2] {
            // left (int) should be cast to long
            assert!(matches!(left.as_ref(), Expr::Cast { target_type: Type::Long, .. }));
        } else {
            panic!("expected return with binary");
        }
    }
}
