//! Current-source factory assembly. These are original graph/profile controls;
//! no receipt-aware ABI, Some native provider or public seal is activated here.
use hirsel_driver::{
    compiler_adapters,
    native::receipt_source::{factory::*, provider::*},
};
use hirsel_engine::*;
use hirsel_rc::{
    instance::{Actuals, key_of},
    sealed_source::SourceError,
};
use hirsel_syntax::{DefId, DefPath};
use hirsel_types::{GenericArgument, SignatureRef, TypeInterner};
use std::sync::Arc;

struct Simulated<'a> {
    front: &'a Front<'a>,
    unavailable: bool,
}
impl Provider for Simulated<'_> {
    fn types(&self) -> &TypeInterner {
        self.front.types()
    }
    fn current(&self, path: &DefPath) -> Result<Current, SourceError> {
        if self.unavailable {
            return Err(SourceError::Unsupported("simulated unknown source"));
        }
        self.front.current(path)
    }
    fn signature(&self, path: &DefPath) -> Result<SignatureRef, SourceError> {
        self.front.signature(path)
    }
    fn original(&self, path: &DefPath) -> Result<Arc<NativeOriginalBody>, SourceError> {
        self.front.original(path)
    }
}
impl SelectionProvider for Simulated<'_> {
    fn selected_instance(
        &self,
        declaration: DefId,
        arguments: &[GenericArgument],
    ) -> Result<Option<DefId>, SourceError> {
        self.front.selected_instance(declaration, arguments)
    }
}
fn source(text: &str) -> (NativeOriginalReader, DefPath) {
    let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
        .with_result_store(store.clone());
    let file = engine
        .sources
        .add("receipt-factory.hir", text.into())
        .unwrap();
    let report = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let path = entry_path(&default_context());
    let roots = engine.native_source_roots(&[path.clone()]).unwrap();
    let reader = engine
        .native_original_reader(&roots.definitions, store)
        .unwrap();
    (reader, path)
}
fn root(front: &Front<'_>, path: &DefPath) -> NamedInput {
    let original = front.original(path).unwrap();
    NamedInput {
        base: key_of(
            front.types(),
            path,
            original.definition().entry,
            &Actuals::none(),
            None,
            None,
        )
        .unwrap(),
        actuals: Actuals::none(),
    }
}

#[test]
fn source_factory_shares_original_named_contexts_and_keeps_ordinary_keys() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
fn id[T](take x:T)->T { take x }
fn wrap[T](take x:T)->T { id(take x) }
pub fn main()->u64 {
  let a=wrap(7u64)
  let b=wrap(11u64)
  a+b
}
"#,
        );
        let front = Front::new(&reader);
        let graph = named_graph(
            &front,
            [root(&front, &path), root(&front, &path)],
            &mut |_, _, _| panic!("no held callable port"),
        )
        .unwrap();
        assert_eq!(graph.contexts.len(), 3);
        assert_eq!(graph.roots[0], graph.roots[1]);
        assert_eq!(graph.requests.len(), 3);
        for (context, request) in graph.contexts.iter().zip(&graph.requests) {
            assert_eq!(request.ordinary_key(), &context.input.base);
            assert!(request.aware_key().is_none());
            assert!(request.declaration().contract().is_none());
            assert_eq!(context.source.entry(), request.declaration().entry());
            assert_eq!(
                context.ports.len(),
                context.source.entry().parameters().len() + 1
            );
        }
        let mut simulated = Simulated {
            front: &front,
            unavailable: false,
        };
        let same = named_graph(&simulated, [root(&front, &path)], &mut |_, _, _| {
            panic!("no callable profile query")
        })
        .unwrap();
        assert_eq!(
            graph
                .requests
                .iter()
                .map(|r| r.context_key())
                .collect::<Vec<_>>(),
            same.requests
                .iter()
                .map(|r| r.context_key())
                .collect::<Vec<_>>()
        );
        simulated.unavailable = true;
        assert!(matches!(
            named_graph(&simulated, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Unsupported("simulated unknown source"))
        ));
    });
}

#[test]
fn source_factory_resolves_a_recursive_original_graph_without_clone_limits() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
fn count(n:u64)->u64 { if n==0u64 {0u64} else {count(n-1u64)} }
pub fn main()->u64 { count(3u64) }
"#,
        );
        let front = Front::new(&reader);
        let graph = named_graph(&front, [root(&front, &path)], &mut |_, _, _| {
            panic!("no held callable port")
        })
        .unwrap();
        assert_eq!(graph.contexts.len(), 2);
        assert_eq!(graph.requests.len(), 2);
        assert!(graph.requests.iter().all(|r| r.aware_key().is_none()));
        assert!(
            graph
                .contexts
                .iter()
                .any(|c| c.input.base.definition().0.name == "count")
        );
    });
}

#[test]
fn source_factory_refuses_unexpanded_original_closures_even_without_a_seal() {
    with_compiler_stack(|| {
        let (reader, path) = source(
            r#"
pub fn main()->u64 {
  let f=fn()->u64 {7u64}
  f()
}
"#,
        );
        let front = Front::new(&reader);
        assert!(matches!(
            named_graph(&front, [root(&front, &path)], &mut |_, _, _| Ok(vec![])),
            Err(SourceError::Unsupported(
                "receipt factory needs original closure context expansion"
            ))
        ));
    });
}
