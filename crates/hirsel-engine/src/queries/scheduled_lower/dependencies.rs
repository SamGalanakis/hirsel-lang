//! Lossless dictionary storage for an ordered signature dependency sequence.
//! BTreeMap assigns first-occurrence slots; serde's SerializeSeq emits the
//! original Vec wire format. Indices are private retained metadata only.
use crate::SignatureDependency;
use serde::{Serialize, Serializer, ser::SerializeSeq};
use std::{collections::BTreeMap, mem::size_of_val};

#[derive(Clone)]
pub(super) enum Dependencies {
    Flat(Box<[SignatureDependency]>),
    Indexed {
        values: Box<[SignatureDependency]>,
        order: Box<[u32]>,
    },
}
impl From<Vec<SignatureDependency>> for Dependencies {
    fn from(dependencies: Vec<SignatureDependency>) -> Self {
        let mut slots = BTreeMap::new();
        for dependency in &dependencies {
            let Ok(next) = u32::try_from(slots.len()) else {
                return Self::Flat(dependencies.into_boxed_slice());
            };
            slots
                .entry((dependency.definition, dependency.fingerprint))
                .or_insert(next);
        }
        let indexed_bytes = slots
            .len()
            .checked_mul(size_of::<SignatureDependency>())
            .and_then(|bytes| bytes.checked_add(dependencies.len().checked_mul(size_of::<u32>())?));
        if !indexed_bytes.is_some_and(|bytes| bytes < size_of_val(dependencies.as_slice())) {
            return Self::Flat(dependencies.into_boxed_slice());
        }
        let mut values = Vec::with_capacity(slots.len());
        let mut order = Vec::with_capacity(dependencies.len());
        for dependency in dependencies {
            let index = slots[&(dependency.definition, dependency.fingerprint)];
            // Slots follow first occurrence, so new values arrive in order.
            if index as usize == values.len() {
                values.push(dependency);
            }
            order.push(index);
        }
        Self::Indexed {
            values: values.into_boxed_slice(),
            order: order.into_boxed_slice(),
        }
    }
}
impl Dependencies {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Flat(values) => values.len(),
            Self::Indexed { order, .. } => order.len(),
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        match self {
            Self::Flat(values) => size_of_val(&**values),
            Self::Indexed { values, order } => size_of_val(&**values) + size_of_val(&**order),
        }
    }
    fn get(&self, index: usize) -> &SignatureDependency {
        match self {
            Self::Flat(values) => &values[index],
            Self::Indexed { values, order } => &values[order[index] as usize],
        }
    }
    pub(super) fn iter(&self) -> impl ExactSizeIterator<Item = &SignatureDependency> {
        (0..self.len()).map(|index| self.get(index))
    }
    pub(super) fn to_vec(&self) -> Vec<SignatureDependency> {
        self.iter().cloned().collect()
    }
}
impl Serialize for Dependencies {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.len()))?;
        for dependency in self.iter() {
            sequence.serialize_element(dependency)?;
        }
        sequence.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::{ContentHash, DefId, DefPath};

    fn dependency(name: &str, fingerprint: &[u8]) -> SignatureDependency {
        SignatureDependency {
            definition: DefId::intern(&DefPath::core(name)),
            fingerprint: ContentHash::of_bytes(fingerprint),
        }
    }
    #[test]
    fn indexed_dependencies_preserve_repeated_order_versions_and_canonical_bytes() {
        let a = dependency("dependency", b"first");
        let changed = dependency("dependency", b"changed");
        let b = dependency("another_dependency_with_a_long_owned_name", b"other");
        let original = vec![
            a.clone(),
            b.clone(),
            a.clone(),
            changed.clone(),
            a.clone(),
            b.clone(),
            changed,
        ];
        let sequence = Dependencies::from(original.clone());
        assert!(matches!(sequence, Dependencies::Indexed { .. }));
        assert_eq!(sequence.to_vec(), original);
        assert!(sequence.retained_bytes() < size_of_val(original.as_slice()));
        let expected = bincode::serialize(&original).unwrap();
        let encoded = bincode::serialize(&sequence).unwrap();
        assert_eq!(encoded, expected);
        assert_eq!(
            bincode::deserialize::<Vec<SignatureDependency>>(&encoded).unwrap(),
            original
        );
    }
    #[test]
    fn sparse_and_empty_dependency_sequences_keep_flat_bounded_storage() {
        for len in [0, 1, 2, 31, 843] {
            for distinct in [1, 2, 192, 843] {
                let original: Vec<_> = (0..len)
                    .map(|index| {
                        dependency(&format!("dependency_{}", index % distinct), b"version")
                    })
                    .collect();
                let sequence = Dependencies::from(original.clone());
                assert_eq!(sequence.to_vec(), original);
                assert!(sequence.retained_bytes() <= size_of_val(original.as_slice()));
                assert_eq!(
                    bincode::serialize(&sequence).unwrap(),
                    bincode::serialize(&original).unwrap()
                );
                if len <= distinct {
                    assert!(matches!(sequence, Dependencies::Flat(_)));
                }
            }
        }
    }
}
