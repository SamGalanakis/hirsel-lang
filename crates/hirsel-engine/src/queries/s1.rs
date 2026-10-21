//! Task-local dependency/read bridge for S1 evaluation. Shared sources expose
//! immutable inputs; the requestor owns its coordinator stack and read frames.
//! Like rustc's body tasks (rustc_hir_analysis/src/lib.rs:166-220), a dependency
//! returns products for its caller to merge, including when evaluation fails.
use super::*;
use hirsel_tir::{DefinitionRef, MaterializedRef, TirProvider};
use hirsel_types::GenericArgument;

/// A dependency's complete read record survives failure and cache hits alike.
/// The source owns depth admission and registration requests; this bridge only
/// transports its result and evidence to the enclosing evaluation.
pub(crate) struct DependencyProduct<T> {
    pub(crate) result: Result<T, QueryError>,
    pub(crate) reads: Reads,
}

/// A worker's request interface. Implementations keep mutable request state
/// local, over immutable shared phase/snapshot handles. No engine is required
/// by the evaluation provider, and no lock spans interpreter execution.
pub(crate) trait DependencySource {
    fn definition(&mut self, path: &DefPath) -> DependencyProduct<DefinitionRef>;
    fn constant(&mut self, path: &DefPath) -> DependencyProduct<MaterializedRef>;
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> DependencyProduct<MaterializedRef>;
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> DependencyProduct<Option<DefPath>>;
}

/// A private evaluation root plus its task-local dependency requestor. Every
/// request merges its complete reads before returning its success or failure.
/// Constants remain lazy: interpreting a root never enumerates other constants.
pub(crate) struct EvaluationProvider<'a, S: DependencySource> {
    root: DefinitionRef,
    source: &'a mut S,
    reads: ReadsBuilder,
}
impl<'a, S: DependencySource> EvaluationProvider<'a, S> {
    pub(crate) fn new(root: DefinitionRef, source: &'a mut S) -> Self {
        Self {
            root,
            source,
            reads: ReadsBuilder::new(),
        }
    }
    fn merge<T>(&mut self, product: DependencyProduct<T>) -> Result<T, QueryError> {
        self.reads.merge(&product.reads);
        product.result
    }
    pub(crate) fn finish(self) -> Result<Reads, QueryError> {
        self.reads.finish().map_err(|_| QueryError::NotImplemented)
    }
}
impl<S: DependencySource> TirProvider for EvaluationProvider<'_, S> {
    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        if path == self.root.path.path() {
            return Ok(self.root.clone());
        }
        let product = self.source.definition(path);
        self.merge(product)
    }
    fn constant(&mut self, path: &DefPath) -> Result<MaterializedRef, QueryError> {
        let product = self.source.constant(path);
        self.merge(product)
    }
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<MaterializedRef, QueryError> {
        let product = self.source.constant_instance(path, arguments);
        self.merge(product)
    }
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        let product = self.source.instance_body(path, arguments);
        self.merge(product)
    }
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_tir::BodyId;

    struct Failed {
        reads: Reads,
        calls: usize,
    }
    impl Failed {
        fn product<T>(&mut self) -> DependencyProduct<T> {
            self.calls += 1;
            DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: self.reads.clone(),
            }
        }
    }
    impl DependencySource for Failed {
        fn definition(&mut self, _: &DefPath) -> DependencyProduct<DefinitionRef> {
            self.product()
        }
        fn constant(&mut self, _: &DefPath) -> DependencyProduct<MaterializedRef> {
            self.product()
        }
        fn constant_instance(
            &mut self,
            _: &DefPath,
            _: &[u64],
        ) -> DependencyProduct<MaterializedRef> {
            self.product()
        }
        fn instance_body(
            &mut self,
            _: &DefPath,
            _: &[GenericArgument],
        ) -> DependencyProduct<Option<DefPath>> {
            self.product()
        }
    }

    #[test]
    fn dependency_failures_preserve_reads_and_private_roots_are_local() {
        let path = DefPath::core("task-root");
        let dependency = DefPath::core("dependency");
        let root = Arc::new(hirsel_tir::Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: Arc::from([]),
            path: DefId::intern(&path),
            signature_hash: ContentHash::default(),
            bodies: vec![],
            entry: BodyId(0),
            contracts: vec![],
        });
        let reads = Reads::from_vec(vec![(
            ReadKey::ExecutedBody(DefId::intern(&dependency)),
            ContentHash::of_bytes(b"body"),
        )])
        .unwrap();
        let mut source = Failed {
            reads: reads.clone(),
            calls: 0,
        };
        let mut provider = EvaluationProvider::new(root.clone(), &mut source);
        assert!(Arc::ptr_eq(&provider.definition(&path).unwrap(), &root));
        assert!(provider.constant_definitions().unwrap().is_empty());
        assert!(provider.definition(&dependency).is_err());
        assert!(provider.constant(&dependency).is_err());
        assert!(provider.constant_instance(&dependency, &[3]).is_err());
        assert!(provider.instance_body(&dependency, &[]).is_err());
        assert_eq!(provider.finish().unwrap(), reads);
        assert_eq!(source.calls, 4);
    }
}
