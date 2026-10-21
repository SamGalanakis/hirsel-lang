//! Private ordered facts with shared key numbering and radix snapshot rows.
//! Entries are shared between snapshots until a write replaces one.
use hirsel_intern::work::Counter;
use hirsel_types::rows::Rows;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
// Three radix levels cover ordinary name domains. This is only a storage
// split: newly encountered keys still get distinct numbers in the full tail.
const PREFIX: usize = 512;
type Segment<K, V> = Rows<Option<Rc<(K, V)>>>;

#[derive(Clone)]
pub(super) struct Indexed<K, V> {
    keys: Rc<RefCell<BTreeMap<K, usize>>>,
    values: [Segment<K, V>; 2],
}
impl<K: Clone + Ord, V: Clone> Default for Indexed<K, V> {
    fn default() -> Self {
        Self {
            keys: Rc::new(RefCell::new(BTreeMap::new())),
            values: [PREFIX, u32::MAX as usize - PREFIX]
                .map(|len| Rows::new(None, len).with_copy_counter(Counter::ValidateCopiedRows)),
        }
    }
}
impl<K: Clone + Ord, V: Clone + PartialEq> Indexed<K, V> {
    fn slot(index: usize) -> (usize, usize) {
        if index < PREFIX {
            (0, index)
        } else {
            (1, index - PREFIX)
        }
    }
    fn value(&self, index: usize) -> &Option<Rc<(K, V)>> {
        let (segment, row) = Self::slot(index);
        &self.values[segment][row]
    }
    fn value_mut(&mut self, index: usize) -> &mut Option<Rc<(K, V)>> {
        let (segment, row) = Self::slot(index);
        &mut self.values[segment][row]
    }
    pub fn get(&self, key: &K) -> Option<&V> {
        let i = *self.keys.borrow().get(key)?;
        self.value(i).as_ref().map(|entry| &entry.1)
    }
    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }
    fn number(&self, key: K) -> usize {
        let mut keys = self.keys.borrow_mut();
        let next = keys.len();
        *keys.entry(key).or_insert(next)
    }
    pub fn insert(&mut self, key: K, value: V) {
        if self.get(&key) == Some(&value) {
            return;
        }
        let i = self.number(key.clone());
        *self.value_mut(i) = Some(Rc::new((key, value)));
    }
    pub fn remove(&mut self, key: &K) {
        let i = self.keys.borrow().get(key).copied();
        if let Some(i) = i
            && self.value(i).is_some()
        {
            *self.value_mut(i) = None;
        }
    }
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        Entry { map: self, key }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        let mut entries: Vec<_> = self
            .values
            .iter()
            .flat_map(|rows| rows.entries())
            .map(|(_, row)| &**row.as_ref().unwrap())
            .collect();
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        entries.into_iter().map(|(key, value)| (key, value))
    }
    pub fn extend(&mut self, values: impl IntoIterator<Item = (K, V)>) {
        for (key, value) in values {
            self.insert(key, value);
        }
    }
    /// Union `other`, which is `base` changed in place, into entries that
    /// already joined `base`: only entries changed since `base` are visited.
    pub fn union_since(&mut self, base: &Self, other: &Self, mut join: impl FnMut(&mut V, &V)) {
        if Rc::ptr_eq(&self.keys, &other.keys) && Rc::ptr_eq(&base.keys, &other.keys) {
            for segment in 0..self.values.len() {
                self.values[segment].union_since(
                    &base.values[segment],
                    &other.values[segment],
                    |row, incoming| {
                        let Some(incoming) = incoming else {
                            return;
                        };
                        match row {
                            Some(old) if !Rc::ptr_eq(old, incoming) => {
                                let mut value = old.1.clone();
                                join(&mut value, &incoming.1);
                                // An equal join result keeps the incoming entry.
                                if value == incoming.1 {
                                    *row = Some(incoming.clone());
                                } else if value != old.1 {
                                    Rc::make_mut(old).1 = value;
                                }
                            }
                            Some(_) => {}
                            None => *row = Some(incoming.clone()),
                        }
                    },
                );
            }
        } else {
            for (key, incoming) in other.iter() {
                let mut value = self.get(key).cloned().unwrap_or_else(|| incoming.clone());
                join(&mut value, incoming);
                self.insert(key.clone(), value);
            }
        }
    }
}
pub(super) struct Entry<'a, K, V> {
    map: &'a mut Indexed<K, V>,
    key: K,
}
impl<'a, K: Clone + Ord, V: Clone + Default + PartialEq> Entry<'a, K, V> {
    pub fn or_insert_with(self, make: impl FnOnce() -> V) -> &'a mut V {
        let i = self.map.number(self.key.clone());
        &mut Rc::make_mut(
            self.map
                .value_mut(i)
                .get_or_insert_with(|| Rc::new((self.key, make()))),
        )
        .1
    }
}
impl<K: Clone + Ord, V: Clone + PartialEq> PartialEq for Indexed<K, V> {
    fn eq(&self, other: &Self) -> bool {
        if Rc::ptr_eq(&self.keys, &other.keys) {
            self.values == other.values
        } else {
            self.iter().eq(other.iter())
        }
    }
}
impl<K: Clone + Ord, V: Clone + Eq> Eq for Indexed<K, V> {}
impl<K: Clone + Ord, V: Clone + PartialEq> From<BTreeMap<K, V>> for Indexed<K, V> {
    fn from(values: BTreeMap<K, V>) -> Self {
        let mut result = Self::default();
        result.extend(values);
        result
    }
}
impl<'a, K: Clone + Ord + 'a, V: Clone + PartialEq + 'a> IntoIterator for &'a Indexed<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::vec::IntoIter<Self::Item>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter().collect::<Vec<_>>().into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    #[test]
    fn growth_across_prefix_preserves_absent_and_empty_facts() {
        let mut facts = Indexed::<usize, BTreeSet<usize>>::default();
        for key in 0..PREFIX + 3 {
            facts.insert(key, BTreeSet::from([key]));
        }
        let original = facts.clone();
        for key in [0, PREFIX - 1, PREFIX, PREFIX + 2] {
            facts.remove(&key);
        }
        let base = facts.clone();
        let mut incoming = base.clone();
        for key in [0, PREFIX - 1, PREFIX, PREFIX + 2] {
            incoming.insert(key, BTreeSet::new());
        }
        // A present empty fact must survive, even on either side of the split.
        facts.union_since(&base, &incoming, |a, b| a.extend(b));
        for key in [0, PREFIX - 1, PREFIX, PREFIX + 2] {
            assert_eq!(facts.get(&key), Some(&BTreeSet::new()));
            assert_eq!(original.get(&key), Some(&BTreeSet::from([key])));
            assert_eq!(base.get(&key), None);
        }
        let mut reversed = Indexed::default();
        for (key, value) in facts.iter().collect::<Vec<_>>().into_iter().rev() {
            reversed.insert(*key, value.clone());
        }
        assert!(facts == reversed);
        let before = facts.clone();
        reversed.insert(PREFIX + 5, BTreeSet::new());
        facts.union_since(&before, &reversed, |a, b| a.extend(b));
        assert_eq!(facts.get(&(PREFIX + 5)), Some(&BTreeSet::new()));
        assert_eq!(before.get(&(PREFIX + 5)), None);
    }

    #[test]
    fn snapshots_and_joins_preserve_ordered_facts() {
        let mut actual = Indexed::<u32, BTreeSet<u32>>::default();
        let mut flat = BTreeMap::<u32, BTreeSet<u32>>::new();
        for round in 0..100 {
            let old = actual.clone();
            let old_flat = flat.clone();
            let key = (round * 79) % 137;
            actual.insert(key, BTreeSet::from([round]));
            flat.insert(key, BTreeSet::from([round]));
            actual.remove(&((key + 31) % 137));
            flat.remove(&((key + 31) % 137));
            assert!(old.iter().eq(old_flat.iter()));
            let base = actual.clone();
            actual.union_since(&base, &old, |a, b| a.extend(b));
            for (key, values) in old_flat {
                flat.entry(key).or_default().extend(values);
            }
            assert!(actual.iter().eq(flat.iter()));
            let independently_numbered = Indexed::from(flat.clone());
            assert!(actual == independently_numbered);
            let base = actual.clone();
            actual.union_since(&base, &independently_numbered, |a, b| a.extend(b));
            assert!(actual.iter().eq(flat.iter()));
        }
        let before = hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCopiedRows);
        let mut snapshot = actual.clone();
        snapshot.insert(1000, BTreeSet::from([7]));
        assert!(
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateCopiedRows) - before
                <= 8
        );
        assert!(actual.get(&1000).is_none());
        snapshot
            .entry(1000)
            .or_insert_with(|| panic!("existing fact"));
    }
}
