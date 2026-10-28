fn user_manager(program: &str) -> Command {
    let mut command = Command::new(program);
    let uid = unsafe { libc::getuid() };
    command
        .arg("--user")
        .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path=/run/user/{uid}/bus"),
        );
    command
}

struct ContractionUnit(String);
impl Drop for ContractionUnit {
    fn drop(&mut self) {
        let _ = user_manager("systemctl").args(["stop", &self.0]).output();
    }
}

fn cpu_controls(path: &std::path::Path) -> Vec<[u64; 5]> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .map(|line| {
            let mut fields = line.split_whitespace();
            assert_eq!(fields.next(), Some("control"));
            let values: Vec<u64> = fields.map(|field| field.parse().unwrap()).collect();
            values.try_into().unwrap()
        })
        .collect()
}

fn host_quota_contraction() {
    let _compiler = QUAL_COMPILER.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let manifest = qual_manifest(
        root,
        r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let outcome = core@1/cpu.map_scoped(tasks, 4u32,
    fn() -> List[u64] uses {} { [0u64, 1u64, 2u64, 3u64, 4u64, 5u64, 6u64, 7u64] },
    fn(take job: u64) -> u64 uses {} {
      let mut x = job
      for i in 0u64..3000000u64 { x = int.wrapping_add(int.wrapping_mul(x, 6364136223846793005u64), i) }
      x
    })
  failure.check(outcome == Error(CapacityLost()), Bounds())
  Ok(())
}
"#,
    );
    let grants = root.join("grants.json");
    fs::write(&grants, qual_grants(4).to_string()).unwrap();
    let manager = checked(
        user_manager("systemctl")
            .args(["show", "--property=ControlGroup", "--value"])
            .output()
            .unwrap(),
    );
    let manager = String::from_utf8(manager.stdout).unwrap();
    let mut allowed = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
    assert_eq!(
        unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(&allowed), &mut allowed) },
        0
    );
    let cpus = (0..libc::CPU_SETSIZE as usize)
        .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &allowed) })
        .map(|cpu| cpu.to_string())
        .collect::<Vec<_>>()
        .join(",");
    for backend in ["llvm", "cranelift"] {
        let executable = qual_build(root, &manifest, backend);
        let unit = ContractionUnit(format!(
            "pool-qual-quota-{}-{backend}.service",
            std::process::id()
        ));
        let group = std::path::Path::new("/sys/fs/cgroup")
            .join(manager.trim().trim_start_matches('/'))
            .join("app.slice")
            .join(&unit.0);
        let tape = root.join("contraction.tape");
        let probe = root.join("contraction.probe");
        let controls = root.join("contraction.probe.control");
        let projection = root.join("contraction.projection");
        let observation = root.join("contraction.observation");
        // Replay leaves semantic job rows, so the next emitter starts empty.
        fs::write(root.join("contraction.probe.jobs"), "").unwrap();
        let monitor = std::thread::spawn({
            let (controls, group, jobs) = (
                controls.clone(),
                group.clone(),
                root.join("contraction.probe.jobs"),
            );
            move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    let rows = cpu_controls(&controls);
                    let events = fs::read_to_string(&jobs).unwrap_or_default();
                    let claim = events
                        .split_inclusive('\n')
                        .filter(|line| line.ends_with('\n'))
                        .find_map(|line| {
                            let fields: Vec<_> = line.split_whitespace().collect();
                            (fields.len() == 6 && fields[0] == "claim")
                                .then(|| (fields[1].to_owned(), fields[5].to_owned()))
                        });
                    if let Some(claim) = claim {
                        struct Thaw(std::path::PathBuf);
                        impl Drop for Thaw {
                            fn drop(&mut self) {
                                let _ = fs::write(&self.0, "0");
                            }
                        }
                        // Keep the claimed mapper live across one refresh
                        // interval without spending more fuel or CPU budget.
                        let thaw = Thaw(group.join("cgroup.freeze"));
                        fs::write(&thaw.0, "1").unwrap();
                        while !fs::read_to_string(group.join("cgroup.events"))
                            .unwrap()
                            .lines()
                            .any(|line| line == "frozen 1")
                        {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "owned CPU cgroup did not freeze"
                            );
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                        let before = fs::read_to_string(group.join("cpu.max")).unwrap();
                        assert_eq!(before.trim(), "400000 100000");
                        fs::write(group.join("cpu.max"), "100000 100000").unwrap();
                        assert_eq!(
                            fs::read_to_string(group.join("cpu.max")).unwrap().trim(),
                            "100000 100000"
                        );
                        let after = fs::read_to_string(&jobs).unwrap();
                        assert!(
                            !after
                                .split_inclusive('\n')
                                .filter(|line| line.ends_with('\n'))
                                .any(|line| {
                                    let fields: Vec<_> = line.split_whitespace().collect();
                                    matches!(fields[0], "transfer" | "withdrawal" | "retire")
                                        && fields[1] == claim.0
                                }),
                            "claimed mapper finished before owned quota contraction: {after}"
                        );
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        drop(thaw);
                        return (
                            claim,
                            rows,
                            fs::read_to_string(group.join("cpu.stat")).unwrap(),
                        );
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "no actual mapper claim in owned cgroup {group:?}"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        });
        let mut command = user_manager("systemd-run");
        command
            .args([
                "--wait",
                "--pipe",
                "--quiet",
                "--collect",
                "--property=Slice=app.slice",
                "--property=Delegate=yes",
                "--property=CPUQuota=400%",
                "--property=MemoryMax=2G",
                "--property=RuntimeMaxSec=15s",
            ])
            .arg(format!("--unit={}", unit.0))
            .arg(format!("--setenv=HIRSEL_RT_CPU_PROBE={}", probe.display()))
            .arg(format!(
                "--setenv=HIRSEL_RT_OBSERVATION={}",
                observation.display()
            ))
            .arg(format!(
                "--setenv={}={}",
                hirsel_rt::free_replay::Report::VAR,
                projection.display()
            ))
            .args(["taskset", "-c", &cpus])
            .arg(&executable)
            .arg("--hirsel-grants")
            .arg(&grants)
            .arg("--hirsel-record")
            .arg(&tape);
        let live = qual_native_output(&mut command, root);
        checked(live);
        let (claim, admitted, stats) = monitor.join().unwrap();
        let jobs = cpu_job_stages(&root.join("contraction.probe.jobs"));
        assert!(jobs.contains_key(&claim.1));
        let rows = cpu_controls(&controls);
        assert!(
            rows.iter().any(|row| row[2] == 1 && row[4] == 8),
            "drain must report CapacityLost: {rows:?}"
        );
        let mut lost = false;
        for row in &rows {
            if row[4] == 8 {
                lost = true;
            }
            if lost && matches!(row[2], 3 | 5) {
                assert_ne!(
                    row[4], 0,
                    "a mapper claimed after contraction was observed: {rows:?}"
                );
            }
        }
        check_cpu_lifecycle(&root.join("contraction.probe.lifecycle"), 4, false, 1);
        let observed = qual_observation(&observation);
        let projected = fs::read(&projection).unwrap();
        let parsed = hirsel_free::tape::parse(&fs::read(&tape).unwrap()).unwrap();
        assert!(!parsed.torn && parsed.end.is_some());
        let mut replay = Command::new(&executable);
        replay
            .arg("--hirsel-replay")
            .arg(&tape)
            .env("HIRSEL_RT_WORKERS", "1")
            .env("HIRSEL_RT_CPU_PROBE", &probe)
            .env("HIRSEL_RT_OBSERVATION", &observation)
            .env(hirsel_rt::free_replay::Report::VAR, &projection);
        checked(qual_native_output(&mut replay, root));
        assert_eq!(qual_observation(&observation), observed);
        assert_eq!(fs::read(&projection).unwrap(), projected);
        assert!(fs::read(&probe).unwrap().is_empty());
        assert!(
            fs::read(root.join("contraction.probe.lifecycle"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(cpu_job_stages(&root.join("contraction.probe.jobs")), jobs);
        let semantic_controls = |rows: Vec<[u64; 5]>| {
            let mut rows: Vec<_> = rows.into_iter().map(|row| row[1..].to_vec()).collect();
            rows.sort_unstable();
            rows
        };
        assert_eq!(
            semantic_controls(cpu_controls(&controls)),
            semantic_controls(rows.clone())
        );
        eprintln!(
            "Free {backend} host quota 4->1 after actual claim {claim:?}, {} jobs retired, admitted={admitted:?}, controls={rows:?}, {stats}, all 4 workers drained; one-permit replay matched",
            jobs.len()
        );
    }
}
