//! Canonical sets of live release obligations. IDs are collision-free structural
//! identities, not hashes. Fixed-width radix paths retain 32 nodes per update;
//! equal states reached by different branches have the same root ID.
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Sets {
    nodes: Vec<(usize, usize)>,
    interned: HashMap<(usize, usize), usize>,
}
impl Sets {
    fn children(&self, root: usize) -> (usize, usize) {
        if root == 0 {
            (0, 0)
        } else {
            self.nodes[root - 2]
        }
    }
    fn node(&mut self, children: (usize, usize)) -> usize {
        if children == (0, 0) {
            return 0;
        }
        *self.interned.entry(children).or_insert_with(|| {
            let id = self.nodes.len() + 2;
            self.nodes.push(children);
            id
        })
    }
    /// The primary set's actual members in ascending numeric order.
    pub fn members(&self, root: usize) -> Vec<u32> {
        let mut result = Vec::new();
        let mut pending = vec![(root, 0u32, 0u32)];
        while let Some((node, prefix, depth)) = pending.pop() {
            if node == 0 {
                continue;
            }
            if depth == 32 {
                result.push(prefix);
            } else {
                let (left, right) = self.children(node);
                pending.push((right, prefix | (1 << (31 - depth)), depth + 1));
                pending.push((left, prefix, depth + 1));
            }
        }
        result
    }
    /// Empty is zero, the present leaf is one. Internal nodes start at two.
    pub fn set(&mut self, mut root: usize, key: u32, present: bool) -> usize {
        let mut path = [(0, 0); 32];
        for (level, slot) in path.iter_mut().enumerate() {
            *slot = self.children(root);
            root = if key & (1 << (31 - level)) == 0 {
                slot.0
            } else {
                slot.1
            };
        }
        root = usize::from(present);
        for (level, mut children) in path.into_iter().enumerate().rev() {
            if key & (1 << (31 - level)) == 0 {
                children.0 = root;
            } else {
                children.1 = root;
            }
            root = self.node(children);
        }
        root
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Chain {
    pub roots: usize,
    pub loans: usize,
    pub first_loan: Option<u32>,
    /// The release that runs next: scope depth, temporary, root.
    pub next: Option<(usize, bool, usize)>,
    pub temporary: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_members_preserve_boundary_keys_and_prior_roots() {
        let mut sets = Sets::default();
        let keys = [0u32, 32, 1 << 31, u32::MAX];
        let mut root = 0;
        for key in keys {
            root = sets.set(root, key, true);
        }
        let old = root;
        root = sets.set(root, 32, false);
        assert_eq!(sets.members(old), keys.to_vec());
        assert_eq!(sets.members(root), vec![0, 1 << 31, u32::MAX]);
        for key in [0, 1 << 31, u32::MAX] {
            root = sets.set(root, key, false);
        }
        assert!(sets.members(root).is_empty());
    }
    #[test]
    fn root_identity_is_independent_of_insertion_order_and_updates_are_bounded() {
        for n in [32, 128] {
            let mut sets = Sets::default();
            let mut a = 0;
            for key in 0..n {
                a = sets.set(a, key, true);
            }
            let first_nodes = sets.nodes.len();
            assert!(first_nodes <= 32 * n as usize);
            let mut b = 0;
            for key in (0..n).rev() {
                b = sets.set(b, key, true);
            }
            assert_eq!(a, b);
            for key in (0..n).rev() {
                a = sets.set(a, key, false);
            }
            assert_eq!(a, 0);
            assert!(sets.nodes.len() <= 64 * n as usize);
        }
    }
}
