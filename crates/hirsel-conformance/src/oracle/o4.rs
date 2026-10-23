//! O4, the verified result store: corpus programs served from a warm store
//! give a cold run's artifacts, with every hit recomputed and compared.
//!
//! - `mem hit`: the same program twice over one shared store; the second run
//!   equals a cold run.
//! - `dir recompute`: a populating and a hitting process over one `dir@1`
//!   store under `VerifyStore::Recompute`, where a mismatching hit is a tool
//!   error.
//! - `verify-cache`: check/build through the matching CLI's `--verify-cache`.
//!
//! Conformance and agreement campaigns select `VerifyStore::Recompute`
//! explicitly in their compiler wiring.
use super::{
    Config, Report, Verdict, difference, o1::produce_in_child, produce, produce_with_store,
    switches,
};
use hirsel_engine::store::MemStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn run(exe: &Path, root: &Path, programs: &[PathBuf], work: &Path) -> Report {
    let mut report = Report::new("O4");
    for (index, program) in programs.iter().enumerate() {
        let name = program
            .strip_prefix(root)
            .unwrap_or(program)
            .display()
            .to_string();
        let config = Config::default();
        let cold = match produce(program, &config) {
            Ok(cold) => cold,
            Err(switch) => {
                report.add(format!("{name} [cold]"), Verdict::Todo(switch));
                continue;
            }
        };
        let store = Arc::new(MemStore::default());
        let hit = produce_with_store(program, &config, store.clone())
            .and_then(|_| produce_with_store(program, &config, store));
        report.add(
            format!("{name} [mem hit]"),
            match hit {
                Ok(hit) => match difference(&cold, &hit) {
                    None => Verdict::Pass,
                    Some(why) => Verdict::Fail(format!("hit differs from cold: {why}")),
                },
                Err(switch) => Verdict::Todo(switch),
            },
        );
        let base = work.join(index.to_string());
        let stored = Config {
            store: Some(base.join("store")),
            verify_store: true,
            ..config.clone()
        };
        let verdict = match switches::missing(stored.switches()) {
            Err(switch) => Verdict::Todo(switch),
            Ok(()) => {
                let runs = produce_in_child(exe, program, &stored, &base.join("populate"))
                    .and_then(|_| produce_in_child(exe, program, &stored, &base.join("hit")));
                match runs {
                    Ok(hit) => match difference(&cold, &hit) {
                        None => Verdict::Pass,
                        Some(why) => Verdict::Fail(format!("hit differs from cold: {why}")),
                    },
                    Err(why) => Verdict::Fail(why),
                }
            }
        };
        report.add(format!("{name} [dir recompute]"), verdict);
        report.add(
            format!("{name} [verify-cache]"),
            match cli_verification(program, &base.join("cli")) {
                Ok(()) => Verdict::Pass,
                Err(why) => Verdict::Fail(why),
            },
        );
    }
    report
}

/// An explicit CLI input keeps this oracle on the exact compiler under test.
/// The pool harness supplies it; standalone oracle runs use HIRSEL_ORACLE_CLI.
fn cli_verification(program: &Path, work: &Path) -> Result<(), String> {
    let cli = std::env::var_os("HIRSEL_ORACLE_CLI")
        .ok_or("O4 needs HIRSEL_ORACLE_CLI pointing to the matching hirsel binary")?;
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    for command in ["check", "build"] {
        let run = |store: &str, verify: bool| -> Result<_, String> {
            let mut child = std::process::Command::new(&cli);
            child
                .env("HIRSEL_DAEMON", "off")
                .env("HIRSEL_RESULTS", store)
                .env("HIRSEL_RESULTS_DIR", work.join("results"))
                .env("HIRSEL_OBJECTS", "none@1")
                .env_remove("HIRSEL_VERIFY_CACHE")
                .arg(command);
            if verify {
                child.arg("--verify-cache");
            }
            if command == "check" {
                child.arg("--json");
            } else {
                child
                    .args(["--debug", "--backend", "cranelift", "-o"])
                    .arg(work.join("program"));
            }
            let output = child.arg(program).output().map_err(|e| e.to_string())?;
            if !matches!(output.status.code(), Some(0 | 1)) {
                return Err(format!(
                    "{command}: {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let bytes = if command == "build" && output.status.success() {
                std::fs::read(work.join("program")).map_err(|e| e.to_string())?
            } else {
                vec![]
            };
            Ok((output.status.code(), output.stdout, output.stderr, bytes))
        };
        let cold = run("none@1", false)?;
        let _ = run("dir@1", false)?;
        let warm = run("dir@1", true)?;
        if cold != warm {
            return Err(format!(
                "{command} --verify-cache differs from cold (status, stdout, stderr or executable bytes)"
            ));
        }
    }
    Ok(())
}
