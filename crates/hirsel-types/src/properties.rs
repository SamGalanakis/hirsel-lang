//! Structural value properties use declaration snapshots, never implementations.
use crate::*;
use std::collections::BTreeSet;

/// Leaves whose declaration evidence is supplied by the constructing phase.
/// Components are already normalized in body/registry construction; signature
/// construction visits them recursively, including its unfinished declaration.
pub enum CleanupLeaf<'a> {
    Component(TypeId),
    Parameter(TypeParameterRef),
    Nominal(&'a hirsel_syntax::DefPath, &'a [GenericArgument]),
}

/// MEM-5 cleanup algebra, independent of invocation allocation and value modes.
/// Lookup, generic scope and recursive declaration construction remain explicit
/// in the leaf callback; no implementation body supplies a declaration proof.
pub fn cleanup_summary(
    kind: &TypeKind,
    mut leaf: impl FnMut(CleanupLeaf<'_>) -> DropAllocation,
) -> DropAllocation {
    let mut join = |ids: Vec<TypeId>| {
        ids.into_iter().fold(DropAllocation::NoAlloc, |a, t| {
            a.join(leaf(CleanupLeaf::Component(t)))
        })
    };
    match kind {
        TypeKind::Primitive(_) => DropAllocation::NoAlloc,
        TypeKind::Tuple(ts) => join(ts.to_vec()),
        TypeKind::Record(fs) => join(fs.iter().map(|f| f.ty).collect()),
        TypeKind::Variant(vs) => join(
            vs.iter()
                .flat_map(|v| v.fields.iter().map(|f| f.1))
                .collect(),
        ),
        TypeKind::Box(t) => leaf(CleanupLeaf::Component(*t)),
        TypeKind::Parameter(p) => leaf(CleanupLeaf::Parameter(*p)),
        TypeKind::Named {
            definition,
            arguments,
        } => leaf(CleanupLeaf::Nominal(definition, arguments)),
        TypeKind::Function(_) | TypeKind::Error => DropAllocation::MayAllocate,
        TypeKind::Builtin { kind, arguments } => match kind {
            BuiltinType::Ptr
            | BuiltinType::FnPtr
            | BuiltinType::SlotId
            | BuiltinType::Atomic
            | BuiltinType::TypeShape => DropAllocation::NoAlloc,
            BuiltinType::Iter | BuiltinType::SortedMap => DropAllocation::MayAllocate,
            _ => join(
                arguments
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(t) => Some(*t),
                        GenericArgument::Size(_) => None,
                    })
                    .collect(),
            ),
        },
    }
}

/// Specialize a published declaration's cleanup promise using owned actuals.
/// An opaque declaration needs an exported NoallocDrop or Plain proof; a resource
/// additionally needs its declared finalizer promise. Missing actuals prove nothing.
pub fn declared_cleanup(
    declaration: &TypeDefinition,
    types: &TypeStore,
    arguments: &[GenericArgument],
    mut component: impl FnMut(TypeId) -> DropAllocation,
) -> DropAllocation {
    if declaration.resource && declaration.destructor_allocation != Allocation::NoAlloc {
        return DropAllocation::MayAllocate;
    }
    if declaration.capability.is_some() {
        // Whole seals keep one public destruction contract, including in the
        // owner. Private field cleanup is proved at their declaration instead.
        return if declaration.bounds.contains(&Bound::NoallocDrop)
            || declaration.bounds.contains(&Bound::Plain)
        {
            DropAllocation::NoAlloc
        } else {
            DropAllocation::MayAllocate
        };
    }
    let effect = declaration
        .representation
        .and_then(|r| types.get(r))
        .map(|r| &r.drop_allocation);
    match effect {
        Some(effect) => specialized_cleanup(effect, arguments, component),
        None if declaration.bounds.contains(&Bound::NoallocDrop)
            || declaration.bounds.contains(&Bound::Plain) =>
        {
            DropAllocation::NoAlloc
        }
        None => DropAllocation::MayAllocate,
    }
}

/// A declaration's cleanup effect over its own parameters, specialized by a
/// use's owned actuals. Missing actuals prove nothing.
pub fn specialized_cleanup(
    effect: &DropAllocation,
    arguments: &[GenericArgument],
    mut component: impl FnMut(TypeId) -> DropAllocation,
) -> DropAllocation {
    match effect {
        DropAllocation::DependsOn(ps) => ps.iter().fold(DropAllocation::NoAlloc, |a, p| {
            a.join(
                match (p.binder() == GenericBinder::Bound(0))
                    .then(|| arguments.get(p.parameter().0 as usize))
                    .flatten()
                {
                    Some(GenericArgument::Type(t)) => component(*t),
                    _ => DropAllocation::MayAllocate,
                },
            )
        }),
        effect => effect.clone(),
    }
}

pub fn parameter_cleanup(
    generics: &GenericDeclarations,
    parameter: TypeParameterRef,
) -> DropAllocation {
    if matches!(generics.get(parameter), Some(GenericParameter::Type { bounds, .. }) if bounds.contains(&Bound::NoallocDrop) || bounds.contains(&Bound::Plain))
    {
        DropAllocation::NoAlloc
    } else {
        DropAllocation::DependsOn(vec![parameter].into())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuralProperty {
    Plain,
    Copyable,
    Sendable,
    NeedsRelease,
}
impl StructuralProperty {
    pub fn bound(self) -> Bound {
        match self {
            Self::Plain | Self::NeedsRelease => Bound::Plain,
            Self::Copyable => Bound::Copyable,
            Self::Sendable => Bound::Sendable,
        }
    }
}
impl Signature {
    pub fn generics(&self) -> &[GenericParameter] {
        match &self.kind {
            SignatureKind::Function(f) => &f.generics,
            SignatureKind::Type(t) => &t.generics,
            SignatureKind::Const { generics, .. } => generics,
        }
    }
}

/// Missing declarations and malformed graphs prove nothing. Lookup errors remain
/// caller-owned, so providers retain their dependency and diagnostic policies.
pub fn structural_property(
    source: &TypeStore,
    root: TypeId,
    property: StructuralProperty,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    let summary = prove(
        source,
        root,
        property.bound(),
        generics,
        true,
        true,
        false,
        false,
        lookup,
    )?;
    let proven = summary == Sendability::Sendable;
    Ok(if property == StructuralProperty::NeedsRelease {
        !proven
    } else {
        proven
    })
}

/// The Flat bound is distinct from compatibility of fields stored in a flat
/// declaration: String/Bytes are compatible fields, but ordinary values own blocks.
pub fn structural_flat(
    source: &TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    Ok(prove(
        source,
        root,
        Bound::Flat,
        generics,
        true,
        true,
        false,
        false,
        lookup,
    )? == Sendability::Sendable)
}

/// MEM-11, CONC-2: a copy of a value whose only counted parts are `Shared`,
/// `Mutex` and `Atomic` handles shares no block of its source. Each handle copy
/// acquires its own count on an immutable root or a run-owned cell, and the rest
/// is plain bits, so the copy is global wherever its source was read. Handle
/// contents are never traversed. Plain values qualify.
pub fn copy_detaches(
    source: &TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    Ok(prove(
        source,
        root,
        Bound::Plain,
        generics,
        true,
        true,
        false,
        true,
        lookup,
    )? == Sendability::Sendable)
}

/// Symbolic sendability is a conjunction of generic obligations. A concrete
/// Sendable bound succeeds only for Sendable, never merely DependsOn.
pub fn structural_sendability(
    source: &TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Sendability, QueryError> {
    // A dependent relation remains a promise until its source is instantiated.
    // Callable/Iter sendability describes captures, apart from the root view.
    // MODE-4: a callable's crossing mode is evidence about what it captures,
    // "computed where it is built", so its own locality (MODE-1's) does not
    // erase it: a borrowed `sendable fn` still holds only what may cross.
    let check_locality = !source.get(root).is_some_and(|t| {
        matches!(
            t.kind,
            TypeKind::Function(_)
                | TypeKind::Builtin {
                    kind: BuiltinType::Iter,
                    ..
                }
        )
    });
    prove(
        source,
        root,
        Bound::Sendable,
        generics,
        check_locality,
        false,
        false,
        false,
        lookup,
    )
}
/// Crossing evidence for the current referent. Its root access lifetime is
/// checked separately; retained part and placement bounds remain obligations.
pub fn content_sendability(
    source: &TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Sendability, QueryError> {
    prove(
        source,
        root,
        Bound::Sendable,
        generics,
        false,
        true,
        false,
        false,
        lookup,
    )
}

/// Recompute crossing after a real counted share, including all owned parts.
/// Written deep uniqueness and prior occurrence proof cannot survive that share.
/// Flat/Plain copies and immutable Shared boundaries retain their normal rules.
pub fn sendability_after_sharing(
    source: &TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Sendability, QueryError> {
    prove(
        source,
        root,
        Bound::Sendable,
        generics,
        false,
        false,
        true,
        false,
        lookup,
    )
}

/// The receiving crossing promise, excluding transient data summaries.
pub fn crossing_requirement(row: &Type) -> Sendability {
    if row.sendability_requirement == SendabilityRequirement::Sendable {
        Sendability::Sendable
    } else if matches!(
        row.kind,
        TypeKind::Function(_)
            | TypeKind::Builtin {
                kind: BuiltinType::Iter,
                ..
            }
    ) {
        row.modes.sendability.clone()
    } else {
        Sendability::NotSendable
    }
}

fn implies(bounds: &[Bound], bound: Bound) -> bool {
    if bound == Bound::Flat {
        return bounds.contains(&Bound::Flat);
    }
    bounds.contains(&bound)
        || bounds.contains(&Bound::Plain)
        || (bound == Bound::Copyable
            && bounds
                .iter()
                .any(|b| matches!(b, Bound::Data | Bound::Key | Bound::Flat)))
        || (bound == Bound::Sendable && bounds.contains(&Bound::Flat))
}
fn prove(
    source: &TypeStore,
    root: TypeId,
    bound: Bound,
    generics: &GenericDeclarations,
    check_root_locality: bool,
    use_evidence: bool,
    shared_storage: bool,
    handles: bool,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Sendability, QueryError> {
    let mut store = crate::graph::RootedScratch::new(source);
    let mut pending = vec![(root, false, false, false)];
    let mut seen = BTreeSet::new();
    let mut nominal_roots = NominalExpansions::default();
    let mut dependencies = BTreeSet::new();
    while let Some((id, inherited_unique, mut plain_known, exit)) = pending.pop() {
        if exit {
            nominal_roots.leave(id, usize::from(inherited_unique));
            continue;
        }
        if !seen.insert((id, inherited_unique)) {
            continue;
        }
        let Some(row) = store.get(id).cloned() else {
            return Ok(Sendability::NotSendable);
        };
        let unique =
            !shared_storage && (inherited_unique || row.modes.uniqueness == Uniqueness::Unique);
        if (bound == Bound::Copyable && row.modes.multiplicity == Multiplicity::Once)
            || (bound == Bound::Sendable
                && !row.modes.relation.is_global()
                && (id != root || check_root_locality))
        {
            return Ok(Sendability::NotSendable);
        }
        // MEM-12: an aggregate that does not opt in, inline arrays included,
        // is flat only through a plain, pointer-free representation.
        if bound == Bound::Flat
            && !plain_known
            && matches!(
                row.kind,
                TypeKind::Tuple(_)
                    | TypeKind::Record(_)
                    | TypeKind::Variant(_)
                    | TypeKind::Builtin {
                        kind: BuiltinType::Inline | BuiltinType::Simd,
                        ..
                    }
            )
        {
            if prove(
                store.store(id),
                id,
                Bound::Plain,
                generics,
                true,
                true,
                false,
                false,
                lookup,
            )? != Sendability::Sendable
            {
                return Ok(Sendability::NotSendable);
            }
            plain_known = true;
        }
        // Current checked occurrence evidence is usable by consumers. Ordinary
        // structural recomputation deliberately does not preserve a data summary.
        if bound == Bound::Sendable
            && use_evidence
            && !shared_storage
            && row.modes.sendability == Sendability::Sendable
        {
            continue;
        }
        let mut children = vec![];
        match &row.kind {
            TypeKind::Error => return Ok(Sendability::NotSendable),
            TypeKind::Primitive(Primitive::String | Primitive::Bytes) => {
                if matches!(bound, Bound::Plain | Bound::Flat)
                    || (bound == Bound::Sendable && !unique)
                {
                    return Ok(Sendability::NotSendable);
                }
            }
            TypeKind::Primitive(_) => (),
            TypeKind::Tuple(ts) => children.extend(ts),
            TypeKind::Record(fs) => children.extend(fs.iter().map(|f| &f.ty)),
            TypeKind::Variant(vs) => {
                children.extend(vs.iter().flat_map(|v| v.fields.iter().map(|f| &f.1)))
            }
            TypeKind::Box(child) => {
                if matches!(bound, Bound::Plain | Bound::Flat)
                    || (bound == Bound::Sendable && !unique)
                {
                    return Ok(Sendability::NotSendable);
                }
                children.push(child);
            }
            TypeKind::Function(_) => {
                if matches!(bound, Bound::Plain | Bound::Flat) {
                    return Ok(Sendability::NotSendable);
                }
                if bound == Bound::Sendable {
                    match &row.modes.sendability {
                        Sendability::NotSendable => return Ok(Sendability::NotSendable),
                        Sendability::DependsOn(ps) => dependencies.extend(ps),
                        Sendability::Sendable => (),
                    }
                }
            }
            TypeKind::Parameter(p) => {
                let proven = matches!(generics.get(*p), Some(GenericParameter::Type { bounds, .. }) if implies(bounds, bound)
                    && (!shared_storage || bound != Bound::Sendable || implies(bounds, Bound::Plain) || implies(bounds, Bound::Flat)));
                if !proven {
                    if bound == Bound::Sendable && !shared_storage {
                        dependencies.insert(*p);
                    } else {
                        return Ok(Sendability::NotSendable);
                    }
                }
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                let Some(signature) = lookup(definition)? else {
                    return Ok(Sendability::NotSendable);
                };
                let SignatureKind::Type(declaration) = &signature.kind else {
                    return Ok(Sendability::NotSendable);
                };
                if bound == Bound::Flat && declaration.capability.is_some() {
                    return Ok(Sendability::NotSendable);
                }
                if bound == Bound::Flat {
                    if declaration.resource {
                        return Ok(Sendability::NotSendable);
                    }
                    if declaration.layout.flat || declaration.bounds.contains(&Bound::Flat) {
                        continue;
                    }
                    // Plain excludes owned blocks, but can contain raw pointers.
                    // Non-alias nominal values also need the structural Flat proof
                    // of their actual representation below.
                    if !signature.metadata.alias && !plain_known {
                        if prove(
                            store.store(id),
                            id,
                            Bound::Plain,
                            generics,
                            true,
                            true,
                            false,
                            false,
                            lookup,
                        )? != Sendability::Sendable
                        {
                            return Ok(Sendability::NotSendable);
                        }
                        // One Plain proof includes every owned child. Do not
                        // re-prove each suffix of a finite deep Flat layout.
                        plain_known = true;
                    }
                }
                if row.is_heap_target(&signature.metadata.roles) {
                    continue;
                }
                if declaration.capability == Some(CapabilityKind::Scoped)
                    || (declaration.resource && bound != Bound::Sendable)
                    || (declaration.layout.flat && bound == Bound::Plain)
                {
                    return Ok(Sendability::NotSendable);
                }
                if implies(&declaration.bounds, bound)
                    && (!shared_storage
                        || bound != Bound::Sendable
                        || implies(&declaration.bounds, Bound::Plain)
                        || implies(&declaration.bounds, Bound::Flat))
                    || (declaration.layout.flat && bound == Bound::Sendable)
                {
                    continue;
                }
                if signature.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque {
                    return Ok(Sendability::NotSendable);
                }
                if !nominal_roots.enter(id, usize::from(unique), |id| store.store(id))? {
                    continue;
                }
                pending.push((id, unique, plain_known, true));
                let Some(representation) = declaration.representation else {
                    return Ok(Sendability::NotSendable);
                };
                let representation = store.expand(
                    &signature.types,
                    representation,
                    arguments,
                    &signature.metadata.roles,
                );
                let Ok(representation) = representation else {
                    return Ok(Sendability::NotSendable);
                };
                pending.push((representation, unique, plain_known, false));
            }
            TypeKind::Builtin { kind, arguments } => {
                if bound == Bound::Flat {
                    if !matches!(
                        kind,
                        BuiltinType::Inline | BuiltinType::Simd | BuiltinType::Packed
                    ) {
                        return Ok(Sendability::NotSendable);
                    }
                }
                if handles
                    && matches!(
                        kind,
                        BuiltinType::Shared | BuiltinType::Mutex | BuiltinType::Atomic
                    )
                {
                    continue;
                }
                if matches!(
                    kind,
                    BuiltinType::SlotId
                        | BuiltinType::TypeShape
                        | BuiltinType::Ptr
                        | BuiltinType::FnPtr
                ) {
                    // GEN-1 pointer bits are Plain; MODE-4 makes Plain values
                    // sendable. Phantom pointees are not retained payloads.
                    continue;
                }
                match bound {
                    Bound::Plain
                        if !matches!(
                            kind,
                            BuiltinType::Inline | BuiltinType::Simd | BuiltinType::Packed
                        ) =>
                    {
                        return Ok(Sendability::NotSendable);
                    }
                    Bound::Copyable => {
                        if matches!(
                            kind,
                            BuiltinType::Task
                                | BuiltinType::Sender
                                | BuiltinType::Receiver
                                | BuiltinType::Region
                                | BuiltinType::Iter
                        ) {
                            return Ok(Sendability::NotSendable);
                        }
                        // Copying a handle never copies its contents.
                        if matches!(
                            kind,
                            BuiltinType::Shared | BuiltinType::Mutex | BuiltinType::Atomic
                        ) {
                            continue;
                        }
                    }
                    Bound::Sendable => match kind {
                        BuiltinType::Shared | BuiltinType::Sender | BuiltinType::Mutex => continue,
                        BuiltinType::Receiver | BuiltinType::Task => {
                            return Ok(Sendability::NotSendable);
                        }
                        BuiltinType::Iter => {
                            match &row.modes.sendability {
                                Sendability::Sendable => (),
                                Sendability::DependsOn(ps) => dependencies.extend(ps),
                                Sendability::NotSendable => return Ok(Sendability::NotSendable),
                            }
                            continue;
                        }
                        BuiltinType::Region
                        | BuiltinType::Inline
                        | BuiltinType::Simd
                        | BuiltinType::Packed
                        | BuiltinType::Atomic => (),
                        _ if !unique => return Ok(Sendability::NotSendable),
                        _ => (),
                    },
                    _ => (),
                }
                children.extend(arguments.iter().filter_map(|a| match a {
                    GenericArgument::Type(t) => Some(t),
                    _ => None,
                }));
            }
        }
        // Check declaration-order counterexamples before a later expanding
        // recursive field can spend the operational proof budget.
        pending.extend(
            children
                .into_iter()
                .rev()
                .map(|t| (*t, unique, plain_known, false)),
        );
    }
    Ok(if dependencies.is_empty() {
        Sendability::Sendable
    } else {
        Sendability::DependsOn(dependencies.into_iter().collect())
    })
}

impl Type {
    /// MODE-1, MODE-2 and PROV-4 describe counted blocks. A plain value owns
    /// none, so its rows carry one spelling of those facts: a written
    /// `unique` or `in S` on plain bits is accepted and inert.
    pub fn canonicalize_plain(&mut self) {
        self.modes.relation = Relation::global();
        self.modes.uniqueness = Uniqueness::Aliased;
        self.modes.sendability = Sendability::Sendable;
        self.sendability_requirement = SendabilityRequirement::None;
    }
    /// MODE-2 / spec #69: Shared qualifiers do not promise a sole atomic root
    /// holder. The immutable boundary has one aliased uniqueness spelling.
    pub fn canonicalize_shared_uniqueness(&mut self) -> bool {
        if matches!(
            self.kind,
            TypeKind::Builtin {
                kind: BuiltinType::Shared,
                ..
            }
        ) && self.modes.uniqueness != Uniqueness::Aliased
        {
            self.modes.uniqueness = Uniqueness::Aliased;
            true
        } else {
            false
        }
    }
    fn is_canonical_plain(&self) -> bool {
        self.modes.relation.is_global()
            && self.modes.uniqueness == Uniqueness::Aliased
            && self.modes.sendability == Sendability::Sendable
    }
}

/// Rebuild a rooted immutable graph with its derived value-property summaries.
/// Structurally plain rows take their canonical modes here;
/// Shared handles have one inert, aliased spelling of root uniqueness.
/// A nested generic function owns its generic bounds. Function and Iter capture
/// summaries remain producer facts; nominal/aggregate facts come from declarations.
pub fn normalize_type_properties(
    store: &mut TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    normalize_graph(
        store,
        root,
        Some(generics),
        Normalization::Properties,
        lookup,
    )
}

/// Canonical inert qualifiers, for a graph whose derived summaries are
/// already final or whose generic scope the caller cannot name.
pub fn canonical_plain_type(
    store: &mut TypeStore,
    root: TypeId,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    normalize_graph(store, root, None, Normalization::Plain, lookup)
}

/// Canonical inert qualifiers using the caller's exact generic declarations.
/// Imported callable fields may substitute a nominal Plain actual or a
/// parameter whose Plain proof belongs to an enclosing lexical binder.
pub fn canonical_plain_type_in(
    store: &mut TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    normalize_graph(store, root, Some(generics), Normalization::Plain, lookup)
}

/// Canonicalize flow-checked current rows without replacing their evidence
/// with a structural summary. Generic Plain assumptions still canonicalize.
pub fn canonical_current_type(
    store: &mut TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    normalize_graph(store, root, Some(generics), Normalization::Current, lookup)
}

/// Resolve cleanup conjunctions from the bounds of each exact declaring binder.
/// Traverses nested callable contracts without changing any other evidence.
pub fn instantiate_cleanup_bounds(
    store: &mut TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
) -> Result<TypeId, QueryError> {
    normalize_graph(
        store,
        root,
        Some(generics),
        Normalization::Cleanup,
        &mut |_| Ok(None),
    )
}

#[derive(Clone, Copy)]
enum Normalization {
    Plain,
    Properties,
    Current,
    Cleanup,
}

/// `summaries` carries the generic scope when derived summaries are rebuilt.
fn normalize_graph(
    store: &mut TypeStore,
    root: TypeId,
    summaries: Option<&GenericDeclarations>,
    mode: Normalization,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    let unscoped = GenericDeclarations::default();
    let generics = summaries.unwrap_or(&unscoped);
    fn visit(
        store: &mut TypeStore,
        id: TypeId,
        generics: &GenericDeclarations,
        mode: Normalization,
        root: TypeId,
        contract: bool,
        binders: &[TypeId],
        memo: &mut std::collections::BTreeMap<(TypeId, Vec<TypeId>), TypeId>,
        active: &mut std::collections::BTreeSet<TypeId>,
        lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
    ) -> Result<TypeId, QueryError> {
        if let Some(id) = memo.get(&(id, binders.to_vec())) {
            return Ok(*id);
        }
        let Some(mut row) = store.get(id).cloned() else {
            return Ok(id);
        };
        // Recovery graphs are validated by the caller, not repaired here.
        if !active.insert(id) {
            return Ok(id);
        }
        let (child_generics, child_binders) = match &row.kind {
            TypeKind::Function(f) => {
                let mut nested = binders.to_vec();
                nested.push(id);
                (generics.nested(&f.generics), nested)
            }
            _ => (generics.clone(), binders.to_vec()),
        };
        let child_contract = contract || matches!(row.kind, TypeKind::Function(_));
        let mut edge = |t: &mut TypeId| -> Result<(), QueryError> {
            *t = visit(
                store,
                *t,
                &child_generics,
                mode,
                root,
                child_contract,
                &child_binders,
                memo,
                active,
                lookup,
            )?;
            Ok(())
        };
        match &mut row.kind {
            TypeKind::Tuple(ts) => {
                for t in ts {
                    edge(t)?;
                }
            }
            TypeKind::Record(fs) => {
                for f in fs {
                    edge(&mut f.ty)?;
                }
            }
            TypeKind::Variant(vs) => {
                for v in vs {
                    for (_, t) in &mut v.fields {
                        edge(t)?;
                    }
                }
            }
            TypeKind::Box(t) => edge(t)?,
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                for a in arguments {
                    if let GenericArgument::Type(t) = a {
                        edge(t)?;
                    }
                }
            }
            TypeKind::Function(f) => {
                for p in &mut f.parameters {
                    edge(&mut p.ty)?;
                }
                edge(&mut f.result)?;
                if let Some(t) = &mut f.error {
                    edge(t)?;
                }
            }
            _ => (),
        }
        let mut current = if store.get(id) == Some(&row) {
            id
        } else {
            store.push(row.clone())
        };
        if matches!(mode, Normalization::Cleanup) {
            if matches!(&row.drop_allocation, DropAllocation::DependsOn(parameters)
                if parameters.iter().all(|parameter| matches!(generics.get(*parameter),
                    Some(GenericParameter::Type { bounds, .. })
                        if bounds.iter().any(|bound| matches!(bound, Bound::NoallocDrop | Bound::Plain)))))
            {
                row.drop_allocation = DropAllocation::NoAlloc;
                current = store.push(row);
            }
            active.remove(&id);
            memo.insert((id, binders.to_vec()), current);
            return Ok(current);
        }
        // A type parameter's plainness is a bound of its declaring binder,
        // which every reference names: the proof reads
        // the same declarations wherever the row travels. Without a scope
        // only a structural proof is canonical.
        if (!row.is_canonical_plain()
            || row.sendability_requirement != SendabilityRequirement::None)
            && structural_property(store, current, StructuralProperty::Plain, generics, lookup)?
        {
            row.canonicalize_plain();
            current = store.push(row.clone());
        }
        if row.canonicalize_shared_uniqueness() {
            current = store.push(row.clone());
        }
        if matches!(mode, Normalization::Properties | Normalization::Current) {
            let structural = structural_sendability(store, current, generics, lookup)?;
            // Only this API consumes replayed current roots/checked part proofs.
            // Fixed receiving syntax alone never supplies evidence.
            let retained = matches!(mode, Normalization::Current);
            let sendability =
                if contract && row.sendability_requirement == SendabilityRequirement::Sendable {
                    // Invocation headers describe future checked receipts, not
                    // owned payload. Their declared refinements are entry/result
                    // assumptions; actual call operands are checked separately.
                    Sendability::Sendable
                } else if retained {
                    row.modes.sendability.clone()
                } else {
                    structural
                };
            if row.modes.sendability != sendability {
                row.modes.sendability = sendability;
                current = store.push(row);
            }
        }
        active.remove(&id);
        memo.insert((id, binders.to_vec()), current);
        Ok(current)
    }
    visit(
        store,
        root,
        generics,
        mode,
        root,
        false,
        &[],
        &mut Default::default(),
        &mut Default::default(),
        lookup,
    )
}

/// CLO-2/4 and CAP-2/3: a value with less held authority satisfies a broader
/// callable promise. Uses restrictions may narrow, never widen. This is the one
/// callable subsumption: the root of a received value and every nested row in
/// a reading position (`graph::Variance`) are judged by it. Structural
/// identity is still decided once by shared rooted equality after this proof.
fn function_value_promises(expected: &Type, actual: &mut Type) -> bool {
    let (TypeKind::Function(expected), TypeKind::Function(actual)) =
        (&expected.kind, &mut actual.kind)
    else {
        return true;
    };
    if !state_promises_satisfy(&actual.state_promises, &expected.state_promises)
        || (expected.allocation == Allocation::NoAlloc && actual.allocation != Allocation::NoAlloc)
        || !actual.authority.is_subset_of(&expected.authority)
        || !match (&actual.uses, &expected.uses) {
            (_, Uses::Unrestricted) => true,
            (Uses::Only(actual), Uses::Only(expected)) => actual.is_subset_of(expected),
            _ => false,
        }
    {
        return false;
    }
    actual.authority = expected.authority.clone();
    actual.uses = expected.uses.clone();
    // CLO-2/MEM-5: noalloc is the stronger callable allocation promise.
    actual.allocation = expected.allocation;
    actual.state_promises = expected.state_promises.clone();
    true
}

/// MEM-9/CLO-4: an erased state (an `Iter`) whose captures hold no more
/// authority than the expected row declares is received there, and unknown
/// captures admit any. A dependent capture promise also controls
/// sendability: merely holding fewer capabilities cannot establish that the
/// state came from its selected parameters (it might contain newly allocated
/// non-sendable values), so only the same promise keeps it.
fn erased_state_promises(expected: &Type, actual: &mut Type) -> bool {
    if matches!(
        expected.erased_captures,
        Some(ErasedCaptures::FromArguments(_))
    ) && expected.erased_captures != actual.erased_captures
    {
        return false;
    }
    let authority = |row: &Type| {
        row.erased_captures
            .as_ref()
            .map(ErasedCaptures::authority)
            .unwrap_or(AuthoritySummary::Unknown)
    };
    if !authority(actual).is_subset_of(&authority(expected)) {
        return false;
    }
    actual.erased_captures = expected.erased_captures.clone();
    true
}

/// MEM-5: cleanup that may allocate is the weakest cleanup summary, so a
/// value whose destruction provably does not allocate is received where
/// allocating cleanup is permitted. Any narrower summary is kept exactly.
fn cleanup_promises(expected: &Type, actual: &mut Type) -> bool {
    if expected.drop_allocation != DropAllocation::MayAllocate
        && expected.drop_allocation != actual.drop_allocation
    {
        return false;
    }
    actual.drop_allocation = expected.drop_allocation.clone();
    true
}

/// The promises a value's row makes where `expected` stands: its callable
/// promise, its erased state's captures and its cleanup summary. Each may be
/// stronger than the place requires.
fn value_promises(expected: &Type, actual: &mut Type) -> bool {
    function_value_promises(expected, actual)
        && erased_state_promises(expected, actual)
        && cleanup_promises(expected, actual)
}

fn receives_crossing(expected: &Sendability, actual: &Sendability) -> bool {
    match (expected, actual) {
        (Sendability::NotSendable, _) | (_, Sendability::Sendable) => true,
        (Sendability::DependsOn(expected), Sendability::DependsOn(actual)) => {
            actual.iter().all(|p| expected.contains(p))
        }
        _ => false,
    }
}

/// The promise of a nested row received where `expected` stands, read in the
/// direction the caller chose. A callable promises as its root would, and a
/// row that is sendable is received where sendability is not promised, since
/// an aggregate's summary follows its parts (MODE-4). Every other fact of a
/// nested row is identity and stays exact.
pub(crate) fn nested_promise(expected: &Type, actual: &mut Type) -> bool {
    if !value_promises(expected, actual) {
        return false;
    }
    if !receives_crossing(&crossing_requirement(expected), &actual.modes.sendability) {
        return false;
    }
    actual.sendability_requirement = expected.sendability_requirement;
    actual.modes.sendability = expected.modes.sendability.clone();
    true
}

/// Root value weakening is distinct from exact nested type identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeRelation {
    Exact,
    Value,
    Parameter,
}

pub fn compatible_type(
    source: &TypeStore,
    expected: TypeId,
    actual: TypeId,
    relation: TypeRelation,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    compatible(
        source, expected, actual, relation, false, None, true, generics, lookup,
    )
}

/// The relation for a `mut` argument. The callee writes the caller's storage
/// back through its parameter, so a callable, an erased state or a cleanup
/// summary can promise neither more nor less than the parameter declares, at
/// the root or below it: every nested contract is exact.
pub fn compatible_handback_type(
    source: &TypeStore,
    expected: TypeId,
    actual: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    compatible(
        source,
        expected,
        actual,
        TypeRelation::Parameter,
        false,
        None,
        false,
        generics,
        lookup,
    )
}

/// The parameter relation for an argument the callee cannot write back: a
/// borrow or a take. Brands are identity evidence, so a data value may be
/// handed to a parameter that names none of them at any depth; the callee
/// simply does not know the brand. A parameter that declares a brand still
/// requires it. Only Data can forget nested brands. MODE-2 also permits nested
/// uniqueness weakening along owned payload edges; callable headers, phantom
/// arguments and interior-mutable contracts remain invariant.
pub fn compatible_view_type(
    source: &TypeStore,
    expected: TypeId,
    actual: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    compatible_view_type_in(
        source,
        expected,
        actual,
        TypeRelation::Parameter,
        generics,
        lookup,
    )
}

/// The same non-writing view relation with an explicit root reception policy.
/// Owned receivers use Value so an arena relation cannot disappear into storage;
/// readonly arguments use Parameter. Mut hand-back keeps compatible_type.
pub fn compatible_view_type_in(
    source: &TypeStore,
    expected: TypeId,
    actual: TypeId,
    relation: TypeRelation,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    if relation == TypeRelation::Exact {
        return compatible_type(source, expected, actual, relation, generics, lookup);
    }
    let data = structural_equality(source, actual, Bound::Data, generics, lookup)?;
    // Only owned payload edges are covariant. A callable's header, phantom
    // argument or interior-mutable cell remains the same contract even when
    // the containing value is taken. Nominal Data excludes hidden callbacks.
    let mut payloads = std::collections::BTreeSet::new();
    let mut pending = vec![actual];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(row) = source.get(id) else { continue };
        let owned = match &row.kind {
            TypeKind::Tuple(_) | TypeKind::Box(_) | TypeKind::Record(_) | TypeKind::Variant(_) => {
                true
            }
            TypeKind::Builtin { kind, .. } => matches!(
                kind,
                BuiltinType::Array
                    | BuiltinType::List
                    | BuiltinType::Map
                    | BuiltinType::Set
                    | BuiltinType::Inline
                    | BuiltinType::Simd
                    | BuiltinType::Packed
                    | BuiltinType::Shared
                    | BuiltinType::Slots
            ),
            TypeKind::Named { .. } => {
                structural_equality(source, id, Bound::Data, generics, lookup)?
            }
            _ => false,
        };
        if owned {
            payloads.insert(id);
            pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
        }
    }
    compatible(
        source,
        expected,
        actual,
        relation,
        data,
        Some(&payloads),
        true,
        generics,
        lookup,
    )
}

fn compatible(
    source: &TypeStore,
    expected: TypeId,
    actual: TypeId,
    relation: TypeRelation,
    forget_brands: bool,
    payloads: Option<&std::collections::BTreeSet<TypeId>>,
    // Nested rows may promise more in reading positions; false for hand-back.
    received: bool,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    let store = source;
    if relation == TypeRelation::Exact {
        return Ok(semantic_equal(&store, expected, &store, actual).unwrap_or(false));
    }
    let Some(expected_row) = store.get(expected).cloned() else {
        return Ok(false);
    };
    let Some(mut actual_row) = store.get(actual).cloned() else {
        return Ok(false);
    };
    if expected_row.allocation_target.is_some()
        && expected_row.allocation_target != actual_row.allocation_target
    {
        return Ok(false);
    }
    actual_row.allocation_target = expected_row.allocation_target;
    let plain = structural_property(&store, actual, StructuralProperty::Plain, generics, lookup)?;
    // A `mut` hand-back is written back through the parameter, so storage
    // that promised less than the parameter admits would return holding it:
    // the callable promise, the erased state's captures and the cleanup
    // summary are exact at the root as they are below it. A core operation
    // that only advances an erased state declares its parameter at the
    // argument's own captures, so it meets this rule by construction.
    let handed_back = received
        || (value_promises(&actual_row, &mut expected_row.clone())
            && actual_row.modes.multiplicity == expected_row.modes.multiplicity
            && (plain || crossing_requirement(&actual_row) == crossing_requirement(&expected_row)));
    let accepts = handed_back
        && value_promises(&expected_row, &mut actual_row)
        && receives_relation(
            &expected_row.modes.relation,
            &actual_row.modes.relation,
            relation,
        )
        && (expected_row.modes.uniqueness == Uniqueness::Aliased
            || actual_row.modes.uniqueness == Uniqueness::Unique
            || plain)
        && (expected_row.modes.multiplicity == Multiplicity::Once
            || actual_row.modes.multiplicity == Multiplicity::Many)
        && receives_crossing(
            &crossing_requirement(&expected_row),
            &actual_row.modes.sendability,
        );
    if !accepts {
        return Ok(false);
    }
    // Root brands may forget identity or join known alternatives. Empty actual
    // evidence cannot manufacture a branded origin; nested brands stay invariant.
    if !expected_row.brand.alternatives.is_empty()
        && (actual_row.brand.alternatives.is_empty()
            || !actual_row
                .brand
                .alternatives
                .iter()
                .all(|o| expected_row.brand.alternatives.contains(o)))
    {
        return Ok(false);
    }
    actual_row.brand = expected_row.brand.clone();
    actual_row.sendability_requirement = expected_row.sendability_requirement;
    actual_row.modes = expected_row.modes;
    Ok(crate::graph::semantic_equal_adjusted(
        store,
        expected,
        None,
        store,
        actual,
        Some(&actual_row),
        forget_brands,
        payloads,
        received,
    )
    .unwrap_or(false))
}

/// MODE-1: where `expected` is a written or declared relation, the actual's
/// relation must be within it. An environment-local expectation is a body's
/// frame-local slot, which receives whatever its current content holds. A
/// relation naming a call's own parameters (a bound reference) is a dependent
/// promise that only the same relation keeps until the call substitutes it.
/// An unrelated borrowed parameter accepts its actual relation.
pub(crate) fn receives_relation(
    expected: &Relation,
    actual: &Relation,
    relation: TypeRelation,
) -> bool {
    let dependent = expected.reaches().iter().any(|r| {
        matches!(
            r,
            Reach::Place(Origin {
                root: OriginRoot::Parameter(ParameterRef::Bound { .. }),
                ..
            })
        )
    });
    expected.has_environment()
        || actual == expected
        || (!dependent && actual.contained_in(expected))
        || (relation == TypeRelation::Parameter && expected.is_global())
}

/// Replacement of caller-owned storage must supply a global value. The Local
/// row of a mut parameter or its projected part describes read access (OWN-8),
/// not permission to retain a view after the call (MODE-1, OWN-3).
pub fn compatible_storage_type(
    source: &TypeStore,
    expected: TypeId,
    actual: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<bool, QueryError> {
    if !source
        .get(actual)
        .is_some_and(|row| row.modes.relation.is_global())
    {
        return Ok(false);
    }
    compatible_type(
        source,
        expected,
        actual,
        TypeRelation::Value,
        generics,
        lookup,
    )
}

/// Derive owned storage content from its declared access type (OWN-3/OWN-8).
/// The borrowed root locality does not describe the displaced value's lifetime.
pub fn storage_content_type(
    store: &mut TypeStore,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    let Some(mut row) = store.get(root).cloned() else {
        return Ok(root);
    };
    if !row.modes.relation.has_environment() {
        return Ok(root);
    }
    row.modes.relation = row.modes.relation.retain(|r| *r != Reach::Environment);
    let root = store.push(row);
    normalize_type_properties(store, root, generics, lookup)
}

/// OWN-8: nonplain parts are bounded by their owner's relation. Recompute only the
/// derived root summary; declaration-owned nested contracts remain invariant.
pub fn projected_type(
    store: &mut TypeStore,
    owner: TypeId,
    part: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    let Some(mut row) = store.get(part).cloned() else {
        return Ok(part);
    };
    // A part read through its owner is bounded by both relations.
    let Some(relation) = store
        .get(owner)
        .map(|owner| row.modes.relation.union(&owner.modes.relation))
    else {
        return Ok(part);
    };
    if relation != row.modes.relation
        && !structural_property(store, part, StructuralProperty::Plain, generics, lookup)?
    {
        row.modes.relation = relation;
        // Projection introduces an access lifetime, not a counted share.
        // The operand's use and ownership flow decide any later invalidation.
        return Ok(store.push(row));
    }
    Ok(part)
}

/// PROV-4: unrelated block fields inherit their aggregate's relation, so fresh
/// blocks go where the aggregate's go. Plain bits have no blocks.
/// This helper supplies the construction contract; read localization remains
/// the separate OWN-8 operation in projected_type.
pub fn placed_part_type(
    store: &mut TypeStore,
    owner: TypeId,
    part: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(&hirsel_syntax::DefPath) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    let Some(mut row) = store.get(part).cloned() else {
        return Ok(part);
    };
    let Some(relation) = store.get(owner).map(|t| t.modes.relation.clone()) else {
        return Ok(part);
    };
    if row.modes.relation.is_global()
        && !relation.is_global()
        && !structural_property(store, part, StructuralProperty::Plain, generics, lookup)?
    {
        row.modes.relation = relation;
        let related = store.push(row.clone());
        row.modes.sendability = structural_sendability(store, related, generics, lookup)?;
        return Ok(store.push(row));
    }
    Ok(part)
}

#[cfg(test)]
mod flat_tests {
    use super::*;
    use hirsel_syntax::{ContentHash, DefPath, ItemPath, PackageId};
    fn row(kind: TypeKind) -> Type {
        Type::with_kind(kind)
    }
    #[test]
    fn shared_handle_canonicalization_preserves_other_contracts() {
        let mut store = TypeStore::default();
        let payload = store.push(row(TypeKind::Primitive(Primitive::Bytes)));
        let mut shared = row(TypeKind::Builtin {
            kind: BuiltinType::Shared,
            arguments: vec![GenericArgument::Type(payload)].into(),
        });
        shared.modes.uniqueness = Uniqueness::Unique;
        shared.modes.relation = Relation::environment();
        let mut expected = shared.clone();
        expected.modes.uniqueness = Uniqueness::Aliased;
        assert!(shared.canonicalize_shared_uniqueness());
        assert_eq!(shared, expected);
        assert!(!shared.canonicalize_shared_uniqueness());
        let mut counted = row(TypeKind::Box(payload));
        counted.modes.uniqueness = Uniqueness::Unique;
        assert!(!counted.canonicalize_shared_uniqueness());
        assert_eq!(counted.modes.uniqueness, Uniqueness::Unique);
    }

    #[test]
    fn flat_roots_use_scoped_bounds_and_independent_declared_snapshots() {
        let path = DefPath(ItemPath {
            package: PackageId {
                name: "fixture".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: "FlatText".into(),
        });
        let mut declared = TypeStore::default();
        let bytes = declared.push(row(TypeKind::Primitive(Primitive::Bytes)));
        let declaration = std::sync::Arc::new(Signature {
            definition: path.clone(),
            types: declared,
            metadata: SignatureMetadata::default(),
            fingerprint: ContentHash::default(),
            kind: SignatureKind::Type(TypeDefinition {
                loop_source: None,
                format: None,
                destructor_allocation: Allocation::MayAllocate,
                generics: vec![],
                representation: Some(bytes),
                bounds: vec![Bound::Flat],
                authority: AuthoritySummary::inert(),
                capability: None,
                implementation_authority: None,
                serializable: false,
                serializable_check: None,
                layout: Layout {
                    flat: true,
                    ..Layout::default()
                },
                distinct: false,
                resource: false,
                drop: None,
                fact: None,
                captures: None,
            }),
        });
        let mut store = TypeStore::default();
        let flat = store.push(row(TypeKind::Named {
            definition: path,
            arguments: vec![].into(),
        }));
        let scalar = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let bytes = store.push(row(TypeKind::Primitive(Primitive::Bytes)));
        let text = store.push(row(TypeKind::Primitive(Primitive::String)));
        let parameter = store.push(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let tuple = store.push(row(TypeKind::Tuple(vec![flat].into())));
        let generics = [GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Flat].into(),
        }];
        let mut lookup = |_: &DefPath| Ok::<_, QueryError>(Some(declaration.clone()));
        for (root, expected) in [
            (flat, true),
            (scalar, true),
            (bytes, false),
            (text, false),
            (parameter, true),
            (tuple, false),
        ] {
            assert_eq!(
                structural_flat(
                    &store,
                    root,
                    &GenericDeclarations::signature(&generics),
                    &mut lookup
                )
                .unwrap(),
                expected
            );
            let mut imported = TypeStore::default();
            let root = import_type(&store, root, &mut imported).unwrap();
            assert_eq!(
                structural_flat(
                    &imported,
                    root,
                    &GenericDeclarations::signature(&generics),
                    &mut lookup
                )
                .unwrap(),
                expected
            );
        }
        let plain = [GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Plain].into(),
        }];
        assert!(
            !structural_flat(
                &store,
                parameter,
                &GenericDeclarations::signature(&plain),
                &mut lookup
            )
            .unwrap()
        );
        let pointer = store.push(row(TypeKind::Builtin {
            kind: BuiltinType::Ptr,
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }));
        let pointer_record = store.push(row(TypeKind::Record(
            vec![Field {
                name: "p".into(),
                ty: pointer,
                growable: false,
            }]
            .into(),
        )));
        for root in [pointer, pointer_record] {
            assert!(
                !structural_flat(
                    &store,
                    root,
                    &GenericDeclarations::signature(&[]),
                    &mut lookup
                )
                .unwrap()
            );
        }
        let mut opaque_plain = declaration.as_ref().clone();
        opaque_plain.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
        if let SignatureKind::Type(td) = &mut opaque_plain.kind {
            td.layout.flat = false;
            td.bounds = vec![Bound::Plain];
            td.representation = None;
        }
        assert!(
            !structural_flat(
                &store,
                flat,
                &GenericDeclarations::signature(&[]),
                &mut |_: &DefPath| Ok::<_, QueryError>(Some(std::sync::Arc::new(
                    opaque_plain.clone()
                )))
            )
            .unwrap()
        );
        assert!(
            !structural_flat(
                &store,
                parameter,
                &GenericDeclarations::signature(&[]),
                &mut lookup
            )
            .unwrap()
        );
        assert!(
            !structural_flat(
                &store,
                flat,
                &GenericDeclarations::signature(&[]),
                &mut |_| Ok::<_, QueryError>(None)
            )
            .unwrap()
        );
        assert_eq!(
            structural_flat(
                &store,
                flat,
                &GenericDeclarations::signature(&[]),
                &mut |_| Err::<Option<SignatureRef>, _>(QueryError::NotImplemented)
            ),
            Err(QueryError::NotImplemented)
        );
    }

    /// A callable returning a callable; the inner one's sendability and uses
    /// are the nested promises under test.
    fn producer(store: &mut TypeStore, sendability: Sendability, uses: Uses) -> TypeId {
        let function = |result, uses: Uses| FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: vec![].into(),
            result,
            error: None,
            allocation: Allocation::MayAllocate,
            authority: match &uses {
                Uses::Only(authority) => authority.clone(),
                Uses::Unrestricted => AuthoritySummary::Unknown,
            },
            uses,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        };
        let unit = store.push(row(TypeKind::Tuple(vec![].into())));
        let mut inner = row(TypeKind::Function(function(unit, uses)));
        inner.modes.sendability = sendability;
        let inner = store.push(inner);
        store.push(row(TypeKind::Function(function(inner, Uses::Unrestricted))))
    }
    #[test]
    fn a_nested_result_promises_more_only_where_the_value_is_not_handed_back() {
        let generics = GenericDeclarations::signature(&[]);
        let mut lookup = |_: &DefPath| Ok::<Option<SignatureRef>, QueryError>(None);
        let inert = || Uses::Only(AuthoritySummary::inert());
        let mut store = TypeStore::default();
        let plain = producer(&mut store, Sendability::NotSendable, Uses::Unrestricted);
        for stronger in [
            producer(&mut store, Sendability::Sendable, Uses::Unrestricted),
            producer(&mut store, Sendability::NotSendable, inert()),
            producer(&mut store, Sendability::Sendable, inert()),
        ] {
            for relation in [TypeRelation::Value, TypeRelation::Parameter] {
                let mut received = |expected, actual| {
                    [
                        compatible_type(&store, expected, actual, relation, &generics, &mut lookup),
                        compatible_view_type_in(
                            &store,
                            expected,
                            actual,
                            relation,
                            &generics,
                            &mut lookup,
                        ),
                    ]
                    .map(Result::unwrap)
                };
                assert_eq!(received(plain, stronger), [true, true]);
                assert_eq!(received(stronger, plain), [false, false]);
            }
            // CLO-2: a `mut` argument is written back, so neither side may
            // promise more (the callee could store a weaker callable).
            for (expected, actual) in [(plain, stronger), (stronger, plain)] {
                assert!(
                    !compatible_handback_type(&store, expected, actual, &generics, &mut lookup)
                        .unwrap()
                );
                assert!(
                    !compatible_type(
                        &store,
                        expected,
                        actual,
                        TypeRelation::Exact,
                        &generics,
                        &mut lookup
                    )
                    .unwrap()
                );
            }
            assert!(
                compatible_handback_type(&store, stronger, stronger, &generics, &mut lookup)
                    .unwrap()
            );
        }
    }
    /// MEM-5 and the erased-state rule of `iter.from`: an iterator made
    /// where its captures and cleanup are known is received, as a tuple part
    /// or a list element, where the written `Iter[T]` states neither, as it
    /// is at the root. The reverse forges evidence, a hand-back keeps both
    /// exact, and a dependent capture promise is only met by itself.
    #[test]
    fn a_nested_erased_state_is_received_where_its_type_states_less() {
        let generics = GenericDeclarations::signature(&[]);
        let mut lookup = |_: &DefPath| Ok::<Option<SignatureRef>, QueryError>(None);
        let mut store = TypeStore::default();
        let mut item = row(TypeKind::Primitive(Primitive::U64));
        item.drop_allocation = DropAllocation::NoAlloc;
        let item = store.push(item);
        let mut iterator = |captures: Option<ErasedCaptures>, cleanup: DropAllocation| {
            let mut part = row(TypeKind::Builtin {
                kind: BuiltinType::Iter,
                arguments: vec![GenericArgument::Type(item)].into(),
            });
            part.erased_captures = captures;
            part.drop_allocation = cleanup;
            let part = store.push(part);
            let holder = store.push(row(TypeKind::Tuple(vec![part].into())));
            store.push(row(TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(holder)].into(),
            }))
        };
        let known = || Some(ErasedCaptures::Known(AuthoritySummary::inert()));
        let written = iterator(None, DropAllocation::MayAllocate);
        let dependent = iterator(
            Some(ErasedCaptures::FromArguments(
                vec![ParameterRef::bound(ParamId(0)).into()].into(),
            )),
            DropAllocation::MayAllocate,
        );
        for made in [
            iterator(known(), DropAllocation::MayAllocate),
            iterator(None, DropAllocation::NoAlloc),
            iterator(known(), DropAllocation::NoAlloc),
        ] {
            let mut received = |expected, actual| {
                compatible_type(
                    &store,
                    expected,
                    actual,
                    TypeRelation::Value,
                    &generics,
                    &mut lookup,
                )
                .unwrap()
            };
            assert!(received(written, made));
            assert!(!received(made, written));
            assert!(!received(dependent, made));
            for (expected, actual) in [(written, made), (made, written)] {
                assert!(
                    !compatible_handback_type(&store, expected, actual, &generics, &mut lookup)
                        .unwrap()
                );
            }
        }
    }
}

#[cfg(test)]
mod seal_cleanup_tests {
    use super::*;

    #[test]
    fn seal_cleanup_depends_on_public_promises_in_both_views() {
        let mut types = TypeStore::default();
        let root = types.push(Type::unit());
        let mut d = TypeDefinition {
            loop_source: None,
            format: None,
            generics: vec![],
            representation: Some(root),
            bounds: vec![],
            authority: AuthoritySummary::inert(),
            capability: Some(CapabilityKind::Global),
            implementation_authority: Some(AuthoritySummary::inert()),
            serializable: false,
            serializable_check: None,
            layout: Layout::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
            destructor_allocation: Allocation::NoAlloc,
        };
        for bounds in [vec![], vec![Bound::Plain], vec![Bound::NoallocDrop]] {
            d.bounds = bounds;
            let expected = if d.bounds.is_empty() {
                DropAllocation::MayAllocate
            } else {
                DropAllocation::NoAlloc
            };
            for representation in [Some(root), None] {
                d.representation = representation;
                assert_eq!(
                    declared_cleanup(&d, &types, &[], |_| unreachable!()),
                    expected
                );
            }
        }
    }
}
