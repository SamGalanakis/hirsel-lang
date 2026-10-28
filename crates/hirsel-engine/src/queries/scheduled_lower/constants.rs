//! Retained constants use the ordinary type/anchor codec.
//! Small values keep their decoded form; large values retain their published record and
//! reconstruct only when a consuming task or admitted inspector needs them.
use super::*;

#[derive(Clone)]
pub(super) struct Constants {
    data: Data,
    pub(super) archived_reads: Option<Box<reads::Archive>>,
    root_bytes: u64,
    data_bytes: u64,
    decode_bytes: u64,
}
#[derive(Clone)]
enum Data {
    Decoded(Materialized, Vec<(hirsel_syntax::ExprId, Materialized)>),
    Packed(Reference),
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
            archived_reads: None,
            root_bytes: root.bytes(),
            data_bytes,
            decode_bytes: 0,
        }
    }
    pub(super) fn decoded(
        &self,
        records: &crate::NativeRecords,
        anchors: &crate::NativeRecordContext,
        store: &dyn ResultStore,
        allowance: u64,
    ) -> Option<(Materialized, Vec<(hirsel_syntax::ExprId, Materialized)>)> {
        match &self.data {
            Data::Decoded(value, substitutions) => Some((value.clone(), substitutions.clone())),
            Data::Packed(record) => {
                if self.decode_bytes > allowance {
                    return None;
                }
                let bytes = store.get_content(*record)?;
                let (value, substitutions, _, _, _, _): (
                    Materialized,
                    Vec<(hirsel_syntax::ExprId, Materialized)>,
                    Reads,
                    DepthEvidence<Span>,
                    bool,
                    staged::Selections,
                ) = records.decode(anchors, &bytes)?;
                Some((value, substitutions))
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(
        &mut self,
        records: &crate::NativeRecords,
        anchors: &crate::NativeRecordContext,
        reads: &Reads,
        depth: &DepthEvidence<Span>,
        executed: bool,
        selections: &staged::Selections,
    ) -> Option<Vec<u8>> {
        let Data::Decoded(value, substitutions) = &self.data else {
            return None;
        };
        let (bytes, record_working) = records.encode_with_decode_bytes(
            anchors,
            &(value, substitutions, reads, depth, executed, selections),
        )?;
        let metadata = reads
            .allocations()
            .map(|(_, bytes)| bytes as u64)
            .try_fold(size_of_val(&*depth.first) as u64, u64::checked_add)?;
        self.decode_bytes = self
            .data_bytes
            .checked_add(metadata)?
            .checked_add(staged::selection_bytes(selections))?
            .checked_mul(2)?
            .checked_add(record_working)?;
        Some(bytes)
    }
    pub(super) fn pack(&mut self, record: Reference) {
        if self.data_bytes >= 4096 {
            // The store already owns this immutable descriptor, as it owns
            // Lower bodies. A settled value needs only its typed reference.
            self.data = Data::Packed(record);
            self.data_bytes = 0;
        }
    }
    pub(super) fn working_bytes(&self) -> u64 {
        self.decode_bytes
    }
    pub(super) fn set_working_bytes(&mut self, bytes: u64) {
        self.decode_bytes = bytes;
    }
    pub(super) fn retained_bytes(&self) -> u64 {
        self.data_bytes
    }
    pub(super) fn root_clone_bytes(&self) -> u64 {
        self.root_bytes
    }
    pub(super) fn decode_bytes(&self) -> u64 {
        if matches!(self.data, Data::Packed(_)) {
            self.decode_bytes
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_tree_preserves_equal_leaves_and_substitutions_with_bounded_decode() {
        let engine = crate::QueryEngine::new(crate::default_environment(), Default::default());
        let anchors = crate::NativeRecordContext::capture(&engine);
        let store = Arc::new(crate::store::MemStore::default());
        let records = crate::NativeRecords::new(engine.types().clone(), store.clone());
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
        let bytes = constants
            .encode(
                &records,
                &anchors,
                &Reads::default(),
                &DepthEvidence::leaf(32),
                false,
                &Vec::new(),
            )
            .unwrap();
        let record = store.put_content(CONSTANT_DESCRIPTOR, &bytes).unwrap();
        constants.pack(record);
        assert!(matches!(constants.data, Data::Packed(_)));
        assert!(constants.retained_bytes() < decoded_bytes / 2);
        assert!(constants.decode_bytes() >= 2 * decoded_bytes);
        assert!(
            constants
                .decoded(&records, &anchors, &*store, constants.decode_bytes() - 1)
                .is_none()
        );
        let (root, restored) = constants
            .decoded(&records, &anchors, &*store, constants.decode_bytes())
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
        assert_eq!(original, copy);
    }
}
