use hirsel_run_config::*;
use std::collections::BTreeMap;

fn empty() -> RunConfig {
    RunConfig::empty([0; 32], "probe@1/main".into(), LaunchKind::Main)
}
fn bytes(record: &RunConfig) -> Vec<u8> {
    serde_json::to_vec(record).unwrap()
}
fn valid(record: &RunConfig) -> ValidatedRun {
    RunConfig::validate(&bytes(record)).unwrap()
}
fn refused(record: &RunConfig) {
    assert!(RunConfig::validate(&bytes(record)).is_err());
}

#[test]
fn empty_record_has_fixed_bytes_and_an_independent_sha256_vector() {
    let run = valid(&empty());
    let golden = concat!(
        "{\"schema\":1,\"program\":\"0000000000000000000000000000000000000000000000000000000000000000\",",
        "\"entry\":\"probe@1/main\",\"launch\":\"main\",\"inputs\":{\"hash_seed\":0,\"random_seed\":0,\"test_seed\":0,\"scheduler\":null},",
        "\"io\":null,\"args\":null,\"env\":null,\"unsafe\":false,\"admission\":null}"
    );
    assert_eq!(run.canonical(), golden.as_bytes());
    // hashlib.sha256 over independently framed domain + golden bytes.
    assert_eq!(
        hex(&run.identity()),
        "1bdcf82b92a53703563521cf395bb2aa79764e953eaf8fb74b85f703a43dbaf4"
    );
    let config = run.config();
    assert_eq!(
        (config.hash_seed(), config.random_seed(), config.test_seed()),
        (0, 0, 0)
    );
    assert_eq!(config.run_identity(), run.identity());
    assert_eq!(std::mem::size_of::<Config>(), 56);
}
#[test]
fn human_key_order_whitespace_and_escape_spelling_normalize() {
    let mut record = empty();
    record.args = Some(vec!["quote\" slash\\ \n\t\r\u{08}\u{0c}\0 café 😀".into()]);
    let run = valid(&record);
    let value: serde_json::Value = serde_json::from_slice(&bytes(&record)).unwrap();
    let pretty = serde_json::to_vec_pretty(&value).unwrap();
    assert_eq!(RunConfig::validate(&pretty).unwrap(), run);
    let text = std::str::from_utf8(run.canonical()).unwrap();
    for escape in [
        "\\u000a", "\\u0009", "\\u000d", "\\u0008", "\\u000c", "\\u0000",
    ] {
        assert!(text.contains(escape));
    }
    assert!(text.contains("café 😀"));
    assert!(!text.contains("\\n"));
    assert_eq!(RunConfig::validate(run.canonical()).unwrap(), run);
}
#[test]
fn malformed_numbers_unknown_and_duplicate_fields_refuse_with_valid_twins() {
    let base = String::from_utf8(bytes(&empty())).unwrap();
    for text in [
        base.replace("\"schema\":1", "\"schema\":1,\"schema\":1"),
        base.replace("\"schema\":1", "\"schema\":1,\"validated\":true"),
        base.replace("\"hash_seed\":0", "\"hash_seed\":0,\"hash_seed\":0"),
        base.replace("\"hash_seed\":0", "\"hash_seed\":01"),
        base.replace("\"hash_seed\":0", "\"hash_seed\":-0"),
        base.replace("\"hash_seed\":0", "\"hash_seed\":1.0"),
        base.replace("\"hash_seed\":0", "\"hash_seed\":1e0"),
        base.replace("\"hash_seed\":0", "\"hash_seed\":18446744073709551616"),
        format!("{base} true"),
    ] {
        assert!(
            matches!(
                RunConfig::validate(text.as_bytes()),
                Err(RunRefusal::Malformed { .. })
            ),
            "{text}"
        );
    }
    let mut record = empty();
    record.inputs.hash_seed = u64::MAX;
    record.admission = Some(Admission {
        not_before: Some(i64::MIN),
        expires: Some(i64::MAX),
        revocation: None,
    });
    valid(&record);
    let duplicate = base.replace("\"env\":null", "\"env\":{\"K\":null,\"K\":\"v\"}");
    assert!(RunConfig::validate(duplicate.as_bytes()).is_err());
    record.env = Some(BTreeMap::from([("K".into(), None)]));
    valid(&record);
    assert!(RunConfig::validate(b"\xff").is_err());
}
#[test]
fn absent_roots_are_denied_and_empty_snapshots_are_granted() {
    let record = empty();
    for root in [Root::Io, Root::Args, Root::Env, Root::Unsafe, Root::Test] {
        assert!(matches!(
            valid(&record).require_roots(&[root]),
            Err(RunRefusal::NotGranted { .. })
        ));
    }
    valid(&record).require_roots(&[]).unwrap();
    let mut record = record;
    record.io = Some(IoGrants::default());
    record.args = Some(vec![]);
    record.env = Some(BTreeMap::new());
    record.unsafe_root = true;
    record.launch = LaunchKind::Test;
    let run = valid(&record);
    run.require_roots(&[Root::Io, Root::Args, Root::Env, Root::Unsafe, Root::Test])
        .unwrap();
    assert_ne!(run.identity(), valid(&empty()).identity());
}
#[test]
fn grant_sets_sort_but_ordered_args_and_unicode_are_not_rewritten() {
    let mut record = empty();
    let io = IoGrants {
        dirs: vec![
            Mount {
                path: "z".into(),
                host: "/host/z".into(),
                access: Access::Read,
            },
            Mount {
                path: "a".into(),
                host: "/host/a".into(),
                access: Access::ReadWrite,
            },
        ],
        ..IoGrants::default()
    };
    record.io = Some(io);
    record.env = Some(BTreeMap::from([
        ("SET".into(), Some("é".into())),
        ("UNSET".into(), None),
    ]));
    record.args = Some(vec!["a".into(), "b".into()]);
    let a = valid(&record);
    record.io.as_mut().unwrap().dirs.reverse();
    assert_eq!(valid(&record), a);
    record.args.as_mut().unwrap().reverse();
    assert_ne!(valid(&record).identity(), a.identity());
    record
        .env
        .as_mut()
        .unwrap()
        .insert("SET".into(), Some("e\u{301}".into()));
    assert!(
        std::str::from_utf8(valid(&record).canonical())
            .unwrap()
            .contains("e\u{301}")
    );
}
#[test]
fn overlapping_mounts_traversal_and_wildcards_have_controls() {
    for (a, b) in [("", "a"), ("a", "a"), ("a", "a/x"), ("a", "a/x/y")] {
        let mut record = empty();
        record.io = Some(IoGrants {
            dirs: vec![
                Mount {
                    path: a.into(),
                    host: "/tmp/a".into(),
                    access: Access::Read,
                },
                Mount {
                    path: "a-foo".into(),
                    host: "/tmp/other".into(),
                    access: Access::Read,
                },
                Mount {
                    path: b.into(),
                    host: "/tmp/b".into(),
                    access: Access::Read,
                },
            ],
            ..IoGrants::default()
        });
        refused(&record);
    }
    let mut record = empty();
    record.io = Some(IoGrants::default());
    for path in ["a/../b", "a/./b", "a//b", "/a", "a/", "a\\b", "a\0b"] {
        record.io.as_mut().unwrap().dirs = vec![Mount {
            path: path.into(),
            host: "/tmp".into(),
            access: Access::Read,
        }];
        refused(&record);
    }
    for host in [".", "relative", "/tmp/..", "/tmp/", "/tmp//x", "/tmp\\x"] {
        record.io.as_mut().unwrap().dirs = vec![Mount {
            path: "a".into(),
            host: host.into(),
            access: Access::Read,
        }];
        refused(&record);
    }
    record.io.as_mut().unwrap().dirs = vec![
        Mount {
            path: "a".into(),
            host: "/same".into(),
            access: Access::Read,
        },
        Mount {
            path: "a-foo".into(),
            host: "/same".into(),
            access: Access::ReadWrite,
        },
    ];
    valid(&record); // Distinct virtual mounts may name the same physical host resource.
    let mut net = NetGrant {
        provider: "p".into(),
        peers: None,
        rules: vec![NetRule {
            address: "*".into(),
            service: "80".into(),
            operation: "connect".into(),
        }],
    };
    record.io.as_mut().unwrap().net = Some(net.clone());
    refused(&record);
    net.rules[0].address = "127.0.0.1".into();
    record.io.as_mut().unwrap().net = Some(net.clone());
    assert_eq!(
        record.validated(),
        Err(RunRefusal::Unsupported {
            field: "io.net.provider".into()
        })
    );
    record.io.as_mut().unwrap().net = None;
    valid(&record); // The supported twin grants no unenforceable network policy.
    net.rules.push(net.rules[0].clone());
    record.io.as_mut().unwrap().net = Some(net);
    refused(&record);
}
#[test]
fn every_semantic_binding_changes_identity_and_refuses_replay() {
    let run = valid(&empty());
    let evidence = AdmissionEvidence {
        at: 50,
        revocation: None,
    };
    run.check_replay(run.canonical(), run.identity(), &evidence)
        .unwrap();
    for mutation in 0..10 {
        let mut record = empty();
        match mutation {
            0 => record.program = hex(&[1; 32]),
            1 => record.entry = "probe@1/other".into(),
            2 => record.launch = LaunchKind::Test,
            3 => record.inputs.hash_seed = 1,
            4 => record.inputs.random_seed = 1,
            5 => record.inputs.test_seed = 1,
            6 => record.io = Some(IoGrants::default()),
            7 => record.args = Some(vec![]),
            8 => record.env = Some(BTreeMap::new()),
            _ => record.unsafe_root = true,
        }
        assert!(matches!(
            valid(&record).check_replay(run.canonical(), run.identity(), &evidence),
            Err(RunRefusal::Binding { .. })
        ));
    }
    assert!(
        run.check_replay(&bytes(&empty()), [7; 32], &evidence)
            .is_err()
    );
    let pretty = serde_json::to_vec_pretty(&empty()).unwrap();
    assert!(
        run.check_replay(&pretty, run.identity(), &evidence)
            .is_err()
    );
    run.check_program([0; 32], "probe@1/main", LaunchKind::Main)
        .unwrap();
    for (program, entry, launch, field) in [
        ([1; 32], "probe@1/main", LaunchKind::Main, "program"),
        ([0; 32], "probe@1/other", LaunchKind::Main, "entry"),
        ([0; 32], "probe@1/main", LaunchKind::Test, "launch"),
    ] {
        assert_eq!(
            run.check_program(program, entry, launch),
            Err(RunRefusal::Binding {
                field: field.into()
            })
        );
    }
}
struct Host {
    calls: u32,
    revoked: bool,
    deny: bool,
}
impl HostAuthority for Host {
    fn authorize(&mut self, _: &RunConfig) -> Result<(), RunRefusal> {
        self.calls += 1;
        if self.deny {
            Err(RunRefusal::Unauthorized { field: "io".into() })
        } else {
            Ok(())
        }
    }
    fn revocation(&mut self, reference: &str) -> Result<RevocationEvidence, RunRefusal> {
        self.calls += 1;
        Ok(RevocationEvidence {
            reference: reference.into(),
            revision: "list@4".into(),
            revoked: self.revoked,
        })
    }
}
#[test]
fn expiry_revocation_and_host_permissions_are_admission_only() {
    let mut record = empty();
    record.admission = Some(Admission {
        not_before: Some(10),
        expires: Some(20),
        revocation: Some("r".into()),
    });
    let run = valid(&record);
    let mut host = Host {
        calls: 0,
        revoked: false,
        deny: false,
    };
    assert!(matches!(
        run.admit(&mut host, 9),
        Err(RunRefusal::NotYetValid { .. })
    ));
    assert!(matches!(
        run.admit(&mut host, 21),
        Err(RunRefusal::Expired { .. })
    ));
    assert_eq!(host.calls, 0);
    let accepted = run.admit(&mut host, 20).unwrap();
    assert_eq!(host.calls, 2);
    host.revoked = true;
    host.deny = true;
    // Historical admission supplies its own time/evidence; no current host reads.
    accepted
        .run()
        .check_replay(run.canonical(), run.identity(), accepted.evidence())
        .unwrap();
    assert_eq!(host.calls, 2);
    assert!(matches!(
        run.admit(&mut host, 21),
        Err(RunRefusal::Expired { .. })
    ));
    assert!(matches!(
        run.admit(&mut host, 20),
        Err(RunRefusal::Revoked { .. })
    ));
    host.revoked = false;
    assert!(matches!(
        run.admit(&mut host, 10),
        Err(RunRefusal::Unauthorized { .. })
    ));
    let no_proof = AdmissionEvidence {
        at: 20,
        revocation: None,
    };
    assert!(matches!(
        run.check_replay(run.canonical(), run.identity(), &no_proof),
        Err(RunRefusal::MissingEvidence { .. })
    ));
    let mut malformed = accepted.evidence().clone();
    malformed.at = 21;
    assert!(matches!(
        run.check_replay(run.canonical(), run.identity(), &malformed),
        Err(RunRefusal::Expired { .. })
    ));
}
#[test]
fn invalid_binding_and_env_names_are_structured_refusals() {
    for entry in [
        "main",
        "probe/main",
        "probe@01/main",
        "probe@1/",
        "probe@1/a//main",
        "probe@18446744073709551616/main",
    ] {
        let mut record = empty();
        record.entry = entry.into();
        refused(&record);
    }
    for program in ["FF".repeat(32), "0".repeat(63), "g".repeat(64)] {
        let mut record = empty();
        record.program = program;
        refused(&record);
    }
    let mut record = empty();
    record.schema = 2;
    assert_eq!(
        RunConfig::validate(&bytes(&record)),
        Err(RunRefusal::Schema { found: 2 })
    );
    for key in ["", "A=B", "A\0B"] {
        record = empty();
        record.env = Some(BTreeMap::from([(key.into(), None)]));
        refused(&record);
    }
    valid(&empty());
    let mut record = empty();
    record.entry = "probe@18446744073709551615/main".into();
    valid(&record);
}

#[test]
fn explicit_snapshot_helpers_preserve_order_keys_and_unset_values() {
    use std::ffi::OsString;
    let mut record = empty();
    record.args = Some(snapshot_args([OsString::from("second"), OsString::from("first")]).unwrap());
    record.env = Some(
        snapshot_env([
            (OsString::from("SET"), Some(OsString::from("é"))),
            (OsString::from("UNSET"), None),
        ])
        .unwrap(),
    );
    let run = valid(&record);
    run.require_roots(&[Root::Args, Root::Env]).unwrap();
    assert_eq!(run.record().args.as_ref().unwrap(), &["second", "first"]);
    assert_eq!(run.record().env.as_ref().unwrap().get("UNSET"), Some(&None));
    assert_eq!(run.record().env.as_ref().unwrap().get("DENIED"), None);
    assert!(snapshot_env([(OsString::from("K"), None), (OsString::from("K"), None)]).is_err());
    assert!(snapshot_env([(OsString::from("A=B"), None)]).is_err());
    assert!(snapshot_args([]).unwrap().is_empty());
    assert!(snapshot_env([]).unwrap().is_empty());
}
#[cfg(unix)]
#[test]
fn byte_invalid_snapshot_inputs_refuse_with_the_item_and_utf8_twins_pass() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    assert_eq!(
        snapshot_args([OsString::from("ok"), OsString::from_vec(vec![0xff])]),
        Err(RunRefusal::InvalidInput {
            item: "args[1]".into()
        })
    );
    assert_eq!(
        snapshot_env([(OsString::from_vec(vec![0xff]), None)]),
        Err(RunRefusal::InvalidInput {
            item: "env.key[0]".into()
        })
    );
    assert_eq!(
        snapshot_env([(OsString::from("KEY"), Some(OsString::from_vec(vec![0xff])))]),
        Err(RunRefusal::InvalidInput {
            item: "env[\"KEY\"]".into()
        })
    );
    snapshot_args([OsString::from("ok"), OsString::from("é")]).unwrap();
    snapshot_env([
        (OsString::from("KEY"), Some(OsString::from("é"))),
        (OsString::from("MISSING"), None),
    ])
    .unwrap();
}

#[test]
fn known_provider_contracts_are_shared_by_live_and_replayed_runs() {
    let mut record = empty();
    record.io = Some(IoGrants {
        stdout: Some(ProviderContract::StdoutBytes.name().into()),
        random: Some(ProviderContract::RandomSplitMix64.name().into()),
        monotonic_clock: Some(ProviderContract::MonotonicNanoseconds.name().into()),
        wall_clock: Some(ProviderContract::WallUnixNanoseconds.name().into()),
        ..IoGrants::default()
    });
    let run = valid(&record);
    run.check_replay(
        run.canonical(),
        run.identity(),
        &AdmissionEvidence {
            at: 0,
            revocation: None,
        },
    )
    .unwrap();
    for contract in ProviderContract::ALL {
        assert_eq!(ProviderContract::parse(contract.name()), Some(contract));
    }
    // External revocation references are host evidence selectors, not providers.
    record.admission = Some(Admission {
        not_before: None,
        expires: None,
        revocation: Some("owner-list@7".into()),
    });
    valid(&record);
}

#[test]
fn unknown_versions_and_wrong_projection_contracts_refuse_with_supported_twins() {
    for (field, contract) in [
        ("io.stdout", ProviderContract::StdoutBytes),
        ("io.random", ProviderContract::RandomSplitMix64),
        ("io.monotonic_clock", ProviderContract::MonotonicNanoseconds),
        ("io.wall_clock", ProviderContract::WallUnixNanoseconds),
    ] {
        for name in [
            "unknown@1".into(),
            contract.name().replace("@1", "@2"),
            format!(" {}", contract.name()),
            "linux.stdout@1".into(),
            "sim.stdout@1".into(),
        ] {
            let mut record = empty();
            let mut io = IoGrants::default();
            let slot = match field {
                "io.stdout" => &mut io.stdout,
                "io.random" => &mut io.random,
                "io.monotonic_clock" => &mut io.monotonic_clock,
                "io.wall_clock" => &mut io.wall_clock,
                _ => unreachable!(),
            };
            *slot = Some(name);
            record.io = Some(io.clone());
            assert_eq!(
                record.validated(),
                Err(RunRefusal::Unsupported {
                    field: field.into()
                })
            );
            let slot = match field {
                "io.stdout" => &mut io.stdout,
                "io.random" => &mut io.random,
                "io.monotonic_clock" => &mut io.monotonic_clock,
                "io.wall_clock" => &mut io.wall_clock,
                _ => unreachable!(),
            };
            *slot = Some(contract.name().into());
            record.io = Some(io);
            valid(&record);
        }
    }
    let mut record = empty();
    record.io = Some(IoGrants {
        stdout: Some(ProviderContract::MonotonicNanoseconds.name().into()),
        ..IoGrants::default()
    });
    assert_eq!(
        record.validated(),
        Err(RunRefusal::Unsupported {
            field: "io.stdout".into()
        })
    );
    record.io.as_mut().unwrap().stdout = Some(ProviderContract::StdoutBytes.name().into());
    valid(&record);
}

#[test]
fn unenforceable_tasks_net_and_scheduler_policy_refuses() {
    for field in ["io.tasks", "io.net.provider", "inputs.scheduler.algorithm"] {
        let mut record = empty();
        record.io = Some(IoGrants::default());
        match field {
            "io.tasks" => record.io.as_mut().unwrap().tasks = Some("tasks@1".into()),
            "io.net.provider" => {
                record.io.as_mut().unwrap().net = Some(NetGrant {
                    provider: "net@1".into(),
                    peers: None,
                    rules: vec![NetRule {
                        address: "127.0.0.1".into(),
                        service: "80".into(),
                        operation: "connect".into(),
                    }],
                })
            }
            _ => {
                record.inputs.scheduler = Some(Scheduler {
                    algorithm: "scheduler@1".into(),
                    version: 1,
                    seed: 0,
                })
            }
        }
        assert_eq!(
            record.validated(),
            Err(RunRefusal::Unsupported {
                field: field.into()
            })
        );
        record.io = Some(IoGrants::default());
        record.inputs.scheduler = None;
        valid(&record);
    }
}

#[test]
fn adapter_policy_comes_only_from_explicit_validated_grants() {
    let mut record = empty();
    let denied = valid(&record);
    assert!(denied.io_projections().is_empty());
    assert_eq!(denied.clock_grants(), (false, false));
    record.io = Some(IoGrants::default());
    assert!(valid(&record).io_projections().is_empty());
    let io = record.io.as_mut().unwrap();
    io.stdout = Some(ProviderContract::StdoutBytes.name().into());
    io.monotonic_clock = Some(ProviderContract::MonotonicNanoseconds.name().into());
    assert_eq!(valid(&record).io_projections(), ["stdout".into()].into());
    assert_eq!(valid(&record).clock_grants(), (true, false));
}

#[test]
fn relative_record_paths_are_refused_before_reading() {
    assert!(
        matches!(RunConfig::read_path(std::path::Path::new("run.json")), Err(RunRefusal::Invalid { field, .. }) if field == "--hirsel-run")
    );
}
