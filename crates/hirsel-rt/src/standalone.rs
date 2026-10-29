//! Standalone process inputs are explicit: one run record and an optional argv snapshot.
use hirsel_run_config::{BoundRun, ProgramFacts, Root, RunConfig};
use hirsel_run_schema::{CodeReach, LaunchKind, RunRefusal};
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

/// A launch's explicit process selection: a live host run, optionally streaming its
/// tape, or a replay of a recording, which binds its own run and needs no grants.
pub(crate) enum Selected {
    Live {
        run: BoundRun,
        tape: Option<std::path::PathBuf>,
        revocations: Option<std::path::PathBuf>,
    },
    Replay {
        run: BoundRun,
        io: std::rc::Rc<std::cell::RefCell<crate::io_sim::SimIo>>,
    },
}
impl Selected {
    /// Whether this launch records or replays a tape.
    pub(crate) fn taped(&self) -> bool {
        matches!(self, Self::Replay { .. } | Self::Live { tape: Some(_), .. })
    }
    pub(crate) fn run(&self) -> &hirsel_run_config::ValidatedRun {
        match self {
            Self::Live { run, .. } | Self::Replay { run, .. } => run.run(),
        }
    }
}

/// Select runtime options before the application boundary, in any order.
/// Replay refuses a recording bound to other code.
pub(crate) fn select(
    arguments: Vec<OsString>,
    program: [u8; 32],
    entry: &str,
    roots: u8,
    escapes: bool,
    fueled: bool,
) -> Result<Selected, RunRefusal> {
    let options = live_options(&arguments)?;
    let policy = options
        .replay_policy
        .as_ref()
        .map(|value| {
            value
                .to_str()
                .ok_or_else(|| {
                    replay_policy_error("expected a JSON object after --hirsel-replay-policy")
                })
                .and_then(replay_policy)
        })
        .transpose()?;
    let arguments = &options.input;
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
        return replay_selected(std::path::Path::new(path), |mut run| {
            if let Some(policy) = policy {
                let admission = crate::tasks::ordered::admission::Admission::prepare(Some(&run))
                    .map_err(|error| replay_policy_error(&error))?;
                if admission.is_none() {
                    return Err(replay_policy_error(
                        "requires qualified FIFO cooperative execution",
                    ));
                }
                let identity = run.identity();
                let mut record = run.record().clone();
                record.placement = Some(policy);
                run = record.validated()?;
                assert_eq!(
                    run.identity(),
                    identity,
                    "replay policy never changes identity"
                );
            }
            run.bind(&facts(program, entry, roots, escapes, fueled)?)
        });
    }
    if policy.is_some() {
        return Err(replay_policy_error("requires --hirsel-replay <tape>"));
    }
    Ok(Selected::Live {
        run: select_run(options.input, program, entry, roots, escapes, fueled)?,
        tape: options.tape,
        revocations: options.revocations,
    })
}

#[inline(never)]
pub(crate) fn replay_selected(
    path: &std::path::Path,
    bind: impl FnOnce(hirsel_run_config::ValidatedRun) -> Result<BoundRun, RunRefusal>,
) -> Result<Selected, RunRefusal> {
    let (io, run) = crate::host_launch::replay(path)?;
    let run = bind(run)?;
    Ok(Selected::Replay {
        run,
        io: std::rc::Rc::new(std::cell::RefCell::new(io)),
    })
}

fn replay_policy_error(detail: &str) -> RunRefusal {
    RunRefusal::Invalid {
        field: "replay.policy".into(),
        detail: detail.into(),
    }
}

/// A fresh host policy is separate from recorded placement provenance. Only
/// these nonidentity Ordered knobs are accepted: no cores, affinity, sidecars
/// or new authority can be requested through replay.
fn replay_policy(json: &str) -> Result<hirsel_run_config::Placement, RunRefusal> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| replay_policy_error(&e.to_string()))?;
    let object = value
        .as_object()
        .ok_or_else(|| replay_policy_error("expected a JSON object"))?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "commit" | "reorder" | "run_ahead" | "speculation_bytes"
        )
    }) {
        return Err(replay_policy_error(
            "expected only commit, reorder, run_ahead or speculation_bytes",
        ));
    }
    hirsel_run_config::Placement::from_json(json.as_bytes())
}
pub(crate) struct LiveOptions {
    pub input: Vec<OsString>,
    pub tape: Option<std::path::PathBuf>,
    pub revocations: Option<std::path::PathBuf>,
    pub replay_policy: Option<OsString>,
    pub trace: Option<(usize, std::path::PathBuf)>,
}

fn argument_error(detail: &str) -> RunRefusal {
    RunRefusal::Invalid {
        field: "argv".into(),
        detail: detail.into(),
    }
}

// Scan only the runtime prefix. The first application word or `--` is a
// permanent boundary, even when later application words spell runtime flags.
pub(crate) fn live_options(arguments: &[OsString]) -> Result<LiveOptions, RunRefusal> {
    let mut options = LiveOptions {
        input: Vec::new(),
        tape: None,
        revocations: None,
        replay_policy: None,
        trace: None,
    };
    let mut selector = None;
    let mut offset = 0;
    let mut delimiter = false;
    while let Some(flag) = arguments.get(offset) {
        if flag == "--" {
            delimiter = true;
            offset += 1;
            break;
        }
        let known = matches!(
            flag.to_str(),
            Some(
                "--hirsel-run"
                    | "--hirsel-grants"
                    | "--hirsel-replay"
                    | "--hirsel-record"
                    | "--hirsel-revocations"
                    | "--hirsel-revocation"
                    | "--hirsel-replay-policy"
                    | "--hirsel-trace-events"
            )
        );
        if !known {
            break;
        }
        let value = arguments
            .get(offset + 1)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                argument_error(&format!("expected a path after {}", flag.to_string_lossy()))
            })?;
        match flag.to_str().unwrap() {
            "--hirsel-run" | "--hirsel-grants" | "--hirsel-replay" => {
                if selector.is_some() {
                    return Err(argument_error("duplicated or conflicting run selection"));
                }
                selector = Some((flag.clone(), value.clone()));
            }
            "--hirsel-record" => {
                if options.tape.replace(value.into()).is_some() {
                    return Err(argument_error("duplicated --hirsel-record"));
                }
            }
            "--hirsel-revocations" | "--hirsel-revocation" => {
                if options.revocations.replace(value.into()).is_some() {
                    return Err(argument_error("duplicated --hirsel-revocations"));
                }
            }
            "--hirsel-replay-policy" => {
                if options.replay_policy.replace(value.clone()).is_some() {
                    return Err(argument_error("duplicated --hirsel-replay-policy"));
                }
            }
            "--hirsel-trace-events" => {
                if options.trace.replace((offset, value.into())).is_some() {
                    return Err(argument_error("duplicated --hirsel-trace-events"));
                }
            }
            _ => unreachable!(),
        }
        offset += 2;
    }
    let replay = selector
        .as_ref()
        .is_some_and(|(flag, _)| flag == "--hirsel-replay");
    if replay && (options.tape.is_some() || options.revocations.is_some()) {
        return Err(argument_error(
            "replay conflicts with recording and revocations",
        ));
    }
    if replay && (delimiter || offset != arguments.len()) {
        return Err(argument_error(
            "expected only replay options and --hirsel-replay <tape>",
        ));
    }
    if let Some((flag, path)) = selector {
        options.input.extend([flag, path]);
    } else if delimiter {
        options.input.push("--".into());
    }
    options.input.extend_from_slice(&arguments[offset..]);
    Ok(options)
}

/// The compiled code a standalone launch binds its run to. An executable keeps only
/// the escape verdict, so its reach names no path.
pub(crate) fn facts(
    program: [u8; 32],
    entry: &str,
    roots: u8,
    escapes: bool,
    fueled: bool,
) -> Result<ProgramFacts, RunRefusal> {
    Ok(ProgramFacts {
        program,
        entry: entry.into(),
        launch: launch_kind(roots),
        roots: required_roots(roots)?,
        code: if escapes {
            CodeReach::Escapes(vec![])
        } else {
            CodeReach::Safe
        },
        fueled,
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
    escapes: bool,
    fueled: bool,
) -> Result<BoundRun, RunRefusal> {
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
            let mut record = if flag == "--hirsel-grants" {
                RunConfig::grants_template(input, program, entry.into(), launch)?
            } else {
                RunConfig::record_template(input)?
            };
            match hirsel_run_schema::snapshot_args(arguments.into_iter().skip(2)) {
                Ok(args) => record.args = Some(args),
                // The selected record's own refusal comes first.
                Err(refusal) => {
                    record.validated()?.check_program(program, entry, launch)?;
                    return Err(refusal);
                }
            }
            // One validation: the record with its snapshot, checked against the code.
            let run = record.validated()?;
            run.check_program(program, entry, launch)?;
            return run.bind(&facts(program, entry, roots, escapes, fueled)?);
        }
        Some(flag) if flag == "--" => {
            let mut record = RunConfig::empty(program, entry.into(), launch);
            record.args = Some(hirsel_run_schema::snapshot_args(
                arguments.into_iter().skip(1),
            )?);
            record
        }
        _ => return Err(invalid()),
    };
    // Preserve all configured inputs and grants; the process snapshot is the
    // only changed field and contributes to the admitted run's identity.
    record
        .validated()?
        .bind(&facts(program, entry, roots, escapes, fueled)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_selection(words: &[&str]) -> Result<Selected, RunRefusal> {
        let scratch = tempfile::tempdir().unwrap();
        let grants = scratch.path().join("grants.json");
        std::fs::write(&grants, "{}").unwrap();
        let words = words
            .iter()
            .map(|word| {
                if *word == "GRANTS" {
                    grants.clone().into_os_string()
                } else {
                    OsString::from(word)
                }
            })
            .collect();
        select(words, [1; 32], "options@1/main", ROOT_ARGS, false, false)
    }

    #[test]
    fn runtime_options_grants_before_record_preserves_only_app_args() {
        let Selected::Live { run, tape, .. } = runtime_selection(&[
            "--hirsel-grants",
            "GRANTS",
            "--hirsel-record",
            "tape",
            "app",
        ])
        .unwrap() else {
            panic!("live")
        };
        assert_eq!(tape, Some("tape".into()));
        assert_eq!(
            run.run().record().args.as_deref(),
            Some(["app".to_string()].as_slice())
        );
    }
    #[test]
    fn runtime_options_record_before_grants_keeps_identity() {
        let Selected::Live { run, tape, .. } = runtime_selection(&[
            "--hirsel-record",
            "tape",
            "--hirsel-grants",
            "GRANTS",
            "app",
        ])
        .unwrap() else {
            panic!("live")
        };
        let Selected::Live { run: other, .. } = runtime_selection(&[
            "--hirsel-grants",
            "GRANTS",
            "--hirsel-record",
            "tape",
            "app",
        ])
        .unwrap() else {
            panic!("live")
        };
        assert_eq!(tape, Some("tape".into()));
        assert_eq!(run.run().identity(), other.run().identity());
        assert_eq!(run.run().canonical(), other.run().canonical());
    }
    #[test]
    fn runtime_options_delimiter_and_first_app_word_stop_parsing() {
        for words in [
            vec!["--hirsel-grants", "GRANTS", "--", "--hirsel-record", "app"],
            vec![
                "--hirsel-grants",
                "GRANTS",
                "first",
                "--hirsel-record",
                "app",
            ],
        ] {
            let Selected::Live { run, tape, .. } = runtime_selection(&words).unwrap() else {
                panic!("live")
            };
            assert!(tape.is_none());
            let expected = if words[2] == "--" {
                vec!["--hirsel-record", "app"]
            } else {
                vec!["first", "--hirsel-record", "app"]
            };
            assert_eq!(run.run().record().args.as_ref().unwrap(), &expected);
        }
    }
    #[test]
    fn runtime_options_duplicates_and_conflicting_inputs_are_typed_refusals() {
        for words in [
            vec!["--hirsel-record", "a", "--hirsel-record", "b", "--"],
            vec!["--hirsel-grants", "GRANTS", "--hirsel-grants", "GRANTS"],
            vec!["--hirsel-grants", "GRANTS", "--hirsel-run", "GRANTS"],
            vec![
                "--hirsel-revocations",
                "a",
                "--hirsel-revocations",
                "b",
                "--",
            ],
        ] {
            assert!(
                matches!(runtime_selection(&words), Err(RunRefusal::Invalid { field, .. }) if field == "argv")
            );
        }
    }

    #[test]
    fn replay_policy_accepts_only_ordered_nonidentity_knobs() {
        let policy = replay_policy(
            r#"{"commit":"dedicated","reorder":"off","run_ahead":3,"speculation_bytes":4096}"#,
        )
        .unwrap();
        let record = RunConfig::empty([1; 32], "entry@1/main".into(), LaunchKind::Main);
        let before = record.validated().unwrap();
        let mut record = record;
        record.placement = Some(policy);
        let after = record.validated().unwrap();
        assert_eq!(before.canonical(), after.canonical());
        assert_eq!(before.identity(), after.identity());
        for json in [
            "[]",
            "null",
            r#"{"cores":8}"#,
            r#"{"control":"socket"}"#,
            r#"{"metrics":"file"}"#,
            r#"{"io":{}}"#,
            r#"{"run_ahead":65}"#,
            r#"{"commit":"other"}"#,
            r#"{"commit":"shared","commit":"dedicated"}"#,
            r#"{"run_ahead":1.0}"#,
        ] {
            assert!(replay_policy(json).is_err(), "{json}");
        }
        assert!(
            select(
                vec!["--hirsel-replay-policy".into(), "{}".into(), "--".into()],
                [1; 32],
                "entry@1/main",
                0,
                false,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn replay_policy_requires_qualified_cooperative_admission() {
        use crate::launch::{RecordedRun, Recording, render_recording};
        let entry = "standalone@0/main";
        let scratch = tempfile::tempdir().unwrap();
        let tape = scratch.path().join("tape");
        for provider in ["tasks.cooperative@1", "tasks.free@1"] {
            let mut record = RunConfig::empty([1; 32], entry.into(), LaunchKind::Main);
            record.io = Some(hirsel_run_schema::IoGrants {
                tasks: Some(provider.into()),
                ..Default::default()
            });
            let run = record.validated().unwrap();
            let recording = Recording {
                arguments: vec![],
                run_identity: run.identity(),
                hash_seed: 0,
                random_seed: 0,
                test_seed: 0,
                binding: Some(RecordedRun {
                    canonical: run.canonical().to_vec(),
                    placement: None,
                    admitted_at: 0,
                    revocation: None,
                    instance: None,
                }),
                tree: crate::io::Tree::new(Default::default()).unwrap(),
                events: vec![],
            };
            let bytes = render_recording(&recording).unwrap();
            std::fs::write(&tape, &bytes).unwrap();
            let selected = select(
                vec![
                    "--hirsel-replay-policy".into(),
                    r#"{"commit":"dedicated","reorder":"off"}"#.into(),
                    "--hirsel-replay".into(),
                    tape.clone().into(),
                ],
                [1; 32],
                entry,
                ROOT_IO,
                false,
                false,
            );
            if provider == "tasks.cooperative@1" {
                let Selected::Replay { run: selected, .. } = selected.unwrap() else {
                    panic!("expected replay");
                };
                assert_eq!(selected.run().identity(), run.identity());
                assert_eq!(
                    selected.run().record().placement.as_ref().unwrap().commit,
                    Some(hirsel_run_config::Commit::Dedicated)
                );
            } else {
                assert!(
                    matches!(selected, Err(RunRefusal::Invalid { field, .. }) if field == "replay.policy")
                );
            }
            assert_eq!(std::fs::read(&tape).unwrap(), bytes.as_bytes());
        }
    }

    #[test]
    fn replay_refuses_a_recording_of_other_code_and_record_needs_a_tape() {
        use crate::launch::{RecordedRun, Recording, render_recording};
        let entry = "standalone@0/main";
        let run = RunConfig::empty([1; 32], entry.into(), LaunchKind::Main)
            .validated()
            .unwrap();
        let tape_scratch = tempfile::Builder::new()
            .prefix("hirsel-select-")
            .tempdir()
            .unwrap();
        let tape = tape_scratch.path().join("tape");
        let recording = Recording {
            arguments: vec![],
            run_identity: run.identity(),
            hash_seed: 0,
            random_seed: 0,
            test_seed: 0,
            binding: Some(RecordedRun {
                canonical: run.canonical().to_vec(),
                placement: None,
                admitted_at: 0,
                revocation: None,
                instance: None,
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
                false,
                false,
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
                0,
                false,
                false,
            ),
            Err(RunRefusal::Invalid { .. })
        ));
        std::fs::remove_file(&tape).unwrap();
        assert!(matches!(
            select(
                vec!["--hirsel-record".into()],
                [1; 32],
                entry,
                0,
                false,
                false
            ),
            Err(RunRefusal::Invalid { .. })
        ));
        assert!(matches!(
            select(
                vec!["--hirsel-record".into(), "tape".into(), "--".into(), "x".into()],
                [1; 32],
                entry,
                ROOT_ARGS,
                false,
                false,
            ),
            Ok(Selected::Live { tape: Some(_), run, .. }) if run.run().record().args.as_deref() == Some(["x".to_string()].as_slice())
        ));
    }
    #[test]
    fn unconfigured_process_denies_roots_and_explicit_argv_grants_only_args() {
        let entry = "standalone@0/main";
        assert!(select_run(vec![], [0; 32], entry, 0, false, false).is_ok());
        assert!(matches!(
            select_run(vec![], [0; 32], entry, ROOT_ARGS, false, false),
            Err(RunRefusal::NotGranted { .. })
        ));
        let run = select_run(
            vec!["--".into(), "one".into(), "--flag".into()],
            [0; 32],
            entry,
            ROOT_ARGS,
            false,
            false,
        )
        .unwrap();
        let run = run.run();
        assert_eq!(
            run.record().args.as_deref(),
            Some(["one".into(), "--flag".into()].as_slice())
        );
        assert!(run.record().io.is_none());
        assert!(run.record().env.is_none());
        assert!(matches!(
            select_run(vec!["--".into()], [0; 32], entry, ROOT_IO, false, false),
            Err(RunRefusal::NotGranted { .. })
        ));
    }
}
