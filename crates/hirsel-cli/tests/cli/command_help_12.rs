//! Short, long and subcommand help controls adapted from clap
//! tests/builder/help.rs:20–56, ccc89b83875474399e9d9d70841163f6319d1a41.
//! MIT, Copyright (c) Individual contributors; see third-party/NOTICES.md.
//! Hirsel uses process exit/stdout/stderr assertions in place of DisplayHelp,
//! and retains its own root-help and operand ownership rules.
//! Compact error/usage/help and trailing-newline vectors also adapt
//! tests/builder/error.rs:1–65 at the same revision and under the same license.
use std::{fs, path::PathBuf, process::Command};

const TOPICS: [&str; 5] = ["check", "run", "build", "test", "fmt"];

struct PrivateCwd(PathBuf);
impl PrivateCwd {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hirsel-command-help-12-{}-{}",
            std::process::id(),
            super::NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(path)
    }
    fn command(&self) -> Command {
        let executable = fs::canonicalize(env!("CARGO_BIN_EXE_hirsel")).unwrap();
        let mut command = Command::new(executable);
        // No inherited toolchain, std, grants, daemon or cache configuration.
        // Redirect every potential output location into this empty directory.
        command.env_clear().current_dir(&self.0);
        for name in ["HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR", "TMPDIR"] {
            command.env(name, &self.0);
        }
        command
    }
    fn help(&self, args: &[&str], topic: &str) -> Vec<u8> {
        let output = self.command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            text.starts_with(&format!("usage: hirsel {topic} ")),
            "{text}"
        );
        assert!(
            text.contains(&format!("Example: hirsel {topic} ")),
            "{text}"
        );
        assert!(text.contains("<source|manifest.json>"), "{text}");
        assert_eq!(
            fs::read_dir(&self.0).unwrap().count(),
            0,
            "{args:?} wrote files"
        );
        output.stdout
    }
    fn usage_error(&self, args: &[&str]) -> Vec<u8> {
        let output = self
            .command()
            .env("HIRSEL_DAEMON", "off")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage: hirsel "));
        assert_eq!(
            fs::read_dir(&self.0).unwrap().count(),
            0,
            "{args:?} wrote files"
        );
        output.stderr
    }
    fn compact_usage_error(&self, args: &[&str], topic: &str, error: &str) {
        let help = String::from_utf8(self.help(&[topic, "--help"], topic)).unwrap();
        let usage = help.lines().next().unwrap();
        assert_eq!(
            self.usage_error(args),
            format!("{error}\n{usage}\n\nFor more information, try 'hirsel {topic} --help'.\n")
                .as_bytes(),
            "{args:?}"
        );
    }
}
impl Drop for PrivateCwd {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn command_help_12_subcommand_matches_flag_forms() {
    let cwd = PrivateCwd::new();
    for topic in TOPICS {
        let expected = cwd.help(&[topic, "--help"], topic);
        assert_eq!(cwd.help(&[topic, "-h"], topic), expected);
        assert_eq!(cwd.help(&["help", topic], topic), expected);
    }
}

#[test]
fn command_help_12_options_and_examples_follow_the_parser() {
    let cwd = PrivateCwd::new();
    for (topic, required) in [
        (
            "check",
            vec!["--json | --safe", "--watch", "--verify-cache"],
        ),
        (
            "run",
            vec![
                "--native",
                "--backend llvm|cranelift",
                "--grants",
                "--run",
                "--replay",
                "-- args...",
                "--hirsel-record",
                "--hirsel-grants",
                "docs/native-verification.md",
            ],
        ),
        (
            "build",
            vec![
                "--release | --debug",
                "--lto",
                "--backend llvm|cranelift",
                "-o <exe>",
                "--target x86_64-linux|aarch64-linux",
                "--fuel",
            ],
        ),
        (
            "test",
            vec![
                "--native",
                "--backend llvm|cranelift",
                "--bench",
                "--bench-time",
                "--scheduler",
                "--explore",
                "--race",
                "--sim-points",
                "[filter...]",
            ],
        ),
    ] {
        let bytes = cwd.help(&[topic, "--help"], topic);
        let text = String::from_utf8(bytes).unwrap();
        for option in required {
            assert!(text.contains(option), "{topic}: missing {option}: {text}");
        }
        for unsupported in ["--help-all", "--version", "--output", "--opt-level"] {
            assert!(!text.contains(unsupported), "{topic}: {unsupported}");
        }
    }
}

#[test]
fn command_help_12_unknown_extra_and_incompatible_flags_are_usage_errors() {
    let cwd = PrivateCwd::new();
    for args in [
        vec!["help", "unknown"],
        vec!["help", "serve"],
        vec!["help", "build", "extra"],
        vec!["help", "build", "--help"],
        vec!["build", "--help", "extra.hir"],
        vec!["run", "--help", "--native"],
        vec!["check", "--help", "--json"],
        vec!["test", "-h", "extra"],
        vec!["fmt", "--help", "extra.hir"],
        vec!["build", "--help=extra"],
        vec!["build", "--release", "--backend", "cranelift", "--help"],
        vec!["run", "--backend", "llvm", "--help"],
        vec!["check", "--no-verify", "--help"],
        vec!["test", "--verify-cache", "--help"],
        vec!["run", "app.hir", "--help"],
        vec!["build", "app.hir", "-h"],
        vec!["check", "app.hir", "--help"],
        vec!["test", "app.hir", "--help"],
    ] {
        cwd.usage_error(&args);
    }
}

#[test]
fn command_help_12_missing_operands_remain_usage_errors() {
    let cwd = PrivateCwd::new();
    for topic in TOPICS {
        cwd.usage_error(&[topic]);
    }
    cwd.usage_error(&[]);
    cwd.usage_error(&["build", "app.hir"]);
}

#[test]
fn command_help_12_backend_errors_show_only_the_failing_command_usage() {
    let cwd = PrivateCwd::new();
    for topic in ["test", "run"] {
        cwd.compact_usage_error(
            &[topic, "app.hir", "--backend", "llvm"],
            topic,
            "--backend requires --native",
        );
    }
    // The original dogfood invocation fails before requiring a source.
    cwd.compact_usage_error(
        &["test", "--backend", "llvm"],
        "test",
        "--backend requires --native",
    );
    for topic in ["test", "run", "build"] {
        cwd.compact_usage_error(
            &[topic, "app.hir", "--backend", "invalid"],
            topic,
            "--backend must be llvm or cranelift",
        );
        cwd.compact_usage_error(
            &[topic, "app.hir", "--backend", "llvm", "--backend", "llvm"],
            topic,
            "invalid option '--backend'",
        );
        cwd.compact_usage_error(
            &[topic, "app.hir", "--backend"],
            topic,
            "missing argument for option '--backend'",
        );
    }
}

#[test]
fn command_help_12_operand_and_global_errors_keep_command_usage() {
    let cwd = PrivateCwd::new();
    for (args, topic, error) in [
        (vec!["check"], "check", "missing source path"),
        (vec!["run"], "run", "missing source path"),
        (
            vec!["build"],
            "build",
            "build requires a source or manifest",
        ),
        (vec!["test"], "test", "test requires a package manifest"),
        (vec!["build", "app.hir"], "build", "build requires -o"),
        (
            vec!["build", "app.hir", "-o"],
            "build",
            "missing argument for option '-o'",
        ),
        (
            vec!["check", "--bad", "app.hir"],
            "check",
            "invalid option '--bad'",
        ),
        (
            vec!["--verbose", "test", "--backend", "llvm"],
            "test",
            "--backend requires --native",
        ),
        (
            vec!["test", "--verify-cache"],
            "test",
            "--verify-cache applies only to check, build and run",
        ),
        (
            vec!["check", "--no-verify", "app.hir"],
            "check",
            "--no-verify applies only to run, test and build",
        ),
        (
            vec!["run", "--timings=invalid"],
            "run",
            "invalid option '--timings=invalid': expected --timings or --timings=json",
        ),
        (
            vec!["build", "--help", "extra.hir"],
            "build",
            "command help takes no extra operands or options",
        ),
    ] {
        cwd.compact_usage_error(&args, topic, error);
    }
}

#[test]
fn command_help_12_unknown_and_missing_commands_keep_root_usage() {
    let cwd = PrivateCwd::new();
    let root = cwd.command().arg("--help").output().unwrap();
    assert_eq!(root.status.code(), Some(0));
    assert!(root.stderr.is_empty());
    for (args, error) in [
        (vec![], "missing command"),
        (vec!["unknown"], "unexpected argument \"unknown\""),
        (vec!["unknown", "test"], "unexpected argument \"unknown\""),
        (vec!["help", "unknown"], "unknown help topic"),
    ] {
        let mut expected = format!("{error}\n").into_bytes();
        expected.extend_from_slice(&root.stdout);
        assert_eq!(cwd.usage_error(&args), expected, "{args:?}");
    }
}

#[test]
fn command_help_12_source_named_help_remains_a_source() {
    let cwd = PrivateCwd::new();
    fs::write(cwd.0.join("help"), "fn  example ( ) { }\n").unwrap();
    let output = cwd.command().args(["fmt", "help"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        fs::read_to_string(cwd.0.join("help")).unwrap(),
        "fn example() {}\n"
    );
}

#[test]
fn command_help_12_run_app_boundary_preserves_help_arguments() {
    let cwd = PrivateCwd::new();
    fs::write(
        cwd.0.join("app.hir"),
        r#"pub fn main(args: Args) -> Result[(), ()] {
  failure.check(arg.len(args) == 3, Arithmetic())
  failure.check(arg.text(args, 0) == Ok("--help"), Arithmetic())
  failure.check(arg.text(args, 1) == Ok("-h"), Arithmetic())
  failure.check(arg.text(args, 2) == Ok("help"), Arithmetic())
  Ok(())
}"#,
    )
    .unwrap();
    // An explicit std-free manifest isolates the operand test from toolchain
    // discovery. This fixture is interpreted once, never rebuilt for help.
    fs::write(cwd.0.join("app.json"), r#"{"schema":1,"package":{"name":"app","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"app.hir","module":[]}],"dependencies":[]}"#).unwrap();
    let output = cwd
        .command()
        .env("HIRSEL_DAEMON", "off")
        .args(["run", "app.json", "--", "--help", "-h", "help"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}
