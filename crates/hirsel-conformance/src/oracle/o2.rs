//! O2, warm equals cold: after each scripted edit, every warm run's artifacts
//! equal a cold run of the edited tree.
//!
//! - `cold`: a fresh run of the edited tree (`none@1`).
//! - `mem`: one result store across fresh sessions on either side of the edit.
//! - `resident`: one `Session` across the edit.
//! - `disk`: two processes sharing a `dir@1` store.
use super::{
    Artifacts, Config, Report, Verdict, difference, edits::Scenario, o1::produce_in_child, produce,
    produce_with_store, request_artifacts, switches,
};
use hirsel_driver::session::{FileChange, Session};
use hirsel_engine::store::MemStore;
use std::path::Path;
use std::sync::Arc;

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
    let store = Arc::new(MemStore::default());
    let warmed = produce_with_store(&entry, before, store.clone());
    let resident_store = before.result_store();
    let mut resident = Session::open_with_compiler(&entry, before.compiler(resident_store.clone()));
    let _resident_before = resident_artifacts(&mut resident, before);
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
    resident.apply(
        scenario
            .before
            .keys()
            .chain(scenario.after.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|path| scenario.before.get(*path) != scenario.after.get(*path))
            .map(|path| FileChange {
                path: tree.join(path),
                bytes: scenario
                    .after
                    .get(path)
                    .map(|text| text.as_bytes().to_vec()),
            })
            .collect(),
    );
    if before != after {
        // A changed verification or compile-time policy selects new immutable
        // factories, while the same Session keeps its delivered input cursor.
        resident.configure(after.compiler(resident_store));
    }
    let cold = match produce(&entry, after) {
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
                .and_then(|_| produce_with_store(&entry, after, store).map_err(Verdict::Todo)),
        ),
    );
    report.add(
        name("resident"),
        compare(Ok({
            let artifacts = resident_artifacts(&mut resident, after);
            super::write_artifacts(&base.join("resident"), &artifacts)
                .map_err(|e| e.to_string())?;
            artifacts
        })),
    );
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

fn resident_artifacts(session: &mut Session, config: &Config) -> Artifacts {
    hirsel_engine::with_compiler_stack(|| request_artifacts(session, config))
}
