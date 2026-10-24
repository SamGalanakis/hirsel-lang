use super::*;
use crate::native::{object_queries, queries::*};
use hirsel_engine::{CompileConfig, FrontCheckPlan, FrontNativePlan, LoweringPolicy};
use hirsel_intern::jobs::Jobs;
use std::sync::atomic::{AtomicU64, Ordering};

struct StopLink(Arc<AtomicU64>);
impl link_tasks::NativeLinkExecutor for StopLink {
    fn link(
        &self,
        _: &mut roots::RootProducts,
        input: link_tasks::LinkExecution<'_>,
    ) -> Result<Vec<u8>, String> {
        assert!(
            !input.root.instances.is_empty(),
            "actual discovered native roots"
        );
        assert!(
            !input.ready.is_empty(),
            "independently admitted current C outputs"
        );
        assert!(
            !input.objects.is_empty(),
            "actual Codegen objects before Link"
        );
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("selected bootstrap Link control refusal".into())
    }
}

#[test]
fn actual_native_bootstrap_suspends_attaches_once_and_preserves_link_refusal_after_loader_drop() {
    hirsel_engine::with_compiler_stack(|| {
        for jobs in [1, 2] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("main.hir");
            let manifest = directory.path().join("hirsel.json");
            std::fs::write(&path, "pub fn main()->Result[(),u64]{Ok(())}\n").unwrap();
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"bootstrap","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
            let mut program = crate::COMPILER
                .with_config(CompileConfig {
                    jobs: Jobs::new(jobs),
                    store: store.clone(),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let paths = program.engine.front_source_roots();
            let entry = program.entry.clone();
            let budget = ProcessBudget::new(1 << 30);
            let diagnostic = |message: &[u8]| {
                store
                    .put_content(
                        Domain {
                            kind: 0x5200_0001,
                            schema: 1,
                        },
                        message,
                    )
                    .unwrap()
            };
            let errors = RuntimeErrors {
                resource: diagnostic(b"bootstrap resource refusal"),
                cycle: diagnostic(b"bootstrap cycle refusal"),
                cancelled: diagnostic(b"bootstrap cancelled"),
                protocol: diagnostic(b"bootstrap protocol refusal"),
                panicked: diagnostic(b"bootstrap producer panic"),
            };
            let weight = MemoryWeight {
                retained: 1 << 20,
                scratch: 16 << 20,
                stack: 0,
            };
            let mut runtime =
                AdmittedRuntime::new(Jobs::new(jobs), budget.reserve(512 << 20).unwrap(), errors)
                    .unwrap();
            let front = program
                .engine
                .front_body_queries(&paths, weight, budget.reserve(24 << 20).unwrap(), errors)
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &paths,
                    front.clone(),
                    weight,
                    paths.len() * 3,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let growth = program
                .engine
                .front_growth_queries(
                    &entry,
                    front.clone(),
                    weight,
                    budget.reserve(12 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let check = program
                .engine
                .front_check_queries(
                    FrontCheckPlan {
                        files: program
                            .files
                            .iter()
                            .map(|(file, cx)| (*file, cx.clone(), LoweringPolicy::Reachable))
                            .collect(),
                        front: front.clone(),
                        lower: lower.clone(),
                        growth: growth.clone(),
                        unsafe_ffi: program.unsafe_ffi.clone(),
                    },
                    weight,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let run = program
                .engine
                .front_run_queries(
                    check.clone(),
                    front.clone(),
                    lower.clone(),
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let definitions = program
                .engine
                .front_native_definition_queries(
                    &paths,
                    front.clone(),
                    lower.clone(),
                    weight,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let source = program
                .engine
                .front_native_queries(
                    FrontNativePlan {
                        entry,
                        roots: vec![],
                        run,
                        definitions,
                    },
                    weight,
                    budget.reserve(12 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let routing = NativeRouting::new(
                &[
                    INSTANCE_KIND,
                    instance_tasks::INSTANCE_INPUT_KIND,
                    instance_tasks::DISCOVERY_KIND,
                    ARENA_CANDIDATES_KIND,
                    ARENA_CHOSEN_KIND,
                    arena_tasks::ARENA_GRAPH_KIND,
                    compilation_tasks::COMPILATION_KIND,
                    READY_KIND,
                    ROOT_KIND,
                    object_queries::CODEGEN_KIND,
                    object_queries::LINK_KIND,
                    link_tasks::LINK_INPUT_KIND,
                    link_tasks::LINK_MANIFEST_KIND,
                ],
                errors.protocol,
                budget.reserve(16 << 10).unwrap(),
            )
            .unwrap();
            let calls = Arc::new(AtomicU64::new(0));
            let target = native::TargetSpec::x86_64_unknown_linux_gnu();
            let bootstrap = NativeBootstrap::new(
                source.clone(),
                lower.clone(),
                paths,
                routing.clone(),
                store.clone(),
                NativePlan {
                    options: native::Options::new(target.clone()),
                    policy: ContentHash::of_bytes(b"bootstrap control"),
                    opt: hirsel_lir::emission::OptLevel::None,
                    debug: false,
                    verify_backend: true,
                    measurement: false,
                    fuel: false,
                    foreign: BTreeMap::new(),
                    requested_run: None,
                    backends: vec![CompilationBackend {
                        backend: "cranelift".into(),
                        backend_version: ContentHash::of_bytes(&crate::COMPILER_IDENTITY),
                        target_features: target.digest(),
                        profile_options: ContentHash::of_bytes(b"none"),
                        runtime_abi: ContentHash::of_bytes(
                            &hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes(),
                        ),
                        linker_options: ContentHash::of_bytes(b"intentional Link refusal"),
                        runtime_archives: vec![],
                        foreign_inputs: vec![],
                        executor: Arc::new(StopLink(calls.clone())),
                    }],
                },
                NativeAdmission {
                    instances: budget.reserve(32 << 20).unwrap(),
                    arena: budget.reserve(32 << 20).unwrap(),
                    compilation: budget.reserve(32 << 20).unwrap(),
                },
                weight,
                budget.reserve(1 << 20).unwrap(),
                errors,
            )
            .unwrap();
            runtime
                .register(hirsel_engine::FRONT_BODY_KIND, front.factory())
                .unwrap();
            runtime
                .register_local(hirsel_engine::FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(hirsel_engine::FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_GROWTH_KIND, growth.factory())
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_CHECK_KIND, check.factory())
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_RUN_KIND, source.run_factory())
                .unwrap();
            runtime
                .register(
                    hirsel_engine::FRONT_NATIVE_DEFINITION_KIND,
                    source.definition_factory(),
                )
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_NATIVE_ROOT_KIND, source.factory())
                .unwrap();
            runtime.register(KIND, bootstrap.factory()).unwrap();
            for (kind, factory) in routing.factories() {
                runtime.register(kind, factory).unwrap();
            }
            drop(program);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                0,
                "factory installation must perform no Link"
            );
            let first = runtime.request(&[bootstrap.query()]);
            let second = runtime.request(&[bootstrap.query()]);
            runtime.cancel(first);
            runtime.completion().wait(second);
            let result = runtime.poll(second);
            let Poll::Ready(Err(error)) = result else {
                panic!("expected exact Link refusal, got {result:?}");
            };
            let bytes = store.get_content(error).unwrap();
            assert_eq!(
                bytes.as_ref(),
                b"selected bootstrap Link control refusal",
                "earlier bootstrap/source/C failure: {:?} / {:?}",
                bootstrap.failure(error),
                source.failure(error)
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "coalesced request must attach and Link exactly once"
            );
            drop(runtime);
            drop(bootstrap);
            drop(routing);
            drop(source);
            drop(check);
            drop(growth);
            drop(lower);
            drop(front);
            assert_eq!(
                budget.reserved(),
                0,
                "failed native bootstrap releases every owner"
            );
        }
    });
}

struct CountLink {
    platform: Arc<super::super::native_link::FrozenPlatform>,
    calls: Arc<AtomicU64>,
}
impl link_tasks::NativeLinkExecutor for CountLink {
    fn codegen_partition(&self) -> link_tasks::CodegenPartition {
        self.platform.codegen_partition()
    }
    fn link(
        &self,
        products: &mut roots::RootProducts,
        input: link_tasks::LinkExecution<'_>,
    ) -> Result<Vec<u8>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.platform.link(products, input)
    }
}

#[test]
#[ignore = "requires native-bootstrap-link-proof selected linker inputs"]
fn actual_native_bootstrap_links_both_backends_and_restores_after_runtime_drop() {
    hirsel_engine::with_compiler_stack(|| {
        for backend in [native::Backend::Cranelift, native::Backend::Llvm] {
            let jobs = 1;
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("main.hir");
            let manifest = directory.path().join("hirsel.json");
            std::fs::write(&path, "pub fn main()->Result[(),u64]{Ok(())}\n").unwrap();
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"bootstrap","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
            let mut program = crate::COMPILER
                .with_config(CompileConfig {
                    jobs: Jobs::new(jobs),
                    store: store.clone(),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let paths = program.engine.front_source_roots();
            let entry = program.entry.clone();
            let budget = ProcessBudget::new(1 << 30);
            let diagnostic = |message: &[u8]| {
                store
                    .put_content(
                        Domain {
                            kind: 0x5200_0001,
                            schema: 1,
                        },
                        message,
                    )
                    .unwrap()
            };
            let errors = RuntimeErrors {
                resource: diagnostic(b"bootstrap resource refusal"),
                cycle: diagnostic(b"bootstrap cycle refusal"),
                cancelled: diagnostic(b"bootstrap cancelled"),
                protocol: diagnostic(b"bootstrap protocol refusal"),
                panicked: diagnostic(b"bootstrap producer panic"),
            };
            let weight = MemoryWeight {
                retained: 1 << 20,
                scratch: 16 << 20,
                stack: 0,
            };
            let mut runtime =
                AdmittedRuntime::new(Jobs::new(jobs), budget.reserve(512 << 20).unwrap(), errors)
                    .unwrap();
            let front = program
                .engine
                .front_body_queries(&paths, weight, budget.reserve(24 << 20).unwrap(), errors)
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &paths,
                    front.clone(),
                    weight,
                    paths.len() * 3,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let growth = program
                .engine
                .front_growth_queries(
                    &entry,
                    front.clone(),
                    weight,
                    budget.reserve(12 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let check = program
                .engine
                .front_check_queries(
                    FrontCheckPlan {
                        files: program
                            .files
                            .iter()
                            .map(|(file, cx)| (*file, cx.clone(), LoweringPolicy::Reachable))
                            .collect(),
                        front: front.clone(),
                        lower: lower.clone(),
                        growth: growth.clone(),
                        unsafe_ffi: program.unsafe_ffi.clone(),
                    },
                    weight,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let run = program
                .engine
                .front_run_queries(
                    check.clone(),
                    front.clone(),
                    lower.clone(),
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let definitions = program
                .engine
                .front_native_definition_queries(
                    &paths,
                    front.clone(),
                    lower.clone(),
                    weight,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let source = program
                .engine
                .front_native_queries(
                    FrontNativePlan {
                        entry: entry.clone(),
                        roots: vec![],
                        run,
                        definitions,
                    },
                    weight,
                    budget.reserve(12 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let routing = NativeRouting::new(
                &[
                    INSTANCE_KIND,
                    instance_tasks::INSTANCE_INPUT_KIND,
                    instance_tasks::DISCOVERY_KIND,
                    ARENA_CANDIDATES_KIND,
                    ARENA_CHOSEN_KIND,
                    arena_tasks::ARENA_GRAPH_KIND,
                    compilation_tasks::COMPILATION_KIND,
                    READY_KIND,
                    ROOT_KIND,
                    object_queries::CODEGEN_KIND,
                    object_queries::LINK_KIND,
                    link_tasks::LINK_INPUT_KIND,
                    link_tasks::LINK_MANIFEST_KIND,
                ],
                errors.protocol,
                budget.reserve(16 << 10).unwrap(),
            )
            .unwrap();
            let calls = Arc::new(AtomicU64::new(0));
            let target = native::TargetSpec::x86_64_unknown_linux_gnu();
            use crate::requests::native_link::{FrozenPlatform, LinkInputs};
            use object_queries::capture::{CaptureLimits, FrozenLinkKit, FrozenLinker};
            let limits = CaptureLimits {
                archive_bytes: 64 << 20,
                files: 256,
            };
            let selected_path = |key| std::path::PathBuf::from(std::env::var_os(key).expect(key));
            let tool =
                FrozenLinker::snapshot(store.as_ref(), &selected_path("HIRSEL_CAPTURE_LINKER"))
                    .unwrap();
            let kit = FrozenLinkKit::snapshot(
                store.as_ref(),
                &selected_path("HIRSEL_LINK_KIT"),
                &target,
                tool,
                limits,
            )
            .unwrap();
            let archive = selected_path("HIRSEL_RUNTIME_ARCHIVES")
                .join(&target.triple)
                .join("default/libhirsel_rt.a");
            use object_queries::{
                platform::{CapturedPlatform, PlatformCaptureInputs, RuntimeSelection},
                runtime::RuntimeLimits,
            };
            let runtime_limits = RuntimeLimits {
                aliases: 128,
                name_bytes: 64 << 10,
            };
            let floor = selected_path("HIRSEL_GLIBC_FLOOR").join(&target.triple);
            let auditor =
                selected_path("HIRSEL_AUDITOR_PACKAGE").join(format!("{}.so", target.triple));
            let captured = CapturedPlatform::capture(
                store.as_ref(),
                &target,
                PlatformCaptureInputs {
                    linker: tool,
                    kit,
                    runtime_archive: &archive,
                    foreign: &[],
                    runtime: RuntimeSelection {
                        loader: &floor.join("ld-2.28.so"),
                        auditor: &auditor,
                        library_path: floor.as_os_str(),
                        emulator: None,
                        timeout: std::time::Duration::from_secs(20),
                        events_bytes: 64 << 10,
                        limits: runtime_limits,
                    },
                    limits,
                },
            )
            .unwrap();
            let platform = FrozenPlatform::new(
                store.clone(),
                LinkInputs {
                    platform: captured,
                    emulator: None,
                    timeout: std::time::Duration::from_secs(20),
                    runtime_limits,
                    limits,
                },
                entry.clone(),
                backend,
                target.clone(),
                None,
                budget.reserve(16 << 10).unwrap(),
            )
            .unwrap();
            let mut selected_backend = platform.backend(
                ContentHash::of_bytes(&crate::COMPILER_IDENTITY),
                target.digest(),
                ContentHash::of_bytes(b"none"),
                ContentHash::of_bytes(&hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes()),
            );
            selected_backend.executor = Arc::new(CountLink {
                platform: platform.clone(),
                calls: calls.clone(),
            });
            let bootstrap = NativeBootstrap::new(
                source.clone(),
                lower.clone(),
                paths,
                routing.clone(),
                store.clone(),
                NativePlan {
                    options: native::Options::new(target.clone()),
                    policy: ContentHash::of_bytes(b"bootstrap control"),
                    opt: hirsel_lir::emission::OptLevel::None,
                    debug: false,
                    verify_backend: true,
                    measurement: false,
                    fuel: false,
                    requested_run: None,
                    backends: vec![selected_backend],
                },
                NativeAdmission {
                    instances: budget.reserve(32 << 20).unwrap(),
                    arena: budget.reserve(32 << 20).unwrap(),
                    compilation: budget.reserve(32 << 20).unwrap(),
                },
                weight,
                budget.reserve(1 << 20).unwrap(),
                errors,
            )
            .unwrap();
            runtime
                .register(hirsel_engine::FRONT_BODY_KIND, front.factory())
                .unwrap();
            runtime
                .register_local(hirsel_engine::FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(hirsel_engine::FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_GROWTH_KIND, growth.factory())
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_CHECK_KIND, check.factory())
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_RUN_KIND, source.run_factory())
                .unwrap();
            runtime
                .register(
                    hirsel_engine::FRONT_NATIVE_DEFINITION_KIND,
                    source.definition_factory(),
                )
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_NATIVE_ROOT_KIND, source.factory())
                .unwrap();
            runtime.register(KIND, bootstrap.factory()).unwrap();
            for (kind, factory) in routing.factories() {
                runtime.register(kind, factory).unwrap();
            }
            drop(program);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                0,
                "factory installation must perform no Link"
            );
            let first = runtime.request(&[bootstrap.query()]);
            let second = runtime.request(&[bootstrap.query()]);
            runtime.cancel(first);
            runtime.completion().wait(second);
            let result = runtime.poll(second);
            let Poll::Ready(Ok(completed)) = result else {
                panic!("native bootstrap failed before executable readiness: {result:?}");
            };
            let [selected] = completed.as_slice() else {
                panic!("exactly one bootstrap root")
            };
            let (compilation, completion, facts) = bootstrap.outcome(*selected).unwrap();
            let mut wrong = *selected;
            wrong.action = Action(ContentHash::of_bytes(b"another completion"));
            assert!(bootstrap.outcome(wrong).is_err());
            let [linked] = compilation.outputs(completion).unwrap() else {
                panic!("exactly one native backend")
            };
            let linked = *linked;
            compilation.observation(completion).unwrap();
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "coalesced native requests Link once"
            );
            drop(runtime);
            drop(bootstrap);
            drop(routing);
            drop(source);
            drop(check);
            drop(growth);
            drop(lower);
            drop(front);
            drop(platform);
            drop(compilation);
            drop(facts);
            let artifact = object_queries::linked_product(store.as_ref(), linked).unwrap();
            let output = directory.path().join("ready");
            object_queries::materialize_executable(store.as_ref(), &artifact, &output).unwrap();
            assert_eq!(&std::fs::read(&output).unwrap()[..4], b"\x7fELF");
            let first = std::process::Command::new(&output)
                .env_clear()
                .output()
                .unwrap();
            assert!(first.status.success(), "{backend:?}: {:?}", first);
            std::fs::remove_file(&output).unwrap();
            object_queries::materialize_executable(store.as_ref(), &artifact, &output).unwrap();
            let second = std::process::Command::new(&output)
                .env_clear()
                .output()
                .unwrap();
            assert_eq!(first.status.code(), second.status.code());
            assert_eq!(first.stdout, second.stdout);
            assert_eq!(first.stderr, second.stderr);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "restoration does not Link again"
            );
            assert_eq!(
                budget.reserved(),
                0,
                "completed native controls release all owners"
            );
        }
    });
}

#[test]
#[ignore = "requires native-closed-bootstrap-proof selected platform inputs"]
fn actual_native_bootstrap_uses_selected_closed_runtime_on_link_and_reopen() {
    hirsel_engine::with_compiler_stack(|| {
        for backend in [native::Backend::Cranelift, native::Backend::Llvm] {
            let jobs = 1;
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("main.hir");
            let manifest = directory.path().join("hirsel.json");
            std::fs::write(&path, "pub fn main()->Result[(),u64]{Ok(())}\n").unwrap();
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"bootstrap","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
            let mut program = crate::COMPILER
                .with_config(CompileConfig {
                    jobs: Jobs::new(jobs),
                    store: store.clone(),
                    hold_bytes: 0,
                    ..Default::default()
                })
                .with_owned_engines()
                .load_program(&manifest)
                .unwrap();
            let paths = program.engine.front_source_roots();
            let entry = program.entry.clone();
            let budget = ProcessBudget::new(1 << 30);
            let diagnostic = |message: &[u8]| {
                store
                    .put_content(
                        Domain {
                            kind: 0x5200_0001,
                            schema: 1,
                        },
                        message,
                    )
                    .unwrap()
            };
            let errors = RuntimeErrors {
                resource: diagnostic(b"bootstrap resource refusal"),
                cycle: diagnostic(b"bootstrap cycle refusal"),
                cancelled: diagnostic(b"bootstrap cancelled"),
                protocol: diagnostic(b"bootstrap protocol refusal"),
                panicked: diagnostic(b"bootstrap producer panic"),
            };
            let weight = MemoryWeight {
                retained: 1 << 20,
                scratch: 16 << 20,
                stack: 0,
            };
            let mut runtime =
                AdmittedRuntime::new(Jobs::new(jobs), budget.reserve(512 << 20).unwrap(), errors)
                    .unwrap();
            let front = program
                .engine
                .front_body_queries(&paths, weight, budget.reserve(24 << 20).unwrap(), errors)
                .unwrap();
            let lower = program
                .engine
                .front_lower_queries(
                    &paths,
                    front.clone(),
                    weight,
                    paths.len() * 3,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let growth = program
                .engine
                .front_growth_queries(
                    &entry,
                    front.clone(),
                    weight,
                    budget.reserve(12 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let check = program
                .engine
                .front_check_queries(
                    FrontCheckPlan {
                        files: program
                            .files
                            .iter()
                            .map(|(file, cx)| (*file, cx.clone(), LoweringPolicy::Reachable))
                            .collect(),
                        front: front.clone(),
                        lower: lower.clone(),
                        growth: growth.clone(),
                        unsafe_ffi: program.unsafe_ffi.clone(),
                    },
                    weight,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let run = program
                .engine
                .front_run_queries(
                    check.clone(),
                    front.clone(),
                    lower.clone(),
                    weight,
                    budget.reserve(16 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let definitions = program
                .engine
                .front_native_definition_queries(
                    &paths,
                    front.clone(),
                    lower.clone(),
                    weight,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let source = program
                .engine
                .front_native_queries(
                    FrontNativePlan {
                        entry: entry.clone(),
                        roots: vec![],
                        run,
                        definitions,
                    },
                    weight,
                    budget.reserve(12 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let routing = NativeRouting::new(
                &[
                    INSTANCE_KIND,
                    instance_tasks::INSTANCE_INPUT_KIND,
                    instance_tasks::DISCOVERY_KIND,
                    ARENA_CANDIDATES_KIND,
                    ARENA_CHOSEN_KIND,
                    arena_tasks::ARENA_GRAPH_KIND,
                    compilation_tasks::COMPILATION_KIND,
                    READY_KIND,
                    ROOT_KIND,
                    object_queries::CODEGEN_KIND,
                    object_queries::LINK_KIND,
                    link_tasks::LINK_INPUT_KIND,
                    link_tasks::LINK_MANIFEST_KIND,
                ],
                errors.protocol,
                budget.reserve(16 << 10).unwrap(),
            )
            .unwrap();
            let calls = Arc::new(AtomicU64::new(0));
            let target = native::TargetSpec::x86_64_unknown_linux_gnu();
            use crate::requests::native_link::{FrozenPlatform, LinkInputs};
            use object_queries::capture::{CaptureLimits, FrozenLinkKit, FrozenLinker};
            let limits = CaptureLimits {
                archive_bytes: 64 << 20,
                files: 256,
            };
            let selected_path = |key| std::path::PathBuf::from(std::env::var_os(key).expect(key));
            let tool =
                FrozenLinker::snapshot(store.as_ref(), &selected_path("HIRSEL_CAPTURE_LINKER"))
                    .unwrap();
            let kit = FrozenLinkKit::snapshot(
                store.as_ref(),
                &selected_path("HIRSEL_LINK_KIT"),
                &target,
                tool,
                limits,
            )
            .unwrap();
            let archive = selected_path("HIRSEL_RUNTIME_ARCHIVES")
                .join(&target.triple)
                .join("default/libhirsel_rt.a");
            use object_queries::{
                platform::{CapturedPlatform, PlatformCaptureInputs, RuntimeSelection},
                runtime::RuntimeLimits,
            };
            let runtime_limits = RuntimeLimits {
                aliases: 128,
                name_bytes: 64 << 10,
            };
            let floor = selected_path("HIRSEL_GLIBC_FLOOR").join(&target.triple);
            let auditor =
                selected_path("HIRSEL_AUDITOR_PACKAGE").join(format!("{}.so", target.triple));
            let captured = CapturedPlatform::capture(
                store.as_ref(),
                &target,
                PlatformCaptureInputs {
                    linker: tool,
                    kit,
                    runtime_archive: &archive,
                    foreign: &[],
                    runtime: RuntimeSelection {
                        loader: &floor.join("ld-2.28.so"),
                        auditor: &auditor,
                        library_path: floor.as_os_str(),
                        emulator: None,
                        timeout: std::time::Duration::from_secs(20),
                        events_bytes: 64 << 10,
                        limits: runtime_limits,
                    },
                    limits,
                },
            )
            .unwrap();
            let platform = FrozenPlatform::new(
                store.clone(),
                LinkInputs {
                    platform: captured,
                    emulator: None,
                    timeout: std::time::Duration::from_secs(20),
                    runtime_limits,
                    limits,
                },
                entry.clone(),
                backend,
                target.clone(),
                None,
                budget.reserve(16 << 10).unwrap(),
            )
            .unwrap();
            let tools = native::Toolchain::in_tree_with_products(
                &directory.path().join("runtime"),
                &[&target],
                &[native::RuntimeBuild::default()],
                None,
                store.clone(),
            )
            .unwrap();
            let mut selected_backend = platform.backend(
                ContentHash::of_bytes(&crate::COMPILER_IDENTITY),
                target.digest(),
                ContentHash::of_bytes(b"none"),
                ContentHash::of_bytes(&hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes()),
            );
            selected_backend.executor = Arc::new(CountLink {
                platform: platform.clone(),
                calls: calls.clone(),
            });
            let bootstrap = NativeBootstrap::new(
                source.clone(),
                lower.clone(),
                paths,
                routing.clone(),
                store.clone(),
                NativePlan {
                    options: native::Options::new(target.clone()),
                    policy: ContentHash::of_bytes(b"bootstrap control"),
                    opt: hirsel_lir::emission::OptLevel::None,
                    debug: false,
                    verify_backend: true,
                    measurement: false,
                    fuel: false,
                    requested_run: None,
                    backends: vec![selected_backend],
                },
                NativeAdmission {
                    instances: budget.reserve(32 << 20).unwrap(),
                    arena: budget.reserve(32 << 20).unwrap(),
                    compilation: budget.reserve(32 << 20).unwrap(),
                },
                weight,
                budget.reserve(1 << 20).unwrap(),
                errors,
            )
            .unwrap();
            runtime
                .register(hirsel_engine::FRONT_BODY_KIND, front.factory())
                .unwrap();
            runtime
                .register_local(hirsel_engine::FRONT_LOWER_KIND, lower.factory())
                .unwrap();
            runtime
                .register_local(hirsel_engine::FRONT_EVALUATE_KIND, lower.factory())
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_GROWTH_KIND, growth.factory())
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_CHECK_KIND, check.factory())
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_RUN_KIND, source.run_factory())
                .unwrap();
            runtime
                .register(
                    hirsel_engine::FRONT_NATIVE_DEFINITION_KIND,
                    source.definition_factory(),
                )
                .unwrap();
            runtime
                .register(hirsel_engine::FRONT_NATIVE_ROOT_KIND, source.factory())
                .unwrap();
            runtime.register(KIND, bootstrap.factory()).unwrap();
            for (kind, factory) in routing.factories() {
                runtime.register(kind, factory).unwrap();
            }
            drop(program);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                0,
                "factory installation must perform no Link"
            );
            let first = runtime.request(&[bootstrap.query()]);
            let second = runtime.request(&[bootstrap.query()]);
            runtime.cancel(first);
            runtime.completion().wait(second);
            let result = runtime.poll(second);
            let Poll::Ready(Ok(completed)) = result else {
                panic!("native bootstrap failed before executable readiness: {result:?}");
            };
            let [selected] = completed.as_slice() else {
                panic!("exactly one bootstrap root")
            };
            let (compilation, completion, facts) = bootstrap.outcome(*selected).unwrap();
            let mut wrong = *selected;
            wrong.action = Action(ContentHash::of_bytes(b"another completion"));
            assert!(bootstrap.outcome(wrong).is_err());
            let [linked] = compilation.outputs(completion).unwrap() else {
                panic!("exactly one native backend")
            };
            let linked = *linked;
            compilation.observation(completion).unwrap();
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "coalesced native requests Link once"
            );
            drop(runtime);
            drop(bootstrap);
            drop(routing);
            drop(source);
            drop(check);
            drop(growth);
            drop(lower);
            drop(front);
            drop(compilation);
            drop(facts);
            let artifact = object_queries::linked_product(store.as_ref(), linked).unwrap();
            let output = directory.path().join("ready");
            object_queries::materialize_executable(store.as_ref(), &artifact, &output).unwrap();
            assert_eq!(&std::fs::read(&output).unwrap()[..4], b"\x7fELF");
            let runnable = platform.executable(output.clone()).unwrap();
            let first = native::launch_runnable(
                &runnable,
                &target,
                &tools,
                &directory.path().join("first"),
                false,
                native::Tape::Live {
                    record: None,
                    revocations: None,
                },
            )
            .unwrap();
            assert_eq!(first.exit, Some(0), "{backend:?}: {first:?}");
            std::fs::remove_file(&output).unwrap();
            object_queries::materialize_executable(store.as_ref(), &artifact, &output).unwrap();
            let restored = platform.executable(output.clone()).unwrap();
            let second = native::launch_runnable(
                &restored,
                &target,
                &tools,
                &directory.path().join("second"),
                false,
                native::Tape::Live {
                    record: None,
                    revocations: None,
                },
            )
            .unwrap();
            assert_eq!(first.exit, second.exit);
            assert_eq!(first.stdout, second.stdout);
            assert_eq!(first.stderr, second.stderr);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "closed restoration does not Link again"
            );
            drop(runnable);
            drop(restored);
            drop(platform);
            assert_eq!(
                budget.reserved(),
                0,
                "completed native controls release all owners"
            );
        }
    });
}
