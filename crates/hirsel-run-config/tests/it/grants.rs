use hirsel_run_config::*;
use hirsel_run_schema::*;
use proptest::prelude::*;
use std::collections::BTreeMap;

/// Choices read from a byte string; an exhausted string picks 0, the simplest choice, so
/// shrinking the bytes shrinks the record.
pub(super) struct Draw<'a>(pub(super) &'a [u8]);
impl Draw<'_> {
    fn pick(&mut self, n: usize) -> usize {
        match self.0.split_first() {
            Some((b, rest)) => {
                self.0 = rest;
                *b as usize % n
            }
            None => 0,
        }
    }
    fn coin(&mut self) -> bool {
        self.pick(2) == 1
    }
}

const PROJECTIONS: [&str; 6] = [
    "stdin.bytes@1",
    "stderr.bytes@1",
    "stdout.bytes@1",
    "tasks.cooperative@1",
    "clock.monotonic.ns@1",
    "clock.wall.unix_ns@1",
];
const RULES: [(&str, &str, &str); 9] = [
    ("example.com", "443", "connect"),
    ("example.com", "443", "connect_tls"),
    ("127.0.0.1", "8080", "listen"),
    ("127.0.0.1", "8080", "listen_tls"),
    ("127.0.0.1", "0", "udp_bind"),
    ("127.0.0.1", "9001", "udp_peer"),
    ("/run/a.sock", "0", "unix_listen_remove_stale"),
    ("/run/a.sock", "0", "unix_listen"),
    ("/run/a.sock", "0", "unix_connect"),
];

fn projection(io: &mut IoGrants, k: usize) -> &mut Option<String> {
    match k {
        0 => &mut io.stdin,
        1 => &mut io.stderr,
        2 => &mut io.stdout,
        3 => &mut io.tasks,
        4 => &mut io.monotonic_clock,
        5 => &mut io.wall_clock,
        _ => &mut io.random,
    }
}

fn join(host: &str, part: &str) -> String {
    if host == "/" {
        format!("/{part}")
    } else {
        format!("{host}/{part}")
    }
}

fn overlaps(a: &str, b: &str) -> bool {
    a.is_empty()
        || b.is_empty()
        || a == b
        || a.starts_with(&format!("{b}/"))
        || b.starts_with(&format!("{a}/"))
}

fn rule((address, service, operation): (&str, &str, &str)) -> NetRule {
    NetRule {
        address: address.into(),
        service: service.into(),
        operation: operation.into(),
    }
}

/// A record over a small universe, so independent records are often related.
pub(super) fn record(d: &mut Draw) -> RunConfig {
    let mut r = RunConfig::empty([0; 32], "probe@1/main".into(), LaunchKind::Main);
    r.unsafe_root = d.coin();
    if d.coin() {
        let mut io = IoGrants::default();
        for k in 0..6 {
            if d.coin() {
                *projection(&mut io, k) = Some(PROJECTIONS[k].into());
            }
        }
        io.random = [None, Some("random.splitmix64@1"), Some("random.system@1")][d.pick(3)]
            .map(str::to_owned);
        for path in ["", "a", "a/x", "b"] {
            if d.coin() && io.dirs.iter().all(|m| !overlaps(&m.path, path)) {
                let base = ["/h", "/g"][d.pick(2)];
                io.dirs.push(Mount {
                    path: path.into(),
                    host: if path.is_empty() {
                        base.into()
                    } else {
                        join(base, path)
                    },
                    access: [Access::Read, Access::ReadWrite][d.pick(2)],
                });
            }
        }
        io.net = match d.pick(3) {
            0 => None,
            n => Some(NetGrant {
                provider: ["net.stream@1", "net.script@2"][n - 1].into(),
                rules: RULES.into_iter().filter(|_| d.coin()).map(rule).collect(),
                peers: (n == 2).then(Vec::new),
                listeners: vec![],
            }),
        };
        if d.coin() {
            io.process = Some(ProcessGrant {
                timed: None,
                provider: "process.spawn@1".into(),
                rules: ["/bin/a", "/bin/b"]
                    .into_iter()
                    .filter(|_| d.coin())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|program| ProcessRule {
                        program: program.into(),
                        env: ["HOME", "PATH"]
                            .into_iter()
                            .filter(|_| d.coin())
                            .map(str::to_owned)
                            .collect(),
                    })
                    .collect(),
                runs: None,
                children: None,
            });
        }
        io.signals = match d.pick(3) {
            0 => None,
            provider => Some(signal_grant(provider, 1 + d.pick(7) as u8, d)),
        };
        io.telemetry = match d.pick(3) {
            0 => None,
            1 => Some(TelemetryGrant {
                provider: "telemetry.capture@1".into(),
                seed: None,
                queue: None,
            }),
            _ => Some(TelemetryGrant {
                provider: "telemetry.null@1".into(),
                seed: Some(d.pick(3) as u64),
                queue: d.coin().then(|| d.pick(3) as u64),
            }),
        };
        r.io = Some(io);
    }
    if d.coin() {
        let lang = ["x", "y"][d.pick(2)];
        r.env = Some(
            [("HOME", Some("h")), ("PATH", None), ("LANG", Some(lang))]
                .into_iter()
                .filter(|_| d.coin())
                .map(|(k, v)| (k.to_owned(), v.map(str::to_owned)))
                .collect(),
        );
    }
    r
}

// Explicit test encoding: independent of SignalKind::bit/bits and signal_kinds.
pub(super) fn signal_grant(provider: usize, mask: u8, d: &mut Draw) -> SignalsGrant {
    let kinds = [
        (1, SignalKind::Interrupt),
        (2, SignalKind::Terminate),
        (4, SignalKind::Hangup),
    ]
    .into_iter()
    .filter(|(bit, _)| mask & bit != 0)
    .map(|(_, kind)| kind)
    .collect();
    SignalsGrant {
        provider: ["signals.host@1", "signals.script@1"][provider - 1].into(),
        kinds,
        script: (provider == 2).then(|| {
            (0..d.pick(5))
                .map(|_| {
                    [
                        SignalKind::Interrupt,
                        SignalKind::Terminate,
                        SignalKind::Hangup,
                    ][d.pick(3)]
                })
                .collect()
        }),
    }
}

fn nth<T>(items: &[T], d: &mut Draw) -> Option<usize> {
    (!items.is_empty()).then(|| d.pick(items.len()))
}

/// One narrowing step: it removes authority or keeps the record as it is.
fn narrow_once(r: &mut RunConfig, d: &mut Draw) {
    match d.pick(17) {
        0 => r.unsafe_root = false,
        1 => r.io = None,
        2 => r.env = None,
        3 => {
            if let Some(env) = &mut r.env
                && let Some(key) = env.keys().nth(d.pick(env.len().max(1))).cloned()
            {
                env.remove(&key);
            }
        }
        op => {
            let Some(io) = &mut r.io else { return };
            match op {
                4 => *projection(io, d.pick(7)) = None,
                5 => {
                    if let Some(i) = nth(&io.dirs, d) {
                        io.dirs.remove(i);
                    }
                }
                6 => {
                    if let Some(i) = nth(&io.dirs, d) {
                        io.dirs[i].access = Access::Read;
                    }
                }
                7 => {
                    if let Some(i) = nth(&io.dirs, d) {
                        let m = &mut io.dirs[i];
                        m.path = if m.path.is_empty() {
                            "n".into()
                        } else {
                            format!("{}/n", m.path)
                        };
                        m.host = join(&m.host, "n");
                    }
                }
                8 => io.net = None,
                9 | 10 => {
                    let Some(net) = &mut io.net else { return };
                    let Some(i) = nth(&net.rules, d) else { return };
                    let narrower = match net.rules[i].operation.as_str() {
                        "connect" => "connect_tls",
                        "listen" => "listen_tls",
                        "unix_listen_remove_stale" => "unix_listen",
                        _ => "",
                    };
                    let replacement = NetRule {
                        operation: narrower.into(),
                        ..net.rules[i].clone()
                    };
                    if op == 9 || narrower.is_empty() || net.rules.contains(&replacement) {
                        net.rules.remove(i);
                    } else {
                        net.rules[i] = replacement;
                    }
                }
                11 => io.process = None,
                12 | 13 => {
                    let Some(process) = &mut io.process else {
                        return;
                    };
                    let Some(i) = nth(&process.rules, d) else {
                        return;
                    };
                    if op == 12 {
                        process.rules.remove(i);
                    } else if let Some(j) = nth(&process.rules[i].env, d) {
                        process.rules[i].env.remove(j);
                    }
                }
                14 => io.signals = None,
                15 => {
                    let Some(signals) = &mut io.signals else {
                        return;
                    };
                    let i = d.pick(signals.kinds.len());
                    if signals.kinds.len() == 1 {
                        io.signals = None;
                    } else {
                        signals.kinds.remove(i);
                    }
                }
                _ => io.telemetry = None,
            }
        }
    }
}

fn narrow(r: &RunConfig, d: &mut Draw) -> RunConfig {
    let mut r = r.clone();
    for _ in 0..d.pick(5) {
        narrow_once(&mut r, d);
    }
    r
}

/// One step that adds authority `r` lacks, or None when it does not apply.
fn widen(r: &RunConfig, d: &mut Draw) -> Option<RunConfig> {
    let mut r = r.clone();
    match d.pick(17) {
        0 if r.unsafe_root => return None,
        0 => r.unsafe_root = true,
        1 if r.io.is_some() => return None,
        1 => r.io = Some(IoGrants::default()),
        2 if r.env.is_some() => return None,
        2 => r.env = Some(BTreeMap::new()),
        3 => {
            r.env.as_mut()?.insert("WIDE".into(), None);
        }
        4 => {
            let env = r.env.as_mut()?;
            let key = env.keys().nth(d.pick(env.len().max(1)))?.clone();
            env.insert(key, Some("wide".into()));
        }
        op => {
            let io = r.io.as_mut()?;
            match op {
                5 => {
                    let k = d.pick(6);
                    let slot = projection(io, k);
                    if slot.is_some() {
                        return None;
                    }
                    *slot = Some(PROJECTIONS[k].into());
                }
                6 => {
                    io.random = Some(
                        match io.random.as_deref() {
                            Some("random.system@1") => "random.splitmix64@1",
                            _ => "random.system@1",
                        }
                        .into(),
                    )
                }
                7 => {
                    let m = io.dirs.iter_mut().find(|m| m.access == Access::Read)?;
                    m.access = Access::ReadWrite;
                }
                8 => {
                    let i = nth(&io.dirs, d)?;
                    io.dirs[i].host = join(&io.dirs[i].host, "elsewhere");
                }
                9 => {
                    let i = nth(&io.dirs, d)?;
                    let (parent, _) = io.dirs[i].path.rsplit_once('/')?;
                    let host = io.dirs[i].host.rsplit_once('/')?.0.to_owned();
                    io.dirs[i].path = parent.into();
                    io.dirs[i].host = host;
                }
                10 if io.net.is_none() => {
                    io.net = Some(NetGrant {
                        provider: "net.stream@1".into(),
                        rules: vec![],
                        peers: None,
                        listeners: vec![],
                    })
                }
                10 => {
                    let net = io.net.as_mut()?;
                    let i = nth(&net.rules, d)?;
                    let wider = match net.rules[i].operation.as_str() {
                        "connect_tls" => "connect",
                        "listen_tls" => "listen",
                        "unix_listen" => "unix_listen_remove_stale",
                        _ => return None,
                    };
                    let replacement = NetRule {
                        operation: wider.into(),
                        ..net.rules[i].clone()
                    };
                    if net.rules.contains(&replacement) {
                        return None;
                    }
                    net.rules[i] = replacement;
                }
                11 => io
                    .net
                    .as_mut()?
                    .rules
                    .push(rule(("wide.example", "1", "connect"))),
                12 if io.process.is_none() => {
                    io.process = Some(ProcessGrant {
                        timed: None,
                        provider: "process.spawn@1".into(),
                        rules: vec![],
                        runs: None,
                        children: None,
                    })
                }
                12 => io.process.as_mut()?.rules.push(ProcessRule {
                    program: "/wide/bin".into(),
                    env: vec![],
                }),
                13 => {
                    let process = io.process.as_mut()?;
                    let i = nth(&process.rules, d)?;
                    process.rules[i].env.push("WIDE".into());
                }
                14 => {
                    if let Some(signals) = &mut io.signals {
                        let kind = [
                            SignalKind::Interrupt,
                            SignalKind::Terminate,
                            SignalKind::Hangup,
                        ]
                        .into_iter()
                        .find(|kind| !signals.kinds.contains(kind))?;
                        signals.kinds.push(kind);
                    } else {
                        io.signals = Some(signal_grant(1 + d.pick(2), 1 + d.pick(7) as u8, d));
                    }
                }
                15 => {
                    let signals = io.signals.as_mut()?;
                    if signals.provider == "signals.host@1" {
                        signals.provider = "signals.script@1".into();
                        signals.script = Some(vec![]);
                    } else {
                        signals.provider = "signals.host@1".into();
                        signals.script = None;
                    }
                }
                _ => {
                    // Preserve current whole-record telemetry comparison; do not infer
                    // that a seed or queue change is a semantic weakening.
                    if let Some(telemetry) = &mut io.telemetry {
                        telemetry.provider = "telemetry.null@1".into();
                        telemetry.seed = Some(telemetry.seed.unwrap_or(0) + 1);
                    } else {
                        io.telemetry = Some(TelemetryGrant {
                            provider: "telemetry.capture@1".into(),
                            seed: None,
                            queue: None,
                        });
                    }
                }
            }
        }
    }
    Some(r)
}

pub(super) fn valid(r: &RunConfig) -> ValidatedRun {
    r.validated().expect("generated records are valid")
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x22_51_6e_a1),
        ..ProptestConfig::default()
    })]

    #[test]
    fn narrowing_yields_a_subset(bytes in proptest::collection::vec(any::<u8>(), 0..128)) {
        let d = &mut Draw(&bytes);
        let wide = record(d);
        let _ = valid(&wide);
        let narrow = narrow(&wide, d);
        prop_assert!(grants_within(&valid(&narrow), &valid(&wide)));
    }

    #[test]
    fn widening_never_does(bytes in proptest::collection::vec(any::<u8>(), 0..128)) {
        let d = &mut Draw(&bytes);
        let base = record(d);
        let _ = valid(&base);
        let base = narrow(&base, d);
        let validated_base = valid(&base);
        if let Some(wide) = widen(&base, d) {
            prop_assert!(!grants_within(&valid(&wide), &validated_base));
        }
    }

    #[test]
    fn reflexivity_and_transitive_narrowing(bytes in proptest::collection::vec(any::<u8>(), 0..128)) {
        let d = &mut Draw(&bytes);
        let wide = record(d);
        let middle = narrow(&wide, d);
        let small = narrow(&middle, d);
        let (wide, middle, small) = (valid(&wide), valid(&middle), valid(&small));
        prop_assert!(grants_within(&wide, &wide));
        prop_assert!(grants_within(&middle, &middle));
        prop_assert!(grants_within(&small, &small));
        prop_assert!(grants_within(&small, &middle));
        prop_assert!(grants_within(&middle, &wide));
        prop_assert!(grants_within(&small, &wide));
    }

}

fn io(io: IoGrants) -> ValidatedRun {
    let mut r = RunConfig::empty([0; 32], "probe@1/main".into(), LaunchKind::Main);
    r.io = Some(io);
    valid(&r)
}

fn mount(path: &str, host: &str, access: Access) -> Mount {
    Mount {
        path: path.into(),
        host: host.into(),
        access,
    }
}

#[test]
fn a_mount_is_within_the_same_subtree_of_its_virtual_path_and_host() {
    let dirs = |dirs| {
        io(IoGrants {
            dirs,
            ..IoGrants::default()
        })
    };
    let root = dirs(vec![mount("", "/", Access::ReadWrite)]);
    let src = dirs(vec![mount("src", "/src", Access::Read)]);
    assert!(grants_within(&src, &root));
    assert!(!grants_within(&root, &src));
    // The same host under another virtual name, or another host under the same name.
    assert!(!grants_within(
        &dirs(vec![mount("lib", "/src", Access::Read)]),
        &root
    ));
    assert!(!grants_within(
        &dirs(vec![mount("src", "/lib", Access::Read)]),
        &root
    ));
    // A sibling whose name extends the mount's is not beneath it.
    let a = dirs(vec![mount("a", "/w/a", Access::ReadWrite)]);
    assert!(!grants_within(
        &dirs(vec![mount("ab", "/w/ab", Access::Read)]),
        &a
    ));
    assert!(!grants_within(
        &dirs(vec![mount("a/b", "/w/ab", Access::Read)]),
        &a
    ));
    assert!(grants_within(
        &dirs(vec![mount("a/b", "/w/a/b", Access::Read)]),
        &a
    ));
}

#[test]
fn absent_io_and_env_differ_from_empty_ones() {
    let none = valid(&RunConfig::empty(
        [0; 32],
        "probe@1/main".into(),
        LaunchKind::Main,
    ));
    let empty = io(IoGrants::default());
    assert!(grants_within(&none, &empty));
    assert!(!grants_within(&empty, &none));
    let mut env = RunConfig::empty([0; 32], "probe@1/main".into(), LaunchKind::Main);
    env.env = Some(BTreeMap::new());
    assert!(!grants_within(&valid(&env), &none));
    env.env = Some(BTreeMap::from([("HOME".into(), None)]));
    let unset = valid(&env);
    env.env = Some(BTreeMap::from([("HOME".into(), Some("/h".into()))]));
    // A granted name's snapshot is part of the grant, unset included.
    assert!(!grants_within(&unset, &valid(&env)));
}

#[test]
fn scripted_provider_data_is_not_authority() {
    let net = |peers: Vec<NetPeer>| {
        io(IoGrants {
            net: Some(NetGrant {
                provider: "net.script@2".into(),
                rules: vec![rule(("example.com", "80", "connect"))],
                peers: Some(peers),
                listeners: vec![],
            }),
            ..IoGrants::default()
        })
    };
    let peer = NetPeer {
        address: "example.com".into(),
        port: 80,
        resolves_to: vec!["192.0.2.1".into()],
        tls: PeerTls::Plain,
        connections: vec![NetConnection::Refused],
    };
    assert!(grants_within(&net(vec![peer]), &net(vec![])));
    let stream = io(IoGrants {
        net: Some(NetGrant {
            provider: "net.stream@1".into(),
            rules: vec![rule(("example.com", "80", "connect"))],
            peers: None,
            listeners: vec![],
        }),
        ..IoGrants::default()
    });
    // A scripted world is a different provider from the host network.
    assert!(!grants_within(&net(vec![]), &stream));
}

fn malformed_template(bytes: &[u8]) -> String {
    let Err(RunRefusal::Malformed { detail }) =
        RunConfig::bind_grants(bytes, [0; 32], "probe@1/main".into(), LaunchKind::Main)
    else {
        panic!("expected a malformed grant template")
    };
    detail
}

#[test]
fn missing_grant_fields_name_the_containing_object_and_field() {
    for (text, path, field) in [
        (r#"{"io":{"stdout":"stdout.bytes@1"}}"#, "io.dirs", "dirs"),
        (
            r#"{"io":{"dirs":[{"path":"data","access":"read"}]}}"#,
            "io.dirs[0].host",
            "host",
        ),
        (
            r#"{"io":{"dirs":[],"telemetry":{}}}"#,
            "io.telemetry.provider",
            "provider",
        ),
    ] {
        let detail = malformed_template(text.as_bytes());
        assert!(
            detail.starts_with(&format!(
                "{path}: missing field `{field}` at line 1 column "
            )),
            "{detail}"
        );
    }
    for text in ["{}", r#"{"io":null}"#, r#"{"io":{"dirs":[]}}"#] {
        RunConfig::bind_grants(
            text.as_bytes(),
            [0; 32],
            "probe@1/main".into(),
            LaunchKind::Main,
        )
        .unwrap();
    }
}

#[test]
fn nested_grant_object_array_and_option_errors_keep_locations_without_values() {
    // serde_path_to_error tests/deserialize.rs:8-95, pinned 7aad3f59: nested
    // struct, Vec and present Option vectors, expressed in Hirsel's closed schema.
    for (text, path, reason) in [
        (
            r#"{"io":{"dirs":[{"path":"data","host":true,"access":"read"}]}}"#,
            "io.dirs[0].host",
            "expected a string",
        ),
        (r#"{"args":["valid",{}]}"#, "args[1]", "expected a string"),
        (
            r#"{"io":{"dirs":[],"telemetry":{"provider":"telemetry.null@1","seed":true}}}"#,
            "io.telemetry.seed",
            "expected u64",
        ),
    ] {
        let detail = malformed_template(text.as_bytes());
        assert!(
            detail.starts_with(&format!("{path}: invalid type:")),
            "{detail}"
        );
        assert!(
            detail.contains(reason) && detail.contains(" at line 1 column "),
            "{detail}"
        );
    }
}

#[test]
fn grant_schema_prechecks_and_complete_json_consumption_stay_strict() {
    for (text, reason) in [
        (r#"{"args":[],"args":[]}"#, "duplicate field `args`"),
        (r#"{"io":{"dirs":[],"dirs":[]}}"#, "duplicate field `dirs`"),
        (r#"{"io":{"dirs":[],"unknown":true}}"#, "unknown field"),
        (
            r#"{"env":{"TOKEN":"secret","TOKEN":"secret"}}"#,
            "duplicate Env key",
        ),
        ("{", "EOF"),
        (r#"{"args":[}"#, "expected value"),
        ("{} true", "trailing characters"),
    ] {
        let detail = malformed_template(text.as_bytes());
        assert!(
            detail.contains(reason) && detail.contains(" at line 1 column "),
            "{detail}"
        );
        assert!(!detail.contains("secret"), "{detail}");
    }
    for number in ["01", "-0", "1.0", "1e0"] {
        let text = format!(r#"{{"inputs":{{"hash_seed":{number}}}}}"#);
        assert_eq!(malformed_template(text.as_bytes()), "noncanonical integer");
    }
    assert!(malformed_template(b"\xff").contains("line 1 column"));
    RunConfig::bind_grants(b"{} \n\t", [0; 32], "probe@1/main".into(), LaunchKind::Main).unwrap();
}

#[test]
fn malformed_grants_never_echo_rejected_or_neighboring_credentials() {
    const SECRET: &str = "credential-DO-NOT-PRINT-\"quoted\"-\\-λ";
    for value in [
        serde_json::json!({"io":{"dirs":SECRET}}),
        serde_json::json!({"args":SECRET}),
        serde_json::json!({"admission":{"expires":SECRET}}),
        serde_json::json!({"io":{"dirs":[{"path":"data","host":"/data","access":SECRET}]}}),
        serde_json::json!({"placement":{"cores":SECRET}}),
    ] {
        let mut value = value;
        value["env"] = serde_json::json!({"TOKEN":SECRET});
        let detail = malformed_template(&serde_json::to_vec(&value).unwrap());
        assert!(
            !detail.contains("credential-DO-NOT-PRINT") && !detail.contains("quoted"),
            "{detail}"
        );
        assert!(detail.contains(" at line 1 column "), "{detail}");
    }
}
