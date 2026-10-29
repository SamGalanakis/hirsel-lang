//! Exact signature request order with repeated rows stored once per sequence.
use super::*;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct OrderedSignatureReads {
    rows: Box<[SignatureDependency]>,
    order: Box<[u32]>,
}
impl OrderedSignatureReads {
    pub(crate) fn new(input: &[SignatureDependency]) -> Self {
        let mut identities = BTreeMap::new();
        let mut rows = Vec::new();
        let mut order = Vec::with_capacity(input.len());
        for row in input {
            let next = rows.len();
            let Ok(next) = u32::try_from(next) else {
                return Self { rows: input.into(), order: Box::new([]) };
            };
            let index = *identities.entry((row.definition, row.fingerprint)).or_insert_with(|| {
                rows.push(row.clone());
                next
            });
            order.push(index);
        }
        if rows.len() * size_of::<SignatureDependency>() + order.len() * size_of::<u32>()
            >= size_of_val(input) {
            Self { rows: input.into(), order: Box::new([]) }
        } else {
            Self { rows: rows.into_boxed_slice(), order: order.into_boxed_slice() }
        }
    }
    pub(crate) fn len(&self) -> usize {
        if self.order.is_empty() { self.rows.len() } else { self.order.len() }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        size_of_val(&*self.rows) + size_of_val(&*self.order)
    }
    pub(crate) fn values(&self) -> Option<Vec<SignatureDependency>> {
        if self.order.is_empty() {
            return Some(self.rows.to_vec());
        }
        self.order.iter().map(|index| self.rows.get(*index as usize).cloned()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordered_signatures_preserve_repetitions_and_distinct_versions_through_the_codec() {
        let dependency = |name, version| SignatureDependency {
            definition: DefId::intern(&DefPath::core(name)),
            fingerprint: ContentHash::of_bytes(version),
        };
        let a = dependency("a", b"first");
        let b = dependency("b", b"second");
        let changed = dependency("a", b"changed");
        let input = [a.clone(), b, a.clone(), changed, a].repeat(16);
        let sequence = OrderedSignatureReads::new(&input);
        assert!(sequence.retained_bytes() < size_of_val(input.as_slice()));
        let encoded = bincode::serialize(&sequence).unwrap();
        let restored: OrderedSignatureReads = bincode::deserialize(&encoded).unwrap();
        assert_eq!(restored.len(), input.len());
        assert_eq!(restored.values().unwrap(), input);
        let malformed = OrderedSignatureReads { rows: Box::new([]), order: Box::new([0]) };
        assert!(malformed.values().is_none());
    }
}
