//! Runner entries (RUN-1, RUN-13, docs/spec/runners.md "Providers"): the places a run
//! may call functions at, each a provider contract with its settings, its seam members
//! and, for a place that runs Hirsel code with authority, the grant set and definition
//! allowlist its callees run under. A member equal to its default is absent from
//! canonical bytes, and a record without runners spells no `runners` member at all.
use crate::{IoGrants, RunRefusal, definition_path, invalid};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One runner of `io.runners`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerEntry {
    pub name: String,
    pub provider: String,
    /// `local.threads@1`: operating-system threads running callees (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<u64>,
    /// Calls admitted to wait for a free place beyond those running (default 64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<u64>,
    /// `sim@1`: the simulated node this runner's callees run on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// `sim@1`: the provider contract the node models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<String>,
    /// `sim@1`: fault sites and their rates, drawn from `inputs.fault_seed`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub faults: BTreeMap<String, FaultRate>,
    /// The callee runs' recording level; absent leaves it to the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<String>,
    /// Re-sending a request whose attempt ended in `NotRun` (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    /// Compute caching; only `null` (none) is supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<serde_json::Value>,
    /// The place's code store entry (`hirsel-code-store`'s `config::open`); absent is
    /// `code.none@1`, the place's own revision only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<serde_json::Value>,
    /// The place's code policy (RUN-8 check 4): `admit.safe@1` refuses a callee that
    /// reaches an escape (CAP-6). Its default, `admit.any@1`, is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<PlaceAdmission>,
    /// The grant set callees run under: the closed `io` schema without runners.
    #[serde(default)]
    pub grants: Option<IoGrants>,
    /// The definitions the runner runs, as full versioned paths; null runs any.
    #[serde(default)]
    pub definitions: Option<Vec<String>>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceAdmission {
    pub code: Option<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retry {
    pub attempts: u32,
}
/// A fault site's rates in parts per million: enabled once per run with probability
/// `enable`, then firing per evaluation with probability `fire`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultRate {
    pub enable: u32,
    pub fire: u32,
}

/// The `sim@1` fault sites, in the order a call meets them.
pub const SIM_FAULT_SITES: &[&str] = &[
    "partition",
    "refuse",
    "overload",
    "lose_request",
    "crash",
    "lose_reply",
];
/// The contracts a `sim@1` node can model, with the fault sites each one's outcomes allow.
pub fn sim_model_sites(model: &str) -> Option<&'static [&'static str]> {
    match model {
        // local.threads@1 can refuse only for capacity.
        "local.threads@1" => Some(&["overload"]),
        "local.process@1" | "remote.tls@1" => Some(SIM_FAULT_SITES),
        _ => None,
    }
}
pub const DEFAULT_THREADS: u64 = 1;
pub const DEFAULT_QUEUE: u64 = 64;

fn name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b == b'_')
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
}

/// Validate and normalize `io.runners` in place; `grants` validates a runner's grant set.
pub(crate) fn validate(
    runners: &mut [RunnerEntry],
    mut grants: impl FnMut(&mut IoGrants, &str) -> Result<(), RunRefusal>,
) -> Result<(), RunRefusal> {
    for pair in runners.windows(2) {
        if pair[0].name >= pair[1].name {
            return Err(invalid(
                "io.runners",
                "runners must be sorted and unique by name",
            ));
        }
    }
    for runner in runners {
        if !name(&runner.name) {
            return Err(invalid(
                "io.runners.name",
                "expected a lowercase runner name",
            ));
        }
        crate::provider("io.runners.provider", &runner.provider)?;
        match runner.provider.as_str() {
            "local.threads@1" => {
                if runner.node.is_some() || runner.models.is_some() || !runner.faults.is_empty() {
                    return Err(invalid(
                        "io.runners",
                        "local.threads@1 takes no node, models or faults",
                    ));
                }
                if runner.threads == Some(0) {
                    return Err(invalid("io.runners.threads", "zero threads"));
                }
            }
            "sim@1" => {
                let node = runner
                    .node
                    .as_deref()
                    .ok_or_else(|| invalid("io.runners.node", "sim@1 needs a node"))?;
                if !name(node) {
                    return Err(invalid("io.runners.node", "expected a lowercase node name"));
                }
                let model = runner
                    .models
                    .as_deref()
                    .ok_or_else(|| invalid("io.runners.models", "sim@1 needs a model"))?;
                let sites = sim_model_sites(model).ok_or_else(|| RunRefusal::Unsupported {
                    field: "io.runners.models".into(),
                })?;
                for (site, rate) in &runner.faults {
                    if !sites.contains(&site.as_str()) {
                        return Err(invalid(
                            "io.runners.faults",
                            "a fault site the modeled contract cannot produce",
                        ));
                    }
                    if rate.enable > 1_000_000 || rate.fire > 1_000_000 {
                        return Err(invalid("io.runners.faults", "rates are parts per million"));
                    }
                }
                if runner.threads.is_some() {
                    return Err(invalid("io.runners.threads", "sim@1 runs no threads"));
                }
            }
            _ => {
                return Err(RunRefusal::Unsupported {
                    field: "io.runners.provider".into(),
                });
            }
        }
        if runner.threads == Some(DEFAULT_THREADS) {
            runner.threads = None;
        }
        if runner.queue == Some(DEFAULT_QUEUE) {
            runner.queue = None;
        }
        match runner.recording.as_deref() {
            None | Some("none" | "record") => (),
            Some("durable") => {
                return Err(RunRefusal::Unsupported {
                    field: "io.runners.recording".into(),
                });
            }
            Some(_) => {
                return Err(invalid(
                    "io.runners.recording",
                    "expected none, record or durable",
                ));
            }
        }
        if runner.retry.is_some_and(|r| r.attempts == 0) {
            runner.retry = None;
        }
        if runner.cache.take().is_some_and(|cache| !cache.is_null()) {
            return Err(RunRefusal::Unsupported {
                field: "io.runners.cache".into(),
            });
        }
        if let Some(code) = &runner.code {
            let provider = code
                .as_object()
                .and_then(|o| o.get("provider"))
                .and_then(|p| p.as_str())
                .ok_or_else(|| invalid("io.runners.code", "expected a code store entry"))?;
            if !matches!(
                provider,
                "code.none@1" | "code.dir@1" | "code.object@1" | "code.sim@1"
            ) {
                return Err(RunRefusal::Unsupported {
                    field: "io.runners.code.provider".into(),
                });
            }
            if provider == "code.none@1" && code.as_object().is_some_and(|o| o.len() == 1) {
                runner.code = None;
            }
        }
        if let Some(admission) = &runner.admission {
            match admission
                .code
                .as_deref()
                .map(crate::ProviderContract::parse)
            {
                None | Some(Some(crate::ProviderContract::AdmitAny)) => runner.admission = None,
                Some(Some(crate::ProviderContract::AdmitSafe)) => {}
                Some(_) => {
                    return Err(RunRefusal::Unsupported {
                        field: "io.runners.admission.code".into(),
                    });
                }
            }
        }
        if let Some(io) = &mut runner.grants {
            if io.stdin.is_some() {
                return Err(invalid(
                    "io.runners.grants.stdin",
                    "a callee's grant set has no stdin",
                ));
            }
            if !io.runners.is_empty() {
                return Err(invalid(
                    "io.runners.grants.runners",
                    "a runner's grants name no runners",
                ));
            }
            grants(io, "io.runners.grants")?;
        }
        if let Some(definitions) = &runner.definitions {
            for path in definitions {
                if !definition_path(path) {
                    return Err(invalid(
                        "io.runners.definitions",
                        "expected full versioned definition paths",
                    ));
                }
            }
            if definitions.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(invalid(
                    "io.runners.definitions",
                    "definitions must be sorted and unique",
                ));
            }
        }
    }
    Ok(())
}

/// Validate one `io.runners` list on its own, as a test fixture names it.
pub fn validate_runners(bytes: &[u8]) -> Result<Vec<RunnerEntry>, RunRefusal> {
    let mut runners: Vec<RunnerEntry> =
        serde_json::from_slice(bytes).map_err(|error| invalid("io.runners", &error.to_string()))?;
    validate(&mut runners, crate::validate_io)?;
    for runner in &runners {
        if let Some(grants) = &runner.grants {
            crate::supported_io(grants, "io.runners.grants")?;
        }
    }
    Ok(runners)
}

impl RunnerEntry {
    pub fn threads(&self) -> u64 {
        self.threads.unwrap_or(DEFAULT_THREADS)
    }
    pub fn queue(&self) -> u64 {
        self.queue.unwrap_or(DEFAULT_QUEUE)
    }
    pub fn attempts(&self) -> u32 {
        self.retry.map_or(0, |r| r.attempts)
    }
    /// Whether the place admits only callees that reach no escape (CAP-6).
    pub fn admits_only_safe(&self) -> bool {
        self.admission.as_ref().is_some_and(|a| a.code.is_some())
    }
    /// The contract whose outcomes the runner produces: its own, or a node's model.
    pub fn contract(&self) -> &str {
        self.models.as_deref().unwrap_or(&self.provider)
    }
}
