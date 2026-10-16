//! Ordered live roots are the primary cleanup input. Immutable radix nodes retain
//! their compiled chain aggregate; a changed membership rebuilds only its path.
use hirsel_intern::work::{self, Counter};
use hirsel_tir::CleanupNodeId;
use std::{cell::RefCell, rc::Rc};
const BITS: usize = 3;
const WIDTH: usize = 1 << BITS;
const HEIGHT: usize = 10; // Covers every u32 local ID.
#[derive(Clone, Default)]
pub(super) struct Live {
    root: Option<Rc<Node>>,
}
struct Node {
    data: Data,
    /// One memo per compilation pass key (scope depth and storage class).
    compiled: RefCell<Vec<(u32, Compiled)>>,
    scopes: std::cell::OnceCell<(u32, u32)>,
}
enum Data {
    Branch([Option<Rc<Node>>; WIDTH]),
    Leaf([bool; WIDTH]),
}
#[derive(Clone, Copy)]
struct Compiled {
    epoch: usize,
    parent: Option<CleanupNodeId>,
    head: Option<CleanupNodeId>,
}
impl Clone for Node {
    fn clone(&self) -> Self {
        Self::new(match &self.data {
            Data::Branch(children) => Data::Branch(children.clone()),
            Data::Leaf(values) => Data::Leaf(*values),
        })
    }
}
impl Node {
    fn new(data: Data) -> Self {
        Self {
            data,
            compiled: RefCell::new(vec![]),
            scopes: std::cell::OnceCell::new(),
        }
    }
}
impl Live {
    fn at(node: &Option<Rc<Node>>, height: usize, root: u32) -> bool {
        match node.as_deref().map(|node| &node.data) {
            None => false,
            Some(Data::Leaf(values)) => values[root as usize % WIDTH],
            Some(Data::Branch(children)) => Self::at(
                &children[(root as usize >> (height * BITS)) % WIDTH],
                height - 1,
                root,
            ),
        }
    }
    fn change(slot: &mut Option<Rc<Node>>, height: usize, root: u32, live: bool) {
        let empty = {
            let node = slot.get_or_insert_with(|| {
                Rc::new(Node::new(if height == 0 {
                    Data::Leaf([false; WIDTH])
                } else {
                    Data::Branch(std::array::from_fn(|_| None))
                }))
            });
            let node = Rc::make_mut(node);
            node.compiled = RefCell::new(vec![]);
            node.scopes = std::cell::OnceCell::new();
            match &mut node.data {
                Data::Leaf(values) => {
                    values[root as usize % WIDTH] = live;
                    !values.iter().any(|value| *value)
                }
                Data::Branch(children) => {
                    Self::change(
                        &mut children[(root as usize >> (height * BITS)) % WIDTH],
                        height - 1,
                        root,
                        live,
                    );
                    children.iter().all(Option::is_none)
                }
            }
        };
        if empty {
            *slot = None;
        }
    }
    pub fn contains(&self, root: &u32) -> bool {
        Self::at(&self.root, HEIGHT, *root)
    }
    pub fn insert(&mut self, root: u32) -> bool {
        if self.contains(&root) {
            false
        } else {
            Self::change(&mut self.root, HEIGHT, root, true);
            true
        }
    }
    pub fn remove(&mut self, root: &u32) -> bool {
        if !self.contains(root) {
            false
        } else {
            Self::change(&mut self.root, HEIGHT, *root, false);
            true
        }
    }
    #[cfg(test)]
    fn roots(node: &Option<Rc<Node>>, height: usize, base: u64, start: u64, roots: &mut Vec<u32>) {
        if base + (1u64 << ((height + 1) * BITS)) <= start {
            return;
        }
        let Some(node) = node else {
            return;
        };
        match &node.data {
            Data::Leaf(values) => {
                for (index, live) in values.iter().enumerate() {
                    let root = base + index as u64;
                    if *live && root >= start {
                        roots.push(root as u32);
                    }
                }
            }
            Data::Branch(children) => {
                for (index, child) in children.iter().enumerate() {
                    Self::roots(
                        child,
                        height - 1,
                        base + ((index as u64) << (height * BITS)),
                        start,
                        roots,
                    );
                }
            }
        }
    }
    #[cfg(test)]
    pub fn roots_from(&self, start: u32) -> Vec<u32> {
        let mut roots = vec![];
        Self::roots(&self.root, HEIGHT, 0, start as u64, &mut roots);
        roots
    }
    #[cfg(test)]
    pub fn iter(&self) -> std::vec::IntoIter<u32> {
        self.roots_from(0).into_iter()
    }
    pub fn union(&mut self, other: &Self) {
        fn merge(a: &mut Option<Rc<Node>>, b: &Option<Rc<Node>>) {
            let Some(incoming) = b else {
                return;
            };
            let Some(old) = a else {
                *a = b.clone();
                return;
            };
            if Rc::ptr_eq(old, incoming) {
                return;
            }
            match (&old.data, &incoming.data) {
                (Data::Leaf(x), Data::Leaf(y)) => {
                    let joined = std::array::from_fn(|i| x[i] || y[i]);
                    if &joined != x {
                        *a = Some(Rc::new(Node::new(Data::Leaf(joined))));
                    }
                }
                (Data::Branch(x), Data::Branch(y)) => {
                    let mut children = x.clone();
                    let mut changed = false;
                    for i in 0..WIDTH {
                        let previous = children[i].clone();
                        merge(&mut children[i], &y[i]);
                        changed |= match (&previous, &children[i]) {
                            (None, None) => false,
                            (Some(a), Some(b)) => !Rc::ptr_eq(a, b),
                            _ => true,
                        };
                    }
                    if changed {
                        *a = Some(Rc::new(Node::new(Data::Branch(children))));
                    }
                }
                _ => unreachable!(),
            }
        }
        merge(&mut self.root, &other.root);
    }
    pub fn compile(
        &self,
        pass: u32,
        epoch: usize,
        parent: Option<CleanupNodeId>,
        action: &mut impl FnMut(u32, Option<CleanupNodeId>) -> Option<CleanupNodeId>,
    ) -> Option<CleanupNodeId> {
        fn visit(
            node: &Option<Rc<Node>>,
            height: usize,
            base: u64,
            pass: u32,
            epoch: usize,
            mut head: Option<CleanupNodeId>,
            action: &mut impl FnMut(u32, Option<CleanupNodeId>) -> Option<CleanupNodeId>,
        ) -> Option<CleanupNodeId> {
            let Some(node) = node else {
                return head;
            };
            let memo = node
                .compiled
                .borrow()
                .iter()
                .find(|(key, _)| *key == pass)
                .map(|(_, compiled)| *compiled);
            if let Some(old) = memo {
                if old.epoch == epoch {
                    if old.head == old.parent {
                        return head;
                    }
                    if old.parent == head {
                        return old.head;
                    }
                }
            }
            let parent = head;
            match &node.data {
                Data::Leaf(values) => {
                    for (index, live) in values.iter().enumerate() {
                        if *live {
                            work::count(Counter::LoweringCleanupRootRows);
                            head = action((base + index as u64) as u32, head);
                        }
                    }
                }
                Data::Branch(children) => {
                    for (index, child) in children.iter().enumerate() {
                        head = visit(
                            child,
                            height - 1,
                            base + ((index as u64) << (height * BITS)),
                            pass,
                            epoch,
                            head,
                            action,
                        );
                    }
                }
            }
            let compiled = Compiled {
                epoch,
                parent,
                head,
            };
            let mut memo = node.compiled.borrow_mut();
            match memo.iter_mut().find(|(key, _)| *key == pass) {
                Some((_, old)) => *old = compiled,
                None => memo.push((pass, compiled)),
            }
            head
        }
        visit(&self.root, HEIGHT, 0, pass, epoch, parent, action)
    }
    pub fn roots_in_scopes(&self, first: u32, end: u32, scope: &impl Fn(u32) -> u32) -> Vec<u32> {
        fn bounds(
            node: &Node,
            height: usize,
            base: u64,
            scope: &impl Fn(u32) -> u32,
        ) -> (u32, u32) {
            *node.scopes.get_or_init(|| match &node.data {
                Data::Leaf(values) => values
                    .iter()
                    .enumerate()
                    .filter(|(_, live)| **live)
                    .map(|(i, _)| {
                        work::count(Counter::LoweringRetirementRows);
                        let s = scope((base + i as u64) as u32);
                        (s, s)
                    })
                    .fold((u32::MAX, 0), |(a, b), (c, d)| (a.min(c), b.max(d))),
                Data::Branch(children) => children
                    .iter()
                    .enumerate()
                    .filter_map(|(i, child)| {
                        child.as_ref().map(|child| {
                            bounds(
                                child,
                                height - 1,
                                base + ((i as u64) << (height * BITS)),
                                scope,
                            )
                        })
                    })
                    .fold((u32::MAX, 0), |(a, b), (c, d)| (a.min(c), b.max(d))),
            })
        }
        fn visit(
            node: &Option<Rc<Node>>,
            height: usize,
            base: u64,
            first: u32,
            end: u32,
            scope: &impl Fn(u32) -> u32,
            roots: &mut Vec<u32>,
        ) {
            let Some(node) = node else {
                return;
            };
            let (min, max) = bounds(node, height, base, scope);
            if max < first || min >= end {
                return;
            }
            match &node.data {
                Data::Leaf(values) => {
                    for (index, live) in values.iter().enumerate() {
                        let root = (base + index as u64) as u32;
                        if *live && {
                            work::count(Counter::LoweringRetirementRows);
                            (first..end).contains(&scope(root))
                        } {
                            roots.push(root);
                        }
                    }
                }
                Data::Branch(children) => {
                    for (index, child) in children.iter().enumerate() {
                        visit(
                            child,
                            height - 1,
                            base + ((index as u64) << (height * BITS)),
                            first,
                            end,
                            scope,
                            roots,
                        );
                    }
                }
            }
        }
        let mut roots = vec![];
        visit(&self.root, HEIGHT, 0, first, end, scope, &mut roots);
        roots
    }
    /// Exact ordered set difference, including old storage becoming live again.
    pub fn added_since(&self, previous: &Self) -> Vec<u32> {
        fn visit(
            now: &Option<Rc<Node>>,
            old: &Option<Rc<Node>>,
            height: usize,
            base: u64,
            added: &mut Vec<u32>,
        ) {
            let Some(now) = now else { return };
            if old.as_ref().is_some_and(|old| Rc::ptr_eq(now, old)) {
                return;
            }
            match (&now.data, old.as_ref().map(|node| &node.data)) {
                (Data::Leaf(values), old) => {
                    let old = match old {
                        Some(Data::Leaf(values)) => Some(values),
                        None => None,
                        _ => unreachable!(),
                    };
                    for (index, live) in values.iter().enumerate() {
                        work::count(Counter::LoweringRetirementRows);
                        if *live && old.is_none_or(|old| !old[index]) {
                            added.push((base + index as u64) as u32);
                        }
                    }
                }
                (Data::Branch(children), old) => {
                    let old = match old {
                        Some(Data::Branch(children)) => Some(children),
                        None => None,
                        _ => unreachable!(),
                    };
                    for (index, child) in children.iter().enumerate() {
                        visit(
                            child,
                            old.map(|old| &old[index]).unwrap_or(&None),
                            height - 1,
                            base + ((index as u64) << (height * BITS)),
                            added,
                        );
                    }
                }
            }
        }
        let mut added = vec![];
        visit(&self.root, &previous.root, HEIGHT, 0, &mut added);
        added
    }
}
impl FromIterator<u32> for Live {
    fn from_iter<I: IntoIterator<Item = u32>>(iter: I) -> Self {
        let mut live = Self::default();
        for root in iter {
            live.insert(root);
        }
        live
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};
    #[test]
    fn snapshots_ranges_and_unions_preserve_ordered_membership() {
        let ids = [0, 1, 7, 8, 63, 64, 511, 512, 4095, 4096, u32::MAX];
        let mut live: Live = ids.into_iter().collect();
        let saved = live.clone();
        for root in ids.into_iter().step_by(2) {
            assert!(live.remove(&root));
        }
        assert_eq!(saved.iter().collect::<Vec<_>>(), ids);
        for start in ids {
            assert_eq!(
                saved.roots_from(start),
                ids.into_iter().filter(|r| *r >= start).collect::<Vec<_>>()
            );
        }
        live.union(&saved);
        assert_eq!(live.iter().collect::<Vec<_>>(), ids);
        assert!(!live.insert(8));
        for root in live.roots_from(512) {
            live.remove(&root);
        }
        assert_eq!(
            live.iter().collect::<BTreeSet<_>>(),
            ids.into_iter().filter(|root| *root < 512).collect()
        );
        assert_eq!(saved.iter().count(), ids.len());
    }
    #[test]
    fn compiled_heads_preserve_lazy_node_order_after_edits_and_ownership_changes() {
        let mut live = Live::default();
        let mut named = BTreeSet::new();
        let mut table = HashMap::new();
        let mut nodes = vec![];
        let mut expected_table = HashMap::new();
        let mut expected_nodes = vec![];
        let mut epoch = 0;
        for step in 0..256u32 {
            live.insert(step);
            if step % 3 == 0 {
                named.insert(step);
                epoch += 1;
            }
            if step > 3 && step % 5 == 0 {
                live.remove(&(step - 3));
            }
            if step > 0 && step % 7 == 0 {
                named.insert(step - 1);
                epoch += 1;
            }
            for _ in 0..2 {
                let mut head = None;
                for temporary in [false, true] {
                    head = live.compile(temporary as u32, epoch, head, &mut |root, parent| {
                        if (root % 2 == 0) == temporary && named.contains(&root) {
                            Some(*table.entry((root, parent)).or_insert_with(|| {
                                let id = CleanupNodeId(nodes.len() as u32);
                                nodes.push((root, parent));
                                id
                            }))
                        } else {
                            parent
                        }
                    });
                }
                let mut expected = None;
                for temporary in [false, true] {
                    for root in live.iter() {
                        if (root % 2 == 0) == temporary && named.contains(&root) {
                            expected = Some(
                                *expected_table.entry((root, expected)).or_insert_with(|| {
                                    let id = CleanupNodeId(expected_nodes.len() as u32);
                                    expected_nodes.push((root, expected));
                                    id
                                }),
                            );
                        }
                    }
                }
                assert_eq!(head, expected);
                assert_eq!(nodes, expected_nodes);
            }
        }
    }
}

#[cfg(test)]
mod retirement_tests {
    use super::*;
    use std::collections::BTreeSet;
    #[test]
    fn differences_include_reactivated_slots_and_scope_bounds_follow_snapshots() {
        let mut live: Live = [0, 7, 8, 511, 512, 4096].into_iter().collect();
        let before = live.clone();
        live.remove(&7);
        live.insert(3);
        live.insert(999);
        assert_eq!(live.added_since(&before), vec![3, 999]);
        let empty = Live::default();
        assert_eq!(live.added_since(&empty), live.iter().collect::<Vec<_>>());
        assert!(live.added_since(&live).is_empty());
        let scope = |root| root / 8;
        for (first, end) in [(0, 1), (1, 64), (64, 128), (128, 1024)] {
            let selected = live.roots_in_scopes(first, end, &scope);
            assert_eq!(
                selected,
                live.iter()
                    .filter(|root| (first..end).contains(&scope(*root)))
                    .collect::<Vec<_>>()
            );
        }
        let saved = live.clone();
        live.remove(&999);
        live.insert(7);
        assert_eq!(live.added_since(&saved), vec![7]);
        assert_eq!(saved.roots_in_scopes(120, 128, &scope), vec![999]);
        assert!(live.roots_in_scopes(120, 128, &scope).is_empty());
        assert_eq!(
            before.iter().collect::<BTreeSet<_>>(),
            [0, 7, 8, 511, 512, 4096].into()
        );
    }
}
