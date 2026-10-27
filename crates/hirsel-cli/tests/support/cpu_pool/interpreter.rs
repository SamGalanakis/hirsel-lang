fn cpu_interpreter_native_phases(width: usize) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let jobs = (1..=2 * width)
        .map(|n| format!("{n}u64"))
        .collect::<Vec<_>>()
        .join(",");
    let expected = (1..=2 * width)
        .map(|n| format!("{}u64", n + 7))
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        root.join("main.hir"),
        include_str!("interpreter.hir")
            .replace("WIDTH", &format!("{width}u32"))
            .replace("JOBS", &format!("[{jobs}]"))
            .replace("EXPECTED", &format!("[{expected}]")),
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_interpreter_phases","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"placement":{"cores":1,"reorder":"auto"},
        "io":{"dirs":[],"tasks":"tasks.free@1","cpu":{"provider":"cpu.scoped@1",
        "max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
    let mut reference = None;
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--fuel", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let tape = root.join(format!("{backend}.tape"));
        let native_projection = root.join(format!("{backend}-native.projection"));
        let events = root.join(format!("{backend}.events"));
        checked(
            Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(&grants)
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-trace-events")
                .arg(&events)
                .env("HIRSEL_RT_FREE_PROJECTION", &native_projection)
                .output()
                .unwrap(),
        );
        let canonical = fs::read(&events).unwrap();
        if let Some(expected) = &reference {
            assert_eq!(&canonical, expected, "{backend}");
        } else {
            reference = Some(canonical);
        }
        let replay_projection = root.join(format!("{backend}-replay.projection"));
        checked(
            Command::new(&executable)
                .arg("--hirsel-replay")
                .arg(&tape)
                .env("HIRSEL_RT_FREE_PROJECTION", &replay_projection)
                .output()
                .unwrap(),
        );
        assert_eq!(
            fs::read(&native_projection).unwrap(),
            fs::read(&replay_projection).unwrap()
        );
        let tir_projection = root.join(format!("{backend}-tir.projection"));
        checked(
            Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .arg(&manifest)
                .arg("--replay")
                .arg(&tape)
                .env("HIRSEL_RT_FREE_PROJECTION", &tir_projection)
                .output()
                .unwrap(),
        );
        assert_eq!(
            fs::read(&native_projection).unwrap(),
            fs::read(&tir_projection).unwrap(),
            "TIR {backend} W{width}"
        );
    }
}
