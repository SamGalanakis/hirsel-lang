//! Closed process grant data; execution engines enforce the validated rules.
use crate::{RunRefusal, invalid};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessRule {
    pub program: String,
    /// Exact variable names passed through from the launcher's environment.
    #[serde(default)]
    pub env: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessGrant {
    pub provider: String,
    pub rules: Vec<ProcessRule>,
    /// Required for process.script@1, forbidden for every other provider.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub runs: Option<Vec<ProcessRun>>,
    /// Scripted streaming children: process.script@1 only, absent for none.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub children: Option<Vec<ChildRun>>,
}

/// One scripted streaming child: the first unused entry whose program and args equal a
/// spawn's, playing its steps in order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildRun {
    pub program: String,
    pub args: Vec<String>,
    pub steps: Vec<ChildStep>,
}

/// `ChildStep`, in its docs/spec/core-library.md variant order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChildStep {
    Print(Vec<u8>),
    PrintErr(Vec<u8>),
    Expect(Vec<u8>),
    Exit(ExitStatus),
}

/// One scripted run: the first unused entry whose program and args equal a call's.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessRun {
    pub program: String,
    pub args: Vec<String>,
    pub outcome: ProcessOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessOutcome {
    Output {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Error(ProcessFailure),
}

/// A `proc.pipeline` completion: every stage's status in stage order, the last stage's
/// standard output and the standard error every stage shares, or the error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PipelineOutcome {
    Output {
        statuses: Vec<ExitStatus>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Error(ProcessFailure),
}

impl PipelineOutcome {
    /// The completion bounded by `max_output` on each captured stream, as every provider
    /// applies it.
    pub fn bounded(self, max_output: u64) -> Self {
        match self {
            Self::Output { stdout, stderr, .. }
                if stdout.len() as u64 > max_output || stderr.len() as u64 > max_output =>
            {
                Self::Error(ProcessFailure::OutputLimit)
            }
            outcome => outcome,
        }
    }
    /// A one-stage pipeline's completion as the `proc.run` completion it is.
    pub fn into_run(self) -> ProcessOutcome {
        match self {
            Self::Output {
                statuses,
                stdout,
                stderr,
            } if statuses.len() == 1 => ProcessOutcome::Output {
                status: statuses[0],
                stdout,
                stderr,
            },
            Self::Output { .. } => ProcessOutcome::Error(ProcessFailure::Fault(1)),
            Self::Error(failure) => ProcessOutcome::Error(failure),
        }
    }
}

impl From<ProcessOutcome> for PipelineOutcome {
    /// A `proc.run` completion as the one-stage pipeline completion it is.
    fn from(outcome: ProcessOutcome) -> Self {
        match outcome {
            ProcessOutcome::Output {
                status,
                stdout,
                stderr,
            } => Self::Output {
                statuses: vec![status],
                stdout,
                stderr,
            },
            ProcessOutcome::Error(failure) => Self::Error(failure),
        }
    }
}

/// A scripted world's pipeline (docs/spec/core-library.md#processes, scripted runs): each
/// stage takes the first unused run entry whose program and args equal its own, all or
/// none (`NotFound()` when one has none). The first error outcome in stage order is the
/// pipeline's; otherwise the statuses are the outcomes', the output the last stage's and
/// the standard error every stage's in stage order.
pub fn script_pipeline(
    runs: &mut [Option<ProcessRun>],
    stages: &[Command],
    max_output: u64,
) -> PipelineOutcome {
    let mut taken: Vec<usize> = vec![];
    for stage in stages {
        let entry = runs.iter().enumerate().position(|(i, r)| {
            !taken.contains(&i)
                && r.as_ref()
                    .is_some_and(|r| r.program == stage.program && r.args == stage.args)
        });
        match entry {
            Some(i) => taken.push(i),
            None => return PipelineOutcome::Error(ProcessFailure::NotFound),
        }
    }
    let (mut statuses, mut stdout, mut stderr) = (vec![], vec![], vec![]);
    let mut failed = None;
    for i in taken {
        match runs[i].take().unwrap().outcome {
            ProcessOutcome::Output {
                status,
                stdout: out,
                stderr: err,
            } => {
                statuses.push(status);
                stdout = out;
                stderr.extend_from_slice(&err);
            }
            ProcessOutcome::Error(failure) => {
                failed.get_or_insert(failure);
            }
        }
    }
    match failed {
        Some(failure) => PipelineOutcome::Error(failure),
        None => PipelineOutcome::Output {
            statuses,
            stdout,
            stderr,
        }
        .bounded(max_output),
    }
}

/// A `proc.wait_group` value: every stage's status and its captured standard error, in
/// stage order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupOutput {
    pub statuses: Vec<ExitStatus>,
    pub stderr: Vec<Vec<u8>>,
}

/// A scripted world's spawned pipeline (docs/spec/core-library.md#processes, scripted runs):
/// each stage takes the first unused child entry whose program and args equal its own, all
/// or none (`NotFound()` when one has none).
pub fn take_children(
    children: &mut [Option<ChildRun>],
    stages: &[Command],
) -> Result<Vec<ChildRun>, ProcessFailure> {
    let mut taken: Vec<usize> = vec![];
    for stage in stages {
        let entry = children.iter().enumerate().position(|(i, c)| {
            !taken.contains(&i)
                && c.as_ref()
                    .is_some_and(|c| c.program == stage.program && c.args == stage.args)
        });
        taken.push(entry.ok_or(ProcessFailure::NotFound)?);
    }
    Ok(taken
        .into_iter()
        .map(|i| children[i].take().unwrap())
        .collect())
}

/// `ExitStatus`, in its docs/spec/core-library.md variant order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ExitStatus {
    Exited(i32),
    Signaled(i32),
}

/// `ProcessError`, in its docs/spec/core-library.md variant order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessFailure {
    NotGranted,
    InvalidInput,
    NotFound,
    PermissionDenied,
    OutputLimit,
    Fault(u32),
    TimedOut,
    /// `Canceled(cause)`, the `CancelCause` ordinal; never a script outcome.
    Canceled(u32),
    InputClosed,
}

// Missing is None; an explicit null is not an array, under any provider.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    de: D,
) -> Result<Option<Vec<T>>, D::Error> {
    Vec::deserialize(de).map(Some)
}

/// Whether a script may hold `outcome`: everything a host run can return except
/// `Canceled`, which only a cancellation produces.
pub fn scriptable(outcome: &ProcessOutcome) -> bool {
    !matches!(outcome, ProcessOutcome::Error(ProcessFailure::Canceled(_)))
}

/// A canonical absolute program path: `/`, then nonempty components other than
/// `.` and `..`, with no trailing `/`, backslash or NUL. No search or link
/// resolution makes a different spelling name the same grant.
pub fn canonical_program(path: &str) -> bool {
    path.strip_prefix('/').is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .split('/')
                .all(|p| !p.is_empty() && !matches!(p, "." | "..") && !p.contains(['\\', '\0']))
    })
}

/// One `Command` (docs/spec/core-library.md#processes): a program, its arguments, the
/// working directory's Dir handle (`None` is `/`), an exact environment (`None`
/// inherits the granting rule's names from the launcher) and whether it runs in a process
/// group of its own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub directory: Option<u64>,
    pub env: Option<Vec<(String, String)>>,
    pub own_group: bool,
}

/// The checks of a pipeline's stage list, before any stage's own: an empty list, or
/// stages whose `own_group` differ (they share one group or none), is `InvalidInput`.
pub fn stages_error(stages: &[Command]) -> Option<ProcessFailure> {
    (stages.is_empty() || stages.iter().any(|s| s.own_group != stages[0].own_group))
        .then_some(ProcessFailure::InvalidInput)
}

impl Command {
    /// The program and environment checks every provider makes, in order, before any
    /// effect: the granting rule, or the failure.
    pub fn check<'r>(&self, rules: &'r [ProcessRule]) -> Result<&'r ProcessRule, ProcessFailure> {
        let vars = self.env.as_deref().unwrap_or_default();
        let mut names: Vec<&str> = vars.iter().map(|(name, _)| name.as_str()).collect();
        names.sort_unstable();
        if !canonical_program(&self.program)
            || self.args.iter().any(|a| a.contains('\0'))
            || vars.iter().any(|(name, value)| {
                name.is_empty() || name.contains(['=', '\0']) || value.contains('\0')
            })
            || names.windows(2).any(|p| p[0] == p[1])
        {
            return Err(ProcessFailure::InvalidInput);
        }
        rules
            .iter()
            .find(|r| r.program == self.program)
            .filter(|rule| names.iter().all(|name| rule.env.iter().any(|e| e == name)))
            .ok_or(ProcessFailure::NotGranted)
    }
}

/// The ordered script encoding used by scripted process tape requests.
/// Encoding data grants no authority and does not admit a RunConfig.
pub fn encode_process_runs(runs: &[ProcessRun]) -> Vec<u8> {
    serde_json::to_vec(runs).expect("script data has no failing JSON values")
}

/// Decode script data without minting a validated run; the serving adapter checks it.
pub fn decode_process_runs(bytes: &[u8]) -> Result<Vec<ProcessRun>, RunRefusal> {
    serde_json::from_slice(bytes).map_err(|error| invalid("io.process.runs", &error.to_string()))
}

/// The ordered streaming-child script encoding of scripted process tape requests.
pub fn encode_child_runs(children: &[ChildRun]) -> Vec<u8> {
    serde_json::to_vec(children).expect("script data has no failing JSON values")
}

/// Decode streaming-child script data; the serving adapter checks it.
pub fn decode_child_runs(bytes: &[u8]) -> Result<Vec<ChildRun>, RunRefusal> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid("io.process.children", &error.to_string()))
}
