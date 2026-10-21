//! Retention and the hold budget (design §1b). Under `Retention::Batch` a
//! published definition's TypedBody and AnalyzedBody are dropped. Bodies that
//! wait on S1 after modes are *held* under `hold_bytes`; past the budget the
//! least recently used hold is evicted, and an evicted body demanded later is
//! recomputed through the ordinary memo miss path, deterministically and
//! counted.
use super::MemoKey;
use std::collections::{BTreeMap, HashMap};

/// What a session keeps after publishing a definition.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Retention {
    /// Drop TypedBody and AnalyzedBody on publication (the CLI).
    #[default]
    Batch,
    /// Keep everything (resident sessions).
    Resident,
}

/// The default hold budget: min(256 MiB, 10% of the memory cap).
pub fn default_hold_bytes(memory_cap: Option<u64>) -> u64 {
    let budget = 256 << 20;
    memory_cap.map_or(budget, |cap| budget.min(cap / 10))
}

/// Hold counters for `--mem-usage` and G4.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HoldStats {
    pub held: u64,
    pub bytes: u64,
    pub peak_bytes: u64,
    pub evictions: u64,
    /// Evicted holds demanded again (each is one recompute through the miss path).
    pub recomputes: u64,
}

/// Bodies held under a byte budget, evicted least recently used first. The
/// eviction order depends only on the order of `hold` and `take` calls,
/// which the serial S1 coordinator makes in key order.
pub struct Holds<T> {
    budget: u64,
    tick: u64,
    entries: HashMap<MemoKey, (T, u64, u64)>,
    by_use: BTreeMap<u64, MemoKey>,
    evicted: std::collections::HashSet<MemoKey>,
    stats: HoldStats,
}

impl<T> Holds<T> {
    pub fn new(budget: u64) -> Self {
        Self {
            budget,
            tick: 0,
            entries: HashMap::new(),
            by_use: BTreeMap::new(),
            evicted: Default::default(),
            stats: HoldStats::default(),
        }
    }
    pub fn stats(&self) -> HoldStats {
        self.stats
    }
    pub fn budget(&self) -> u64 {
        self.budget
    }
    /// Holds `value` of `bytes`, then evicts least recently used holds until
    /// within budget, including evicting this value if it alone exceeds the
    /// budget. Active callers own their in-flight values separately. Returns
    /// the evicted keys.
    pub fn hold(&mut self, key: MemoKey, value: T, bytes: u64) -> Vec<MemoKey> {
        self.release(&key);
        self.tick += 1;
        self.entries.insert(key, (value, bytes, self.tick));
        self.by_use.insert(self.tick, key);
        self.stats.held += 1;
        self.stats.bytes += bytes;
        self.stats.peak_bytes = self.stats.peak_bytes.max(self.stats.bytes);
        let mut evicted = vec![];
        while self.stats.bytes > self.budget {
            let (&tick, &victim) = self.by_use.iter().next().unwrap();
            self.by_use.remove(&tick);
            let (_, size, _) = self.entries.remove(&victim).unwrap();
            self.stats.bytes -= size;
            self.stats.evictions += 1;
            self.evicted.insert(victim);
            evicted.push(victim);
        }
        evicted
    }
    /// Retire a completed root's holds and eviction markers, preserving
    /// cumulative insertion, eviction, recompute and peak occupancy counters.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.by_use.clear();
        self.evicted.clear();
        self.stats.bytes = 0;
        self.tick = 0;
    }
    /// Uses a held body, refreshing its recency.
    pub fn get(&mut self, key: &MemoKey) -> Option<&T> {
        let (_, _, tick) = self.entries.get_mut(key)?;
        self.by_use.remove(tick);
        self.tick += 1;
        *tick = self.tick;
        self.by_use.insert(self.tick, *key);
        self.entries.get(key).map(|e| &e.0)
    }
    /// Removes and returns a hold whose work items settled (drain as you go).
    pub fn take(&mut self, key: &MemoKey) -> Option<T> {
        let (value, size, tick) = self.entries.remove(key)?;
        self.by_use.remove(&tick);
        self.stats.bytes -= size;
        Some(value)
    }
    /// Whether `key` was evicted and has not been recomputed since; a demand
    /// for it counts one recompute.
    pub fn demand_evicted(&mut self, key: &MemoKey) -> bool {
        let was = self.evicted.remove(key);
        if was {
            self.stats.recomputes += 1;
        }
        was
    }
    fn release(&mut self, key: &MemoKey) {
        self.take(key);
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryKind;
    use hirsel_syntax::{DefId, DefPath, ItemPath, PackageId};
    fn key(i: u32) -> MemoKey {
        MemoKey::new(
            QueryKind::Modes,
            DefId::intern(&DefPath(ItemPath {
                package: PackageId {
                    name: "h".into(),
                    version: "1".into(),
                },
                modules: vec![],
                name: format!("d{i}"),
            })),
        )
    }

    #[test]
    fn holds_stay_within_budget_evicting_least_recently_used() {
        let mut holds = Holds::new(300);
        assert!(holds.hold(key(0), "a", 100).is_empty());
        assert!(holds.hold(key(1), "b", 100).is_empty());
        assert!(holds.hold(key(2), "c", 100).is_empty());
        assert_eq!(holds.get(&key(0)), Some(&"a"), "refreshes key 0");
        assert_eq!(holds.hold(key(3), "d", 100), vec![key(1)]);
        assert_eq!(holds.stats().bytes, 300);
        assert_eq!(holds.hold(key(4), "e", 250), vec![key(2), key(0), key(3)]);
        assert_eq!(holds.stats().evictions, 4);
        assert_eq!(holds.stats().peak_bytes, 550);
        assert!(holds.demand_evicted(&key(1)));
        assert!(!holds.demand_evicted(&key(1)), "counted once");
        assert_eq!(holds.stats().recomputes, 1);
        assert_eq!(holds.take(&key(4)), Some("e"));
        assert!(holds.is_empty());
        assert_eq!(holds.stats().bytes, 0);
    }

    #[test]
    fn zero_and_oversized_holds_release_cache_ownership_immediately() {
        use std::sync::Arc;
        for budget in [0, 10] {
            let mut holds = Holds::new(budget);
            let active = Arc::new(0);
            let weak = Arc::downgrade(&active);
            assert_eq!(holds.hold(key(0), active.clone(), 50), vec![key(0)]);
            assert_eq!(holds.stats().bytes, 0);
            assert!(holds.is_empty());
            assert_eq!(
                Arc::strong_count(&active),
                1,
                "only the active caller retains it"
            );
            drop(active);
            assert!(weak.upgrade().is_none());
            assert!(holds.demand_evicted(&key(0)));
        }
        assert_eq!(default_hold_bytes(None), 256 << 20);
        assert_eq!(default_hold_bytes(Some(1 << 30)), (1 << 30) / 10);
    }

    #[test]
    fn clearing_roots_preserves_counters_and_retires_eviction_markers() {
        let mut holds = Holds::new(10);
        holds.hold(key(0), 0, 7);
        holds.hold(key(1), 1, 7);
        assert!(holds.demand_evicted(&key(0)));
        holds.hold(key(2), 2, 7);
        let before = holds.stats();
        holds.clear();
        let after = holds.stats();
        assert_eq!(after.bytes, 0);
        assert!(holds.is_empty());
        assert_eq!(after.held, before.held);
        assert_eq!(after.evictions, before.evictions);
        assert_eq!(after.recomputes, before.recomputes);
        assert_eq!(after.peak_bytes, before.peak_bytes);
        assert!(
            !holds.demand_evicted(&key(1)),
            "new roots have no stale eviction markers"
        );
        assert!(holds.hold(key(0), 3, 7).is_empty());
        assert_eq!(holds.stats().bytes, 7);
    }
}
