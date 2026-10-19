//! The `code` seam's configuration entry, as a runner or host lists it in
//! its run configuration.
use crate::sign::{PublicKey, Signatures};
use crate::store::CodeStore;
use hirsel_object_storage::{DirObjects, MemoryObjects};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;

fn members<'a>(entry: &'a Value, what: &str) -> Result<(&'a Map<String, Value>, &'a str), String> {
    let members = entry
        .as_object()
        .ok_or_else(|| format!("a {what} entry is a JSON object"))?;
    let provider = members
        .get("provider")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("a {what} entry names its provider"))?;
    Ok((members, provider))
}

fn closed(members: &Map<String, Value>, provider: &str, allowed: &[&str]) -> Result<(), String> {
    match members.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(unknown) => Err(format!("{provider} has no member {unknown:?}")),
        None => Ok(()),
    }
}

/// Parse a `signatures` entry: `{"provider": "sign.none@1"}` or
/// `{"provider": "sign.ed25519@1", "keys": ["<64 hex digits>", ...]}` with
/// the keys sorted, unique and at least one.
pub fn signatures(entry: &Value) -> Result<Signatures, String> {
    let (members, provider) = members(entry, "signatures")?;
    match provider {
        "sign.none@1" => {
            closed(members, provider, &["provider"])?;
            Ok(Signatures::None)
        }
        "sign.ed25519@1" => {
            closed(members, provider, &["provider", "keys"])?;
            let keys = members
                .get("keys")
                .and_then(Value::as_array)
                .ok_or("sign.ed25519@1 keys is a list of public keys")?
                .iter()
                .map(|key| {
                    key.as_str().and_then(PublicKey::parse).ok_or_else(|| {
                        format!("sign.ed25519@1 key {key} is not 64 lowercase hex digits")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if keys.is_empty() || !keys.windows(2).all(|w| w[0] < w[1]) {
                return Err("sign.ed25519@1 keys are sorted, unique and at least one".into());
            }
            Ok(Signatures::Ed25519 { keys })
        }
        other => Err(format!("unsupported signature provider {other}")),
    }
}

/// Open a `code` entry:
///
/// - `{"provider": "code.none@1"}` (the default): no store;
/// - `{"provider": "code.dir@1", "path": "<absolute directory>"}`;
/// - `{"provider": "code.object@1", "objects": <object storage entry>}`,
///   the entry opened by [`hirsel_object_storage::open`];
/// - `{"provider": "code.sim@1", "seed": <u64>, "fail_one_in": <u64>}`: an
///   in-memory store whose operations fail at seeded points (both default
///   0, never).
///
/// Every provider but `code.none@1` takes an optional `signatures` entry
/// (default `{"provider": "sign.none@1"}`). Unknown members and providers
/// are refused.
pub fn open(entry: &Value) -> Result<CodeStore, String> {
    let (members, provider) = members(entry, "code")?;
    let signed = || match members.get("signatures") {
        None => Ok(Signatures::None),
        Some(entry) => signatures(entry),
    };
    let uint = |name: &str| match members.get(name) {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("{provider} {name} is an unsigned integer")),
    };
    match provider {
        "code.none@1" => {
            closed(members, provider, &["provider"])?;
            Ok(CodeStore::none())
        }
        "code.dir@1" => {
            closed(members, provider, &["provider", "path", "signatures"])?;
            let path = members
                .get("path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .ok_or("code.dir@1 path is an absolute directory")?;
            let objects = DirObjects::open(path).map_err(|e| e.to_string())?;
            Ok(CodeStore::over("code.dir@1", Arc::new(objects), signed()?))
        }
        "code.object@1" => {
            closed(members, provider, &["provider", "objects", "signatures"])?;
            let objects = hirsel_object_storage::open(
                members
                    .get("objects")
                    .ok_or("code.object@1 names its objects entry")?,
            )?;
            Ok(CodeStore::over("code.object@1", objects, signed()?))
        }
        "code.sim@1" => {
            closed(
                members,
                provider,
                &["provider", "seed", "fail_one_in", "signatures"],
            )?;
            let objects = MemoryObjects::with_faults(uint("seed")?, uint("fail_one_in")?);
            Ok(CodeStore::over("code.sim@1", Arc::new(objects), signed()?))
        }
        other => Err(format!("unsupported code store provider {other}")),
    }
}
