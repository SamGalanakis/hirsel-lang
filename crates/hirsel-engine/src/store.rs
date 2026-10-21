//! The persistent result store (design §1d): a content-keyed seam with four
//! implementations the host selects by name. `none@1` stores nothing,
//! `mem@1` is shared by every session of the process, `dir@1` (the CLI
//! default) persists under the user cache directory, and `sim@1` is a
//! deterministic in-memory store with injectable faults for tests. No choice
//! changes what a program means: a hit is used only when every recorded read
//! still observes its value, so the store changes only how much is recomputed.
use crate::memo::{ReadKey, Reads};
use crate::{LoweringPolicy, QueryKind};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

mod dir;
pub use dir::{DirStore, GcReport};

/// `H("hirsel.result.v2", environment hash, DefPath, item text hash,
/// QueryKind, LoweringPolicy)`: a DefinitionKey without its reads.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntryKey(pub ContentHash);
impl EntryKey {
    pub fn new(
        environment: ContentHash,
        definition: &DefPath,
        text: ContentHash,
        kind: QueryKind,
        policy: LoweringPolicy,
    ) -> Self {
        Self(ContentHash::of_parts(
            "hirsel.result.v2",
            [
                environment.0.as_slice(),
                definition.fingerprint().0.as_slice(),
                text.0.as_slice(),
                &[kind as u8],
                &[policy as u8],
            ],
        ))
    }
    pub fn hex(&self) -> String {
        self.0.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// A content-keyed byte store. Implementations never fail a compile: a
/// missing, unreadable or corrupt entry is a miss.
pub trait ResultStore: Send + Sync {
    /// Whether this provider can retain results. An off provider disables
    /// record capture and encoding as well as I/O; names are only labels.
    fn enabled(&self) -> bool {
        true
    }
    fn get(&self, key: &EntryKey) -> Option<Arc<[u8]>>;
    fn put(&self, key: &EntryKey, value: &[u8]);
    /// The provider name the run config selects it by (`dir@1`, ...).
    fn name(&self) -> &'static str;
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
/// variant's reads complete the DefinitionKey; its payload is the outputs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Entry {
    pub variants: Vec<Variant>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Variant {
    pub reads: Reads,
    pub payload: Arc<[u8]>,
}

const ENTRY_MAGIC: &[u8; 4] = b"HRE2";

impl Entry {
    /// The first variant whose every read still observes its recorded value.
    pub fn hit(
        &self,
        mut current: impl FnMut(&ReadKey) -> Option<ContentHash>,
    ) -> Option<&Variant> {
        self.variants
            .iter()
            .find(|v| v.reads.first_changed(&mut current).is_none())
    }
    /// Adds a variant first, replacing one with the same reads and dropping
    /// the least recently written past `VARIANTS`.
    pub fn insert(&mut self, variant: Variant) {
        self.variants.retain(|v| v.reads != variant.reads);
        self.variants.insert(0, variant);
        self.variants.truncate(VARIANTS);
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = ENTRY_MAGIC.to_vec();
        out.push(self.variants.len() as u8);
        for v in &self.variants {
            v.reads.encode(&mut out);
            out.extend((v.payload.len() as u64).to_le_bytes());
            out.extend(v.payload.iter());
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Option<Entry> {
        let mut input = bytes.strip_prefix(ENTRY_MAGIC.as_slice())?;
        let (&count, rest) = input.split_first()?;
        input = rest;
        if count as usize > VARIANTS {
            return None;
        }
        let mut variants = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let reads = Reads::decode(&mut input)?;
            let (len, rest) = input.split_at_checked(8)?;
            let len = u64::from_le_bytes(len.try_into().ok()?) as usize;
            let (payload, rest) = rest.split_at_checked(len)?;
            input = rest;
            variants.push(Variant {
                reads,
                payload: payload.into(),
            });
        }
        input.is_empty().then_some(Entry { variants })
    }
    /// Reads `key`'s entry from `store`; an undecodable entry is a miss.
    pub fn load(store: &dyn ResultStore, key: &EntryKey) -> Entry {
        store
            .get(key)
            .and_then(|bytes| Entry::decode(&bytes))
            .unwrap_or_default()
    }
}

/// `none@1`: stores nothing.
#[derive(Debug, Default)]
pub struct NoneStore;
impl ResultStore for NoneStore {
    fn enabled(&self) -> bool {
        false
    }
    fn get(&self, _: &EntryKey) -> Option<Arc<[u8]>> {
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
    entries: Mutex<HashMap<EntryKey, Arc<[u8]>>>,
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
            .map(|v| v.len() + std::mem::size_of::<EntryKey>())
            .sum()
    }
    /// Empties the store, returning what it held.
    pub fn take(&self) -> HashMap<EntryKey, Arc<[u8]>> {
        std::mem::take(&mut *self.entries.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
impl ResultStore for MemStore {
    fn get(&self, key: &EntryKey) -> Option<Arc<[u8]>> {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }
    fn put(&self, key: &EntryKey, value: &[u8]) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*key, value.into());
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
    entries: HashMap<EntryKey, Vec<u8>>,
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
    fn get(&self, key: &EntryKey) -> Option<Arc<[u8]>> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let ordinal = state.gets;
        state.gets += 1;
        let Some(framed) = state.entries.get(key).cloned() else {
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
                state.entries.remove(key);
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
        state.entries.insert(*key, frame(value));
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
        "none@1" => Arc::new(NoneStore),
        "mem@1" => MemStore::shared(),
        "dir@1" => Arc::new(match root {
            Some(root) => DirStore::new(root),
            None => DirStore::default_location()
                .ok_or("dir@1: no cache directory (set an absolute XDG_CACHE_HOME or HOME, or select none@1)")?,
        }),
        "sim@1" => Arc::new(SimStore::default()),
        other => return Err(format!("unknown result store `{other}`")),
    })
}

#[cfg(test)]
mod tests;
