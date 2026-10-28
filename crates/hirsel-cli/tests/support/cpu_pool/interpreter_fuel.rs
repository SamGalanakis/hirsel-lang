fn cpu_native_fuel_ends(bytes: &[u8]) -> Vec<(Vec<u32>, u64, String)> {
    let mut seen = std::collections::BTreeSet::new();
    // Canonical rows describe a retired prefix. The source endpoint is its
    // next charge, numbered from one; both adjacent budgets are tested below.
    let mut fuel = 1;
    let mut ends = vec![];
    for line in std::str::from_utf8(bytes).unwrap().lines() {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        if seen.insert((
            row["task"].to_string(),
            row["ordinal"].as_u64().unwrap(),
            row["retirement"].as_u64().unwrap(),
        )) {
            fuel += row["before"]
                .as_array()
                .unwrap()
                .iter()
                .map(|piece| piece["fuel"].as_u64().unwrap())
                .sum::<u64>();
        }
        if row["outcome"]["kind"] == "task_end" && !row["outcome"]["result"].is_null() {
            let text = row["outcome"]["result"].as_str().unwrap();
            let payload = (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
                .collect();
            ends.push((
                serde_json::from_value(row["task"].clone()).unwrap(),
                fuel,
                format!("{:?}", hirsel_interp::library::CpuPhase::End(payload)),
            ));
        }
    }
    ends
}
fn cpu_tir_fuel_ends(report: &serde_json::Value) -> Vec<(Vec<u32>, u64, String)> {
    report["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["end"] == true)
        .map(|row| {
            (
                serde_json::from_value(row["task"].clone()).unwrap(),
                row["fuel"].as_u64().unwrap(),
                row["phase"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}
fn cpu_interpreter_fuel_cuts(width: usize) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let jobs = (1..=2 * width)
        .map(|n| format!("{n}u64"))
        .collect::<Vec<_>>()
        .join(",");
    let expected = (1..=2 * width)
        .map(|n| format!("{}u64", n + n * (n - 1) / 2))
        .collect::<Vec<_>>()
        .join(",");
    let source = include_str!("interpreter.hir")
        .replace("WIDTH", &format!("{width}u32"))
        .replace("JOBS", &format!("[{jobs}]"))
        .replace("EXPECTED", &format!("[{expected}]"))
        .replace(
            "{ job + 7u64 }",
            "{ let mut x = job; for i in 0u64..job { x = int.wrapping_add(x, i) }; x }",
        );
    fs::write(root.join("main.hir"), source).unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest,serde_json::json!({"schema":1,"package":{"name":"cpu_fuel_cuts","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    let mut authority = serde_json::json!({"budgets":{"provider":"budgets.limits@2","fuel":100000},"io":{"dirs":[],"tasks":"tasks.cooperative@1","stdout":"stdout.bytes@1","cpu":{"provider":"cpu.scoped@1","max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}});
    fs::write(&grants, authority.to_string()).unwrap();
    let tir_report = root.join("tir.fuel");
    checked(
        cpu_interpreter_cli(width, true)
            .arg("run")
            .arg(&manifest)
            .arg("--grants")
            .arg(&grants)
            .env("HIRSEL_TIR_CPU_FUEL", &tir_report)
            .output()
            .unwrap(),
    );
    let tir_reference =
        cpu_tir_fuel_ends(&serde_json::from_slice(&fs::read(&tir_report).unwrap()).unwrap());
    assert_eq!(tir_reference.len(), 4 * width);
    let tape = root.join("cut.tape");
    let events = root.join("native.events");
    let mut native_reference = None;
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            cpu_interpreter_cli(width, true)
                .args(["build", "--fuel", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        authority["budgets"]["fuel"] = serde_json::json!(100000);
        fs::write(&grants, authority.to_string()).unwrap();
        checked(
            Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(&grants)
                .arg("--hirsel-trace-events")
                .arg(&events)
                .output()
                .unwrap(),
        );
        let native_ends = cpu_native_fuel_ends(&fs::read(&events).unwrap());
        assert_eq!(native_ends.len(), 4 * width);
        if let Some(reference) = &native_reference {
            assert_eq!(&native_ends, reference);
        } else {
            native_reference = Some(native_ends.clone());
        }
        for ordinal in 0..2 * width {
            assert_eq!(tir_reference[ordinal].0, native_ends[ordinal].0);
            assert_eq!(tir_reference[ordinal].2, native_ends[ordinal].2);
            for admitted in [false, true] {
                for (native, cost) in [
                    (false, tir_reference[ordinal].1),
                    (true, native_ends[ordinal].1),
                ] {
                    let budget = cost - u64::from(!admitted);
                    assert!(budget > 0);
                    authority["budgets"]["fuel"] = serde_json::json!(budget);
                    fs::write(&grants, authority.to_string()).unwrap();
                    let report = root.join("cut.fuel");
                    let mut previous = None;
                    for replay in [false, true] {
                        let mut command = if native {
                            Command::new(&executable)
                        } else {
                            let mut c = cpu_interpreter_cli(width, true);
                            c.arg("run")
                                .arg(&manifest)
                                .env("HIRSEL_TIR_CPU_FUEL", &report);
                            c
                        };
                        if native {
                            command.env("HIRSEL_RT_BOUNDED_STOP_TRACE", "1");
                            command.arg("--hirsel-trace-events").arg(&events);
                        }
                        if replay {
                            command
                                .arg(if native {
                                    "--hirsel-replay"
                                } else {
                                    "--replay"
                                })
                                .arg(&tape);
                        } else {
                            command
                                .arg(if native {
                                    "--hirsel-grants"
                                } else {
                                    "--grants"
                                })
                                .arg(&grants)
                                .arg(if native {
                                    "--hirsel-record"
                                } else {
                                    "--record"
                                })
                                .arg(&tape);
                        }
                        let output = command.output().unwrap();
                        assert_eq!(
                            output.status.code(),
                            Some(hirsel_run_schema::exit::BUDGET_EXCEEDED),
                            "W{width} {backend} native={native} job={ordinal} budget={budget} admitted={admitted} replay={replay}: {output:?}"
                        );
                        assert!(output.stdout.is_empty());
                        let (ends, stop_task) = if native {
                            let bytes = fs::read(&events).unwrap();
                            let rows: Vec<serde_json::Value> = std::str::from_utf8(&bytes)
                                .unwrap()
                                .lines()
                                .map(|line| serde_json::from_str(line).unwrap())
                                .collect();
                            let stopped = rows
                                .iter()
                                .find(|row| row["outcome"]["kind"] == "stopped")
                                .unwrap();
                            (cpu_native_fuel_ends(&bytes), stopped["task"].clone())
                        } else {
                            let report: serde_json::Value =
                                serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
                            (cpu_tir_fuel_ends(&report), report["task"].clone())
                        };
                        let actual: Vec<_> = ends
                            .into_iter()
                            .map(|(task, _, phase)| (task, phase))
                            .collect();
                        let expected: Vec<_> = tir_reference[..ordinal + usize::from(admitted)]
                            .iter()
                            .map(|(task, _, phase)| (task.clone(), phase.clone()))
                            .collect();
                        assert_eq!(
                            actual, expected,
                            "W{width} {backend} native={native} job={ordinal} admitted={admitted} replay={replay}"
                        );
                        if !admitted {
                            assert_eq!(stop_task, serde_json::json!(tir_reference[ordinal].0));
                        }
                        if let Some(previous) = &previous {
                            assert_eq!(&(actual.clone(), stop_task.clone()), previous);
                        } else {
                            previous = Some((actual, stop_task));
                        }
                    }
                }
            }
        }
        eprintln!(
            "Ordered W{width} {backend}: {} exact source completion fuel cuts across TIR/native, record/replay, using each engine's specified charge count",
            2 * width * 2 * 2 * 2
        );
    }
}
