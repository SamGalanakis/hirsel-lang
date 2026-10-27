//! Actual source/native diagnostic roots; no linked executable or old Session.
use hirsel_driver::{COMPILER, native, requests::*, resident::ObservedSession};
use hirsel_engine::{
    CompileConfig,
    runtime::ProcessBudget,
    store::{MemStore, ResultStore},
};
use hirsel_lir::ownership_trace::{Kind, native::LeafPlan};
use std::sync::Arc;

#[test]
fn ownership_manifest_request_preserves_resource_plans_and_last_output_admission() {
    source_request(true);
}

#[test]
fn ownership_request_selects_native_source_products_and_keeps_readiness_compiler_free() {
    source_request(false);
}

fn source_request(explicit_manifest: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.hir");
        std::fs::write(&path, "resource type Token { value:u64 }\ndrop(x) {}\npub fn main()->Result[(),u64] { let first=Token(value:7u64)\nlet second=take first\nOk(()) }\n").unwrap();
        let path = if explicit_manifest {
            let manifest = dir.path().join("hirsel.json");
            std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"ownership","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            manifest
        } else {
            path
        };
        let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
        let config = CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::ONE,
            store,
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
        let output = session
            .request(CompileOperation::NativeOwnership(OwnershipRequest {
                definition: None,
                target: native::TargetSpec::x86_64_unknown_linux_gnu(),
                validation: native::Validation::EveryStage,
                launch: Default::default(),
                fuel: false,
            }))
            .unwrap_or_else(|e| panic!("ownership source request: {e}"));
        let CompileResult::Ownership(execution) = &output.result else {
            panic!("missing ownership output");
        };
        let execution = execution.clone();
        drop(output);
        drop(session);
        assert!(
            budget.reserved() > 0,
            "selected outputs retain their own admitted source/native owners"
        );
        let mut creates = 0;
        let mut drops = 0;
        for product in execution.units() {
            execution
                .with_unit(*product, |observed| {
                    assert_eq!(observed.recipes.len(), 10);
                    assert_eq!(observed.recipes[0].before, observed.initial.unit);
                    LeafPlan::from_relocated(&observed.unit, &observed.layouts, &observed.plan)?;
                    creates += observed
                        .initial
                        .observations
                        .iter()
                        .filter(|row| row.marker.kind == Kind::Create)
                        .count();
                    drops += observed
                        .initial
                        .observations
                        .iter()
                        .filter(|row| row.marker.kind == Kind::Drop)
                        .count();
                    Ok(())
                })
                .unwrap();
        }
        assert!(
            creates > 0 && drops > 0,
            "real resource create/drop observations"
        );
        let mut wrong = execution.units()[0];
        wrong.action = hirsel_engine::store::Action(hirsel_syntax::ContentHash::of_bytes(
            b"another diagnostic output",
        ));
        assert!(execution.with_unit(wrong, |_| Ok(())).is_err());
        drop(execution);
        assert_eq!(
            budget.reserved(),
            0,
            "all diagnostic reservations release with the last output"
        );
    });
}
