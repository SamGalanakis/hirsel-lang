//! Actual source request, original Value plans, both native objects and runs.
use hirsel_driver::{COMPILER, native, requests::*, resident::ObservedSession};
use hirsel_engine::{CompileConfig, runtime::ProcessBudget, store::MemStore};
use std::sync::Arc;

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn ownership_selected_source_objects_execute_original_lineage_on_both_backends() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        std::fs::write(&source, "resource type Token { value:u64 }\ndrop(x) {}\npub fn main()->Result[(),u64] { let first=Token(value:7u64)\nlet moved=take first\nOk(()) }\n").unwrap();
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
        let variants = [
            native::RuntimeBuild::default(),
            native::RuntimeBuild {
                ownership_trace: true,
                ..Default::default()
            },
        ];
        let tools = native::Toolchain::in_tree_with_runtimes(
            &directory.path().join("tools"),
            &[&target],
            &variants,
        )
        .unwrap();
        let mut traces = Vec::new();
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let objects = execution
                .emit(backend)
                .unwrap_or_else(|error| panic!("{backend:?} source-plan emission: {error}"));
            assert_eq!(
                objects.objects().len(),
                2,
                "observed unit closure and ordinary checked glue"
            );
            assert!(
                objects
                    .objects()
                    .iter()
                    .all(|object| !object.bytes.is_empty())
            );
            assert_eq!(
                objects
                    .metadata()
                    .values()
                    .flat_map(|markers| markers.values())
                    .filter(|marker| marker.action == 0)
                    .count(),
                1
            );
            assert!(
                execution
                    .launch(&objects, &tools, &directory.path().join("refused"), 1000)
                    .is_err(),
                "default runtime cannot consume ownership objects"
            );
            let run = execution
                .launch(
                    &objects,
                    &tools.with_runtime(variants[1]),
                    &directory.path().join(backend.name()),
                    1000,
                )
                .unwrap_or_else(|error| panic!("{backend:?} source-plan launch: {error}"));
            assert_eq!(run.execution.exit, Some(0));
            assert!(run.execution.stdout.is_empty());
            assert!(
                run.execution.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&run.execution.stderr)
            );
            let trace = run
                .trace
                .unwrap_or_else(|error| panic!("{backend:?} source-plan trace: {error}"));
            let analysis = trace.analyze(run.program, 1000).unwrap();
            assert_eq!(
                (analysis.created, analysis.dropped, analysis.live.len()),
                (1, 1, 0)
            );
            traces.push(trace);
        }
        assert_eq!(traces[0], traces[1]);
        drop(execution);
        assert_eq!(
            budget.reserved(),
            0,
            "objects and source products release their admitted owners"
        );
    });
}
