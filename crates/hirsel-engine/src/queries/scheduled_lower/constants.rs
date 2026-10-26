//! Retained constants use the ordinary type/anchor codec.
//! Small values keep their decoded form; large values retain compact bytes and
//! reconstruct only when a consuming task or admitted inspector needs them.
use super::*;

#[derive(Clone)]
pub(super) struct Constants {
    data: Data,
    root_bytes: u64,
    data_bytes: u64,
    decode_bytes: u64,
}
#[derive(Clone)]
enum Data {
    Decoded(Materialized, Vec<(hirsel_syntax::ExprId, Materialized)>),
    Packed(Arc<[u8]>),
}
impl Constants {
    pub(super) fn new(
        value: Materialized,
        substitutions: Vec<(hirsel_syntax::ExprId, Materialized)>,
    ) -> Self {
        let mut root = crate::NativeMetadataWeight::default();
        root.materialized(&value);
        let mut rest = crate::NativeMetadataWeight::default();
        rest.vec(&substitutions, |weight, (_, value)| {
            weight.materialized(value)
        });
        let data_bytes = root.bytes().saturating_add(rest.bytes());
        Self {
            data: Data::Decoded(value, substitutions),
            root_bytes: root.bytes(),
            data_bytes,
            decode_bytes: 0,
        }
    }
    pub(super) fn decoded(
        &self,
        records: &crate::NativeRecords,
        anchors: &crate::NativeRecordContext,
        allowance: u64,
    ) -> Option<(Materialized, Vec<(hirsel_syntax::ExprId, Materialized)>)> {
        match &self.data {
            Data::Decoded(value, substitutions) => Some((value.clone(), substitutions.clone())),
            Data::Packed(bytes) => {
                if self.decode_bytes > allowance {
                    return None;
                }
                let (value, substitutions) = records.decode(anchors, bytes)?;
                Some((value, substitutions))
            }
        }
    }
    pub(super) fn encode(
        &self,
        owner: &FrontLowerQueries,
        reads: &Reads,
        depth: &DepthEvidence<Span>,
        executed: bool,
    ) -> Option<Vec<u8>> {
        match &self.data {
            Data::Decoded(value, substitutions) => owner.records.encode(
                &owner.anchors,
                &(value, substitutions, reads, depth, executed),
            ),
            Data::Packed(_) => None,
        }
    }
    pub(super) fn pack(
        &mut self,
        records: &crate::NativeRecords,
        anchors: &crate::NativeRecordContext,
    ) -> Option<()> {
        let Data::Decoded(value, substitutions) = &self.data else {
            return Some(());
        };
        // Small inline/scalar outcomes cannot repay a separate packed record.
        if self.data_bytes < 4096 {
            return Some(());
        }
        let (bytes, record_working) =
            records.encode_with_decode_bytes(anchors, &(value, substitutions))?;
        let retained = (bytes.len() as u64).checked_add(2 * size_of::<usize>() as u64)?;
        if retained < self.data_bytes {
            // A decoded Vec can overlap its construction buffer. The ordinary
            // codec's record/type/anchor workspace is independent of the tree.
            self.decode_bytes = self
                .data_bytes
                .checked_mul(2)?
                .checked_add(record_working)?;
            self.data = Data::Packed(bytes.into());
            self.data_bytes = retained;
        }
        Some(())
    }
    pub(super) fn retained_bytes(&self) -> u64 {
        self.data_bytes
    }
    pub(super) fn root_clone_bytes(&self) -> u64 {
        self.root_bytes
    }
    pub(super) fn decode_bytes(&self) -> u64 {
        self.decode_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_tree_preserves_equal_leaves_and_substitutions_with_bounded_decode() {
        let engine = crate::QueryEngine::new(crate::default_environment(), Default::default());
        let anchors = crate::NativeRecordContext::capture(&engine);
        let records = crate::NativeRecords::new(
            engine.types().clone(),
            Arc::new(crate::store::MemStore::default()),
        );
        let ty = engine.types().intern(hirsel_types::Type::unit());
        let leaf = hirsel_tir::ValueNode {
            ty,
            kind: hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit),
        };
        let value = Materialized {
            root: hirsel_tir::ValueNode {
                ty,
                kind: hirsel_tir::ValueKind::Construct {
                    constructor: hirsel_tir::Constructor::Tuple,
                    fields: vec![leaf; 2048],
                },
            },
        };
        let substitutions = vec![(hirsel_syntax::ExprId(0), value.clone())];
        let mut constants = Constants::new(value.clone(), substitutions.clone());
        let decoded_bytes = constants.retained_bytes();
        constants.pack(&records, &anchors).unwrap();
        assert!(matches!(constants.data, Data::Packed(_)));
        assert!(constants.retained_bytes() < decoded_bytes / 2);
        assert!(constants.decode_bytes() >= 2 * decoded_bytes);
        assert!(
            constants
                .decoded(&records, &anchors, constants.decode_bytes() - 1)
                .is_none()
        );
        let (root, restored) = constants
            .decoded(&records, &anchors, constants.decode_bytes())
            .unwrap();
        assert_eq!(root.root, value.root);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].0, substitutions[0].0);
        assert_eq!(restored[0].1.root, substitutions[0].1.root);
        let bytes = records.encode(&anchors, &(value, substitutions)).unwrap();
        assert_eq!(records.encode(&anchors, &(root, restored)).unwrap(), bytes);
        let clone = constants.clone();
        let (Data::Packed(original), Data::Packed(copy)) = (&constants.data, &clone.data) else {
            unreachable!()
        };
        assert!(Arc::ptr_eq(original, copy));
    }
}
