//! O1, jobs and salt invariance: every corpus program compiled in a separate
//! process per cell gives byte-identical diagnostics, TIR export and, with
//! `native`, receipts and objects. Ids are process-global and salted at process
//! start, so each cell is its own process (`oracle produce`).
//!
//! The cells are the baseline (one job, unsalted), a repeat of it in another
//! process, then every `jobs × salts` pair; each is compared with the baseline.
use super::{Config, Report, Verdict, difference, read_artifacts};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub label: String,
    pub config: Config,
}

/// The baseline, its repeat, then `jobs × salts` (`None` is unsalted), without
/// duplicating a cell.
pub fn cells(jobs: &[usize], salts: &[Option<u64>], native: bool) -> Vec<Cell> {
    let cell = |label: String, jobs: usize, salt: Option<u64>| Cell {
        label,
        config: Config {
            jobs,
            salt,
            native,
            ..Config::default()
        },
    };
    let mut cells = vec![
        cell("baseline".into(), 1, None),
        cell("repeat".into(), 1, None),
    ];
    for &j in jobs {
        for &salt in salts {
            if (j, salt) == (1, None) {
                continue;
            }
            let label = match salt {
                Some(salt) => format!("j{j}-salt{salt}"),
                None => format!("j{j}"),
            };
            cells.push(cell(label, j, salt));
        }
    }
    cells
}

/// Run every cell of every program through `exe produce`, with each cell's
/// artifacts under `work/<program index>/<cell>`.
pub fn run(exe: &Path, root: &Path, programs: &[PathBuf], cells: &[Cell], work: &Path) -> Report {
    let mut report = Report::new("O1");
    for (index, program) in programs.iter().enumerate() {
        let name = program
            .strip_prefix(root)
            .unwrap_or(program)
            .display()
            .to_string();
        let mut baseline = None;
        for cell in cells {
            let check = format!("{name} [{}]", cell.label);
            if let Err(switch) = super::switches::missing(cell.config.switches()) {
                report.add(check, Verdict::Todo(switch));
                continue;
            }
            let out = work.join(index.to_string()).join(&cell.label);
            let artifacts = match produce_in_child(exe, program, &cell.config, &out) {
                Ok(artifacts) => artifacts,
                Err(why) => {
                    report.add(check, Verdict::Fail(why));
                    continue;
                }
            };
            match &baseline {
                None => {
                    baseline = Some(artifacts);
                    report.add(check, Verdict::Pass);
                }
                Some(expected) => report.add(
                    check,
                    match difference(expected, &artifacts) {
                        None => Verdict::Pass,
                        Some(why) => Verdict::Fail(format!("differs from baseline: {why}")),
                    },
                ),
            }
        }
    }
    report
}

/// `exe produce <config> --out <out> <program>`, then the artifacts it wrote.
pub fn produce_in_child(
    exe: &Path,
    program: &Path,
    config: &Config,
    out: &Path,
) -> Result<super::Artifacts, String> {
    let _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let output = Command::new(exe)
        .arg("produce")
        .args(config.args())
        .arg("--out")
        .arg(out)
        .arg(program)
        .output()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<_> = stderr.lines().rev().take(5).collect();
        return Err(format!(
            "produce exited with {}: {}",
            output.status,
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        ));
    }
    read_artifacts(out).map_err(|e| format!("{}: {e}", out.display()))
}
