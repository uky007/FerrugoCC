//! 型検査ロジック（Chapter 11, 12, 13）
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
    symbols.insert(decl.name.clone(), SymbolType::Variable(decl.var_type));

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
    let param_types: Vec<Type> = func.params.iter().map(|(t, _)| *t).collect();

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
        return_type: func.return_type,
        param_types: param_types.clone(),
    });

    if let Some(body) = &mut func.body {
        // パラメータをローカルシンボルに追加
        let mut local_symbols = symbols.clone();
        for (param_type, param_name) in &func.params {
            local_symbols.insert(param_name.clone(), SymbolType::Variable(*param_type));
        }

        for item in body.iter_mut() {
            typecheck_block_item(item, &mut local_symbols, func.return_type)?;
        }
    }

    Ok(())
}

/// ブロック要素の型検査。
fn typecheck_block_item(
    item: &mut BlockItem,
    symbols: &mut HashMap<String, SymbolType>,
    return_type: Type,
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
    symbols.insert(decl.name.clone(), SymbolType::Variable(decl.var_type));

    if let Some(init) = &mut decl.init {
        let init_type = typecheck_expr(init, symbols)?;
        // 右辺の型が左辺と異なる場合、キャストを挿入
        if init_type != decl.var_type {
            let old_init = std::mem::replace(init, Expr::Constant(0)); // placeholder
            *init = Expr::Cast {
                target_type: decl.var_type,
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
    return_type: Type,
) -> Result<()> {
    match stmt {
        Statement::Return(expr) => {
            let expr_type = typecheck_expr(expr, symbols)?;
            if expr_type != return_type {
                let old_expr = std::mem::replace(expr, Expr::Constant(0));
                *expr = Expr::Cast {
                    target_type: return_type,
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

        Expr::Cast { target_type, expr: inner, .. } => {
            typecheck_expr(inner, symbols)?;
            Ok(*target_type)
        }

        Expr::Var(name) => {
            match symbols.get(name) {
                Some(SymbolType::Variable(t)) => Ok(*t),
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

        Expr::Assign(name, rhs) => {
            let var_type = match symbols.get(name) {
                Some(SymbolType::Variable(t)) => *t,
                _ => return Err(CompileError::TypeError(format!(
                    "undeclared variable '{}'", name
                ))),
            };
            let rhs_type = typecheck_expr(rhs, symbols)?;
            if rhs_type != var_type {
                let old_rhs = std::mem::replace(rhs.as_mut(), Expr::Constant(0));
                **rhs = Expr::Cast {
                    target_type: var_type,
                    source_type: rhs_type,
                    expr: Box::new(old_rhs),
                };
            }
            Ok(var_type)
        }

        Expr::CompoundAssign(_, name, rhs) => {
            let var_type = match symbols.get(name) {
                Some(SymbolType::Variable(t)) => *t,
                _ => return Err(CompileError::TypeError(format!(
                    "undeclared variable '{}'", name
                ))),
            };
            let rhs_type = typecheck_expr(rhs, symbols)?;
            // 共通型に昇格して演算し、結果を変数の型にキャスト
            // compound assign は var_type を結果型とする
            let _common = common_type(var_type, rhs_type);
            Ok(var_type)
        }

        Expr::PostfixIncrement(name) | Expr::PostfixDecrement(name) => {
            match symbols.get(name) {
                Some(SymbolType::Variable(t)) => Ok(*t),
                _ => Err(CompileError::TypeError(format!(
                    "undeclared variable '{}'", name
                ))),
            }
        }

        Expr::Unary(op, inner) => {
            let inner_type = typecheck_expr(inner, symbols)?;
            match op {
                UnaryOp::Not => {
                    // `!` は常に Int を返す
                    Ok(Type::Int)
                }
                UnaryOp::Complement => {
                    if inner_type == Type::Double {
                        return Err(CompileError::TypeError(
                            "bitwise complement '~' cannot be applied to double".to_string()
                        ));
                    }
                    Ok(inner_type)
                }
                UnaryOp::Negate => {
                    Ok(inner_type)
                }
                UnaryOp::PreIncrement | UnaryOp::PreDecrement => {
                    Ok(inner_type)
                }
            }
        }

        Expr::Binary(op, left, right) => {
            let left_type = typecheck_expr(left, symbols)?;
            let right_type = typecheck_expr(right, symbols)?;

            match op {
                // 論理演算子: オペランドは共通型に昇格するが結果は Int
                BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
                    Ok(Type::Int)
                }

                // 比較演算子: オペランドは共通型に昇格、結果は Int
                BinaryOp::LessThan | BinaryOp::LessEqual
                | BinaryOp::GreaterThan | BinaryOp::GreaterEqual
                | BinaryOp::Equal | BinaryOp::NotEqual => {
                    let common = common_type(left_type, right_type);
                    convert_operand(left, left_type, common);
                    convert_operand(right, right_type, common);
                    Ok(Type::Int)
                }

                // 算術演算子: 共通型に昇格、結果も共通型
                BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply
                | BinaryOp::Divide | BinaryOp::Remainder => {
                    let common = common_type(left_type, right_type);
                    if matches!(op, BinaryOp::Remainder) && common == Type::Double {
                        return Err(CompileError::TypeError(
                            "remainder '%' cannot be applied to double".to_string()
                        ));
                    }
                    convert_operand(left, left_type, common);
                    convert_operand(right, right_type, common);
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
            let common = common_type(then_type, else_type);
            convert_operand(then_expr, then_type, common);
            convert_operand(else_expr, else_type, common);
            Ok(common)
        }

        Expr::FunctionCall(name, args) => {
            let (return_type, param_types) = match symbols.get(name) {
                Some(SymbolType::Function { return_type, param_types }) => {
                    (*return_type, param_types.clone())
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
                        target_type: *expected_type,
                        source_type: arg_type,
                        expr: Box::new(old_arg),
                    };
                }
            }

            Ok(return_type)
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

/// 2つの型の共通型を求める（通常算術変換 / usual arithmetic conversions）。
///
/// | a \ b    | Int  | Long | UInt  | ULong |
/// |----------|------|------|-------|-------|
/// | Int      | Int  | Long | UInt  | ULong |
/// | Long     | Long | Long | Long  | ULong |
/// | UInt     | UInt | Long | UInt  | ULong |
/// | ULong    | ULong| ULong| ULong | ULong |
fn common_type(a: Type, b: Type) -> Type {
    if a == b {
        return a;
    }
    // Double is the highest rank (Chapter 13)
    if a == Type::Double || b == Type::Double {
        return Type::Double;
    }
    // If either is ULong, result is ULong
    if a == Type::ULong || b == Type::ULong {
        return Type::ULong;
    }
    // If either is Long
    if a == Type::Long || b == Type::Long {
        // Long + UInt → Long (Long can represent all UInt values)
        // Long + Int → Long
        return Type::Long;
    }
    // If either is UInt
    if a == Type::UInt || b == Type::UInt {
        return Type::UInt;
    }
    Type::Int
}

/// 必要に応じて Cast ノードを挿入する。
fn convert_operand(expr: &mut Box<Expr>, from: Type, to: Type) {
    if from != to {
        let old = std::mem::replace(expr.as_mut(), Expr::Constant(0));
        **expr = Expr::Cast {
            target_type: to,
            source_type: from,
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
