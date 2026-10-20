//! Lexical value-parameter transport, independent of syntax traversal and storage.
use crate::*;
use std::collections::BTreeMap;

/// Rebuild a rooted graph while visiting parameter references with their binder
/// depth. Root value qualifiers are outside a FunctionType's binder; its parameter,
/// result, error, authority and uses rows are inside. Shared DAG edges at different
/// depths are distinct visits. Inserted references are never visited again.
pub fn map_parameter_references(
    store: &TypeInterner,
    root: TypeId,
    mut map: impl FnMut(ParameterRef, u32) -> Result<ParameterRef, GraphError>,
) -> Result<TypeId, GraphError> {
    fn summary(
        a: &mut AuthoritySummary,
        depth: u32,
        map: &mut impl FnMut(ParameterRef, u32) -> Result<ParameterRef, GraphError>,
    ) -> Result<(), GraphError> {
        if let AuthoritySummary::Known(terms) = a {
            for term in terms.iter_mut() {
                if let AuthorityTerm::Parameter(p) = term {
                    *p = map(*p, depth)?;
                }
            }
            terms.sort();
            terms.dedup();
        }
        Ok(())
    }
    rewrite_parameter_graph(store, root, |_, t, depth| {
        for o in &mut t.brand.alternatives {
            if let OriginRoot::Parameter(p) | OriginRoot::ArgumentBrand(p) = &mut o.root {
                *p = map(*p, depth)?;
            }
        }
        t.brand.alternatives.sort();
        t.brand.alternatives.dedup();
        if t.modes.relation.reaches().iter().any(|r| {
            matches!(
                r,
                Reach::Place(Origin {
                    root: OriginRoot::Parameter(_) | OriginRoot::ArgumentBrand(_),
                    ..
                })
            )
        }) {
            let mut reaches = Vec::new();
            for r in t.modes.relation.reaches() {
                let mut r = r.clone();
                if let Reach::Place(Origin {
                    root: OriginRoot::Parameter(p) | OriginRoot::ArgumentBrand(p),
                    ..
                }) = &mut r
                {
                    *p = map(*p, depth)?;
                }
                reaches.push(r);
            }
            t.modes.relation = Relation::new(reaches);
        }
        match &mut t.erased_captures {
            Some(ErasedCaptures::Known(a)) => summary(a, depth, &mut map)?,
            Some(ErasedCaptures::FromArguments(ps)) => {
                for p in ps.iter_mut() {
                    p.parameter = map(p.parameter, depth)?;
                }
                ps.sort();
                ps.dedup();
            }
            None => (),
        }
        if let TypeKind::Function(f) = &mut t.kind {
            for promise in &mut f.state_promises {
                promise.selector.parameter = map(promise.selector.parameter, depth + 1)?;
            }
            summary(&mut f.authority, depth + 1, &mut map)?;
            if let Uses::Only(a) = &mut f.uses {
                summary(a, depth + 1, &mut map)?;
            }
        }
        Ok(())
    })
}
/// Rewrite generic binder identities throughout all type/size leaves and summaries.
/// The callback receives the number of callable binders crossed from the root.
pub fn map_generic_binders(
    store: &TypeInterner,
    root: TypeId,
    mut map: impl FnMut(GenericBinder, u32) -> Result<GenericBinder, GraphError>,
) -> Result<TypeId, GraphError> {
    fn reference<P: Copy>(
        p: &mut BinderRef<P>,
        depth: u32,
        map: &mut impl FnMut(GenericBinder, u32) -> Result<GenericBinder, GraphError>,
    ) -> Result<(), GraphError> {
        *p = match map(p.binder(), depth)? {
            GenericBinder::Bound(depth) => BinderRef::Bound {
                depth,
                parameter: p.parameter(),
            },
            GenericBinder::Body(scope) => BinderRef::body(scope, p.parameter()),
        };
        Ok(())
    }
    fn authority(
        a: &mut AuthoritySummary,
        depth: u32,
        map: &mut impl FnMut(GenericBinder, u32) -> Result<GenericBinder, GraphError>,
    ) -> Result<(), GraphError> {
        if let AuthoritySummary::Known(terms) = a {
            for term in terms.iter_mut() {
                if let AuthorityTerm::TypeParameter(p) = term {
                    reference(p, depth, map)?;
                }
            }
            terms.sort();
            terms.dedup();
        }
        Ok(())
    }
    rewrite_parameter_graph(store, root, |_, row, depth| {
        if let DropAllocation::DependsOn(ps) = &mut row.drop_allocation {
            for p in ps.iter_mut() {
                reference(p, depth, &mut map)?;
            }
            ps.sort();
            ps.dedup();
        }
        if let Sendability::DependsOn(ps) = &mut row.modes.sendability {
            for p in ps.iter_mut() {
                reference(p, depth, &mut map)?;
            }
            ps.sort();
            ps.dedup();
        }
        if let Some(ErasedCaptures::Known(a)) = &mut row.erased_captures {
            authority(a, depth, &mut map)?;
        }
        match &mut row.kind {
            TypeKind::Parameter(p) => reference(p, depth, &mut map)?,
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                for a in arguments {
                    if let GenericArgument::Size(Size::Parameter(p)) = a {
                        reference(p, depth, &mut map)?;
                    }
                }
            }
            TypeKind::Function(f) => {
                authority(&mut f.authority, depth + 1, &mut map)?;
                if let Uses::Only(a) = &mut f.uses {
                    authority(a, depth + 1, &mut map)?;
                }
            }
            _ => (),
        }
        Ok(())
    })
}

/// Move a type beneath additional callable binders without capturing free names.
pub fn shift_type_binders(
    store: &TypeInterner,
    root: TypeId,
    amount: u32,
) -> Result<TypeId, GraphError> {
    if amount == 0 {
        return Ok(root);
    }
    let root = map_parameter_references(store, root, |p, nested| {
        Ok(match p {
            ParameterRef::Bound { depth, parameter } if depth >= nested => ParameterRef::Bound {
                depth: depth + amount,
                parameter,
            },
            p => p,
        })
    })?;
    map_generic_binders(store, root, |b, nested| {
        Ok(match b {
            GenericBinder::Bound(d) if d >= nested => GenericBinder::Bound(d + amount),
            b => b,
        })
    })
}

/// Install entry/result evidence from a declared receiving contract. Use only
/// at body entry or after validating all flows into that contract; substitution
/// and ordinary normalization must never call this on an incoming actual.
pub fn declared_receipt_evidence(store: &TypeInterner, root: TypeId) -> Result<TypeId, GraphError> {
    rewrite_parameter_graph(store, root, |_, row, _| {
        if row.sendability_requirement == SendabilityRequirement::Sendable {
            row.modes.sendability = Sendability::Sendable;
        }
        Ok(())
    })
}

/// Comparisons observe contents without receiving their crossing contracts.
pub fn erase_comparison_crossing(store: &TypeInterner, root: TypeId) -> Result<TypeId, GraphError> {
    rewrite_parameter_graph(store, root, |_, row, _| {
        row.sendability_requirement = SendabilityRequirement::None;
        row.modes.sendability = Sendability::NotSendable;
        Ok(())
    })
}

fn rewrite_parameter_graph(
    store: &TypeInterner,
    root: TypeId,
    mut map: impl FnMut(TypeId, &mut Type, u32) -> Result<(), GraphError>,
) -> Result<TypeId, GraphError> {
    fn walk(
        store: &TypeInterner,
        id: TypeId,
        depth: u32,
        map: &mut impl FnMut(TypeId, &mut Type, u32) -> Result<(), GraphError>,
        memo: &mut std::collections::HashMap<(TypeId, u32), TypeId>,
        active: &mut std::collections::HashSet<TypeId>,
    ) -> Result<TypeId, GraphError> {
        if let Some(id) = memo.get(&(id, depth)) {
            return Ok(*id);
        }
        if !active.insert(id) {
            return Err(GraphError::StructuralCycle(id));
        }
        let original = store.get(id).ok_or(GraphError::MissingType(id))?;
        let mut t = original.clone();
        map(id, &mut t, depth)?;
        let child_depth = depth + u32::from(matches!(t.kind, TypeKind::Function(_)));
        t.kind = t
            .kind
            .try_map_edges(|id| walk(store, id, child_depth, map, memo, active))?;
        active.remove(&id);
        let out = if &t == original { id } else { store.intern(t) };
        memo.insert((id, depth), out);
        Ok(out)
    }
    walk(
        store,
        root,
        0,
        &mut map,
        &mut std::collections::HashMap::new(),
        &mut std::collections::HashSet::new(),
    )
}

/// Interpret free signature references in a checked body's lexical scope chain,
/// nearest scope first. A nested callable's own bound references stay bound.
pub fn bind_parameter_scopes(
    store: &TypeInterner,
    root: TypeId,
    scopes: &[SiteRef],
) -> Result<TypeId, GraphError> {
    let root = map_parameter_references(store, root, |p, nested| match p {
        ParameterRef::Bound { depth, parameter } if depth >= nested => scopes
            .get((depth - nested) as usize)
            .map(|scope| ParameterRef::body(*scope, parameter))
            .ok_or(GraphError::UnboundParameter(p)),
        _ => Ok(p),
    })?;
    map_generic_binders(store, root, |binder, nested| match binder {
        GenericBinder::Bound(depth) if depth >= nested => scopes
            .get((depth - nested) as usize)
            .copied()
            .map(GenericBinder::Body)
            .ok_or(GraphError::UnboundGeneric(binder)),
        b => Ok(b),
    })
}

/// Actual identity and lifetime are independent: a heap owner's identity is
/// concrete while a result selected from it can remain Global (MODE-1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OriginArgument {
    pub identity: Origin,
    pub brand: Brand,
    /// The bound the argument lends a result that names its parameter: its
    /// own relation, plus its place where that place itself bounds it (an
    /// arena, or a borrowed binding home).
    pub relation: Relation,
}

fn free_parameter(p: ParameterRef, nested: u32) -> Option<ParameterRef> {
    match p {
        ParameterRef::Bound { depth, parameter } if depth >= nested => Some(ParameterRef::Bound {
            depth: depth - nested,
            parameter,
        }),
        ParameterRef::Bound { .. } => None,
        p => Some(p),
    }
}

/// Substitute a removed call binder throughout the graph, including outer
/// references in nested callables. Actual lifetime comes from the selected
/// operand, never from whether its identity happens to be a local name.
pub fn instantiate_parameter_origins(
    store: &TypeInterner,
    root: TypeId,
    arguments: &BTreeMap<ParameterRef, OriginArgument>,
) -> Result<TypeId, GraphError> {
    instantiate_source_arguments(store, root, arguments, false)
}

/// Instantiate a dependent result from the same complete argument types used
/// for call checking (MODE-1): each named parameter is replaced by its
/// argument's relation at the same field path, so the result is the union of
/// what its sources reach, arenas included. The caller must retain loans on
/// every declared source, independently of this substitution.
pub fn instantiate_parameter_sources(
    store: &TypeInterner,
    root: TypeId,
    arguments: &BTreeMap<ParameterRef, OriginArgument>,
    actuals: &BTreeMap<ParameterRef, TypeId>,
) -> Result<TypeId, GraphError> {
    for id in actuals.values() {
        store.get(*id).ok_or(GraphError::MissingType(*id))?;
    }
    instantiate_source_arguments(store, root, arguments, true)
}

/// `relation` with each free parameter place replaced by its argument's
/// relation, the place's field path appended to the argument's places (#77).
/// A parameter without an argument keeps its place.
pub fn instantiate_relation(
    relation: &Relation,
    depth: u32,
    arguments: &BTreeMap<ParameterRef, OriginArgument>,
) -> (Relation, bool) {
    let mut selected = false;
    let relation = relation.map(|r| {
        let Reach::Place(Origin {
            root: OriginRoot::Parameter(p),
            fields,
        }) = r
        else {
            return vec![r.clone()];
        };
        let Some(actual) = free_parameter(*p, depth).and_then(|p| arguments.get(&p)) else {
            return vec![r.clone()];
        };
        selected = true;
        actual
            .relation
            .reaches()
            .iter()
            .map(|r| match r {
                Reach::Place(o) => {
                    let mut o = o.clone();
                    o.fields.extend(fields.iter().copied());
                    // Actual references belong to the caller: shift them
                    // under the nested binders crossed.
                    if let OriginRoot::Parameter(ParameterRef::Bound { depth: d, .. })
                    | OriginRoot::ArgumentBrand(ParameterRef::Bound { depth: d, .. }) =
                        &mut o.root
                    {
                        *d += depth;
                    }
                    Reach::Place(o)
                }
                r => r.clone(),
            })
            .collect()
    });
    (relation, selected)
}

fn instantiate_source_arguments(
    store: &TypeInterner,
    root: TypeId,
    arguments: &BTreeMap<ParameterRef, OriginArgument>,
    sources: bool,
) -> Result<TypeId, GraphError> {
    let identities = arguments
        .iter()
        .map(|(p, a)| (*p, a.identity.clone()))
        .collect();
    let root = instantiate_origin_identities(store, root, &identities)?;
    rewrite_parameter_graph(store, root, |_, t, depth| {
        let mut alternatives = Vec::new();
        for origin in &t.brand.alternatives {
            if let OriginRoot::ArgumentBrand(p) = origin.root {
                if let Some(actual) = free_parameter(p, depth).and_then(|p| arguments.get(&p)) {
                    alternatives.extend(actual.brand.alternatives.iter().map(|source| {
                        let mut source = source.clone();
                        source.fields.extend(&origin.fields);
                        source
                    }));
                    continue;
                }
            }
            alternatives.push(origin.clone());
        }
        alternatives.sort();
        alternatives.dedup();
        t.brand.alternatives = alternatives.into();
        // A relation names places, never an argument's retained brand.
        if let Some(p) = t.modes.relation.reaches().iter().find_map(|r| match r {
            Reach::Place(Origin {
                root: OriginRoot::ArgumentBrand(p),
                ..
            }) => Some(*p),
            _ => None,
        }) {
            return Err(GraphError::UnboundOrigin(OriginRoot::ArgumentBrand(p)));
        }
        let (relation, selected) = instantiate_relation(&t.modes.relation, depth, arguments);
        t.modes.relation = relation;
        if sources && selected && !t.modes.relation.is_global() {
            t.modes.sendability = Sendability::NotSendable;
        }
        Ok(())
    })
}

/// Identity-only refinement for an inferred type occurrence. Locality must not
/// change merely because a previously free identity becomes concrete.
fn instantiate_origin_identities(
    store: &TypeInterner,
    root: TypeId,
    arguments: &BTreeMap<ParameterRef, Origin>,
) -> Result<TypeId, GraphError> {
    fn origin(o: &mut Origin, depth: u32, arguments: &BTreeMap<ParameterRef, Origin>) {
        let OriginRoot::Parameter(p) = o.root else {
            return;
        };
        if let Some(actual) = free_parameter(p, depth).and_then(|p| arguments.get(&p)) {
            let fields = std::mem::take(&mut o.fields);
            *o = actual.clone();
            o.fields.extend(fields);
        }
    }
    rewrite_parameter_graph(store, root, |_, t, depth| {
        for o in &mut t.brand.alternatives {
            origin(o, depth, arguments);
        }
        t.brand.alternatives.sort();
        t.brand.alternatives.dedup();
        Ok(())
    })
}

/// Refresh unresolved free origins and erased-capture selections in an inferred
/// generic actual from that occurrence's checked value. Concrete declared facts
/// and nested binders remain intact; later occurrences must satisfy the same
/// contract. Unknown erased types are never strengthened from another argument.
pub fn refine_inferred_value_contract(
    store: &TypeInterner,
    root: TypeId,
    actual: TypeId,
) -> Result<TypeId, GraphError> {
    let mut pending = vec![(root, actual, 0)];
    let mut seen = std::collections::HashSet::new();
    let mut origins = BTreeMap::new();
    let mut captures = std::collections::HashMap::new();
    let mut weakened = std::collections::HashSet::new();
    while let Some((old, new, depth)) = pending.pop() {
        if !seen.insert((old, new, depth)) {
            continue;
        }
        let old_id = old;
        let old = store.get(old).ok_or(GraphError::MissingType(old))?;
        let new = store.get(new).ok_or(GraphError::MissingType(new))?;
        if std::mem::discriminant(&old.kind) != std::mem::discriminant(&new.kind) {
            continue;
        }
        // A part the checked value holds as a counted copy is aliased, whatever
        // the row it was inferred from said. The root's modes are the
        // parameter's own and are not part of the type argument.
        if old_id != root
            && old.modes.uniqueness == Uniqueness::Unique
            && new.modes.uniqueness == Uniqueness::Aliased
        {
            weakened.insert((old_id, depth));
        }
        if matches!(&old.erased_captures, Some(ErasedCaptures::FromArguments(ps))
            if !ps.is_empty() && ps.iter().all(|p| matches!(p.parameter, ParameterRef::Bound { depth: d, .. } if d >= depth)))
            && !matches!(&new.erased_captures, Some(ErasedCaptures::FromArguments(_)))
        {
            captures
                .entry((old_id, depth))
                .or_insert((new.erased_captures.clone(), new.modes.sendability.clone()));
        }
        for (old, new) in old.brand.alternatives.iter().zip(&new.brand.alternatives) {
            if let OriginRoot::Parameter(ParameterRef::Bound {
                depth: d,
                parameter,
            }) = old.root
            {
                if d >= depth && old.fields == new.fields {
                    let mut origin = new.clone();
                    origin.fields.clear();
                    origins
                        .entry(ParameterRef::Bound {
                            depth: d - depth,
                            parameter,
                        })
                        .or_insert(origin);
                }
            }
        }
        pending.extend(
            old.kind
                .graph_edges()
                .into_iter()
                .zip(new.kind.graph_edges())
                .map(|(a, b)| (a.ty, b.ty, depth + a.role.parameter_binders())),
        );
    }
    let root = rewrite_parameter_graph(store, root, |id, row, depth| {
        if let Some((evidence, sendability)) = captures.get(&(id, depth)) {
            row.erased_captures = evidence.clone();
            row.modes.sendability = sendability.clone();
        }
        if weakened.contains(&(id, depth)) {
            row.modes.uniqueness = Uniqueness::Aliased;
        }
        Ok(())
    })?;
    instantiate_origin_identities(store, root, &origins)
}

/// Instantiate the held/allowed authority in callback types from the actual
/// arguments of the surrounding call. Inner callable parameters remain bound.
pub fn instantiate_parameter_authorities(
    store: &TypeInterner,
    root: TypeId,
    arguments: &BTreeMap<ParameterRef, AuthoritySummary>,
) -> Result<TypeId, GraphError> {
    fn summary(
        a: &mut AuthoritySummary,
        depth: u32,
        arguments: &BTreeMap<ParameterRef, AuthoritySummary>,
    ) {
        let AuthoritySummary::Known(terms) = a else {
            return;
        };
        *a = terms.iter().fold(AuthoritySummary::inert(), |a, t| {
            let key = match t {
                AuthorityTerm::Parameter(ParameterRef::Bound {
                    depth: d,
                    parameter,
                }) if *d >= depth => Some(ParameterRef::Bound {
                    depth: d - depth,
                    parameter: *parameter,
                }),
                AuthorityTerm::Parameter(p @ ParameterRef::Body { .. }) => Some(*p),
                _ => None,
            };
            a.join(
                key.and_then(|p| arguments.get(&p))
                    .cloned()
                    .unwrap_or_else(|| AuthoritySummary::Known(vec![t.clone()].into())),
            )
        });
    }
    rewrite_parameter_graph(store, root, |_, t, depth| {
        if let Some(ErasedCaptures::Known(a)) = &mut t.erased_captures {
            summary(a, depth, arguments);
        }
        if let TypeKind::Function(f) = &mut t.kind {
            summary(&mut f.authority, depth + 1, arguments);
            if let Uses::Only(a) = &mut f.uses {
                summary(a, depth + 1, arguments);
            }
        }
        Ok(())
    })
}

/// Publish a checked callable using its own lexical binder while retaining
/// concrete references to captured outer scopes. `root` is the callable value.
pub fn abstract_parameter_scope(
    store: &TypeInterner,
    root: TypeId,
    scope: SiteRef,
) -> Result<TypeId, GraphError> {
    let root = map_parameter_references(store, root, |p, depth| match p {
        ParameterRef::Body {
            scope: owner,
            parameter,
        } if owner == scope => depth
            .checked_sub(1)
            .map(|depth| ParameterRef::Bound { depth, parameter })
            .ok_or(GraphError::UnboundParameter(p)),
        p => Ok(p),
    })?;
    map_generic_binders(store, root, |b, depth| match b {
        GenericBinder::Body(owner) if owner == scope => depth
            .checked_sub(1)
            .map(GenericBinder::Bound)
            .ok_or(GraphError::UnboundGeneric(b)),
        b => Ok(b),
    })
}

/// Instantiate an erased environment from the selected actual values. The
/// selection belongs to the declared callable, never to its claimed result type.
/// Inner callable binders and unresolved captured outer references stay intact.
pub fn instantiate_parameter_captures(
    store: &TypeInterner,
    root: TypeId,
    arguments: &BTreeMap<RelationPlace, CaptureProperties>,
) -> Result<TypeId, GraphError> {
    rewrite_parameter_graph(store, root, |_, t, depth| {
        let Some(ErasedCaptures::FromArguments(parameters)) = &t.erased_captures else {
            return Ok(());
        };
        let mut authority = AuthoritySummary::inert();
        let mut sendability = Sendability::Sendable;
        for p in parameters {
            let parameter = match p.parameter {
                ParameterRef::Bound {
                    depth: d,
                    parameter,
                } if d >= depth => ParameterRef::Bound {
                    depth: d - depth,
                    parameter,
                },
                ParameterRef::Bound { .. } => return Ok(()),
                p => p,
            };
            let key = RelationPlace {
                parameter,
                fields: p.fields.clone(),
            };
            let Some(actual) = arguments.get(&key) else {
                return Ok(());
            };
            authority = authority.join(actual.authority.clone());
            sendability = match (sendability, &actual.sendability) {
                (Sendability::NotSendable, _) | (_, Sendability::NotSendable) => {
                    Sendability::NotSendable
                }
                (Sendability::Sendable, actual) => actual.clone(),
                (current, Sendability::Sendable) => current,
                (Sendability::DependsOn(mut ps), Sendability::DependsOn(other)) => {
                    ps.extend(other);
                    ps.sort();
                    ps.dedup();
                    Sendability::DependsOn(ps)
                }
            };
        }
        t.erased_captures = Some(ErasedCaptures::Known(authority));
        // MODE-4: this is evidence about the erased environment, independent
        // of the iterator view's own relation (MODE-1).
        t.modes.sendability = sendability;
        Ok(())
    })
}

/// Resolve an allocation destination from the allocator denoted by an actual.
/// A relation naming an Arena parameter whose argument is the ambient heap
/// names nothing there: the heap is the empty relation (PROV-4). Inner callable
/// parameters retain their binder; field projections of an Arena token do not
/// denote the same allocator.
pub fn instantiate_allocation_targets(
    store: &TypeInterner,
    root: TypeId,
    arguments: &BTreeMap<ParameterRef, AllocationTarget>,
) -> Result<TypeId, GraphError> {
    rewrite_parameter_graph(store, root, |_, t, depth| {
        let heap = |r: &Reach| {
            let Reach::Place(Origin {
                root: OriginRoot::Parameter(p),
                fields,
            }) = r
            else {
                return false;
            };
            fields.is_empty()
                && free_parameter(*p, depth).and_then(|p| arguments.get(&p))
                    == Some(&AllocationTarget::Heap)
        };
        if t.modes.relation.reaches().iter().any(heap) {
            t.modes.relation = t.modes.relation.retain(|r| !heap(r));
        }
        Ok(())
    })
}

/// Rebuild origin-bearing edges with lexical binder depth. The callback must
/// preserve inner bound parameters; graph shape, modes and authority are untouched.
pub fn map_type_origins(
    store: &TypeInterner,
    root: TypeId,
    map: impl FnMut(&Origin, u32) -> Result<Origin, GraphError>,
) -> Result<TypeId, GraphError> {
    map_origins(store, root, true, true, map)
}

/// Map only the brand origins of a type graph, leaving relation places.
pub fn map_brand_origins(
    store: &TypeInterner,
    root: TypeId,
    map: impl FnMut(&Origin, u32) -> Result<Origin, GraphError>,
) -> Result<TypeId, GraphError> {
    map_origins(store, root, true, false, map)
}

/// Remove every brand of a type graph, nested rows included, leaving relation
/// places, modes and shape.
pub fn erase_brands(store: &TypeInterner, root: TypeId) -> Result<TypeId, GraphError> {
    rewrite_parameter_graph(store, root, |_, row, _| {
        row.brand = Brand::default();
        Ok(())
    })
}

/// Remove every `unique` qualifier of a type graph, nested rows included,
/// leaving brands, relations and shape (MODE-2).
pub fn erase_uniqueness(store: &TypeInterner, root: TypeId) -> Result<TypeId, GraphError> {
    rewrite_parameter_graph(store, root, |_, row, _| {
        row.modes.uniqueness = Uniqueness::Aliased;
        Ok(())
    })
}

/// Map only the relation places of a type graph, leaving brand origins.
pub fn map_relation_places(
    store: &TypeInterner,
    root: TypeId,
    map: impl FnMut(&Origin, u32) -> Result<Origin, GraphError>,
) -> Result<TypeId, GraphError> {
    map_origins(store, root, false, true, map)
}

fn map_origins(
    store: &TypeInterner,
    root: TypeId,
    brands: bool,
    relations: bool,
    mut map: impl FnMut(&Origin, u32) -> Result<Origin, GraphError>,
) -> Result<TypeId, GraphError> {
    rewrite_parameter_graph(store, root, |_, row, depth| {
        if brands {
            for origin in &mut row.brand.alternatives {
                *origin = map(origin, depth)?;
            }
            row.brand.alternatives.sort();
            row.brand.alternatives.dedup();
        }
        if relations
            && row
                .modes
                .relation
                .reaches()
                .iter()
                .any(|r| matches!(r, Reach::Place(_)))
        {
            let mut reaches = Vec::new();
            for r in row.modes.relation.reaches() {
                reaches.push(match r {
                    Reach::Place(origin) => Reach::Place(map(origin, depth)?),
                    r => r.clone(),
                });
            }
            row.modes.relation = Relation::new(reaches);
        }
        Ok(())
    })
}

/// Resolve a record field's sibling selectors from the supplied field identities.
/// An absent field stays unresolved; it is never silently erased into an
/// unbranded value or an inferred allocation destination.
pub fn instantiate_sibling_origins(
    store: &TypeInterner,
    root: TypeId,
    fields: &BTreeMap<FieldId, Origin>,
) -> Result<TypeId, GraphError> {
    map_type_origins(store, root, |origin, nested| {
        let OriginRoot::Sibling(field) = origin.root else {
            return Ok(origin.clone());
        };
        let Some(actual) = fields.get(&field) else {
            return Ok(origin.clone());
        };
        let mut actual = actual.clone();
        if let OriginRoot::Parameter(ParameterRef::Bound { depth, .. })
        | OriginRoot::ArgumentBrand(ParameterRef::Bound { depth, .. }) = &mut actual.root
        {
            *depth += nested;
        }
        actual.fields.extend(&origin.fields);
        Ok(actual)
    })
}
