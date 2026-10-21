//! Primary retained dependencies with both value and owner views. Every write
//! updates the owner view; expiration has no whole-value-table fallback.
use super::{flow_origins::Environment, indexed::Indexed};
use hirsel_intern::work::Counter;
use hirsel_types::rows::Rows;
use hirsel_types::{Origin, OriginRoot};
use std::{collections::BTreeSet, ops::Index};

pub(super) trait Dependencies: Clone + Default + Eq {
    fn roots(&self) -> BTreeSet<OriginRoot>;
}
impl Dependencies for BTreeSet<Origin> {
    fn roots(&self) -> BTreeSet<OriginRoot> {
        self.iter().map(|origin| origin.root.clone()).collect()
    }
}
impl Dependencies for Environment {
    fn roots(&self) -> BTreeSet<OriginRoot> {
        self.retained()
            .into_iter()
            .map(|origin| origin.root)
            .collect()
    }
}

#[derive(Clone)]
pub(super) struct DependencyRows<T: Dependencies> {
    values: Rows<T>,
    holders: Indexed<OriginRoot, Rows<bool>>,
}
impl<T: Dependencies> Default for DependencyRows<T> {
    fn default() -> Self {
        Self::new(0)
    }
}
impl<T: Dependencies> PartialEq for DependencyRows<T> {
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values
    }
}
impl<T: Dependencies> Eq for DependencyRows<T> {}
impl<T: Dependencies> DependencyRows<T> {
    pub fn new(len: usize) -> Self {
        Self {
            values: Rows::new(T::default(), len).with_copy_counter(Counter::ValidateCopiedRows),
            holders: Indexed::default(),
        }
    }
    pub fn set(&mut self, index: usize, value: T) {
        if self.values[index] == value {
            return;
        }
        let before = self.values[index].roots();
        let after = value.roots();
        let len = self.values.len();
        for root in before.difference(&after) {
            self.holders
                .entry(root.clone())
                .or_insert_with(|| {
                    Rows::new(false, len).with_copy_counter(Counter::ValidateCopiedRows)
                })
                .set(index, false);
        }
        for root in after.difference(&before) {
            self.holders
                .entry(root.clone())
                .or_insert_with(|| {
                    Rows::new(false, len).with_copy_counter(Counter::ValidateCopiedRows)
                })
                .set(index, true);
        }
        self.values.set(index, value);
    }
    pub fn update(&mut self, index: usize, change: impl FnOnce(&mut T)) {
        let mut value = self.values[index].clone();
        change(&mut value);
        self.set(index, value);
    }
    pub fn holders(&self, root: &OriginRoot) -> Vec<usize> {
        self.holders
            .get(root)
            .map(|rows| rows.entries().into_iter().map(|(index, _)| index).collect())
            .unwrap_or_default()
    }
    /// Union `other`, which is `base` changed in place, into rows that
    /// already joined `base`: only rows changed since `base` are visited.
    pub fn union_since(&mut self, base: &Self, other: &Self, mut join: impl FnMut(&mut T, &T)) {
        for index in base.values.differences(&other.values) {
            let mut value = self.values[index].clone();
            join(&mut value, &other.values[index]);
            self.set(index, value);
        }
    }
}
impl<T: Dependencies> Index<usize> for DependencyRows<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        &self.values[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::lexical;
    #[test]
    fn overwrites_and_snapshot_joins_keep_exact_dependency_holders() {
        let a = Origin {
            root: OriginRoot::Local(lexical(), hirsel_types::LocalId(1)),
            fields: vec![].into(),
        };
        let b = Origin {
            root: OriginRoot::Local(lexical(), hirsel_types::LocalId(2)),
            fields: vec![].into(),
        };
        let mut rows = DependencyRows::new(64);
        rows.set(7, BTreeSet::from([a.clone()]));
        rows.set(31, BTreeSet::from([b.clone()]));
        let old = rows.clone();
        rows.set(7, BTreeSet::from([b.clone()]));
        assert!(rows.holders(&a.root).is_empty());
        assert_eq!(rows.holders(&b.root), vec![7, 31]);
        assert_eq!(old.holders(&a.root), vec![7]);
        let base = rows.clone();
        rows.union_since(&base, &old, |a, b| a.extend(b.iter().cloned()));
        assert_eq!(rows.holders(&a.root), vec![7]);
        assert_eq!(rows[7], BTreeSet::from([a, b]));
        rows.update(7, BTreeSet::clear);
        assert!(
            rows.holders(&OriginRoot::Local(lexical(), hirsel_types::LocalId(1)))
                .is_empty()
        );
    }
}
