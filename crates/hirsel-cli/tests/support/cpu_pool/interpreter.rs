fn cpu_interpreter_native_phases(width: usize) {
    cpu_interpreter_native_policy(width, true);
}

fn cpu_interpreter_ordered_receipts(width: usize) {
    cpu_interpreter_native_policy(width, false);
}

fn cpu_interpreter_native_policy(width: usize, free: bool) {
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
        "io":{"stdout":"stdout.bytes@1","dirs":[],"tasks":if free { "tasks.free@1" } else { "tasks.cooperative@1" },"cpu":{"provider":"cpu.scoped@1",
        "max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
    let interpreted_tape = root.join("interpreted.tape");
    if !free {
        checked(
            Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .arg(&manifest)
                .arg("--grants")
                .arg(&grants)
                .arg("--record")
                .arg(&interpreted_tape)
                .output()
                .unwrap(),
        );
    }
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
        if !free {
            checked(
                Command::new(&executable)
                    .arg("--hirsel-replay")
                    .arg(&interpreted_tape)
                    .output()
                    .unwrap(),
            );
        }
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
        let rows: Vec<serde_json::Value> = std::str::from_utf8(&canonical)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let controls: Vec<_> = rows
            .iter()
            .filter(|row| row["outcome"]["kind"] == "cpu_control")
            .collect();
        assert_eq!(controls.len(), 2 * (3 + 2 * width));
        for batch in controls.chunks(3 + 2 * width) {
            assert_eq!(batch[0]["outcome"]["action"], 4);
            assert_eq!(batch[1]["outcome"]["action"], 2);
            assert_eq!(batch[1]["outcome"]["count"], 2 * width);
            for (ordinal, row) in batch[2..2 + 2 * width].iter().enumerate() {
                assert_eq!(row["outcome"]["action"], 5);
                assert_eq!(row["outcome"]["count"], ordinal);
            }
            assert_eq!(batch.last().unwrap()["outcome"]["action"], 1);
            for row in batch {
                assert_eq!(row["outcome"]["width"], width);
                assert_eq!(row["outcome"]["outcome"], 0);
            }
        }
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
        if free {
            assert_eq!(
                fs::read(&native_projection).unwrap(),
                fs::read(&replay_projection).unwrap()
            );
        }
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
        if free {
            assert_eq!(
                fs::read(&native_projection).unwrap(),
                fs::read(&tir_projection).unwrap(),
                "TIR {backend} W{width}"
            );
        }
    }
}
