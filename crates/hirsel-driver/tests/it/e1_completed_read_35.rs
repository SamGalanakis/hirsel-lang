//! The public Unix-read/compute program, with only the recurrence reduced for
//! the reference engine's step budget. Native acceptance uses the full fixture.
use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn public_completed_read_compute_agrees_on_four_engines() {
    use hirsel_driver::native::*;
    with_compiler_stack(|| {
        let source = include_str!("../fixtures/e1-completed-read-35/main.hir");
        let mut engine = crate::source_std::engine();
        let file = engine
            .sources
            .add("public-read-compute.hir", source.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let net = serde_json::from_value(serde_json::json!({
            "provider": "net.script@2",
            "rules": [{"address": "/fixture/gate.sock", "service": "0", "operation": "unix_connect"}],
            "peers": [{"address": "/fixture/gate.sock", "port": 0, "resolves_to": [], "tls": "plain",
                "connections": [
                    {"open": [{"expect": [71]}, {"send": [71]}, "close"]},
                    {"open": [{"expect": [71]}, {"send": [71]}, "close"]},
                    {"open": [{"expect": [71]}, {"send": [71]}, "close"]},
                    {"open": [{"expect": [71]}, {"send": [71]}, "close"]}
                ]}]
        })).unwrap();
        let fixture = Fixture {
            args: vec!["1".into(), "/fixture/gate.sock".into()],
            net: Some(net),
            ..Default::default()
        };
        let entry = entry_path(&default_context());
        let reference = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 35,
                inputs: vec![],
            },
            (&fixture).into(),
        )
        .unwrap();
        let expected = Observation::of_reference(reference.observation.as_ref().unwrap());
        assert_eq!(expected.exit_code, Some(0), "{expected:?}");
        assert!(expected.failures.is_empty(), "{expected:?}");
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = [
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: false },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Default,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Default,
                },
            },
        ];
        let scratch = tempfile::Builder::new()
            .prefix("e1-completed-read-35-")
            .tempdir()
            .unwrap();
        let toolchain =
            Toolchain::for_matrix(&scratch.path().join("runtime"), &configurations).unwrap();
        for (configuration, outcome) in differential_in(
            &validated,
            &entry,
            &expected,
            &configurations,
            &toolchain,
            scratch.path(),
            &fixture,
        ) {
            assert_eq!(outcome, Outcome::Agree, "{configuration}: {outcome:?}");
        }
    });
}
