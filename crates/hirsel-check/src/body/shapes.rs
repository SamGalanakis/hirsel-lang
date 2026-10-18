//! CT-3 placement, GEN-3 eligibility and PROG-2 opacity of `shape(T)`.
use super::*;
impl Checker<'_, '_> {
    /// The shaped type with transparent aliases followed, or None once a
    /// diagnostic is reported. Reads only declared signatures.
    pub(super) fn shape_subject(
        &mut self,
        id: ast::ExprId,
        subject: TypeId,
    ) -> Result<Option<TypeId>, QueryError> {
        let site = self.span(id);
        let compile_time = matches!(
            self.input.tree.items[self.input.item.0 as usize].kind,
            ast::ItemKind::Const { .. }
        ) || self.input.tree.expressions.iter().any(|e| match e.kind {
            ast::ExprKind::Comptime(child) => {
                let region = self.span(child);
                region.start <= site.start && site.end <= region.end
            }
            _ => false,
        });
        if !compile_time {
            self.report(
                const { RuleId::of("CT-3") },
                site,
                "shape(T) is evaluated only at compile time: write it inside comptime or a constant initializer",
            );
            return Ok(None);
        }
        let mut current = subject;
        let mut seen = BTreeSet::new();
        loop {
            let TypeKind::Named { definition, .. } = self.kind(current) else {
                // An abstract T is CT-2's closed-frame judgment.
                if matches!(self.kind(current), TypeKind::Parameter(_)) {
                    return Ok(Some(current));
                }
                if self.invalid(current) {
                    return Ok(None);
                }
                break;
            };
            if !seen.insert(definition.clone()) {
                break;
            }
            let signature = match self.snapshot(&definition) {
                Ok(s) => s,
                Err(QueryError::Diagnostics(_)) => return Ok(None),
                Err(e) => return Err(e),
            };
            let SignatureKind::Type(declaration) = &signature.kind else {
                break;
            };
            if declaration.representation.is_none() {
                self.report(
                    const { RuleId::of("PROG-2") },
                    site,
                    "opaque/private representation is hidden: only its owning package can take its shape",
                );
                return Ok(None);
            }
            let alias = signature.metadata.alias;
            let distinct = declaration.distinct;
            let (_, representation) = self.representation(current)?;
            match self.kind(representation) {
                TypeKind::Record(_) | TypeKind::Variant(_) if !alias && !distinct => {
                    return Ok(Some(current));
                }
                TypeKind::Named { .. } if alias => current = representation,
                _ => break,
            }
        }
        self.report(
            const { RuleId::of("GEN-3") },
            site,
            "only nominal record and variant types have a shape",
        );
        Ok(None)
    }
}
