use crate::{COMPILER, native::Fixture, requests::*, *};
use hirsel_engine::{CompileConfig, Verify, with_compiler_stack};

#[test]
fn prepared_host_record_replay_keeps_independent_state_and_exact_program_binding() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(&source, "pub fn main()->Result[(),()]{Ok(())}\n").unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"prepared_host","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let prepare = || {
            let config = CompileConfig::default();
            let mut requests = CompilerRequests::for_host(
                COMPILER.with_config(config.clone()).with_verify(Verify::On),
                config,
            );
            let output = requests
                .execute(&manifest, CompileOperation::Tir { definition: None })
                .unwrap();
            let CompileResult::Tir(prepared) = &output.result else {
                panic!("expected prepared TIR")
            };
            prepared.clone()
        };
        let prepared = prepare();
        let run = directory.path().join("run.json");
        let config = hirsel_run_config::RunConfig::for_program(
            &prepared.revision,
            hirsel_run_schema::LaunchKind::Main,
        );
        std::fs::write(&run, serde_json::to_vec(&config).unwrap()).unwrap();
        let fixture = Fixture::default();
        for record_checked in [false, true] {
            let tape = directory.path().join(format!("{record_checked}.tape"));
            let recorded = record_prepared_fixture_observations(
                &prepared,
                &run,
                &fixture,
                &tape,
                record_checked,
            )
            .unwrap();
            assert_eq!(recorded.exit_code, Some(0));
            assert!(recorded.output.is_empty() && recorded.failures.is_empty());
            let bytes = std::fs::read(&tape).unwrap();
            for _ in 0..2 {
                for checked in [false, true] {
                    let replay =
                        replay_prepared_observations(&prepared, &tape, Some(1_000_000), checked)
                            .unwrap();
                    assert!(replay.equivalent(&recorded));
                    assert_eq!(std::fs::read(&tape).unwrap(), bytes);
                }
            }
            std::fs::write(
                &source,
                "pub fn main()->Result[(),()]{let n = 1u64; Ok(())}\n",
            )
            .unwrap();
            let changed = prepare();
            assert_ne!(changed.revision, prepared.revision);
            for checked in [false, true] {
                let error = replay_prepared_observations(&changed, &tape, Some(1_000_000), checked)
                    .unwrap_err()
                    .to_string();
                assert!(
                    error.contains("binding") && error.contains("program"),
                    "{error}"
                );
            }
            assert_eq!(std::fs::read(&tape).unwrap(), bytes);
            std::fs::write(&source, "pub fn main()->Result[(),()]{Ok(())}\n").unwrap();
        }
    });
}
