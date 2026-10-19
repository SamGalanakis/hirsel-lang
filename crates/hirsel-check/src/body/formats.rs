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
        Char => "char",
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
/// Integers and floats pad on the right by default, other text on the left.
fn numeric(primitive: Primitive) -> bool {
    !matches!(
        primitive,
        Primitive::String | Primitive::Bool | Primitive::Char
    )
}
impl Checker<'_, '_> {
    /// An append checks its value first, with no expected type, because the
    /// value's type selects the operation; a mark is `core list.len`, a pad
    /// is selected by its hole's type, and the finish is `text.finish_valid`.
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
            ast::FormatStep::Mark => self.format_call(
                id,
                callee,
                ast::DefPath::core("list.len"),
                vec![0],
                expected,
                frame,
                0,
            ),
            ast::FormatStep::Pad(hole) => {
                let ty = self.value(hole);
                let primitive = match self.kind(ty) {
                    TypeKind::Primitive(p) if append_operation(p).is_some() => Some(p),
                    TypeKind::Named { definition, .. }
                        if self.format_association(&definition)?.is_some() =>
                    {
                        None
                    }
                    // The hole's append reported its type.
                    _ => return self.recover_format(id, callee, arguments, expected, frame, 0),
                };
                let name = if primitive.is_some_and(numeric) {
                    "pad_number_from"
                } else {
                    "pad_from"
                };
                self.format_call(
                    id,
                    callee,
                    std_operation("text", name),
                    (0..arguments.len()).collect(),
                    expected,
                    frame,
                    0,
                )
            }
            ast::FormatStep::Finish => self.format_call(
                id,
                callee,
                std_operation("text", "finish_valid"),
                vec![0],
                expected,
                frame,
                0,
            ),
        }
    }
    /// The function a nominal type's `@format` association names, if any.
    fn format_association(
        &mut self,
        definition: &ast::DefPath,
    ) -> Result<Option<ast::DefPath>, QueryError> {
        Ok(match &self.snapshot(definition)?.kind {
            SignatureKind::Type(nominal) => nominal.format.clone(),
            _ => None,
        })
    }
    pub(super) fn format_append(
        &mut self,
        id: ast::ExprId,
        expected: Option<TypeId>,
        frame: usize,
    ) -> Result<(), QueryError> {
        let ast::ExprKind::Call { callee, arguments } = self.input.syntax.tree.expressions
            [id.0 as usize]
            .kind
            .clone()
        else {
            unreachable!()
        };
        let value = arguments[0].value;
        let ty = self.value(value);
        if self.invalid(ty) {
            return self.recover_format(id, callee, &arguments, expected, frame, 1);
        }
        let spec = match self.input.syntax.tree.expressions[callee.0 as usize].kind {
            ast::ExprKind::Formatter(ast::FormatStep::AppendSpec(spec)) => Some(spec),
            _ => None,
        };
        let primitive = match self.kind(ty) {
            TypeKind::Primitive(p) => append_operation(p).map(|_| p),
            _ => None,
        };
        let association = match self.kind(ty) {
            TypeKind::Named { definition, .. } => self.format_association(&definition)?,
            _ => None,
        };
        let (operation, ordinals) = match (primitive, association, spec) {
            (Some(primitive), _, None) => {
                let (module, name, ordinals) = append_operation(primitive).unwrap();
                (std_operation(module, name), ordinals.to_vec())
            }
            (Some(Primitive::Char), _, Some(_)) => {
                self.report(
                    const { RuleId::of("SEM-3") },
                    self.span(value),
                    "a char hole takes only fill, alignment and width; precision, sign, `#`, \
                     `0` and radix formats apply to bool, integers, floats and String",
                );
                return self.recover_format(id, callee, &arguments, expected, frame, 1);
            }
            (Some(primitive), _, Some(spec)) => {
                let integer =
                    numeric(primitive) && !matches!(primitive, Primitive::F32 | Primitive::F64);
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
                    return self.recover_format(id, callee, &arguments, expected, frame, 1);
                }
                let module = if primitive == Primitive::String {
                    "text"
                } else {
                    append_operation(primitive).unwrap().0
                };
                (
                    std_operation(module, "format_into_spec"),
                    (0..arguments.len()).collect(),
                )
            }
            (None, Some(function), None) => (function, vec![0, 1]),
            (None, Some(_), Some(_)) => {
                self.report(
                    const { RuleId::of("SEM-3") },
                    self.span(value),
                    &format!(
                        "`{}` formats through its @format function, which takes only fill, \
                         alignment and width; precision, sign, `#`, `0` and radix formats \
                         apply to bool, integers, floats and String",
                        self.type_spelling(ty)
                    ),
                );
                return self.recover_format(id, callee, &arguments, expected, frame, 1);
            }
            (None, None, _) => {
                let named = matches!(self.kind(ty), TypeKind::Named { .. });
                self.report(
                    const { RuleId::of("SEM-3") },
                    self.span(value),
                    &format!(
                        "an f-string hole of type `{}` has no text formatting; String, bool, \
                         char, integers and floats format, {}",
                        self.type_spelling(ty),
                        if named {
                            "and a declared type formats when it names its function with \
                             `@format(function)`"
                        } else {
                            "so convert the value to one of them first"
                        }
                    ),
                );
                return self.recover_format(id, callee, &arguments, expected, frame, 1);
            }
        };
        self.format_call(id, callee, operation, ordinals, expected, frame, 1)
    }
    /// The callee becomes a named reference to `operation`, as a resolved path
    /// would be. The first `checked` written arguments, an append's value, are
    /// already checked.
    #[allow(clippy::too_many_arguments)]
    fn format_call(
        &mut self,
        id: ast::ExprId,
        callee: ast::ExprId,
        operation: ast::DefPath,
        ordinals: Vec<usize>,
        expected: Option<TypeId>,
        frame: usize,
        checked: usize,
    ) -> Result<(), QueryError> {
        let ast::ExprKind::Call { arguments, .. } = self.input.syntax.tree.expressions
            [id.0 as usize]
            .kind
            .clone()
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
                return self.recover_format(id, callee, &arguments, expected, frame, checked);
            }
            Err(QueryError::Diagnostics(_)) => {
                return self.recover_format(id, callee, &arguments, expected, frame, checked);
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
        self.start_call(id, expected, frame, checked)
    }
    fn recover_format(
        &mut self,
        id: ast::ExprId,
        callee: ast::ExprId,
        arguments: &[ast::Argument],
        expected: Option<TypeId>,
        frame: usize,
        checked: usize,
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
        for argument in arguments[checked..].iter().rev() {
            self.work.push(Work::Enter(argument.value, Some(e), frame));
        }
        Ok(())
    }
}
