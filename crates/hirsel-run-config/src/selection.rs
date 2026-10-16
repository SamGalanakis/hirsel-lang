//! Standalone record selection, with denial defaults and no process snapshots.
use crate::{LaunchKind, RunConfig, RunRefusal, ValidatedRun};
use std::ffi::OsString;
use std::path::Path;

/// Select one absolute `--hirsel-run` record and bind it to compiled semantic
/// code. Arguments here are tooling syntax, never the source Args snapshot.
pub fn select_run_record(
    program: [u8; 32],
    entry: &str,
    launch: LaunchKind,
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<ValidatedRun, RunRefusal> {
    let arguments: Vec<_> = arguments.into_iter().collect();
    let run = match arguments.as_slice() {
        [] => RunConfig::empty(program, entry.into(), launch).validated()?,
        [flag, path] if flag == "--hirsel-run" => RunConfig::read_path(Path::new(path))?,
        _ => {
            return Err(RunRefusal::Invalid {
                field: "--hirsel-run".into(),
                detail: "expected at most one --hirsel-run <absolute record path>".into(),
            });
        }
    };
    run.check_program(program, entry, launch)?;
    Ok(run)
}
