fn cpu_free_canonical_receipts(width: usize) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let expected = (1..=2 * width as u64)
        .map(|job| {
            let mut value = job;
            for i in 0..128 {
                value = value.wrapping_mul(6364136223846793005).wrapping_add(i);
                value ^= value >> 29;
            }
            format!("{value}u64")
        })
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        root.join("main.hir"),
        include_str!("jitter_integer.hir")
            .replace("WIDTH", &format!("{width}u32"))
            .replace("COUNT", &format!("{}u64", width * 2))
            .replace("EXPECTED_INTS", &format!("[{expected}]")),
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_canonical","version":"1"},
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
        for mode in ["live", "record", "replay"] {
            let events = root.join(format!("{backend}-{mode}.jsonl"));
            let mut command = Command::new(&executable);
            if mode == "replay" {
                command.arg("--hirsel-replay").arg(&tape);
            } else {
                command.arg("--hirsel-grants").arg(&grants);
                if mode == "record" {
                    command.arg("--hirsel-record").arg(&tape);
                }
            }
            checked(
                command
                    .arg("--hirsel-trace-events")
                    .arg(&events)
                    .output()
                    .unwrap(),
            );
            let bytes = fs::read(&events).unwrap();
            let rows: Vec<serde_json::Value> = std::str::from_utf8(&bytes)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(rows.last().unwrap()["kind"], "terminal");
            let receipts: Vec<_> = rows
                .iter()
                .filter(|row| row["outcome"]["kind"] == "cpu_control")
                .collect();
            assert_eq!(receipts.len(), 2 * (3 + 2 * width));
            for batch in receipts.chunks(3 + 2 * width) {
                assert!(matches!(
                    batch[0]["outcome"]["action"].as_u64(),
                    Some(0 | 4)
                ));
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
            if let Some(reference) = &reference {
                assert_eq!(&bytes, reference, "{backend} {mode}");
            } else {
                reference = Some(bytes);
            }
        }
    }
}
