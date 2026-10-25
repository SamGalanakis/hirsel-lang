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
    use_context: impl FnOnce(&native::ReadyProgram, &[hirsel_lir::stage::ObjectArtifact]) -> R,
) -> (Objects, R) {
    let verification = engine.verify_store;
    let mut cache = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
    let mut declarations = NativeDeclarations::collect(engine, &mut cache, &[]).unwrap();
    let preliminary = engine
        .native_semantic_program(declarations.roots.definitions.clone())
        .unwrap();
    let mut paths = declarations
        .context
        .sources
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    paths.extend(
        preliminary
            .source_bindings()
            .map(|(path, _, _)| path.clone()),
    );
    // Semantic revisions also bind compile-time generators. Retain their source
    // declarations/anchors without registering them as runtime instances.
    let mut link_declarations = declarations.sources.clone();
    let mut link_anchors = declarations.source_anchors.clone();
    for path in &paths {
        if link_declarations.contains_key(path) {
            continue;
        }
        let product = cache.definition(engine, path, &paths).unwrap();
        link_anchors.insert(
            path.clone(),
            product.contracts.as_ref().map_or_else(
                || vec![product.source.source.syntax.anchor],
                |contracts| contracts.source_anchors.clone(),
            ),
        );
        declarations
            .program
            .signatures
            .insert(path.clone(), product.source.signature.clone());
        link_declarations.insert(path.clone(), product.source);
    }
    let original = engine
        .native_original_reader(&paths.into_iter().collect::<Vec<_>>(), store.clone())
        .unwrap();
    for (path, source) in &mut declarations.sources {
        source.lower = original
            .input(path)
            .and_then(|(output, _, _, _)| output.cloned());
    }
    for (path, source) in &mut link_declarations {
        source.lower = original
            .input(path)
            .and_then(|(output, _, _, _)| output.cloned());
    }
    let semantic = Arc::new(
        engine
            .native_semantic_program(declarations.roots.definitions.clone())
            .unwrap(),
    );
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
                declarations: &link_declarations,
                entries: &entries,
                anchors: &link_anchors,
                signatures: engine.signature_snapshot(),
                semantic,
                selected: std::slice::from_ref(entry),
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
    let weight = MemoryWeight {
        retained: 1 << 20,
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
    for kind in [queries::INSTANCE_KIND, INSTANCE_INPUT_KIND, DISCOVERY_KIND] {
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
            roots: vec![entry.clone()],
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
    let factory = Arc::new(NativeTaskFactory::new(native));
    for kind in [
        queries::INSTANCE_KIND,
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
    let (summaries, output) = products
        .with_link_context(&root, &readies, |program| {
            assert!(
                program.units.is_empty(),
                "fragment checks reconstructed Ready bodies"
            );
            Ok((program.summaries.clone(), use_context(program, &artifacts)))
        })
        .unwrap();
    assert_eq!(summaries.len(), artifacts.len());
    (
        Objects {
            summaries,
            objects: artifacts,
            hits,
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
