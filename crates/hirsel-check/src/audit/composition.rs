//! Layout composition (MEM-7, MEM-12, MEM-13): the attribute combinations one
//! declaration may carry, and the fixed-stride column contract of `@soa`.
use super::*;

/// Why a part held inline by an `@soa` element cannot be a fixed-stride column.
enum ColumnIssue {
    Packed,
    VariableFlat,
    Resource,
}

impl Audit<'_, '_> {
    pub(super) fn layout_composition(&mut self) -> Result<(), QueryError> {
        let item = &self.input.syntax.item;
        let SignatureKind::Type(definition) = &self.input.signature.kind else {
            return Ok(());
        };
        let p = definition.layout.clone();
        let span = |names: &[&str]| {
            item.attributes
                .iter()
                .find(|a| names.contains(&a.name.as_str()))
                .map_or(item.origin.span, |a| a.origin.span)
        };
        let attributed = p.c || p.packed || p.align.is_some() || p.soa;
        if p.flat && attributed {
            self.report(
                const { RuleId::of("MEM-12") },
                span(&["layout", "packed", "align", "soa"]),
                "a flat type takes no layout attribute",
            );
            return Ok(());
        }
        if p.soa && (p.c || p.packed || p.align.is_some()) {
            self.report(
                const { RuleId::of("MEM-7") },
                span(&["layout", "packed", "align"]),
                "@soa excludes @layout(c), @packed and @align on one declaration",
            );
            return Ok(());
        }
        if p.packed && p.align.is_some() {
            self.report(
                const { RuleId::of("MEM-7") },
                span(&["align"]),
                "@packed excludes @align on one declaration",
            );
            return Ok(());
        }
        if p.flat {
            if let Some(representation) = definition.representation
                && self.holds_attributed(representation, &mut std::collections::HashSet::new())?
            {
                self.report(
                    const { RuleId::of("MEM-12") },
                    item.origin.span,
                    "a flat type cannot hold a type declared with a layout attribute",
                );
            }
            return Ok(());
        }
        if !p.soa {
            return Ok(());
        }
        let at = span(&["soa"]);
        if definition.resource {
            self.report(
                const { RuleId::of("MEM-7") },
                at,
                "an @soa element cannot be a resource",
            );
            return Ok(());
        }
        let Some(representation) = definition.representation else {
            return Ok(());
        };
        let parts: Vec<TypeId> = match self.types.get(representation).unwrap().kind.clone() {
            TypeKind::Record(fields) => fields.iter().map(|f| f.ty).collect(),
            TypeKind::Variant(_) if definition.serializable => {
                self.report(
                    const { RuleId::of("MEM-7") },
                    at,
                    "an @soa variant is not serializable",
                );
                return Ok(());
            }
            TypeKind::Variant(variants) => variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|(_, t)| *t))
                .collect(),
            _ => {
                self.report(
                    const { RuleId::of("MEM-7") },
                    at,
                    "@soa applies to a record or variant declaration",
                );
                return Ok(());
            }
        };
        for part in parts {
            let issue = self.column_issue(part, &mut BTreeSet::new())?;
            let (rule, message) = match issue {
                None => continue,
                Some(ColumnIssue::Packed) => (
                    const { RuleId::of("MEM-13") },
                    "an @soa column cannot hold a Packed value, whose extent varies",
                ),
                Some(ColumnIssue::VariableFlat) => (
                    const { RuleId::of("MEM-7") },
                    "an @soa column cannot hold a variable-size flat value",
                ),
                Some(ColumnIssue::Resource) => (
                    const { RuleId::of("MEM-7") },
                    "an @soa column cannot hold a resource",
                ),
            };
            self.report(rule, at, message);
            return Ok(());
        }
        Ok(())
    }

    /// The first reason the inline storage of `id` has no fixed stride or
    /// holds a resource. Blocks (`box`, `List`, `String`, ...) are fixed-size
    /// carriers that keep their owners and are not entered.
    fn column_issue(
        &mut self,
        id: TypeId,
        active: &mut BTreeSet<ast::DefPath>,
    ) -> Result<Option<ColumnIssue>, QueryError> {
        let children: Vec<TypeId> = match self.types.get(id).unwrap().kind.clone() {
            TypeKind::Builtin {
                kind: BuiltinType::Packed,
                ..
            } => return Ok(Some(ColumnIssue::Packed)),
            TypeKind::Builtin {
                kind: BuiltinType::Inline | BuiltinType::Simd,
                arguments,
            } => arguments
                .iter()
                .filter_map(|a| match a {
                    GenericArgument::Type(t) => Some(*t),
                    _ => None,
                })
                .collect(),
            TypeKind::Tuple(items) => items.to_vec(),
            TypeKind::Record(fields) => fields.iter().map(|f| f.ty).collect(),
            TypeKind::Variant(variants) => variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|(_, t)| *t))
                .collect(),
            TypeKind::Named { definition, .. } => {
                if !active.insert(definition.path().clone()) {
                    return Ok(None);
                }
                let s = self.snapshot(&definition)?;
                let SignatureKind::Type(d) = &s.kind else {
                    return Ok(None);
                };
                if d.resource {
                    return Ok(Some(ColumnIssue::Resource));
                }
                if d.layout.flat {
                    let fixed = self.fixed_size(id, &mut BTreeSet::new())?;
                    return Ok((!fixed).then_some(ColumnIssue::VariableFlat));
                }
                let (repr, _) = self.representation(id)?;
                let issue = if repr == id {
                    None
                } else {
                    self.column_issue(repr, active)?
                };
                active.remove(definition.path());
                return Ok(issue);
            }
            _ => vec![],
        };
        for child in children {
            if let Some(issue) = self.column_issue(child, active)? {
                return Ok(Some(issue));
            }
        }
        Ok(None)
    }
}

impl Audit<'_, '_> {
    /// MEM-7: `Inline[T, n]` of an `@soa` `T`, in a declaration's fields or a
    /// body's expression types, reported once per item.
    pub(super) fn inline_soa(&mut self) -> Result<(), QueryError> {
        let item = &self.input.syntax.item;
        let mut roots: Vec<(hirsel_span::Span, TypeId)> = Vec::new();
        if let SignatureKind::Type(definition) = &self.input.signature.kind
            && let Some(representation) = definition.representation
        {
            roots.push((item.origin.span, representation));
        }
        for row in &self.input.checked.expressions {
            let span = self.input.syntax.tree.expressions[row.expression.0 as usize]
                .origin
                .span;
            roots.push((span, row.ty));
        }
        let mut seen = std::collections::HashSet::new();
        for (span, root) in roots {
            if self.holds_inline_soa(root, &mut seen)? {
                self.report(
                    const { RuleId::of("MEM-7") },
                    span,
                    "Inline[T, n] cannot hold an @soa element",
                );
                return Ok(());
            }
        }
        Ok(())
    }

    fn holds_inline_soa(
        &mut self,
        id: TypeId,
        seen: &mut std::collections::HashSet<TypeId>,
    ) -> Result<bool, QueryError> {
        if !seen.insert(id) {
            return Ok(false);
        }
        let kind = self.types.get(id).unwrap().kind.clone();
        if let TypeKind::Builtin {
            kind: BuiltinType::Inline,
            arguments,
        } = &kind
            && let Some(GenericArgument::Type(element)) = arguments.first()
            && let TypeKind::Named { definition, .. } =
                self.types.get(*element).unwrap().kind.clone()
            && let SignatureKind::Type(d) = &self.snapshot(&definition)?.kind
            && d.layout.soa
        {
            return Ok(true);
        }
        for part in kind.value_parts() {
            if self.holds_inline_soa(part, seen)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Audit<'_, '_> {
    /// Whether flat storage of `id` reaches a nominal declared with
    /// `@layout(c)`, `@packed`, `@align` or `@soa`, inline or as a sequence
    /// element: flat bytes have no columns and no target layout (MEM-12).
    fn holds_attributed(
        &mut self,
        id: TypeId,
        seen: &mut std::collections::HashSet<TypeId>,
    ) -> Result<bool, QueryError> {
        if !seen.insert(id) {
            return Ok(false);
        }
        let kind = self.types.get(id).unwrap().kind.clone();
        if let TypeKind::Named { definition, .. } = &kind
            && let SignatureKind::Type(d) = &self.snapshot(definition)?.kind
        {
            let p = &d.layout;
            if p.c || p.packed || p.align.is_some() || p.soa {
                return Ok(true);
            }
        }
        for part in kind.value_parts() {
            if self.holds_attributed(part, seen)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
