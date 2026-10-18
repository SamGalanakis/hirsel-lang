//! Closed process grant data; execution engines enforce the validated rules.
use crate::{ProviderContract, RunRefusal, invalid};
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
/// working directory's Dir handle (`None` is `/`) and an exact environment (`None`
/// inherits the granting rule's names from the launcher).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub directory: Option<u64>,
    pub env: Option<Vec<(String, String)>>,
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

/// A completion's streams bounded by `max_output`, as every provider applies it.
pub fn bounded(outcome: ProcessOutcome, max_output: u64) -> ProcessOutcome {
    match outcome {
        ProcessOutcome::Output { stdout, stderr, .. }
            if stdout.len() as u64 > max_output || stderr.len() as u64 > max_output =>
        {
            ProcessOutcome::Error(ProcessFailure::OutputLimit)
        }
        outcome => outcome,
    }
}

impl ProcessGrant {
    pub(crate) fn validate(&mut self) -> Result<(), RunRefusal> {
        let script =
            ProviderContract::parse(&self.provider) == Some(ProviderContract::ProcessScript);
        if self.runs.is_some() != script {
            return Err(invalid(
                "io.process.runs",
                "runs is required only for process.script@1",
            ));
        }
        if !script && self.children.is_some() {
            return Err(invalid(
                "io.process.children",
                "children is only for process.script@1",
            ));
        }
        self.rules.sort_by(|a, b| a.program.cmp(&b.program));
        for rule in &mut self.rules {
            if !canonical_program(&rule.program) {
                return Err(invalid(
                    "io.process.rules.program",
                    "expected a canonical absolute path",
                ));
            }
            rule.env.sort();
            if rule
                .env
                .iter()
                .any(|name| name.is_empty() || name.contains(['=', '\0']))
            {
                return Err(invalid("io.process.rules.env", "invalid environment name"));
            }
            if rule.env.windows(2).any(|p| p[0] == p[1]) {
                return Err(invalid(
                    "io.process.rules.env",
                    "duplicate environment name",
                ));
            }
        }
        if self.rules.windows(2).any(|p| p[0].program == p[1].program) {
            return Err(invalid("io.process.rules", "duplicate program"));
        }
        for run in self.runs.iter().flatten() {
            if !canonical_program(&run.program) {
                return Err(invalid(
                    "io.process.runs.program",
                    "expected a canonical absolute path",
                ));
            }
            if !scriptable(&run.outcome) {
                return Err(invalid(
                    "io.process.runs.outcome",
                    "a script cannot be canceled",
                ));
            }
        }
        for child in self.children.iter().flatten() {
            if !canonical_program(&child.program) {
                return Err(invalid(
                    "io.process.children.program",
                    "expected a canonical absolute path",
                ));
            }
        }
        Ok(())
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

/// A launcher's standalone `io.process` grant: the JSON of a RunConfig's `io.process`
/// field, admitted exactly as that field is. Scripted runs never widen the rules.
pub fn validate_process_grant(bytes: &[u8]) -> Result<ProcessGrant, RunRefusal> {
    let mut grant: ProcessGrant =
        serde_json::from_slice(bytes).map_err(|error| invalid("io.process", &error.to_string()))?;
    if !matches!(
        ProviderContract::parse(&grant.provider),
        Some(ProviderContract::ProcessSpawn | ProviderContract::ProcessScript)
    ) {
        return Err(RunRefusal::Unsupported {
            field: "io.process.provider".into(),
        });
    }
    grant.validate()?;
    Ok(grant)
}
