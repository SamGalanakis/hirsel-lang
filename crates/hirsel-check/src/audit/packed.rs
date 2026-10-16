use super::*;

impl Audit<'_, '_> {
    pub(super) fn variant_places(&mut self) -> Result<(), QueryError> {
        for row in &self.input.checked.expressions {
            if self.ordinary_payload(row.expression)? {
                let span = self
                    .input
                    .tree
                    .statements
                    .iter()
                    .find_map(|s| match s.kind {
                        ast::StmtKind::Assign { place, .. } if place == row.expression => {
                            Some(s.origin.span)
                        }
                        _ => None,
                    })
                    .unwrap_or(
                        self.input.tree.expressions[row.expression.0 as usize]
                            .origin
                            .span,
                    );
                self.report(
                    "MEM-13",
                    span,
                    "ordinary variant values expose no payload places",
                );
            }
        }
        for statement in &self.input.tree.statements {
            let ast::StmtKind::Assign { place, .. } = statement.kind else {
                continue;
            };
            let Some(ty) = self.expression_type(place) else {
                continue;
            };
            if matches!(
                self.types.get(ty).unwrap().kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Packed,
                    ..
                }
            ) && !self.owns_container(place)
            {
                self.report(
                    "MEM-13",
                    statement.origin.span,
                    "a Packed variant cannot be replaced through a payload or element alias",
                );
            }
        }
        Ok(())
    }
    fn owns_container(&self, expression: ExprId) -> bool {
        let place = self.source_place(expression).expect("H source place");
        if !place.projections.is_empty() || place.access != SourceAccess::Read {
            return false;
        }
        let SourceRoot::Binding { owner, binding } = &place.root else {
            return false;
        };
        self.source_binding(binding, Some(*owner))
            .is_some_and(|b| !matches!(b.usage, UseKind::Borrow | UseKind::Mut))
    }
    fn ordinary_payload(&mut self, expression: ExprId) -> Result<bool, QueryError> {
        let place = self
            .source_place(expression)
            .cloned()
            .expect("H source place");
        // An access wrapper retains its operand's path; diagnose the node
        // actually applying the payload step, as before.
        if !place.projections.iter().any(|p| p.expression == expression) {
            return Ok(false);
        }
        let mut ty = self.place_root_type(&place);
        for projection in place.projections {
            let Some(current) = ty else { return Ok(false) };
            if matches!(projection.kind, SourceProjectionKind::VariantField { .. })
                && !matches!(
                    self.types.get(current).unwrap().kind,
                    TypeKind::Builtin {
                        kind: BuiltinType::Packed,
                        ..
                    }
                )
            {
                return Ok(true);
            }
            ty = self.projected_type(current, &projection.kind)?;
        }
        Ok(false)
    }
}
