//! The persistent result store (design §1d): a content-keyed seam with four
//! implementations the host selects by name. `none@1` stores nothing,
//! `mem@1` is shared by every session of the process, `dir@1` (the CLI
//! default) persists under the user cache directory, and `sim@1` is a
//! deterministic in-memory store with injectable faults for tests. No choice
//! changes what a program means: a hit is used only when every recorded read
//! still observes its value, so the store changes only how much is recomputed.
use crate::memo::{ReadKey, Reads};
use crate::{LoweringPolicy, QueryKind};
pub use hirsel_artifact_store::{
    Action, Address, Association, Bytes, Contents, Domain, MAX_RECORD, Output, Reference,
};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

mod dir;
mod request;
pub use dir::{DirStore, FORMAT, GcReport};
pub use request::RequestStore;

/// Stable logical address plus explicit source inputs. The address does not
/// change when the body changes; variants bind inputs and dependency outputs
/// into complete actions before a payload can be adopted.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntryKey(pub ContentHash, pub ContentHash, pub QueryKind);
impl EntryKey {
    pub fn new(
        environment: ContentHash,
        definition: &DefPath,
        text: ContentHash,
        kind: QueryKind,
        policy: LoweringPolicy,
    ) -> Self {
        Self(
            ContentHash::of_parts(
                "hirsel.result.address.v3",
                [
                    environment.0.as_slice(),
                    definition.fingerprint().0.as_slice(),
                    &[kind as u8],
                    &[policy as u8],
                ],
            ),
            text,
            kind,
        )
    }
    pub fn hex(&self) -> String {
        self.0.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// A content-keyed byte store. Implementations never fail a compile: a
/// missing, unreadable or corrupt entry is a miss.
pub trait ResultStore: Send + Sync {
    /// Retain bounded installed-product read state for one compiler operation.
    /// Dropping the guard refreshes the next operation; it never publishes or
    /// supplies validator authority. Writable transactions remain independent.
    fn read_scope(&self) -> Option<Box<dyn Send + Sync>> {
        None
    }
    /// Explicit host-local writable storage for independently owned validator
    /// associations. Ordinary product bytes and read-only roots confer no
    /// authority; only the relevant validator may issue its accepted record.
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        None
    }
    /// Local verifier capability, never imported through producer bytes. Custom
    /// and remote providers default to no authority and require first V.
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        None
    }
    /// Whether this provider can serve ordinary products. A disabled provider
    /// skips record capture and encoding as well as I/O; names are only labels.
    fn enabled(&self) -> bool {
        true
    }
    fn get(&self, key: &EntryKey) -> Option<Bytes>;
    fn put(&self, key: &EntryKey, value: &[u8]);
    /// Generic product associations share this provider with front queries.
    /// Their registered domains are independent of front QueryKind tags. An
    /// association supplies integrity only, never verifier/native authority.
    fn lookup_product(&self, address: Address) -> Option<Association> {
        if !self.enabled() {
            return None;
        }
        let bytes = self.get(&product_association_key(address))?;
        if bytes.len() > 1024 {
            return None;
        }
        let (owner, action, reference): (ContentHash, ContentHash, Reference) =
            serde_json::from_slice(&bytes).ok()?;
        (owner == address.0 && reference.len <= hirsel_artifact_store::MAX_RECORD as u64).then_some(
            Association {
                action: Action(action),
                reference,
            },
        )
    }
    /// Publish only after immutable output publication. Eviction or a failed
    /// write leaves a miss; read-only roots never receive an association.
    fn associate_product(&self, address: Address, action: Action, reference: Reference) {
        if !self.enabled() || self.get_content(reference).is_none() {
            return;
        }
        let bytes = serde_json::to_vec(&(address.0, action.0, reference))
            .expect("fixed product association serializes");
        if bytes.len() <= 1024 {
            self.put(&product_association_key(address), &bytes);
        }
    }
    /// Immutable content and logical action indexes use distinct addresses.
    /// Byte providers share their map; directory providers map content directly.
    fn get_content(&self, reference: Reference) -> Option<Bytes> {
        let bytes = self.get(&content_key(reference))?;
        (reference.domain.reference(&bytes)? == reference).then_some(bytes)
    }
    fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
        if !self.enabled() {
            return None;
        }
        let reference = domain.reference(bytes)?;
        self.put(&content_key(reference), bytes);
        Some(reference)
    }
    /// Publish a query product with its domain explicit at the provider seam.
    /// The key already binds the kind; byte-only providers share `put`.
    fn put_query(&self, kind: QueryKind, key: &EntryKey, action: Action, value: &[u8]) {
        assert_eq!(kind, key.2);
        let _ = action;
        self.put(key, value);
    }
    /// The provider name the run config selects it by (`dir@1`, ...).
    fn name(&self) -> &'static str;
}

fn product_association_key(address: Address) -> EntryKey {
    // Interpret is the byte-provider transport slot, as for content_key below;
    // it is never a semantic query kind or a scheduler dispatch conversion.
    EntryKey(
        ContentHash::of_parts("hirsel.product.association.v1", [address.0.0.as_slice()]),
        ContentHash::default(),
        QueryKind::Interpret,
    )
}

fn content_key(reference: Reference) -> EntryKey {
    EntryKey(
        reference.output.0,
        ContentHash::default(),
        QueryKind::Interpret,
    )
}
pub(crate) fn product_domain(kind: QueryKind) -> Domain {
    Domain {
        kind: kind as u32 + 16,
        schema: 8,
    }
}
/// The type records a result payload cites.
pub const TYPE_RECORDS: Domain = hirsel_product_codec::TypeRecords::DOMAIN;
/// Whether `domain` holds result content: a payload a stored row's variants
/// reference (`VariantRef::valid`), or a type record such a payload cites.
pub fn is_result_content(domain: Domain) -> bool {
    domain == TYPE_RECORDS
        || domain.schema == 8 && (16..=QueryKind::SemanticBody as u32 + 16).contains(&domain.kind)
}

/// Whether a store hit is recomputed and compared byte for byte
/// (`--verify-cache`; gate campaigns force it on). A mismatch is a tool error.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VerifyStore {
    #[default]
    Off,
    Recompute,
}

/// At most this many variants (distinct read sets) per entry.
pub const VARIANTS: usize = 4;

/// One entry: up to `VARIANTS` variants, most recently written first. Each
/// variant's reads complete its action; an immutable reference names its output.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Entry {
    pub variants: Vec<VariantRef>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Variant {
    pub inputs: ContentHash,
    pub reads: Reads,
    pub payload: Bytes,
}

pub(crate) fn payload_reads_hash(reads: &Reads) -> ContentHash {
    // Reads' in-memory ordering is by interned identity. Persisted witnesses
    // sort the canonical key bytes, so another process/salt verifies the same
    // complete set without depending on its DefId assignment order.
    // One contiguous arena retains row boundaries for the original length-
    // framed digest. Sorting offsets avoids one growable allocation per read.
    let mut bytes = Vec::new();
    let mut rows = Vec::with_capacity(reads.len());
    for (key, value) in reads.iter() {
        let start = bytes.len();
        key.encode(&mut bytes);
        bytes.extend(value.0);
        rows.push(start..bytes.len());
    }
    rows.sort_unstable_by(|a, b| bytes[a.clone()].cmp(&bytes[b.clone()]));
    ContentHash::of_parts(
        "hirsel.result.reads.v1",
        rows.iter().map(|row| &bytes[row.clone()]),
    )
}

/// The complete ordinary-query action, shared by S0 variants and already
/// settled immutable query owners. Reclaiming an evicted runtime node need
/// not depend on an optional durable index or reconstruct its body.
pub(crate) fn query_action(key: &EntryKey, inputs: ContentHash, reads: &Reads) -> Action {
    Action(ContentHash::of_parts(
        "hirsel.result.action.v1",
        [
            key.0.0.as_slice(),
            inputs.0.as_slice(),
            payload_reads_hash(reads).0.as_slice(),
        ],
    ))
}

/// An action index references immutable content without reading its pages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariantRef {
    pub inputs: ContentHash,
    pub reads: Reads,
    pub output: Reference,
    witness: ContentHash,
}
impl VariantRef {
    pub fn publish(
        store: &dyn ResultStore,
        key: &EntryKey,
        reads: Reads,
        bytes: &[u8],
    ) -> Option<Self> {
        let output = store.put_content(product_domain(key.2), bytes)?;
        let mut variant = Self {
            inputs: key.1,
            reads,
            output,
            witness: ContentHash::default(),
        };
        variant.witness = variant.binding(key);
        Some(variant)
    }
    pub(crate) fn action(&self, key: &EntryKey) -> Action {
        query_action(key, self.inputs, &self.reads)
    }
    fn binding(&self, key: &EntryKey) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.result.binding.v1",
            [
                self.action(key).0.0.as_slice(),
                self.output.output.0.0.as_slice(),
                self.output.domain.kind.to_le_bytes().as_slice(),
                self.output.domain.schema.to_le_bytes().as_slice(),
                self.output.len.to_le_bytes().as_slice(),
            ],
        )
    }
    pub(crate) fn valid(&self, key: &EntryKey) -> bool {
        self.output.domain == product_domain(key.2)
            && self.output.len <= hirsel_artifact_store::MAX_RECORD as u64
            && self.witness == self.binding(key)
    }
    pub fn resolve(&self, store: &dyn ResultStore, key: &EntryKey) -> Option<Variant> {
        if !self.valid(key) {
            return None;
        }
        self.resolve_bound(store)
    }
    // Only callers in this module that checked valid(key) can resolve here.
    // Provider content integrity and domain checks still run at get_content.
    fn resolve_bound(&self, store: &dyn ResultStore) -> Option<Variant> {
        Some(Variant {
            inputs: self.inputs,
            reads: self.reads.clone(),
            payload: store.get_content(self.output)?,
        })
    }
}

const ENTRY_MAGIC: &[u8; 4] = b"HRE5";

impl Entry {
    /// The first variant whose every read still observes its recorded value.
    pub fn hit(
        &self,
        mut current: impl FnMut(&ReadKey) -> Option<ContentHash>,
    ) -> Option<&VariantRef> {
        self.variants
            .iter()
            .find(|v| v.reads.first_changed(&mut current).is_none())
    }
    /// Check each candidate's binding once before observing any of its reads.
    /// Fetch content only for the first bound candidate whose reads are current.
    pub(crate) fn resolve_current(
        &self,
        store: &dyn ResultStore,
        key: &EntryKey,
        mut current: impl FnMut(&ReadKey) -> Option<ContentHash>,
    ) -> Option<Variant> {
        for variant in &self.variants {
            if variant.inputs == key.1
                && variant.valid(key)
                && variant.reads.first_changed(&mut current).is_none()
            {
                return variant.resolve_bound(store);
            }
        }
        None
    }

    /// Adds a variant first, replacing one with the same inputs/reads and dropping
    /// the least recently written past `VARIANTS`.
    pub fn insert(&mut self, variant: VariantRef) {
        self.variants
            .retain(|v| v.inputs != variant.inputs || v.reads != variant.reads);
        self.variants.insert(0, variant);
        self.variants.truncate(VARIANTS);
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = ENTRY_MAGIC.to_vec();
        out.push(self.variants.len() as u8);
        for v in &self.variants {
            out.extend(v.inputs.0);
            v.reads.encode(&mut out);
            out.extend(v.output.output.0.0);
            out.extend(v.output.domain.kind.to_le_bytes());
            out.extend(v.output.domain.schema.to_le_bytes());
            out.extend(v.output.len.to_le_bytes());
            out.extend(v.witness.0);
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Option<Entry> {
        Self::decode_bytes(bytes.into())
    }
    fn decode_bytes(bytes: Bytes) -> Option<Entry> {
        let mut input = bytes.strip_prefix(ENTRY_MAGIC.as_slice())?;
        let (&count, rest) = input.split_first()?;
        input = rest;
        if count as usize > VARIANTS {
            return None;
        }
        let mut variants = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let (inputs, rest) = input.split_at_checked(32)?;
            let inputs = ContentHash(inputs.try_into().ok()?);
            input = rest;
            let reads = Reads::decode(&mut input)?;
            let (reference, rest) = input.split_at_checked(80)?;
            input = rest;
            variants.push(VariantRef {
                inputs,
                reads,
                output: Reference {
                    output: Output(ContentHash(reference[..32].try_into().ok()?)),
                    domain: Domain {
                        kind: u32::from_le_bytes(reference[32..36].try_into().ok()?),
                        schema: u32::from_le_bytes(reference[36..40].try_into().ok()?),
                    },
                    len: u64::from_le_bytes(reference[40..48].try_into().ok()?),
                },
                witness: ContentHash(reference[48..80].try_into().ok()?),
            });
        }
        input.is_empty().then_some(Entry { variants })
    }
    /// Bind the complete entry (including every read) to its requested key.
    /// Rustc's cache uses an expected tag and exact length before adopting a
    /// query (rustc_middle/src/query/on_disk_cache.rs:468-485). Here the existing
    /// checksum frame additionally detects damaged reads and payload bytes.
    pub fn encode_for(&self, key: &EntryKey) -> Vec<u8> {
        let mut bytes = b"HRE5".to_vec();
        bytes.extend(key.0.0);
        bytes.extend(self.encode());
        frame(&bytes)
    }
    /// Reads `key`'s entry from `store`; an undecodable entry is a miss.
    pub fn load(store: &dyn ResultStore, key: &EntryKey) -> Entry {
        store
            .get(key)
            .and_then(|bytes| {
                let payload = unframe(&bytes)?.strip_prefix(b"HRE5")?;
                let payload = payload.strip_prefix(key.0.0.as_slice())?;
                let offset = payload.as_ptr() as usize - bytes.as_ptr() as usize;
                Entry::decode_bytes(bytes.range(offset as u64, payload.len() as u64, 1)?)
            })
            .unwrap_or_default()
    }
    pub(crate) fn action(&self, key: &EntryKey) -> Action {
        let actions: Vec<_> = self.variants.iter().map(|v| v.action(key)).collect();
        Action(ContentHash::of_parts(
            "hirsel.result.variants.v1",
            actions.iter().map(|a| a.0.0.as_slice()),
        ))
    }
}

/// `none@1`: stores nothing.
#[derive(Debug, Default)]
pub struct NoneStore;
impl ResultStore for NoneStore {
    fn enabled(&self) -> bool {
        false
    }
    fn get(&self, _: &EntryKey) -> Option<Bytes> {
        None
    }
    fn put(&self, _: &EntryKey, _: &[u8]) {}
    fn name(&self) -> &'static str {
        "none@1"
    }
}

/// `mem@1`: one map per process, shared by every session.
#[derive(Debug, Default)]
pub struct MemStore {
    entries: Mutex<HashMap<ContentHash, (EntryKey, Bytes)>>,
    verifier: Arc<hirsel_validate::VerifierCache>,
}
impl MemStore {
    /// The process's shared store.
    pub fn shared() -> Arc<MemStore> {
        static SHARED: OnceLock<Arc<MemStore>> = OnceLock::new();
        SHARED.get_or_init(Default::default).clone()
    }
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The bytes held, for `--mem-usage`.
    pub fn bytes(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|(_, v)| v.len() + std::mem::size_of::<EntryKey>())
            .sum()
    }
    /// Empties the store, returning what it held.
    pub fn take(&self) -> HashMap<EntryKey, Bytes> {
        std::mem::take(&mut *self.entries.lock().unwrap_or_else(PoisonError::into_inner))
            .into_values()
            .collect()
    }
}
impl ResultStore for MemStore {
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        Some(self.verifier.clone())
    }
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key.0)
            .map(|(_, bytes)| bytes.clone())
    }
    fn put(&self, key: &EntryKey, value: &[u8]) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.0, (*key, value.into()));
    }
    fn name(&self) -> &'static str {
        "mem@1"
    }
}

/// Faults `sim@1` injects, each by operation ordinal (0-based, counted over
/// gets and puts separately), so a test names exactly which operation fails.
#[derive(Clone, Debug, Default)]
pub struct SimFaults {
    /// Puts that are silently lost (a crash before rename).
    pub lose_puts: Vec<u64>,
    /// Gets that return a corrupted payload, which the checksum rejects.
    pub corrupt_gets: Vec<u64>,
}

/// `sim@1`: a deterministic in-memory store with an operation log and
/// injectable faults. Payloads carry the same checksum framing as `dir@1`,
/// so corruption is detected the same way.
#[derive(Debug, Default)]
pub struct SimStore {
    state: Mutex<SimState>,
}
#[derive(Debug, Default)]
struct SimState {
    entries: HashMap<ContentHash, Vec<u8>>,
    faults: SimFaults,
    gets: u64,
    puts: u64,
    log: Vec<SimOp>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SimOp {
    Hit(EntryKey),
    Miss(EntryKey),
    Corrupt(EntryKey),
    Put(EntryKey),
    Lost(EntryKey),
}
impl SimStore {
    pub fn with_faults(faults: SimFaults) -> Self {
        Self {
            state: Mutex::new(SimState {
                faults,
                ..Default::default()
            }),
        }
    }
    pub fn log(&self) -> Vec<SimOp> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .log
            .clone()
    }
    pub fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
impl ResultStore for SimStore {
    fn get(&self, key: &EntryKey) -> Option<Bytes> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let ordinal = state.gets;
        state.gets += 1;
        let Some(framed) = state.entries.get(&key.0).cloned() else {
            state.log.push(SimOp::Miss(*key));
            return None;
        };
        let mut framed = framed;
        if state.faults.corrupt_gets.contains(&ordinal)
            && let Some(last) = framed.last_mut()
        {
            *last ^= 0xff;
        }
        match unframe(&framed) {
            Some(payload) => {
                state.log.push(SimOp::Hit(*key));
                Some(payload.into())
            }
            None => {
                state.entries.remove(&key.0);
                state.log.push(SimOp::Corrupt(*key));
                None
            }
        }
    }
    fn put(&self, key: &EntryKey, value: &[u8]) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let ordinal = state.puts;
        state.puts += 1;
        if state.faults.lose_puts.contains(&ordinal) {
            state.log.push(SimOp::Lost(*key));
            return;
        }
        state.entries.insert(key.0, frame(value));
        state.log.push(SimOp::Put(*key));
    }
    fn name(&self) -> &'static str {
        "sim@1"
    }
}

const FRAME_MAGIC: &[u8; 8] = b"HIRRES01";

/// A stored payload: magic, length, SHA-256 of the payload, payload.
pub(crate) fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 48);
    out.extend(FRAME_MAGIC);
    out.extend((payload.len() as u64).to_le_bytes());
    out.extend(ContentHash::of_parts("hirsel.result.payload.v1", [payload]).0);
    out.extend(payload);
    out
}
/// The payload of a framed record, or `None` on any mismatch.
pub(crate) fn unframe(bytes: &[u8]) -> Option<&[u8]> {
    let rest = bytes.strip_prefix(FRAME_MAGIC.as_slice())?;
    let (len, rest) = rest.split_at_checked(8)?;
    let (sum, payload) = rest.split_at_checked(32)?;
    (u64::from_le_bytes(len.try_into().ok()?) as usize == payload.len()
        && ContentHash::of_parts("hirsel.result.payload.v1", [payload]).0 == sum)
        .then_some(payload)
}

/// The store a run-config provider name selects: `none@1`, `mem@1`,
/// `dir@1` (at `root`, else the default cache directory) or `sim@1`.
/// An explicit `dir@1` without a location is an error. The compiler's optional
/// environment-selected result cache treats an unavailable provider as off.
pub fn store_by_name(
    name: &str,
    root: Option<std::path::PathBuf>,
) -> Result<Arc<dyn ResultStore>, String> {
    Ok(match name {
        "none@1" => match DirStore::installed_only() {
            Some(store) => Arc::new(store),
            None => Arc::new(NoneStore),
        },
        "mem@1" => MemStore::shared(),
        "dir@1" => Arc::new(match root {
            Some(root) => DirStore::new(root).with_installed_products(),
            None => DirStore::default_location()
                .ok_or("dir@1: no cache directory (set an absolute XDG_CACHE_HOME or HOME, or select none@1)")?,
        }),
        "sim@1" => Arc::new(SimStore::default()),
        other => return Err(format!("unknown result store `{other}`")),
    })
}

#[cfg(test)]
mod tests;

/// Release shared result handles when reporting measured compiler memory.
pub fn release_shared_caches() {
    if hirsel_intern::memory::metered() {
        hirsel_intern::memory::released("result store: mem@1", MemStore::shared().take());
    }
}
