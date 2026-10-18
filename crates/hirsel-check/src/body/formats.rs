//! F-string steps (SEM-3). The parser desugars an f-string into a block whose
//! steps call `ExprKind::Formatter` callees; each becomes an ordinary call of
//! std@1's formatting protocol, so later phases see only ordinary calls.
use super::*;

/// The std@1 operation appending a value of this type, and the declared ordinals
/// of the written `(value, mut builder)` arguments.
fn append_operation(primitive: Primitive) -> Option<(&'static str, &'static str, [usize; 2])> {
    use Primitive::*;
    let module = match primitive {
        String => return Some(("text", "append", [1, 0])),
        Bool => "bool",
        U8 => "u8",
        U16 => "u16",
        U32 => "u32",
        U64 => "u64",
        U128 => "u128",
        I8 => "i8",
        I16 => "i16",
        I32 => "i32",
        I64 => "i64",
        I128 => "i128",
        F32 => "f32",
        F64 => "f64",
        _ => return None,
    };
    Some((module, "format_into", [0, 1]))
}
fn std_operation(module: &str, name: &str) -> ast::DefPath {
    ast::DefPath(ast::ItemPath {
        package: ast::PackageId {
            name: "std".into(),
            version: "1".into(),
        },
        modules: vec![module.into()],
        name: name.into(),
    })
}
impl Checker<'_, '_> {
    /// An append checks its value first, with no expected type, because the
    /// value's type selects the operation; the finish is `text.finish_valid`.
    pub(super) fn start_format_step(
        &mut self,
        id: ast::ExprId,
        callee: ast::ExprId,
        step: ast::FormatStep,
        arguments: &[ast::Argument],
        expected: Option<TypeId>,
        frame: usize,
    ) -> Result<(), QueryError> {
        match step {
            ast::FormatStep::Append | ast::FormatStep::AppendSpec(_) => {
                self.work
                    .push(Work::AfterChild(After::FormatAppend(id, expected, frame)));
                self.work.push(Work::Enter(arguments[0].value, None, frame));
                Ok(())
            }
            ast::FormatStep::Finish => self.format_call(
                id,
                callee,
                std_operation("text", "finish_valid"),
                vec![0],
                expected,
                frame,
            ),
        }
    }
    pub(super) fn format_append(
        &mut self,
        id: ast::ExprId,
        expected: Option<TypeId>,
        frame: usize,
    ) -> Result<(), QueryError> {
        let ast::ExprKind::Call { callee, arguments } =
            self.input.tree.expressions[id.0 as usize].kind.clone()
        else {
            unreachable!()
        };
        let value = arguments[0].value;
        let ty = self.value(value);
        if self.invalid(ty) {
            return self.recover_format(id, callee, &arguments, expected, frame);
        }
        let operation = match self.kind(ty) {
            TypeKind::Primitive(primitive) => append_operation(primitive),
            _ => None,
        };
        let Some((mut module, mut name, ordinals)) = operation else {
            self.report(
                const { RuleId::of("SEM-3") },
                self.span(value),
                &format!(
                    "an f-string hole of type `{}` has no text formatting; String, bool, \
                     integers and floats format, so convert the value to one of them first",
                    self.type_spelling(ty)
                ),
            );
            return self.recover_format(id, callee, &arguments, expected, frame);
        };
        let mut ordinals = ordinals.to_vec();
        if let ast::ExprKind::Formatter(ast::FormatStep::AppendSpec(spec)) =
            self.input.tree.expressions[callee.0 as usize].kind
        {
            let primitive = match self.kind(ty) {
                TypeKind::Primitive(p) => p,
                _ => unreachable!(),
            };
            let integer = matches!(
                primitive,
                Primitive::U8
                    | Primitive::U16
                    | Primitive::U32
                    | Primitive::U64
                    | Primitive::U128
                    | Primitive::I8
                    | Primitive::I16
                    | Primitive::I32
                    | Primitive::I64
                    | Primitive::I128
            );
            if spec.kind != 0 && !integer {
                self.report(
                    const { RuleId::of("SEM-3") },
                    self.span(value),
                    &format!(
                        "format type `{}` requires an integer; `{}` does not support it",
                        char::from_u32(spec.kind as u32).unwrap_or('?'),
                        self.type_spelling(ty)
                    ),
                );
                return self.recover_format(id, callee, &arguments, expected, frame);
            }
            if primitive == Primitive::String {
                module = "text";
            }
            name = "format_into_spec";
            ordinals = (0..arguments.len()).collect();
        }
        self.format_call(
            id,
            callee,
            std_operation(module, name),
            ordinals,
            expected,
            frame,
        )
    }
    /// The callee becomes a named reference to `operation`, as a resolved path
    /// would be. Every written argument but the final builder, an append's
    /// value, is already checked.
    fn format_call(
        &mut self,
        id: ast::ExprId,
        callee: ast::ExprId,
        operation: ast::DefPath,
        ordinals: Vec<usize>,
        expected: Option<TypeId>,
        frame: usize,
    ) -> Result<(), QueryError> {
        let ast::ExprKind::Call { arguments, .. } =
            self.input.tree.expressions[id.0 as usize].kind.clone()
        else {
            unreachable!()
        };
        let signature = match self.snapshot(&operation) {
            Ok(signature) => signature,
            Err(QueryError::NotImplemented) => {
                self.report(
                    const { RuleId::of("SEM-3") },
                    self.span(id),
                    &format!(
                        "f-strings format through std@1, which this program does not select \
                         ({operation} is unavailable); add std@1 to its manifest's dependencies"
                    ),
                );
                return self.recover_format(id, callee, &arguments, expected, frame);
            }
            Err(QueryError::Diagnostics(_)) => {
                return self.recover_format(id, callee, &arguments, expected, frame);
            }
            Err(e) => return Err(e),
        };
        let SignatureKind::Function(f) = &signature.kind else {
            return Err(QueryError::Operational(OperationalFailure {
                operation: "f-string".into(),
                message: format!("{operation} is not a function"),
            }));
        };
        let f = self.import_function(f, &signature.types)?;
        let ty = self.named_function(f);
        self.put(callee, ty, None, UseKind::Borrow)?;
        self.body
            .resolution
            .expressions
            .push((callee, ResolvedName::Definition(operation.clone())));
        self.formatters.insert(callee, (operation, ordinals));
        self.start_call(id, expected, frame, usize::from(arguments.len() > 1))
    }
    fn recover_format(
        &mut self,
        id: ast::ExprId,
        callee: ast::ExprId,
        arguments: &[ast::Argument],
        expected: Option<TypeId>,
        frame: usize,
    ) -> Result<(), QueryError> {
        let e = self.error();
        self.put(callee, e, None, UseKind::Borrow)?;
        self.body.calls.push(CheckedCall {
            expression: id,
            target: CallTarget::Value(callee),
            arguments: arguments
                .iter()
                .map(|a| CheckedArgument {
                    expression: a.value,
                    parameter: None,
                })
                .collect(),
            generic_arguments: vec![],
            result: e,
            state: CallState::Recovery,
        });
        self.put(id, e, expected, UseKind::Borrow)?;
        for argument in arguments[arguments.len() - 1..].iter().rev() {
            self.work.push(Work::Enter(argument.value, Some(e), frame));
        }
        Ok(())
    }
}
