fn cpu_interpreter_native_phases(width: usize) {
    cpu_interpreter_native_policy(width, true);
}

fn cpu_interpreter_ordered_receipts(width: usize) {
    cpu_interpreter_native_policy(width, false);
}

fn cpu_interpreter_native_policy(width: usize, free: bool) {
    let jobs = (1..=2 * width)
        .map(|n| format!("{n}u64"))
        .collect::<Vec<_>>()
        .join(",");
    let expected = (1..=2 * width)
        .map(|n| format!("{}u64", n + 7))
        .collect::<Vec<_>>()
        .join(",");
    let source = include_str!("interpreter.hir")
        .replace("WIDTH", &format!("{width}u32"))
        .replace("JOBS", &format!("[{jobs}]"))
        .replace("EXPECTED", &format!("[{expected}]"));
    cpu_interpreter_matrix(width, free, &source, true);
}

fn cpu_interpreter_matrix(width: usize, free: bool, source: &str, scalar: bool) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(root.join("main.hir"), source).unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_interpreter_phases","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"placement":{"cores":1,"reorder":"auto"},
        "io":{"stdout":"stdout.bytes@1","dirs":[],"tasks":if free { "tasks.free@1" } else { "tasks.cooperative@1" },"cpu":{"provider":"cpu.scoped@1",
        "max_workers":width,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
    let interpreted_tape = root.join("interpreted.tape");
    {
        checked(
            cpu_interpreter_cli(width, scalar)
                .arg("run")
                .arg(&manifest)
                .arg("--grants")
                .arg(&grants)
                .arg("--record")
                .arg(&interpreted_tape)
                .env(
                    "HIRSEL_RT_FREE_PROJECTION",
                    root.join("interpreted.projection"),
                )
                .env("HIRSEL_TIR_CPU_PHASES", root.join("tir.phases"))
                .output()
                .unwrap(),
        );
    }
    let mut reference = None;
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            cpu_interpreter_cli(width, scalar)
                .args(["build", "--fuel", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let interpreted_projection = root.join("interpreted.projection");
        if free && !interpreted_projection.exists() {
            panic!("missing live TIR Free projection");
        }
        let cross_projection = root.join(format!("{backend}-from-tir.projection"));
        checked(
            Command::new(&executable)
                .arg("--hirsel-replay")
                .arg(&interpreted_tape)
                .env("HIRSEL_RT_FREE_PROJECTION", &cross_projection)
                .output()
                .unwrap(),
        );
        if free {
            assert_eq!(
                fs::read(&interpreted_projection).unwrap(),
                fs::read(&cross_projection).unwrap(),
                "live TIR to {backend} W{width}"
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
        eprintln!("Ordered/Free={free} W{width} {backend}: native recording complete");
        let canonical = fs::read(&events).unwrap();
        if !free {
            let expected = cpu_ordered_boundaries(&canonical);
            let expected = expected
                .iter()
                .map(|event| format!("{event:?}\n"))
                .collect::<String>();
            let actual = fs::read_to_string(root.join("tir.phases")).unwrap();
            let actual: Vec<_> = actual.lines().collect();
            let expected: Vec<_> = expected.lines().collect();
            for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                assert_eq!(actual, expected, "Ordered W{width} {backend} event {index}");
            }
            assert_eq!(
                actual.len(),
                expected.len(),
                "Ordered W{width} {backend} event count"
            );
        }
        let rows: Vec<serde_json::Value> = std::str::from_utf8(&canonical)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let controls: Vec<_> = rows
            .iter()
            .filter(|row| row["outcome"]["kind"] == "cpu_control")
            .collect();
        if scalar {
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
            cpu_interpreter_cli(width, scalar)
                .arg("run")
                .arg(&manifest)
                .arg("--replay")
                .arg(&tape)
                .env("HIRSEL_RT_FREE_PROJECTION", &tir_projection)
                .env("HIRSEL_TIR_CPU_PHASES", root.join("replayed.phases"))
                .output()
                .unwrap(),
        );
        if !free {
            assert_eq!(
                fs::read(root.join("tir.phases")).unwrap(),
                fs::read(root.join("replayed.phases")).unwrap(),
                "independent Ordered replay W{width} {backend}"
            );
        }
        if free {
            assert_eq!(
                fs::read(&native_projection).unwrap(),
                fs::read(&tir_projection).unwrap(),
                "TIR {backend} W{width}"
            );
        }
    }
}

fn cpu_ordered_boundaries(events: &[u8]) -> Vec<hirsel_interp::library::CpuBoundary> {
    use hirsel_interp::library::{CpuBoundary, CpuControl, CpuPhase};
    let mut boundaries = vec![];
    let mut paths = std::collections::BTreeMap::new();
    let mut active = std::collections::BTreeSet::<Vec<u32>>::new();
    for line in std::str::from_utf8(events).unwrap().lines() {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        let outcome = &row["outcome"];
        let task: Vec<u32> = serde_json::from_value(row["task"].clone()).unwrap();
        let control = outcome["kind"] == "cpu_control";
        if control && matches!(outcome["action"].as_u64(), Some(0 | 4)) && outcome["outcome"] == 0 {
            assert!(active.insert(task.clone()));
        }
        if !control && !active.iter().any(|parent| task.starts_with(parent)) {
            continue;
        }
        let id = |value: &serde_json::Value| value.to_string();
        let payload = |value: &serde_json::Value| {
            let text = value.as_str().expect("initialized scalar payload");
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
                .collect::<Vec<_>>()
        };
        let phase = match outcome["kind"].as_str().unwrap() {
            "cpu_control" => Some(CpuPhase::Control(
                CpuControl {
                    width: outcome["width"].as_u64().unwrap() as u32,
                    action: outcome["action"].as_u64().unwrap() as u32,
                    count: outcome["count"].as_u64().unwrap(),
                },
                outcome["outcome"].as_u64().unwrap() as u32,
            )),
            "created" if row["op"] == "task.scope_enter" => Some(CpuPhase::ScopeEnter),
            "spawned" => {
                let path: Vec<u32> = serde_json::from_value(outcome["path"].clone()).unwrap();
                assert!(paths.insert(id(&row["item"]["id"]), path.clone()).is_none());
                Some(CpuPhase::Spawn(path))
            }
            "joining" => Some(CpuPhase::Join(paths[&id(&row["item"]["id"])].clone())),
            "joined" => {
                assert_eq!(outcome["status"], 0);
                assert_eq!(outcome["failures"], serde_json::json!([]));
                Some(CpuPhase::Joined(
                    paths[&id(&row["item"]["id"])].clone(),
                    payload(&outcome["payload"]),
                ))
            }
            "task_end" if !outcome["result"].is_null() => {
                Some(CpuPhase::End(payload(&outcome["result"])))
            }
            "task_end" => Some(CpuPhase::Rest),
            "scope_exiting" => Some(CpuPhase::ScopeExit),
            "scope_exited" => {
                assert_eq!(outcome["status"], 0);
                assert_eq!(outcome["expired"], false);
                assert_eq!(outcome["failures"], serde_json::json!([]));
                Some(CpuPhase::ScopeExited)
            }
            _ => None,
        };
        if control && outcome["action"] == 1 {
            assert!(active.remove(&task));
        }
        if let Some(phase) = phase {
            boundaries.push(CpuBoundary { task, phase });
        }
    }
    assert!(!boundaries.is_empty());
    boundaries
}

fn cpu_interpreter_cancel_cuts(width: usize) {
    let count = 2 * width;
    let jobs = (0..count)
        .map(|n| format!("{n}u64"))
        .collect::<Vec<_>>()
        .join(",");
    let cuts = 2 * count + 10;
    let source = format!(
        r#"
pub fn main(rt: Io) -> Result[(), GrantError] {{
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  for cut in 0u64..{cuts}u64 {{
    task.scope(tasks, fn(scope: Scope) -> () {{
      let child = task.async(scope, fn() -> bool {{
        let batch = core@1/cpu.map_scoped(tasks, {width}u32,
          fn() -> List[u64] uses {{}} {{ [{jobs}] }},
          fn(take job: u64) -> u64 uses {{}} {{ job + 7u64 }})
        match batch {{
          Ok(rows) => {{ failure.check(list.len(rows) == {count}u64, Bounds()); true }},
          Error(Canceled(Requested())) => false,
          Error(WidthUnavailable()) => {{ failure.check(false, Bounds()); false }},
          _ => {{ failure.check(false, Arithmetic()); false }},
        }}
      }})
      for tick in 0u64..cut {{ let _ = task.check_canceled(scope) }}
      task.cancel(scope, child)
      let completed = task.await(scope, take child)
      let _ = out.print(output, if completed {{ "1" }} else {{ "0" }})
    }})
  }}
  let _ = out.print(output, "\n")
  Ok(())
}}
"#
    );
    cpu_interpreter_matrix(width, false, &source, false);
}

fn cpu_interpreter_cli(width: usize, scalar: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    // Sixteen 128 MiB compiler stacks plus the front/native transports exceed
    // this harness's unchanged 3 GiB query allowance. Runtime width is separate.
    if width == 8 && !scalar {
        command.env("HIRSEL_JOBS", "1");
    }
    command
}
