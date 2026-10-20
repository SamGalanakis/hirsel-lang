//! Primary indices for immutable source-row identities; values remain mutable.
use super::walk::BindingKey;
use super::*;
use std::collections::BTreeMap;
pub(super) struct SourceRows {
    pub places: BTreeMap<ExprId, usize>,
    pub calls: BTreeMap<ExprId, usize>,
    pub patterns: BTreeMap<hirsel_syntax::PatternId, usize>,
    pub scopes: BTreeMap<NameSite, usize>,
    pub bindings: BTreeMap<BindingKey, usize>,
    pub unscoped: BTreeMap<(u8, u32), usize>,
    pub sites: BTreeMap<NameSite, usize>,
    pub local_sites: BTreeMap<NameSite, usize>,
    pub parameters: BTreeMap<NameSite, Vec<usize>>,
    pub other: Vec<usize>,
}
impl SourceRows {
    pub fn new(body: &CheckedBody) -> Self {
        let mut out = Self {
            places: BTreeMap::new(),
            calls: BTreeMap::new(),
            patterns: BTreeMap::new(),
            scopes: BTreeMap::new(),
            bindings: BTreeMap::new(),
            unscoped: BTreeMap::new(),
            sites: BTreeMap::new(),
            local_sites: BTreeMap::new(),
            parameters: BTreeMap::new(),
            other: vec![],
        };
        for (i, p) in body.places.iter().enumerate() {
            work::count(Counter::AuthoritySourceRows);
            out.places.entry(p.expression).or_insert(i);
        }
        for (i, c) in body.calls.iter().enumerate() {
            work::count(Counter::AuthoritySourceRows);
            out.calls.entry(c.expression).or_insert(i);
        }
        for (i, p) in body.patterns.iter().enumerate() {
            work::count(Counter::AuthoritySourceRows);
            out.patterns.entry(p.pattern).or_insert(i);
        }
        for (i, (site, _)) in body.resolution.reference_scopes.iter().enumerate() {
            work::count(Counter::AuthoritySourceRows);
            out.scopes.entry(*site).or_insert(i);
        }
        for (i, b) in body.bindings.iter().enumerate() {
            work::count(Counter::AuthoritySourceRows);
            out.sites.entry(b.site).or_insert(i);
            let key = match b.binding {
                ResolvedName::Parameter(p) => {
                    out.parameters.entry(b.owner).or_default().push(i);
                    Some((0, p.0))
                }
                ResolvedName::Local(l) => {
                    out.local_sites.entry(b.site).or_insert(i);
                    Some((1, l.0))
                }
                _ => {
                    out.other.push(i);
                    None
                }
            };
            if let Some((kind, ordinal)) = key {
                out.bindings.entry((b.owner, kind, ordinal)).or_insert(i);
                out.unscoped.entry((kind, ordinal)).or_insert(i);
            }
        }
        out
    }
    pub fn binding(
        &self,
        body: &CheckedBody,
        name: &ResolvedName,
        owner: Option<NameSite>,
    ) -> Option<usize> {
        work::count(Counter::AuthoritySourceRows);
        let key = match name {
            ResolvedName::Parameter(p) => Some((0, p.0)),
            ResolvedName::Local(l) => Some((1, l.0)),
            _ => None,
        };
        if let Some((kind, ordinal)) = key {
            if let Some(owner) = owner {
                self.bindings.get(&(owner, kind, ordinal)).copied()
            } else {
                self.unscoped.get(&(kind, ordinal)).copied()
            }
        } else {
            self.other.iter().copied().find(|i| {
                work::count(Counter::AuthoritySourceRows);
                let b = &body.bindings[*i];
                b.binding == *name && owner.is_none_or(|o| o == b.owner)
            })
        }
    }
}
