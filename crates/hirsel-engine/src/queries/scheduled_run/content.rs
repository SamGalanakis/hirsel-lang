//! Run stores immutable content references and retains only source-owned handles.
use super::*;

#[derive(Clone)]
struct Bytes {
    reference: Reference,
    decode_bytes: u64,
}
impl Bytes {
    fn new(
        store: &dyn ResultStore,
        allowance: u64,
        value: &Vec<Vec<u8>>,
    ) -> Result<Self, QueryError> {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(std::mem::size_of::<Vec<Vec<u8>>>());
        weight.vec(value, |weight, field| weight.flat_vec(field));
        let bytes = bincode::serialize(value).map_err(|_| QueryError::NotImplemented)?;
        let decode_bytes = weight
            .bytes()
            .checked_mul(2)
            .and_then(|size| size.checked_add(bytes.len() as u64))
            .ok_or(QueryError::NotImplemented)?;
        if decode_bytes > allowance {
            return Err(operational("Run content exceeds scratch admission"));
        }
        let reference = store
            .put_content(INPUT_CONTENT, &bytes)
            .ok_or(QueryError::NotImplemented)?;
        Ok(Self {
            reference,
            decode_bytes,
        })
    }
    fn load(&self, store: &dyn ResultStore, allowance: u64) -> Result<Vec<Vec<u8>>, QueryError> {
        use bincode::Options;
        if self.reference.domain != INPUT_CONTENT
            || self.reference.len > allowance
            || self.decode_bytes > allowance
        {
            return Err(operational("Run content exceeds decode admission"));
        }
        let bytes = store
            .get_content(self.reference)
            .ok_or(QueryError::NotImplemented)?;
        if INPUT_CONTENT.reference(&bytes) != Some(self.reference) {
            return Err(operational("Run content descriptor changed"));
        }
        bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(self.reference.len)
            .reject_trailing_bytes()
            .deserialize(&bytes)
            .map_err(|_| QueryError::NotImplemented)
    }
}
enum Header {
    Body(hirsel_types::SignatureRef),
    Named {
        signature: hirsel_types::SignatureRef,
        source: Bytes,
        reads: Box<[DefId]>,
    },
}
pub(super) struct Content {
    header: Header,
    body: bool,
    references: Box<[DefId]>,
}
impl Content {
    pub(super) fn new(
        queries: &FrontRunQueries,
        content: DefinitionContent,
    ) -> Result<Self, QueryError> {
        let snapshot = queries.lower.signatures();
        let references = hirsel_proof_export::references(snapshot.types(), &content)
            .iter()
            .map(DefId::intern)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let (header, body) = match content {
            DefinitionContent::Body { signature, tir } => (Header::Body(signature), tir.is_some()),
            DefinitionContent::Named {
                signature,
                source,
                reads,
            } => (
                Header::Named {
                    signature,
                    source: Bytes::new(&*queries.store, queries.weight.scratch, &source)?,
                    reads: reads
                        .iter()
                        .map(DefId::intern)
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                },
                false,
            ),
        };
        Ok(Self {
            header,
            body,
            references,
        })
    }
    pub(super) fn references(&self) -> Vec<DefPath> {
        self.references.iter().map(|id| id.path().clone()).collect()
    }
    pub(super) fn has_body(&self) -> bool {
        self.body
    }
    pub(super) fn load(
        &self,
        queries: &FrontRunQueries,
        path: &DefPath,
        allowance: u64,
    ) -> Result<DefinitionContent, QueryError> {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(std::mem::size_of::<DefinitionContent>());
        let working = match &self.header {
            Header::Body(_) if self.body => queries.lower.settled_definition_decode_bytes(path)?,
            Header::Body(_) => 0,
            Header::Named { source, reads, .. } => {
                weight.add(
                    reads
                        .len()
                        .checked_mul(std::mem::size_of::<DefPath>())
                        .ok_or(QueryError::NotImplemented)?,
                );
                for read in reads {
                    weight.path(read.path());
                }
                source.decode_bytes
            }
        };
        if weight
            .bytes()
            .checked_mul(2)
            .and_then(|size| size.checked_add(working))
            .is_none_or(|size| size > allowance)
        {
            return Err(operational("Run content exceeds scratch admission"));
        }
        Ok(match &self.header {
            Header::Body(signature) => DefinitionContent::Body {
                signature: signature.clone(),
                tir: if self.body {
                    Some(queries.lower.settled_definition(path)?)
                } else {
                    None
                },
            },
            Header::Named {
                signature,
                source,
                reads,
            } => DefinitionContent::Named {
                signature: signature.clone(),
                source: source.load(&*queries.store, allowance)?,
                reads: reads.iter().map(|id| id.path().clone()).collect(),
            },
        })
    }
    pub(super) fn weight(&self, weight: &mut crate::NativeMetadataWeight) {
        weight.add(std::mem::size_of_val(&*self.references));
        if let Header::Named { reads, .. } = &self.header {
            weight.add(std::mem::size_of_val(&**reads));
        }
    }
}
pub(super) struct Node {
    content: Bytes,
    pub dependencies: Box<[DefId]>,
}
impl Node {
    pub(super) fn new(
        queries: &FrontRunQueries,
        content: Vec<Vec<u8>>,
        dependencies: &BTreeSet<DefPath>,
    ) -> Result<Self, QueryError> {
        Ok(Self {
            content: Bytes::new(&*queries.store, queries.weight.scratch, &content)?,
            dependencies: dependencies
                .iter()
                .map(DefId::intern)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        })
    }
    pub(super) fn program(
        &self,
        queries: &FrontRunQueries,
        path: &DefPath,
    ) -> Result<hirsel_run_schema::ProgramNode, QueryError> {
        let mut dependencies: Vec<_> = self
            .dependencies
            .iter()
            .map(|id| id.path().to_string())
            .collect();
        dependencies.push("$hirsel/runtime@1".into());
        Ok(hirsel_run_schema::ProgramNode {
            identity: path.to_string(),
            content: self.content.load(&*queries.store, queries.weight.scratch)?,
            dependencies,
        })
    }
    pub(super) fn weight(&self, weight: &mut crate::NativeMetadataWeight) {
        weight.add(std::mem::size_of_val(&*self.dependencies));
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
            "run-content-bound-test"
        }
        fn get(&self, key: &crate::store::EntryKey) -> Option<crate::store::Bytes> {
            self.gets.fetch_add(1, Ordering::Relaxed);
            self.inner.get(key)
        }
        fn put(&self, key: &crate::store::EntryKey, bytes: &[u8]) {
            self.inner.put(key, bytes)
        }
    }
    #[test]
    fn archived_node_content_keeps_order_and_refuses_before_provider_access() {
        let store = Counting::default();
        let source = vec![vec![7; 8192], vec![], vec![7; 8192], vec![9; 3]];
        let saved = Bytes::new(&store, 1 << 20, &source).unwrap();
        assert!(std::mem::size_of_val(&saved) < source.iter().map(Vec::len).sum::<usize>());
        store.gets.store(0, Ordering::Relaxed);
        assert!(saved.load(&store, saved.decode_bytes - 1).is_err());
        assert_eq!(store.gets.load(Ordering::Relaxed), 0);
        assert_eq!(saved.load(&store, saved.decode_bytes).unwrap(), source);
        assert!(store.gets.load(Ordering::Relaxed) > 0);
        let mut wrong = saved.clone();
        wrong.reference.domain = OUTPUT;
        store.gets.store(0, Ordering::Relaxed);
        assert!(wrong.load(&store, saved.decode_bytes).is_err());
        assert_eq!(store.gets.load(Ordering::Relaxed), 0);
        let mut missing = saved.clone();
        missing.reference.output.0 = ContentHash([0x91; 32]);
        assert!(missing.load(&store, saved.decode_bytes).is_err());
    }
}
