//! Run grant normalization and validation.
use crate::invalid;
use hirsel_run_schema::canonical_net_address as canonical_address;
use hirsel_run_schema::*;
use std::net::IpAddr;
fn canonical_ip(address: &str) -> bool {
    address
        .parse::<IpAddr>()
        .is_ok_and(|ip| ip.to_string() == address)
}

/// A launcher's standalone `io.net` grant: the JSON of a RunConfig's `io.net` field, admitted
/// exactly as that field is (provider contract, canonical rules, peers only for
/// net.script@2). Peers supply data and never widen the rules.
pub fn validate_net_grant(bytes: &[u8]) -> Result<NetGrant, RunRefusal> {
    let mut grant: NetGrant =
        serde_json::from_slice(bytes).map_err(|error| invalid("io.net", &error.to_string()))?;
    if !matches!(
        ProviderContract::parse(&grant.provider),
        Some(ProviderContract::NetStream | ProviderContract::NetScript)
    ) {
        return Err(RunRefusal::Unsupported {
            field: "io.net.provider".into(),
        });
    }
    validate(&mut grant)?;
    Ok(grant)
}

pub(super) fn validate(grant: &mut NetGrant) -> Result<(), RunRefusal> {
    let script = ProviderContract::parse(&grant.provider) == Some(ProviderContract::NetScript);
    if grant.peers.is_some() != script {
        return Err(invalid(
            "io.net.peers",
            "peers is required only for net.script@2",
        ));
    }
    if !grant.listeners.is_empty() && !script {
        return Err(invalid(
            "io.net.listeners",
            "listeners are allowed only for net.script@2",
        ));
    }
    check_net_listeners(&grant.listeners).map_err(|e| invalid("io.net.listeners", &e))?;
    grant.rules.sort();
    for rule in &grant.rules {
        let unix = UNIX_OPERATIONS.contains(&rule.operation.as_str());
        if unix && !canonical_unix_path(&rule.address) {
            return Err(invalid(
                "io.net.rules.address",
                "expected a canonical absolute socket path",
            ));
        }
        if unix && rule.service != "0" {
            return Err(invalid(
                "io.net.rules.service",
                "a Unix socket rule has service 0",
            ));
        }
        if !unix && !canonical_address(&rule.address) {
            return Err(invalid(
                "io.net.rules.address",
                "expected a canonical host name or IP literal",
            ));
        }
        if rule.service.is_empty()
            || !rule.service.bytes().all(|b| b.is_ascii_digit())
            || (rule.service.len() > 1 && rule.service.starts_with('0'))
            || rule.service.parse::<u16>().is_err()
        {
            return Err(invalid(
                "io.net.rules.service",
                "expected a canonical decimal port 0–65535",
            ));
        }
        if !unix
            && !matches!(
                rule.operation.as_str(),
                "connect" | "connect_tls" | "listen" | "udp_bind" | "udp_peer" | "listen_tls"
            )
        {
            return Err(invalid(
                "io.net.rules.operation",
                "expected connect, connect_tls, listen, udp_bind, udp_peer, listen_tls, unix_connect, unix_listen or unix_listen_remove_stale",
            ));
        }
    }
    if grant.rules.windows(2).any(|p| p[0] == p[1]) {
        return Err(invalid("io.net.rules", "duplicate restriction"));
    }
    if let Some(peers) = &grant.peers {
        let mut seen = std::collections::BTreeMap::new();
        for (index, peer) in peers.iter().enumerate() {
            if let Some(first) = seen.insert((&peer.address, peer.port), index) {
                return Err(invalid(
                    "io.net.peers",
                    &format!("duplicate address/port at entries {first} and {index}"),
                ));
            }
            check_peer_address(peer).map_err(|e| invalid("io.net.peers.address", &e))?;
            if !peer.resolves_to.iter().all(|address| canonical_ip(address)) {
                return Err(invalid(
                    "io.net.peers.resolves_to",
                    "expected canonical IP literals",
                ));
            }
        }
    }
    Ok(())
}
