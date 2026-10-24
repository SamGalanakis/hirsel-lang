//! OWN-1 context only: recover the inspected match from checked pattern rows.
//! The pattern/source range structure follows rust-analyzer's
//! ide-diagnostics/src/handlers/mutable_ref.rs:6-17,24-62. Hirsel's LANG-13
//! determines borrowing; no Rust binding or edition rule is imported.
use super::*;
use hirsel_modes::explain::{Caution, Repair, spell};

impl Checker<'_, '_> {
    pub(super) fn borrowed_match_explanation(
        &self,
        state: &State,
        place: &Place,
        value: &Value,
        required: Fact<Requirement>,
    ) -> causes::Pending {
        let mut pending = self.locality(state, value, required);
        let mut binding = place.binding;
        let mut seen = BTreeSet::new();
        let mut matches = 0;
        while seen.insert(binding) && state.slots[binding].value.aliased().is_some() {
            let NameSite::Pattern(pattern) = self.body.bindings[binding].site else {
                break;
            };
            let Some(source) = self.cursor_pattern_source(binding) else {
                break;
            };
            let Some((matching, node)) = self.tree.expressions.iter().enumerate().find(
                |(_, e)| matches!(&e.kind, ExprKind::Match { value, .. } if *value == source),
            ) else {
                break;
            };
            let matching = ExprId(matching as u32);
            // Let conditions keep their existing guidance, especially while-let's
            // repeated evaluation of the same owner.
            if self
                .tree
                .conditions
                .iter()
                .any(|c| c.matching() == matching)
            {
                break;
            }
            let Some(owner) = place_for_match(self, source) else {
                break;
            };
            let (name, _) = self.binding_site(binding);
            let source_name = spell(self.tree, source).unwrap_or_else(|| "the scrutinee".into());
            let source_span = self.tree.expressions[source.0 as usize].origin.span;
            let mut match_span = node.origin.span;
            match_span.end = source_span.end;
            // Prepend context: leave the existing root cause, root identity and
            // requirement intact, so diagnostic folding and acceptance agree.
            pending.explanation.provided.splice(
                0..0,
                [
                    Fact::new(
                        Cause::BorrowedPattern,
                        self.tree.patterns[pattern.0 as usize].origin.span,
                        &[&name],
                    ),
                    Fact::new(Cause::InspectedMatch, match_span, &[&source_name]),
                ],
            );
            matches += 1;
            if !owner.projections.is_empty() {
                break;
            }
            if matches == 1 && self.consuming_match_owner(state, &owner, source, node.origin.span) {
                let (name, _) = self.binding_site(owner.binding);
                pending
                    .explanation
                    .repairs
                    .push(Repair::ConsumingMatch(name));
                break;
            }
            binding = owner.binding;
        }
        if matches > 1 {
            pending.explanation.cautions.push(Caution::NestedMatch);
        }
        pending
    }

    fn consuming_match_owner(
        &self,
        state: &State,
        owner: &Place,
        source: ExprId,
        matching: Span,
    ) -> bool {
        let binding = &self.body.bindings[owner.binding];
        let slot = &state.slots[owner.binding];
        let property = self.properties[binding.ty];
        if binding.owner != self.graph.owner
            || slot.available != LIVE
            || slot.value.invalid
            || slot.value.aliased().is_some()
            || slot.value.element_place.is_some()
            || !property.valid
            || !property.data
            || self.finalizer_self(owner.binding)
            || self.loaned(state, owner)
            || state
                .argument_loans
                .values()
                .any(|(loan, _)| loan.overlaps(owner))
        {
            return false;
        }
        // The error is later than inspection: a guard may have made a loan's
        // last use in between. Without an entry-state snapshot, require that
        // no other loan-capable holder was live at the source read. This is
        // deliberately conservative, and uses existing backward demand only.
        let Some((entry, _)) = self
            .graph
            .nodes
            .iter()
            .enumerate()
            .find(|(_, n)| matches!(n.op, Op::Use { expression, .. } if expression == source))
        else {
            return false;
        };
        if self.live.at(entry).iter().any(|holder| match *holder {
            Holder::Binding(index) => index != owner.binding,
            Holder::Temporary(expression) => {
                expression != source && self.property(expression).is_none_or(|p| !p.plain)
            }
            Holder::Iteration(_) => true,
        }) {
            return false;
        }
        if let ResolvedName::Parameter(parameter) = binding.binding
            && self.parameter_passing(&binding.owner, parameter) != PassingMode::Take
        {
            return false;
        }
        // A match inside a loop may inspect the same owner repeatedly. Even a
        // currently available owned slot is not a consuming-loop recipe.
        if self.tree.statements.iter().any(|s| {
            matches!(
                s.kind,
                hirsel_syntax::StmtKind::While { .. } | hirsel_syntax::StmtKind::For { .. }
            ) && contains(s.origin.span, matching)
        }) {
            return false;
        }
        // A concrete enum declaration is required. Resources (including enum
        // payloads), unknown/generic representations and recovery rows have no
        // automatic whole-owner recipe here.
        let Some(TypeKind::Named { definition, .. }) = self.t.get(binding.ty).map(|t| &t.kind)
        else {
            return false;
        };
        let Some(signature) = self.signatures.cached(*definition) else {
            return false;
        };
        matches!(&signature.kind, SignatureKind::Type(declaration)
            if !declaration.resource && declaration.drop.is_none()
                && declaration.representation.and_then(|ty| self.t.get(ty))
                    .is_some_and(|ty| matches!(ty.kind, TypeKind::Variant(_))))
    }
}

fn place_for_match(checker: &Checker<'_, '_>, source: ExprId) -> Option<Place> {
    // A computed or explicitly consumed source did not establish this borrow.
    matches!(
        checker.tree.expressions[source.0 as usize].kind,
        ExprKind::Path(_) | ExprKind::Field { .. } | ExprKind::Index { .. }
    )
    .then(|| place(checker.body, source, checker.source_rows))
    .flatten()
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.anchor == inner.anchor && outer.start <= inner.start && inner.end <= outer.end
}
