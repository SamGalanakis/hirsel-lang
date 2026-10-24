use super::scheduled_link::Platform;
use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::ResultStore, *};
use hirsel_syntax::ContentHash;
use native::queries::{
    ARENA_CANDIDATES_KIND, ARENA_CHOSEN_KIND, INSTANCE_KIND, NativeRootKey, NativeRoots,
    READY_KIND, ROOT_KIND,
    arena_tasks::{ARENA_GRAPH_KIND, ArenaTaskFactory, ArenaTasks},
    compilation_tasks::{
        COMPILATION_KIND, CompilationBackend, CompilationFactory, CompilationTasks,
    },
    declarations::NativeDeclarations,
    instance_tasks::{DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTaskFactory, InstanceTasks},
    link_tasks::{LINK_INPUT_KIND, LINK_MANIFEST_KIND},
    platform::FrozenPlatform,
    products::EmissionOptions,
    root_source::NativeLinkSource,
    source_inputs::NativeInstanceInputs,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::Poll,
};

#[derive(Default)]
struct Events(Mutex<Vec<RuntimeEvent>>);
impl RuntimeObserver for Events {
    fn record(&self, event: RuntimeEvent) {
        self.0.lock().unwrap().push(event);
    }
}
struct TraceFactory(Arc<dyn QueryFactory>, hirsel_lir::construction::ValueTrace);
struct TraceTask(Box<dyn QueryTask>, hirsel_lir::construction::ValueTrace);
impl QueryFactory for TraceFactory {
    fn create(&self, key: QueryId) -> Result<Box<dyn QueryTask>, store::Reference> {
        Ok(Box::new(TraceTask(self.0.create(key)?, self.1.clone())))
    }
}
impl QueryTask for TraceTask {
    fn id(&self) -> QueryId {
        self.0.id()
    }
    fn weight(&self) -> MemoryWeight {
        self.0.weight()
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let _guard = self.1.enter();
        self.0.poll(cx)
    }
}
struct Pending;
impl QueryContext for Pending {
    fn read(&mut self, _: QueryId) -> Poll<Result<ProductRef, store::Reference>> {
        Poll::Pending
    }
}
struct Failed(store::Reference);
impl QueryContext for Failed {
    fn read(&mut self, _: QueryId) -> Poll<Result<ProductRef, store::Reference>> {
        Poll::Ready(Err(self.0))
    }
}

#[test]
fn one_native_runtime_produces_cold_instances_both_arena_strata_ready_roots_and_linked_executables()
{
    with_compiler_stack(|| {
        for jobs in [1, 4] {
            run(jobs, false);
        }
    });
}
#[test]
fn frozen_platform_links_standalone_after_original_tool_kit_runtime_and_foreign_paths_disappear() {
    with_compiler_stack(|| {
        for jobs in [1, 4] {
            run(jobs, true);
        }
    });
}
fn copy_files(source: &std::path::Path, destination: &std::path::Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let path = entry.unwrap().path();
        let target = destination.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_files(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}
struct CountedPlatform {
    inner: Arc<dyn native::queries::link_tasks::NativeLinkExecutor>,
    calls: Arc<AtomicU64>,
}
impl native::queries::link_tasks::NativeLinkExecutor for CountedPlatform {
    fn link(
        &self,
        native: &mut native::queries::roots::RootProducts,
        input: native::queries::link_tasks::LinkExecution<'_>,
    ) -> Result<Vec<u8>, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.link(native, input)
    }
}
fn run(jobs: usize, freeze_platform: bool) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store::DirStore::new(directory.path().join("products")));
    let target = native::TargetSpec::x86_64_unknown_linux_gnu();
    let tools =
        native::Toolchain::in_tree_diagnostic(&directory.path().join("runtime"), &[&target])
            .unwrap();
    let archive = native::object_queries::store_link_input(
        store.as_ref(),
        &std::fs::read(tools.archive(&target.triple).unwrap()).unwrap(),
    )
    .unwrap();
    let platform = ContentHash::of_parts(
        "fixture frozen platform",
        [
            std::fs::read(&tools.lld).unwrap().as_slice(),
            std::fs::read(tools.kit.join("manifest.json"))
                .unwrap()
                .as_slice(),
            std::fs::read(tools.kit.join("digest")).unwrap().as_slice(),
            COMPILER_IDENTITY.as_slice(),
        ],
    );
    let frozen = if freeze_platform {
        let source = directory.path().join("mutable-platform");
        let tree = tools.lld.parent().unwrap().parent().unwrap();
        std::fs::create_dir_all(source.join("llvm/bin")).unwrap();
        std::fs::copy(&tools.lld, source.join("llvm/bin/ld.lld")).unwrap();
        // Only regular top-level linker runtime libraries are consumed.
        std::fs::create_dir_all(source.join("llvm/lib")).unwrap();
        for entry in std::fs::read_dir(tree.join("lib")).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                std::fs::copy(
                    &path,
                    source.join("llvm/lib").join(path.file_name().unwrap()),
                )
                .unwrap();
            }
        }
        copy_files(&tools.kit, &source.join("kit"));
        std::fs::copy(
            tools.archive(&target.triple).unwrap(),
            source.join("runtime.a"),
        )
        .unwrap();
        let foreign = source.join("foreign.a");
        std::fs::write(&foreign, b"!<arch>\n").unwrap();
        let mut selected = tools.clone();
        selected.lld = source.join("llvm/bin/ld.lld");
        selected.kit = source.join("kit");
        selected.runtime.insert(
            (target.triple.clone(), selected.runtime_build),
            source.join("runtime.a"),
        );
        let frozen = FrozenPlatform::capture(
            store.clone(),
            &selected,
            &target,
            &[LinkInput::File(foreign)],
        )
        .unwrap();
        let manifest = store.get_content(frozen.manifest()).unwrap();
        assert!(
            !std::str::from_utf8(&manifest)
                .unwrap()
                .contains(source.to_str().unwrap()),
            "platform identities contain no admission directory"
        );
        std::fs::rename(&source, directory.path().join("retired-platform")).unwrap();
        assert!(!selected.lld.exists());
        assert!(!selected.kit.exists());
        assert!(!selected.archive(&target.triple).unwrap().exists());
        Some(frozen)
    } else {
        None
    };
    let policy = ContentHash::of_bytes(b"one runtime cold native Link control");
    let mut previous = None;
    let mut reference = None;
    for (iteration, (literal, fresh)) in [(7u64, false), (7, true), (8, false), (8, true)]
        .into_iter()
        .enumerate()
    {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine.sources.add("cold-scheduled-link.hir", format!("const SECOND:u64=comptime {literal}u64\npub fn main()->Result[(),u64] {{ Error(SECOND) }}\n")).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let main = entry_path(&default_context());
        if !fresh {
            let execution = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: main.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            reference = Some(native::Observation::of_reference(
                execution.observation.as_ref().unwrap(),
            ));
        } else {
            engine.adapters.lowering = None;
        }
        let mut source_queries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let declarations =
            NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
        if fresh {
            assert_eq!(source_queries.counts.body_demands, 0);
        }
        let source = Arc::new(NativeLinkSource::capture(&mut engine, &declarations).unwrap());
        let mut originals = Vec::new();
        if !fresh {
            let body = engine.lower_to_tir(&main).unwrap().value;
            originals.push(Arc::downgrade(&body));
        }
        let emission = EmissionOptions {
            input: declarations.program.input.clone(),
            opt: native::OptLevel::None,
            debug: true,
            callbacks: false,
            tracking: false,
            foreign: Default::default(),
            inline_instances: Default::default(),
            verify_backend: true,
            measurement: false,
            fuel: false,
        };
        let inputs =
            NativeInstanceInputs::complete(&mut engine, declarations, store.clone()).unwrap();
        drop(engine);
        assert!(originals.iter().all(|body| body.upgrade().is_none()));
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _guard = trace.enter();
        let budget = ProcessBudget::new(768 << 20);
        let weight = MemoryWeight {
            scratch: 32 << 20,
            retained: 1 << 20,
            stack: 0,
        };
        let instances = InstanceTasks::new(
            inputs,
            store.clone(),
            native::Options::new(target.clone()),
            policy,
            weight,
            budget.reserve(32 << 20).unwrap(),
        )
        .unwrap();
        let arena = ArenaTasks::new(
            instances.clone(),
            store.clone(),
            0,
            weight,
            budget.reserve(32 << 20).unwrap(),
        )
        .unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let backends = [native::Backend::Llvm, native::Backend::Cranelift]
            .into_iter()
            .map(|backend| CompilationBackend {
                backend: backend.name().into(),
                backend_version: ContentHash::of_parts(
                    "backend",
                    [backend.name().as_bytes(), COMPILER_IDENTITY.as_slice()],
                ),
                target_features: target.digest(),
                profile_options: ContentHash::of_bytes(b"debug opt none"),
                runtime_abi: ContentHash::of_bytes(&hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes()),
                linker_options: if let Some(platform) = &frozen {
                    platform.linker_options(backend, &main).unwrap()
                } else {
                    ContentHash::of_parts(
                        "fixture selected backend",
                        [platform.0.as_slice(), backend.name().as_bytes()],
                    )
                },
                runtime_archives: frozen
                    .as_ref()
                    .map_or_else(|| vec![archive], |platform| platform.runtime_archives()),
                foreign_inputs: frozen
                    .as_ref()
                    .map_or_else(Vec::new, |platform| platform.foreign_inputs().to_vec()),
                executor: if let Some(platform) = &frozen {
                    Arc::new(CountedPlatform {
                        inner: platform.executor(
                            backend,
                            main.clone(),
                            directory
                                .path()
                                .join(format!("{literal}-{fresh}-{}", backend.name())),
                        ),
                        calls: calls.clone(),
                    })
                } else {
                    Arc::new(Platform {
                        store: store.clone(),
                        tools: tools.clone(),
                        target: target.clone(),
                        backend,
                        entry: main.clone(),
                        directory: directory
                            .path()
                            .join(format!("{literal}-{fresh}-{}", backend.name())),
                        calls: calls.clone(),
                    })
                },
            })
            .collect();
        let compilation = CompilationTasks::new(
            instances.clone(),
            arena.clone(),
            source.clone(),
            NativeRootKey {
                roots: vec![main],
                target_abi: target.digest(),
                policy,
            },
            emission,
            backends,
            weight,
            budget.reserve(32 << 20).unwrap(),
        )
        .unwrap();
        let compilation_factory = Arc::new(CompilationFactory::new(compilation.clone()));
        let mut lazy = compilation_factory.create(compilation.query_id()).unwrap();
        assert!(
            matches!(lazy.poll(&mut Pending), QueryPoll::Waiting(ref keys) if keys == &vec![instances.discovery_id(), arena.query_id()])
        );
        assert_eq!(instances.imports.load(Ordering::Relaxed), 0);
        assert_eq!(arena.solves.load(Ordering::Relaxed), 0);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(trace.totals(), Default::default());
        let denied = instances.failure("source discovery refused");
        assert!(
            matches!(lazy.poll(&mut Failed(denied)), QueryPoll::Failed(error) if error == denied)
        );
        drop(lazy);
        let errors = RuntimeErrors {
            resource: denied,
            cycle: denied,
            cancelled: denied,
            protocol: denied,
            panicked: denied,
        };
        let mut runtime = AdmittedRuntime::new(
            hirsel_intern::jobs::Jobs::new(jobs),
            budget.reserve(672 << 20).unwrap(),
            errors,
        )
        .unwrap();
        let instance_factory = Arc::new(TraceFactory(
            Arc::new(InstanceTaskFactory::new(instances.clone())),
            trace.clone(),
        ));
        let arena_factory = Arc::new(TraceFactory(
            Arc::new(ArenaTaskFactory::new(arena.clone())),
            trace.clone(),
        ));
        let compilation_factory = Arc::new(TraceFactory(compilation_factory, trace.clone()));
        for kind in [INSTANCE_KIND, INSTANCE_INPUT_KIND, DISCOVERY_KIND] {
            runtime.register(kind, instance_factory.clone()).unwrap();
        }
        for kind in [ARENA_CANDIDATES_KIND, ARENA_CHOSEN_KIND, ARENA_GRAPH_KIND] {
            runtime.register(kind, arena_factory.clone()).unwrap();
        }
        for kind in [
            COMPILATION_KIND,
            READY_KIND,
            ROOT_KIND,
            native::object_queries::CODEGEN_KIND,
            native::object_queries::LINK_KIND,
            LINK_INPUT_KIND,
            LINK_MANIFEST_KIND,
        ] {
            runtime.register(kind, compilation_factory.clone()).unwrap();
        }
        let events = Arc::new(Events::default());
        runtime.observe(events.clone()).unwrap();
        let request = runtime.request(&[compilation.query_id()]);
        let selected = runtime.wait(request).unwrap_or_else(|error| {
            panic!(
                "actual cold native compile failed: {}",
                String::from_utf8_lossy(&store.get_content(error).unwrap())
            )
        })[0];
        let results = compilation.outputs(selected).unwrap().to_vec();
        assert_eq!(results.len(), 2);
        let native = compilation.native(selected).unwrap();
        let roots = compilation.roots(selected).unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), if fresh { 0 } else { 2 });
        assert_eq!(roots.publications.load(Ordering::Relaxed), 1);
        let request = runtime.request(&[instances.discovery_id()]);
        let discovery = runtime.wait(request).unwrap()[0];
        let completed = instances.completed_instances(discovery).unwrap();
        assert_eq!(
            instances.imports.load(Ordering::Relaxed),
            if fresh {
                0
            } else if iteration == 0 {
                completed.len() as u64
            } else {
                1
            }
        );
        assert_eq!(
            native.ready_transformations.load(Ordering::Relaxed),
            if fresh {
                0
            } else if iteration == 0 {
                completed.len() as u64
            } else {
                1
            }
        );
        assert_eq!(
            native.snapshot.ready_materializations(),
            if fresh {
                0
            } else if iteration == 0 {
                completed.len() as u64 * 2
            } else {
                2
            }
        );
        if fresh {
            assert_eq!(arena.solves.load(Ordering::Relaxed), 0);
            assert_eq!(trace.totals(), Default::default());
        }
        let events = events.0.lock().unwrap();
        for (from, to) in [
            (COMPILATION_KIND, DISCOVERY_KIND),
            (COMPILATION_KIND, ARENA_GRAPH_KIND),
            (DISCOVERY_KIND, INSTANCE_KIND),
            (INSTANCE_KIND, INSTANCE_INPUT_KIND),
            (ARENA_CANDIDATES_KIND, INSTANCE_KIND),
            (ARENA_CHOSEN_KIND, ARENA_CANDIDATES_KIND),
            (READY_KIND, ARENA_CHOSEN_KIND),
            (native::object_queries::CODEGEN_KIND, READY_KIND),
            (
                native::object_queries::LINK_KIND,
                native::object_queries::CODEGEN_KIND,
            ),
            (LINK_MANIFEST_KIND, ROOT_KIND),
            (native::object_queries::LINK_KIND, READY_KIND),
            (COMPILATION_KIND, native::object_queries::LINK_KIND),
        ] {
            assert!(events.iter().any(|event| matches!(event, RuntimeEvent::Read {query, dependency} if query.kind == from && dependency.kind == to)), "missing actual edge {from:x}->{to:x}");
        }
        drop(events);
        let mut metadata = vec![roots.query_id()];
        metadata.extend(completed.iter().map(|(key, _)| key.ready_query_id()));
        let request = runtime.request(&metadata);
        let ready = runtime.wait(request).unwrap();
        let (mut view, root) = roots.products(ready[0], &ready[1..]).unwrap();
        let ready = ready[1..]
            .iter()
            .map(|read| {
                native::queries::ready_product(store.as_ref(), *read)
                    .map(Arc::new)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let current = (root.static_manifest, root.launch_manifest);
        if let Some(old) = previous {
            if fresh {
                assert_eq!(current, old);
            } else {
                assert_ne!(current.0, old.0);
                assert_ne!(current.1, old.1);
            }
        }
        previous = Some(current);
        let before = trace.totals();
        let bodies = native.snapshot.ready_materializations();
        for (index, backend) in ["llvm", "cranelift"].into_iter().enumerate() {
            let linked =
                native::object_queries::linked_product(store.as_ref(), results[index]).unwrap();
            let directory = directory
                .path()
                .join(format!("restored-{literal}-{fresh}-{backend}"));
            std::fs::create_dir_all(&directory).unwrap();
            let executable = directory.join("program");
            native::object_queries::materialize_executable(store.as_ref(), &linked, &executable)
                .unwrap();
            let run = native::launch(&executable, &target, &tools, &directory, true).unwrap();
            view.with_link_context(&root, &ready, |program| {
                assert!(program.units.is_empty());
                let actual =
                    native::observe(&run, program, &source.sites()).map_err(|e| e.to_string())?;
                assert_eq!(Some(&actual), reference.as_ref());
                Ok(())
            })
            .unwrap();
            assert!(String::from_utf8_lossy(&run.stderr).contains(&literal.to_string()));
        }
        assert_eq!(trace.totals(), before);
        assert_eq!(native.snapshot.ready_materializations(), bodies);
        drop(runtime);
        drop(compilation_factory);
        drop(arena_factory);
        drop(instance_factory);
        assert_eq!(budget.reserved(), 96 << 20);
        drop(view);
        drop(roots);
        drop(native);
        drop(compilation);
        drop(arena);
        drop(instances);
        assert_eq!(budget.reserved(), 0);
    }
}
