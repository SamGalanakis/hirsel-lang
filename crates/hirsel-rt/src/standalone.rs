//! Standalone process inputs are explicit: one run record and an optional argv snapshot.
use hirsel_run_config::{LaunchKind, Root, RunConfig, RunRefusal, ValidatedRun};
use std::ffi::OsString;

pub(crate) const ROOT_IO: u8 = 1;
pub(crate) const ROOT_ARGS: u8 = 2;
pub(crate) const ROOT_ENV: u8 = 4;
pub(crate) const ROOT_UNSAFE: u8 = 8;
pub(crate) const ROOT_TEST: u8 = 16;
/// Not a root: the program calls C outside a deterministic wrapper (PROG-4).
pub(crate) const UNREPLAYABLE: u8 = 32;

/// Splits the compiled selection into its roots and whether recording or
/// replaying is refused.
pub(crate) fn unreplayable(mask: u8) -> (u8, bool) {
    (mask & !UNREPLAYABLE, mask & UNREPLAYABLE != 0)
}

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

/// A launch's explicit process selection: a live host run, optionally streaming its
/// tape, or a replay of a recording, which binds its own run and needs no grants.
pub(crate) enum Selected {
    Live {
        run: ValidatedRun,
        tape: Option<std::path::PathBuf>,
        revocations: Option<std::path::PathBuf>,
    },
    Replay {
        run: ValidatedRun,
        io: crate::io_sim::SimIo,
    },
}
impl Selected {
    /// Whether this launch records or replays a tape.
    pub(crate) fn taped(&self) -> bool {
        matches!(self, Self::Replay { .. } | Self::Live { tape: Some(_), .. })
    }
    pub(crate) fn run(&self) -> &ValidatedRun {
        match self {
            Self::Live { run, .. } | Self::Replay { run, .. } => run,
        }
    }
}

/// `--hirsel-replay <tape>` alone, or an optional `--hirsel-record <tape>` prefix
/// before the ordinary launch input. Replay refuses a recording bound to other code.
pub(crate) fn select(
    arguments: Vec<OsString>,
    program: [u8; 32],
    entry: &str,
    roots: u8,
) -> Result<Selected, RunRefusal> {
    if arguments
        .first()
        .is_some_and(|flag| flag == "--hirsel-replay")
    {
        let [_, path] = arguments.as_slice() else {
            return Err(RunRefusal::Invalid {
                field: "argv".into(),
                detail: "expected only --hirsel-replay <tape>".into(),
            });
        };
        let (io, run) = crate::host_launch::replay(std::path::Path::new(path))?;
        run.check_program(program, entry, launch_kind(roots))?;
        run.require_roots(&required_roots(roots)?)?;
        return Ok(Selected::Replay { run, io });
    }
    let options = live_options(&arguments)?;
    Ok(Selected::Live {
        run: select_run(options.input.to_vec(), program, entry, roots)?,
        tape: options.tape,
        revocations: options.revocations,
    })
}
pub(crate) struct LiveOptions<'a> {
    pub input: &'a [OsString],
    pub tape: Option<std::path::PathBuf>,
    pub revocations: Option<std::path::PathBuf>,
}
pub(crate) fn live_options(arguments: &[OsString]) -> Result<LiveOptions<'_>, RunRefusal> {
    let (mut tape, mut revocations) = (None, None);
    let mut offset = 0;
    while let Some(flag) = arguments.get(offset) {
        let slot = if flag == "--hirsel-record" && tape.is_none() {
            &mut tape
        } else if flag == "--hirsel-revocations" && revocations.is_none() {
            &mut revocations
        } else {
            break;
        };
        let path = arguments
            .get(offset + 1)
            .ok_or_else(|| RunRefusal::Invalid {
                field: "argv".into(),
                detail: "expected a path after launch option".into(),
            })?;
        *slot = Some(std::path::PathBuf::from(path));
        offset += 2;
    }
    Ok(LiveOptions {
        input: &arguments[offset..],
        tape,
        revocations,
    })
}

fn launch_kind(roots: u8) -> LaunchKind {
    if roots == ROOT_TEST {
        LaunchKind::Test
    } else {
        LaunchKind::Main
    }
}

/// Normalize process input into the shared record *before* admission. Only the
/// explicit argument delimiter grants Args without a run record; no ambient
/// environment variable supplies authority.
fn select_run(
    arguments: Vec<OsString>,
    program: [u8; 32],
    entry: &str,
    roots: u8,
) -> Result<ValidatedRun, RunRefusal> {
    let launch = launch_kind(roots);
    let invalid = || {
        RunRefusal::Invalid {
        field: "argv".into(),
        detail: "expected [--hirsel-record <tape>] --hirsel-run /absolute/run.json [args...], --hirsel-grants grants.json [args...] or -- [args...], or --hirsel-replay <tape>".into(),
    }
    };
    let record = match arguments.first() {
        None => RunConfig::empty(program, entry.into(), launch),
        Some(flag) if flag == "--hirsel-run" || flag == "--hirsel-grants" => {
            let input = arguments.get(1).ok_or_else(invalid)?;
            let run = if flag == "--hirsel-grants" {
                RunConfig::grants_input(input, program, entry.into(), launch)?
            } else {
                RunConfig::record_input(input)?
            };
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
    fn replay_refuses_a_recording_of_other_code_and_record_needs_a_tape() {
        use crate::launch::{RecordedRun, Recording, render_recording};
        let entry = "standalone@0/main";
        let run = RunConfig::empty([1; 32], entry.into(), LaunchKind::Main)
            .validated()
            .unwrap();
        let tape = std::env::temp_dir().join(format!("hirsel-select-{}", std::process::id()));
        let recording = Recording {
            arguments: vec![],
            run_identity: run.identity(),
            hash_seed: 0,
            random_seed: 0,
            test_seed: 0,
            binding: Some(RecordedRun {
                canonical: run.canonical().to_vec(),
                admitted_at: 0,
                revocation: None,
            }),
            tree: crate::io::Tree::new(Default::default()).unwrap(),
            events: vec![],
        };
        std::fs::write(&tape, render_recording(&recording).unwrap()).unwrap();
        let replay = |program| {
            select(
                vec!["--hirsel-replay".into(), tape.clone().into()],
                program,
                entry,
                0,
            )
        };
        assert!(matches!(replay([1; 32]), Ok(Selected::Replay { .. })));
        assert!(matches!(
            replay([0; 32]),
            Err(RunRefusal::Binding { field }) if field == "program"
        ));
        // Replay takes no other launch input: the recording binds its run.
        assert!(matches!(
            select(
                vec!["--hirsel-replay".into(), tape.clone().into(), "--".into()],
                [1; 32],
                entry,
                0
            ),
            Err(RunRefusal::Invalid { .. })
        ));
        std::fs::remove_file(&tape).unwrap();
        assert!(matches!(
            select(vec!["--hirsel-record".into()], [1; 32], entry, 0),
            Err(RunRefusal::Invalid { .. })
        ));
        assert!(matches!(
            select(
                vec!["--hirsel-record".into(), "tape".into(), "--".into(), "x".into()],
                [1; 32],
                entry,
                ROOT_ARGS
            ),
            Ok(Selected::Live { tape: Some(_), run, .. }) if run.record().args.as_deref() == Some(["x".to_string()].as_slice())
        ));
    }
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
