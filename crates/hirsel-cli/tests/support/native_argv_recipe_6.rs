//! Launch the shipped recipe, using the examples policy fixture's temporary root.
use super::*;
use std::{os::unix::fs::PermissionsExt, process::Output};

fn fixture() -> Example {
    let example = Example::new(
        "native argv recipe 6",
        &[(
            "main.hir",
            include_str!("../../../../examples/argv-contract/main.hir"),
        )],
        include_str!("../../../../examples/argv-contract/hirsel.json"),
    );
    std::fs::write(
        example.dir.join("grants.json"),
        include_str!("../../../../examples/argv-contract/grants.json"),
    )
    .unwrap();
    std::fs::write(
        example.dir.join("launch.sh"),
        include_str!("../../../../examples/argv-contract/launch.sh"),
    )
    .unwrap();
    std::fs::set_permissions(
        example.dir.join("launch.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    example
}

fn observed(output: Output) -> Outcome {
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

// Independent vectors, including Go exec_test.go:285's embedded-space vector.
const CASES: &[(&[&str], &str)] = &[
    (&[], "[]\n"),
    (&["foo bar"], "[\"foo bar\"]\n"),
    (&[""], "[\"\"]\n"),
    (&["--follow"], "[\"--follow\"]\n"),
    (&["羊", "café"], "[\"羊\",\"café\"]\n"),
    (&["foo bar", "baz"], "[\"foo bar\",\"baz\"]\n"),
    (
        &["a\"b", "c\\d", "line\nbreak", "\t"],
        "[\"a\\\"b\",\"c\\\\d\",\"line\\nbreak\",\"\\t\"]\n",
    ),
    (
        &["--hirsel-grants", "literal.json"],
        "[\"--hirsel-grants\",\"literal.json\"]\n",
    ),
    (
        &["foo bar", "", "--follow", "羊", "baz"],
        "[\"foo bar\",\"\",\"--follow\",\"羊\",\"baz\"]\n",
    ),
    (&["--", "--follow"], "[\"--\",\"--follow\"]\n"),
    (
        &["$HOME", "$(printf injected)", "*", "x;y"],
        "[\"$HOME\",\"$(printf injected)\",\"*\",\"x;y\"]\n",
    ),
];

fn run(example: &Example, backend: Option<&str>, args: Option<&[&str]>) -> Outcome {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command.arg("run").arg(example.dir.join("hirsel.json"));
    if let Some(backend) = backend {
        command.args(["--native", "--backend", backend]);
    }
    command.arg("--grants").arg(example.dir.join("grants.json"));
    if let Some(args) = args {
        command.arg("--").args(args);
    }
    observed(command.output().unwrap())
}

fn build(example: &Example, backend: &str, name: &str) -> PathBuf {
    let exe = example.dir.join(name);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["build", "--backend", backend])
        .arg(example.dir.join("hirsel.json"))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap();
    assert!(output.status.success(), "{backend}: {output:?}");
    exe
}

#[test]
fn native_argv_recipe_6_interpreted() {
    let example = fixture();
    assert_eq!(
        run(&example, None, None),
        (
            0,
            "[\"TEMPLATE ONLY\",\"template-tail\"]\n".into(),
            String::new()
        )
    );
    for &(args, expected) in CASES {
        assert_eq!(
            run(&example, None, Some(args)),
            (0, expected.into(), String::new()),
            "{args:?}"
        );
    }
    // No stdout authority is implied by explicit Args.
    let denied = observed(
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(example.dir.join("hirsel.json"))
            .args(["--", "argument"])
            .output()
            .unwrap(),
    );
    assert_eq!(denied.0, 3, "{denied:?}");
    assert!(denied.1.is_empty());
}

fn native(backend: &str) {
    let example = fixture();
    let exe = build(&example, backend, "argv-contract");
    let grants = example.dir.join("grants.json");
    // Executing the actual shipped script also tests its path resolution from a
    // different working directory. The fixture grants only stdout and Args.
    for &(args, expected) in CASES {
        let expected = (0, expected.to_string(), String::new());
        assert_eq!(
            run(&example, Some(backend), Some(args)),
            expected,
            "run {backend} {args:?}"
        );
        assert_eq!(
            example.launch(&exe, &grants, args),
            expected,
            "built {backend} {args:?}"
        );
        let wrapped = observed(
            Command::new(example.dir.join("launch.sh"))
                .args(args)
                .current_dir("/")
                .env_clear()
                .output()
                .unwrap(),
        );
        assert_eq!(wrapped, expected, "wrapper {backend} {args:?}");
    }
    assert_eq!(
        run(&example, Some(backend), None),
        (
            0,
            "[\"TEMPLATE ONLY\",\"template-tail\"]\n".into(),
            String::new()
        )
    );
    let denied = observed(
        Command::new(&exe)
            .args(["--", "argument"])
            .env_clear()
            .output()
            .unwrap(),
    );
    assert_eq!(denied.0, 3, "{denied:?}");
    assert!(denied.1.is_empty());
    std::fs::write(&grants, r#"{"io":{"dirs":[]},"args":["TEMPLATE ONLY"]}"#).unwrap();
    let denied = example.launch(&exe, &grants, &[]);
    assert_eq!(
        denied,
        (1, String::new(), "error: Grant(NotGranted)\n".into())
    );
    let wrapped_denied = observed(
        Command::new(example.dir.join("launch.sh"))
            .env_clear()
            .output()
            .unwrap(),
    );
    assert_eq!(wrapped_denied, denied);
    assert_eq!(run(&example, Some(backend), Some(&[])), denied);
    std::fs::write(
        &grants,
        include_str!("../../../../examples/argv-contract/grants.json"),
    )
    .unwrap();
    let tape = example.dir.join("argv.tape");
    let recorded = observed(
        Command::new(&exe)
            .arg("--hirsel-record")
            .arg(&tape)
            .arg("--hirsel-grants")
            .arg(&grants)
            .args(CASES[8].0)
            .env_clear()
            .output()
            .unwrap(),
    );
    assert_eq!(recorded, (0, CASES[8].1.into(), String::new()));
    std::fs::remove_file(&grants).unwrap();
    let replayed = observed(
        Command::new(&exe)
            .arg("--hirsel-replay")
            .arg(&tape)
            .env_clear()
            .output()
            .unwrap(),
    );
    assert_eq!(replayed, recorded);
    let replayed = observed(
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(example.dir.join("hirsel.json"))
            .arg("--replay")
            .arg(&tape)
            .output()
            .unwrap(),
    );
    assert_eq!(replayed, recorded);

    // The motivating app still requires its explicit path. No fallback to the
    // recognizable nonempty template is allowed at native launch.
    let log = Example::new(
        "native_argv_recipe_6_required",
        &[(
            "main.hir",
            include_str!("../../../../examples/logdigest/main.hir"),
        )],
        include_str!("../../../../examples/logdigest/hirsel.json"),
    );
    let log_exe = build(&log, backend, "logdigest");
    let log_grants = log.dir.join("grants.json");
    std::fs::write(
        &log_grants,
        r#"{"io":{"dirs":[]},"args":["app.log","--follow"]}"#,
    )
    .unwrap();
    assert_eq!(
        log.launch(&log_exe, &log_grants, &[]),
        (1, String::new(), "error: Usage\n".into())
    );
    assert_eq!(
        run(&log, Some(backend), Some(&[])),
        (1, String::new(), "error: Usage\n".into())
    );
    assert_eq!(
        run(&log, None, Some(&[])),
        (1, String::new(), "error: Usage\n".into())
    );
    println!(
        "{backend}: {} vectors x 3 routes; template retention, 4 denials, 2 replays, 3 required-argument errors checked",
        CASES.len()
    );
}

#[test]
fn native_argv_recipe_6_llvm() {
    native("llvm");
}

#[test]
fn native_argv_recipe_6_cranelift() {
    native("cranelift");
}
