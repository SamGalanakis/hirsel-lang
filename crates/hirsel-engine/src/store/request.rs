//! One admitted request's transport over its selected best-effort durable store.
//! This changes physical retention only, never semantic query invalidation or
//! verifier authority. All producers receive the same Arc<RequestStore>.
use super::*;
use crate::runtime::MemoryReservation;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

// Bounds payload ownership, Arc/region headers and BTreeMap allocation overhead.
const ENTRY_CHARGE: u64 = 2048;
struct State {
    used: u64,
    contents: BTreeMap<ContentHash, (Reference, Bytes)>,
    associations: BTreeMap<ContentHash, Association>,
}
pub struct RequestStore {
    backend: Arc<dyn ResultStore>,
    memory: Arc<MemoryReservation>,
    state: Mutex<State>,
    exhausted: AtomicBool,
}
impl RequestStore {
    pub fn new(backend: Arc<dyn ResultStore>, memory: MemoryReservation) -> Self {
        Self {
            backend,
            memory: Arc::new(memory),
            state: Mutex::new(State {
                used: 0,
                contents: BTreeMap::new(),
                associations: BTreeMap::new(),
            }),
            exhausted: AtomicBool::new(false),
        }
    }
    /// The native task reports the existing deterministic resource failure
    /// before settling a product when its admitted transport allowance is full.
    pub fn exhausted(&self) -> bool {
        self.exhausted.load(Ordering::Acquire)
    }
    pub fn retained_bytes(&self) -> u64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).used
    }
    fn charge(&self, state: &mut State, bytes: u64) -> bool {
        if bytes > self.memory.bytes().saturating_sub(state.used) {
            self.exhausted.store(true, Ordering::Release);
            return false;
        }
        state.used += bytes;
        true
    }
}
impl ResultStore for RequestStore {
    fn name(&self) -> &'static str {
        self.backend.name()
    }
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.backend.local_product_store()
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.backend.verifier()
    }
    // Transient transport is enabled even if the selected durable policy is off.
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        self.backend.get(key)
    }
    fn put(&self, key: &EntryKey, value: &[u8]) {
        self.backend.put(key, value)
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
        self.backend.associate_product(address, action, reference);
        let association = Association { action, reference };
        let durable = self.backend.lookup_product(address) == Some(association)
            && self.backend.get_content(reference).is_some();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if durable {
            state.associations.remove(&address.0);
            // Charges remain monotone for this request, including external
            // Bytes views. No eviction can spend a live mapping's allowance.
        } else if state.associations.contains_key(&address.0)
            || self.charge(&mut state, ENTRY_CHARGE)
        {
            state.associations.insert(address.0, association);
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
}
