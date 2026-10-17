//! An indexed snapshot: clones share the tree, writes copy only their path.
//! IDs are u32, so eight four-bit levels bound lookup, copying and drop work.
//! Empty subtrees are absent; the same tree also holds sparse audit candidates.
use std::fmt;
use std::ops::{Index, IndexMut};
use std::sync::Arc;

const WIDTH: usize = 16;

#[derive(Clone)]
enum Node<T> {
    Branch([Option<Arc<Node<T>>>; WIDTH]),
    Leaf([Option<T>; WIDTH]),
}

/// Append-only IDs remain stable even when a sparse entry is removed.
#[derive(Clone)]
pub struct Snapshot<T, const LEVEL: u32 = 7> {
    root: Option<Arc<Node<T>>>,
    len: usize,
}

impl<T, const LEVEL: u32> Default for Snapshot<T, LEVEL> {
    fn default() -> Self {
        Self { root: None, len: 0 }
    }
}

impl<T, const LEVEL: u32> Snapshot<T, LEVEL> {
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        if LEVEL == 7 && u32::try_from(index).is_err() {
            return None;
        }
        let mut node = self.root.as_deref()?;
        for level in (1..=LEVEL).rev() {
            let Node::Branch(children) = node else {
                unreachable!()
            };
            node = children[(index >> (4 * level)) & 15].as_deref()?;
        }
        let Node::Leaf(values) = node else {
            unreachable!()
        };
        values[index & 15].as_ref()
    }

    pub fn indexed(&self) -> Entries<'_, T> {
        let mut stack = Vec::new();
        if let Some(root) = self.root.as_deref() {
            stack.push((root, LEVEL, 0, 0));
        }
        Entries { stack }
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.indexed().map(|(_, v)| v)
    }

    /// Visit every potentially different entry, skipping shared subtrees.
    /// Leaf neighbors may be visited too; callers can compare value identity.
    pub(crate) fn differences(
        &self,
        old: &Self,
        mut visit: impl FnMut(usize, Option<&T>, Option<&T>),
    ) {
        fn walk<T>(
            old: Option<&Arc<Node<T>>>,
            new: Option<&Arc<Node<T>>>,
            level: u32,
            base: usize,
            visit: &mut impl FnMut(usize, Option<&T>, Option<&T>),
        ) {
            if matches!((old, new), (None, None))
                || matches!((old,new), (Some(a),Some(b)) if Arc::ptr_eq(a,b))
            {
                return;
            }
            if level == 0 {
                fn values<T>(node: Option<&Arc<Node<T>>>) -> Option<&[Option<T>; WIDTH]> {
                    match node.map(Arc::as_ref) {
                        Some(Node::Leaf(v)) => Some(v),
                        None => None,
                        _ => unreachable!(),
                    }
                }
                let before = values(old);
                let after = values(new);
                for i in 0..WIDTH {
                    let a = before.and_then(|v| v[i].as_ref());
                    let b = after.and_then(|v| v[i].as_ref());
                    if a.is_some() || b.is_some() {
                        visit(base + i, a, b);
                    }
                }
            } else {
                fn children<T>(
                    node: Option<&Arc<Node<T>>>,
                ) -> Option<&[Option<Arc<Node<T>>>; WIDTH]> {
                    match node.map(Arc::as_ref) {
                        Some(Node::Branch(v)) => Some(v),
                        None => None,
                        _ => unreachable!(),
                    }
                }
                let before = children(old);
                let after = children(new);
                for i in 0..WIDTH {
                    walk(
                        before.and_then(|v| v[i].as_ref()),
                        after.and_then(|v| v[i].as_ref()),
                        level - 1,
                        base + (i << (4 * level)),
                        visit,
                    );
                }
            }
        }
        walk(old.root.as_ref(), self.root.as_ref(), LEVEL, 0, &mut visit);
    }
}

impl<T: Clone, const LEVEL: u32> Snapshot<T, LEVEL> {
    pub fn push(&mut self, value: T) {
        self.set(self.len, value);
    }
    pub(crate) fn set(&mut self, index: usize, value: T) {
        assert!(
            LEVEL == 15 || (LEVEL == 7 && u32::try_from(index).is_ok()),
            "snapshot index exceeds its radix width"
        );
        fn slot<T: Clone>(
            root: &mut Option<Arc<Node<T>>>,
            level: u32,
            index: usize,
        ) -> &mut Option<T> {
            let node = Arc::make_mut(root.get_or_insert_with(|| {
                Arc::new(if level == 0 {
                    Node::Leaf(std::array::from_fn(|_| None))
                } else {
                    Node::Branch(std::array::from_fn(|_| None))
                })
            }));
            match node {
                Node::Leaf(values) => &mut values[index & 15],
                Node::Branch(children) => {
                    slot(&mut children[(index >> (4 * level)) & 15], level - 1, index)
                }
            }
        }
        *slot(&mut self.root, LEVEL, index) = Some(value);
        self.len = self.len.max(index.saturating_add(1));
    }
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        // An absent lookup must not copy or grow the snapshot.
        self.get(index)?;
        let mut node = Arc::make_mut(self.root.as_mut().unwrap());
        for level in (1..=LEVEL).rev() {
            let Node::Branch(children) = node else {
                unreachable!()
            };
            node = Arc::make_mut(children[(index >> (4 * level)) & 15].as_mut().unwrap());
        }
        let Node::Leaf(values) = node else {
            unreachable!()
        };
        values[index & 15].as_mut()
    }
    pub(crate) fn remove(&mut self, index: usize) {
        if self.get(index).is_none() {
            return;
        }
        fn remove<T: Clone>(root: &mut Option<Arc<Node<T>>>, level: u32, index: usize) {
            let node = Arc::make_mut(root.as_mut().unwrap());
            let empty = match node {
                Node::Leaf(values) => {
                    values[index & 15] = None;
                    values.iter().all(Option::is_none)
                }
                Node::Branch(children) => {
                    remove(&mut children[(index >> (4 * level)) & 15], level - 1, index);
                    children.iter().all(Option::is_none)
                }
            };
            if empty {
                *root = None;
            }
        }
        remove(&mut self.root, LEVEL, index);
    }
}

impl<T, const LEVEL: u32> Index<usize> for Snapshot<T, LEVEL> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        self.get(index).expect("snapshot index out of bounds")
    }
}
impl<T: Clone, const LEVEL: u32> IndexMut<usize> for Snapshot<T, LEVEL> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        self.get_mut(index).expect("snapshot index out of bounds")
    }
}
impl<T: PartialEq, const LEVEL: u32> PartialEq for Snapshot<T, LEVEL> {
    fn eq(&self, other: &Self) -> bool {
        if self.len != other.len {
            return false;
        }
        let mut same = true;
        self.differences(other, |_, a, b| same &= a == b);
        same
    }
}
impl<T: Eq, const LEVEL: u32> Eq for Snapshot<T, LEVEL> {}
impl<T: fmt::Debug, const LEVEL: u32> fmt::Debug for Snapshot<T, LEVEL> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

pub struct Entries<'a, T> {
    stack: Vec<(&'a Node<T>, u32, usize, usize)>,
}
impl<'a, T> Iterator for Entries<'a, T> {
    type Item = (usize, &'a T);
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((node, level, base, i)) = self.stack.pop() {
            if i == WIDTH {
                continue;
            }
            self.stack.push((node, level, base, i + 1));
            match node {
                Node::Leaf(values) => {
                    if let Some(v) = &values[i] {
                        return Some((base + i, v));
                    }
                }
                Node::Branch(children) => {
                    if let Some(child) = &children[i] {
                        self.stack
                            .push((child, level - 1, base + (i << (4 * level)), 0));
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fork_isolation_across_radix_boundaries_and_sparse_removal() {
        let mut original = Snapshot::<_>::default();
        for i in 0..4097 {
            original.push(i);
        }
        let mut fork = original.clone();
        for i in [0, 15, 16, 255, 256, 4095, 4096] {
            *fork.get_mut(i).unwrap() += 10000;
        }
        fork.push(4097);
        let mut changed = Vec::new();
        fork.differences(&original, |i, a, b| {
            if a != b {
                changed.push(i)
            }
        });
        assert_eq!(changed, [0, 15, 16, 255, 256, 4095, 4096, 4097]);
        assert_eq!(
            original.iter().copied().collect::<Vec<_>>(),
            (0..4097).collect::<Vec<_>>()
        );
        for i in 0..4098 {
            fork.remove(i);
        }
        assert!(fork.is_empty());
        assert_eq!(original.get(4096), Some(&4096));
    }
    #[test]
    fn unchanged_history_is_neither_cloned_nor_visited() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COPIES: AtomicUsize = AtomicUsize::new(0);
        struct Value;
        impl Clone for Value {
            fn clone(&self) -> Self {
                COPIES.fetch_add(1, Ordering::Relaxed);
                Self
            }
        }
        let mut original = Snapshot::<_>::default();
        for _ in 0..65536 {
            original.push(Value);
        }
        COPIES.store(0, Ordering::Relaxed);
        let mut fork = original.clone();
        assert_eq!(COPIES.load(Ordering::Relaxed), 0);
        *fork.get_mut(35000).unwrap() = Value;
        assert_eq!(COPIES.load(Ordering::Relaxed), 16);
        let mut visited = 0;
        fork.differences(&original, |_, _, _| visited += 1);
        assert_eq!(visited, 16);
    }
}
/// Ordered copy-on-write map for interned paths. AVL
/// rotations keep writes and fork-local copies logarithmic in the entry count.
#[derive(Clone)]
pub(crate) struct Map<K, V> {
    root: Option<Arc<MapNode<K, V>>>,
}
#[derive(Clone)]
struct MapNode<K, V> {
    key: K,
    value: V,
    left: Option<Arc<Self>>,
    right: Option<Arc<Self>>,
    height: u32,
}
impl<K, V> Default for Map<K, V> {
    fn default() -> Self {
        Self { root: None }
    }
}
impl<K, V> MapNode<K, V> {
    fn height(n: &Option<Arc<Self>>) -> u32 {
        n.as_ref().map_or(0, |n| n.height)
    }
    fn balance(&self) -> i64 {
        i64::from(Self::height(&self.left)) - i64::from(Self::height(&self.right))
    }
    fn update(&mut self) {
        self.height = 1 + Self::height(&self.left).max(Self::height(&self.right));
    }
}
impl<K: Ord, V> Map<K, V> {
    pub(crate) fn get(&self, key: &K) -> Option<&V> {
        let mut n = self.root.as_deref();
        while let Some(node) = n {
            match key.cmp(&node.key) {
                std::cmp::Ordering::Less => n = node.left.as_deref(),
                std::cmp::Ordering::Greater => n = node.right.as_deref(),
                std::cmp::Ordering::Equal => return Some(&node.value),
            }
        }
        None
    }
    #[cfg(test)]
    pub(crate) fn iter(&self) -> MapEntries<'_, K, V> {
        MapEntries {
            stack: Vec::new(),
            next: self.root.as_deref(),
        }
    }
}
impl<K: Ord + Clone, V: Clone> Map<K, V> {
    pub(crate) fn insert(&mut self, key: K, value: V) -> Option<V> {
        fn rotate<K: Clone, V: Clone>(root: &mut Arc<MapNode<K, V>>, left: bool) {
            let n = Arc::make_mut(root);
            let mut pivot = if left { n.right.take() } else { n.left.take() }.unwrap();
            let p = Arc::make_mut(&mut pivot);
            if left {
                n.right = p.left.take();
            } else {
                n.left = p.right.take();
            }
            n.update();
            let old = std::mem::replace(root, pivot);
            let n = Arc::make_mut(root);
            if left {
                n.left = Some(old);
            } else {
                n.right = Some(old);
            }
            n.update();
        }
        fn put<K: Ord + Clone, V: Clone>(
            root: &mut Option<Arc<MapNode<K, V>>>,
            key: K,
            value: V,
        ) -> Option<V> {
            let Some(tree) = root else {
                *root = Some(Arc::new(MapNode {
                    key,
                    value,
                    left: None,
                    right: None,
                    height: 1,
                }));
                return None;
            };
            let node = Arc::make_mut(tree);
            let old = match key.cmp(&node.key) {
                std::cmp::Ordering::Less => put(&mut node.left, key, value),
                std::cmp::Ordering::Greater => put(&mut node.right, key, value),
                std::cmp::Ordering::Equal => {
                    return Some(std::mem::replace(&mut node.value, value));
                }
            };
            node.update();
            let balance = node.balance();
            if balance > 1 {
                if node.left.as_ref().unwrap().balance() < 0 {
                    rotate(node.left.as_mut().unwrap(), true);
                }
                rotate(tree, false);
            } else if balance < -1 {
                if node.right.as_ref().unwrap().balance() > 0 {
                    rotate(node.right.as_mut().unwrap(), false);
                }
                rotate(tree, true);
            }
            old
        }
        put(&mut self.root, key, value)
    }
}
#[cfg(test)]
pub(crate) struct MapEntries<'a, K, V> {
    stack: Vec<&'a MapNode<K, V>>,
    next: Option<&'a MapNode<K, V>>,
}
#[cfg(test)]
impl<'a, K, V> Iterator for MapEntries<'a, K, V> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(n) = self.next {
            self.stack.push(n);
            self.next = n.left.as_deref();
        }
        let node = self.stack.pop()?;
        self.next = node.right.as_deref();
        Some((&node.key, &node.value))
    }
}
#[cfg(test)]
impl<'a, K: Ord, V> IntoIterator for &'a Map<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = MapEntries<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod map_tests {
    use super::*;
    #[test]
    fn ordered_forks_match_btree_map_through_single_and_double_rotations() {
        for order in [0, 1, 2] {
            let mut map = Map::default();
            let mut reference = std::collections::BTreeMap::new();
            for i in 0..4096 {
                let key = match order {
                    0 => i,
                    1 => 4095 - i,
                    _ => (i * 1789) % 4096,
                };
                assert_eq!(map.insert(key, key * 2), reference.insert(key, key * 2));
            }
            let base = map.clone();
            for i in (0..4096).step_by(3) {
                map.insert(i, i * 2 + 1);
                *reference.get_mut(&i).unwrap() += 1;
                assert_eq!(base.get(&i), Some(&(i * 2)));
            }
            assert_eq!(
                map.iter().collect::<Vec<_>>(),
                reference.iter().collect::<Vec<_>>()
            );
            fn check<K, V>(n: &Option<Arc<MapNode<K, V>>>) -> u32 {
                let Some(n) = n else { return 0 };
                let a = check(&n.left);
                let b = check(&n.right);
                assert!(a.abs_diff(b) <= 1);
                assert_eq!(n.height, 1 + a.max(b));
                n.height
            }
            assert!(check(&map.root) < 20);
        }
    }
}

impl<T: PartialEq, const LEVEL: u32> Snapshot<T, LEVEL> {
    pub(crate) fn contains(&self, value: &T) -> bool {
        self.iter().any(|v| v == value)
    }
}
impl<T: Clone, const LEVEL: u32> IntoIterator for Snapshot<T, LEVEL> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter().cloned().collect::<Vec<_>>().into_iter()
    }
}

impl<T: Clone, const LEVEL: u32> FromIterator<T> for Snapshot<T, LEVEL> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut snapshot = Self::default();
        for value in iter {
            snapshot.push(value);
        }
        snapshot
    }
}
impl<'a, T, const LEVEL: u32> IntoIterator for &'a Snapshot<T, LEVEL> {
    type Item = &'a T;
    type IntoIter = std::iter::Map<Entries<'a, T>, fn((usize, &'a T)) -> &'a T>;
    fn into_iter(self) -> Self::IntoIter {
        self.indexed().map(|(_, v)| v)
    }
}
