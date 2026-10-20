//! A runner host's configuration (RUN-13 `remote.tls@1`, open item O24): where it listens,
//! the TLS identity it presents, which clients it admits and under which caller name, the
//! grant set and allowlist each caller's callees run within, and the host's own seams
//! (code store, code policy, call journal, callee recording) and limits.
//!
//! ```json
//! {"listen": {"unix": "/run/hirsel/billing.sock"},
//!  "identity": {"chain": "/etc/hirsel/host.pem", "key": "/etc/hirsel/host.key"},
//!  "clients": {"pem": "/etc/hirsel/client-ca.pem"},
//!  "callers": [{"name": "app", "identity": {"subject": "agent-7.hirsel.test"},
//!               "grants": {"tasks": "tasks.cooperative@1"}, "definitions": null}],
//!  "journal": {"provider": "journal.dir@1", "path": "/var/lib/hirsel/calls"},
//!  "recording": "record", "tapes": "/var/lib/hirsel/tapes",
//!  "runs": 1, "queue": 64}
//! ```
//!
//! `clients` null serves clients without certificates, all of them the one caller whose
//! identity is `"anonymous"`; with trust anchors, a client presents a certificate they
//! issued and is the caller whose identity names its certificate's SHA-256 or a DNS
//! subject-alternative name of it. A client no caller names is refused at its session.
use crate::{IoGrants, RunRefusal, absolute_path, definition_path, invalid};
use hirsel_run_schema::{Endpoint, PlaceAdmission, TlsFiles};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    pub listen: Endpoint,
    pub identity: TlsFiles,
    /// Trust anchors for client certificates, or null for clients without one.
    pub clients: Option<ClientRoots>,
    pub callers: Vec<CallerEntry>,
    /// The place's code policy (RUN-8 check 4); absent admits any checked code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<PlaceAdmission>,
    /// The code store fetched code comes from; absent is `code.none@1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<serde_json::Value>,
    /// The call journal; absent is `journal.none@1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<JournalEntry>,
    /// The callee runs' recording level, `none` (default) or `record`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<String>,
    /// The directory callee tapes are written to, required when recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tapes: Option<String>,
    /// Callees running at once (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs: Option<u64>,
    /// Admitted calls waiting for a run (default 64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<u64>,
    /// The largest frame read, in bytes (default 64 MiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_bytes: Option<u64>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientRoots {
    pub pem: String,
}
/// One admitted caller: its name, the identity that authenticates it, and the grant set and
/// allowlist within which its requests are admitted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallerEntry {
    pub name: String,
    pub identity: CallerIdentity,
    pub grants: IoGrants,
    pub definitions: Option<Vec<String>>,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum CallerIdentity {
    /// The SHA-256 of the client's end-entity certificate (DER), 64 lowercase hex digits.
    Certificate(String),
    /// A DNS subject-alternative name of the client's certificate.
    Subject(String),
    /// A client without a certificate (`clients` null).
    Anonymous,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalEntry {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

pub const DEFAULT_RUNS: u64 = 1;
pub const DEFAULT_QUEUE: u64 = 64;
pub const DEFAULT_FRAME_BYTES: u64 = 64 * 1024 * 1024;

impl HostConfig {
    /// Parse, validate and normalize a host configuration.
    pub fn validate(bytes: &[u8]) -> Result<Self, RunRefusal> {
        let mut config: Self =
            serde_json::from_slice(bytes).map_err(|e| invalid("host", &e.to_string()))?;
        match &config.listen {
            Endpoint::Unix(path) if hirsel_run_schema::canonical_unix_path(path) => {}
            Endpoint::Tcp { address, service }
                if hirsel_run_schema::canonical_net_address(address)
                    && service
                        .parse::<u16>()
                        .is_ok_and(|p| p.to_string() == *service) => {}
            _ => {
                return Err(invalid(
                    "host.listen",
                    "expected a canonical Unix socket path or address and service",
                ));
            }
        }
        if !absolute_path(&config.identity.chain) || !absolute_path(&config.identity.key) {
            return Err(invalid(
                "host.identity",
                "expected absolute chain and key paths",
            ));
        }
        if let Some(roots) = &config.clients
            && !absolute_path(&roots.pem)
        {
            return Err(invalid("host.clients.pem", "expected an absolute path"));
        }
        if config.callers.windows(2).any(|p| p[0].name >= p[1].name) {
            return Err(invalid(
                "host.callers",
                "callers must be sorted and unique by name",
            ));
        }
        let mut identities = std::collections::BTreeSet::new();
        for caller in &mut config.callers {
            if caller.name.is_empty() || caller.name.chars().any(char::is_control) {
                return Err(invalid("host.callers.name", "expected a caller name"));
            }
            if !identities.insert(caller.identity.clone()) {
                return Err(invalid(
                    "host.callers.identity",
                    "two callers share an identity",
                ));
            }
            match (&caller.identity, &config.clients) {
                (CallerIdentity::Anonymous, None) => {}
                (CallerIdentity::Certificate(hash), Some(_))
                    if hash.len() == 64
                        && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) => {}
                (CallerIdentity::Subject(name), Some(_))
                    if hirsel_run_schema::tls::server_name(name).is_some() => {}
                _ => {
                    return Err(invalid(
                        "host.callers.identity",
                        "anonymous callers need clients null; certificate and subject callers need client roots",
                    ));
                }
            }
            if caller.grants.stdin.is_some() || !caller.grants.runners.is_empty() {
                return Err(invalid(
                    "host.callers.grants",
                    "a caller's grant set has no stdin and no runners",
                ));
            }
            crate::validate_io(&mut caller.grants, "host.callers.grants")?;
            crate::supported_io(&caller.grants, "host.callers.grants")?;
            if let Some(definitions) = &caller.definitions
                && (!definitions.iter().all(|p| definition_path(p))
                    || definitions.windows(2).any(|p| p[0] >= p[1]))
            {
                return Err(invalid(
                    "host.callers.definitions",
                    "expected sorted unique full versioned paths",
                ));
            }
        }
        if let Some(admission) = &config.admission {
            match admission
                .code
                .as_deref()
                .map(crate::ProviderContract::parse)
            {
                None | Some(Some(crate::ProviderContract::AdmitAny)) => config.admission = None,
                Some(Some(crate::ProviderContract::AdmitSafe)) => {}
                Some(_) => {
                    return Err(RunRefusal::Unsupported {
                        field: "host.admission.code".into(),
                    });
                }
            }
        }
        if let Some(code) = &config.code {
            let provider = code
                .as_object()
                .and_then(|o| o.get("provider"))
                .and_then(|p| p.as_str())
                .ok_or_else(|| invalid("host.code", "expected a code store entry"))?;
            if provider == "code.none@1" && code.as_object().is_some_and(|o| o.len() == 1) {
                config.code = None;
            }
        }
        match config
            .journal
            .as_ref()
            .map(|j| (j.provider.as_str(), &j.path))
        {
            None => {}
            Some(("journal.none@1", None)) => config.journal = None,
            Some(("journal.dir@1", Some(path))) if absolute_path(path) => {}
            Some(("journal.dir@1", _)) => {
                return Err(invalid("host.journal.path", "expected an absolute path"));
            }
            Some(_) => {
                return Err(RunRefusal::Unsupported {
                    field: "host.journal.provider".into(),
                });
            }
        }
        match (config.recording.as_deref(), &config.tapes) {
            (None | Some("none"), None) => config.recording = None,
            (Some("record"), Some(tapes)) if absolute_path(tapes) => {}
            (Some("record"), _) => {
                return Err(invalid(
                    "host.tapes",
                    "recording needs an absolute tape directory",
                ));
            }
            (Some("durable"), _) => {
                return Err(RunRefusal::Unsupported {
                    field: "host.recording".into(),
                });
            }
            _ => {
                return Err(invalid(
                    "host.recording",
                    "expected none or record, and tapes only when recording",
                ));
            }
        }
        for (field, value, default) in [
            ("host.runs", &mut config.runs, DEFAULT_RUNS),
            ("host.queue", &mut config.queue, DEFAULT_QUEUE),
            (
                "host.frame_bytes",
                &mut config.frame_bytes,
                DEFAULT_FRAME_BYTES,
            ),
        ] {
            match *value {
                Some(0) if field != "host.queue" => return Err(invalid(field, "zero")),
                Some(v) if v == default => *value = None,
                _ => {}
            }
        }
        Ok(config)
    }
    pub fn runs(&self) -> u64 {
        self.runs.unwrap_or(DEFAULT_RUNS)
    }
    pub fn queue(&self) -> u64 {
        self.queue.unwrap_or(DEFAULT_QUEUE)
    }
    pub fn frame_bytes(&self) -> u64 {
        self.frame_bytes.unwrap_or(DEFAULT_FRAME_BYTES)
    }
    /// The caller a client authenticates as: by its end-entity certificate's SHA-256
    /// `digest` and the subject names `names` accepts, or the anonymous caller for a client
    /// without a certificate.
    pub fn caller(
        &self,
        digest: Option<&[u8; 32]>,
        names: impl Fn(&str) -> bool,
    ) -> Option<&CallerEntry> {
        self.callers.iter().find(|c| match (&c.identity, digest) {
            (CallerIdentity::Anonymous, None) => true,
            (CallerIdentity::Certificate(hash), Some(digest)) => {
                *hash == hirsel_run_schema::hex(digest)
            }
            (CallerIdentity::Subject(name), Some(_)) => names(name),
            _ => false,
        })
    }
}
