//! The grant subset relation (CAP-3): whether one validated grant record holds no
//! authority another lacks. It is lexical over validated data and reads no host state.
use crate::{Access, IoGrants, Mount, NetGrant, NetRule, ProcessGrant, ProcessRule};
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
