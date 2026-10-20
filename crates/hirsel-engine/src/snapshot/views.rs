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
use hirsel_types::{HeapRole, Signature, SignatureRef};
use hirsel_validate::{
    SignatureLookup, UniverseErrors, VRead, ValidatedSignatures, ValidationDigest, ViewVerdict,
    Violation,
};
use std::collections::BTreeSet;
use std::sync::Arc;

/// One signature view and V's digest of it.
#[derive(Clone, Debug)]
pub struct View {
    pub d: DefId,
    pub signature: SignatureRef,
    pub digest: ValidationDigest,
}
impl View {
    /// A view of `signature`, keeping `previous`'s digest when it is the same view.
    pub fn new(signature: SignatureRef, previous: Option<&View>) -> Self {
        let digest = match previous {
            Some(view) if Arc::ptr_eq(&view.signature, &signature) => view.digest,
            _ => hirsel_validate::digest(&signature),
        };
        Self {
            d: DefId::intern(&signature.definition),
            signature,
            digest,
        }
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
        v.signature
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
}
impl ViewTable {
    /// The table of `signatures` (one per path), reusing the digests of
    /// `previous` for unchanged views and the verdicts of `store`'s previous
    /// revision whose reads still hold.
    pub fn new(
        signatures: impl IntoIterator<Item = SignatureRef>,
        previous: Option<&ViewTable>,
        store: &mut ValidatedSignatures,
        old: Option<&ValidatedSignatures>,
    ) -> Self {
        let mut sorted: Vec<SignatureRef> = signatures.into_iter().collect();
        sorted.sort_by(|a, b| a.definition.cmp(&b.definition));
        let views: Vec<View> = sorted
            .into_iter()
            .map(|signature| {
                let d = DefId::intern(&signature.definition);
                View::new(signature, previous.and_then(|p| p.get(d)))
            })
            .collect();
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
        };
        let verdicts: Vec<_> = table
            .views
            .iter()
            .map(|v| store.verdict_from(v.d, &table, old))
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
    pub fn views(&self) -> &[View] {
        &self.views
    }
    pub fn verdict(&self, d: DefId) -> Option<&Arc<ViewVerdict>> {
        self.index.get(&d).map(|&i| &self.verdicts[i as usize])
    }
}
impl SignatureLookup for ViewTable {
    fn view(&self, d: DefId) -> Option<(&Signature, ValidationDigest)> {
        self.get(d).map(|v| (&*v.signature, v.digest))
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
}
impl PackageViews {
    /// The universe `base` with `replacements` in place of (or beside) its views.
    pub fn new(
        base: Arc<ViewTable>,
        replacements: impl IntoIterator<Item = View>,
        store: &mut ValidatedSignatures,
        old: Option<&ValidatedSignatures>,
    ) -> Self {
        let replaced: DefMap<View> = replacements.into_iter().map(|v| (v.d, v)).collect();
        Self::over(base, replaced, &DefMap::default(), store, old)
    }
    /// This universe with `signature` in place of its view: compile-time
    /// evaluation's materialized owner.
    pub fn replace(
        &self,
        signature: SignatureRef,
        store: &mut ValidatedSignatures,
        old: Option<&ValidatedSignatures>,
    ) -> Self {
        let view = View::new(signature, None);
        let mut replaced = self.replaced.clone();
        replaced.insert(view.d, view);
        Self::over(self.base.clone(), replaced, &self.verdicts, store, old)
    }
    /// The view this universe has in place of `d`'s public one.
    pub fn replacement(&self, d: DefId) -> Option<&View> {
        self.replaced.get(&d)
    }
    fn over(
        base: Arc<ViewTable>,
        replaced: DefMap<View>,
        checked: &DefMap<Arc<ViewVerdict>>,
        store: &mut ValidatedSignatures,
        old: Option<&ValidatedSignatures>,
    ) -> Self {
        // The universe in DefPath order: the base's views and new ones.
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
        };
        let verdicts: DefMap<_> = again
            .iter()
            .map(|&d| (d, store.verdict_from(d, &views, old)))
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
    fn view(&self, d: DefId) -> Option<(&Signature, ValidationDigest)> {
        match self.replaced.get(&d) {
            Some(v) => Some((&*v.signature, v.digest)),
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
