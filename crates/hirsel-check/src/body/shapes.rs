//! CT-3 placement, GEN-3 eligibility and PROG-2 opacity of `shape(T)`, and the
//! compile-time-only values (TypeShape, Body) run-time code never holds.
use super::*;
impl Checker<'_, '_> {
    /// CT-3: outside compile-time code and compile-time-only definitions (whose
    /// signature mentions TypeShape or Body), no expression has a type that
    /// mentions one, so no run-time code calls a compile-time-only definition.
    pub(super) fn check_compile_time_values(&mut self) {
        let types = &self.body.types;
        let compile_time_only = match &self.input.signature.kind {
            SignatureKind::Const { .. } => true,
            SignatureKind::Function(f) => {
                let types = &self.input.signature.types;
                f.parameters
                    .iter()
                    .map(|p| p.ty)
                    .chain([f.result])
                    .any(|t| mentions_compile_time_only(types, t))
            }
            SignatureKind::Type(_) => false,
        };
        if compile_time_only {
            return;
        }
        // A `comptime` result crossing into run-time code is CT-2's data judgment.
        let regions: Vec<Span> = self
            .input
            .syntax
            .tree
            .expressions
            .iter()
            .filter(|e| matches!(e.kind, ast::ExprKind::Comptime(_)))
            .map(|e| e.origin.span)
            .collect();
        let mut sites: Vec<Span> = self
            .body
            .expressions
            .iter()
            .filter(|row| mentions_compile_time_only(types, row.ty))
            .map(|row| self.span(row.expression))
            .filter(|site| {
                !regions
                    .iter()
                    .any(|r| r.anchor == site.anchor && r.start <= site.start && site.end <= r.end)
            })
            .collect();
        sites.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end)));
        let mut outer: Option<Span> = None;
        for site in sites {
            if outer.is_some_and(|o| o.anchor == site.anchor && site.end <= o.end) {
                continue;
            }
            outer = Some(site);
            self.report(
                const { RuleId::of("CT-3") },
                site,
                "a TypeShape or Body value exists only at compile time: use it inside comptime, a constant initializer or a definition whose signature mentions TypeShape or Body",
            );
        }
    }
    /// The shaped type with transparent aliases followed, or None once a
    /// diagnostic is reported. Reads only declared signatures.
    pub(super) fn shape_subject(
        &mut self,
        id: ast::ExprId,
        subject: TypeId,
    ) -> Result<Option<TypeId>, QueryError> {
        let site = self.span(id);
        let compile_time = matches!(self.input.syntax.item.kind, ast::ItemKind::Const { .. })
            || self
                .input
                .syntax
                .tree
                .expressions
                .iter()
                .any(|e| match e.kind {
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
