const ORDERED_FUEL: &str = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let outcome = core@1/cpu.map_scoped(tasks, 4u32,
    fn() -> List[u64] uses {} { [0u64, 0u64, 0u64, 100000u64] },
    fn(take job: u64) -> u64 uses {} {
      let mut x = job
      for i in 0u64..job { x = int.wrapping_add(int.wrapping_mul(x, 6364136223846793005u64), i) }
      x
    })
  failure.check(false, Bounds())
  Ok(())
}
"#;
const ORDERED_FAILURE: &str = r#"
fn batch(tasks: Tasks, fail: bool) -> Result[List[u64], CpuPoolError] {
  core@1/cpu.map_scoped(tasks, 4u32,
    fn() -> List[u64] uses {} { [0u64, 1u64] },
    fn(take job: u64) -> u64 uses {} {
      let mut x = job
      if fail && job == 0u64 {
        for i in 0u64..5000000u64 { x = int.wrapping_add(int.wrapping_mul(x, 6364136223846793005u64), i) }
      }
      if fail {
        if job == 0u64 { failure.check(false, Bounds()) }
        else { failure.check(false, Arithmetic()) }
      }
      x
    })
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let caught = task.isolate(tasks, fn() -> () { let _ = batch(tasks, true) })
  match caught {
    Ok(_) => failure.check(false, Arithmetic()),
    Error(primary) => failure.check(primary.site.line == FIRST_LINE, Arithmetic()),
  }
  failure.check(batch(tasks, false) == Ok([0u64, 1u64]), Arithmetic())
  Ok(())
}
"#;
const ORDERED_CANCEL: &str = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  for repeat in 0u64..2u64 {
    task.scope(tasks, fn(scope: Scope) -> () {
      let child = task.async(scope, fn() -> bool {
        let batch = core@1/cpu.map_scoped(tasks, 4u32,
          fn() -> List[u64] uses {} { [0u64, 1u64, 2u64, 3u64] },
          fn(take job: u64) -> u64 uses {} {
            let mut x = job
            for i in 0u64..5000000u64 { x = int.wrapping_add(int.wrapping_mul(x, 6364136223846793005u64), i) }
            x
          })
        match batch { Error(Canceled(_)) => true, _ => false }
      })
      for turn in 0u64..CHECKS { let _ = task.check_canceled(scope) }
      task.cancel(scope, child)
      failure.check(task.await(scope, take child), Bounds())
    })
  }
  Ok(())
}
"#;
const ORDERED_NO_CONTROLLER: &str = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  task.scope(tasks, fn(scope: Scope) -> () {
    let child = task.async(scope, fn() -> bool {
      let batch = core@1/cpu.map_scoped(tasks, 8u32,
        fn() -> List[u64] uses {} { failure.check(false, Bounds()); [1u64] },
        fn(take job: u64) -> u64 uses {} { job })
      batch == Error(WidthUnavailable())
    })
    failure.check(task.await(scope, take child), Bounds())
  })
  Ok(())
}
"#;
fn ordered_remaining(name: &str, template: &str, status: i32) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let first = template
        .lines()
        .position(|line| line.contains("failure.check(false, Bounds())"))
        .map_or(0, |i| i + 1);
    let source = template
        .replace("FIRST_LINE", &format!("{first}u64"))
        .replace(
            "CHECKS",
            if name == "cancel-after" {
                "3u64"
            } else {
                "0u64"
            },
        );
    fs::write(root.join("main.hir"), source).unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_ordered_remaining","version":"1"},
            "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    let mut authority = serde_json::json!({"io":{"dirs":[],"tasks":"tasks.cooperative@1","cpu":{"provider":"cpu.scoped@1",
            "max_workers":4,"max_domains":1,"max_physical_bytes":1073741824u64,"max_batch_bytes":8388608}}});
    if name == "fuel" {
        authority["budgets"] = serde_json::json!({"provider":"budgets.limits@2","fuel":10000});
    }
    if name == "no-controller" {
        authority["io"]["cpu"]["max_workers"] = serde_json::json!(8);
    }
    fs::write(&grants, authority.to_string()).unwrap();
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
        let tape = root.join("recording.tape");
        let trace = root.join("live.events");
        let probe = root.join("physical.probe");
        let observation = root.join("live.observation");
        let mut live = Command::new(&executable);
        live.arg("--hirsel-grants")
            .arg(&grants)
            .arg("--hirsel-record")
            .arg(&tape)
            .env("HIRSEL_RT_CPU_PROBE", &probe)
            .env("HIRSEL_RT_OBSERVATION", &observation);
        // Isolate's existing generic yield has no canonical trace projection.
        // Keep strict C's refusal; its failure/cleanup case uses durable replay.
        if name != "failure" {
            live.arg("--hirsel-trace-events").arg(&trace);
        }
        let live = live.output().unwrap();
        let events = fs::read_to_string(&trace).unwrap_or_default();
        assert_eq!(
            live.status.code(),
            Some(status),
            "{name} {backend}: {live:?}\n{events}"
        );
        let claims: Vec<serde_json::Value> = events
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|row| {
                row["outcome"]["kind"] == "cpu_control"
                    && row["outcome"]["action"] == 5
                    && row["outcome"]["outcome"] == 0
            })
            .collect();
        if name == "fuel" {
            assert_eq!(claims.last().unwrap()["outcome"]["count"], 3);
            let terminal = events
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .find(|row| row["outcome"]["kind"] == "stopped")
                .unwrap();
            assert_eq!(terminal["task"], serde_json::json!([3]), "{events}");
            assert!(
                fs::read_to_string(&observation)
                    .unwrap()
                    .contains("budget_exceeded fuel")
            );
            eprintln!(
                "{backend} fuel stop live job 4, ordinal 3, task {}",
                terminal["task"]
            );
        } else if matches!(name, "cancel-before" | "no-controller") {
            assert!(claims.is_empty(), "{events}");
        } else if name == "cancel-after" {
            assert!(!claims.is_empty(), "{events}");
        } else {
            let samples = fs::read_to_string(&probe).unwrap();
            let intervals: Vec<Vec<u64>> = samples
                .lines()
                .map(|line| {
                    line.split_whitespace()
                        .skip(1)
                        .map(|x| x.parse().unwrap())
                        .collect()
                })
                .collect();
            let older: Vec<_> = intervals.iter().filter(|fields| fields[1] == 1).collect();
            let younger = intervals.iter().find(|fields| fields[1] == 2).unwrap();
            assert!(older.len() > 8, "{samples}");
            assert!(
                younger[4] < older[7][3],
                "job 1 must reach its checked failure while job 0 is still computing: {samples}"
            );
            eprintln!(
                "{backend} reversed FAIL-2: job 1 failure available at {}, job 0 still computing at {}; reported job 0; reentry succeeded",
                younger[4], older[7][3]
            );
        }
        let replayed = root.join("replay.events");
        let replay_probe = root.join("replay.probe");
        let replay_observation = root.join("replay.observation");
        let mut replay = Command::new(&executable);
        replay
            .arg("--hirsel-replay")
            .arg(&tape)
            .env("HIRSEL_RT_WORKERS", "1")
            .env("HIRSEL_RT_CPU_PROBE", &replay_probe)
            .env("HIRSEL_RT_OBSERVATION", &replay_observation);
        if name != "failure" {
            replay.arg("--hirsel-trace-events").arg(&replayed);
        }
        let replay = replay.output().unwrap();
        assert_eq!(
            replay.status.code(),
            Some(status),
            "{name} {backend}: {replay:?}"
        );
        if name != "failure" {
            assert_eq!(events.as_bytes(), fs::read(&replayed).unwrap());
        }
        assert_eq!(live.stdout, replay.stdout);
        assert_eq!(live.stderr, replay.stderr);
        let semantic_observation = |path: &std::path::Path| {
            fs::read_to_string(path)
                .unwrap()
                .lines()
                // Native admission placement is physical and absent on replay.
                .filter(|line| !line.starts_with("placement "))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            semantic_observation(&observation),
            semantic_observation(&replay_observation)
        );
        assert!(fs::read(&replay_probe).unwrap().is_empty());
        eprintln!("{backend} {name}: live == replay, cleanup completed");
    }
}
