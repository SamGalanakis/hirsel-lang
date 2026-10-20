//! Constant-time tables keyed by a run's `u64` identities: Io handles (`handles.rs`), the host
//! providers' per-handle state and the cooperative scheduler's wait tokens.
//!
//! Handles and tokens are allocated in increasing order and never reused, and handle numbers
//! are recorded on tapes (DET-1), so the key keeps its value and the table hashes it: a
//! Fibonacci multiply keeps consecutive keys in distinct buckets and fills the high bits the
//! SwissTable group tag reads. A lookup costs the same whatever the number of live rows. The
//! iteration order of a hash table is not part of any outcome: [`IdMap::sorted`] is the only
//! ordered walk, and [`IdMap::iter_unordered`] serves callers whose result is
//! order-independent (a filter or a count).
use std::collections::{BTreeMap, HashMap, hash_map};
use std::hash::{BuildHasherDefault, Hasher};

/// The hasher of a `u64` identity: `key * 2^64/φ`.
#[derive(Clone, Copy, Default)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("an IdHasher hashes u64 identities only")
    }
    fn write_u64(&mut self, key: u64) {
        self.0 = key.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

/// A table keyed by `u64` identities.
#[derive(Clone)]
pub struct IdMap<V>(HashMap<u64, V, BuildHasherDefault<IdHasher>>);

impl<V> Default for IdMap<V> {
    fn default() -> Self {
        Self(HashMap::default())
    }
}

impl<V: PartialEq> PartialEq for IdMap<V> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<V: Eq> Eq for IdMap<V> {}

impl<V: std::fmt::Debug> std::fmt::Debug for IdMap<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.sorted()).finish()
    }
}

impl<V> IdMap<V> {
    pub fn get(&self, key: &u64) -> Option<&V> {
        self.0.get(key)
    }
    pub fn get_mut(&mut self, key: &u64) -> Option<&mut V> {
        self.0.get_mut(key)
    }
    pub fn insert(&mut self, key: u64, value: V) -> Option<V> {
        self.0.insert(key, value)
    }
    pub fn remove(&mut self, key: &u64) -> Option<V> {
        self.0.remove(key)
    }
    pub fn contains_key(&self, key: &u64) -> bool {
        self.0.contains_key(key)
    }
    pub fn entry(&mut self, key: u64) -> hash_map::Entry<'_, u64, V> {
        self.0.entry(key)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// The rows in key order.
    pub fn sorted(&self) -> Vec<(u64, &V)> {
        let mut rows: Vec<_> = self.0.iter().map(|(k, v)| (*k, v)).collect();
        rows.sort_unstable_by_key(|(k, _)| *k);
        rows
    }
    /// The rows as an ordered map, for observers and tests.
    pub fn to_btree(&self) -> BTreeMap<u64, V>
    where
        V: Clone,
    {
        self.0.iter().map(|(k, v)| (*k, v.clone())).collect()
    }
    /// The rows in no particular order, for an order-independent result.
    pub fn iter_unordered(&self) -> impl Iterator<Item = (u64, &V)> {
        self.0.iter().map(|(k, v)| (*k, v))
    }
    /// The values in no particular order, for an order-independent result.
    pub fn values_unordered(&self) -> impl Iterator<Item = &V> {
        self.0.values()
    }
}

impl<V> std::ops::Index<&u64> for IdMap<V> {
    type Output = V;
    fn index(&self, key: &u64) -> &V {
        &self.0[key]
    }
}

impl<V> FromIterator<(u64, V)> for IdMap<V> {
    fn from_iter<I: IntoIterator<Item = (u64, V)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// A set of `u64` identities.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdSet(IdMap<()>);

impl IdSet {
    pub fn insert(&mut self, key: u64) -> bool {
        self.0.insert(key, ()).is_none()
    }
    pub fn remove(&mut self, key: &u64) -> bool {
        self.0.remove(key).is_some()
    }
    pub fn contains(&self, key: &u64) -> bool {
        self.0.contains_key(key)
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// The members in no particular order, for an order-independent result.
    pub fn members_unordered(&self) -> impl Iterator<Item = u64> + '_ {
        self.0.0.keys().copied()
    }
    /// Remove and return every member, in no particular order.
    pub fn drain_unordered(&mut self) -> impl Iterator<Item = u64> + '_ {
        self.0.0.drain().map(|(k, ())| k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_walks_in_key_order_whatever_the_insertion_order() {
        let mut map = IdMap::default();
        for key in [9u64, 2, 1 << 40, 7, 3] {
            map.insert(key, key * 2);
        }
        map.remove(&7);
        let keys: Vec<u64> = map.sorted().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec![2, 3, 9, 1 << 40]);
        assert_eq!(map[&9], 18);
        assert_eq!(
            format!("{map:?}"),
            "{2: 4, 3: 6, 9: 18, 1099511627776: 2199023255552}"
        );
    }

    #[test]
    fn consecutive_keys_land_in_distinct_low_buckets_with_mixed_tags() {
        let hash = |k: u64| {
            let mut h = IdHasher::default();
            h.write_u64(k);
            h.finish()
        };
        let buckets: std::collections::BTreeSet<u64> = (0..1024).map(|k| hash(k) & 1023).collect();
        assert_eq!(buckets.len(), 1024);
        let tags: std::collections::BTreeSet<u64> = (0..1024).map(|k| hash(k) >> 57).collect();
        assert!(tags.len() > 100);
    }
}
