use hirsel_driver::*;
use hirsel_engine::{runtime::*, store::ResultStore, *};
use hirsel_syntax::ContentHash;
use native::{
    object_queries::{linked_product, materialize_executable},
    queries::{
        NativeRootKey, NativeRoots,
        arena_tasks::ArenaTasks,
        compilation_tasks::{CompilationBackend, CompilationFactory, CompilationTasks},
        declarations::NativeDeclarations,
        instance_tasks::InstanceTasks,
        link_tasks::{LinkExecution, NativeLinkExecutor},
        products::EmissionOptions,
        roots::{RootExecutableInputs, RootLinkInputs, RootProducts},
        source_inputs::NativeInstanceInputs,
    },
    root_source::{NativeLinkFacts, NativeLinkSource},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::Poll,
};

// Preserve the real directory provider and its validator capabilities while
// reporting the I/O error that its ordinary Option publication seam omits.
struct DiagnosedStore(store::DirStore);
impl ResultStore for DiagnosedStore {
    fn local_product_store(&self) -> Option<hirsel_artifact_store::Store> {
        self.0.local_product_store()
    }
    fn verifier(&self) -> Option<Arc<hirsel_validate::VerifierCache>> {
        self.0.verifier()
    }
    fn enabled(&self) -> bool {
        self.0.enabled()
    }
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn get(&self, key: &store::EntryKey) -> Option<store::Bytes> {
        self.0.get(key)
    }
    fn put(&self, key: &store::EntryKey, bytes: &[u8]) {
        self.0.put(key, bytes);
    }
    fn get_content(&self, reference: store::Reference) -> Option<store::Bytes> {
        self.0.get_content(reference)
    }
    fn put_content(&self, domain: store::Domain, bytes: &[u8]) -> Option<store::Reference> {
        // Exactly DirStore's one physical put: no retry or fallback provider.
        match self.0.local_product_store()?.put(domain, bytes) {
            Ok(reference) => Some(reference),
            Err(error) => {
                eprintln!(
                    "directory publication refused: domain={domain:?} bytes={} error={error:?}",
                    bytes.len()
                );
                None
            }
        }
    }
    fn lookup_product(&self, address: store::Address) -> Option<store::Association> {
        self.0.lookup_product(address)
    }
    fn associate_product(
        &self,
        address: store::Address,
        action: store::Action,
        reference: store::Reference,
    ) {
        self.0.associate_product(address, action, reference);
    }
    fn put_query(
        &self,
        kind: QueryKind,
        key: &store::EntryKey,
        action: store::Action,
        bytes: &[u8],
    ) {
        self.0.put_query(kind, key, action, bytes);
    }
}

// The platform is explicit fixture input. Production frozen-tool/foreign
// admission remains a separate owner; this tests native query composition.
struct Platform {
    store: Arc<dyn ResultStore>,
    tools: native::Toolchain,
    target: native::TargetSpec,
    backend: native::Backend,
    entry: hirsel_syntax::DefPath,
    directory: PathBuf,
    calls: Arc<AtomicU64>,
    frozen: Option<native::object_queries::capture::FrozenLinker>,
}
impl NativeLinkExecutor for Platform {
    fn link(
        &self,
        products: &mut RootProducts,
        input: LinkExecution<'_>,
    ) -> Result<Vec<u8>, String> {
        if input.inputs.runtime_archives.len() != 1 || !input.inputs.foreign_inputs.is_empty() {
            return Err("unexpected fixture platform inputs".into());
        }
        std::fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let archive = self.directory.join("selected-runtime.a");
        native::object_queries::materialize_link_input(
            self.store.as_ref(),
            input.inputs.runtime_archives[0],
            &archive,
            false,
        )?;
        if let Some(linker) = self.frozen {
            let plan = products.plan_executable(
                input.root,
                input.ready,
                RootExecutableInputs {
                    entry: &self.entry,
                    backend: self.backend,
                    objects: input.objects,
                    directory: &self.directory,
                    requested: None,
                    standalone: false,
                    platform: native::ExecutableLinkInputs {
                        kit: &self.tools.kit,
                        runtime: &archive,
                        foreign: &[],
                        loader: None,
                    },
                },
            )?;
            let captured = plan.capture(
                self.store.as_ref(),
                linker,
                native::object_queries::capture::CaptureLimits {
                    archive_bytes: 64 << 20,
                    files: 256,
                },
            )?;
            // Remove generated input files before replay. The captured tool and
            // closed VFS, rather than any live Toolchain executable, do the link.
            for file in std::fs::read_dir(&self.directory).map_err(|e| e.to_string())? {
                std::fs::remove_file(file.map_err(|e| e.to_string())?.path())
                    .map_err(|e| e.to_string())?;
            }
            let output = self.directory.join("program");
            captured.link(self.store.as_ref(), &output)?;
            self.calls.fetch_add(1, Ordering::Relaxed);
            return std::fs::read(output).map_err(|e| e.to_string());
        }
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
    fn continuation_bytes(&self) -> u64 {
        self.0.continuation_bytes()
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let _guard = self.1.enter();
        self.0.poll(cx)
    }
}

#[test]
fn immutable_native_compilation_links_both_backends_and_reopens_body_free_hits_after_edits() {
    with_compiler_stack(|| run(4));
}
#[test]
fn immutable_native_compilation_at_width_one() {
    with_compiler_stack(|| run(1));
}
#[test]
fn immutable_root_observation_uses_only_admitted_portable_summaries() {
    with_compiler_stack(|| run_case(1, true, false));
}
#[test]
#[ignore = "requires root-plan-proof frozen linker package"]
fn frozen_root_plans_run_both_backends_without_live_linker_inputs() {
    with_compiler_stack(|| run_case(1, false, true));
}
fn run(jobs: usize) {
    run_case(jobs, false, false);
}
fn run_case(jobs: usize, observation_only: bool, frozen_plan: bool) {
    let directory = tempfile::tempdir().unwrap();
    // The same admitted transport is selected before source or native work.
    // Its 24 MiB comes from dispatch's former 96 MiB; total stays 768 MiB.
    let budget = ProcessBudget::new(768 << 20);
    let backend = Arc::new(DiagnosedStore(store::DirStore::new(
        directory.path().join("products"),
    )));
    let store: Arc<dyn ResultStore> = Arc::new(store::RequestStore::new(
        backend,
        budget.reserve(24 << 20).unwrap(),
    ));
    let target = native::TargetSpec::x86_64_unknown_linux_gnu();
    let tools = native::Toolchain::in_tree_with_products(
        &directory.path().join("runtime"),
        &[&target],
        &[native::RuntimeBuild {
            diagnostic: true,
            ..Default::default()
        }],
        None,
        store.clone(),
    )
    .unwrap();
    let archive = native::object_queries::store_link_input(
        store.as_ref(),
        &std::fs::read(tools.archive(&target.triple).unwrap()).unwrap(),
    )
    .unwrap();
    let frozen = frozen_plan.then(|| {
        native::object_queries::capture::FrozenLinker::snapshot(
            store.as_ref(),
            &PathBuf::from(std::env::var_os("HIRSEL_CAPTURE_LINKER").unwrap()),
        )
        .unwrap()
    });
    let tool_identity = frozen
        .map(|linker| format!("{:?}", linker.reference()))
        .unwrap_or_default();
    let policy = ContentHash::of_bytes(b"immutable native compilation control");
    let mut previous: Option<(Vec<store::Reference>, store::Reference)> = None;
    let mut addresses = None;
    for (literal, fresh) in [(7, false), (7, true), (8, false), (8, true)]
        .into_iter()
        .take(if observation_only { 2 } else { 4 })
    {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
            .with_result_store(store.clone());
        let file = engine.sources.add("immutable-compilation.hir", format!(
            "const SECOND:u64=comptime {literal}u64\npub fn main()->Result[(),u64]{{Error(SECOND)}}\n"
        )).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let main = entry_path(&default_context());
        let reference = if fresh {
            engine.adapters.lowering = None;
            None
        } else {
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
            Some(native::Observation::of_reference(
                execution.observation.as_ref().unwrap(),
            ))
        };
        let mut cache = NativeDiscoveryCache::new(engine.types().clone(), store.clone());
        let mut declarations = NativeDeclarations::collect(&mut engine, &mut cache, &[]).unwrap();
        let paths = declarations
            .context
            .sources
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let lower = engine
            .native_original_reader(&paths, store.clone())
            .unwrap();
        for (path, source) in &mut declarations.sources {
            source.lower = lower
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
                hirsel_types::SignatureKind::Function(f) if f.generics.is_empty())
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
                    signatures: engine.signature_snapshot(),
                    semantic,
                    selected: std::slice::from_ref(&main),
                    callees: None,
                },
                &lower,
            )
            .unwrap(),
        );
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
        let inputs = NativeInstanceInputs::from_products(declarations, lower).unwrap();
        drop(cache);
        drop(engine);
        let trace = hirsel_lir::construction::ValueTrace::default();
        let _guard = trace.enter();
        // Fixture allocation only. Host source/C admission supplies its own
        // complete budget and is not installed by these native components.
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
            budget.reserve(24 << 20).unwrap(),
        )
        .unwrap();
        let arena = ArenaTasks::new(
            instances.clone(),
            store.clone(),
            0,
            weight,
            budget.reserve(24 << 20).unwrap(),
        )
        .unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let backends = [native::Backend::Llvm, native::Backend::Cranelift]
            .into_iter()
            .filter(|backend| !observation_only || *backend == native::Backend::Llvm)
            .map(|backend| CompilationBackend {
                backend: backend.name().into(),
                backend_version: ContentHash::of_parts(
                    "fixture backend",
                    [backend.name().as_bytes(), COMPILER_IDENTITY.as_slice()],
                ),
                target_features: target.digest(),
                profile_options: ContentHash::of_bytes(b"debug opt none"),
                runtime_abi: ContentHash::of_bytes(&hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes()),
                linker_options: ContentHash::of_parts(
                    "fixture selected platform",
                    [
                        backend.name().as_bytes(),
                        COMPILER_IDENTITY.as_slice(),
                        tool_identity.as_bytes(),
                    ],
                ),
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
                    frozen,
                }),
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
            budget.reserve(24 << 20).unwrap(),
        )
        .unwrap();
        let factory = CompilationFactory::new(compilation.clone());
        let mut task = factory.create(compilation.query_id()).unwrap();
        assert!(
            matches!(task.poll(&mut Pending), QueryPoll::Waiting(ref keys)
            if keys == &vec![instances.discovery_id(), arena.query_id()])
        );
        assert_eq!(trace.totals(), Default::default());
        assert_eq!(instances.imports.load(Ordering::Relaxed), 0);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        let denied = instances.failure("compilation fixture dependency refused");
        assert!(
            matches!(task.poll(&mut Failed(denied)), QueryPoll::Failed(error) if error == denied)
        );
        drop(task);
        let mut runtime = AdmittedRuntime::new(
            hirsel_intern::jobs::Jobs::new(jobs),
            budget.reserve(672 << 20).unwrap(),
            RuntimeErrors {
                resource: denied,
                cycle: denied,
                cancelled: denied,
                protocol: denied,
                panicked: denied,
            },
        )
        .unwrap();
        for (kind, factory) in compilation.factories() {
            runtime
                .register(kind, Arc::new(TraceFactory(factory, trace.clone())))
                .unwrap();
        }
        let request = runtime.request(&[compilation.query_id()]);
        let selected = runtime.wait(request).unwrap_or_else(|error| {
            panic!(
                "jobs={jobs} literal={literal} fresh={fresh}: {}",
                String::from_utf8_lossy(&store.get_content(error).unwrap())
            )
        })[0];
        let from_compilation = observation_only.then(|| {
            let before = trace.totals();
            let metadata = compilation.observation(selected).unwrap();
            let mut wrong = selected;
            wrong.action = store::Action(ContentHash::of_bytes(b"wrong compilation action"));
            assert!(compilation.observation(wrong).is_err());
            assert_eq!(trace.totals(), before);
            metadata
        });
        let outputs = compilation.outputs(selected).unwrap();
        let backend_count = if observation_only { 1 } else { 2 };
        assert_eq!(outputs.len(), backend_count);
        let current_addresses = outputs
            .iter()
            .map(|output| output.address)
            .collect::<Vec<_>>();
        if let Some(previous) = &addresses {
            assert_eq!(&current_addresses, previous);
        }
        addresses = Some(current_addresses);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            if fresh { 0 } else { backend_count as u64 }
        );
        let native = compilation.native(selected).unwrap();
        if fresh {
            assert_eq!(instances.imports.load(Ordering::Relaxed), 0);
            assert_eq!(instances.source_body_demands.load(Ordering::Relaxed), 0);
            assert_eq!(native.ready_transformations.load(Ordering::Relaxed), 0);
            assert_eq!(native.snapshot.ready_materializations(), 0);
            assert_eq!(trace.totals(), Default::default());
        }
        let roots = compilation.roots(selected).unwrap();
        let request = runtime.request(&[instances.discovery_id()]);
        let discovery = runtime.wait(request).unwrap()[0];
        let completed = instances.completed_instances(discovery).unwrap();
        let mut reads = vec![roots.query_id()];
        reads.extend(completed.iter().map(|(owner, _)| owner.ready_query_id()));
        let request = runtime.request(&reads);
        let selected = runtime.wait(request).unwrap();
        let (mut view, root) = roots.products(selected[0], &selected[1..]).unwrap();
        let ready = selected[1..]
            .iter()
            .map(|product| {
                Arc::new(native::queries::ready_product(store.as_ref(), *product).unwrap())
            })
            .collect::<Vec<_>>();
        let records = outputs
            .iter()
            .map(|output| output.record)
            .collect::<Vec<_>>();
        if let Some((old, launch)) = &previous {
            if fresh {
                assert_eq!(&records, old);
                assert_eq!(root.launch_manifest, *launch);
            } else {
                assert_ne!(&records, old);
                assert_ne!(root.launch_manifest, *launch);
            }
        }
        previous = Some((records, root.launch_manifest));
        let before = trace.totals();
        let bodies = native.snapshot.ready_materializations();
        let observation = observation_only.then(|| {
            let metadata = view.observation_metadata(&root).unwrap();
            let compiled = from_compilation.as_ref().unwrap();
            assert_eq!(metadata.failure_rules, compiled.failure_rules);
            assert_eq!(metadata.sites, compiled.sites);
            assert_eq!(metadata.anchors, compiled.anchors);
            assert!(!metadata.sites.is_empty());
            assert_eq!(&metadata.anchors, source.source_anchors());
            let mut wrong = root.as_ref().clone();
            wrong.static_manifest = wrong.launch_manifest;
            assert!(view.observation_metadata(&Arc::new(wrong)).is_err());
            assert_eq!(trace.totals(), before);
            assert_eq!(native.snapshot.ready_materializations(), bodies);
            metadata
        });
        for (index, output) in outputs.iter().enumerate() {
            let linked = linked_product(store.as_ref(), *output).unwrap();
            let destination = directory
                .path()
                .join(format!("restored-{literal}-{fresh}-{index}"));
            std::fs::create_dir_all(&destination).unwrap();
            let executable = destination.join("program");
            materialize_executable(store.as_ref(), &linked, &executable).unwrap();
            let run = native::launch(&executable, &target, &tools, &destination, true).unwrap();
            view.with_link_context(&root, &ready, |program| {
                assert!(program.units.is_empty());
                if let Some(metadata) = &observation {
                    assert_eq!(metadata.failure_rules, native::failure_rules(program));
                    assert_eq!(metadata.sites, native::site_codes(program));
                }
                let observed =
                    native::observe(&run, program, &source.sites()).map_err(|e| e.to_string())?;
                if let Some(reference) = &reference {
                    assert_eq!(&observed, reference);
                }
                Ok(())
            })
            .unwrap();
            assert!(String::from_utf8_lossy(&run.stderr).contains(&literal.to_string()));
        }
        assert_eq!(trace.totals(), before);
        assert_eq!(native.snapshot.ready_materializations(), bodies);
    }
}
