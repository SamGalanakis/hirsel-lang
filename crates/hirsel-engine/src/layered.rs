//! Engine-local overlays over immutable shared maps (shared package images).
use std::{
    borrow::Borrow,
    collections::{BTreeMap, btree_map},
    iter::Peekable,
    ops::RangeBounds,
    sync::Arc,
};

/// An ordered map whose entries are a local overlay over an immutable shared
/// base. Every write goes to the overlay, and a removed base entry is hidden
/// by a tombstone, so the base is never mutated and other holders never
/// observe this map's changes. Iteration order is the merged key order.
pub struct Layered<K, V> {
    base: Arc<BTreeMap<K, V>>,
    local: BTreeMap<K, Option<V>>,
}
impl<K, V> Default for Layered<K, V> {
    fn default() -> Self {
        Self {
            base: Arc::default(),
            local: BTreeMap::new(),
        }
    }
}
impl<K: Ord + Clone, V: Clone> Layered<K, V> {
    pub fn shared(base: Arc<BTreeMap<K, V>>) -> Self {
        Self {
            base,
            local: BTreeMap::new(),
        }
    }
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        match self.local.get(key) {
            Some(entry) => entry.as_ref(),
            None => self.base.get(key),
        }
    }
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get(key).is_some()
    }
    pub fn insert(&mut self, key: K, value: V) {
        self.local.insert(key, Some(value));
    }
    pub fn remove(&mut self, key: &K) {
        if self.base.contains_key(key) {
            self.local.insert(key.clone(), None);
        } else {
            self.local.remove(key);
        }
    }
    /// The entry, copied into the overlay first when it lives in the base.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        if !self.local.contains_key(key) {
            let value = self.base.get(key)?.clone();
            self.local.insert(key.clone(), Some(value));
        }
        self.local.get_mut(key)?.as_mut()
    }
    pub fn entry_or_default(&mut self, key: K) -> &mut V
    where
        V: Default,
    {
        if self.get(&key).is_none() {
            self.insert(key.clone(), V::default());
        }
        self.get_mut(&key).unwrap()
    }
    pub fn iter(
        &self,
    ) -> Merge<'_, K, V, btree_map::Iter<'_, K, V>, btree_map::Iter<'_, K, Option<V>>> {
        Merge {
            base: self.base.iter().peekable(),
            local: self.local.iter().peekable(),
        }
    }
    pub fn range<R: RangeBounds<K> + Clone>(
        &self,
        range: R,
    ) -> Merge<'_, K, V, btree_map::Range<'_, K, V>, btree_map::Range<'_, K, Option<V>>> {
        Merge {
            base: self.base.range(range.clone()).peekable(),
            local: self.local.range(range).peekable(),
        }
    }
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }
    /// Copies every base entry into the overlay; for whole-map rewrites only.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        for (key, value) in self.base.iter() {
            if !self.local.contains_key(key) {
                self.local.insert(key.clone(), Some(value.clone()));
            }
        }
        self.local.values_mut().flatten()
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        let removed: Vec<K> = self
            .iter()
            .filter(|(k, v)| !keep(k, v))
            .map(|(k, _)| k.clone())
            .collect();
        for key in removed {
            self.remove(&key);
        }
    }
    pub fn clear(&mut self) {
        self.base = Arc::default();
        self.local.clear();
    }
    pub fn len(&self) -> usize {
        self.iter().count()
    }
    /// Merge the overlay into a new immutable base and share it.
    pub fn freeze(&mut self) -> Arc<BTreeMap<K, V>> {
        if !self.local.is_empty() {
            let base = Arc::make_mut(&mut self.base);
            for (key, value) in std::mem::take(&mut self.local) {
                match value {
                    Some(value) => base.insert(key, value),
                    None => base.remove(&key),
                };
            }
        }
        self.base.clone()
    }
}
impl<K: Ord + Clone, V: Clone> Extend<(K, V)> for Layered<K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, entries: I) {
        for (key, value) in entries {
            self.insert(key, value);
        }
    }
}
impl<K: Ord + Clone, V: Clone> FromIterator<(K, V)> for Layered<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(entries: I) -> Self {
        let mut map = Self::default();
        map.extend(entries);
        map
    }
}
impl<K: Ord + Clone, V: Clone, Q: Ord + ?Sized> std::ops::Index<&Q> for Layered<K, V>
where
    K: Borrow<Q>,
{
    type Output = V;
    fn index(&self, key: &Q) -> &V {
        self.get(key).expect("no entry for key")
    }
}
impl<'a, K: Ord + Clone, V: Clone> IntoIterator for &'a Layered<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Merge<'a, K, V, btree_map::Iter<'a, K, V>, btree_map::Iter<'a, K, Option<V>>>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Merged iteration: overlay entries replace base entries; tombstones hide them.
pub struct Merge<'a, K: 'a, V: 'a, B, L>
where
    B: Iterator<Item = (&'a K, &'a V)>,
    L: Iterator<Item = (&'a K, &'a Option<V>)>,
{
    base: Peekable<B>,
    local: Peekable<L>,
}
impl<'a, K: Ord + 'a, V: 'a, B, L> Iterator for Merge<'a, K, V, B, L>
where
    B: Iterator<Item = (&'a K, &'a V)>,
    L: Iterator<Item = (&'a K, &'a Option<V>)>,
{
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let order = match (self.base.peek(), self.local.peek()) {
                (None, None) => return None,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some((b, _)), Some((l, _))) => b.cmp(l),
            };
            match order {
                std::cmp::Ordering::Less => return self.base.next(),
                std::cmp::Ordering::Equal => {
                    self.base.next();
                }
                std::cmp::Ordering::Greater => {}
            }
            let (key, value) = self.local.next().unwrap();
            if let Some(value) = value {
                return Some((key, value));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlay_writes_never_reach_the_shared_base() {
        let mut first: Layered<u32, &str> = [(1, "a"), (2, "b"), (4, "d")].into_iter().collect();
        let base = first.freeze();
        let mut second = Layered::shared(base.clone());
        second.insert(3, "c");
        second.remove(&2);
        *second.get_mut(&4).unwrap() = "D";
        second.retain(|k, _| *k != 1);
        assert_eq!(second.iter().collect::<Vec<_>>(), [(&3, &"c"), (&4, &"D")]);
        assert_eq!(second.range(3..).count(), 2);
        assert!(!second.contains_key(&2));
        assert_eq!(
            first.iter().collect::<Vec<_>>(),
            [(&1, &"a"), (&2, &"b"), (&4, &"d")]
        );
        assert_eq!(base.len(), 3);
        second.insert(2, "B");
        assert_eq!(
            second.freeze().iter().collect::<Vec<_>>(),
            [(&2, &"B"), (&3, &"c"), (&4, &"D")]
        );
        assert_eq!(base.get(&4), Some(&"d"));
    }
}
