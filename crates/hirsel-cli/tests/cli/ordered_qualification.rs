use super::*;

// Qualification is an explicit build, changing only the runtime admission bit.
// Keep these tests compiled in default builds; invoking them there must refuse.
#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_normal_native_launcher_exports_child_atomic_trace() {
    let source = file(include_str!(
        "../../../../conformance/CLO/CLO-3-child-atomic-capture-run.hir"
    ));
    let trace = source.with_extension("ordered.jsonl");
    let tape = source.with_extension("ordered.tape");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--native", "--backend", "llvm"])
        .arg(&source)
        .args(["--grants", r#"{"args":[],"io":{"tasks":"tasks.cooperative@1","dirs":[]},"placement":{"cores":1,"commit":"shared","reorder":"off"}}"#])
        .arg("--trace-events").arg(&trace)
        .arg("--record").arg(&tape)
        .output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let text = std::fs::read_to_string(&trace).unwrap();
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(!rows.is_empty());
    assert!(text.contains("atomic.store"), "{text}");
    assert!(text.contains("task.end"), "{text}");
    for path in [source, trace, tape] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_replay_policy_rejects_extra_and_duplicate_fields() {
    let scratch = tempfile::tempdir().unwrap();
    let source = scratch.path().join("main.hir");
    let executable = scratch.path().join("app");
    let tape = scratch.path().join("input.tape");
    let grants = scratch.path().join("grants.json");
    std::fs::write(&source, "pub fn main() -> Result[(), ()] { Ok(()) }").unwrap();
    std::fs::write(
        &grants,
        r#"{"io":{"tasks":"tasks.cooperative@1","dirs":[]}}"#,
    )
    .unwrap();
    let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["build", "--backend", "llvm"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");
    let recorded = Command::new(&executable)
        .arg("--hirsel-record")
        .arg(&tape)
        .arg("--hirsel-grants")
        .arg(&grants)
        .output()
        .unwrap();
    assert!(recorded.status.success(), "{recorded:?}");
    let bytes = std::fs::read(&tape).unwrap();
    for policy in [
        r#"{"commit":"shared","unknown":0}"#,
        r#"{"commit":"shared","cores":8}"#,
        r#"{"commit":"shared","io":{}}"#,
        r#"{"commit":"shared","control":"socket"}"#,
        r#"{"commit":"shared","commit":"dedicated"}"#,
    ] {
        let refused = Command::new(&executable)
            .arg("--hirsel-replay-policy")
            .arg(policy)
            .arg("--hirsel-replay")
            .arg(&tape)
            .output()
            .unwrap();
        assert!(!refused.status.success(), "{policy}: {refused:?}");
        assert!(refused.stdout.is_empty(), "{policy}: {refused:?}");
        assert!(!refused.stderr.is_empty(), "{policy}: {refused:?}");
        assert_eq!(std::fs::read(&tape).unwrap(), bytes);
    }
}

/// Compare complete recordings except their documented admission/placement
/// provenance. Every event, payload, seed, identity and initial-tree byte stays.
fn semantic_tape(path: &std::path::Path) -> hirsel_rt::launch::Recording {
    let mut recording = hirsel_rt::host_launch::read_recording(path).unwrap();
    let binding = recording.binding.as_mut().unwrap();
    binding.admitted_at = 0;
    binding.placement = None;
    recording
}

fn corpus(program: &str, expected: &[u8]) {
    corpus_backends(program, expected, &["llvm", "cranelift"]);
}

fn corpus_backends(program: &str, expected: &[u8], backends: &[&str]) {
    let scratch = tempfile::tempdir().unwrap();
    let source = scratch.path().join("main.hir");
    std::fs::write(&source, program).unwrap();
    let grants_path = scratch.path().join("grants.json");
    let grants = serde_json::json!({"args":[], "io":{"tasks":"tasks.cooperative@1","stdout":"stdout.bytes@1","dirs":[]}});
    let interpreted = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--grants")
        .arg(grants.to_string())
        .output()
        .unwrap();
    assert_eq!(
        interpreted.status.code(),
        Some(0),
        "interpreter: {interpreted:?}"
    );
    assert_eq!(interpreted.stdout, expected);
    for &backend in backends {
        let executable = scratch.path().join(backend);
        let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--backend", backend])
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(built.status.success(), "{backend}: {built:?}");
        let mut reference = None;
        let prerecorded = scratch.path().join(format!("{backend}-prerecorded.tape"));
        for width in [1, 2, 4, 8] {
            for jitter in [false, true] {
                for reorder in ["off", "auto"] {
                    for commit in ["shared", "dedicated"] {
                        let cell = format!("{backend}-w{width}-j{jitter}-{reorder}-{commit}");
                        let tape = scratch.path().join(format!("{cell}.tape"));
                        let trace = scratch.path().join(format!("{cell}.jsonl"));
                        let mut grants = grants.clone();
                        grants["placement"] =
                            serde_json::json!({"cores":width,"reorder":reorder,"commit":commit});
                        std::fs::write(&grants_path, grants.to_string()).unwrap();
                        let mut command = Command::new(&executable);
                        command
                            .env_remove("HIRSEL_RT_MIGRATE")
                            .env_remove("HIRSEL_RT_WORKERS");
                        if jitter {
                            // Existing W1 physical perturbation: force migration at
                            // every suspension, without adding semantic operations.
                            command.env("HIRSEL_RT_MIGRATE", "1");
                        }
                        let output = command
                            .arg("--hirsel-trace-events")
                            .arg(&trace)
                            .arg("--hirsel-record")
                            .arg(&tape)
                            .arg("--hirsel-grants")
                            .arg(&grants_path)
                            .output()
                            .unwrap();
                        assert_eq!(output.status.code(), Some(0), "{cell}: {output:?}");
                        assert_eq!(output.stdout, expected, "{cell}");
                        assert!(output.stderr.is_empty(), "{cell}: {output:?}");
                        let observed = (std::fs::read(&trace).unwrap(), semantic_tape(&tape));
                        if let Some(reference) = &reference {
                            assert_eq!(&observed, reference, "{cell}");
                        } else {
                            assert!(!observed.0.is_empty());
                            reference = Some(observed);
                            std::fs::copy(&tape, &prerecorded).unwrap();
                        }
                    }
                }
            }
        }
        replay_cells(
            &executable,
            &prerecorded,
            expected,
            &reference.as_ref().unwrap().0,
            0,
        );
        let replayed = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&source)
            .arg("--replay")
            .arg(&prerecorded)
            .output()
            .unwrap();
        assert_eq!(
            replayed.status.code(),
            Some(0),
            "interpreter replay: {replayed:?}"
        );
        assert_eq!(replayed.stdout, expected);
        eprintln!(
            "{backend}: 32 live cells + 32 immutable-input replay cells; interpreter agreement"
        );
    }
}

fn replay_cells(
    executable: &std::path::Path,
    prerecorded: &std::path::Path,
    expected: &[u8],
    reference: &[u8],
    status: i32,
) {
    let tape_bytes = std::fs::read(prerecorded).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    for width in [1, 2, 4, 8] {
        for jitter in [false, true] {
            for reorder in ["off", "auto"] {
                for commit in ["shared", "dedicated"] {
                    let cell = format!("w{width}-j{jitter}-{reorder}-{commit}");
                    let trace = scratch.path().join(format!("{cell}.jsonl"));
                    let policy = serde_json::json!({"commit":commit,"reorder":reorder});
                    let mut command = Command::new(executable);
                    command
                        .env("HIRSEL_RT_WORKERS", width.to_string())
                        .env_remove("HIRSEL_RT_MIGRATE");
                    if jitter && width > 1 {
                        command.env("HIRSEL_RT_MIGRATE", "1");
                    }
                    let output = command
                        .arg("--hirsel-trace-events")
                        .arg(&trace)
                        .arg("--hirsel-replay-policy")
                        .arg(policy.to_string())
                        .arg("--hirsel-replay")
                        .arg(prerecorded)
                        .output()
                        .unwrap();
                    assert_eq!(
                        output.status.code(),
                        Some(status),
                        "replay {cell}: {output:?}"
                    );
                    assert_eq!(output.stdout, expected, "replay {cell}");
                    assert_eq!(std::fs::read(trace).unwrap(), reference, "replay {cell}");
                    assert_eq!(std::fs::read(prerecorded).unwrap(), tape_bytes);
                }
            }
        }
    }
    let refused = Command::new(executable)
        .arg("--hirsel-trace-events")
        .arg(prerecorded)
        .arg("--hirsel-replay-policy")
        .arg(r#"{"commit":"dedicated","reorder":"off"}"#)
        .arg("--hirsel-replay")
        .arg(prerecorded)
        .output()
        .unwrap();
    assert_ne!(
        refused.status.code(),
        Some(0),
        "trace aliases replay input: {refused:?}"
    );
    assert_eq!(std::fs::read(prerecorded).unwrap(), tape_bytes);
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_captured_atomic() {
    corpus(
        include_str!("../../../../conformance/CLO/CLO-3-child-atomic-capture-run.hir"),
        b"",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_saved_receiver() {
    corpus(
        include_str!("../../../../conformance/CONC/CONC-6-suspended-receiver-run.hir"),
        b"",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_topology_reaches_eight_physical_workers() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;
    let scratch = tempfile::tempdir().unwrap();
    let source = scratch.path().join("main.hir");
    std::fs::write(
        &source,
        r#"pub fn main(rt: Io) -> Result[(), GrantError] {
      let output = io.stdout(rt)?
      let input = io.stdin(rt)?
      let _ = out.print(output, "ready\n")
      let mut bytes: List[u8] = []
      let _ = stdin.read(input, mut bytes, 1u64)
      Ok(())
    }"#,
    )
    .unwrap();
    let socket = scratch.path().join("control");
    let grants = serde_json::json!({"io":{"tasks":"tasks.cooperative@1","stdout":"stdout.bytes@1","stdin":"stdin.bytes@1","dirs":[]},"placement":{"cores":8,"control":socket}});
    let mut child = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--native", "--backend", "llvm"])
        .arg(&source)
        .arg("--grants")
        .arg(grants.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    let mut connection = UnixStream::connect(socket).unwrap();
    connection
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    connection.write_all(b"{\"metrics\":true}\n").unwrap();
    let mut reply = String::new();
    BufReader::new(connection).read_line(&mut reply).unwrap();
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let snapshot: serde_json::Value = serde_json::from_str(&reply).unwrap();
    eprintln!("topology: {snapshot}");
    assert_eq!(snapshot["width"]["effective"], 8);
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_captured_mutex() {
    corpus(
        include_str!("../../../../conformance/CLO/CLO-3-child-mutex-capture-run.hir"),
        b"",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_child_resource_cleanup() {
    corpus(
        include_str!("../../../../conformance/CONC/CONC-2-child-resource-drop-run.hir"),
        b"released\n",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_shared_root_child_thaw() {
    corpus(
        include_str!("../../../../conformance/MODE/MODE-4-shared-root-child-run.hir"),
        b"ok\n",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_canceled_drop_send() {
    corpus(
        include_str!("../../../../conformance/CONC/CONC-3-shielded-drop-send-run.hir"),
        b"shielded\n",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_two_sibling_channel_cycle() {
    corpus(
        include_str!("../../../../conformance/CONC/CONC-8-two-task-channel-run.hir"),
        b"waiting\ndestroyed\n",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_owned_mutex_payloads() {
    corpus(
        include_str!("../../../../conformance/CONC/CONC-5-owned-payload-mutexes-run.hir"),
        b"ok\n",
    );
}

const TWO_HOLDING: &str = r#"resource type Note { output: Stdout, value: String }
drop(note) { let _ = out.print(note.output, note.value) }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  task.scope(tasks, fn(s: Scope) {
    let a = task.async(s, fn() { let guard = Note(output: output, value: "first\n") })
    let b = task.async(s, fn() { let guard = Note(output: output, value: "second\n") })
    task.await(s, take a)
    task.await(s, take b)
  })
  Ok(())
}"#;

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_two_holding_child_finalizers() {
    corpus(TWO_HOLDING, b"first\nsecond\n");
}

const CALLED_BEFORE_SEND: &str = r#"resource type Message { tasks: Tasks, sender: Sender[u64] }
drop(message) {
  let mut sender = sender.dup(message.tasks, message.sender)
  let _ = channel.send(message.tasks, mut sender, 11u64)
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  task.scope(tasks, fn(s: Scope) {
    let (sender, receiver): (Sender[u64], Receiver[u64]) = channel.open(tasks)
    let cleanup_sender = sender.dup(tasks, sender)
    let a = task.async(s, fn() { let guard = Message(tasks: tasks, sender: take cleanup_sender) })
    let b = task.async(s, fn() {
      let mut tx = take sender
      let _ = channel.send(tasks, mut tx, 22u64)
    })
    task.await(s, take a)
    task.await(s, take b)
    let mut rx = take receiver
    failure.check(channel.receive(tasks, mut rx) == Ok(11u64), Arithmetic())
    failure.check(channel.receive(tasks, mut rx) == Ok(22u64), Arithmetic())
  })
  Ok(())
}"#;

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_head_calls_child_before_independent_send() {
    corpus(CALLED_BEFORE_SEND, b"");
}

fn prerecorded_corpus(
    program: &str,
    expected: &[u8],
    io: serde_json::Value,
    budgets: Option<serde_json::Value>,
    status: i32,
) {
    let scratch = tempfile::tempdir().unwrap();
    let source = scratch.path().join("main.hir");
    std::fs::write(&source, program).unwrap();
    let mut grants = serde_json::json!({"io":io,"args":[],"placement":{"cores":1,"commit":"shared","reorder":"off"}});
    if let Some(budgets) = &budgets {
        grants["budgets"] = budgets.clone();
    }
    let grant_path = scratch.path().join("grants.json");
    std::fs::write(&grant_path, grants.to_string()).unwrap();
    for backend in ["llvm", "cranelift"] {
        let executable = scratch.path().join(backend);
        let mut build = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        build.args(["build", "--backend", backend]);
        if budgets.is_some() {
            build.arg("--fuel");
        }
        let built = build
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(built.status.success(), "{backend}: {built:?}");
        let tape = scratch.path().join(format!("{backend}.tape"));
        let trace = scratch.path().join(format!("{backend}.jsonl"));
        let output = Command::new(&executable)
            .arg("--hirsel-trace-events")
            .arg(&trace)
            .arg("--hirsel-record")
            .arg(&tape)
            .arg("--hirsel-grants")
            .arg(&grant_path)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(status), "{backend}: {output:?}");
        assert_eq!(output.stdout, expected, "{backend}: {output:?}");
        let reference = std::fs::read(trace).unwrap();
        assert!(!reference.is_empty());
        replay_cells(&executable, &tape, expected, &reference, status);
        let interpreted = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&source)
            .arg("--replay")
            .arg(&tape)
            .output()
            .unwrap();
        assert_eq!(
            interpreted.status.code(),
            Some(status),
            "{backend} interpreter: {interpreted:?}"
        );
        assert_eq!(interpreted.stdout, expected);
        eprintln!(
            "{backend}: 32 immutable-input policy replay cells; interpreter agreement at exit {status}"
        );
    }
}

const CANCEL_OLDER_HOST: &str = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let clock = io.monotonic_clock(rt)?
  let Ok(now) = clock.monotonic(clock) else { return Ok(()) }
  let Some(until) = instant.checked_add(clock, now, 3600000000000u64) else { return Ok(()) }
  task.scope(tasks, fn(s: Scope) {
    let a = task.async(s, fn() -> bool {
      match clock.sleep_until(clock, until) {
        Error(Canceled(Requested())) => true,
        _ => false,
      }
    })
    let b = task.async(s, fn() -> bool { true })
    failure.check(task.await(s, take b), Arithmetic())
    task.cancel(s, a)
    failure.check(task.await(s, take a), Arithmetic())
  })
  let _ = out.print(output, "canceled\n")
  Ok(())
}"#;

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_cancel_older_saved_host_wait() {
    prerecorded_corpus(
        CANCEL_OLDER_HOST,
        b"canceled\n",
        serde_json::json!({"tasks":"tasks.cooperative@1","stdout":"stdout.bytes@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[]}),
        None,
        0,
    );
}

const FIRST_BUDGET_STOP: &str = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let _ = out.print(output, "start\n")
  task.scope(tasks, fn(s: Scope) {
    let a = task.async(s, fn() {
      let mut i: u64 = 0
      while true { i += 1 }
    })
    let b = task.async(s, fn() { let _ = out.print(output, "younger suffix\n") })
    task.await(s, take a)
    task.await(s, take b)
  })
  Ok(())
}"#;

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_first_budget_stop_truncates_younger_suffix() {
    prerecorded_corpus(
        FIRST_BUDGET_STOP,
        b"start\n",
        serde_json::json!({"tasks":"tasks.cooperative@1","stdout":"stdout.bytes@1","dirs":[]}),
        Some(serde_json::json!({"provider":"budgets.limits@2","fuel":10000})),
        5,
    );
}

const THAW_BEFORE_YOUNGER_RELEASE: &str = r#"fn discard_root(take root: Shared[String]) -> () {}
fn thawed(take root: Shared[String]) -> bool {
  match shared.thaw(take root) { Ok(text) => text == "value", Error(other) => false }
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let initial: String = "value"
  let second: Shared[String] = shared.freeze(take initial)
  let first = copy second
  let tick: Atomic[u64] = atomic.create(tasks, 0u64)
  let refused = task.scope(tasks, fn(s: Scope) -> bool {
    let a = task.async(s, fn() -> bool {
      atomic.store(tasks, tick, 1u64)
      let mut i: u64 = 0
      while i < 2000u64 { i += 1 }
      failure.check(i == 2000u64, Arithmetic())
      !thawed(take first)
    })
    let b = task.async(s, fn() { discard_root(take second) })
    let observed = task.await(s, take a)
    task.await(s, take b)
    observed
  })
  failure.check(refused, Arithmetic())
  let _ = out.print(output, "refused\n")
  Ok(())
}"#;

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_thaw_precedes_younger_root_release() {
    corpus(THAW_BEFORE_YOUNGER_RELEASE, b"refused\n");
}

const CALLABLE_CHANNEL: &str = r#"fn shuttle[T: sendable](tasks: Tasks, take value: T) -> Option[T] {
  let (tx, rx): (Sender[T], Receiver[T]) = channel.open(tasks)
  let mut sender = take tx
  let mut receiver = take rx
  let _ = channel.send(tasks, mut sender, take value)
  match channel.receive(tasks, mut receiver) { Ok(item) => Some(take item), Error(_) => None }
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let action: unique sendable fn() -> u64 uses {} = fn() -> u64 { 42u64 }
  let Some(called) = shuttle(tasks, take action) else { return Ok(()) }
  failure.check(called() == 42u64, Arithmetic())
  let _ = out.print(output, "callable\n")
  Ok(())
}"#;

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_callable_channel_carrier_llvm() {
    corpus_backends(CALLABLE_CHANNEL, b"callable\n", &["llvm"]);
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_callable_channel_carrier_cranelift() {
    corpus_backends(CALLABLE_CHANNEL, b"callable\n", &["cranelift"]);
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_captured_value_carrier() {
    corpus(
        r#"fn shuttle[T: sendable](tasks: Tasks, take value: T) -> Option[T] {
  let (tx, rx): (Sender[T], Receiver[T]) = channel.open(tasks)
  let mut sender = take tx
  let mut receiver = take rx
  let _ = channel.send(tasks, mut sender, take value)
  match channel.receive(tasks, mut receiver) { Ok(item) => Some(take item), Error(_) => None }
}
pub fn main(rt: Io) -> Result[(), GrantError] {
 let tasks=io.tasks(rt)?
 let output=io.stdout(rt)?
 let n = 8u64
 let action: unique sendable fn() -> u64 uses {} = fn() -> u64 { n }
 let Some(called) = shuttle(tasks, take action) else { return Ok(()) }
 failure.check(called() == 8u64, Arithmetic())
 let _ = out.print(output, "captured\n")
 Ok(())
}
"#,
        b"captured\n",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_iterator_value_carrier() {
    corpus(
        r#"fn shuttle[T: sendable](tasks: Tasks, take value: T) -> Option[T] {
  let (tx, rx): (Sender[T], Receiver[T]) = channel.open(tasks)
  let mut sender = take tx
  let mut receiver = take rx
  let _ = channel.send(tasks, mut sender, take value)
  match channel.receive(tasks, mut receiver) { Ok(item) => Some(take item), Error(_) => None }
}
fn count(mut n: u64) -> Option[u64] uses {} { let v=n; n+=1; if v<3u64 { Some(v) } else { None } }
pub fn main(rt: Io) -> Result[(), GrantError] {
 let tasks=io.tasks(rt)?
 let output=io.stdout(rt)?
 let values = iter.from(0u64, count)
 let Some(received) = shuttle(tasks, take values) else { return Ok(()) }
 let mut total=0u64
 for n in take received { total+=n }
 failure.check(total==3u64, Arithmetic())
 let _ = out.print(output, "iterator\n")
 Ok(())
}
"#,
        b"iterator\n",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_flat_value_carrier() {
    corpus(
        r#"fn shuttle[T: sendable](tasks: Tasks, take value: T) -> Option[T] {
  let (tx, rx): (Sender[T], Receiver[T]) = channel.open(tasks)
  let mut sender = take tx
  let mut receiver = take rx
  let _ = channel.send(tasks, mut sender, take value)
  match channel.receive(tasks, mut receiver) { Ok(item) => Some(take item), Error(_) => None }
}
flat type Item { text: String, n: u64 }
flat type Batch { items: List[Item] @ growable }
pub fn main(rt: Io) -> Result[(), GrantError] {
 let tasks=io.tasks(rt)?
 let output=io.stdout(rt)?
 let value=Batch(items: [Item(text:"",n:1u64), Item(text:"abc",n:2u64)])
 let Some(received) = shuttle(tasks, take value) else { return Ok(()) }
 let mut total = list.len(received.items)
 for item in received.items { total += item.n + string.byte_len(item.text) }
 failure.check(total == 8u64, Arithmetic())
 let _ = out.print(output, "flat\n")
 Ok(())
}
"#,
        b"flat\n",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_packed_value_carrier() {
    corpus(
        r#"fn shuttle[T: sendable](tasks: Tasks, take value: T) -> Option[T] {
  let (tx, rx): (Sender[T], Receiver[T]) = channel.open(tasks)
  let mut sender = take tx
  let mut receiver = take rx
  let _ = channel.send(tasks, mut sender, take value)
  match channel.receive(tasks, mut receiver) { Ok(item) => Some(take item), Error(_) => None }
}
type Choice = Small(value: u64) | Large(value: u64, extra: u64)
fn score(x: Packed[Choice], large: bool) -> u64 { if large { x.Large.value * x.Large.extra } else { x.Small.value } }
pub fn main(rt: Io) -> Result[(), GrantError] {
 let tasks=io.tasks(rt)?
 let output=io.stdout(rt)?
 let values: List[Packed[Choice]] = [Small(value:1u64), Large(value:2u64,extra:3u64)]
 let Some(received) = shuttle(tasks, take values) else { return Ok(()) }
 let mut total=0u64
 let mut index=0u64
 for item in received { total += score(item, index == 1u64); index += 1u64 }
 let single: Packed[Choice] = Large(value:4u64,extra:5u64)
 let Some(one) = shuttle(tasks, take single) else { return Ok(()) }
 failure.check(total == 7u64 && score(one, true) == 20u64, Arithmetic())
 let _ = out.print(output, "packed\n")
 Ok(())
}
"#,
        b"packed\n",
    );
}

#[test]
#[ignore = "requires -c hirsel.ordered_qualification=true"]
fn ordered_qualification_corpus_soa_value_carrier() {
    corpus(
        r#"fn shuttle[T: sendable](tasks: Tasks, take value: T) -> Option[T] {
  let (tx, rx): (Sender[T], Receiver[T]) = channel.open(tasks)
  let mut sender = take tx
  let mut receiver = take rx
  let _ = channel.send(tasks, mut sender, take value)
  match channel.receive(tasks, mut receiver) { Ok(item) => Some(take item), Error(_) => None }
}
@soa
type Row { small: u8, n: u64, text: String, flag: bool }
pub fn main(rt: Io) -> Result[(), GrantError] {
 let tasks=io.tasks(rt)?
 let output=io.stdout(rt)?
 let mut values: List[Row] = [Row(small:1,n:2u64,text:"abc",flag:true)]
 list.reserve(mut values, 8u64)
 list.push(mut values, Row(small:3,n:4u64,text:"",flag:false))
 let Some(received) = shuttle(tasks, take values) else { return Ok(()) }
 failure.check(list.len(received)==2u64 && received[0].text=="abc" && received[1].n==4u64 && !received[1].flag, Arithmetic())
 let _ = out.print(output, "soa\n")
 Ok(())
}
"#,
        b"soa\n",
    );
}
