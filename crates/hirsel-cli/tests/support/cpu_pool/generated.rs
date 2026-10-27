static QUAL_COMPILER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn qual_manifest(root: &std::path::Path, source: &str) -> std::path::PathBuf {
    fs::write(root.join("main.hir"), source).unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_qualification","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    manifest
}

fn qual_build(
    root: &std::path::Path,
    manifest: &std::path::Path,
    backend: &str,
) -> std::path::PathBuf {
    let executable = root.join(backend);
    checked(
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--fuel", "--validate-stages", "--backend", backend])
            .arg(manifest)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap(),
    );
    executable
}

fn qual_grants(width: usize) -> serde_json::Value {
    serde_json::json!({"io":{"dirs":[],"tasks":"tasks.free@1",
        "cpu":{"provider":"cpu.scoped@1","max_workers":width,"max_domains":1,
        "max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}})
}

fn qual_observation(path: &std::path::Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("placement "))
        .map(str::to_owned)
        .collect()
}

fn generated_free_schedules(width: usize) {
    let _compiler = QUAL_COMPILER.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let mut counts = vec![0, 1, width.saturating_sub(1), width, width + 1, 2 * width];
    counts.sort_unstable();
    counts.dedup();
    let expected = (0..4u64)
        .map(|seed| {
            let values = (0..2 * width as u64)
                .map(|ordinal| {
                    let mut x = ordinal.wrapping_add(seed);
                    for i in 0..(ordinal ^ seed) * 37 + 1 {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
                        x ^= x >> 29;
                    }
                    format!("{x}u64")
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("[{values}]")
        })
        .collect::<Vec<_>>()
        .join(",");
    let source = include_str!("generated.hir")
        .replace("WIDTH", &format!("{width}u32"))
        .replace(
            "COUNTS",
            &format!(
                "[{}]",
                counts
                    .iter()
                    .map(|n| format!("{n}u64"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        )
        .replace("EXPECTED", &format!("[{expected}]"));
    let manifest = qual_manifest(root, &source);
    let grants = root.join("grants.json");
    fs::write(&grants, qual_grants(width).to_string()).unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = qual_build(root, &manifest, backend);
        for seed in [1, 7, 29] {
            let tape = root.join("native.tape");
            let observation = root.join("native.observation");
            let probe = root.join("native.probe");
            let live = checked(
                Command::new(&executable)
                    .arg("--hirsel-grants")
                    .arg(&grants)
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .env("HIRSEL_RT_CPU_JITTER", seed.to_string())
                    .env("HIRSEL_RT_CPU_PROBE", &probe)
                    .env("HIRSEL_RT_OBSERVATION", &observation)
                    .output()
                    .unwrap(),
            );
            let parsed = hirsel_free::tape::parse(&fs::read(&tape).unwrap()).unwrap();
            assert!(!parsed.torn && parsed.end.is_some());
            assert!(parsed.logs.len() > 1 && parsed.cut.len() > 1);
            let observed = qual_observation(&observation);
            assert!(!observed.is_empty());
            let replay = checked(
                Command::new(&executable)
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .env("HIRSEL_RT_WORKERS", "1")
                    .env("HIRSEL_RT_CPU_PROBE", &probe)
                    .env("HIRSEL_RT_OBSERVATION", &observation)
                    .output()
                    .unwrap(),
            );
            assert_eq!(replay.stdout, live.stdout);
            assert_eq!(replay.stderr, live.stderr);
            assert_eq!(qual_observation(&observation), observed);
            assert!(fs::read(&probe).unwrap().is_empty());
            assert!(
                fs::read(root.join("native.probe.lifecycle"))
                    .unwrap()
                    .is_empty()
            );
            eprintln!(
                "Free generated {backend} W{width} jitter={seed}: {} batches, {} typed results, live/replay matched",
                4 * counts.len(),
                4 * counts.iter().sum::<usize>()
            );
        }
    }
}

fn typed_preparation_prefix_census(width: usize) {
    let _compiler = QUAL_COMPILER.lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let count = 2 * width;
    let template = include_str!("prefix.hir");
    let prepare_line = template
        .lines()
        .position(|line| line.contains("failure.check(i != prefix"))
        .unwrap()
        + 1;
    let source = template
        .replace("PREPARE_LINE", &format!("{prepare_line}u64"))
        .replace("WIDTH", &format!("{width}u32"))
        .replace("COUNT", &format!("{count}u64"))
        .replace("LIMIT", &format!("{}u64", count + 1));
    let manifest = qual_manifest(root, &source);
    let grants = root.join("grants.json");
    fs::write(&grants, qual_grants(width).to_string()).unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = qual_build(root, &manifest, backend);
        let tape = root.join("prefix.tape");
        let live = checked(
            Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(&grants)
                .arg("--hirsel-record")
                .arg(&tape)
                .output()
                .unwrap(),
        );
        let probe = root.join("prefix.probe");
        let replay = checked(
            Command::new(&executable)
                .arg("--hirsel-replay")
                .arg(&tape)
                .env("HIRSEL_RT_WORKERS", "1")
                .env("HIRSEL_RT_CPU_PROBE", &probe)
                .output()
                .unwrap(),
        );
        assert_eq!(live.stdout, replay.stdout);
        assert_eq!(live.stderr, replay.stderr);
        assert!(fs::read(&probe).unwrap().is_empty());
        eprintln!(
            "Free {backend} W{width}: preparation prefixes 0..={count}, {} independent row roots thawed after cleanup, one-permit replay matched",
            count * (count + 1)
        );
    }
}
