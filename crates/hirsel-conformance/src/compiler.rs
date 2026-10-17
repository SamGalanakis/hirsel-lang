use crate::*;
use hirsel_engine::{QueryError, ToolError};

#[derive(Clone, Debug, Default)]
pub struct Observation {
    pub actual: String,
    pub phase: String,
    /// Checked execution differed from normal execution.
    pub checked: Option<String>,
    /// First diagnostic/failure's token identity, independent of formatted lines.
    pub primary: Option<crate::corpus_format::Primary>,
}
pub fn evaluate(path: &Path, expect: &Expect) -> (Status, Observation) {
    evaluate_with_hash_seed(path, expect, 0)
}
pub fn evaluate_with_hash_seed(
    path: &Path,
    expect: &Expect,
    hash_seed: u64,
) -> (Status, Observation) {
    hirsel_engine::with_compiler_stack(|| evaluate_on_compiler_thread(path, expect, hash_seed))
}
fn evaluate_on_compiler_thread(
    path: &Path,
    expect: &Expect,
    hash_seed: u64,
) -> (Status, Observation) {
    let loaded = hirsel_driver::load_program(path);
    let mut program = match loaded {
        Ok(program) => program,
        Err(ToolError::Query {
            error: QueryError::Diagnostics(diagnostics),
            sources,
            ..
        }) if !diagnostics.is_empty() => {
            let d = &diagnostics[0];
            let line = sources
                .location(d.primary)
                .map(|l| l.line as usize)
                .unwrap_or(0);
            let observation = Observation {
                actual: format!("{} at line {line}: {}", d.rule, d.message),
                phase: "source indexing".into(),
                ..Default::default()
            };
            let status = match expect {
                Expect::Reject {
                    rule,
                    line: expected,
                } if rule == &d.rule && *expected == line => Status::Pass,
                _ => Status::Fail(format!("expected {expect:?}; got {}", observation.actual)),
            };
            return (status, observation);
        }
        Err(error) => {
            return (
                Status::Fail(format!("tool error: {error}")),
                Observation::default(),
            );
        }
    };
    let text = fs::read_to_string(path).unwrap_or_default();
    let context = program
        .files
        .iter()
        .find(|(file, _)| *file == program.primary)
        .unwrap()
        .1
        .clone();
    let entry = if path.with_extension("hir.json").exists() {
        program.entry.clone()
    } else {
        crate::selected_entry(&text, &context)
    };
    let checked = program.check().map_err(|e| format!("{e:?}"));
    let mut engine = program.engine;
    let mut observation = Observation {
        phase: engine
            .last_phase
            .map(|p| p.phase_name())
            .unwrap_or("tool input")
            .into(),
        ..Default::default()
    };
    let output = match checked {
        Ok(value) => value,
        Err(error) => {
            observation.actual = format!("tool error: {error}");
            return (Status::Fail(observation.actual.clone()), observation);
        }
    };
    if let Some(d) = output.diagnostics.first() {
        observation.primary =
            crate::corpus_format::primary_at_span(&mut engine, d.rule.as_str(), d.primary);
        let line = output
            .sources
            .location(d.primary)
            .map(|l| l.line)
            .unwrap_or(0);
        observation.actual = format!("{} at line {line}: {}", d.rule, d.message);
        if let Some(phase) = engine.diagnostic_phase(d) {
            observation.phase = phase;
        }
        let status = match expect {
            Expect::Reject {
                rule,
                line: expected,
            } if *rule == d.rule && *expected == line as usize => Status::Pass,
            _ => Status::Fail(format!("expected {expect:?}; got {}", observation.actual)),
        };
        return (status, observation);
    }
    if let hirsel_engine::CheckCompletion::Incomplete { phase } = output.completion {
        observation.actual = "not implemented".into();
        observation.phase = phase.phase_name().into();
        return (Status::NotImplemented, observation);
    }
    observation.actual = "accept".into();
    if matches!(expect, Expect::Accept) {
        return (Status::Pass, observation);
    }
    if matches!(expect, Expect::Reject { .. }) {
        return (
            Status::Fail(format!("expected {expect:?}; got accept")),
            observation,
        );
    }
    let config = hirsel_engine::RunConfiguration {
        entry,
        hash_seed,
        inputs: vec![],
    };
    observation.phase = "interpretation".into();
    let execution = match hirsel_engine::run_observed_snapshot(
        &mut engine,
        &config,
        hirsel_engine::SimulationConfig::default(),
    ) {
        Ok(outcome) => outcome,
        Err(ToolError::Query {
            error: QueryError::NotImplemented,
            ..
        }) => {
            observation.actual = "not implemented".into();
            return (Status::NotImplemented, observation);
        }
        Err(error) => {
            observation.actual = format!("tool error: {error}");
            return (Status::Fail(observation.actual.clone()), observation);
        }
    };
    // Third witness: the same IR under checked execution must observe the same.
    observation.checked = match hirsel_engine::run_observed_checked(
        &mut engine,
        &config,
        hirsel_engine::SimulationConfig::default(),
    ) {
        Ok(checked) => checked_divergence(&execution, &checked),
        Err(error) => Some(format!("checked execution: tool error: {error}")),
    };
    let stdout = execution
        .observation
        .as_ref()
        .map(|o| o.output.as_slice())
        .unwrap_or(&execution.tooling_output);
    if let Some(failure) = execution
        .observation
        .as_ref()
        .and_then(|o| o.failures.first())
    {
        observation.primary = crate::corpus_format::primary_at_site(
            &mut engine,
            path,
            failure.rule.as_str(),
            failure.stable_site.line as usize,
            failure.stable_site.column as usize,
        );
    }
    let outcome = match execution.outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            let missing = matches!(
                &error,
                hirsel_engine::RuntimeError::Query(QueryError::NotImplemented)
            );
            let text = match error {
                hirsel_engine::RuntimeError::Query(error) => ToolError::Query {
                    error,
                    phase: Some(hirsel_engine::QueryKind::Interpret),
                    sources: output.sources.clone(),
                }
                .to_string(),
                hirsel_engine::RuntimeError::Io(error) => error.to_string(),
                hirsel_engine::RuntimeError::Tool(message) => message,
            };
            observation.actual = format!(
                "tool error: {text}; stdout {:?}",
                String::from_utf8_lossy(stdout)
            );
            return (
                if missing {
                    Status::NotImplemented
                } else {
                    Status::Fail(observation.actual.clone())
                },
                observation,
            );
        }
    };
    observation.actual = format!("{outcome:?}; stdout {:?}", String::from_utf8_lossy(stdout));
    let status = match (expect, execution.observation.as_ref()) {
        (
            Expect::Run {
                stdout: expected,
                exit,
            },
            Some(actual),
        ) if actual.exit_code == Some(*exit)
            && actual.failures.is_empty()
            && expected.as_bytes() == actual.output =>
        {
            Status::Pass
        }
        (
            Expect::Fail {
                rule,
                line,
                stdout: expected,
            },
            Some(actual),
        ) if actual.failures.first().is_some_and(|failure| {
            failure.rule == *rule && failure.stable_site.line as usize == *line
        }) && expected
            .as_ref()
            .is_none_or(|expected| expected.as_bytes() == actual.output) =>
        {
            Status::Pass
        }
        _ => Status::Fail(format!("expected {expect:?}; got {}", observation.actual)),
    };
    (status, observation)
}
/// Checked execution either reproduces the normal execution exactly or traps.
pub(crate) fn checked_divergence(
    normal: &hirsel_engine::ObservedExecution,
    checked: &hirsel_engine::ObservedExecution,
) -> Option<String> {
    if let Ok(RunOutcome::Trapped(trap)) = &checked.outcome {
        return Some(trap.to_string());
    }
    // Checked execution can run an operation normal execution still lacks (M2).
    if matches!(
        &normal.outcome,
        Err(hirsel_engine::RuntimeError::Query(
            QueryError::NotImplemented
        ))
    ) {
        return None;
    }
    let outcome = |e: &hirsel_engine::ObservedExecution| match &e.outcome {
        Ok(outcome) => format!("{outcome:?}"),
        Err(error) => format!("error {error:?}"),
    };
    let (normal_outcome, checked_outcome) = (outcome(normal), outcome(checked));
    if normal_outcome != checked_outcome {
        return Some(format!(
            "checked execution diverged: outcome {checked_outcome}, normally {normal_outcome}"
        ));
    }
    if normal.observation != checked.observation || normal.tooling_output != checked.tooling_output
    {
        return Some("checked execution diverged: observation differs".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inner_unavailable_execution_preserves_stdout_in_conformance_observation() {
        hirsel_engine::with_compiler_stack(|| {
            let path = std::env::temp_dir().join(format!(
                "hirsel-conformance-api-error-{}.hir",
                std::process::id()
            ));
            fs::write(&path, "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"before unavailable\\n\"); let frozen: Shared[u8] = shared.freeze(7u8); Ok(()) }").unwrap();
            let (status, observation) = evaluate(
                &path,
                &Expect::Run {
                    stdout: "before unavailable\n".into(),
                    exit: 0,
                },
            );
            assert_eq!(status, Status::NotImplemented);
            assert!(
                observation.actual.contains("not implemented"),
                "{}",
                observation.actual
            );
            assert!(
                observation
                    .actual
                    .contains("stdout \"before unavailable\\n\""),
                "{}",
                observation.actual
            );
            fs::remove_file(path).unwrap();
        });
    }
}
