//! Host use of a completed immutable native request. Build destinations use
//! the selected ordinary provider; runs share the existing process launcher
//! and observation decoder without reconstructing a native program.
use super::{NativeError, StageName, executable::Scratch, link, object_queries};
use crate::requests::NativeExecution;
use crate::{BoundHostRun, HostLaunch, RunOutcome, ToolError, launch_binding::LaunchSelection};
use std::{io::Write, path::Path};

pub(crate) fn materialize_output(
    input: &NativeExecution,
    destination: &Path,
) -> Result<Vec<String>, NativeError> {
    let error = |message| NativeError::new(StageName::Link, message);
    let linked =
        object_queries::linked_product(input.store.as_ref(), input.product).map_err(error)?;
    object_queries::materialize_executable(input.store.as_ref(), &linked, destination)
        .map_err(error)?;
    input.platform.validate_image(destination).map_err(error)?;
    Ok(input.report.clone())
}

pub(crate) fn run_output(
    input: &NativeExecution,
    launch: HostLaunch<'_>,
    selection: LaunchSelection,
    output: super::tool::Output<'_>,
    trace_events: Option<&Path>,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    if selection.requires_replayable(launch) {
        if let Some((user, extern_)) = input.foreign.unmodeled.first_key_value() {
            return Err(ToolError::Runtime(format!(
                "launch: {user} calls the extern {extern_} and is not a wrapper with a \
                 deterministic model, so this run cannot be recorded or replayed (PROG-4)"
            )));
        }
    }
    if matches!(launch, HostLaunch::Callee { .. }) {
        return Err(ToolError::Runtime(
            "launch: a runner host runs its callees interpreted".into(),
        ));
    }
    // The executable admits the bound run exactly once, as the ordinary native
    // launcher does. Binding source facts here never performs host admission.
    let bound = selection.bind(
        launch,
        &input.types,
        &input.signature,
        &input.revision,
        input.code.clone(),
    )?;
    run_bound_output(input, launch, bound, output, trace_events)
}

pub(crate) fn run_bound_output(
    input: &NativeExecution,
    launch: HostLaunch<'_>,
    bound: BoundHostRun,
    output: super::tool::Output<'_>,
    trace_events: Option<&Path>,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    if matches!(launch, HostLaunch::Callee { .. }) {
        return Err(ToolError::Runtime(
            "launch: a runner host runs its callees interpreted".into(),
        ));
    }
    if (matches!(&bound, BoundHostRun::Replay { .. })
        || matches!(
            launch,
            HostLaunch::Live {
                record: Some(_),
                ..
            }
        ))
        && let Some((user, extern_)) = input.foreign.unmodeled.first_key_value()
    {
        return Err(ToolError::Runtime(format!(
            "launch: {user} calls the extern {extern_} and is not a wrapper with a \
             deterministic model, so this run cannot be recorded or replayed (PROG-4)"
        )));
    }
    let (run, tape) = match (bound, launch) {
        (
            BoundHostRun::Live(bound),
            HostLaunch::Live {
                record,
                revocations,
                ..
            },
        ) => {
            hirsel_rt::host_launch::recording_level(bound.run(), record.is_some())
                .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
            (
                bound.into_run(),
                link::Tape::Live {
                    record,
                    revocations,
                },
            )
        }
        (BoundHostRun::Replay { run, .. }, HostLaunch::Replay(path)) => {
            (run, link::Tape::Replay(path))
        }
        _ => unreachable!("selected launch binding preserves its form"),
    };
    let (mut host_stdout, mut host_stderr) = (std::io::stdout(), std::io::stderr());
    let (inherit, json, stdout, stderr): (bool, bool, &mut dyn Write, &mut dyn Write) = match output
    {
        super::tool::Output::Inherit { json } => (true, json, &mut host_stdout, &mut host_stderr),
        super::tool::Output::Streams { stdout, stderr } => (false, false, stdout, stderr),
    };
    let observed = (|| {
        if input
            .run
            .as_ref()
            .is_none_or(|selected| selected.identity() != run.identity())
        {
            return Err(NativeError::new(
                StageName::Launch,
                "native launch differs from the selected validated run",
            ));
        }
        if run.budgets().is_some_and(|budgets| budgets.fuel.is_some()) && !input.request.fuel {
            return Err(NativeError::new(
                StageName::Launch,
                "the selected executable has no fuel checks",
            ));
        }
        let scratch = Scratch::new()?;
        std::fs::write(scratch.0.join("run.json"), run.launch_bytes())
            .map_err(|error| NativeError::new(StageName::Launch, error.to_string()))?;
        let executable = scratch.0.join("program");
        materialize_output(input, &executable)?;
        for line in &input.report {
            writeln!(stderr, "hirsel: {line}")
                .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
        }
        let runnable = input
            .platform
            .executable(executable)
            .map_err(|message| NativeError::new(StageName::Launch, message))?;
        let launched = link::launch_live(
            &runnable,
            &input.request.target,
            &input.toolchain,
            &scratch.0,
            tape,
            link::LiveRun {
                run: &run,
                trace_events,
                inherit,
                stdout,
                stderr,
            },
        )?;
        link::observe_request(&launched, &input.observation, &input.sources)
    })();
    match observed {
        Ok(link::Observed::Stopped(outcome)) => Ok(Ok(outcome)),
        Ok(link::Observed::Refused(reason)) => {
            Err(match hirsel_run_schema::RunRefusal::parse(&reason) {
                Some(refusal) => ToolError::Launch(refusal),
                None => ToolError::Runtime(format!("launch: {reason}")),
            })
        }
        Ok(link::Observed::Ran(observation, waits)) => Ok((|| {
            let sites = super::Sites::from_anchors(
                &input.sources,
                input
                    .observation
                    .anchors
                    .iter()
                    .map(|(path, anchors)| (path, anchors.clone()))
                    .collect(),
            );
            for failure in &observation.failures {
                writeln!(stderr, "{}", sites.report(failure, json)?)
                    .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
            }
            observation.outcome(&sites, waits)
        })()),
        Err(error) => Ok(Err(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requests::NativeObservation;
    use hirsel_run_schema::{BudgetKind, Returned};

    fn launched(body: &str, exit: i32) -> link::Launched {
        link::Launched {
            audit_required: true,
            exit: Some(exit),
            signal: None,
            stdout: b"output bytes".to_vec(),
            stderr: vec![],
            record: format!(
                "hirsel-observation {}\n{body}",
                hirsel_rt::launch::OBSERVATION_VERSION
            ),
        }
    }

    #[test]
    fn immutable_native_tables_preserve_exit_budget_audit_and_malformed_record_checks() {
        let sources = hirsel_span::SourceMap::default();
        let metadata = NativeObservation::default();
        let sites = super::super::Sites::from_anchors(&sources, Default::default());
        for (body, expected) in [
            (
                "status 0\nresult 00000000\naudit 0 0 0 0 0\n",
                RunOutcome::Returned(Returned::Ok),
            ),
            (
                "status 0\nresult 01000000\naudit 0 0 0 0 0\n",
                RunOutcome::Returned(Returned::Error),
            ),
            (
                "budget_exceeded fuel\naudit unavailable\n",
                RunOutcome::BudgetExceeded(BudgetKind::Fuel),
            ),
        ] {
            let process = launched(body, expected.exit_code());
            let link::Observed::Ran(observation, waits) =
                link::observe_request(&process, &metadata, &sources).unwrap()
            else {
                panic!("a valid observed run must decode")
            };
            assert_eq!(observation.output, b"output bytes");
            assert_eq!(observation.outcome(&sites, waits).unwrap(), expected);
            let wrong_exit = launched(body, (expected.exit_code() + 1) % 7);
            assert!(link::observe_request(&wrong_exit, &metadata, &sources).is_err());
        }
        for body in [
            "status 0\nresult 00000000\naudit 1 0 0 0 0\n",
            "status 0\nresult 00000000\nunknown record\n",
            "failure 0 1 0000000000000001 0000000000000000\naudit 0 0 0 0 0\n",
        ] {
            assert!(link::observe_request(&launched(body, 0), &metadata, &sources).is_err());
        }
    }

    #[test]
    fn immutable_native_failure_tables_resolve_selected_anchors_and_refuse_unknown_sites() {
        let mut sources = hirsel_span::SourceMap::default();
        let file = sources
            .add_logical(
                "/physical/main.hir",
                "standalone@0/main.hir",
                "fn main() {}".into(),
            )
            .unwrap();
        let anchor = sources.file(file).unwrap().anchor;
        let path = hirsel_engine::entry_path(&hirsel_engine::default_context());
        let metadata = NativeObservation {
            failure_rules: vec!["DIV-1".into()],
            sites: [(
                0xa5,
                (
                    hirsel_syntax::DefId::intern(&path),
                    hirsel_lir::ir::SiteRange {
                        source: 0,
                        start: 1,
                        end: 2,
                    },
                ),
            )]
            .into_iter()
            .collect(),
            anchors: [(path, vec![anchor])].into_iter().collect(),
        };
        let body = "status 1\nfailure 0 1 00000000000000a5 0000000000000000\naudit 0 0 0 0 0\n";
        let process = launched(body, hirsel_run_schema::exit::FAILED);
        let link::Observed::Ran(observed, _) =
            link::observe_request(&process, &metadata, &sources).unwrap()
        else {
            panic!("selected failure metadata must resolve")
        };
        assert_eq!(observed.failures.len(), 1);
        assert_eq!(observed.failures[0].rule, "DIV-1");
        assert_eq!(observed.failures[0].file, "/physical/main.hir");
        assert_eq!(
            (observed.failures[0].line, observed.failures[0].column),
            (1, 2)
        );
        let mut missing = metadata.clone();
        missing.sites.clear();
        assert!(link::observe_request(&process, &missing, &sources).is_err());
        let mut wrong_anchor = metadata;
        wrong_anchor.anchors.clear();
        assert!(link::observe_request(&process, &wrong_anchor, &sources).is_err());
    }
}
