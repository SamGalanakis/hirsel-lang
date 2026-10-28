use hirsel_driver::{COMPILER, requests::*, testing::observe_tir};
use hirsel_engine::{
    CompileConfig, RunConfiguration, RunOutcome, SimulationConfig,
    loader_inputs::{CaptureInputs, InputLimits},
    with_compiler_stack,
};
use std::sync::Arc;

#[test]
fn frozen_source_requests_ignore_newer_disk_and_retain_detached_execution() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("main.hir");
        let manifest = dir.path().join("hirsel.json");
        let original = "pub fn value()->u64{7u64}\npub fn main()->Result[(),()]{failure.check(value()==7u64,Arithmetic());Ok(())}\n";
        std::fs::write(&source, original).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"frozen","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[]}"#).unwrap();
        let capture = CaptureInputs::new(InputLimits {
            bytes: 1 << 20,
            operations: 4096,
        });
        drop(
            COMPILER
                .load_program_inputs(&manifest, &capture, false)
                .unwrap(),
        );
        let inputs = capture.freeze();
        std::fs::write(&source, "pub fn main(){missing}\n").unwrap();
        let mut requests = CompilerRequests::for_host(COMPILER, CompileConfig::default());
        let prepare = |requests: &mut CompilerRequests, inputs| {
            let output = requests
                .execute_request(&CompileRequest {
                    path: manifest.clone(),
                    inputs,
                    operation: CompileOperation::Tir { definition: None },
                })
                .unwrap();
            let CompileResult::Tir(tir) = &output.result else {
                panic!("expected TIR")
            };
            tir.clone()
        };
        let first = prepare(&mut requests, inputs.clone());
        let replacement = original.replace("7u64", "8u64");
        let overlaid = inputs
            .replace_sources(
                &[(
                    source.canonicalize().unwrap(),
                    Arc::<[u8]>::from(replacement.as_bytes()),
                )]
                .into(),
                1 << 20,
            )
            .unwrap();
        let second = prepare(&mut requests, overlaid);
        assert_ne!(first.revision, second.revision);
        drop(requests);
        for tir in [first, second] {
            for checked in [false, true] {
                let execution = observe_tir(
                    &tir,
                    &RunConfiguration {
                        entry: tir.entry.clone(),
                        hash_seed: 0,
                        inputs: vec![],
                    },
                    SimulationConfig::default(),
                    hirsel_interp::Limits {
                        steps: Some(10_000),
                        ..Default::default()
                    },
                    checked,
                )
                .unwrap();
                assert!(
                    matches!(
                        execution.outcome,
                        Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                    ),
                    "{:?}",
                    execution.outcome
                );
                assert!(execution.observation.unwrap().failures.is_empty());
            }
        }
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "pub fn main(){missing}\n"
        );
    });
}
