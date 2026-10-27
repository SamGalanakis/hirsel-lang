use hirsel_driver::{
    COMPILER,
    requests::{CompileOperation, CompileRequest, CompileResult, CompilerRequests, RequestLimits},
};
use hirsel_engine::{
    CompileConfig, LoweringPolicy,
    requests::{OutputReadiness, RequestEngine, observe_inputs},
    runtime::{ProcessBudget, QueryRuntime},
    store::MemStore,
    with_compiler_stack,
};
use std::sync::Arc;

#[test]
fn export_stages_reclaim_idle_graphs_before_refusing_process_pressure() {
    pressure(true);
}
#[test]
fn export_can_retry_the_same_snapshot_after_temporary_process_pressure() {
    pressure(false);
}
fn pressure(reclaim_idle: bool) {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            directory.path().join("main.hir"),
            "pub fn main()->u8{7u8}\n",
        )
        .unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"export_pressure","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let budget = ProcessBudget::new(384 << 20);
        let config = CompileConfig {
            store: Arc::new(MemStore::default()),
            jobs: hirsel_intern::jobs::Jobs::new(1),
            hold_bytes: 0,
            ..Default::default()
        };
        let mut idle = reclaim_idle.then(|| {
            CompilerRequests::new(
                COMPILER,
                config.clone(),
                budget.clone(),
                RequestLimits {
                    runtime_bytes: 64 << 20,
                    snapshot_bytes: 32 << 20,
                    transport_bytes: 8 << 20,
                },
            )
        });
        if let Some(idle) = &mut idle {
            drop(
                idle.execute(
                    &manifest,
                    CompileOperation::Check {
                        lowering: LoweringPolicy::Reachable,
                    },
                )
                .unwrap(),
            );
        }
        let mut selector = CompilerRequests::new(
            COMPILER,
            config,
            budget.clone(),
            RequestLimits {
                runtime_bytes: 96 << 20,
                snapshot_bytes: 64 << 20,
                transport_bytes: 16 << 20,
            },
        );
        let compiler = selector.compiler().clone();
        let (loaded, inputs) = observe_inputs(
            selector.input_limits(),
            selector.capture_attempts(),
            |inputs| compiler.load_check_package_inputs(&manifest, inputs, true),
        )
        .unwrap();
        drop(loaded.unwrap());
        let request = CompileRequest {
            path: manifest,
            inputs,
            operation: CompileOperation::ExportTir,
        };
        let mut selected = selector.select(&request).unwrap();
        let pressure = budget.reserve(budget.limit() - budget.reserved()).unwrap();
        let completed = {
            let mut runtime = selected.runtime.lock().unwrap();
            let pending = runtime.request(&selected.roots);
            runtime.wait(pending).unwrap()
        };
        drop(pressure);
        let first = selected.output.ready(&completed);
        drop(selected);
        if reclaim_idle {
            assert!(
                matches!(&first.unwrap().result, CompileResult::ExportTir(text) if !text.is_empty())
            );
        } else {
            let error = first
                .err()
                .expect("export must refuse while no bytes can be reclaimed");
            assert!(
                error.to_string().contains("shared process budget"),
                "{error}"
            );
            let mut selected = selector.select(&request).unwrap();
            let completed = {
                let mut runtime = selected.runtime.lock().unwrap();
                let pending = runtime.request(&selected.roots);
                runtime.wait(pending).unwrap()
            };
            let output = selected.output.ready(&completed).unwrap();
            assert!(matches!(&output.result, CompileResult::ExportTir(text) if !text.is_empty()));
        }
    });
}
