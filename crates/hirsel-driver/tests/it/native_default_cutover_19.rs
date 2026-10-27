//! Public fresh build readiness through the compiler-owned native query graph.
use hirsel_driver::native::{self, Backend, BuildProfile, TargetSpec};
use hirsel_engine::{Verify, with_compiler_stack};

#[test]
fn public_native_build_produces_runnable_outputs_on_both_backends() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        std::fs::write(&source, "fn helper(x:u64)->u64{x+1u64}\npub fn main()->Result[(),u64]{if helper(7u64)==8u64 {Ok(())} else {Error(9u64)}}").unwrap();
        for backend in [Backend::Cranelift, Backend::Llvm] {
            let output = directory.path().join(backend.name());
            let built = native::build_path_native_backend_with_validation(
                &source,
                &output,
                TargetSpec::x86_64_unknown_linux_gnu(),
                BuildProfile::Debug,
                backend,
                None,
                false,
                Verify::On,
                native::Validation::EveryStage,
            )
            .unwrap();
            assert!(built.is_ok(), "{backend:?}: {built:?}");
            assert!(output.is_file(), "completed readiness omitted executable");
            let executed = std::process::Command::new(&output).output().unwrap();
            assert!(
                executed.status.success(),
                "{backend:?}: {}",
                String::from_utf8_lossy(&executed.stderr)
            );
            assert!(executed.stdout.is_empty());
        }
    });
}

#[test]
fn public_native_run_binds_and_launches_on_both_backends() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        std::fs::write(&source, "fn helper(x:u64)->u64{x+1u64}\npub fn main()->Result[(),u64]{if helper(7u64)==8u64 {Ok(())} else {Error(9u64)}}").unwrap();
        let started = std::time::Instant::now();
        for backend in [Backend::Cranelift, Backend::Llvm] {
            let _ = std::io::Write::write_fmt(
                &mut std::io::stderr(),
                format_args!("native default run {backend:?} started\n"),
            );
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            let outcome = native::run_path_native_backend_with_validation(
                &source,
                hirsel_driver::HostLaunch::empty(),
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                Verify::On,
                native::Validation::EveryStage,
            )
            .unwrap()
            .unwrap();
            let _ = std::io::Write::write_fmt(
                &mut std::io::stderr(),
                format_args!("native default run {backend:?} completed after {:?}\n", started.elapsed()),
            );
            assert_eq!(outcome.exit_code(), 0, "{backend:?}: {outcome:?}");
            assert!(stdout.is_empty(), "{backend:?}: {stdout:?}");
            assert!(stderr.is_empty(), "{backend:?}: {stderr:?}");
        }
    });
}

fn public_record_replay(backend: Backend) {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("main.hir");
        let tape = directory.path().join("recording");
        let text = "pub fn main(rt:Io,args:Args)->Result[(),GrantError]{let _=out.print(io.stdout(rt)?,\"cutover replay\\n\");Ok(())}";
        std::fs::write(&source, text).unwrap();
        let run = |launch| {
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            let outcome = native::run_path_native_backend_with_validation(
                &source,
                launch,
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut stdout,
                    stderr: &mut stderr,
                },
                Verify::On,
                native::Validation::EveryStage,
            );
            (outcome, stdout, stderr)
        };
        let launch = hirsel_driver::HostLaunch::Live {
            config: hirsel_driver::LaunchConfig::Grants {
                input: std::ffi::OsStr::new(
                    r#"{"io":{"stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","dirs":[]},"args":[]}"#,
                ),
                arguments: None,
            },
            record: Some(&tape),
            revocations: None,
        };
        let (recorded, stdout, stderr) = run(launch);
        assert_eq!(recorded.unwrap().unwrap().exit_code(), 0);
        assert_eq!(stdout, b"cutover replay\n");
        assert!(stderr.is_empty(), "{backend:?}: {stderr:?}");
        assert!(tape.is_file(), "recording was not published");
        let (replayed, replay_stdout, replay_stderr) =
            run(hirsel_driver::HostLaunch::Replay(&tape));
        assert_eq!(replayed.unwrap().unwrap().exit_code(), 0);
        assert_eq!(replay_stdout, stdout);
        assert_eq!(replay_stderr, stderr);
        std::fs::write(&source, text.replace("cutover replay", "changed replay")).unwrap();
        let (refused, stdout, stderr) = run(hirsel_driver::HostLaunch::Replay(&tape));
        let Err(hirsel_engine::ToolError::Runtime(message)) = refused else {
            panic!("changed source replay was not refused");
        };
        assert!(message.contains("replay refused"), "{message}");
        assert!(
            stdout.is_empty() && stderr.is_empty(),
            "refusal must precede entry"
        );
    });
}

#[test]
fn public_record_replay_cranelift() {
    public_record_replay(Backend::Cranelift);
}

#[test]
fn public_record_replay_llvm() {
    public_record_replay(Backend::Llvm);
}
