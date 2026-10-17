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
        deserialize_with = "runs"
    )]
    pub runs: Option<Vec<ProcessRun>>,
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
}

// Missing is None; an explicit null is not a runs array, under any provider.
fn runs<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Option<Vec<ProcessRun>>, D::Error> {
    Vec::deserialize(de).map(Some)
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

/// The checks every provider makes, in order, before any effect.
pub fn check(program: &str, args: &[String], rules: &[ProcessRule]) -> Result<(), ProcessFailure> {
    if !canonical_program(program) || args.iter().any(|a| a.contains('\0')) {
        return Err(ProcessFailure::InvalidInput);
    }
    if !rules.iter().any(|r| r.program == program) {
        return Err(ProcessFailure::NotGranted);
    }
    Ok(())
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
