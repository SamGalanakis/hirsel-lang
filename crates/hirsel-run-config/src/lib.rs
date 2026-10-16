//! One closed canonical run description. Parsed data never mints host authority.
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;

mod program;
pub use program::{ProgramNode, ProgramRevision, RevisionError};
mod selection;
pub use selection::select_run_record;
mod net;
pub use net::{
    NetConnection, NetGrant, NetPeer, NetRule, NetStep, PeerTls, TlsFailure,
    canonical_address as canonical_net_address, decode_net_peers, encode_net_peers,
    validate_net_grant,
};

pub const SCHEMA: u32 = 1;
pub const IDENTITY_DOMAIN: &str = "hirsel.run-config.v1";

/// Versioned observable contracts, shared by live hosts and simulation/replay.
/// Execution origin never changes the requested provider contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderContract {
    StdoutBytes,
    RandomSplitMix64,
    MonotonicNanoseconds,
    WallUnixNanoseconds,
    NetStream,
    NetScript,
}
impl ProviderContract {
    pub const ALL: [Self; 6] = [
        Self::StdoutBytes,
        Self::RandomSplitMix64,
        Self::MonotonicNanoseconds,
        Self::WallUnixNanoseconds,
        Self::NetStream,
        Self::NetScript,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::StdoutBytes => "stdout.bytes@1",
            Self::RandomSplitMix64 => "random.splitmix64@1",
            Self::MonotonicNanoseconds => "clock.monotonic.ns@1",
            Self::WallUnixNanoseconds => "clock.wall.unix_ns@1",
            Self::NetStream => "net.stream@1",
            Self::NetScript => "net.script@2",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|contract| contract.name() == name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchKind {
    Main,
    Test,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub hash_seed: u64,
    pub random_seed: u64,
    pub test_seed: u64,
    pub scheduler: Option<Scheduler>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scheduler {
    pub algorithm: String,
    pub version: u32,
    pub seed: u64,
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
    pub stdout: Option<String>,
    pub tasks: Option<String>,
    pub monotonic_clock: Option<String>,
    pub wall_clock: Option<String>,
    pub random: Option<String>,
    pub dirs: Vec<Mount>,
    pub net: Option<NetGrant>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    pub not_before: Option<i64>,
    pub expires: Option<i64>,
    pub revocation: Option<String>,
}

/// Untrusted requested endowments. This is the only serialized run schema.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    pub schema: u32,
    pub program: String,
    pub entry: String,
    pub launch: LaunchKind,
    pub inputs: Inputs,
    pub io: Option<IoGrants>,
    pub args: Option<Vec<String>>,
    #[serde(default, deserialize_with = "unique_env")]
    pub env: Option<BTreeMap<String, Option<String>>>,
    #[serde(rename = "unsafe")]
    pub unsafe_root: bool,
    pub admission: Option<Admission>,
}

/// Machine-readable refusal category and the exact offending input field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
}
impl fmt::Display for RunRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&serde_json::to_string(self).expect("refusal serialization"))
    }
}
impl std::error::Error for RunRefusal {}
fn invalid(field: &str, detail: &str) -> RunRefusal {
    RunRefusal::Invalid {
        field: field.into(),
        detail: detail.into(),
    }
}

/// Immutable normalized data. No public constructor or Deserialize implementation.
///
/// ```compile_fail
/// let forged: hirsel_run_config::ValidatedRun = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedRun {
    record: RunConfig,
    canonical: Vec<u8>,
    identity: [u8; 32],
}
/// Seeds and identity at the native ABI, produced only from validated record bytes.
///
/// ```compile_fail
/// let forged = hirsel_run_config::Config {
///     hash_seed: 0, random_seed: 0, test_seed: 0, run_identity: [0; 32],
/// };
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    hash_seed: u64,
    random_seed: u64,
    test_seed: u64,
    run_identity: [u8; 32],
}
impl Config {
    pub fn hash_seed(&self) -> u64 {
        self.hash_seed
    }
    pub fn random_seed(&self) -> u64 {
        self.random_seed
    }
    pub fn test_seed(&self) -> u64 {
        self.test_seed
    }
    pub fn run_identity(&self) -> [u8; 32] {
        self.run_identity
    }
}

impl RunConfig {
    /// Read an explicitly selected record; relative paths cannot resolve against cwd.
    pub fn read_path(path: &std::path::Path) -> Result<ValidatedRun, RunRefusal> {
        if !path.is_absolute() {
            return Err(invalid("--hirsel-run", "expected an absolute record path"));
        }
        let bytes =
            std::fs::read(path).map_err(|error| invalid("--hirsel-run", &error.to_string()))?;
        Self::validate(&bytes)
    }
    /// Deny all roots while selecting the canonical semantic dependency closure.
    pub fn for_program(program: &ProgramRevision, launch: LaunchKind) -> Self {
        Self::empty(program.digest(), program.entry().into(), launch)
    }
    /// Explicit denial default. Selecting code gives no authority.
    pub fn empty(program: [u8; 32], entry: String, launch: LaunchKind) -> Self {
        Self {
            schema: SCHEMA,
            program: hex(&program),
            entry,
            launch,
            inputs: Inputs::default(),
            io: None,
            args: None,
            env: None,
            unsafe_root: false,
            admission: None,
        }
    }
    /// Parse human JSON, rejecting duplicates, unknown fields and noncanonical numbers.
    /// Member order, whitespace and string escapes may normalize; grants never widen.
    pub fn validate(bytes: &[u8]) -> Result<ValidatedRun, RunRefusal> {
        integer_tokens(bytes)?;
        let mut record: Self =
            serde_json::from_slice(bytes).map_err(|e| RunRefusal::Malformed {
                detail: e.to_string(),
            })?;
        if record.schema != SCHEMA {
            return Err(RunRefusal::Schema {
                found: record.schema,
            });
        }
        digest(&record.program)
            .ok_or_else(|| invalid("program", "expected 64 lowercase hex digits"))?;
        if !definition_path(&record.entry) {
            return Err(invalid(
                "entry",
                "expected a full versioned definition path",
            ));
        }
        if let Some(scheduler) = &record.inputs.scheduler {
            provider("inputs.scheduler.algorithm", &scheduler.algorithm)?;
            if scheduler.version == 0 {
                return Err(invalid("inputs.scheduler.version", "zero version"));
            }
        }
        if let Some(io) = &mut record.io {
            for (field, value) in [
                ("io.stdout", &io.stdout),
                ("io.tasks", &io.tasks),
                ("io.monotonic_clock", &io.monotonic_clock),
                ("io.wall_clock", &io.wall_clock),
                ("io.random", &io.random),
            ] {
                if let Some(value) = value {
                    provider(field, value)?;
                }
            }
            io.dirs.sort_by(|a, b| a.path.cmp(&b.path));
            for mount in &io.dirs {
                if !portable_path(&mount.path, true) {
                    return Err(invalid("io.dirs.path", "noncanonical virtual mount"));
                }
                if !absolute_path(&mount.host) {
                    return Err(invalid(
                        "io.dirs.host",
                        "expected a canonical absolute host root",
                    ));
                }
            }
            let mut paths = BTreeSet::new();
            for mount in &io.dirs {
                if !paths.insert(mount.path.as_str()) {
                    return Err(invalid("io.dirs", "duplicate virtual mount"));
                }
            }
            for mount in &io.dirs {
                if !mount.path.is_empty() && paths.contains("") {
                    return Err(invalid("io.dirs", "overlapping virtual mounts"));
                }
                let mut path = mount.path.as_str();
                while let Some((parent, _)) = path.rsplit_once('/') {
                    if paths.contains(parent) {
                        return Err(invalid("io.dirs", "overlapping virtual mounts"));
                    }
                    path = parent;
                }
            }
            if let Some(net) = &mut io.net {
                provider("io.net.provider", &net.provider)?;
                net.validate()?;
            }
        }
        if let Some(env) = &record.env {
            for key in env.keys() {
                if key.is_empty() || key.contains(['=', '\0']) {
                    return Err(invalid("env", "invalid environment key"));
                }
            }
        }
        if let Some(admission) = &record.admission {
            if let (Some(start), Some(end)) = (admission.not_before, admission.expires)
                && start > end
            {
                return Err(invalid("admission", "not-before exceeds expiry"));
            }
            if let Some(reference) = &admission.revocation {
                provider("admission.revocation", reference)?;
            }
        }
        supported_providers(&record)?;
        let canonical = canonical_bytes(&record);
        let identity = hash_parts(IDENTITY_DOMAIN, [&canonical[..]]);
        Ok(ValidatedRun {
            record,
            canonical,
            identity,
        })
    }
    /// Builders pass through exactly the same parser/validator as standalone bytes.
    pub fn validated(&self) -> Result<ValidatedRun, RunRefusal> {
        Self::validate(&serde_json::to_vec(self).expect("run serialization"))
    }
}
impl ValidatedRun {
    /// Acquisition policy for both real and simulated adapters. Io presence alone
    /// grants no projections; clocks retain their independent grant booleans.
    pub fn io_projections(&self) -> BTreeSet<String> {
        let Some(io) = &self.record.io else {
            return BTreeSet::new();
        };
        [
            ("stdout", io.stdout.is_some()),
            ("tasks", io.tasks.is_some()),
            ("net", io.net.is_some()),
            ("random", io.random.is_some()),
            ("dir", !io.dirs.is_empty()),
        ]
        .into_iter()
        .filter(|(_, granted)| *granted)
        .map(|(name, _)| name.into())
        .collect()
    }
    pub fn clock_grants(&self) -> (bool, bool) {
        self.record.io.as_ref().map_or((false, false), |io| {
            (io.monotonic_clock.is_some(), io.wall_clock.is_some())
        })
    }
    /// Exact validated Net policy; peers supply data and never widen its rules.
    pub fn net_grant(&self) -> Option<&NetGrant> {
        self.record.io.as_ref()?.net.as_ref()
    }
    /// Bind launch to semantic code, independently of native artifact provenance.
    pub fn check_revision(
        &self,
        program: &ProgramRevision,
        launch: LaunchKind,
    ) -> Result<(), RunRefusal> {
        self.check_program(program.digest(), program.entry(), launch)
    }
    pub fn record(&self) -> &RunConfig {
        &self.record
    }
    pub fn canonical(&self) -> &[u8] {
        &self.canonical
    }
    pub fn identity(&self) -> [u8; 32] {
        self.identity
    }
    pub fn program(&self) -> [u8; 32] {
        digest(&self.record.program).expect("validated digest")
    }
    pub fn config(&self) -> Config {
        Config {
            hash_seed: self.record.inputs.hash_seed,
            random_seed: self.record.inputs.random_seed,
            test_seed: self.record.inputs.test_seed,
            run_identity: self.identity,
        }
    }
    /// Compare against compiled semantic code selection before program entry.
    /// Artifact provenance and compiler topology are deliberately not a binding key.
    pub fn check_program(
        &self,
        program: [u8; 32],
        entry: &str,
        launch: LaunchKind,
    ) -> Result<(), RunRefusal> {
        for (field, matches) in [
            ("program", self.program() == program),
            ("entry", self.record.entry == entry),
            ("launch", self.record.launch == launch),
        ] {
            if !matches {
                return Err(RunRefusal::Binding {
                    field: field.into(),
                });
            }
        }
        Ok(())
    }
    pub fn require_roots(&self, roots: &[Root]) -> Result<(), RunRefusal> {
        for root in roots {
            let present = match root {
                Root::Io => self.record.io.is_some(),
                Root::Args => self.record.args.is_some(),
                Root::Env => self.record.env.is_some(),
                Root::Unsafe => self.record.unsafe_root,
                Root::Test => self.record.launch == LaunchKind::Test,
            };
            if !present {
                return Err(RunRefusal::NotGranted {
                    root: format!("{root:?}"),
                });
            }
        }
        Ok(())
    }
    /// The host supplies authority and evidence; these inputs cannot come from the record.
    /// No time/revocation check is performed by operations on the admitted result.
    pub fn admit(&self, host: &mut dyn HostAuthority, now: i64) -> Result<AdmittedRun, RunRefusal> {
        let mut revocation = None;
        if let Some(admission) = &self.record.admission {
            if let Some(not_before) = admission.not_before
                && now < not_before
            {
                return Err(RunRefusal::NotYetValid { not_before, now });
            }
            if let Some(expires) = admission.expires
                && now > expires
            {
                return Err(RunRefusal::Expired { expires, now });
            }
            if let Some(reference) = &admission.revocation {
                let evidence = host.revocation(reference)?;
                if evidence.reference != *reference || evidence.revision.is_empty() {
                    return Err(RunRefusal::MissingEvidence {
                        reference: reference.clone(),
                    });
                }
                if evidence.revoked {
                    return Err(RunRefusal::Revoked {
                        reference: reference.clone(),
                    });
                }
                revocation = Some(evidence);
            }
        }
        host.authorize(&self.record)?;
        Ok(AdmittedRun {
            run: self.clone(),
            evidence: AdmissionEvidence {
                at: now,
                revocation,
            },
        })
    }
    /// Historical accepted admission replay. No HostAuthority is available on this path.
    pub fn check_replay(
        &self,
        canonical: &[u8],
        identity: [u8; 32],
        evidence: &AdmissionEvidence,
    ) -> Result<(), RunRefusal> {
        let recorded = RunConfig::validate(canonical)?;
        if recorded.canonical() != canonical {
            return Err(invalid("recording.run", "noncanonical header bytes"));
        }
        if recorded.identity != identity {
            return Err(RunRefusal::Binding {
                field: "recording.run_identity".into(),
            });
        }
        if self.identity != identity {
            return Err(RunRefusal::Binding {
                field: "run_identity".into(),
            });
        }
        if let Some(admission) = &recorded.record.admission {
            if let Some(start) = admission.not_before
                && evidence.at < start
            {
                return Err(RunRefusal::NotYetValid {
                    not_before: start,
                    now: evidence.at,
                });
            }
            if let Some(end) = admission.expires
                && evidence.at > end
            {
                return Err(RunRefusal::Expired {
                    expires: end,
                    now: evidence.at,
                });
            }
            match (&admission.revocation, &evidence.revocation) {
                (Some(reference), Some(proof))
                    if reference == &proof.reference
                        && !proof.revision.is_empty()
                        && !proof.revoked => {}
                (Some(reference), _) => {
                    return Err(RunRefusal::MissingEvidence {
                        reference: reference.clone(),
                    });
                }
                (None, None) => {}
                (None, Some(_)) => {
                    return Err(invalid(
                        "recording.admission",
                        "unexpected revocation evidence",
                    ));
                }
            }
        } else if evidence.revocation.is_some() {
            return Err(invalid(
                "recording.admission",
                "unexpected revocation evidence",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Root {
    Io,
    Args,
    Env,
    Unsafe,
    Test,
}
/// Implemented by a launch adapter which can actually enforce every requested restriction.
/// authorize must acquire/pin grants and reject recorder overlap before reporting success.
pub trait HostAuthority {
    fn authorize(&mut self, record: &RunConfig) -> Result<(), RunRefusal>;
    fn revocation(&mut self, reference: &str) -> Result<RevocationEvidence, RunRefusal>;
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevocationEvidence {
    pub reference: String,
    pub revision: String,
    pub revoked: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionEvidence {
    pub at: i64,
    pub revocation: Option<RevocationEvidence>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedRun {
    run: ValidatedRun,
    evidence: AdmissionEvidence,
}
impl AdmittedRun {
    pub fn run(&self) -> &ValidatedRun {
        &self.run
    }
    pub fn evidence(&self) -> &AdmissionEvidence {
        &self.evidence
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
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn digest(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}
pub fn hash_parts<'a>(domain: &'a str, parts: impl IntoIterator<Item = &'a [u8]>) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in std::iter::once(domain.as_bytes()).chain(parts) {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    hash.finalize().into()
}
fn provider(field: &str, name: &str) -> Result<(), RunRefusal> {
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(invalid(field, "empty or invalid provider/reference"));
    }
    Ok(())
}
// The closed provider names describe observable contracts,
// not Linux/simulator implementations. Unsupported policy cannot be widened.
fn supported_providers(record: &RunConfig) -> Result<(), RunRefusal> {
    let unsupported = |field: &str| RunRefusal::Unsupported {
        field: field.into(),
    };
    if record.inputs.scheduler.is_some() {
        return Err(unsupported("inputs.scheduler.algorithm"));
    }
    if let Some(io) = &record.io {
        for (field, value, expected) in [
            ("io.stdout", &io.stdout, Some(ProviderContract::StdoutBytes)),
            ("io.tasks", &io.tasks, None),
            (
                "io.monotonic_clock",
                &io.monotonic_clock,
                Some(ProviderContract::MonotonicNanoseconds),
            ),
            (
                "io.wall_clock",
                &io.wall_clock,
                Some(ProviderContract::WallUnixNanoseconds),
            ),
            (
                "io.random",
                &io.random,
                Some(ProviderContract::RandomSplitMix64),
            ),
        ] {
            if let Some(name) = value
                && (expected.is_none() || ProviderContract::parse(name) != expected)
            {
                return Err(unsupported(field));
            }
        }
        if let Some(net) = &io.net
            && !matches!(
                ProviderContract::parse(&net.provider),
                Some(ProviderContract::NetStream | ProviderContract::NetScript)
            )
        {
            return Err(unsupported("io.net.provider"));
        }
    }
    Ok(())
}
fn portable_path(path: &str, empty: bool) -> bool {
    (empty && path.is_empty())
        || (!path.is_empty()
            && path
                .split('/')
                .all(|p| !p.is_empty() && !matches!(p, "." | "..") && !p.contains(['\\', '\0'])))
}
fn absolute_path(path: &str) -> bool {
    path == "/"
        || path
            .strip_prefix('/')
            .is_some_and(|p| portable_path(p, false))
}
fn definition_path(path: &str) -> bool {
    fn name(s: &str) -> bool {
        let mut chars = s.bytes();
        chars
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && chars.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }
    let Some((package, tail)) = path.split_once('@') else {
        return false;
    };
    let Some((version, definition)) = tail.split_once('/') else {
        return false;
    };
    name(package)
        && !version.is_empty()
        && version.bytes().all(|b| b.is_ascii_digit())
        && version.parse::<u64>().is_ok()
        && (version == "0" || !version.starts_with('0'))
        && definition.split('/').all(|part| part.split('.').all(name))
}
fn integer_tokens(bytes: &[u8]) -> Result<(), RunRefusal> {
    let (mut i, mut string) = (0, false);
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if string => {
                i += 2;
                continue;
            }
            b'"' => string = !string,
            b'-' | b'0'..=b'9' if !string => {
                let start = i;
                i += 1;
                while i < bytes.len() && !b" \t\r\n,]}:".contains(&bytes[i]) {
                    i += 1;
                }
                let word = &bytes[start..i];
                let digits = word.strip_prefix(b"-").unwrap_or(word);
                if digits.is_empty()
                    || !digits.iter().all(u8::is_ascii_digit)
                    || (digits.len() > 1 && digits[0] == b'0')
                    || word == b"-0"
                {
                    return Err(RunRefusal::Malformed {
                        detail: "noncanonical integer".into(),
                    });
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}
fn unique_env<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> Result<Option<BTreeMap<String, Option<String>>>, D::Error> {
    struct Entries;
    impl<'de> serde::de::Visitor<'de> for Entries {
        type Value = Option<BTreeMap<String, Option<String>>>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("null or a unique Env snapshot object")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: serde::Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
            de.deserialize_map(self)
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, Option<String>>()? {
                if values.insert(key.clone(), value).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate Env key {key:?}"
                    )));
                }
            }
            Ok(Some(values))
        }
    }
    de.deserialize_option(Entries)
}
fn canonical_bytes(record: &RunConfig) -> Vec<u8> {
    struct Canonical;
    impl serde_json::ser::Formatter for Canonical {
        fn write_char_escape<W: ?Sized + std::io::Write>(
            &mut self,
            writer: &mut W,
            escape: serde_json::ser::CharEscape,
        ) -> std::io::Result<()> {
            use serde_json::ser::CharEscape::*;
            match escape {
                Quote => writer.write_all(b"\\\""),
                ReverseSolidus => writer.write_all(b"\\\\"),
                Solidus => writer.write_all(b"/"),
                AsciiControl(c) => write!(writer, "\\u{c:04x}"),
                Backspace => writer.write_all(b"\\u0008"),
                FormFeed => writer.write_all(b"\\u000c"),
                LineFeed => writer.write_all(b"\\u000a"),
                CarriageReturn => writer.write_all(b"\\u000d"),
                Tab => writer.write_all(b"\\u0009"),
            }
        }
    }
    let mut bytes = vec![];
    record
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut bytes, Canonical,
        ))
        .expect("run serialization");
    bytes
}
