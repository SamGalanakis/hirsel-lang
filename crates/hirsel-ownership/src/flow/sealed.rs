//! Source MEM-4 checks supplement ordinary affine flow with storage closure.
use super::*;

impl Checker<'_, '_> {
    pub(super) fn sealed_owner(&self, expression: ExprId) -> bool {
        self.ty(expression).is_some_and(|ty| {
            matches!(
                self.t.row(ty).kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Region,
                    ..
                }
            )
        })
    }

    /// Owned components retain their actual storage bounds even when an inline
    /// tuple's own row is global. Callable headers are not owned components;
    /// captures are checked through the flow value's content below.
    fn foreign_seal_payload(&self, root: TypeId, allowed: Option<&OriginRoot>) -> bool {
        let mut work = vec![(root, false)];
        let mut seen = std::collections::HashSet::<TypeId, hirsel_intern::IdHasher>::default();
        let mut expansions = hirsel_types::NominalExpansions::default();
        while let Some((ty, exit)) = work.pop() {
            if exit {
                expansions.leave(ty, 0);
                continue;
            }
            if !seen.insert(ty) {
                continue;
            }
            let row = self.t.row(ty);
            if matches!(
                row.kind,
                TypeKind::Builtin {
                    kind: BuiltinType::Shared,
                    ..
                }
            ) {
                continue;
            }
            let plain = self.signatures.prove_flow(|memo| {
                hirsel_modes::proofs::property(memo, ty, StructuralProperty::Plain)
            });
            if plain == Ok(true) {
                continue;
            }
            if let Err(error) = plain {
                *self.proof_error.borrow_mut() = Some(error);
                return true;
            }
            if row
                .modes
                .relation
                .reaches()
                .iter()
                .any(|reach| match reach {
                    Reach::Region(_) => true,
                    Reach::Place(origin) if self.arenas.arena(origin) => {
                        Some(&origin.root) != allowed
                    }
                    _ => false,
                })
            {
                return true;
            }
            match &row.kind {
                TypeKind::Function(_) => (),
                TypeKind::Named { .. } => {
                    match expansions.enter(ty, 0, self.t) {
                        Ok(true) => (),
                        Ok(false) => continue,
                        Err(error) => {
                            *self.proof_error.borrow_mut() = Some(error);
                            return true;
                        }
                    }
                    let Ok(expanded) = hirsel_type_analysis::representation::expand_representation(
                        self.t,
                        self.signatures.type_memo(),
                        ty,
                        &mut |_, definition, arguments| {
                            Ok(self.signatures.cached(definition).map(|signature| {
                                (signature, vec![AuthoritySummary::Unknown; arguments.len()])
                            }))
                        },
                        Some(1),
                    ) else {
                        return true;
                    };
                    work.push((ty, true));
                    work.push((expanded, false));
                }
                kind => work.extend(kind.value_parts().into_iter().map(|part| (part, false))),
            }
        }
        false
    }
    pub(super) fn seal_entry(
        &mut self,
        state: &State,
        expression: ExprId,
        arguments: &[hirsel_syntax::Argument],
        requirements: &[Option<super::super::cfg::Requirement>],
    ) -> bool {
        for (argument, requirement) in arguments.iter().zip(requirements) {
            if requirement.as_ref().is_some_and(|requirement| {
                requirement.passing == PassingMode::Take
                    && self.arenas.definition.as_ref().is_some_and(|arena| {
                        hirsel_types::is_arena_type(self.t, requirement.ty, arena)
                    })
            }) && self
                .ty(argument.value)
                .and_then(|ty| self.t.get(ty))
                .is_some_and(|row| row.allocation_target.is_some())
            {
                self.diagnostic(
                    const { RuleId::of("MEM-4") },
                    self.span_of(expression),
                    "heap is not a consumable arena scope grant",
                );
                return false;
            }
        }
        let seal = self.body.calls.iter().any(|call| call.expression == expression && matches!(&call.target, CallTarget::Definition(path) if path.is_core("arena.seal")));
        if !seal {
            return true;
        }
        let argument = |slot| {
            arguments
                .iter()
                .zip(requirements)
                .find(|(_, requirement)| requirement.as_ref().is_some_and(|r| r.parameter == slot))
                .map(|(a, _)| a.value)
        };
        let (Some(grant), Some(root)) = (argument(0), argument(1)) else {
            return false;
        };
        let unwrap = |mut expression: ExprId| {
            while let ExprKind::Unary {
                op: UnaryOp::Take,
                value,
            } = self.tree.expressions[expression.0 as usize].kind
            {
                expression = value;
            }
            expression
        };
        let grant_place = place(self.body, unwrap(grant), self.source_rows);
        let root_place = place(self.body, unwrap(root), self.source_rows);
        let allowed = grant_place.as_ref().map(|p| {
            let binding = &self.body.bindings[p.binding];
            match binding.binding {
                ResolvedName::Parameter(parameter) => OriginRoot::Parameter(ParameterRef::body(
                    self.body.lexical.site(binding.owner),
                    parameter,
                )),
                ResolvedName::Local(local) => OriginRoot::Local(self.body.lexical, local),
                _ => unreachable!("source place binding"),
            }
        });
        let root_value = self.value(state, root);
        let root_binding = root_place.as_ref().map(|p| p.binding);
        let grant_binding = grant_place.as_ref().map(|p| p.binding);
        let heap = self
            .ty(grant)
            .and_then(|ty| self.t.get(ty))
            .is_some_and(|row| row.allocation_target.is_some());
        let foreign = |relation: &Relation| {
            relation.reaches().iter().any(|reach| match reach {
                Reach::Region(_) => true,
                Reach::Place(origin) if self.arenas.arena(origin) => {
                    Some(&origin.root) != allowed.as_ref()
                }
                _ => false,
            })
        };
        let foreign_content = root_value
            .content
            .summary()
            .iter()
            .any(|loan| match loan.on {
                On::Binding(binding) => {
                    Some(binding) != root_binding && Some(binding) != grant_binding
                }
                On::Scope(_) | On::Environment | On::Placement => true,
            });
        let foreign_parts = self
            .ty(root)
            .is_none_or(|ty| self.foreign_seal_payload(ty, allowed.as_ref()));
        if heap
            || allowed.is_none()
            || foreign(&root_value.modes.relation)
            || foreign_content
            || foreign_parts
        {
            self.diagnostic(const { RuleId::of("MEM-4") }, self.span_of(expression), "seal requires a live explicit arena grant and an owned root closed over that arena and the heap");
            return false;
        }
        let retains = |value: &Value| {
            // A live scoped environment can borrow an alias rather than the
            // grant directly. Follow those binding dependencies to their
            // current content and declared bounds; captures do not erase an
            // arena view's lifetime. The visited set also makes joins finite.
            let names_grant = |relation: &Relation| {
                relation.reaches().iter().any(
                |reach| matches!(reach, Reach::Place(origin) if Some(&origin.root) == allowed.as_ref())
            )
            };
            let mut pending = vec![value];
            let mut seen = BTreeSet::new();
            while let Some(value) = pending.pop() {
                if names_grant(&value.modes.relation) {
                    return true;
                }
                let content = value.content.summary();
                for loan in value.loans.iter().chain(content.iter()) {
                    let On::Binding(binding) = loan.on else {
                        continue;
                    };
                    if Some(binding) == grant_binding {
                        return true;
                    }
                    if seen.insert(binding) {
                        if names_grant(&self.t.row(self.body.bindings[binding].ty).modes.relation) {
                            return true;
                        }
                        pending.push(&state.slots[binding].value);
                    }
                }
            }
            false
        };
        let alias = self.live.at(self.node).iter().find_map(|holder| {
            let Holder::Binding(binding) = holder else {
                return None;
            };
            (Some(*binding) != root_binding
                && Some(*binding) != grant_binding
                && state.slots[*binding].available & LIVE != 0
                && !self.properties[self.body.bindings[*binding].ty].plain
                && (retains(&state.slots[*binding].value)
                    || self.t.row(self.body.bindings[*binding].ty).modes.relation.reaches().iter().any(|reach| matches!(reach, Reach::Place(origin) if Some(&origin.root) == allowed.as_ref()))))
            .then_some(*binding)
        });
        if alias.is_some() {
            self.diagnostic(
                const { RuleId::of("MEM-4") },
                self.span_of(expression),
                "a live second view still depends on the arena being sealed",
            );
            return false;
        }
        true
    }
}
