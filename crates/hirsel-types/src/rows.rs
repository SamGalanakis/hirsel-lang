//! Fixed-size rows with shared snapshots and copy-on-write radix paths.
//! Callers supply every merge operation; the container makes no flow judgments.
use hirsel_intern::work::{self, Counter};
use std::{
    collections::HashMap,
    ops::{Index, IndexMut},
    rc::Rc,
};
const BITS: usize = 3;
const WIDTH: usize = 1 << BITS;
#[derive(Clone)]
pub struct Rows<T> {
    len: usize,
    height: usize,
    default: T,
    root: Option<Rc<Node<T>>>,
    copy_counter: Option<Counter>,
}
enum Node<T> {
    Branch([Option<Rc<Node<T>>>; WIDTH]),
    Leaf([T; WIDTH]),
}
// Inputs stay alive with their memo entry: allocator pointer reuse cannot turn
// a different node pair into a cache hit. Keys are ordered, not commutative.
struct Joined<T> {
    _inputs: [Option<Rc<Node<T>>>; 2],
    output: Option<Rc<Node<T>>>,
    changed: bool,
}
/// Exact node-pair memoization for one pure, idempotent row join operation.
/// Equality must have the same value semantics as the operation. The cache is
/// analysis-local; dropping it releases all retained operands and results.
/// Instrumentation may count calls, but the operation must not depend on them.
pub struct JoinCache<T, F> {
    operation: F,
    default: Option<T>,
    entries: HashMap<(usize, usize, usize), Joined<T>>,
}
impl<T, F> JoinCache<T, F> {
    pub fn new(operation: F) -> Self {
        Self {
            operation,
            default: None,
            entries: HashMap::new(),
        }
    }
}
fn identity<T>(node: &Option<Rc<Node<T>>>) -> usize {
    node.as_ref().map_or(0, |node| Rc::as_ptr(node) as usize)
}
impl<T: Clone> Clone for Node<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Branch(children) => Self::Branch(children.clone()),
            Self::Leaf(values) => Self::Leaf(values.clone()),
        }
    }
}
fn same<T>(a: &Option<Rc<Node<T>>>, b: &Option<Rc<Node<T>>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}
impl<T: Clone + Default> Default for Rows<T> {
    fn default() -> Self {
        Self::new(T::default(), 0)
    }
}
impl<T: Clone> Rows<T> {
    pub fn new(default: T, len: usize) -> Self {
        let mut height = 0;
        let mut capacity = WIDTH;
        while capacity < len {
            capacity = capacity.checked_mul(WIDTH).expect("row arena too large");
            height += 1;
        }
        Self {
            len,
            height,
            default,
            root: None,
            copy_counter: None,
        }
    }
    /// Select optional deterministic measurement of copied leaf rows.
    pub fn with_copy_counter(mut self, counter: Counter) -> Self {
        self.copy_counter = Some(counter);
        self
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        (0..self.len).map(move |i| &self[i])
    }
    fn at<'a>(node: &'a Option<Rc<Node<T>>>, height: usize, index: usize, default: &'a T) -> &'a T {
        match node.as_deref() {
            None => default,
            Some(Node::Leaf(values)) => &values[index % WIDTH],
            Some(Node::Branch(children)) => Self::at(
                &children[(index >> (height * BITS)) % WIDTH],
                height - 1,
                index,
                default,
            ),
        }
    }
    fn at_mut<'a>(
        node: &'a mut Option<Rc<Node<T>>>,
        height: usize,
        index: usize,
        default: &T,
        copy_counter: Option<Counter>,
    ) -> &'a mut T {
        let node = node.get_or_insert_with(|| {
            Rc::new(if height == 0 {
                Node::Leaf(std::array::from_fn(|_| default.clone()))
            } else {
                Node::Branch(std::array::from_fn(|_| None))
            })
        });
        if height == 0 && Rc::strong_count(node) > 1 {
            if let Some(counter) = copy_counter {
                work::count_by(counter, WIDTH as u64);
            }
        }
        match Rc::make_mut(node) {
            Node::Leaf(values) => &mut values[index % WIDTH],
            Node::Branch(children) => Self::at_mut(
                &mut children[(index >> (height * BITS)) % WIDTH],
                height - 1,
                index,
                default,
                copy_counter,
            ),
        }
    }
}
impl<T: Clone + PartialEq> Rows<T> {
    fn compare(
        a: &Option<Rc<Node<T>>>,
        b: &Option<Rc<Node<T>>>,
        height: usize,
        base: usize,
        len: usize,
        default: &T,
        visit: &mut impl FnMut(usize, &T, &T),
    ) {
        if same(a, b) || base >= len {
            return;
        }
        if height == 0 {
            for i in 0..WIDTH.min(len - base) {
                let a = Self::at(a, 0, i, default);
                let b = Self::at(b, 0, i, default);
                if a != b {
                    visit(base + i, a, b);
                }
            }
        } else {
            let empty = None;
            for i in 0..WIDTH {
                let child = |node: &Option<Rc<Node<T>>>| match node.as_deref() {
                    Some(Node::Branch(children)) => children[i].clone(),
                    _ => empty.clone(),
                };
                Self::compare(
                    &child(a),
                    &child(b),
                    height - 1,
                    base + (i << (height * BITS)),
                    len,
                    default,
                    visit,
                );
            }
        }
    }
    pub fn set(&mut self, index: usize, value: T) {
        if self[index] != value {
            self[index] = value;
        }
    }
    pub fn differences(&self, other: &Self) -> Vec<usize> {
        assert_eq!(self.len, other.len);
        assert!(self.default == other.default);
        let mut differences = vec![];
        Self::compare(
            &self.root,
            &other.root,
            self.height,
            0,
            self.len,
            &self.default,
            &mut |i, _, _| differences.push(i),
        );
        differences
    }
    /// The operation must be idempotent: equal rows are not visited.
    pub fn join(&mut self, other: &Self, mut join: impl FnMut(&mut T, &T)) {
        self.merge(other, false, &mut join, None);
    }
    /// Union whose default row is an identity. Entire disjoint subtrees share.
    pub fn union(&mut self, other: &Self, mut join: impl FnMut(&mut T, &T)) {
        self.merge(other, true, &mut join, None);
    }
    /// Memoize exact ordered node pairs, without assigning a neutral default.
    /// A cache belongs to one operation and one equal default-row domain.
    /// Returns whether any row value changed, without a second comparison walk.
    pub fn join_cached<F: FnMut(&mut T, &T)>(
        &mut self,
        other: &Self,
        cache: &mut JoinCache<T, F>,
    ) -> bool {
        if let Some(default) = &cache.default {
            assert!(default == &self.default);
        } else {
            cache.default = Some(self.default.clone());
        }
        self.merge(other, false, &mut cache.operation, Some(&mut cache.entries))
    }
    fn merge(
        &mut self,
        other: &Self,
        neutral: bool,
        join: &mut impl FnMut(&mut T, &T),
        cache: Option<&mut HashMap<(usize, usize, usize), Joined<T>>>,
    ) -> bool {
        assert_eq!(self.len, other.len);
        assert!(self.default == other.default);
        fn merge<T: Clone + PartialEq>(
            a: &mut Option<Rc<Node<T>>>,
            b: &Option<Rc<Node<T>>>,
            height: usize,
            default: &T,
            neutral: bool,
            copy_counter: Option<Counter>,
            join: &mut impl FnMut(&mut T, &T),
            mut cache: Option<&mut HashMap<(usize, usize, usize), Joined<T>>>,
        ) -> bool {
            if same(a, b) || neutral && b.is_none() {
                return false;
            }
            if neutral && a.is_none() {
                *a = b.clone();
                return true;
            }
            let key = (identity(a), identity(b), height);
            if let Some(hit) = cache.as_ref().and_then(|cache| cache.get(&key)) {
                *a = hit.output.clone();
                return hit.changed;
            }
            let inputs = cache.as_ref().map(|_| [a.clone(), b.clone()]);
            let mut changed = false;
            if height == 0 {
                for i in 0..WIDTH {
                    let old = Rows::at(a, 0, i, default);
                    let incoming = Rows::at(b, 0, i, default);
                    if old == incoming {
                        continue;
                    }
                    let mut value = old.clone();
                    join(&mut value, incoming);
                    if &value != old {
                        changed = true;
                        *Rows::at_mut(a, 0, i, default, copy_counter) = value;
                    }
                }
            } else {
                let empty = None;
                let mut children = match a.as_deref() {
                    Some(Node::Branch(children)) => children.clone(),
                    _ => std::array::from_fn(|_| None),
                };
                for i in 0..WIDTH {
                    let incoming = match b.as_deref() {
                        Some(Node::Branch(children)) => &children[i],
                        _ => &empty,
                    };
                    changed |= merge(
                        &mut children[i],
                        incoming,
                        height - 1,
                        default,
                        neutral,
                        copy_counter,
                        join,
                        cache.as_deref_mut(),
                    );
                }
                if changed {
                    *a = Some(Rc::new(Node::Branch(children)));
                }
            }
            if let Some(cache) = cache {
                // Reuse the incoming tree when the computed result is equal.
                // This canonical sharing lets later default/prefix joins hit
                // the same exact pair, instead of expanding it at every level.
                let incoming = if height == 0 {
                    (0..WIDTH).all(|i| Rows::at(a, 0, i, default) == Rows::at(b, 0, i, default))
                } else {
                    let child = |node: &Option<Rc<Node<T>>>, i: usize| match node.as_deref() {
                        Some(Node::Branch(children)) => {
                            children[i].as_ref().map_or(0, |n| Rc::as_ptr(n) as usize)
                        }
                        _ => 0,
                    };
                    (0..WIDTH).all(|i| child(a, i) == child(b, i))
                };
                if changed && incoming {
                    *a = b.clone();
                }
                cache.insert(
                    key,
                    Joined {
                        _inputs: inputs.unwrap(),
                        output: a.clone(),
                        changed,
                    },
                );
            }
            changed
        }
        merge(
            &mut self.root,
            &other.root,
            self.height,
            &self.default,
            neutral,
            self.copy_counter,
            join,
            cache,
        )
    }
    pub fn entries(&self) -> Vec<(usize, &T)> {
        fn visit<'a, T: PartialEq>(
            node: &'a Option<Rc<Node<T>>>,
            height: usize,
            base: usize,
            len: usize,
            default: &T,
            rows: &mut Vec<(usize, &'a T)>,
        ) {
            if base >= len {
                return;
            }
            match node.as_deref() {
                None => (),
                Some(Node::Leaf(values)) => {
                    for (i, value) in values.iter().take(len - base).enumerate() {
                        if value != default {
                            rows.push((base + i, value));
                        }
                    }
                }
                Some(Node::Branch(children)) => {
                    for (i, child) in children.iter().enumerate() {
                        visit(
                            child,
                            height - 1,
                            base + (i << (height * BITS)),
                            len,
                            default,
                            rows,
                        );
                    }
                }
            }
        }
        let mut rows = vec![];
        visit(
            &self.root,
            self.height,
            0,
            self.len,
            &self.default,
            &mut rows,
        );
        rows
    }
}
impl<T: Clone> Index<usize> for Rows<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        assert!(i < self.len);
        Self::at(&self.root, self.height, i, &self.default)
    }
}
impl<T: Clone> IndexMut<usize> for Rows<T> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        assert!(i < self.len);
        Self::at_mut(
            &mut self.root,
            self.height,
            i,
            &self.default,
            self.copy_counter,
        )
    }
}
impl<T: Clone + PartialEq> PartialEq for Rows<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.default == other.default && self.differences(other).is_empty()
    }
}
impl<T: Clone + Eq> Eq for Rows<T> {}
impl<T: Clone + std::fmt::Debug> std::fmt::Debug for Rows<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_joins_preserve_order_and_snapshot_mutations() {
        let join = |a: &mut Vec<u32>, b: &Vec<u32>| {
            for value in b {
                if !a.contains(value) {
                    a.push(*value);
                }
            }
        };
        let mut cache = JoinCache::new(join);
        for len in [1, 17, 257, 4097] {
            let mut left = Rows::new(vec![], len);
            let mut right = left.clone();
            for step in 0..100 {
                let index = (step * 971 + 7) % len;
                let value = step as u32;
                left[index] = vec![value];
                right[index] = vec![value + 1];
                let (a, b) = (left.clone(), right.clone());
                let mut expected = a.clone();
                expected.join(&b, join);
                let changed = left.join_cached(&b, &mut cache);
                assert_eq!(changed, expected != a);
                assert!(left == expected);
                let mut reverse = b.clone();
                let mut expected = b;
                expected.join(&a, join);
                let original = reverse.clone();
                let changed = reverse.join_cached(&a, &mut cache);
                assert_eq!(changed, expected != original);
                assert!(reverse == expected);
                assert_eq!(left[index], vec![value, value + 1]);
                assert_eq!(reverse[index], vec![value + 1, value]);
            }
        }
    }

    #[test]
    fn repeated_non_neutral_subtrees_are_joined_once() {
        #[derive(Clone, Default, PartialEq)]
        struct Row {
            proof: bool,
            history: u32,
        }
        fn measure(count: usize) -> usize {
            let calls = std::cell::Cell::new(0);
            let join = |a: &mut Row, b: &Row| {
                calls.set(calls.get() + 1);
                a.proof &= b.proof;
                a.history = a.history.max(b.history);
            };
            let mut cache = JoinCache::new(join);
            let base = Rows::new(Row::default(), count);
            let mut joined = base.clone();
            let mut plain = base.clone();
            for i in 0..count {
                let mut branch = base.clone();
                branch[i] = Row {
                    proof: true,
                    history: i as u32 + 1,
                };
                let mut expected = branch.clone();
                expected.join(&plain, |a, b| {
                    a.proof &= b.proof;
                    a.history = a.history.max(b.history);
                });
                branch.join_cached(&joined, &mut cache);
                assert!(branch == expected);
                joined = branch;
                plain = expected;
            }
            calls.get()
        }
        let small = measure(32);
        let large = measure(128);
        assert!(large <= 8 * small, "{small} / {large} row joins");
    }

    #[test]
    fn disjoint_unions_share_whole_subtrees() {
        let mut incoming = Rows::new(std::collections::BTreeSet::<u32>::new(), 4097)
            .with_copy_counter(Counter::ValidateCopiedRows);
        for i in 0..4097 {
            incoming[i].insert(i as u32);
        }
        let mut target = Rows::new(std::collections::BTreeSet::<u32>::new(), 4097)
            .with_copy_counter(Counter::ValidateCopiedRows);
        let before = work::read(Counter::ValidateCopiedRows);
        target.union(&incoming, |a, b| a.extend(b));
        assert_eq!(work::read(Counter::ValidateCopiedRows), before);
        assert!(target == incoming);
        target[17].clear();
        assert!(incoming[17].contains(&17));
        assert_eq!(target.entries().len(), 4096);
    }
    #[test]
    fn snapshot_updates_and_joins_equal_flat_rows() {
        for len in [1, 16, 17, 256, 257, 4097] {
            let mut actual = Rows::new(0u8, len).with_copy_counter(Counter::CopiedSlotRows);
            let mut expected = vec![0u8; len];
            let mut snapshots = vec![];
            for n in 0..100 {
                let i = (n * 971 + 7) % len;
                snapshots.push((actual.clone(), expected.clone()));
                actual[i] = (n % 7 + 1) as u8;
                expected[i] = (n % 7 + 1) as u8;
                assert!(actual.iter().eq(&expected));
            }
            for (old, flat) in snapshots {
                assert!(old.iter().eq(&flat));
                actual.join(&old, |a, b| *a |= b);
                for (a, b) in expected.iter_mut().zip(flat) {
                    *a |= b;
                }
                assert!(actual.iter().eq(&expected));
            }
            assert!(actual == actual.clone());
            let before = work::read(Counter::CopiedSlotRows);
            let mut one = actual.clone();
            one[0] ^= 1;
            assert_eq!(work::read(Counter::CopiedSlotRows) - before, WIDTH as u64);
        }
    }
}
