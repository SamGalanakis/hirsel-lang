//! Arena places in a body relation (PROV-4). A relation place naming an
//! Arena-typed binding sets where blocks live; every other reach bounds the
//! value's lifetime. The ownership pass tracks lifetimes; placement stays on
//! the checked rows it refines.
use hirsel_syntax::DefPath;
use hirsel_types::{
    CheckedBody, Origin, OriginRoot, ParameterRef, Reach, Relation, ResolvedName, Signature,
    TypeId, TypeInterner, is_arena_type,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default)]
pub(crate) struct Arenas {
    roots: BTreeSet<OriginRoot>,
    /// The heap role's Arena type.
    pub(crate) definition: Option<DefPath>,
}
impl Arenas {
    pub(crate) fn new(t: &TypeInterner, body: &CheckedBody, signature: &Signature) -> Self {
        let Some(heap) = &signature.metadata.roles.heap else {
            return Self::default();
        };
        let roots = body
            .bindings
            .iter()
            .filter(|b| is_arena_type(t, b.ty, &heap.arena))
            .filter_map(|b| match b.binding {
                ResolvedName::Local(l) => Some(OriginRoot::Local(body.lexical, l)),
                ResolvedName::Parameter(p) => Some(OriginRoot::Parameter(ParameterRef::body(
                    body.lexical.site(b.owner),
                    p,
                ))),
                _ => None,
            })
            .collect();
        Self {
            roots,
            definition: Some(heap.arena.clone()),
        }
    }
    /// Whether a relation place names an Arena-typed binding of this body.
    pub(crate) fn arena(&self, origin: &Origin) -> bool {
        origin.fields.is_empty() && self.roots.contains(&origin.root)
    }
    /// A relation's lifetime sources: every reach except arena places.
    pub(crate) fn lifetime(&self, relation: &Relation) -> Relation {
        relation.retain(|r| !matches!(r, Reach::Place(o) if self.arena(o)))
    }
    /// A relation's placement: its arena places and regions only.
    pub(crate) fn placement(&self, relation: &Relation) -> Relation {
        relation.retain(|r| match r {
            Reach::Place(o) => self.arena(o),
            Reach::Region(_) => true,
            Reach::Environment => false,
        })
    }
    /// Whether a relation places its blocks in an arena or region.
    pub(crate) fn placed(&self, relation: &Relation) -> bool {
        relation.scoped(|o| self.arena(o))
    }
    /// Whether a relation's lifetime is global, whatever its placement.
    pub(crate) fn global(&self, relation: &Relation) -> bool {
        self.lifetime(relation).is_global()
    }
    /// Whether `holder`, at its root or any part, is an unplaced local join of
    /// scoped content `value` holds at the same position (PROV-4). The content
    /// is then bounded by the frame that made the join, as its placement was.
    pub(crate) fn unplaced_scoped(&self, store: &TypeInterner, holder: TypeId, value: TypeId) -> bool {
        let (Some(h), Some(v)) = (store.get(holder), store.get(value)) else {
            return false;
        };
        if !self.placed(&h.modes.relation)
            && self.lifetime(&h.modes.relation).has_environment()
            && self.placed(&v.modes.relation)
        {
            return true;
        }
        let (hs, vs) = (h.kind.value_parts(), v.kind.value_parts());
        hs.len() == vs.len()
            && hs
                .iter()
                .zip(&vs)
                .any(|(h, v)| self.unplaced_scoped(store, *h, *v))
    }
    /// `row` with the lifetime of `lifetime` and its own placement.
    pub(crate) fn relocate(&self, row: &Relation, lifetime: &Relation) -> Relation {
        self.lifetime(lifetime).union(&self.placement(row))
    }
}
