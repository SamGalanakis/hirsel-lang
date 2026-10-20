//! Runner normalization and validation.
use crate::{definition_path, invalid};
use hirsel_run_schema::{
    DEFAULT_QUEUE, DEFAULT_THREADS, Endpoint, IoGrants, Roots, RunRefusal, RunnerEntry,
    sim_model_sites,
};
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
            "remote.tls@1" => remote(runner)?,
            _ => {
                return Err(RunRefusal::Unsupported {
                    field: "io.runners.provider".into(),
                });
            }
        }
        if runner.provider != "remote.tls@1"
            && (runner.endpoint.is_some()
                || runner.server_name.is_some()
                || runner.roots.is_some()
                || runner.identity.is_some()
                || runner.timeout_ns.is_some()
                || runner.reconnect.is_some()
                || runner.reconcile)
        {
            return Err(invalid(
                "io.runners",
                "endpoint, server_name, roots, identity, timeout_ns, reconnect and reconcile are remote.tls@1 settings",
            ));
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
        // A simulation has no other host: it models one with `sim@1`.
        if runner.provider == "remote.tls@1" {
            return Err(RunRefusal::Unsupported {
                field: "io.runners.provider".into(),
            });
        }
        if let Some(grants) = &runner.grants {
            crate::supported_io(grants, "io.runners.grants")?;
        }
    }
    Ok(runners)
}

/// `remote.tls@1`: an endpoint, a server name and trust anchors, all required; a client
/// identity, a timeout and reconnection attempts optional. The place's code store, code
/// policy, recording and queue are the host's own configuration, never the caller's.
fn remote(runner: &mut RunnerEntry) -> Result<(), RunRefusal> {
    if runner.node.is_some()
        || runner.models.is_some()
        || !runner.faults.is_empty()
        || runner.threads.is_some()
        || runner.queue.is_some()
    {
        return Err(invalid(
            "io.runners",
            "remote.tls@1 takes no node, models, faults, threads or queue",
        ));
    }
    for (field, present) in [
        ("io.runners.code", runner.code.is_some()),
        ("io.runners.admission", runner.admission.is_some()),
        ("io.runners.recording", runner.recording.is_some()),
    ] {
        if present {
            return Err(RunRefusal::Unsupported {
                field: field.into(),
            });
        }
    }
    match runner
        .endpoint
        .as_ref()
        .ok_or_else(|| invalid("io.runners.endpoint", "remote.tls@1 needs an endpoint"))?
    {
        Endpoint::Tcp { address, service } => {
            if !hirsel_run_schema::canonical_net_address(address)
                || service.is_empty()
                || !service.bytes().all(|b| b.is_ascii_digit())
                || (service.len() > 1 && service.starts_with('0'))
                || !service.parse::<u16>().is_ok_and(|port| port != 0)
            {
                return Err(invalid(
                    "io.runners.endpoint.tcp",
                    "expected a canonical address and a nonzero decimal service",
                ));
            }
        }
        Endpoint::Unix(path) => {
            if !hirsel_run_schema::canonical_unix_path(path) {
                return Err(invalid(
                    "io.runners.endpoint.unix",
                    "expected a canonical absolute socket path",
                ));
            }
        }
    }
    let name = runner
        .server_name
        .as_deref()
        .ok_or_else(|| invalid("io.runners.server_name", "remote.tls@1 needs a server name"))?;
    if hirsel_run_schema::tls::server_name(name).is_none() {
        return Err(invalid(
            "io.runners.server_name",
            "expected a DNS name or an IP address",
        ));
    }
    match runner
        .roots
        .as_ref()
        .ok_or_else(|| invalid("io.runners.roots", "remote.tls@1 needs roots"))?
    {
        Roots::Named(name) if name == "webpki" => {}
        Roots::Pem { pem } if crate::absolute_path(pem) => {}
        _ => {
            return Err(invalid(
                "io.runners.roots",
                "expected \"webpki\" or {\"pem\": absolute path}",
            ));
        }
    }
    if let Some(identity) = &runner.identity
        && !(crate::absolute_path(&identity.chain) && crate::absolute_path(&identity.key))
    {
        return Err(invalid(
            "io.runners.identity",
            "expected absolute chain and key paths",
        ));
    }
    if runner.timeout_ns == Some(0) {
        return Err(invalid("io.runners.timeout_ns", "a zero timeout"));
    }
    if runner
        .reconnect
        .is_some_and(|r| r.attempts == hirsel_run_schema::DEFAULT_RECONNECT)
    {
        runner.reconnect = None;
    }
    Ok(())
}
