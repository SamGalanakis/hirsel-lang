//! An indexed snapshot: clones share the tree, writes copy only their path.
//! Root height follows populated indexes; the u32/u64 width caps lookup and copying.
//! Empty subtrees are absent; the same tree also holds sparse audit candidates.
use std::fmt;
use std::ops::{Index, IndexMut};
use std::sync::Arc;

const WIDTH: usize = 16;

#[derive(Clone)]
enum Node<T> {
    Branch(Slots<Arc<Node<T>>>),
    Leaf(Slots<T>),
}

// Prior-art license:
// MIT License
//
// Copyright (c) 2025 Diogo Sousa
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// RPDS vector assoc/get_mut/drop_last (42d227e4, src/vector/mod.rs:140-240)
// copies only the selected shared path and prunes empty children. Sparse IDs
// need holes, so an occupancy bitmap replaces its dense Vec length. In
// particular, Some(None) is populated when T itself is an Option.
struct Slots<T> {
    occupied: u16,
    values: [Option<T>; WIDTH],
}
impl<T> Default for Slots<T> {
    fn default() -> Self {
        Self {
            occupied: 0,
            values: std::array::from_fn(|_| None),
        }
    }
}
impl<T: Clone> Clone for Slots<T> {
    fn clone(&self) -> Self {
        let mut copy = Self::default();
        let mut remaining = self.occupied;
        while remaining != 0 {
            let i = remaining.trailing_zeros() as usize;
            remaining &= remaining - 1;
            copy.values[i] = self.values[i].clone();
        }
        copy.occupied = self.occupied;
        copy
    }
}
impl<T> Node<T> {
    fn occupied(&self) -> u16 {
        match self {
            Self::Branch(slots) => slots.occupied,
            Self::Leaf(slots) => slots.occupied,
        }
    }
}

/// Append-only IDs remain stable even when a sparse entry is removed.
#[derive(Clone)]
pub struct Snapshot<T, const LEVEL: u32 = 7> {
    root: Option<Arc<Node<T>>>,
    height: u32,
    len: usize,
}

impl<T, const LEVEL: u32> Default for Snapshot<T, LEVEL> {
    fn default() -> Self {
        Self {
            root: None,
            height: 0,
            len: 0,
        }
    }
}

impl<T, const LEVEL: u32> Snapshot<T, LEVEL> {
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }
    /// Advance the ID high-water mark without materializing default entries.
    pub(crate) fn extend_len(&mut self, len: usize) {
        self.len = self.len.max(len);
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        if LEVEL == 7 && u32::try_from(index).is_err() {
            return None;
        }
        if self.height < 15 && index >> (4 * (self.height + 1)) != 0 {
            return None;
        }
        let mut node = self.root.as_deref()?;
        for level in (1..=self.height).rev() {
            let Node::Branch(children) = node else {
                unreachable!()
            };
            node = children.values[(index >> (4 * level)) & 15].as_deref()?;
        }
        let Node::Leaf(values) = node else {
            unreachable!()
        };
        values.values[index & 15].as_ref()
    }

    pub fn indexed(&self) -> Entries<'_, T> {
        let mut stack = Vec::new();
        if let Some(root) = self.root.as_deref() {
            stack.push((root, self.height, 0, root.occupied()));
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
        // A shorter root is a virtual chain of leading zero branches. No
        // nodes or values are copied just to align two retained revisions.
        fn child<T>(
            node: Option<&Arc<Node<T>>>,
            height: u32,
            level: u32,
            i: usize,
        ) -> Option<&Arc<Node<T>>> {
            let node = node?;
            if height < level {
                return (i == 0).then_some(node);
            }
            let Node::Branch(children) = node.as_ref() else {
                unreachable!()
            };
            children.values[i].as_ref()
        }
        fn walk<T>(
            (old, old_height): (Option<&Arc<Node<T>>>, u32),
            (new, new_height): (Option<&Arc<Node<T>>>, u32),
            level: u32,
            base: usize,
            visit: &mut impl FnMut(usize, Option<&T>, Option<&T>),
        ) {
            if matches!((old, new), (None, None))
                || old_height == new_height
                    && matches!((old,new), (Some(a),Some(b)) if Arc::ptr_eq(a,b))
            {
                return;
            }
            if level == 0 {
                fn values<T>(node: Option<&Arc<Node<T>>>) -> Option<&[Option<T>; WIDTH]> {
                    match node.map(Arc::as_ref) {
                        Some(Node::Leaf(v)) => Some(&v.values),
                        None => None,
                        _ => unreachable!(),
                    }
                }
                let before = values(old);
                let after = values(new);
                let mut remaining =
                    old.map_or(0, |n| n.occupied()) | new.map_or(0, |n| n.occupied());
                while remaining != 0 {
                    let i = remaining.trailing_zeros() as usize;
                    remaining &= remaining - 1;
                    let a = before.and_then(|v| v[i].as_ref());
                    let b = after.and_then(|v| v[i].as_ref());
                    if a.is_some() || b.is_some() {
                        visit(base + i, a, b);
                    }
                }
            } else {
                let occupied = |node: Option<&Arc<Node<T>>>, height| {
                    node.map_or(0, |n| if height < level { 1 } else { n.occupied() })
                };
                let mut remaining = occupied(old, old_height) | occupied(new, new_height);
                while remaining != 0 {
                    let i = remaining.trailing_zeros() as usize;
                    remaining &= remaining - 1;
                    walk(
                        (child(old, old_height, level, i), old_height.min(level - 1)),
                        (child(new, new_height, level, i), new_height.min(level - 1)),
                        level - 1,
                        base + (i << (4 * level)),
                        visit,
                    );
                }
            }
        }
        walk(
            (old.root.as_ref(), old.height),
            (self.root.as_ref(), self.height),
            old.height.max(self.height),
            0,
            &mut visit,
        );
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
                    Node::Leaf(Slots::default())
                } else {
                    Node::Branch(Slots::default())
                })
            }));
            match node {
                Node::Leaf(values) => {
                    values.occupied |= 1 << (index & 15);
                    &mut values.values[index & 15]
                }
                Node::Branch(children) => {
                    let i = (index >> (4 * level)) & 15;
                    children.occupied |= 1 << i;
                    slot(&mut children.values[i], level - 1, index)
                }
            }
        }
        let needed = (usize::BITS - index.leading_zeros()).saturating_sub(1) / 4;
        if self.root.is_none() {
            self.height = needed;
        }
        while self.height < needed {
            let mut children = Slots::default();
            children.values[0] = self.root.take();
            children.occupied = 1;
            self.root = Some(Arc::new(Node::Branch(children)));
            self.height += 1;
        }
        *slot(&mut self.root, self.height, index) = Some(value);
        self.len = self.len.max(index.saturating_add(1));
    }
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        // An absent lookup must not copy or grow the snapshot.
        self.get(index)?;
        let mut node = Arc::make_mut(self.root.as_mut().unwrap());
        for level in (1..=self.height).rev() {
            let Node::Branch(children) = node else {
                unreachable!()
            };
            node = Arc::make_mut(
                children.values[(index >> (4 * level)) & 15]
                    .as_mut()
                    .unwrap(),
            );
        }
        let Node::Leaf(values) = node else {
            unreachable!()
        };
        values.values[index & 15].as_mut()
    }
    pub(crate) fn remove(&mut self, index: usize) {
        if self.get(index).is_none() {
            return;
        }
        fn remove<T: Clone>(root: &mut Option<Arc<Node<T>>>, level: u32, index: usize) {
            let node = Arc::make_mut(root.as_mut().unwrap());
            let empty = match node {
                Node::Leaf(values) => {
                    let i = index & 15;
                    values.values[i] = None;
                    values.occupied &= !(1 << i);
                    values.occupied == 0
                }
                Node::Branch(children) => {
                    let i = (index >> (4 * level)) & 15;
                    remove(&mut children.values[i], level - 1, index);
                    if children.values[i].is_none() {
                        children.occupied &= !(1 << i);
                    }
                    children.occupied == 0
                }
            };
            if empty {
                *root = None;
            }
        }
        remove(&mut self.root, self.height, index);
        while self.height > 0 {
            let Some(root) = self.root.as_ref() else {
                self.height = 0;
                break;
            };
            let Node::Branch(children) = root.as_ref() else {
                unreachable!()
            };
            if children.occupied != 1 {
                break;
            }
            self.root = children.values[0].clone();
            self.height -= 1;
        }
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
    stack: Vec<(&'a Node<T>, u32, usize, u16)>,
}
impl<'a, T> Iterator for Entries<'a, T> {
    type Item = (usize, &'a T);
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((node, level, base, remaining)) = self.stack.pop() {
            if remaining == 0 {
                continue;
            }
            let i = remaining.trailing_zeros() as usize;
            self.stack
                .push((node, level, base, remaining & (remaining - 1)));
            match node {
                Node::Leaf(values) => {
                    return Some((base + i, values.values[i].as_ref().unwrap()));
                }
                Node::Branch(children) => {
                    let child = children.values[i].as_deref().unwrap();
                    self.stack.push((
                        child,
                        level - 1,
                        base + (i << (4 * level)),
                        child.occupied(),
                    ));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Boundary-sized adaptations of RPDS vector overwrite/clone/index_mut
    // vectors (42d227e4, src/vector/test.rs:481-521,688-720). The independent
    // ordered model also covers holes, removals and immutable older forks.
    #[test]
    fn sparse_snapshot_histories_match_independent_btree_oracle() {
        use std::collections::{BTreeMap, BTreeSet};
        type Tree = Snapshot<Option<u64>, 15>;
        type Model = BTreeMap<usize, Option<u64>>;
        fn verify(tree: &Tree, model: &Model, len: usize) {
            assert_eq!(tree.len(), len);
            assert_eq!(tree.is_empty(), model.is_empty());
            assert_eq!(
                tree.indexed().map(|(k, v)| (k, *v)).collect::<Model>(),
                *model
            );
            for key in model.keys().copied().chain([0, 14, 17, 257, usize::MAX]) {
                assert_eq!(tree.get(key), model.get(&key));
            }
            let mut visited = 0;
            tree.differences(&tree.clone(), |_, _, _| visited += 1);
            assert_eq!(visited, 0, "identical roots must be skipped");
        }
        let keys = [
            0,
            15,
            16,
            255,
            256,
            4095,
            4096,
            u32::MAX as usize,
            1usize << 48,
            usize::MAX,
        ];
        let mut tree = Tree::default();
        let mut model = Model::new();
        let mut len = 0;
        let mut history = vec![(tree.clone(), model.clone(), len)];
        for (n, key) in keys.into_iter().enumerate() {
            tree.set(key, Some(n as u64));
            model.insert(key, Some(n as u64));
            len = len.max(key.saturating_add(1));
            // A unique-path update must preserve a populated None as well.
            *tree.get_mut(key).unwrap() = None;
            *model.get_mut(&key).unwrap() = None;
            verify(&tree, &model, len);
            history.push((tree.clone(), model.clone(), len));
            *tree.get_mut(key).unwrap() = Some(100 + n as u64);
            *model.get_mut(&key).unwrap() = Some(100 + n as u64);
            history.push((tree.clone(), model.clone(), len));
        }
        for key in keys.into_iter().rev() {
            tree.remove(key);
            model.remove(&key);
            verify(&tree, &model, len);
            history.push((tree.clone(), model.clone(), len));
        }
        assert_eq!(tree.height, 0);
        tree.push(None); // The saturated high-water ID is still usize::MAX.
        model.insert(usize::MAX, None);
        history.push((tree, model, len));
        for (base, expected, len) in &history {
            verify(base, expected, *len);
            let mut fork = base.clone();
            let root = fork.root.clone();
            assert!(fork.get_mut(17).is_none());
            fork.remove(17);
            assert!(
                match (&root, &fork.root) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                },
                "absent mutations must not copy or grow roots"
            );
            fork.set(17, Some(42));
            verify(base, expected, *len);
            let mut fork_model = expected.clone();
            fork_model.insert(17, Some(42));
            verify(&fork, &fork_model, (*len).max(18));
            for (old, before, _) in &history {
                let keys: BTreeSet<_> = before.keys().chain(fork_model.keys()).copied().collect();
                let expected_diff: BTreeMap<_, _> = keys
                    .into_iter()
                    .filter(|k| before.get(k) != fork_model.get(k))
                    .map(|k| (k, (before.get(&k).copied(), fork_model.get(&k).copied())))
                    .collect();
                let mut actual = BTreeMap::new();
                let mut visited = BTreeSet::new();
                fork.differences(old, |k, a, b| {
                    assert!(visited.insert(k), "duplicate diff key {k}");
                    if a != b {
                        actual.insert(k, (a.copied(), b.copied()));
                    }
                });
                assert_eq!(actual, expected_diff);
            }
        }
    }

    #[test]
    fn dense_roots_and_sparse_forks_preserve_values_at_every_height() {
        let mut current = Snapshot::<u64, 15>::default();
        let mut model = std::collections::BTreeMap::new();
        for i in 0..16 {
            current.push(i as u64);
            model.insert(i, i as u64);
        }
        // Dense register tables have no redundant high radix chain.
        assert_eq!(current.height, 0);
        for index in [16, 255, 256, 4096, u32::MAX as usize, usize::MAX] {
            let before = current.clone();
            let expected_before = model.clone();
            current.set(index, 99);
            model.insert(index, 99);
            assert_eq!(current.get(index), Some(&99));
            assert_eq!(before.get(index), expected_before.get(&index));
            let mut actual = std::collections::BTreeMap::new();
            current.differences(&before, |i, a, b| {
                if a != b {
                    actual.insert(i, (a.copied(), b.copied()));
                }
            });
            assert_eq!(
                actual,
                std::collections::BTreeMap::from([(index, (None, Some(99)))])
            );
            current.remove(index);
            model.remove(&index);
            assert_eq!(current.height, 0);
            assert_eq!(
                current.indexed().map(|(i, v)| (i, *v)).collect::<Vec<_>>(),
                model.iter().map(|(i, v)| (*i, *v)).collect::<Vec<_>>()
            );
            let mut removed = Vec::new();
            before.differences(&current, |i, a, b| {
                if a != b {
                    removed.push(i);
                }
            });
            assert!(removed.is_empty());
        }
        let mut original = Snapshot::<u64>::default();
        let mut expected = std::collections::BTreeMap::new();
        let mut seed = 19u64;
        for step in 0..1000 {
            let before = original.clone();
            let old = expected.clone();
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let index = (seed as usize) & 8191;
            if step % 3 == 0 {
                original.remove(index);
                expected.remove(&index);
            } else {
                original.set(index, seed);
                expected.insert(index, seed);
            }
            let mut actual = std::collections::BTreeMap::new();
            original.differences(&before, |i, a, b| {
                if a != b {
                    actual.insert(i, (a.copied(), b.copied()));
                }
            });
            let expected_diff = if old.get(&index) == expected.get(&index) {
                std::collections::BTreeMap::new()
            } else {
                std::collections::BTreeMap::from([(
                    index,
                    (old.get(&index).copied(), expected.get(&index).copied()),
                )])
            };
            assert_eq!(actual, expected_diff);
            assert_eq!(
                original.indexed().map(|(i, v)| (i, *v)).collect::<Vec<_>>(),
                expected.iter().map(|(i, v)| (*i, *v)).collect::<Vec<_>>()
            );
        }
    }
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
