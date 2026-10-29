//! Original object-control vectors over actual native continuations. The two
//! runtimes settle source discovery and then the immutable native snapshot;
//! no legacy Ready/object controller or test implementation supplies products.
use hirsel_driver::{COMPILER_IDENTITY, native};
use hirsel_engine::{runtime::*, store::ResultStore, *};
use hirsel_syntax::ContentHash;
use native::{
    object_queries::{CodegenKey, ObjectTaskFactory, fragment_product, load_object},
    queries::{
        self, NativeRootKey, NativeRoots,
        arena_tasks::{ARENA_GRAPH_KIND, ArenaTaskFactory, ArenaTasks},
        declarations::NativeDeclarations,
        execution::{EXECUTION_KIND, ExecutionTasks, Factory as ExecutionFactory},
        instance_tasks::{DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTaskFactory, InstanceTasks},
        products::{EmissionOptions, NativeSnapshot},
        root_tasks::{NativeRootFactory, NativeRootTasks},
        source_inputs::NativeInstanceInputs,
        tasks::{CodegenTasks, NativeTaskFactory, NativeTasks},
    },
    root_source::{NativeLinkFacts, NativeLinkSource},
};
use std::{collections::BTreeMap, sync::Arc};

pub(super) struct Objects {
    pub summaries: Vec<hirsel_lir::fragment::UnitSummary>,
    pub objects: Vec<hirsel_lir::stage::ObjectArtifact>,
    pub hits: usize,
    pub products: Vec<(CodegenKey, ProductRef)>,
    pub ready_transformations: u64,
    pub instance_imports: u64,
    pub ready_products: Vec<ProductRef>,
}

pub(super) fn compile(
    engine: &mut QueryEngine,
    store: Arc<dyn ResultStore>,
    backend: native::Backend,
    opt: native::OptLevel,
    options: native::Options,
    debug: bool,
    jobs: hirsel_intern::jobs::Jobs,
) -> Objects {
    compile_entry(
        engine,
        store,
        &entry_path(&default_context()),
        backend,
        opt,
        options,
        debug,
        jobs,
    )
}

pub(super) fn compile_entry(
    engine: &mut QueryEngine,
    store: Arc<dyn ResultStore>,
    entry: &hirsel_syntax::DefPath,
    backend: native::Backend,
    opt: native::OptLevel,
    options: native::Options,
    debug: bool,
    jobs: hirsel_intern::jobs::Jobs,
) -> Objects {
    compile_with_context(
        engine,
        store,
        entry,
        backend,
        opt,
        options,
        debug,
        jobs,
        hirsel_engine::store::VerifyStore::Off,
        |_, _| (),
    )
    .0
}

pub(super) fn compile_with_context<R>(
    engine: &mut QueryEngine,
    store: Arc<dyn ResultStore>,
    entry: &hirsel_syntax::DefPath,
    backend: native::Backend,
    opt: native::OptLevel,
    options: native::Options,
    debug: bool,
    jobs: hirsel_intern::jobs::Jobs,
    verification: hirsel_engine::store::VerifyStore,
    use_context: impl FnOnce(&native::ReadyProgram, &[hirsel_lir::stage::ObjectArtifact]) -> R,
) -> (Objects, R) {
    compile_roots(
        engine,
        store,
        std::slice::from_ref(entry),
        &[],
        backend,
        opt,
        options,
        debug,
        jobs,
        verification,
        use_context,
    )
}

pub(super) fn compile_roots<R>(
    engine: &mut QueryEngine,
    store: Arc<dyn ResultStore>,
    source_selection: &[hirsel_syntax::DefPath],
    requested: &[hirsel_syntax::DefPath],
    backend: native::Backend,
    opt: native::OptLevel,
    options: native::Options,
    debug: bool,
    jobs: hirsel_intern::jobs::Jobs,
    verification: hirsel_engine::store::VerifyStore,
    use_context: impl FnOnce(&native::ReadyProgram, &[hirsel_lir::stage::ObjectArtifact]) -> R,
) -> (Objects, R) {
    compile_roots_inspect(
        engine,
        store,
        source_selection,
        requested,
        backend,
        opt,
        options,
        debug,
        jobs,
        verification,
        |_, _, _| (),
        use_context,
    )
}

pub(super) fn compile_with_context_inspect<R>(
    engine: &mut QueryEngine,
    store: Arc<dyn ResultStore>,
    entry: &hirsel_syntax::DefPath,
    backend: native::Backend,
    opt: native::OptLevel,
    options: native::Options,
    debug: bool,
    jobs: hirsel_intern::jobs::Jobs,
    verification: hirsel_engine::store::VerifyStore,
    inspect: impl FnOnce(
        &mut queries::roots::RootProducts,
        &Arc<queries::NativeRootProduct>,
        &[Arc<queries::ReadyProduct>],
    ),
    use_context: impl FnOnce(&native::ReadyProgram, &[hirsel_lir::stage::ObjectArtifact]) -> R,
) -> (Objects, R) {
    compile_roots_inspect(
        engine,
        store,
        std::slice::from_ref(entry),
        &[],
        backend,
        opt,
        options,
        debug,
        jobs,
        verification,
        inspect,
        use_context,
    )
}

pub(super) fn compile_roots_inspect<R>(
    engine: &mut QueryEngine,
    store: Arc<dyn ResultStore>,
    source_selection: &[hirsel_syntax::DefPath],
    requested: &[hirsel_syntax::DefPath],
    backend: native::Backend,
    opt: native::OptLevel,
    options: native::Options,
    debug: bool,
    jobs: hirsel_intern::jobs::Jobs,
    verification: hirsel_engine::store::VerifyStore,
    inspect: impl FnOnce(
        &mut queries::roots::RootProducts,
        &Arc<queries::NativeRootProduct>,
        &[Arc<queries::ReadyProduct>],
    ),
    use_context: impl FnOnce(&native::ReadyProgram, &[hirsel_lir::stage::ObjectArtifact]) -> R,
) -> (Objects, R) {
    let mut cache = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
    let mut declarations = NativeDeclarations::collect(engine, &mut cache, requested).unwrap();
    let paths = declarations.sources.keys().cloned().collect::<Vec<_>>();
    let original = engine
        .native_original_reader(&paths, store.clone())
        .unwrap();
    for (path, source) in &mut declarations.sources {
        Arc::make_mut(source).lower = original
            .input(path)
            .and_then(|(output, _, _, _)| output.cloned());
    }
    let semantic = Arc::new(
        engine
            .native_semantic_program(declarations.roots.definitions.clone())
            .unwrap(),
    );
    compile_settled(
        declarations,
        original,
        engine.signature_snapshot(),
        semantic,
        store,
        source_selection,
        backend,
        opt,
        options,
        debug,
        jobs,
        verification,
        false,
        inspect,
        use_context,
    )
}

pub(super) fn compile_front<R>(
    facts: Arc<FrontNativeProducts>,
    store: Arc<dyn ResultStore>,
    source_selection: &[hirsel_syntax::DefPath],
    backend: native::Backend,
    opt: native::OptLevel,
    options: native::Options,
    debug: bool,
    jobs: hirsel_intern::jobs::Jobs,
    verification: hirsel_engine::store::VerifyStore,
    execution: bool,
    inspect: impl FnOnce(
        &mut queries::roots::RootProducts,
        &Arc<queries::NativeRootProduct>,
        &[Arc<queries::ReadyProduct>],
    ),
    use_context: impl FnOnce(&native::ReadyProgram, &[hirsel_lir::stage::ObjectArtifact]) -> R,
) -> (Objects, R) {
    let declarations = NativeDeclarations::from_front(facts.clone()).unwrap();
    let original = facts
        .original_context
        .reader(
            facts.originals.iter().map(|product| (**product).clone()),
            store.clone(),
        )
        .unwrap();
    compile_settled(
        declarations,
        original,
        facts.signatures.clone(),
        facts.semantic.clone(),
        store,
        source_selection,
        backend,
        opt,
        options,
        debug,
        jobs,
        verification,
        execution,
        inspect,
        use_context,
    )
}

fn compile_settled<R>(
    declarations: NativeDeclarations,
    original: NativeOriginalReader,
    signatures: Arc<SignatureSnapshot>,
    semantic: Arc<NativeSemanticProgram>,
    store: Arc<dyn ResultStore>,
    source_selection: &[hirsel_syntax::DefPath],
    backend: native::Backend,
    opt: native::OptLevel,
    options: native::Options,
    debug: bool,
    jobs: hirsel_intern::jobs::Jobs,
    verification: hirsel_engine::store::VerifyStore,
    execution: bool,
    inspect: impl FnOnce(
        &mut queries::roots::RootProducts,
        &Arc<queries::NativeRootProduct>,
        &[Arc<queries::ReadyProduct>],
    ),
    use_context: impl FnOnce(&native::ReadyProgram, &[hirsel_lir::stage::ObjectArtifact]) -> R,
) -> (Objects, R) {
    let entries = declarations
        .context
        .sources
        .iter()
        .filter(|(path, _)| {
            matches!(&declarations.program.signatures[*path].kind,
            hirsel_types::SignatureKind::Function(function) if function.generics.is_empty())
        })
        .map(|(path, source)| (path.clone(), source.entry.0))
        .collect::<BTreeMap<_, _>>();
    let source = Arc::new(
        NativeLinkSource::from_settled(
            NativeLinkFacts {
                program: &declarations.program,
                operations: Arc::new(declarations.program.operation_registry()),
                roots: &declarations.roots,
                declarations: &declarations.sources,
                entries: &entries,
                anchors: &declarations.source_anchors,
                signatures,
                semantic,
                selected: source_selection,
                callees: None,
            },
            &original,
        )
        .unwrap(),
    );
    let manifest = declarations.manifest().unwrap();
    let emission = EmissionOptions::request(
        declarations.program.input.clone(),
        opt,
        debug,
        source.foreign_dispatch(),
        true,
        false,
        false,
    );
    let inputs = NativeInstanceInputs::from_products(declarations, original).unwrap();
    let policy = ContentHash::of_bytes(b"original native object controls");
    let target = options.target.clone();
    let runtime_bytes = (jobs.get() as u64)
        .checked_mul(hirsel_intern::jobs::COMPILER_STACK_BYTES as u64)
        .unwrap()
        + (64 << 20);
    let budget = ProcessBudget::new(runtime_bytes + (192 << 20));
    // Source/registry products retain their own measured grants. Continuation
    // constructors bound the descriptor frame within this separate allowance.
    let weight = MemoryWeight {
        retained: 256 << 10,
        scratch: 16 << 20,
        stack: 0,
    };
    let instances = InstanceTasks::new(
        inputs,
        store.clone(),
        options.clone(),
        policy,
        weight,
        budget.reserve(64 << 20).unwrap(),
    )
    .unwrap()
    .with_store_verification(verification)
    .unwrap();
    let arenas = ArenaTasks::new(
        instances.clone(),
        store.clone(),
        0,
        weight,
        budget.reserve(64 << 20).unwrap(),
    )
    .unwrap();
    let denied = instances.failure("original native object control resource refusal");
    let errors = RuntimeErrors {
        resource: denied,
        cycle: denied,
        cancelled: denied,
        protocol: denied,
        panicked: denied,
    };
    let mut discovery =
        AdmittedRuntime::new(jobs, budget.reserve(runtime_bytes).unwrap(), errors).unwrap();
    let factory = Arc::new(InstanceTaskFactory::new(instances.clone()));
    for kind in [
        queries::INSTANCE_KIND,
        queries::instance::CALLABLE_VALUE_KIND,
        queries::instance_tasks::CALLABLE_GRAPH_KIND,
        INSTANCE_INPUT_KIND,
        DISCOVERY_KIND,
    ] {
        discovery.register(kind, factory.clone()).unwrap();
    }
    let factory = Arc::new(ArenaTaskFactory::new(arenas.clone()));
    for kind in [
        queries::ARENA_CANDIDATES_KIND,
        queries::ARENA_CHOSEN_KIND,
        ARENA_GRAPH_KIND,
    ] {
        discovery.register(kind, factory.clone()).unwrap();
    }
    let request = discovery.request(&[instances.discovery_id(), arenas.query_id()]);
    let selected = discovery.wait(request).unwrap_or_else(|error| {
        panic!(
            "{}",
            String::from_utf8_lossy(&store.get_content(error).unwrap())
        )
    });
    let completed = instances.completed_instances(selected[0]).unwrap();
    let operations = instances.source_operations(selected[0]).unwrap();
    let snapshot = NativeSnapshot::new(
        store.clone(),
        instances.products(selected[0]).unwrap(),
        arenas.graph(selected[1]).unwrap(),
        options,
        emission,
        manifest,
        Arc::new(operations),
    )
    .unwrap()
    .with_store_verification(verification)
    .unwrap();
    let native = NativeTasks::new(snapshot, weight).unwrap();
    let roots = NativeRootTasks::new(
        native.clone(),
        source.clone(),
        NativeRootKey {
            roots: source_selection.to_vec(),
            target_abi: target.digest(),
            policy,
        },
        weight,
    )
    .unwrap();
    let keys = completed
        .iter()
        .map(|(owner, _)| CodegenKey {
            owner: owner.clone(),
            backend: backend.name().into(),
            backend_version: ContentHash::of_bytes(&COMPILER_IDENTITY),
            target_features: target.digest(),
            profile_options: ContentHash::of_bytes(format!("{opt:?}/{debug}").as_bytes()),
            runtime_abi: ContentHash::of_bytes(&hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes()),
        })
        .collect::<Vec<_>>();
    let previous = keys
        .iter()
        .map(|key| store.lookup_product(key.address()))
        .collect::<Vec<_>>();
    let objects = CodegenTasks::new(native.clone(), keys.clone(), weight).unwrap();
    drop(discovery);
    let mut runtime =
        AdmittedRuntime::new(jobs, budget.reserve(runtime_bytes).unwrap(), errors).unwrap();
    let factory = Arc::new(NativeTaskFactory::new(native.clone()));
    for kind in [
        queries::INSTANCE_KIND,
        queries::instance::CALLABLE_VALUE_KIND,
        queries::ARENA_CANDIDATES_KIND,
        queries::ARENA_CHOSEN_KIND,
        queries::READY_KIND,
    ] {
        runtime.register(kind, factory.clone()).unwrap();
    }
    runtime
        .register(
            queries::ROOT_KIND,
            Arc::new(NativeRootFactory::new(roots.clone())),
        )
        .unwrap();
    runtime
        .register(
            native::object_queries::CODEGEN_KIND,
            Arc::new(ObjectTaskFactory::new(objects)),
        )
        .unwrap();
    if execution {
        let tasks = ExecutionTasks::new(roots.clone(), &[], weight);
        runtime
            .register(EXECUTION_KIND, Arc::new(ExecutionFactory(tasks.clone())))
            .unwrap();
        let request = runtime.request(&[tasks.query_id()]);
        runtime.wait(request).unwrap_or_else(|error| {
            panic!(
                "{}",
                String::from_utf8_lossy(&store.get_content(error).unwrap())
            )
        });
    }
    let requests = keys
        .iter()
        .map(CodegenKey::query_id)
        .chain(keys.iter().map(|key| key.owner.ready_query_id()))
        .chain(std::iter::once(roots.query_id()))
        .collect::<Vec<_>>();
    let request = runtime.request(&requests);
    let selected = runtime.wait(request).unwrap_or_else(|error| {
        panic!(
            "{}",
            String::from_utf8_lossy(&store.get_content(error).unwrap())
        )
    });
    let count = keys.len();
    let hits = previous
        .iter()
        .zip(&selected[..count])
        .filter(|(old, now)| {
            old.as_ref()
                .is_some_and(|old| old.action == now.action && old.reference == now.record)
        })
        .count();
    let artifacts = selected[..count]
        .iter()
        .map(|product| {
            load_object(
                store.as_ref(),
                fragment_product(store.as_ref(), *product).unwrap().object,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let ready = &selected[count..count * 2];
    let (mut products, root) = roots.products(selected[count * 2], ready).unwrap();
    let readies = ready
        .iter()
        .map(|product| queries::ready_product(store.as_ref(), *product).map(Arc::new))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    inspect(&mut products, &root, &readies);
    let link_objects = products
        .backend_objects(&root, &readies, backend, &artifacts)
        .unwrap();
    let (summaries, output) = products
        .with_link_context(&root, &readies, |program| {
            assert!(
                program.units.is_empty(),
                "fragment checks reconstructed Ready bodies"
            );
            Ok((
                program.summaries.clone(),
                use_context(program, &link_objects),
            ))
        })
        .unwrap();
    assert_eq!(summaries.len(), artifacts.len());
    (
        Objects {
            summaries,
            objects: artifacts,
            hits,
            products: keys
                .into_iter()
                .zip(selected[..count].iter().copied())
                .collect(),
            instance_imports: instances.imports.load(std::sync::atomic::Ordering::Relaxed),
            ready_products: ready.to_vec(),
            ready_transformations: native
                .ready_transformations
                .load(std::sync::atomic::Ordering::Relaxed),
        },
        output,
    )
}

#[test]
fn generic_lexical_callable_keeps_its_original_recipe_dependency() {
    with_compiler_stack(|| {
        let store: Arc<dyn ResultStore> = Arc::new(store::MemStore::default());
        let mut engine =
            QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters())
                .with_result_store(store.clone());
        let file = engine
            .sources
            .add(
                "generic-lexical.hir",
                r#"
pub fn main()->Result[(),()] {
  let tag=9u64
  fn tagged[T](take value:T)->(u64,T) { (tag,value) }
  let pair:fn(take u64)->(u64,u64) uses {}=tagged
  failure.check(pair(4u64)==(9u64,4u64),Bounds())
  Ok(())
}
"#
                .into(),
            )
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let actual = compile(
            &mut engine,
            store,
            native::Backend::Cranelift,
            native::OptLevel::None,
            native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            false,
            hirsel_intern::jobs::Jobs::ONE,
        );
        assert!(!actual.objects.is_empty());
        assert_eq!(actual.objects.len(), actual.summaries.len());
    });
}
