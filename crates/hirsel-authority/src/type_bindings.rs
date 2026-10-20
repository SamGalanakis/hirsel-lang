//! Primary binding-type rows; branch snapshots share unchanged radix nodes.
use super::walk::BindingKey;
use hirsel_types::TypeId;
use hirsel_types::rows::Rows;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Index,
    rc::Rc,
};
struct Keys {
    numbers: BTreeMap<BindingKey, usize>,
    ordered: Vec<BindingKey>,
}
#[derive(Clone)]
pub(super) struct BindingTypes {
    keys: Rc<Keys>,
    values: Rows<Option<TypeId>>,
}
impl BindingTypes {
    pub fn new(bindings: impl IntoIterator<Item = (BindingKey, TypeId)>) -> Self {
        let bindings: BTreeMap<_, _> = bindings.into_iter().collect();
        let ordered: Vec<_> = bindings.keys().cloned().collect();
        let numbers = ordered
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, k)| (k, i))
            .collect();
        let mut out = Self {
            values: Rows::new(None, ordered.len())
                .with_copy_counter(hirsel_intern::work::Counter::CopiedSlotRows),
            keys: Rc::new(Keys { numbers, ordered }),
        };
        for (key, value) in bindings {
            out.insert(key, value);
        }
        out
    }
    pub fn get(&self, key: &BindingKey) -> Option<&TypeId> {
        self.keys
            .numbers
            .get(key)
            .and_then(|i| self.values[*i].as_ref())
    }
    pub fn insert(&mut self, key: BindingKey, value: TypeId) {
        let i = self.keys.numbers[&key];
        if self.values[i] != Some(value) {
            self.values[i] = Some(value);
        }
    }
    pub fn keys(&self) -> impl Iterator<Item = &BindingKey> {
        self.keys.ordered.iter()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&BindingKey, &TypeId)> {
        self.keys()
            .map(|key| (key, self.get(key).expect("initial binding row")))
    }
    pub fn len(&self) -> usize {
        self.keys.ordered.len()
    }
    pub fn changed_keys(&self, others: &[&Self]) -> Vec<BindingKey> {
        let mut changed = BTreeSet::new();
        for other in others {
            assert!(Rc::ptr_eq(&self.keys, &other.keys));
            changed.extend(self.values.differences(&other.values));
        }
        changed
            .into_iter()
            .map(|i| self.keys.ordered[i].clone())
            .collect()
    }
}
impl Index<&BindingKey> for BindingTypes {
    type Output = TypeId;
    fn index(&self, key: &BindingKey) -> &TypeId {
        self.get(key).expect("binding row")
    }
}
