//! Keyed construction facts with the original replacement/insertion order.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::{Deref, DerefMut},
};
pub(super) struct OrderedRows<T> {
    values: BTreeMap<ExprId, (u64, T)>,
    order: BTreeMap<u64, ExprId>,
    next: u64,
}
impl<T> OrderedRows<T> {
    pub fn new() -> Self {
        Self {
            values: BTreeMap::new(),
            order: BTreeMap::new(),
            next: 0,
        }
    }
    fn remove(&mut self, key: &ExprId) -> Option<(u64, T)> {
        work::count(Counter::AuthoritySourceRows);
        let (sequence, value) = self.values.remove(key)?;
        self.order.remove(&sequence);
        Some((sequence, value))
    }
    pub fn insert(&mut self, key: ExprId, value: T) {
        self.remove(&key);
        let sequence = self.next;
        self.next += 1;
        self.order.insert(sequence, key);
        self.values.insert(key, (sequence, value));
    }
    pub fn get(&self, key: &ExprId) -> Option<&T> {
        work::count(Counter::AuthoritySourceRows);
        self.values.get(key).map(|(_, value)| value)
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.order.values().map(|key| {
            work::count(Counter::AuthoritySourceRows);
            &self.values[key].1
        })
    }
    pub fn into_sorted(self) -> Vec<T> {
        self.values.into_values().map(|(_, value)| value).collect()
    }
}
pub(super) struct ClosureRows {
    rows: OrderedRows<ClosureInfo>,
    types: BTreeMap<TypeId, BTreeSet<u64>>,
}
impl ClosureRows {
    pub fn new(values: Vec<ClosureInfo>) -> Self {
        let mut out = Self {
            rows: OrderedRows::new(),
            types: BTreeMap::new(),
        };
        for value in values {
            out.insert(value.expression, value);
        }
        out
    }
    fn unindex(&mut self, ty: TypeId, sequence: u64) {
        let empty = if let Some(entries) = self.types.get_mut(&ty) {
            entries.remove(&sequence);
            entries.is_empty()
        } else {
            false
        };
        if empty {
            self.types.remove(&ty);
        }
    }
    pub fn insert(&mut self, key: ExprId, value: ClosureInfo) {
        if let Some((sequence, old)) = self.rows.remove(&key) {
            self.unindex(old.ty, sequence);
        }
        let sequence = self.rows.next;
        self.types.entry(value.ty).or_default().insert(sequence);
        self.rows.insert(key, value);
    }
    pub fn get_mut(&mut self, key: &ExprId) -> Option<ClosureEdit<'_>> {
        let ty = self.rows.get(key)?.ty;
        Some(ClosureEdit {
            rows: self,
            key: *key,
            previous_type: ty,
        })
    }
    pub fn by_type(&self, ty: TypeId) -> Option<&ClosureInfo> {
        work::count(Counter::AuthoritySourceRows);
        let sequence = self.types.get(&ty)?.first()?;
        self.rows.get(self.rows.order.get(sequence)?)
    }
    pub fn iter(&self) -> impl Iterator<Item = &ClosureInfo> {
        self.rows.iter()
    }
    pub fn into_sorted(self) -> Vec<ClosureInfo> {
        self.rows.into_sorted()
    }
}
pub(super) struct ClosureEdit<'a> {
    rows: &'a mut ClosureRows,
    key: ExprId,
    previous_type: TypeId,
}
impl Deref for ClosureEdit<'_> {
    type Target = ClosureInfo;
    fn deref(&self) -> &ClosureInfo {
        &self.rows.rows.values[&self.key].1
    }
}
impl DerefMut for ClosureEdit<'_> {
    fn deref_mut(&mut self) -> &mut ClosureInfo {
        &mut self.rows.rows.values.get_mut(&self.key).unwrap().1
    }
}
impl Drop for ClosureEdit<'_> {
    fn drop(&mut self) {
        let (sequence, value) = &self.rows.rows.values[&self.key];
        let (sequence, ty) = (*sequence, value.ty);
        if ty != self.previous_type {
            self.rows.unindex(self.previous_type, sequence);
            self.rows.types.entry(ty).or_default().insert(sequence);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_order_and_current_type_index_follow_mutations() {
        let mut store = TypeStore::default();
        let a = store.push(Type::with_kind(TypeKind::Primitive(Primitive::U8)));
        let b = store.push(store.get(a).unwrap().clone());
        let row = |expression, ty| ClosureInfo {
            expression: ExprId(expression),
            ty,
            captures: vec![],
            result_evidence: None,
        };
        let mut rows = ClosureRows::new(vec![row(2, a), row(1, a)]);
        assert_eq!(rows.by_type(a).unwrap().expression, ExprId(2));
        rows.insert(ExprId(2), row(2, a));
        assert_eq!(
            rows.iter().map(|r| r.expression.0).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(rows.by_type(a).unwrap().expression, ExprId(1));
        rows.get_mut(&ExprId(1)).unwrap().ty = b;
        assert_eq!(rows.by_type(a).unwrap().expression, ExprId(2));
        assert_eq!(rows.by_type(b).unwrap().expression, ExprId(1));
        assert_eq!(
            rows.into_sorted()
                .iter()
                .map(|r| r.expression.0)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
}
