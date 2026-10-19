//! `proc.run` through compiled calls: a live run records a granted host program's
//! completions on the tape, and replay returns them with the program deleted, so nothing
//! is spawned. Replay re-derives grants: under rules that no longer name the program, the
//! recorded run refuses (exit 3).
#[path = "../../../hirsel-driver/tests/support/source_std.rs"]
mod source_std;
use hirsel_driver::native::{
    Launched, OptLevel, Options, Partition, Sites, Toolchain, build, link, observe,
};
use hirsel_engine::{check_progress, default_context, entry_path, with_compiler_stack};
use hirsel_lir::layout_types::TargetSpec;
use hirsel_rt::io::{Completion, Request};
use std::path::Path;
use std::process::Command;

/// Runs the program named by argument 0 twice: once to completion and once past a
/// one-byte output bound.
const SOURCE: &str = r#"fn grant(take e: GrantError) -> () {}
fn output(take e: OutputError) -> () {}
fn argument(take e: ArgError) -> () {}

pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let out = io.stdout(rt) ? grant
  let p = io.process(rt) ? grant
  let program = arg.path(args, 0u64) ? argument
  match proc.run(p, Command(program: program, args: ["recorded", "once"], dir: None(), env: None(), own_group: false), b"", 64, None()) {
    Ok(o) => {
      if o.status == Exited(0) && o.stdout == b"recorded once\n" {
        out.print(out, "echo ran\n") ? output
      }
    },
    Error(_) => out.print(out, "echo failed\n") ? output,
  }
  if proc.run(p, Command(program: program, args: ["too long"], dir: None(), env: None(), own_group: false), b"", 1, None()) == Error(OutputLimit()) {
    out.print(out, "bound held\n") ? output
  }
  Ok(())
}
"#;

fn run(executable: &Path, work: &Path, tape: (&str, &Path), grant: &str, echo: &Path) -> Launched {
    let root = work.join("root");
    let record = work.join("observation");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&record);
    std::fs::create_dir_all(&root).unwrap();
    let output = Command::new(executable)
        .arg(echo)
        .env_clear()
        .env("HIRSEL_RT_ROOT", &root)
        .env("HIRSEL_RT_OBSERVATION", &record)
        .env("HIRSEL_RT_DIAGNOSTIC", "1")
        .env("HIRSEL_RT_PROCESS", grant)
        .env(tape.0, tape.1)
        .output()
        .unwrap();
    Launched {
        audit_required: true,
        exit: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        record: std::fs::read_to_string(&record).unwrap_or_default(),
    }
}

#[test]
fn process_runs_record_on_the_host_and_replay_without_spawning_in_smoke() {
    with_compiler_stack(|| {
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("process-native-smoke");
        std::fs::create_dir_all(&scratch).unwrap();
        let scratch = std::fs::canonicalize(&scratch).unwrap();
        let tools = Toolchain::in_tree_diagnostic(&scratch.join("runtime"), &[&target]).unwrap();
        let mut engine = source_std::engine();
        let context = default_context();
        let file = engine
            .sources
            .add("process-run.hir", SOURCE.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&context);
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let sites = Sites::new(input.snapshot());
        let program = build(&input, &Options::new(target.clone())).unwrap();
        let work = scratch.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let executable = link(
            &program,
            &entry,
            OptLevel::Speed,
            Partition::Whole,
            &tools,
            &work,
        )
        .unwrap();
        // A private copy of echo, so deleting it proves replay spawns nothing.
        let echo = work.join("echo");
        std::fs::copy("/bin/echo", &echo).unwrap();
        let grant = format!(
            r#"{{"provider":"process.spawn@1","rules":[{{"program":"{}"}}]}}"#,
            echo.display()
        );
        let tape = work.join("tape");
        let recorded = run(&executable, &work, ("HIRSEL_RT_TAPE", &tape), &grant, &echo);
        let observed = observe(&recorded, &program, &sites).unwrap();
        assert_eq!(observed.exit_code, Some(0), "{recorded:?}");
        assert!(observed.failures.is_empty(), "{observed:?}");
        assert_eq!(
            String::from_utf8_lossy(&observed.output),
            "echo ran\nbound held\n"
        );
        let recording =
            hirsel_rt::launch::parse_recording(&std::fs::read_to_string(&tape).unwrap()).unwrap();
        let runs: Vec<_> = recording
            .events
            .iter()
            .filter(|e| matches!(e.request, Request::ProcessRun { .. }))
            .map(|e| e.completion.clone())
            .collect();
        assert_eq!(runs.len(), 2, "{:?}", recording.events);
        assert!(
            runs.iter().all(|c| matches!(c, Completion::Process(_))),
            "{runs:?}"
        );
        std::fs::remove_file(&echo).unwrap();
        let replayed = run(
            &executable,
            &work,
            ("HIRSEL_RT_REPLAY", &tape),
            &grant,
            &echo,
        );
        assert_eq!(
            observe(&replayed, &program, &sites).unwrap(),
            observed,
            "{replayed:?}"
        );
        let refused = run(
            &executable,
            &work,
            ("HIRSEL_RT_REPLAY", &tape),
            r#"{"provider":"process.spawn@1","rules":[]}"#,
            &echo,
        );
        assert_eq!(refused.exit, Some(3), "{refused:?}");
    });
}
