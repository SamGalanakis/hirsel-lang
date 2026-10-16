//! Primary binding identities with snapshot rows and mandatory holder updates.
use super::walk::{BindingKey, Identity};
use hirsel_types::OriginRoot;
use hirsel_types::rows::Rows;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
struct Keys {
    numbers: BTreeMap<BindingKey, usize>,
    ordered: Vec<BindingKey>,
}
#[derive(Clone)]
pub(super) struct Identities {
    keys: Rc<Keys>,
    values: Rows<Option<Identity>>,
    roots: Rc<RefCell<BTreeMap<OriginRoot, usize>>>,
    holders: Rows<Option<Rows<bool>>>,
}
impl PartialEq for Identities {
    fn eq(&self, other: &Self) -> bool {
        assert!(Rc::ptr_eq(&self.keys, &other.keys));
        self.values == other.values
    }
}
impl Eq for Identities {}
impl Identities {
    pub fn new(keys: impl IntoIterator<Item = BindingKey>) -> Self {
        let ordered: Vec<_> = keys
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let numbers = ordered
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, key)| (key, i))
            .collect();
        let len = ordered.len();
        Self {
            keys: Rc::new(Keys { numbers, ordered }),
            values: Rows::new(None, len)
                .with_copy_counter(hirsel_intern::work::Counter::CopiedSlotRows),
            roots: Rc::new(RefCell::new(BTreeMap::new())),
            holders: Rows::new(None, u32::MAX as usize)
                .with_copy_counter(hirsel_intern::work::Counter::CopiedSlotRows),
        }
    }
    pub fn get(&self, key: &BindingKey) -> Option<&Identity> {
        self.keys
            .numbers
            .get(key)
            .and_then(|i| self.values[*i].as_ref())
    }
    fn set(&mut self, index: usize, value: Option<Identity>) {
        if self.values[index] == value {
            return;
        }
        let roots = |value: &Option<Identity>| {
            value
                .as_ref()
                .into_iter()
                .flat_map(Identity::roots)
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        let before = roots(&self.values[index]);
        let after = roots(&value);
        for (root, present) in before
            .difference(&after)
            .map(|root| (root, false))
            .chain(after.difference(&before).map(|root| (root, true)))
        {
            let number = {
                let mut roots = self.roots.borrow_mut();
                let next = roots.len();
                *roots.entry(root.clone()).or_insert(next)
            };
            self.holders[number].get_or_insert_with(|| {
                Rows::new(false, self.keys.ordered.len())
                    .with_copy_counter(hirsel_intern::work::Counter::CopiedSlotRows)
            })[index] = present;
        }
        self.values[index] = value;
    }
    pub fn insert(&mut self, key: BindingKey, value: Identity) {
        let index = self.keys.numbers[&key];
        self.set(index, Some(value));
    }
    pub fn remove(&mut self, key: &BindingKey) {
        if let Some(index) = self.keys.numbers.get(key).copied() {
            self.set(index, None);
        }
    }
    pub fn held(&self, root: &OriginRoot) -> bool {
        self.held_outside(root, &BTreeSet::new())
    }
    pub fn held_outside(&self, root: &OriginRoot, excluded: &BTreeSet<BindingKey>) -> bool {
        let number = self.roots.borrow().get(root).copied();
        number
            .and_then(|i| self.holders[i].as_ref())
            .is_some_and(|rows| {
                rows.entries().into_iter().any(|(i, _)| {
                    hirsel_intern::work::count(hirsel_intern::work::Counter::AuthorityHolderRows);
                    !excluded.contains(&self.keys.ordered[i])
                })
            })
    }
    pub fn retain_agreeing(&mut self, other: &Self) {
        for index in self.values.differences(&other.values) {
            if self.values[index].is_some() {
                self.set(index, None);
            }
        }
    }
    pub fn changed_keys(&self, others: &[&Self]) -> Vec<BindingKey> {
        let mut indices = BTreeSet::new();
        for other in others {
            indices.extend(self.values.differences(&other.values));
        }
        indices
            .into_iter()
            .map(|i| self.keys.ordered[i].clone())
            .collect()
    }
}
