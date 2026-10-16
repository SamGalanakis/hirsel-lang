use std::{
    fmt,
    ops::{Deref, DerefMut},
    sync::Arc,
};

/// Immutable child storage shared by type rows. Editing an unpublished row uses
/// copy-on-write; cloning or changing another qualifier never copies its children.
#[derive(Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct SharedSlice<T>(Arc<[T]>);
impl<T> Default for SharedSlice<T> {
    fn default() -> Self {
        Self(Arc::from([]))
    }
}
impl<T> SharedSlice<T> {
    pub fn new() -> Self {
        Self::default()
    }
    /// Allocation identity for retained-storage accounting, never semantic identity.
    pub fn storage_identity(&self) -> usize {
        self.0.as_ptr() as usize
    }
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
}
impl<T> Deref for SharedSlice<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}
impl<T: Clone> DerefMut for SharedSlice<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        Arc::make_mut(&mut self.0)
    }
}
impl<T> From<Vec<T>> for SharedSlice<T> {
    fn from(v: Vec<T>) -> Self {
        Self(v.into())
    }
}
impl<T, const N: usize> From<[T; N]> for SharedSlice<T> {
    fn from(v: [T; N]) -> Self {
        Self(Arc::from(v))
    }
}
impl<T> FromIterator<T> for SharedSlice<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}
impl<T: fmt::Debug> fmt::Debug for SharedSlice<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
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
        value.0
    }
}
