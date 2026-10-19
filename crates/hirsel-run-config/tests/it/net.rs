use hirsel_run_config::*;
use serde_json::{Value, json};

fn record(net: Value) -> Value {
    let mut record = serde_json::to_value(RunConfig::empty(
        [0; 32],
        "probe@1/main".into(),
        LaunchKind::Main,
    ))
    .unwrap();
    record["io"] = json!({"dirs": [], "net": net});
    record
}

fn grant(address: &str) -> Value {
    json!({"provider": "net.stream@1", "rules": [
        {"address": address, "service": "443", "operation": "connect_tls"}
    ]})
}

fn validate(net: Value) -> Result<ValidatedRun, RunRefusal> {
    RunConfig::validate(&serde_json::to_vec(&record(net)).unwrap())
}

fn invalid(net: Value, expected: &str) {
    assert!(
        matches!(validate(net), Err(RunRefusal::Invalid { field, .. }) if field == expected),
        "expected invalid {expected}"
    );
    validate(grant("example.com")).unwrap();
}

#[test]
fn zig_hostname_validate_vectors_with_canonical_grant_restrictions() {
    // Ported first from Zig lib/std/Io/net/HostName.zig, test validate,
    // bff490a2. Hirsel additionally rejects trailing dots and uppercase.
    for name in [
        "example",
        "example.com",
        "www.example.com",
        "sub.domain.example.com",
        "host-name.example.com",
        "123.example.com",
        "a-b.com",
        "a.b.c.d.e.f.g",
        "127.0.0.1",
        &format!("{}.com", "a".repeat(63)),
        &format!("{}a", "a.".repeat(126)),
    ] {
        validate(grant(name)).unwrap();
    }
    for name in [
        "",
        ".example.com",
        "example.com..",
        "host..domain",
        "-hostname",
        "hostname-",
        "hostname-.com",
        "a.-.b",
        "host_name.com",
        ".",
        "..",
        &format!("{}a.com", "a".repeat(63)),
        &format!("{}ab", "a.".repeat(126)),
        &format!("{}ab.", "a.".repeat(126)),
        "example.com.",
        "host-name.example.com.",
        "123.example.com.",
        &format!("{}a.", "a.".repeat(126)),
        "Example.com",
        "EXAMPLE",
        "café.example",
        " example.com",
        "example.com ",
        "example.com\0",
    ] {
        invalid(grant(name), "io.net.rules.address");
    }
}

#[test]
fn numeric_last_labels_require_canonical_ipv4_in_rules_and_peers() {
    // WHATWG URL, ends-in-a-number checker; the IPv4-number parser also
    // accepts an empty hexadecimal suffix ("0x" denotes zero).
    for address in [
        "0x7f.0.0.1",
        "example.123",
        "127.1",
        "1.2.3.4.5",
        "example.0x7f",
        "example.0XFF",
        "example.0x",
        "0x7f000001",
    ] {
        invalid(grant(address), "io.net.rules.address");
        invalid(
            json!({"provider":"net.script@2", "rules":[], "peers":[{
                "address":address, "port":443, "resolves_to":[], "tls":"plain", "connections":[]
            }]}),
            "io.net.peers.address",
        );
    }
    for address in ["127.0.0.1", "example.123a", "example.0xgg", "123.example"] {
        validate(grant(address)).unwrap();
    }
}

#[test]
fn rust_ip_parser_and_display_vectors_require_exact_canonical_spelling() {
    // Rust library/coretests/tests/net/{parser,ip_addr}.rs, f6c1107f:
    // parse_ip, ipv6_corner_cases/failures, ipv6_addr_to_string.
    for address in [
        "192.168.0.1",
        "0.0.0.0",
        "255.255.255.255",
        "2001:db8::c0a8:1",
        "1::",
        "1:1::",
        "::1",
        "::1:1",
        "::",
        "::c0a8:1",
        "::1:c0a8:1",
        "1:1:1:1:1:1:c0a8:1",
        "::ffff:192.0.2.128",
        "::c000:280",
        "8:9:a:b:c:d:e:f",
        "1111:2222:3333:4444:5555:6666:7777:8888",
        "ae::ffff:102:304",
        "1:2:3:4:5:6:0:8",
        "1:0:0:4::8",
        "1::4:5:0:0:8",
    ] {
        validate(grant(address)).unwrap();
    }
    for address in [
        "192.168.0.1:8080",
        "0127.0.0.1",
        "127.00.0.1",
        "127.1",
        "2130706433",
        "256.0.0.1",
        "1.2.3.4.5",
        "2001:db8:0:0:0:0:c0a8:1",
        "2001:db8::192.168.0.1",
        "2001:DB8::c0a8:1",
        "2001:0db8::c0a8:1",
        "2001:db8::0127.0.0.1",
        "2001:db8::0x10.0.0.1",
        "[2001:db8::c0a8:1]:8080",
        "1:192.168.0.1::",
        "1:1:1:1::1:1:1:1",
        "1:1:1:1:1:1:1:1:8080",
        "[::1]",
        "fe80::1%eth0",
        "fe80::1%1",
        "::ffff:c000:280",
        "1:0:0:4:5::8",
        "1:2:3:4:5:6::8",
    ] {
        invalid(grant(address), "io.net.rules.address");
    }
}

#[test]
fn wildcard_subnet_range_and_url_addresses_have_exact_host_twins() {
    for address in [
        "*",
        "*.example.com",
        "192.0.2.0/24",
        "2001:db8::/32",
        "https://example.com",
        "example.com:443",
        "user@example.com",
        "[::]",
        "?",
    ] {
        invalid(grant(address), "io.net.rules.address");
    }
}

#[test]
fn services_are_canonical_decimal_u16_ports_and_operations_are_closed() {
    for service in ["0", "1", "80", "443", "65535"] {
        for operation in ["connect", "connect_tls", "listen"] {
            let mut net = grant("example.com");
            net["rules"][0]["service"] = service.into();
            net["rules"][0]["operation"] = operation.into();
            validate(net).unwrap();
        }
    }
    for service in [
        "",
        "*",
        "https",
        "443-444",
        "0443",
        "+443",
        "-1",
        "65536",
        "99999999999999999999",
        "443 ",
        " 443",
        "4.43",
        "4e2",
        "４４３",
        "443\0",
    ] {
        let mut net = grant("example.com");
        net["rules"][0]["service"] = service.into();
        invalid(net, "io.net.rules.service");
    }
    for operation in [
        "",
        "*",
        "read",
        "write",
        "Connect",
        "connect-tls",
        "listen ",
        "connect\0",
    ] {
        let mut net = grant("example.com");
        net["rules"][0]["operation"] = operation.into();
        invalid(net, "io.net.rules.operation");
    }
}

#[test]
fn net_providers_are_closed_and_cannot_fill_other_projections() {
    for name in ["net.stream@1", "net.script@2"] {
        let mut net = grant("example.com");
        net["provider"] = name.into();
        if name == "net.script@2" {
            net["peers"] = json!([]);
        }
        validate(net).unwrap();
        assert_eq!(ProviderContract::parse(name).unwrap().name(), name);
        let mut config = record(grant("example.com"));
        config["io"]["stdout"] = name.into();
        assert!(
            matches!(RunConfig::validate(&serde_json::to_vec(&config).unwrap()),
            Err(RunRefusal::Unsupported { field }) if field == "io.stdout")
        );
    }
    for name in [
        "net@1",
        "net.stream@2",
        "net.script@0",
        "net.script@1",
        "net.script@3",
        "stdout.bytes@1",
        " net.stream@1",
    ] {
        let mut net = grant("example.com");
        net["provider"] = name.into();
        assert_eq!(
            validate(net),
            Err(RunRefusal::Unsupported {
                field: "io.net.provider".into()
            })
        );
    }
}

#[test]
fn rules_sort_as_unique_triples_and_an_empty_net_still_grants_a_projection() {
    let mut net = grant("example.com");
    let rule = net["rules"][0].clone();
    net["rules"].as_array_mut().unwrap().push(rule);
    invalid(net.clone(), "io.net.rules");
    net["rules"][1]["operation"] = "connect".into();
    let first = validate(net.clone()).unwrap();
    net["rules"].as_array_mut().unwrap().reverse();
    assert_eq!(validate(net).unwrap(), first);
    assert_eq!(first.io_projections(), ["net".into()].into());
    assert_eq!(
        first
            .record()
            .io
            .as_ref()
            .unwrap()
            .net
            .as_ref()
            .unwrap()
            .rules[0]
            .operation,
        "connect"
    );
    let empty = validate(json!({"provider": "net.stream@1", "rules": []})).unwrap();
    assert_eq!(empty.io_projections(), ["net".into()].into());
    assert_ne!(first.identity(), empty.identity());
    first
        .check_replay(
            first.canonical(),
            first.identity(),
            &AdmissionEvidence {
                at: 0,
                revocation: None,
            },
        )
        .unwrap();
}

fn script() -> Value {
    let mut net = grant("example.com");
    net["provider"] = "net.script@2".into();
    net["peers"] = json!([{
        "address": "example.com", "port": 443,
        "resolves_to": ["192.0.2.1", "2001:db8::1"],
        "tls": {"certificate": ["example.com"]},
        "connections": ["refused", "unanswered", {"open": [
            {"send": [0, 255]}, {"expect": [71, 69, 84]}, "stall", "close", "reset"
        ]}]
    }]);
    net
}

#[test]
fn peers_are_required_only_for_the_script_provider() {
    validate(script()).unwrap();
    validate(json!({"provider": "net.script@2", "rules": [], "peers": []})).unwrap();
    invalid(
        json!({"provider": "net.script@2", "rules": []}),
        "io.net.peers",
    );
    for provider in ["net.stream@1", "net.other@1", "stdout.bytes@1"] {
        for peers in [json!([]), script()["peers"].clone()] {
            let mut net = grant("example.com");
            net["provider"] = provider.into();
            net["peers"] = peers;
            invalid(net, "io.net.peers");
        }
    }
    for provider in ["net.stream@1", "net.script@2"] {
        let mut net = grant("example.com");
        net["provider"] = provider.into();
        net["peers"] = Value::Null;
        assert!(matches!(validate(net), Err(RunRefusal::Malformed { .. })));
    }
}

#[test]
fn scripted_peer_data_is_closed_and_validated_without_reordering() {
    let mut net = script();
    net["peers"].as_array_mut().unwrap().push(json!({
        "address": "another.example", "port": 80, "resolves_to": [],
        "tls": "plain", "connections": []
    }));
    let first = validate(net.clone()).unwrap();
    assert_eq!(RunConfig::validate(first.canonical()).unwrap(), first);
    net["peers"].as_array_mut().unwrap().reverse();
    assert_ne!(validate(net).unwrap().identity(), first.identity());
    for (key, value) in [
        ("address", json!("Example.com")),
        ("resolves_to", json!(["example.com"])),
        ("resolves_to", json!(["2001:0db8::1"])),
    ] {
        let mut net = script();
        net["peers"][0][key] = value;
        invalid(net, &format!("io.net.peers.{key}"));
    }
    for (key, value) in [
        ("port", json!(65536)),
        ("tls", json!({"insecure": true})),
        ("connections", json!([{"open": [{"send": [256]}]}])),
        ("connections", json!([{"open": [{"send": "text"}]}])),
        ("unknown", json!(true)),
    ] {
        let mut net = script();
        net["peers"][0][key] = value;
        assert!(matches!(validate(net), Err(RunRefusal::Malformed { .. })));
        validate(script()).unwrap();
    }
}

#[test]
fn net_policy_preserves_explicit_authority_and_script_identity() {
    let absent = RunConfig::empty([0; 32], "probe@1/main".into(), LaunchKind::Main)
        .validated()
        .unwrap();
    assert!(absent.net_grant().is_none());
    let run = validate(script()).unwrap();
    assert_eq!(
        run.net_grant(),
        run.record().io.as_ref().unwrap().net.as_ref()
    );
    let mut net = script();
    net["rules"] = json!([]);
    let denied = validate(net.clone()).unwrap();
    assert!(denied.net_grant().unwrap().rules.is_empty());
    assert_eq!(denied.io_projections(), ["net".into()].into());
    net["peers"][0]["connections"][2]["open"][0]["send"][0] = 1.into();
    let edited = validate(net).unwrap();
    assert_ne!(edited.identity(), denied.identity());
    assert!(
        denied
            .check_replay(
                edited.canonical(),
                edited.identity(),
                &AdmissionEvidence {
                    at: 0,
                    revocation: None
                }
            )
            .is_err()
    );
}

#[test]
fn duplicate_peer_endpoints_name_both_entries_and_distinct_ports_are_allowed() {
    let mut net = script();
    let second = net["peers"][0].clone();
    net["peers"].as_array_mut().unwrap().push(second);
    assert!(
        matches!(validate(net.clone()), Err(RunRefusal::Invalid {field, detail})
        if field == "io.net.peers" && detail.contains("entries 0 and 1"))
    );
    net["peers"][1]["port"] = 8443.into();
    validate(net).unwrap();
}

#[test]
fn peer_tape_data_preserves_every_variant_and_order_without_admitting_a_run() {
    let peers = vec![
        NetPeer {
            address: "example.com".into(),
            port: 80,
            resolves_to: vec!["::1".into(), "127.0.0.1".into(), "::1".into()],
            tls: PeerTls::Certificate(vec!["example.com".into(), "alternate.example.com".into()]),
            connections: vec![
                NetConnection::Refused,
                NetConnection::Unanswered,
                NetConnection::Open(vec![
                    NetStep::Send(vec![0, 255]),
                    NetStep::Expect(vec![]),
                    NetStep::Stall,
                    NetStep::Close,
                    NetStep::Reset,
                    NetStep::Truncate,
                ]),
            ],
        },
        NetPeer {
            address: "127.0.0.1".into(),
            port: 443,
            resolves_to: vec![],
            tls: PeerTls::Fails(TlsFailure::AlertReceived(40)),
            connections: vec![],
        },
        NetPeer {
            address: "::1".into(),
            port: 81,
            resolves_to: vec![],
            tls: PeerTls::Plain,
            connections: vec![],
        },
    ];
    assert_eq!(decode_net_peers(&encode_net_peers(&peers)).unwrap(), peers);
    assert_eq!(encode_net_peers(&[]), b"[]");
    assert!(decode_net_peers(b"null").is_err());
    assert!(decode_net_peers(b"[{}]").is_err());
    assert!(decode_net_peers(&[255]).is_err());
    let mut extra: Value = serde_json::from_slice(&encode_net_peers(&peers)).unwrap();
    extra[0]["extra"] = json!(true);
    assert!(decode_net_peers(&serde_json::to_vec(&extra).unwrap()).is_err());
}

#[test]
fn a_standalone_net_grant_is_admitted_exactly_as_the_record_field() {
    let admit = |net: Value| validate_net_grant(&serde_json::to_vec(&net).unwrap());
    // Every grant the record admits is admitted standalone, rules sorted the same way.
    for net in [grant("example.com"), script()] {
        let standalone = admit(net.clone()).unwrap();
        assert_eq!(Some(&standalone), validate(net).unwrap().net_grant());
    }
    // And every refusal is the record field's refusal.
    for net in [
        grant("Example.com"),
        json!({"provider": "net.stream@1", "rules": [], "peers": []}),
        json!({"provider": "net.script@2", "rules": []}),
        json!({"provider": "net.stream@1", "rules": [], "extra": 1}),
    ] {
        let refusal = admit(net.clone()).unwrap_err();
        assert!(
            matches!(refusal, RunRefusal::Invalid { .. }),
            "{net}: {refusal:?}"
        );
        assert!(validate(net).is_err());
    }
    let refusal = admit(json!({"provider": "net.host@9", "rules": []})).unwrap_err();
    assert!(matches!(refusal, RunRefusal::Unsupported { field } if field == "io.net.provider"));
    assert!(admit(json!(null)).is_err());
}

const FAILURES: [(TlsFailure, &str); 10] = [
    (TlsFailure::UnknownIssuer, r#""unknown_issuer""#),
    (TlsFailure::Expired, r#""expired""#),
    (TlsFailure::NotYetValid, r#""not_yet_valid""#),
    (TlsFailure::NameMismatch, r#""name_mismatch""#),
    (TlsFailure::Revoked, r#""revoked""#),
    (TlsFailure::BadCertificate, r#""bad_certificate""#),
    (TlsFailure::AlertReceived(255), r#"{"alert_received":255}"#),
    (TlsFailure::HandshakeFailure, r#""handshake_failure""#),
    (TlsFailure::PeerMisbehaved, r#""peer_misbehaved""#),
    (TlsFailure::Truncated, r#""truncated""#),
];

#[test]
fn every_tls_failure_is_a_failing_peer_and_truncate_is_a_step() {
    for (failure, tag) in FAILURES {
        let mut net = script();
        net["peers"][0]["tls"] = json!({"fails": serde_json::from_str::<Value>(tag).unwrap()});
        net["peers"][0]["connections"][2]["open"]
            .as_array_mut()
            .unwrap()
            .push("truncate".into());
        let run = validate(net).unwrap();
        let peer = &run.net_grant().unwrap().peers.as_ref().unwrap()[0];
        assert_eq!(peer.tls, PeerTls::Fails(failure));
        let NetConnection::Open(steps) = &peer.connections[2] else {
            panic!("open connection")
        };
        assert_eq!(steps.last(), Some(&NetStep::Truncate));
        assert_eq!(RunConfig::validate(run.canonical()).unwrap(), run);
    }
}

#[test]
fn malformed_failing_peers_refuse_at_their_located_tls_field() {
    // serde_json locates each refusal by line and column; the column falls in
    // the peer's `tls` (or step) value, past every well-formed member before it.
    let refuse = |net: Value, member: &str| {
        let bytes = serde_json::to_vec(&record(net)).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        let at = text.find(member).unwrap();
        let Err(RunRefusal::Malformed { detail }) = RunConfig::validate(&bytes) else {
            panic!("{text}: expected a malformed refusal")
        };
        let column: usize = detail
            .rsplit_once(" at line 1 column ")
            .unwrap_or_else(|| panic!("{detail}"))
            .1
            .parse()
            .unwrap();
        assert!(column > at, "{detail} precedes {member} at {at}");
    };
    for tls in [
        json!("fails"),
        json!({"fails": null}),
        json!({"fails": "unknown"}),
        json!({"fails": "UnknownIssuer"}),
        json!({"fails": ["unknown_issuer"]}),
        json!({"fails": "alert_received"}),
        json!({"fails": {"alert_received": 256}}),
        json!({"fails": {"alert_received": -1}}),
        json!({"fails": {"expired": 1}}),
        json!({"fails": "expired", "plain": null}),
        json!({"fail": "expired"}),
    ] {
        let mut net = script();
        net["peers"][0]["tls"] = tls;
        refuse(net, r#""tls":"#);
        // The accepted twin differs only in a well-formed failure.
        let mut net = script();
        net["peers"][0]["tls"] = json!({"fails": "expired"});
        validate(net).unwrap();
    }
    for step in [
        json!("truncated"),
        json!({"truncate": []}),
        json!("Truncate"),
    ] {
        let mut net = script();
        net["peers"][0]["connections"][2]["open"][0] = step;
        refuse(net, r#""connections":"#);
    }
}

#[test]
fn a_net_script_1_document_refuses_as_an_unknown_version_and_its_twin_admits() {
    // net.script@2 replaced net.script@1 when PeerTls gained Fails and NetStep
    // gained Truncate. As for every provider other than the script contract, a
    // record's peers refuse first; a standalone grant names the provider.
    let mut old = script();
    old["provider"] = "net.script@1".into();
    assert!(
        matches!(validate(old.clone()), Err(RunRefusal::Invalid { field, .. }) if field == "io.net.peers")
    );
    assert_eq!(
        validate_net_grant(&serde_json::to_vec(&old).unwrap()),
        Err(RunRefusal::Unsupported {
            field: "io.net.provider".into()
        })
    );
    old.as_object_mut().unwrap().remove("peers");
    assert_eq!(
        validate(old),
        Err(RunRefusal::Unsupported {
            field: "io.net.provider".into()
        })
    );
    validate(script()).unwrap();
    validate_net_grant(&serde_json::to_vec(&script()).unwrap()).unwrap();
}

#[test]
fn scripted_listeners_are_script_only_canonical_and_round_trip() {
    let listener = json!({"address": "127.0.0.1", "port": 8080, "clients": [
        {"address": "192.0.2.1", "port": 40001, "tls": "plain", "steps": [{"send": [71]}, {"expect": [72]}, "close"]}
    ]});
    let mut net = script();
    net["listeners"] = json!([listener]);
    validate(net.clone()).unwrap();
    let mut stream = grant("example.com");
    stream["listeners"] = json!([listener]);
    invalid(stream, "io.net.listeners");
    for (field, value) in [
        ("address", json!("localhost")),
        ("address", json!("::0001")),
    ] {
        let mut bad = listener.clone();
        bad[field] = value;
        let mut net = script();
        net["listeners"] = json!([bad]);
        invalid(net, "io.net.listeners");
    }
    let mut ephemeral = listener.clone();
    ephemeral["port"] = json!(0);
    let mut ephemeral_net = script();
    ephemeral_net["listeners"] = json!([ephemeral]);
    validate(ephemeral_net).unwrap();
    let mut client = listener.clone();
    client["clients"][0]["address"] = json!("192.000.2.1");
    let mut net = script();
    net["listeners"] = json!([client]);
    invalid(net, "io.net.listeners");
    let mut twice = script();
    twice["listeners"] = json!([listener, listener]);
    invalid(twice, "io.net.listeners");
    let listeners: Vec<NetListener> = serde_json::from_value(json!([listener])).unwrap();
    assert_eq!(
        decode_net_listeners(&encode_net_listeners(&listeners)).unwrap(),
        listeners
    );
}

#[test]
fn unix_rules_name_canonical_socket_paths_with_service_zero() {
    let unix = |address: &str, service: &str, operation: &str| {
        json!({"provider": "net.stream@1", "rules": [
            {"address": address, "service": service, "operation": operation}
        ]})
    };
    let longest = format!("/{}", "s".repeat(UNIX_PATH_MAX - 1));
    for operation in UNIX_OPERATIONS {
        for path in ["/run/docker.sock", "/tmp/a b.sock", longest.as_str()] {
            validate(unix(path, "0", operation)).unwrap();
        }
        for path in [
            "",
            "/",
            "relative.sock",
            "/run/",
            "/run//docker.sock",
            "/run/./docker.sock",
            "/run/../docker.sock",
            "/run\\docker.sock",
            "/run/\0docker",
            "example.com",
            "127.0.0.1",
            &format!("{longest}s"),
        ] {
            invalid(unix(path, "0", operation), "io.net.rules.address");
        }
        invalid(
            unix("/run/docker.sock", "80", operation),
            "io.net.rules.service",
        );
    }
    // A path never names a TCP or UDP rule, nor a host a Unix one.
    for operation in ["connect", "connect_tls", "listen", "udp_bind", "udp_peer"] {
        invalid(
            unix("/run/docker.sock", "0", operation),
            "io.net.rules.address",
        );
    }
}

#[test]
fn unix_script_peers_are_port_zero_paths() {
    let script = |peer: Value| json!({"provider": "net.script@2", "rules": [], "peers": [peer]});
    let peer = |port: u16, resolves_to: Value, tls: Value| {
        json!({"address": "/run/app.sock", "port": port, "resolves_to": resolves_to,
               "tls": tls, "connections": []})
    };
    validate(script(peer(0, json!([]), json!("plain")))).unwrap();
    validate(script(peer(
        0,
        json!([]),
        json!({"certificate": ["localhost"]}),
    )))
    .unwrap();
    invalid(
        script(peer(80, json!([]), json!("plain"))),
        "io.net.peers.address",
    );
    invalid(
        script(peer(0, json!(["127.0.0.1"]), json!("plain"))),
        "io.net.peers.address",
    );
    let mut relative = peer(0, json!([]), json!("plain"));
    relative["address"] = "run/app.sock".into();
    invalid(script(relative), "io.net.peers.address");
}
