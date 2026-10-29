use hirsel_run_config::{managed_cores::*, *};
use hirsel_run_schema::{LaunchKind, RunRefusal};
use std::{
    fs, io,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

fn cpus(text: &str) -> CpuSet {
    CpuSet::parse(text).unwrap()
}
fn placement(cores: Cores) -> Placement {
    Placement {
        cores,
        ..Default::default()
    }
}
fn fixed(n: u64) -> Placement {
    placement(Cores::Fixed(n))
}
fn auto() -> Placement {
    placement(Cores::Auto(AutoCores::Auto))
}
fn limits() -> HostLimits {
    HostLimits {
        admission_affinity: cpus("0-15"),
        effective_cpuset: cpus("0-15"),
        quota: None,
        throttled_usec: 0,
    }
}
fn grant() -> HostGrant {
    HostGrant {
        max_cores: 16,
        cpus: cpus("0-15"),
    }
}

#[test]
fn cpulists_are_intervals_not_unbounded_cpu_expansions() {
    assert_eq!(cpus("0-4294967295").len(), 1u64 << 32);
    assert_eq!(cpus("8-12,2,0-3,3-8").ranges(), &[(0, 12)]);
    assert_eq!(
        cpus("0-4,9-15").intersection(&cpus("3-10,20")).ranges(),
        &[(3, 4), (9, 10)]
    );
    for text in ["", "-1", "1-0", "0,,1", "00", "4294967296", "1-2-3"] {
        assert!(CpuSet::parse(text).is_err(), "{text}");
    }
}

#[test]
fn go_cpu_max_vectors_and_exact_fractional_comparison() {
    assert_eq!(Quota::parse("max 100000\n").unwrap(), None);
    for (text, ceil) in [
        ("500000 100000\n", 5),
        ("50000 100000\n", 1),
        ("250000 100000\n", 3),
        ("9223372036854775807 9223372036854775807\n", 1),
    ] {
        assert_eq!(Quota::parse(text).unwrap().unwrap().ceil(), ceil);
    }
    for text in [
        "500000 100000",
        "500000\n",
        "500000us 100000\n",
        "500000 100000us\n",
        "max 0\n",
        "0 100000\n",
        "1 0\n",
        "-1 100000\n",
        "max 1 extra\n",
    ] {
        assert!(Quota::parse(text).is_err(), "{text}");
    }
    let a = Quota::parse("18446744073709551614 18446744073709551613\n")
        .unwrap()
        .unwrap();
    let b = Quota::parse("18446744073709551615 18446744073709551614\n")
        .unwrap()
        .unwrap();
    assert_eq!(a.minimum(b), b);
}

#[test]
fn every_request_form_respects_affinity_cpuset_and_host_grant() {
    let mut l = limits();
    l.admission_affinity = cpus("2-11");
    l.effective_cpuset = cpus("0-7");
    l.quota = Quota::parse("250000 100000\n").unwrap();
    let g = HostGrant {
        max_cores: 4,
        cpus: cpus("4-15"),
    };
    for (p, target) in [
        (fixed(1), 1),
        (fixed(99), 4),
        (auto(), 3),
        (placement(Cores::Range(CoreRange { min: 4, max: 8 })), 4),
        (placement(Cores::Range(CoreRange { min: 1, max: 2 })), 2),
    ] {
        let r = resolve(&p, &g, &l, false).unwrap();
        assert_eq!((r.target, r.hard_limit), (target, 4));
        assert_eq!(r.cpus, cpus("4-7"));
    }
    l.quota = None;
    assert_eq!(resolve(&auto(), &g, &l, false).unwrap().target, 4);
    // The hard ceiling wins even over a range's minimum; fixed skips quota.
    l.quota = Quota::parse("50000 100000\n").unwrap();
    assert_eq!(resolve(&fixed(4), &g, &l, false).unwrap().target, 4);
    assert_eq!(
        resolve(
            &placement(Cores::Range(CoreRange { min: 8, max: 9 })),
            &g,
            &l,
            false
        )
        .unwrap()
        .target,
        4
    );
}

#[test]
fn empty_admission_and_control_refuse_external_contraction_reports_conflict() {
    let mut p = auto();
    p.cpus = Some("8-15".into());
    let mut resolver = CoreResolver::admit(p.clone(), &grant(), &limits(), Duration::ZERO).unwrap();
    let mut contracted = limits();
    contracted.effective_cpuset = cpus("0-3");
    assert!(resolve(&p, &grant(), &contracted, false).is_err());
    assert!(resolver.replace(p, &grant(), &contracted).is_err());
    assert_eq!(resolver.resolution().target, 8);
    assert!(
        resolver
            .refresh(Duration::from_secs(1), || Ok((grant(), contracted.clone())))
            .unwrap()
    );
    assert_eq!(resolver.resolution().target, 4);
    assert!(resolver.resolution().cpus_conflict);
    assert_eq!(resolver.resolution().cpus, cpus("0-3"));
    let zero = HostGrant {
        max_cores: 0,
        ..grant()
    };
    assert!(resolve(&fixed(1), &zero, &limits(), true).is_err());
    contracted.effective_cpuset = cpus("20");
    assert!(resolve(&fixed(1), &grant(), &contracted, true).is_err());
}

#[test]
fn every_form_rechecks_once_a_second_and_preserves_last_good_state() {
    for p in [
        fixed(16),
        auto(),
        placement(Cores::Range(CoreRange { min: 1, max: 16 })),
    ] {
        let mut r = CoreResolver::admit(p, &grant(), &limits(), Duration::ZERO).unwrap();
        assert!(
            !r.refresh(Duration::from_millis(999), || panic!("early read"))
                .unwrap()
        );
        let mut l = limits();
        l.effective_cpuset = cpus("0-1");
        assert!(
            r.refresh(Duration::from_secs(1), || Ok((grant(), l)))
                .unwrap()
        );
        assert_eq!(r.resolution().target, 2);
        assert!(
            !r.refresh(Duration::from_millis(1999), || panic!("early read"))
                .unwrap()
        );
        assert!(
            r.refresh(Duration::from_secs(2), || Ok((grant(), limits())))
                .unwrap()
        );
        assert_eq!(r.resolution().target, 16);
        assert!(
            r.refresh(Duration::from_secs(3), || Err(RunRefusal::Invalid {
                field: "test".into(),
                detail: "read failed".into()
            }))
            .is_err()
        );
        assert_eq!(r.resolution().target, 16);
        assert!(
            !r.refresh(Duration::from_millis(3500), || panic!(
                "error retry too soon"
            ))
            .unwrap()
        );
    }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "hirsel-cores-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let f = Self(root);
        f.put(
            "proc/thread-self/status",
            "Name:\tfixture\nCpus_allowed_list:\t0-15\n",
        );
        f.put("proc/self/cgroup", "0::/parent/leaf\n");
        f.put("cg/parent/leaf/cpuset.cpus.effective", "0-15\n");
        f.put("cg/parent/leaf/cpu.max", "max 100000\n");
        f.put(
            "cg/parent/leaf/cpu.stat",
            "usage_usec 500\nthrottled_usec 123\n",
        );
        f.put(
            "proc/self/mountinfo",
            &format!(
                "1 0 0:1 / {} rw - cgroup2 cgroup rw\n",
                f.0.join("cg").display()
            ),
        );
        f
    }
    fn put(&self, path: &str, text: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn reader(&self) -> LinuxLimits {
        LinuxLimits::from_proc(&self.0.join("proc")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn parent_only_quota_and_visible_namespace_root_are_included() {
    let f = Fixture::new();
    f.put("cg/cpu.max", "400000 100000\n");
    f.put("cg/parent/cpu.max", "250000 100000\n");
    let l = f.reader().read().unwrap();
    assert_eq!(resolve(&auto(), &grant(), &l, false).unwrap().target, 3);
    assert_eq!(l.throttled_usec, 123);
    // A bind mount must stop at its root, not read cpu.max above that mount.
    f.put("cg/cpu.max", "100000 100000\n");
    f.put(
        "proc/self/mountinfo",
        &format!(
            "1 0 0:1 /parent {} rw - cgroup2 cgroup rw\n",
            f.0.join("cg/parent").display()
        ),
    );
    let l = f.reader().read().unwrap();
    assert_eq!(resolve(&auto(), &grant(), &l, false).unwrap().target, 3);
}

#[test]
fn two_and_eight_core_limits_refresh_and_ignore_worker_pinning() {
    let f = Fixture::new();
    let reader = f.reader();
    for n in [2, 8] {
        f.put("cg/parent/cpu.max", &format!("{} 100000\n", n * 100000));
        // Simulate a worker pinned to CPU 0. Admission view was already saved.
        f.put("proc/thread-self/status", "Cpus_allowed_list:\t0\n");
        let l = reader.read().unwrap();
        assert_eq!(l.admission_affinity.len(), 16);
        assert_eq!(resolve(&auto(), &grant(), &l, false).unwrap().target, n);
    }
    f.put("cg/parent/leaf/cpuset.cpus.effective", "2-3\n");
    let l = reader.read().unwrap();
    assert_eq!(resolve(&fixed(8), &grant(), &l, true).unwrap().target, 2);
    f.put("cg/parent/leaf/cpu.max", "1 0\n");
    assert!(reader.read().is_err());
    // An I/O failure is not an unlimited quota. A directory is unreadable as
    // a control file even when these fixtures run under an administrative UID.
    let path = f.0.join("cg/parent/leaf/cpu.max");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let expected = fs::read_to_string(&path).unwrap_err();
    assert_eq!(
        reader.read().unwrap_err(),
        RunRefusal::Invalid {
            field: "host limits".into(),
            detail: format!("{}: {expected}", path.display()),
        }
    );
}

#[test]
fn discovery_handles_escaped_mounts_migration_and_does_not_skip_host_errors() {
    let f = Fixture::new();
    fs::rename(f.0.join("cg"), f.0.join("cg space")).unwrap();
    f.put(
        "proc/self/mountinfo",
        &format!(
            "1 0 0:1 / {} rw - cgroup2 cgroup rw\n",
            f.0.join("cg space")
                .display()
                .to_string()
                .replace(' ', "\\040")
        ),
    );
    let reader = f.reader();
    assert_eq!(reader.read().unwrap().throttled_usec, 123);
    f.put("cg space/other/cpu.max", "200000 100000\n");
    f.put("proc/self/cgroup", "0::/other\n");
    assert_eq!(reader.read().unwrap().quota.unwrap().ceil(), 2);
    f.put("proc/self/cgroup", "0::/../../escape\n");
    assert!(reader.read().is_err());
    f.put("proc/self/cgroup", "0::/missing\n");
    assert!(reader.read().is_err());
    f.put("proc/self/cgroup", "");
    assert!(reader.read().unwrap().quota.is_none());
}

#[test]
fn metrics_are_jsonl_with_actual_widths_all_signals_and_throttle_deltas() {
    let mut m = MetricsSnapshot::new(
        Width {
            requested: Cores::Fixed(8),
            effective: 2,
            pending: 8,
        },
        true,
    );
    m.lost_permits = Some(1);
    let mut w = MetricsWriter::new(Vec::new(), Duration::ZERO, 100);
    assert!(!w.emit(Duration::from_millis(999), 110, &mut m).unwrap());
    assert!(w.emit(Duration::from_secs(1), 125, &mut m).unwrap());
    assert!(w.emit(Duration::from_secs(2), 140, &mut m).unwrap());
    assert!(w.emit(Duration::from_secs(3), 3, &mut m).unwrap());
    let bytes = w.into_inner();
    assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 3);
    let lines: Vec<serde_json::Value> = bytes
        .split(|&b| b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(
        lines[0]["width"],
        serde_json::json!({"requested":8,"effective":2,"pending":8})
    );
    assert_eq!(lines[0]["throttled_usec"], 25);
    assert_eq!(lines[1]["throttled_usec"], 15);
    assert_eq!(lines[2]["throttled_usec"], 0);
    assert_eq!(lines[0]["lost_permits"], 1);
    assert_eq!(lines[0]["cpus_conflict"], true);
    assert!(lines[0]["runnable"].is_null());
    assert!(lines[0]["worker_phase"].is_null());
    let expected = [
        "width",
        "worker_phase",
        "runnable",
        "segment_ready_delay_ns",
        "role_busy",
        "hol_stall",
        "early_commits",
        "run_ahead_depth",
        "speculation_pauses",
        "cut_pause_ns",
        "gate_waits",
        "remote_frees_per_s",
        "orphaned_heaps",
        "steals_per_s",
        "migrations_per_s",
        "switches_per_op",
        "throttled_usec",
        "reclamation_pending",
        "lost_permits",
        "home_busy",
        "stop_requested",
        "cpus_conflict",
        "stall_sites",
    ];
    assert_eq!(lines[0].as_object().unwrap().len(), expected.len());
    for key in expected {
        assert!(lines[0].get(key).is_some(), "{key}");
    }
}

#[test]
fn broken_metrics_destination_is_disabled_after_first_error() {
    struct Broken;
    impl io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            unreachable!()
        }
    }
    let mut w = MetricsWriter::new(Broken, Duration::ZERO, 0);
    let mut m = MetricsSnapshot::new(
        Width {
            requested: Cores::Fixed(1),
            effective: 1,
            pending: 0,
        },
        false,
    );
    assert!(w.emit(Duration::from_secs(1), 0, &mut m).is_err());
    assert!(
        w.emit(Duration::from_secs(2), 0, &mut m)
            .unwrap_err()
            .to_string()
            .contains("previously failed")
    );
}

#[test]
fn resolution_and_metrics_leave_identity_unchanged() {
    let mut record = RunConfig::empty([0; 32], "cores@1/main".into(), LaunchKind::Main);
    let identity = record.clone().validated().unwrap().identity();
    for n in [1, 2, 8] {
        record.placement = Some(fixed(n));
        let run = record.clone().validated().unwrap();
        let r = resolve(
            run.record().placement.as_ref().unwrap(),
            &grant(),
            &limits(),
            false,
        )
        .unwrap();
        let mut m = MetricsSnapshot::new(
            Width {
                requested: r.requested,
                effective: r.target,
                pending: 0,
            },
            r.cpus_conflict,
        );
        MetricsWriter::new(Vec::new(), Duration::ZERO, 0)
            .emit(Duration::from_secs(1), 25, &mut m)
            .unwrap();
        assert_eq!(run.identity(), identity);
    }
}

#[test]
fn large_mount_tables_preserve_root_selection_and_unterminated_last_lines() {
    let f = Fixture::new();
    f.put("cg/cpu.max", "200000 100000\n");
    // Ordinary mounts dwarf the cgroup records on container hosts. Include a
    // line larger than BufReader's buffer, CRLF, and a final line without LF.
    let mut mounts = format!(
        "2 0 0:2 / /ordinary rw - tmpfs tmpfs {}\n",
        "x".repeat(20000)
    );
    mounts.push_str(&"2 0 0:2 / /ordinary rw - tmpfs tmpfs rw\n".repeat(5000));
    mounts.push_str(&format!(
        "3 0 0:3 /parent {} rw - cgroup2 cgroup rw\r\n",
        f.0.join("cg/parent").display()
    ));
    mounts.push_str(&format!(
        "4 0 0:4 / {} rw - cgroup2 cgroup rw",
        f.0.join("cg").display()
    ));
    f.put("proc/self/mountinfo", &mounts);
    let limits = f.reader().read().unwrap();
    assert_eq!(limits.quota.unwrap().ceil(), 2);
    assert_eq!(limits.effective_cpuset, cpus("0-15"));
    assert_eq!(limits.throttled_usec, 123);
}

#[test]
fn mount_discovery_rejects_invalid_utf8_after_a_usable_mount() {
    let f = Fixture::new();
    let path = f.0.join("proc/self/mountinfo");
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(b"2 0 0:2 / /ordinary rw - tmpfs tmpfs ");
    bytes.extend_from_slice(&[0xff, b'\n']);
    fs::write(&path, bytes).unwrap();
    assert!(f.reader().read().is_err());
}

#[test]
fn kernel_affinity_and_complete_root_match_proc_discovery() {
    let fast = LinuxLimits::capture().unwrap().read().unwrap();
    let table = LinuxLimits::from_proc(std::path::Path::new("/proc"))
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(fast.admission_affinity, table.admission_affinity);
    assert_eq!(fast.effective_cpuset, table.effective_cpuset);
    assert_eq!(fast.quota, table.quota);
    assert!(fast.throttled_usec <= table.throttled_usec);
}

#[test]
fn cgroup_controls_preserve_long_cpu_lists_stat_tails_and_utf8_errors() {
    let f = Fixture::new();
    let text = (0..5000u32)
        .map(|cpu| cpu.to_string())
        .collect::<Vec<_>>()
        .join(",")
        + "\n";
    f.put("cg/parent/leaf/cpuset.cpus.effective", &text);
    f.put(
        "cg/parent/leaf/cpu.stat",
        &("usage_usec 0\n".repeat(1024) + "throttled_usec 456\n"),
    );
    let limits = f.reader().read().unwrap();
    assert_eq!(limits.effective_cpuset, cpus("0-4999"));
    assert_eq!(limits.throttled_usec, 456);
    let path = f.0.join("cg/parent/leaf/cpuset.cpus.effective");
    let mut malformed = text.into_bytes();
    malformed.push(0xff);
    fs::write(&path, malformed).unwrap();
    let expected = fs::read_to_string(&path).unwrap_err();
    assert_eq!(
        f.reader().read().unwrap_err(),
        RunRefusal::Invalid {
            field: "host limits".into(),
            detail: format!("{}: {expected}", path.display()),
        }
    );
}

#[test]
fn cgroup_stat_tails_and_utf8_refusals_survive_both_read_boundaries() {
    let f = Fixture::new();
    let tail = "throttled_usec 456\n";
    let path = f.0.join("cg/parent/leaf/cpu.stat");
    for size in [255, 256, 257, 8447, 8448, 8449] {
        let text = " ".repeat(size - tail.len() - 1) + "\n" + tail;
        f.put("cg/parent/leaf/cpu.stat", &text);
        assert_eq!(f.reader().read().unwrap().throttled_usec, 456);
        let mut malformed = text.into_bytes();
        malformed.push(0xff);
        fs::write(&path, malformed).unwrap();
        let expected = fs::read_to_string(&path).unwrap_err();
        assert_eq!(
            f.reader().read().unwrap_err(),
            RunRefusal::Invalid {
                field: "host limits".into(),
                detail: format!("{}: {expected}", path.display()),
            }
        );
    }
}

#[test]
fn cgroup_search_permissions_do_not_require_directory_listing() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let expected = f.reader().read().unwrap();
    let path = f.0.join("cg/parent");
    let original = fs::metadata(&path).unwrap().permissions();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o111)).unwrap();
    let actual = f.reader().read();
    fs::set_permissions(&path, original).unwrap();
    let actual = actual.unwrap();
    assert_eq!(
        (
            actual.admission_affinity,
            actual.effective_cpuset,
            actual.quota,
            actual.throttled_usec
        ),
        (
            expected.admission_affinity,
            expected.effective_cpuset,
            expected.quota,
            expected.throttled_usec
        )
    );
}
