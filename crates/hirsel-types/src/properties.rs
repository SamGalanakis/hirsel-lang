//! Structural value properties use declaration snapshots, never implementations.
type IdSet<T> = std::collections::HashSet<T, hirsel_intern::IdHasher>;
type IdMap<K, V> = std::collections::HashMap<K, V, hirsel_intern::IdHasher>;
use crate::*;
use std::collections::BTreeSet;

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
            SignatureKind::Family(_) | SignatureKind::Implementation(_) => &[],
        }
    }
}

/// Missing declarations and malformed graphs prove nothing. Lookup errors remain
/// caller-owned, so providers retain their dependency and diagnostic policies.
pub fn structural_property(
    source: &TypeInterner,
    root: TypeId,
    property: StructuralProperty,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
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
    source: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
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
    source: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
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
    source: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
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
    source: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
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
    source: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
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
/// The lookups and nested Plain proofs a proof step performs.
pub(crate) trait ProofContext {
    fn signature(&mut self, definition: DefId) -> Result<Option<SignatureRef>, QueryError>;
    /// `id` is Plain, as `structural_property` proves it.
    fn plain(&mut self, id: TypeId) -> Result<bool, QueryError>;
}

/// The flags of a `structural_property` proof.
pub const STRUCTURAL: ProofFlags = ProofFlags {
    check_root_locality: true,
    use_evidence: true,
    shared_storage: false,
    handles: false,
};

struct Direct<'a, L> {
    t: &'a TypeInterner,
    generics: &'a GenericDeclarations,
    lookup: &'a mut L,
}
impl<L: FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>> ProofContext for Direct<'_, L> {
    fn signature(&mut self, definition: DefId) -> Result<Option<SignatureRef>, QueryError> {
        (self.lookup)(definition)
    }
    fn plain(&mut self, id: TypeId) -> Result<bool, QueryError> {
        Ok(prove_flags(
            self.t,
            id,
            Bound::Plain,
            self.generics,
            STRUCTURAL,
            self.lookup,
        )? == Sendability::Sendable)
    }
}

/// What one row of a proof decides locally.
pub(crate) enum ProofStep {
    /// The row fails the bound: so does the whole proof.
    Fail,
    /// The row holds once each part holds, in declaration order.
    Parts(Vec<TypeId>),
    /// A nominal row whose representation carries the proof, inside an
    /// expansion the driver enters (and may assume coinductively).
    Expand {
        signature: SignatureRef,
        arguments: SharedSlice<GenericArgument>,
    },
}

/// The local obligations of `id` in a proof rooted at `root`: its verdict or
/// the parts carrying the proof on, with the parts' inherited uniqueness and
/// whether their Plain proof is already known (both updated in place).
/// Parameter, callable and iterator dependencies join `dependencies`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn proof_step(
    store: &TypeInterner,
    root: TypeId,
    id: TypeId,
    inherited_unique: bool,
    plain_known: &mut bool,
    bound: Bound,
    generics: &GenericDeclarations,
    flags: ProofFlags,
    dependencies: &mut BTreeSet<TypeParameterRef>,
    context: &mut dyn ProofContext,
) -> Result<(ProofStep, bool), QueryError> {
    let ProofFlags {
        check_root_locality,
        use_evidence,
        shared_storage,
        handles,
    } = flags;
    let Some(row) = store.get(id) else {
        return Ok((ProofStep::Fail, false));
    };
    let unique =
        !shared_storage && (inherited_unique || row.modes.uniqueness == Uniqueness::Unique);
    let fail = Ok((ProofStep::Fail, unique));
    let done = Ok((ProofStep::Parts(vec![]), unique));
    if (bound == Bound::Copyable && row.modes.multiplicity == Multiplicity::Once)
        || (bound == Bound::Sendable
            && !row.modes.relation.is_global()
            && (id != root || check_root_locality))
    {
        return fail;
    }
    // MEM-12: an aggregate that does not opt in, inline arrays included,
    // is flat only through a plain, pointer-free representation.
    if bound == Bound::Flat
        && !*plain_known
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
        if !context.plain(id)? {
            return fail;
        }
        *plain_known = true;
    }
    // Current checked occurrence evidence is usable by consumers. Ordinary
    // structural recomputation deliberately does not preserve a data summary.
    if bound == Bound::Sendable
        && use_evidence
        && !shared_storage
        && row.modes.sendability == Sendability::Sendable
    {
        return done;
    }
    let mut children: Vec<TypeId> = vec![];
    match &row.kind {
        TypeKind::Error => return fail,
        TypeKind::Primitive(Primitive::String | Primitive::Bytes) => {
            if matches!(bound, Bound::Plain | Bound::Flat) || (bound == Bound::Sendable && !unique)
            {
                return fail;
            }
        }
        TypeKind::Primitive(_) => (),
        TypeKind::Tuple(ts) => children.extend(ts.iter()),
        TypeKind::Record(fs) => children.extend(fs.iter().map(|f| f.ty)),
        TypeKind::Variant(vs) => {
            children.extend(vs.iter().flat_map(|v| v.fields.iter().map(|f| f.1)))
        }
        TypeKind::Box(child) => {
            if matches!(bound, Bound::Plain | Bound::Flat) || (bound == Bound::Sendable && !unique)
            {
                return fail;
            }
            children.push(*child);
        }
        TypeKind::Function(_) => {
            if matches!(bound, Bound::Plain | Bound::Flat) {
                return fail;
            }
            if bound == Bound::Sendable {
                match &row.modes.sendability {
                    Sendability::NotSendable => return fail,
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
                    return fail;
                }
            }
        }
        TypeKind::Named {
            definition,
            arguments,
        } => {
            let Some(signature) = context.signature(*definition)? else {
                return fail;
            };
            let SignatureKind::Type(declaration) = &signature.kind else {
                return fail;
            };
            if bound == Bound::Flat && declaration.capability.is_some() {
                return fail;
            }
            if bound == Bound::Flat {
                if declaration.resource {
                    return fail;
                }
                if declaration.layout.flat || declaration.bounds.contains(&Bound::Flat) {
                    return done;
                }
                // Plain excludes owned blocks, but can contain raw pointers.
                // Non-alias nominal values also need the structural Flat proof
                // of their actual representation below.
                if !signature.metadata.alias && !*plain_known {
                    if !context.plain(id)? {
                        return fail;
                    }
                    // One Plain proof includes every owned child. Do not
                    // re-prove each suffix of a finite deep Flat layout.
                    *plain_known = true;
                }
            }
            if row.is_heap_target(&signature.metadata.roles) {
                return done;
            }
            if declaration.capability == Some(CapabilityKind::Scoped)
                || (declaration.resource && bound != Bound::Sendable)
                || (declaration.layout.flat && bound == Bound::Plain)
            {
                return fail;
            }
            if implies(&declaration.bounds, bound)
                && (!shared_storage
                    || bound != Bound::Sendable
                    || implies(&declaration.bounds, Bound::Plain)
                    || implies(&declaration.bounds, Bound::Flat))
                || (declaration.layout.flat && bound == Bound::Sendable)
            {
                return done;
            }
            // MEM-4: only the reviewed core owner exposes this structural
            // crossing rule through its opaque nominal surface. Its payload
            // remains opaque for every other bound and for source field access.
            let sealed_owner = bound == Bound::Sendable
                && signature.definition.path().is_core("Region")
                && declaration.resource
                && declaration.representation.is_some_and(|representation| {
                    matches!(
                        store.get(representation).map(|row| &row.kind),
                        Some(TypeKind::Builtin {
                            kind: BuiltinType::Region,
                            ..
                        })
                    )
                });
            if signature.metadata.visibility == hirsel_syntax::Visibility::PublicOpaque
                && !sealed_owner
            {
                return fail;
            }
            let arguments = arguments.clone();
            return Ok((
                ProofStep::Expand {
                    signature,
                    arguments,
                },
                unique,
            ));
        }
        TypeKind::Builtin { kind, arguments } => {
            if bound == Bound::Flat
                && !matches!(
                    kind,
                    BuiltinType::Inline | BuiltinType::Simd | BuiltinType::Packed
                )
            {
                return fail;
            }
            if handles
                && matches!(
                    kind,
                    BuiltinType::Shared | BuiltinType::Mutex | BuiltinType::Atomic
                )
            {
                return done;
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
                return done;
            }
            match bound {
                Bound::Plain
                    if !matches!(
                        kind,
                        BuiltinType::Inline | BuiltinType::Simd | BuiltinType::Packed
                    ) =>
                {
                    return fail;
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
                        return fail;
                    }
                    // Copying a handle never copies its contents.
                    if matches!(
                        kind,
                        BuiltinType::Shared | BuiltinType::Mutex | BuiltinType::Atomic
                    ) {
                        return done;
                    }
                }
                Bound::Sendable => match kind {
                    BuiltinType::Shared | BuiltinType::Sender | BuiltinType::Mutex => return done,
                    BuiltinType::Receiver | BuiltinType::Task => return fail,
                    BuiltinType::Iter => {
                        match &row.modes.sendability {
                            Sendability::Sendable => (),
                            Sendability::DependsOn(ps) => dependencies.extend(ps),
                            Sendability::NotSendable => return fail,
                        }
                        return done;
                    }
                    // The owner's affinity/outer uniqueness cannot upgrade
                    // the complete root's own modes into deep uniqueness.
                    BuiltinType::Region => {
                        return Ok((
                            ProofStep::Parts(
                                arguments
                                    .iter()
                                    .filter_map(|argument| match argument {
                                        GenericArgument::Type(ty) => Some(*ty),
                                        _ => None,
                                    })
                                    .collect(),
                            ),
                            false,
                        ));
                    }
                    BuiltinType::Inline
                    | BuiltinType::Simd
                    | BuiltinType::Packed
                    | BuiltinType::Atomic => (),
                    _ if !unique => return fail,
                    _ => (),
                },
                _ => (),
            }
            children.extend(arguments.iter().filter_map(|a| match a {
                GenericArgument::Type(t) => Some(*t),
                _ => None,
            }));
        }
    }
    Ok((ProofStep::Parts(children), unique))
}

/// The representation an entered nominal expansion proves, or None when the
/// declaration has none or its actuals do not substitute.
pub(crate) fn expansion(
    store: &TypeInterner,
    signature: &Signature,
    arguments: &[GenericArgument],
) -> Option<TypeId> {
    let SignatureKind::Type(declaration) = &signature.kind else {
        return None;
    };
    crate::graph::expand(
        store,
        declaration.representation?,
        arguments,
        &signature.metadata.roles,
    )
    .ok()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prove(
    source: &TypeInterner,
    root: TypeId,
    bound: Bound,
    generics: &GenericDeclarations,
    check_root_locality: bool,
    use_evidence: bool,
    shared_storage: bool,
    handles: bool,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Sendability, QueryError> {
    let flags = ProofFlags {
        check_root_locality,
        use_evidence,
        shared_storage,
        handles,
    };
    prove_flags(source, root, bound, generics, flags, lookup)
}

#[cfg(test)]
thread_local! { static DIRECT_PROOF_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn direct_proof_walks() -> usize {
    DIRECT_PROOF_WALKS.with(std::cell::Cell::get)
}
#[cfg(test)]
pub(crate) fn prove_full(
    store: &TypeInterner,
    root: TypeId,
    bound: Bound,
    generics: &GenericDeclarations,
    flags: ProofFlags,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Sendability, QueryError> {
    prove_flags_walk(store, root, bound, generics, flags, lookup)
}

/// Main's worklist proof: one walk with a proof-wide visited set.
fn prove_flags(
    store: &TypeInterner,
    root: TypeId,
    bound: Bound,
    generics: &GenericDeclarations,
    flags: ProofFlags,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Sendability, QueryError> {
    // Rustc15c10447 type_ir/fast_reject.rs:94-104,130-145 settles
    // leaf kinds locally. Reuse the exact proof step as ContextMemo does;
    // these kinds never produce graph children or nominal expansions.
    if store.get(root).is_some_and(|row| {
        matches!(
            row.kind,
            TypeKind::Primitive(_)
                | TypeKind::Parameter(_)
                | TypeKind::Function(_)
                | TypeKind::Error
        )
    }) {
        let mut context = Direct {
            t: store,
            generics,
            lookup,
        };
        let mut dependencies = BTreeSet::new();
        let (step, _) = proof_step(
            store,
            root,
            root,
            false,
            &mut false,
            bound,
            generics,
            flags,
            &mut dependencies,
            &mut context,
        )?;
        match step {
            ProofStep::Fail => return Ok(Sendability::NotSendable),
            ProofStep::Parts(parts) => {
                assert!(parts.is_empty(), "a leaf has no proof children");
                return Ok(if dependencies.is_empty() {
                    Sendability::Sendable
                } else {
                    Sendability::DependsOn(dependencies.into_iter().collect())
                });
            }
            ProofStep::Expand { .. } => unreachable!("a leaf cannot expand a declaration"),
        }
    }
    prove_flags_walk(store, root, bound, generics, flags, lookup)
}
fn prove_flags_walk(
    store: &TypeInterner,
    root: TypeId,
    bound: Bound,
    generics: &GenericDeclarations,
    flags: ProofFlags,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<Sendability, QueryError> {
    #[cfg(test)]
    DIRECT_PROOF_WALKS.with(|n| n.set(n.get() + 1));
    let mut context = Direct {
        t: store,
        generics,
        lookup,
    };
    let mut pending = vec![(root, false, false, false)];
    let mut seen = IdSet::default();
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
        let (step, unique) = proof_step(
            store,
            root,
            id,
            inherited_unique,
            &mut plain_known,
            bound,
            generics,
            flags,
            &mut dependencies,
            &mut context,
        )?;
        match step {
            ProofStep::Fail => return Ok(Sendability::NotSendable),
            // Check declaration-order counterexamples before a later expanding
            // recursive field can spend the operational proof budget.
            ProofStep::Parts(children) => pending.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|t| (t, unique, plain_known, false)),
            ),
            ProofStep::Expand {
                signature,
                arguments,
            } => {
                if !nominal_roots.enter(id, usize::from(unique), store)? {
                    continue;
                }
                pending.push((id, unique, plain_known, true));
                let Some(representation) = expansion(store, &signature, &arguments) else {
                    return Ok(Sendability::NotSendable);
                };
                pending.push((representation, unique, plain_known, false));
            }
        }
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
    store: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
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
    store: &TypeInterner,
    root: TypeId,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    normalize_graph(store, root, None, Normalization::Plain, lookup)
}

/// Canonical inert qualifiers using the caller's exact generic declarations.
/// Imported callable fields may substitute a nominal Plain actual or a
/// parameter whose Plain proof belongs to an enclosing lexical binder.
pub fn canonical_plain_type_in(
    store: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    normalize_graph(store, root, Some(generics), Normalization::Plain, lookup)
}

/// Canonicalize flow-checked current rows without replacing their evidence
/// with a structural summary. Generic Plain assumptions still canonicalize.
pub fn canonical_current_type(
    store: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    normalize_graph(store, root, Some(generics), Normalization::Current, lookup)
}

/// Canonicalize the current rows of one query under one immutable signature
/// view and generic context. Completed subgraphs are reused across roots; their
/// first signature reads remain recorded by the query's lookup. A new batch
/// must be used for a different private view or generic declaration context.
///
/// Borrowed from rustc traits/project.rs:310-319,350-365: normalize under the
/// exact parameter environment and retain the obligations of reused answers.
/// Here the query's Reader retains first reads, so a hit adds no new read.
pub fn canonical_current_types(
    store: &TypeInterner,
    roots: &mut [TypeId],
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<(), QueryError> {
    let mut memo = Default::default();
    for root in roots {
        // Recovery/cyclic graphs retain the single-root traversal's behavior:
        // an active-cycle fallback is not a completed result for another root.
        *root = if store.valid(*root) {
            normalize_graph_cached(
                store,
                *root,
                Some(generics),
                Normalization::Current,
                lookup,
                &mut memo,
            )?
        } else {
            canonical_current_type(store, *root, generics, lookup)?
        };
    }
    Ok(())
}

/// Resolve cleanup conjunctions from the bounds of each exact declaring binder.
/// Traverses nested callable contracts without changing any other evidence.
pub fn instantiate_cleanup_bounds(
    store: &TypeInterner,
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
    store: &TypeInterner,
    root: TypeId,
    summaries: Option<&GenericDeclarations>,
    mode: Normalization,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    normalize_graph_cached(
        store,
        root,
        summaries,
        mode,
        lookup,
        &mut Default::default(),
    )
}
fn normalize_graph_cached(
    store: &TypeInterner,
    root: TypeId,
    summaries: Option<&GenericDeclarations>,
    mode: Normalization,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
    memo: &mut IdMap<(TypeId, Vec<TypeId>), TypeId>,
) -> Result<TypeId, QueryError> {
    let unscoped = GenericDeclarations::default();
    let generics = summaries.unwrap_or(&unscoped);
    fn visit(
        store: &TypeInterner,
        id: TypeId,
        generics: &GenericDeclarations,
        mode: Normalization,
        root: TypeId,
        contract: bool,
        binders: &[TypeId],
        memo: &mut IdMap<(TypeId, Vec<TypeId>), TypeId>,
        active: &mut IdSet<TypeId>,
        lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
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
        row.kind = row.kind.try_map_edges(|ty| {
            visit(
                store,
                ty,
                &child_generics,
                mode,
                root,
                child_contract,
                &child_binders,
                memo,
                active,
                lookup,
            )
        })?;
        let mut current = if store.get(id) == Some(&row) {
            id
        } else {
            store.intern(row.clone())
        };
        if matches!(mode, Normalization::Cleanup) {
            if matches!(&row.drop_allocation, DropAllocation::DependsOn(parameters)
                if parameters.iter().all(|parameter| matches!(generics.get(*parameter),
                    Some(GenericParameter::Type { bounds, .. })
                        if bounds.iter().any(|bound| matches!(bound, Bound::NoallocDrop | Bound::Plain)))))
            {
                row.drop_allocation = DropAllocation::NoAlloc;
                current = store.intern(row);
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
            current = store.intern(row.clone());
        }
        if row.canonicalize_shared_uniqueness() {
            current = store.intern(row.clone());
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
                current = store.intern(row);
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
        memo,
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
pub(crate) fn value_promises(expected: &Type, actual: &mut Type) -> bool {
    function_value_promises(expected, actual)
        && erased_state_promises(expected, actual)
        && cleanup_promises(expected, actual)
}

pub(crate) fn receives_crossing(expected: &Sendability, actual: &Sendability) -> bool {
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TypeRelation {
    Exact,
    Value,
    Parameter,
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
    expected.is_environment()
        || actual == expected
        || (!dependent && actual.contained_in(expected))
        || (relation == TypeRelation::Parameter && expected.is_global())
}

/// Derive owned storage content from its declared access type (OWN-3/OWN-8).
/// The borrowed root locality does not describe the displaced value's lifetime.
pub fn storage_content_type(
    store: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    let Some(mut row) = store.get(root).cloned() else {
        return Ok(root);
    };
    if !row.modes.relation.has_environment() {
        return Ok(root);
    }
    row.modes.relation = row.modes.relation.retain(|r| *r != Reach::Environment);
    let root = store.intern(row);
    normalize_type_properties(store, root, generics, lookup)
}

/// An unrelated borrowed aggregate is viewed in its actual placement. Its
/// implicit payload relations inherit that placement on both sides; explicit
/// payload relations and nonstructural contracts remain unchanged.
pub(crate) fn placed_receiving_type(
    store: &TypeInterner,
    expected: TypeId,
    actual: TypeId,
    relation: TypeRelation,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    let mut row = store
        .get(expected)
        .ok_or(QueryError::NotImplemented)?
        .clone();
    let expected = if relation == TypeRelation::Parameter
        && row.modes.relation.is_global()
        && matches!(
            row.kind,
            TypeKind::Tuple(_) | TypeKind::Box(_) | TypeKind::Record(_) | TypeKind::Variant(_)
        ) {
        row.modes.relation = store
            .get(actual)
            .ok_or(QueryError::NotImplemented)?
            .modes
            .relation
            .clone();
        store.intern(row)
    } else {
        expected
    };
    placed_structure_type(store, expected, generics, lookup)
}

/// Compare structural payloads under the same placement their projections inherit.
/// Callable headers and generic arguments retain their declared contracts.
pub(crate) fn placed_structure_type(
    store: &TypeInterner,
    root: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
) -> Result<TypeId, QueryError> {
    fn visit(
        store: &TypeInterner,
        root: TypeId,
        generics: &GenericDeclarations,
        lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
        memo: &mut IdMap<TypeId, TypeId>,
    ) -> Result<TypeId, QueryError> {
        if let Some(id) = memo.get(&root) {
            return Ok(*id);
        }
        let Some(original) = store.get(root) else {
            return Ok(root);
        };
        if !matches!(
            original.kind,
            TypeKind::Tuple(_) | TypeKind::Box(_) | TypeKind::Record(_) | TypeKind::Variant(_)
        ) {
            return Ok(root);
        }
        let mut row = original.clone();
        row.kind = row.kind.try_map_edges(|part| {
            let part = placed_part_type(store, root, part, generics, lookup)?;
            visit(store, part, generics, lookup, memo)
        })?;
        let id = if &row == original {
            root
        } else {
            store.intern(row)
        };
        memo.insert(root, id);
        Ok(id)
    }
    store
        .validate(root)
        .map_err(|_| QueryError::NotImplemented)?;
    visit(store, root, generics, lookup, &mut IdMap::default())
}

pub fn placed_part_type(
    store: &TypeInterner,
    owner: TypeId,
    part: TypeId,
    generics: &GenericDeclarations,
    lookup: &mut impl FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
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
        let related = store.intern(row.clone());
        row.modes.sendability = structural_sendability(store, related, generics, lookup)?;
        return Ok(store.intern(row));
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
        let store = TypeInterner::new(None);
        let payload = store.intern(row(TypeKind::Primitive(Primitive::Bytes)));
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
        let declared = TypeInterner::new(None);
        let bytes = declared.intern(row(TypeKind::Primitive(Primitive::Bytes)));
        let declaration = std::sync::Arc::new(Signature {
            definition: DefId::intern(&path),
            metadata: SignatureMetadata::default(),
            fingerprint: ContentHash::default(),
            kind: SignatureKind::Type(Box::new(TypeDefinition {
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
            })),
        });
        let store = std::sync::Arc::clone(&declared);
        let flat = store.intern(row(TypeKind::Named {
            definition: DefId::intern(&path),
            arguments: vec![].into(),
        }));
        let scalar = store.intern(row(TypeKind::Primitive(Primitive::U8)));
        let bytes = store.intern(row(TypeKind::Primitive(Primitive::Bytes)));
        let text = store.intern(row(TypeKind::Primitive(Primitive::String)));
        let parameter = store.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let tuple = store.intern(row(TypeKind::Tuple(vec![flat].into())));
        let generics = [GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Flat].into(),
        }];
        let mut lookup = |_: DefId| Ok::<_, QueryError>(Some(declaration.clone()));
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
        let pointer = store.intern(row(TypeKind::Builtin {
            kind: BuiltinType::Ptr,
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }));
        let pointer_record = store.intern(row(TypeKind::Record(
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
                &mut |_: DefId| Ok::<_, QueryError>(Some(std::sync::Arc::new(
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
    fn producer(store: &TypeInterner, sendability: Sendability, uses: Uses) -> TypeId {
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
        let unit = store.intern(row(TypeKind::Tuple(vec![].into())));
        let mut inner = row(TypeKind::Function(function(unit, uses)));
        inner.modes.sendability = sendability;
        let inner = store.intern(inner);
        store.intern(row(TypeKind::Function(function(inner, Uses::Unrestricted))))
    }
    #[test]
    fn a_nested_result_promises_more_only_where_the_value_is_not_handed_back() {
        let generics = GenericDeclarations::signature(&[]);
        let mut lookup = |_: DefId| Ok::<Option<SignatureRef>, QueryError>(None);
        let inert = || Uses::Only(AuthoritySummary::inert());
        let store = TypeInterner::new(None);
        let plain = producer(&store, Sendability::NotSendable, Uses::Unrestricted);
        for stronger in [
            producer(&store, Sendability::Sendable, Uses::Unrestricted),
            producer(&store, Sendability::NotSendable, inert()),
            producer(&store, Sendability::Sendable, inert()),
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
        let mut lookup = |_: DefId| Ok::<Option<SignatureRef>, QueryError>(None);
        let store = TypeInterner::new(None);
        let mut item = row(TypeKind::Primitive(Primitive::U64));
        item.drop_allocation = DropAllocation::NoAlloc;
        let item = store.intern(item);
        let iterator = |captures: Option<ErasedCaptures>, cleanup: DropAllocation| {
            let mut part = row(TypeKind::Builtin {
                kind: BuiltinType::Iter,
                arguments: vec![GenericArgument::Type(item)].into(),
            });
            part.erased_captures = captures;
            part.drop_allocation = cleanup;
            let part = store.intern(part);
            let holder = store.intern(row(TypeKind::Tuple(vec![part].into())));
            store.intern(row(TypeKind::Builtin {
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
    #[test]
    fn sealed_regions_33_opaque_owner_keeps_the_roots_complete_crossing_evidence() {
        let store = TypeInterner::new(None);
        let scalar = store.intern(row(TypeKind::Primitive(Primitive::U64)));
        let counted = store.intern(row(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }));
        let mut unique_row = store.row(counted).clone();
        unique_row.modes.uniqueness = Uniqueness::Unique;
        let unique = store.intern(unique_row.clone());
        unique_row.modes.relation = Relation::environment();
        let local = store.intern(unique_row);
        let core = DefPath::core("Region");
        let foreign = DefPath(ItemPath {
            package: PackageId {
                name: "fixture".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: "Region".into(),
        });
        for (payload, outer_unique, expected) in [
            (scalar, false, true),
            (counted, false, false),
            (counted, true, false),
            (unique, false, true),
            (local, false, false),
        ] {
            let representation = store.intern(row(TypeKind::Builtin {
                kind: BuiltinType::Region,
                arguments: vec![GenericArgument::Type(payload)].into(),
            }));
            for (path, wanted) in [(&core, expected), (&foreign, false)] {
                let definition = DefId::intern(path);
                let mut nominal = row(TypeKind::Named {
                    definition,
                    arguments: vec![].into(),
                });
                if outer_unique {
                    nominal.modes.uniqueness = Uniqueness::Unique;
                }
                let nominal = store.intern(nominal);
                let declaration = std::sync::Arc::new(Signature {
                    definition,
                    fingerprint: ContentHash::default(),
                    metadata: SignatureMetadata {
                        visibility: hirsel_syntax::Visibility::PublicOpaque,
                        ..Default::default()
                    },
                    kind: SignatureKind::Type(Box::new(TypeDefinition {
                        loop_source: None,
                        format: None,
                        destructor_allocation: Allocation::NoAlloc,
                        generics: vec![],
                        representation: Some(representation),
                        bounds: vec![],
                        authority: AuthoritySummary::inert(),
                        capability: None,
                        implementation_authority: None,
                        serializable: false,
                        serializable_check: None,
                        layout: Layout::default(),
                        distinct: false,
                        resource: true,
                        drop: None,
                        fact: None,
                        captures: None,
                    })),
                });
                let mut lookup = |_| Ok::<_, QueryError>(Some(declaration.clone()));
                let generics = GenericDeclarations::signature(&[]);
                assert_eq!(
                    structural_property(
                        &store,
                        nominal,
                        StructuralProperty::Sendable,
                        &generics,
                        &mut lookup
                    )
                    .unwrap(),
                    wanted,
                    "{payload:?} {outer_unique} {path}"
                );
                assert!(
                    !structural_property(
                        &store,
                        nominal,
                        StructuralProperty::Copyable,
                        &generics,
                        &mut lookup
                    )
                    .unwrap()
                );
            }
        }
    }
}
