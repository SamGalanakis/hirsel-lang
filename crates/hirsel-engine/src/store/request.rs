//! One admitted request's transport over its selected best-effort durable store.
//! This changes physical retention only, never semantic query invalidation or
//! verifier authority. All producers receive the same Arc<RequestStore>.
use super::*;
use crate::runtime::MemoryReservation;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
mod scratch;

// Bounds payload ownership, Arc/region headers and BTreeMap allocation overhead.
const ENTRY_CHARGE: u64 = 2048;
fn association_bytes(entries: usize) -> u64 {
    if entries == 0 {
        return 0;
    }
    let mut weight = crate::NativeMetadataWeight::default();
    weight.tree::<ContentHash, Association>(entries);
    weight.bytes()
}
struct State {
    used: u64,
    contents: BTreeMap<ContentHash, (Reference, Bytes)>,
    associations: BTreeMap<ContentHash, Association>,
}
struct Admission {
    memory: MemoryReservation,
    used: AtomicU64,
    exhausted: AtomicBool,
    scratch: OnceLock<Arc<dyn Send + Sync>>,
}
impl Admission {
    fn charge(&self, bytes: u64) -> bool {
        if self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= self.memory.bytes())
            })
            .is_err()
        {
            self.exhausted.store(true, Ordering::Release);
            return false;
        }
        true
    }
    fn release(&self, bytes: u64) {
        let previous = self.used.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes);
    }
}
pub struct RequestStore {
    backend: Arc<dyn ResultStore>,
    verifier: Option<Arc<hirsel_validate::VerifierCache>>,
    memory: Arc<Admission>,
    state: Mutex<State>,
}
impl std::fmt::Debug for RequestStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        f.debug_struct("RequestStore")
            .field("retained_bytes", &self.retained_bytes())
            .field("metadata_bytes", &state.used)
            .field("associations", &state.associations.len())
            .field("contents", &state.contents.len())
            .finish()
    }
}
impl RequestStore {
    pub fn new(backend: Arc<dyn ResultStore>, memory: MemoryReservation) -> Self {
        Self::admitted(
            backend,
            Arc::new(Admission {
                memory,
                used: AtomicU64::new(0),
                exhausted: AtomicBool::new(false),
                scratch: OnceLock::new(),
            }),
        )
    }
    /// Explicit request-local scratch transport. The selected backend and its
    /// verifier/local-store authority remain unchanged. Metadata and live read
    /// buffers share the original RAM reservation; physical files have the
    /// caller's disk bound and are deleted after the last transported view.
    pub fn with_scratch(
        backend: Arc<dyn ResultStore>,
        memory: MemoryReservation,
        parent: &std::path::Path,
        disk_bytes: u64,
    ) -> std::io::Result<Self> {
        let admission = Arc::new(Admission {
            memory,
            used: AtomicU64::new(0),
            exhausted: AtomicBool::new(false),
            scratch: OnceLock::new(),
        });
        let backend = Arc::new(scratch::ScratchStore::new(
            backend,
            admission.clone(),
            parent,
            disk_bytes,
        )?);
        Ok(Self::admitted(backend, admission))
    }
    /// Preserve V's private content under the same admitted request transport.
    /// Its selected cache may evict rows or bodies independently; only V still
    /// issues associations and opaque acceptance capabilities.
    pub fn with_scratch_and_verifier_transport(
        backend: Arc<dyn ResultStore>,
        memory: MemoryReservation,
        parent: &std::path::Path,
        disk_bytes: u64,
    ) -> std::io::Result<Self> {
        let mut request = Self::with_scratch(backend, memory, parent, disk_bytes)?;
        if let Some(verifier) = &request.verifier {
            request.verifier = Some(verifier.with_transport(Arc::new(RequestVerifierRows {
                backend: request.backend.clone(),
                admission: request.memory.clone(),
                rows: Mutex::new(BTreeMap::new()),
            })));
        }
        Ok(request)
    }
    fn admitted(backend: Arc<dyn ResultStore>, memory: Arc<Admission>) -> Self {
        let verifier = backend.verifier();
        Self {
            backend,
            verifier,
            memory,
            state: Mutex::new(State {
                used: 0,
                contents: BTreeMap::new(),
                associations: BTreeMap::new(),
            }),
        }
    }
    /// The native task reports the existing deterministic resource failure
    /// before settling a product when its admitted transport allowance is full.
    pub fn exhausted(&self) -> bool {
        self.memory.exhausted.load(Ordering::Acquire)
    }
    pub fn retained_bytes(&self) -> u64 {
        self.memory.used.load(Ordering::Acquire)
    }
    fn charge(&self, state: &mut State, bytes: u64) -> bool {
        if !self.memory.charge(bytes) {
            return false;
        }
        state.used += bytes;
        true
    }
}
struct RequestVerifierRows {
    backend: Arc<dyn ResultStore>,
    admission: Arc<Admission>,
    rows: Mutex<BTreeMap<ContentHash, Reference>>,
}
fn verifier_row_bytes(entries: usize) -> u64 {
    if entries == 0 {
        return 0;
    }
    let mut weight = crate::NativeMetadataWeight::default();
    weight.tree::<ContentHash, Reference>(entries);
    weight.bytes()
}
impl Drop for RequestVerifierRows {
    fn drop(&mut self) {
        let entries = self
            .rows
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .len();
        self.admission.release(verifier_row_bytes(entries));
    }
}
impl hirsel_artifact_store::Contents for RequestVerifierRows {
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        self.backend.get_content(reference)
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        self.backend.put_content(domain, bytes)
    }
}
impl hirsel_validate::VerifierRows for RequestVerifierRows {
    fn row(&self, key: ContentHash) -> Option<Reference> {
        self.rows
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&key)
            .copied()
    }
    fn set_row(&self, key: ContentHash, reference: Reference) {
        let mut rows = self.rows.lock().unwrap_or_else(|error| error.into_inner());
        let additional = verifier_row_bytes(rows.len().saturating_add(1))
            .saturating_sub(verifier_row_bytes(rows.len()));
        if rows.contains_key(&key) || self.admission.charge(additional) {
            rows.insert(key, reference);
        }
    }
}
impl ResultStore for RequestStore {
    fn cache_product(&self, address: Address, action: Action, domain: Domain, bytes: &[u8]) {
        self.backend.cache_product(address, action, domain, bytes);
    }
    fn query_cache_enabled(&self) -> bool {
        self.backend.query_cache_enabled()
    }
    fn read_scope(&self) -> Option<Box<dyn Send + Sync>> {
        self.backend.read_scope()
    }
    fn name(&self) -> &'static str {
        self.backend.name()
    }
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.backend.local_product_store()
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.verifier.clone()
    }
    // Transient transport is enabled even if the selected durable policy is off.
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        self.backend.get(key)
    }
    fn put(&self, key: &EntryKey, value: &[u8]) {
        self.backend.put(key, value)
    }
    fn demand(&self, keys: &[EntryKey]) {
        self.backend.demand(keys)
    }
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        if let Some((actual, bytes)) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contents
            .get(&reference.output.0)
        {
            return (*actual == reference).then(|| bytes.clone());
        }
        self.backend.get_content(reference)
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        let expected = domain.reference(bytes)?;
        if self.backend.put_content(domain, bytes) == Some(expected) {
            return Some(expected);
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((actual, _)) = state.contents.get(&expected.output.0) {
            return (*actual == expected).then_some(expected);
        }
        if !self.charge(&mut state, expected.len.checked_add(ENTRY_CHARGE)?) {
            return None;
        }
        let bytes = Bytes::from(bytes).retain_owner(self.memory.clone());
        state.contents.insert(expected.output.0, (expected, bytes));
        Some(expected)
    }
    fn lookup_product(&self, address: Address) -> Option<Association> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .associations
            .get(&address.0)
            .copied()
            .or_else(|| self.backend.lookup_product(address))
    }
    fn associate_product(&self, address: Address, action: Action, reference: Reference) {
        // An association can never make absent/truncated bytes transferable.
        if self.get_content(reference).is_none() {
            return;
        }
        let association = Association { action, reference };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Durable associations are a shared latest-result index. Another
        // admitted request can publish a different action at the same address
        // immediately, so they cannot replace this request's selected result.
        // Retain only metadata here, under the same bounded transport charge.
        let exists = state.associations.contains_key(&address.0);
        let Some(next) = state.associations.len().checked_add(usize::from(!exists)) else {
            self.memory.exhausted.store(true, Ordering::Release);
            return;
        };
        let growth =
            association_bytes(next).saturating_sub(association_bytes(state.associations.len()));
        if exists || self.charge(&mut state, growth) {
            state.associations.insert(address.0, association);
            drop(state);
            self.backend.associate_product(address, action, reference);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ProcessBudget;
    fn domain() -> Domain {
        Domain {
            kind: 0x4e00001f,
            schema: 1,
        }
    }
    fn address() -> Address {
        Address(ContentHash::of_bytes(b"request transport"))
    }
    #[test]
    fn request_associations_charge_their_physical_tree_and_replacements_keep_its_grant() {
        for count in [32u32, 64, 128] {
            let backend = Arc::new(MemStore::default());
            let reference = backend.put_content(domain(), b"selected").unwrap();
            let allowance = 64 << 10;
            let budget = ProcessBudget::new(allowance);
            let request = RequestStore::new(backend, budget.reserve(allowance).unwrap());
            let action = Action(address().0);
            let (_, actual) = crate::test_alloc::measured(|| {
                for index in 0..count {
                    let address = Address(ContentHash::of_bytes(&index.to_le_bytes()));
                    request.associate_product(address, action, reference);
                }
            });
            let charged = request.retained_bytes();
            assert!(!request.exhausted());
            assert!(actual.live > 0);
            assert!(
                charged >= actual.live as u64,
                "charged={charged}, actual={}",
                actual.live
            );
            assert!(charged >= actual.peak as u64);
            assert!(charged < u64::from(count) * ENTRY_CHARGE);
            for index in 0..count {
                let address = Address(ContentHash::of_bytes(&index.to_le_bytes()));
                assert_eq!(
                    request.lookup_product(address),
                    Some(Association { action, reference })
                );
                request.associate_product(address, action, reference);
                assert_eq!(request.retained_bytes(), charged);
            }
            drop(request);
            assert_eq!(budget.reserved(), 0);
        }
    }
    #[test]
    fn request_transport_survives_durable_failure_and_held_child_retains_admission() {
        let allowance = ENTRY_CHARGE * 4;
        let budget = ProcessBudget::new(allowance);
        let request = Arc::new(RequestStore::new(
            Arc::new(NoneStore),
            budget.reserve(allowance).unwrap(),
        ));
        let reference = request.put_content(domain(), b"required product").unwrap();
        let action = Action(ContentHash::of_bytes(b"action"));
        request.associate_product(address(), action, reference);
        assert_eq!(
            request.lookup_product(address()),
            Some(Association { action, reference })
        );
        assert_eq!(
            &*request.get_content(reference).unwrap(),
            b"required product"
        );
        assert!(!request.exhausted());
        assert!(request.verifier().is_none());
        assert!(request.local_product_store().is_none());
        let held = request
            .get_content(reference)
            .unwrap()
            .range(9, 7, 1)
            .unwrap();
        drop(request);
        assert_eq!(budget.reserved(), allowance);
        assert_eq!(&*held, b"product");
        drop(held);
        assert_eq!(budget.reserved(), 0);
        // The next request has no durable success or process-global overlay.
        let next = RequestStore::new(Arc::new(NoneStore), budget.reserve(allowance).unwrap());
        assert!(next.lookup_product(address()).is_none());
        assert!(next.get_content(reference).is_none());
    }
    #[test]
    fn request_transport_zero_tiny_and_concurrent_allowances_never_expand() {
        for limit in [0, 1, ENTRY_CHARGE, ENTRY_CHARGE + 3] {
            let budget = ProcessBudget::new(limit);
            let request = RequestStore::new(Arc::new(NoneStore), budget.reserve(limit).unwrap());
            let value = request.put_content(domain(), b"1234");
            assert!(value.is_none());
            assert!(request.exhausted());
            assert!(request.retained_bytes() <= limit);
            assert!(budget.reserve(1).is_err());
        }
        let allowance = ENTRY_CHARGE * 4;
        let budget = ProcessBudget::new(allowance);
        let a = RequestStore::new(Arc::new(NoneStore), budget.reserve(allowance / 2).unwrap());
        let b = RequestStore::new(Arc::new(NoneStore), budget.reserve(allowance / 2).unwrap());
        assert!(budget.reserve(1).is_err());
        assert!(a.put_content(domain(), b"a").is_some());
        assert!(b.put_content(domain(), b"b").is_some());
        assert_eq!(budget.reserved(), allowance);
    }
    #[test]
    fn request_transport_forwards_exact_local_authority_and_checks_reference_domains() {
        let backend = Arc::new(MemStore::default());
        let expected = backend.verifier().unwrap();
        let budget = ProcessBudget::new(0);
        let request = RequestStore::new(backend, budget.reserve(0).unwrap());
        assert!(Arc::ptr_eq(&request.verifier().unwrap(), &expected));
        let reference = request.put_content(domain(), b"durable").unwrap();
        assert_eq!(request.retained_bytes(), 0);
        let mut wrong = reference;
        wrong.domain.schema += 1;
        assert!(request.get_content(wrong).is_none());
        request.associate_product(address(), Action(address().0), wrong);
        assert!(request.lookup_product(address()).is_none());
        assert!(!request.exhausted());
    }

    #[test]
    fn request_associations_remain_selected_across_interleaved_durable_publications() {
        let backend = Arc::new(MemStore::default());
        let budget = ProcessBudget::new(ENTRY_CHARGE * 2);
        let a = RequestStore::new(backend.clone(), budget.reserve(ENTRY_CHARGE).unwrap());
        let b = RequestStore::new(backend.clone(), budget.reserve(ENTRY_CHARGE).unwrap());
        let first = a.put_content(domain(), b"first request").unwrap();
        let second = b.put_content(domain(), b"second request").unwrap();
        let action_a = Action(ContentHash::of_bytes(b"first action"));
        let action_b = Action(ContentHash::of_bytes(b"second action"));
        let selected_a = Association {
            action: action_a,
            reference: first,
        };
        let selected_b = Association {
            action: action_b,
            reference: second,
        };
        a.associate_product(address(), action_a, first);
        b.associate_product(address(), action_b, second);
        assert_eq!(a.lookup_product(address()), Some(selected_a));
        assert_eq!(b.lookup_product(address()), Some(selected_b));
        assert_eq!(backend.lookup_product(address()), Some(selected_b));
        // Repeated publication by one request spends no additional allowance
        // and cannot change the other request's settled read.
        a.associate_product(address(), action_a, first);
        assert_eq!(backend.lookup_product(address()), Some(selected_a));
        assert_eq!(b.lookup_product(address()), Some(selected_b));
        assert_eq!(a.retained_bytes(), association_bytes(1));
        assert_eq!(b.retained_bytes(), association_bytes(1));
        assert!(!a.exhausted() && !b.exhausted());
        drop(a);
        drop(b);
        assert_eq!(budget.reserved(), 0);
        let next = RequestStore::new(backend, budget.reserve(ENTRY_CHARGE).unwrap());
        assert_eq!(next.lookup_product(address()), Some(selected_a));
    }

    #[test]
    fn durable_association_pinning_obeys_the_admitted_metadata_bound() {
        let backend = Arc::new(MemStore::default());
        let budget = ProcessBudget::new(ENTRY_CHARGE);
        let request = RequestStore::new(backend.clone(), budget.reserve(ENTRY_CHARGE).unwrap());
        let reference = request.put_content(domain(), b"durable content").unwrap();
        let action = Action(address().0);
        request.associate_product(address(), action, reference);
        for index in 1u32..4 {
            let next = Address(ContentHash::of_bytes(&index.to_le_bytes()));
            request.associate_product(next, action, reference);
            assert!(request.lookup_product(next).is_some());
        }
        let other = Address(ContentHash::of_bytes(b"another address"));
        request.associate_product(other, action, reference);
        assert!(request.exhausted());
        assert_eq!(request.retained_bytes(), association_bytes(4));
        assert!(request.retained_bytes() <= ENTRY_CHARGE);
        assert!(request.lookup_product(other).is_none());
        assert!(backend.lookup_product(other).is_none());
        assert!(request.lookup_product(address()).is_some());
    }
    #[test]
    fn optional_cache_projections_never_spend_required_transport_admission() {
        let directory = tempfile::tempdir().unwrap();
        let backend = Arc::new(MemStore::default());
        let allowance = 64 << 10;
        let budget = ProcessBudget::new(allowance);
        let request = RequestStore::with_scratch(
            backend.clone(),
            budget.reserve(allowance).unwrap(),
            directory.path(),
            1 << 20,
        )
        .unwrap();
        let before = request.retained_bytes();
        let action = Action(address().0);
        for index in 0u32..256 {
            let address = Address(ContentHash::of_bytes(&index.to_le_bytes()));
            request.cache_product(address, action, domain(), b"optional projection");
            assert!(backend.lookup_product(address).is_some());
        }
        assert_eq!(request.retained_bytes(), before);
        assert!(!request.exhausted());
        let required = request.put_content(domain(), b"required output").unwrap();
        request.associate_product(address(), action, required);
        assert!(request.lookup_product(address()).is_some());
        assert!(!request.exhausted());
        drop(request);
        assert_eq!(budget.reserved(), 0);

        let disabled = RequestStore::new(Arc::new(NoneStore), budget.reserve(0).unwrap());
        disabled.cache_product(address(), action, domain(), b"optional projection");
        assert!(disabled.lookup_product(address()).is_none());
        assert_eq!(disabled.retained_bytes(), 0);
        assert!(!disabled.exhausted());
    }
    #[test]
    fn verifier_transport_admits_its_actual_index_and_releases_all_rows() {
        use hirsel_validate::VerifierRows;
        let allowance = 64 << 10;
        let budget = ProcessBudget::new(allowance);
        let admission = Arc::new(Admission {
            memory: budget.reserve(allowance).unwrap(),
            used: AtomicU64::new(0),
            exhausted: AtomicBool::new(false),
            scratch: OnceLock::new(),
        });
        let reference = domain().reference(b"verifier-owned content").unwrap();
        let (rows, allocated) = crate::test_alloc::measured(|| {
            let rows = RequestVerifierRows {
                backend: Arc::new(NoneStore),
                admission: admission.clone(),
                rows: Mutex::new(BTreeMap::new()),
            };
            for index in 0u32..256 {
                rows.set_row(ContentHash::of_bytes(&index.to_le_bytes()), reference);
            }
            rows
        });
        assert!(!admission.exhausted.load(Ordering::Acquire));
        assert!(allocated.live > 0);
        assert!(allocated.live as u64 <= admission.used.load(Ordering::Acquire));
        for index in 0u32..256 {
            let key = ContentHash::of_bytes(&index.to_le_bytes());
            assert_eq!(rows.row(key), Some(reference));
            let before = admission.used.load(Ordering::Acquire);
            rows.set_row(key, reference);
            assert_eq!(admission.used.load(Ordering::Acquire), before);
        }
        drop(rows);
        assert_eq!(admission.used.load(Ordering::Acquire), 0);
        drop(admission);
        assert_eq!(budget.reserved(), 0);
    }
}
