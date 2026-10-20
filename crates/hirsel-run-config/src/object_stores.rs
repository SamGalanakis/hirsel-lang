//! Declarative S3 authority, expanded into the existing Env/Net/Dir projections.
use crate::{RunConfig, invalid};
use hirsel_run_schema::{
    IoGrants, NetGrant, NetRule, RunRefusal, canonical_net_address, confined_relative,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectStore {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub cache_bytes: u64,
    pub credentials: CredentialsReference,
}
/// A relative file within an explicitly granted directory mount; never a secret.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialsReference {
    pub mount: String,
    pub path: String,
}

fn endpoint(url: &str) -> Option<NetRule> {
    let (authority, operation, default_port) = if let Some(rest) = url.strip_prefix("https://") {
        (rest, "connect_tls", 443)
    } else {
        (url.strip_prefix("http://")?, "connect", 80)
    };
    if !authority.is_ascii()
        || authority
            .bytes()
            .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
    {
        return None;
    }
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.contains(['/', '?', '#', '@', '\\']) {
        return None;
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, rest) = rest.split_once(']')?;
        host.parse::<std::net::Ipv6Addr>().ok()?;
        (
            host,
            if rest.is_empty() {
                default_port
            } else {
                parse_port(rest.strip_prefix(':')?)?
            },
        )
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.contains(':') {
            return None;
        }
        (host, parse_port(port)?)
    } else {
        (authority, default_port)
    };
    if !canonical_net_address(host) {
        return None;
    }
    Some(NetRule {
        address: host.into(),
        service: port.to_string(),
        operation: operation.into(),
    })
}
fn parse_port(text: &str) -> Option<u16> {
    let port: u16 = text.parse().ok()?;
    (port != 0 && port.to_string() == text).then_some(port)
}
pub(super) fn expand(record: &mut RunConfig) -> Result<(), RunRefusal> {
    if record.object_stores.is_empty() {
        return Ok(());
    }
    let io = record.io.get_or_insert_with(IoGrants::default);
    let env = record.env.get_or_insert_with(Default::default);
    if io.wall_clock.is_none() {
        io.wall_clock = Some("clock.wall.unix_ns@1".into());
    }
    for (name, store) in &record.object_stores {
        let fail = |detail: &str| invalid(&format!("object_stores.{name}"), detail);
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            || !name.as_bytes()[0].is_ascii_lowercase()
        {
            return Err(fail(
                "name is lowercase ASCII letters and digits, starting with a letter",
            ));
        }
        let rule = endpoint(&store.endpoint).ok_or_else(|| {
            fail("endpoint is an http(s) origin URL with a canonical host and port")
        })?;
        if store.bucket.len() < 3
            || store.bucket.len() > 63
            || !store
                .bucket
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'.'))
            || store.bucket.starts_with(['-', '.'])
            || store.bucket.ends_with(['-', '.'])
        {
            return Err(fail("invalid S3 bucket"));
        }
        if store.region.is_empty()
            || !store
                .region
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            return Err(fail("invalid signing region"));
        }
        if !store.prefix.is_empty() && !crate::portable_path(&store.prefix, false) {
            return Err(fail(
                "prefix is a canonical relative path without a trailing slash",
            ));
        }
        let reference = &store.credentials;
        if !io.dirs.iter().any(|m| m.path == reference.mount)
            || reference.path.is_empty()
            || confined_relative("", &reference.path).as_deref() != Some(reference.path.as_str())
        {
            return Err(fail(
                "credentials reference must name a canonical relative file inside a granted directory mount",
            ));
        }
        let grant = io.net.get_or_insert_with(|| NetGrant {
            provider: "net.stream@1".into(),
            rules: vec![],
            peers: None,
            listeners: vec![],
        });
        if !grant.rules.contains(&rule) {
            grant.rules.push(rule);
            grant.rules.sort();
        }
        for (key, value) in [
            ("ENDPOINT", store.endpoint.trim_end_matches('/').to_string()),
            ("BUCKET", store.bucket.clone()),
            ("REGION", store.region.clone()),
            ("PREFIX", store.prefix.clone()),
            ("CACHE_BYTES", store.cache_bytes.to_string()),
            ("CREDENTIALS_DIR", reference.mount.clone()),
            ("CREDENTIALS", reference.path.clone()),
        ] {
            let key = format!("HIRSEL_S3_{}_{key}", name.to_ascii_uppercase());
            match env.get(&key) {
                Some(Some(old)) if *old == value => {}
                Some(_) => {
                    return Err(fail(
                        "object store projection conflicts with an explicit Env snapshot",
                    ));
                }
                None => {
                    env.insert(key, Some(value));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn unique<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> Result<std::collections::BTreeMap<String, ObjectStore>, D::Error> {
    struct Entries;
    impl<'de> serde::de::Visitor<'de> for Entries {
        type Value = std::collections::BTreeMap<String, ObjectStore>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unique named object stores")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut values = std::collections::BTreeMap::new();
            while let Some((name, value)) = map.next_entry::<String, ObjectStore>()? {
                if values.insert(name.clone(), value).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate object store {name:?}"
                    )));
                }
            }
            Ok(values)
        }
    }
    de.deserialize_map(Entries)
}
