//! Lexical value-parameter transport, independent of syntax traversal and storage.
type IdSet<T> = std::collections::HashSet<T, hirsel_intern::IdHasher>;
type IdMap<K, V> = std::collections::HashMap<K, V, hirsel_intern::IdHasher>;
use crate::*;
use std::collections::BTreeMap;

#[cfg(test)]
thread_local! { static REWRITE_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
static FORCE_FULL_REWRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

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

const BRANDS: u8 = 1;
const RELATIONS: u8 = 2;
const AUTHORITIES: u8 = 4;
const CAPTURES: u8 = 8;
const GENERICS: u8 = 16;
const CALLABLES: u8 = 32;
const ALL_FEATURES: u8 = u8::MAX;

/// Generic transport changes type/size references, their derived summaries,
/// and a callable's role-derived error channel, never concrete child graphs.
pub(crate) fn needs_substitution(store: &TypeInterner, root: TypeId, result_role: bool) -> bool {
    let mask = GENERICS | AUTHORITIES | if result_role { CALLABLES } else { 0 };
    rewrite_features(store, root) & mask != 0
}

#[cfg(test)]
pub(crate) fn with_full_rewrites<T>(f: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            FORCE_FULL_REWRITE.with(|force| force.set(self.0));
        }
    }
    let _reset = Reset(FORCE_FULL_REWRITE.with(|force| force.replace(true)));
    f()
}

/// Pure reachability summaries, like rustc15c1044 type_ir/flags.rs:5-23
/// and middle/ty/erase_regions.rs:26-34: skip a rewrite when its input
/// cannot contain any selected feature. Immutable rows share this summary.
/// Image forward references require lazy computation. Invalid/recovery graphs
/// conservatively retain the original walk and its first-error ordering.
fn rewrite_features(store: &TypeInterner, root: TypeId) -> u8 {
    #[cfg(test)]
    if FORCE_FULL_REWRITE.with(std::cell::Cell::get) {
        return ALL_FEATURES;
    }
    let cached = |id| {
        store
            .entry(id)
            .and_then(|e| e.rewrite_features.get())
            .copied()
    };
    if let Some(flags) = cached(root) {
        return flags;
    }
    if !store.valid(root) {
        return ALL_FEATURES;
    }
    let known = |authority: &AuthoritySummary| matches!(authority, AuthoritySummary::Known(terms) if !terms.is_empty());
    let mut pending = vec![(root, false)];
    while let Some((id, exit)) = pending.pop() {
        if cached(id).is_some() {
            continue;
        }
        let entry = store.entry(id).expect("a valid row");
        let row = &entry.row;
        if !exit {
            pending.push((id, true));
            pending.extend(
                row.kind
                    .graph_edges()
                    .into_iter()
                    .rev()
                    .map(|edge| (edge.ty, false)),
            );
            continue;
        }
        let mut flags = 0;
        if matches!(row.drop_allocation, DropAllocation::DependsOn(_))
            || matches!(row.modes.sendability, Sendability::DependsOn(_))
            || matches!(row.kind, TypeKind::Parameter(_))
        {
            flags |= GENERICS;
        }
        if matches!(&row.kind, TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. }
            if arguments.iter().any(|a| matches!(a, GenericArgument::Size(Size::Parameter(_)))))
        {
            flags |= GENERICS;
        }
        if !row.brand.alternatives.is_empty() {
            flags |= BRANDS;
        }
        if !row.modes.relation.is_global() {
            flags |= RELATIONS;
        }
        match &row.erased_captures {
            Some(ErasedCaptures::FromArguments(_)) => flags |= CAPTURES,
            Some(ErasedCaptures::Known(authority)) if known(authority) => flags |= AUTHORITIES,
            _ => (),
        }
        if let TypeKind::Function(f) = &row.kind {
            flags |= CALLABLES;
            if known(&f.authority) || matches!(&f.uses, Uses::Only(authority) if known(authority)) {
                flags |= AUTHORITIES;
            }
        }
        for edge in row.kind.graph_edges() {
            flags |= cached(edge.ty).expect("child summary before parent");
        }
        let _ = entry.rewrite_features.set(flags);
    }
    cached(root).expect("root summary completed")
}

fn rewrite_parameter_graph(
    store: &TypeInterner,
    root: TypeId,
    map: impl FnMut(TypeId, &mut Type, u32) -> Result<(), GraphError>,
) -> Result<TypeId, GraphError> {
    rewrite_parameter_graph_for(store, root, ALL_FEATURES, map)
}

fn rewrite_parameter_graph_for(
    store: &TypeInterner,
    root: TypeId,
    features: u8,
    mut map: impl FnMut(TypeId, &mut Type, u32) -> Result<(), GraphError>,
) -> Result<TypeId, GraphError> {
    fn walk(
        store: &TypeInterner,
        id: TypeId,
        depth: u32,
        features: u8,
        map: &mut impl FnMut(TypeId, &mut Type, u32) -> Result<(), GraphError>,
        memo: &mut IdMap<(TypeId, u32), TypeId>,
        active: &mut IdSet<TypeId>,
    ) -> Result<TypeId, GraphError> {
        if features != ALL_FEATURES && rewrite_features(store, id) & features == 0 {
            return Ok(id);
        }
        if let Some(id) = memo.get(&(id, depth)) {
            return Ok(*id);
        }
        if !active.insert(id) {
            return Err(GraphError::StructuralCycle(id));
        }
        #[cfg(test)]
        REWRITE_ROWS.with(|rows| rows.set(rows.get() + 1));
        let original = store.get(id).ok_or(GraphError::MissingType(id))?;
        let mut t = original.clone();
        map(id, &mut t, depth)?;
        let child_depth = depth + u32::from(matches!(t.kind, TypeKind::Function(_)));
        t.kind = t
            .kind
            .try_map_edges(|id| walk(store, id, child_depth, features, map, memo, active))?;
        active.remove(&id);
        let out = if &t == original { id } else { store.intern(t) };
        memo.insert((id, depth), out);
        Ok(out)
    }
    walk(
        store,
        root,
        0,
        features,
        &mut map,
        &mut IdMap::default(),
        &mut IdSet::default(),
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

#[cfg(test)]
thread_local! { static COPIED_IDENTITY_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn instantiate_source_arguments(
    store: &TypeInterner,
    root: TypeId,
    arguments: &BTreeMap<ParameterRef, OriginArgument>,
    sources: bool,
) -> Result<TypeId, GraphError> {
    // Rust alloc/collections/btree/map.rs:727-741: borrow the selected value
    // from the retained argument map rather than cloning a projected map.
    let root =
        instantiate_origin_identities(store, root, |p| arguments.get(&p).map(|a| &a.identity))?;
    rewrite_parameter_graph_for(store, root, BRANDS | RELATIONS, |_, t, depth| {
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
fn instantiate_origin_identities<'a>(
    store: &TypeInterner,
    root: TypeId,
    arguments: impl Fn(ParameterRef) -> Option<&'a Origin>,
) -> Result<TypeId, GraphError> {
    fn origin<'a>(
        o: &mut Origin,
        depth: u32,
        arguments: &impl Fn(ParameterRef) -> Option<&'a Origin>,
    ) {
        let OriginRoot::Parameter(p) = o.root else {
            return;
        };
        if let Some(actual) = free_parameter(p, depth).and_then(arguments) {
            let fields = std::mem::take(&mut o.fields);
            *o = actual.clone();
            o.fields.extend(fields);
        }
    }
    rewrite_parameter_graph_for(store, root, BRANDS, |_, t, depth| {
        for o in &mut t.brand.alternatives {
            origin(o, depth, &arguments);
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
    let mut seen = IdSet::default();
    let mut origins = BTreeMap::new();
    let mut captures = IdMap::default();
    let mut weakened = IdSet::default();
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
    instantiate_origin_identities(store, root, |p| origins.get(&p))
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
    rewrite_parameter_graph_for(store, root, AUTHORITIES, |_, t, depth| {
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

/// Comparison-only expansion of a callable's own selectors whose parameter
/// authority is fixed (a nominal capability or an inert value). Unknown and
/// attenuable callable environments keep their exact symbolic selector. The
/// caller derives fixed summaries from its own declaration provider; published
/// contracts retain ParamIds for actual-argument substitution at invocation.
pub fn normalize_callable_authorities(
    store: &TypeInterner,
    root: TypeId,
    mut fixed: impl FnMut(TypeId) -> Option<AuthoritySummary>,
) -> Result<TypeId, GraphError> {
    rewrite_parameter_graph_for(store, root, AUTHORITIES, |_, row, _| {
        let TypeKind::Function(function) = &mut row.kind else {
            return Ok(());
        };
        let parameters: Vec<_> = function.parameters.iter().map(|p| fixed(p.ty)).collect();
        let expand = |summary: &mut AuthoritySummary| {
            if let AuthoritySummary::Known(terms) = summary {
                *summary = terms.iter().fold(AuthoritySummary::inert(), |out, term| {
                    let replacement = match term {
                        AuthorityTerm::Parameter(ParameterRef::Bound {
                            depth: 0,
                            parameter,
                        }) => parameters.get(parameter.0 as usize).and_then(|a| a.clone()),
                        _ => None,
                    };
                    out.join(
                        replacement
                            .unwrap_or_else(|| AuthoritySummary::Known(vec![term.clone()].into())),
                    )
                });
            }
        };
        expand(&mut function.authority);
        if let Uses::Only(uses) = &mut function.uses {
            expand(uses);
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
    rewrite_parameter_graph_for(store, root, CAPTURES, |_, t, depth| {
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
    rewrite_parameter_graph_for(store, root, RELATIONS, |_, t, depth| {
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
    rewrite_parameter_graph_for(store, root, BRANDS, |_, row, _| {
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
    rewrite_parameter_graph_for(
        store,
        root,
        (u8::from(brands) * BRANDS) | (u8::from(relations) * RELATIONS),
        |_, row, depth| {
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
        },
    )
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

#[cfg(test)]
mod rewrite_tests {
    use super::*;

    #[test]
    fn irrelevant_call_rewrites_skip_unchanged_dags_and_preserve_error_order() {
        let t = TypeInterner::new(None);
        let mut root = t.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
        for _ in 0..128 {
            root = t.intern(Type::with_kind(TypeKind::Tuple(vec![root, root].into())));
        }
        let before = REWRITE_ROWS.with(std::cell::Cell::get);
        for _ in 0..5 {
            assert_eq!(
                instantiate_parameter_sources(&t, root, &BTreeMap::new(), &BTreeMap::new()),
                Ok(root)
            );
            assert_eq!(
                instantiate_allocation_targets(&t, root, &BTreeMap::new()),
                Ok(root)
            );
            assert_eq!(
                instantiate_parameter_authorities(&t, root, &BTreeMap::new()),
                Ok(root)
            );
            assert_eq!(
                instantiate_parameter_captures(&t, root, &BTreeMap::new()),
                Ok(root)
            );
        }
        assert_eq!(
            REWRITE_ROWS.with(std::cell::Cell::get),
            before,
            "irrelevant call rewrites must not revisit an unchanged DAG"
        );

        let origin = Origin {
            root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
            fields: vec![].into(),
        };
        let mut broken = Type::with_kind(TypeKind::Box(TypeId::DANGLING));
        broken.brand.alternatives = vec![origin].into();
        let broken = t.intern(broken);
        let expected = GraphError::UnboundParameter(ParameterRef::bound(ParamId(0)));
        assert_eq!(
            map_type_origins(&t, broken, |_, _| Err(expected.clone())),
            Err(expected),
            "the root callback error precedes a missing child"
        );
        assert_eq!(
            instantiate_parameter_captures(&t, broken, &BTreeMap::new()),
            Err(GraphError::MissingType(TypeId::DANGLING))
        );
        let cycle = t.intern_graph(1, |ids| vec![Type::with_kind(TypeKind::Box(ids[0]))])[0];
        assert_eq!(
            instantiate_parameter_authorities(&t, cycle, &BTreeMap::new()),
            Err(GraphError::StructuralCycle(cycle))
        );
        let recovery = t.intern(Type::with_kind(TypeKind::Error));
        assert_eq!(
            instantiate_parameter_captures(&t, recovery, &BTreeMap::new()),
            Ok(recovery)
        );
    }
}

#[cfg(test)]
mod feature_oracle_tests {
    use super::*;
    use crate::declarations::tests::path;

    fn rewrites(t: &TypeInterner, root: TypeId) -> Vec<Result<TypeId, GraphError>> {
        let actual = t.intern(Type::with_kind(TypeKind::Primitive(Primitive::U8)));
        let parameter = ParameterRef::bound(ParamId(0));
        let origin = Origin {
            root: OriginRoot::Definition(path("actual")),
            fields: vec![FieldId(1)].into(),
        };
        let arguments = [(
            parameter,
            OriginArgument {
                identity: origin.clone(),
                brand: Brand {
                    alternatives: vec![origin.clone()].into(),
                },
                relation: Relation::place(origin.clone()),
            },
        )]
        .into();
        let capture = CaptureProperties::from_type(
            t.row(actual),
            AuthoritySummary::inert(),
            Sendability::Sendable,
        );
        vec![
            instantiate_parameter_sources(t, root, &arguments, &[(parameter, actual)].into()),
            instantiate_allocation_targets(t, root, &[(parameter, AllocationTarget::Heap)].into()),
            instantiate_parameter_authorities(
                t,
                root,
                &[(parameter, AuthoritySummary::inert())].into(),
            ),
            instantiate_parameter_captures(
                t,
                root,
                &[(
                    RelationPlace {
                        parameter,
                        fields: vec![].into(),
                    },
                    capture,
                )]
                .into(),
            ),
            map_type_origins(t, root, |_, _| Ok(origin.clone())),
            erase_brands(t, root),
        ]
    }

    #[test]
    fn feature_pruning_matches_full_rewrites_across_nested_and_invalid_graphs() {
        let t = TypeInterner::new(None);
        let plain = t.intern(Type::with_kind(TypeKind::Primitive(Primitive::U8)));
        let mut roots = vec![plain, TypeId::DANGLING];
        for bits in 0..32 {
            let origin = Origin {
                root: if bits & 16 == 0 {
                    OriginRoot::Parameter(ParameterRef::bound(ParamId(0)))
                } else {
                    OriginRoot::ArgumentBrand(ParameterRef::bound(ParamId(0)))
                },
                fields: vec![].into(),
            };
            let mut row = Type::with_kind(TypeKind::Tuple(vec![plain, plain].into()));
            if bits & 1 != 0 {
                row.brand.alternatives = vec![origin.clone(), origin.clone()].into();
            }
            if bits & 2 != 0 {
                row.modes.relation = Relation::place(origin);
            }
            if bits & 4 != 0 {
                row.erased_captures = Some(ErasedCaptures::FromArguments(
                    vec![RelationPlace {
                        parameter: ParameterRef::bound(ParamId(0)),
                        fields: vec![].into(),
                    }]
                    .into(),
                ));
            }
            let child = t.intern(row);
            let function = t.intern(Type::with_kind(TypeKind::Function(FunctionType {
                state_promises: Default::default(),
                generics: vec![].into(),
                parameters: vec![].into(),
                result: child,
                error: Some(plain),
                allocation: Allocation::MayAllocate,
                authority: if bits & 8 != 0 {
                    AuthoritySummary::Known(
                        vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0)))].into(),
                    )
                } else {
                    AuthoritySummary::inert()
                },
                uses: Uses::Unrestricted,
                contracts: vec![].into(),
                incomplete: false,
                foreign: None,
            })));
            roots.extend([
                child,
                function,
                t.intern(Type::with_kind(TypeKind::Tuple(
                    vec![function, child, plain].into(),
                ))),
            ]);
        }
        roots.push(t.intern_graph(1, |ids| vec![Type::with_kind(TypeKind::Box(ids[0]))])[0]);
        roots.push(t.intern(Type::with_kind(TypeKind::Box(TypeId::DANGLING))));
        roots.push(t.intern(Type::with_kind(TypeKind::Error)));
        for root in roots {
            let pruned = rewrites(&t, root);
            FORCE_FULL_REWRITE.with(|force| force.set(true));
            let original = rewrites(&t, root);
            FORCE_FULL_REWRITE.with(|force| force.set(false));
            assert_eq!(pruned, original, "root {root:?}");
        }
    }
}

#[cfg(test)]
mod borrowed_identity_tests {
    use super::*;
    use crate::declarations::tests::path;
    #[test]
    fn source_instantiation_borrows_identity_inputs_and_preserves_source_and_brand_paths() {
        let t = TypeInterner::new(None);
        let scalar = t.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
        let arguments: BTreeMap<_, _> = (0..64)
            .map(|i| {
                let identity = Origin {
                    root: OriginRoot::Definition(path(&format!("actual_{i}"))),
                    fields: vec![FieldId(1)].into(),
                };
                (
                    ParameterRef::bound(ParamId(i)),
                    OriginArgument {
                        identity: identity.clone(),
                        brand: Brand {
                            alternatives: vec![identity].into(),
                        },
                        relation: Relation::place(Origin {
                            root: OriginRoot::Definition(path(&format!("home_{i}"))),
                            fields: vec![].into(),
                        }),
                    },
                )
            })
            .collect();
        let actuals = (0..64)
            .map(|i| (ParameterRef::bound(ParamId(i)), scalar))
            .collect();
        let retained = arguments.clone();
        let p = ParameterRef::bound(ParamId(0));
        let mut row = t.row(scalar).clone();
        row.brand.alternatives = vec![Origin {
            root: OriginRoot::Parameter(p),
            fields: vec![FieldId(2)].into(),
        }]
        .into();
        row.modes.relation = Relation::place(Origin {
            root: OriginRoot::Parameter(p),
            fields: vec![FieldId(4)].into(),
        });
        row.modes.sendability = Sendability::Sendable;
        let root = t.intern(row);
        let before = COPIED_IDENTITY_ROWS.with(std::cell::Cell::get);
        for _ in 0..8 {
            assert_eq!(
                instantiate_parameter_sources(&t, scalar, &arguments, &actuals),
                Ok(scalar)
            );
            let replaced = instantiate_parameter_sources(&t, root, &arguments, &actuals).unwrap();
            let row = t.row(replaced);
            let mut identity = arguments[&p].identity.clone();
            identity.fields.push(FieldId(2));
            assert_eq!(row.brand.alternatives.as_slice(), &[identity]);
            let home = Origin {
                root: OriginRoot::Definition(path("home_0")),
                fields: vec![FieldId(4)].into(),
            };
            assert_eq!(row.modes.relation, Relation::place(home.clone()));
            assert_eq!(row.modes.sendability, Sendability::NotSendable);
            let origins = instantiate_parameter_origins(&t, root, &arguments).unwrap();
            assert_eq!(t.row(origins).modes.sendability, Sendability::Sendable);
        }
        assert_eq!(arguments, retained);
        assert_eq!(
            instantiate_parameter_sources(&t, scalar, &arguments, &[(p, TypeId::DANGLING)].into()),
            Err(GraphError::MissingType(TypeId::DANGLING))
        );
        assert_eq!(
            COPIED_IDENTITY_ROWS.with(std::cell::Cell::get) - before,
            0,
            "select identities from retained arguments without constructing another map"
        );
    }
}
