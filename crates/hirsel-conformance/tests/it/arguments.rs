//! Argument failures are tooling outcomes, independent of corpus execution.
use std::process::Command;

#[test]
fn common_argument_failures_have_the_same_message_and_exit_code() {
    for (args, message) in [
        (vec!["--root"], "--root requires a path\n"),
        (
            vec!["--hash-seed"],
            "--hash-seed requires an unsigned integer\n",
        ),
        (
            vec!["--hash-seed", "-1"],
            "--hash-seed requires an unsigned integer\n",
        ),
        (
            vec!["--shard"],
            "--shard requires <index>/<count>, 1 <= index <= count\n",
        ),
        (
            vec!["--shard", "0/2"],
            "--shard requires <index>/<count>, 1 <= index <= count\n",
        ),
    ] {
        for binary in [
            env!("CARGO_BIN_EXE_hirsel-conformance"),
            // Native and agree join this check when their parser migrations land.
        ] {
            let output = Command::new(binary).args(&args).output().unwrap();
            assert_eq!(output.status.code(), Some(2), "{binary}: {args:?}");
            assert!(output.stdout.is_empty(), "{binary}: {args:?}");
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                message,
                "{binary}: {args:?}"
            );
        }
    }
    let output = Command::new(env!("CARGO_BIN_EXE_profile"))
        .args(["program.hir", "--root"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, b"--root requires a path\n");
    assert!(output.stdout.is_empty());
}

#[test]
fn profile_requires_separator_before_program_arguments() {
    let output = Command::new(env!("CARGO_BIN_EXE_profile"))
        .args(["program.hir", "argument"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument"));
    // Parsing succeeds after --, including arguments that look like profile
    // options. The missing source then fails as IO, before any native build.
    let output = Command::new(env!("CARGO_BIN_EXE_profile"))
        .args([
            "missing-program.hir",
            "--output",
            "/dev/null",
            "--",
            "--root",
            "--not-profile-option",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("profile: "));
}
