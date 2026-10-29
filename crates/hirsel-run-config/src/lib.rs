//! One closed canonical run description. Parsed data never mints host authority.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_run_schema::{
    Admission, Budgets, CallRecord, CodeReach, Inputs, IoGrants, LaunchKind, NetGrant,
    ProcessGrant, ProgramRevision, ProviderContract, RecordingLevel, RunRefusal, Scheduler,
    SignalsGrant, call_key, callee_instance, hash_parts, hex,
};
use serde::{Deserialize, Serialize};
pub mod managed_cores;
mod placement;
pub use placement::{
    AutoCores, CapacityAdmission, CapacityExclusion, Commit, CoreRange, Cores, Pin, Placement,
    RecordedPlacement, Reorder,
};
mod calls;
mod object_stores;
pub use object_stores::{CredentialsReference, ObjectStore};

mod grants;
pub use grants::grants_within;
pub mod host;
mod runners;
pub use runners::validate_runners;
mod net;
mod process;
pub use net::validate_net_grant;
pub use process::validate_process_grant;
mod cpu;
mod signals;
use sha2::{Digest as _, Sha256};
pub use signals::validate_signals_grant;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A host-supplied, immutable admission snapshot. Its revision hashes the exact file.
#[derive(Clone, Debug)]
pub struct Revocations {
    revoked: BTreeSet<String>,
    revision: String,
}
impl Revocations {
    pub fn read_path(path: &std::path::Path) -> Result<Self, RunRefusal> {
        let bytes = std::fs::read(path)
            .map_err(|e| invalid("revocations", &format!("{}: {e}", path.display())))?;
        Self::validate(&bytes)
    }
    pub fn validate(bytes: &[u8]) -> Result<Self, RunRefusal> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct List {
            schema: u32,
            revoked: Vec<String>,
        }
        let list: List =
            serde_json::from_slice(bytes).map_err(|e| invalid("revocations", &e.to_string()))?;
        if list.schema != 1 || list.revoked.iter().any(|r| r.is_empty()) {
            return Err(invalid(
                "revocations",
                "expected schema 1 and nonempty references",
            ));
        }
        let revision = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(Self {
            revoked: list.revoked.into_iter().collect(),
            revision,
        })
    }
    pub fn evidence(&self, reference: &str) -> RevocationEvidence {
        RevocationEvidence {
            reference: reference.into(),
            revision: self.revision.clone(),
            revoked: self.revoked.contains(reference),
        }
    }
}

pub const SCHEMA: u32 = 1;
pub const IDENTITY_DOMAIN: &str = "hirsel.run-config.v1";

/// Untrusted requested endowments. This is the only serialized run schema.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    pub schema: u32,
    /// Missing legacy revisions deserialize only to produce a precise refusal.
    #[serde(default)]
    pub depth_semantics: String,
    pub program: String,
    pub entry: String,
    pub launch: LaunchKind,
    /// A callee run's call (RUN-10): present exactly when `launch` is `call`, and absent
    /// from every other record's canonical bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<CallRecord>,
    pub inputs: Inputs,
    pub io: Option<IoGrants>,
    pub args: Option<Vec<String>>,
    #[serde(default, deserialize_with = "unique_env")]
    pub env: Option<BTreeMap<String, Option<String>>>,
    #[serde(rename = "unsafe")]
    pub unsafe_root: bool,
    pub admission: Option<Admission>,
    /// Omitted, not null, under `budgets.none@1`, so records without budgets keep the bytes
    /// and identity they had before budgets existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budgets: Option<Budgets>,
    /// The recording level, a run-policy member (determinism.md, Recording levels). Absent,
    /// the default, leaves it to the launch: a recording destination records the tape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<RecordingLevel>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "object_stores::unique"
    )]
    pub object_stores: BTreeMap<String, ObjectStore>,
    /// Validated execution policy, always removed from canonical identity bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
}

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
    data: std::sync::Arc<ValidatedData>,
}

#[derive(Debug, Eq, PartialEq)]
struct ValidatedData {
    program: [u8; 32],
    record: RunConfig,
    canonical: Vec<u8>,
    identity: [u8; 32],
}
/// Seeds and identity at the native ABI, produced only from validated record bytes.
///
/// ```compile_fail
/// let forged = hirsel_run_config::Config {
///     hash_seed: 0, random_seed: 0, test_seed: 0, scheduler: 0, scheduler_seed: 0,
///     run_identity: [0; 32],
/// };
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    hash_seed: u64,
    random_seed: u64,
    test_seed: u64,
    /// The scheduler contract's launch word ([`Scheduler::CONTRACTS`]); 0 is `fifo@1`.
    scheduler: u64,
    scheduler_seed: u64,
    run_identity: [u8; 32],
}
impl Config {
    /// The scheduler contract's launch word and its seed.
    pub fn scheduler(&self) -> (u64, u64) {
        (self.scheduler, self.scheduler_seed)
    }
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
    /// Bind explicit grants to the selected code, then use ordinary record validation.
    /// Binding fields are not template inputs: they always come from the launcher.
    pub fn bind_grants(
        bytes: &[u8],
        program: [u8; 32],
        entry: String,
        launch: LaunchKind,
    ) -> Result<ValidatedRun, RunRefusal> {
        Self::template_at(bytes, program, entry, launch, None)?.validated()
    }

    /// The record a grants template binds to the selected code, before validation.
    fn template_at(
        bytes: &[u8],
        program: [u8; 32],
        entry: String,
        launch: LaunchKind,
        base: Option<&std::path::Path>,
    ) -> Result<RunConfig, RunRefusal> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Grants {
            #[serde(default)]
            inputs: Inputs,
            io: Option<IoGrants>,
            args: Option<Vec<String>>,
            #[serde(default, deserialize_with = "unique_env")]
            env: Option<BTreeMap<String, Option<String>>>,
            #[serde(default, rename = "unsafe")]
            unsafe_root: bool,
            admission: Option<Admission>,
            #[serde(default)]
            budgets: Option<Budgets>,
            recording: Option<RecordingLevel>,
            #[serde(default, deserialize_with = "object_stores::unique")]
            object_stores: BTreeMap<String, ObjectStore>,
            placement: Option<Placement>,
        }
        integer_tokens(bytes)?;
        let mut grants: Grants = schema_diagnostic::from_slice(bytes)?;
        if let (Some(base), Some(io)) = (base, &mut grants.io) {
            // A runner's grant set names host directories as the run's own mounts do.
            let runners = io.runners.iter_mut().filter_map(|r| r.grants.as_mut());
            let mounts: Vec<&mut hirsel_run_schema::Mount> =
                runners.flat_map(|grants| grants.dirs.iter_mut()).collect();
            for mount in io.dirs.iter_mut().chain(mounts) {
                let host = std::path::Path::new(&mount.host);
                if host.is_relative() {
                    let candidate = base.join(host);
                    let canonical = candidate.canonicalize().map_err(|e| {
                        invalid(
                            "io.dirs.host",
                            &format!(
                                "{}: {e}; relative host paths resolve relative to the grants file; host {:?}; grants file directory {}; resolved candidate {}",
                                candidate.display(),
                                mount.host,
                                base.display(),
                                candidate.display()
                            ),
                        )
                    })?;
                    mount.host = canonical
                        .to_str()
                        .ok_or_else(|| invalid("io.dirs.host", "directory path is not UTF-8"))?
                        .into();
                }
            }
        }
        let mut record = Self::empty(program, entry, launch);
        record.inputs = grants.inputs;
        record.io = grants.io;
        record.args = grants.args;
        record.env = grants.env;
        record.unsafe_root = grants.unsafe_root;
        record.admission = grants.admission;
        record.budgets = grants.budgets;
        record.recording = grants.recording;
        record.object_stores = grants.object_stores;
        record.placement = grants.placement;
        Ok(record)
    }

    /// Select inline JSON only when the argument starts with `{`; otherwise read a path.
    /// Inline templates have no file location, so directory hosts must be absolute.
    pub fn grants_input(
        input: &std::ffi::OsStr,
        program: [u8; 32],
        entry: String,
        launch: LaunchKind,
    ) -> Result<ValidatedRun, RunRefusal> {
        Self::grants_template(input, program, entry, launch)?.validated()
    }

    /// The record [`Self::grants_input`] validates, for a launcher that completes it (its
    /// argument snapshot) before the one validation.
    pub fn grants_template(
        input: &std::ffi::OsStr,
        program: [u8; 32],
        entry: String,
        launch: LaunchKind,
    ) -> Result<RunConfig, RunRefusal> {
        if input.as_encoded_bytes().starts_with(b"{") {
            Self::template_at(input.as_encoded_bytes(), program, entry, launch, None)
        } else {
            Self::read_template(std::path::Path::new(input), program, entry, launch)
        }
    }

    /// Select an inline bound record or an explicit absolute record path.
    pub fn record_input(input: &std::ffi::OsStr) -> Result<ValidatedRun, RunRefusal> {
        if input.as_encoded_bytes().starts_with(b"{") {
            Self::validate(input.as_encoded_bytes())
        } else {
            Self::read_path(std::path::Path::new(input))
        }
    }

    /// The record [`Self::record_input`] selects, parsed but not validated, for a launcher
    /// that completes it before the one validation.
    pub fn record_template(input: &std::ffi::OsStr) -> Result<RunConfig, RunRefusal> {
        if input.as_encoded_bytes().starts_with(b"{") {
            Self::parse(input.as_encoded_bytes())
        } else {
            Self::parse(&Self::read_record(std::path::Path::new(input))?)
        }
    }

    /// Read the explicitly selected template. Relative template paths are tooling inputs.
    pub fn read_grants(
        path: &std::path::Path,
        program: [u8; 32],
        entry: String,
        launch: LaunchKind,
    ) -> Result<ValidatedRun, RunRefusal> {
        Self::read_template(path, program, entry, launch)?.validated()
    }

    fn read_template(
        path: &std::path::Path,
        program: [u8; 32],
        entry: String,
        launch: LaunchKind,
    ) -> Result<RunConfig, RunRefusal> {
        let bytes = std::fs::read(path)
            .map_err(|e| invalid("grants", &format!("{}: {e}", path.display())))?;
        Self::template_at(&bytes, program, entry, launch, path.parent())
    }

    /// Read an explicitly selected record; relative paths cannot resolve against cwd.
    pub fn read_path(path: &std::path::Path) -> Result<ValidatedRun, RunRefusal> {
        Self::validate(&Self::read_record(path)?)
    }

    fn read_record(path: &std::path::Path) -> Result<Vec<u8>, RunRefusal> {
        if !path.is_absolute() {
            return Err(invalid("--hirsel-run", "expected an absolute record path"));
        }
        std::fs::read(path)
            .map_err(|error| invalid("--hirsel-run", &format!("{}: {error}", path.display())))
    }
    /// Deny all roots while selecting the canonical semantic dependency closure.
    pub fn for_program(program: &ProgramRevision, launch: LaunchKind) -> Self {
        Self::empty(program.digest(), program.entry().into(), launch)
    }
    /// Explicit denial default. Selecting code gives no authority.
    pub fn empty(program: [u8; 32], entry: String, launch: LaunchKind) -> Self {
        Self {
            schema: SCHEMA,
            depth_semantics: hirsel_run_schema::DEPTH_SEMANTICS.into(),
            program: hex(&program),
            entry,
            launch,
            call: None,
            inputs: Inputs::default(),
            io: None,
            args: None,
            env: None,
            unsafe_root: false,
            admission: None,
            budgets: None,
            recording: None,
            object_stores: BTreeMap::new(),
            placement: None,
        }
    }
    /// Parse human JSON, rejecting duplicates, unknown fields and noncanonical numbers.
    /// Member order, whitespace and string escapes may normalize; grants never widen.
    pub fn validate(bytes: &[u8]) -> Result<ValidatedRun, RunRefusal> {
        let mut record = Self::parse(bytes)?;
        if record.schema != SCHEMA {
            return Err(RunRefusal::Schema {
                found: record.schema,
            });
        }
        if record.depth_semantics != hirsel_run_schema::DEPTH_SEMANTICS {
            return Err(invalid(
                "depth_semantics",
                "incompatible depth semantics; expected hirsel.depth.invocations.v2",
            ));
        }
        let program = digest(&record.program)
            .ok_or_else(|| invalid("program", "expected 64 lowercase hex digits"))?;
        if !definition_path(&record.entry) {
            return Err(invalid(
                "entry",
                "expected a full versioned definition path",
            ));
        }
        match (&record.call, record.launch) {
            (Some(call), LaunchKind::Call) => calls::validate(call)?,
            (None, LaunchKind::Main | LaunchKind::Test) => {}
            _ => return Err(invalid("call", "a call member exactly for a call launch")),
        }
        if let Some(scripts) = &record.inputs.fixture_file_faults {
            hirsel_run_schema::validate_file_faults(scripts)
                .map_err(|e| invalid("inputs.fixture_file_faults", &e))?;
        }
        if let Some(scheduler) = &record.inputs.scheduler {
            provider("inputs.scheduler.algorithm", &scheduler.algorithm)?;
            if scheduler.version == 0 {
                return Err(invalid("inputs.scheduler.version", "zero version"));
            }
        }
        object_stores::expand(&mut record)?;
        if let Some(io) = &mut record.io {
            validate_io(io, "io")?;
            runners::validate(&mut io.runners, validate_io)?;
        }
        if let Some(env) = &record.env {
            env_keys(env)?;
        }
        if let Some(admission) = &mut record.admission {
            // A policy equal to its default is absent from canonical bytes.
            if admission.code.as_deref() == Some(ProviderContract::AdmitAny.name()) {
                admission.code = None;
            }
            if let (Some(start), Some(end)) = (admission.not_before, admission.expires)
                && start > end
            {
                return Err(invalid("admission", "not-before exceeds expiry"));
            }
            if let Some(reference) = &admission.revocation {
                provider("admission.revocation", reference)?;
            }
        }
        if let Some(budgets) = &record.budgets {
            provider("budgets.provider", &budgets.provider)?;
            match ProviderContract::parse(&budgets.provider) {
                Some(ProviderContract::BudgetsNone) => {
                    if budgets.fuel.is_some() || budgets.memory_bytes.is_some() {
                        return Err(invalid("budgets", "budgets.none@1 takes no limits"));
                    }
                    record.budgets = None;
                }
                Some(ProviderContract::BudgetsLimits) => {
                    if budgets.fuel.is_none() && budgets.memory_bytes.is_none() {
                        return Err(invalid(
                            "budgets",
                            "budgets.limits@2 needs fuel or memory_bytes",
                        ));
                    }
                    if budgets.fuel == Some(0) || budgets.memory_bytes == Some(0) {
                        return Err(invalid("budgets", "a zero limit"));
                    }
                }
                _ => {
                    return Err(RunRefusal::Unsupported {
                        field: "budgets.provider".into(),
                    });
                }
            }
        }
        if let Some(placement) = &record.placement {
            placement.validate()?;
        }
        if record.inputs.scheduler.is_some()
            && record.io.as_ref().and_then(|io| io.tasks.as_deref())
                == Some(ProviderContract::TasksFree.name())
        {
            return Err(invalid(
                "inputs.scheduler",
                "tasks.free@1 takes no scheduler",
            ));
        }
        supported_providers(&record)?;
        let canonical = canonical_bytes(&record);
        let timed = record
            .io
            .as_ref()
            .and_then(|io| io.process.as_ref())
            .is_some_and(|g| g.timed.is_some());
        if timed
            && (record.inputs.fixture_monotonic.is_none()
                || record.io.as_ref().is_none_or(|io| {
                    io.monotonic_clock.is_none()
                        || io.wall_clock.is_some()
                        || io
                            .random
                            .as_deref()
                            .is_some_and(|p| p != ProviderContract::RandomSplitMix64.name())
                        || io
                            .runners
                            .iter()
                            .any(|r| r.provider != ProviderContract::RunnerSim.name())
                        || io
                            .net
                            .as_ref()
                            .is_some_and(|n| n.provider != "net.script@2")
                        || io
                            .signals
                            .as_ref()
                            .is_some_and(|s| s.provider == "signals.host@1")
                }))
        {
            return Err(invalid(
                "io.process.clock",
                "timed process requires an isolated granted fixture clock",
            ));
        }
        if !timed && record.inputs.fixture_monotonic.is_some() {
            return Err(invalid(
                "inputs.fixture_monotonic",
                "only for timed process fixtures",
            ));
        }
        let identity = hash_parts(IDENTITY_DOMAIN, [&canonical[..]]);
        Ok(ValidatedRun {
            data: std::sync::Arc::new(ValidatedData {
                program,
                record,
                canonical,
                identity,
            }),
        })
    }
    /// The record's JSON, rejecting duplicates, unknown fields and noncanonical numbers,
    /// before validation.
    fn parse(bytes: &[u8]) -> Result<Self, RunRefusal> {
        integer_tokens(bytes)?;
        schema_diagnostic::from_slice(bytes)
    }
    /// Builders pass through exactly the same parser/validator as standalone bytes.
    pub fn validated(&self) -> Result<ValidatedRun, RunRefusal> {
        Self::validate(&serde_json::to_vec(self).expect("run serialization"))
    }
}
impl ValidatedRun {
    /// Override only placement.cores, then pass through ordinary validation.
    pub fn with_cores(&self, cores: u64) -> Result<Self, RunRefusal> {
        let mut record = self.data.record.clone();
        record.placement.get_or_insert_default().cores = Cores::Fixed(cores);
        record.validated()
    }
    /// The complete validated launch record, retaining non-identity placement.
    pub fn launch_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.data.record).expect("run serialization")
    }
    /// Acquisition policy for both real and simulated adapters. Io presence alone
    /// grants no projections; clocks retain their independent grant booleans.
    pub fn io_projections(&self) -> BTreeSet<String> {
        let Some(io) = &self.data.record.io else {
            return BTreeSet::new();
        };
        [
            ("stdin", io.stdin.is_some()),
            ("stderr", io.stderr.is_some()),
            ("stdout", io.stdout.is_some()),
            ("tasks", io.tasks.is_some()),
            ("net", io.net.is_some()),
            ("process", io.process.is_some()),
            ("random", io.random.is_some()),
            ("signals", io.signals.is_some()),
            ("dir", !io.dirs.is_empty()),
            ("telemetry", io.telemetry.is_some()),
        ]
        .into_iter()
        .filter(|(_, granted)| *granted)
        .map(|(name, _)| name.into())
        .collect()
    }
    pub fn clock_grants(&self) -> (bool, bool) {
        self.data.record.io.as_ref().map_or((false, false), |io| {
            (io.monotonic_clock.is_some(), io.wall_clock.is_some())
        })
    }
    /// The granted Random provider, if any.
    pub fn random_provider(&self) -> Option<ProviderContract> {
        ProviderContract::parse(self.data.record.io.as_ref()?.random.as_ref()?)
    }
    /// The granted Telemetry provider: its contract, identity seed and simulated exporter
    /// capacity (`telemetry.null@1` only).
    pub fn telemetry_provider(&self) -> Option<(ProviderContract, u64, Option<u64>)> {
        let grant = self.data.record.io.as_ref()?.telemetry.as_ref()?;
        Some((
            ProviderContract::parse(&grant.provider)?,
            grant.seed.unwrap_or(0),
            grant.queue,
        ))
    }
    /// Exact validated Net policy; peers supply data and never widen its rules.
    pub fn net_grant(&self) -> Option<&NetGrant> {
        self.data.record.io.as_ref()?.net.as_ref()
    }
    /// The run's `budgets.limits@2` entry, or None under `budgets.none@1`.
    pub fn budgets(&self) -> Option<&Budgets> {
        self.data.record.budgets.as_ref()
    }
    /// Exact validated process policy; scripted runs never widen its rules.
    pub fn process_grant(&self) -> Option<&ProcessGrant> {
        self.data.record.io.as_ref()?.process.as_ref()
    }
    /// The observed signal kinds and their provider; a script never widens the kinds.
    pub fn signals_grant(&self) -> Option<&SignalsGrant> {
        self.data.record.io.as_ref()?.signals.as_ref()
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
        &self.data.record
    }
    /// Whether the run calls runners, and so has a run instance.
    pub fn calls(&self) -> bool {
        self.data
            .record
            .io
            .as_ref()
            .is_some_and(|io| !io.runners.is_empty())
    }
    /// A callee run's instance, derived from its call's key as its seeds are.
    pub fn instance(&self) -> Result<Option<[u8; 16]>, RunRefusal> {
        let Some(call) = &self.data.record.call else {
            return Ok(None);
        };
        let cause = call.cause()?;
        Ok(Some(callee_instance(&call_key(
            &cause.instance,
            cause.ordinal,
        ))))
    }
    pub fn canonical(&self) -> &[u8] {
        &self.data.canonical
    }
    pub fn identity(&self) -> [u8; 32] {
        self.data.identity
    }
    pub fn program(&self) -> [u8; 32] {
        self.data.program
    }
    pub fn config(&self) -> Config {
        Config {
            hash_seed: self.data.record.inputs.hash_seed,
            random_seed: self.data.record.inputs.random_seed,
            test_seed: self.data.record.inputs.test_seed,
            scheduler: self
                .data
                .record
                .inputs
                .scheduler
                .as_ref()
                .map_or(0, |s| s.code().expect("validated scheduler")),
            scheduler_seed: self
                .data
                .record
                .inputs
                .scheduler
                .as_ref()
                .map_or(0, |s| s.seed),
            run_identity: self.data.identity,
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
            ("entry", self.data.record.entry == entry),
            ("launch", self.data.record.launch == launch),
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
                Root::Io => self.data.record.io.is_some(),
                Root::Args => self.data.record.args.is_some(),
                Root::Env => self.data.record.env.is_some(),
                Root::Unsafe => self.data.record.unsafe_root,
                Root::Test => self.data.record.launch == LaunchKind::Test,
            };
            if !present {
                return Err(RunRefusal::NotGranted {
                    root: format!("{root:?}"),
                });
            }
        }
        Ok(())
    }
    /// Check this run against the code it would execute, before any host is consulted: the
    /// binding (program, entry, launch kind), then the entry's roots, then the code policy
    /// against what the code reaches (CAP-6). Pure: no clock, grant or host state is read,
    /// so a replay binds exactly as a live launch does.
    pub fn bind(self, facts: &ProgramFacts) -> Result<BoundRun, RunRefusal> {
        self.check_program(facts.program, &facts.entry, facts.launch)?;
        self.require_roots(&facts.roots)?;
        if let (Some(policy), CodeReach::Escapes(path)) = (
            self.data
                .record
                .admission
                .as_ref()
                .and_then(|a| a.code.as_ref()),
            &facts.code,
        ) && ProviderContract::parse(policy) == Some(ProviderContract::AdmitSafe)
        {
            return Err(RunRefusal::UnsafeCode { path: path.clone() });
        }
        // Budget enforceability: code without fuel checks cannot enforce a fuel budget.
        if self.budgets().is_some_and(|b| b.fuel.is_some()) && !facts.fueled {
            return Err(RunRefusal::Unsupported {
                field: "budgets.fuel".into(),
            });
        }
        Ok(BoundRun { run: self })
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
        if recorded.data.identity != identity {
            return Err(RunRefusal::Binding {
                field: "recording.run_identity".into(),
            });
        }
        if self.data.identity != identity {
            return Err(RunRefusal::Binding {
                field: "run_identity".into(),
            });
        }
        if let Some(admission) = &recorded.data.record.admission {
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
/// What binding needs to know about the compiled code a run would execute: its semantic
/// program digest, entry, launch kind, the roots the entry declares, what the entry
/// reaches (CAP-6) and whether the code charges fuel, so that it can enforce a fuel budget.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgramFacts {
    pub program: [u8; 32],
    pub entry: String,
    pub launch: LaunchKind,
    pub roots: Vec<Root>,
    pub code: CodeReach,
    pub fueled: bool,
}
impl ProgramFacts {
    /// The facts of code an engine interprets or compiles for this run, which charges fuel
    /// whenever the run has a fuel budget.
    pub fn of(
        revision: &ProgramRevision,
        launch: LaunchKind,
        roots: Vec<Root>,
        code: CodeReach,
    ) -> Self {
        Self {
            program: revision.digest(),
            entry: revision.entry().into(),
            launch,
            roots,
            code,
            fueled: true,
        }
    }
}

/// A validated run bound to the code it executes ([`ValidatedRun::bind`]). Binding mints no
/// authority: only [`BoundRun::admit`], with a host, does. A replay runs a bound run without
/// admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundRun {
    run: ValidatedRun,
}
impl BoundRun {
    pub fn run(&self) -> &ValidatedRun {
        &self.run
    }
    pub fn into_run(self) -> ValidatedRun {
        self.run
    }
    /// The host supplies authority and evidence; these inputs cannot come from the record.
    /// No time/revocation check is performed by operations on the admitted result.
    pub fn admit(&self, host: &mut dyn HostAuthority, now: i64) -> Result<AdmittedRun, RunRefusal> {
        let mut revocation = None;
        if let Some(admission) = &self.run.data.record.admission {
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
        host.authorize(&self.run)?;
        Ok(AdmittedRun {
            run: self.run.clone(),
            evidence: AdmissionEvidence {
                at: now,
                revocation,
            },
            instance: match self.run.instance()? {
                Some(instance) => Some(instance),
                None if self.run.calls() => Some(host.run_instance()?),
                None => None,
            },
        })
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
    /// Pin the host authority the validated run's grants name.
    fn authorize(&mut self, run: &ValidatedRun) -> Result<(), RunRefusal>;
    fn revocation(&mut self, reference: &str) -> Result<RevocationEvidence, RunRefusal>;
    /// A fresh run instance from the host's secure generator (RUN-11), for a run that calls
    /// runners. A host without one cannot admit such a run.
    fn run_instance(&mut self) -> Result<[u8; 16], RunRefusal> {
        Err(RunRefusal::Unsupported {
            field: "io.runners".into(),
        })
    }
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
    instance: Option<[u8; 16]>,
}
impl AdmittedRun {
    pub fn run(&self) -> &ValidatedRun {
        &self.run
    }
    pub fn evidence(&self) -> &AdmissionEvidence {
        &self.evidence
    }
    /// The run instance fixed at admission (RUN-11): a callee run's derived one, or the
    /// host's fresh one for a run that calls runners; none for any other run.
    pub fn instance(&self) -> Option<[u8; 16]> {
        self.instance
    }
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
fn provider(field: &str, name: &str) -> Result<(), RunRefusal> {
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(invalid(field, "empty or invalid provider/reference"));
    }
    Ok(())
}
// The closed provider names describe observable contracts,
// not Linux/simulator implementations. Unsupported policy cannot be widened.
/// Validate and normalize one `io` grant set: the record's own, or a runner's at `field`.
fn validate_io(io: &mut IoGrants, field: &str) -> Result<(), RunRefusal> {
    let at = |member: &str| format!("{field}.{member}");
    if let Some(cpu) = &io.cpu {
        cpu::validate(cpu, io.tasks.is_some(), &at("cpu"))?;
    }
    for (field, value) in [
        (at("stdin"), &io.stdin),
        (at("stderr"), &io.stderr),
        (at("stdout"), &io.stdout),
        (at("tasks"), &io.tasks),
        (at("monotonic_clock"), &io.monotonic_clock),
        (at("wall_clock"), &io.wall_clock),
        (at("random"), &io.random),
    ] {
        if let Some(value) = value {
            provider(&field, value)?;
        }
    }
    io.dirs.sort_by(|a, b| a.path.cmp(&b.path));
    for mount in &io.dirs {
        if !portable_path(&mount.path, true) {
            return Err(invalid(&at("dirs.path"), "noncanonical virtual mount"));
        }
        if !absolute_path(&mount.host) {
            return Err(invalid(
                &at("dirs.host"),
                "expected a canonical absolute host root",
            ));
        }
    }
    let mut paths = BTreeSet::new();
    for mount in &io.dirs {
        if !paths.insert(mount.path.as_str()) {
            return Err(invalid(&at("dirs"), "duplicate virtual mount"));
        }
    }
    for mount in &io.dirs {
        if !mount.path.is_empty() && paths.contains("") {
            return Err(invalid(&at("dirs"), "overlapping virtual mounts"));
        }
        let mut path = mount.path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            if paths.contains(parent) {
                return Err(invalid(&at("dirs"), "overlapping virtual mounts"));
            }
            path = parent;
        }
    }
    if let Some(net) = &mut io.net {
        provider(&at("net.provider"), &net.provider)?;
        net::validate(net)?;
    }
    if let Some(process) = &mut io.process {
        provider(&at("process.provider"), &process.provider)?;
        process::validate(process)?;
    }
    if let Some(signals) = &mut io.signals {
        provider(&at("signals.provider"), &signals.provider)?;
        signals::validate(signals)?;
    }
    if let Some(telemetry) = &io.telemetry {
        provider(&at("telemetry.provider"), &telemetry.provider)?;
        // The host provider draws its identities; only the null provider takes a seed
        // and a simulated exporter.
        let null = telemetry.provider == ProviderContract::TelemetryNull.name();
        if null != telemetry.seed.is_some() || (!null && telemetry.queue.is_some()) {
            return Err(invalid(
                &at("telemetry"),
                "telemetry.null@1 requires a seed and alone takes a queue",
            ));
        }
    }
    Ok(())
}
/// Whether every provider of one `io` grant set, at `field`, is supported.
fn supported_io(io: &IoGrants, field: &str) -> Result<(), RunRefusal> {
    let unsupported = |field: &str| RunRefusal::Unsupported {
        field: field.into(),
    };
    let at = |member: &str| format!("{field}.{member}");
    for (field, value, expected) in [
        (at("stdin"), &io.stdin, Some(ProviderContract::StdinBytes)),
        (
            at("stderr"),
            &io.stderr,
            Some(ProviderContract::StderrBytes),
        ),
        (
            at("stdout"),
            &io.stdout,
            Some(ProviderContract::StdoutBytes),
        ),
        (
            at("monotonic_clock"),
            &io.monotonic_clock,
            Some(ProviderContract::MonotonicNanoseconds),
        ),
        (
            at("wall_clock"),
            &io.wall_clock,
            Some(ProviderContract::WallUnixNanoseconds),
        ),
    ] {
        if let Some(name) = value
            && (expected.is_none() || ProviderContract::parse(name) != expected)
        {
            return Err(unsupported(&field));
        }
    }
    if let Some(tasks) = &io.tasks
        && !matches!(
            ProviderContract::parse(tasks),
            Some(ProviderContract::TasksCooperative | ProviderContract::TasksFree)
        )
    {
        return Err(unsupported(&at("tasks")));
    }
    if let Some(random) = &io.random
        && !matches!(
            ProviderContract::parse(random),
            Some(ProviderContract::RandomSplitMix64 | ProviderContract::RandomSystem)
        )
    {
        return Err(unsupported(&at("random")));
    }
    if let Some(net) = &io.net
        && !matches!(
            ProviderContract::parse(&net.provider),
            Some(ProviderContract::NetStream | ProviderContract::NetScript)
        )
    {
        return Err(unsupported(&at("net.provider")));
    }
    if let Some(process) = &io.process
        && !matches!(
            ProviderContract::parse(&process.provider),
            Some(
                ProviderContract::ProcessSpawn
                    | ProviderContract::ProcessScript
                    | ProviderContract::ProcessScriptTimed
            )
        )
    {
        return Err(unsupported(&at("process.provider")));
    }
    if let Some(telemetry) = &io.telemetry
        && !matches!(
            ProviderContract::parse(&telemetry.provider),
            Some(ProviderContract::TelemetryCapture | ProviderContract::TelemetryNull)
        )
    {
        return Err(unsupported(&at("telemetry.provider")));
    }
    if let Some(signals) = &io.signals
        && !matches!(
            ProviderContract::parse(&signals.provider),
            Some(ProviderContract::SignalsHost | ProviderContract::SignalsScript)
        )
    {
        return Err(unsupported(&at("signals.provider")));
    }
    Ok(())
}
fn supported_providers(record: &RunConfig) -> Result<(), RunRefusal> {
    let unsupported = |field: &str| RunRefusal::Unsupported {
        field: field.into(),
    };
    if record
        .inputs
        .scheduler
        .as_ref()
        .is_some_and(|s| s.code().is_none())
    {
        return Err(unsupported("inputs.scheduler.algorithm"));
    }
    if let Some(io) = &record.io {
        supported_io(io, "io")?;
        for runner in &io.runners {
            if !matches!(
                ProviderContract::parse(&runner.provider),
                Some(
                    ProviderContract::RunnerLocalThreads
                        | ProviderContract::RunnerSim
                        | ProviderContract::RunnerRemoteTls
                )
            ) {
                return Err(unsupported("io.runners.provider"));
            }
            if let Some(grants) = &runner.grants {
                supported_io(grants, "io.runners.grants")?;
                // A callee run captures no telemetry of its own.
                if grants.telemetry.is_some() {
                    return Err(unsupported("io.runners.grants.telemetry"));
                }
            }
        }
    }
    if let Some(code) = record.admission.as_ref().and_then(|a| a.code.as_ref())
        && !matches!(
            ProviderContract::parse(code),
            Some(ProviderContract::AdmitAny | ProviderContract::AdmitSafe)
        )
    {
        return Err(unsupported("admission.code"));
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
fn env_keys(env: &BTreeMap<String, Option<String>>) -> Result<(), RunRefusal> {
    if env
        .keys()
        .any(|key| key.is_empty() || key.contains(['=', '\0']))
    {
        return Err(invalid("env", "invalid environment key"));
    }
    Ok(())
}

/// A standalone Env snapshot object, validated as a record's `env`: unique keys, each
/// nonempty without `=` or NUL, mapping to a string or null (granted but unset).
pub fn validate_env(bytes: &[u8]) -> Result<BTreeMap<String, Option<String>>, RunRefusal> {
    #[derive(Deserialize)]
    struct Snapshot(
        #[serde(deserialize_with = "unique_env")] Option<BTreeMap<String, Option<String>>>,
    );
    let Snapshot(env) =
        serde_json::from_slice(bytes).map_err(|error| invalid("env", &error.to_string()))?;
    let env = env.ok_or_else(|| invalid("env", "expected an Env snapshot object"))?;
    env_keys(&env)?;
    Ok(env)
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
    // Placement is validated before this point, but never contributes to identity.
    let mut semantic = record.clone();
    semantic.placement = None;
    let mut bytes = vec![];
    semantic
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut bytes, Canonical,
        ))
        .expect("run serialization");
    bytes
}

// The forwarding structure follows serde_path_to_error 0.1.20, src/de.rs
// (7aad3f59acd6d239615ee8b315e0c666915a13bf). Use its path tracker directly;
// this bridge only captures structured missing-field errors and redacts values.
/*
serde_path_to_error (David Tolnay), MIT license:
Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
*/
mod schema_diagnostic {
    use super::RunRefusal;
    use serde::de::{self, Deserialize, DeserializeSeed, Visitor};
    use std::{cell::Cell, fmt};

    pub(super) fn from_slice<'de, T: Deserialize<'de>>(bytes: &'de [u8]) -> Result<T, RunRefusal> {
        let missing = Cell::new(None);
        let mut deserializer = serde_json::Deserializer::from_slice(bytes);
        let value = serde_path_to_error::deserialize(Wrap(&mut deserializer, &missing)).map_err(
            |error| {
                let mut path = error.path().to_string();
                if let Some(field) = missing.get() {
                    if path == "." {
                        path.clear();
                    } else {
                        path.push('.');
                    }
                    path.push_str(field);
                }
                RunRefusal::Malformed {
                    detail: format!("{path}: {}", redact_strings(&error.inner().to_string())),
                }
            },
        )?;
        // serde_json::from_slice also requires the entire input to be consumed.
        deserializer.end().map_err(|error| RunRefusal::Malformed {
            detail: error.to_string(),
        })?;
        Ok(value)
    }

    // serde_json can reject a typed request before invoking the visitor (e.g.
    // a string where a sequence was requested). Its Debug-quoted input strings
    // need redaction too. This only scrubs rendered values; paths and missing
    // fields always come from structured callbacks, never from error text.
    fn redact_strings(message: &str) -> String {
        let mut result = String::new();
        let mut chars = message.chars();
        while let Some(ch) = chars.next() {
            result.push(ch);
            if ch == '"' {
                result.push_str("<redacted>\"");
                while let Some(ch) = chars.next() {
                    match ch {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
        }
        result
    }

    #[derive(Debug)]
    struct Error<E> {
        source: E,
        missing: Option<&'static str>,
    }
    impl<E> Error<E> {
        fn source(source: E) -> Self {
            Self {
                source,
                missing: None,
            }
        }
        fn unwrap(self, missing: &Cell<Option<&'static str>>) -> E {
            if self.missing.is_some() {
                missing.set(self.missing);
            }
            self.source
        }
    }
    impl<E: fmt::Display> fmt::Display for Error<E> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.source.fmt(f)
        }
    }
    impl<E: std::error::Error> std::error::Error for Error<E> {}
    impl<E: de::Error> de::Error for Error<E> {
        fn custom<T: fmt::Display>(message: T) -> Self {
            Self::source(E::custom(message))
        }
        fn missing_field(field: &'static str) -> Self {
            Self {
                source: E::missing_field(field),
                missing: Some(field),
            }
        }
        fn duplicate_field(field: &'static str) -> Self {
            Self::source(E::duplicate_field(field))
        }
        fn unknown_field(_field: &str, expected: &'static [&'static str]) -> Self {
            Self::source(E::unknown_field("<redacted>", expected))
        }
        fn unknown_variant(_variant: &str, expected: &'static [&'static str]) -> Self {
            Self::source(E::unknown_variant("<redacted>", expected))
        }
        fn invalid_type(value: de::Unexpected<'_>, expected: &dyn de::Expected) -> Self {
            Self::source(E::invalid_type(kind(value), expected))
        }
        fn invalid_value(value: de::Unexpected<'_>, expected: &dyn de::Expected) -> Self {
            Self::source(E::invalid_value(kind(value), expected))
        }
    }
    fn kind(value: de::Unexpected<'_>) -> de::Unexpected<'static> {
        use de::Unexpected::*;
        Other(match value {
            Bool(_) => "boolean",
            Unsigned(_) => "integer",
            Signed(_) => "integer",
            Float(_) => "floating point",
            Char(_) => "character",
            Str(_) => "string",
            Bytes(_) => "byte array",
            Unit => "null",
            Option => "Option",
            NewtypeStruct => "newtype struct",
            Seq => "sequence",
            Map => "map",
            Enum => "enum",
            UnitVariant => "unit variant",
            NewtypeVariant => "newtype variant",
            TupleVariant => "tuple variant",
            StructVariant => "struct variant",
            Other(_) => "value",
        })
    }

    struct Wrap<'a, T>(T, &'a Cell<Option<&'static str>>);
    macro_rules! deserialize {
        ($($method:ident $(($($arg:ident: $ty:ty),*))?;)*) => {$(
            fn $method<V: Visitor<'de>>(self, $($($arg: $ty,)*)? visitor: V) -> Result<V::Value, Self::Error> {
                self.0.$method($($($arg,)*)? Wrap(visitor, self.1)).map_err(Error::source)
            }
        )*};
    }
    impl<'de, D: de::Deserializer<'de>> de::Deserializer<'de> for Wrap<'_, D> {
        type Error = Error<D::Error>;
        deserialize! {
            deserialize_any; deserialize_bool; deserialize_i8; deserialize_i16; deserialize_i32;
            deserialize_i64; deserialize_i128; deserialize_u8; deserialize_u16; deserialize_u32;
            deserialize_u64; deserialize_u128; deserialize_f32; deserialize_f64; deserialize_char;
            deserialize_str; deserialize_string; deserialize_bytes; deserialize_byte_buf;
            deserialize_option; deserialize_unit; deserialize_unit_struct(name: &'static str);
            deserialize_newtype_struct(name: &'static str); deserialize_seq; deserialize_tuple(len: usize);
            deserialize_tuple_struct(name: &'static str, len: usize); deserialize_map;
            deserialize_struct(name: &'static str, fields: &'static [&'static str]);
            deserialize_enum(name: &'static str, variants: &'static [&'static str]);
            deserialize_identifier; deserialize_ignored_any;
        }
        fn is_human_readable(&self) -> bool {
            self.0.is_human_readable()
        }
    }
    impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for Wrap<'_, S> {
        type Value = S::Value;
        fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
            self.0
                .deserialize(Wrap(de, self.1))
                .map_err(|error| error.unwrap(self.1))
        }
    }
    macro_rules! visit {
        ($($method:ident($ty:ty);)*) => {$(
            fn $method<E: de::Error>(self, value: $ty) -> Result<Self::Value, E> {
                self.0.$method::<Error<E>>(value).map_err(|error| error.unwrap(self.1))
            }
        )*};
    }
    impl<'de, V: Visitor<'de>> Visitor<'de> for Wrap<'_, V> {
        type Value = V::Value;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.expecting(f)
        }
        visit! {
            visit_bool(bool); visit_i8(i8); visit_i16(i16); visit_i32(i32); visit_i64(i64); visit_i128(i128);
            visit_u8(u8); visit_u16(u16); visit_u32(u32); visit_u64(u64); visit_u128(u128);
            visit_f32(f32); visit_f64(f64); visit_char(char); visit_str(&str); visit_borrowed_str(&'de str);
            visit_string(String); visit_bytes(&[u8]); visit_borrowed_bytes(&'de [u8]); visit_byte_buf(Vec<u8>);
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            self.0
                .visit_unit::<Error<E>>()
                .map_err(|error| error.unwrap(self.1))
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            self.0
                .visit_none::<Error<E>>()
                .map_err(|error| error.unwrap(self.1))
        }
        fn visit_some<D: de::Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
            self.0
                .visit_some(Wrap(de, self.1))
                .map_err(|error| error.unwrap(self.1))
        }
        fn visit_newtype_struct<D: de::Deserializer<'de>>(
            self,
            de: D,
        ) -> Result<Self::Value, D::Error> {
            self.0
                .visit_newtype_struct(Wrap(de, self.1))
                .map_err(|error| error.unwrap(self.1))
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
            self.0
                .visit_seq(Wrap(access, self.1))
                .map_err(|error| error.unwrap(self.1))
        }
        fn visit_map<A: de::MapAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
            self.0
                .visit_map(Wrap(access, self.1))
                .map_err(|error| error.unwrap(self.1))
        }
        fn visit_enum<A: de::EnumAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
            self.0
                .visit_enum(Wrap(access, self.1))
                .map_err(|error| error.unwrap(self.1))
        }
    }
    impl<'de, A: de::SeqAccess<'de>> de::SeqAccess<'de> for Wrap<'_, A> {
        type Error = Error<A::Error>;
        fn next_element_seed<S: DeserializeSeed<'de>>(
            &mut self,
            seed: S,
        ) -> Result<Option<S::Value>, Self::Error> {
            self.0
                .next_element_seed(Wrap(seed, self.1))
                .map_err(Error::source)
        }
        fn size_hint(&self) -> Option<usize> {
            self.0.size_hint()
        }
    }
    impl<'de, A: de::MapAccess<'de>> de::MapAccess<'de> for Wrap<'_, A> {
        type Error = Error<A::Error>;
        fn next_key_seed<S: DeserializeSeed<'de>>(
            &mut self,
            seed: S,
        ) -> Result<Option<S::Value>, Self::Error> {
            self.0
                .next_key_seed(Wrap(seed, self.1))
                .map_err(Error::source)
        }
        fn next_value_seed<S: DeserializeSeed<'de>>(
            &mut self,
            seed: S,
        ) -> Result<S::Value, Self::Error> {
            self.0
                .next_value_seed(Wrap(seed, self.1))
                .map_err(Error::source)
        }
        fn size_hint(&self) -> Option<usize> {
            self.0.size_hint()
        }
    }
    impl<'a, 'de, A: de::EnumAccess<'de>> de::EnumAccess<'de> for Wrap<'a, A> {
        type Error = Error<A::Error>;
        type Variant = Wrap<'a, A::Variant>;
        fn variant_seed<S: DeserializeSeed<'de>>(
            self,
            seed: S,
        ) -> Result<(S::Value, Self::Variant), Self::Error> {
            self.0
                .variant_seed(Wrap(seed, self.1))
                .map(|(value, variant)| (value, Wrap(variant, self.1)))
                .map_err(Error::source)
        }
    }
    impl<'de, A: de::VariantAccess<'de>> de::VariantAccess<'de> for Wrap<'_, A> {
        type Error = Error<A::Error>;
        fn unit_variant(self) -> Result<(), Self::Error> {
            self.0.unit_variant().map_err(Error::source)
        }
        fn newtype_variant_seed<S: DeserializeSeed<'de>>(
            self,
            seed: S,
        ) -> Result<S::Value, Self::Error> {
            self.0
                .newtype_variant_seed(Wrap(seed, self.1))
                .map_err(Error::source)
        }
        fn tuple_variant<V: Visitor<'de>>(
            self,
            len: usize,
            visitor: V,
        ) -> Result<V::Value, Self::Error> {
            self.0
                .tuple_variant(len, Wrap(visitor, self.1))
                .map_err(Error::source)
        }
        fn struct_variant<V: Visitor<'de>>(
            self,
            fields: &'static [&'static str],
            visitor: V,
        ) -> Result<V::Value, Self::Error> {
            self.0
                .struct_variant(fields, Wrap(visitor, self.1))
                .map_err(Error::source)
        }
    }
}
