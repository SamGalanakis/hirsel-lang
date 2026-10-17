//! Standalone process inputs are explicit: one run record and an optional argv snapshot.
use hirsel_run_config::{LaunchKind, Root, RunConfig, RunRefusal, ValidatedRun};
use std::ffi::OsString;

pub(crate) const ROOT_IO: u8 = 1;
pub(crate) const ROOT_ARGS: u8 = 2;
pub(crate) const ROOT_ENV: u8 = 4;
pub(crate) const ROOT_UNSAFE: u8 = 8;
pub(crate) const ROOT_TEST: u8 = 16;

pub(crate) fn required_roots(mask: u8) -> Result<Vec<Root>, RunRefusal> {
    if mask & !31 != 0 || mask & ROOT_TEST != 0 && mask != ROOT_TEST {
        return Err(RunRefusal::Invalid {
            field: "entry.roots".into(),
            detail: "invalid compiled root selection".into(),
        });
    }
    Ok([
        (ROOT_IO, Root::Io),
        (ROOT_ARGS, Root::Args),
        (ROOT_ENV, Root::Env),
        (ROOT_UNSAFE, Root::Unsafe),
        (ROOT_TEST, Root::Test),
    ]
    .into_iter()
    .filter_map(|(bit, root)| (mask & bit != 0).then_some(root))
    .collect())
}

/// Normalize process input into the shared record *before* admission. Only the
/// explicit argument delimiter grants Args without a run record; no ambient
/// environment variable supplies authority.
pub(crate) fn select_run(
    arguments: Vec<OsString>,
    program: [u8; 32],
    entry: &str,
    roots: u8,
) -> Result<ValidatedRun, RunRefusal> {
    let launch = if roots == ROOT_TEST {
        LaunchKind::Test
    } else {
        LaunchKind::Main
    };
    let invalid = || RunRefusal::Invalid {
        field: "argv".into(),
        detail: "expected --hirsel-run /absolute/run.json [args...] or -- [args...]".into(),
    };
    let record = match arguments.first() {
        None => RunConfig::empty(program, entry.into(), launch),
        Some(flag) if flag == "--hirsel-run" => {
            let path = arguments.get(1).ok_or_else(invalid)?;
            let run = RunConfig::read_path(std::path::Path::new(path))?;
            run.check_program(program, entry, launch)?;
            let mut record = run.record().clone();
            record.args = Some(hirsel_run_config::snapshot_args(
                arguments.into_iter().skip(2),
            )?);
            record
        }
        Some(flag) if flag == "--" => {
            let mut record = RunConfig::empty(program, entry.into(), launch);
            record.args = Some(hirsel_run_config::snapshot_args(
                arguments.into_iter().skip(1),
            )?);
            record
        }
        _ => return Err(invalid()),
    };
    // Preserve all configured inputs and grants; the process snapshot is the
    // only changed field and contributes to the admitted run's identity.
    let run = record.validated()?;
    run.check_program(program, entry, launch)?;
    run.require_roots(&required_roots(roots)?)?;
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unconfigured_process_denies_roots_and_explicit_argv_grants_only_args() {
        let entry = "standalone@0/main";
        assert!(select_run(vec![], [0; 32], entry, 0).is_ok());
        assert!(matches!(
            select_run(vec![], [0; 32], entry, ROOT_ARGS),
            Err(RunRefusal::NotGranted { .. })
        ));
        let run = select_run(
            vec!["--".into(), "one".into(), "--flag".into()],
            [0; 32],
            entry,
            ROOT_ARGS,
        )
        .unwrap();
        assert_eq!(
            run.record().args.as_deref(),
            Some(["one".into(), "--flag".into()].as_slice())
        );
        assert!(run.record().io.is_none());
        assert!(run.record().env.is_none());
        assert!(matches!(
            select_run(vec!["--".into()], [0; 32], entry, ROOT_IO),
            Err(RunRefusal::NotGranted { .. })
        ));
    }
}
