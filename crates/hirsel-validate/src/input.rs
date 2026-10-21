//! V's input: the signature views it reads, each identified by V's own digest,
//! and the reads it records against them.
//!
//! V never trusts the producer's fingerprint. Every view V reads is identified
//! by a `ValidationDigest` that this crate computes over every field of the
//! view, private shape included, so a producer that leaves a field out of its
//! fingerprint (a malformed row, a private field) still changes the digest.
//! Every lookup V makes is recorded as a `VRead`: a view by digest, a missing
//! signature, the first heap role's selection, and the universe's errors. A
//! verdict is reusable exactly while its reads still hold (`read_holds`).
use super::*;
use crate::hashed::HashMap;
use hirsel_syntax::{ContentHash, DefId};
use hirsel_types::{HeapRole, TypeInterner};
use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

/// V's identity of one signature view: a hash over every field it holds.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub struct ValidationDigest(pub ContentHash);

/// One input V observed.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd,
)]
pub enum VRead {
    /// The view of a signature V read, by its digest.
    View(DefId, ValidationDigest),
    /// A signature V looked for and did not find.
    Missing(DefId),
    /// Which signature supplies the first heap role, if any.
    FirstHeap(Option<DefId>),
    /// The aggregate of every signature error in the universe.
    UniverseErrors(ContentHash),
}

/// The signature views V validates against: one universe, as one package's
/// bodies see it (its own opaque types through their private views).
pub trait SignatureLookup: Sync {
    fn view(&self, d: DefId) -> Option<(&Signature, ValidationDigest)>;
    /// The heap role of the first signature in DefPath order that has one.
    fn first_heap(&self) -> Option<(DefId, HeapRole)>;
    /// Every signature error of the universe, in DefPath order, and their digest.
    fn universe_errors(&self) -> (ContentHash, &[Violation]);
}

/// V's digest of a view. Every field is encoded, including private shape,
/// malformed rows and metadata a producer could leave out of its fingerprint.
///
/// The encoding is the view's derived `Debug` form, which names every field,
/// followed by every row its types reach in first-encounter order. Each
/// `TypeId` is written as its ordinal in that order, never as the interner's
/// number, so the digest is the same in every session. It follows the shapes
/// without a hand-kept field list.
pub fn digest(t: &TypeInterner, signature: &Signature) -> ValidationDigest {
    let order = hirsel_types::closure(t, hirsel_types::signature_types(t, signature));
    let ordinals: HashMap<u32, usize> = order
        .iter()
        .enumerate()
        .map(|(i, id)| (raw(*id), i))
        .collect();
    let mut encoded = renumber(&format!("{signature:?}"), &ordinals);
    for id in order.iter() {
        encoded.push('\n');
        encoded.push_str(&renumber(&format!("{:?}", t.row(*id)), &ordinals));
    }
    ValidationDigest(ContentHash::of_parts(
        "hirsel.validate.view.v2",
        [encoded.as_bytes()],
    ))
}
const TYPE_ID: &str = "TypeId(";
fn raw(id: hirsel_types::TypeId) -> u32 {
    let text = format!("{id:?}");
    type_ids(&text).next().expect("a TypeId prints its number")
}
/// The numbers of the `TypeId(n)`s in `text`, in order.
fn type_ids(text: &str) -> impl Iterator<Item = u32> + '_ {
    text.match_indices(TYPE_ID).filter_map(|(at, _)| {
        let digits = &text[at + TYPE_ID.len()..];
        let end = digits.find(')')?;
        digits[..end].parse().ok()
    })
}
/// `text` with each `TypeId(n)` of a row in `ordinals` written as `TypeId(#i)`,
/// its ordinal; an id without a row keeps its number, which names the same
/// absence for the whole session.
fn renumber(text: &str, ordinals: &HashMap<u32, usize>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(TYPE_ID) {
        let (before, after) = rest.split_at(at + TYPE_ID.len());
        out.push_str(before);
        let end = after.find(')').unwrap_or(0);
        match after[..end]
            .parse::<u32>()
            .ok()
            .and_then(|n| ordinals.get(&n))
        {
            Some(ordinal) => {
                out.push('#');
                out.push_str(&ordinal.to_string());
            }
            None => out.push_str(&after[..end]),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// The digest of a universe's errors, in the order V reports them.
pub fn errors_digest(errors: &[Violation]) -> ContentHash {
    let text = format!("{errors:?}");
    ContentHash::of_parts("hirsel.validate.universe-errors.v1", [text.as_bytes()])
}

/// Whether `read` observes the same value in `signatures` now.
pub fn read_holds(read: &VRead, signatures: &dyn SignatureLookup) -> bool {
    match read {
        VRead::View(d, digest) => signatures.view(*d).is_some_and(|(_, now)| now == *digest),
        VRead::Missing(d) => signatures.view(*d).is_none(),
        VRead::FirstHeap(selected) => signatures.first_heap().map(|(d, _)| d) == *selected,
        VRead::UniverseErrors(digest) => signatures.universe_errors().0 == *digest,
    }
}

/// The recording lookup V's checks read through, for one validation.
pub(crate) struct Reader<'a> {
    signatures: &'a dyn SignatureLookup,
    /// A set: a check reads the same views many times, and V's verdict reads
    /// each once.
    reads: RefCell<crate::hashed::HashSet<VRead>>,
}
impl<'a> Reader<'a> {
    pub(crate) fn new(signatures: &'a dyn SignatureLookup) -> Self {
        Self {
            signatures,
            reads: RefCell::default(),
        }
    }
    pub(crate) fn get(&self, d: impl Into<DefId>) -> Option<&'a Signature> {
        let d = d.into();
        match self.signatures.view(d) {
            Some((signature, digest)) => {
                self.reads.borrow_mut().insert(VRead::View(d, digest));
                Some(signature)
            }
            None => {
                self.reads.borrow_mut().insert(VRead::Missing(d));
                None
            }
        }
    }
    /// The first heap role. The selection is a read, and so is the view that
    /// supplies the role.
    pub(crate) fn first_heap(&self) -> Option<HeapRole> {
        let first = self.signatures.first_heap();
        let selected = first.as_ref().map(|(d, _)| *d);
        self.reads.borrow_mut().insert(VRead::FirstHeap(selected));
        if let Some(d) = selected {
            self.get(d);
        }
        first.map(|(_, role)| role)
    }
    pub(crate) fn universe_errors(&self) -> &'a [Violation] {
        let (digest, errors) = self.signatures.universe_errors();
        self.reads
            .borrow_mut()
            .insert(VRead::UniverseErrors(digest));
        errors
    }
    /// The reads made so far, sorted and without duplicates.
    pub(crate) fn finish(self, reads: &mut Vec<VRead>) {
        let mut own: Vec<_> = self.reads.into_inner().into_iter().collect();
        own.sort_unstable();
        reads.extend(own);
    }
}

/// V's verdict on one view: the error it found, if any, and what it read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewVerdict {
    pub error: Option<Violation>,
    pub reads: Vec<VRead>,
}

/// V's verdict on the view of `d`, read through `signatures`.
pub fn check_view(
    t: &TypeInterner,
    d: DefId,
    signature: &Signature,
    signatures: &dyn SignatureLookup,
) -> ViewVerdict {
    let reader = Reader::new(signatures);
    let error = signature_error(t, d, signature, &reader);
    let mut reads = vec![];
    reader.finish(&mut reads);
    ViewVerdict { error, reads }
}

/// The signature errors of one universe, with their digest.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UniverseErrors {
    pub digest: ContentHash,
    pub errors: Vec<Violation>,
}
impl UniverseErrors {
    pub fn new(errors: Vec<Violation>) -> Self {
        Self {
            digest: errors_digest(&errors),
            errors,
        }
    }
}

/// Per-view verdicts keyed by digest. A verdict is reused for a view with the
/// same digest while every read it made still holds; otherwise V checks the
/// view again. A revision's store starts empty and takes what still holds
/// from the previous revision's (`verdict_from`), so it keeps only verdicts
/// its own views use.
#[derive(Clone, Default)]
pub struct ValidatedSignatures {
    verdicts: HashMap<ValidationDigest, Vec<Arc<ViewVerdict>>>,
}
impl ValidatedSignatures {
    /// Successful per-view receipts, for durable snapshot publication. The
    /// consumer recomputes each view's digest and checks every contextual read.
    pub fn successful(&self) -> impl Iterator<Item = (ValidationDigest, &[VRead])> {
        self.verdicts.iter().flat_map(|(digest, verdicts)| {
            verdicts
                .iter()
                .filter(|verdict| verdict.error.is_none())
                .map(move |verdict| (*digest, verdict.reads.as_slice()))
        })
    }
    /// Restore a successful view receipt. This supplies a candidate only:
    /// `verdict_from` still requires V's independently computed view digest
    /// and every observed read to agree with the current universe.
    pub fn seed_success(&mut self, digest: ValidationDigest, mut reads: Vec<VRead>) {
        reads.sort_unstable();
        reads.dedup();
        let kept = self.verdicts.entry(digest).or_default();
        if !kept
            .iter()
            .any(|verdict| verdict.error.is_none() && verdict.reads == reads)
        {
            kept.push(Arc::new(ViewVerdict { error: None, reads }));
        }
    }
    /// The verdict on view `d` of `signatures`, reused or computed.
    pub fn verdict(
        &mut self,
        t: &TypeInterner,
        d: DefId,
        signatures: &dyn SignatureLookup,
    ) -> Arc<ViewVerdict> {
        self.verdict_from(t, d, signatures, None)
    }
    /// The verdict on view `d` of `signatures`: one this store or `previous`
    /// holds whose reads still hold, or a new one.
    pub fn verdict_from(
        &mut self,
        t: &TypeInterner,
        d: DefId,
        signatures: &dyn SignatureLookup,
        previous: Option<&ValidatedSignatures>,
    ) -> Arc<ViewVerdict> {
        let (signature, digest) = signatures.view(d).expect("a verdict names a view");
        let holds = |v: &&Arc<ViewVerdict>| v.reads.iter().all(|read| read_holds(read, signatures));
        if let Some(verdict) = self
            .verdicts
            .get(&digest)
            .and_then(|kept| kept.iter().find(holds))
        {
            return verdict.clone();
        }
        let verdict = previous
            .and_then(|previous| previous.verdicts.get(&digest))
            .and_then(|kept| kept.iter().find(holds).cloned())
            .unwrap_or_else(|| Arc::new(check_view(t, d, signature, signatures)));
        self.verdicts
            .entry(digest)
            .or_default()
            .push(verdict.clone());
        verdict
    }
    /// The signature errors of the universe `views` (every view of
    /// `signatures`, in DefPath order).
    pub fn universe(
        &mut self,
        t: &TypeInterner,
        views: impl IntoIterator<Item = DefId>,
        signatures: &dyn SignatureLookup,
    ) -> UniverseErrors {
        let errors = views
            .into_iter()
            .filter_map(|d| self.verdict(t, d, signatures).error.clone())
            .collect();
        UniverseErrors::new(errors)
    }
    /// How many verdicts the store holds.
    pub fn len(&self) -> usize {
        self.verdicts.values().map(Vec::len).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.verdicts.is_empty()
    }
}

/// An owned universe of signatures for tools and tests, over one interner.
/// Each view's digest and the universe's errors are computed when first asked
/// for, and changing a signature forgets them.
#[derive(Clone)]
pub struct SignatureMap {
    types: Arc<TypeInterner>,
    views: BTreeMap<DefId, View>,
    errors: OnceLock<UniverseErrors>,
}
#[derive(Clone)]
struct View {
    signature: Signature,
    digest: OnceLock<ValidationDigest>,
}
impl View {
    fn new(signature: Signature) -> Self {
        Self {
            signature,
            digest: OnceLock::new(),
        }
    }
}
impl SignatureMap {
    pub fn new(types: Arc<TypeInterner>) -> Self {
        Self {
            types,
            views: BTreeMap::new(),
            errors: OnceLock::new(),
        }
    }
    pub fn types(&self) -> &Arc<TypeInterner> {
        &self.types
    }
    pub fn insert(&mut self, d: impl Into<DefId>, signature: Signature) -> Option<Signature> {
        self.errors = OnceLock::new();
        self.views
            .insert(d.into(), View::new(signature))
            .map(|old| old.signature)
    }
    pub fn remove(&mut self, d: impl Into<DefId>) -> Option<Signature> {
        self.errors = OnceLock::new();
        self.views.remove(&d.into()).map(|old| old.signature)
    }
    pub fn clear(&mut self) {
        self.errors = OnceLock::new();
        self.views.clear();
    }
    pub fn get(&self, d: impl Into<DefId>) -> Option<&Signature> {
        self.views.get(&d.into()).map(|v| &v.signature)
    }
    pub fn get_mut(&mut self, d: impl Into<DefId>) -> Option<&mut Signature> {
        self.errors = OnceLock::new();
        let view = self.views.get_mut(&d.into())?;
        view.digest = OnceLock::new();
        Some(&mut view.signature)
    }
    pub fn contains_key(&self, d: impl Into<DefId>) -> bool {
        self.views.contains_key(&d.into())
    }
    pub fn len(&self) -> usize {
        self.views.len()
    }
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }
    /// The views' definitions, in DefPath order.
    pub fn keys(&self) -> impl Iterator<Item = DefId> + '_ {
        self.views.keys().copied()
    }
    pub fn values(&self) -> impl Iterator<Item = &Signature> {
        self.views.values().map(|v| &v.signature)
    }
    pub fn iter(&self) -> impl Iterator<Item = (DefId, &Signature)> {
        self.views.iter().map(|(d, v)| (*d, &v.signature))
    }
    /// Every signature, keyed by its own definition.
    pub fn extend_signatures(&mut self, signatures: impl IntoIterator<Item = Signature>) {
        for signature in signatures {
            self.insert(signature.definition, signature);
        }
    }
}
impl<D: Into<DefId>> Extend<(D, Signature)> for SignatureMap {
    fn extend<I: IntoIterator<Item = (D, Signature)>>(&mut self, iter: I) {
        for (d, signature) in iter {
            self.insert(d, signature);
        }
    }
}
impl SignatureLookup for SignatureMap {
    fn view(&self, d: DefId) -> Option<(&Signature, ValidationDigest)> {
        self.views.get(&d).map(|v| {
            let digest = *v.digest.get_or_init(|| digest(&self.types, &v.signature));
            (&v.signature, digest)
        })
    }
    fn first_heap(&self) -> Option<(DefId, HeapRole)> {
        self.views.iter().find_map(|(d, v)| {
            v.signature
                .metadata
                .roles
                .heap
                .clone()
                .map(|role| (*d, role))
        })
    }
    fn universe_errors(&self) -> (ContentHash, &[Violation]) {
        let universe = self.errors.get_or_init(|| {
            UniverseErrors::new(
                self.views
                    .iter()
                    .filter_map(|(d, v)| check_view(&self.types, *d, &v.signature, self).error)
                    .collect(),
            )
        });
        (universe.digest, &universe.errors)
    }
}
