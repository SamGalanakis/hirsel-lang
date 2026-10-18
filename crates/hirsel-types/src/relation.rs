//! One relation bounds a value (MODE-1, PROV-4).
//!
//! `T in S` names the places a value may reach: parameters and their field
//! paths, bindings, arenas and a callable's environment. The empty relation is
//! global. An arena in S also places fresh blocks there; the destination is
//! derived from the relation (`destination`) and never stored on a type edge.
//! Joins are unions at every depth of a value.
use crate::{
    FieldId, GenericArgument, Origin, OriginRoot, ParameterRef, RegionId, Sendability, SharedSlice,
    Type, TypeId, TypeKind, TypeStore, Uniqueness,
};

/// One place that bounds a value (MODE-1). Field paths are meaningful (#77).
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum Reach {
    /// A parameter or local (arenas included), with its field path.
    Place(Origin),
    /// The callable's environment (`in env`) on closure and function-type
    /// results. Inside a body it is also the frame-local bound of a value
    /// whose places the ownership flow tracks itself (CLO-1: "Body checking
    /// keeps the environment-local frame result").
    Environment,
    /// A body region chosen by MEM-10; never written.
    Region(RegionId),
}

/// The places bounding a value. Empty is global (no relation, or `in heap`).
/// Canonical: sorted and deduplicated, with a place absorbing its extensions
/// (`p` absorbs `p.a`); Plain types normalise to empty.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Relation(SharedSlice<Reach>);

impl Relation {
    pub fn global() -> Self {
        Self::default()
    }
    /// Canonicalises: sorted, deduplicated, extensions absorbed by their prefix.
    pub fn new(reaches: impl IntoIterator<Item = Reach>) -> Self {
        let mut reaches: Vec<_> = reaches.into_iter().collect();
        reaches.sort();
        reaches.dedup();
        let absorbed = |r: &Reach, all: &[Reach]| {
            let Reach::Place(o) = r else { return false };
            all.iter().any(|other| {
                matches!(other, Reach::Place(p)
                    if p != o && p.root == o.root && o.fields.starts_with(&p.fields))
            })
        };
        let kept: Vec<_> = reaches
            .iter()
            .filter(|r| !absorbed(r, &reaches))
            .cloned()
            .collect();
        Self(kept.into())
    }
    pub fn reaches(&self) -> &[Reach] {
        &self.0
    }
    pub fn is_global(&self) -> bool {
        self.0.is_empty()
    }
    /// Union: branch joins, closure captures and multi-source results.
    pub fn union(&self, other: &Self) -> Self {
        if other.is_global() || self == other {
            return self.clone();
        }
        if self.is_global() {
            return other.clone();
        }
        Self::new(self.0.iter().chain(other.0.iter()).cloned())
    }
    /// Syntactic containment: every reach equals or extends a reach of `outer`.
    /// MODE-1's "within S" also follows parameter relations; hirsel-modes does that.
    pub fn contained_in(&self, outer: &Self) -> bool {
        self.0.iter().all(|r| {
            outer.0.iter().any(|o| match (r, o) {
                (Reach::Place(r), Reach::Place(o)) => {
                    r.root == o.root && r.fields.starts_with(&o.fields)
                }
                (r, o) => r == o,
            })
        })
    }
    /// `in env`: bounded by a callable's environment (or, in a body, its frame).
    pub fn environment() -> Self {
        Self(vec![Reach::Environment].into())
    }
    /// One place.
    pub fn place(origin: Origin) -> Self {
        Self(vec![Reach::Place(origin)].into())
    }
    /// Whole parameters, as a call-site source list.
    pub fn parameters(parameters: impl IntoIterator<Item = ParameterRef>) -> Self {
        Self::new(parameters.into_iter().map(|p| {
            Reach::Place(Origin {
                root: OriginRoot::Parameter(p),
                fields: Default::default(),
            })
        }))
    }
    /// The parameter roots this relation names, in order and without duplicates.
    pub fn parameter_roots(&self) -> Vec<ParameterRef> {
        let mut roots: Vec<_> = self
            .0
            .iter()
            .filter_map(|r| match r {
                Reach::Place(Origin {
                    root: OriginRoot::Parameter(p),
                    ..
                }) => Some(*p),
                _ => None,
            })
            .collect();
        roots.dedup();
        roots
    }
    /// The parameters of a relation naming only parameter places: the call-site
    /// dependent form a signature writes (`in x`, `in {x, y}`). None when the
    /// relation is global or names anything other than a parameter.
    pub fn named_parameters(&self) -> Option<Vec<ParameterRef>> {
        if self.is_global() {
            return None;
        }
        self.0
            .iter()
            .all(|r| {
                matches!(
                    r,
                    Reach::Place(Origin {
                        root: OriginRoot::Parameter(_),
                        ..
                    })
                )
            })
            .then(|| self.parameter_roots())
    }
    /// The field path within `parameter` that this relation names, when it
    /// names only field paths of it (`in p.a`): their longest common prefix,
    /// or None when it names the whole parameter or no path of it (#77).
    pub fn named_field_path(&self, parameter: &ParameterRef) -> Option<Vec<FieldId>> {
        let mut common: Option<Vec<FieldId>> = None;
        for reach in self.0.iter() {
            let Reach::Place(origin) = reach else {
                continue;
            };
            if origin.root != OriginRoot::Parameter(parameter.clone()) {
                continue;
            }
            let path: Vec<FieldId> = origin.fields.iter().copied().collect();
            common = Some(match common {
                None => path,
                Some(mut common) => {
                    let shared = common.iter().zip(&path).take_while(|(a, b)| a == b).count();
                    common.truncate(shared);
                    common
                }
            });
        }
        common.filter(|path| !path.is_empty())
    }
    /// Whether the relation names the environment (or a body's frame).
    pub fn has_environment(&self) -> bool {
        self.0.contains(&Reach::Environment)
    }
    /// Whether this is exactly the environment relation.
    pub fn is_environment(&self) -> bool {
        self.0.len() == 1 && self.0[0] == Reach::Environment
    }
    /// The places of the relation for which `arena` holds.
    pub fn arenas(&self, arena: impl Fn(&Origin) -> bool) -> Vec<&Origin> {
        self.0
            .iter()
            .filter_map(|r| match r {
                Reach::Place(o) if arena(o) => Some(o),
                _ => None,
            })
            .collect()
    }
    /// Whether the relation reaches an arena place or a body region: scoped
    /// storage that fresh blocks can be placed in (PROV-4, MEM-10).
    pub fn scoped(&self, arena: impl Fn(&Origin) -> bool) -> bool {
        self.0.iter().any(|r| match r {
            Reach::Place(o) => arena(o),
            Reach::Region(_) => true,
            Reach::Environment => false,
        })
    }
    /// The reaches for which `keep` holds.
    pub fn retain(&self, keep: impl Fn(&Reach) -> bool) -> Self {
        Self(self.0.iter().filter(|r| keep(r)).cloned().collect())
    }
    /// Rewrite every reach; the result is canonicalised again.
    pub fn map(&self, f: impl FnMut(&Reach) -> Vec<Reach>) -> Self {
        Self::new(self.0.iter().flat_map(f))
    }
}

impl FromIterator<Reach> for Relation {
    fn from_iter<I: IntoIterator<Item = Reach>>(iter: I) -> Self {
        Self::new(iter)
    }
}

/// Whether `ty` is the registry's Arena (PROV-4); `arena` is its DefPath.
pub fn is_arena_type(store: &TypeStore, ty: TypeId, arena: &hirsel_syntax::DefPath) -> bool {
    store.get(ty).is_some_and(
        |t| matches!(&t.kind, TypeKind::Named { definition, .. } if definition == arena),
    )
}

/// Whether `origin` is a whole Arena parameter of `function`, written as a
/// bound reference `depth` callable binders out from the function's own.
pub fn arena_parameter(
    store: &TypeStore,
    function: &crate::FunctionType,
    origin: &Origin,
    depth: u32,
    arena: &hirsel_syntax::DefPath,
) -> bool {
    match &origin.root {
        OriginRoot::Parameter(ParameterRef::Bound {
            depth: d,
            parameter,
        }) if *d == depth && origin.fields.is_empty() => function
            .parameters
            .get(parameter.0 as usize)
            .is_some_and(|p| is_arena_type(store, p.ty, arena)),
        _ => false,
    }
}

/// Where a construction places fresh blocks (PROV-4, MEM-10). It is derived from
/// the expected relation, never stored on a type edge.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum Destination {
    Heap,
    Arena(Origin),
    Caller(SharedSlice<ParameterRef>),
    Region(RegionId),
    Inferred,
}

/// `arena` answers whether a place is an Arena binding; at most one may be named directly.
/// A relation naming parameters only is placed by the caller; a global relation
/// leaves the destination to MEM-10.
pub fn destination(relation: &Relation, arena: impl Fn(&Origin) -> bool) -> Destination {
    if let Some(origin) = relation.arenas(&arena).first() {
        return Destination::Arena((*origin).clone());
    }
    if let Some(region) = relation.reaches().iter().find_map(|r| match r {
        Reach::Region(r) => Some(*r),
        _ => None,
    }) {
        return Destination::Region(region);
    }
    match relation.named_parameters() {
        Some(parameters) => Destination::Caller(parameters.into()),
        None => Destination::Inferred,
    }
}

impl Destination {
    /// A destination whose scope bounds the blocks placed there.
    pub fn scoped(&self) -> bool {
        matches!(self, Self::Arena(_) | Self::Region(_) | Self::Caller(_))
    }
}

/// Whether a value of this row is global at its root and every structural
/// part. A callable's header is not a part; a generic parameter is judged by
/// its argument; a nominal type's declared fields are not inspected.
/// `arena` says which places name arenas: placement is not a lifetime bound.
pub fn structurally_global(
    store: &TypeStore,
    ty: TypeId,
    arena: &impl Fn(&Origin) -> bool,
) -> bool {
    store.get(ty).is_some_and(|row| {
        row.modes
            .relation
            .reaches()
            .iter()
            .all(|r| matches!(r, Reach::Place(o) if arena(o)))
            && row
                .kind
                .value_parts()
                .into_iter()
                .all(|part| structurally_global(store, part, arena))
    })
}

/// `holder` receiving `value` as an owned whole: every environment-local row of
/// the holder that names no other place, at the root or any part, holds whatever
/// the value holds there, as a local slot does at its root. A row whose relation
/// contains the value's (a join's union of placements, PROV-4) holds it too. It
/// takes the value's relation and any stronger uniqueness and sendability, so
/// the remaining structure compares exactly.
pub fn adopt_local(store: &mut TypeStore, holder: TypeId, value: TypeId) -> TypeId {
    let (Some(h), Some(v)) = (store.get(holder), store.get(value)) else {
        return holder;
    };
    let mut h: Type = h.clone();
    let v = v.clone();
    let mut changed = false;
    if h.modes.relation.is_environment() || v.modes.relation.contained_in(&h.modes.relation) {
        let mut adopted = h.clone();
        adopted.modes.relation = v.modes.relation.clone();
        if adopted.modes.uniqueness == Uniqueness::Aliased {
            adopted.modes.uniqueness = v.modes.uniqueness;
        }
        if adopted.modes.sendability == Sendability::NotSendable {
            adopted.modes.sendability = v.modes.sendability;
        }
        changed = adopted != h;
        h = adopted;
    }
    let values = v.kind.value_parts();
    if h.kind.value_parts().len() == values.len() {
        let mut values = values.into_iter();
        let mut adopt = |part: TypeId| match values.next() {
            Some(value) => {
                let adopted = adopt_local(store, part, value);
                changed |= adopted != part;
                adopted
            }
            None => part,
        };
        match &mut h.kind {
            TypeKind::Tuple(ts) => {
                for t in ts {
                    *t = adopt(*t)
                }
            }
            TypeKind::Record(fs) => {
                for f in fs {
                    f.ty = adopt(f.ty)
                }
            }
            TypeKind::Variant(vs) => {
                for v in vs {
                    for f in &mut v.fields {
                        f.1 = adopt(f.1)
                    }
                }
            }
            TypeKind::Box(t) => *t = adopt(*t),
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                for a in arguments {
                    if let GenericArgument::Type(t) = a {
                        *t = adopt(*t)
                    }
                }
            }
            _ => (),
        }
    }
    if changed { store.push(h) } else { holder }
}

impl crate::Modes {
    /// Join current value evidence: the union of relations (PROV-4), and
    /// equal uniqueness and sendability summaries. Brands and derived
    /// structural properties remain separate.
    pub fn join(&self, other: &Self) -> Self {
        use crate::Multiplicity;
        Self {
            relation: self.relation.union(&other.relation),
            uniqueness: if self.uniqueness == other.uniqueness {
                self.uniqueness
            } else {
                Uniqueness::Aliased
            },
            multiplicity: if self.multiplicity == Multiplicity::Once
                || other.multiplicity == Multiplicity::Once
            {
                Multiplicity::Once
            } else {
                Multiplicity::Many
            },
            sendability: if self.sendability == other.sendability {
                self.sendability.clone()
            } else {
                Sendability::NotSendable
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FieldId, Modes, ParamId};

    fn param(i: u32, fields: &[u32]) -> Reach {
        Reach::Place(Origin {
            root: OriginRoot::Parameter(ParameterRef::bound(ParamId(i))),
            fields: fields.iter().map(|f| FieldId(*f)).collect(),
        })
    }

    #[test]
    fn relations_are_canonical_and_places_absorb_their_extensions() {
        let a = Relation::new([param(1, &[]), param(0, &[2]), param(0, &[]), param(1, &[])]);
        assert_eq!(a.reaches(), &[param(0, &[]), param(1, &[])]);
        let b = Relation::new([param(0, &[1]), param(0, &[1, 3]), Reach::Environment]);
        assert_eq!(b.reaches(), &[param(0, &[1]), Reach::Environment]);
        assert!(Relation::global().is_global());
        assert_eq!(Relation::new([]), Relation::global());
    }

    #[test]
    fn containment_follows_field_paths() {
        let p = Relation::new([param(0, &[])]);
        let pa = Relation::new([param(0, &[1])]);
        let pb = Relation::new([param(0, &[2])]);
        assert!(pa.contained_in(&p));
        assert!(!p.contained_in(&pa));
        assert!(!pb.contained_in(&pa));
        assert!(Relation::global().contained_in(&pa));
        assert!(!Relation::environment().contained_in(&p));
        assert_eq!(
            pa.named_parameters(),
            Some(vec![ParameterRef::bound(ParamId(0))])
        );
        assert_eq!(Relation::environment().named_parameters(), None);
    }

    #[test]
    fn destination_prefers_the_named_arena_then_the_caller() {
        let arena = |o: &Origin| o.root == OriginRoot::Parameter(ParameterRef::bound(ParamId(1)));
        let r = Relation::new([param(0, &[]), param(1, &[])]);
        assert!(matches!(destination(&r, arena), Destination::Arena(_)));
        let r = Relation::new([param(0, &[])]);
        assert!(matches!(destination(&r, arena), Destination::Caller(_)));
        assert_eq!(
            destination(&Relation::global(), arena),
            Destination::Inferred
        );
        assert_eq!(
            destination(&Relation::environment(), arena),
            Destination::Inferred
        );
    }

    #[test]
    fn an_owned_whole_adopts_parts_its_holder_contains_and_only_those() {
        use crate::{Primitive, Type, TypeKind, TypeStore};
        let row = |kind, relation| Type {
            modes: Modes {
                relation,
                ..Modes::global()
            },
            ..Type::with_kind(kind)
        };
        let pair = |store: &mut TypeStore, part: Relation| {
            let part = store.push(row(TypeKind::Primitive(Primitive::String), part));
            let tag = store.push(row(TypeKind::Primitive(Primitive::U8), Relation::global()));
            store.push(row(
                TypeKind::Tuple(vec![part, tag].into()),
                Relation::global(),
            ))
        };
        let part = |store: &TypeStore, t: TypeId| match &store.get(t).unwrap().kind {
            TypeKind::Tuple(ts) => store.get(ts[0]).unwrap().modes.relation.clone(),
            _ => unreachable!(),
        };
        let mut store = TypeStore::default();
        // A join's part `in a` holds global content (PROV-4 union join).
        let holder = pair(&mut store, Relation::new([param(0, &[])]));
        let global = pair(&mut store, Relation::global());
        let adopted = adopt_local(&mut store, holder, global);
        assert_eq!(part(&store, adopted), Relation::global());
        // Content in another place is not within the holder's part.
        let other = pair(&mut store, Relation::new([param(1, &[])]));
        assert_eq!(adopt_local(&mut store, holder, other), holder);
        assert_eq!(part(&store, holder), Relation::new([param(0, &[])]));
    }
}
