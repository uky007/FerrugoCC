//! C AST → TACKY IR 変換器
//!
//! C の AST を走査し、三アドレスコードの TACKY IR に変換する。
//! 各式は `(Vec<TackyInstruction>, TackyVal)` を返す（命令列 + 結果値）。
//! 無制限の仮想変数（`tmp.N`）を使い、レジスタ割り当ては不要。

use super::tacky_ast::*;
use crate::error::{CompileError, Result};
use crate::parse::ast::{
    BinaryOp, BlockItem, Declaration, Expr, ForInit, FunctionDecl, Program, Statement,
    StorageClass, TopLevelDecl, Type, UnaryOp, struct_member_offset,
};
use std::collections::{HashMap, HashSet};

/// ループ/switch 内の break/continue ジャンプ先ラベル
struct LoopLabels {
    break_label: String,
    /// ループでは Some(label)、switch 内では None（continue は外側ループに委譲）
    continue_label: Option<String>,
}

/// 関数シンボルテーブル用の情報
#[allow(dead_code)]
struct FunctionInfo {
    param_count: usize,
    defined: bool,
    return_type: Type,
    param_types: Vec<Type>,
    is_variadic: bool,
}

/// 変数の種別
#[derive(Debug, Clone)]
enum VarKind {
    Local(Type),
    Static(String, Type),
}

/// TACKY 生成器
#[allow(dead_code)]
struct TackyGenerator {
    temp_counter: usize,
    label_counter: usize,
    static_label_counter: usize,
    string_label_counter: usize,
    const_label_counter: usize,
    /// 現在の関数内の変数 → 型マッピング（最適化で使用）
    var_types: HashMap<String, Type>,
    /// 変数のスコープ管理
    var_map: HashMap<String, VarKind>,
    /// double 定数プール: ビットパターン → (ラベル, アラインメント)
    double_constants: HashMap<u64, (String, usize)>,
    /// 文字列定数プール: (ラベル, 内容)
    string_constants: Vec<(String, String)>,
    /// static 変数リスト
    static_vars: Vec<TackyStaticVar>,
    /// 現在の関数の名前付きパラメータ型リスト（va_start で gp/fp offset 計算に使用）
    current_func_params: Vec<Type>,
}

impl TackyGenerator {
    fn new() -> Self {
        TackyGenerator {
            temp_counter: 0,
            label_counter: 0,
            static_label_counter: 0,
            string_label_counter: 0,
            const_label_counter: 0,
            var_types: HashMap::new(),
            var_map: HashMap::new(),
            double_constants: HashMap::new(),
            string_constants: Vec::new(),
            static_vars: Vec::new(),
            current_func_params: Vec::new(),
        }
    }

    /// 新しい一時変数名を生成し、型を登録する
    fn new_temp(&mut self, ty: Type) -> TackyVal {
        let name = format!("tmp.{}", self.temp_counter);
        self.temp_counter += 1;
        self.var_types.insert(name.clone(), ty);
        TackyVal::Var(name)
    }

    /// 新しいラベル名を生成する
    fn new_label(&mut self, prefix: &str) -> String {
        let label = format!(".L{}_{}", prefix, self.label_counter);
        self.label_counter += 1;
        label
    }

    /// 変数の型を取得する
    fn var_type(&self, name: &str) -> Type {
        if let Some(kind) = self.var_map.get(name) {
            match kind {
                VarKind::Local(ty) => ty.clone(),
                VarKind::Static(_, ty) => ty.clone(),
            }
        } else {
            // Fallback: check var_types (for temps)
            self.var_types.get(name).cloned().unwrap_or(Type::Int)
        }
    }

    /// 変数が static かどうか判定し、static ラベルを返す
    fn resolve_var_name(&self, name: &str) -> String {
        if let Some(VarKind::Static(label, _)) = self.var_map.get(name) {
            label.clone()
        } else {
            name.to_string()
        }
    }

    /// double 定数をプールに追加し、ラベルを返す
    #[allow(dead_code)]
    fn add_double_constant(&mut self, value: f64, alignment: usize) -> String {
        let bits = value.to_bits();
        if let Some((label, existing_align)) = self.double_constants.get_mut(&bits) {
            if alignment > *existing_align {
                *existing_align = alignment;
            }
            label.clone()
        } else {
            let label = format!(".Lconst_{}", self.const_label_counter);
            self.const_label_counter += 1;
            self.double_constants
                .insert(bits, (label.clone(), alignment));
            label
        }
    }

    /// 静的定数初期値を解決する
    fn resolve_static_init(
        init_expr: &Expr,
        string_constants: &mut Vec<(String, String)>,
        string_label_counter: &mut usize,
    ) -> std::result::Result<TackyStaticInit, String> {
        match init_expr {
            Expr::Constant(v) => Ok(TackyStaticInit::IntInit(*v)),
            Expr::ConstantLong(v) => Ok(TackyStaticInit::IntInit(*v)),
            Expr::ConstantUInt(v) => Ok(TackyStaticInit::IntInit(*v as i64)),
            Expr::ConstantULong(v) => Ok(TackyStaticInit::IntInit(*v as i64)),
            Expr::ConstantDouble(v) => Ok(TackyStaticInit::DoubleInit(*v)),
            Expr::StringLiteral(s) => {
                // String literal → emit string constant and return pointer to it
                let label = format!(".Lstr_{}", *string_label_counter);
                *string_label_counter += 1;
                string_constants.push((label.clone(), s.clone()));
                Ok(TackyStaticInit::PointerArrayInit(vec![label]))
            }
            Expr::CompoundInit(inits) => {
                let init_vals: Vec<TackyStaticInit> = inits
                    .iter()
                    .map(|e| {
                        TackyGenerator::resolve_static_init(
                            e,
                            string_constants,
                            string_label_counter,
                        )
                    })
                    .collect::<std::result::Result<_, _>>()?;
                Ok(TackyStaticInit::ArrayInit(init_vals))
            }
            // NULL pointer: cast of 0 to pointer type
            Expr::Cast { target_type: _, expr, .. } => {
                if let Expr::Constant(0) = expr.as_ref() {
                    Ok(TackyStaticInit::IntInit(0))
                } else {
                    TackyGenerator::resolve_static_init(expr, string_constants, string_label_counter)
                }
            }
            _ => Err("must be initialized with a constant expression".to_string()),
        }
    }
}

/// 式の Type を推定する（コード生成なし）
fn expr_type(
    expr: &Expr,
    var_map: &HashMap<String, VarKind>,
    func_table: &HashMap<String, FunctionInfo>,
) -> Type {
    match expr {
        Expr::Constant(_) => Type::Int,
        Expr::ConstantLong(_) => Type::Long,
        Expr::ConstantUInt(_) => Type::UInt,
        Expr::ConstantULong(_) => Type::ULong,
        Expr::ConstantDouble(_) => Type::Double,
        Expr::StringLiteral(_) => Type::Pointer(Box::new(Type::Char)),
        Expr::Cast { target_type, .. } => target_type.clone(),
        Expr::Var(name) => {
            let ty = match var_map.get(name) {
                Some(VarKind::Local(ty)) => ty.clone(),
                Some(VarKind::Static(_, ty)) => ty.clone(),
                None => Type::Int,
            };
            match ty {
                Type::Array(elem, _) => Type::Pointer(elem),
                other => other,
            }
        }
        Expr::Assign(lhs, _) | Expr::CompoundAssign(_, lhs, _) => {
            expr_type(lhs, var_map, func_table)
        }
        Expr::PostfixIncrement(inner) | Expr::PostfixDecrement(inner) => {
            expr_type(inner, var_map, func_table)
        }
        Expr::Unary(op, inner) => match op {
            UnaryOp::Not => Type::Int,
            _ => expr_type(inner, var_map, func_table),
        },
        Expr::Binary(op, left, right) => match op {
            BinaryOp::LogicalAnd
            | BinaryOp::LogicalOr
            | BinaryOp::LessThan
            | BinaryOp::LessEqual
            | BinaryOp::GreaterThan
            | BinaryOp::GreaterEqual
            | BinaryOp::Equal
            | BinaryOp::NotEqual => Type::Int,
            BinaryOp::Comma => expr_type(right, var_map, func_table),
            _ => expr_type(left, var_map, func_table),
        },
        Expr::Conditional { then_expr, .. } => expr_type(then_expr, var_map, func_table),
        Expr::FunctionCall(name, _) => {
            if let Some(info) = func_table.get(name) {
                info.return_type.clone()
            } else {
                Type::Int
            }
        }
        Expr::Dereref(inner) => {
            let inner_t = expr_type(inner, var_map, func_table);
            match inner_t {
                Type::Pointer(target) => *target,
                _ => Type::Int,
            }
        }
        Expr::AddrOf(inner) => {
            let inner_t = expr_type(inner, var_map, func_table);
            Type::Pointer(Box::new(inner_t))
        }
        Expr::SizeOfType(_) | Expr::SizeOfExpr(_) => Type::ULong,
        Expr::Dot(inner, member_name) => {
            let inner_t = expr_type(inner, var_map, func_table);
            if let Type::Struct { ref members, .. } = inner_t {
                struct_member_offset(members, member_name)
                    .map(|(_, t)| t)
                    .unwrap_or(Type::Int)
            } else {
                Type::Int
            }
        }
        Expr::CompoundInit(_) => Type::Int,
        Expr::VaStart(_) | Expr::VaEnd(_) | Expr::VaCopy(_, _) => Type::Void,
        Expr::VaArg { arg_type, .. } => arg_type.clone(),
    }
}

/// C の AST を TACKY IR に変換する。
pub fn generate_tacky(program: &Program) -> Result<TackyProgram> {
    let mut tgen = TackyGenerator::new();
    let mut func_table: HashMap<String, FunctionInfo> = HashMap::new();
    let mut global_var_map: HashMap<String, VarKind> = HashMap::new();

    // Pass 1: 関数・変数宣言を収集
    for decl in &program.declarations {
        match decl {
            TopLevelDecl::Function(func_decl) => {
                let has_body = func_decl.body.is_some();
                let param_types: Vec<Type> =
                    func_decl.params.iter().map(|(t, _)| t.clone()).collect();
                if let Some(existing) = func_table.get(&func_decl.name) {
                    if existing.param_count != func_decl.params.len() {
                        return Err(CompileError::CodegenError(format!(
                            "conflicting parameter count for function '{}'",
                            func_decl.name
                        )));
                    }
                    if has_body && existing.defined {
                        return Err(CompileError::CodegenError(format!(
                            "function '{}' defined multiple times",
                            func_decl.name
                        )));
                    }
                }
                let entry = func_table
                    .entry(func_decl.name.clone())
                    .or_insert(FunctionInfo {
                        param_count: func_decl.params.len(),
                        defined: false,
                        return_type: func_decl.return_type.clone(),
                        param_types: param_types.clone(),
                        is_variadic: func_decl.is_variadic,
                    });
                if has_body {
                    entry.defined = true;
                }
            }
            TopLevelDecl::Typedef { .. } => continue,
            TopLevelDecl::Variable(var_decl) => {
                if var_decl.name.is_empty() {
                    continue;
                }

                let sc = var_decl.storage_class;

                if sc == Some(StorageClass::Extern) && var_decl.init.is_some() {
                    return Err(CompileError::CodegenError(format!(
                        "extern variable '{}' cannot have initializer",
                        var_decl.name
                    )));
                }

                let init_val = if let Some(init_expr) = &var_decl.init {
                    TackyGenerator::resolve_static_init(
                        init_expr,
                        &mut tgen.string_constants,
                        &mut tgen.string_label_counter,
                    )
                    .map_err(|msg| {
                        CompileError::CodegenError(format!(
                            "file-scope variable '{}' {}",
                            var_decl.name, msg
                        ))
                    })?
                } else if var_decl.var_type.is_array() || var_decl.var_type.is_struct() {
                    TackyStaticInit::ZeroInit(var_decl.var_type.size())
                } else if var_decl.var_type == Type::Double {
                    TackyStaticInit::DoubleInit(0.0)
                } else {
                    TackyStaticInit::IntInit(0)
                };

                global_var_map.insert(
                    var_decl.name.clone(),
                    VarKind::Static(var_decl.name.clone(), var_decl.var_type.clone()),
                );

                match sc {
                    Some(StorageClass::Extern) => {}
                    Some(StorageClass::Static) => {
                        tgen.static_vars.push(TackyStaticVar {
                            name: var_decl.name.clone(),
                            global: false,
                            var_type: var_decl.var_type.clone(),
                            init: init_val,
                        });
                    }
                    None => {
                        tgen.static_vars.push(TackyStaticVar {
                            name: var_decl.name.clone(),
                            global: true,
                            var_type: var_decl.var_type.clone(),
                            init: init_val,
                        });
                    }
                }
            }
        }
    }

    // Pass 2: 関数定義のコード生成
    let mut functions = Vec::new();
    for decl in &program.declarations {
        if let TopLevelDecl::Function(func_decl) = decl
            && func_decl.body.is_some()
        {
            let global = func_decl.storage_class != Some(StorageClass::Static);
            functions.push(tgen.generate_function(
                func_decl,
                &func_table,
                &global_var_map,
                global,
            )?);
        }
    }

    // 定数プールを TackyStaticConstant に変換
    let mut static_constants: Vec<TackyStaticConstant> = tgen
        .double_constants
        .into_iter()
        .map(|(bits, (name, alignment))| TackyStaticConstant {
            name,
            alignment,
            init: TackyStaticInit::DoubleInit(f64::from_bits(bits)),
        })
        .collect();
    for (label, content) in tgen.string_constants {
        let byte_len = content.len() + 1;
        static_constants.push(TackyStaticConstant {
            name: label,
            alignment: 1,
            init: TackyStaticInit::StringInit(content, byte_len),
        });
    }
    static_constants.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(TackyProgram {
        functions,
        static_vars: tgen.static_vars,
        static_constants,
    })
}

impl TackyGenerator {
    fn generate_function(
        &mut self,
        func: &FunctionDecl,
        func_table: &HashMap<String, FunctionInfo>,
        global_var_map: &HashMap<String, VarKind>,
        global: bool,
    ) -> Result<TackyFunction> {
        // 関数ごとにテンポラリカウンタとvar_typesをリセット
        self.temp_counter = 0;
        self.var_types = HashMap::new();
        self.var_map = global_var_map.clone();
        self.current_func_params = func.params.iter().map(|(t, _)| t.clone()).collect();

        let params: Vec<String> = func.params.iter().map(|(_, name)| name.clone()).collect();

        // パラメータを変数マップに登録
        for (param_type, param_name) in &func.params {
            self.var_map
                .insert(param_name.clone(), VarKind::Local(param_type.clone()));
            self.var_types
                .insert(param_name.clone(), param_type.clone());
        }

        let body = func.body.as_ref().unwrap();
        let mut instrs = Vec::new();
        let mut scope_decls: HashSet<String> = HashSet::new();
        for (_, param_name) in &func.params {
            scope_decls.insert(param_name.clone());
        }

        for item in body {
            self.generate_block_item(item, &mut instrs, Some(&mut scope_decls), None, func_table)?;
        }

        // 暗黙の return
        if func.return_type.is_void() {
            instrs.push(TackyInstruction::ReturnVoid);
        } else if func.return_type == Type::Double {
            instrs.push(TackyInstruction::Return(TackyVal::Constant(
                TackyConst::Double(0.0),
            )));
        } else {
            let ret_val = match &func.return_type {
                Type::Long | Type::ULong | Type::Pointer(_) => {
                    TackyVal::Constant(TackyConst::Long(0))
                }
                _ => TackyVal::Constant(TackyConst::Int(0)),
            };
            instrs.push(TackyInstruction::Return(ret_val));
        }

        let static_var_names: HashSet<String> = self
            .var_map
            .iter()
            .filter_map(|(_name, kind)| match kind {
                VarKind::Static(label, _) => Some(label.clone()),
                _ => None,
            })
            .chain(
                // Also include the original names (for resolve_var_name lookups)
                self.var_map.iter().filter_map(|(name, kind)| match kind {
                    VarKind::Static(_, _) => Some(name.clone()),
                    _ => None,
                }),
            )
            .collect();

        Ok(TackyFunction {
            name: func.name.clone(),
            global,
            params,
            body: instrs,
            return_type: func.return_type.clone(),
            var_types: self.var_types.clone(),
            is_variadic: func.is_variadic,
            static_var_names,
        })
    }

    fn generate_block_item(
        &mut self,
        item: &BlockItem,
        instrs: &mut Vec<TackyInstruction>,
        scope_decls: Option<&mut HashSet<String>>,
        loop_labels: Option<&LoopLabels>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<()> {
        match item {
            BlockItem::Statement(stmt) => {
                self.generate_statement(stmt, instrs, loop_labels, func_table)
            }
            BlockItem::Declaration(decl) => {
                self.generate_declaration(decl, instrs, scope_decls, func_table)
            }
            BlockItem::Typedef { .. } => Ok(()), // No codegen for typedef
        }
    }

    fn generate_declaration(
        &mut self,
        decl: &Declaration,
        instrs: &mut Vec<TackyInstruction>,
        scope_decls: Option<&mut HashSet<String>>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<()> {
        if decl.name.is_empty() {
            return Ok(());
        }

        let sc = decl.storage_class;
        let is_struct = decl.var_type.is_struct();

        if sc == Some(StorageClass::Extern) {
            if decl.init.is_some() {
                return Err(CompileError::CodegenError(format!(
                    "extern variable '{}' cannot have initializer",
                    decl.name
                )));
            }
            self.var_map.insert(
                decl.name.clone(),
                VarKind::Static(decl.name.clone(), decl.var_type.clone()),
            );
            return Ok(());
        }

        if sc == Some(StorageClass::Static) {
            let init_val = if let Some(init_expr) = &decl.init {
                TackyGenerator::resolve_static_init(
                    init_expr,
                    &mut self.string_constants,
                    &mut self.string_label_counter,
                )
                .map_err(|msg| {
                    CompileError::CodegenError(format!("static variable '{}' {}", decl.name, msg))
                })?
            } else if decl.var_type.is_array() || is_struct {
                TackyStaticInit::ZeroInit(decl.var_type.size())
            } else if decl.var_type == Type::Double {
                TackyStaticInit::DoubleInit(0.0)
            } else {
                TackyStaticInit::IntInit(0)
            };

            let unique_label = format!("{}.{}", decl.name, self.static_label_counter);
            self.static_label_counter += 1;

            self.static_vars.push(TackyStaticVar {
                name: unique_label.clone(),
                global: false,
                var_type: decl.var_type.clone(),
                init: init_val,
            });

            self.var_map.insert(
                decl.name.clone(),
                VarKind::Static(unique_label, decl.var_type.clone()),
            );
            return Ok(());
        }

        // 通常のローカル変数
        if let Some(decls) = scope_decls
            && !decls.insert(decl.name.clone())
        {
            return Err(CompileError::CodegenError(format!(
                "variable '{}' already declared in this scope",
                decl.name
            )));
        }

        self.var_map
            .insert(decl.name.clone(), VarKind::Local(decl.var_type.clone()));
        self.var_types
            .insert(decl.name.clone(), decl.var_type.clone());

        if let Some(init) = &decl.init {
            if is_struct {
                if let Expr::CompoundInit(init_exprs) = init {
                    if let Type::Struct { ref members, .. } = decl.var_type {
                        self.generate_compound_init(
                            init_exprs, members, &decl.name, instrs, func_table,
                        )?;
                    }
                } else {
                    // 構造体コピー初期化: struct b = a;
                    let (src_val, _) = self.generate_expr(init, instrs, func_table)?;
                    let dst = TackyVal::Var(self.resolve_var_name(&decl.name));
                    instrs.push(TackyInstruction::CopyStruct {
                        src: src_val,
                        dst,
                        size: decl.var_type.size(),
                    });
                }
            } else if decl.var_type.is_array() {
                if let Expr::CompoundInit(init_exprs) = init {
                    self.generate_array_init(
                        init_exprs,
                        &decl.var_type,
                        &decl.name,
                        instrs,
                        func_table,
                    )?;
                }
            } else {
                let (val, _) = self.generate_expr(init, instrs, func_table)?;
                let dst_name = self.resolve_var_name(&decl.name);
                instrs.push(TackyInstruction::Copy {
                    src: val,
                    dst: TackyVal::Var(dst_name),
                });
            }
        }

        Ok(())
    }

    fn generate_statement(
        &mut self,
        stmt: &Statement,
        instrs: &mut Vec<TackyInstruction>,
        loop_labels: Option<&LoopLabels>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<()> {
        match stmt {
            Statement::Return(opt_expr) => match opt_expr {
                Some(expr) => {
                    let (val, _) = self.generate_expr(expr, instrs, func_table)?;
                    instrs.push(TackyInstruction::Return(val));
                }
                None => {
                    instrs.push(TackyInstruction::ReturnVoid);
                }
            },
            Statement::Expression(expr) => {
                self.generate_expr(expr, instrs, func_table)?;
            }
            Statement::Null => {}
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let (cond_val, cond_type) = self.generate_expr(condition, instrs, func_table)?;

                if let Some(else_stmt) = else_branch {
                    let else_label = self.new_label("if_else");
                    let end_label = self.new_label("if_end");

                    let cond_int = self.convert_to_condition(cond_val, &cond_type, instrs);
                    instrs.push(TackyInstruction::JumpIfZero {
                        condition: cond_int,
                        target: else_label.clone(),
                    });
                    self.generate_statement(then_branch, instrs, loop_labels, func_table)?;
                    instrs.push(TackyInstruction::Jump(end_label.clone()));
                    instrs.push(TackyInstruction::Label(else_label));
                    self.generate_statement(else_stmt, instrs, loop_labels, func_table)?;
                    instrs.push(TackyInstruction::Label(end_label));
                } else {
                    let end_label = self.new_label("if_end");
                    let cond_int = self.convert_to_condition(cond_val, &cond_type, instrs);
                    instrs.push(TackyInstruction::JumpIfZero {
                        condition: cond_int,
                        target: end_label.clone(),
                    });
                    self.generate_statement(then_branch, instrs, loop_labels, func_table)?;
                    instrs.push(TackyInstruction::Label(end_label));
                }
            }
            Statement::Compound(items) => {
                let saved_var_map = self.var_map.clone();
                let mut scope_decls: HashSet<String> = HashSet::new();
                for item in items {
                    self.generate_block_item(
                        item,
                        instrs,
                        Some(&mut scope_decls),
                        loop_labels,
                        func_table,
                    )?;
                }
                self.var_map = saved_var_map;
            }
            Statement::While { condition, body } => {
                let start_label = self.new_label("while_start");
                let end_label = self.new_label("while_end");
                let labels = LoopLabels {
                    break_label: end_label.clone(),
                    continue_label: Some(start_label.clone()),
                };

                instrs.push(TackyInstruction::Label(start_label.clone()));
                let (cond_val, cond_type) = self.generate_expr(condition, instrs, func_table)?;
                let cond_int = self.convert_to_condition(cond_val, &cond_type, instrs);
                instrs.push(TackyInstruction::JumpIfZero {
                    condition: cond_int,
                    target: end_label.clone(),
                });
                self.generate_statement(body, instrs, Some(&labels), func_table)?;
                instrs.push(TackyInstruction::Jump(start_label));
                instrs.push(TackyInstruction::Label(end_label));
            }
            Statement::DoWhile { body, condition } => {
                let start_label = self.new_label("do_start");
                let continue_label = self.new_label("do_continue");
                let end_label = self.new_label("do_end");
                let labels = LoopLabels {
                    break_label: end_label.clone(),
                    continue_label: Some(continue_label.clone()),
                };

                instrs.push(TackyInstruction::Label(start_label.clone()));
                self.generate_statement(body, instrs, Some(&labels), func_table)?;
                instrs.push(TackyInstruction::Label(continue_label));
                let (cond_val, cond_type) = self.generate_expr(condition, instrs, func_table)?;
                let cond_int = self.convert_to_condition(cond_val, &cond_type, instrs);
                instrs.push(TackyInstruction::JumpIfNotZero {
                    condition: cond_int,
                    target: start_label,
                });
                instrs.push(TackyInstruction::Label(end_label));
            }
            Statement::For {
                init,
                condition,
                post,
                body,
            } => {
                let start_label = self.new_label("for_start");
                let continue_label = self.new_label("for_continue");
                let end_label = self.new_label("for_end");
                let labels = LoopLabels {
                    break_label: end_label.clone(),
                    continue_label: Some(continue_label.clone()),
                };

                let saved_var_map = self.var_map.clone();

                match init {
                    ForInit::Declaration(decls) => {
                        for decl in decls {
                            self.generate_declaration(decl, instrs, None, func_table)?;
                        }
                    }
                    ForInit::Expression(Some(expr)) => {
                        self.generate_expr(expr, instrs, func_table)?;
                    }
                    ForInit::Expression(None) => {}
                }

                instrs.push(TackyInstruction::Label(start_label.clone()));

                if let Some(cond) = condition {
                    let (cond_val, cond_type) = self.generate_expr(cond, instrs, func_table)?;
                    let cond_int = self.convert_to_condition(cond_val, &cond_type, instrs);
                    instrs.push(TackyInstruction::JumpIfZero {
                        condition: cond_int,
                        target: end_label.clone(),
                    });
                }

                self.generate_statement(body, instrs, Some(&labels), func_table)?;

                instrs.push(TackyInstruction::Label(continue_label));
                if let Some(post_expr) = post {
                    self.generate_expr(post_expr, instrs, func_table)?;
                }

                instrs.push(TackyInstruction::Jump(start_label));
                instrs.push(TackyInstruction::Label(end_label));

                self.var_map = saved_var_map;
            }
            Statement::Break => {
                let labels = loop_labels
                    .ok_or_else(|| CompileError::CodegenError("break outside loop".to_string()))?;
                instrs.push(TackyInstruction::Jump(labels.break_label.clone()));
            }
            Statement::Continue => {
                let labels = loop_labels.ok_or_else(|| {
                    CompileError::CodegenError("continue outside loop".to_string())
                })?;
                let continue_target = labels.continue_label.as_ref().ok_or_else(|| {
                    CompileError::CodegenError("continue outside loop".to_string())
                })?;
                instrs.push(TackyInstruction::Jump(continue_target.clone()));
            }
            Statement::Switch { expr, body } => {
                let switch_end = self.new_label("switch_end");

                // switch 式を評価
                let (switch_val, switch_type) = self.generate_expr(expr, instrs, func_table)?;

                // Pass 1: case/default ラベルを収集
                let (cases, default_label) = self.collect_switch_labels(body)?;

                // 比較チェーン
                for (case_val, case_label) in &cases {
                    let case_const = self.make_case_constant(*case_val, &switch_type);
                    let cmp_result = self.new_temp(Type::Int);
                    instrs.push(TackyInstruction::Binary {
                        op: TackyBinaryOp::Equal,
                        left: switch_val.clone(),
                        right: case_const,
                        dst: cmp_result.clone(),
                    });
                    instrs.push(TackyInstruction::JumpIfNotZero {
                        condition: cmp_result,
                        target: case_label.clone(),
                    });
                }

                // default or switch_end
                let fallthrough_target =
                    default_label.clone().unwrap_or_else(|| switch_end.clone());
                instrs.push(TackyInstruction::Jump(fallthrough_target));

                // switch 用 LoopLabels: break → switch_end, continue → 外側ループの continue
                let outer_continue = loop_labels.and_then(|l| l.continue_label.clone());
                let switch_labels = LoopLabels {
                    break_label: switch_end.clone(),
                    continue_label: outer_continue,
                };

                // Pass 2: case 本体をソース順でラベル付きで生成（fall-through）
                self.generate_switch_body(
                    body,
                    instrs,
                    &cases,
                    &default_label,
                    &switch_labels,
                    func_table,
                )?;

                instrs.push(TackyInstruction::Label(switch_end));
            }
            Statement::Goto(label) => {
                instrs.push(TackyInstruction::Jump(format!(".Luser_{label}")));
            }
            Statement::Label { name, body } => {
                instrs.push(TackyInstruction::Label(format!(".Luser_{name}")));
                self.generate_statement(body, instrs, loop_labels, func_table)?;
            }
            Statement::Case { .. } => {
                return Err(CompileError::CodegenError(
                    "case outside switch".to_string(),
                ));
            }
            Statement::Default(_) => {
                return Err(CompileError::CodegenError(
                    "default outside switch".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// switch body の AST を走査し、case/default ラベルを収集する（Pass 1）。
    /// ネストした switch には再帰しない。
    #[allow(clippy::type_complexity)]
    fn collect_switch_labels(
        &mut self,
        stmt: &Statement,
    ) -> Result<(Vec<(i64, String)>, Option<String>)> {
        let mut cases: Vec<(i64, String)> = Vec::new();
        let mut default_label: Option<String> = None;
        self.collect_labels_recursive(stmt, &mut cases, &mut default_label)?;
        Ok((cases, default_label))
    }

    fn collect_labels_recursive(
        &mut self,
        stmt: &Statement,
        cases: &mut Vec<(i64, String)>,
        default_label: &mut Option<String>,
    ) -> Result<()> {
        match stmt {
            Statement::Case { value, body } => {
                // 重複チェック
                if cases.iter().any(|(v, _)| *v == *value) {
                    return Err(CompileError::CodegenError(format!(
                        "duplicate case value: {}",
                        value
                    )));
                }
                let label = self.new_label("case");
                cases.push((*value, label));
                self.collect_labels_recursive(body, cases, default_label)?;
            }
            Statement::Default(body) => {
                if default_label.is_some() {
                    return Err(CompileError::CodegenError(
                        "multiple default labels in switch".to_string(),
                    ));
                }
                *default_label = Some(self.new_label("default"));
                self.collect_labels_recursive(body, cases, default_label)?;
            }
            Statement::Compound(items) => {
                for item in items {
                    if let BlockItem::Statement(s) = item {
                        self.collect_labels_recursive(s, cases, default_label)?;
                    }
                }
            }
            Statement::If {
                then_branch,
                else_branch,
                ..
            } => {
                self.collect_labels_recursive(then_branch, cases, default_label)?;
                if let Some(else_stmt) = else_branch {
                    self.collect_labels_recursive(else_stmt, cases, default_label)?;
                }
            }
            Statement::While { body, .. } | Statement::DoWhile { body, .. } => {
                self.collect_labels_recursive(body, cases, default_label)?;
            }
            Statement::For { body, .. } => {
                self.collect_labels_recursive(body, cases, default_label)?;
            }
            Statement::Label { body, .. } => {
                self.collect_labels_recursive(body, cases, default_label)?;
            }
            // ネストした switch には再帰しない
            Statement::Switch { .. } => {}
            _ => {}
        }
        Ok(())
    }

    /// switch 式の型に合わせた case 定数を生成する。
    fn make_case_constant(&self, value: i64, switch_type: &Type) -> TackyVal {
        match switch_type {
            Type::Long => TackyVal::Constant(TackyConst::Long(value)),
            Type::UInt => TackyVal::Constant(TackyConst::UInt(value as u32)),
            Type::ULong => TackyVal::Constant(TackyConst::ULong(value as u64)),
            Type::Char => TackyVal::Constant(TackyConst::Char(value as i8)),
            Type::UChar => TackyVal::Constant(TackyConst::UChar(value as u8)),
            _ => TackyVal::Constant(TackyConst::Int(value as i32)),
        }
    }

    /// switch body のコード生成（Pass 2）。
    /// case/default のラベルを正しい位置に挿入しながら文を生成する。
    fn generate_switch_body(
        &mut self,
        stmt: &Statement,
        instrs: &mut Vec<TackyInstruction>,
        cases: &[(i64, String)],
        default_label: &Option<String>,
        switch_labels: &LoopLabels,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<()> {
        match stmt {
            Statement::Case { value, body } => {
                if let Some((_, label)) = cases.iter().find(|(v, _)| *v == *value) {
                    instrs.push(TackyInstruction::Label(label.clone()));
                }
                self.generate_switch_body(
                    body,
                    instrs,
                    cases,
                    default_label,
                    switch_labels,
                    func_table,
                )?;
            }
            Statement::Default(body) => {
                if let Some(label) = default_label {
                    instrs.push(TackyInstruction::Label(label.clone()));
                }
                self.generate_switch_body(
                    body,
                    instrs,
                    cases,
                    default_label,
                    switch_labels,
                    func_table,
                )?;
            }
            Statement::Compound(items) => {
                let saved_var_map = self.var_map.clone();
                for item in items {
                    match item {
                        BlockItem::Statement(s) => {
                            self.generate_switch_body(
                                s,
                                instrs,
                                cases,
                                default_label,
                                switch_labels,
                                func_table,
                            )?;
                        }
                        BlockItem::Declaration(decl) => {
                            self.generate_declaration(decl, instrs, None, func_table)?;
                        }
                        BlockItem::Typedef { .. } => {}
                    }
                }
                self.var_map = saved_var_map;
            }
            // 通常の文はそのまま生成（switch_labels を渡して break/continue を処理）
            _ => {
                self.generate_statement(stmt, instrs, Some(switch_labels), func_table)?;
            }
        }
        Ok(())
    }

    /// double/pointer 条件値を int（0/非0）の値に変換（JumpIfZero/JumpIfNotZero 用）
    /// 整数型はそのまま使える。double は != 0.0 の比較結果を返す。
    fn convert_to_condition(
        &mut self,
        val: TackyVal,
        ty: &Type,
        instrs: &mut Vec<TackyInstruction>,
    ) -> TackyVal {
        if ty.is_double() {
            let result = self.new_temp(Type::Int);
            instrs.push(TackyInstruction::Binary {
                op: TackyBinaryOp::NotEqual,
                left: val,
                right: TackyVal::Constant(TackyConst::Double(0.0)),
                dst: result.clone(),
            });
            result
        } else {
            val
        }
    }

    /// 式を TACKY に変換する。結果値と結果の型を返す。
    fn generate_expr(
        &mut self,
        expr: &Expr,
        instrs: &mut Vec<TackyInstruction>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<(TackyVal, Type)> {
        match expr {
            Expr::Constant(value) => Ok((
                TackyVal::Constant(TackyConst::Int(*value as i32)),
                Type::Int,
            )),
            Expr::ConstantLong(value) => {
                Ok((TackyVal::Constant(TackyConst::Long(*value)), Type::Long))
            }
            Expr::ConstantUInt(value) => Ok((
                TackyVal::Constant(TackyConst::UInt(*value as u32)),
                Type::UInt,
            )),
            Expr::ConstantULong(value) => {
                Ok((TackyVal::Constant(TackyConst::ULong(*value)), Type::ULong))
            }
            Expr::ConstantDouble(value) => {
                Ok((TackyVal::Constant(TackyConst::Double(*value)), Type::Double))
            }
            // 文字列リテラル: `.Lstr_N` ラベルで static_constants に StringInit として登録し、
            // GetAddress でそのアドレスを取得する。
            // 難読化時は obfuscate::string_encryption() が StringInit を暗号化 ByteArrayInit に変換する。
            Expr::StringLiteral(content) => {
                let label = format!(".Lstr_{}", self.string_label_counter);
                self.string_label_counter += 1;
                self.string_constants.push((label.clone(), content.clone()));
                let dst = self.new_temp(Type::Pointer(Box::new(Type::Char)));
                instrs.push(TackyInstruction::GetAddress {
                    src: TackyVal::Var(label),
                    dst: dst.clone(),
                });
                Ok((dst, Type::Pointer(Box::new(Type::Char))))
            }

            Expr::Cast {
                target_type,
                source_type,
                expr: inner,
            } => {
                let (src_val, _) = self.generate_expr(inner, instrs, func_table)?;

                // (void)expr — evaluate and discard
                if target_type.is_void() {
                    return Ok((TackyVal::Constant(TackyConst::Int(0)), Type::Void));
                }

                // Double ↔ Integer conversions
                if source_type.is_double() && !target_type.is_double() {
                    // Double → Integer
                    match target_type {
                        Type::Int | Type::Long | Type::Char | Type::UChar => {
                            let dst = self.new_temp(target_type.clone());
                            instrs.push(TackyInstruction::DoubleToInt {
                                src: src_val,
                                dst: dst.clone(),
                            });
                            Ok((dst, target_type.clone()))
                        }
                        Type::UInt => {
                            // double → uint: convert to long first, then truncate
                            let tmp_long = self.new_temp(Type::Long);
                            instrs.push(TackyInstruction::DoubleToInt {
                                src: src_val,
                                dst: tmp_long.clone(),
                            });
                            let dst = self.new_temp(Type::UInt);
                            instrs.push(TackyInstruction::Truncate {
                                src: tmp_long,
                                dst: dst.clone(),
                            });
                            Ok((dst, Type::UInt))
                        }
                        Type::ULong => {
                            let dst = self.new_temp(Type::ULong);
                            instrs.push(TackyInstruction::DoubleToUInt {
                                src: src_val,
                                dst: dst.clone(),
                            });
                            Ok((dst, Type::ULong))
                        }
                        _ => unreachable!("double to {:?}", target_type),
                    }
                } else if !source_type.is_double() && target_type.is_double() {
                    // Integer → Double
                    match source_type {
                        Type::ULong => {
                            let dst = self.new_temp(Type::Double);
                            instrs.push(TackyInstruction::UIntToDouble {
                                src: src_val,
                                dst: dst.clone(),
                            });
                            Ok((dst, Type::Double))
                        }
                        Type::UInt => {
                            // Zero-extend to long first, then convert
                            let tmp_long = self.new_temp(Type::Long);
                            instrs.push(TackyInstruction::ZeroExtend {
                                src: src_val,
                                dst: tmp_long.clone(),
                            });
                            let dst = self.new_temp(Type::Double);
                            instrs.push(TackyInstruction::IntToDouble {
                                src: tmp_long,
                                dst: dst.clone(),
                            });
                            Ok((dst, Type::Double))
                        }
                        Type::UChar => {
                            // Zero-extend byte to int, then convert
                            let tmp_int = self.new_temp(Type::Int);
                            instrs.push(TackyInstruction::ZeroExtend {
                                src: src_val,
                                dst: tmp_int.clone(),
                            });
                            let dst = self.new_temp(Type::Double);
                            instrs.push(TackyInstruction::IntToDouble {
                                src: tmp_int,
                                dst: dst.clone(),
                            });
                            Ok((dst, Type::Double))
                        }
                        Type::Char => {
                            // Sign-extend byte to int, then convert
                            let tmp_int = self.new_temp(Type::Int);
                            instrs.push(TackyInstruction::SignExtend {
                                src: src_val,
                                dst: tmp_int.clone(),
                            });
                            let dst = self.new_temp(Type::Double);
                            instrs.push(TackyInstruction::IntToDouble {
                                src: tmp_int,
                                dst: dst.clone(),
                            });
                            Ok((dst, Type::Double))
                        }
                        _ => {
                            let dst = self.new_temp(Type::Double);
                            instrs.push(TackyInstruction::IntToDouble {
                                src: src_val,
                                dst: dst.clone(),
                            });
                            Ok((dst, Type::Double))
                        }
                    }
                } else if !source_type.is_double() && !target_type.is_double() {
                    // Integer ↔ Integer
                    let src_size = source_type.size();
                    let dst_size = target_type.size();
                    if src_size == dst_size {
                        // Same size: no-op (just copy with new type for tracking)
                        Ok((src_val, target_type.clone()))
                    } else if src_size < dst_size {
                        let dst = self.new_temp(target_type.clone());
                        if source_type.is_unsigned() {
                            instrs.push(TackyInstruction::ZeroExtend {
                                src: src_val,
                                dst: dst.clone(),
                            });
                        } else {
                            instrs.push(TackyInstruction::SignExtend {
                                src: src_val,
                                dst: dst.clone(),
                            });
                        }
                        Ok((dst, target_type.clone()))
                    } else {
                        let dst = self.new_temp(target_type.clone());
                        instrs.push(TackyInstruction::Truncate {
                            src: src_val,
                            dst: dst.clone(),
                        });
                        Ok((dst, target_type.clone()))
                    }
                } else {
                    // Double → Double: no-op
                    Ok((src_val, target_type.clone()))
                }
            }

            Expr::Var(name) if name == "__func__" || name == "__FUNCTION__" => {
                // C99 predefined identifier — emit as string constant
                let label = format!(".Lstr_{}", self.string_label_counter);
                self.string_label_counter += 1;
                self.string_constants
                    .push((label.clone(), "?".to_string()));
                let dst = self.new_temp(Type::Pointer(Box::new(Type::Char)));
                instrs.push(TackyInstruction::GetAddress {
                    src: TackyVal::Var(label),
                    dst: dst.clone(),
                });
                Ok((dst, Type::Pointer(Box::new(Type::Char))))
            }

            Expr::Var(name) => {
                // Function name used as value → get function address
                if !self.var_map.contains_key(name)
                    && !self.var_types.contains_key(name)
                    && func_table.contains_key(name)
                {
                    let dst = self.new_temp(Type::Pointer(Box::new(Type::Void)));
                    instrs.push(TackyInstruction::GetAddress {
                        src: TackyVal::Var(name.to_string()),
                        dst: dst.clone(),
                    });
                    return Ok((dst, Type::Pointer(Box::new(Type::Void))));
                }

                let resolved = self.resolve_var_name(name);
                let ty = self.var_type(name);
                match &ty {
                    Type::Array(_, _) | Type::Struct { .. } => {
                        // 配列/構造体: アドレスを返す
                        let dst = self.new_temp(if ty.is_array() {
                            Type::Pointer(Box::new(ty.target_type().unwrap().clone()))
                        } else {
                            Type::Pointer(Box::new(ty.clone()))
                        });
                        instrs.push(TackyInstruction::GetAddress {
                            src: TackyVal::Var(resolved),
                            dst: dst.clone(),
                        });
                        let result_type = if ty.is_array() {
                            Type::Pointer(Box::new(ty.target_type().unwrap().clone()))
                        } else {
                            // struct はアドレスで表現
                            ty.clone()
                        };
                        Ok((dst, result_type))
                    }
                    _ => Ok((TackyVal::Var(resolved), ty)),
                }
            }

            Expr::Assign(lhs, rhs) => {
                let lhs_type = expr_type(lhs, &self.var_map, func_table);

                // 構造体代入
                if lhs_type.is_struct() {
                    let struct_size = lhs_type.size();
                    let (lhs_addr, _) = self.generate_lvalue_addr(lhs, instrs, func_table)?;
                    let (rhs_val, _) = self.generate_expr(rhs, instrs, func_table)?;
                    // Store struct: copy rhs to *lhs_addr
                    // Use CopyStruct but we need to express src=rhs_val addr, dst=lhs_addr
                    // rhs_val is already an address (struct type)
                    let _dst_tmp = self.new_temp(lhs_type.clone());
                    // CopyStruct copies from src address to dst address
                    // We need to Load each part... Let's use Store approach:
                    // Actually, CopyStruct { src: rhs_addr, dst: lhs_addr, size }
                    instrs.push(TackyInstruction::CopyStruct {
                        src: rhs_val.clone(),
                        dst: lhs_addr,
                        size: struct_size,
                    });
                    return Ok((rhs_val, lhs_type));
                }

                if let Expr::Var(name) = lhs.as_ref() {
                    // 変数への直接代入
                    let (rhs_val, _) = self.generate_expr(rhs, instrs, func_table)?;
                    let dst_name = self.resolve_var_name(name);
                    instrs.push(TackyInstruction::Copy {
                        src: rhs_val.clone(),
                        dst: TackyVal::Var(dst_name.clone()),
                    });
                    Ok((TackyVal::Var(dst_name), lhs_type))
                } else {
                    // 一般的な左辺値（*ptr 等）への代入
                    let (lhs_addr, _) = self.generate_lvalue_addr(lhs, instrs, func_table)?;
                    let (rhs_val, _) = self.generate_expr(rhs, instrs, func_table)?;
                    instrs.push(TackyInstruction::Store {
                        src: rhs_val.clone(),
                        dst_ptr: lhs_addr,
                    });
                    Ok((rhs_val, lhs_type))
                }
            }

            Expr::Unary(op, inner) => match op {
                UnaryOp::PreIncrement | UnaryOp::PreDecrement => {
                    self.generate_pre_inc_dec(op, inner, instrs, func_table)
                }
                _ => {
                    let (src_val, src_type) = self.generate_expr(inner, instrs, func_table)?;
                    match op {
                        UnaryOp::Negate => {
                            let result_type = src_type.clone();
                            let dst = self.new_temp(result_type.clone());
                            instrs.push(TackyInstruction::Unary {
                                op: TackyUnaryOp::Negate,
                                src: src_val,
                                dst: dst.clone(),
                            });
                            Ok((dst, result_type))
                        }
                        UnaryOp::Complement => {
                            let result_type = src_type.clone();
                            let dst = self.new_temp(result_type.clone());
                            instrs.push(TackyInstruction::Unary {
                                op: TackyUnaryOp::Complement,
                                src: src_val,
                                dst: dst.clone(),
                            });
                            Ok((dst, result_type))
                        }
                        UnaryOp::Not => {
                            let dst = self.new_temp(Type::Int);
                            instrs.push(TackyInstruction::Unary {
                                op: TackyUnaryOp::Not,
                                src: src_val,
                                dst: dst.clone(),
                            });
                            Ok((dst, Type::Int))
                        }
                        _ => unreachable!(),
                    }
                }
            },

            Expr::Conditional {
                condition,
                then_expr,
                else_expr,
            } => {
                let else_label = self.new_label("tern_else");
                let end_label = self.new_label("tern_end");
                let result_type = expr_type(expr, &self.var_map, func_table);

                let (cond_val, cond_type) = self.generate_expr(condition, instrs, func_table)?;
                let cond_int = self.convert_to_condition(cond_val, &cond_type, instrs);
                instrs.push(TackyInstruction::JumpIfZero {
                    condition: cond_int,
                    target: else_label.clone(),
                });

                let (then_val, _) = self.generate_expr(then_expr, instrs, func_table)?;
                let result = self.new_temp(result_type.clone());
                if !result_type.is_void() {
                    instrs.push(TackyInstruction::Copy {
                        src: then_val,
                        dst: result.clone(),
                    });
                }
                instrs.push(TackyInstruction::Jump(end_label.clone()));

                instrs.push(TackyInstruction::Label(else_label));
                let (else_val, _) = self.generate_expr(else_expr, instrs, func_table)?;
                if !result_type.is_void() {
                    instrs.push(TackyInstruction::Copy {
                        src: else_val,
                        dst: result.clone(),
                    });
                }
                instrs.push(TackyInstruction::Label(end_label));

                Ok((result, result_type))
            }

            Expr::CompoundAssign(op, lhs, rhs) => {
                self.generate_compound_assign(op, lhs, rhs, instrs, func_table)
            }

            Expr::PostfixIncrement(inner) => {
                self.generate_postfix_inc_dec(inner, true, instrs, func_table)
            }

            Expr::PostfixDecrement(inner) => {
                self.generate_postfix_inc_dec(inner, false, instrs, func_table)
            }

            Expr::FunctionCall(name, args) => {
                self.generate_function_call(name, args, instrs, func_table)
            }

            Expr::Binary(op, left, right) => {
                self.generate_binary(op, left, right, instrs, func_table)
            }

            Expr::SizeOfType(_) | Expr::SizeOfExpr(_) => {
                unreachable!("sizeof should be resolved by type checker")
            }

            Expr::AddrOf(inner) => {
                let (addr, _) = self.generate_lvalue_addr(inner, instrs, func_table)?;
                let inner_type = expr_type(inner, &self.var_map, func_table);
                let result_type = Type::Pointer(Box::new(inner_type));
                Ok((addr, result_type))
            }

            Expr::Dereref(inner) => {
                let target_type = expr_type(expr, &self.var_map, func_table);
                if target_type.is_struct() {
                    // struct pointer deref: just return the address
                    return self.generate_expr(inner, instrs, func_table);
                }
                let (ptr_val, _) = self.generate_expr(inner, instrs, func_table)?;
                let dst = self.new_temp(target_type.clone());
                instrs.push(TackyInstruction::Load {
                    src_ptr: ptr_val,
                    dst: dst.clone(),
                });
                Ok((dst, target_type))
            }

            Expr::Dot(inner, member_name) => {
                let inner_type = expr_type(inner, &self.var_map, func_table);
                if let Type::Struct { ref members, .. } = inner_type {
                    let (member_off, member_type) = struct_member_offset(members, member_name)
                        .ok_or_else(|| {
                            CompileError::CodegenError(format!(
                                "no member '{}' in struct",
                                member_name
                            ))
                        })?;

                    // Get inner's address
                    let (inner_addr, _) = self.generate_lvalue_addr(inner, instrs, func_table)?;

                    if member_type.is_struct() {
                        // Nested struct: return address + offset
                        if member_off != 0 {
                            let offset_val =
                                TackyVal::Constant(TackyConst::Long(member_off as i64));
                            let result =
                                self.new_temp(Type::Pointer(Box::new(member_type.clone())));
                            instrs.push(TackyInstruction::Binary {
                                op: TackyBinaryOp::Add,
                                left: inner_addr,
                                right: offset_val,
                                dst: result.clone(),
                            });
                            return Ok((result, member_type));
                        } else {
                            return Ok((inner_addr, member_type));
                        }
                    }

                    // Load member from offset
                    let _inner_name = match &inner_addr {
                        TackyVal::Var(n) => n.clone(),
                        _ => {
                            // Store address to temp, use CopyFromOffset
                            let tmp = self.new_temp(Type::Pointer(Box::new(inner_type.clone())));
                            instrs.push(TackyInstruction::Copy {
                                src: inner_addr.clone(),
                                dst: tmp.clone(),
                            });
                            if let TackyVal::Var(n) = &tmp {
                                n.clone()
                            } else {
                                unreachable!()
                            }
                        }
                    };

                    // Use pointer arithmetic + Load for member access
                    if member_off != 0 {
                        let offset_ptr =
                            self.new_temp(Type::Pointer(Box::new(member_type.clone())));
                        instrs.push(TackyInstruction::Binary {
                            op: TackyBinaryOp::Add,
                            left: inner_addr,
                            right: TackyVal::Constant(TackyConst::Long(member_off as i64)),
                            dst: offset_ptr.clone(),
                        });
                        let dst = self.new_temp(member_type.clone());
                        instrs.push(TackyInstruction::Load {
                            src_ptr: offset_ptr,
                            dst: dst.clone(),
                        });
                        Ok((dst, member_type))
                    } else {
                        let dst = self.new_temp(member_type.clone());
                        instrs.push(TackyInstruction::Load {
                            src_ptr: inner_addr,
                            dst: dst.clone(),
                        });
                        Ok((dst, member_type))
                    }
                } else {
                    Err(CompileError::CodegenError(
                        "member access on non-struct type".to_string(),
                    ))
                }
            }

            Expr::CompoundInit(_) => {
                unreachable!("CompoundInit should be handled in generate_declaration")
            }

            Expr::VaStart(ap) => {
                let (ap_val, _) = self.generate_expr(ap, instrs, func_table)?;

                // 現在の関数の名前付きパラメータから gp/fp offset を計算
                let mut named_gp_count: i32 = 0;
                let mut named_xmm_count: i32 = 0;
                for param_type in &self.current_func_params {
                    if param_type.is_double() {
                        if named_xmm_count < 8 {
                            named_xmm_count += 1;
                        }
                    } else if named_gp_count < 6 {
                        named_gp_count += 1;
                    }
                }
                let gp_offset_init = named_gp_count * 8;
                let fp_offset_init = 48 + named_xmm_count * 16;

                instrs.push(TackyInstruction::VaStart {
                    ap: ap_val,
                    gp_offset_init,
                    fp_offset_init,
                });
                Ok((TackyVal::Constant(TackyConst::Int(0)), Type::Void))
            }

            Expr::VaArg { ap, arg_type } => {
                let (ap_val, _) = self.generate_expr(ap, instrs, func_table)?;

                let dst = self.new_temp(arg_type.clone());
                instrs.push(TackyInstruction::VaArg {
                    ap: ap_val,
                    dst: dst.clone(),
                    arg_type: arg_type.clone(),
                });
                Ok((dst, arg_type.clone()))
            }

            Expr::VaEnd(_ap) => {
                instrs.push(TackyInstruction::VaEnd);
                Ok((TackyVal::Constant(TackyConst::Int(0)), Type::Void))
            }

            Expr::VaCopy(dst, src) => {
                let (dst_val, _) = self.generate_expr(dst, instrs, func_table)?;
                let (src_val, _) = self.generate_expr(src, instrs, func_table)?;
                // va_list is a 24-byte struct on x86-64 SysV ABI
                instrs.push(TackyInstruction::CopyStruct {
                    src: src_val,
                    dst: dst_val,
                    size: 24,
                });
                Ok((TackyVal::Constant(TackyConst::Int(0)), Type::Void))
            }
        }
    }

    /// 左辺値のアドレスを計算する
    fn generate_lvalue_addr(
        &mut self,
        expr: &Expr,
        instrs: &mut Vec<TackyInstruction>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<(TackyVal, Type)> {
        match expr {
            Expr::Var(name) => {
                let resolved = self.resolve_var_name(name);
                let ty = self.var_type(name);
                let dst = self.new_temp(Type::Pointer(Box::new(ty.clone())));
                instrs.push(TackyInstruction::GetAddress {
                    src: TackyVal::Var(resolved),
                    dst: dst.clone(),
                });
                Ok((dst, ty))
            }
            Expr::Dereref(inner) => {
                // *ptr: inner の値がアドレスそのもの
                let (val, _) = self.generate_expr(inner, instrs, func_table)?;
                let target_type = expr_type(expr, &self.var_map, func_table);
                Ok((val, target_type))
            }
            Expr::Dot(inner, member_name) => {
                let inner_type = expr_type(inner, &self.var_map, func_table);
                if let Type::Struct { ref members, .. } = inner_type {
                    let (member_off, member_type) = struct_member_offset(members, member_name)
                        .ok_or_else(|| {
                            CompileError::CodegenError(format!(
                                "no member '{}' in struct",
                                member_name
                            ))
                        })?;

                    let (inner_addr, _) = self.generate_lvalue_addr(inner, instrs, func_table)?;

                    if member_off != 0 {
                        let result = self.new_temp(Type::Pointer(Box::new(member_type.clone())));
                        instrs.push(TackyInstruction::Binary {
                            op: TackyBinaryOp::Add,
                            left: inner_addr,
                            right: TackyVal::Constant(TackyConst::Long(member_off as i64)),
                            dst: result.clone(),
                        });
                        Ok((result, member_type))
                    } else {
                        Ok((inner_addr, member_type))
                    }
                } else {
                    Err(CompileError::CodegenError(
                        "member access on non-struct type".to_string(),
                    ))
                }
            }
            _ => Err(CompileError::CodegenError(
                "cannot take address of non-lvalue expression".to_string(),
            )),
        }
    }

    /// 前置インクリメント/デクリメント
    fn generate_pre_inc_dec(
        &mut self,
        op: &UnaryOp,
        inner: &Expr,
        instrs: &mut Vec<TackyInstruction>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<(TackyVal, Type)> {
        let is_increment = matches!(op, UnaryOp::PreIncrement);

        if let Expr::Var(name) = inner {
            let resolved = self.resolve_var_name(name);
            let ty = self.var_type(name);
            let var_val = TackyVal::Var(resolved.clone());

            let result = self.prefix_compute_new(&var_val, &ty, is_increment, instrs);
            instrs.push(TackyInstruction::Copy {
                src: result.clone(),
                dst: TackyVal::Var(resolved),
            });

            Ok((result, ty))
        } else {
            // 一般的な lvalue（構造体メンバ、ポインタ間接参照等）
            let (addr, ty) = self.generate_lvalue_addr(inner, instrs, func_table)?;

            let old_val = self.new_temp(ty.clone());
            instrs.push(TackyInstruction::Load {
                src_ptr: addr.clone(),
                dst: old_val.clone(),
            });

            let result = self.prefix_compute_new(&old_val, &ty, is_increment, instrs);
            instrs.push(TackyInstruction::Store {
                src: result.clone(),
                dst_ptr: addr,
            });

            Ok((result, ty))
        }
    }

    fn prefix_compute_new(
        &mut self,
        old_val: &TackyVal,
        ty: &Type,
        is_increment: bool,
        instrs: &mut Vec<TackyInstruction>,
    ) -> TackyVal {
        let result = self.new_temp(ty.clone());
        if ty.is_double() {
            let inc_val = TackyVal::Constant(TackyConst::Double(1.0));
            let tacky_op = if is_increment {
                TackyBinaryOp::AddDouble
            } else {
                TackyBinaryOp::SubDouble
            };
            instrs.push(TackyInstruction::Binary {
                op: tacky_op,
                left: old_val.clone(),
                right: inc_val,
                dst: result.clone(),
            });
        } else {
            let increment = if ty.is_pointer() {
                ty.target_type().unwrap().size() as i64
            } else {
                1
            };
            let inc_const = self.make_increment_const(ty, increment);
            let tacky_op = if is_increment {
                TackyBinaryOp::Add
            } else {
                TackyBinaryOp::Subtract
            };
            instrs.push(TackyInstruction::Binary {
                op: tacky_op,
                left: old_val.clone(),
                right: inc_const,
                dst: result.clone(),
            });
        }
        result
    }

    /// 後置インクリメント/デクリメント
    fn generate_postfix_inc_dec(
        &mut self,
        inner: &Expr,
        is_increment: bool,
        instrs: &mut Vec<TackyInstruction>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<(TackyVal, Type)> {
        if let Expr::Var(name) = inner {
            let resolved = self.resolve_var_name(name);
            let ty = self.var_type(name);
            let var_val = TackyVal::Var(resolved.clone());

            let old_val = self.new_temp(ty.clone());
            instrs.push(TackyInstruction::Copy {
                src: var_val.clone(),
                dst: old_val.clone(),
            });

            let new_val = self.postfix_compute_new(&old_val, &ty, is_increment, instrs);
            instrs.push(TackyInstruction::Copy {
                src: new_val,
                dst: TackyVal::Var(resolved),
            });

            Ok((old_val, ty))
        } else {
            // 一般的な lvalue（構造体メンバ、ポインタ間接参照等）
            let (addr, ty) = self.generate_lvalue_addr(inner, instrs, func_table)?;

            let old_val = self.new_temp(ty.clone());
            instrs.push(TackyInstruction::Load {
                src_ptr: addr.clone(),
                dst: old_val.clone(),
            });

            let new_val = self.postfix_compute_new(&old_val, &ty, is_increment, instrs);
            instrs.push(TackyInstruction::Store {
                src: new_val,
                dst_ptr: addr,
            });

            Ok((old_val, ty))
        }
    }

    fn postfix_compute_new(
        &mut self,
        old_val: &TackyVal,
        ty: &Type,
        is_increment: bool,
        instrs: &mut Vec<TackyInstruction>,
    ) -> TackyVal {
        let new_val = self.new_temp(ty.clone());
        if ty.is_double() {
            let inc_val = TackyVal::Constant(TackyConst::Double(1.0));
            let tacky_op = if is_increment {
                TackyBinaryOp::AddDouble
            } else {
                TackyBinaryOp::SubDouble
            };
            instrs.push(TackyInstruction::Binary {
                op: tacky_op,
                left: old_val.clone(),
                right: inc_val,
                dst: new_val.clone(),
            });
        } else {
            let increment = if ty.is_pointer() {
                ty.target_type().unwrap().size() as i64
            } else {
                1
            };
            let inc_const = self.make_increment_const(ty, increment);
            let tacky_op = if is_increment {
                TackyBinaryOp::Add
            } else {
                TackyBinaryOp::Subtract
            };
            instrs.push(TackyInstruction::Binary {
                op: tacky_op,
                left: old_val.clone(),
                right: inc_const,
                dst: new_val.clone(),
            });
        }
        new_val
    }

    fn make_increment_const(&self, ty: &Type, increment: i64) -> TackyVal {
        match ty {
            Type::Long | Type::ULong | Type::Pointer(_) => {
                TackyVal::Constant(TackyConst::Long(increment))
            }
            _ => TackyVal::Constant(TackyConst::Int(increment as i32)),
        }
    }

    /// 関数呼び出し
    fn generate_function_call(
        &mut self,
        name: &str,
        args: &[Expr],
        instrs: &mut Vec<TackyInstruction>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<(TackyVal, Type)> {
        // GCC builtins: evaluate args, return first arg value
        if name.starts_with("__builtin_") {
            let (val, ty) = self.generate_expr(&args[0], instrs, func_table)?;
            for arg in args.iter().skip(1) {
                let _ = self.generate_expr(arg, instrs, func_table)?;
            }
            return Ok((val, ty));
        }

        if let Some(info) = func_table.get(name) {
            if info.is_variadic {
                if args.len() < info.param_count {
                    return Err(CompileError::CodegenError(format!(
                        "function '{}' requires at least {} arguments, got {}",
                        name,
                        info.param_count,
                        args.len()
                    )));
                }
            } else if info.param_count != args.len() {
                return Err(CompileError::CodegenError(format!(
                    "function '{}' expects {} arguments, got {}",
                    name,
                    info.param_count,
                    args.len()
                )));
            }
        }

        let return_type = func_table
            .get(name)
            .map(|info| info.return_type.clone())
            .unwrap_or(Type::Int);

        // Evaluate all arguments
        let mut arg_vals = Vec::new();
        for arg in args {
            let (val, _) = self.generate_expr(arg, instrs, func_table)?;
            arg_vals.push(val);
        }

        let is_variadic = func_table
            .get(name)
            .map(|info| info.is_variadic)
            .unwrap_or(false);

        let dst = self.new_temp(return_type.clone());
        instrs.push(TackyInstruction::FunCall {
            name: name.to_string(),
            args: arg_vals,
            dst: dst.clone(),
            dst_type: return_type.clone(),
            is_variadic,
        });

        Ok((dst, return_type))
    }

    /// 二項演算
    fn generate_binary(
        &mut self,
        op: &BinaryOp,
        left: &Expr,
        right: &Expr,
        instrs: &mut Vec<TackyInstruction>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<(TackyVal, Type)> {
        let result_type = expr_type(
            &Expr::Binary(*op, Box::new(left.clone()), Box::new(right.clone())),
            &self.var_map,
            func_table,
        );
        let operand_type = expr_type(left, &self.var_map, func_table);

        match op {
            BinaryOp::LogicalAnd => {
                let false_label = self.new_label("and_false");
                let end_label = self.new_label("and_end");

                let (left_val, left_type) = self.generate_expr(left, instrs, func_table)?;
                let left_cond = self.convert_to_condition(left_val, &left_type, instrs);
                instrs.push(TackyInstruction::JumpIfZero {
                    condition: left_cond,
                    target: false_label.clone(),
                });

                let (right_val, right_type) = self.generate_expr(right, instrs, func_table)?;
                let right_cond = self.convert_to_condition(right_val, &right_type, instrs);
                instrs.push(TackyInstruction::JumpIfZero {
                    condition: right_cond,
                    target: false_label.clone(),
                });

                let result = self.new_temp(Type::Int);
                instrs.push(TackyInstruction::Copy {
                    src: TackyVal::Constant(TackyConst::Int(1)),
                    dst: result.clone(),
                });
                instrs.push(TackyInstruction::Jump(end_label.clone()));
                instrs.push(TackyInstruction::Label(false_label));
                instrs.push(TackyInstruction::Copy {
                    src: TackyVal::Constant(TackyConst::Int(0)),
                    dst: result.clone(),
                });
                instrs.push(TackyInstruction::Label(end_label));
                Ok((result, Type::Int))
            }

            BinaryOp::LogicalOr => {
                let true_label = self.new_label("or_true");
                let end_label = self.new_label("or_end");

                let (left_val, left_type) = self.generate_expr(left, instrs, func_table)?;
                let left_cond = self.convert_to_condition(left_val, &left_type, instrs);
                instrs.push(TackyInstruction::JumpIfNotZero {
                    condition: left_cond,
                    target: true_label.clone(),
                });

                let (right_val, right_type) = self.generate_expr(right, instrs, func_table)?;
                let right_cond = self.convert_to_condition(right_val, &right_type, instrs);
                instrs.push(TackyInstruction::JumpIfNotZero {
                    condition: right_cond,
                    target: true_label.clone(),
                });

                let result = self.new_temp(Type::Int);
                instrs.push(TackyInstruction::Copy {
                    src: TackyVal::Constant(TackyConst::Int(0)),
                    dst: result.clone(),
                });
                instrs.push(TackyInstruction::Jump(end_label.clone()));
                instrs.push(TackyInstruction::Label(true_label));
                instrs.push(TackyInstruction::Copy {
                    src: TackyVal::Constant(TackyConst::Int(1)),
                    dst: result.clone(),
                });
                instrs.push(TackyInstruction::Label(end_label));
                Ok((result, Type::Int))
            }

            BinaryOp::Comma => {
                self.generate_expr(left, instrs, func_table)?;
                self.generate_expr(right, instrs, func_table)
            }

            BinaryOp::Add => {
                let left_type = expr_type(left, &self.var_map, func_table);

                if left_type.is_pointer() {
                    // ptr + int
                    let elem_size = left_type.target_type().unwrap().size();
                    let (ptr_val, _) = self.generate_expr(left, instrs, func_table)?;
                    let (idx_val, _) = self.generate_expr(right, instrs, func_table)?;
                    let dst = self.new_temp(left_type.clone());
                    instrs.push(TackyInstruction::AddPtr {
                        ptr: ptr_val,
                        index: idx_val,
                        scale: elem_size,
                        dst: dst.clone(),
                    });
                    Ok((dst, left_type))
                } else if operand_type.is_double() {
                    let (left_val, _) = self.generate_expr(left, instrs, func_table)?;
                    let (right_val, _) = self.generate_expr(right, instrs, func_table)?;
                    let dst = self.new_temp(Type::Double);
                    instrs.push(TackyInstruction::Binary {
                        op: TackyBinaryOp::AddDouble,
                        left: left_val,
                        right: right_val,
                        dst: dst.clone(),
                    });
                    Ok((dst, Type::Double))
                } else {
                    let (left_val, _) = self.generate_expr(left, instrs, func_table)?;
                    let (right_val, _) = self.generate_expr(right, instrs, func_table)?;
                    let dst = self.new_temp(result_type.clone());
                    instrs.push(TackyInstruction::Binary {
                        op: TackyBinaryOp::Add,
                        left: left_val,
                        right: right_val,
                        dst: dst.clone(),
                    });
                    Ok((dst, result_type))
                }
            }

            BinaryOp::Subtract => {
                let left_type = expr_type(left, &self.var_map, func_table);
                let right_type = expr_type(right, &self.var_map, func_table);

                if left_type.is_pointer() && right_type.is_pointer() {
                    // ptr - ptr: element count difference
                    let elem_size = left_type.target_type().unwrap().size();
                    let (left_val, _) = self.generate_expr(left, instrs, func_table)?;
                    let (right_val, _) = self.generate_expr(right, instrs, func_table)?;
                    // byte_diff = left - right
                    let byte_diff = self.new_temp(Type::Long);
                    instrs.push(TackyInstruction::Binary {
                        op: TackyBinaryOp::Subtract,
                        left: left_val,
                        right: right_val,
                        dst: byte_diff.clone(),
                    });
                    if elem_size != 1 {
                        let divisor = TackyVal::Constant(TackyConst::Long(elem_size as i64));
                        let dst = self.new_temp(Type::Long);
                        instrs.push(TackyInstruction::Binary {
                            op: TackyBinaryOp::Divide,
                            left: byte_diff,
                            right: divisor,
                            dst: dst.clone(),
                        });
                        Ok((dst, Type::Long))
                    } else {
                        Ok((byte_diff, Type::Long))
                    }
                } else if left_type.is_pointer() && !right_type.is_pointer() {
                    // ptr - int: scaled subtraction
                    let elem_size = left_type.target_type().unwrap().size();
                    let (ptr_val, _) = self.generate_expr(left, instrs, func_table)?;
                    let (idx_val, _) = self.generate_expr(right, instrs, func_table)?;
                    // Scale the index
                    let scaled = if elem_size != 1 {
                        let tmp = self.new_temp(Type::Long);
                        instrs.push(TackyInstruction::Binary {
                            op: TackyBinaryOp::Multiply,
                            left: idx_val,
                            right: TackyVal::Constant(TackyConst::Long(elem_size as i64)),
                            dst: tmp.clone(),
                        });
                        tmp
                    } else {
                        idx_val
                    };
                    let dst = self.new_temp(left_type.clone());
                    instrs.push(TackyInstruction::Binary {
                        op: TackyBinaryOp::Subtract,
                        left: ptr_val,
                        right: scaled,
                        dst: dst.clone(),
                    });
                    Ok((dst, left_type))
                } else if operand_type.is_double() {
                    let (left_val, _) = self.generate_expr(left, instrs, func_table)?;
                    let (right_val, _) = self.generate_expr(right, instrs, func_table)?;
                    let dst = self.new_temp(Type::Double);
                    instrs.push(TackyInstruction::Binary {
                        op: TackyBinaryOp::SubDouble,
                        left: left_val,
                        right: right_val,
                        dst: dst.clone(),
                    });
                    Ok((dst, Type::Double))
                } else {
                    let (left_val, _) = self.generate_expr(left, instrs, func_table)?;
                    let (right_val, _) = self.generate_expr(right, instrs, func_table)?;
                    let dst = self.new_temp(result_type.clone());
                    instrs.push(TackyInstruction::Binary {
                        op: TackyBinaryOp::Subtract,
                        left: left_val,
                        right: right_val,
                        dst: dst.clone(),
                    });
                    Ok((dst, result_type))
                }
            }

            BinaryOp::Multiply => {
                let (left_val, _) = self.generate_expr(left, instrs, func_table)?;
                let (right_val, _) = self.generate_expr(right, instrs, func_table)?;
                let dst = self.new_temp(result_type.clone());
                let tacky_op = if operand_type.is_double() {
                    TackyBinaryOp::MulDouble
                } else {
                    TackyBinaryOp::Multiply
                };
                instrs.push(TackyInstruction::Binary {
                    op: tacky_op,
                    left: left_val,
                    right: right_val,
                    dst: dst.clone(),
                });
                Ok((dst, result_type))
            }

            BinaryOp::Divide => {
                let (left_val, _) = self.generate_expr(left, instrs, func_table)?;
                let (right_val, _) = self.generate_expr(right, instrs, func_table)?;
                let dst = self.new_temp(result_type.clone());
                let tacky_op = if operand_type.is_double() {
                    TackyBinaryOp::DivDouble
                } else {
                    TackyBinaryOp::Divide
                };
                instrs.push(TackyInstruction::Binary {
                    op: tacky_op,
                    left: left_val,
                    right: right_val,
                    dst: dst.clone(),
                });
                Ok((dst, result_type))
            }

            BinaryOp::Remainder => {
                let (left_val, _) = self.generate_expr(left, instrs, func_table)?;
                let (right_val, _) = self.generate_expr(right, instrs, func_table)?;
                let dst = self.new_temp(result_type.clone());
                instrs.push(TackyInstruction::Binary {
                    op: TackyBinaryOp::Remainder,
                    left: left_val,
                    right: right_val,
                    dst: dst.clone(),
                });
                Ok((dst, result_type))
            }

            BinaryOp::LessThan
            | BinaryOp::LessEqual
            | BinaryOp::GreaterThan
            | BinaryOp::GreaterEqual
            | BinaryOp::Equal
            | BinaryOp::NotEqual => {
                let (left_val, _) = self.generate_expr(left, instrs, func_table)?;
                let (right_val, _) = self.generate_expr(right, instrs, func_table)?;
                let dst = self.new_temp(Type::Int);
                let tacky_op = match op {
                    BinaryOp::LessThan => TackyBinaryOp::LessThan,
                    BinaryOp::LessEqual => TackyBinaryOp::LessOrEqual,
                    BinaryOp::GreaterThan => TackyBinaryOp::GreaterThan,
                    BinaryOp::GreaterEqual => TackyBinaryOp::GreaterOrEqual,
                    BinaryOp::Equal => TackyBinaryOp::Equal,
                    BinaryOp::NotEqual => TackyBinaryOp::NotEqual,
                    _ => unreachable!(),
                };
                instrs.push(TackyInstruction::Binary {
                    op: tacky_op,
                    left: left_val,
                    right: right_val,
                    dst: dst.clone(),
                });
                Ok((dst, Type::Int))
            }

            BinaryOp::BitwiseAnd
            | BinaryOp::BitwiseOr
            | BinaryOp::BitwiseXor
            | BinaryOp::ShiftLeft
            | BinaryOp::ShiftRight => {
                let (left_val, _) = self.generate_expr(left, instrs, func_table)?;
                let (right_val, _) = self.generate_expr(right, instrs, func_table)?;
                let dst = self.new_temp(result_type.clone());
                let tacky_op = match op {
                    BinaryOp::BitwiseAnd => TackyBinaryOp::BitwiseAnd,
                    BinaryOp::BitwiseOr => TackyBinaryOp::BitwiseOr,
                    BinaryOp::BitwiseXor => TackyBinaryOp::BitwiseXor,
                    BinaryOp::ShiftLeft => TackyBinaryOp::ShiftLeft,
                    BinaryOp::ShiftRight => TackyBinaryOp::ShiftRight,
                    _ => unreachable!(),
                };
                instrs.push(TackyInstruction::Binary {
                    op: tacky_op,
                    left: left_val,
                    right: right_val,
                    dst: dst.clone(),
                });
                Ok((dst, result_type))
            }
        }
    }

    /// 複合代入式の生成
    fn generate_compound_assign(
        &mut self,
        op: &BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        instrs: &mut Vec<TackyInstruction>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<(TackyVal, Type)> {
        if let Expr::Var(name) = lhs {
            let resolved = self.resolve_var_name(name);
            let var_type = self.var_type(name);
            let var_val = TackyVal::Var(resolved.clone());

            match op {
                BinaryOp::Add | BinaryOp::Subtract => {
                    if var_type.is_pointer() {
                        // ptr += int / ptr -= int
                        let elem_size = var_type.target_type().unwrap().size();
                        let (rhs_val, _) = self.generate_expr(rhs, instrs, func_table)?;

                        if matches!(op, BinaryOp::Add) {
                            let dst = self.new_temp(var_type.clone());
                            instrs.push(TackyInstruction::AddPtr {
                                ptr: var_val,
                                index: rhs_val,
                                scale: elem_size,
                                dst: dst.clone(),
                            });
                            instrs.push(TackyInstruction::Copy {
                                src: dst.clone(),
                                dst: TackyVal::Var(resolved),
                            });
                            return Ok((dst, var_type));
                        } else {
                            // ptr -= int: scale and subtract
                            let scaled = if elem_size != 1 {
                                let tmp = self.new_temp(Type::Long);
                                instrs.push(TackyInstruction::Binary {
                                    op: TackyBinaryOp::Multiply,
                                    left: rhs_val,
                                    right: TackyVal::Constant(TackyConst::Long(elem_size as i64)),
                                    dst: tmp.clone(),
                                });
                                tmp
                            } else {
                                rhs_val
                            };
                            let dst = self.new_temp(var_type.clone());
                            instrs.push(TackyInstruction::Binary {
                                op: TackyBinaryOp::Subtract,
                                left: var_val,
                                right: scaled,
                                dst: dst.clone(),
                            });
                            instrs.push(TackyInstruction::Copy {
                                src: dst.clone(),
                                dst: TackyVal::Var(resolved),
                            });
                            return Ok((dst, var_type));
                        }
                    }
                }
                _ => {}
            }

            // Normal compound assignment
            let (rhs_val, _) = self.generate_expr(rhs, instrs, func_table)?;
            let dst = self.new_temp(var_type.clone());

            let tacky_op = if var_type.is_double() {
                match op {
                    BinaryOp::Add => TackyBinaryOp::AddDouble,
                    BinaryOp::Subtract => TackyBinaryOp::SubDouble,
                    BinaryOp::Multiply => TackyBinaryOp::MulDouble,
                    BinaryOp::Divide => TackyBinaryOp::DivDouble,
                    _ => {
                        return Err(CompileError::CodegenError(format!(
                            "unsupported compound assignment operator: {:?}",
                            op
                        )));
                    }
                }
            } else {
                match op {
                    BinaryOp::Add => TackyBinaryOp::Add,
                    BinaryOp::Subtract => TackyBinaryOp::Subtract,
                    BinaryOp::Multiply => TackyBinaryOp::Multiply,
                    BinaryOp::Divide => TackyBinaryOp::Divide,
                    BinaryOp::Remainder => TackyBinaryOp::Remainder,
                    BinaryOp::BitwiseAnd => TackyBinaryOp::BitwiseAnd,
                    BinaryOp::BitwiseOr => TackyBinaryOp::BitwiseOr,
                    BinaryOp::BitwiseXor => TackyBinaryOp::BitwiseXor,
                    BinaryOp::ShiftLeft => TackyBinaryOp::ShiftLeft,
                    BinaryOp::ShiftRight => TackyBinaryOp::ShiftRight,
                    _ => {
                        return Err(CompileError::CodegenError(format!(
                            "unsupported compound assignment operator: {:?}",
                            op
                        )));
                    }
                }
            };

            instrs.push(TackyInstruction::Binary {
                op: tacky_op,
                left: var_val,
                right: rhs_val,
                dst: dst.clone(),
            });
            instrs.push(TackyInstruction::Copy {
                src: dst.clone(),
                dst: TackyVal::Var(resolved),
            });

            Ok((dst, var_type))
        } else {
            // General lvalue: use generate_lvalue_addr + Load/Store
            let (addr, var_type) = self.generate_lvalue_addr(lhs, instrs, func_table)?;

            let old_val = self.new_temp(var_type.clone());
            instrs.push(TackyInstruction::Load {
                src_ptr: addr.clone(),
                dst: old_val.clone(),
            });

            let (rhs_val, _) = self.generate_expr(rhs, instrs, func_table)?;
            let dst = self.new_temp(var_type.clone());

            let tacky_op = if var_type.is_double() {
                match op {
                    BinaryOp::Add => TackyBinaryOp::AddDouble,
                    BinaryOp::Subtract => TackyBinaryOp::SubDouble,
                    BinaryOp::Multiply => TackyBinaryOp::MulDouble,
                    BinaryOp::Divide => TackyBinaryOp::DivDouble,
                    _ => {
                        return Err(CompileError::CodegenError(format!(
                            "unsupported compound assignment operator: {:?}",
                            op
                        )));
                    }
                }
            } else if var_type.is_pointer() && matches!(op, BinaryOp::Add) {
                let elem_size = var_type.target_type().unwrap().size();
                instrs.push(TackyInstruction::AddPtr {
                    ptr: old_val.clone(),
                    index: rhs_val,
                    scale: elem_size,
                    dst: dst.clone(),
                });
                instrs.push(TackyInstruction::Store {
                    src: dst.clone(),
                    dst_ptr: addr,
                });
                return Ok((dst, var_type));
            } else {
                match op {
                    BinaryOp::Add => TackyBinaryOp::Add,
                    BinaryOp::Subtract => TackyBinaryOp::Subtract,
                    BinaryOp::Multiply => TackyBinaryOp::Multiply,
                    BinaryOp::Divide => TackyBinaryOp::Divide,
                    BinaryOp::Remainder => TackyBinaryOp::Remainder,
                    BinaryOp::BitwiseAnd => TackyBinaryOp::BitwiseAnd,
                    BinaryOp::BitwiseOr => TackyBinaryOp::BitwiseOr,
                    BinaryOp::BitwiseXor => TackyBinaryOp::BitwiseXor,
                    BinaryOp::ShiftLeft => TackyBinaryOp::ShiftLeft,
                    BinaryOp::ShiftRight => TackyBinaryOp::ShiftRight,
                    _ => {
                        return Err(CompileError::CodegenError(format!(
                            "unsupported compound assignment operator: {:?}",
                            op
                        )));
                    }
                }
            };

            instrs.push(TackyInstruction::Binary {
                op: tacky_op,
                left: old_val,
                right: rhs_val,
                dst: dst.clone(),
            });
            instrs.push(TackyInstruction::Store {
                src: dst.clone(),
                dst_ptr: addr,
            });

            Ok((dst, var_type))
        }
    }

    /// 構造体の複合初期化子
    fn generate_compound_init(
        &mut self,
        init_exprs: &[Expr],
        members: &[crate::parse::ast::MemberDecl],
        dst_name: &str,
        instrs: &mut Vec<TackyInstruction>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<()> {
        let resolved_dst = self.resolve_var_name(dst_name);
        let mut member_offset: usize = 0;

        for (init_expr, member) in init_exprs.iter().zip(members.iter()) {
            let align = member.member_type.alignment();
            if !member_offset.is_multiple_of(align) {
                member_offset += align - (member_offset % align);
            }

            let (val, _) = self.generate_expr(init_expr, instrs, func_table)?;
            instrs.push(TackyInstruction::CopyToOffset {
                src: val,
                dst: resolved_dst.clone(),
                offset: member_offset,
            });

            member_offset += member.member_type.size();
        }

        Ok(())
    }

    /// 配列の複合初期化子（ローカル変数用）
    fn generate_array_init(
        &mut self,
        init_exprs: &[Expr],
        array_type: &Type,
        dst_name: &str,
        instrs: &mut Vec<TackyInstruction>,
        func_table: &HashMap<String, FunctionInfo>,
    ) -> Result<()> {
        let (elem_type, count) = match array_type {
            Type::Array(e, c) => (e, *c),
            _ => unreachable!(),
        };
        let resolved_dst = self.resolve_var_name(dst_name);
        let elem_size = elem_type.size();

        // 明示的な要素を CopyToOffset で書き込み
        for (i, init_expr) in init_exprs.iter().enumerate() {
            let (val, _) = self.generate_expr(init_expr, instrs, func_table)?;
            instrs.push(TackyInstruction::CopyToOffset {
                src: val,
                dst: resolved_dst.clone(),
                offset: i * elem_size,
            });
        }

        // 残りをゼロ初期化（init_exprs.len() < count の場合）
        if init_exprs.len() < count {
            for i in init_exprs.len()..count {
                let zero = if **elem_type == Type::Double {
                    TackyVal::Constant(TackyConst::Double(0.0))
                } else {
                    match elem_type.size() {
                        8 => TackyVal::Constant(TackyConst::Long(0)),
                        _ => TackyVal::Constant(TackyConst::Int(0)),
                    }
                };
                instrs.push(TackyInstruction::CopyToOffset {
                    src: zero,
                    dst: resolved_dst.clone(),
                    offset: i * elem_size,
                });
            }
        }

        Ok(())
    }
}
