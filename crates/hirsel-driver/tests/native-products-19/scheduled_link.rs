use hirsel_driver::*;
use hirsel_engine::runtime::*;
use hirsel_engine::*;
use hirsel_syntax::ContentHash;
use native::object_queries::{CodegenKey, LinkKey, ObjectTaskFactory, ObjectTaskSnapshot};
use native::queries::{
    NativeRootKey, NativeRoots,
    arena::{ArenaQueryCounts, EquationProduct},
    declarations::NativeDeclarations,
    demand::DemandInstances,
    graph::{ArenaNode, settle_arena_queries},
    link_tasks::{
        LINK_INPUT_KIND, LINK_MANIFEST_KIND, LinkExecution, LinkInputFactory, LinkTasks,
        NativeLinkExecutor, NativeLinkPlan,
    },
    products::{EmissionOptions, NativeSnapshot},
    root_source::NativeLinkSource,
    root_tasks::{NativeRootFactory, NativeRootTasks},
    roots::{RootLinkInputs, RootProducts},
    tasks::{CodegenTasks, NativeTaskFactory, NativeTasks},
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub(super) struct Platform {
    pub(super) store: Arc<store::DirStore>,
    pub(super) tools: native::Toolchain,
    pub(super) target: native::TargetSpec,
    pub(super) backend: native::Backend,
    pub(super) entry: hirsel_syntax::DefPath,
    pub(super) directory: PathBuf,
    pub(super) calls: Arc<AtomicU64>,
}
impl NativeLinkExecutor for Platform {
    fn link(
        &self,
        products: &mut RootProducts,
        input: LinkExecution<'_>,
    ) -> Result<Vec<u8>, String> {
        if input.inputs.runtime_archives.len() != 1 || !input.inputs.foreign_inputs.is_empty() {
            return Err("unexpected selected fixture platform inputs".into());
        }
        std::fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let bytes = native::object_queries::link_input_bytes(
            self.store.as_ref(),
            input.inputs.runtime_archives[0],
        )?;
        let archive = self.directory.join("selected-runtime.a");
        std::fs::write(&archive, bytes).map_err(|e| e.to_string())?;
        let mut tools = self.tools.clone();
        tools
            .runtime
            .insert((self.target.triple.clone(), tools.runtime_build), archive);
        self.calls.fetch_add(1, Ordering::Relaxed);
        products.link(
            input.root,
            input.ready,
            RootLinkInputs {
                entry: &self.entry,
                backend: self.backend,
                objects: input.objects,
                toolchain: &tools,
                directory: &self.directory,
            },
        )
    }
}
#[derive(Default)]
struct Events(std::sync::Mutex<Vec<RuntimeEvent>>);
impl RuntimeObserver for Events {
    fn record(&self, event: RuntimeEvent) {
        self.0.lock().unwrap().push(event);
    }
}
struct Pending;
impl QueryContext for Pending {
    fn read(&mut self, _: QueryId) -> std::task::Poll<Result<ProductRef, store::Reference>> {
        std::task::Poll::Pending
    }
}
#[test]
fn native_link_tasks_settle_real_roots_ready_codegen_and_snapshot_inputs_at_widths_one_and_four() {
    with_compiler_stack(|| {
        for jobs in [1, 4] {
            run(jobs);
        }
    });
}
fn run(jobs: usize) {
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
    // Platform identity includes actual selected tool/kit bytes. Archive bytes
    // are independent explicit leaves, materialized by Platform above.
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
    let policy = ContentHash::of_bytes(b"real source native Link continuations");
    let options = native::Options::new(target.clone());
    let mut previous = None;
    let mut reference = None;
    for (literal, fresh) in [(7u64, false), (7, true), (8, false), (8, true)] {
        let cold = previous.is_none();
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine.sources.add("scheduled-link.hir", format!("const SECOND:u64=comptime {literal}u64\npub fn main()->Result[(),u64] {{ Error(SECOND) }}\n")).unwrap();
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
        let mut declarations =
            NativeDeclarations::collect(&mut engine, &mut source_queries, &[]).unwrap();
        let source = Arc::new(NativeLinkSource::capture(&mut engine, &declarations).unwrap());
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _observe = trace.enter();
        let mut instances = DemandInstances::new(&engine, store.clone(), target.clone(), policy);
        let mut pending = instances.roots(&engine, &declarations).unwrap();
        let mut seen = BTreeSet::new();
        let mut nodes = Vec::new();
        while let Some(key) = pending.pop() {
            if !seen.insert(key.instance.encoding().to_vec()) {
                continue;
            }
            let result = instances
                .instance(&mut engine, &mut declarations, &key, &options)
                .unwrap();
            pending.extend(result.dependencies.iter().cloned());
            nodes.push(ArenaNode {
                equations: EquationProduct {
                    owner: key,
                    reference: result.product.local_equations,
                },
                callees: result.callees.clone(),
            });
        }
        nodes.sort_by(|a, b| a.equations.owner.canonical_cmp(&b.equations.owner));
        let arena = settle_arena_queries(
            store.as_ref(),
            &nodes,
            &[],
            0,
            &mut ArenaQueryCounts::default(),
        )
        .unwrap();
        let operations = declarations.program.operation_registry();
        let manifest = declarations.manifest().unwrap();
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
        if fresh {
            assert_eq!((instances.imports, instances.recipe_decodes), (0, 0));
        }
        let snapshot = NativeSnapshot::new(
            store.clone(),
            instances,
            arena,
            options.clone(),
            emission,
            manifest,
            Arc::new(operations),
        )
        .unwrap();
        drop(declarations);
        drop(engine);
        let weight = MemoryWeight {
            scratch: 32 << 20,
            retained: 64 << 10,
            stack: 0,
        };
        let native = NativeTasks::new(snapshot.clone(), weight).unwrap();
        let root_key = NativeRootKey {
            roots: vec![main.clone()],
            target_abi: target.digest(),
            policy,
        };
        let roots = NativeRootTasks::new(native.clone(), source.clone(), root_key, weight).unwrap();
        let codegen = ["llvm", "cranelift"].map(|backend| {
            nodes
                .iter()
                .map(|node| CodegenKey {
                    owner: node.equations.owner.clone(),
                    backend: backend.into(),
                    backend_version: ContentHash::of_parts(
                        "backend",
                        [backend.as_bytes(), COMPILER_IDENTITY.as_slice()],
                    ),
                    target_features: target.digest(),
                    profile_options: ContentHash::of_bytes(b"debug opt none"),
                    runtime_abi: ContentHash::of_bytes(
                        &hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes(),
                    ),
                })
                .collect::<Vec<_>>()
        });
        let objects = CodegenTasks::new(
            native.clone(),
            codegen.iter().flatten().cloned().collect(),
            weight,
        )
        .unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let mut plans = Vec::new();
        let mut link_keys = Vec::new();
        for (index, backend) in [native::Backend::Llvm, native::Backend::Cranelift]
            .into_iter()
            .enumerate()
        {
            let key = LinkKey {
                roots: nodes.iter().map(|n| n.equations.owner.clone()).collect(),
                target: target.digest(),
                linker_options: ContentHash::of_parts(
                    "fixture selected backend",
                    [platform.0.as_slice(), backend.name().as_bytes()],
                ),
            };
            plans.push(NativeLinkPlan {
                key: key.clone(),
                fragments: codegen[index].clone(),
                runtime_archives: vec![archive],
                foreign_inputs: vec![],
                executor: Arc::new(Platform {
                    store: store.clone(),
                    tools: tools.clone(),
                    target: target.clone(),
                    backend,
                    entry: main.clone(),
                    directory: directory
                        .path()
                        .join(format!("{literal}-{fresh}-{}", backend.name())),
                    calls: calls.clone(),
                }),
                weight,
            });
            link_keys.push(key);
        }
        let links = LinkTasks::new(roots.clone(), plans, weight).unwrap();
        let link_factory = Arc::new(ObjectTaskFactory::new(links.clone()));
        let before = trace.totals();
        let mut pending = link_factory.create(link_keys[0].query_id()).unwrap();
        assert!(
            matches!(pending.poll(&mut Pending), QueryPoll::Waiting(ref ids) if ids.len()==1 && ids[0].kind==LINK_MANIFEST_KIND)
        );
        let link_request = links.request(link_keys[0].query_id()).unwrap();
        let native::object_queries::ObjectTaskRequest::Link { manifest, .. } = link_request else {
            panic!("expected Link request")
        };
        let input_factory = Arc::new(LinkInputFactory::new(links.clone()));
        let mut lazy_manifest = input_factory.create(manifest).unwrap();
        assert!(
            matches!(lazy_manifest.poll(&mut Pending), QueryPoll::Waiting(ref ids) if ids==&vec![roots.query_id()])
        );
        assert_eq!(trace.totals(), before);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(snapshot.ready_materializations(), 0);
        assert_eq!(native.ready_transformations.load(Ordering::Relaxed), 0);
        drop(pending);
        drop(lazy_manifest);
        let errors = RuntimeErrors {
            resource: native.failure("resource"),
            cycle: native.failure("cycle"),
            cancelled: native.failure("cancelled"),
            protocol: native.failure("protocol"),
            panicked: native.failure("panic"),
        };
        let budget = ProcessBudget::new(768 << 20);
        let mut runtime = AdmittedRuntime::new(
            hirsel_intern::jobs::Jobs::new(jobs),
            budget.reserve(768 << 20).unwrap(),
            errors,
        )
        .unwrap();
        let events = Arc::new(Events::default());
        runtime.observe(events.clone()).unwrap();
        let native_factory = Arc::new(NativeTaskFactory::new(native.clone()));
        for kind in [
            native::queries::INSTANCE_KIND,
            native::queries::ARENA_CANDIDATES_KIND,
            native::queries::ARENA_CHOSEN_KIND,
            native::queries::READY_KIND,
        ] {
            runtime.register(kind, native_factory.clone()).unwrap();
        }
        runtime
            .register(
                native::queries::ROOT_KIND,
                Arc::new(NativeRootFactory::new(roots.clone())),
            )
            .unwrap();
        runtime
            .register(
                native::object_queries::CODEGEN_KIND,
                Arc::new(ObjectTaskFactory::new(objects)),
            )
            .unwrap();
        runtime
            .register(native::object_queries::LINK_KIND, link_factory)
            .unwrap();
        runtime
            .register(LINK_MANIFEST_KIND, input_factory.clone())
            .unwrap();
        runtime.register(LINK_INPUT_KIND, input_factory).unwrap();
        let request = runtime.request(&link_keys.iter().map(LinkKey::query_id).collect::<Vec<_>>());
        let result = runtime.wait(request).unwrap_or_else(|error| {
            use hirsel_engine::store::ResultStore;
            panic!(
                "actual Link task failed: {:?}",
                store
                    .get_content(error)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
            )
        });
        assert_eq!(calls.load(Ordering::Relaxed), if fresh { 0 } else { 2 });
        assert_eq!(roots.publications.load(Ordering::Relaxed), 1);
        assert_eq!(
            native.ready_transformations.load(Ordering::Relaxed),
            if fresh {
                0
            } else if cold {
                nodes.len() as u64
            } else {
                1
            }
        );
        assert_eq!(
            snapshot.ready_materializations(),
            if fresh {
                0
            } else if cold {
                2 * nodes.len() as u64
            } else {
                2
            }
        );
        let events = events.0.lock().unwrap();
        for (from, to) in [
            (native::object_queries::LINK_KIND, LINK_MANIFEST_KIND),
            (LINK_MANIFEST_KIND, native::queries::ROOT_KIND),
            (
                native::object_queries::LINK_KIND,
                native::object_queries::CODEGEN_KIND,
            ),
            (
                native::object_queries::LINK_KIND,
                native::queries::READY_KIND,
            ),
            (
                native::object_queries::CODEGEN_KIND,
                native::queries::READY_KIND,
            ),
            (
                native::queries::READY_KIND,
                native::queries::ARENA_CHOSEN_KIND,
            ),
        ] {
            assert!(events.iter().any(|e| matches!(e, RuntimeEvent::Read {query, dependency} if query.kind==from && dependency.kind==to)), "missing actual edge {from:x}->{to:x}");
        }
        drop(events);
        let mut metadata = vec![roots.query_id()];
        metadata.extend(
            nodes
                .iter()
                .map(|node| node.equations.owner.ready_query_id()),
        );
        let request = runtime.request(&metadata);
        let selected = runtime.wait(request).unwrap();
        let (mut view, root) = roots.products(selected[0], &selected[1..]).unwrap();
        let ready = selected[1..]
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
        drop(runtime);
        assert_eq!(budget.reserved(), 0);
        let before_observe = trace.totals();
        let bodies = snapshot.ready_materializations();
        for (index, backend) in ["llvm", "cranelift"].into_iter().enumerate() {
            let linked =
                native::object_queries::linked_product(store.as_ref(), result[index]).unwrap();
            let dir = directory
                .path()
                .join(format!("restored-{literal}-{fresh}-{backend}"));
            std::fs::create_dir_all(&dir).unwrap();
            let executable = dir.join("program");
            native::object_queries::materialize_executable(store.as_ref(), &linked, &executable)
                .unwrap();
            let run = native::launch(&executable, &target, &tools, &dir, true).unwrap();
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
        assert_eq!(trace.totals(), before_observe);
        assert_eq!(snapshot.ready_materializations(), bodies);
        if fresh {
            assert_eq!(trace.totals(), Default::default());
        }
    }
}
