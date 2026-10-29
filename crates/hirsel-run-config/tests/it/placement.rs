use hirsel_run_config::*;
use hirsel_run_schema::*;
use serde_json::{Value, json};

fn record(placement: Value) -> Result<ValidatedRun, RunRefusal> {
    let mut record = serde_json::to_value(RunConfig::empty(
        [0; 32],
        "placement@1/main".into(),
        LaunchKind::Main,
    ))
    .unwrap();
    record["placement"] = placement;
    RunConfig::validate(&serde_json::to_vec(&record).unwrap())
}

#[test]
fn generated_placement_matrix_never_changes_canonical_bytes_or_digest() {
    let base = record(Value::Null).unwrap();
    let mut count = 0;
    for cores in [
        json!(1),
        json!(8),
        json!(u64::MAX),
        json!("auto"),
        json!({"min": 1, "max": 8}),
    ] {
        for cpus in [Value::Null, json!("0"), json!("0-3,8,12-15")] {
            for pin in ["none", "compact", "spread"] {
                for commit in ["shared", "dedicated"] {
                    for run_ahead in [0, 8, 64] {
                        for reorder in ["off", "auto"] {
                            for speculation in [Value::Null, json!(0), json!(u64::MAX)] {
                                let placement = json!({
                                    "cores": cores, "cpus": cpus, "pin": pin, "commit": commit,
                                    "run_ahead": run_ahead, "reorder": reorder,
                                    "speculation_bytes": speculation,
                                    "metrics": "/does/not/exist/metrics", "control": "/does/not/exist/control"
                                });
                                let run = record(placement).unwrap();
                                assert_eq!(run.canonical(), base.canonical());
                                assert_eq!(run.identity(), base.identity());
                                assert_eq!(RunConfig::validate(&run.launch_bytes()).unwrap(), run);
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(count, 1620);
    assert_eq!(
        record(json!({})).unwrap().record().placement,
        Some(Placement::default())
    );
}

#[test]
fn closed_placement_schema_rejects_unknowns_duplicates_and_invalid_values() {
    for placement in [
        json!({"unknown": 0}),
        json!({"cores": 0}),
        json!({"cores": -1}),
        json!({"cores": "8"}),
        json!({"cores": {"auto": null}}),
        json!({"cores": {"min": 0, "max": 2}}),
        json!({"cores": {"min": 3, "max": 2}}),
        json!({"cores": {"min": 1}}),
        json!({"cores": {"min": 1, "max": 2, "unknown": 3}}),
        json!({"cpus": ""}),
        json!({"cpus": "3-1"}),
        json!({"cpus": "0,,1"}),
        json!({"cpus": "-1"}),
        json!({"cpus": "0-1-2"}),
        json!({"cpus": "4294967296"}),
        json!({"pin": "random"}),
        json!({"commit": "random"}),
        json!({"reorder": "on"}),
        json!({"run_ahead": 65}),
        json!({"run_ahead": -1}),
        json!({"speculation_bytes": -1}),
        json!({"metrics": ""}),
        json!({"control": "\0"}),
    ] {
        assert!(record(placement.clone()).is_err(), "{placement}");
    }
    // Every nested object is closed; each supported member still rejects siblings.
    for member in [
        "cores",
        "cpus",
        "pin",
        "commit",
        "run_ahead",
        "reorder",
        "speculation_bytes",
        "metrics",
        "control",
    ] {
        let mut placement = json!({"unknown": true});
        placement[member] = serde_json::to_value(Placement::default()).unwrap()[member].clone();
        assert!(record(placement).is_err(), "{member}");
    }
    for bytes in [
        br#"{"cores":1,"cores":2}"#.as_slice(),
        br#"{"cores":{"min":1,"min":2,"max":8}}"#,
        br#"{"run_ahead":1.0}"#,
        br#"{"cores":1e0}"#,
    ] {
        assert!(Placement::from_json(bytes).is_err());
    }
}

#[test]
fn cores_override_preserves_other_placement_and_equals_record_member() {
    let original = record(json!({"cores": 2, "cpus": "0-3", "control": "/tmp/ctl"})).unwrap();
    let expected = record(json!({"cores": 8, "cpus": "0-3", "control": "/tmp/ctl"})).unwrap();
    let clone = original.clone();
    let bytes = original.launch_bytes();
    let canonical = original.canonical().to_vec();
    let identity = original.identity();
    assert_eq!(original.with_cores(8).unwrap(), expected);
    assert!(original.with_cores(0).is_err());
    drop(original);
    assert_eq!(clone.launch_bytes(), bytes);
    assert_eq!(clone.canonical(), canonical);
    assert_eq!(clone.identity(), identity);
    assert_eq!(clone.with_cores(8).unwrap(), expected);
}

#[test]
fn free_tasks_are_an_identity_contract_and_refuse_scheduler() {
    assert_eq!(
        ProviderContract::parse("tasks.free@1"),
        Some(ProviderContract::TasksFree)
    );
    let mut config = RunConfig::empty([0; 32], "placement@1/main".into(), LaunchKind::Main);
    config.io = Some(IoGrants {
        tasks: Some("tasks.free@1".into()),
        ..Default::default()
    });
    let free = config.validated().unwrap();
    config.io.as_mut().unwrap().tasks = Some("tasks.cooperative@1".into());
    assert_ne!(free.identity(), config.validated().unwrap().identity());
    config.io.as_mut().unwrap().tasks = Some("tasks.free@1".into());
    config.inputs.scheduler = Scheduler::parse("fifo@1:0");
    assert!(config.validated().is_err());
}
