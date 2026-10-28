//! Shared run data and engine helpers. Record parsing lives in hirsel-run-config.
#[cfg(test)]
hirsel_guard::install!();

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;

mod file_faults;
pub use file_faults::{
    FileFault, FileFaultError, FileFaults, FileOperation, take_file_fault, validate_file_faults,
};
mod file_reads;
mod path;
pub use file_reads::{FileReadScript, FileReadiness};
pub mod ownership_trace;
pub mod tape;
pub use path::confined_relative;

mod calls;
mod grants;
mod runners;
pub use calls::{
    CallCause, CallForm, CallOutcome, CallRecord, CalleeFacts, CalleeOutcome, Cause,
    RunnerCompletion, RunnerRequest, call_key, callee_instance, callee_seed, callees_json,
    parse_callees, request_digest,
};
pub use grants::{
    GrantSet, NET_OPERATIONS, allowlist_bytes, env_within, io_within, mount_within, narrow,
    net_operation_within, net_rule_within, process_rule_within,
};
pub use runners::{
    DEFAULT_QUEUE, DEFAULT_RECONNECT, DEFAULT_THREADS, Endpoint, FaultRate, PlaceAdmission,
    PlaceState, Placed, Retry, Roots, RunnerEntry, SIM_FAULT_SITES, TlsFiles, sim_model_sites,
};
mod outcome;
pub use outcome::{
    BudgetKind, Environmental, ExecutionDiagnostic, Failure, Returned, RunOutcome, Site,
    ToolingLimit, Wait, exit,
};
mod program;
pub use program::{ProgramGraph, ProgramNode, ProgramRevision, RevisionError};
pub const DEPTH_SEMANTICS: &str = hirsel_core_contract::depth::SEMANTICS;
mod net;
pub use net::{
    NetClient, NetConnection, NetGrant, NetListener, NetPeer, NetRule, NetStep, PeerTls,
    TlsFailure, TlsIdentity, TlsServer, UNIX_OPERATIONS, UNIX_PATH_MAX,
    canonical_address as canonical_net_address, canonical_unix_path, check_net_listeners,
    check_peer_address, decode_net_listeners, decode_net_peers, encode_net_listeners,
    encode_net_peers, first_expecting_client, listener_name, net_client_mismatch,
    net_client_unaccepted, net_client_unmet,
};
mod child;
mod signals;
pub use signals::{SignalKind, SignalsGrant, signal_kinds};
pub mod tls;
pub use child::{ChildStream, ScriptChild, ScriptGroup, ScriptRead, Signal};
mod process;
pub use process::{
    ChildRun, ChildStep, Command, ExitStatus, GroupOutput, PipelineOutcome, ProcessFailure,
    ProcessGrant, ProcessOutcome, ProcessRule, ProcessRun, TimedChildRun, TimedChildStep,
    TimedProcessScript, canonical_program, decode_child_runs, decode_process_runs,
    encode_child_runs, encode_process_runs, script_pipeline, scriptable, stages_error,
    take_children, take_timed_children,
};

/// Versioned observable contracts, shared by live hosts and simulation/replay.
/// Execution origin never changes the requested provider contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderContract {
    TasksCooperative,
    TasksFree,
    CpuScoped,
    StdinBytes,
    StderrBytes,
    StdoutBytes,
    RandomSplitMix64,
    RandomSystem,
    MonotonicNanoseconds,
    WallUnixNanoseconds,
    NetStream,
    NetScript,
    ProcessSpawn,
    ProcessScript,
    ProcessScriptTimed,
    SignalsHost,
    SignalsScript,
    AdmitAny,
    AdmitSafe,
    BudgetsNone,
    BudgetsLimits,
    RunnerLocalThreads,
    RunnerSim,
    RunnerRemoteTls,
    TelemetryCapture,
    TelemetryNull,
}
impl ProviderContract {
    pub const ALL: &[Self] = &[
        Self::AdmitAny,
        Self::AdmitSafe,
        Self::BudgetsLimits,
        Self::BudgetsNone,
        Self::MonotonicNanoseconds,
        Self::WallUnixNanoseconds,
        Self::CpuScoped,
        Self::RunnerLocalThreads,
        Self::NetScript,
        Self::NetStream,
        Self::ProcessScriptTimed,
        Self::ProcessScript,
        Self::ProcessSpawn,
        Self::RandomSplitMix64,
        Self::RandomSystem,
        Self::RunnerRemoteTls,
        Self::SignalsHost,
        Self::SignalsScript,
        Self::RunnerSim,
        Self::StderrBytes,
        Self::StdinBytes,
        Self::StdoutBytes,
        Self::TasksCooperative,
        Self::TasksFree,
        Self::TelemetryCapture,
        Self::TelemetryNull,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::TasksCooperative => "tasks.cooperative@1",
            Self::TasksFree => "tasks.free@1",
            Self::CpuScoped => "cpu.scoped@1",
            Self::StdinBytes => "stdin.bytes@1",
            Self::StderrBytes => "stderr.bytes@1",
            Self::StdoutBytes => "stdout.bytes@1",
            Self::RandomSplitMix64 => "random.splitmix64@1",
            Self::RandomSystem => "random.system@1",
            Self::MonotonicNanoseconds => "clock.monotonic.ns@1",
            Self::WallUnixNanoseconds => "clock.wall.unix_ns@1",
            Self::NetStream => "net.stream@1",
            Self::NetScript => "net.script@2",
            Self::ProcessSpawn => "process.spawn@1",
            Self::ProcessScript => "process.script@1",
            Self::ProcessScriptTimed => "process.script.timed@1",
            Self::SignalsHost => "signals.host@1",
            Self::SignalsScript => "signals.script@1",
            Self::AdmitAny => "admit.any@1",
            Self::AdmitSafe => "admit.safe@1",
            Self::BudgetsNone => "budgets.none@1",
            Self::BudgetsLimits => "budgets.limits@2",
            Self::RunnerLocalThreads => "local.threads@1",
            Self::RunnerSim => "sim@1",
            Self::RunnerRemoteTls => "remote.tls@1",
            Self::TelemetryCapture => "telemetry.capture@1",
            Self::TelemetryNull => "telemetry.null@1",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|contract| contract.name() == name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchKind {
    Main,
    Test,
    /// A callee run answering a runner call (RUN-10); its record has a `call` member.
    Call,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    /// Initial virtual clock bound only for the timed process fixture provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_monotonic: Option<u64>,
    /// Test snapshot scripts bound into recording identity; never host injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_file_faults: Option<FileFaults>,
    pub hash_seed: u64,
    pub random_seed: u64,
    pub test_seed: u64,
    pub scheduler: Option<Scheduler>,
    /// The seed `sim@1` runners draw their faults from; absent from canonical bytes when
    /// absent, so records without one keep their identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault_seed: Option<u64>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scheduler {
    pub algorithm: String,
    pub version: u32,
    pub seed: u64,
}
impl Scheduler {
    /// The supported scheduler contracts by launch word (docs/spec/determinism.md);
    /// `hirsel_kernels::schedule::Algorithm::code` uses the same words.
    pub const CONTRACTS: [(&'static str, u64); 3] = [("fifo", 0), ("random", 1), ("pct", 2)];
    /// The launch word of a supported contract, version 1.
    pub fn code(&self) -> Option<u64> {
        Self::CONTRACTS
            .into_iter()
            .find(|(name, _)| *name == self.algorithm && self.version == 1)
            .map(|(_, code)| code)
    }
    /// `name@1:seed`, the spelling of `hirsel test --scheduler`.
    pub fn parse(text: &str) -> Option<Scheduler> {
        let (contract, seed) = text.split_once(':')?;
        let (algorithm, version) = contract.split_once('@')?;
        let scheduler = Scheduler {
            algorithm: algorithm.into(),
            version: version
                .parse()
                .ok()
                .filter(|v: &u32| v.to_string() == version)?,
            seed: seed.parse().ok().filter(|s: &u64| s.to_string() == seed)?,
        };
        scheduler.code().map(|_| scheduler)
    }
    pub fn from_code(code: u64, seed: u64) -> Option<Scheduler> {
        Self::CONTRACTS
            .into_iter()
            .find(|(_, word)| *word == code)
            .map(|(name, _)| Scheduler {
                algorithm: name.into(),
                version: 1,
                seed,
            })
    }
}
impl std::fmt::Display for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}:{}", self.algorithm, self.version, self.seed)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    ReadWrite,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mount {
    pub path: String,
    pub host: String,
    pub access: Access,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IoGrants {
    pub stdin: Option<String>,
    pub stderr: Option<String>,
    pub stdout: Option<String>,
    pub tasks: Option<String>,
    pub monotonic_clock: Option<String>,
    pub wall_clock: Option<String>,
    pub random: Option<String>,
    pub dirs: Vec<Mount>,
    pub net: Option<NetGrant>,
    pub process: Option<ProcessGrant>,
    /// The runners this run calls through (RUN-1), sorted and unique by name; absent from
    /// canonical bytes when there are none, so records without runners keep their identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runners: Vec<RunnerEntry>,
    /// The Telemetry capture provider; absent from canonical bytes when absent, so
    /// records without telemetry keep their identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<TelemetryGrant>,
    /// Omitted, not null, when denied, so records without signals keep the bytes and
    /// identity they had before the member existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signals: Option<SignalsGrant>,
    /// Finite scoped CPU service authority, subordinate to the Tasks grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuGrant>,
}
/// A scoped CPU service's bounds. Validation requires Tasks, a known provider,
/// 1..=16 workers, one active domain, and positive byte limits.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuGrant {
    pub provider: String,
    pub max_workers: u32,
    pub max_domains: u32,
    pub max_physical_bytes: u64,
    pub max_batch_bytes: u64,
}
/// `io.telemetry`: `telemetry.capture@1` (span identities from the host's secure
/// generator) or `telemetry.null@1` (identities from `seed`, and, with `queue`, a simulated
/// exporter of that capacity that never drains). Neither changes what a program observes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryGrant {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<u64>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    pub not_before: Option<i64>,
    pub expires: Option<i64>,
    pub revocation: Option<String>,
    /// The code admission policy (CAP-6). Its default, `admit.any@1`, is absent from
    /// canonical bytes, so records without one keep their identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// What the selected code reaches (CAP-6), found by the launcher from checked code,
/// never read from the record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodeReach {
    Safe,
    /// The path from the entry to its first escape, each definition's versioned
    /// spelling; empty where the launcher keeps only the verdict (an executable).
    Escapes(Vec<String>),
}
/// The run's resource budgets: host policy, never authority. `budgets.none@1`
/// imposes none and normalizes to an absent entry; `budgets.limits@2` bounds the
/// run's fuel (function entries and loop back-edges) and its logical storage (live blocks'
/// size classes, large requests rounded up to 4 KiB, regions' canonical segments), each null
/// for unbounded and at least one present; it retires `budgets.limits@1`, which refuses.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    pub provider: String,
    #[serde(default)]
    pub fuel: Option<u64>,
    #[serde(default)]
    pub memory_bytes: Option<u64>,
}
impl BudgetKind {
    /// The kind's name in an observation record and a diagnostic.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fuel => "fuel",
            Self::Memory => "memory",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Fuel, Self::Memory]
            .into_iter()
            .find(|kind| kind.name() == name)
    }
}

/// How a host run is recorded. The level never changes what the program may do or observe.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingLevel {
    /// Nothing is recorded; a recording destination refuses.
    None,
    /// The ordinary tape, which cannot be resumed.
    Record,
    /// The durable journal: each request durable before issue, each completion before delivery.
    Durable,
}

/// Machine-readable refusal category and the exact offending input field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunRefusal {
    Malformed { detail: String },
    InvalidInput { item: String },
    Schema { found: u32 },
    Invalid { field: String, detail: String },
    Binding { field: String },
    NotGranted { root: String },
    Unsupported { field: String },
    Unauthorized { field: String },
    NotYetValid { not_before: i64, now: i64 },
    Expired { expires: i64, now: i64 },
    Revoked { reference: String },
    MissingEvidence { reference: String },
    RecorderOverlap { mount: String },
    UnsafeCode { path: Vec<String> },
}
impl fmt::Display for RunRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&serde_json::to_string(self).expect("refusal serialization"))
    }
}
impl std::error::Error for RunRefusal {}
impl RunRefusal {
    /// A refusal from its JSON (`Display`), as a launcher reports it to its host.
    pub fn parse(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}
fn invalid(field: &str, detail: &str) -> RunRefusal {
    RunRefusal::Invalid {
        field: field.into(),
        detail: detail.into(),
    }
}

/// Snapshot only explicitly supplied arguments; this never reads process argv.
pub fn snapshot_args(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Vec<String>, RunRefusal> {
    arguments
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            value.into_string().map_err(|_| RunRefusal::InvalidInput {
                item: format!("args[{index}]"),
            })
        })
        .collect()
}
/// Snapshot only explicitly granted keys. None means granted but unset; omission
/// means denied. No live process-environment lookup or lossy UTF-8 conversion.
pub fn snapshot_env(
    entries: impl IntoIterator<Item = (OsString, Option<OsString>)>,
) -> Result<BTreeMap<String, Option<String>>, RunRefusal> {
    let mut snapshot = BTreeMap::new();
    for (index, (key, value)) in entries.into_iter().enumerate() {
        let key = key.into_string().map_err(|_| RunRefusal::InvalidInput {
            item: format!("env.key[{index}]"),
        })?;
        if key.is_empty() || key.contains(['=', '\0']) {
            return Err(invalid("env", "invalid environment key"));
        }
        let value = value
            .map(|value| {
                value.into_string().map_err(|_| RunRefusal::InvalidInput {
                    item: format!("env[{key:?}]"),
                })
            })
            .transpose()?;
        if snapshot.insert(key, value).is_some() {
            return Err(invalid("env", "duplicate environment key"));
        }
    }
    Ok(snapshot)
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}
pub fn hash_parts<'a>(domain: &'a str, parts: impl IntoIterator<Item = &'a [u8]>) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in std::iter::once(domain.as_bytes()).chain(parts) {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    hash.finalize().into()
}
