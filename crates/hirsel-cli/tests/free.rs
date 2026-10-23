//! Free mode at width 1 and one permit (lane F0), through its test channel
//! (`HIRSEL_RT_FREE_TEST`), on the interpreter and both native backends: each trace program
//! records under seeded schedules with injected flushes, terminal and durable, then replays
//! to its live projection under other seeds, and from every cut its report names, truncated
//! there and torn five bytes past it, to the live projection at that cut.
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

hirsel_guard::install!();

static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The interpreter, then each native backend.
const ENGINES: [&[&str]; 3] = [&[], &["--native"], &["--native", "--backend", "cranelift"]];

const GRANTS: &str =
    r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"args":[]}"#;
/// `finite_release` spends its fuel inside a releasing walk.
const FUEL_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"args":[],"budgets":{"provider":"budgets.limits@2","fuel":200000}}"#;

/// Each trace, its grants and the live outcome every recording of it ends with.
const TRACES: [(&str, &str, &str); 8] = [
    ("gap", GRANTS, "returned"),
    ("finite_release", FUEL_GRANTS, "fuel"),
    ("mutex_cycle", GRANTS, "returned"),
    ("parked_receive", GRANTS, "blocked"),
    ("nested_root", GRANTS, "returned"),
    ("acquire_reply", GRANTS, "returned"),
    ("coalesced_root", GRANTS, "returned"),
    ("thaw_race", GRANTS, "returned"),
];

/// The recording seeds, and the seed each replay runs under instead. Every native run
/// compiles its program, so one seed per trace keeps the binary within its time budget; the
/// racing traces' test adds the seeds that reach each outcome.
const SEEDS: [u64; 1] = [1];
const REPLAY_SEED: u64 = 7;
/// Truncated at a cut, and torn this many bytes past it.
const TORN: usize = 5;

/// A trace program's source (`tests/free/<name>.hir`).
fn source(name: &str) -> &'static str {
    match name {
        "gap" => include_str!("free/gap.hir"),
        "deadline" => include_str!("free/deadline.hir"),
        "finite_release" => include_str!("free/finite_release.hir"),
        "mutex_cycle" => include_str!("free/mutex_cycle.hir"),
        "parked_receive" => include_str!("free/parked_receive.hir"),
        "nested_root" => include_str!("free/nested_root.hir"),
        "acquire_reply" => include_str!("free/acquire_reply.hir"),
        "coalesced_root" => include_str!("free/coalesced_root.hir"),
        "thaw_race" => include_str!("free/thaw_race.hir"),
        _ => unreachable!("no trace {name}"),
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Scratch {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("hirsel-free-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Free replay artifacts retained at {}", self.0.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One run of `program` under the test channel `hook`. Bound each child so
/// a stuck replay names its engine, phase/cut and seed before the pool's
/// whole-harness timeout discards its output. Follow cli/dwarf.rs's watchdog,
/// using files so detached runners cannot hold a pipe open. Native runners form
/// their own process groups, so a
/// timeout stops and collects this child's descendants before killing them.
fn run(engine: &[&str], program: &Path, grants: &str, hook: &str) -> Output {
    use std::time::{Duration, Instant};
    let stdout = program.with_extension("child.stdout");
    let stderr = program.with_extension("child.stderr");
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .arg("run")
        .args(engine)
        .arg(program)
        .arg("--grants")
        .arg(grants)
        .env("HIRSEL_RT_FREE_TEST", hook)
        .process_group(0)
        .stdout(std::fs::File::create(&stdout).unwrap())
        .stderr(std::fs::File::create(&stderr).unwrap());
    let diagnostics = std::env::var_os("HIRSEL_FREE_PROGRESS").is_some();
    if diagnostics {
        eprintln!("free child start {engine:?} {program:?}: {hook}");
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            stop_tree(child.id());
            let _ = child.kill();
            break child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = Output {
        status,
        stdout: std::fs::read(stdout).unwrap(),
        stderr: std::fs::read(stderr).unwrap(),
    };
    assert!(!timed_out, "{command:?} exceeded 120 seconds: {output:?}");
    if diagnostics {
        eprintln!("free child ended {engine:?} {program:?}: {}", output.status);
    }
    output
}

/// Linux child-tree traversal follows hirsel-conformance/src/bin/profile.rs's
/// watchdog. Stop each
/// parent before reading its children: it cannot reap/reuse their identities
/// while we address them, and no stopped process can create more descendants.
/// Read every thread's children (compilers can launch from worker threads).
fn stop_tree(root: u32) {
    let mut tree = vec![root];
    let signal = |pid: u32, kind: &str| {
        let _ = Command::new("kill")
            .args([kind, "--", &pid.to_string()])
            .status();
    };
    let mut at = 0;
    while at < tree.len() {
        let pid = tree[at];
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        let wchan = std::fs::read_to_string(format!("/proc/{pid}/wchan"));
        signal(pid, "-STOP");
        if let Ok(threads) = std::fs::read_dir(format!("/proc/{pid}/task")) {
            for thread in threads.flatten() {
                if let Ok(children) = std::fs::read_to_string(thread.path().join("children")) {
                    for child in children.split_whitespace().filter_map(|s| s.parse().ok()) {
                        if !tree.contains(&child) {
                            tree.push(child);
                        }
                    }
                }
            }
        }
        eprintln!("free child timeout pid {pid}: stat {stat:?}; wchan {wchan:?}");
        at += 1;
    }
    for pid in tree.into_iter().rev() {
        signal(pid, "-KILL");
    }
}

#[test]
fn watchdog_stops_a_native_descendant_in_its_own_group() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let scratch = Scratch::new("watchdog");
    let pid_file = scratch.path("native.pid");
    let mut child = Command::new("/bin/sh")
        .args([
            "-c",
            r#"setsid /bin/sh -c 'echo $$ > "$1"; exec sleep 30' sh "$1" & wait"#,
            "sh",
        ])
        .arg(&pid_file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let pid = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
        {
            break pid;
        }
        if Instant::now() >= until {
            stop_tree(child.id());
            child.wait().unwrap();
            panic!("native descendant did not start");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    stop_tree(child.id());
    assert!(!child.wait().unwrap().success());
    // The orphan may briefly remain a zombie until the action's init reaps
    // it; neither a zombie nor an absent process can keep output pipes open.
    loop {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        if stat.as_ref().is_err()
            || stat.as_ref().is_ok_and(|text| {
                text.rsplit_once(") ")
                    .is_some_and(|(_, rest)| rest.starts_with('Z'))
            })
        {
            break;
        }
        assert!(Instant::now() < until, "descendant survived: {stat:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A replay of `tape` under `seed`: the projection it reports.
fn replayed(engine: &[&str], program: &Path, grants: &str, tape: &Path, seed: u64) -> String {
    let report = tape.with_extension("replayed");
    let _ = std::fs::remove_file(&report);
    let output = run(
        engine,
        program,
        grants,
        &format!(
            "mode=replay,tape={},report={},seed={seed}",
            tape.display(),
            report.display()
        ),
    );
    let text = std::fs::read_to_string(&report)
        .unwrap_or_else(|e| panic!("replay {engine:?} {tape:?}, seed {seed}: {e}: {output:?}"));
    if text.starts_with("replayed Projection { outcome: \"returned\"") {
        assert!(
            output.status.success(),
            "returned replay {engine:?} {tape:?}: {output:?}"
        );
    }
    text.trim_end().to_owned()
}

/// A recording: its scratch directory (holding its program and tape), its stdout and its report's cuts,
/// each a tape offset and the live projection there, the terminal cut last.
struct Recorded {
    scratch: Scratch,
    what: String,
    stdout: String,
    cuts: Vec<(usize, String)>,
}

/// Record `name` under `seed`, with a flush after one publish in three, durable or not.
fn record(engine: &[&str], name: &str, grants: &str, seed: u64, durable: bool) -> Recorded {
    let scratch = Scratch::new(name);
    let program = scratch.path("main.hir");
    std::fs::write(&program, source(name)).unwrap();
    let (tape, report) = (scratch.path("tape"), scratch.path("report"));
    let mode = if durable { ",durable=1" } else { "" };
    let output = run(
        engine,
        &program,
        grants,
        &format!(
            "mode=record,tape={},report={},seed={seed},flush=3{mode}",
            tape.display(),
            report.display()
        ),
    );
    let what = format!("{name} on {engine:?}, seed {seed}, durable {durable}");
    let report =
        std::fs::read_to_string(&report).unwrap_or_else(|e| panic!("{what}: {e}: {output:?}"));
    let cuts = report
        .lines()
        .map(|line| {
            let rest = line.strip_prefix("cut ").unwrap();
            let (offset, projection) = rest.split_once(' ').unwrap();
            (offset.parse().unwrap(), projection.to_owned())
        })
        .collect::<Vec<_>>();
    assert!(!cuts.is_empty(), "{what}: no cut");
    if cuts.last().unwrap().1.contains("outcome: \"returned\"") {
        assert!(
            output.status.success(),
            "returned recording {what}: {output:?}"
        );
    }
    Recorded {
        scratch,
        what,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        cuts,
    }
}

/// Check every replay of `recorded`: the whole tape under another seed to the live
/// projection, which ends with `outcome`, and each earlier cut, truncated and torn.
fn verify(engine: &[&str], grants: &str, outcome: &str, recorded: &Recorded) {
    let (program, what) = (recorded.scratch.path("main.hir"), &recorded.what);
    let tape = recorded.scratch.path("tape");
    let (_, live) = recorded.cuts.last().unwrap();
    assert!(
        live.contains(&format!("outcome: \"{outcome}\"")),
        "{what}: {live}"
    );
    assert_eq!(
        replayed(engine, &program, grants, &tape, REPLAY_SEED),
        format!("replayed {live}"),
        "{what}: the replay under seed {REPLAY_SEED}"
    );
    let bytes = std::fs::read(&tape).unwrap();
    let prefix = recorded.scratch.path("prefix");
    for (offset, projection) in &recorded.cuts[..recorded.cuts.len() - 1] {
        for extra in [0, TORN] {
            std::fs::write(&prefix, &bytes[..(offset + extra).min(bytes.len())]).unwrap();
            assert_eq!(
                replayed(engine, &program, grants, &prefix, 3),
                format!("replayed {projection}"),
                "{what}: the cut at {offset} (+{extra})"
            );
        }
    }
}

/// Run `each` for every trace on its own thread: a native run spends most of its time
/// compiling, so the traces proceed together.
fn per_trace(each: impl Fn(&str, &str, &str) + Sync) {
    std::thread::scope(|scope| {
        for (name, grants, outcome) in TRACES {
            let each = &each;
            scope.spawn(move || each(name, grants, outcome));
        }
    });
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_on_every_engine() {
    per_trace(|name, grants, outcome| {
        for engine in ENGINES {
            for seed in SEEDS {
                for durable in [false, true] {
                    let recorded = record(engine, name, grants, seed, durable);
                    verify(engine, grants, outcome, &recorded);
                }
            }
        }
    });
}

/// The traces whose outcome a schedule decides reach each outcome under some seed on every
/// engine, and that seed's durable recording replays as above.
#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_on_every_engine() {
    // A diagnostic invocation can select one of the nine trace/engine pairs,
    // preserving its seed search, both outcomes and every replay cut. With no
    // selection the test still covers the full matrix.
    let selection = std::env::var("HIRSEL_FREE_MATRIX").ok().map(|value| {
        let (trace, engine) = value.split_once(':').expect("trace:engine selection");
        assert!(["mutex_cycle", "acquire_reply", "thaw_race"].contains(&trace));
        let engine = ["interpreter", "llvm", "cranelift"]
            .iter()
            .position(|name| *name == engine)
            .expect("engine selection must be interpreter, llvm or cranelift");
        (trace.to_owned(), engine)
    });
    std::thread::scope(|scope| {
        for (name, outcomes) in [
            ("mutex_cycle", ["thawed\n", "shared\n"]),
            ("acquire_reply", ["a first\n", "b first\n"]),
            ("thaw_race", ["thawed\n", "shared\n"]),
        ] {
            for (index, engine) in ENGINES.into_iter().enumerate() {
                if selection
                    .as_ref()
                    .is_some_and(|(trace, selected)| trace != name || *selected != index)
                {
                    continue;
                }
                scope.spawn(move || {
                    for want in outcomes {
                        let seed = (0..64u64)
                            .find(|&seed| record(engine, name, GRANTS, seed, false).stdout == want)
                            .unwrap_or_else(|| {
                                panic!("{name} on {engine:?}: no seed prints {want:?}")
                            });
                        let recorded = record(engine, name, GRANTS, seed, true);
                        assert_eq!(recorded.stdout, want, "{}", recorded.what);
                        verify(engine, GRANTS, "returned", &recorded);
                    }
                });
            }
        }
    });
}

/// Repeated dup and drop of one root in one segment leave the root to the joiner on every
/// engine (lane R3): the deferred releases coalesce and the child's end applies them.
#[test]
fn coalesced_root_releases_reach_the_joiners_thaw_on_every_engine() {
    std::thread::scope(|scope| {
        for engine in ENGINES {
            scope.spawn(move || {
                let recorded = record(engine, "coalesced_root", GRANTS, 1, false);
                assert_eq!(recorded.stdout, "thawed\n", "{}", recorded.what);
            });
        }
    });
}

/// Result readiness and the cleanup after a moved result have identical ordinals,
/// including rests with no remaining value to drop, across all three engines.
#[test]
fn native_returned_rest_completion_replays_across_engines() {
    for durable in [false, true] {
        let recorded = record(ENGINES[1], "mutex_cycle", GRANTS, 0, durable);
        let (_, live) = recorded.cuts.last().unwrap();
        assert!(
            live.contains("outcome: \"returned\""),
            "{}: {live}",
            recorded.what
        );
        let program = recorded.scratch.path("main.hir");
        let tape = recorded.scratch.path("tape");
        for engine in ENGINES {
            assert_eq!(
                replayed(engine, &program, GRANTS, &tape, REPLAY_SEED),
                format!("replayed {live}"),
                "{} replayed on {engine:?}",
                recorded.what
            );
        }
        // The join is admitted only through entry, but its child's returned
        // cleanup may already have completed. Both sides must reach that cut.
        let bytes = std::fs::read(&tape).unwrap();
        let prefix = recorded.scratch.path("prefix");
        let calls: Vec<_> = recorded
            .cuts
            .iter()
            .filter(|(_, projection)| projection.contains("[]: Progress { op: 5, phase: Entered }"))
            .collect();
        assert!(!calls.is_empty(), "{}: no calling cut", recorded.what);
        for (offset, projection) in calls {
            for extra in [0, TORN] {
                std::fs::write(&prefix, &bytes[..(offset + extra).min(bytes.len())]).unwrap();
                for engine in ENGINES {
                    assert_eq!(
                        replayed(engine, &program, GRANTS, &prefix, 3),
                        format!("replayed {projection}"),
                        "{} calling cut {offset} (+{extra}) on {engine:?}",
                        recorded.what
                    );
                }
            }
        }
    }
}

#[test]
fn recorded_deadline_exit_with_a_child_rest_replays_across_engines() {
    let grants = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[]},"args":[]}"#;
    for durable in [false, true] {
        let recorded = record(ENGINES[1], "deadline", grants, 1, durable);
        assert_eq!(recorded.stdout, "expired\n", "{}", recorded.what);
        let (_, live) = recorded.cuts.last().unwrap();
        assert!(
            live.contains("outcome: \"returned\""),
            "{}: {live}",
            recorded.what
        );
        for engine in ENGINES {
            assert_eq!(
                replayed(
                    engine,
                    &recorded.scratch.path("main.hir"),
                    grants,
                    &recorded.scratch.path("tape"),
                    REPLAY_SEED
                ),
                format!("replayed {live}"),
                "{} on {engine:?}",
                recorded.what
            );
        }
    }
}

#[test]
fn f1_compiled_eight_worker_admission_records_production_host_phases() {
    let scratch = Scratch::new("f1-ordinary-admission");
    let program = scratch.path("main.hir");
    std::fs::write(
        &program,
        r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let _ = out.print(out, "f1 production\n")
  Ok(())
}
"#,
    )
    .unwrap();
    let tape = scratch.path("recording");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--native"])
        .arg(&program)
        .args([
            "--grants",
            r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[]}"#,
            "--cores",
            "8",
            "--record",
        ])
        .arg(&tape)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"f1 production\n");
    let bytes = std::fs::read(&tape).unwrap();
    let qualified = hirsel_rt::free_driver::admission_enabled();
    assert_eq!(
        bytes.starts_with(b"hirsel-free-tape 2\n"),
        qualified,
        "compiled admission must use F1 phases exactly in the qualification build"
    );
    // A replay must be bound to this exact program before entering code.
    let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--native"])
        .arg(&program)
        .arg("--replay")
        .arg(&tape)
        .args(["--cores", "1"])
        .output()
        .unwrap();
    assert!(replay.status.success(), "{replay:?}");
    assert_eq!(replay.stdout, output.stdout);
}

#[test]
fn f1_native_recording_replays_stdout_and_bound_root_on_interpreter() {
    let scratch = Scratch::new("f1-interpreter-host");
    let program = scratch.path("main.hir");
    std::fs::write(
        &program,
        r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let _ = out.print(out, "f1 engine replay\n")
  Ok(())
}
"#,
    )
    .unwrap();
    let tape = scratch.path("recording");
    let live = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--native"])
        .arg(&program)
        .args([
            "--grants",
            r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[]}"#,
            "--cores",
            "8",
            "--record",
        ])
        .arg(&tape)
        .output()
        .unwrap();
    assert!(live.status.success(), "{live:?}");
    assert_eq!(live.stdout, b"f1 engine replay\n");
    assert!(
        std::fs::read(&tape)
            .unwrap()
            .starts_with(b"hirsel-free-tape 2\n")
    );
    let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&program)
        .arg("--replay")
        .arg(&tape)
        .output()
        .unwrap();
    assert!(replay.status.success(), "{replay:?}");
    assert_eq!(replay.stdout, live.stdout);
}

#[test]
fn f1_native_task_channel_recording_replays_across_engines() {
    let scratch = Scratch::new("f1-task-channel");
    let program = scratch.path("main.hir");
    std::fs::write(&program, source("gap")).unwrap();
    let tape = scratch.path("recording");
    let report = scratch.path("projection");
    let live = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .env(hirsel_rt::free_replay::Report::VAR, &report)
        .args(["run", "--native"])
        .arg(&program)
        .args([
            "--grants",
            r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[]}"#,
            "--cores",
            "8",
            "--record",
        ])
        .arg(&tape)
        .output()
        .unwrap();
    assert!(live.status.success(), "{live:?}");
    assert!(
        live.stdout == b"sent\n" || live.stdout == b"closed\n",
        "{live:?}"
    );
    assert_eq!(
        std::fs::read(&tape)
            .unwrap()
            .starts_with(b"hirsel-free-tape 2\n"),
        hirsel_rt::free_driver::admission_enabled()
    );
    let live_projection = if hirsel_rt::free_driver::admission_enabled() {
        Some(std::fs::read_to_string(&report).unwrap())
    } else {
        None
    };
    for engine in ENGINES {
        for width in ["1", "8"] {
            let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .env(hirsel_rt::free_replay::Report::VAR, &report)
                .arg("run")
                .args(engine)
                .arg(&program)
                .arg("--replay")
                .arg(&tape)
                .args(["--cores", width])
                .output()
                .unwrap();
            assert!(
                replay.status.success(),
                "{engine:?} width {width}: {replay:?}"
            );
            assert_eq!(replay.stdout, live.stdout, "{engine:?} width {width}");
            if let Some(live) = &live_projection {
                assert_eq!(
                    &std::fs::read_to_string(&report).unwrap(),
                    live,
                    "{engine:?} width {width}: full normalized projection"
                );
            }
        }
    }
}

/// Restrict a child to a subset of the test's admitted CPUs. The runtime's
/// process permit grant uses available_parallelism, while replay retains its
/// requested logical width independently of the host topology.
fn f1_permits(command: &mut Command, permits: usize) {
    let mut admitted = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
    assert_eq!(
        unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(&admitted), &mut admitted) },
        0
    );
    let cpus: Vec<_> = (0..libc::CPU_SETSIZE as usize)
        .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &admitted) })
        .take(permits)
        .collect();
    assert_eq!(
        cpus.len(),
        permits,
        "acceptance needs {permits} CPUs inside its admitted mask"
    );
    assert!(
        std::thread::available_parallelism().unwrap().get() >= permits,
        "acceptance permit grant exceeds this process's quota"
    );
    let mut mask = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
    for cpu in cpus {
        unsafe {
            libc::CPU_SET(cpu, &mut mask);
        }
    }
    unsafe {
        command.pre_exec(move || {
            if libc::sched_setaffinity(0, std::mem::size_of_val(&mask), &mask) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn f1_corpus_matrix(name: &str, text: &str) {
    f1_corpus_matrix_with_io(
        name,
        text,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "dirs":[]}),
    );
}

fn f1_corpus_matrix_with_io(
    name: &str,
    text: &str,
    grants: impl FnOnce(&Scratch) -> serde_json::Value,
) {
    f1_corpus_matrix_with_exit(name, text, grants, 0);
}

fn f1_corpus_matrix_with_exit(
    name: &str,
    text: &str,
    grants: impl FnOnce(&Scratch) -> serde_json::Value,
    expected_exit: i32,
) {
    f1_corpus_matrix_with_run(
        name,
        text,
        |scratch| serde_json::json!({"io":grants(scratch),"args":[]}),
        expected_exit,
        b"",
    );
}

fn f1_corpus_matrix_with_run(
    name: &str,
    text: &str,
    grants: impl FnOnce(&Scratch) -> serde_json::Value,
    expected_exit: i32,
    stdin: &[u8],
) {
    f1_corpus_matrix_with_live(name, text, grants, expected_exit, stdin, || Box::new(|| {}));
}

fn f1_corpus_matrix_with_live(
    name: &str,
    text: &str,
    grants: impl FnOnce(&Scratch) -> serde_json::Value,
    expected_exit: i32,
    stdin: &[u8],
    mut live_peer: impl FnMut() -> Box<dyn FnOnce()>,
) {
    let scratch = Scratch::new(name);
    let mut grants = grants(&scratch);
    let io = grants["io"].clone();
    let program = scratch.path("main.hir");
    let input = scratch.path("live-stdin");
    std::fs::write(&program, text).unwrap();
    std::fs::write(&input, stdin).unwrap();
    for level in ["record", "durable"] {
        let tape = scratch.path(level);
        let report = scratch.path("projection");
        grants["recording"] = level.into();
        let encoded_grants = grants.to_string();
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .args(["run", "--native"])
            .arg(&program)
            .args(["--grants", &encoded_grants, "--cores", "8", "--record"])
            .arg(&tape)
            .stdin(std::fs::File::open(&input).unwrap())
            .env(hirsel_rt::free_replay::Report::VAR, &report);
        f1_permits(&mut command, 8);
        let stop_peer = live_peer();
        let live = command.output().unwrap();
        stop_peer();
        assert_eq!(
            live.status.code(),
            Some(expected_exit),
            "{name} {level} live: {live:?}"
        );
        assert_eq!(
            std::fs::read(&tape)
                .unwrap()
                .starts_with(b"hirsel-free-tape 2\n"),
            hirsel_rt::free_driver::admission_enabled()
        );
        let projection = if hirsel_rt::free_driver::admission_enabled() {
            Some(std::fs::read_to_string(&report).unwrap())
        } else {
            None
        };
        // Replay must use recorded completions even when the granted tree is
        // unavailable. Restore it only for the next independent live recording.
        let withheld: Vec<_> = io["dirs"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, grant)| {
                let host = PathBuf::from(grant["host"].as_str().unwrap());
                let saved = scratch.path(&format!("withheld-{index}"));
                std::fs::rename(&host, &saved).unwrap();
                (host, saved)
            })
            .collect();
        for engine in ENGINES {
            for width in ["1", "8"] {
                for permits in [1, 8] {
                    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                    command
                        .arg("run")
                        .args(engine)
                        .arg(&program)
                        .arg("--replay")
                        .arg(&tape)
                        .args(["--cores", width])
                        .env(hirsel_rt::free_replay::Report::VAR, &report);
                    f1_permits(&mut command, permits);
                    let replay = command.output().unwrap();
                    let cell = format!("{name} {level} {engine:?} width {width} permits {permits}");
                    assert_eq!(
                        replay.status.code(),
                        live.status.code(),
                        "{cell}: {replay:?}"
                    );
                    assert_eq!(replay.stdout, live.stdout, "{cell}");
                    assert_eq!(replay.stderr, live.stderr, "{cell}");
                    if let Some(projection) = &projection {
                        assert_eq!(
                            &std::fs::read_to_string(&report).unwrap(),
                            projection,
                            "{cell}: full normalized projection"
                        );
                    }
                }
            }
        }
        for (host, saved) in withheld {
            std::fs::rename(saved, host).unwrap();
        }
    }
}

#[test]
fn f1_atomic_corpus_full_projection_width_and_permit_matrix() {
    f1_corpus_matrix(
        "atomic-capture",
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/CLO/CLO-3-child-atomic-capture-run.hir"),
        )
        .unwrap(),
    );
}
#[test]
fn f1_mutex_corpus_full_projection_width_and_permit_matrix() {
    f1_corpus_matrix(
        "mutex-capture",
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/CLO/CLO-3-child-mutex-capture-run.hir"),
        )
        .unwrap(),
    );
}
#[test]
fn f1_task_set_corpus_full_projection_width_and_permit_matrix() {
    f1_corpus_matrix(
        "task-set",
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/CONC/CONC-2-task-set-first-completed-run.hir"),
        )
        .unwrap(),
    );
}

/// The scalar's tape value is independent of Option tag, padding and backend.
#[test]
fn atomic_scalar_tape_bytes_match_interpreter_llvm_and_cranelift() {
    let scratch = Scratch::new("atomic-scalars");
    let program = scratch.path("main.hir");
    let scalars = [
        ("u8", 255u128),
        ("u32", u32::MAX as u128),
        ("u64", u64::MAX as u128),
        ("u128", u128::MAX),
    ];
    let mut text = String::new();
    for (ty, value) in scalars {
        text.push_str(&format!(
            "@noalloc\nfn check_{ty}(x: {ty}) -> () requires x == {value} {{ () }}\n"
        ));
    }
    text.push_str("pub fn main(rt: Io) -> Result[(), GrantError] {\n let tasks = io.tasks(rt)?\n");
    for (ty, value) in scalars {
        text.push_str(&format!(" let a_{ty}: Atomic[{ty}] = atomic.create(tasks, 0)\n atomic.store(tasks, a_{ty}, {value})\n check_{ty}(atomic.load(tasks, a_{ty}))\n"));
    }
    text.push_str(" Ok(())\n}\n");
    std::fs::write(&program, text).unwrap();
    let canonical = |tape: &Path| {
        let tape = hirsel_free::tape::parse(&std::fs::read(tape).unwrap()).unwrap();
        let loads: Vec<_> = tape
            .logs
            .values()
            .flat_map(|bytes| hirsel_free::record::decode(bytes).unwrap())
            .filter_map(|record| match record {
                hirsel_free::record::Record::Load { bits, .. } => Some(bits),
                _ => None,
            })
            .collect();
        assert_eq!(loads, scalars.map(|(_, bits)| bits));
        let mut bytes = Vec::new();
        for bits in loads {
            hirsel_free::record::put_varint(&mut bytes, bits);
        }
        bytes
    };
    let mut expected = None;
    for (index, engine) in ENGINES.into_iter().enumerate() {
        let tape = scratch.path(&format!("f0-{index}"));
        let report = scratch.path(&format!("f0-report-{index}"));
        let output = run(
            engine,
            &program,
            GRANTS,
            &format!(
                "mode=record,tape={},report={},seed=1",
                tape.display(),
                report.display()
            ),
        );
        assert!(output.status.success(), "{engine:?}: {output:?}");
        let bytes = canonical(&tape);
        if let Some(expected) = &expected {
            assert_eq!(&bytes, expected, "{engine:?}: canonical scalar tape bytes");
        } else {
            expected = Some(bytes);
        }
        if index != 0 {
            let grants =
                r#"{"io":{"tasks":"tasks.free@1","dirs":[]},"args":[],"recording":"record"}"#;
            let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .args(engine)
                .arg(&program)
                .args(["--grants", grants, "--cores", "8", "--record"])
                .arg(&tape)
                .output()
                .unwrap();
            assert!(output.status.success(), "F1 {engine:?}: {output:?}");
            if hirsel_rt::free_driver::admission_enabled() {
                assert_eq!(
                    canonical(&tape),
                    *expected.as_ref().unwrap(),
                    "F1 {engine:?}: canonical scalar tape bytes"
                );
            } else {
                assert!(
                    !std::fs::read(&tape)
                        .unwrap()
                        .starts_with(hirsel_free::tape::MAGIC)
                );
                let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .arg("run")
                    .args(engine)
                    .arg(&program)
                    .arg("--replay")
                    .arg(&tape)
                    .output()
                    .unwrap();
                assert!(
                    replay.status.success(),
                    "default admission {engine:?}: {replay:?}"
                );
            }
        }
    }
}

#[test]
fn f1_atomic_all_scalar_widths_full_projection_and_permit_matrix() {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/CLO/CLO-3-child-atomic-capture-run.hir"),
    )
    .unwrap();
    for ty in ["u8", "u64", "u128"] {
        f1_corpus_matrix(&format!("atomic-{ty}"), &text.replace("u32", ty));
    }
}

#[test]
fn f1_storage_and_clock_requests_replay_without_live_host_access() {
    f1_corpus_matrix_with_io(
        "storage-clock",
        r#"
pub type AppError = Grant(GrantError) | Files(FileError) | Clock(ClockError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let clock = io.monotonic_clock(rt) ? Grant
  let _ = clock.monotonic(clock) ? Clock
  let dir = io.dir(rt, path.of("data")) ? Files
  file.write_bytes(dir, path.of("message"), b"owned") ? Files
  let data = file.read_bytes(dir, path.of("message")) ? Files
  out.print(output, if data == b"owned" { "storage ok\n" } else { "wrong\n" }) ? Output
  Ok(())
}
"#,
        |scratch| {
            let host = scratch.path("data");
            std::fs::create_dir(&host).unwrap();
            serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "monotonic_clock":"clock.monotonic.ns@1", "dirs":[{"path":"data","host":host,"access":"read_write"}]})
        },
    );
}

#[test]
fn f1_random_and_sleep_requests_replay_through_recorded_attempts() {
    f1_corpus_matrix_with_io(
        "random-sleep",
        r#"
pub type AppError = Grant(GrantError) | Random(RandomError) | Clock(ClockError) | Wait(WaitError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let random = io.random(rt) ? Grant
  let draw = random.bytes(random, 8) ? Random
  let c = io.monotonic_clock(rt) ? Grant
  let now = clock.monotonic(c) ? Clock
  let later = match instant.checked_add(c, now, 1000000u64) { Some(at) => at, None() => now }
  clock.sleep_until(c, later) ? Wait
  out.print(output, if bytes.len(draw) == 8 { "awake\n" } else { "wrong\n" }) ? Output
  Ok(())
}
"#,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "random":"random.system@1", "monotonic_clock":"clock.monotonic.ns@1", "dirs":[]}),
    );
}

#[test]
fn f1_failed_root_terminal_outcome_replays_exactly() {
    f1_corpus_matrix_with_exit(
        "failed-root",
        "pub fn main(rt: Io) -> Result[(), GrantError] { let tasks = io.tasks(rt)?; failure.check(false, Arithmetic()); Ok(()) }",
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "dirs":[]}),
        hirsel_run_schema::exit::FAILED,
    );
}

#[test]
fn f1_blocked_terminal_outcome_replays_exactly() {
    f1_corpus_matrix_with_exit(
        "blocked-root",
        "pub fn main(rt: Io) -> Result[(), GrantError] { let tasks = io.tasks(rt)?; let (tx, rx): (Sender[u32], Receiver[u32]) = channel.open(tasks); let mut sender = take tx; let mut receiver = take rx; let _ = channel.receive(tasks, mut receiver); let _ = channel.send(tasks, mut sender, 1u32); Ok(()) }",
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "dirs":[]}),
        hirsel_run_schema::exit::BLOCKED,
    );
}

#[test]
fn f1_terminal_restoration_rejects_forged_root_and_unconsumed_records() {
    use hirsel_free::{
        phase::Progress,
        record::{Encoder, Record},
        tape::{self, TaskChunk},
    };
    let scratch = Scratch::new("terminal-forgery");
    let program = scratch.path("main.hir");
    std::fs::write(
        &program,
        "pub fn main(rt: Io) -> Result[(), GrantError] { let tasks = io.tasks(rt)?; Ok(()) }",
    )
    .unwrap();
    let path = scratch.path("record");
    let live = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--native"])
        .arg(&program)
        .args([
            "--cores",
            "8",
            "--grants",
            r#"{"io":{"tasks":"tasks.free@1","dirs":[]},"args":[],"recording":"record"}"#,
            "--record",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(live.status.success(), "{live:?}");
    let original = std::fs::read(&path).unwrap();
    if !hirsel_rt::free_driver::admission_enabled() {
        assert!(!original.starts_with(tape::MAGIC));
        let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&program)
            .arg("--replay")
            .arg(&path)
            .output()
            .unwrap();
        assert!(replay.status.success(), "default admission: {replay:?}");
        return;
    }
    let parsed = tape::parse(&original).unwrap();
    for mutation in ["root", "record", "progress"] {
        let mut tasks: Vec<_> = parsed
            .logs
            .iter()
            .map(|(path, bytes)| TaskChunk {
                path: path.clone(),
                progress: parsed.cut[path],
                bytes: bytes.clone(),
            })
            .collect();
        let root = tasks.iter_mut().find(|task| task.path.is_empty()).unwrap();
        match mutation {
            "root" => {
                let mut records = hirsel_free::record::decode(&root.bytes).unwrap();
                let outcome = records
                    .iter_mut()
                    .rev()
                    .find_map(|record| match record {
                        Record::Observation { op, bytes } if *op == root.progress.op => Some(bytes),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(outcome, &[0; 8]); // Returned(Ok)
                outcome[4] = 1; // Returned(Error), with every other phase unchanged.
                root.bytes.clear();
                let mut encoder = Encoder::default();
                for record in records {
                    encoder.encode(&record, &mut root.bytes);
                }
            }
            "record" => {
                let mut records = hirsel_free::record::decode(&root.bytes).unwrap();
                records.push(Record::Load {
                    op: root.progress.op,
                    bits: 7,
                });
                root.bytes.clear();
                let mut encoder = Encoder::default();
                for record in records {
                    encoder.encode(&record, &mut root.bytes);
                }
            }
            "progress" => root.progress = Progress::new(root.progress.op + 1, root.progress.phase),
            _ => unreachable!(),
        }
        let mut forged = tape::header(&parsed.binding);
        forged.extend(tape::stop(
            &parsed.keys,
            &tasks,
            parsed.end.as_ref().unwrap(),
        ));
        std::fs::write(&path, forged).unwrap();
        for engine in ENGINES {
            let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .args(engine)
                .arg(&program)
                .arg("--replay")
                .arg(&path)
                .args(["--cores", "1"])
                .output()
                .unwrap();
            assert_eq!(
                replay.status.code(),
                Some(hirsel_run_schema::exit::REFUSED),
                "{mutation} {engine:?}: {replay:?}"
            );
            let diagnostic = String::from_utf8_lossy(&replay.stderr);
            assert!(
                if mutation == "root" {
                    if engine.is_empty() {
                        diagnostic.contains("Free root outcome differs from recording")
                    } else {
                        // Native root observations use the same exact byte
                        // comparison as requests, at this root's final op.
                        diagnostic.contains(&format!(
                            "host request differs at [] operation {}",
                            parsed.cut[&Vec::new()].op
                        ))
                    }
                } else {
                    diagnostic.contains("unconsumed") || diagnostic.contains("cut")
                },
                "{mutation} {engine:?}: {diagnostic}"
            );
        }
    }
}

#[test]
fn f1_canceled_sleep_completion_is_consumed_before_terminal_restoration() {
    f1_corpus_matrix_with_io(
        "canceled-sleep",
        r#"
pub type AppError = Grant(GrantError) | Clock(ClockError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let c = io.monotonic_clock(rt) ? Grant
  let now = clock.monotonic(c) ? Clock
  let later = match instant.checked_add(c, now, 3600000000000u64) { Some(at) => at, None() => now }
  let canceled = task.scope(tasks, fn(s: Scope) -> bool {
    let child = task.async(s, fn() -> bool {
      match clock.sleep_until(c, later) { Error(Canceled(Requested())) => true, _ => false }
    })
    task.cancel(s, child)
    task.await(s, take child)
  })
  out.print(output, if canceled { "canceled\n" } else { "wrong\n" }) ? Output
  Ok(())
}
"#,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "monotonic_clock":"clock.monotonic.ns@1", "dirs":[]}),
    );
}

#[test]
fn f1_process_completion_replays_with_its_executable_unavailable() {
    use std::os::unix::fs::PermissionsExt;
    let executable = Scratch::new("recorded-child");
    let host = executable.path("data");
    std::fs::create_dir(&host).unwrap();
    let child = host.join("child");
    std::fs::write(&child, "#!/bin/sh\nprintf 'recorded-child\\n'\n").unwrap();
    std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = format!(
        r#"
pub type AppError = Grant(GrantError) | Process(ProcessError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {{
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let p = io.process(rt) ? Grant
  let ran = proc.run(p, Command(program: path.of("{}"), args: [], dir: None(), env: None(), own_group: false), b"", 64u64, None()) ? Process
  out.print(output, if ran.status == Exited(0) && ran.stdout == b"recorded-child\n" {{ "child ok\n" }} else {{ "wrong\n" }}) ? Output
  Ok(())
}}
"#,
        child.display()
    );
    f1_corpus_matrix_with_io(
        "process-run",
        &source,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "process":{"provider":"process.spawn@1","rules":[{"program":child,"env":[]}]}, "dirs":[{"path":"data","host":host,"access":"read"}]}),
    );
}

#[test]
fn f1_child_streaming_and_narrowing_replay_without_spawning() {
    let executable = Scratch::new("recorded-cat");
    let host = executable.path("data");
    std::fs::create_dir(&host).unwrap();
    let child = host.join("cat");
    std::fs::copy("/bin/cat", &child).unwrap();
    let source = format!(
        r#"
pub type AppError = Grant(GrantError) | Process(ProcessError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {{
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let original = io.process(rt) ? Grant
  let p = proc.narrow(original, [ProcessRule(program: "{}", env: [])]) ? Process
  let mut c = proc.spawn(p, Command(program: path.of("{}"), args: [], dir: None(), env: None(), own_group: false)) ? Process
  child.write(p, mut c, b"streamed\n") ? Process
  child.close_stdin(p, mut c)
  let mut got: List[u8] = []
  while child.read_stdout(p, mut c, mut got, 64u64) ? Process != 0u64 {{}}
  failure.check(bytes.from_list(take got) == b"streamed\n", Bounds())
  let mut errors: List[u8] = []
  failure.check(child.read_stderr(p, mut c, mut errors, 64u64) ? Process == 0u64, Bounds())
  failure.check(child.wait(p, take c, None()) ? Process == Exited(0), Bounds())
  let mut killed = proc.spawn(p, Command(program: path.of("{}"), args: [], dir: None(), env: None(), own_group: false)) ? Process
  child.kill(p, mut killed, Kill())
  failure.check(child.wait(p, take killed, None()) ? Process == Signaled(9), Bounds())
  out.print(output, "streamed and killed\n") ? Output
  Ok(())
}}
"#,
        child.display(),
        child.display(),
        child.display()
    );
    f1_corpus_matrix_with_io(
        "child-streaming",
        &source,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "process":{"provider":"process.spawn@1","rules":[{"program":child,"env":[]}]}, "dirs":[{"path":"data","host":host,"access":"read"}]}),
    );
}

#[test]
fn f1_pipeline_and_group_replay_without_spawning() {
    let executable = Scratch::new("recorded-pipeline");
    let host = executable.path("data");
    std::fs::create_dir(&host).unwrap();
    let child = host.join("cat");
    std::fs::copy("/bin/cat", &child).unwrap();
    let source = format!(
        r#"
fn command() -> Command {{
  Command(program: path.of("{}"), args: [], dir: None(), env: None(), own_group: false)
}}
pub type AppError = Grant(GrantError) | Process(ProcessError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {{
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let p = io.process(rt) ? Grant
  let ran = proc.pipeline(p, [command(), command()], b"pipeline\n", 64u64, None()) ? Process
  failure.check(ran.statuses == [Exited(0), Exited(0)] && ran.stdout == b"pipeline\n", Bounds())
  let mut g = proc.spawn_pipeline(p, [command(), command()], 64u64) ? Process
  group.write(p, mut g, b"group\n") ? Process
  group.close_stdin(p, mut g)
  let mut got: List[u8] = []
  while group.read_stdout(p, mut g, mut got, 64u64) ? Process != 0u64 {{}}
  failure.check(bytes.from_list(take got) == b"group\n", Bounds())
  let done = proc.wait_group(p, take g, None()) ? Process
  failure.check(done.statuses == [Exited(0), Exited(0)], Bounds())
  let mut killed = proc.spawn_pipeline(p, [command(), command()], 64u64) ? Process
  group.kill(p, mut killed, Kill())
  let dead = proc.wait_group(p, take killed, None()) ? Process
  failure.check(dead.statuses == [Signaled(9), Signaled(9)], Bounds())
  out.print(output, "pipelines complete\n") ? Output
  Ok(())
}}
"#,
        child.display()
    );
    f1_corpus_matrix_with_io(
        "pipeline-group",
        &source,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "process":{"provider":"process.spawn@1","rules":[{"program":child,"env":[]}]}, "dirs":[{"path":"data","host":host,"access":"read"}]}),
    );
}

#[test]
fn f1_owned_file_and_child_drops_replay_through_host_phases() {
    let executable = Scratch::new("recorded-drops");
    let host = executable.path("data");
    std::fs::create_dir(&host).unwrap();
    let child = host.join("cat");
    std::fs::copy("/bin/cat", &child).unwrap();
    let source = format!(
        r#"
pub type AppError = Grant(GrantError) | File(FileError) | Process(ProcessError) | Output(OutputError)
fn write(d: Dir) -> Result[(), FileError] {{
  let mut f = file.create(d, path.of("result")) ?
  file.write(d, mut f, b"owned file\n") ?
  Ok(())
}}
pub fn main(rt: Io) -> Result[(), AppError] {{
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let d = io.dir(rt, path.of("data")) ? File
  write(d) ? File
  failure.check(file.read_bytes(d, path.of("result")) ? File == b"owned file\n", Bounds())
  let p = io.process(rt) ? Grant
  let c = proc.spawn(p, Command(program: path.of("{}"), args: [], dir: None(), env: None(), own_group: false)) ? Process
  out.print(output, "owners closed\n") ? Output
  Ok(())
}}
"#,
        child.display()
    );
    f1_corpus_matrix_with_io(
        "owned-drops",
        &source,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "process":{"provider":"process.spawn@1","rules":[{"program":child,"env":[]}]}, "dirs":[{"path":"data","host":host,"access":"read_write"}]}),
    );
}

#[test]
fn f1_environment_and_stdin_replay_without_live_input() {
    f1_corpus_matrix_with_run(
        "input-env",
        r#"
pub type AppError = Grant(GrantError) | Input(InputError) | Env(EnvError) | Output(OutputError)
pub fn main(rt: Io, e: Env) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  failure.check(env.get(e, "SET") ? Env == Some("recorded"), Bounds())
  failure.check(env.get(e, "UNSET") ? Env == None(), Bounds())
  failure.check(env.get(e, "EMPTY") ? Env == Some(""), Bounds())
  failure.check(env.get(e, "DENIED") == Error(NotGranted()), Bounds())
  let narrowed = env.narrow(e, ["SET", "EMPTY"]) ? Env
  failure.check(env.get(narrowed, "SET") ? Env == Some("recorded"), Bounds())
  failure.check(env.get(narrowed, "UNSET") == Error(NotGranted()), Bounds())
  failure.check(match env.narrow(narrowed, ["UNSET"]) { Error(NotGranted()) => true, _ => false }, Bounds())
  let input = io.stdin(rt) ? Grant
  let mut read: List[u8] = []
  while stdin.read(input, mut read, 3u64) ? Input != 0u64 {}
  failure.check(bytes.from_list(take read) == b"recorded stdin\n", Bounds())
  out.print(output, "inputs restored\n") ? Output
  Ok(())
}
"#,
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1", "stdin":"stdin.bytes@1", "tasks":"tasks.free@1", "dirs":[]},"args":[],"env":{"SET":"recorded", "UNSET":null,"EMPTY":""}}),
        0,
        b"recorded stdin\n",
    );
}

#[test]
fn f1_canceled_stdin_consumes_its_checked_completion_once() {
    f1_corpus_matrix_with_io(
        "canceled-stdin",
        r#"
pub type AppError = Grant(GrantError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let input = io.stdin(rt) ? Grant
  let done = task.scope(tasks, fn(s: Scope) -> bool {
    let (tx, rx): (Sender[bool], Receiver[bool]) = channel.open(tasks)
    let mut sender = take tx
    let child = task.async(s, fn() -> bool {
      let mut receiver = take rx
      let _ = channel.receive(tasks, mut receiver)
      let mut read: List[u8] = []
      let canceled = stdin.read(input, mut read, 1u64) == Error(Canceled(Requested()))
      let invalid = stdin.read(input, mut read, 0u64) == Error(InvalidInput())
      canceled && invalid && list.len(read) == 0u64
    })
    task.cancel(s, child)
    let _ = channel.send(tasks, mut sender, true)
    task.await(s, take child)
  })
  failure.check(done, Bounds())
  out.print(output, "canceled input\n") ? Output
  Ok(())
}
"#,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "stdin":"stdin.bytes@1", "tasks":"tasks.free@1", "dirs":[]}),
    );
}

#[test]
fn f1_signal_waits_replay_checked_arrivals_without_a_signal_source() {
    f1_corpus_matrix_with_io(
        "signal-waits",
        r#"
pub type AppError = Grant(GrantError) | Signal(SignalError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let signals = io.signals(rt) ? Grant
  failure.check(signal.wait(signals, []) == Error(InvalidInput()), Bounds())
  failure.check(signal.wait(signals, [Hangup()]) == Error(NotGranted()), Bounds())
  failure.check(signal.wait(signals, [Interrupt(), Terminate()]) ? Signal == Interrupt(), Bounds())
  failure.check(signal.wait(signals, [Terminate()]) ? Signal == Terminate(), Bounds())
  out.print(output, "signals restored\n") ? Output
  Ok(())
}
"#,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "signals":{"provider":"signals.script@1","kinds":["interrupt","terminate"],"script":["interrupt","terminate"]}, "tasks":"tasks.free@1", "dirs":[]}),
    );
}

#[test]
fn f1_canceled_signals_consume_completions_after_program_checks() {
    f1_corpus_matrix_with_io(
        "canceled-signals",
        r#"
pub type AppError = Grant(GrantError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let signals = io.signals(rt) ? Grant
  let done = task.scope(tasks, fn(s: Scope) -> bool {
    let (tx, rx): (Sender[bool], Receiver[bool]) = channel.open(tasks)
    let mut sender = take tx
    let child = task.async(s, fn() -> bool {
      let mut receiver = take rx
      let _ = channel.receive(tasks, mut receiver)
      let canceled = signal.wait(signals, [Interrupt()]) == Error(Canceled(Requested()))
      let invalid = signal.wait(signals, []) == Error(InvalidInput())
      let denied = signal.wait(signals, [Hangup()]) == Error(NotGranted())
      canceled && invalid && denied
    })
    task.cancel(s, child)
    let _ = channel.send(tasks, mut sender, true)
    task.await(s, take child)
  })
  failure.check(done, Bounds())
  out.print(output, "canceled signals\n") ? Output
  Ok(())
}
"#,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "signals":{"provider":"signals.script@1","kinds":["interrupt"],"script":[]}, "tasks":"tasks.free@1", "dirs":[]}),
    );
}

#[test]
fn f1_network_aliases_replay_without_a_tcp_peer() {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener};
    use std::time::Duration;
    // Reuse stream_replay.rs's nonblocking accept + stop channel structure so
    // a failed source compilation cannot leave the peer waiting indefinitely.
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let source = r#"pub type E = Grant(GrantError) | Network(NetError) | Output(OutputError)
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let root = io.net(rt) ? Grant
  let n = net.narrow(root, [NetRule(address: "127.0.0.1", port: PORTu16, operation: Connect())]) ? Network
  let out = io.stdout(rt) ? Grant
  let addresses = net.resolve(n, "127.0.0.1", PORTu16) ? Network
  failure.check(list.len(addresses) > 0u64, Arithmetic())
  let stream = net.connect(n, "127.0.0.1", PORTu16, Unbounded()) ? Network
  let (initial_read, initial_write) = net.split(n, take stream)
  let joined = net.join(n, take initial_read, take initial_write) ? Network
  let (read, write) = net.split(n, take joined)
  let mut r = take read
  let mut w = take write
  write_half.set_timeout(n, mut w, Unbounded()) ? Network
  read_half.set_timeout(n, mut r, Unbounded()) ? Network
  failure.check(write_half.write(n, mut w, b"hi") == Ok(2u64), Arithmetic())
  write_half.write_all(n, mut w, b"!") ? Network
  write_half.shutdown(n, mut w) ? Network
  let mut received: List[u8] = []
  let mut more = true
  while more {
    more = (read_half.read(n, mut r, mut received, 8u64) ? Network) > 0u64
  }
  failure.check(bytes.from_list(take received) == b"ok", Arithmetic())
  read_half.close(n, take r)
  write_half.close(n, take w)
  out.print(out, "tcp aliases ok\n") ? Output
  Ok(())
}
"#.replace("PORT", &port.to_string());
    f1_corpus_matrix_with_live(
        "tcp-aliases",
        &source,
        |_| {
            serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[],
            "net":{"provider":"net.stream@1","rules":[{"address":"127.0.0.1","service":port.to_string(),"operation":"connect"}]}},"args":[]})
        },
        0,
        b"",
        || {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let (stop, stopped) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if !matches!(
                                stopped.recv_timeout(Duration::from_millis(10)),
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                            ) {
                                return;
                            }
                        }
                        Err(e) => panic!("TCP accept: {e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(30)))
                    .unwrap();
                let mut request = Vec::new();
                stream.read_to_end(&mut request).unwrap();
                assert_eq!(request, b"hi!");
                stream.write_all(b"ok").unwrap();
            });
            Box::new(move || {
                let _ = stop.send(());
                server.join().unwrap();
            })
        },
    );
}

#[test]
fn f1_udp_datagrams_replay_without_a_udp_peer() {
    use std::net::{Ipv4Addr, UdpSocket};
    use std::time::Duration;
    let reservation = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let source = r#"pub type E = Grant(GrantError) | Network(NetError)
fn address(port: u16) -> SocketAddr uses {} {
  SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: port)
}
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let n = io.net(rt) ? Grant
  let mut socket = net.udp_bind(n, address(0u16)) ? Network
  failure.check(udp.send_to(n, mut socket, b"q", address(PORTu16)) == Ok(1u64), Arithmetic())
  let mut buffer: List[u8] = []
  let answer = udp.recv_from(n, mut socket, mut buffer, 2u64) ? Network
  let empty = udp.recv_from(n, mut socket, mut buffer, 2u64) ? Network
  failure.check(answer.count == 2u64 && answer.truncated && answer.peer.port == PORTu16 && empty.count == 0u64 && !empty.truncated && buffer == [97u8, 98u8], Arithmetic())
  udp.close(n, take socket)
  Ok(())
}
"#.replace("PORT", &port.to_string());
    f1_corpus_matrix_with_live(
        "udp-datagrams",
        &source,
        |_| {
            serde_json::json!({"io":{"tasks":"tasks.free@1","dirs":[],
            "net":{"provider":"net.stream@1","rules":[
                {"address":"127.0.0.1","service":"0","operation":"udp_bind"},
                {"address":"127.0.0.1","service":port.to_string(),"operation":"udp_peer"}]}},"args":[]})
        },
        0,
        b"",
        || {
            let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_millis(10)))
                .unwrap();
            let (stop, stopped) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                let mut request = [0; 8];
                loop {
                    match socket.recv_from(&mut request) {
                        Ok((count, peer)) => {
                            assert_eq!(&request[..count], b"q");
                            socket.send_to(b"abc", peer).unwrap();
                            socket.send_to(b"", peer).unwrap();
                            return;
                        }
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            if stopped.try_recv().is_ok() {
                                return;
                            }
                        }
                        Err(e) => panic!("UDP recv: {e}"),
                    }
                }
            });
            Box::new(move || {
                let _ = stop.send(());
                server.join().unwrap();
            })
        },
    );
}

#[test]
fn f1_tcp_listener_and_connect_addr_replay_without_sockets() {
    let reservation = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    f1_corpus_matrix_with_io(
        "tcp-listener",
        &r#"pub type E = Grant(GrantError) | Network(NetError)
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let n = io.net(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let listener = net.listen(n, SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: PORTu16)) ? Network
  let address = net.local_addr(n, listener) ? Network
  task.scope(tasks, fn(s: Scope) -> Result[(), E] {
    let server = task.async(s, fn() -> Result[(), E] {
      let mut owned = take listener
      let mut (stream, from) = net.accept(n, mut owned) ? Network
      let peer = net.peer_addr(n, stream) ? Network
      failure.check(peer.port == from.port, Arithmetic())
      let mut buffer: Inline[u8, 3] = [0u8; 3]
      let count = net.read_into(n, mut stream, mut buffer) ? Network
      failure.check(count == 1u64 && buffer[0u64] == 120u8 && buffer[2u64] == 0u8, Arithmetic())
      net.write_all(n, mut stream, b"y") ? Network
      net.close(n, take stream)
      Ok(())
    })
    let client = task.async(s, fn() -> Result[(), E] {
      let mut stream = net.connect_addr(n, address, Unbounded()) ? Network
      net.write_all(n, mut stream, b"x") ? Network
      let mut buffer: List[u8] = []
      let count = net.read(n, mut stream, mut buffer, 1u64) ? Network
      failure.check(count == 1u64 && buffer == [121u8], Arithmetic())
      net.shutdown(n, mut stream, Both()) ? Network
      net.close(n, take stream)
      Ok(())
    })
    task.await(s, take server)?
    task.await(s, take client)?
    Ok(())
  })
}
"#
        .replace("PORT", &port.to_string()),
        |_| {
            serde_json::json!({"tasks":"tasks.free@1","dirs":[],"net":{"provider":"net.stream@1","rules":[
            {"address":"127.0.0.1","service":port.to_string(),"operation":"listen"},
            {"address":"127.0.0.1","service":port.to_string(),"operation":"connect"}]}})
        },
    );
}

#[test]
fn f1_tls_client_setup_replays_without_a_tls_peer() {
    f1_tls_client_matrix("tls.connect_with(n, take stream, \"localhost\", client)");
}

#[test]
fn f1_tls_basic_and_identity_connect_replay_without_a_tls_peer() {
    f1_tls_client_matrix("tls.connect(n, take stream, \"localhost\", Pem(bCA))");
    f1_tls_client_matrix(
        "tls.connect_as(n, take stream, \"localhost\", Pem(bCA), TlsIdentity(chain: bCHAIN, key: bKEY))",
    );
}

fn f1_tls_client_matrix(upgrade: &str) {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::Arc;
    use std::time::Duration;
    // rustls 0.23.46 public test-ca fixtures, reused by rt/tests/support/tls_peer.rs.
    const CA: &str = include_str!("../../hirsel-rt/tests/it/tls-data/ca.cert");
    const CHAIN: &str = include_str!("../../hirsel-rt/tests/it/tls-data/end.fullchain");
    const KEY: &str = include_str!("../../hirsel-rt/tests/it/tls-data/end.key");
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let source = r#"pub type E = Grant(GrantError) | Network(NetError)
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let n = io.net(rt) ? Grant
  let stream = net.connect(n, "127.0.0.1", PORTu16, Unbounded()) ? Network
  let client = TlsClient(roots: Pem(bCA), identity: Some(TlsIdentity(chain: bCHAIN, key: bKEY)), alpn: ["h2"])
  let mut secure = UPGRADE ? Network
  failure.check(tls.negotiated_protocol(n, secure) == Ok(PROTOCOL), Arithmetic())
  failure.check(match tls.peer(n, secure) ? Network { Some(chain) => bytes.len(chain) > 0u64, None() => false }, Arithmetic())
  net.write_all(n, mut secure, b"ping") ? Network
  let mut buffer: List[u8] = []
  let mut more = true
  while more { more = (net.read(n, mut secure, mut buffer, 8u64) ? Network) > 0u64 }
  failure.check(bytes.from_list(take buffer) == b"pong", Arithmetic())
  tls.close(n, take secure) ? Network
  Ok(())
}
"#.replace("PROTOCOL", if upgrade.contains("connect_with") { "Some(\"h2\")" } else { "None()" })
      .replace("UPGRADE", upgrade)
      .replace("PORT", &port.to_string())
      .replace("CA", &format!("{CA:?}"))
      .replace("CHAIN", &format!("{CHAIN:?}"))
      .replace("KEY", &format!("{KEY:?}"));
    f1_corpus_matrix_with_live(
        "tls-client",
        &source,
        |_| {
            serde_json::json!({"io":{"tasks":"tasks.free@1","dirs":[],"net":{"provider":"net.stream@1","rules":[
            {"address":"127.0.0.1","service":port.to_string(),"operation":"connect"}]}},"args":[]})
        },
        0,
        b"",
        || {
            let certs = CertificateDer::pem_slice_iter(CHAIN.as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let key = PrivateKeyDer::from_pem_slice(KEY.as_bytes()).unwrap();
            let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap();
            config.alpn_protocols = vec![b"h2".to_vec()];
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let (stop, stopped) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                let socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if !matches!(
                                stopped.recv_timeout(Duration::from_millis(10)),
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                            ) {
                                return;
                            }
                        }
                        Err(e) => panic!("TLS accept: {e}"),
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let conn = rustls::ServerConnection::new(Arc::new(config)).unwrap();
                let mut stream = rustls::StreamOwned::new(conn, socket);
                let mut request = [0; 4];
                stream.read_exact(&mut request).unwrap();
                assert_eq!(&request, b"ping");
                stream.write_all(b"pong").unwrap();
                stream.conn.send_close_notify();
                stream.flush().unwrap();
                assert_eq!(stream.read(&mut [0]).unwrap(), 0);
            });
            Box::new(move || {
                let _ = stop.send(());
                server.join().unwrap();
            })
        },
    );
}

#[test]
fn f1_tls_server_mutual_auth_replays_without_handshakes() {
    const CA: &str = include_str!("../../hirsel-rt/tests/it/tls-data/ca.cert");
    const CHAIN: &str = include_str!("../../hirsel-rt/tests/it/tls-data/end.fullchain");
    const KEY: &str = include_str!("../../hirsel-rt/tests/it/tls-data/end.key");
    const CLIENT_CA: &str = include_str!("../../hirsel-rt/tests/it/tls-data/client-ca.cert");
    const CLIENT_CHAIN: &str = include_str!("../../hirsel-rt/tests/it/tls-data/client.cert");
    const CLIENT_KEY: &str = include_str!("../../hirsel-rt/tests/it/tls-data/client.key");
    let reservation = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let source = r#"pub type E = Grant(GrantError) | Network(NetError)
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let n = io.net(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let address = SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: PORTu16)
  let listener = net.listen(n, address) ? Network
  task.scope(tasks, fn(s: Scope) -> Result[(), E] {
    let server = task.async(s, fn() -> Result[(), E] {
      let mut owned = take listener
      let (stream, _) = net.accept(n, mut owned) ? Network
      let config = TlsServer(identities: [TlsIdentity(chain: b{{CHAIN}}, key: b{{KEY}})], clients: Require(b{{CLIENT_CA}}), alpn: ["h2"])
      let mut secure = tls.accept(n, take stream, config) ? Network
      failure.check(tls.negotiated_protocol(n, secure) == Ok(Some("h2")), Arithmetic())
      failure.check(match tls.peer(n, secure) ? Network { Some(chain) => bytes.len(chain) > 0u64, None() => false }, Arithmetic())
      let mut buffer: List[u8] = []
      failure.check((net.read(n, mut secure, mut buffer, 1u64) ? Network) == 1u64 && buffer == [120u8], Arithmetic())
      net.write_all(n, mut secure, b"y") ? Network
      tls.close(n, take secure) ? Network
      Ok(())
    })
    let client = task.async(s, fn() -> Result[(), E] {
      let stream = net.connect_addr(n, address, Unbounded()) ? Network
      let config = TlsClient(roots: Pem(b{{CA}}), identity: Some(TlsIdentity(chain: b{{CLIENT_CHAIN}}, key: b{{CLIENT_KEY}})), alpn: ["h2"])
      let mut secure = tls.connect_with(n, take stream, "localhost", config) ? Network
      net.write_all(n, mut secure, b"x") ? Network
      let mut buffer: List[u8] = []
      failure.check((net.read(n, mut secure, mut buffer, 1u64) ? Network) == 1u64 && buffer == [121u8], Arithmetic())
      tls.close(n, take secure) ? Network
      Ok(())
    })
    task.await(s, take server)?
    task.await(s, take client)?
    Ok(())
  })
}
"#.replace("PORT", &port.to_string())
      .replace("{{CLIENT_CHAIN}}", &format!("{CLIENT_CHAIN:?}"))
      .replace("{{CLIENT_KEY}}", &format!("{CLIENT_KEY:?}"))
      .replace("{{CLIENT_CA}}", &format!("{CLIENT_CA:?}"))
      .replace("{{CHAIN}}", &format!("{CHAIN:?}"))
      .replace("{{KEY}}", &format!("{KEY:?}"))
      .replace("{{CA}}", &format!("{CA:?}"));
    f1_corpus_matrix_with_io("tls-server", &source, |_| {
        serde_json::json!({"tasks":"tasks.free@1","dirs":[],
        "net":{"provider":"net.stream@1","rules":[
            {"address":"127.0.0.1","service":port.to_string(),"operation":"listen_tls"},
            {"address":"127.0.0.1","service":port.to_string(),"operation":"connect"}]}})
    });
}

#[test]
fn f1_unix_streams_replay_without_socket_paths() {
    let socket = Scratch::new("unix-endpoint");
    let path = socket.path("peer.sock");
    let source = r#"pub type E = Grant(GrantError) | Network(NetError)
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let n = io.net(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let address = path.of(SOCKET)
  let listener = net.unix_listen(n, address) ? Network
  task.scope(tasks, fn(s: Scope) -> Result[(), E] {
    let server = task.async(s, fn() -> Result[(), E] {
      let mut owned = take listener
      let mut (stream, _) = net.accept(n, mut owned) ? Network
      let mut buffer: List[u8] = []
      failure.check((net.read(n, mut stream, mut buffer, 1u64) ? Network) == 1u64 && buffer == [120u8], Arithmetic())
      net.write_all(n, mut stream, b"y") ? Network
      net.close(n, take stream)
      Ok(())
    })
    let client = task.async(s, fn() -> Result[(), E] {
      let mut stream = net.unix_connect(n, path.of(SOCKET)) ? Network
      net.write_all(n, mut stream, b"x") ? Network
      let mut buffer: List[u8] = []
      failure.check((net.read(n, mut stream, mut buffer, 1u64) ? Network) == 1u64 && buffer == [121u8], Arithmetic())
      net.close(n, take stream)
      Ok(())
    })
    task.await(s, take server)?
    task.await(s, take client)?
    Ok(())
  })
}
"#.replace("SOCKET", &format!("{:?}", path.to_str().unwrap()));
    f1_corpus_matrix_with_io("unix-stream", &source, |_| {
        serde_json::json!({"tasks":"tasks.free@1","dirs":[],
        "net":{"provider":"net.stream@1","rules":[
            {"address":path,"service":"0","operation":"unix_listen"},
            {"address":path,"service":"0","operation":"unix_connect"}]}})
    });
    assert!(!path.exists(), "owned listener must unlink its socket");
}

#[test]
fn f1_canceled_network_requests_replay_checked_precedence() {
    f1_corpus_matrix_with_io(
        "canceled-network",
        r#"pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let n = io.net(rt)?
  let bind_address = SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: 0u16)
  let listener = match net.listen(n, bind_address) { Ok(value) => take value, Error(_) => { failure.check(false, Arithmetic()); return Ok(()) } }
  let socket = match net.udp_bind(n, bind_address) { Ok(value) => take value, Error(_) => { failure.check(false, Arithmetic()); return Ok(()) } }
  let (sender, receiver): (Sender[bool], Receiver[bool]) = channel.open(tasks)
  task.scope(tasks, fn(s: Scope) {
    let mut tx = take sender
    let child = task.async(s, fn() {
      let mut rx = take receiver
      let mut owned = take listener
      let mut datagram = take socket
      let _ = channel.receive(tasks, mut rx)
      failure.check(match net.accept(n, mut owned) { Error(Canceled(Requested())) => true, _ => false }, Arithmetic())
      failure.check(net.resolve(n, "INVALID", 7u16) == Error(InvalidInput()), Arithmetic())
      failure.check(net.resolve(n, "example.com", 7u16) == Error(NotGranted()), Arithmetic())
      failure.check(net.resolve(n, "127.0.0.1", 7u16) == Error(Canceled(Requested())), Arithmetic())
      failure.check(match net.connect(n, "127.0.0.1", 7u16, Nanos(0u64)) { Error(InvalidInput()) => true, _ => false }, Arithmetic())
      failure.check(match net.connect(n, "127.0.0.1", 7u16, Unbounded()) { Error(Canceled(Requested())) => true, _ => false }, Arithmetic())
      let mut buffer: List[u8] = [7u8]
      failure.check(match udp.recv_from(n, mut datagram, mut buffer, 1u64) { Error(Canceled(Requested())) => true, _ => false }, Arithmetic())
      failure.check(match udp.recv_from(n, mut datagram, mut buffer, 0u64) { Error(InvalidInput()) => true, _ => false }, Arithmetic())
      let peer = SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: 7u16)
      failure.check(udp.send_to(n, mut datagram, b"no", peer) == Error(Canceled(Requested())), Arithmetic())
      failure.check(buffer == [7u8], Arithmetic())
      udp.close(n, take datagram)
    })
    task.cancel(s, child)
    let _ = channel.send(tasks, mut tx, true)
    task.await(s, take child)
  })
  Ok(())
}
"#,
        |_| {
            serde_json::json!({"tasks":"tasks.free@1","dirs":[],"net":{"provider":"net.stream@1","rules":[
            {"address":"127.0.0.1","service":"0","operation":"listen"},
            {"address":"127.0.0.1","service":"0","operation":"udp_bind"},
            {"address":"127.0.0.1","service":"7","operation":"udp_peer"},
            {"address":"127.0.0.1","service":"7","operation":"connect"}]}})
        },
    );
}
