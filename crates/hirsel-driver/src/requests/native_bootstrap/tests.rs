use super::*;
use crate::native::{object_queries, queries::*};
use hirsel_engine::{
    CompileConfig, DefinitionQueries, FrontCheckPlan, FrontNativePlan, LoweringPolicy,
};
use hirsel_intern::jobs::Jobs;
use std::sync::atomic::{AtomicU64, Ordering};

struct StopLink(Arc<AtomicU64>, Vec<(DefPath, u64)>);
impl link_tasks::NativeLinkExecutor for StopLink {
    fn link(
        &self,
        products: &mut roots::RootProducts,
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
        assert_eq!(products.test_entries(input.root).unwrap(), self.1);
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("selected bootstrap Link control refusal".into())
    }
}

#[test]
fn actual_native_bootstrap_suspends_attaches_once_and_preserves_link_refusal_after_loader_drop() {
    bootstrap_link_refusal(false, false);
}

#[test]
fn native_test_table_binds_before_admission_and_survives_loader_drop() {
    bootstrap_link_refusal(true, false);
}

#[test]
fn native_emit_only_bootstrap_completes_actual_codegen_without_link_after_loader_drop() {
    bootstrap_link_refusal(false, true);
}

fn bootstrap_link_refusal(test_table: bool, emit_only: bool) {
    hirsel_engine::with_compiler_stack(|| {
        for jobs in [1, 2] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("main.hir");
            let manifest = directory.path().join("hirsel.json");
            std::fs::write(
                &path,
                if test_table {
                    "pub fn main(t:Test)->Result[(),u64]{Ok(())}\n"
                } else {
                    "pub fn main()->Result[(),u64]{Ok(())}\n"
                },
            )
            .unwrap();
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
                        lint_package: None,
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
                super::super::native_assembly::KINDS,
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
                    verify_store: hirsel_engine::store::VerifyStore::Off,
                    opt: hirsel_lir::emission::OptLevel::None,
                    debug: false,
                    verify_backend: true,
                    measurement: false,
                    fuel: false,
                    requested_run: None,
                    emit_only,
                    test_entries: if test_table {
                        vec![(program.entry.clone(), 731)]
                    } else {
                        vec![]
                    },
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
                        executor: Arc::new(StopLink(
                            calls.clone(),
                            if test_table {
                                vec![(program.entry.clone(), 731)]
                            } else {
                                vec![]
                            },
                        )),
                    }],
                },
                NativeAdmission {
                    instances: budget.reserve(32 << 20).unwrap(),
                    arena: budget.reserve(32 << 20).unwrap(),
                    compilation: budget.reserve(32 << 20).unwrap(),
                },
                None,
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
            if emit_only {
                let Poll::Ready(Ok(complete)) = result else {
                    panic!("expected actual Codegen completion, got {result:?}");
                };
                let (compilation, product, _) = bootstrap.outcome(complete[0]).unwrap();
                assert_eq!(
                    calls.load(Ordering::SeqCst),
                    0,
                    "emit-only must not call Link"
                );
                let outputs = compilation.outputs(product).unwrap();
                assert!(!outputs.is_empty(), "actual emitted objects");
                assert!(compilation.emitted_instances(product).unwrap() > 0);
                for output in outputs {
                    let fragment =
                        object_queries::fragment_product(store.as_ref(), *output).unwrap();
                    let object =
                        object_queries::load_object(store.as_ref(), fragment.object).unwrap();
                    assert!(!object.bytes.is_empty(), "real backend object bytes");
                }
                continue;
            }
            let Poll::Ready(Err(error)) = result else {
                panic!("expected exact Link refusal, got {result:?}");
            };
            let bytes = store.get_content(error).unwrap();
            assert_eq!(
                bootstrap.failure(error).as_deref(),
                Some("selected bootstrap Link control refusal")
            );
            let mut foreign = error;
            foreign.domain.schema += 1;
            assert!(bootstrap.failure(foreign).is_none());
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
                        lint_package: None,
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
                super::super::native_assembly::KINDS,
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
            let original_archive = selected_path("HIRSEL_RUNTIME_ARCHIVES")
                .join(&target.triple)
                .join("default/libhirsel_rt.a");
            let package = directory.path().join("profiled-runtime");
            std::fs::create_dir(&package).unwrap();
            let archive = package.join("libhirsel_rt.a");
            std::os::unix::fs::symlink(original_archive, &archive).unwrap();
            let order = package.join("startup.order");
            std::fs::write(&order, b"hirsel_rt_v2_main\n").unwrap();
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
            // The selected platform owns layout too: code generation, Link and
            // restoration must not reopen the now-removed orderfile.
            std::fs::remove_file(order).unwrap();
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
                    verify_store: hirsel_engine::store::VerifyStore::Off,
                    opt: hirsel_lir::emission::OptLevel::None,
                    debug: false,
                    verify_backend: true,
                    measurement: false,
                    fuel: false,
                    requested_run: None,
                    test_entries: Vec::new(),
                    emit_only: false,
                    backends: vec![selected_backend],
                },
                NativeAdmission {
                    instances: budget.reserve(32 << 20).unwrap(),
                    arena: budget.reserve(32 << 20).unwrap(),
                    compilation: budget.reserve(32 << 20).unwrap(),
                },
                None,
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
    native_bootstrap_policy_case(false);
}
#[test]
#[ignore = "requires native-bound-bootstrap-proof selected platform inputs"]
fn actual_native_bootstrap_binds_run_identity_and_delivers_live_output() {
    native_bootstrap_policy_case(true);
}
fn native_bootstrap_policy_case(bind_run: bool) {
    hirsel_engine::with_compiler_stack(|| {
        for backend in [native::Backend::Cranelift, native::Backend::Llvm] {
            let jobs = 1;
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("main.hir");
            let manifest = directory.path().join("hirsel.json");
            std::fs::write(&path, "pub fn main()->Result[(),u64]{Ok(())}\n").unwrap();
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"bootstrap","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
            let budget = ProcessBudget::new(1 << 30);
            let launch = crate::HostLaunch::empty();
            let mut bound = if bind_run {
                let config = CompileConfig {
                    jobs: Jobs::ONE,
                    store: store.clone(),
                    hold_bytes: 0,
                    ..Default::default()
                };
                let compiler = crate::COMPILER
                    .with_config(config.clone())
                    .with_owned_engines();
                let selector = crate::requests::CompilerRequests::new(
                    compiler.clone(),
                    config.clone(),
                    budget.clone(),
                    crate::requests::RequestLimits {
                        runtime_bytes: 256 << 20,
                        snapshot_bytes: 64 << 20,
                        transport_bytes: 32 << 20,
                    },
                );
                let limits = selector.input_limits();
                let mut session = crate::resident::ObservedSession::new(
                    manifest.clone(),
                    compiler,
                    config,
                    selector,
                    limits,
                    3,
                );
                let output = session
                    .request(crate::requests::CompileOperation::Tir { definition: None })
                    .unwrap();
                let crate::requests::CompileResult::Tir(facts) = &output.result else {
                    panic!("expected settled TIR launch facts")
                };
                let selected = crate::launch_binding::LaunchSelection::read(launch)
                    .unwrap()
                    .bind(
                        launch,
                        &facts.runtime.types,
                        &facts.signature,
                        &facts.revision,
                        facts.code.clone(),
                    )
                    .unwrap();
                drop(output);
                drop(session);
                assert_eq!(
                    budget.reserved(),
                    0,
                    "bound run facts retain no compiler graph"
                );
                Some(selected)
            } else {
                None
            };
            let validated_run = bound.as_ref().map(|bound| {
                Arc::new(match bound {
                    crate::BoundHostRun::Live(bound) => bound.run().clone(),
                    crate::BoundHostRun::Replay { run, .. } => run.clone(),
                })
            });
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
                        lint_package: None,
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
                super::super::native_assembly::KINDS,
                errors.protocol,
                budget.reserve(16 << 10).unwrap(),
            )
            .unwrap();
            let calls = Arc::new(AtomicU64::new(0));
            let target = native::TargetSpec::x86_64_unknown_linux_gnu();
            use crate::requests::{
                NativeRequest,
                native_platform::{
                    NativePlatformInputs, NativePlatformPolicy, NativePlatformSelection,
                    NativePurpose,
                },
            };
            let tools = Arc::new(
                native::Toolchain::in_tree_with_products(
                    &directory.path().join("runtime"),
                    &[&target],
                    &[native::RuntimeBuild::default()],
                    None,
                    store.clone(),
                )
                .unwrap(),
            );
            let policy = NativePlatformPolicy::capture().unwrap();
            let request = NativeRequest {
                definition: Some(entry.clone()),
                test_entries: Vec::new(),
                supplemental_sources: Vec::new(),
                output: crate::requests::NativeOutput::Executable,
                optimization: None,
                backend,
                profile: native::BuildProfile::Default,
                target: target.clone(),
                validation: native::Validation::EveryStage,
                launch: validated_run
                    .as_ref()
                    .map(|run| run.record().inputs.clone())
                    .unwrap_or_default(),
                run: validated_run,
                fuel: false,
                evidence: None,
            };
            let NativePlatformSelection { mut plan, platform } = NativePlatformSelection::capture(
                store.clone(),
                NativePlatformInputs {
                    verify_store: hirsel_engine::store::VerifyStore::Off,
                    purpose: if bind_run {
                        NativePurpose::Run
                    } else {
                        NativePurpose::Build
                    },
                    request: &request,
                    entry: &entry,
                    tools: &tools,
                    links: &[],
                    policy: Some(&policy),
                },
                object_queries::capture::CaptureLimits {
                    archive_bytes: 64 << 20,
                    files: 256,
                },
                object_queries::runtime::RuntimeLimits {
                    aliases: 128,
                    name_bytes: 64 << 10,
                },
                budget.reserve(16 << 10).unwrap(),
            )
            .unwrap();
            let platform = platform.expect("executable request captures its platform");
            plan.backends[0].executor = Arc::new(CountLink {
                platform: platform.clone(),
                calls: calls.clone(),
            });
            let bootstrap = NativeBootstrap::new(
                source.clone(),
                lower.clone(),
                paths,
                routing.clone(),
                store.clone(),
                plan,
                NativeAdmission {
                    instances: budget.reserve(32 << 20).unwrap(),
                    arena: budget.reserve(32 << 20).unwrap(),
                    compilation: budget.reserve(32 << 20).unwrap(),
                },
                None,
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
            let execution = if bind_run {
                Some(
                    crate::requests::native_products::NativeBuildReadiness::new(
                        bootstrap.clone(),
                        request.clone(),
                        tools.clone(),
                        Some(platform.clone()),
                    )
                    .unwrap()
                    .materialize(*selected)
                    .unwrap(),
                )
            } else {
                None
            };
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
            if let Some(execution) = execution {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                let first = native::request_output::run_bound_output(
                    &execution,
                    launch,
                    bound.take().unwrap(),
                    native::Output::Streams {
                        stdout: &mut stdout,
                        stderr: &mut stderr,
                    },
                    None,
                )
                .unwrap()
                .unwrap();
                assert_eq!(
                    first,
                    crate::RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                );
                let first_stdout = std::mem::take(&mut stdout);
                let first_stderr = std::mem::take(&mut stderr);
                std::fs::remove_file(&execution.executable).unwrap();
                let rebound = crate::launch_binding::LaunchSelection::read(launch)
                    .unwrap()
                    .bind(
                        launch,
                        &execution.types,
                        &execution.signature,
                        &execution.revision,
                        execution.code.clone(),
                    )
                    .unwrap();
                let second = native::request_output::run_bound_output(
                    &execution,
                    launch,
                    rebound,
                    native::Output::Streams {
                        stdout: &mut stdout,
                        stderr: &mut stderr,
                    },
                    None,
                )
                .unwrap()
                .unwrap();
                assert_eq!(first, second);
                assert_eq!(first_stdout, stdout);
                assert_eq!(first_stderr, stderr);
                assert_eq!(
                    calls.load(Ordering::SeqCst),
                    1,
                    "live readiness never compiles or Links again"
                );
                drop(execution);
            } else {
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
            }
            drop(platform);
            assert_eq!(
                budget.reserved(),
                0,
                "completed native controls release all owners"
            );
        }
    });
}

const PROOF_SOURCE: &str = r#"
fn bump(xs:List[u32],i:u64,y:u32)->u32 ensures result>=y {xs[i]+y}
pub fn main()->Result[(),()] {let n=bump([1u32,2u32],1u64,3u32);Ok(())}
"#;

/// One complete source graph over `PROOF_SOURCE`, with its PS9 export
/// attached to the exact native source root a bootstrap would select.
struct ProofGraph {
    runtime: AdmittedRuntime,
    source: Arc<FrontNativeQueries>,
    lower: Arc<FrontLowerQueries>,
    paths: Vec<DefPath>,
    export: Arc<FrontProofExport>,
    owners: Vec<Arc<dyn Send + Sync>>,
}
fn proof_graph(
    manifest: &std::path::Path,
    store: &Arc<dyn ResultStore>,
    budget: &Arc<ProcessBudget>,
    errors: RuntimeErrors,
    weight: MemoryWeight,
) -> ProofGraph {
    let mut program = crate::COMPILER
        .with_config(CompileConfig {
            jobs: Jobs::ONE,
            store: store.clone(),
            hold_bytes: 0,
            ..Default::default()
        })
        .with_owned_engines()
        .load_program(manifest)
        .unwrap();
    let paths = program.engine.front_source_roots();
    let entry = program.entry.clone();
    let mut runtime =
        AdmittedRuntime::new(Jobs::ONE, budget.reserve(512 << 20).unwrap(), errors).unwrap();
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
                lint_package: None,
            },
            weight,
            budget.reserve(24 << 20).unwrap(),
            errors,
        )
        .unwrap();
    let headers = program
        .engine
        .front_checked_signatures(front.clone(), budget.reserve(4 << 20).unwrap())
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
    let export = program
        .engine
        .front_proof_export_from_native(&entry, &source, headers, budget.reserve(4 << 20).unwrap())
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
    ProofGraph {
        runtime,
        source,
        lower,
        paths,
        export,
        owners: vec![front, growth, check],
    }
}

/// Evidence for the export's two `bump` obligations, signed by `prover@1`.
fn proof_evidence(graph: &mut ProofGraph) -> native::Evidence {
    let request = graph.runtime.request(&[graph.source.query()]);
    let completed = graph.runtime.wait(request).unwrap()[0];
    let input = graph
        .export
        .input_for_native(&graph.source, completed)
        .unwrap();
    let table = hirsel_proof_export::build(&input);
    let checker = hirsel_proof_export::sidecar::Checker::parse("prover@1").unwrap();
    let records = table
        .definitions
        .iter()
        .flat_map(|d| &d.obligations)
        .filter(|o| {
            let id = o.id.to_string();
            id.contains("/bump#FACT-1/") || id.contains("/bump#SEM-1/")
        })
        .map(|o| hirsel_proof_export::sidecar::Record {
            revision: table.header.program_revision.unwrap(),
            obligation: o.id.to_string(),
            checksum: o.checksum,
            checker: checker.clone(),
            budget: hirsel_proof_export::sidecar::Budget {
                unit: "steps".into(),
                count: 1000,
            },
            verdict: hirsel_proof_export::sidecar::Verdict::Proved,
        })
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    native::Evidence {
        records,
        trusted: vec![checker],
    }
}

#[test]
fn native_bootstrap_assesses_evidence_against_its_settled_source_root_before_instances() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("main.hir"), PROOF_SOURCE).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"proof_bootstrap","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
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
            resource: diagnostic(b"proof bootstrap resource refusal"),
            cycle: diagnostic(b"proof bootstrap cycle refusal"),
            cancelled: diagnostic(b"proof bootstrap cancelled"),
            protocol: diagnostic(b"proof bootstrap protocol refusal"),
            panicked: diagnostic(b"proof bootstrap producer panic"),
        };
        let weight = MemoryWeight {
            retained: 1 << 20,
            scratch: 16 << 20,
            stack: 0,
        };
        let evidence = {
            let mut graph = proof_graph(&manifest, &store, &budget, errors, weight);
            proof_evidence(&mut graph)
        };
        let mut untrusted = evidence.clone();
        untrusted.trusted.clear();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let plan = |calls: &Arc<AtomicU64>| NativePlan {
            options: native::Options::new(target.clone()),
            policy: ContentHash::of_bytes(b"proof bootstrap control"),
            verify_store: hirsel_engine::store::VerifyStore::Off,
            opt: hirsel_lir::emission::OptLevel::None,
            debug: false,
            verify_backend: true,
            measurement: false,
            fuel: false,
            requested_run: None,
            test_entries: Vec::new(),
            emit_only: false,
            backends: vec![CompilationBackend {
                backend: "cranelift".into(),
                backend_version: ContentHash::of_bytes(&crate::COMPILER_IDENTITY),
                target_features: target.digest(),
                profile_options: ContentHash::of_bytes(b"none"),
                runtime_abi: ContentHash::of_bytes(&hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes()),
                linker_options: ContentHash::of_bytes(b"intentional Link refusal"),
                runtime_archives: vec![],
                foreign_inputs: vec![],
                executor: Arc::new(StopLink(calls.clone(), vec![])),
            }],
        };
        let admission = || NativeAdmission {
            instances: budget.reserve(32 << 20).unwrap(),
            arena: budget.reserve(32 << 20).unwrap(),
            compilation: budget.reserve(32 << 20).unwrap(),
        };
        let mut queries = vec![];
        for (selected, removed) in [(Some(&evidence), 2), (Some(&untrusted), 0), (None, 0)] {
            let mut graph = proof_graph(&manifest, &store, &budget, errors, weight);
            let routing = NativeRouting::new(
                super::super::native_assembly::KINDS,
                errors.protocol,
                budget.reserve(16 << 10).unwrap(),
            )
            .unwrap();
            let calls = Arc::new(AtomicU64::new(0));
            let bootstrap = NativeBootstrap::new(
                graph.source.clone(),
                graph.lower.clone(),
                graph.paths.clone(),
                routing.clone(),
                store.clone(),
                plan(&calls),
                admission(),
                selected.map(|evidence| NativeProofs {
                    evidence: Arc::new(evidence.clone()),
                    export: graph.export.clone(),
                }),
                weight,
                budget.reserve(1 << 20).unwrap(),
                errors,
            )
            .unwrap();
            graph.runtime.register(KIND, bootstrap.factory()).unwrap();
            for (kind, factory) in routing.factories() {
                graph.runtime.register(kind, factory).unwrap();
            }
            let request = graph.runtime.request(&[bootstrap.query()]);
            graph.runtime.completion().wait(request);
            let result = graph.runtime.poll(request);
            let Poll::Ready(Err(error)) = result else {
                panic!("expected exact Link refusal, got {result:?}");
            };
            // Link is reached only after the complete discovery accepted the
            // assessed discharge; an unremoved licensed site fails earlier.
            assert_eq!(
                store.get_content(error).unwrap().as_ref(),
                b"selected bootstrap Link control refusal",
                "earlier assessment/discovery failure: {:?} / {:?}",
                bootstrap.failure(error),
                graph.source.failure(error)
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            match (selected, bootstrap.proofs()) {
                (None, None) => {}
                (Some(_), Some(proofs)) => {
                    let sites = proofs
                        .discharge
                        .0
                        .values()
                        .map(|sites| sites.len())
                        .sum::<usize>();
                    assert_eq!(sites, removed, "assessed discharge sites");
                    let report = proofs.report_discharged(&proofs.discharge);
                    assert!(
                        report
                            .iter()
                            .any(|line| line.starts_with(&format!("proofs: {removed} removed,"))),
                        "{report:?}"
                    );
                }
                _ => panic!("assessment presence differs from evidence"),
            }
            queries.push(bootstrap.query().address.0);
            graph.runtime.shutdown();
            drop(graph);
            drop(bootstrap);
            drop(routing);
        }
        queries.sort();
        queries.dedup();
        assert_eq!(
            queries.len(),
            3,
            "evidence identity selects its own bootstrap"
        );

        // A caller-planned discharge cannot bypass the settled assessment.
        let graph = proof_graph(&manifest, &store, &budget, errors, weight);
        let calls = Arc::new(AtomicU64::new(0));
        let mut planned = plan(&calls);
        planned.options.discharge.0.insert(
            graph.paths[0].clone(),
            [hirsel_lir::discharge::Discharged {
                body: 0,
                block: 0,
                statement: 0,
                form: hirsel_lir::discharge::Form::Integer,
            }]
            .into(),
        );
        let routing = NativeRouting::new(
            super::super::native_assembly::KINDS,
            errors.protocol,
            budget.reserve(16 << 10).unwrap(),
        )
        .unwrap();
        let refused = NativeBootstrap::new(
            graph.source.clone(),
            graph.lower.clone(),
            graph.paths.clone(),
            routing,
            store.clone(),
            planned,
            admission(),
            Some(NativeProofs {
                evidence: Arc::new(evidence),
                export: graph.export.clone(),
            }),
            weight,
            budget.reserve(1 << 20).unwrap(),
            errors,
        );
        assert!(refused.is_err());
        drop(refused);
        drop(graph);
        assert_eq!(budget.reserved(), 0, "every proof owner releases");
    });
}

#[test]
fn original_plan_inputs_bind_source_edits_membership_options_and_foreign_bytes() {
    hirsel_engine::with_compiler_stack(|| {
        const SOURCE: &str = "fn helper(x:u64)->u64{x+1u64}\nfn main()->u64{helper(7u64)}";
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("plan.hir");
        let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
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
            resource: diagnostic(b"manifest resource refusal"),
            cycle: diagnostic(b"manifest dependency cycle"),
            cancelled: diagnostic(b"manifest cancelled"),
            protocol: diagnostic(b"manifest protocol refusal"),
            panicked: diagnostic(b"manifest producer panic"),
        };
        let weight = MemoryWeight {
            retained: 1 << 20,
            scratch: 16 << 20,
            stack: 0,
        };
        let source_output = |text: &str| {
            std::fs::write(&path, text).unwrap();
            // Match the original vector's two local declarations. Loading an
            // ordinary std package here would demand thousands of unrelated
            // native roots before this fixture can test its input bindings.
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                crate::compiler_adapters(),
            )
            .with_result_store(store.clone());
            let file = engine.sources.add("plan.hir", text.into()).unwrap();
            engine
                .parse_file(file, &hirsel_engine::default_context())
                .unwrap();
            let paths = engine.front_source_roots();
            assert_eq!(paths.len(), if text.contains("fn other") { 3 } else { 2 });
            let front = engine
                .front_body_queries(&paths, weight, budget.reserve(24 << 20).unwrap(), errors)
                .unwrap();
            let lower = engine
                .front_lower_queries(
                    &paths,
                    front.clone(),
                    weight,
                    paths.len() * 3,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let definitions = engine
                .front_native_definition_queries(
                    &paths,
                    front.clone(),
                    lower.clone(),
                    weight,
                    budget.reserve(24 << 20).unwrap(),
                    errors,
                )
                .unwrap();
            let mut runtime =
                AdmittedRuntime::new(Jobs::ONE, budget.reserve(512 << 20).unwrap(), errors)
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
                .register(
                    hirsel_engine::FRONT_NATIVE_DEFINITION_KIND,
                    definitions.factory(),
                )
                .unwrap();
            let queries = paths
                .iter()
                .map(|path| definitions.query(path).unwrap())
                .collect::<Vec<_>>();
            drop(engine);
            let request = runtime.request(&queries);
            runtime
                .wait(request)
                .unwrap_or_else(|error| {
                    panic!(
                        "{}; paths={}; runtime={:?}; producer={:?}",
                        String::from_utf8_lossy(&store.get_content(error).unwrap()),
                        paths.len(),
                        runtime.stats(),
                        definitions.failure(error)
                    )
                })
                .iter()
                .map(|product| product.record)
                .collect::<Vec<_>>()
        };
        let original = source_output(SOURCE);
        for changed in [
            SOURCE.replace("1u64", "2u64"),
            format!("{SOURCE}\nfn other()->u64{{1u64}}"),
        ] {
            assert_ne!(original, source_output(&changed));
        }
        std::fs::write(&path, SOURCE).unwrap();
        // Keep the original two-declaration input vector explicit. A standalone
        // .hir load implicitly imports std, whose generated declarations need
        // their own accepted namespace and are not inputs to this binding test.
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            &manifest,
            r#"{"schema":1,"package":{"name":"plan_inputs","version":"1"},"sources":[{"path":"plan.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#,
        )
        .unwrap();
        let graph = proof_graph(&manifest, &store, &budget, errors, weight);
        assert_eq!(graph.paths.len(), 2);
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let calls = Arc::new(AtomicU64::new(0));
        let query = |case: u8, foreign: Vec<Reference>| {
            let mut options = native::Options::new(target.clone());
            if case == 1 {
                options.flat_walk = !options.flat_walk;
            }
            if case == 2 {
                options.scoped_specialization = !options.scoped_specialization;
            }
            let plan = NativePlan {
                options,
                policy: ContentHash::of_bytes(b"original plan input controls"),
                verify_store: hirsel_engine::store::VerifyStore::Off,
                opt: hirsel_lir::emission::OptLevel::None,
                debug: false,
                verify_backend: true,
                measurement: false,
                fuel: case == 3,
                requested_run: None,
                test_entries: vec![],
                emit_only: false,
                backends: vec![CompilationBackend {
                    backend: "cranelift".into(),
                    backend_version: ContentHash::of_bytes(&crate::COMPILER_IDENTITY),
                    target_features: target.digest(),
                    profile_options: ContentHash::of_bytes(b"none"),
                    runtime_abi: ContentHash::of_bytes(
                        &hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes(),
                    ),
                    linker_options: ContentHash::of_bytes(b"input binding link"),
                    runtime_archives: vec![],
                    foreign_inputs: foreign,
                    executor: Arc::new(StopLink(calls.clone(), vec![])),
                }],
            };
            let routing = NativeRouting::new(
                super::super::native_assembly::KINDS,
                errors.protocol,
                budget.reserve(16 << 10).unwrap(),
            )
            .unwrap();
            let bootstrap = NativeBootstrap::new(
                graph.source.clone(),
                graph.lower.clone(),
                graph.paths.clone(),
                routing,
                store.clone(),
                plan,
                NativeAdmission {
                    instances: budget.reserve(32 << 20).unwrap(),
                    arena: budget.reserve(32 << 20).unwrap(),
                    compilation: budget.reserve(32 << 20).unwrap(),
                },
                None,
                weight,
                budget.reserve(1 << 20).unwrap(),
                errors,
            )
            .unwrap();
            bootstrap.query()
        };
        let base = query(0, vec![]);
        for case in [1, 2, 3] {
            assert_ne!(base, query(case, vec![]));
        }
        let foreign = directory.path().join("foreign.o");
        let capture = || {
            object_queries::store_link_input_reader(
                store.as_ref(),
                std::fs::File::open(&foreign).unwrap(),
            )
            .unwrap()
        };
        std::fs::write(&foreign, b"one").unwrap();
        let before = query(0, vec![capture()]);
        std::fs::write(&foreign, b"two").unwrap();
        assert_ne!(before, query(0, vec![capture()]));
        let tools = native::Toolchain::in_tree(directory.path(), &[&target]).unwrap();
        assert!(
            native::selected_foreign_inputs(
                &tools,
                &target,
                &[hirsel_engine::LinkInput::Library("unknown".into())]
            )
            .is_err()
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "input selection runs no linker"
        );
        drop(graph);
        assert_eq!(budget.reserved(), 0, "selection releases every reservation");
    });
}
