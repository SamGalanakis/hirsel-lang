//! Process-global session identities (LANG-4): the append-only spelling
//! interner, the shared id assignment rule, and deterministic fan-out. Session
//! ids never reach persisted or printed output; spellings and paths do.

#[cfg(test)]
hirsel_guard::install!();

pub mod jobs;
pub mod memory;
pub mod persist;
mod salt;
mod table;
pub mod timings;
pub mod work;

pub use salt::Scramble;
#[cfg(feature = "id-salt")]
pub use salt::{salt_ids, under_salts, under_salts_with_env};
pub use table::AppendTable;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering as Atomic};
use std::sync::{PoisonError, RwLock};

/// An interned spelling. Eq and Hash compare ids; Ord compares spellings, so
/// ordered collections iterate the same whatever order spellings were interned in.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct Symbol(NonZeroU32);

/// Index shards, so concurrent interning of different spellings rarely contends.
const SHARDS: usize = 16;

struct Symbols {
    shards: [RwLock<HashMap<&'static str, Symbol, IdHasher>>; SHARDS],
    assigned: AtomicU32,
    spellings: AppendTable<&'static str>,
    scramble: Scramble,
}

static SYMBOLS: Symbols = Symbols {
    shards: [const { RwLock::new(HashMap::with_hasher(BuildHasherDefault::new())) }; SHARDS],
    assigned: AtomicU32::new(0),
    spellings: AppendTable::new(),
    scramble: Scramble::new("symbol"),
};

impl Symbol {
    /// The symbol for `text`, assigning one the first time this process sees it.
    pub fn intern(text: &str) -> Symbol {
        let mut hasher = IdHash::default();
        hasher.write(text.as_bytes());
        let shard = &SYMBOLS.shards[(hasher.finish() >> 60) as usize % SHARDS];
        if let Some(&symbol) = shard
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(text)
        {
            return symbol;
        }
        let mut index = shard.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(&symbol) = index.get(text) {
            return symbol;
        }
        let position = SYMBOLS.assigned.fetch_add(1, Atomic::Relaxed);
        let spelling: &'static str = Box::leak(text.into());
        SYMBOLS.spellings.set(position, spelling);
        let symbol = Symbol(SYMBOLS.scramble.encode(position));
        index.insert(spelling, symbol);
        symbol
    }

    /// This symbol's spelling.
    #[inline]
    pub fn as_str(self) -> &'static str {
        SYMBOLS
            .spellings
            .get(SYMBOLS.scramble.decode(self.0))
            .expect("a Symbol names a spelling interned in this process")
    }

    /// How many spellings this process has interned.
    pub fn count() -> usize {
        SYMBOLS.assigned.load(Atomic::Relaxed) as usize
    }
}

impl Ord for Symbol {
    fn cmp(&self, other: &Self) -> Ordering {
        if self == other {
            return Ordering::Equal;
        }
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for Symbol {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<&str> for Symbol {
    fn from(text: &str) -> Self {
        Symbol::intern(text)
    }
}

/// Persisted as its spelling, or as its number in a persisted string table
/// (`persist`); never as a session id.
impl serde::Serialize for Symbol {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match persist::symbol_number(*self) {
            Some(number) => s.serialize_u32(number),
            None => s.serialize_str(self.as_str()),
        }
    }
}
impl<'de> serde::Deserialize<'de> for Symbol {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if let Some(source) = persist::symbol_source() {
            let number = <u32 as serde::Deserialize>::deserialize(d)?;
            return source
                .id(number)
                .ok_or_else(|| serde::de::Error::custom("symbol number out of range"));
        }
        let text = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(d)?;
        Ok(Symbol::intern(&text))
    }
}

impl std::fmt::Debug for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Symbol").field(&self.as_str()).finish()
    }
}

impl std::fmt::Display for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The hasher for session-id keyed maps (`DefMap`) and spelling indexes.
pub type IdHasher = BuildHasherDefault<IdHash>;

/// FxHash (rustc-hash 1.1): a rotate, xor and multiply per word. Session ids
/// are dense, so this spreads them well, and it is cheap on short spellings.
#[derive(Clone, Copy, Default)]
pub struct IdHash(u64);

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl IdHash {
    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for IdHash {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (words, mut rest) = bytes.as_chunks::<8>();
        for word in words {
            self.add(u64::from_le_bytes(*word));
        }
        if let Some((half, tail)) = rest.split_first_chunk::<4>() {
            self.add(u64::from(u32::from_le_bytes(*half)));
            rest = tail;
        }
        for &byte in rest {
            self.add(u64::from(byte));
        }
    }
    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.add(u64::from(n));
    }
    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.add(u64::from(n));
    }
    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }
    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_interning_the_same_spellings_agree() {
        let spellings: Vec<String> = (0..500).map(|i| format!("intern-test-race-{i}")).collect();
        let runs: Vec<Vec<Symbol>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| spellings.iter().map(|s| Symbol::intern(s)).collect()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(runs.iter().all(|run| run == &runs[0]));
        for (symbol, spelling) in runs[0].iter().zip(&spellings) {
            assert_eq!(symbol.as_str(), spelling);
        }
    }
}
