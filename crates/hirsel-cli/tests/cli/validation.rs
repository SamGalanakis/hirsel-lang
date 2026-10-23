//! The production default and the explicit harness mode lower identical native
//! code, with four fewer LIR validations per unit in the production mode.
use super::file;
use std::process::Command;

#[test]
fn validate_stages_preserves_native_bytes_and_restores_four_checks_per_unit() {
    // task.async names task_main only through ClosureSite.code: the worker's
    // dependency graph must keep that template and bind it into the object key.
    let source = file(
        r#"
        pub fn main(rt: Io) -> Result[(), GrantError] {
          let tasks = io.tasks(rt)?
          task.scope(tasks, fn(s: Scope) {
            let child = task.async(s, fn() -> u64 { 1u64 })
            let answer = task.await(s, take child)
            failure.check(answer == 1u64, Arithmetic())
          })
          Ok(())
        }
    "#,
    );
    let output = source.with_extension("ready.exe");
    let mut results = Vec::new();
    for jobs in [1, 2, 8] {
        for stages in [false, true] {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            cmd.env_remove("HIRSEL_VALIDATE_STAGES")
                .env("HIRSEL_OBJECTS", "none@1")
                .env("HIRSEL_JOBS", jobs.to_string())
                .args([
                    "--timings=json",
                    "build",
                    "--debug",
                    "--backend",
                    "cranelift",
                ]);
            if stages {
                cmd.arg("--validate-stages");
            }
            let built = cmd.arg(&source).arg("-o").arg(&output).output().unwrap();
            assert!(built.status.success(), "{stages}: {built:?}");
            let report: serde_json::Value = serde_json::from_str(
                std::str::from_utf8(&built.stderr)
                    .unwrap()
                    .lines()
                    .last()
                    .unwrap(),
            )
            .unwrap();
            assert!(
                report["counts"]["object misses"].as_u64().unwrap() > 0,
                "worker path not selected: {report}"
            );
            let calls = report["phases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["phase"] == "lir validate")
                .unwrap()["calls"]
                .as_u64()
                .unwrap();
            results.push((
                std::fs::read(&output).unwrap(),
                calls,
                report["counts"]["units"].as_u64().unwrap(),
            ));
            let launched = Command::new(&output)
                .args([
                    "--hirsel-grants",
                    r#"{"io":{"tasks":"tasks.cooperative@1","dirs":[]}}"#,
                ])
                .output()
                .unwrap();
            assert!(launched.status.success(), "{jobs} {stages}: {launched:?}");
            assert!(
                launched.stdout.is_empty() && launched.stderr.is_empty(),
                "{launched:?}"
            );
        }
    }
    assert_eq!(
        results[0].0, results[1].0,
        "validation policy changed the native executable"
    );
    assert!(results[0].2 > 0);
    assert_eq!(results[0].2, results[1].2);
    assert_eq!(results[1].1 - results[0].1, 4 * results[0].2);
    assert!(
        results
            .chunks_exact(2)
            .all(|pair| { pair[0] == results[0] && pair[1] == results[1] }),
        "worker count changed executable bytes or validation counts"
    );
    let invalid = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--validate-stages"])
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("--validate-stages requires --native")
    );
}

#[test]
fn optimized_warm_fragments_preserve_executables_and_runtime_across_policies_and_jobs() {
    let source = file(
        "fn step(x:u64)->u64{x+1u64}\npub fn main()->Result[(),()]{failure.check(step(3u64)==4u64,Arithmetic());Ok(())}",
    );
    let output = source.with_extension("warm.exe");
    let cache = source.with_extension("cache");
    let mut expected = None;
    let mut cold_units = 0;
    let mut cold_checks = 0;
    for jobs in [1, 2, 8] {
        for stages in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command
                .env_remove("HIRSEL_VALIDATE_STAGES")
                .env("HIRSEL_OBJECTS", "dir@1")
                .env("XDG_CACHE_HOME", &cache)
                .env("HIRSEL_JOBS", jobs.to_string())
                .args(["--timings=json", "build", "--release", "--backend", "llvm"]);
            if stages {
                command.arg("--validate-stages");
            }
            let built = command
                .arg(&source)
                .arg("-o")
                .arg(&output)
                .output()
                .unwrap();
            assert!(built.status.success(), "{jobs} {stages}: {built:?}");
            let report: serde_json::Value = serde_json::from_str(
                std::str::from_utf8(&built.stderr)
                    .unwrap()
                    .lines()
                    .last()
                    .unwrap(),
            )
            .unwrap();
            let units = report["counts"]["units"].as_u64().unwrap();
            let hits = report["counts"]["object hits"].as_u64().unwrap();
            let checks = report["phases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["phase"] == "lir validate")
                .unwrap()["calls"]
                .as_u64()
                .unwrap();
            let bytes = std::fs::read(&output).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(&bytes, expected, "{jobs} {stages}: executable changed");
                assert!(hits > 0, "warm fragment control had no hits: {report}");
                assert_eq!(report["counts"]["object misses"], 0);
                if stages {
                    assert_eq!(units, cold_units);
                    assert_eq!(checks - cold_checks, 4 * units);
                } else {
                    assert_eq!(units, 0, "Ready hit repeated the pipeline");
                }
            } else {
                assert!(units > 0);
                assert_eq!(hits, 0);
                cold_units = units;
                cold_checks = checks;
                expected = Some(bytes);
            }
            let launched = Command::new(&output).output().unwrap();
            assert!(launched.status.success(), "{jobs} {stages}: {launched:?}");
            assert!(
                launched.stdout.is_empty() && launched.stderr.is_empty(),
                "{launched:?}"
            );
        }
    }
    std::fs::remove_dir_all(cache).unwrap();
}

#[test]
fn malformed_launch_grants_report_paths_before_running_and_empty_dirs_still_run() {
    let source = file(
        r#"pub fn main(rt: Io) -> Result[(), GrantError] {
      match io.stdout(rt) {
        Ok(output) => { let _ = out.print(output, "program-ran"); () }
        Error(_) => ()
      }
      Ok(())
    }"#,
    );
    let exe = source.with_extension("grants.exe");
    let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .env("HIRSEL_DAEMON", "off")
        .env("HIRSEL_JOBS", "1")
        .args(["build", "--debug", "--backend", "cranelift"])
        .arg(&source)
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");
    let cases = [
        (
            r#"{"io":{"stdout":"stdout.bytes@1"},"env":{"TOKEN":"credential-must-stay-private"}}"#,
            "io.dirs",
            "missing field `dirs`",
        ),
        (
            r#"{"io":{"stdout":"stdout.bytes@1","dirs":[],"net":{"provider":"net.script@2","rules":[],"peers":[{"address":"example.com","port":443,"resolves_to":[{"v4":"192.0.2.1"}],"tls":"plain","connections":[]}]}},"env":{"TOKEN":"credential-must-stay-private"}}"#,
            "io.net.peers[0].resolves_to[0]",
            "invalid type: map, expected a string",
        ),
    ];
    for engine in ["interpreted", "native", "built"] {
        let launch = |grants: &str| {
            let mut cmd = if engine == "built" {
                Command::new(&exe)
            } else {
                let mut cmd = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                cmd.arg("run").arg(&source);
                if engine == "native" {
                    cmd.args(["--native", "--backend", "cranelift"]);
                }
                cmd
            };
            cmd.env("HIRSEL_DAEMON", "off")
                .env("HIRSEL_JOBS", "1")
                .arg(if engine == "built" {
                    "--hirsel-grants"
                } else {
                    "--grants"
                })
                .arg(grants)
                .output()
                .unwrap()
        };
        for (grants, path, reason) in cases {
            let output = launch(grants);
            assert!(!output.status.success(), "{engine}: {output:?}");
            assert!(
                output.stdout.is_empty(),
                "program ran: {engine}: {output:?}"
            );
            let detail = String::from_utf8(output.stderr).unwrap();
            assert!(
                detail.contains(&format!("{path}: {reason}"))
                    && detail.contains(" at line 1 column "),
                "{engine}: {detail}"
            );
            assert!(
                detail.contains(r#""kind":"malformed""#),
                "{engine}: {detail}"
            );
            assert!(
                !detail.contains("credential-must-stay-private"),
                "{engine}: {detail}"
            );
        }
        for (grants, expected) in [
            (
                r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]}}"#,
                &b"program-ran"[..],
            ),
            (r#"{"io":{"dirs":[]}}"#, &b""[..]),
        ] {
            let output = launch(grants);
            assert!(output.status.success(), "{engine}: {output:?}");
            assert_eq!(output.stdout, expected);
            assert!(output.stderr.is_empty(), "{engine}: {output:?}");
        }
    }
    std::fs::remove_file(exe).unwrap();
    std::fs::remove_file(source).unwrap();
}
