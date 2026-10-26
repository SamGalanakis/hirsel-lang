use hirsel_run_config::{RunConfig, grants_within};
use hirsel_run_schema::{CpuGrant, IoGrants, LaunchKind, ProviderContract, RunRefusal};

fn record() -> RunConfig {
    let mut record = RunConfig::empty([0; 32], "probe@1/main".into(), LaunchKind::Main);
    record.io = Some(IoGrants {
        tasks: Some(ProviderContract::TasksFree.name().into()),
        cpu: Some(CpuGrant {
            provider: "cpu.scoped@1".into(),
            max_workers: 16,
            max_domains: 1,
            max_physical_bytes: 1 << 30,
            max_batch_bytes: 1 << 20,
        }),
        ..Default::default()
    });
    record
}

#[test]
fn cpu_grant_is_canonical_semantic_input_and_absence_keeps_old_bytes() {
    let record = record();
    let run = record.clone().validated().unwrap();
    assert_eq!(RunConfig::validate(run.canonical()).unwrap(), run);
    let mut value = serde_json::to_value(&record).unwrap();
    for member in [
        "max_workers",
        "max_domains",
        "max_physical_bytes",
        "max_batch_bytes",
    ] {
        value["io"]["cpu"][member] = 1.into();
        let changed = RunConfig::validate(&serde_json::to_vec(&value).unwrap()).unwrap();
        if member != "max_domains" {
            assert_ne!(changed.identity(), run.identity(), "{member}");
        }
        value = serde_json::to_value(&record).unwrap();
    }
    value["io"]["cpu"] = serde_json::Value::Null;
    let absent = RunConfig::validate(&serde_json::to_vec(&value).unwrap()).unwrap();
    value["io"].as_object_mut().unwrap().remove("cpu");
    assert_eq!(
        RunConfig::validate(&serde_json::to_vec(&value).unwrap()).unwrap(),
        absent
    );
    assert!(
        !String::from_utf8(absent.canonical().to_vec())
            .unwrap()
            .contains("cpu")
    );
}

#[test]
fn cpu_grant_requires_tasks_known_version_and_finite_explicit_limits() {
    let valid = serde_json::to_value(record()).unwrap();
    for (member, bad) in [
        ("max_workers", vec![0u64, 17, u64::MAX]),
        ("max_domains", vec![0, 2, u64::MAX]),
        ("max_physical_bytes", vec![0]),
        ("max_batch_bytes", vec![0]),
    ] {
        for number in bad {
            let mut value = valid.clone();
            value["io"]["cpu"][member] = number.into();
            assert!(
                RunConfig::validate(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{member}={number}"
            );
        }
        let mut missing = valid.clone();
        missing["io"]["cpu"].as_object_mut().unwrap().remove(member);
        assert!(
            RunConfig::validate(&serde_json::to_vec(&missing).unwrap()).is_err(),
            "missing {member}"
        );
    }
    for provider in ["cpu.scoped@2", "tasks.free@1", "cpu.scoped"] {
        let mut value = valid.clone();
        value["io"]["cpu"]["provider"] = provider.into();
        assert!(matches!(
            RunConfig::validate(&serde_json::to_vec(&value).unwrap()),
            Err(RunRefusal::Unsupported { .. })
        ));
    }
    let mut value = valid.clone();
    value["io"]["tasks"] = serde_json::Value::Null;
    assert!(RunConfig::validate(&serde_json::to_vec(&value).unwrap()).is_err());
    for width in 1..=16 {
        let mut value = valid.clone();
        value["io"]["cpu"]["max_workers"] = width.into();
        RunConfig::validate(&serde_json::to_vec(&value).unwrap()).unwrap();
    }
}

#[test]
fn cpu_unknown_and_duplicate_members_are_rejected() {
    let text = serde_json::to_string(&record()).unwrap();
    for text in [
        text.replace(
            "\"max_workers\":16",
            "\"max_workers\":16,\"max_workers\":16",
        ),
        text.replace("\"max_workers\":16", "\"max_workers\":16,\"auto\":true"),
    ] {
        assert!(matches!(
            RunConfig::validate(text.as_bytes()),
            Err(RunRefusal::Malformed { .. })
        ));
    }
}

#[test]
fn cpu_grant_subset_checks_each_limit_and_denial() {
    let wide = record().validated().unwrap();
    for member in ["max_workers", "max_physical_bytes", "max_batch_bytes"] {
        let mut value = serde_json::to_value(record()).unwrap();
        value["io"]["cpu"][member] = 1.into();
        let narrow = RunConfig::validate(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(grants_within(&narrow, &wide), "{member}");
        assert!(!grants_within(&wide, &narrow), "{member}");
    }
    let mut denied = record();
    denied.io.as_mut().unwrap().cpu = None;
    let denied = denied.validated().unwrap();
    assert!(grants_within(&denied, &wide));
    assert!(!grants_within(&wide, &denied));
}
