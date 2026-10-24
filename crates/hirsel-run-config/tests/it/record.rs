use hirsel_run_config::*;
use hirsel_run_schema::*;
use hirsel_run_schema::*;
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
/// The run bound to the code it names, with no roots.
fn bound(run: &ValidatedRun) -> BoundRun {
    let facts = ProgramFacts {
        program: run.program(),
        entry: run.record().entry.clone(),
        launch: run.record().launch,
        roots: vec![],
        code: CodeReach::Safe,
        fueled: true,
    };
    run.clone().bind(&facts).unwrap()
}
fn refused(record: &RunConfig) {
    assert!(RunConfig::validate(&bytes(record)).is_err());
}

#[test]
fn empty_record_has_fixed_bytes_and_an_independent_sha256_vector() {
    let run = valid(&empty());
    let golden = concat!(
        "{\"schema\":1,\"depth_semantics\":\"hirsel.depth.invocations.v1\",\"program\":\"0000000000000000000000000000000000000000000000000000000000000000\",",
        "\"entry\":\"probe@1/main\",\"launch\":\"main\",\"inputs\":{\"hash_seed\":0,\"random_seed\":0,\"test_seed\":0,\"scheduler\":null},",
        "\"io\":null,\"args\":null,\"env\":null,\"unsafe\":false,\"admission\":null}"
    );
    assert_eq!(run.canonical(), golden.as_bytes());
    // hashlib.sha256 over independently framed domain + golden bytes.
    assert_eq!(
        hex(&run.identity()),
        "aa85228e50999b368b090e9ed2c17093c8cce79d929f3d5437707df81e992c65"
    );
    let config = run.config();
    assert_eq!(
        (config.hash_seed(), config.random_seed(), config.test_seed()),
        (0, 0, 0)
    );
    assert_eq!(config.scheduler(), (0, 0));
    assert_eq!(config.run_identity(), run.identity());
    assert_eq!(std::mem::size_of::<Config>(), 72);
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
        code: None,
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
        listeners: vec![],
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
    fn authorize(&mut self, _: &ValidatedRun) -> Result<(), RunRefusal> {
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
fn safe_code_policy_refuses_an_escaping_program_at_bind() {
    let path: Vec<String> = vec!["app@1/main".into(), "app@1/demo_add".into()];
    let policy = |code: Option<&str>| {
        let mut record = empty();
        record.admission = Some(Admission {
            not_before: None,
            expires: None,
            revocation: None,
            code: code.map(Into::into),
        });
        record
    };
    let bind = |run: &ValidatedRun, code: CodeReach| {
        run.clone().bind(&ProgramFacts {
            program: run.program(),
            entry: run.record().entry.clone(),
            launch: run.record().launch,
            roots: vec![],
            code,
            fueled: true,
        })
    };
    let safe = valid(&policy(Some("admit.safe@1")));
    assert_eq!(
        bind(&safe, CodeReach::Escapes(path.clone())).unwrap_err(),
        RunRefusal::UnsafeCode { path: path.clone() }
    );
    assert!(bind(&safe, CodeReach::Safe).is_ok());
    for run in [
        valid(&policy(None)),
        valid(&policy(Some("admit.any@1"))),
        valid(&empty()),
    ] {
        assert!(bind(&run, CodeReach::Escapes(path.clone())).is_ok());
    }
    assert_eq!(
        policy(Some("admit.none@1")).validated().unwrap_err(),
        RunRefusal::Unsupported {
            field: "admission.code".into()
        }
    );
    // The default policy adds no member, so earlier records keep their identity.
    let absent = valid(&policy(None));
    assert!(!String::from_utf8_lossy(absent.canonical()).contains("code"));
    assert_eq!(
        valid(&policy(Some("admit.any@1"))).identity(),
        absent.identity()
    );
    assert!(String::from_utf8_lossy(safe.canonical()).contains(r#""code":"admit.safe@1""#));
    assert_ne!(absent.identity(), safe.identity());
}
#[test]
fn expiry_revocation_and_host_permissions_are_admission_only() {
    let mut record = empty();
    record.admission = Some(Admission {
        not_before: Some(10),
        expires: Some(20),
        revocation: Some("r".into()),
        code: None,
    });
    let run = valid(&record);
    let mut host = Host {
        calls: 0,
        revoked: false,
        deny: false,
    };
    assert!(matches!(
        bound(&run).admit(&mut host, 9),
        Err(RunRefusal::NotYetValid { .. })
    ));
    assert!(matches!(
        bound(&run).admit(&mut host, 21),
        Err(RunRefusal::Expired { .. })
    ));
    assert_eq!(host.calls, 0);
    let accepted = bound(&run).admit(&mut host, 20).unwrap();
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
        bound(&run).admit(&mut host, 21),
        Err(RunRefusal::Expired { .. })
    ));
    assert!(matches!(
        bound(&run).admit(&mut host, 20),
        Err(RunRefusal::Revoked { .. })
    ));
    host.revoked = false;
    assert!(matches!(
        bound(&run).admit(&mut host, 10),
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
        stdin: Some(ProviderContract::StdinBytes.name().into()),
        stderr: Some(ProviderContract::StderrBytes.name().into()),
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
    assert!(
        ProviderContract::ALL
            .windows(2)
            .all(|pair| pair[0].name() < pair[1].name()),
        "provider contracts must be sorted and unique by name"
    );
    for &contract in ProviderContract::ALL {
        assert_eq!(ProviderContract::parse(contract.name()), Some(contract));
    }
    // External revocation references are host evidence selectors, not providers.
    record.admission = Some(Admission {
        not_before: None,
        expires: None,
        revocation: Some("owner-list@7".into()),
        code: None,
    });
    valid(&record);
}

#[test]
fn unknown_versions_and_wrong_projection_contracts_refuse_with_supported_twins() {
    for (field, contract) in [
        ("io.tasks", ProviderContract::TasksCooperative),
        ("io.stdin", ProviderContract::StdinBytes),
        ("io.stderr", ProviderContract::StderrBytes),
        ("io.stdout", ProviderContract::StdoutBytes),
        ("io.random", ProviderContract::RandomSplitMix64),
        ("io.random", ProviderContract::RandomSystem),
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
                "io.tasks" => &mut io.tasks,
                "io.stdin" => &mut io.stdin,
                "io.stderr" => &mut io.stderr,
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
                "io.tasks" => &mut io.tasks,
                "io.stdin" => &mut io.stdin,
                "io.stderr" => &mut io.stderr,
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
                    listeners: vec![],
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
fn scheduler_contracts_validate_and_project_their_word_and_seed() {
    assert_eq!(valid(&empty()).config().scheduler(), (0, 0));
    for (algorithm, word) in [("fifo", 0), ("random", 1), ("pct", 2)] {
        let mut record = empty();
        record.inputs.scheduler = Some(Scheduler {
            algorithm: algorithm.into(),
            version: 1,
            seed: 7,
        });
        assert_eq!(valid(&record).config().scheduler(), (word, 7));
        record.inputs.scheduler.as_mut().unwrap().version = 2;
        assert_eq!(
            record.validated(),
            Err(RunRefusal::Unsupported {
                field: "inputs.scheduler.algorithm".into()
            })
        );
    }
    let parsed = Scheduler::parse("pct@1:42").unwrap();
    assert_eq!(
        (parsed.algorithm.as_str(), parsed.version, parsed.seed),
        ("pct", 1, 42)
    );
    assert_eq!(parsed.to_string(), "pct@1:42");
    for text in [
        "pct@2:1",
        "pct@1:01",
        "pct@01:1",
        "pct@1",
        "pct:1",
        "chaos@1:1",
        "random@1:-1",
        "random@1:18446744073709551616",
    ] {
        assert_eq!(Scheduler::parse(text), None, "{text}");
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

#[test]
fn grants_templates_bind_only_selected_code_and_use_record_validation() {
    let bytes = br#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":["x"],"env":{"KEY":null},"inputs":{"hash_seed":7,"random_seed":8,"test_seed":9}}"#;
    let run =
        RunConfig::bind_grants(bytes, [1; 32], "probe@1/main".into(), LaunchKind::Main).unwrap();
    run.check_program([1; 32], "probe@1/main", LaunchKind::Main)
        .unwrap();
    assert!(
        run.check_program([2; 32], "probe@1/main", LaunchKind::Main)
            .is_err()
    );
    assert_eq!(run.record().inputs.hash_seed, 7);
    assert_eq!(run.record().args, Some(vec!["x".into()]));
    assert_eq!(RunConfig::validate(run.canonical()).unwrap(), run);
    let denied =
        RunConfig::bind_grants(b"{}", [1; 32], "probe@1/main".into(), LaunchKind::Main).unwrap();
    assert!(denied.record().io.is_none());
    assert!(denied.record().args.is_none());
    assert!(denied.record().env.is_none());
    assert!(!denied.record().unsafe_root);
    for bad in [
        r#"{"schema":1}"#,
        r#"{"program":"ignored"}"#,
        r#"{"entry":"probe@1/other"}"#,
        r#"{"launch":"test"}"#,
        r#"{"args":[],"args":["x"]}"#,
        r#"{"env":{"K":null,"K":"x"}}"#,
        r#"{"io":{"dirs":[],"stdout":"unknown@1"}}"#,
        r#"{"inputs":{"hash_seed":1.0,"random_seed":0,"test_seed":0}}"#,
        r#"{"admission":{"not_before":9,"expires":1}}"#,
        r#"{"unknown":true}"#,
    ] {
        assert!(
            RunConfig::bind_grants(
                bad.as_bytes(),
                [1; 32],
                "probe@1/main".into(),
                LaunchKind::Main
            )
            .is_err(),
            "{bad}"
        );
    }
}

#[test]
fn revocation_source_is_closed_and_hashes_the_exact_snapshot() {
    let bytes = br#"{"schema":1,"revoked":["grant-A"]}"#;
    let source = Revocations::validate(bytes).unwrap();
    assert!(source.evidence("grant-A").revoked);
    assert!(!source.evidence("grant-B").revoked);
    let revision = source.evidence("grant-A").revision;
    assert_eq!(revision, source.evidence("grant-B").revision);
    assert_ne!(
        revision,
        Revocations::validate(b"{\"schema\":1,\"revoked\":[\"grant-A\"]}\n")
            .unwrap()
            .evidence("grant-A")
            .revision
    );
    for invalid in [
        r#"{"schema":2,"revoked":[]}"#,
        r#"{"schema":1,"revoked":[""]}"#,
        r#"{"schema":1,"revoked":[],"unknown":true}"#,
        r#"{"schema":1,"revoked":[],"revoked":["grant-A"]}"#,
        r#"{"schema":1}"#,
    ] {
        assert!(
            Revocations::validate(invalid.as_bytes()).is_err(),
            "{invalid}"
        );
    }
    assert!(
        !Revocations::validate(br#"{"schema":1,"revoked":[]}"#)
            .unwrap()
            .evidence("grant-A")
            .revoked
    );
}

#[test]
fn relative_grant_hosts_resolve_beside_the_template_and_records_remain_strict() {
    let root = std::env::temp_dir().join(format!("hirsel-relative-grants-{}", std::process::id()));
    std::fs::create_dir_all(root.join("templates")).unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("templates/grants.json");
    let template = br#"{"io":{"dirs":[{"path":"data","host":"../data","access":"read"}]}}"#;
    std::fs::write(&path, template).unwrap();
    let run =
        RunConfig::read_grants(&path, [0; 32], "probe@1/main".into(), LaunchKind::Main).unwrap();
    assert_eq!(
        run.record().io.as_ref().unwrap().dirs[0].host,
        root.join("data").canonicalize().unwrap().to_str().unwrap()
    );
    assert!(RunConfig::validate(run.canonical()).is_ok());
    // A runner's grant set resolves its hosts beside the template too.
    let runner = br#"{"io":{"dirs":[],"runners":[{"name":"place","provider":"local.threads@1","grants":{"dirs":[{"path":"data","host":"../data","access":"read"}]},"definitions":null}]}}"#;
    std::fs::write(&path, runner).unwrap();
    let runs =
        RunConfig::read_grants(&path, [0; 32], "probe@1/main".into(), LaunchKind::Main).unwrap();
    let grants = runs.record().io.as_ref().unwrap().runners[0]
        .grants
        .as_ref();
    assert_eq!(
        grants.unwrap().dirs[0].host,
        root.join("data").canonicalize().unwrap().to_str().unwrap()
    );
    // Bytes without a template location cannot bind a relative host.
    assert!(
        RunConfig::bind_grants(template, [0; 32], "probe@1/main".into(), LaunchKind::Main).is_err()
    );
    let mut relative_record = run.record().clone();
    relative_record.io.as_mut().unwrap().dirs[0].host = "../data".into();
    assert!(relative_record.validated().is_err());
    for bad in [
        r#"{"io":{"dirs":[{"path":"data","host":"missing","access":"read"}]}}"#,
        r#"{"io":{"dirs":[{"path":"data","host":"../data","host":"../data","access":"read"}]}}"#,
        r#"{"io":{"dirs":[{"path":"data","host":"../data","access":"read","unknown":true}]}}"#,
    ] {
        std::fs::write(&path, bad).unwrap();
        assert!(
            RunConfig::read_grants(&path, [0; 32], "probe@1/main".into(), LaunchKind::Main)
                .is_err(),
            "{bad}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn launch_inputs_select_inline_json_without_widening_validation() {
    use std::ffi::OsStr;
    let template = r#"{"args":["λ","","--flag"]}"#;
    let run = RunConfig::grants_input(
        OsStr::new(template),
        [0; 32],
        "probe@1/main".into(),
        LaunchKind::Main,
    )
    .unwrap();
    assert_eq!(run.record().args.as_ref().unwrap(), &["λ", "", "--flag"]);
    assert!(run.record().io.is_none());
    assert!(run.record().env.is_none());
    let record = String::from_utf8(run.canonical().to_vec()).unwrap();
    assert_eq!(
        RunConfig::record_input(OsStr::new(&record))
            .unwrap()
            .identity(),
        run.identity()
    );
    for input in [
        "{",
        r#"{"args":[],"args":[]}"#,
        r#"{"unknown":true}"#,
        r#"{"io":true}"#,
    ] {
        assert!(
            RunConfig::grants_input(
                OsStr::new(input),
                [0; 32],
                "probe@1/main".into(),
                LaunchKind::Main
            )
            .is_err()
        );
        assert!(RunConfig::record_input(OsStr::new(input)).is_err());
    }
    assert!(
        RunConfig::grants_input(
            OsStr::new(r#"{"io":{"dirs":[{"path":"","host":"relative","access":"read"}]}}"#),
            [0; 32],
            "probe@1/main".into(),
            LaunchKind::Main
        )
        .is_err()
    );
    assert!(matches!(
        RunConfig::record_input(OsStr::new("run.json")),
        Err(RunRefusal::Invalid { .. })
    ));
    assert!(matches!(
        RunConfig::record_input(OsStr::new(" {}")),
        Err(RunRefusal::Invalid { .. })
    ));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert!(RunConfig::record_input(OsStr::from_bytes(b"{\xff}")).is_err());
    }
}

#[test]
fn a_fuel_budget_binds_only_to_code_that_charges_fuel_and_a_memory_budget_to_any() {
    let facts = |run: &ValidatedRun, fueled| ProgramFacts {
        program: run.program(),
        entry: run.record().entry.clone(),
        launch: run.record().launch,
        roots: vec![],
        code: CodeReach::Safe,
        fueled,
    };
    let mut record = empty();
    record.budgets = Some(Budgets {
        provider: "budgets.limits@2".into(),
        fuel: Some(1000),
        memory_bytes: None,
    });
    let run = valid(&record);
    assert!(run.clone().bind(&facts(&run, true)).is_ok());
    assert!(matches!(
        run.clone().bind(&facts(&run, false)),
        Err(RunRefusal::Unsupported { field }) if field == "budgets.fuel"
    ));
    record.budgets = Some(Budgets {
        provider: "budgets.limits@2".into(),
        fuel: None,
        memory_bytes: Some(1 << 20),
    });
    let run = valid(&record);
    assert!(run.clone().bind(&facts(&run, false)).is_ok());
}

#[test]
fn budgets_name_a_provider_none_normalizes_away_and_limits_need_a_nonzero_bound() {
    let limits = |fuel, memory_bytes| Budgets {
        provider: "budgets.limits@2".into(),
        fuel,
        memory_bytes,
    };
    let mut record = empty();
    record.budgets = Some(limits(Some(1000), None));
    let run = valid(&record);
    assert_eq!(run.budgets(), Some(&limits(Some(1000), None)));
    let text = std::str::from_utf8(run.canonical()).unwrap();
    assert!(
        text.ends_with(
            r#""budgets":{"provider":"budgets.limits@2","fuel":1000,"memory_bytes":null}}"#
        ),
        "{text}"
    );
    assert_eq!(RunConfig::validate(run.canonical()).unwrap(), run);
    // The budget is part of the run's identity.
    assert_ne!(run.identity(), valid(&empty()).identity());
    record.budgets = Some(limits(None, Some(1 << 20)));
    assert_eq!(valid(&record).budgets(), Some(&limits(None, Some(1 << 20))));
    // budgets.none@1 is the default: it has one canonical spelling, the absent entry.
    record.budgets = Some(Budgets {
        provider: "budgets.none@1".into(),
        fuel: None,
        memory_bytes: None,
    });
    assert_eq!(valid(&record), valid(&empty()));
    for (budgets, refusal) in [
        (limits(None, None), "invalid"),
        (limits(Some(0), None), "invalid"),
        (limits(None, Some(0)), "invalid"),
        (
            Budgets {
                provider: "budgets.none@1".into(),
                fuel: Some(1),
                memory_bytes: None,
            },
            "invalid",
        ),
        // `budgets.limits@2` retires @1.
        (
            Budgets {
                provider: "budgets.limits@1".into(),
                fuel: Some(1),
                memory_bytes: None,
            },
            "unsupported",
        ),
    ] {
        record.budgets = Some(budgets);
        let error = RunConfig::validate(&bytes(&record)).unwrap_err();
        assert!(
            matches!(
                (&error, refusal),
                (RunRefusal::Invalid { .. }, "invalid")
                    | (RunRefusal::Unsupported { .. }, "unsupported")
            ),
            "{error}"
        );
    }
    // A grants template chooses the budget like any other host policy.
    let run = RunConfig::bind_grants(
        br#"{"budgets":{"provider":"budgets.limits@2","fuel":5}}"#,
        [0; 32],
        "probe@1/main".into(),
        LaunchKind::Main,
    )
    .unwrap();
    assert_eq!(run.budgets(), Some(&limits(Some(5), None)));
    assert!(
        RunConfig::bind_grants(
            br#"{"budgets":{"provider":"budgets.limits@2","fuel":5,"wall":1}}"#,
            [0; 32],
            "probe@1/main".into(),
            LaunchKind::Main,
        )
        .is_err()
    );
}

#[test]
fn telemetry_grants_name_a_provider_and_absent_ones_keep_their_bytes() {
    let telemetry = |provider: &str, seed: Option<u64>, queue: Option<u64>| {
        let mut record = empty();
        record.io = Some(IoGrants {
            telemetry: Some(TelemetryGrant {
                provider: provider.into(),
                seed,
                queue,
            }),
            ..IoGrants::default()
        });
        record
    };
    let mut absent = empty();
    absent.io = Some(IoGrants::default());
    assert!(
        !String::from_utf8(valid(&absent).canonical().to_vec())
            .unwrap()
            .contains("telemetry")
    );
    let run = valid(&telemetry("telemetry.null@1", Some(7), Some(2)));
    assert!(run.io_projections().contains("telemetry"));
    assert_eq!(
        run.telemetry_provider(),
        Some((ProviderContract::TelemetryNull, 7, Some(2)))
    );
    let run = valid(&telemetry("telemetry.capture@1", None, None));
    assert_eq!(
        run.telemetry_provider(),
        Some((ProviderContract::TelemetryCapture, 0, None))
    );
    for refused_record in [
        telemetry("telemetry.null@1", None, None),
        telemetry("telemetry.capture@1", Some(1), None),
        telemetry("telemetry.capture@1", None, Some(1)),
        telemetry("telemetry.otlp@1", None, None),
    ] {
        refused(&refused_record);
    }
}
