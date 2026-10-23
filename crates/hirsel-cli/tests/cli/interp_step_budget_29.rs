//! Explicitly bounded reference application work; tooling usage is reviewable.
use hirsel_engine::*;
use std::{fs, path::PathBuf};

fn program(source: &str) -> (tempfile::TempDir, LoadedProgram) {
    let scratch = tempfile::tempdir().unwrap();
    fs::write(scratch.path().join("main.hir"), source).unwrap();
    let manifest = serde_json::json!({"schema":1,"package":{"name":"stepbudget","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"entry":{"module":[],"name":"main"},
        "dependencies":[{"manifest":PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json").canonicalize().unwrap(),"package":{"name":"std","version":"1"},"expose":[]}]});
    let path = scratch.path().join("hirsel.json");
    fs::write(&path, manifest.to_string()).unwrap();
    let mut loaded = hirsel_driver::load_program(&path).unwrap();
    loaded.check_for_execution().unwrap();
    (scratch, loaded)
}

fn config(loaded: &LoadedProgram) -> RunConfiguration {
    RunConfiguration {
        entry: loaded.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    }
}

fn assert_completed(result: &ObservedExecution, limit: u64) -> u64 {
    assert!(
        matches!(
            result.outcome,
            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
        ),
        "{result:?}"
    );
    let observed = result
        .observation
        .as_ref()
        .expect("complete semantic observation");
    assert!(observed.failures.is_empty(), "{observed:?}");
    let usage = result.step_usage.expect("reference step accounting");
    assert_eq!(usage.limit, Some(limit));
    assert!(usage.consumed <= limit);
    eprintln!("STEP_USAGE limit={limit} consumed={}", usage.consumed);
    usage.consumed
}

#[test]
fn sha256_kilobyte_reference_witness() {
    use sha2::{Digest, Sha256};
    with_compiler_stack(|| {
        let input = "a".repeat(1024);
        let source = format!(
            r#"pub fn main(rt: Io) -> Result[(), GrantError] {{
            let digest = std@1/sha256.digest_hex(b"{input}")
            let _ = out.print(io.stdout(rt)?, digest)
            Ok(())
        }}"#
        );
        let (_scratch, mut loaded) = program(&source);
        let run = config(&loaded);
        // Measured 1.24M steps for this input: a fixture policy, never a default.
        const STEPS: u64 = 2_000_000;
        let plain = run_observed_snapshot_bounded(
            &mut loaded.engine,
            &run,
            SimulationConfig::default(),
            STEPS,
        )
        .unwrap();
        let used = assert_completed(&plain, STEPS);
        assert_eq!(
            plain.observation.as_ref().unwrap().output,
            format!("{:x}", Sha256::digest(input.as_bytes())).as_bytes()
        );
        let checked = run_observed_checked_bounded(
            &mut loaded.engine,
            &run,
            SimulationConfig::default(),
            STEPS,
        )
        .unwrap();
        assert_eq!(assert_completed(&checked, STEPS), used);
        assert_eq!(plain.observation, checked.observation);
        // Restoring the default is required even after larger fixture execution.
        let default =
            run_observed_snapshot(&mut loaded.engine, &run, SimulationConfig::default()).unwrap();
        assert!(default.observation.is_none());
        assert!(matches!(default.outcome, Ok(RunOutcome::ToolingLimit(_))));
        assert_eq!(
            default.step_usage,
            Some(StepUsage {
                limit: Some(1_000_000),
                consumed: 1_000_000
            })
        );
        // The measured bound has exact stop semantics, not an approximate fuel unit.
        let exact = run_observed_snapshot_bounded(
            &mut loaded.engine,
            &run,
            SimulationConfig::default(),
            used,
        )
        .unwrap();
        assert_completed(&exact, used);
        let short = run_observed_snapshot_bounded(
            &mut loaded.engine,
            &run,
            SimulationConfig::default(),
            used - 1,
        )
        .unwrap();
        assert!(short.observation.is_none());
        assert!(matches!(short.outcome, Ok(RunOutcome::ToolingLimit(_))));
        assert_eq!(short.step_usage.unwrap().consumed, used - 1);
    });
}

#[test]
fn config_migration_document_reference_witness() {
    with_compiler_stack(|| {
        let source = r##"pub fn main() -> Result[(), std@1/toml.TomlError] {
            let original = "# service config\nversion = 1 # version\nendpoint = \"https://example.test\" # moved\ntimeout_ms = 2000\nretries = 2\nlog_level = \"info\"\n# app-owned\n\"app.name\" = \"wool\"\n[[workers]] # pool\nname = \"one\"\n"
            let mut doc = std@1/toml.document(copy original)?
            std@1/toml.set(mut doc, [Key("version")], Integer(2))?
            std@1/toml.set(mut doc, [Key("service"), Key("endpoint")], Text("https://example.test"))?
            std@1/toml.set(mut doc, [Key("timeouts"), Key("connect_ms")], Integer(1000))?
            std@1/toml.set(mut doc, [Key("timeouts"), Key("request_ms")], Integer(2000))?
            std@1/toml.remove(mut doc, [Key("endpoint")])?
            std@1/toml.remove(mut doc, [Key("timeout_ms")])?
            failure.check(std@1/toml.source(doc) == "# service config\nversion = 2 # version\nretries = 2\nlog_level = \"info\"\n# app-owned\n\"app.name\" = \"wool\"\n[[workers]] # pool\nname = \"one\"\n\n[service]\nendpoint = \"https://example.test\"\n\n[timeouts]\nconnect_ms = 1000\nrequest_ms = 2000\n", Arithmetic())
            let patch = std@1/diff.unified("0.toml", original, "0.toml", std@1/toml.source(doc), Myers(), 3)
            failure.check(string.contains(patch, "-version = 1"), Arithmetic())
            Ok(())
        }"##;
        let (_scratch, mut loaded) = program(source);
        let run = config(&loaded);
        const STEPS: u64 = 2_000_000;
        let plain = run_observed_snapshot_bounded(
            &mut loaded.engine,
            &run,
            SimulationConfig::default(),
            STEPS,
        )
        .unwrap();
        let used = assert_completed(&plain, STEPS);
        let checked = run_observed_checked_bounded(
            &mut loaded.engine,
            &run,
            SimulationConfig::default(),
            STEPS,
        )
        .unwrap();
        assert_eq!(assert_completed(&checked, STEPS), used);
        assert_eq!(plain.observation, checked.observation);
    });
}

#[test]
fn explicit_budget_restores_policy_after_launch_error_and_rejects_small_bounds() {
    with_compiler_stack(|| {
        let (_scratch, mut loaded) = program("pub fn main(rt: Io) -> Result[(), ()] { Ok(()) }");
        let run = config(&loaded);
        let mut denied = SimulationConfig::default();
        denied.grants.clear();
        assert!(
            run_observed_snapshot_bounded(&mut loaded.engine, &run, denied, 2_000_000).is_err()
        );
        for bound in [0, hirsel_interp::DEFAULT_STEP_LIMIT - 1] {
            let error = run_observed_snapshot_bounded(
                &mut loaded.engine,
                &run,
                SimulationConfig::default(),
                bound,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("must be at least 1000000"),
                "{error}"
            );
        }
        let default =
            run_observed_snapshot(&mut loaded.engine, &run, SimulationConfig::default()).unwrap();
        assert_completed(&default, 1_000_000);
    });
}

#[test]
fn host_file_reads_append_and_replay_without_the_host_file() {
    use std::process::Command;
    with_compiler_stack(|| {
        let source = r#"pub type Problem = Grant(GrantError) | File(FileError)
        pub fn main(rt: Io) -> Result[(), Problem] {
            let output = io.stdout(rt) ? Grant
            let d = io.dir(rt, path.of("data")) ? File
            let mut f = file.open(d, path.of("input")) ? File
            let mut buffer: List[u8] = []
            for i in 0u64..8u64 {
                failure.check(file.read(d, mut f, mut buffer, 64) ? File == 64, Arithmetic())
            }
            failure.check(file.read(d, mut f, mut buffer, 64) ? File == 0, Arithmetic())
            let bytes = bytes.from_list(take buffer)
            failure.check(bytes == b"REPLACE_INPUT", Arithmetic())
            let _ = out.print(output, "read all\n")
            Ok(())
        }"#
        .replace("REPLACE_INPUT", &"x".repeat(512));
        let (scratch, _loaded) = program(&source);
        let root = scratch.path();
        fs::create_dir(root.join("data")).unwrap();
        fs::write(root.join("data/input"), vec![b'x'; 512]).unwrap();
        let grants = serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"data","host":root.join("data"),"access":"read"}]}});
        let tape = root.join("read.tape");
        let live = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(root.join("hirsel.json"))
            .arg("--grants")
            .arg(grants.to_string())
            .arg("--record")
            .arg(&tape)
            .output()
            .unwrap();
        assert!(live.status.success(), "{live:?}");
        assert_eq!(live.stdout, b"read all\n");
        fs::remove_file(root.join("data/input")).unwrap();
        let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(root.join("hirsel.json"))
            .arg("--replay")
            .arg(&tape)
            .output()
            .unwrap();
        assert!(replay.status.success(), "{replay:?}");
        assert_eq!(live.stdout, replay.stdout);
    });
}
