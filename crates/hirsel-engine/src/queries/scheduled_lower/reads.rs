//! Large settled read sets reuse their existing immutable Lower descriptor.
use super::*;

pub(super) type Descriptor = (
    DefId,
    Reference,
    Vec<Diagnostic>,
    Vec<SignatureDependency>,
    Reference,
    Reads,
    DepthEvidence<Span>,
    bool,
);
#[derive(Clone)]
pub(super) struct Archive {
    pub reference: Reference,
    pub decode_bytes: u64,
    pub len: usize,
    pub dependencies: usize,
}
impl Archive {
    pub(super) fn load(
        &self,
        records: &crate::NativeRecords,
        anchors: &crate::NativeRecordContext,
        store: &dyn ResultStore,
        allowance: u64,
    ) -> Option<Reads> {
        if self.reference.domain == CONSTANT_DESCRIPTOR {
            if self.reference.len > allowance || self.decode_bytes > allowance {
                return None;
            }
            let bytes = store.get_content(self.reference)?;
            let (_, _, reads, _, _): (
                Materialized,
                Vec<(hirsel_syntax::ExprId, Materialized)>,
                Reads,
                DepthEvidence<Span>,
                bool,
            ) = records.decode(anchors, &bytes)?;
            return (reads.len() == self.len && self.dependencies == 0).then_some(reads);
        }
        Some(self.descriptor(records, anchors, store, allowance)?.5)
    }
    pub(super) fn dependencies(
        &self,
        records: &crate::NativeRecords,
        anchors: &crate::NativeRecordContext,
        store: &dyn ResultStore,
        allowance: u64,
    ) -> Option<Vec<SignatureDependency>> {
        Some(self.descriptor(records, anchors, store, allowance)?.3)
    }
    fn descriptor(
        &self,
        records: &crate::NativeRecords,
        anchors: &crate::NativeRecordContext,
        store: &dyn ResultStore,
        allowance: u64,
    ) -> Option<Descriptor> {
        if self.reference.domain != LOWER_DESCRIPTOR
            || self.reference.len > allowance
            || self.decode_bytes > allowance
        {
            return None;
        }
        let bytes = store.get_content(self.reference)?;
        let descriptor: Descriptor = records.decode(anchors, &bytes)?;
        (descriptor.5.len() == self.len && descriptor.3.len() == self.dependencies)
            .then_some(descriptor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    struct Counting {
        inner: crate::store::MemStore,
        gets: AtomicUsize,
    }
    impl ResultStore for Counting {
        fn name(&self) -> &'static str {
            "counting-read-test"
        }
        fn get(&self, key: &EntryKey) -> Option<crate::store::Bytes> {
            self.gets.fetch_add(1, Ordering::Relaxed);
            self.inner.get(key)
        }
        fn put(&self, key: &EntryKey, bytes: &[u8]) {
            self.inner.put(key, bytes);
        }
    }
    #[test]
    fn archived_lower_reads_preserve_exact_rows_and_refuse_before_provider_access() {
        let engine = crate::QueryEngine::new(crate::default_environment(), Default::default());
        let anchors = crate::NativeRecordContext::capture(&engine);
        let store = Arc::new(Counting::default());
        let records = crate::NativeRecords::new(engine.types().clone(), store.clone());
        let definition = DefId::intern(&DefPath::core("archive_read_fixture"));
        let expected = Reads::from_vec(
            (0u32..128)
                .map(|index| {
                    (
                        ReadKey::Signature(DefId::intern(&DefPath::core(&format!("read_{index}")))),
                        ContentHash::of_bytes(&index.to_le_bytes()),
                    )
                })
                .collect(),
        )
        .unwrap();
        let body = store
            .put_content(crate::NATIVE_ORIGINAL_BODY, b"body reference only")
            .unwrap();
        let materialized = store
            .put_content(LOWER_MATERIALIZED, b"materialized reference only")
            .unwrap();
        let depth = DepthEvidence::<Span>::leaf(32);
        let dependencies = vec![
            SignatureDependency {
                definition,
                fingerprint: ContentHash::default()
            };
            100
        ];
        let descriptor: Descriptor = (
            definition,
            body,
            Vec::new(),
            dependencies,
            materialized,
            expected.clone(),
            depth,
            false,
        );
        let (bytes, working) = records
            .encode_with_decode_bytes(&anchors, &descriptor)
            .unwrap();
        let bound = working
            + 2 * (size_of::<Descriptor>()
                + size_of_val(descriptor.3.as_slice())
                + size_of_val(&*descriptor.6.first)
                + expected
                    .allocations()
                    .map(|(_, bytes)| bytes)
                    .sum::<usize>()) as u64;
        let reference = store.put_content(LOWER_DESCRIPTOR, &bytes).unwrap();
        let archive = Archive {
            reference,
            decode_bytes: bound,
            len: expected.len(),
            dependencies: descriptor.3.len(),
        };
        store.gets.store(0, Ordering::Relaxed);
        assert!(
            archive
                .load(&records, &anchors, &*store, bound - 1)
                .is_none()
        );
        assert_eq!(store.gets.load(Ordering::Relaxed), 0);
        let actual = archive.load(&records, &anchors, &*store, bound).unwrap();
        assert!(store.gets.load(Ordering::Relaxed) > 0);
        assert_eq!(actual, expected);
        let actual_dependencies = archive
            .dependencies(&records, &anchors, &*store, bound)
            .unwrap();
        assert_eq!(actual_dependencies, descriptor.3);
        let wrong_dependencies = Archive {
            dependencies: archive.dependencies - 1,
            ..archive.clone()
        };
        assert!(
            wrong_dependencies
                .dependencies(&records, &anchors, &*store, bound)
                .is_none()
        );
        assert_eq!(
            records.encode(&anchors, &actual).unwrap(),
            records.encode(&anchors, &expected).unwrap()
        );
        let mut outcome = Outcome {
            result: Ok(Value::Lower(Box::new(LowerProduct {
                definition,
                body,
                diagnostics: Vec::new(),
                dependencies: descriptor.3.clone().into(),
                archived_metadata: None,
                verified: None,
                acceptance: None,
                materialized,
                definition_decode_layout: DefinitionDecodeLayout {
                    retained: 0,
                    transient: 0,
                },
                constant_selection_bytes: 0,
            }))),
            reads: expected.clone(),
            reads_decode_bytes: bound,
            depth: descriptor.6.clone().into(),
            executed: false,
            execution: None,
            execution_subject: ContentHash::default(),
        };
        let before = outcome.retained_bytes().unwrap();
        outcome.archive_reads(reference);
        assert_eq!(outcome.read_len(), expected.len());
        assert!(outcome.archive().is_some());
        let Ok(Value::Lower(lower)) = &outcome.result else {
            unreachable!()
        };
        assert!(lower.archived_metadata.is_some());
        assert_eq!(lower.dependencies.len(), 0);
        assert_eq!(
            lower
                .archived_metadata
                .as_ref()
                .unwrap()
                .dependencies(&records, &anchors, &*store, bound)
                .unwrap(),
            descriptor.3,
        );
        assert!(outcome.retained_bytes().unwrap() < before);
        let wrong_count = Archive {
            len: expected.len() - 1,
            ..archive.clone()
        };
        assert!(
            wrong_count
                .load(&records, &anchors, &*store, bound)
                .is_none()
        );
        let mut wrong_domain = archive.clone();
        wrong_domain.reference.domain = LOWER_MATERIALIZED;
        store.gets.store(0, Ordering::Relaxed);
        assert!(
            wrong_domain
                .load(&records, &anchors, &*store, bound)
                .is_none()
        );
        assert_eq!(store.gets.load(Ordering::Relaxed), 0);
        assert!(
            archive
                .load(
                    &records,
                    &anchors,
                    &crate::store::MemStore::default(),
                    bound
                )
                .is_none()
        );
    }
    #[test]
    fn archived_constant_reads_keep_complete_dependencies_and_bound_decode_before_io() {
        let engine = crate::QueryEngine::new(crate::default_environment(), Default::default());
        let anchors = crate::NativeRecordContext::capture(&engine);
        let store = Arc::new(Counting::default());
        let records = crate::NativeRecords::new(engine.types().clone(), store.clone());
        let reads = Reads::from_vec(
            (0u32..128)
                .map(|index| {
                    (
                        ReadKey::Signature(DefId::intern(&DefPath::core(&format!(
                            "constant_read_{index}"
                        )))),
                        ContentHash::of_bytes(&index.to_le_bytes()),
                    )
                })
                .collect(),
        )
        .unwrap();
        let ty = engine.types().intern(hirsel_types::Type::unit());
        let mut constants = Constants::new(
            Materialized {
                root: hirsel_tir::ValueNode {
                    ty,
                    kind: hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit),
                },
            },
            Vec::new(),
        );
        let depth = DepthEvidence::leaf(32);
        let bytes = constants
            .encode(&records, &anchors, &reads, &depth, false)
            .unwrap();
        let reference = store.put_content(CONSTANT_DESCRIPTOR, &bytes).unwrap();
        let bound = constants.working_bytes();
        let mut outcome = Outcome {
            result: Ok(Value::Constant(Box::new(constants))),
            reads: reads.clone(),
            reads_decode_bytes: 0,
            depth: depth.into(),
            executed: false,
            execution: None,
            execution_subject: ContentHash::default(),
        };
        let before = outcome.retained_bytes().unwrap();
        outcome.archive_reads(reference);
        assert!(outcome.reads.is_empty());
        assert_eq!(outcome.read_len(), reads.len());
        assert!(outcome.retained_bytes().unwrap() < before);
        let archive = outcome.archive().unwrap();
        store.gets.store(0, Ordering::Relaxed);
        assert!(
            archive
                .load(&records, &anchors, &*store, bound - 1)
                .is_none()
        );
        assert_eq!(store.gets.load(Ordering::Relaxed), 0);
        assert_eq!(
            archive.load(&records, &anchors, &*store, bound).unwrap(),
            reads
        );
        let wrong = Archive {
            len: reads.len() - 1,
            ..archive.clone()
        };
        assert!(wrong.load(&records, &anchors, &*store, bound).is_none());
        let wrong = Archive {
            reference: Reference {
                domain: LOWER_MATERIALIZED,
                ..reference
            },
            ..archive.clone()
        };
        store.gets.store(0, Ordering::Relaxed);
        assert!(wrong.load(&records, &anchors, &*store, bound).is_none());
        assert_eq!(store.gets.load(Ordering::Relaxed), 0);
        assert!(
            archive
                .load(
                    &records,
                    &anchors,
                    &crate::store::MemStore::default(),
                    bound
                )
                .is_none()
        );
    }
}
