use super::*;
use crate::host::BodyRun;
use hirsel_run_config::{Cores, managed_cores::CpuSet};

fn isolated(name: &str) -> bool {
    const CHILD: &str = "HIRSEL_WIDTH_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    // Process permits are shared; other tests deliberately contract the grant
    // and retain stopped workers. Keep this test's counters independent.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("managed_cores::width_tests::{name}"),
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    false
}

fn manager(control: RunControl, width: u32) -> Arc<ManagedCores> {
    let cpus = CpuSet::parse("0-7").unwrap();
    Arc::new(
        ManagedCores::admit(
            Placement {
                cores: Cores::Fixed(width.into()),
                ..Default::default()
            },
            HostGrant {
                max_cores: 8,
                cpus: cpus.clone(),
            },
            HostLimits {
                admission_affinity: cpus.clone(),
                effective_cpuset: cpus,
                quota: None,
                throttled_usec: 0,
            },
            Duration::ZERO,
            control,
            true,
        )
        .unwrap(),
    )
}

#[test]
fn partial_startup_is_not_running_and_pending_is_the_full_target() {
    if !isolated("partial_startup_is_not_running_and_pending_is_the_full_target") {
        return;
    }
    use hirsel_run_config::managed_cores::WorkerPhase;
    let control = Arc::new(crate::stop::Control::default());
    let manager = manager(RunControl::from_inner(control.clone()), 4);
    // Deterministic equivalent of sampling grow between its third and fourth
    // registration. No root can run until start_with_home has completed grow.
    control
        .live_width
        .store(3, std::sync::atomic::Ordering::Release);
    let sample = manager.snapshot();
    assert_eq!(sample.worker_phase, Some(WorkerPhase::Starting));
    assert_eq!(
        sample.width.effective,
        u64::from(3.min(crate::pool::compute_permits().0))
    );
    assert_eq!(sample.width.pending, 4);
    assert_eq!(sample.runnable, None);
    assert_eq!(
        serde_json::to_value(sample).unwrap()["worker_phase"],
        "starting"
    );
    control.finish(0);
    assert_eq!(manager.snapshot().worker_phase, Some(WorkerPhase::Finished));
}

#[test]
fn ordered_width_is_installed_before_root_readiness_in_repeated_runs() {
    if !isolated("ordered_width_is_installed_before_root_readiness_in_repeated_runs") {
        return;
    }
    use hirsel_run_config::managed_cores::WorkerPhase;
    for width in [1, 2, 4, 8] {
        for _ in 0..10 {
            let mut run = BodyRun::new(width, false);
            let control = run.control();
            let manager = manager(control.clone(), width);
            let before = manager.snapshot();
            assert_eq!(before.worker_phase, Some(WorkerPhase::Starting));
            assert_eq!(before.width.effective, 0);
            assert_eq!(before.width.pending, u64::from(width));
            // An actual readiness handshake, held until the observer has
            // sampled the real Ordered topology. No startup sleeps or retries.
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
            let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
            let observer = manager.clone();
            let observer = std::thread::spawn(move || {
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let sample = observer.snapshot();
                release_tx.send(()).unwrap();
                sample
            });
            unsafe { run.use_ordered_home_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    crate::tasks::ordered::engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                },
                move |_| {
                    assert_eq!(control.worker_width(), width);
                    ready_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            let sample = observer.join().unwrap();
            assert_eq!(sample.worker_phase, Some(WorkerPhase::Running));
            // The shared host may grant fewer real compute permits. That
            // ceiling is distinct from installing all requested destinations.
            let effective = width.min(crate::pool::compute_permits().0);
            assert_eq!(sample.width.effective, u64::from(effective));
            assert_eq!(
                sample.width.pending,
                if effective == width { 0 } else { width.into() }
            );
            assert_eq!(sample.lost_permits, Some(0));
            assert_eq!(manager.snapshot().worker_phase, Some(WorkerPhase::Finished));
            assert_eq!(manager.snapshot().width.effective, 0);
        }
    }
}

#[test]
fn claim_and_settlement_share_the_interval_and_stop_on_unreadable_capacity() {
    if !isolated("claim_and_settlement_share_the_interval_and_stop_on_unreadable_capacity") {
        return;
    }
    let run = BodyRun::new(4, false);
    let control = run.control();
    let manager = manager(control.clone(), 4);
    let proc = tempfile::tempdir().unwrap();
    std::fs::create_dir(proc.path().join("thread-self")).unwrap();
    std::fs::write(
        proc.path().join("thread-self/status"),
        "Cpus_allowed_list:\t0-7\n",
    )
    .unwrap();
    let service = Service::new(
        LinuxLimits::from_proc(proc.path()).unwrap(),
        manager.clone(),
        None,
        None,
        None,
    );
    let mut contracted = manager.state.lock().unwrap().limits.clone();
    contracted.quota = hirsel_run_config::managed_cores::Quota::parse("100000 100000\n").unwrap();
    assert!(service.permits(4));
    assert!(service.check(Duration::from_millis(999), || Ok(contracted.clone())));
    assert!(service.permits(4));
    assert!(service.check(Duration::from_secs(1), || Ok(contracted.clone())));
    assert!(!service.permits(4));
    assert!(service.permits(1));
    assert_eq!(
        manager.state.lock().unwrap().resolver.resolution().target,
        1
    );
    assert!(!control.status().stop_requested);
    assert!(!service.check(Duration::from_secs(2), || Err(
        "unreadable hard limit".into()
    )));
    assert!(control.status().stop_requested);
    assert!(!service.permits(1));
    assert_eq!(
        manager.state.lock().unwrap().resolver.resolution().target,
        1
    );
}
