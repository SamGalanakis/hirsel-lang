//! Host-owned transient products shared by one Session's source revisions while
//! no durable store is selected. Hits are ordinary ResultStore variants, so the
//! engine adopts one only when every recorded read still validates; this store
//! changes how much is recomputed, never what a program means.
use hirsel_engine::{
    runtime::{MemoryReservation, ProcessBudget},
    store::{Bytes, Contents, Domain, EntryKey, Reference, ResultStore},
};
use hirsel_syntax::ContentHash;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex, PoisonError},
};

/// The payload region and Bytes headers beside each payload, owned by its bytes.
const PAYLOAD_CHARGE: u64 = 128;
/// One HashMap slot plus one BTreeMap order entry. Evicting an entry leaves
/// both maps' allocations behind, so slots stay charged at their high water.
const SLOT_CHARGE: u64 = 192;

/// Disjoint key spaces: ResultStore get/put never reach verifier rows.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Key {
    Result(ContentHash),
    Content(ContentHash),
    VerifierRow(ContentHash),
    VerifierContent(ContentHash),
}
enum Value {
    /// Retains its reservation through every clone and range a get returns,
    /// so an evicted payload stays charged until its last reader drops it.
    Bytes(Bytes),
    Row(Reference, MemoryReservation),
}
/// What a full store evicts first: entries outside the latest demand, then
/// the rest, each least recently used first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tier {
    Other,
    /// A demanded row or content one of its variants names. Each demand
    /// replaces the last.
    Demanded,
    /// Type records, which every payload cites and which stay small.
    Shared,
}
struct Entry {
    value: Value,
    charge: u64,
    stamp: u64,
    tier: Tier,
}
impl Entry {
    fn rank(&self) -> (bool, u64) {
        (self.tier != Tier::Other, self.stamp)
    }
}
#[derive(Default)]
struct State {
    entries: HashMap<Key, Entry>,
    /// Ranks, first evicted first.
    order: BTreeMap<(bool, u64), Key>,
    /// Result keys of the latest demand.
    demanded: HashSet<ContentHash>,
    clock: u64,
    /// Charges of the entries this store still retains.
    used: u64,
    /// Doubling slot reservations, held until the store drops.
    slots: Vec<MemoryReservation>,
    slot_count: u64,
}
impl State {
    fn remove(&mut self, key: &Key) {
        if let Some(entry) = self.entries.remove(key) {
            self.order.remove(&entry.rank());
            self.used -= entry.charge;
        }
    }
    fn set_tier(&mut self, key: Key, tier: Tier) {
        let Some(entry) = self.entries.get_mut(&key) else {
            return;
        };
        self.order.remove(&entry.rank());
        entry.tier = tier;
        self.order.insert(entry.rank(), key);
    }
    /// Keep a demanded row and the content its variants name.
    fn keep(&mut self, key: &EntryKey) {
        let Some(Value::Bytes(row)) = self.entries.get(&Key::Result(key.0)).map(|e| &e.value)
        else {
            return;
        };
        let cited = hirsel_engine::store::Entry::decode_for(key, row)
            .map(|row| row.variants)
            .unwrap_or_default();
        self.set_tier(Key::Result(key.0), Tier::Demanded);
        for variant in cited {
            self.set_tier(Key::Content(variant.output.output.0), Tier::Demanded);
        }
    }
    fn slot_bytes(&self) -> u64 {
        self.slot_count * SLOT_CHARGE
    }
}

/// Entries within `capacity` bytes, slots included, each charged to the
/// shared process budget and evicted by tier, then least recently used. A put
/// that does not fit or that the budget refuses is dropped.
struct Bounded {
    budget: Arc<ProcessBudget>,
    capacity: u64,
    state: Mutex<State>,
}
impl Bounded {
    /// Read one entry and mark it most recently used.
    fn read<T>(&self, key: Key, read: impl FnOnce(&Value) -> Option<T>) -> Option<T> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let previous = state.entries.get(&key)?.rank();
        state.order.remove(&previous);
        state.clock += 1;
        let stamp = state.clock;
        let entry = state.entries.get_mut(&key)?;
        entry.stamp = stamp;
        let rank = entry.rank();
        state.order.insert(rank, key);
        read(&state.entries.get(&key)?.value)
    }
    fn put(&self, key: Key, value: impl FnOnce(MemoryReservation) -> Value, len: u64, tier: Tier) {
        let charge = len + PAYLOAD_CHARGE;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        // An entry that cannot fit even alone never evicts the others.
        if charge + state.slot_count.max(16) * SLOT_CHARGE > self.capacity {
            return;
        }
        state.remove(&key);
        // Evict until this entry and the slots it needs fit, then commit growth.
        let slots = loop {
            let len = state.entries.len() as u64;
            let slots = if len >= state.slot_count {
                (2 * state.slot_count).max(16)
            } else {
                state.slot_count
            };
            if state.used + charge + slots * SLOT_CHARGE <= self.capacity {
                break slots;
            }
            let Some(oldest) = state.order.first_key_value().map(|(_, key)| *key) else {
                return;
            };
            state.remove(&oldest);
        };
        if slots > state.slot_count {
            let Ok(grown) = self
                .budget
                .reserve((slots - state.slot_count) * SLOT_CHARGE)
            else {
                return;
            };
            state.slots.push(grown);
            state.slot_count = slots;
        }
        let Ok(memory) = self.budget.reserve(charge) else {
            return;
        };
        state.used += charge;
        state.clock += 1;
        let entry = Entry {
            value: value(memory),
            charge,
            stamp: state.clock,
            tier,
        };
        state.order.insert(entry.rank(), key);
        state.entries.insert(key, entry);
    }
    fn get_bytes(&self, key: Key) -> Option<Bytes> {
        self.read(key, |value| match value {
            Value::Bytes(bytes) => Some(bytes.clone()),
            Value::Row(..) => None,
        })
    }
    fn put_bytes(&self, key: Key, bytes: &[u8], tier: Tier) {
        self.put(
            key,
            |memory| Value::Bytes(Bytes::from(bytes).retain_owner(Arc::new(memory))),
            bytes.len() as u64,
            tier,
        );
    }
}
impl Contents for Bounded {
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        let bytes = self.get_bytes(Key::VerifierContent(reference.output.0))?;
        (reference.domain.reference(&bytes)? == reference).then_some(bytes)
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        let reference = domain.reference(bytes)?;
        self.put_bytes(Key::VerifierContent(reference.output.0), bytes, Tier::Other);
        Some(reference)
    }
}
impl hirsel_validate::VerifierRows for Bounded {
    fn row(&self, key: ContentHash) -> Option<Reference> {
        self.read(Key::VerifierRow(key), |value| match value {
            Value::Row(reference, _) => Some(*reference),
            Value::Bytes(_) => None,
        })
    }
    fn set_row(&self, key: ContentHash, reference: Reference) {
        self.put(
            Key::VerifierRow(key),
            |memory| Value::Row(reference, memory),
            std::mem::size_of::<Reference>() as u64,
            Tier::Other,
        );
    }
}

/// `resident@1`: products and the local verifier's rows share one bound.
pub struct ResidentStore {
    bounded: Arc<Bounded>,
    verifier: Arc<hirsel_validate::VerifierCache>,
}
impl ResidentStore {
    pub fn new(budget: Arc<ProcessBudget>, capacity: u64) -> Self {
        let bounded = Arc::new(Bounded {
            budget,
            capacity,
            state: Mutex::new(State::default()),
        });
        Self {
            verifier: Arc::new(hirsel_validate::VerifierCache::hosted(bounded.clone())),
            bounded,
        }
    }
    /// Bytes this store retains: its entries and slot high water.
    pub fn used(&self) -> u64 {
        let state = self
            .bounded
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.used + state.slot_bytes()
    }
}
impl ResultStore for ResidentStore {
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        Some(self.verifier.clone())
    }
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        self.bounded.get_bytes(Key::Result(key.0))
    }
    fn put(&self, key: &EntryKey, value: &[u8]) {
        self.bounded
            .put_bytes(Key::Result(key.0), value, Tier::Other);
        let mut state = self
            .bounded
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.demanded.contains(&key.0) {
            state.keep(key);
        }
    }
    /// The products a session's next revision is likeliest to adopt survive
    /// a request that publishes more than this bound holds. Each demand names
    /// the request's root package rows, which replace the last demand's.
    fn demand(&self, keys: &[EntryKey]) {
        let mut state = self
            .bounded
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let demoted: Vec<Key> = state
            .entries
            .iter()
            .filter(|(_, entry)| entry.tier == Tier::Demanded)
            .map(|(key, _)| *key)
            .collect();
        for key in demoted {
            state.set_tier(key, Tier::Other);
        }
        state.demanded = keys.iter().map(|key| key.0).collect();
        for key in keys {
            state.keep(key);
        }
    }
    /// Retained best-effort for later revisions, but never claimed: a request
    /// keeps its own transport copy, since this bound may drop or evict
    /// content that the request has still to read. Only result content is
    /// retained: rows reference nothing else, and every revision recaptures
    /// its source inputs and staged records, which would evict the rows the
    /// next revision adopts.
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        let reference = domain.reference(bytes)?;
        if hirsel_engine::store::is_result_content(domain) {
            let tier = if domain == hirsel_engine::store::TYPE_RECORDS {
                Tier::Shared
            } else {
                Tier::Other
            };
            self.bounded
                .put_bytes(Key::Content(reference.output.0), bytes, tier);
        }
        None
    }
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        let bytes = self.bounded.get_bytes(Key::Content(reference.output.0))?;
        (reference.domain.reference(&bytes)? == reference).then_some(bytes)
    }
    fn name(&self) -> &'static str {
        "resident@1"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_engine::QueryKind;
    use hirsel_validate::VerifierRows;

    fn key(byte: u8) -> EntryKey {
        let hash = ContentHash::of_parts("resident-store-test", [&[byte][..]]);
        EntryKey(hash, hash, QueryKind::Check)
    }
    const SLOTS: u64 = 16 * SLOT_CHARGE;
    const ENTRY: u64 = 100 + PAYLOAD_CHARGE;

    #[test]
    fn resident_store_evicts_least_recent_within_capacity_and_releases_its_charge() {
        let budget = ProcessBudget::new(1 << 20);
        let store = ResidentStore::new(budget.clone(), SLOTS + 2 * ENTRY);
        store.put(&key(1), &[1; 100]);
        store.put(&key(2), &[2; 100]);
        assert!(store.get(&key(1)).is_some());
        store.put(&key(3), &[3; 100]);
        assert!(
            store.get(&key(2)).is_none(),
            "least recently used entry is evicted"
        );
        assert_eq!(&store.get(&key(1)).unwrap()[..], &[1; 100]);
        assert_eq!(&store.get(&key(3)).unwrap()[..], &[3; 100]);
        assert_eq!(store.used(), SLOTS + 2 * ENTRY);
        assert_eq!(budget.reserved(), store.used());
        store.put(&key(4), &[4; 1 << 10]);
        assert!(
            store.get(&key(4)).is_none(),
            "an entry beyond capacity is dropped"
        );
        drop(store);
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn resident_store_keeps_an_evicted_payload_charged_until_its_reader_drops_it() {
        let budget = ProcessBudget::new(1 << 20);
        let store = ResidentStore::new(budget.clone(), SLOTS + ENTRY);
        store.put(&key(1), &[1; 100]);
        let held = store.get(&key(1)).unwrap();
        store.put(&key(2), &[2; 100]);
        assert!(store.get(&key(1)).is_none());
        assert_eq!(store.used(), SLOTS + ENTRY);
        assert_eq!(
            budget.reserved(),
            SLOTS + 2 * ENTRY,
            "the held payload stays charged"
        );
        drop(store);
        assert_eq!(budget.reserved(), ENTRY);
        assert_eq!(&held[..], &[1; 100]);
        drop(held);
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn resident_store_keeps_demanded_rows_and_their_content_until_the_next_demand() {
        use hirsel_engine::store::{Entry, RequestStore, VariantRef};
        let budget = ProcessBudget::new(1 << 20);
        let resident = Arc::new(ResidentStore::new(budget.clone(), 2 * SLOTS + 16 * ENTRY));
        let request = RequestStore::new(resident.clone(), budget.reserve(1 << 16).unwrap());
        let demanded = key(0);
        request.demand(std::slice::from_ref(&demanded));
        let variant = VariantRef::publish(
            &request,
            &demanded,
            hirsel_engine::Reads::empty(),
            &[7; 100],
        )
        .unwrap();
        let mut row = Entry::default();
        row.insert(variant.clone());
        request.put(&demanded, &row.encode_for(&demanded));
        let flood = |store: &ResidentStore| {
            for byte in 1..=64 {
                store.put(&key(byte), &[byte; 100]);
            }
        };
        flood(&resident);
        assert!(resident.get(&key(1)).is_none(), "others are evicted");
        assert!(resident.get(&demanded).is_some());
        assert_eq!(
            &resident.get_content(variant.output).unwrap()[..],
            &[7; 100]
        );
        resident.demand(&[]);
        flood(&resident);
        assert!(
            resident.get(&demanded).is_none(),
            "a later demand releases it"
        );
        assert!(resident.get_content(variant.output).is_none());
    }

    #[test]
    fn resident_store_retains_result_content_but_not_request_transports() {
        let budget = ProcessBudget::new(1 << 20);
        let store = ResidentStore::new(budget.clone(), 1 << 16);
        let product = Domain {
            kind: QueryKind::Modes as u32 + 16,
            schema: 8,
        };
        let types = hirsel_engine::store::TYPE_RECORDS;
        let source_input = Domain {
            kind: 0x4600_0013,
            schema: 1,
        };
        assert_eq!(store.put_content(product, b"modes"), None);
        assert_eq!(store.put_content(types, b"type"), None);
        assert_eq!(store.put_content(source_input, b"syntax"), None);
        let product = product.reference(b"modes").unwrap();
        assert_eq!(&store.get_content(product).unwrap()[..], b"modes");
        let types = types.reference(b"type").unwrap();
        assert_eq!(&store.get_content(types).unwrap()[..], b"type");
        assert!(
            store
                .get_content(source_input.reference(b"syntax").unwrap())
                .is_none()
        );
        assert_eq!(store.used(), SLOTS + 9 + 2 * PAYLOAD_CHARGE);
        assert_eq!(budget.reserved(), store.used());
    }

    #[test]
    fn resident_store_never_charges_slots_beyond_its_capacity() {
        let budget = ProcessBudget::new(1 << 20);
        let store = ResidentStore::new(budget.clone(), SLOTS - 1);
        store.put(&key(1), &[1; 10]);
        assert!(store.get(&key(1)).is_none());
        assert_eq!((store.used(), budget.reserved()), (0, 0));
        let refused = ProcessBudget::new(SLOTS - 1);
        let store = ResidentStore::new(refused.clone(), 1 << 20);
        store.put(&key(1), &[1; 10]);
        assert_eq!((store.used(), refused.reserved()), (0, 0));
    }

    #[test]
    fn resident_store_drops_puts_the_process_budget_refuses() {
        let budget = ProcessBudget::new(SLOTS + 10 + PAYLOAD_CHARGE);
        let store = ResidentStore::new(budget.clone(), 1 << 20);
        store.put(&key(1), &[1; 10]);
        store.put(&key(2), &[2; 10]);
        assert!(store.get(&key(1)).is_some());
        assert!(store.get(&key(2)).is_none());
        assert_eq!(budget.reserved(), SLOTS + 10 + PAYLOAD_CHARGE);
    }

    #[test]
    fn resident_store_retains_product_content_without_claiming_request_transport() {
        let budget = ProcessBudget::new(1 << 20);
        let store = ResidentStore::new(budget.clone(), 1 << 16);
        let domain = Domain {
            kind: QueryKind::Check as u32 + 16,
            schema: 8,
        };
        assert_eq!(store.put_content(domain, b"product"), None);
        let reference = domain.reference(b"product").unwrap();
        assert_eq!(&store.get_content(reference).unwrap()[..], b"product");
        let request = hirsel_engine::store::RequestStore::new(
            Arc::new(store),
            budget.reserve(1 << 12).unwrap(),
        );
        let fresh = request.put_content(domain, b"in flight").unwrap();
        assert_eq!(&request.get_content(fresh).unwrap()[..], b"in flight");
    }

    #[test]
    fn resident_verifier_rows_share_the_bound_and_product_keys_cannot_reach_them() {
        let budget = ProcessBudget::new(1 << 20);
        let store = ResidentStore::new(budget.clone(), 1 << 16);
        let domain = Domain { kind: 1, schema: 1 };
        let reference = store.bounded.put_content(domain, b"receipt").unwrap();
        let row = key(9).0;
        store.bounded.set_row(row, reference);
        assert_eq!(store.bounded.row(row), Some(reference));
        assert_eq!(
            &store.bounded.get_content(reference).unwrap()[..],
            b"receipt"
        );
        assert!(store.get(&EntryKey(row, row, QueryKind::Check)).is_none());
        assert_eq!(budget.reserved(), store.used());
        drop(store);
        assert_eq!(budget.reserved(), 0);
    }
}
