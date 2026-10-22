use hirsel_rt::{
    host::BodyRun,
    host_paths::{PinnedDirectory, RecorderDestination},
    managed_cores::ManagedCores,
};
use hirsel_run_config::{
    Cores, Placement,
    managed_cores::{CpuSet, HostGrant, HostLimits},
};
use std::{fs, sync::Arc, time::Duration};
fn limits(cpus: &str) -> HostLimits {
    HostLimits {
        admission_affinity: CpuSet::parse("0-7").unwrap(),
        effective_cpuset: CpuSet::parse(cpus).unwrap(),
        quota: None,
        throttled_usec: 0,
    }
}
fn grant() -> HostGrant {
    HostGrant {
        cpus: CpuSet::parse("0-7").unwrap(),
        max_cores: 8,
    }
}
fn manager() -> ManagedCores {
    ManagedCores::admit(
        Placement {
            cores: Cores::Fixed(8),
            cpus: Some("0-3".into()),
            ..Default::default()
        },
        grant(),
        limits("0-7"),
        Duration::ZERO,
        BodyRun::new(1, false).control(),
        true,
    )
    .unwrap()
}
#[test]
fn whole_control_requests_refuse_without_changing_requested_placement() {
    let manager = manager();
    for bytes in [
        br#"{"placement":{"cores":2,"cpus":"7"},"unknown":1}"#.as_slice(),
        br#"{"placement":{"cores":2,"cpus":"20"}}"#,
        br#"{"placement":{"cores":0}}"#,
        br#"{"placement":{"cores":0,"cores":2}}"#,
        br#"{"placement":{"cores":2.0}}"#,
        br#"{"placement":{"cores":null}}"#,
        br#"{"placement":{"cores":2,"mystery":1}}"#,
        br#"{"placement":{"control":"/tmp/new"}}"#,
    ] {
        assert!(manager.request(bytes).is_err());
        assert_eq!(manager.snapshot().width.requested, Cores::Fixed(8));
    }
    let next = manager.request(br#"{"placement":{"cores":2}}"#).unwrap();
    assert_eq!(next.width.requested, Cores::Fixed(2));
    assert_eq!(next.width.pending, 2);
}
#[test]
fn one_second_external_contraction_overrides_a_conflicting_cpulist() {
    let manager = manager();
    assert!(
        !manager
            .refresh(Duration::from_millis(999), || panic!("early recheck"))
            .unwrap()
    );
    assert!(
        manager
            .refresh(Duration::from_secs(1), || Ok((grant(), limits("6-7"))))
            .unwrap()
    );
    let contracted = manager.snapshot();
    assert!(contracted.cpus_conflict);
    assert_eq!(contracted.width.pending, 2);
    // A control request must still refuse an empty intersection after external contraction.
    assert!(
        manager
            .request(br#"{"placement":{"cores":1,"cpus":"0"}}"#)
            .is_err()
    );
    assert_eq!(manager.snapshot().width.requested, Cores::Fixed(8));
}
#[test]
fn production_gate_preserves_requested_width_without_enabling_unproven_execution() {
    let run = BodyRun::new(1, false);
    let manager = Arc::new(
        ManagedCores::admit(
            Placement {
                cores: Cores::Fixed(8),
                ..Default::default()
            },
            grant(),
            limits("0-7"),
            Duration::ZERO,
            run.control(),
            false,
        )
        .unwrap(),
    );
    let inside = manager.clone();
    assert_eq!(
        run.run(
            |_| {},
            move |_| {
                let snapshot = inside.snapshot();
                assert_eq!(snapshot.width.requested, Cores::Fixed(8));
                assert_eq!(snapshot.width.effective, 1);
                assert_eq!(snapshot.width.pending, 8);
                0
            }
        )
        .ended,
        Ok(0)
    );
}
#[test]
fn control_admission_excludes_mounts_and_sinks_and_creates_a_private_socket() {
    use std::os::unix::fs::PermissionsExt;
    let path_scratch = tempfile::Builder::new()
        .prefix("hirsel-control-")
        .tempdir()
        .unwrap();
    let path = path_scratch.path().to_path_buf();
    let socket_path = path.join("ctl");
    let mount = PinnedDirectory::open(&path).unwrap();
    assert!(
        RecorderDestination::pin(&socket_path)
            .unwrap()
            .bind_control(&[("readable", &mount)], &[])
            .is_err()
    );
    assert!(!socket_path.exists());
    let socket = RecorderDestination::pin(&socket_path)
        .unwrap()
        .bind_control(&[], &[])
        .unwrap();
    assert_eq!(
        fs::metadata(&socket_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        RecorderDestination::pin(&socket_path)
            .unwrap()
            .bind_control(&[], &[])
            .is_err()
    );
    drop(socket);
    assert!(!socket_path.exists());
    let sink_path = path.join("sink");
    let sink = fs::File::create(&sink_path).unwrap();
    fs::hard_link(&sink_path, &socket_path).unwrap();
    assert!(
        RecorderDestination::pin(&socket_path)
            .unwrap()
            .bind_control(&[], &[("stdout", &sink)])
            .is_err()
    );
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn external_cpu_contraction_migrates_live_workers_then_growth_restores_width() {
    let run = BodyRun::new(1, true);
    let width_control = run.control();
    let manager = Arc::new(
        ManagedCores::admit(
            Placement {
                cores: Cores::Fixed(8),
                cpus: Some("0-3".into()),
                ..Default::default()
            },
            grant(),
            limits("0-7"),
            Duration::ZERO,
            run.control(),
            true,
        )
        .unwrap(),
    );
    let inside = manager.clone();
    let result = run.run(
        |_| {},
        move |ctx| {
            assert_eq!(
                inside.snapshot().width.effective.min(4),
                inside.snapshot().width.effective
            );
            inside
                .refresh(Duration::from_secs(1), || Ok((grant(), limits("6-7"))))
                .unwrap();
            let wait = |target| {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                loop {
                    unsafe { hirsel_rt::tasks::yield_now(&mut *ctx) };
                    if width_control.worker_width() == target {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "target {target} did not settle"
                    );
                }
            };
            wait(2);
            assert!(inside.snapshot().cpus_conflict);
            assert_eq!(
                inside.snapshot().width.effective,
                u64::from(2.min(hirsel_rt::pool::compute_permits().0))
            );
            inside.request(br#"{"placement":{"cpus":null}}"#).unwrap();
            inside
                .refresh(Duration::from_secs(2), || Ok((grant(), limits("0-7"))))
                .unwrap();
            wait(8);
            assert_eq!(
                inside.snapshot().width.effective,
                u64::from(8.min(hirsel_rt::pool::compute_permits().0))
            );
            assert!(!inside.snapshot().cpus_conflict);
            0
        },
    );
    assert_eq!(result.ended, Ok(0));
}

#[test]
fn real_live_width_probe() {
    use hirsel_run_config::{AutoCores, managed_cores::LinuxLimits};
    let reader = LinuxLimits::capture().unwrap();
    let limits = reader.read().unwrap();
    let grant = HostGrant {
        max_cores: limits.admission_affinity.len(),
        cpus: limits.admission_affinity.clone(),
    };
    let run = BodyRun::new(1, true);
    let control = run.control();
    let manager = ManagedCores::admit(
        Placement {
            cores: Cores::Auto(AutoCores::Auto),
            ..Default::default()
        },
        grant,
        limits,
        Duration::ZERO,
        control.clone(),
        true,
    )
    .unwrap();
    let target = manager.snapshot().width.pending;
    assert_eq!(
        run.run(
            |_| {},
            move |_| {
                assert_eq!(u64::from(control.worker_width()), target);
                if let Ok(expected) = std::env::var("HIRSEL_CORES_EXPECTED") {
                    assert_eq!(target, expected.parse::<u64>().unwrap());
                }
                println!("live_workers={target}");
                0
            }
        )
        .ended,
        Ok(0)
    );
}
