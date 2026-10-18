//! CT-2 (#65): inline evaluations have no ambient run-time frame.
use super::*;
impl Checker<'_, '_> {
    pub(super) fn check_closed_comptimes(&mut self) -> Result<(), QueryError> {
        let mut regions: Vec<_> = self
            .body
            .expressions
            .iter()
            .filter_map(
                |row| match self.input.tree.expressions[row.expression.0 as usize].kind {
                    ast::ExprKind::Comptime(child) => {
                        Some((self.span(child), self.span(row.expression)))
                    }
                    _ => None,
                },
            )
            .collect();
        if regions.is_empty() {
            return Ok(());
        }
        regions.sort_by_key(|(child, _)| (child.start, std::cmp::Reverse(child.end)));
        let span = |site| match site {
            NameSite::Expression(id) => self.input.tree.expressions[id.0 as usize].origin.span,
            NameSite::Pattern(id) => self.input.tree.patterns[id.0 as usize].origin.span,
            NameSite::Statement(id) => self.input.tree.statements[id.0 as usize].origin.span,
            NameSite::Item(id) => self.input.tree.items[id.0 as usize].origin.span,
            NameSite::Type(id) => self.input.tree.types[id.0 as usize].origin.span,
        };
        let scopes: BTreeMap<_, _> = self
            .body
            .resolution
            .reference_scopes
            .iter()
            .copied()
            .collect();
        let locals: BTreeMap<_, _> = self
            .body
            .bindings
            .iter()
            .filter_map(|row| {
                if let ResolvedName::Local(id) = row.binding {
                    Some((id, span(row.site)))
                } else {
                    None
                }
            })
            .collect();
        let parameters: BTreeMap<_, _> = self
            .body
            .bindings
            .iter()
            .filter_map(|row| {
                if let ResolvedName::Parameter(id) = row.binding {
                    Some(((row.owner, id), span(row.site)))
                } else {
                    None
                }
            })
            .collect();
        let mut references: Vec<_> = self
            .body
            .resolution
            .expressions
            .iter()
            .map(|(id, name)| (NameSite::Expression(*id), name.clone()))
            .chain(
                self.body
                    .resolution
                    .types
                    .iter()
                    .map(|(id, name)| (NameSite::Type(*id), name.clone())),
            )
            .collect();
        references.sort_by_key(|(site, _)| span(*site).start);
        let mut type_names = BTreeMap::new();
        let mut size_names = BTreeMap::new();
        for (site, name) in &references {
            match name {
                ResolvedName::TypeParameter(parameter) => {
                    type_names.entry(*parameter).or_insert_with(|| span(*site));
                }
                ResolvedName::Size(parameter) => {
                    size_names.entry(*parameter).or_insert_with(|| span(*site));
                }
                _ => (),
            }
        }
        let mut diagnosed = BTreeSet::new();
        let mut next = 0;
        let mut active: Vec<(Span, Span)> = vec![];
        for (site, name) in references {
            let mut at = span(site);
            while next < regions.len() && regions[next].0.start <= at.start {
                while active
                    .last()
                    .is_some_and(|(child, _)| child.end <= regions[next].0.start)
                {
                    active.pop();
                }
                active.push(regions[next]);
                next += 1;
            }
            while active
                .last()
                .is_some_and(|(child, _)| child.end <= at.start)
            {
                active.pop();
            }
            let Some((child, wrapper)) = active.last().copied() else {
                continue;
            };
            if at.end > child.end {
                continue;
            }
            let declaration = match name {
                ResolvedName::Local(id) => locals.get(&id).copied(),
                ResolvedName::Parameter(id) => scopes
                    .get(&site)
                    .and_then(|owner| parameters.get(&(*owner, id)))
                    .copied(),
                ResolvedName::TypeParameter(parameter) => match parameter.binder() {
                    GenericBinder::Body(owner) => Some(span(owner)),
                    _ => None,
                },
                ResolvedName::Size(_) => continue,
                _ => continue,
            };
            if declaration.is_some_and(|declaration| {
                declaration.start >= child.start && declaration.end <= child.end
            }) {
                continue;
            }
            let path = match site {
                NameSite::Expression(id) => {
                    match &self.input.tree.expressions[id.0 as usize].kind {
                        ast::ExprKind::Path(path) => Some(path),
                        _ => None,
                    }
                }
                NameSite::Type(id) => match &self.input.tree.types[id.0 as usize].kind {
                    ast::TypeExprKind::Named { path, .. } => Some(path),
                    _ => None,
                },
                _ => None,
            };
            if let Some(name) = path.and_then(|path| path.segments.first()) {
                at.end = at.start + self.name(*name).len() as u32;
            }
            let mut diagnostic = Diagnostic::error(
                const { RuleId::of("CT-2") },
                at,
                "comptime depends on a value not known in its closed evaluation frame",
            );
            diagnostic.secondary.push(hirsel_diag::Label {
                span: wrapper,
                message: "this expression is evaluated at definition time".into(),
            });
            self.diagnostics.push(diagnostic);
            diagnosed.insert((wrapper.start, wrapper.end));
        }
        let results: Vec<_> = self
            .body
            .expressions
            .iter()
            .filter_map(|row| {
                if let ast::ExprKind::Comptime(child) =
                    self.input.tree.expressions[row.expression.0 as usize].kind
                {
                    Some((row.ty, self.span(child), self.span(row.expression)))
                } else {
                    None
                }
            })
            .collect();
        for (ty, child, wrapper) in results {
            if diagnosed.contains(&(wrapper.start, wrapper.end)) {
                continue;
            }
            let unknown = |binder| {
                if let GenericBinder::Body(owner) = binder {
                    let declaration = span(owner);
                    declaration.start < child.start || declaration.end > child.end
                } else {
                    false
                }
            };
            let mut pending = vec![ty];
            let mut seen = BTreeSet::new();
            let mut parameter = None;
            while let Some(ty) = pending.pop() {
                if !seen.insert(ty) {
                    continue;
                }
                let row = self.body.types.get(ty).unwrap();
                parameter = match &row.kind {
                    TypeKind::Parameter(parameter) if unknown(parameter.binder()) => {
                        Some(ResolvedName::TypeParameter(*parameter))
                    }
                    _ => None,
                };
                if parameter.is_some() {
                    break;
                }
                pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
            }
            let Some(parameter) = parameter else {
                if !matches!(
                    self.body.types.get(ty).unwrap().kind,
                    TypeKind::Error | TypeKind::Primitive(hirsel_types::Primitive::Never)
                ) && !self.bound(ty, ast::Bound::Data)?
                {
                    self.diagnostics.push(Diagnostic::error(
                        const { RuleId::of("CT-2") },
                        wrapper,
                        "comptime result must have a data type",
                    ));
                }
                continue;
            };
            // Expected types can carry an abstract argument into an empty
            // construction without a textual reference inside the wrapper.
            let at = match parameter {
                ResolvedName::TypeParameter(parameter) => type_names.get(&parameter),
                ResolvedName::Size(parameter) => size_names.get(&parameter),
                _ => None,
            }
            .copied()
            .unwrap_or(wrapper);
            let mut diagnostic = Diagnostic::error(
                const { RuleId::of("CT-2") },
                at,
                "comptime needs a concrete result type at definition time",
            );
            diagnostic.secondary.push(hirsel_diag::Label {
                span: wrapper,
                message: "this expression has an abstract result argument".into(),
            });
            self.diagnostics.push(diagnostic);
        }
        Ok(())
    }
}
