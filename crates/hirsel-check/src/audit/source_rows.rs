//! First-row positions for immutable producer decisions consumed by the audit.
use super::*;
pub(super) struct SourceRows {
    places: BTreeMap<ExprId, usize>,
    names: BTreeMap<ExprId, usize>,
    scopes: BTreeMap<NameSite, usize>,
    bindings: BTreeMap<(NameSite, u8, u32), usize>,
    unscoped: BTreeMap<(u8, u32), usize>,
    other: Vec<usize>,
    lets: BTreeMap<ast::PatternId, Vec<usize>>,
}
impl SourceRows {
    pub fn new(body: &CheckedBody, tree: &ast::SyntaxTree) -> Self {
        let mut out = Self {
            places: BTreeMap::new(),
            names: BTreeMap::new(),
            scopes: BTreeMap::new(),
            bindings: BTreeMap::new(),
            unscoped: BTreeMap::new(),
            other: vec![],
            lets: BTreeMap::new(),
        };
        for (i, r) in body.places.iter().enumerate() {
            work::count(Counter::AuditSourceRows);
            out.places.entry(r.expression).or_insert(i);
        }
        for (i, (e, _)) in body.resolution.expressions.iter().enumerate() {
            work::count(Counter::AuditSourceRows);
            out.names.entry(*e).or_insert(i);
        }
        for (i, (s, _)) in body.resolution.reference_scopes.iter().enumerate() {
            work::count(Counter::AuditSourceRows);
            out.scopes.entry(*s).or_insert(i);
        }
        for (i, r) in body.bindings.iter().enumerate() {
            work::count(Counter::AuditSourceRows);
            let key = match r.binding {
                ResolvedName::Parameter(p) => Some((0, p.0)),
                ResolvedName::Local(l) => Some((1, l.0)),
                _ => None,
            };
            if let Some((k, n)) = key {
                out.bindings.entry((r.owner, k, n)).or_insert(i);
                out.unscoped.entry((k, n)).or_insert(i);
            } else {
                out.other.push(i);
            }
        }
        for (i, s) in tree.statements.iter().enumerate() {
            work::count(Counter::AuditSourceRows);
            if let ast::StmtKind::Let { pattern, .. } = s.kind {
                out.lets.entry(pattern).or_default().push(i);
            }
        }
        out
    }
}
impl<'a, 'p> Audit<'a, 'p> {
    pub(super) fn source_place(&self, e: ExprId) -> Option<&'a SourcePlace> {
        work::count(Counter::AuditSourceRows);
        self.source_rows
            .places
            .get(&e)
            .map(|i| &self.input.checked.places[*i])
    }
    pub(super) fn source_name(&self, e: ExprId) -> Option<&'a (ExprId, ResolvedName)> {
        work::count(Counter::AuditSourceRows);
        self.source_rows
            .names
            .get(&e)
            .map(|i| &self.input.checked.resolution.expressions[*i])
    }
    pub(super) fn source_scope(&self, s: NameSite) -> Option<&'a (NameSite, NameSite)> {
        work::count(Counter::AuditSourceRows);
        self.source_rows
            .scopes
            .get(&s)
            .map(|i| &self.input.checked.resolution.reference_scopes[*i])
    }
    pub(super) fn source_binding(
        &self,
        name: &ResolvedName,
        owner: Option<NameSite>,
    ) -> Option<&'a BindingDecision> {
        work::count(Counter::AuditSourceRows);
        let key = match name {
            ResolvedName::Parameter(p) => Some((0, p.0)),
            ResolvedName::Local(l) => Some((1, l.0)),
            _ => None,
        };
        let i = if let Some((k, n)) = key {
            if let Some(owner) = owner {
                self.source_rows.bindings.get(&(owner, k, n)).copied()
            } else {
                self.source_rows.unscoped.get(&(k, n)).copied()
            }
        } else {
            self.source_rows.other.iter().copied().find(|i| {
                work::count(Counter::AuditSourceRows);
                let r = &self.input.checked.bindings[*i];
                r.binding == *name && owner.is_none_or(|o| r.owner == o)
            })
        }?;
        Some(&self.input.checked.bindings[i])
    }
    pub(super) fn let_statements(
        &self,
        p: ast::PatternId,
    ) -> impl Iterator<Item = &'a ast::Stmt> + '_ {
        work::count(Counter::AuditSourceRows);
        self.source_rows
            .lets
            .get(&p)
            .into_iter()
            .flatten()
            .map(|i| {
                work::count(Counter::AuditSourceRows);
                &self.input.tree.statements[*i]
            })
    }
}
