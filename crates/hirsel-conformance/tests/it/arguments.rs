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
            env!("CARGO_BIN_EXE_native"),
            env!("CARGO_BIN_EXE_agree"),
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
    for (args, message) in [
        (vec!["--root"], "--root requires a path\n"),
        (
            vec!["--shard", "0/2"],
            "--shard requires <index>/<count>, 1 <= index <= count\n",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_native-select"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stderr, message.as_bytes());
        assert!(output.stdout.is_empty());
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

// Stop parsing at an unknown sentinel before any campaign or filesystem work.
// The error must name the sentinel, proving every preceding flag was consumed.
#[test]
fn native_diet_flags_reach_the_end_of_argument_parsing() {
    for (binary, args) in [
        (
            env!("CARGO_BIN_EXE_agree"),
            vec![
                "--seed",
                "7",
                "--hash-seed",
                "0",
                "--limit",
                "0",
                "--root",
                "checkout",
                "--native",
                "heap",
                "--native",
                "full",
                "--program",
                "program.hir",
                "--path",
                "one",
                "--path",
                "two",
                "--shard",
                "2/3",
                "--variants-per-program",
                "1",
                "--exhaustive",
            ],
        ),
        (
            env!("CARGO_BIN_EXE_native"),
            vec![
                "--full",
                "--smoke",
                "--sanitize",
                "--hash-sweep",
                "--replay",
                "artifact",
                "--reduce",
                "artifact",
                "--exhaustive",
                "--seed",
                "7",
                "--hash-seed",
                "0",
                "--root",
                "checkout",
                "--out",
                "output",
                "--filter",
                "one",
                "--filter",
                "two",
                "--shard",
                "2/3",
                "--bless",
                "--jobs",
                "2",
                "--engine",
                "heap",
                "--engine",
                "llvm",
                "--engine",
                "diagnostic",
                "--engine",
                "default",
                "--engine",
                "system",
            ],
        ),
        (
            env!("CARGO_BIN_EXE_native-select"),
            vec![
                "--root",
                "checkout",
                "--program",
                "program.hir",
                "--derive",
                "selection.json",
                "--replay-history",
                "--inventory-input",
                "inventory.json",
                "--inventory-output",
                "output.json",
                "--merge",
                "one.json",
                "--merge",
                "two.json",
                "--filter",
                "one",
                "--filter",
                "two",
                "--shard",
                "2/3",
                "--seed",
                "7",
            ],
        ),
    ] {
        let output = Command::new(binary)
            .args(args)
            .arg("--parser-sentinel")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{binary}");
        assert!(output.stdout.is_empty(), "{binary}");
        assert_eq!(
            output.stderr, b"invalid option '--parser-sentinel'\n",
            "{binary}"
        );
    }
}

#[test]
fn agreement_budget_and_selection_constraints_survive_parsing() {
    let binary = env!("CARGO_BIN_EXE_agree");
    for (args, message) in [
        (
            vec!["--variants-per-program", "0"],
            "--variants-per-program needs a positive integer\n",
        ),
        (
            vec!["--program", "missing.hir", "--shard", "1/2"],
            "--shard cannot be combined with --program\n",
        ),
    ] {
        let output = Command::new(binary).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(output.stderr, message.as_bytes());
    }
    let output = Command::new(binary)
        .args(["--variants-per-program", "1", "--shard", "1/2", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(output.stdout.starts_with(b"usage: agree"));
    let output = Command::new(env!("CARGO_BIN_EXE_native"))
        .args(["--full", "--bless", "--engine", "heap"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, b"native: --bless requires --exhaustive\n");
    let output = Command::new(env!("CARGO_BIN_EXE_native"))
        .args(["--full", "--bless", "--exhaustive", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(output.stdout.starts_with(b"usage: native"));
}
