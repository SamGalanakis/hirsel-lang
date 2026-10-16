//! First-row indices into source facts; mutable fact values stay in their arenas.
use super::*;
#[derive(Default)]
pub(super) struct SourceRows {
    pub names: BTreeMap<ast::ExprId, usize>,
    pub scopes: BTreeMap<NameSite, usize>,
    pub functions: BTreeMap<ast::StmtId, usize>,
    pub elaborations: BTreeMap<ast::ExprId, usize>,
}
impl SourceRows {
    pub fn new(resolution: &Resolution) -> Self {
        let mut rows = Self::default();
        for (i, (expression, _)) in resolution.expressions.iter().enumerate() {
            work::count(Counter::BodySourceRows);
            rows.names.entry(*expression).or_insert(i);
        }
        for (i, (site, _)) in resolution.reference_scopes.iter().enumerate() {
            work::count(Counter::BodySourceRows);
            rows.scopes.entry(*site).or_insert(i);
        }
        for (i, (statement, _)) in resolution.local_functions.iter().enumerate() {
            work::count(Counter::BodySourceRows);
            rows.functions.entry(*statement).or_insert(i);
        }
        rows
    }
}
impl<'a, 'p> Checker<'a, 'p> {
    pub(super) fn input_name(&self, id: ast::ExprId) -> Option<&'a (ast::ExprId, ResolvedName)> {
        work::count(Counter::BodySourceRows);
        self.source_rows
            .names
            .get(&id)
            .map(|i| &self.input.resolution.expressions[*i])
    }
    pub(super) fn body_name(&self, id: ast::ExprId) -> Option<&(ast::ExprId, ResolvedName)> {
        work::count(Counter::BodySourceRows);
        self.source_rows
            .names
            .get(&id)
            .map(|i| &self.body.resolution.expressions[*i])
    }
    pub(super) fn input_scope(&self, site: NameSite) -> Option<&'a (NameSite, NameSite)> {
        work::count(Counter::BodySourceRows);
        self.source_rows
            .scopes
            .get(&site)
            .map(|i| &self.input.resolution.reference_scopes[*i])
    }
    pub(super) fn body_scope(&self, site: NameSite) -> Option<&(NameSite, NameSite)> {
        work::count(Counter::BodySourceRows);
        self.source_rows
            .scopes
            .get(&site)
            .map(|i| &self.body.resolution.reference_scopes[*i])
    }
    pub(super) fn local_function(&self, id: ast::StmtId) -> Option<&'a (ast::StmtId, LocalId)> {
        work::count(Counter::BodySourceRows);
        self.source_rows
            .functions
            .get(&id)
            .map(|i| &self.input.resolution.local_functions[*i])
    }
    pub(super) fn elaboration(&mut self, id: ast::ExprId) -> Option<&mut Elaboration> {
        work::count(Counter::BodySourceRows);
        let i = *self.source_rows.elaborations.get(&id)?;
        self.body.expressions.get_mut(i)
    }
}
