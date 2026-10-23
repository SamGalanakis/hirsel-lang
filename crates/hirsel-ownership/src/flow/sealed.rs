//! Source MEM-4 checks supplement ordinary affine flow with storage closure.
use super::*;

impl Checker<'_, '_> {
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
        if heap || allowed.is_none() || foreign(&root_value.modes.relation) || foreign_content {
            self.diagnostic(const { RuleId::of("MEM-4") }, self.span_of(expression), "seal requires a live explicit arena grant and an owned root closed over that arena and the heap");
            return false;
        }
        let retains = |value: &Value| {
            value.modes.relation.reaches().iter().any(|reach| matches!(reach, Reach::Place(origin) if Some(&origin.root) == allowed.as_ref()))
            || grant_binding.is_some_and(|grant| value.content.summary().iter().any(|loan| loan.on == On::Binding(grant)))
        };
        let alias = self.live.at(self.node).iter().find_map(|holder| {
            let Holder::Binding(binding) = holder else {
                return None;
            };
            (Some(*binding) != root_binding
                && Some(*binding) != grant_binding
                && state.slots[*binding].available & LIVE != 0
                && !self.properties[self.body.bindings[*binding].ty].plain
                && retains(&state.slots[*binding].value))
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
