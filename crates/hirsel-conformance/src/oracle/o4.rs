//! O4, the verified result store: corpus programs served from a warm store
//! give a cold run's artifacts, with every hit recomputed and compared.
//!
//! - `mem hit`: the same program twice over one shared store; the second run
//!   equals a cold run (today's `PackageCache`, which `mem@1` replaces).
//! - `dir recompute`: a populating and a hitting process over one `dir@1`
//!   store under `VerifyStore::Recompute`, where a mismatching hit is a tool
//!   error (TODO until the store exists).
//! - `verify-cache`: the same through the CLI's `--verify-cache` (TODO).
//!
//! The gate campaigns themselves (conformance and agreement shards) run under
//! `VerifyStore::Recompute` once it exists; `tools/buck2/sync.py` then passes
//! the switch to their targets.
use super::{
    Config, Report, Switch, Verdict, difference, fresh_cache, o1::produce_in_child, produce,
    switches,
};
use std::path::{Path, PathBuf};

pub fn run(exe: &Path, root: &Path, programs: &[PathBuf], work: &Path) -> Report {
    let mut report = Report::new("O4");
    for (index, program) in programs.iter().enumerate() {
        let name = program
            .strip_prefix(root)
            .unwrap_or(program)
            .display()
            .to_string();
        let config = Config::default();
        let cold = match produce(program, &config, &fresh_cache()) {
            Ok(cold) => cold,
            Err(switch) => {
                report.add(format!("{name} [cold]"), Verdict::Todo(switch));
                continue;
            }
        };
        let shared = fresh_cache();
        let hit =
            produce(program, &config, &shared).and_then(|_| produce(program, &config, &shared));
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
        // `hirsel check --json --verify-cache` and `hirsel build --verify-cache`
        // twice over one store, each output equal to the cold run's.
        report.add(
            format!("{name} [verify-cache]"),
            Verdict::Todo(Switch::VerifyCache),
        );
    }
    report
}
