//! Prefix-shared paths: each extension retains one node and cloning is constant size.
use std::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    ops::{Index, IndexMut},
    sync::Arc,
};

pub struct SharedPath<T> {
    tail: Option<Arc<Node<T>>>,
}
#[derive(Clone)]
struct Node<T> {
    prefix: SharedPath<T>,
    value: T,
    len: usize,
}
impl<T> Default for SharedPath<T> {
    fn default() -> Self {
        Self { tail: None }
    }
}
impl<T> Clone for SharedPath<T> {
    fn clone(&self) -> Self {
        Self {
            tail: self.tail.clone(),
        }
    }
}
impl<T> Drop for SharedPath<T> {
    fn drop(&mut self) {
        let mut tail = self.tail.take();
        while let Some(node) = tail {
            match Arc::try_unwrap(node) {
                Ok(mut node) => tail = node.prefix.tail.take(),
                Err(_) => break,
            }
        }
    }
}
impl<T> SharedPath<T> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.tail.as_ref().map_or(0, |n| n.len)
    }
    pub fn is_empty(&self) -> bool {
        self.tail.is_none()
    }
    pub fn last(&self) -> Option<&T> {
        self.tail.as_ref().map(|n| &n.value)
    }
    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }
    pub fn prefix(&self) -> Option<&Self> {
        self.tail.as_ref().map(|n| &n.prefix)
    }
    /// Identity only for deduplicating retained nodes within this process.
    pub fn node_identity(&self) -> Option<usize> {
        self.tail.as_ref().map(|n| Arc::as_ptr(n) as usize)
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len() {
            return None;
        }
        let mut path = self;
        while path.len() > index + 1 {
            path = path.prefix()?;
        }
        path.last()
    }
    pub fn push(&mut self, value: T) {
        let len = self.len() + 1;
        let prefix = std::mem::take(self);
        self.tail = Some(Arc::new(Node { prefix, value, len }));
    }
    pub fn truncate(&mut self, len: usize) {
        while self.len() > len {
            *self = self.prefix().unwrap().clone();
        }
    }
    pub fn clear(&mut self) {
        *self = Self::new();
    }
    pub fn iter(&self) -> std::iter::Rev<std::vec::IntoIter<&T>> {
        let mut values = Vec::with_capacity(self.len());
        let mut path = self;
        while let Some(node) = &path.tail {
            values.push(&node.value);
            path = &node.prefix;
        }
        values.into_iter().rev()
    }
}
impl<T: Clone> SharedPath<T> {
    pub fn last_mut(&mut self) -> Option<&mut T> {
        self.tail
            .as_mut()
            .map(|node| &mut Arc::make_mut(node).value)
    }
    pub fn pop(&mut self) -> Option<T> {
        let node = Arc::unwrap_or_clone(self.tail.take()?);
        *self = node.prefix;
        Some(node.value)
    }
    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }
    pub fn extend_from_slice(&mut self, values: &[T]) {
        self.extend(values.iter().cloned());
    }
}
impl<T: PartialEq> SharedPath<T> {
    pub fn starts_with<'a>(&self, other: impl IntoIterator<Item = &'a T>) -> bool
    where
        T: 'a,
    {
        let mut values = self.iter();
        for value in other {
            if values.next() != Some(value) {
                return false;
            }
        }
        true
    }
}
impl<T> Extend<T> for SharedPath<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, values: I) {
        for value in values {
            self.push(value);
        }
    }
}
impl<T: serde::Serialize> serde::Serialize for SharedPath<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}
impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for SharedPath<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<T>::deserialize(deserializer).map(Self::from)
    }
}
impl<T> FromIterator<T> for SharedPath<T> {
    fn from_iter<I: IntoIterator<Item = T>>(values: I) -> Self {
        let mut path = Self::new();
        path.extend(values);
        path
    }
}
impl<T> From<Vec<T>> for SharedPath<T> {
    fn from(values: Vec<T>) -> Self {
        values.into_iter().collect()
    }
}
impl<T> Index<usize> for SharedPath<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        self.get(index).expect("path index out of bounds")
    }
}
impl<T: Clone> IndexMut<usize> for SharedPath<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        assert!(index < self.len(), "path index out of bounds");
        let mut path = self;
        loop {
            let node = Arc::make_mut(path.tail.as_mut().unwrap());
            if node.len == index + 1 {
                return &mut node.value;
            }
            path = &mut node.prefix;
        }
    }
}
impl<'a, T> IntoIterator for &'a SharedPath<T> {
    type Item = &'a T;
    type IntoIter = std::iter::Rev<std::vec::IntoIter<&'a T>>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<T: Clone> IntoIterator for SharedPath<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.to_vec().into_iter()
    }
}
impl<T: fmt::Debug> fmt::Debug for SharedPath<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}
impl<T: PartialEq> PartialEq for SharedPath<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && (self.node_identity() == other.node_identity() || self.iter().eq(other.iter()))
    }
}
impl<T: Eq> Eq for SharedPath<T> {}
impl<T: PartialEq> PartialEq<Vec<T>> for SharedPath<T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}
impl<T: Hash> Hash for SharedPath<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        for value in self {
            value.hash(state);
        }
    }
}
impl<T: Ord> Ord for SharedPath<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.iter().cmp(other.iter())
    }
}
impl<T: Ord> PartialOrd for SharedPath<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a, T: Copy + 'a> Extend<&'a T> for SharedPath<T> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, values: I) {
        self.extend(values.into_iter().copied());
    }
}
impl<T: Clone> SharedPath<T> {
    pub fn insert(&mut self, index: usize, value: T) {
        let mut values = self.to_vec();
        values.insert(index, value);
        *self = values.into();
    }
}

/// Canonicalize paths reconstructed by producers so repeated prefixes retain one node.
pub struct PathInterner<T> {
    nodes: std::collections::HashMap<(Option<usize>, T), SharedPath<T>>,
}
impl<T> Default for PathInterner<T> {
    fn default() -> Self {
        Self {
            nodes: std::collections::HashMap::new(),
        }
    }
}
impl<T: Clone + Eq + Hash> PathInterner<T> {
    pub fn intern(&mut self, path: impl IntoIterator<Item = T>) -> SharedPath<T> {
        let mut prefix = SharedPath::new();
        for value in path {
            let key = (prefix.node_identity(), value.clone());
            prefix = self
                .nodes
                .entry(key)
                .or_insert_with(|| {
                    let mut next = prefix.clone();
                    next.push(value);
                    next
                })
                .clone();
        }
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_prefix_rows_grow_linearly_and_mutations_preserve_other_paths() {
        for count in [32, 128] {
            let mut prefixes = vec![];
            let mut path = SharedPath::new();
            for value in 0..count {
                path.push(value);
                prefixes.push(path.clone());
            }
            let nodes: std::collections::BTreeSet<_> = prefixes
                .iter()
                .filter_map(SharedPath::node_identity)
                .collect();
            assert_eq!(nodes.len(), count);
            assert_eq!(std::mem::size_of_val(&path), std::mem::size_of::<usize>());
            let mut changed = path.clone();
            changed[0] = count;
            assert_eq!(path[0], 0);
            assert_eq!(changed[0], count);
            changed.truncate(count / 2);
            assert_eq!(changed.len(), count / 2);
            assert_eq!(path.len(), count);
            let mut interner = PathInterner::default();
            for prefix in &prefixes {
                interner.intern(prefix.iter().copied());
            }
            assert_eq!(interner.nodes.len(), count);
        }
    }
    #[test]
    fn unshared_deep_paths_drop_without_host_recursion() {
        std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(|| {
                let path: SharedPath<_> = (0..100_000).collect();
                assert_eq!(path.last(), Some(&99_999));
                drop(path);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
