//! Explain a failed nested-mode judgment using H's inference provenance.
//! This runs only after the existing invariant-contract check has rejected.
use super::*;

impl Checker<'_> {
    pub(super) fn explain_contextual_contract(
        &mut self,
        expression: ExprId,
        ordinal: usize,
        formal: TypeId,
        actual: TypeId,
        expected: TypeId,
    ) {
        let Some(call) = self.call_row(expression) else {
            return;
        };
        if call.contextual_contracts.is_empty()
            || !self.contextual_difference(formal, actual, expected, &call.contextual_contracts)
        {
            return;
        }
        let Some(argument) = call.arguments.iter().find(|a| {
            a.parameter
                .as_ref()
                .is_some_and(|(i, _)| *i as usize == ordinal)
        }) else {
            return;
        };
        let operand = argument.expression;
        let ExprKind::Call { callee, .. } = self.tree.expressions[expression.0 as usize].kind
        else {
            return;
        };
        let Some(callee_name) = self.contract_expression(callee, 0) else {
            return;
        };
        let operand_name = match self.tree.expressions[operand.0 as usize].kind {
            ExprKind::Unary {
                op: UnaryOp::Mut | UnaryOp::Take,
                value,
            } => self.spelled(value),
            _ => self.spelled(operand),
        };
        let Some(operand_name) = operand_name else {
            return;
        };
        // `?` carries the surrounding success expectation into the call's
        // Result payload. Bind the whole `?` expression to retain that receipt.
        let boundary = self
            .tree
            .expressions
            .iter()
            .enumerate()
            .find_map(|(i, e)| {
                matches!(e.kind, ExprKind::Try { value, .. } if value == expression)
                    .then_some(ExprId(i as u32))
            })
            .unwrap_or(expression);
        let outer = self.out.body.calls.iter().find_map(|outer| {
            outer
                .arguments
                .iter()
                .find(|a| a.expression == boundary)
                .and_then(|a| a.parameter.as_ref())
                .map(|(_, p)| (outer.expression, p))
        });
        let context_type = outer.map(|(_, p)| p.ty).or_else(|| self.ty(boundary));
        let Some(context_type) = context_type else {
            return;
        };
        let spell = |ty| {
            hirsel_type_analysis::spelling::type_spelling(
                self.t,
                ty,
                &self.out.body.resolution.generic_declarations,
                &self.signature.definition.path().0.package,
                true,
            )
        };
        let [required, provided] = hirsel_type_analysis::spelling::type_spellings_apart(
            self.t,
            [expected, actual],
            &self.out.body.resolution.generic_declarations,
            &self.signature.definition.path().0.package,
            true,
        );
        let context_type = spell(context_type);
        let context = outer.and_then(|(outer, p)| {
            let ExprKind::Call { callee, .. } = self.tree.expressions[outer.0 as usize].kind else {
                return None;
            };
            let name = self.contract_expression(callee, 0)?;
            let parameter = p.name.as_deref().unwrap_or("value");
            Some((
                self.span(callee),
                format!("the `{parameter}` argument of `{name}`"),
                name,
            ))
        });
        let context_description = context.as_ref().map_or(
            "the surrounding expression".to_owned(),
            |(_, description, _)| description.clone(),
        );
        let message = format!(
            "`{callee_name}` requires `{required}` here because {context_description} expects `{context_type}`; `{operand_name}` has `{provided}`. Mutable or invariant payload contracts must match."
        );
        let bindings: BTreeSet<_> = self
            .out
            .body
            .bindings
            .iter()
            .map(|b| self.sites().binding(b).0)
            .collect();
        let local = std::iter::once("row".to_owned())
            .chain((1..).map(|i| format!("row{i}")))
            .find(|name| !bindings.contains(name))
            .unwrap();
        let repair_expression = self.contract_expression(boundary, 0);
        let repair = match repair_expression {
            Some(value) => format!(
                "help: Bind the result first (`let {local} = {value}`), then pass `take {local}`{}.",
                context
                    .as_ref()
                    .map_or(String::new(), |(_, _, name)| format!(" to `{name}`"))
            ),
            None => format!(
                "help: Bind the result of `{callee_name}` first to an inferred local, then pass that local with `take`."
            ),
        };
        let operand_span = self.span(operand);
        // Replace only the explanation just emitted for this exact failed
        // operand. Clause diagnostics and other MODE-2 failures keep theirs.
        let Some(diagnostic) = self
            .out
            .diagnostics
            .last_mut()
            .filter(|d| d.rule.as_str() == "MODE-2" && d.primary == operand_span)
        else {
            return;
        };
        diagnostic.message = message;
        diagnostic.secondary = vec![
            Label {
                span: operand_span,
                message: format!("provided: `{operand_name}` has `{provided}`"),
            },
            Label {
                span: self.tree.expressions[callee.0 as usize].origin.span,
                message: format!(
                    "required: `{callee_name}` requires `{required}` under this result context"
                ),
            },
        ];
        if let Some((span, description, _)) = context {
            diagnostic.secondary.push(Label {
                span,
                message: format!("required: {description} expects `{context_type}`"),
            });
        }
        diagnostic.notes = vec![repair];
    }

    /// A differing mode must belong to a generic contract H actually fixed
    /// from context. An unrelated fixed generic or a root count conflict is
    /// insufficient. Walk the declaration alongside both instantiated rows.
    fn contextual_difference(
        &self,
        formal: TypeId,
        actual: TypeId,
        expected: TypeId,
        fixed: &[u32],
    ) -> bool {
        let mut different = false;
        let mut pending = vec![(formal, actual, expected, false)];
        let mut seen = HashSet::default();
        while let Some((f, a, e, contract)) = pending.pop() {
            if !seen.insert((f, a, e, contract)) {
                continue;
            }
            let (Some(f), Some(a), Some(e)) = (self.t.get(f), self.t.get(a), self.t.get(e)) else {
                continue;
            };
            if let TypeKind::Parameter(p) = f.kind
                && p.binder() == GenericBinder::Bound(0)
                && contract
                && fixed.contains(&p.parameter().0)
            {
                let mut parts = vec![(a, e)];
                let mut seen = HashSet::default();
                while let Some((a, e)) = parts.pop() {
                    if a.modes.uniqueness != e.modes.uniqueness {
                        // A binding boundary can forget a unique receipt; it
                        // cannot manufacture uniqueness for counted content.
                        if e.modes.uniqueness == Uniqueness::Unique {
                            return false;
                        }
                        different = true;
                    }
                    for pair in a.kind.value_parts().into_iter().zip(e.kind.value_parts()) {
                        if seen.insert(pair)
                            && let (Some(a), Some(e)) = (self.t.get(pair.0), self.t.get(pair.1))
                        {
                            parts.push((a, e));
                        }
                    }
                }
            }
            let nested =
                contract || matches!(f.kind, TypeKind::Builtin { .. } | TypeKind::Named { .. });
            pending.extend(
                f.kind
                    .value_parts()
                    .into_iter()
                    .zip(a.kind.value_parts())
                    .zip(e.kind.value_parts())
                    .map(|((f, a), e)| (f, a, e, nested)),
            );
        }
        different
    }

    /// A suggested expression must retain package qualifications, argument
    /// labels and error mappers. Omit the concrete snippet if any part cannot
    /// be spelled faithfully; never silently drop source syntax in a repair.
    fn contract_expression(&self, expression: ExprId, depth: usize) -> Option<String> {
        if depth > 32 {
            return None;
        }
        let path = |path: &hirsel_syntax::Path| {
            let name = path
                .segments
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(".");
            path.package
                .as_ref()
                .map_or(name.clone(), |p| format!("{}@{}/{name}", p.name, p.version))
        };
        match &self.tree.expressions[expression.0 as usize].kind {
            ExprKind::Path(p) => Some(path(p)),
            ExprKind::Call { callee, arguments } => Some(format!(
                "{}({})",
                self.contract_expression(*callee, depth + 1)?,
                arguments
                    .iter()
                    .map(|a| {
                        let value = self.contract_expression(a.value, depth + 1)?;
                        Some(
                            a.label
                                .map_or(value.clone(), |label| format!("{label}: {value}")),
                        )
                    })
                    .collect::<Option<Vec<_>>>()?
                    .join(", ")
            )),
            ExprKind::Try { value, map_error } => Some(format!(
                "{}?{}",
                self.contract_expression(*value, depth + 1)?,
                map_error
                    .as_ref()
                    .map_or(String::new(), |p| format!(" {}", path(p)))
            )),
            ExprKind::Unary { op, value } => {
                let prefix = match op {
                    UnaryOp::Mut => "mut ",
                    UnaryOp::Take => "take ",
                    UnaryOp::Copy => "copy ",
                    UnaryOp::Unique => "unique ",
                    _ => return None,
                };
                Some(format!(
                    "{prefix}{}",
                    self.contract_expression(*value, depth + 1)?
                ))
            }
            ExprKind::Field { base, field } => Some(format!(
                "{}.{}",
                self.contract_expression(*base, depth + 1)?,
                field
            )),
            ExprKind::Literal(_) => self.spelled(expression),
            _ => None,
        }
    }
}
