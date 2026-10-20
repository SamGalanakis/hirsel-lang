//! Private ordered facts with shared key numbering and radix snapshot rows.
//! Entries are shared between snapshots until a write replaces one.
use hirsel_intern::work::Counter;
use hirsel_types::rows::Rows;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
#[derive(Clone)]
pub(super) struct Indexed<K, V> {
    keys: Rc<RefCell<BTreeMap<K, usize>>>,
    values: Rows<Option<Rc<(K, V)>>>,
}
impl<K: Clone + Ord, V: Clone> Default for Indexed<K, V> {
    fn default() -> Self {
        Self {
            keys: Rc::new(RefCell::new(BTreeMap::new())),
            values: Rows::new(None, u32::MAX as usize)
                .with_copy_counter(Counter::ValidateCopiedRows),
        }
    }
}
impl<K: Clone + Ord, V: Clone + PartialEq> Indexed<K, V> {
    pub fn get(&self, key: &K) -> Option<&V> {
        let i = *self.keys.borrow().get(key)?;
        self.values[i].as_ref().map(|entry| &entry.1)
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
        self.values[i] = Some(Rc::new((key, value)));
    }
    pub fn remove(&mut self, key: &K) {
        let i = self.keys.borrow().get(key).copied();
        if let Some(i) = i {
            if self.values[i].is_some() {
                self.values[i] = None;
            }
        }
    }
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        Entry { map: self, key }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        let mut entries: Vec<_> = self
            .values
            .entries()
            .into_iter()
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
    pub fn union(&mut self, other: &Self, mut join: impl FnMut(&mut V, &V)) {
        if Rc::ptr_eq(&self.keys, &other.keys) {
            self.values.union(&other.values, |row, incoming| {
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
            });
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
        &mut Rc::make_mut(self.map.values[i].get_or_insert_with(|| Rc::new((self.key, make())))).1
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
            actual.union(&old, |a, b| a.extend(b));
            for (key, values) in old_flat {
                flat.entry(key).or_default().extend(values);
            }
            assert!(actual.iter().eq(flat.iter()));
            let independently_numbered = Indexed::from(flat.clone());
            assert!(actual == independently_numbered);
            actual.union(&independently_numbered, |a, b| a.extend(b));
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
