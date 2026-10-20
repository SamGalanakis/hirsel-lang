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
use hirsel_syntax::{ContentHash, DefId};
use hirsel_types::HeapRole;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// V's identity of one signature view: a hash over every field it holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ValidationDigest(pub ContentHash);

/// One input V observed.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
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
/// The encoding is the view's derived `Debug` form, which names every field
/// of every row (store-local `TypeId`s print their row index), so it follows
/// the shapes without a hand-kept field list.
pub fn digest(signature: &Signature) -> ValidationDigest {
    let text = format!("{signature:?}");
    ValidationDigest(ContentHash::of_parts(
        "hirsel.validate.view.v1",
        [text.as_bytes()],
    ))
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
    reads: RefCell<Vec<VRead>>,
}
impl<'a> Reader<'a> {
    pub(crate) fn new(signatures: &'a dyn SignatureLookup) -> Self {
        Self {
            signatures,
            reads: RefCell::default(),
        }
    }
    pub(crate) fn get(&self, path: &DefPath) -> Option<&'a Signature> {
        let d = DefId::intern(path);
        match self.signatures.view(d) {
            Some((signature, digest)) => {
                self.reads.borrow_mut().push(VRead::View(d, digest));
                Some(signature)
            }
            None => {
                self.reads.borrow_mut().push(VRead::Missing(d));
                None
            }
        }
    }
    /// The first heap role. The selection is a read, and so is the view that
    /// supplies the role.
    pub(crate) fn first_heap(&self) -> Option<HeapRole> {
        let first = self.signatures.first_heap();
        let selected = first.as_ref().map(|(d, _)| *d);
        self.reads.borrow_mut().push(VRead::FirstHeap(selected));
        if let Some(d) = selected {
            self.get(d.path());
        }
        first.map(|(_, role)| role)
    }
    pub(crate) fn universe_errors(&self) -> &'a [Violation] {
        let (digest, errors) = self.signatures.universe_errors();
        self.reads.borrow_mut().push(VRead::UniverseErrors(digest));
        errors
    }
    /// The reads made so far, sorted and without duplicates.
    pub(crate) fn finish(self, reads: &mut Vec<VRead>) {
        let mut own = self.reads.into_inner();
        own.sort();
        own.dedup();
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
    d: DefId,
    signature: &Signature,
    signatures: &dyn SignatureLookup,
) -> ViewVerdict {
    let reader = Reader::new(signatures);
    let error = signature_error(d.path(), signature, &reader);
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
    /// The verdict on view `d` of `signatures`, reused or computed.
    pub fn verdict(&mut self, d: DefId, signatures: &dyn SignatureLookup) -> Arc<ViewVerdict> {
        self.verdict_from(d, signatures, None)
    }
    /// The verdict on view `d` of `signatures`: one this store or `previous`
    /// holds whose reads still hold, or a new one.
    pub fn verdict_from(
        &mut self,
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
            .unwrap_or_else(|| Arc::new(check_view(d, signature, signatures)));
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
        views: impl IntoIterator<Item = DefId>,
        signatures: &dyn SignatureLookup,
    ) -> UniverseErrors {
        let errors = views
            .into_iter()
            .filter_map(|d| self.verdict(d, signatures).error.clone())
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

/// An owned universe of signatures for tools and tests. Each view's digest
/// and the universe's errors are computed when first asked for, and changing
/// a signature forgets them.
#[derive(Clone, Debug, Default)]
pub struct SignatureMap {
    views: BTreeMap<DefPath, View>,
    errors: OnceLock<UniverseErrors>,
}
#[derive(Clone, Debug)]
struct View {
    d: DefId,
    signature: Signature,
    digest: OnceLock<ValidationDigest>,
}
impl View {
    fn new(path: &DefPath, signature: Signature) -> Self {
        Self {
            d: DefId::intern(path),
            signature,
            digest: OnceLock::new(),
        }
    }
    fn digest(&self) -> ValidationDigest {
        *self.digest.get_or_init(|| digest(&self.signature))
    }
}
impl SignatureMap {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&mut self, path: DefPath, signature: Signature) -> Option<Signature> {
        self.errors = OnceLock::new();
        let view = View::new(&path, signature);
        self.views.insert(path, view).map(|old| old.signature)
    }
    pub fn remove(&mut self, path: &DefPath) -> Option<Signature> {
        self.errors = OnceLock::new();
        self.views.remove(path).map(|old| old.signature)
    }
    pub fn clear(&mut self) {
        self.errors = OnceLock::new();
        self.views.clear();
    }
    pub fn get(&self, path: &DefPath) -> Option<&Signature> {
        self.views.get(path).map(|v| &v.signature)
    }
    pub fn get_mut(&mut self, path: &DefPath) -> Option<&mut Signature> {
        self.errors = OnceLock::new();
        let view = self.views.get_mut(path)?;
        view.digest = OnceLock::new();
        Some(&mut view.signature)
    }
    pub fn contains_key(&self, path: &DefPath) -> bool {
        self.views.contains_key(path)
    }
    pub fn len(&self) -> usize {
        self.views.len()
    }
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }
    pub fn keys(&self) -> impl Iterator<Item = &DefPath> {
        self.views.keys()
    }
    pub fn values(&self) -> impl Iterator<Item = &Signature> {
        self.views.values().map(|v| &v.signature)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&DefPath, &Signature)> {
        self.views.iter().map(|(p, v)| (p, &v.signature))
    }
}
impl std::ops::Index<&DefPath> for SignatureMap {
    type Output = Signature;
    fn index(&self, path: &DefPath) -> &Signature {
        self.get(path).expect("a signature at the path")
    }
}
impl<const N: usize> From<[(DefPath, Signature); N]> for SignatureMap {
    fn from(signatures: [(DefPath, Signature); N]) -> Self {
        signatures.into_iter().collect()
    }
}
impl FromIterator<(DefPath, Signature)> for SignatureMap {
    fn from_iter<I: IntoIterator<Item = (DefPath, Signature)>>(iter: I) -> Self {
        let mut map = Self::new();
        map.extend(iter);
        map
    }
}
impl Extend<(DefPath, Signature)> for SignatureMap {
    fn extend<I: IntoIterator<Item = (DefPath, Signature)>>(&mut self, iter: I) {
        for (path, signature) in iter {
            self.insert(path, signature);
        }
    }
}
impl SignatureLookup for SignatureMap {
    fn view(&self, d: DefId) -> Option<(&Signature, ValidationDigest)> {
        self.views.get(d.path()).map(|v| (&v.signature, v.digest()))
    }
    fn first_heap(&self) -> Option<(DefId, HeapRole)> {
        self.views.values().find_map(|v| {
            v.signature
                .metadata
                .roles
                .heap
                .clone()
                .map(|role| (v.d, role))
        })
    }
    fn universe_errors(&self) -> (ContentHash, &[Violation]) {
        let universe = self.errors.get_or_init(|| {
            UniverseErrors::new(
                self.views
                    .values()
                    .filter_map(|v| check_view(v.d, &v.signature, self).error)
                    .collect(),
            )
        });
        (universe.digest, &universe.errors)
    }
}
