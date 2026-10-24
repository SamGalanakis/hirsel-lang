//! The separate timed schema must never accept an At through the old provider.
use hirsel_run_config::validate_process_grant;
use serde_json::{Value, json};
fn checked(steps: Value) -> Result<hirsel_run_schema::ProcessGrant, hirsel_run_schema::RunRefusal> {
    validate_process_grant(&serde_json::to_vec(&json!({"provider":"process.script.timed@1","clock":"run.monotonic","rules":[],"runs":[],"children":[{"program":"/bin/cat","args":[],"steps":steps}]})).unwrap())
}
#[test]
fn timed_a03_admission_boundaries() {
    for steps in [
        json!([{"at":-1}]),
        json!([{"at":0.5}]),
        json!([{"at":5},{"expect":[]},{"at":4}]),
        json!([{"exit":{"exited":0}},{"at":5}]),
        json!([{"at":null}]),
        json!([{"at":5,"unknown":0}]),
        json!([{"sleep":5}]),
    ] {
        assert!(checked(steps.clone()).is_err(), "{steps}");
    }
    assert!(checked(json!([{"at":0},{"at":0},{"at":u64::MAX}])).is_ok());
    let too_large = br#"{"provider":"process.script.timed@1","clock":"run.monotonic","rules":[],"runs":[],"children":[{"program":"/bin/cat","args":[],"steps":[{"at":18446744073709551616}]}]}"#;
    assert!(validate_process_grant(too_large).is_err());
    for field in ["clock", "children", "runs"] {
        let mut value = json!({"provider":"process.script.timed@1","clock":"run.monotonic","rules":[],"runs":[],"children":[]});
        value[field] = Value::Null;
        assert!(validate_process_grant(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
#[test]
fn untimed_json_control_bytes_and_at_refusal() {
    let bytes = br#"{"provider":"process.script@1","rules":[],"runs":[],"children":[{"program":"/bin/cat","args":[],"steps":[{"print":[120]},{"expect":[121]},{"exit":{"exited":7}}]}]}"#;
    let old = validate_process_grant(bytes).unwrap();
    assert_eq!(serde_json::to_vec(&old).unwrap(), bytes);
    let mut value: Value = serde_json::from_slice(bytes).unwrap();
    value["children"][0]["steps"][0] = json!({"at":5});
    assert!(validate_process_grant(&serde_json::to_vec(&value).unwrap()).is_err());
    value["provider"] = json!("process.script.timed@1");
    assert!(validate_process_grant(&serde_json::to_vec(&value).unwrap()).is_err());
    value["clock"] = json!("host.monotonic");
    assert!(validate_process_grant(&serde_json::to_vec(&value).unwrap()).is_err());
    value["clock"] = json!("run.monotonic");
    assert!(validate_process_grant(&serde_json::to_vec(&value).unwrap()).is_ok());
}

#[test]
fn timed_a04_full_record_requires_isolated_fixture_clock() {
    use hirsel_run_schema::{LaunchKind, ProgramNode, ProgramRevision};
    let program = ProgramRevision::encode(
        "app@1/main",
        vec![ProgramNode {
            identity: "app@1/main".into(),
            content: vec![b"main".to_vec()],
            dependencies: vec![],
        }],
    )
    .unwrap();
    let mut record = hirsel_run_config::RunConfig::for_program(&program, LaunchKind::Main);
    record.inputs.fixture_monotonic = Some(0);
    record.io = Some(serde_json::from_value(json!({"dirs":[], "monotonic_clock":"clock.monotonic.ns@1", "process":{"provider":"process.script.timed@1","clock":"run.monotonic","rules":[],"runs":[],"children":[]}})).unwrap());
    assert!(record.clone().validated().is_ok());
    let mut missing = record.clone();
    missing.inputs.fixture_monotonic = None;
    assert!(missing.validated().is_err());
    let mut missing = record.clone();
    missing.io.as_mut().unwrap().monotonic_clock = None;
    assert!(missing.validated().is_err());
    let mut wall = record.clone();
    wall.io.as_mut().unwrap().wall_clock = Some("clock.wall.unix_ns@1".into());
    assert!(wall.validated().is_err());
    let mut random = record.clone();
    random.io.as_mut().unwrap().random = Some("random.system@1".into());
    assert!(random.validated().is_err());
    let mut net = record.clone();
    net.io.as_mut().unwrap().net =
        Some(serde_json::from_value(json!({"provider":"net.stream@1","rules":[]})).unwrap());
    assert!(net.validated().is_err());
}
#[test]
fn timed_r01_initial_script_grants_and_program_are_immutable_bindings() {
    use hirsel_run_schema::{LaunchKind, ProgramNode, ProgramRevision};
    let program = ProgramRevision::encode(
        "app@1/main",
        vec![ProgramNode {
            identity: "app@1/main".into(),
            content: vec![b"main".to_vec()],
            dependencies: vec![],
        }],
    )
    .unwrap();
    let mut record = hirsel_run_config::RunConfig::for_program(&program, LaunchKind::Main);
    record.inputs.fixture_monotonic = Some(0);
    record.io=Some(serde_json::from_value(json!({"dirs":[],"monotonic_clock":"clock.monotonic.ns@1","process":{"provider":"process.script.timed@1","clock":"run.monotonic","rules":[{"program":"/bin/cat"}],"runs":[],"children":[{"program":"/bin/cat","args":[],"steps":[{"at":5},{"print":[120]},{"exit":{"exited":0}}]}]}})).unwrap());
    let original = record.validated().unwrap();
    let bytes = original.canonical().to_vec();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    for (pointer, replacement) in [
        ("/inputs/fixture_monotonic", json!(1)),
        ("/io/process/children/0/steps/0/at", json!(6)),
        ("/io/process/rules/0/program", json!("/bin/other")),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        let changed =
            hirsel_run_config::RunConfig::validate(&serde_json::to_vec(&changed).unwrap())
                .unwrap()
                .canonical()
                .to_vec();
        assert!(
            original
                .check_replay(
                    &changed,
                    original.identity(),
                    &hirsel_run_config::AdmissionEvidence {
                        at: 0,
                        revocation: None
                    }
                )
                .is_err(),
            "{pointer}"
        );
    }
    let mut identity = original.identity();
    identity[0] ^= 1;
    assert!(
        original
            .check_replay(
                &bytes,
                identity,
                &hirsel_run_config::AdmissionEvidence {
                    at: 0,
                    revocation: None
                }
            )
            .is_err()
    );
    assert_eq!(original.canonical(), bytes);
}
