//! Runner entries (RUN-1, RUN-13, docs/spec/runners.md "Providers"): the places a run
//! may call functions at, each a provider contract with its settings, its seam members
//! and, for a place that runs Hirsel code with authority, the grant set and definition
//! allowlist its callees run under. A member equal to its default is absent from
//! canonical bytes, and a record without runners spells no `runners` member at all.
use crate::IoGrants;
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
