//! Explicit grants and tape selection preserve checked diagnostic source binding.
use hirsel_driver::{COMPILER, native, requests::*, resident::ObservedSession};
use hirsel_engine::{CompileConfig, runtime::ProcessBudget, store::MemStore};
use std::sync::Arc;

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn ownership_native_granted_args_record_and_replay_keep_the_selected_resource_lineage() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        std::fs::write(&source, "resource type Token { value:u64 }\ndrop(x) {}\npub fn main(args:Args)->Result[(),u64] { let first=Token(value:7u64)\nlet moved=take first\nOk(()) }\n").unwrap();
        let path = directory.path().join("hirsel.json");
        std::fs::write(&path, r#"{"schema":1,"package":{"name":"ownership","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let config = CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::ONE,
            store: Arc::new(MemStore::default()),
            hold_bytes: 0,
            ..Default::default()
        };
        let budget = ProcessBudget::new(1 << 30);
        let compiler = COMPILER.with_config(config.clone()).with_owned_engines();
        let selector = CompilerRequests::new(
            compiler.clone(),
            config.clone(),
            budget.clone(),
            RequestLimits {
                runtime_bytes: 256 << 20,
                snapshot_bytes: 64 << 20,
                transport_bytes: 32 << 20,
            },
        );
        let limits = selector.input_limits();
        let mut session = ObservedSession::new(path, compiler, config, selector, limits, 3);
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let output = session
            .request(CompileOperation::NativeOwnership(OwnershipRequest {
                selection: hirsel_lir::ownership_trace::Selection::Resources,
                definition: None,
                target: target.clone(),
                validation: native::Validation::EveryStage,
                launch: Default::default(),
                fuel: false,
            }))
            .unwrap();
        let CompileResult::Ownership(execution) = &output.result else {
            panic!("missing diagnostic output")
        };
        let execution = execution.clone();
        drop(output);
        drop(session);
        let tools = native::Toolchain::in_tree_with_runtime(
            &directory.path().join("tools"),
            &[&target],
            native::RuntimeBuild {
                ownership_trace: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut traces = Vec::new();
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let objects = execution.emit(backend).unwrap();
            let denied = directory.path().join(format!("{}-denied", backend.name()));
            assert!(
                execution
                    .launch_with(
                        &objects,
                        &tools,
                        &denied,
                        1000,
                        hirsel_driver::HostLaunch::empty()
                    )
                    .is_err()
            );
            assert!(
                !denied.exists(),
                "ungranted roots refuse before process creation"
            );
            let fuel_denied = directory
                .path()
                .join(format!("{}-fuel-denied", backend.name()));
            let error = execution
                .launch_with(
                    &objects,
                    &tools,
                    &fuel_denied,
                    1000,
                    hirsel_driver::HostLaunch::Live {
                        config: hirsel_driver::LaunchConfig::Grants {
                            input: std::ffi::OsStr::new(
                                r#"{"args":["kept"],"budgets":{"provider":"budgets.limits@2","fuel":1000000}}"#,
                            ),
                            arguments: None,
                        },
                        record: None,
                        revocations: None,
                    },
                )
                .err()
                .expect("a fuel budget must not run objects compiled without checks");
            assert!(error.contains("has no fuel checks"), "{error}");
            assert!(
                !fuel_denied.exists(),
                "fuel mismatch refuses before linking"
            );
            let tape = directory.path().join(format!("{}.tape", backend.name()));
            for replay in [false, true] {
                let launch = if replay {
                    hirsel_driver::HostLaunch::Replay(&tape)
                } else {
                    hirsel_driver::HostLaunch::Live {
                        config: hirsel_driver::LaunchConfig::Grants {
                            input: std::ffi::OsStr::new(r#"{"args":["kept"]}"#),
                            arguments: None,
                        },
                        record: Some(&tape),
                        revocations: None,
                    }
                };
                let run = execution
                    .launch_with(
                        &objects,
                        &tools,
                        &directory
                            .path()
                            .join(format!("{}-{replay}", backend.name())),
                        1000,
                        launch,
                    )
                    .unwrap();
                assert_eq!(run.execution.exit, Some(0));
                assert!(run.execution.stdout.is_empty());
                assert!(
                    run.execution.stderr.is_empty(),
                    "{}",
                    String::from_utf8_lossy(&run.execution.stderr)
                );
                assert_eq!(
                    execution
                        .observe_run(&objects, &run.execution)
                        .unwrap()
                        .exit_code(),
                    0
                );
                let trace = run.trace.unwrap();
                let analysis = trace.analyze(run.program, 1000).unwrap();
                assert_eq!(
                    (analysis.created, analysis.dropped, analysis.live.len()),
                    (1, 1, 0)
                );
                traces.push(trace);
            }
            assert!(std::fs::metadata(tape).unwrap().len() > 0);
        }
        for trace in &traces[1..] {
            assert_eq!(trace, &traces[0]);
        }
        drop(execution);
        assert_eq!(
            budget.reserved(),
            0,
            "objects and source products release their admitted owners"
        );
    });
}
