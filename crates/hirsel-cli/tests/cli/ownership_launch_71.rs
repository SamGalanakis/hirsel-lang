//! Native ownership diagnostics use the ordinary admitted launch and tape.
use super::*;
use hirsel_run_schema::ownership_trace::Trace;

#[test]
fn native_ownership_record_still_requires_an_explicit_launch() {
    let path = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    let directory = tempfile::tempdir().unwrap();
    let tape = directory.path().join("missing-launch.tape");
    let sidecar = directory.path().join("ownership.json");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&path)
        .arg("--native")
        .arg("--record")
        .arg(&tape)
        .arg("--trace-ownership")
        .arg(&sidecar)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--record requires --run or --grants"),
        "{output:?}"
    );
    assert!(output.stdout.is_empty());
    assert!(!tape.exists());
    assert!(!sidecar.exists());
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn native_ownership_accepts_explicit_empty_grants_for_the_original_grant_free_vector() {
    let path = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    let directory = tempfile::tempdir().unwrap();
    for backend in ["llvm", "cranelift"] {
        let sidecar = directory.path().join(format!("{backend}.json"));
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&path)
            .args(["--native", "--backend", backend, "--grants", "{}"])
            .arg("--trace-ownership")
            .arg(&sidecar)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{backend}: {output:?}");
        assert!(output.stdout.is_empty());
        let trace: Trace = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
        let analysis = trace.analyze(trace.program, 1 << 20).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (0, 0, 0)
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn native_ownership_grants_args_record_and_replay_keep_actual_child_lifetimes() {
    let path = file(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io,args:Args)->Result[(),GrantError] {
 failure.check(arg.len(args)==1u64,Arithmetic())
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let first=task.async(s,fn(){let token=Token()})
  let second=task.async(s,fn(){let token=Token()})
  let _=task.await(s,take first)
  let _=task.await(s,take second)
 })
 Ok(())
}
"#,
    );
    let directory = tempfile::tempdir().unwrap();
    let grants = r#"{"args":[],"io":{"tasks":"tasks.cooperative@1","dirs":[]}}"#;
    let mut expected_program = None;
    for backend in ["llvm", "cranelift"] {
        let tape = directory.path().join(format!("{backend}.tape"));
        let mut recorded = None;
        for mode in ["record", "native-replay", "reference-replay"] {
            let sidecar = directory.path().join(format!("{backend}-{mode}.json"));
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run").arg(&path);
            if mode != "reference-replay" {
                command.args(["--native", "--backend", backend]);
            }
            command.arg("--trace-ownership").arg(&sidecar);
            if mode == "record" {
                command
                    .args(["--grants", grants, "--record"])
                    .arg(&tape)
                    .args(["--", "kept"]);
            } else {
                command.arg("--replay").arg(&tape);
            }
            let output = command.output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(0),
                "{backend}/{mode}: {output:?}"
            );
            assert!(output.stdout.is_empty(), "{output:?}");
            let trace: Trace = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            let analysis = trace.analyze(trace.program, 1 << 20).unwrap();
            assert_eq!(
                (analysis.created, analysis.dropped, analysis.live.len()),
                (2, 2, 0)
            );
            let paths = trace
                .events
                .iter()
                .map(|event| event.task.clone())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(paths.len(), 2);
            assert!(paths.iter().all(|path| !path.0.is_empty()));
            match expected_program {
                None => expected_program = Some(trace.program),
                Some(program) => assert_eq!(trace.program, program),
            }
            let bytes = std::fs::read(&tape).unwrap();
            if mode == "record" {
                recorded = Some(bytes);
            } else {
                assert_eq!(
                    &bytes,
                    recorded.as_ref().unwrap(),
                    "replay changed its tape"
                );
            }
        }
    }
    std::fs::remove_file(path).unwrap();
}
