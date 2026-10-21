use std::{
    fmt,
    ops::{Deref, DerefMut},
    sync::Arc,
};

#[cfg(test)]
thread_local! { static EDIT_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn copied_edit_rows() -> usize {
    EDIT_ROWS.with(std::cell::Cell::get)
}

/// Immutable child storage shared by type rows. Editing an unpublished row uses
/// copy-on-write; cloning or changing another qualifier never copies its children.
// Rustc15c1044 ty/list.rs:167-183 shares allocation-free empty lists.
// None is empty; nonempty children retain their existing Arc COW payload.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct SharedSlice<T>(Option<Arc<[T]>>);
impl<T: std::hash::Hash> std::hash::Hash for SharedSlice<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}
impl<T> Default for SharedSlice<T> {
    fn default() -> Self {
        Self(None)
    }
}
impl<T> SharedSlice<T> {
    pub fn new() -> Self {
        Self::default()
    }
    /// Allocation identity for retained-storage accounting, never semantic identity.
    pub fn storage_identity(&self) -> usize {
        self.0.as_ref().map_or(0, |data| data.as_ptr() as usize)
    }
    pub fn as_slice(&self) -> &[T] {
        self.0.as_deref().unwrap_or(&[])
    }
}
impl<T> Deref for SharedSlice<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.0.as_deref().unwrap_or(&[])
    }
}
impl<T: Clone> DerefMut for SharedSlice<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        match &mut self.0 {
            Some(data) => Arc::make_mut(data),
            None => &mut [],
        }
    }
}
impl<T: serde::Serialize> serde::Serialize for SharedSlice<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}
impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for SharedSlice<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<T>::deserialize(deserializer).map(Self::from)
    }
}
impl<T> From<Vec<T>> for SharedSlice<T> {
    fn from(v: Vec<T>) -> Self {
        if v.is_empty() {
            Self(None)
        } else {
            Self(Some(v.into()))
        }
    }
}
impl<T, const N: usize> From<[T; N]> for SharedSlice<T> {
    fn from(v: [T; N]) -> Self {
        if N == 0 {
            Self(None)
        } else {
            Self(Some(Arc::from(v)))
        }
    }
}
impl<T> FromIterator<T> for SharedSlice<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}
impl<T: fmt::Debug> fmt::Debug for SharedSlice<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}
impl<'a, T> IntoIterator for &'a SharedSlice<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<'a, T: Clone> IntoIterator for &'a mut SharedSlice<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
impl<T: Clone> IntoIterator for SharedSlice<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.to_vec().into_iter()
    }
}
impl<T: Clone> SharedSlice<T> {
    fn edit(&mut self, f: impl FnOnce(&mut Vec<T>)) {
        #[cfg(test)]
        EDIT_ROWS.with(|rows| rows.set(rows.get() + self.len()));
        let mut v = self.to_vec();
        f(&mut v);
        *self = v.into();
    }
    pub fn push(&mut self, value: T) {
        self.edit(|v| v.push(value));
    }
    pub fn clear(&mut self) {
        *self = Self::new();
    }
    pub fn truncate(&mut self, len: usize) {
        if len < self.len() {
            self.edit(|v| v.truncate(len));
        }
    }
    pub fn insert(&mut self, index: usize, value: T) {
        self.edit(|v| v.insert(index, value));
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.edit(|v| v.retain(|x| keep(x)));
    }
    pub fn dedup(&mut self)
    where
        T: PartialEq,
    {
        self.edit(|v| v.dedup());
    }
}
impl<T: Clone> Extend<T> for SharedSlice<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.edit(|v| v.extend(iter));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clones_share_children_until_edited() {
        let a: SharedSlice<_> = (0..64).collect();
        let mut b = a.clone();
        assert_eq!(a.storage_identity(), b.storage_identity());
        b[0] = 99;
        assert_eq!(a[0], 0);
        assert_eq!(b[0], 99);
        assert_ne!(a.storage_identity(), b.storage_identity());
    }
}
impl<'a, T: Clone + 'a> Extend<&'a T> for SharedSlice<T> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        self.extend(iter.into_iter().cloned());
    }
}
impl<T: PartialEq> PartialEq<Vec<T>> for SharedSlice<T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.as_slice() == other.as_slice()
    }
}
impl<T: PartialEq> PartialEq<SharedSlice<T>> for Vec<T> {
    fn eq(&self, other: &SharedSlice<T>) -> bool {
        self.as_slice() == other.as_slice()
    }
}
impl<T: PartialEq, const N: usize> PartialEq<[T; N]> for SharedSlice<T> {
    fn eq(&self, other: &[T; N]) -> bool {
        self.as_slice() == other
    }
}
impl<T> From<SharedSlice<T>> for Arc<[T]> {
    fn from(value: SharedSlice<T>) -> Self {
        value.0.unwrap_or_else(|| Arc::from([]))
    }
}

#[cfg(test)]
mod empty_tests {
    use super::*;
    use std::hash::{Hash, Hasher};
    fn hash(value: &impl Hash) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut h);
        h.finish()
    }
    #[test]
    fn empty_slices_keep_value_and_wire_semantics_without_retained_allocations() {
        let decoded: SharedSlice<u64> = serde_json::from_str("[]").unwrap();
        let mut choices = vec![
            SharedSlice::new(),
            SharedSlice::default(),
            Vec::new().into(),
            [].into(),
            std::iter::empty().collect(),
            decoded,
        ];
        for action in 0..3 {
            let mut values: SharedSlice<_> = vec![1, 2, 3].into();
            let retained = values.clone();
            match action {
                0 => values.clear(),
                1 => values.truncate(0),
                _ => values.retain(|_| false),
            }
            assert_eq!(retained.as_slice(), &[1, 2, 3]);
            choices.push(values);
        }
        for value in &mut choices {
            value.sort();
            value.dedup();
            value.extend(std::iter::empty::<u64>());
            assert!(value.is_empty());
            assert_eq!(value, &SharedSlice::new());
            assert_eq!(hash(value), hash(&Vec::<u64>::new()));
            assert_eq!(format!("{value:?}"), "[]");
            assert_eq!(serde_json::to_string(value).unwrap(), "[]");
            let owned: Arc<[u64]> = value.clone().into();
            assert!(owned.is_empty());
        }
        let mut grown = choices[0].clone();
        grown.push(7);
        let old = grown.clone();
        grown[0] = 9;
        assert_eq!(old.as_slice(), &[7]);
        assert_eq!(grown.as_slice(), &[9]);
        assert_eq!(hash(&grown), hash(&vec![9u64]));
        assert!(choices[0] < grown);
        let zst: SharedSlice<()> = [(); 3].into();
        assert_eq!(zst.len(), 3);
        #[derive(Clone)]
        #[repr(align(256))]
        struct Aligned;
        let mut aligned = SharedSlice::<Aligned>::new();
        assert_eq!(aligned.as_ptr() as usize % 256, 0);
        assert_eq!(aligned.iter_mut().count(), 0);
        assert_eq!(
            std::mem::size_of::<SharedSlice<u64>>(),
            std::mem::size_of::<Arc<[u64]>>()
        );
        assert!(
            choices.iter().all(|value| value.storage_identity() == 0),
            "empty child arrays retain no allocation"
        );
    }
}
