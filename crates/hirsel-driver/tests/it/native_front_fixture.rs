use hirsel_driver::requests::{CompileOperation, CompileRequest, CompileResult, CompilerRequests};
use hirsel_engine::{
    requests::{OutputReadiness, RequestEngine, observe_inputs},
    runtime::QueryRuntime,
    *,
};
use hirsel_types::Signature;
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct GuardedLowering(Arc<AtomicBool>);
impl LoweringPhase for GuardedLowering {
    fn lower(
        &self,
        types: &hirsel_types::TypeInterner,
        memo: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        comptime: &Comptime,
        signature: &Signature,
    ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
        assert!(
            self.0.load(Ordering::Acquire),
            "fresh native fixture attempted lowering instead of reading its product"
        );
        hirsel_driver::LowerAdapter.lower(types, memo, body, comptime, signature)
    }
    fn lower_evaluation(
        &self,
        types: &hirsel_types::TypeInterner,
        memo: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
        signature: &Signature,
        expression: hirsel_syntax::ExprId,
        ty: hirsel_types::TypeId,
        constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
    ) -> Result<hirsel_tir::Definition, QueryError> {
        assert!(
            self.0.load(Ordering::Acquire),
            "fresh native fixture attempted evaluation lowering instead of reading its product"
        );
        hirsel_driver::LowerAdapter.lower_evaluation(
            types, memo, body, staged, signature, expression, ty, constants,
        )
    }
    fn entry_body(&self) -> hirsel_tir::BodyId {
        hirsel_driver::LowerAdapter.entry_body()
    }
}
struct Host {
    store: Arc<dyn store::ResultStore>,
    verify: Verify,
    selector: CompilerRequests,
}
thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    static LOWERING_ALLOWED: Arc<AtomicBool> = Arc::new(AtomicBool::new(true));
}
pub(super) fn release_host() {
    HOST.with(|host| drop(host.borrow_mut().take()));
}

fn guarded_adapters() -> PhaseAdapters {
    let mut adapters = hirsel_driver::compiler_adapters();
    adapters.lowering = Some(Arc::new(GuardedLowering(LOWERING_ALLOWED.with(Arc::clone))));
    adapters
}

pub(super) fn products(
    engine: &mut QueryEngine,
    requested: &[DefPath],
) -> Result<Arc<FrontNativeProducts>, QueryError> {
    let refused = |error: ToolError| match error {
        ToolError::Query { error, .. } => error,
        error => QueryError::Operational(hirsel_types::OperationalFailure {
            operation: "native source fixture".into(),
            message: error.to_string(),
        }),
    };
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("hirsel.json");
    let paths = engine.front_source_roots();
    let root = paths
        .first()
        .ok_or(QueryError::NotImplemented)?
        .0
        .package
        .clone();
    let mut files = std::collections::BTreeMap::new();
    for path in &paths {
        assert_eq!(path.0.package, root);
        let syntax = engine.parse_definition(path)?.value;
        if syntax.item.name.as_str() != path.0.name {
            continue;
        }
        let file = engine
            .generated_source(path)
            .map(|generated| engine.sources.resolve(generated.site).file)
            .or_else(|| engine.sources.anchor(syntax.anchor).map(|(file, _)| file))
            .ok_or(QueryError::NotImplemented)?;
        files.insert(file, path.0.modules.clone());
    }
    let mut sources = Vec::new();
    for (index, (file, modules)) in files.into_iter().enumerate() {
        let name = format!("source-{index}.hir");
        std::fs::write(
            directory.path().join(&name),
            engine.sources.file(file).unwrap().text(),
        )
        .unwrap();
        sources.push(serde_json::json!({"path":name, "module":modules}));
    }
    std::fs::write(
        &manifest,
        serde_json::to_vec(&serde_json::json!({
            "schema":1, "package":{"name":root.name,"version":root.version},
            "sources":sources,"dependencies":[],
        }))
        .unwrap(),
    )
    .unwrap();
    let config = CompileConfig {
        store: engine
            .product_store()
            .unwrap_or_else(|| Arc::new(store::MemStore::default())),
        verify: engine.verify,
        hold_bytes: 0,
        ..Default::default()
    };
    LOWERING_ALLOWED
        .with(|allowed| allowed.store(engine.adapters.lowering.is_some(), Ordering::Release));
    HOST.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_none_or(|host| {
            !Arc::ptr_eq(&host.store, &config.store) || host.verify != config.verify
        }) {
            let compiler = hirsel_driver::COMPILER
                .with_config(config.clone())
                .with_owned_engines()
                .with_phase_adapters(guarded_adapters, hirsel_driver::COMPILER_IDENTITY);
            *slot = Some(Host {
                store: config.store.clone(),
                verify: config.verify,
                selector: CompilerRequests::for_host(compiler, config),
            });
        }
        let selector = &mut slot.as_mut().unwrap().selector;
        let compiler = selector.compiler().clone();
        let (loaded, inputs) = observe_inputs(
            selector.input_limits(),
            selector.capture_attempts(),
            |inputs| compiler.load_check_package_inputs(&manifest, inputs, true),
        )
        .unwrap();
        drop(loaded.map_err(&refused)?);
        let mut selected = selector
            .select(&CompileRequest {
                path: manifest,
                inputs,
                operation: CompileOperation::NativeSources {
                    roots: requested.to_vec(),
                },
            })
            .map_err(&refused)?;
        let completed = {
            let mut runtime = selected.runtime.lock().unwrap();
            let request = runtime.request(&selected.roots);
            runtime.wait(request)
        }
        .map_err(|reference| {
            refused(selected.output.failed(reference).unwrap_or_else(|| {
                ToolError::Runtime("native source fixture protocol failure".into())
            }))
        })?;
        let output = selected.output.ready(&completed).map_err(&refused)?;
        match &output.result {
            CompileResult::NativeSources(facts) => Ok(facts.clone()),
            _ => unreachable!(),
        }
    })
}

pub(super) fn inputs(
    engine: &mut QueryEngine,
    requested: &[DefPath],
) -> Arc<hirsel_driver::native::queries::source_inputs::NativeInstanceInputs> {
    let facts = products(engine, requested).unwrap();
    let reader = facts
        .original_context
        .reader(facts.originals.iter().map(|product| (**product).clone()))
        .unwrap();
    let declarations =
        hirsel_driver::native::queries::declarations::NativeDeclarations::from_front(facts)
            .unwrap();
    hirsel_driver::native::queries::source_inputs::NativeInstanceInputs::from_products(
        declarations,
        reader,
    )
    .unwrap()
}
