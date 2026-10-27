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

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn native_ownership_compiles_the_selected_seeds_and_scheduler_before_recording_and_replay() {
    let path = file(
        r#"
resource type Token {}
drop(x) {}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 task.scope(tasks,fn(s:Scope) {
  let child=task.async(s,fn(){let first=Token()
   let second=Token()})
  let _=task.await(s,take child)
 })
 Ok(())
}
"#,
    );
    let directory = tempfile::tempdir().unwrap();
    let grants = r#"{"inputs":{"hash_seed":13,"random_seed":17,"test_seed":19,"scheduler":{"algorithm":"random","version":1,"seed":23}},"io":{"tasks":"tasks.cooperative@1","dirs":[]}}"#;
    for backend in ["llvm", "cranelift"] {
        let tape = directory.path().join(format!("{backend}.tape"));
        let mut original_tape = None;
        for mode in ["record", "native-replay", "reference-replay"] {
            let sidecar = directory.path().join(format!("{backend}-{mode}.json"));
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command
                .arg("run")
                .arg(&path)
                .arg("--trace-ownership")
                .arg(&sidecar);
            if mode != "reference-replay" {
                command.args(["--native", "--backend", backend]);
            }
            if mode == "record" {
                command.args(["--grants", grants, "--record"]).arg(&tape);
            } else {
                command.arg("--replay").arg(&tape);
            }
            let output = command.output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(0),
                "{backend}/{mode}: {output:?}"
            );
            assert!(output.stdout.is_empty());
            let trace: Trace = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            let analysis = trace.analyze(trace.program, 1 << 20).unwrap();
            assert_eq!(
                (analysis.created, analysis.dropped, analysis.live.len()),
                (2, 2, 0)
            );
            let (_, run) = hirsel_rt::host_launch::replay(&tape).unwrap();
            assert_eq!(
                (
                    run.record().inputs.hash_seed,
                    run.record().inputs.random_seed,
                    run.record().inputs.test_seed
                ),
                (13, 17, 19)
            );
            let scheduler = run.record().inputs.scheduler.as_ref().unwrap();
            assert_eq!(
                (
                    scheduler.algorithm.as_str(),
                    scheduler.version,
                    scheduler.seed
                ),
                ("random", 1, 23)
            );
            let bytes = std::fs::read(&tape).unwrap();
            if mode == "record" {
                original_tape = Some(bytes);
            } else {
                assert_eq!(&bytes, original_tape.as_ref().unwrap());
            }
        }
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore = "requires diagnostic runtime: -c hirsel.ownership_trace=true -- --test-arg=--include-ignored"]
fn native_ownership_keeps_the_admitted_fuel_checks_while_tracing_loop_lifetimes() {
    let path = file(
        r#"
resource type Token {}
drop(x) {}
pub fn main()->Result[(),()] {
 let mut i=0u64
 while i<8u64 {
  let token=Token()
  i=i+1u64
 }
 Ok(())
}
"#,
    );
    let directory = tempfile::tempdir().unwrap();
    let grants = r#"{"budgets":{"provider":"budgets.limits@2","fuel":1000000}}"#;
    for backend in ["llvm", "cranelift"] {
        let sidecar = directory.path().join(format!("{backend}.json"));
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&path)
            .args(["--native", "--backend", backend, "--grants", grants])
            .arg("--trace-ownership")
            .arg(&sidecar)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{backend}: {output:?}");
        let trace: Trace = serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap();
        let analysis = trace.analyze(trace.program, 1 << 20).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (8, 8, 0)
        );
    }
    std::fs::remove_file(path).unwrap();
}
