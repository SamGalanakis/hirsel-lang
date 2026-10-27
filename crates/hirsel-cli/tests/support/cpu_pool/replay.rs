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
            Command::new(env!("CARGO_BIN_EXE_hirsel"))
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
