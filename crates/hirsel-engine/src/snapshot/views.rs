//! The signature views V validates against, with V's verdict on each.
//!
//! `ViewTable` is the public universe of one snapshot: every signature's
//! public view in DefPath order, its digest, V's verdict on it and a reverse
//! index from each view to the verdicts that read it. `PackageViews` is that
//! universe as one package's bodies see it: the package's opaque types through
//! their private views, and, for compile-time evaluation, a materialized
//! owner's signature in place of its declared one. Only the replaced views and
//! the verdicts that read them are checked again.
use hirsel_syntax::{ContentHash, DefId, DefMap};
use hirsel_types::{HeapRole, SignatureRef, TypeInterner};
use hirsel_validate::{
    SignatureLookup, UniverseErrors, VRead, ValidatedSignatures, ValidationDigest, ViewVerdict,
    Violation,
};
use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

/// One signature view and V's digest of it.
#[derive(Clone, Debug)]
pub struct View {
    pub d: DefId,
    pub(super) input: Arc<hirsel_validate::IdentifiedSignature>,
}
impl View {
    /// A view of `signature`, keeping `previous`'s digest when it is the same view.
    pub fn new(t: &Arc<TypeInterner>, signature: SignatureRef, previous: Option<&View>) -> Self {
        let input = match previous {
            Some(view)
                if view.input.belongs_to(t) && Arc::ptr_eq(view.input.signature(), &signature) =>
            {
                view.input.clone()
            }
            _ => Arc::new(hirsel_validate::IdentifiedSignature::new(
                t.clone(),
                signature,
            )),
        };
        Self {
            d: input.signature().definition,
            input,
        }
    }
    pub(super) fn matches(&self, t: &TypeInterner, signature: &SignatureRef) -> bool {
        self.input.belongs_to(t) && Arc::ptr_eq(self.input.signature(), signature)
    }
    pub fn signature(&self) -> &SignatureRef {
        self.input.signature()
    }
    pub fn digest(&self) -> ValidationDigest {
        self.input.digest()
    }
}

/// Every view V's verdict on `d` reads, by the view it names.
fn read_views(verdict: &ViewVerdict) -> impl Iterator<Item = DefId> + '_ {
    verdict.reads.iter().filter_map(|read| match read {
        VRead::View(d, _) | VRead::Missing(d) => Some(*d),
        VRead::FirstHeap(_) | VRead::UniverseErrors(_) => None,
    })
}
fn reads_heap(verdict: &ViewVerdict) -> bool {
    verdict
        .reads
        .iter()
        .any(|read| matches!(read, VRead::FirstHeap(_)))
}
fn first_heap<'a>(views: impl Iterator<Item = &'a View>) -> Option<(DefId, HeapRole)> {
    views.into_iter().find_map(|v| {
        v.signature()
            .metadata
            .roles
            .heap
            .clone()
            .map(|role| (v.d, role))
    })
}

/// The public universe of one snapshot, with V's verdict on every view.
pub struct ViewTable {
    views: Vec<View>,
    index: DefMap<u32>,
    first_heap: Option<(DefId, HeapRole)>,
    verdicts: Vec<Arc<ViewVerdict>>,
    /// The views whose verdict read each view (or its absence).
    readers: DefMap<Vec<u32>>,
    /// The views whose verdict read the first heap role.
    heap_readers: Vec<u32>,
    errors: UniverseErrors,
    input_digest: OnceLock<ContentHash>,
}
impl ViewTable {
    /// The table of `signatures` (one per path), reusing the digests of
    /// `previous` for unchanged views and the verdicts of `store`'s previous
    /// revision whose reads still hold.
    pub fn new(
        t: &Arc<TypeInterner>,
        signatures: impl IntoIterator<Item = SignatureRef>,
        previous: Option<&ViewTable>,
        store: &mut ValidatedSignatures,
        old: Option<&ValidatedSignatures>,
    ) -> Self {
        Self::from_views(
            t,
            signatures.into_iter().map(|signature| {
                let d = signature.definition;
                View::new(t, signature, previous.and_then(|p| p.get(d)))
            }),
            store,
            old,
        )
    }
    /// Build a universe from already identified immutable products. V still
    /// checks each declaration's verdict against this universe's exact reads.
    pub(crate) fn from_views(
        t: &Arc<TypeInterner>,
        views: impl IntoIterator<Item = View>,
        store: &mut ValidatedSignatures,
        old: Option<&ValidatedSignatures>,
    ) -> Self {
        let _batch = store.publication_batch();
        let mut views: Vec<View> = views.into_iter().collect();
        views.sort_by_key(|view| view.d);
        let index = views
            .iter()
            .enumerate()
            .map(|(i, v)| (v.d, i as u32))
            .collect();
        let mut table = Self {
            first_heap: first_heap(views.iter()),
            views,
            index,
            verdicts: vec![],
            readers: DefMap::default(),
            heap_readers: vec![],
            errors: UniverseErrors::default(),
            input_digest: OnceLock::new(),
        };
        let verdicts: Vec<_> = table
            .views
            .iter()
            .map(|v| store.verdict_from(t, v.d, &table, old))
            .collect();
        for (i, verdict) in verdicts.iter().enumerate() {
            for d in read_views(verdict).collect::<BTreeSet<_>>() {
                table.readers.entry(d).or_default().push(i as u32);
            }
            if reads_heap(verdict) {
                table.heap_readers.push(i as u32);
            }
        }
        table.errors =
            UniverseErrors::new(verdicts.iter().filter_map(|v| v.error.clone()).collect());
        table.verdicts = verdicts;
        table
    }
    pub fn get(&self, d: DefId) -> Option<&View> {
        self.index.get(&d).map(|&i| &self.views[i as usize])
    }
    /// Identity of all inputs a private evaluation overlay can observe. This
    /// includes valid rows: an overlay can invalidate a dependent declaration
    /// even when both base universes have no errors. Compute once per universe.
    pub fn input_digest(&self) -> ContentHash {
        *self.input_digest.get_or_init(|| {
            input_digest(
                self.views.iter(),
                self.first_heap.as_ref(),
                self.errors.digest,
            )
        })
    }
    pub fn views(&self) -> &[View] {
        &self.views
    }
    pub fn verdict(&self, d: DefId) -> Option<&Arc<ViewVerdict>> {
        self.index.get(&d).map(|&i| &self.verdicts[i as usize])
    }
}
impl SignatureLookup for ViewTable {
    fn view(&self, d: DefId) -> Option<hirsel_validate::ViewInput<'_>> {
        self.get(d).map(|v| v.input.input())
    }
    fn first_heap(&self) -> Option<(DefId, HeapRole)> {
        self.first_heap.clone()
    }
    fn universe_errors(&self) -> (ContentHash, &[Violation]) {
        (self.errors.digest, &self.errors.errors)
    }
}

/// One package's universe: the public table with some views replaced.
pub struct PackageViews {
    base: Arc<ViewTable>,
    replaced: DefMap<View>,
    /// Verdicts checked again under the replacements, by view.
    verdicts: DefMap<Arc<ViewVerdict>>,
    first_heap: Option<(DefId, HeapRole)>,
    errors: UniverseErrors,
    input_digest: OnceLock<ContentHash>,
}
impl PackageViews {
    /// The universe `base` with `replacements` in place of (or beside) its views.
    pub fn new(
        t: &Arc<TypeInterner>,
        base: Arc<ViewTable>,
        replacements: impl IntoIterator<Item = View>,
        store: &mut ValidatedSignatures,
        old: Option<&ValidatedSignatures>,
    ) -> Self {
        let replaced: DefMap<View> = replacements.into_iter().map(|v| (v.d, v)).collect();
        Self::over(t, base, replaced, &DefMap::default(), store, old)
    }
    /// This universe with `signature` in place of its view: compile-time
    /// evaluation's materialized owner.
    pub fn replace(
        &self,
        t: &Arc<TypeInterner>,
        signature: SignatureRef,
        store: &mut ValidatedSignatures,
        old: Option<&ValidatedSignatures>,
    ) -> Self {
        let view = View::new(t, signature, None);
        let mut replaced = self.replaced.clone();
        replaced.insert(view.d, view);
        Self::over(t, self.base.clone(), replaced, &self.verdicts, store, old)
    }
    /// The view this universe has in place of `d`'s public one.
    pub fn replacement(&self, d: DefId) -> Option<&View> {
        self.replaced.get(&d)
    }
    /// This package's complete V input identity, including its private views.
    /// Replacement artifacts use their base package's digest as provenance;
    /// their own bytes remain identified by their producing execution reads.
    pub fn input_digest(&self) -> ContentHash {
        *self.input_digest.get_or_init(|| {
            let views = self
                .base
                .views
                .iter()
                .map(|view| self.replaced.get(&view.d).unwrap_or(view))
                .chain(
                    self.replaced
                        .values()
                        .filter(|view| self.base.get(view.d).is_none()),
                );
            input_digest(views, self.first_heap.as_ref(), self.errors.digest)
        })
    }
    fn over(
        t: &Arc<TypeInterner>,
        base: Arc<ViewTable>,
        replaced: DefMap<View>,
        checked: &DefMap<Arc<ViewVerdict>>,
        store: &mut ValidatedSignatures,
        old: Option<&ValidatedSignatures>,
    ) -> Self {
        // The universe in DefPath order: the base's views and new ones.
        let _batch = store.publication_batch();
        let mut order: Vec<&View> = base
            .views
            .iter()
            .map(|v| replaced.get(&v.d).unwrap_or(v))
            .collect();
        order.extend(replaced.values().filter(|v| base.get(v.d).is_none()));
        order.sort_by(|a, b| a.d.path().cmp(b.d.path()));
        let heap = first_heap(order.iter().copied());
        // Verdicts to check again: the replaced views and every view whose
        // verdict, public or already checked here, read one of them.
        let mut again: BTreeSet<DefId> = replaced.keys().copied().collect();
        for d in replaced.keys() {
            for &i in base.readers.get(d).into_iter().flatten() {
                again.insert(base.views[i as usize].d);
            }
        }
        for (d, verdict) in checked {
            if read_views(verdict).any(|read| replaced.contains_key(&read)) {
                again.insert(*d);
            }
        }
        if heap.as_ref().map(|h| h.0) != base.first_heap.as_ref().map(|h| h.0) {
            again.extend(base.heap_readers.iter().map(|&i| base.views[i as usize].d));
        }
        let order: Vec<DefId> = order.iter().map(|v| v.d).collect();
        let mut views = Self {
            base,
            replaced,
            verdicts: DefMap::default(),
            first_heap: heap,
            errors: UniverseErrors::default(),
            input_digest: OnceLock::new(),
        };
        let verdicts: DefMap<_> = again
            .iter()
            .map(|&d| (d, store.verdict_from(t, d, &views, old)))
            .collect();
        let errors = order
            .iter()
            .filter_map(|d| match verdicts.get(d) {
                Some(verdict) => verdict.error.clone(),
                None => views.base.verdict(*d).and_then(|v| v.error.clone()),
            })
            .collect();
        views.errors = UniverseErrors::new(errors);
        views.verdicts = verdicts;
        views
    }
}
impl SignatureLookup for PackageViews {
    fn view(&self, d: DefId) -> Option<hirsel_validate::ViewInput<'_>> {
        match self.replaced.get(&d) {
            Some(v) => Some(v.input.input()),
            None => self.base.view(d),
        }
    }
    fn first_heap(&self) -> Option<(DefId, HeapRole)> {
        self.first_heap.clone()
    }
    fn universe_errors(&self) -> (ContentHash, &[Violation]) {
        (self.errors.digest, &self.errors.errors)
    }
}

/// Salsa validates a query against its recorded inputs (runtime.rs:316-334).
/// An overlay additionally reads the whole signature universe through its
/// error aggregate; hash that immutable input once rather than inventing an
/// error-only receipt or recording O(N) inputs for every evaluation.
fn input_digest<'a>(
    views: impl Iterator<Item = &'a View>,
    heap: Option<&(DefId, HeapRole)>,
    errors: ContentHash,
) -> ContentHash {
    // GEN-6 specializations are left out: one materializing in a program must
    // not invalidate every validation of its declaring package. A caller of
    // one reads its declaration and implementations, which determine it.
    let mut views: Vec<_> = views
        .filter(|view| hirsel_types::specialization_parts(view.d.path()).is_none())
        .collect();
    views.sort_by(|a, b| a.d.path().cmp(b.d.path()));
    let mut bytes = Vec::with_capacity(views.len() * 96);
    bytes.extend((views.len() as u64).to_le_bytes());
    for view in views {
        crate::memo::encode_def_path(&mut bytes, view.d.path());
        bytes.extend(view.digest().0.0);
    }
    bytes.push(u8::from(heap.is_some()));
    if let Some((d, _)) = heap {
        crate::memo::encode_def_path(&mut bytes, d.path());
    }
    bytes.extend(errors.0);
    ContentHash::of_parts("hirsel.validate.universe-input.v1", [bytes.as_slice()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universe_input_digest_covers_valid_views_and_package_replacements() {
        let types = crate::registry::session_types();
        let registry = crate::registry::BuiltinRegistry::new();
        let option = registry
            .get(DefId::intern(&hirsel_syntax::DefPath::core("Option")))
            .unwrap();
        let result = registry
            .get(DefId::intern(&hirsel_syntax::DefPath::core("Result")))
            .unwrap();
        let table = |rows: Vec<SignatureRef>| {
            Arc::new(ViewTable::new(
                &types,
                rows,
                None,
                &mut ValidatedSignatures::default(),
                None,
            ))
        };
        let first = table(vec![option.clone(), result.clone()]);
        let reordered = table(vec![result.clone(), option.clone()]);
        assert_eq!(first.input_digest(), reordered.input_digest());
        let mut changed = (*option).clone();
        changed.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
        // Even a forged unchanged producer fingerprint cannot hide V input changes.
        assert_eq!(changed.fingerprint, option.fingerprint);
        let changed = Arc::new(changed);
        let second = table(vec![changed.clone(), result]);
        assert_ne!(first.input_digest(), second.input_digest());
        assert_ne!(first.input_digest(), table(vec![option]).input_digest());
        let package = PackageViews::new(
            &types,
            first.clone(),
            [View::new(&types, changed, None)],
            &mut ValidatedSignatures::default(),
            None,
        );
        assert_eq!(package.input_digest(), second.input_digest());
        assert_eq!(
            first.input_digest(),
            reordered.input_digest(),
            "replacement leaves base immutable"
        );
    }
}
