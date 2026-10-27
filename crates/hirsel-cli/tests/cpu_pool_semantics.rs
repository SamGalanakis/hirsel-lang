hirsel_guard::install!();
use std::{fs, process::Command};

fn checked(output: std::process::Output) {
    assert!(
        output.status.success(),
        "{:?}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn refusal_does_not_invoke_callbacks_and_retired_shared_results_replay() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        include_str!("support/cpu_pool/refusals.hir"),
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_semantics","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"tasks":"tasks.free@1","cpu":{"provider":"cpu.scoped@1",
        "max_workers":1,"max_domains":1,"max_physical_bytes":268435456u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
    let cli = env!("CARGO_BIN_EXE_hirsel");
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            Command::new(cli)
                .args(["build", "--backend", backend])
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
        checked(
            Command::new(&executable)
                .arg("--hirsel-replay")
                .arg(&tape)
                .output()
                .unwrap(),
        );
    }
}

#[test]
fn aggregate_jobs_keep_cleanup_ownership_until_mapper_admission() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        include_str!("support/cpu_pool/aggregate.hir"),
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_aggregate","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"tasks":"tasks.free@1","cpu":{"provider":"cpu.scoped@1",
        "max_workers":1,"max_domains":1,"max_physical_bytes":268435456u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
    let cli = env!("CARGO_BIN_EXE_hirsel");
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            Command::new(cli)
                .args(["build", "--validate-stages", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        checked(
            Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(&grants)
                .output()
                .unwrap(),
        );
    }
}

#[test]
fn canceled_and_failed_batches_drain_before_recorded_return_or_reentry() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"tasks":"tasks.free@1","cpu":{"provider":"cpu.scoped@1",
        "max_workers":1,"max_domains":1,"max_physical_bytes":268435456u64,"max_batch_bytes":8388608}}}).to_string()).unwrap();
    for (name, source) in [
        ("canceled", include_str!("support/cpu_pool/canceled.hir")),
        ("failed", include_str!("support/cpu_pool/failed.hir")),
    ] {
        fs::write(root.join("main.hir"), source).unwrap();
        let manifest = root.join("hirsel.json");
        fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_lifetime","version":"1"},
            "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let executable = root.join(name);
        checked(
            Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--validate-stages", "--backend", "llvm"])
                .arg(&manifest)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap(),
        );
        let tape = root.join(format!("{name}.tape"));
        checked(
            Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(&grants)
                .arg("--hirsel-record")
                .arg(&tape)
                .output()
                .unwrap(),
        );
        checked(
            Command::new(&executable)
                .arg("--hirsel-replay")
                .arg(&tape)
                .output()
                .unwrap(),
        );
    }
}
#[test]
fn physical_refusal_consumes_callbacks_without_invoking_them_and_replays() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let denied = core@1/cpu.map_scoped(tasks, 1u32,
    fn() -> List[u64] uses {} { failure.check(false, Bounds()); [1] },
    fn(take job: u64) -> u64 uses {} { failure.check(false, Bounds()); job })
  failure.check(denied == Error(ResourceLimit()), Bounds())
  Ok(())
}
"#,
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_resources","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(
        &grants,
        serde_json::json!({"io":{"dirs":[],"tasks":"tasks.free@1","cpu":{"provider":"cpu.scoped@1",
        "max_workers":1,"max_domains":1,"max_physical_bytes":1,"max_batch_bytes":8388608}}})
        .to_string(),
    )
    .unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = root.join(backend);
        checked(
            Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--backend", backend])
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
        checked(
            Command::new(&executable)
                .arg("--hirsel-replay")
                .arg(&tape)
                .output()
                .unwrap(),
        );
    }
}
#[test]
fn batch_backing_cap_stops_mapper_and_replays_the_stop() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let _ = core@1/cpu.map_scoped(tasks, 1u32,
    fn() -> List[u64] uses {} { [1] },
    fn(take job: u64) -> u64 uses {} {
      let mut storage: List[u64] = []
      list.reserve(mut storage, 2000000)
      job + list.len(storage)
    })
  Ok(())
}
"#,
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_heap_cap","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"tasks":"tasks.free@1","cpu":{"provider":"cpu.scoped@1",
        "max_workers":1,"max_domains":1,"max_physical_bytes":268435456u64,"max_batch_bytes":1048576}}}).to_string()).unwrap();
    let executable = root.join("batch");
    checked(
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--backend", "llvm"])
            .arg(&manifest)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap(),
    );
    let tape = root.join("tape");
    for args in [
        vec![
            "--hirsel-grants",
            grants.to_str().unwrap(),
            "--hirsel-record",
            tape.to_str().unwrap(),
        ],
        vec!["--hirsel-replay", tape.to_str().unwrap()],
    ] {
        let observation = root.join("observation");
        let result = Command::new(&executable)
            .args(args)
            .env("HIRSEL_RT_OBSERVATION", &observation)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(hirsel_run_schema::exit::BUDGET_EXCEEDED),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report = fs::read_to_string(&observation).unwrap();
        assert!(report.contains("budget_exceeded memory"), "{report}");
    }
}

#[test]
fn cumulative_batch_backing_cap_stops_mapper_and_replays_the_stop() {
    let provider = "tasks.free@1";
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let _ = core@1/cpu.map_scoped(tasks, 1u32,
    fn() -> List[u64] uses {} { [1] },
    fn(take job: u64) -> u64 uses {} {
      let mut left: List[u64] = []
      list.reserve(mut left, 90000)
      let mut right: List[u64] = []
      list.reserve(mut right, 90000)
      job + list.len(left) + list.len(right)
    })
  Ok(())
}
"#,
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_heap_cap","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"tasks":provider,"cpu":{"provider":"cpu.scoped@1",
        "max_workers":1,"max_domains":1,"max_physical_bytes":268435456u64,"max_batch_bytes":1048576}}}).to_string()).unwrap();
    let executable = root.join("batch");
    checked(
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--backend", "llvm"])
            .arg(&manifest)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap(),
    );
    let tape = root.join("tape");
    for args in [
        vec![
            "--hirsel-grants",
            grants.to_str().unwrap(),
            "--hirsel-record",
            tape.to_str().unwrap(),
        ],
        vec!["--hirsel-replay", tape.to_str().unwrap()],
    ] {
        let observation = root.join("observation");
        let result = Command::new(&executable)
            .args(&args)
            .env("HIRSEL_RT_OBSERVATION", &observation)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(hirsel_run_schema::exit::BUDGET_EXCEEDED),
            "{provider} {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report = fs::read_to_string(&observation).unwrap();
        assert!(report.contains("budget_exceeded memory"), "{report}");
    }
}

#[test]
fn ordered_uncertified_refusal_replays_without_invoking_closed_callbacks() {
    ordered_mapper_refusal("let xs = [job]; xs[0]");
}

#[test]
fn ordered_float_mapper_refuses_before_preparation_and_replays() {
    ordered_mapper_refusal("core@1/f64.bits(core@1/f64.from_bits(job) + 1.0f64)");
}

fn ordered_mapper_refusal(mapper: &str) {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let outcome = core@1/cpu.map_scoped(tasks, 1u32,
    fn() -> List[u64] uses {} { failure.check(false, Bounds()); [1] },
    fn(take job: u64) -> u64 uses {} { MAPPER })
  failure.check(outcome == Error(MapperNotCertified()), Bounds())
  Ok(())
}
"#
        .replace("MAPPER", mapper),
    )
    .unwrap();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, serde_json::json!({"schema":1,"package":{"name":"cpu_ordered_refusal","version":"1"},
        "sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    fs::write(&grants, serde_json::json!({"io":{"dirs":[],"tasks":"tasks.cooperative@1","cpu":{"provider":"cpu.scoped@1",
        "max_workers":1,"max_domains":1,"max_physical_bytes":268435456u64,"max_batch_bytes":1048576}}}).to_string()).unwrap();
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
        let recorded = root.join("recorded.events");
        let replayed = root.join("replayed.events");
        let probe = root.join("probe");
        checked(
            Command::new(&executable)
                .arg("--hirsel-grants")
                .arg(&grants)
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-trace-events")
                .arg(&recorded)
                .env("HIRSEL_RT_CPU_PROBE", &probe)
                .output()
                .unwrap(),
        );
        assert!(fs::read(&probe).unwrap().is_empty());
        checked(
            Command::new(&executable)
                .arg("--hirsel-replay")
                .arg(&tape)
                .arg("--hirsel-trace-events")
                .arg(&replayed)
                .env("HIRSEL_RT_CPU_PROBE", &probe)
                .output()
                .unwrap(),
        );
        assert!(fs::read(&probe).unwrap().is_empty());
        let events = fs::read(recorded).unwrap();
        assert!(String::from_utf8_lossy(&events).contains("cpu_control"));
        assert_eq!(events, fs::read(replayed).unwrap());
    }
}
