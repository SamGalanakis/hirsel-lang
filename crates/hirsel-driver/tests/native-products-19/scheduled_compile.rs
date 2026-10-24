use super::scheduled_link::Platform;
use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::ResultStore, *};
use hirsel_syntax::ContentHash;
use native::queries::{
    ARENA_CANDIDATES_KIND, ARENA_CHOSEN_KIND, INSTANCE_KIND, NativeRootKey, NativeRoots,
    READY_KIND, ROOT_KIND,
    arena_tasks::{ARENA_GRAPH_KIND, ArenaTasks},
    compilation_tasks::{
        COMPILATION_KIND, CompilationBackend, CompilationFactory, CompilationTasks,
    },
    declarations::NativeDeclarations,
    instance_tasks::{DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTasks},
    link_tasks::LINK_MANIFEST_KIND,
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
            run(jobs, false, false, false, false);
        }
    });
}
#[test]
fn frozen_platform_links_standalone_after_original_tool_kit_runtime_and_foreign_paths_disappear() {
    with_compiler_stack(|| {
        for jobs in [1, 4] {
            run(jobs, true, false, false, false);
        }
    });
}
#[test]
fn optimized_codegen_reads_transitive_recursive_templates_without_recursive_ready_dependencies() {
    with_compiler_stack(|| {
        for jobs in [1, 4] {
            run(jobs, false, true, false, false);
        }
    });
}
#[test]
fn frozen_platform_restores_root_bound_runner_images_after_link_hits() {
    with_compiler_stack(|| {
        for jobs in [1, 4] {
            run(jobs, true, false, true, false);
        }
    });
}
#[test]
fn frozen_test_dispatchers_keep_ordered_seeds_and_reopen_body_free_link_hits() {
    with_compiler_stack(|| {
        for jobs in [1, 4] {
            run(jobs, true, false, false, true);
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
fn run(
    jobs: usize,
    freeze_platform: bool,
    optimized: bool,
    requested_run: bool,
    test_dispatch: bool,
) {
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
        std::fs::copy(&tools.builtins[&target.triple], source.join("builtins.a")).unwrap();
        let foreign = source.join("foreign.a");
        let foreign_member = source.join("foreign-member.o");
        let member = hirsel_llvm::compile_ir(
            &format!(
                "target triple = \"{}\"\ndefine i64 @unused_foreign() {{ ret i64 7 }}",
                target.triple
            ),
            &target,
            native::OptLevel::None,
        )
        .unwrap();
        std::fs::write(&foreign_member, member.bytes).unwrap();
        let original_tree = tools.lld.parent().unwrap().parent().unwrap();
        let status = std::process::Command::new(original_tree.join("bin/llvm-ar"))
            .env_clear()
            .env("LD_LIBRARY_PATH", original_tree.join("lib"))
            .args(["qcTD"])
            .arg(&foreign)
            .arg(&foreign_member)
            .status()
            .unwrap();
        assert!(status.success());
        // Thin expansion uses the same admitted archiver and its dynamic libs.
        std::fs::copy(
            original_tree.join("bin/llvm-ar"),
            source.join("llvm/bin/llvm-ar"),
        )
        .unwrap();
        let mut selected = tools.clone();
        selected.lld = source.join("llvm/bin/ld.lld");
        selected.kit = source.join("kit");
        selected.runtime.insert(
            (target.triple.clone(), selected.runtime_build),
            source.join("runtime.a"),
        );
        selected
            .builtins
            .insert(target.triple.clone(), source.join("builtins.a"));
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
        assert!(!selected.builtins[&target.triple].exists());
        assert_eq!(frozen.runtime_archives().len(), 2);
        Some(frozen)
    } else {
        None
    };
    let policy = ContentHash::of_bytes(b"one runtime cold native Link control");
    let mut previous = None;
    let mut previous_actions = std::collections::BTreeMap::new();
    let mut previous_ready = std::collections::BTreeMap::new();
    let mut reference = None;
    let mut second_reference = None;
    let mut stale_launch = None;
    let mut old_link_addresses = std::collections::BTreeMap::new();
    for (iteration, (literal, fresh)) in [(7u64, false), (7, true), (8, false), (8, true)]
        .into_iter()
        .enumerate()
    {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let text = if test_dispatch {
            format!(
                "pub fn main(t:Test)->Result[(),()] {{test.check(t,test.seed(t)=={literal}u64);Ok(())}}\npub fn second(t:Test)->Result[(),()] {{test.check(t,test.seed(t)=={}u64);Ok(())}}\n",
                literal + 100
            )
        } else if optimized {
            format!(
                "fn leaf(n:u64)->u64 {{if n==0u64 {{{literal}u64}} else {{middle(n-1u64)}}}}\nfn middle(n:u64)->u64 {{leaf(n)}}\npub fn main()->Result[(),u64] {{Error(middle(0u64))}}\n"
            )
        } else if freeze_platform {
            format!(
                "const SECOND:u64=comptime {literal}u64\nfn worker(n:u64)->Result[(),u64] uses {{}} {{let _=n;Error(SECOND)}}\nfn dispatch(f:fn(u64)->Result[(),u64] uses {{}})->Result[(),u64] uses {{f}} {{f(0u64)}}\npub fn main()->Result[(),u64] {{dispatch(worker)}}\n"
            )
        } else {
            format!(
                "const SECOND:u64=comptime {literal}u64\npub fn main()->Result[(),u64] {{ Error(SECOND) }}\n"
            )
        };
        let file = engine.sources.add("cold-scheduled-link.hir", text).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let main = entry_path(&default_context());
        let mut second = main.clone();
        second.0.name = "second".into();
        let test_entries = vec![(main.clone(), literal), (second.clone(), literal + 100)];
        if !fresh {
            let execution = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: main.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig {
                    test_seed: if test_dispatch { literal } else { 0 },
                    ..Default::default()
                },
            )
            .unwrap();
            reference = Some(native::Observation::of_reference(
                execution.observation.as_ref().unwrap(),
            ));
            if test_dispatch {
                let execution = run_observed_snapshot(
                    &mut engine,
                    &RunConfiguration {
                        entry: second.clone(),
                        hash_seed: 0,
                        inputs: vec![],
                    },
                    SimulationConfig {
                        test_seed: literal + 100,
                        ..Default::default()
                    },
                )
                .unwrap();
                second_reference = Some(native::Observation::of_reference(
                    execution.observation.as_ref().unwrap(),
                ));
            }
        } else {
            engine.adapters.lowering = None;
        }
        let mut source_queries = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let declarations =
            NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
        if fresh {
            assert_eq!(source_queries.counts.body_demands, 0);
        }
        let run_record = requested_run.then(|| {
            let revision = engine.semantic_program_revision(&main).unwrap();
            Arc::new(
                hirsel_run_config::RunConfig::for_program(
                    &revision,
                    hirsel_run_schema::LaunchKind::Main,
                )
                .validated()
                .unwrap(),
            )
        });
        let source = Arc::new(if test_dispatch {
            NativeLinkSource::capture_for_tests(&mut engine, &declarations, &test_entries).unwrap()
        } else if let Some(run) = &run_record {
            NativeLinkSource::capture_for_run(&mut engine, &declarations, &main, run.clone())
                .unwrap()
        } else {
            NativeLinkSource::capture_for_entry(&mut engine, &declarations, &main).unwrap()
        });
        let mut originals = Vec::new();
        if !fresh {
            let body = engine.lower_to_tir(&main).unwrap().value;
            originals.push(Arc::downgrade(&body));
        }
        let emission = EmissionOptions {
            input: declarations.program.input.clone(),
            opt: if optimized {
                native::OptLevel::Default
            } else {
                native::OptLevel::None
            },
            debug: !optimized,
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
                profile_options: ContentHash::of_bytes(if optimized {
                    b"default opt"
                } else {
                    b"debug opt none"
                }),
                runtime_abi: ContentHash::of_bytes(&hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes()),
                linker_options: if let Some(platform) = &frozen {
                    if test_dispatch {
                        platform.linker_options_for_tests(backend, &main).unwrap()
                    } else if requested_run {
                        platform.linker_options_for_run(backend, &main).unwrap()
                    } else {
                        platform
                            .linker_options_with_debug(backend, &main, true)
                            .unwrap()
                    }
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
                        inner: if test_dispatch {
                            platform.executor_for_tests(
                                backend,
                                main.clone(),
                                directory
                                    .path()
                                    .join(format!("{literal}-{fresh}-{}", backend.name())),
                            )
                        } else if requested_run {
                            platform.executor_for_run(
                                backend,
                                main.clone(),
                                directory
                                    .path()
                                    .join(format!("{literal}-{fresh}-{}", backend.name())),
                            )
                        } else {
                            platform.executor_debug(
                                backend,
                                main.clone(),
                                directory
                                    .path()
                                    .join(format!("{literal}-{fresh}-{}", backend.name())),
                                true,
                            )
                        },
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
                roots: if test_dispatch {
                    vec![main, second]
                } else {
                    vec![main]
                },
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
        for (kind, factory) in compilation.factories() {
            runtime
                .register(kind, Arc::new(TraceFactory(factory, trace.clone())))
                .unwrap();
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
                if test_dispatch { 2 } else { 1 }
            }
        );
        assert_eq!(
            native.ready_transformations.load(Ordering::Relaxed),
            if fresh {
                0
            } else if iteration == 0 {
                completed.len() as u64
            } else {
                if test_dispatch { 2 } else { 1 }
            }
        );
        assert_eq!(
            native.snapshot.ready_materializations(),
            if fresh {
                0
            } else if optimized {
                // Every one of the 13 roots emits once per backend. The
                // recursive leaf/middle each copy one body; main copies two.
                // On edit only leaf Ready changes: LLVM reads 2+2+3, CRAN 1.
                if iteration == 0 {
                    completed.len() as u64 * 2 + 4
                } else {
                    8
                }
            } else if iteration == 0 {
                completed.len() as u64 * 2
            } else {
                if test_dispatch { 4 } else { 2 }
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
        if optimized {
            assert_eq!(completed.len(), 13);
            assert_eq!(
                completed
                    .iter()
                    .filter(|(_, output)| output.templates.library)
                    .count(),
                10
            );
            assert_eq!(
                completed
                    .iter()
                    .filter(|(_, output)| !output.templates.library)
                    .map(|(owner, _)| owner.instance.definition().0.name.as_str())
                    .collect::<std::collections::BTreeSet<_>>(),
                std::collections::BTreeSet::from(["leaf", "main", "middle"])
            );
            assert!(!events.iter().any(|event| matches!(event,
                RuntimeEvent::Read { query, dependency } if query.kind == READY_KIND && dependency.kind == READY_KIND)),
                "template copying must not create recursive Ready dependencies");
            for (owner, output) in &completed {
                assert!(output.templates.small);
                let own_ready = store
                    .lookup_product(owner.ready_query_id().address)
                    .unwrap();
                let name = owner.instance.definition().0.name.to_string();
                if let Some(old) = previous_ready.insert(name.clone(), own_ready.reference) {
                    if fresh || name != "leaf" {
                        assert_eq!(
                            own_ready.reference, old,
                            "unchanged caller Ready remains equal"
                        );
                    } else {
                        assert_ne!(own_ready.reference, old);
                    }
                }
                for backend in ["llvm", "cranelift"] {
                    let key = native::object_queries::CodegenKey {
                        owner: owner.clone(),
                        backend: backend.into(),
                        backend_version: ContentHash::of_parts(
                            "backend",
                            [backend.as_bytes(), COMPILER_IDENTITY.as_slice()],
                        ),
                        target_features: target.digest(),
                        profile_options: ContentHash::of_bytes(b"default opt"),
                        runtime_abi: ContentHash::of_bytes(
                            &hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes(),
                        ),
                    };
                    let reads = events
                        .iter()
                        .filter_map(|event| match event {
                            RuntimeEvent::Read { query, dependency }
                                if *query == key.query_id() && dependency.kind == READY_KIND =>
                            {
                                Some(dependency.address.0)
                            }
                            _ => None,
                        })
                        .collect::<std::collections::BTreeSet<_>>();
                    // Library roots have no references; source recursion
                    // copies only the named source bodies, never those roots.
                    let expected = completed
                        .iter()
                        .filter(|(copy, copy_output)| {
                            if backend == "cranelift" || output.templates.library {
                                copy == owner
                            } else {
                                !copy_output.templates.library
                                    && (name == "main"
                                        || copy.instance.definition().0.name != "main")
                            }
                        })
                        .map(|(copy, _)| copy.ready_query_id().address.0)
                        .collect::<std::collections::BTreeSet<_>>();
                    assert_eq!(
                        reads, expected,
                        "{backend} {name} explicit template inventory"
                    );
                    let current = store.lookup_product(key.address()).unwrap().action;
                    if let Some(old) = previous_actions.insert((name.clone(), backend), current) {
                        if fresh
                            || output.templates.library
                            || (backend == "cranelift" && name != "leaf")
                        {
                            assert_eq!(current, old);
                        } else {
                            assert_ne!(
                                current, old,
                                "copied changed leaf invalidates LLVM caller Codegen"
                            );
                        }
                    }
                }
            }
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
                if optimized || test_dispatch {
                    assert_eq!(current.0, old.0);
                } else {
                    assert_ne!(current.0, old.0);
                }
                assert_ne!(current.1, old.1);
            }
        }
        previous = Some(current);
        if freeze_platform {
            // Current root owns the sole projection. Crossing its descriptor
            // with an old launch/source map is rejected even when a valid
            // executable exists and the real current Link is a fresh hit.
            let original = *stale_launch.get_or_insert(root.launch_manifest);
            let profiles = view.profile_sources(&root).unwrap();
            assert!(
                profiles
                    .values()
                    .any(|source| source.file.text().contains(&literal.to_string()))
            );
            if original != root.launch_manifest {
                let mut wrong = root.as_ref().clone();
                wrong.launch_manifest = original;
                let before = trace.totals();
                let bodies = native.snapshot.ready_materializations();
                assert!(view.profile_sources(&Arc::new(wrong)).is_err());
                assert_eq!(trace.totals(), before);
                assert_eq!(native.snapshot.ready_materializations(), bodies);
                if fresh {
                    assert_eq!(calls.load(Ordering::Relaxed), 0);
                }
            }
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let key = native::object_queries::LinkKey {
                    roots: root.instances.clone(),
                    target: target.digest(),
                    linker_options: if test_dispatch {
                        frozen
                            .as_ref()
                            .unwrap()
                            .linker_options_for_tests(
                                backend,
                                &hirsel_engine::entry_path(&default_context()),
                            )
                            .unwrap()
                    } else if requested_run {
                        frozen
                            .as_ref()
                            .unwrap()
                            .linker_options_for_run(
                                backend,
                                &hirsel_engine::entry_path(&default_context()),
                            )
                            .unwrap()
                    } else {
                        frozen
                            .as_ref()
                            .unwrap()
                            .linker_options_with_debug(
                                backend,
                                &hirsel_engine::entry_path(&default_context()),
                                true,
                            )
                            .unwrap()
                    },
                };
                if let Some(old) = old_link_addresses.insert(backend.name(), key.address()) {
                    assert_eq!(
                        key.address(),
                        old,
                        "source edits change action inputs, never logical Link address"
                    );
                }
            }
        }
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
            if freeze_platform && !requested_run && !test_dispatch {
                use object::Object as _;
                let bytes = std::fs::read(&executable).unwrap();
                let elf = object::File::parse(bytes.as_slice()).unwrap();
                assert!(
                    elf.section_by_name(".debug_line").is_some(),
                    "{backend} link must retain source line tables"
                );
                assert!(
                    bytes
                        .windows(b"cold-scheduled-link.hir".len())
                        .any(|part| part == b"cold-scheduled-link.hir")
                );
            }
            for case in 0..if test_dispatch { 2 } else { 1 } {
                let run = if test_dispatch {
                    let platform = frozen.as_ref().unwrap();
                    let runnable = platform.runnable(&linked, &executable).unwrap();
                    assert!(matches!(runnable, native::Runnable::Image(_)));
                    platform
                        .launch_test(&runnable, &directory, case, None)
                        .unwrap()
                        .0
                } else if requested_run {
                    let platform = frozen.as_ref().unwrap();
                    let runnable = platform.runnable(&linked, &executable).unwrap();
                    assert!(matches!(runnable, native::Runnable::Image(_)));
                    std::fs::write(
                        directory.join("run.json"),
                        run_record.as_ref().unwrap().launch_bytes(),
                    )
                    .unwrap();
                    native::launch_runnable(
                        &runnable,
                        &target,
                        platform.toolchain(),
                        &directory,
                        true,
                        native::Tape::Live {
                            record: None,
                            revocations: None,
                        },
                    )
                    .unwrap()
                } else {
                    native::launch(&executable, &target, &tools, &directory, true).unwrap()
                };
                view.with_link_context(&root, &ready, |program| {
                    assert!(program.units.is_empty());
                    if test_dispatch {
                        assert!(program.callee_metadata().is_none());
                    } else {
                        let callees = program
                            .callee_metadata()
                            .expect("actual checked runner metadata");
                        assert!(callees.request_schemas.is_some());
                        if freeze_platform {
                            assert!(
                                callees.facts.keys().any(|path| path.ends_with("/worker")),
                                "referenced entry facts survive body-free link assembly"
                            );
                        }
                    }
                    let actual = native::observe(&run, program, &source.sites())
                        .map_err(|e| e.to_string())?;
                    assert_eq!(
                        Some(&actual),
                        if case == 0 {
                            reference.as_ref()
                        } else {
                            second_reference.as_ref()
                        }
                    );
                    Ok(())
                })
                .unwrap();
                if !test_dispatch {
                    assert!(String::from_utf8_lossy(&run.stderr).contains(&literal.to_string()));
                }
            }
        }
        assert_eq!(trace.totals(), before);
        assert_eq!(native.snapshot.ready_materializations(), bodies);
        drop(runtime);
        drop(compilation_factory);
        assert_eq!(budget.reserved(), 96 << 20);
        let retained_snapshot = native.snapshot.clone();
        drop(compilation);
        drop(arena);
        drop(instances);
        assert_eq!(budget.reserved(), 96 << 20);
        assert!(budget.reserve((672 << 20) + 1).is_err());
        // A view and a directly returned snapshot independently extend the
        // reservation lifetime, even after every producer/controller is gone.
        drop(roots);
        drop(native);
        assert_eq!(budget.reserved(), 96 << 20);
        view.with_link_context(&root, &ready, |program| {
            assert!(program.units.is_empty());
            Ok(())
        })
        .unwrap();
        drop(view);
        assert_eq!(budget.reserved(), 96 << 20);
        drop(retained_snapshot);
        assert_eq!(budget.reserved(), 0);
    }
}
