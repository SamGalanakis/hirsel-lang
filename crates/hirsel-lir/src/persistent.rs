//! A persistent vector for dataflow states.
//!
//! A forward analysis keeps one state per block and per edge, and most of a
//! state is the same as its neighbours'. `PVec` is a 32-way tree whose clones
//! share every node: a clone is one reference count, a write copies the path
//! to its leaf (sized by the row, at most 32 elements), and `join` skips every subtree the two sides share, so a
//! state costs what it changes, not what it holds.
//!
//! Every node knows which of its element type's flags (`Element::flags`)
//! hold of everything below it. `join` skips a subtree whose flags say it
//! absorbs the other side's (`Element::absorbs`): the fan-in of many failure
//! edges into one cleanup block, where each edge differs from the joined
//! state in the few elements its own site touched and is dead storage in the
//! rest. `lacking` lists the elements without a flag, skipping every subtree
//! that has it throughout: the slots that hold something, the active loans.
//!
//! The shape of the tree is a function of the length alone, whatever pushes,
//! resizes and truncations built it, so equal vectors have equal trees:
//! equality stops at shared nodes, and a hash combines hashes cached per
//! node, which a write invalidates along its path only.
use std::cell::Cell;
use std::collections::HashSet;
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::{Index, IndexMut};
use std::rc::Rc;

const BITS: u32 = 5;
const WIDTH: usize = 1 << BITS;

/// Match a leaf's copied storage to an inner node's pointer array, as Immer
/// derives BL separately from B (immer/detail/rbts/node.hpp:1027-1039 at
/// bd4fc749b97dfa2b66a8f3de00bbf234db4856ef). Keep compact rows at width 32.
/// Large ownership rows then copy only the neighbours that fit in that space.
fn leaf_bits<T>() -> u32 {
    let bytes = WIDTH * std::mem::size_of::<Rc<()>>();
    (bytes / std::mem::size_of::<T>().max(1))
        .max(1)
        .ilog2()
        .min(BITS)
}

fn leaf_width<T>() -> usize {
    1 << leaf_bits::<T>()
}

/// The index shift of an inner node `height` levels above the leaves.
fn shift<T>(height: u32) -> u32 {
    height.saturating_sub(1) * BITS + leaf_bits::<T>()
}

/// The number of elements under a full node `height` levels above the leaves.
fn size<T>(height: u32) -> usize {
    leaf_width::<T>() << (height * BITS)
}

/// An element of a joined state.
pub trait Element: Clone + PartialEq {
    /// The properties of this element that operations skip by, as bits
    /// below `0x80`: dead storage, an inactive loan, storage holding nothing.
    fn flags(&self) -> u8;
    /// Whether joining any element with flags `other` into any element with
    /// flags `own` leaves it unchanged.
    fn absorbs(own: u8, other: u8) -> bool;
}

/// Set in a cached summary: the flags beside it are computed.
const KNOWN: u8 = 0x80;

#[derive(Clone)]
struct Node<T> {
    kind: Kind<T>,
    /// `KNOWN` and the flags every element below has, computed on demand;
    /// zero after a write.
    summary: Cell<u8>,
    /// The hash of everything below, computed on demand; zero after a write.
    hash: Cell<u64>,
}

#[derive(Clone)]
enum Kind<T> {
    Leaf(Vec<T>),
    Inner(Vec<Rc<Node<T>>>),
}

impl<T> Node<T> {
    fn new(kind: Kind<T>) -> Rc<Self> {
        Rc::new(Self {
            kind,
            summary: Cell::new(0),
            hash: Cell::new(0),
        })
    }

    /// A node `height` levels above the leaves with nothing below it.
    fn empty(height: u32) -> Rc<Self> {
        Self::new(if height == 0 {
            Kind::Leaf(Vec::new())
        } else {
            Kind::Inner(Vec::new())
        })
    }
}

/// The node behind `node` for writing: unshared, its caches cleared.
fn touch<T: Clone>(node: &mut Rc<Node<T>>) -> &mut Node<T> {
    let node = Rc::make_mut(node);
    node.summary.set(0);
    node.hash.set(0);
    node
}

impl<T: Element> Node<T> {
    /// The flags every element below has.
    fn flags(&self) -> u8 {
        let known = self.summary.get();
        if known != 0 {
            return known & !KNOWN;
        }
        let all = match &self.kind {
            Kind::Leaf(items) => items.iter().fold(!KNOWN, |all, x| all & x.flags()),
            Kind::Inner(children) => children.iter().fold(!KNOWN, |all, c| all & c.flags()),
        };
        self.summary.set(KNOWN | all);
        all
    }
}

impl<T: Hash> Node<T> {
    fn hash(&self) -> u64 {
        let known = self.hash.get();
        if known != 0 {
            return known;
        }
        let mut hasher = DefaultHasher::new();
        match &self.kind {
            Kind::Leaf(items) => items.hash(&mut hasher),
            Kind::Inner(children) => {
                for child in children {
                    hasher.write_u64(child.hash());
                }
            }
        }
        let hash = hasher.finish().max(1);
        self.hash.set(hash);
        hash
    }
}

#[derive(Clone)]
pub struct PVec<T> {
    root: Rc<Node<T>>,
    len: usize,
    /// Levels of inner nodes above the leaves: the least that hold `len`.
    depth: u32,
}

impl<T> Default for PVec<T> {
    fn default() -> Self {
        Self {
            root: Node::empty(0),
            len: 0,
            depth: 0,
        }
    }
}

impl<T: Clone> FromIterator<T> for PVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(items: I) -> Self {
        let mut out = Self::default();
        for item in items {
            out.push(item);
        }
        out
    }
}

impl<T> PVec<T> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        (index < self.len).then(|| &self[index])
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        (0..self.len).map(|i| &self[i])
    }
}

impl<T: Clone> PVec<T> {
    /// `len` copies of `value`, sharing one node per level.
    pub fn filled(len: usize, value: T) -> Self {
        let mut out = Self::default();
        out.resize(len, value);
        out
    }

    /// One more level above a full root.
    fn deepen(&mut self) {
        self.root = Node::new(Kind::Inner(vec![self.root.clone()]));
        self.depth += 1;
    }

    pub fn push(&mut self, value: T) {
        if self.len == size::<T>(self.depth) {
            self.deepen();
        }
        let mut node = touch(&mut self.root);
        let mut height = self.depth;
        loop {
            match &mut node.kind {
                Kind::Leaf(items) => {
                    items.push(value);
                    break;
                }
                Kind::Inner(children) => {
                    let i = (self.len >> shift::<T>(height)) & (WIDTH - 1);
                    if i == children.len() {
                        children.push(Node::empty(height - 1));
                    }
                    node = touch(&mut children[i]);
                    height -= 1;
                }
            }
        }
        self.len += 1;
    }

    /// Append a full subtree `height` levels above the leaves; the length is
    /// a multiple of its size.
    fn append(&mut self, subtree: Rc<Node<T>>, height: u32) {
        if self.len == 0 {
            self.root = subtree;
            self.depth = height;
            self.len = size::<T>(height);
            return;
        }
        while self.depth <= height || self.len == size::<T>(self.depth) {
            self.deepen();
        }
        let mut node = touch(&mut self.root);
        let mut at = self.depth;
        loop {
            let Kind::Inner(children) = &mut node.kind else {
                unreachable!("a subtree hangs off an inner node");
            };
            if at == height + 1 {
                children.push(subtree);
                break;
            }
            let i = (self.len >> shift::<T>(at)) & (WIDTH - 1);
            if i == children.len() {
                children.push(Node::empty(at - 1));
            }
            node = touch(&mut children[i]);
            at -= 1;
        }
        self.len += size::<T>(height);
    }

    /// Set the length, filling with `value`. The fill shares one node per
    /// level, so growing by any amount costs the height of the tree.
    pub fn resize(&mut self, len: usize, value: T) {
        if len < self.len {
            return self.truncate(len);
        }
        while self.len < len && !self.len.is_multiple_of(leaf_width::<T>()) {
            self.push(value.clone());
        }
        let mut full: Vec<Rc<Node<T>>> = Vec::new();
        while len - self.len >= leaf_width::<T>() {
            let mut height = 0;
            while len - self.len >= size::<T>(height + 1) && self.len % size::<T>(height + 1) == 0 {
                height += 1;
            }
            while full.len() <= height as usize {
                let kind = match full.last() {
                    None => Kind::Leaf(vec![value.clone(); leaf_width::<T>()]),
                    Some(below) => Kind::Inner(vec![below.clone(); WIDTH]),
                };
                full.push(Node::new(kind));
            }
            self.append(full[height as usize].clone(), height);
        }
        while self.len < len {
            self.push(value.clone());
        }
    }

    fn truncate(&mut self, len: usize) {
        if len == 0 {
            *self = Self::default();
            return;
        }
        let last = len - 1;
        let mut node = touch(&mut self.root);
        let mut height = self.depth;
        loop {
            match &mut node.kind {
                Kind::Leaf(items) => {
                    items.truncate((last & (leaf_width::<T>() - 1)) + 1);
                    break;
                }
                Kind::Inner(children) => {
                    let i = (last >> shift::<T>(height)) & (WIDTH - 1);
                    children.truncate(i + 1);
                    node = touch(&mut children[i]);
                    height -= 1;
                }
            }
        }
        self.len = len;
        while self.depth > 0 && len <= size::<T>(self.depth - 1) {
            let Kind::Inner(children) = &self.root.kind else {
                unreachable!("a root above the leaves is an inner node");
            };
            self.root = children[0].clone();
            self.depth -= 1;
        }
    }
}

impl<T: Element> PVec<T> {
    /// Every element without `flag`, with its index, in order. Subtrees
    /// that have it throughout are skipped whole.
    pub fn lacking(&self, flag: u8) -> Vec<(usize, &T)> {
        fn walk<'a, T: Element>(
            node: &'a Node<T>,
            flag: u8,
            base: usize,
            height: u32,
            out: &mut Vec<(usize, &'a T)>,
        ) {
            if node.flags() & flag != 0 {
                return;
            }
            match &node.kind {
                Kind::Leaf(items) => out.extend(
                    items
                        .iter()
                        .enumerate()
                        .filter(|(_, x)| x.flags() & flag == 0)
                        .map(|(i, x)| (base + i, x)),
                ),
                Kind::Inner(children) => {
                    for (i, child) in children.iter().enumerate() {
                        walk(
                            child,
                            flag,
                            base + i * size::<T>(height - 1),
                            height - 1,
                            out,
                        );
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, flag, 0, self.depth, &mut out);
        out
    }

    /// Join `other`, of the same length, into this vector element by element
    /// with `f`, and say whether anything changed. `f(x, y)` must leave `x`
    /// unchanged when `x == y`, and when `T::absorbs(x.flags(), y.flags())`.
    pub fn join(&mut self, other: &Self, mut f: impl FnMut(&mut T, &T)) -> bool {
        assert_eq!(self.len, other.len, "states of one unit");
        join(&mut self.root, &other.root, &mut f)
    }
}

fn join<T: Element>(a: &mut Rc<Node<T>>, b: &Rc<Node<T>>, f: &mut impl FnMut(&mut T, &T)) -> bool {
    if Rc::ptr_eq(a, b) || T::absorbs(a.flags(), b.flags()) {
        return false;
    }
    match (&a.kind, &b.kind) {
        (Kind::Leaf(xs), Kind::Leaf(ys)) => {
            let mut joined: Option<Vec<T>> = None;
            for (i, (x, y)) in xs.iter().zip(ys).enumerate() {
                if x == y {
                    continue;
                }
                let mut new = x.clone();
                f(&mut new, y);
                if new != *x {
                    joined.get_or_insert_with(|| xs.clone())[i] = new;
                }
            }
            match joined {
                Some(items) => {
                    *a = Node::new(Kind::Leaf(items));
                    true
                }
                None => false,
            }
        }
        (Kind::Inner(xs), Kind::Inner(ys)) => {
            let mut joined: Option<Vec<Rc<Node<T>>>> = None;
            for (i, (x, y)) in xs.iter().zip(ys).enumerate() {
                let mut child = x.clone();
                if join(&mut child, y, f) {
                    joined.get_or_insert_with(|| xs.clone())[i] = child;
                }
            }
            match joined {
                Some(children) => {
                    *a = Node::new(Kind::Inner(children));
                    true
                }
                None => false,
            }
        }
        _ => unreachable!("vectors of one length have one shape"),
    }
}

impl<T> Index<usize> for PVec<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        assert!(index < self.len, "index {index} of {}", self.len);
        let mut node = &self.root;
        let mut shift = shift::<T>(self.depth);
        loop {
            match &node.kind {
                Kind::Leaf(items) => return &items[index & (leaf_width::<T>() - 1)],
                Kind::Inner(children) => {
                    node = &children[(index >> shift) & (WIDTH - 1)];
                    shift = shift.saturating_sub(BITS);
                }
            }
        }
    }
}

impl<T: Clone> IndexMut<usize> for PVec<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        assert!(index < self.len, "index {index} of {}", self.len);
        let mut node = touch(&mut self.root);
        let mut shift = shift::<T>(self.depth);
        loop {
            match &mut node.kind {
                Kind::Leaf(items) => return &mut items[index & (leaf_width::<T>() - 1)],
                Kind::Inner(children) => {
                    node = touch(&mut children[(index >> shift) & (WIDTH - 1)]);
                    shift = shift.saturating_sub(BITS);
                }
            }
        }
    }
}

impl<T: Hash> Hash for PVec<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.len);
        state.write_u64(self.root.hash());
    }
}

/// Pairs of nodes found equal, kept while comparing two families of vectors
/// that share structure within each family but not with each other (the
/// states of two passes over one function): a pair is compared once however
/// many vectors hold it. The pairs name nodes by address, so a memo lives no
/// longer than the vectors it has compared.
pub struct Matched<'a, T> {
    pairs: HashSet<(*const Node<T>, *const Node<T>)>,
    held: std::marker::PhantomData<&'a PVec<T>>,
}

impl<T> Default for Matched<'_, T> {
    fn default() -> Self {
        Self {
            pairs: HashSet::new(),
            held: std::marker::PhantomData,
        }
    }
}

impl<T: PartialEq> PVec<T> {
    /// Equality, skipping every pair of nodes `matched` already holds and
    /// adding the pairs this comparison finds equal.
    pub fn matches<'a>(&'a self, other: &'a Self, matched: &mut Matched<'a, T>) -> bool {
        self.len == other.len && equal(&self.root, &other.root, &mut Some(matched))
    }
}

fn equal<T: PartialEq>(
    a: &Rc<Node<T>>,
    b: &Rc<Node<T>>,
    matched: &mut Option<&mut Matched<'_, T>>,
) -> bool {
    if Rc::ptr_eq(a, b) {
        return true;
    }
    let pair = (Rc::as_ptr(a), Rc::as_ptr(b));
    if matched.as_ref().is_some_and(|m| m.pairs.contains(&pair)) {
        return true;
    }
    let same = match (&a.kind, &b.kind) {
        (Kind::Leaf(xs), Kind::Leaf(ys)) => xs == ys,
        (Kind::Inner(xs), Kind::Inner(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| equal(x, y, matched))
        }
        _ => false,
    };
    if same && let Some(matched) = matched {
        matched.pairs.insert(pair);
    }
    same
}

impl<T: PartialEq> PartialEq for PVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && equal(&self.root, &other.root, &mut None)
    }
}

impl<T: Eq> Eq for PVec<T> {}

impl<T: fmt::Debug> fmt::Debug for PVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A join is the maximum, so every element absorbs a zero.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Max(u32);

    const ZERO: u8 = 1;
    const TOP: u8 = 2;

    impl Element for Max {
        fn flags(&self) -> u8 {
            u8::from(self.0 == 0) * ZERO
        }
        fn absorbs(_: u8, other: u8) -> bool {
            other & ZERO != 0
        }
    }

    /// Unequal elements join to the top: the top absorbs everything, and a
    /// zero absorbs a zero.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Mark(u32);

    impl Element for Mark {
        fn flags(&self) -> u8 {
            u8::from(self.0 == 0) * ZERO + u8::from(self.0 == u32::MAX) * TOP
        }
        fn absorbs(own: u8, other: u8) -> bool {
            own & TOP != 0 || own & other & ZERO != 0
        }
    }

    fn mark(x: &mut Mark, y: &Mark) {
        if x != y {
            x.0 = u32::MAX;
        }
    }

    fn hash_of(v: &PVec<u32>) -> u64 {
        let mut hasher = DefaultHasher::new();
        v.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn pushes_resizes_and_writes_agree_with_a_vector() {
        // A fixed linear congruential sequence drives the operations.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |bound: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize % bound
        };
        let mut model: Vec<u32> = Vec::new();
        let mut v: PVec<u32> = PVec::default();
        let mut kept: Vec<(Vec<u32>, PVec<u32>)> = Vec::new();
        for step in 0..3000 {
            match next(8) {
                0..=2 => {
                    let x = next(1000) as u32;
                    model.push(x);
                    v.push(x);
                }
                3 => {
                    let len = match next(6) {
                        0 => next(40),
                        1 => next(40_000),
                        2 => model.len() + next(70),
                        3 => model.len().saturating_sub(next(70)),
                        4 => model.len().next_multiple_of(1024) + 32 * next(3),
                        _ => next(1100),
                    };
                    let x = next(1000) as u32;
                    model.resize(len, x);
                    v.resize(len, x);
                }
                4..=6 if !model.is_empty() => {
                    let i = next(model.len());
                    let x = next(1000) as u32;
                    model[i] = x;
                    v[i] = x;
                }
                _ => kept.push((model.clone(), v.clone())),
            }
            assert_eq!(v.len(), model.len(), "step {step}");
            if step % 50 == 0 {
                assert!(v.iter().eq(model.iter()), "step {step}");
                // The same content built by pushes alone is the same tree.
                let pushed: PVec<u32> = model.iter().copied().collect();
                assert!(v == pushed, "step {step}: one shape per length");
                assert_eq!(hash_of(&v), hash_of(&pushed), "step {step}");
                assert_eq!(v.get(model.len()), None);
            }
        }
        assert!(v.iter().eq(model.iter()));
        for (model, kept) in &kept {
            assert!(kept.iter().eq(model.iter()), "a clone keeps its elements");
        }
    }

    #[test]
    fn lacking_lists_the_elements_without_a_flag() {
        for len in [0, 5, 32, 33, 2000, 40_000] {
            let mut v: PVec<Max> = PVec::filled(len, Max(0));
            assert!(v.lacking(ZERO).is_empty());
            let mut expected = Vec::new();
            for i in (0..len).step_by(977) {
                v[i] = Max(i as u32 + 1);
                expected.push((i, Max(i as u32 + 1)));
            }
            let found: Vec<_> = v.lacking(ZERO).into_iter().map(|(i, x)| (i, *x)).collect();
            assert_eq!(found, expected, "{len}");
        }
    }

    #[test]
    fn a_memo_compares_a_pair_of_nodes_once() {
        // Two families built apart: equal content, no shared node.
        let family = |bump: usize| -> Vec<PVec<u32>> {
            let mut v: PVec<u32> = (0..4000).collect();
            (0..50)
                .map(|i| {
                    v[i * 64] += 1;
                    if i == bump {
                        v[3999] = 0;
                    }
                    v.clone()
                })
                .collect()
        };
        let (a, b, c) = (family(usize::MAX), family(usize::MAX), family(40));
        let mut matched = Matched::default();
        for (x, y) in a.iter().zip(&b) {
            assert!(x.matches(y, &mut matched));
        }
        // 125 leaves and their parents once, then one path per vector.
        assert!(matched.pairs.len() < 400, "{}", matched.pairs.len());
        let mut matched = Matched::default();
        let agree: Vec<bool> = a
            .iter()
            .zip(&c)
            .map(|(x, y)| x.matches(y, &mut matched))
            .collect();
        assert_eq!(agree.iter().filter(|x| **x).count(), 40);
        assert!(agree[..40].iter().all(|x| *x));
    }

    /// Immer's separate B/BL shapes, including one-element leaves: vector
    /// operations must agree with dense storage at every leaf/branch boundary.
    #[test]
    fn large_rows_keep_canonical_shapes_across_every_tree_boundary() {
        fn rows<const N: usize>() {
            let mut dense = Vec::new();
            let mut tree = PVec::default();
            let mut kept = Vec::new();
            for len in [1, 2, 3, 31, 32, 33, 63, 64, 65, 1023, 1024, 1025, 4097] {
                dense.resize(len, [len as u64; N]);
                tree.resize(len, [len as u64; N]);
                for i in [0, len / 2, len - 1] {
                    dense[i] = [i as u64; N];
                    tree[i] = [i as u64; N];
                }
                kept.push((dense.clone(), tree.clone()));
                let pushed: PVec<_> = dense.iter().copied().collect();
                assert_eq!(tree, pushed);
                let mut matched = Matched::default();
                assert!(tree.matches(&pushed, &mut matched));
                let hash = |v: &PVec<[u64; N]>| {
                    let mut h = DefaultHasher::new();
                    v.hash(&mut h);
                    h.finish()
                };
                assert_eq!(hash(&tree), hash(&pushed));
            }
            for (dense, mut tree) in kept {
                tree.resize(1, [0; N]);
                tree.push([99; N]);
                assert_eq!(
                    tree.iter().copied().collect::<Vec<_>>(),
                    [dense[0], [99; N]]
                );
            }
        }
        rows::<0>();
        rows::<1>();
        rows::<2>();
        rows::<4>();
        rows::<8>();
        rows::<16>();
        rows::<64>();
    }

    #[test]
    fn a_sparse_large_row_write_does_not_copy_a_full_branch_of_rows() {
        #[derive(Debug, PartialEq)]
        struct Large {
            value: [u64; 16],
            clones: Rc<Cell<usize>>,
        }
        impl Clone for Large {
            fn clone(&self) -> Self {
                self.clones.set(self.clones.get() + 1);
                Self {
                    value: self.value,
                    clones: self.clones.clone(),
                }
            }
        }
        let clones = Rc::new(Cell::new(0));
        let base: PVec<_> = (0..4096)
            .map(|i| Large {
                value: [i; 16],
                clones: clones.clone(),
            })
            .collect();
        let mut changed = base.clone();
        clones.set(0);
        changed[1025].value[0] = 0;
        assert!(
            clones.get() <= 2,
            "a sparse write copied {} large rows",
            clones.get()
        );
        assert_eq!(base[1025].value, [1025; 16]);
        assert_ne!(changed, base);
    }

    #[test]
    fn a_join_agrees_with_the_element_wise_join() {
        let n = 3000;
        let dense = |seed: u32| -> Vec<Mark> {
            (0..n)
                .map(|i: u32| {
                    Mark(if (i.wrapping_mul(seed) >> 3) % 5 == 0 {
                        i % 3
                    } else {
                        0
                    })
                })
                .collect()
        };
        for (s, t) in [(3, 7), (11, 11), (1, 64), (64, 1)] {
            let (a, b) = (dense(s), dense(t));
            let mut expected = a.clone();
            for (x, y) in expected.iter_mut().zip(&b) {
                mark(x, y);
            }
            let mut joined: PVec<Mark> = a.iter().copied().collect();
            let other: PVec<Mark> = b.iter().copied().collect();
            let changed = joined.join(&other, mark);
            assert_eq!(changed, expected != a);
            assert_eq!(joined.iter().copied().collect::<Vec<_>>(), expected);
            // Joining again changes nothing, whatever is shared by now.
            assert!(!joined.join(&other, mark));
            let mut shared = joined.clone();
            shared[5] = Mark(9);
            assert!(joined.join(&shared, mark));
            assert_eq!(joined[5], Mark(u32::MAX));
        }
    }

    #[test]
    fn a_join_calls_nothing_for_shared_or_absorbed_subtrees() {
        let base: PVec<Max> = (0..5000).map(|_| Max(0)).collect();
        let mut acc = base.clone();
        for i in 0..5000 {
            acc[i] = Max(7);
        }
        // A state that differs from the base in one element meets the joined
        // state: only that element's leaf is visited.
        let mut one = base.clone();
        one[4321] = Max(9);
        let mut calls = 0;
        let changed = acc.join(&one, |x, y| {
            calls += 1;
            x.0 = x.0.max(y.0);
        });
        assert!(changed);
        assert!(calls <= WIDTH, "{calls}");
        assert_eq!(acc[4321], Max(9));
        // A clone written and joined back visits one leaf too.
        let mut next = acc.clone();
        next[17] = Max(100);
        let mut calls = 0;
        assert!(acc.join(&next, |x, y| {
            calls += 1;
            x.0 = x.0.max(y.0);
        }));
        assert_eq!(calls, 1);
    }
}
