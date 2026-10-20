//! O2, warm equals cold: after each scripted edit, every warm run's artifacts
//! equal a cold run of the edited tree.
//!
//! - `cold`: a fresh run of the edited tree (`none@1`).
//! - `mem`: one process-shared store across the edit (`mem@1` once loading
//!   consults the result store).
//! - `resident`: one `Session` across the edit (TODO until it exists).
//! - `disk`: two processes sharing a `dir@1` store (TODO until it exists).
use super::{
    Artifacts, Config, Report, Switch, Verdict, difference, edits::Scenario, fresh_prechecks,
    o1::produce_in_child, produce, switches,
};
use std::path::Path;

pub fn run(exe: &Path, root: &Path, scenarios: &[Scenario], work: &Path) -> Report {
    let mut report = Report::new("O2");
    for scenario in scenarios {
        let base = work.join(&scenario.name);
        let _ = std::fs::remove_dir_all(&base);
        if let Err(why) = one(exe, root, scenario, &base, &mut report) {
            report.add(format!("{}: setup", scenario.name), Verdict::Fail(why));
        }
    }
    report
}

fn one(
    exe: &Path,
    root: &Path,
    scenario: &Scenario,
    base: &Path,
    report: &mut Report,
) -> Result<(), String> {
    let tree = base.join("tree");
    let (before, after) = (&scenario.config_before, &scenario.config_after);
    scenario.write(&tree, root, false)?;
    let entry = scenario.entry(&tree);
    let mem = fresh_prechecks();
    let warmed = produce(&entry, before, &mem);
    let disk = base.join("store");
    let disk_before = Config {
        store: Some(disk.clone()),
        ..before.clone()
    };
    let disk_warmed = switches::missing(disk_before.switches())
        .map_err(Verdict::Todo)
        .and_then(|()| {
            produce_in_child(exe, &entry, &disk_before, &base.join("disk-before"))
                .map_err(Verdict::Fail)
        });
    scenario.write(&tree, root, true)?;
    let cold = match produce(&entry, after, &fresh_prechecks()) {
        Ok(cold) => cold,
        Err(switch) => {
            for mode in ["mem", "resident", "disk"] {
                report.add(format!("{} [{mode}]", scenario.name), Verdict::Todo(switch));
            }
            return Ok(());
        }
    };
    // Kept for reading under `--work`.
    super::write_artifacts(&base.join("cold"), &cold).map_err(|e| e.to_string())?;
    let name = |mode: &str| format!("{} [{mode}]", scenario.name);
    let compare = |warm: Result<Artifacts, Verdict>| match warm {
        Ok(warm) => match difference(&cold, &warm) {
            None => Verdict::Pass,
            Some(why) => Verdict::Fail(format!("warm differs from cold: {why}")),
        },
        Err(verdict) => verdict,
    };
    report.add(
        name("mem"),
        compare(
            warmed
                .map_err(Verdict::Todo)
                .and_then(|_| produce(&entry, after, &mem).map_err(Verdict::Todo)),
        ),
    );
    // The resident run: `Session::open` on the before tree, `check`, `apply` the
    // edit's file changes, `check` again, and compare that output.
    report.add(name("resident"), Verdict::Todo(Switch::Session));
    let disk_after = Config {
        store: Some(disk),
        ..after.clone()
    };
    report.add(
        name("disk"),
        compare(disk_warmed.and_then(|_| {
            produce_in_child(exe, &entry, &disk_after, &base.join("disk-after"))
                .map_err(Verdict::Fail)
        })),
    );
    Ok(())
}
