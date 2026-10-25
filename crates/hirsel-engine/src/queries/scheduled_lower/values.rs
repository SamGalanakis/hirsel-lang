//! Private immutable value sharing. Arc keeps one payload per equal value;
//! BTreeMap indexes candidates, while full node equality guards hash collisions.
//! The producer retains and debits every pool allocation until its last owner.
use hirsel_syntax::{ContentHash, ExprId};
use hirsel_tir::{Materialized, MaterializedRef};
use std::{collections::BTreeMap, ops::Deref, sync::Arc};

#[derive(Clone, serde::Serialize)]
#[serde(transparent)]
pub(super) struct SharedMaterialized(MaterializedRef);
impl From<Materialized> for SharedMaterialized {
    fn from(value: Materialized) -> Self {
        Self(Arc::new(value))
    }
}
impl Deref for SharedMaterialized {
    type Target = Materialized;
    fn deref(&self) -> &Materialized {
        &self.0
    }
}
impl SharedMaterialized {
    pub(super) fn owned(&self) -> Materialized {
        (**self).clone()
    }
}
pub(super) fn expand(values: &[(ExprId, SharedMaterialized)]) -> Vec<(ExprId, Materialized)> {
    values
        .iter()
        .map(|(id, value)| (*id, value.owned()))
        .collect()
}

#[derive(Default)]
pub(super) struct MaterializedPool {
    values: BTreeMap<ContentHash, Box<[MaterializedRef]>>,
}
fn table_bytes(rows: usize) -> u64 {
    if rows == 0 {
        return 0;
    }
    let mut weight = crate::NativeMetadataWeight::default();
    weight.tree::<ContentHash, Box<[MaterializedRef]>>(rows);
    weight.bytes()
}
impl MaterializedPool {
    pub(super) fn intern(
        &mut self,
        key: ContentHash,
        value: &mut SharedMaterialized,
        available: &mut u64,
    ) -> Option<()> {
        let bucket = self.values.get(&key);
        if let Some(previous) =
            bucket.and_then(|values| values.iter().find(|v| v.root == value.root))
        {
            value.0 = previous.clone();
            return Some(());
        }
        let next_rows = self
            .values
            .len()
            .checked_add(usize::from(bucket.is_none()))?;
        let growth = table_bytes(next_rows).checked_sub(table_bytes(self.values.len()))?;
        let mut weight = crate::NativeMetadataWeight::default();
        weight.arc(&value.0);
        weight.materialized(value);
        // Collision buckets are exact boxed slices, never spare Vec capacity.
        weight.add(size_of::<MaterializedRef>());
        let charge = weight.bytes().checked_add(growth)?;
        let remaining = available.checked_sub(charge)?;
        let next = bucket
            .into_iter()
            .flatten()
            .cloned()
            .chain([value.0.clone()])
            .collect();
        self.values.insert(key, next);
        *available = remaining;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_tir::{Constant, ValueKind, ValueNode};
    fn value(byte: u8) -> SharedMaterialized {
        let mut bytes = Vec::with_capacity(4096);
        bytes.push(byte);
        Materialized {
            root: ValueNode {
                ty: hirsel_types::TypeId::DANGLING,
                kind: ValueKind::Constant(Constant::Bytes(bytes)),
            },
        }
        .into()
    }
    #[test]
    fn equal_materialized_payloads_share_one_actual_debit_and_collisions_stay_distinct() {
        let mut pool = MaterializedPool::default();
        let mut available = 1 << 20;
        let initial = available;
        let key = ContentHash::of_bytes(b"same hash, including a deliberate collision");
        let mut first = value(1);
        pool.intern(key, &mut first, &mut available).unwrap();
        assert!(initial - available >= 4096 + crate::memo::retained_arc_bytes(&*first));
        let after_first = available;
        let weak = Arc::downgrade(&first.0);
        let mut second = value(1);
        pool.intern(key, &mut second, &mut available).unwrap();
        assert_eq!(available, after_first);
        assert!(Arc::ptr_eq(&first.0, &second.0));
        let mut collision = value(2);
        pool.intern(key, &mut collision, &mut available).unwrap();
        assert!(!Arc::ptr_eq(&first.0, &collision.0));
        assert!(after_first - available >= 4096);
        assert_eq!(pool.values[&key].len(), 2);
        let owned = first.owned();
        assert_eq!(owned.root, first.root);
        drop((first, second, collision, pool));
        assert!(weak.upgrade().is_none());
        assert!(matches!(owned.root.kind, ValueKind::Constant(Constant::Bytes(v)) if v == [1]));
    }
    #[test]
    fn materialized_pool_refuses_insufficient_capacity_without_retaining_or_spending() {
        let mut pool = MaterializedPool::default();
        let mut value = value(1);
        let key = ContentHash::default();
        let mut available = 4095;
        let weak = Arc::downgrade(&value.0);
        assert!(pool.intern(key, &mut value, &mut available).is_none());
        assert_eq!(available, 4095);
        assert!(pool.values.is_empty());
        drop(value);
        assert!(weak.upgrade().is_none());
        let types = hirsel_types::TypeInterner::new(None);
        let ty = types.intern(hirsel_types::Type::unit());
        let original = Materialized {
            root: ValueNode {
                ty,
                kind: ValueKind::Constant(Constant::Bytes(vec![1, 2, 3])),
            },
        };
        let shared = SharedMaterialized::from(original.clone());
        let owned_bytes =
            hirsel_product_codec::canonical_bytes(types.clone(), &original, |_| None).unwrap();
        let shared_bytes = hirsel_product_codec::canonical_bytes(types, &shared, |_| None).unwrap();
        assert_eq!(
            owned_bytes, shared_bytes,
            "private sharing preserves canonical payload bytes"
        );
    }
}
