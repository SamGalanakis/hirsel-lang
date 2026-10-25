use super::*;
use crate::requests::{
    CompileOperation, CompileRequest, CompileResult, CompilerRequests, RequestLimits,
};
use hirsel_engine::{
    CompileConfig,
    requests::{RequestEngine, observe_inputs},
    runtime::*,
    store::{MemStore, ResultStore},
};
use hirsel_intern::jobs::Jobs;
use std::task::Poll;

#[test]
#[ignore = "requires native-selector-proof selected platform inputs"]
fn actual_native_selector_coalesces_build_and_bound_run_without_a_retained_controller() {
    hirsel_engine::with_compiler_stack(|| {
        for backend in [native::Backend::Cranelift, native::Backend::Llvm] {
            let directory = tempfile::tempdir().unwrap();
            let manifest = directory.path().join("hirsel.json");
            std::fs::write(
                directory.path().join("main.hir"),
                "pub fn main()->Result[(),u64]{Ok(())}\n",
            )
            .unwrap();
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"selected","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
            let target = native::TargetSpec::x86_64_unknown_linux_gnu();
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
            let config = CompileConfig {
                jobs: Jobs::ONE,
                store: store.clone(),
                hold_bytes: 0,
                ..Default::default()
            };
            let compiler = crate::COMPILER
                .with_config(config.clone())
                .with_owned_engines();
            let budget = ProcessBudget::new(1 << 30);
            let mut selector = CompilerRequests::new(
                compiler.clone(),
                config,
                budget.clone(),
                RequestLimits {
                    runtime_bytes: 512 << 20,
                    snapshot_bytes: 256 << 20,
                    transport_bytes: 32 << 20,
                },
            )
            .with_toolchain(tools.clone());
            let (loaded, inputs) = observe_inputs(selector.input_limits(), 3, |inputs| {
                compiler
                    .load_check_package_inputs(&manifest, inputs, true)
                    .map(|_| ())
            })
            .unwrap();
            loaded.unwrap();
            let mut native = NativeRequest {
                definition: None,
                backend,
                profile: native::BuildProfile::Default,
                target: target.clone(),
                validation: native::Validation::EveryStage,
                launch: Default::default(),
                run: None,
                fuel: false,
                evidence: None,
            };
            let launch = crate::HostLaunch::empty();
            let mut bound = None;
            for run in [false, true] {
                let request = CompileRequest {
                    path: manifest.clone(),
                    inputs: inputs.clone(),
                    operation: if run {
                        CompileOperation::NativeRun(native.clone())
                    } else {
                        CompileOperation::NativeBuild(native.clone())
                    },
                };
                let first = selector.select(&request).unwrap();
                let second = selector.select(&request).unwrap();
                assert!(
                    Arc::ptr_eq(&first.runtime, &second.runtime),
                    "same source and recaptured platform must coalesce"
                );
                assert_eq!(first.roots, second.roots);
                let cancelled = first.runtime.lock().unwrap().request(&first.roots);
                let active = second.runtime.lock().unwrap().request(&second.roots);
                first.runtime.lock().unwrap().cancel(cancelled);
                let completion = second.runtime.lock().unwrap().completion();
                completion.wait(active);
                let completed = second.runtime.lock().unwrap().poll(active);
                let Poll::Ready(Ok(products)) = completed else {
                    panic!(
                        "selected native producer failed: {completed:?}; stats={:?}",
                        second.runtime.lock().unwrap().stats()
                    );
                };
                let output = (second.output.materialize)(&products).unwrap();
                let repeated = (first.output.materialize)(&products).unwrap();
                let (CompileResult::Native(a), CompileResult::Native(b)) =
                    (&output.result, &repeated.result)
                else {
                    panic!("native readiness results");
                };
                assert!(
                    Arc::ptr_eq(a, b),
                    "coalesced callers share the admitted execution metadata"
                );
                drop(repeated);
                let CompileResult::Native(execution) = &output.result else {
                    panic!("selected native readiness did not produce executable");
                };
                assert_eq!(execution.entry, output.entry);
                assert_eq!(execution.request.backend, backend);
                let destination = directory.path().join("delivered");
                native::request_output::materialize_output(execution, &destination).unwrap();
                assert_eq!(&std::fs::read(&destination).unwrap()[..4], b"\x7fELF");
                std::fs::remove_file(&execution.executable).unwrap();
                if run {
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    let result = native::request_output::run_bound_output(
                        execution,
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
                        result,
                        crate::RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                    );
                } else {
                    let selected = crate::launch_binding::LaunchSelection::read(launch)
                        .unwrap()
                        .bind(
                            launch,
                            &execution.types,
                            &execution.signature,
                            &execution.revision,
                            execution.code.clone(),
                        )
                        .unwrap();
                    native.run = Some(Arc::new(match &selected {
                        crate::BoundHostRun::Live(bound) => bound.run().clone(),
                        crate::BoundHostRun::Replay { run, .. } => run.clone(),
                    }));
                    native.launch = native.run.as_ref().unwrap().record().inputs.clone();
                    bound = Some(selected);
                    let runnable = execution.platform.executable(destination.clone()).unwrap();
                    let child = native::launch_runnable(
                        &runnable,
                        &target,
                        &tools,
                        &directory.path().join("child"),
                        false,
                        native::Tape::Live {
                            record: None,
                            revocations: None,
                        },
                    )
                    .unwrap();
                    assert_eq!(child.exit, Some(0));
                }
                drop(output);
                drop(first);
                drop(second);
            }
            drop(selector);
            assert_eq!(
                budget.reserved(),
                0,
                "all selected graph and output admissions release"
            );
        }
    });
}
