fn replay_width(width: usize) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        include_str!("replay.hir")
            .replace("WIDTH", &format!("{width}u32"))
            .replace("COUNT", &format!("{}u64", 2 * width)),
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_wide_replay","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"tasks":"tasks.free@1","cpu":{"provider":"cpu.scoped@1",
        "max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            cpu_compiler()
                .args(["build", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let tape = root.join(format!("{backend}.tape"));
        checked(
            Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(&grants)
                .arg("--hirsel-record")
                .arg(&tape)
                .output()
                .unwrap(),
        );
        let probe = root.join("replay-probe");
        let mut replay = Command::new(&executable);
        replay
            .arg("--hirsel-replay")
            .arg(&tape)
            .env("HIRSEL_RT_CPU_PROBE", &probe)
            .env("HIRSEL_RT_WORKERS", "1");
        use std::os::unix::process::CommandExt;
        unsafe {
            replay.pre_exec(|| {
                let mut allowed = std::mem::zeroed::<libc::cpu_set_t>();
                if libc::sched_getaffinity(0, std::mem::size_of_val(&allowed), &mut allowed) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let cpu = (0..libc::CPU_SETSIZE as usize)
                    .find(|&cpu| libc::CPU_ISSET(cpu, &allowed))
                    .ok_or_else(|| std::io::Error::other("empty replay affinity"))?;
                let mut selected = std::mem::zeroed::<libc::cpu_set_t>();
                libc::CPU_SET(cpu, &mut selected);
                if libc::sched_setaffinity(0, std::mem::size_of_val(&selected), &selected) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        checked(replay.output().unwrap());
        assert!(
            fs::read_to_string(probe).unwrap().is_empty(),
            "replay cannot execute CPU pool workers"
        );
    }
}

fn ordered_replay_width(width: usize) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let source = include_str!("physical.hir")
        .replace("3000000u64", "2048u64")
        .replace("WIDTH64", &format!("{}u64", 2 * width))
        .replace("WIDTH", &format!("{width}u32"));
    let mut expected = vec![];
    for q in 1..=2 * width as u64 {
        let mut x = q;
        for i in 0..2048 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
            x ^= x >> 29;
        }
        expected.push(format!("{x}u64"));
    }
    fs::write(
        root.join("main.hir"),
        source.replace("EXPECTED", &format!("[{}]", expected.join(","))),
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_ordered_replay","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            cpu_compiler()
                .args(["build", "--fuel", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let mut reference = None;
        for reorder in ["off", "auto"] {
            fs::write(&grants, serde_json::json!({"placement":{"cores":1,"reorder":reorder},
                "io":{"dirs":[],"tasks":"tasks.cooperative@1","cpu":{"provider":"cpu.scoped@1",
                "max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
            let tape = root.join(format!("{backend}-{reorder}.tape"));
            let trace = root.join("live.events");
            let probe = root.join("ordered.probe");
            checked(
                Command::new(&executable)
                    .env("HIRSEL_RT_CPU_PROBE", &probe)
                    .arg("--hirsel-grants")
                    .arg(&grants)
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .arg("--hirsel-trace-events")
                    .arg(&trace)
                    .output()
                    .unwrap(),
            );
            let jobs = cpu_job_stages(&root.join("ordered.probe.jobs"));
            assert_eq!(jobs.len(), 2 * width);
            let mut recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
            let binding = recording.binding.as_mut().unwrap();
            binding.admitted_at = 0;
            binding.placement = None;
            let live = (fs::read(&trace).unwrap(), recording);
            if let Some(reference) = &reference {
                assert_eq!(&live, reference, "{backend} W{width} {reorder}");
            } else {
                reference = Some(live);
            }
            let replayed = root.join("replay.events");
            let probe = root.join("replay.probe");
            checked(
                Command::new(&executable)
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .arg("--hirsel-trace-events")
                    .arg(&replayed)
                    .env("HIRSEL_RT_WORKERS", "1")
                    .env("HIRSEL_RT_CPU_PROBE", &probe)
                    .output()
                    .unwrap(),
            );
            assert_eq!(fs::read(&replayed).unwrap(), reference.as_ref().unwrap().0);
            assert_eq!(cpu_job_stages(&root.join("replay.probe.jobs")), jobs);
            assert!(fs::read(&probe).unwrap().is_empty());
        }
        eprintln!(
            "D02 {backend} W{width}: serial off == physical auto == one-permit replay, complete canonical trace and projected tape"
        );
    }
}
