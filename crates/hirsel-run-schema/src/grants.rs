//! The grant subset relation (CAP-3): whether one validated grant record holds no
//! authority another lacks. It is lexical over validated data and reads no host state.
use crate::{Access, IoGrants, Mount, NetGrant, NetRule, ProcessGrant, ProcessRule, SignalsGrant};
use std::collections::BTreeMap;

fn optional<T>(a: &Option<T>, b: &Option<T>, within: impl Fn(&T, &T) -> bool) -> bool {
    match (a, b) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(a), Some(b)) => within(a, b),
    }
}

pub fn io_within(a: &IoGrants, b: &IoGrants) -> bool {
    let provider = |a: &Option<String>, b: &Option<String>| optional(a, b, |a, b| a == b);
    provider(&a.stdin, &b.stdin)
        && provider(&a.stderr, &b.stderr)
        && provider(&a.stdout, &b.stdout)
        && provider(&a.tasks, &b.tasks)
        && provider(&a.monotonic_clock, &b.monotonic_clock)
        && provider(&a.wall_clock, &b.wall_clock)
        && provider(&a.random, &b.random)
        && a.dirs
            .iter()
            .all(|m| b.dirs.iter().any(|w| mount_within(m, w)))
        && optional(&a.net, &b.net, net_within)
        && optional(&a.process, &b.process, process_within)
        && optional(&a.telemetry, &b.telemetry, |a, b| a == b)
        && optional(&a.signals, &b.signals, signals_within)
        && optional(&a.cpu, &b.cpu, |a, b| {
            a.provider == b.provider
                && a.max_workers <= b.max_workers
                && a.max_domains <= b.max_domains
                && a.max_physical_bytes <= b.max_physical_bytes
                && a.max_batch_bytes <= b.max_batch_bytes
        })
}

/// The same provider, observing a subset of the other's kinds.
fn signals_within(a: &SignalsGrant, b: &SignalsGrant) -> bool {
    a.provider == b.provider && a.kinds.iter().all(|kind| b.kinds.contains(kind))
}

/// A mount is within another when its virtual path and its host root extend the
/// other's by the same components and its access is no wider.
pub fn mount_within(narrow: &Mount, wide: &Mount) -> bool {
    let suffix = if narrow.path == wide.path {
        ""
    } else if wide.path.is_empty() {
        narrow.path.as_str()
    } else {
        match narrow
            .path
            .strip_prefix(wide.path.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
        {
            Some(rest) => rest,
            None => return false,
        }
    };
    let host = if suffix.is_empty() {
        wide.host.clone()
    } else if wide.host == "/" {
        format!("/{suffix}")
    } else {
        format!("{}/{suffix}", wide.host)
    };
    narrow.host == host && narrow.access.within(wide.access)
}

impl Access {
    /// `Read` is within both accesses; `ReadWrite` only within itself.
    pub fn within(self, wide: Access) -> bool {
        self == Access::Read || wide == Access::ReadWrite
    }
}

fn net_within(a: &NetGrant, b: &NetGrant) -> bool {
    a.provider == b.provider
        && a.rules
            .iter()
            .all(|r| b.rules.iter().any(|w| net_rule_within(r, w)))
}

/// `net.narrow` coverage: equal rules, or the narrower operation of the same address and
/// port: `connect_tls` under `connect`, `listen_tls` under `listen` and `unix_listen`
/// under `unix_listen_remove_stale`.
pub fn net_rule_within(narrow: &NetRule, wide: &NetRule) -> bool {
    narrow.address == wide.address
        && narrow.service == wide.service
        && net_operation_within(&narrow.operation, &wide.operation)
}

/// Whether `narrow` is `wide` or its one narrowing.
pub fn net_operation_within(narrow: &str, wide: &str) -> bool {
    narrow == wide
        || matches!(
            (narrow, wide),
            ("connect_tls", "connect")
                | ("listen_tls", "listen")
                | ("unix_listen", "unix_listen_remove_stale")
        )
}

fn process_within(a: &ProcessGrant, b: &ProcessGrant) -> bool {
    a.provider == b.provider
        && a.rules
            .iter()
            .all(|r| b.rules.iter().any(|w| process_rule_within(r, w)))
}

/// `proc.narrow` coverage: the same program, passing through a subset of its names.
pub fn process_rule_within(narrow: &ProcessRule, wide: &ProcessRule) -> bool {
    narrow.program == wide.program && narrow.env.iter().all(|name| wide.env.contains(name))
}

/// Each granted name of `a` is granted by `b` with the same snapshot, set or unset.
pub fn env_within(
    a: &BTreeMap<String, Option<String>>,
    b: &BTreeMap<String, Option<String>>,
) -> bool {
    a.iter().all(|(name, value)| b.get(name) == Some(value))
}

/// `NetOperation`'s variants in ordinal order, as the run configuration spells them.
pub const NET_OPERATIONS: [&str; 9] = [
    "connect",
    "connect_tls",
    "listen",
    "udp_bind",
    "udp_peer",
    "listen_tls",
    "unix_connect",
    "unix_listen",
    "unix_listen_remove_stale",
];

/// RUN-2's core `GrantSet` as plain data: the roots a runner's grant set holds, by presence,
/// its mounts by virtual path, and its network and process rules. It names no provider and
/// no host path, so it is what a request carries to another place.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GrantSet {
    pub stdout: bool,
    pub stderr: bool,
    pub tasks: bool,
    pub monotonic_clock: bool,
    pub wall_clock: bool,
    pub random: bool,
    pub dirs: Vec<(String, Access)>,
    pub net: Vec<NetRule>,
    pub process: Vec<ProcessRule>,
}
impl GrantSet {
    /// The grant set `grants` holds.
    pub fn of(grants: &IoGrants) -> Self {
        Self {
            stdout: grants.stdout.is_some(),
            stderr: grants.stderr.is_some(),
            tasks: grants.tasks.is_some(),
            monotonic_clock: grants.monotonic_clock.is_some(),
            wall_clock: grants.wall_clock.is_some(),
            random: grants.random.is_some(),
            dirs: grants
                .dirs
                .iter()
                .map(|m| (m.path.clone(), m.access))
                .collect(),
            net: grants.net.iter().flat_map(|n| n.rules.clone()).collect(),
            process: grants
                .process
                .iter()
                .flat_map(|p| p.rules.clone())
                .collect(),
        }
    }
}

impl GrantSet {
    /// The set's canonical codec bytes (serialization.md, codec version 1), as a runner
    /// request's digest frames them (RUN-11): the six flags, then the `DirGrant`, `NetRule`
    /// and `ProcessRule` lists. None when a net rule's service is not a port or its
    /// operation is unknown.
    pub fn canonical(&self) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        for flag in [
            self.stdout,
            self.stderr,
            self.tasks,
            self.monotonic_clock,
            self.wall_clock,
            self.random,
        ] {
            out.push(u8::from(flag));
        }
        out.extend_from_slice(&(self.dirs.len() as u64).to_le_bytes());
        for (path, access) in &self.dirs {
            text(&mut out, path);
            out.extend_from_slice(&u32::from(*access == Access::ReadWrite).to_le_bytes());
        }
        out.extend_from_slice(&(self.net.len() as u64).to_le_bytes());
        for rule in &self.net {
            text(&mut out, &rule.address);
            out.extend_from_slice(&rule.service.parse::<u16>().ok()?.to_le_bytes());
            let operation = NET_OPERATIONS.iter().position(|o| *o == rule.operation)?;
            out.extend_from_slice(&(operation as u32).to_le_bytes());
        }
        out.extend_from_slice(&(self.process.len() as u64).to_le_bytes());
        for rule in &self.process {
            text(&mut out, &rule.program);
            texts(&mut out, &rule.env);
        }
        Some(out)
    }
}

/// An allowlist's canonical codec bytes, `Option[List[String]]`, as a runner request's
/// digest frames them (RUN-11).
pub fn allowlist_bytes(definitions: Option<&[String]>) -> Vec<u8> {
    let mut out = Vec::new();
    match definitions {
        None => out.extend_from_slice(&0u32.to_le_bytes()),
        Some(paths) => {
            out.extend_from_slice(&1u32.to_le_bytes());
            texts(&mut out, paths);
        }
    }
    out
}

fn text(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u64).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

fn texts(out: &mut Vec<u8>, texts: &[String]) {
    out.extend_from_slice(&(texts.len() as u64).to_le_bytes());
    for t in texts {
        text(out, t);
    }
}

/// RUN-2 coverage: the grant set `grants` narrowed to exactly `keep`, when `grants` covers
/// it; None otherwise. Flags are kept only where granted, mounts by their exact virtual path
/// with equal or lesser access (sorted, unique), net rules under `net.narrow` coverage and
/// process rules naming a granted program with a subset of its environment.
pub fn narrow(grants: &IoGrants, keep: &GrantSet) -> Option<IoGrants> {
    let flag = |kept: bool, granted: &Option<String>| match (kept, granted) {
        (false, _) => Some(None),
        (true, Some(provider)) => Some(Some(provider.clone())),
        (true, None) => None,
    };
    let mut narrow = IoGrants {
        stdout: flag(keep.stdout, &grants.stdout)?,
        stderr: flag(keep.stderr, &grants.stderr)?,
        tasks: flag(keep.tasks, &grants.tasks)?,
        monotonic_clock: flag(keep.monotonic_clock, &grants.monotonic_clock)?,
        wall_clock: flag(keep.wall_clock, &grants.wall_clock)?,
        random: flag(keep.random, &grants.random)?,
        ..IoGrants::default()
    };
    if keep.dirs.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
        return None;
    }
    for (path, access) in &keep.dirs {
        // v1 narrows a mount's access, never to a subdirectory.
        let mount = grants.dirs.iter().find(|m| &m.path == path)?;
        if !access.within(mount.access) {
            return None;
        }
        narrow.dirs.push(Mount {
            path: path.clone(),
            host: mount.host.clone(),
            access: *access,
        });
    }
    if !keep.net.is_empty() {
        let net = grants.net.as_ref()?;
        if !keep
            .net
            .iter()
            .all(|rule| net.rules.iter().any(|wide| net_rule_within(rule, wide)))
        {
            return None;
        }
        let mut net = net.clone();
        net.rules = keep.net.clone();
        narrow.net = Some(net);
    }
    if !keep.process.is_empty() {
        let process = grants.process.as_ref()?;
        if !keep.process.iter().all(|rule| {
            process
                .rules
                .iter()
                .any(|wide| process_rule_within(rule, wide))
        }) {
            return None;
        }
        let mut process = process.clone();
        process.rules = keep.process.clone();
        narrow.process = Some(process);
    }
    Some(narrow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u64s(n: u64) -> Vec<u8> {
        n.to_le_bytes().to_vec()
    }

    /// serialization.md codec version 1: bools as one byte, lists and strings behind a u64
    /// count or length, variant ordinals as u32, a port as u16, all little-endian.
    #[test]
    fn a_grant_set_and_an_allowlist_encode_canonically() {
        let set = GrantSet {
            stdout: true,
            stderr: false,
            tasks: true,
            monotonic_clock: false,
            wall_clock: false,
            random: true,
            dirs: vec![("/d".into(), Access::ReadWrite)],
            net: vec![NetRule {
                address: "h".into(),
                service: "443".into(),
                operation: "connect_tls".into(),
            }],
            process: vec![ProcessRule {
                program: "p".into(),
                env: vec!["E".into()],
            }],
        };
        let mut expected = vec![1, 0, 1, 0, 0, 1];
        expected.extend(u64s(1));
        expected.extend(u64s(2));
        expected.extend(b"/d");
        expected.extend(1u32.to_le_bytes());
        expected.extend(u64s(1));
        expected.extend(u64s(1));
        expected.extend(b"h");
        expected.extend(443u16.to_le_bytes());
        expected.extend(1u32.to_le_bytes());
        expected.extend(u64s(1));
        expected.extend(u64s(1));
        expected.extend(b"p");
        expected.extend(u64s(1));
        expected.extend(u64s(1));
        expected.extend(b"E");
        assert_eq!(set.canonical(), Some(expected));
        let unknown = GrantSet {
            net: vec![NetRule {
                address: "h".into(),
                service: "443".into(),
                operation: "teleport".into(),
            }],
            ..set
        };
        assert_eq!(unknown.canonical(), None);
        assert_eq!(allowlist_bytes(None), 0u32.to_le_bytes().to_vec());
        let mut some = 1u32.to_le_bytes().to_vec();
        some.extend(u64s(1));
        some.extend(u64s(1));
        some.extend(b"a");
        assert_eq!(allowlist_bytes(Some(&["a".to_string()])), some);
    }
}
