//! Published SigV4 vectors execute more than the corpus's 1M source steps.
use hirsel_driver::run_path_with_step_budget;
use hirsel_engine::{RunOutcome, SimulationConfig, Verify, with_compiler_stack};
use std::path::PathBuf;

#[test]
fn published_aws_sigv4_vectors_execute_package_source() {
    with_compiler_stack(|| {
        for index in 0..6 {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../packages/s3@1/tests/aws-{index}.hir.json"));
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let outcome = run_path_with_step_budget(
                &path,
                SimulationConfig::default(),
                Some(5_000_000),
                Verify::On,
                &mut stdout,
                &mut stderr,
            )
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "AWS vector {index}: {outcome:?}; stdout={}; stderr={}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
    });
}

#[test]
fn client_facets_confine_authority_before_effects() {
    use hirsel_driver::load_program_source;
    use hirsel_engine::{InterpreterAdapter, RunConfiguration, run_observed_snapshot};
    use hirsel_interp::observation::Effect;

    with_compiler_stack(|| {
        for (name, expected_connects) in [
            ("facets-denied", 0),
            ("facets-allowed", 4),
            ("endpoint-grants", 0),
        ] {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../packages/s3@1/tests/{name}.hir.json"));
            let mut program = load_program_source(&path, None).unwrap();
            program.check_for_execution().unwrap();
            program.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                limits: hirsel_interp::Limits {
                    steps: Some(20_000_000),
                    ..Default::default()
                },
                ..Default::default()
            }));
            let mut simulation = SimulationConfig::default();
            simulation.grants.insert("WallClock".into());
            simulation.net = Some(
                serde_json::from_value(serde_json::json!({
                    "provider": "net.script@2",
                    "rules": [
                        {"address": "example.com", "service": "80", "operation": "connect"},
                        {"address": "example.com", "service": "443", "operation": "connect"},
                        {"address": "example.com", "service": "8443", "operation": "connect_tls"},
                        {"address": "::1", "service": "9000", "operation": "connect"}
                    ],
                    "peers": [{"address": "example.com", "port": 80,
                        "resolves_to": [], "tls": "plain", "connections": ["refused", "refused", "refused", "refused"]}]
                }))
                .unwrap(),
            );
            let config = RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let recorded = run_observed_snapshot(&mut program.engine, &config, simulation).unwrap();
            assert!(
                matches!(
                    recorded.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{name}: {:?}",
                recorded.outcome
            );
            let observation = recorded.observation.unwrap();
            let events: Vec<_> = observation
                .effects
                .iter()
                .filter_map(|effect| match &effect.effect {
                    Effect::CapabilityConsumed(event) => Some(event),
                    _ => None,
                })
                .collect();
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.operation == "net.connect")
                    .count(),
                expected_connects,
                "{name}"
            );
            if expected_connects == 0 {
                assert!(
                    events.iter().all(|e| !e.operation.starts_with("clock.")
                        && (!e.operation.starts_with("net.") || e.operation == "net.narrow")
                        && !e.operation.starts_with("tls.")),
                    "{name}: rejected operations reached an effect: {events:?}"
                );
            }
        }
    });
}

#[test]
fn opaque_client_examples_typecheck() {
    with_compiler_stack(|| {
        for example in ["hirsel.json", "read-only.hir.json"] {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../packages/s3@1/e2e")
                .join(example);
            hirsel_driver::load_program_source(&path, None)
                .unwrap()
                .check_for_execution()
                .unwrap();
        }
    });
}
