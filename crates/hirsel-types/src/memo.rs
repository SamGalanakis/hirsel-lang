//! Memos over the session interner, split by whether they depend on context
//! (design §1a). `TypeMemo` holds pure transformations keyed by every explicit
//! input and lives for the session (one for I, one for V). `ContextMemo`
//! belongs to one query: it is bound to that query's generic declarations and
//! signature lookup, and a hit replays the lookups its entry observed into the
//! query, so the query's recorded reads are those of a cache-free evaluation.
use crate::encoding::{Encoder, EncodingError, canonical_encoding};
use crate::*;
use hirsel_intern::IdHasher;
use hirsel_syntax::{Bound, ContentHash};
use std::collections::{BTreeSet, HashMap as StdHashMap, HashSet as StdHashSet};

type HashMap<K, V, S = IdHasher> = StdHashMap<K, V, S>;
type HashSet<K> = StdHashSet<K, IdHasher>;
use std::hash::{BuildHasher, Hash};
use std::sync::{PoisonError, RwLock};

const SHARDS: usize = 16;

#[cfg(test)]
thread_local! { static PROOF_WALK_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn proof_walk_rows() -> usize {
    PROOF_WALK_ROWS.with(std::cell::Cell::get)
}

/// A sharded concurrent map: the structural memo is read by every worker.
struct Shards<K, V> {
    shards: [RwLock<HashMap<K, V, IdHasher>>; SHARDS],
}
impl<K: Eq + Hash, V: Clone> Shards<K, V> {
    fn new() -> Self {
        Self {
            shards: std::array::from_fn(|_| RwLock::new(HashMap::default())),
        }
    }
    fn shard(&self, key: &K) -> &RwLock<HashMap<K, V, IdHasher>> {
        &self.shards[(IdHasher::default().hash_one(key) >> 60) as usize % SHARDS]
    }
    fn get_or(&self, key: K, compute: impl FnOnce() -> V) -> V {
        let shard = self.shard(&key);
        if let Some(value) = shard
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return value.clone();
        }
        // Computed outside the lock: the computation may consult the memo.
        let value = compute();
        shard
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_insert(value)
            .clone()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct SubstitutionKey {
    ty: TypeId,
    binder: Option<GenericBinder>,
    arguments: Box<[GenericArgument]>,
    authorities: Box<[AuthoritySummary]>,
    roles: NominalRoles,
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct InstanceKey {
    view: ViewDigest,
    arguments: Box<[GenericArgument]>,
    authorities: Box<[AuthoritySummary]>,
    roles: NominalRoles,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ExpansionKey {
    representation: TypeId,
    arguments: SharedSlice<GenericArgument>,
    role: u32,
}

/// Structural memos only: every entry is a pure function of its key, so the
/// memo is shared session-wide by one checker. I and V each own one.
pub struct TypeMemo {
    substitutions: Shards<SubstitutionKey, Result<TypeId, GraphError>>,
    instances: Shards<InstanceKey, Result<Instantiated, GraphError>>,
    projections: Shards<(TypeId, TypeId, bool), TypeId>,
}
impl Default for TypeMemo {
    fn default() -> Self {
        Self::new()
    }
}
impl TypeMemo {
    pub fn new() -> Self {
        Self {
            substitutions: Shards::new(),
            instances: Shards::new(),
            projections: Shards::new(),
        }
    }
    pub(crate) fn substitute(
        &self,
        ty: TypeId,
        s: &Substitution<'_>,
        compute: impl FnOnce() -> Result<TypeId, GraphError>,
    ) -> Result<TypeId, GraphError> {
        if s.binder.is_none() && s.roles.result_type.is_none() {
            // Transport without a binder is the identity on a valid graph.
            return compute();
        }
        self.substitutions.get_or(
            SubstitutionKey {
                ty,
                binder: s.binder,
                arguments: s.arguments.into(),
                authorities: s.authorities.into(),
                roles: s.roles.clone(),
            },
            compute,
        )
    }
}
impl std::fmt::Debug for TypeMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TypeMemo")
    }
}

/// The exact content of one signature view: a hash over every field, the
/// private shape and implementation ceiling included. Two views with equal
/// public fingerprints but different private representations differ here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewDigest(pub ContentHash);

/// The `ViewDigest` of `view`, computed when the view enters the snapshot.
pub fn view_digest(t: &TypeInterner, view: &Signature) -> Result<ViewDigest, EncodingError> {
    let mut bytes = canonical_encoding(t, view, &[])?;
    // The ceiling is excluded from public encodings; a view still binds it.
    if let SignatureKind::Type(declaration) = &view.kind {
        let mut e = Encoder::default();
        e.option(
            declaration.implementation_authority.as_ref(),
            |e, ceiling| e.authority(ceiling),
        );
        bytes.extend_from_slice(&e.bytes);
    }
    Ok(ViewDigest(ContentHash::of_parts(
        "hirsel.signature-view.v1",
        [bytes.as_slice(), view.fingerprint.0.as_slice()],
    )))
}

/// A signature's kind with every type edge substituted at its own binder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instantiated {
    pub kind: SignatureKind,
}

/// `sig` at `arguments`: its header (or constant type, or representation)
/// substituted for its own binder. Keyed by the view's exact digest and the
/// actuals, never by its DefId or public fingerprint, so an edited resident
/// signature or private shape cannot hit a stale entry.
pub fn instantiate(
    t: &TypeInterner,
    m: &TypeMemo,
    sig: &Signature,
    view: ViewDigest,
    s: &Substitution<'_>,
) -> Result<Instantiated, GraphError> {
    let key = InstanceKey {
        view,
        arguments: s.arguments.into(),
        authorities: s.authorities.into(),
        roles: s.roles.clone(),
    };
    m.instances.get_or(key, || {
        let s = Substitution {
            binder: Some(GenericBinder::Bound(0)),
            ..*s
        };
        let kind = match &sig.kind {
            SignatureKind::Function(f) => {
                SignatureKind::Function(crate::graph::substitute_header(t, m, f, &s)?)
            }
            SignatureKind::Const { ty, generics } => SignatureKind::Const {
                ty: substitute(t, m, *ty, &s)?,
                generics: generics.clone(),
            },
            SignatureKind::Type(declaration) => {
                let mut declaration = declaration.clone();
                declaration.representation = declaration
                    .representation
                    .map(|r| substitute(t, m, r, &s))
                    .transpose()?;
                SignatureKind::Type(declaration)
            }
            SignatureKind::Family(family) => SignatureKind::Family(family.clone()),
            SignatureKind::Implementation(implementation) => {
                SignatureKind::Implementation(implementation.clone())
            }
        };
        Ok(Instantiated { kind })
    })
}

/// OWN-8: a part read through its owner is bounded by both relations unless
/// it is Plain. `plain` is the contextual Plain proof of `part`
/// (`ContextMemo::plain`); the transformation itself is structural.
pub fn project(t: &TypeInterner, m: &TypeMemo, owner: TypeId, part: TypeId, plain: bool) -> TypeId {
    if plain {
        return part;
    }
    m.projections.get_or((owner, part, plain), || {
        let (Some(row), Some(owner)) = (t.get(part), t.get(owner)) else {
            return part;
        };
        let relation = row.modes.relation.union(&owner.modes.relation);
        if relation == row.modes.relation {
            return part;
        }
        // Projection introduces an access lifetime, not a counted share.
        // The operand's use and ownership flow decide any later invalidation.
        let mut row = row.clone();
        row.modes.relation = relation;
        t.intern(row)
    })
}

/// The proof mode of `ContextMemo::prove`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProofFlags {
    pub check_root_locality: bool,
    pub use_evidence: bool,
    pub shared_storage: bool,
    /// MEM-11: `Shared`, `Mutex` and `Atomic` handles count as detached parts.
    pub handles: bool,
}

/// An owned-payload set interned in one `ContextMemo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PayloadSetId(u32);

/// The policy of one directional comparison. `payloads` are the owned payload
/// rows of the actual along which a view may weaken nested evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CompatPolicy {
    pub forget_brands: bool,
    pub payloads: Option<PayloadSetId>,
    /// Nested rows may promise more in reading positions; false for hand-back.
    pub received: bool,
}
impl CompatPolicy {
    /// Ordinary reception: no brand forgetting, no payload view.
    pub const RECEIVED: Self = Self {
        forget_brands: false,
        payloads: None,
        received: true,
    };
    /// `mut` hand-back: every nested contract is exact.
    pub const HANDBACK: Self = Self {
        forget_brands: false,
        payloads: None,
        received: false,
    };
}

/// A memoized contextual result and the lookups it observed.
#[derive(Clone)]
struct Recorded<T> {
    value: Result<T, QueryError>,
    reads: Box<[DefId]>,
}

/// The signature lookup of a query: missing declarations are None.
pub type Lookup<'q> = dyn FnMut(DefId) -> Result<Option<SignatureRef>, QueryError> + 'q;

/// Contextual memos of one query, bound to its generic declarations and its
/// signature lookup (and so to its snapshot). Entries die with the query.
pub struct ContextMemo<'q> {
    t: &'q TypeInterner,
    generics: &'q GenericDeclarations,
    lookup: &'q mut Lookup<'q>,
    /// Every lookup this memo's evaluations performed, in order; an entry
    /// records the range its evaluation appended.
    log: Vec<DefId>,
    /// Whole proofs and the interior sub-proofs that are facts.
    proofs: HashMap<ProofKey, ProofEntry>,
    // A nominal's qualifiers can differ across property proofs while its
    // representation, actuals and roles select the same pure expansion.
    expansions: HashMap<ExpansionKey, Option<TypeId>>,
    compatible: HashMap<(TypeId, TypeId, TypeRelation, CompatPolicy), Recorded<bool>>,
    equality: HashMap<(TypeId, Bound), Recorded<bool>>,
    authorities: HashMap<(TypeId, u32), Recorded<AuthoritySummary>>,
    role_ids: HashMap<NominalRoles, u32>,
    // Retain the borrow: the metadata cannot mutate or reuse this address.
    bound_roles: Option<(&'q NominalRoles, u32)>,
    #[cfg(test)]
    pub(crate) role_key_lookups: usize,
    view_policies: HashMap<TypeId, Recorded<CompatPolicy>>,
    payload_ids: HashMap<Vec<TypeId>, PayloadSetId>,
    payload_sets: Vec<StdHashSet<TypeId>>,
    hits: u64,
}

impl<'q> ContextMemo<'q> {
    pub fn new(
        t: &'q TypeInterner,
        generics: &'q GenericDeclarations,
        lookup: &'q mut Lookup<'q>,
    ) -> Self {
        Self {
            t,
            generics,
            lookup,
            log: vec![],
            proofs: HashMap::default(),
            expansions: HashMap::default(),
            compatible: HashMap::default(),
            equality: HashMap::default(),
            authorities: HashMap::default(),
            role_ids: HashMap::default(),
            bound_roles: None,
            #[cfg(test)]
            role_key_lookups: 0,
            view_policies: HashMap::default(),
            payload_ids: HashMap::default(),
            payload_sets: vec![],
            hits: 0,
        }
    }
    /// Retain an immutable role context for this query's repeated proofs.
    /// Borrowed from rustc15c1044 data_structures/intern.rs:13-24: equal
    /// pointers avoid content hashing. Distinct pointers still use content
    /// equality here; public metadata need not be globally interned.
    pub fn with_nominal_roles(mut self, roles: &'q NominalRoles) -> Self {
        let role = self.role_id(roles);
        self.bound_roles = Some((roles, role));
        self
    }
    pub fn types(&self) -> &'q TypeInterner {
        self.t
    }
    pub fn generics(&self) -> &'q GenericDeclarations {
        self.generics
    }
    /// Memo hits so far, for the lane report.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Look up `definition` through the query's provider, recording the read.
    pub fn signature(&mut self, definition: DefId) -> Result<Option<SignatureRef>, QueryError> {
        self.log.push(definition);
        (self.lookup)(definition)
    }

    fn replay(&mut self, reads: &[DefId]) {
        self.hits += 1;
        for &definition in reads {
            // The entry's value already reflects this read's result; replay
            // records the dependency in the owning query.
            let _ = self.signature(definition);
        }
    }

    /// Run `f` with a lookup that records into this memo's log.
    fn observed<T>(
        &mut self,
        f: impl FnOnce(
            &'q TypeInterner,
            &'q GenericDeclarations,
            &mut dyn FnMut(DefId) -> Result<Option<SignatureRef>, QueryError>,
        ) -> Result<T, QueryError>,
    ) -> Recorded<T> {
        let start = self.log.len();
        let (t, generics) = (self.t, self.generics);
        let log = &mut self.log;
        let lookup = &mut *self.lookup;
        let value = f(t, generics, &mut |definition| {
            log.push(definition);
            lookup(definition)
        });
        Recorded {
            value,
            reads: distinct(&self.log[start..]),
        }
    }

    /// `ty` satisfies `bound` (Sendable: its crossing summary) under `flags`.
    /// Main's proof walk, with every sub-proof that completes as a fact (no
    /// coinductive assumption on an enclosing row, no budgeted expansion
    /// growth, no root exemption) memoized for later rows and proofs.
    pub fn prove(
        &mut self,
        ty: TypeId,
        bound: Bound,
        flags: ProofFlags,
    ) -> Result<Sendability, QueryError> {
        // Like rustc15c1044 erase_regions.rs:19-34,47-54, erase only inputs
        // irrelevant to this judgment. Copyable/Sendable keep exact modes.
        // Plain ignores occurrence modes and brands, including nested rows.
        // Keep exact generic declarations, roles and private views in this
        // query; canonicalize only the pure part of the proof key.
        let ty = if bound == Bound::Plain {
            self.t.strip_modes(ty)
        } else {
            ty
        };
        // A Sendable proof that exempts its root's own lifetime treats that
        // row differently wherever it recurs.
        let sensitive = bound == Bound::Sendable
            && !flags.check_root_locality
            && self
                .t
                .get(ty)
                .is_some_and(|r| !r.modes.relation.is_global());
        let key = ProofKey {
            ty,
            bound,
            flags,
            inherited_unique: false,
            plain_known: false,
            root: sensitive,
        };
        if let Some(entry) = self.proofs.get(&key).cloned() {
            self.replay(&entry.reads);
            return entry.value;
        }
        // Borrowed from rustc15c1044 type_ir/fast_reject.rs:94-104,130-145:
        // settle local leaf kinds before allocating recursive-walk state.
        // Hirsel delegates to its exact proof_step, retaining all flags and
        // lexical bounds; this is a full judgment, not an approximate reject.
        if self.t.get(ty).is_some_and(|row| {
            matches!(
                row.kind,
                TypeKind::Primitive(_)
                    | TypeKind::Parameter(_)
                    | TypeKind::Function(_)
                    | TypeKind::Error
            )
        }) {
            let (t, generics) = (self.t, self.generics);
            let mut walked = Walked::default();
            let (step, _) = crate::properties::proof_step(
                t,
                ty,
                ty,
                false,
                &mut false,
                bound,
                generics,
                flags,
                &mut walked.deps,
                self,
            )?;
            match step {
                crate::properties::ProofStep::Fail => walked.failed = true,
                crate::properties::ProofStep::Parts(parts) => {
                    assert!(parts.is_empty(), "a leaf has no proof children")
                }
                crate::properties::ProofStep::Expand { .. } => {
                    unreachable!("a leaf cannot expand a declaration")
                }
            }
            let value = walked.verdict();
            self.proofs.insert(
                key,
                ProofEntry {
                    value: Ok(value.clone()),
                    reads: Box::default(),
                    heights: Box::default(),
                    nonglobal: sensitive,
                    interior: !sensitive,
                },
            );
            return Ok(value);
        }
        let start = self.log.len();
        let value = self.walk(ty, bound, flags, sensitive);
        if let Err(error) = &value {
            let entry = ProofEntry {
                value: Err(error.clone()),
                reads: distinct(&self.log[start..]),
                heights: Box::default(),
                nonglobal: true,
                interior: false,
            };
            self.proofs.insert(key, entry);
        }
        value
    }

    fn walk(
        &mut self,
        root: TypeId,
        bound: Bound,
        flags: ProofFlags,
        sensitive: bool,
    ) -> Result<Sendability, QueryError> {
        let mut w = Walk::default();
        let mut stack: Vec<Frame> = vec![];
        let mut next = Some((root, false, false));
        loop {
            let settled = match next.take() {
                Some((id, inherited_unique, plain_known)) => self.visit(
                    &mut w,
                    &mut stack,
                    (root, id, inherited_unique, plain_known),
                    bound,
                    flags,
                    sensitive,
                )?,
                None => {
                    let frame = stack.last_mut().expect("a walk ends with its root");
                    if !frame.walked.failed
                        && let Some(child) = frame.children.next()
                    {
                        next = Some((child, frame.unique, frame.plain_known));
                        continue;
                    }
                    let frame = stack.pop().unwrap();
                    Some(self.finish(&mut w, frame, sensitive, stack.is_empty()))
                }
            };
            if let Some(walked) = settled {
                match stack.last_mut() {
                    None => return Ok(walked.verdict()),
                    Some(parent) => parent.walked.join(&walked, walked.low),
                }
            }
        }
    }

    /// Settle `id` from this walk or the memo, or push its frame.
    fn visit(
        &mut self,
        w: &mut Walk,
        stack: &mut Vec<Frame>,
        (root, id, inherited_unique, plain_known): (TypeId, TypeId, bool, bool),
        bound: Bound,
        flags: ProofFlags,
        sensitive: bool,
    ) -> Result<Option<Walked>, QueryError> {
        if let Some(visit) = w.seen.get(&(id, inherited_unique)) {
            return Ok(Some(w.settled(*visit)));
        }
        #[cfg(test)]
        PROOF_WALK_ROWS.with(|rows| rows.set(rows.get() + 1));
        let key = ProofKey {
            ty: id,
            bound,
            flags,
            inherited_unique,
            plain_known,
            root: sensitive && id == root,
        };
        if !stack.is_empty()
            && let Some(entry) = self.proofs.get(&key)
            && entry.interior
            && !(sensitive && entry.nonglobal)
            && entry
                .heights
                .iter()
                .all(|(d, h)| !w.nominal.may_grow_into(*d, *h))
        {
            let entry = entry.clone();
            self.replay(&entry.reads);
            let walked = Walked::from_entry(&entry)?;
            w.seen
                .insert((id, inherited_unique), Visit::Done(w.done.len()));
            w.done.push(walked.clone());
            return Ok(Some(walked));
        }
        let serial = w.resolved.len();
        w.resolved.push(None);
        w.seen.insert((id, inherited_unique), Visit::Active(serial));
        let log_start = self.log.len();
        let mut walked = Walked::default();
        if sensitive && let Some(row) = self.t.get(id) {
            walked.nonglobal = !row.modes.relation.is_global();
            walked.saw_root = id == root;
        }
        let mut known = plain_known;
        let (t, generics) = (self.t, self.generics);
        let (step, unique) = crate::properties::proof_step(
            t,
            root,
            id,
            inherited_unique,
            &mut known,
            bound,
            generics,
            flags,
            &mut walked.deps,
            self,
        )?;
        let mut children = vec![];
        let mut entered = None;
        match step {
            crate::properties::ProofStep::Fail => walked.failed = true,
            crate::properties::ProofStep::Parts(parts) => children = parts,
            crate::properties::ProofStep::Expand {
                signature,
                arguments,
            } => {
                let context = usize::from(unique);
                let class = w.nominal.class_of(t, id);
                if !w.nominal.enter(id, context, t)? {
                    // An instantiation already entered in this walk holds
                    // coinductively, or was proven.
                    match class.and_then(|c| w.classes.get(&(c, context))) {
                        Some(visit) => {
                            let assumed = w.settled(*visit);
                            walked.join(&assumed, assumed.low);
                        }
                        None => walked.growth = true,
                    }
                } else {
                    entered = Some((context, class));
                    if let Some(class) = class {
                        w.classes.insert((class, context), Visit::Active(serial));
                    }
                    walked.growth |= w.nominal.last_grew();
                    match (&t.row(id).kind, w.nominal.height_of(t, id)) {
                        (TypeKind::Named { definition, .. }, Some(height)) => {
                            walked.height(*definition, height)
                        }
                        _ => walked.growth = true,
                    }
                    match self.expansion(&signature, &arguments) {
                        Some(representation) => children.push(representation),
                        None => walked.failed = true,
                    }
                }
            }
        }
        stack.push(Frame {
            key,
            id,
            inherited_unique,
            unique,
            plain_known: known,
            serial,
            children: children.into_iter(),
            walked,
            log_start,
            entered,
        });
        Ok(None)
    }

    /// Complete `frame`: resolve its assumptions and memoize it when it is a fact.
    fn finish(&mut self, w: &mut Walk, frame: Frame, sensitive: bool, root: bool) -> Walked {
        let Frame {
            key,
            id,
            inherited_unique,
            serial,
            mut walked,
            log_start,
            entered,
            ..
        } = frame;
        if let Some((context, _)) = entered
            && !walked.failed
        {
            w.nominal.leave(id, context);
        }
        // An assumption of this frame itself, or of a completed descendant,
        // is discharged by this frame's completion.
        let low = w.effective(walked.low);
        walked.low = if low >= serial { NONE } else { low };
        w.resolved[serial] = Some(walked.low);
        let fact = !walked.growth
            && !(sensitive && walked.saw_root)
            && (walked.failed || walked.low == NONE);
        if fact || root {
            let entry = ProofEntry {
                value: Ok(walked.verdict()),
                reads: distinct(&self.log[log_start..]),
                heights: walked.heights.clone().into(),
                nonglobal: walked.nonglobal,
                interior: fact && !(sensitive && root),
            };
            self.proofs.insert(key, entry);
        }
        let done = Visit::Done(w.done.len());
        w.seen.insert((id, inherited_unique), done);
        if let Some((context, Some(class))) = entered {
            w.classes.insert((class, context), done);
        }
        w.done.push(walked.clone());
        walked
    }

    /// One pure representation substitution, shared across this query's
    /// property proofs. The signature was read by proof_step before this call;
    /// a cache hit skips no contextual lookup, bound, or expansion-growth check.
    ///
    /// Borrowed from rustc15c1044 traits/project.rs:310-319,350-365: memoize
    /// normalization at its exact inputs and retain the caller's obligations.
    /// Hirsel's expansion has no contextual reads; its complete key is the
    /// immutable representation, actual type/size arguments and nominal roles.
    fn expansion(
        &mut self,
        signature: &Signature,
        arguments: &SharedSlice<GenericArgument>,
    ) -> Option<TypeId> {
        let SignatureKind::Type(declaration) = &signature.kind else {
            return None;
        };
        let representation = declaration.representation?;
        let key = ExpansionKey {
            representation,
            arguments: arguments.clone(),
            role: self.role_id(&signature.metadata.roles),
        };
        let t = self.t;
        *self.expansions.entry(key).or_insert_with(|| {
            crate::graph::expand(t, representation, arguments, &signature.metadata.roles).ok()
        })
    }

    /// Whether `part` is Plain: the contextual half of `projected_type`.
    pub fn plain(&mut self, part: TypeId) -> Result<bool, QueryError> {
        Ok(self.prove(part, Bound::Plain, crate::properties::STRUCTURAL)? == Sendability::Sendable)
    }

    /// OWN-8 projection of `part` through `owner`: the Plain proof here, the
    /// transformation in the structural memo.
    pub fn projected(
        &mut self,
        m: &TypeMemo,
        owner: TypeId,
        part: TypeId,
    ) -> Result<TypeId, QueryError> {
        if !self.t.contains(part) || !self.t.contains(owner) {
            return Ok(part);
        }
        let relation = self
            .t
            .row(part)
            .modes
            .relation
            .union(&self.t.row(owner).modes.relation);
        if relation == self.t.row(part).modes.relation {
            return Ok(part);
        }
        let plain = self.plain(part)?;
        Ok(project(self.t, m, owner, part, plain))
    }

    /// Intern an owned-payload set for a `CompatPolicy`.
    pub fn payload_set(&mut self, ids: impl IntoIterator<Item = TypeId>) -> PayloadSetId {
        let set: StdHashSet<TypeId> = ids.into_iter().collect();
        let mut key: Vec<TypeId> = set.iter().copied().collect();
        key.sort_unstable_by_key(|id| id.to_raw());
        if let Some(id) = self.payload_ids.get(&key) {
            return *id;
        }
        let id = PayloadSetId(self.payload_sets.len() as u32);
        self.payload_sets.push(set);
        self.payload_ids.insert(key, id);
        id
    }

    /// The directional relation: `actual` received where `expected` stands
    /// (CLO-2, MODE-1/2/4, MEM-5). Variance reverses through callable
    /// parameters, `mut` parameters and nominal arguments stay exact, stronger
    /// promises are accepted only when receiving, and hand-back is exact.
    pub fn compatible(
        &mut self,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
        policy: CompatPolicy,
    ) -> Result<bool, QueryError> {
        if relation == TypeRelation::Exact {
            return Ok(self.t.semantic_eq(expected, actual));
        }
        let key = (expected, actual, relation, policy);
        if let Some(entry) = self.compatible.get(&key).cloned() {
            self.replay(&entry.reads);
            return entry.value;
        }
        let start = self.log.len();
        let value = self.compatible_uncached(expected, actual, relation, policy);
        let entry = Recorded {
            value,
            reads: distinct(&self.log[start..]),
        };
        self.compatible.insert(key, entry.clone());
        entry.value
    }

    fn compatible_uncached(
        &mut self,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
        policy: CompatPolicy,
    ) -> Result<bool, QueryError> {
        let placed_expected = self
            .observed(|t, generics, lookup| {
                crate::properties::placed_receiving_type(
                    t,
                    expected,
                    actual,
                    relation,
                    generics,
                    &mut |d| lookup(d),
                )
            })
            .value?;
        let placed_actual = self
            .observed(|t, generics, lookup| {
                crate::properties::placed_structure_type(t, actual, generics, &mut |d| lookup(d))
            })
            .value?;
        if (placed_expected, placed_actual) != (expected, actual) {
            let policy = if policy.payloads.is_some() {
                self.view_policy(placed_actual)?
            } else {
                policy
            };
            return self.compatible(placed_expected, placed_actual, relation, policy);
        }
        let t = self.t;
        let Some(expected_row) = t.get(expected).cloned() else {
            return Ok(false);
        };
        let Some(mut actual_row) = t.get(actual).cloned() else {
            return Ok(false);
        };
        if expected_row.allocation_target.is_some()
            && expected_row.allocation_target != actual_row.allocation_target
        {
            return Ok(false);
        }
        actual_row.allocation_target = expected_row.allocation_target;
        let plain = self.plain(actual)?;
        let received = policy.received;
        // A `mut` hand-back is written back through the parameter, so storage
        // that promised less than the parameter admits would return holding it:
        // the callable promise, the erased state's captures and the cleanup
        // summary are exact at the root as they are below it. A core operation
        // that only advances an erased state declares its parameter at the
        // argument's own captures, so it meets this rule by construction.
        let handed_back = received
            || (value_promises(&actual_row, &mut expected_row.clone())
                && actual_row.modes.multiplicity == expected_row.modes.multiplicity
                && (plain
                    || crossing_requirement(&actual_row) == crossing_requirement(&expected_row)));
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
        let payloads = policy.payloads.map(|id| &self.payload_sets[id.0 as usize]);
        Ok(crate::graph::directional_equal(
            t,
            expected,
            actual,
            actual_row,
            policy.forget_brands,
            payloads,
            received,
        )
        .unwrap_or(false))
    }

    /// `compatible` with ordinary reception (main's `compatible_type`).
    pub fn compatible_type(
        &mut self,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
    ) -> Result<bool, QueryError> {
        self.compatible(expected, actual, relation, CompatPolicy::RECEIVED)
    }

    /// The relation for a `mut` argument (main's `compatible_handback_type`):
    /// the callee writes the caller's storage back through its parameter, so
    /// every contract, root and nested, is exact.
    pub fn handback(&mut self, expected: TypeId, actual: TypeId) -> Result<bool, QueryError> {
        self.compatible(
            expected,
            actual,
            TypeRelation::Parameter,
            CompatPolicy::HANDBACK,
        )
    }

    /// The non-writing view policy of `actual` (main's `compatible_view_type_in`):
    /// Data may forget nested brands, and owned payload edges may weaken nested
    /// uniqueness. Payload classification reads declarations.
    pub fn view_policy(&mut self, actual: TypeId) -> Result<CompatPolicy, QueryError> {
        if let Some(entry) = self.view_policies.get(&actual).cloned() {
            self.replay(&entry.reads);
            return entry.value;
        }
        let start = self.log.len();
        let value = self.view_policy_uncached(actual);
        self.view_policies.insert(
            actual,
            Recorded {
                value: value.clone(),
                reads: distinct(&self.log[start..]),
            },
        );
        value
    }

    fn view_policy_uncached(&mut self, actual: TypeId) -> Result<CompatPolicy, QueryError> {
        let data = self.prove_bound(actual, Bound::Data)?;
        // Only owned payload edges are covariant. A callable's header, phantom
        // argument or interior-mutable cell remains the same contract even when
        // the containing value is taken. Nominal Data excludes hidden callbacks.
        let mut payloads = HashSet::default();
        let mut pending = vec![actual];
        let mut seen = HashSet::default();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(row) = self.t.get(id) else { continue };
            let owned = match &row.kind {
                TypeKind::Tuple(_)
                | TypeKind::Box(_)
                | TypeKind::Record(_)
                | TypeKind::Variant(_) => true,
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
                TypeKind::Named { .. } => self.prove_bound(id, Bound::Data)?,
                _ => false,
            };
            if owned {
                payloads.insert(id);
                pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
            }
        }
        Ok(CompatPolicy {
            forget_brands: data,
            payloads: Some(self.payload_set(payloads)),
            received: true,
        })
    }

    fn role_id(&mut self, roles: &NominalRoles) -> u32 {
        if let Some((bound, role)) = self.bound_roles
            && std::ptr::eq(bound, roles)
        {
            return role;
        }
        #[cfg(test)]
        {
            self.role_key_lookups += 1;
        }
        match self.role_ids.get(roles) {
            Some(role) => *role,
            None => {
                let role = self.role_ids.len() as u32;
                self.role_ids.insert(roles.clone(), role);
                role
            }
        }
    }

    /// Held authority under this query's exact declarations, roles and view.
    /// Hits retain the same observed-read replay as contextual property proofs.
    pub fn authority(
        &mut self,
        ty: TypeId,
        roles: &NominalRoles,
    ) -> Result<AuthoritySummary, QueryError> {
        let role = self.role_id(roles);
        let key = (ty, role);
        if let Some(entry) = self.authorities.get(&key).cloned() {
            self.replay(&entry.reads);
            return entry.value;
        }
        let entry = self.observed(|t, generics, lookup| {
            structural_authority(t, ty, roles, generics, &mut |_, definition| {
                definition.map_or(Ok(None), &mut *lookup)
            })
        });
        let value = entry.value.clone();
        self.authorities.insert(key, entry);
        value
    }

    /// The Data representation proof, under this query's declarations and view.
    pub fn data(&mut self, ty: TypeId) -> Result<bool, QueryError> {
        self.prove_bound(ty, Bound::Data)
    }

    /// Data/Key structural equality proof (`structural_equality`), recorded.
    fn prove_bound(&mut self, ty: TypeId, bound: Bound) -> Result<bool, QueryError> {
        // Data/Key are representation properties; current value modes and
        // brands do not change their structural equality judgment.
        let ty = self.t.strip_modes(ty);
        let key = (ty, bound);
        if let Some(entry) = self.equality.get(&key).cloned() {
            self.replay(&entry.reads);
            return entry.value;
        }
        let entry = self.observed(|t, generics, lookup| {
            structural_equality(t, ty, bound, generics, &mut |d| lookup(d))
        });
        let value = entry.value.clone();
        self.equality.insert(key, entry);
        value
    }

    /// MODE-2 alone for nested value parts. Other reception checks own locality,
    /// brands and callable headers; a mismatch on those axes is not a uniqueness
    /// failure. Reuse the ordinary view's owned-payload policy, including Data
    /// requirements for nominal generic arguments and callable boundaries.
    pub fn nested_uniqueness(
        &mut self,
        expected: TypeId,
        actual: TypeId,
        view: bool,
    ) -> Result<bool, QueryError> {
        let policy = if view {
            Some(self.view_policy(actual)?)
        } else {
            None
        };
        let payloads = policy
            .and_then(|policy| policy.payloads)
            .map(|id| &self.payload_sets[id.0 as usize]);
        let mut pending = vec![(expected, actual, view, false)];
        let mut seen = HashSet::default();
        while let Some((expected, actual, view, nested)) = pending.pop() {
            if !seen.insert((expected, actual, view, nested)) {
                continue;
            }
            let (Some(a), Some(b)) = (self.t.get(expected), self.t.get(actual)) else {
                return Ok(false);
            };
            if matches!(a.kind, TypeKind::Error) || matches!(b.kind, TypeKind::Error) {
                continue;
            }
            if nested
                && a.modes.uniqueness != b.modes.uniqueness
                && !(view
                    && a.modes.uniqueness == Uniqueness::Aliased
                    && b.modes.uniqueness == Uniqueness::Unique)
            {
                return Ok(false);
            }
            let children_view = view && payloads.is_some_and(|set| set.contains(&actual));
            pending.extend(
                a.kind
                    .value_parts()
                    .into_iter()
                    .zip(b.kind.value_parts())
                    .map(|(a, b)| (a, b, children_view, true)),
            );
        }
        Ok(true)
    }

    /// The non-writing view relation (main's `compatible_view_type_in`).
    pub fn view(
        &mut self,
        expected: TypeId,
        actual: TypeId,
        relation: TypeRelation,
    ) -> Result<bool, QueryError> {
        if relation == TypeRelation::Exact {
            return self.compatible_type(expected, actual, relation);
        }
        let policy = self.view_policy(actual)?;
        self.compatible(expected, actual, relation, policy)
    }

    /// Replacement of caller-owned storage must supply a global value
    /// (main's `compatible_storage_type`).
    pub fn storage(&mut self, expected: TypeId, actual: TypeId) -> Result<bool, QueryError> {
        if !self
            .t
            .get(actual)
            .is_some_and(|row| row.modes.relation.is_global())
        {
            return Ok(false);
        }
        self.compatible_type(expected, actual, TypeRelation::Value)
    }
}

impl crate::properties::ProofContext for ContextMemo<'_> {
    fn signature(&mut self, definition: DefId) -> Result<Option<SignatureRef>, QueryError> {
        ContextMemo::signature(self, definition)
    }
    fn plain(&mut self, id: TypeId) -> Result<bool, QueryError> {
        ContextMemo::plain(self, id)
    }
}

/// A row of main's proof worklist: its proof, and the walk state that
/// identifies it (inherited uniqueness, a known Plain proof, root exemption).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ProofKey {
    ty: TypeId,
    bound: Bound,
    flags: ProofFlags,
    inherited_unique: bool,
    plain_known: bool,
    root: bool,
}

#[derive(Clone)]
struct ProofEntry {
    value: Result<Sendability, QueryError>,
    reads: Box<[DefId]>,
    /// Per declaration, the tallest instantiation the proof entered: a walk
    /// with no active instantiation that short can reuse it, since none of
    /// these can grow from its expansions.
    heights: Box<[(DefId, usize)]>,
    /// Some reached row has a non-global lifetime.
    nonglobal: bool,
    /// A fact inside other walks, not only as a whole proof.
    interior: bool,
}

/// No enclosing frame.
const NONE: usize = usize::MAX;

/// A settled sub-proof as its enclosing frame joins it.
#[derive(Clone)]
struct Walked {
    failed: bool,
    deps: BTreeSet<TypeParameterRef>,
    /// The outermost in-progress frame (by serial) the result assumed.
    low: usize,
    heights: Vec<(DefId, usize)>,
    growth: bool,
    nonglobal: bool,
    saw_root: bool,
}
impl Default for Walked {
    fn default() -> Self {
        Self {
            failed: false,
            deps: BTreeSet::new(),
            low: NONE,
            heights: vec![],
            growth: false,
            nonglobal: false,
            saw_root: false,
        }
    }
}
impl Walked {
    fn from_entry(entry: &ProofEntry) -> Result<Self, QueryError> {
        let mut walked = Walked {
            heights: entry.heights.to_vec(),
            nonglobal: entry.nonglobal,
            ..Walked::default()
        };
        match entry.value.clone()? {
            Sendability::Sendable => (),
            Sendability::DependsOn(ps) => walked.deps.extend(ps),
            Sendability::NotSendable => walked.failed = true,
        }
        Ok(walked)
    }
    fn verdict(&self) -> Sendability {
        if self.failed {
            Sendability::NotSendable
        } else if self.deps.is_empty() {
            Sendability::Sendable
        } else {
            Sendability::DependsOn(self.deps.iter().copied().collect())
        }
    }
    fn height(&mut self, definition: DefId, height: usize) {
        match self.heights.iter_mut().find(|(d, _)| *d == definition) {
            Some((_, h)) => *h = (*h).max(height),
            None => self.heights.push((definition, height)),
        }
    }
    /// Join a settled part, whose assumptions resolve to `low`.
    fn join(&mut self, part: &Walked, low: usize) {
        self.failed |= part.failed;
        self.deps.extend(part.deps.iter().copied());
        self.low = self.low.min(low);
        for (d, h) in &part.heights {
            self.height(*d, *h);
        }
        self.growth |= part.growth;
        self.nonglobal |= part.nonglobal;
        self.saw_root |= part.saw_root;
    }
}

#[derive(Clone, Copy)]
enum Visit {
    Active(usize),
    Done(usize),
}

/// The state of one proof walk, shared by all its frames as main's worklist
/// shares its visited set and nominal expansions.
#[derive(Default)]
struct Walk {
    nominal: NominalExpansions,
    seen: HashMap<(TypeId, bool), Visit>,
    classes: HashMap<(TypeId, usize), Visit>,
    done: Vec<Walked>,
    /// Per frame serial: None while in progress, then its resolved `low`.
    resolved: Vec<Option<usize>>,
}
impl Walk {
    /// The in-progress frame an assumption on frame `serial` now rests on.
    fn effective(&self, mut serial: usize) -> usize {
        while serial != NONE {
            match self.resolved[serial] {
                None => return serial,
                Some(low) => serial = low,
            }
        }
        NONE
    }
    /// A part this walk already settled or is proving.
    fn settled(&self, visit: Visit) -> Walked {
        match visit {
            Visit::Active(serial) => Walked {
                low: serial,
                ..Walked::default()
            },
            Visit::Done(i) => {
                let mut walked = self.done[i].clone();
                walked.low = self.effective(walked.low);
                walked
            }
        }
    }
}

struct Frame {
    key: ProofKey,
    id: TypeId,
    inherited_unique: bool,
    unique: bool,
    plain_known: bool,
    serial: usize,
    children: std::vec::IntoIter<TypeId>,
    walked: Walked,
    log_start: usize,
    /// The expansion context and class this frame entered.
    entered: Option<(usize, Option<TypeId>)>,
}

/// A log range's reads, first occurrences only: replay records dependencies,
/// and a dependency recorded once is recorded.
fn distinct(log: &[DefId]) -> Box<[DefId]> {
    let mut seen = HashSet::default();
    log.iter().copied().filter(|d| seen.insert(*d)).collect()
}

#[cfg(test)]
mod expansion_tests {
    use super::*;
    use crate::declarations::tests::{def, nominal, path, row};
    use hirsel_syntax::Visibility;
    use std::sync::Arc;

    #[test]
    fn expansion_reuse_binds_representation_arguments_roles_and_preserves_reads() {
        let t = TypeInterner::new(None);
        let byte = t.intern(row(TypeKind::Primitive(Primitive::U8)));
        let text = t.intern(row(TypeKind::Primitive(Primitive::String)));
        let parameter = t.intern(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let pair = t.intern(row(TypeKind::Tuple(vec![parameter, parameter].into())));
        let boxed = t.intern(row(TypeKind::Box(parameter)));
        let mut first = (*nominal(Some(pair), vec![])).clone();
        first.definition = def("Private");
        first.metadata.visibility = Visibility::Private;
        let mut second = first.clone();
        let SignatureKind::Type(declaration) = &mut second.kind else {
            unreachable!()
        };
        declaration.representation = Some(boxed);
        let generics = GenericDeclarations::default();
        let mut lookup = |_| Ok(None);
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        for arguments in [
            vec![],
            vec![GenericArgument::Type(byte)],
            vec![GenericArgument::Type(text)],
        ] {
            let arguments: SharedSlice<_> = arguments.into();
            for view in [&first, &second] {
                for _ in 0..2 {
                    assert_eq!(
                        memo.expansion(view, &arguments),
                        crate::properties::expansion(&t, view, &arguments)
                    );
                }
            }
        }
        assert_eq!(
            memo.expansions.len(),
            6,
            "failed actuals and private representations have distinct keys"
        );
        let result = t.intern(row(TypeKind::Named {
            definition: def("Result"),
            arguments: vec![GenericArgument::Type(byte), GenericArgument::Type(text)].into(),
        }));
        let function = t.intern(row(TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: vec![].into(),
            result,
            error: None,
            allocation: Allocation::NoAlloc,
            authority: AuthoritySummary::inert(),
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        })));
        let mut unmarked = (*nominal(Some(function), vec![])).clone();
        std::sync::Arc::make_mut(&mut unmarked.metadata.roles).result_type = None;
        let mut marked = unmarked.clone();
        std::sync::Arc::make_mut(&mut marked.metadata.roles).result_type = Some(path("Result"));
        let plain = memo.expansion(&unmarked, &vec![].into()).unwrap();
        let error = memo.expansion(&marked, &vec![].into()).unwrap();
        assert_ne!(
            plain, error,
            "Result role controls the derived error channel"
        );

        // Repeated proofs with different flags still read the declaration before
        // the pure expansion hit; caching representation work cannot hide reads.
        let private = t.intern(row(TypeKind::Named {
            definition: def("Private"),
            arguments: vec![GenericArgument::Type(byte)].into(),
        }));
        let reads = std::cell::RefCell::new(vec![]);
        let mut lookup = |d| {
            reads.borrow_mut().push(d);
            Ok(Some(Arc::new(first.clone())))
        };
        let mut memo = ContextMemo::new(&t, &generics, &mut lookup);
        for bound in [Bound::Plain, Bound::Copyable, Bound::Sendable] {
            memo.prove(private, bound, crate::properties::STRUCTURAL)
                .unwrap();
        }
        assert_eq!(memo.expansions.len(), 1);
        assert!(reads.borrow().len() >= 3);
        assert!(reads.borrow().iter().all(|d| *d == def("Private")));
    }
}
