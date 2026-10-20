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
    /// `remote.tls@1`: the runner host's endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<Endpoint>,
    /// `remote.tls@1`: the name the host's certificate must carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    /// `remote.tls@1`: the trust anchors the host's certificate is verified against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<Roots>,
    /// `remote.tls@1`: the client identity presented when the host asks for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<TlsFiles>,
    /// `remote.tls@1`: the bound on the wait for an outcome after the commit point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ns: Option<u64>,
    /// `remote.tls@1`: reconnection attempts after a lost connection (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconnect: Option<Retry>,
    /// `remote.tls@1`: the host must record call keys durably before admitting them, so
    /// its status answers prove a call never ran (default false, absent).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reconcile: bool,
}
/// A runner host's endpoint: `{"tcp": {"address", "service"}}` or `{"unix": path}`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum Endpoint {
    Tcp { address: String, service: String },
    Unix(String),
}
/// Trust anchors: `"webpki"` (the built-in set) or `{"pem": absolute path}`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Roots {
    Named(String),
    Pem { pem: String },
}
/// A certificate chain and its key, as absolute paths of PEM files.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsFiles {
    pub chain: String,
    pub key: String,
}
/// `remote.tls@1`'s default reconnection attempts.
pub const DEFAULT_RECONNECT: u32 = 1;
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
    /// `remote.tls@1`'s reconnection attempts.
    pub fn reconnects(&self) -> u32 {
        self.reconnect.map_or(DEFAULT_RECONNECT, |r| r.attempts)
    }
    /// The contract whose outcomes the runner produces: its own, or a node's model.
    pub fn contract(&self) -> &str {
        self.models.as_deref().unwrap_or(&self.provider)
    }
}

/// How a place met one call (RUN-5 to RUN-7): refused before its commit point with a
/// `NotRunReason` ordinal, lost after it with an `InDoubtReason` ordinal, or sent, with the
/// callee suspension after which `sim@1` crashes it and whether the reply is to be lost.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Placed {
    NotRun(u32),
    InDoubt(u32),
    Sent {
        crash_after: Option<u32>,
        lose_reply: bool,
    },
}
const UNREACHABLE: u32 = 2;
const OVERLOADED: u32 = 3;
const LOST: u32 = 0;

/// `sim@1`'s crash points: a crash stops a callee before one of its first this many
/// requests (or suspensions).
pub const CRASH_POINTS: u64 = 32;

/// One place's state in one run: the fault sites enabled at its first call, the callees
/// sent and not yet answered, and whether it is partitioned. Both engines decide calls here.
#[derive(Clone, Debug, Default)]
pub struct PlaceState {
    enabled: Option<std::collections::BTreeSet<String>>,
    pub running: u64,
    partitioned: bool,
}

/// The first eight bytes, little-endian, of a domain-separated hash of `parts`.
fn draw(domain: &str, parts: &[&[u8]]) -> u64 {
    let hash = crate::hash_parts(domain, parts.iter().copied());
    u64::from_le_bytes(hash[..8].try_into().unwrap())
}
/// Whether a draw in parts per million falls under `rate`.
fn fires(rate: u32, domain: &str, parts: &[&[u8]]) -> bool {
    draw(domain, parts) % 1_000_000 < u64::from(rate)
}

impl PlaceState {
    /// The place's contract for call `ordinal` of `entry`: capacity, `sim@1`'s faults drawn
    /// from `fault_seed` and re-sends of a request refused before its commit point (RUN-7).
    /// A sent call counts as running until [`PlaceState::answered`].
    pub fn place(&mut self, entry: &RunnerEntry, fault_seed: u64, ordinal: u64) -> Placed {
        let seed = fault_seed.to_le_bytes();
        let enabled = self.enabled.get_or_insert_with(|| {
            entry
                .faults
                .iter()
                .filter(|(site, rate)| {
                    fires(
                        rate.enable,
                        "hirsel.runner.fault.enable.v1",
                        &[&seed, entry.name.as_bytes(), site.as_bytes()],
                    )
                })
                .map(|(site, _)| site.clone())
                .collect()
        });
        let fire = |site: &str, attempt: u32| {
            enabled.contains(site)
                && fires(
                    entry.faults[site].fire,
                    "hirsel.runner.fault.fire.v1",
                    &[
                        &seed,
                        entry.name.as_bytes(),
                        site.as_bytes(),
                        &ordinal.to_le_bytes(),
                        &attempt.to_le_bytes(),
                    ],
                )
        };
        let capacity = entry.threads() + entry.queue();
        let mut attempt = 0;
        let refusal = loop {
            let refusal = if self.partitioned && !fire("partition", attempt) {
                Some(UNREACHABLE)
            } else if fire("partition", attempt) && !self.partitioned {
                self.partitioned = true;
                Some(UNREACHABLE)
            } else if fire("refuse", attempt) {
                Some(UNREACHABLE)
            } else if self.running >= capacity || fire("overload", attempt) {
                Some(OVERLOADED)
            } else {
                self.partitioned = false;
                None
            };
            match refusal {
                Some(_) if attempt < entry.attempts() => attempt += 1,
                refusal => break refusal,
            }
        };
        if let Some(reason) = refusal {
            return Placed::NotRun(reason);
        }
        // The commit point: the request is sent.
        if fire("lose_request", attempt) {
            return Placed::InDoubt(LOST);
        }
        self.running += 1;
        // The crash point: one of the callee's first 32 requests, or suspensions where it
        // makes none; a callee that ends first has its outcome lost.
        let crash_after = fire("crash", attempt).then(|| {
            (draw(
                "hirsel.runner.fault.crash.v1",
                &[&seed, entry.name.as_bytes(), &ordinal.to_le_bytes()],
            ) % CRASH_POINTS) as u32
        });
        Placed::Sent {
            crash_after,
            lose_reply: fire("lose_reply", attempt),
        }
    }
    /// A sent call was answered: its place is free.
    pub fn answered(&mut self) {
        self.running = self.running.saturating_sub(1);
    }
}
