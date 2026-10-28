use super::super::*;
use hirsel_run_config::managed_cores::{CpuSet, HostLimits, Quota};

#[test]
fn admission_seeds_the_process_grant_once_without_replacing_live_permits() {
    const CHILD: &str = "HIRSEL_STARTUP_GRANT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "pool::audit_modules::startup_tests::admission_seeds_the_process_grant_once_without_replacing_live_permits",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let affinity = CpuSet::parse("0-7").unwrap();
    let mut limits = HostLimits {
        admission_affinity: affinity.clone(),
        effective_cpuset: affinity,
        quota: Quota::parse("250000 100000\n").unwrap(),
        throttled_usec: 0,
    };
    init_compute_grant(&limits);
    assert_eq!(compute_permits(), (2, 0, 0));
    let permits = process_permits();
    let exec = Exec::new(Kind::Home, std::ptr::null_mut(), false);
    assert!(permits.acquire(&exec, None, None).unwrap());
    set_compute_grant(3);
    limits.quota = Quota::parse("50000 100000\n").unwrap();
    init_compute_grant(&limits);
    assert_eq!(compute_permits(), (3, 1, 0));
    assert!(Arc::ptr_eq(&permits, &process_permits()));
    permits.release(&exec);
    assert_eq!(compute_permits(), (3, 0, 0));
}
