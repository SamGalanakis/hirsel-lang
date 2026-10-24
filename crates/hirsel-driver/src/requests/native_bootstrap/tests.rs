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
