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

// Compile children share the action's memory allowance across test bodies
// and prefix workers. Match Condvar::wait_while's predicate-under-lock pattern
// in Rust std a7c37f9f, library/std/src/sync/poison/condvar.rs:182-193.
static COMPILERS: (std::sync::Mutex<usize>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());

struct CompilerSlot;
impl CompilerSlot {
    fn take() -> Self {
        let (active, available) = &COMPILERS;
        let mut active = available
            .wait_while(active.lock().unwrap(), |active| *active == 2)
            .unwrap();
        *active += 1;
        Self
    }
}
impl Drop for CompilerSlot {
    fn drop(&mut self) {
        let (active, available) = &COMPILERS;
        let mut active = active.lock().unwrap();
        *active -= 1;
        available.notify_one();
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
    let _compiler = engine.is_empty().then(CompilerSlot::take);
    // Concurrent replays of one recording each capture their own output.
    static CHILD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let child = CHILD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stdout = program.with_extension(format!("child-{child}.stdout"));
    let stderr = program.with_extension(format!("child-{child}.stderr"));
    let mut command = if engine.is_empty() {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run").arg(program).arg("--grants").arg(grants);
        command
    } else {
        let mut command = Command::new(native_trace(engine, program, grants));
        command.args(["--hirsel-grants", grants]);
        command
    };
    command
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

// A trace source is immutable for its scratch directory's lifetime. Compile
// each backend/fuel configuration once, then run every prefix on that image,
// as the existing standalone external-stop and queued-stop fixtures do.
fn native_trace(engine: &[&str], program: &Path, grants: &str) -> PathBuf {
    static BUILDS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let backend = match engine {
        ["--native"] => "llvm",
        ["--native", "--backend", "cranelift"] => "cranelift",
        _ => panic!("unsupported native trace engine: {engine:?}"),
    };
    let grants: serde_json::Value = serde_json::from_str(grants).unwrap();
    let fueled = !grants["budgets"]["fuel"].is_null();
    let executable = program.with_extension(format!("{backend}-fuel-{fueled}"));
    let _build = BUILDS.lock().unwrap();
    if !executable.exists() {
        let mut build = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        build
            .args(["build", "--backend", backend])
            .arg(program)
            .arg("-o")
            .arg(&executable);
        if fueled {
            build.arg("--fuel");
        }
        let output = bounded_output(&mut build, program, "native trace build");
        assert!(output.status.success(), "{engine:?}: {output:?}");
    }
    executable
}

/// Production acceptance children use the same bounded watchdog as test-channel
/// runs. File capture also prevents a detached native runner retaining a pipe.
fn bounded_output(command: &mut Command, program: &Path, cell: &str) -> Output {
    use std::time::{Duration, Instant};
    let _compiler = CompilerSlot::take();
    let stdout = program.with_extension("matrix.stdout");
    let stderr = program.with_extension("matrix.stderr");
    command
        .process_group(0)
        .stdout(std::fs::File::create(&stdout).unwrap())
        .stderr(std::fs::File::create(&stderr).unwrap());
    eprintln!("Free matrix start {cell}");
    let started = Instant::now();
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
    assert!(
        !timed_out,
        "{cell}: {command:?} exceeded 120 seconds: {output:?}"
    );
    eprintln!(
        "Free matrix ended {cell}: {} in {:.1}s",
        output.status,
        started.elapsed().as_secs_f64()
    );
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
    // Every cut of this one recording, truncated and torn. Native prefixes
    // share the compiled image, so only runtime processes run concurrently.
    let prefixes: Vec<_> = recorded.cuts[..recorded.cuts.len() - 1]
        .iter()
        .flat_map(|(offset, projection)| [0, TORN].map(|extra| (*offset, extra, projection)))
        .collect();
    let width = std::thread::available_parallelism().map_or(1, |n| n.get());
    for group in prefixes.chunks(width) {
        std::thread::scope(|scope| {
            for &(offset, extra, projection) in group {
                let (program, bytes) = (&program, &bytes);
                scope.spawn(move || {
                    let prefix = recorded.scratch.path(&format!("prefix-{offset}-{extra}"));
                    std::fs::write(&prefix, &bytes[..(offset + extra).min(bytes.len())]).unwrap();
                    assert_eq!(
                        replayed(engine, program, grants, &prefix, 3),
                        format!("replayed {projection}"),
                        "{what}: the cut at {offset} (+{extra})"
                    );
                });
            }
        });
    }
}

/// Every recording of trace `name` on `engine` replays to its live projection
/// at every cut. Each trace and engine is one case: a native run spends most of
/// its time compiling.
fn every_f0_trace(name: &str, engine: &[&str], recordings: &[bool]) {
    let &(_, grants, outcome) = TRACES.iter().find(|(trace, ..)| *trace == name).unwrap();
    for seed in SEEDS {
        for &durable in recordings {
            let recorded = record(engine, name, grants, seed, durable);
            verify(engine, grants, outcome, &recorded);
        }
    }
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_gap_interpreter() {
    every_f0_trace_all("gap", ENGINES[0]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_gap_llvm_plain() {
    every_f0_trace("gap", ENGINES[1], &[false]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_gap_llvm_durable() {
    every_f0_trace("gap", ENGINES[1], &[true]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_gap_cranelift() {
    every_f0_trace_all("gap", ENGINES[2]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_finite_release_interpreter() {
    every_f0_trace_all("finite_release", ENGINES[0]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_finite_release_llvm() {
    every_f0_trace_all("finite_release", ENGINES[1]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_finite_release_cranelift() {
    every_f0_trace_all("finite_release", ENGINES[2]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_mutex_cycle_interpreter_plain() {
    every_f0_trace("mutex_cycle", ENGINES[0], &[false]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_mutex_cycle_interpreter_durable() {
    every_f0_trace("mutex_cycle", ENGINES[0], &[true]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_mutex_cycle_llvm_plain() {
    every_f0_trace("mutex_cycle", ENGINES[1], &[false]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_mutex_cycle_llvm_durable() {
    every_f0_trace("mutex_cycle", ENGINES[1], &[true]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_mutex_cycle_cranelift_plain() {
    every_f0_trace("mutex_cycle", ENGINES[2], &[false]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_mutex_cycle_cranelift_durable() {
    every_f0_trace("mutex_cycle", ENGINES[2], &[true]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_parked_receive_interpreter_plain() {
    every_f0_trace("parked_receive", ENGINES[0], &[false]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_parked_receive_interpreter_durable() {
    every_f0_trace("parked_receive", ENGINES[0], &[true]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_parked_receive_llvm() {
    every_f0_trace_all("parked_receive", ENGINES[1]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_parked_receive_cranelift() {
    every_f0_trace_all("parked_receive", ENGINES[2]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_nested_root_interpreter() {
    every_f0_trace_all("nested_root", ENGINES[0]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_nested_root_llvm() {
    every_f0_trace_all("nested_root", ENGINES[1]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_nested_root_cranelift() {
    every_f0_trace_all("nested_root", ENGINES[2]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_acquire_reply_interpreter() {
    every_f0_trace_all("acquire_reply", ENGINES[0]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_acquire_reply_llvm_plain() {
    every_f0_trace("acquire_reply", ENGINES[1], &[false]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_acquire_reply_llvm_durable() {
    every_f0_trace("acquire_reply", ENGINES[1], &[true]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_acquire_reply_cranelift_plain() {
    every_f0_trace("acquire_reply", ENGINES[2], &[false]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_acquire_reply_cranelift_durable() {
    every_f0_trace("acquire_reply", ENGINES[2], &[true]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_coalesced_root_interpreter() {
    every_f0_trace_all("coalesced_root", ENGINES[0]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_coalesced_root_llvm() {
    every_f0_trace_all("coalesced_root", ENGINES[1]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_coalesced_root_cranelift() {
    every_f0_trace_all("coalesced_root", ENGINES[2]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_thaw_race_interpreter_plain() {
    every_f0_trace("thaw_race", ENGINES[0], &[false]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_thaw_race_interpreter_durable() {
    every_f0_trace("thaw_race", ENGINES[0], &[true]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_thaw_race_llvm() {
    every_f0_trace_all("thaw_race", ENGINES[1]);
}

#[test]
fn every_f0_trace_replays_to_its_live_projection_at_every_cut_thaw_race_cranelift() {
    every_f0_trace_all("thaw_race", ENGINES[2]);
}

fn every_f0_trace_all(name: &str, engine: &[&str]) {
    every_f0_trace(name, engine, &[false, true]);
}

fn racing_trace_all(name: &str, outcomes: [&str; 2], engine: &[&str]) {
    racing_trace(name, &outcomes, engine);
}

/// A trace whose outcome a schedule decides reaches each outcome under some
/// seed on `engine`, and that seed's durable recording replays as above.
fn racing_trace(name: &str, outcomes: &[&str], engine: &[&str]) {
    // Seeds are searched in order, a group of concurrent recordings at a time;
    // the smallest seed that prints `want` is the one a sequential search finds.
    let seeds: Vec<u64> = (0..64).collect();
    let width = if engine.is_empty() {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    } else {
        2
    };
    for &want in outcomes {
        let seed = seeds
            .chunks(width)
            .find_map(|group| {
                std::thread::scope(|scope| {
                    let recordings: Vec<_> = group
                        .iter()
                        .map(|&seed| {
                            scope.spawn(move || {
                                (
                                    seed,
                                    record(engine, name, GRANTS, seed, false).stdout == want,
                                )
                            })
                        })
                        .collect();
                    recordings
                        .into_iter()
                        .map(|recording| recording.join().unwrap())
                        .filter(|&(_, prints)| prints)
                        .map(|(seed, _)| seed)
                        .min()
                })
            })
            .unwrap_or_else(|| panic!("{name} on {engine:?}: no seed prints {want:?}"));
        let recorded = record(engine, name, GRANTS, seed, true);
        assert_eq!(recorded.stdout, want, "{}", recorded.what);
        verify(engine, GRANTS, "returned", &recorded);
    }
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_mutex_cycle_interpreter_first() {
    racing_trace("mutex_cycle", &["thawed\n"], ENGINES[0]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_mutex_cycle_interpreter_second() {
    racing_trace("mutex_cycle", &["shared\n"], ENGINES[0]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_mutex_cycle_llvm() {
    racing_trace_all("mutex_cycle", ["thawed\n", "shared\n"], ENGINES[1]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_mutex_cycle_cranelift_first() {
    racing_trace("mutex_cycle", &["thawed\n"], ENGINES[2]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_mutex_cycle_cranelift_second() {
    racing_trace("mutex_cycle", &["shared\n"], ENGINES[2]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_acquire_reply_interpreter_first() {
    racing_trace("acquire_reply", &["a first\n"], ENGINES[0]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_acquire_reply_interpreter_second() {
    racing_trace("acquire_reply", &["b first\n"], ENGINES[0]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_acquire_reply_llvm_first() {
    racing_trace("acquire_reply", &["a first\n"], ENGINES[1]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_acquire_reply_llvm_second() {
    racing_trace("acquire_reply", &["b first\n"], ENGINES[1]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_acquire_reply_cranelift_first() {
    racing_trace("acquire_reply", &["a first\n"], ENGINES[2]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_acquire_reply_cranelift_second() {
    racing_trace("acquire_reply", &["b first\n"], ENGINES[2]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_thaw_race_interpreter_first() {
    racing_trace("thaw_race", &["thawed\n"], ENGINES[0]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_thaw_race_interpreter_second() {
    racing_trace("thaw_race", &["shared\n"], ENGINES[0]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_thaw_race_llvm() {
    racing_trace_all("thaw_race", ["thawed\n", "shared\n"], ENGINES[1]);
}

#[test]
fn the_racing_traces_reach_both_outcomes_and_replay_thaw_race_cranelift() {
    racing_trace_all("thaw_race", ["thawed\n", "shared\n"], ENGINES[2]);
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
    native_returned_rest_completion_at(false);
}

#[test]
fn native_returned_rest_completion_replays_across_engines_durable() {
    native_returned_rest_completion_at(true);
}

fn native_returned_rest_completion_at(durable: bool) {
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
    let calls: Vec<_> = recorded
        .cuts
        .iter()
        .filter(|(_, projection)| projection.contains("[]: Progress { op: 5, phase: Entered }"))
        .collect();
    assert!(!calls.is_empty(), "{}: no calling cut", recorded.what);
    let cases: Vec<_> = calls
        .into_iter()
        .flat_map(|(offset, projection)| {
            [0, TORN].into_iter().flat_map(move |extra| {
                ENGINES
                    .into_iter()
                    .enumerate()
                    .map(move |(index, engine)| (*offset, extra, index, engine, projection))
            })
        })
        .collect();
    let width = std::thread::available_parallelism().map_or(1, |n| n.get());
    for group in cases.chunks(width) {
        std::thread::scope(|scope| {
            for &(offset, extra, index, engine, projection) in group {
                let (program, bytes, recorded) = (&program, &bytes, &recorded);
                scope.spawn(move || {
                    let prefix = recorded
                        .scratch
                        .path(&format!("prefix-{offset}-{extra}-{index}"));
                    std::fs::write(&prefix, &bytes[..(offset + extra).min(bytes.len())]).unwrap();
                    assert_eq!(
                        replayed(engine, program, GRANTS, &prefix, 3),
                        format!("replayed {projection}"),
                        "{} calling cut {offset} (+{extra}) on {engine:?}",
                        recorded.what
                    );
                });
            }
        });
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
    assert!(
        bytes.starts_with(b"hirsel-free-tape 2\n"),
        "ordinary admission records F1 phases"
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
    assert!(
        std::fs::read(&tape)
            .unwrap()
            .starts_with(b"hirsel-free-tape 2\n")
    );
    let live_projection = Some(std::fs::read_to_string(&report).unwrap());
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
    f1_corpus_matrix_with_exit_cells(name, text, grants, expected_exit, Cells::ALL);
}

fn f1_corpus_matrix_with_exit_cells(
    name: &str,
    text: &str,
    grants: impl FnOnce(&Scratch) -> serde_json::Value,
    expected_exit: i32,
    cells: Cells,
) {
    f1_corpus_matrix_with_live(
        name,
        text,
        |scratch| serde_json::json!({"io":grants(scratch),"args":[]}),
        expected_exit,
        b"",
        || Box::new(|| {}),
        cells,
    );
}

fn f1_corpus_matrix_with_run(
    name: &str,
    text: &str,
    grants: impl FnOnce(&Scratch) -> serde_json::Value,
    expected_exit: i32,
    stdin: &[u8],
) {
    f1_corpus_matrix_with_live(
        name,
        text,
        grants,
        expected_exit,
        stdin,
        || Box::new(|| {}),
        Cells::ALL,
    );
}

fn f1_corpus_matrix_with_live(
    name: &str,
    text: &str,
    grants: impl FnOnce(&Scratch) -> serde_json::Value,
    expected_exit: i32,
    stdin: &[u8],
    live_peer: impl FnMut() -> Box<dyn FnOnce()>,
    cells: Cells,
) {
    f1_corpus_matrix_cells(
        name,
        text,
        grants,
        (expected_exit, |_, _, _, _| {}),
        stdin,
        live_peer,
        cells,
    );
}

type LiveWitness = fn(&Output, &hirsel_free::tape::Tape, &Scratch, &str);

/// A bounded part of a full matrix: these recording levels, each replayed on
/// these engines at every width and permit count. A case whose whole matrix
/// exceeds one test action's limit runs one part per case.
#[derive(Clone, Copy)]
struct Cells {
    levels: &'static [&'static str],
    engines: &'static [&'static [&'static str]],
    dimensions: &'static [(&'static str, usize)],
}

impl Cells {
    const DIMENSIONS: &'static [(&'static str, usize)] = &[("1", 1), ("1", 8), ("8", 1), ("8", 8)];
    const ALL: Cells = Cells {
        levels: &["record", "durable"],
        engines: &ENGINES,
        dimensions: Self::DIMENSIONS,
    };
    const INTERPRETER: &'static [&'static [&'static str]] = &[&[]];
    const NATIVE: &'static [&'static [&'static str]] =
        &[&["--native"], &["--native", "--backend", "cranelift"]];
    const LLVM: &'static [&'static [&'static str]] = &[&["--native"]];
    const CRANELIFT: &'static [&'static [&'static str]] =
        &[&["--native", "--backend", "cranelift"]];
    const fn part(durable: bool, native: bool) -> Cells {
        Cells::at(
            durable,
            if native {
                Cells::NATIVE
            } else {
                Cells::INTERPRETER
            },
        )
    }
    /// One recording level, replayed on `engines`.
    const fn at(durable: bool, engines: &'static [&'static [&'static str]]) -> Cells {
        Cells {
            levels: if durable { &["durable"] } else { &["record"] },
            engines,
            dimensions: Self::DIMENSIONS,
        }
    }
    /// One recording level, replayed on every engine.
    const fn level(durable: bool) -> Cells {
        Cells::at(durable, &ENGINES)
    }
}

/// Check the required live state before comparing its complete replay matrix.
fn f1_corpus_matrix_with_witness(
    name: &str,
    text: &str,
    grants: impl FnOnce(&Scratch) -> serde_json::Value,
    expected: (i32, LiveWitness),
    stdin: &[u8],
    live_peer: impl FnMut() -> Box<dyn FnOnce()>,
) {
    f1_corpus_matrix_cells(name, text, grants, expected, stdin, live_peer, Cells::ALL);
}

fn f1_corpus_matrix_cells(
    name: &str,
    text: &str,
    grants: impl FnOnce(&Scratch) -> serde_json::Value,
    expected: (i32, LiveWitness),
    stdin: &[u8],
    mut live_peer: impl FnMut() -> Box<dyn FnOnce()>,
    cells: Cells,
) {
    let scratch = Scratch::new(name);
    let mut grants = grants(&scratch);
    let io = grants["io"].clone();
    let program = scratch.path("main.hir");
    let input = scratch.path("live-stdin");
    std::fs::write(&program, text).unwrap();
    std::fs::write(&input, stdin).unwrap();
    for &level in cells.levels {
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
        let live = bounded_output(
            &mut command,
            &program,
            &format!("{name} {level} live LLVM width 8 permits 8"),
        );
        stop_peer();
        assert_eq!(
            live.status.code(),
            Some(expected.0),
            "{name} {level} live: {live:?}"
        );
        assert!(
            std::fs::read(&tape)
                .unwrap()
                .starts_with(b"hirsel-free-tape 2\n")
        );
        let bytes = std::fs::read(&tape).unwrap();
        (expected.1)(
            &live,
            &hirsel_free::tape::parse(&bytes).unwrap(),
            &scratch,
            level,
        );
        let projection = Some(std::fs::read_to_string(&report).unwrap());
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
        for &engine in cells.engines {
            for &(width, permits) in cells.dimensions {
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
                let cell = format!("{name} {level} {engine:?} width {width} permits {permits}");
                command.stdin(std::process::Stdio::null());
                let replay = bounded_output(&mut command, &program, &cell);
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
        for (host, saved) in withheld {
            std::fs::rename(saved, host).unwrap();
        }
    }
}

/// The child atomic capture corpus with its `u32` cells as `ty`, at one
/// recording level: each level is its own case, inside the action's time.
fn f1_atomic_at(name: &str, ty: &str, level: &'static [&'static str; 1]) {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/CLO/CLO-3-child-atomic-capture-run.hir"),
    )
    .unwrap();
    f1_corpus_matrix_cells(
        &format!("{name}-{}", level[0]),
        &text.replace("u32", ty),
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "dirs":[]},"args":[]}),
        (0, |_, _, _, _| {}),
        b"",
        || Box::new(|| {}),
        Cells {
            levels: level,
            engines: &ENGINES,
            dimensions: Cells::DIMENSIONS,
        },
    );
}

#[test]
fn f1_atomic_corpus_full_projection_width_and_permit_matrix() {
    f1_atomic_at("atomic-capture", "u32", &["record"]);
}

#[test]
fn f1_atomic_corpus_full_projection_width_and_permit_matrix_durable() {
    f1_atomic_at("atomic-capture", "u32", &["durable"]);
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
/// Two children each loop on a disjoint mutex and a contended shared mutex
/// beside atomic and channel operations. Role-free immediate custody records
/// the same per-object histories that every engine, width and permit count
/// replays. Each recording level is its own case, inside the action's time.
const RUNTIME50_OBJECTS: &str = r#"
pub type AppError = Grant(GrantError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let shared: Mutex[u32] = mutex.create(tasks, 0)
  let hits: Atomic[u32] = atomic.create(tasks, 0)
  let total = task.scope(tasks, fn(s: Scope) -> u32 {
    let (sender, receiver): (Sender[u32], Receiver[u32]) = channel.open(tasks)
    let first = sender.dup(tasks, sender)
    let second = sender.dup(tasks, sender)
    let a = task.async(s, fn() -> () {
      let contended = copy shared
      let own: Mutex[u32] = mutex.create(tasks, 0)
      let mut i = 0u32
      while i < 16u32 {
        mutex.with(tasks, own, fn(mut value: unique u32) { value += 1 })
        mutex.with(tasks, contended, fn(mut value: unique u32) { value += 1 })
        atomic.store(tasks, hits, i)
        i += 1u32
      }
      let mut tx = take first
      let _ = channel.send(tasks, mut tx, mutex.with(tasks, own, fn(mut value: unique u32) -> u32 { value }))
    })
    let b = task.async(s, fn() -> () {
      let contended = copy shared
      let own: Mutex[u32] = mutex.create(tasks, 0)
      let mut i = 0u32
      while i < 16u32 {
        mutex.with(tasks, own, fn(mut value: unique u32) { value += 1 })
        mutex.with(tasks, contended, fn(mut value: unique u32) { value += 1 })
        let _ = atomic.load(tasks, hits)
        i += 1u32
      }
      let mut tx = take second
      let _ = channel.send(tasks, mut tx, mutex.with(tasks, own, fn(mut value: unique u32) -> u32 { value }))
    })
    let mut r = take receiver
    let mut sum = 0u32
    let mut n = 0u32
    while n < 2u32 {
      match channel.receive(tasks, mut r) {
        Ok(v) => { sum += v },
        Error(_) => (),
      }
      n += 1u32
    }
    task.await(s, take a)
    task.await(s, take b)
    sum
  })
  failure.check(total == 32u32, Bounds())
  failure.check(mutex.with(tasks, shared, fn(mut value: unique u32) -> u32 { value }) == 32u32, Bounds())
  out.print(output, "objects\n") ? Output
  Ok(())
}
"#;

fn runtime50_objects_at(cells: Cells) {
    f1_corpus_matrix_cells(
        &format!("runtime50-objects-{}", cells.levels[0]),
        RUNTIME50_OBJECTS,
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "dirs":[]},"args":[]}),
        (0, |_, _, _, _| {}),
        b"",
        || Box::new(|| {}),
        cells,
    );
}

#[test]
fn runtime50_object_phase_cross_engine_replay() {
    runtime50_objects_at(Cells {
        levels: &["record"],
        engines: &ENGINES,
        dimensions: Cells::DIMENSIONS,
    });
}

#[test]
fn runtime50_object_phase_cross_engine_replay_durable() {
    runtime50_objects_at(Cells {
        levels: &["durable"],
        engines: &ENGINES,
        dimensions: Cells::DIMENSIONS,
    });
}

/// A receiver drains a child's messages in FIFO order and sees Closed only
/// after its last live sender closes (Tokio `tests/sync_mpsc.rs:213-228`,
/// `:483-521`); a canceled child's ready receives on its leased endpoint then
/// report Canceled before their queued message. Replayed receives that find
/// their outcome ready complete without the scheduler role.
const RECEIVES: &str = r#"pub type AppError = Grant(GrantError) | Output(OutputError)
fn discard[T](take value: T) -> () {
  ()
}
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  task.scope(tasks, fn(s: Scope) {
    let (sender, receiver): (Sender[u32], Receiver[u32]) = channel.open(tasks)
    let spare = sender.dup(tasks, sender)
    let child = task.async(s, fn() {
      let mut tx = take sender
      let mut i = 1u32
      while i <= 6u32 {
        let _ = channel.send(tasks, mut tx, i)
        i += 1u32
      }
    })
    let mut rx = take receiver
    failure.check(channel.receive(tasks, mut rx) == Ok(1u32), Bounds())
    discard(take spare)
    let mut expected = 2u32
    while expected <= 6u32 {
      failure.check(channel.receive(tasks, mut rx) == Ok(expected), Bounds())
      expected += 1u32
    }
    failure.check(channel.receive(tasks, mut rx) == Error(Closed()), Bounds())
    task.await(s, take child)
  })
  task.scope(tasks, fn(s: Scope) {
    let (sender, receiver): (Sender[u32], Receiver[u32]) = channel.open(tasks)
    let (ready, started): (Sender[u32], Receiver[u32]) = channel.open(tasks)
    let (wake, sleep): (Sender[u32], Receiver[u32]) = channel.open(tasks)
    let mut tx = take sender
    let _ = channel.send(tasks, mut tx, 7u32)
    let _ = channel.send(tasks, mut tx, 8u32)
    discard(take tx)
    let child = task.async(s, fn() -> bool {
      let mut rx = take receiver
      let first = channel.receive(tasks, mut rx) == Ok(7u32)
      let mut up = take ready
      let _ = channel.send(tasks, mut up, 1u32)
      let mut gate = take sleep
      let waiting = channel.receive(tasks, mut gate) == Error(Canceled(Requested()))
      let queued = channel.receive(tasks, mut rx) == Error(Canceled(Requested()))
      let again = channel.receive(tasks, mut rx) == Error(Canceled(Requested()))
      first && waiting && queued && again
    })
    let mut go = take started
    failure.check(channel.receive(tasks, mut go) == Ok(1u32), Bounds())
    task.cancel(s, child)
    failure.check(task.await(s, take child), Bounds())
    let _ = wake
  })
  out.print(output, "receives\n") ? Output
  Ok(())
}
"#;

/// The reference and checked executors replay the same tape in-process.
fn receives_reference_and_checked(
    live: &Output,
    _: &hirsel_free::tape::Tape,
    scratch: &Scratch,
    level: &str,
) {
    assert_eq!(live.stdout, b"receives\n");
    let (program, tape) = (scratch.path("main.hir"), scratch.path(level));
    hirsel_engine::with_compiler_stack(|| {
        let plain = hirsel_driver::replay_observations(&program, &tape, Some(1_000_000)).unwrap();
        let checked =
            hirsel_driver::replay_observations_checked(&program, &tape, Some(1_000_000)).unwrap();
        assert!(plain.equivalent(&checked), "{level}");
        assert_eq!(plain.output, b"receives\n", "{level}");
        assert_eq!(plain.exit_code, Some(0), "{level}");
        assert!(plain.failures.is_empty(), "{level}");
    });
}

fn replayed_receives_at(level: &'static [&'static str; 1]) {
    f1_corpus_matrix_cells(
        &format!("replayed-receives-{}", level[0]),
        RECEIVES,
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "dirs":[]},"args":[]}),
        (0, receives_reference_and_checked),
        b"",
        || Box::new(|| {}),
        Cells {
            levels: level,
            engines: &ENGINES,
            dimensions: &[("1", 1), ("4", 1)],
        },
    );
}

#[test]
fn replayed_receive_outcomes_cross_engine_replay() {
    replayed_receives_at(&["record"]);
}

#[test]
fn replayed_receive_outcomes_cross_engine_replay_durable() {
    replayed_receives_at(&["durable"]);
}

/// A child resolves its own mutex, then a failing body kills it and a later
/// acquisition reads its cause; the dead mutex, both dup'd senders and the
/// receiver close on resolved leases at their owners' ends. Role-free close,
/// kill and cause record the same histories every engine, width and permit
/// count replays.
const FREE51_CLOSE: &str = r#"
pub type AppError = Grant(GrantError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let total = task.scope(tasks, fn(s: Scope) -> u32 {
    let (sender, receiver): (Sender[u32], Receiver[u32]) = channel.open(tasks)
    let first = sender.dup(tasks, sender)
    let second = sender.dup(tasks, sender)
    let a = task.async(s, fn() -> () {
      let own: Mutex[u32] = mutex.create(tasks, 0)
      let mut i = 0u32
      while i < 4u32 {
        mutex.with(tasks, own, fn(mut value: unique u32) { value += 1 })
        i += 1u32
      }
      let killed: Result[(), Failure] = task.isolate(tasks, fn() -> () {
        mutex.with(tasks, own, fn(mut value: unique u32) -> () uses {} {
          let n: u8 = 255
          let broken = n + 1
        })
      })
      let observed: Result[u32, Failure] = task.isolate(tasks, fn() -> u32 {
        mutex.with(tasks, own, fn(mut value: unique u32) -> u32 uses {} { value })
      })
      let dead: u32 = match (killed, observed) {
        (Error(_), Error(_)) => 1u32,
        _ => 0u32,
      }
      let mut tx = take first
      let _ = channel.send(tasks, mut tx, dead)
      let _ = channel.send(tasks, mut tx, 10u32)
    })
    let b = task.async(s, fn() -> () {
      let mut tx = take second
      let _ = channel.send(tasks, mut tx, 100u32)
      let _ = channel.send(tasks, mut tx, 1000u32)
    })
    let mut r = take receiver
    let mut sum = 0u32
    let mut n = 0u32
    while n < 4u32 {
      match channel.receive(tasks, mut r) {
        Ok(v) => { sum += v },
        Error(_) => (),
      }
      n += 1u32
    }
    task.await(s, take a)
    task.await(s, take b)
    sum
  })
  failure.check(total == 1111u32, Bounds())
  out.print(output, "closed\n") ? Output
  Ok(())
}
"#;

fn free51_close_at(cells: Cells) {
    f1_corpus_matrix_cells(
        &format!("free51-close-{}", cells.levels[0]),
        FREE51_CLOSE,
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "dirs":[]},"args":[]}),
        (0, |_, _, _, _| {}),
        b"",
        || Box::new(|| {}),
        cells,
    );
}

#[test]
fn free51_close_kill_cause_cross_engine_replay() {
    free51_close_at(Cells {
        levels: &["record"],
        engines: &ENGINES,
        dimensions: Cells::DIMENSIONS,
    });
}

#[test]
fn free51_close_kill_cause_cross_engine_replay_durable() {
    free51_close_at(Cells {
        levels: &["durable"],
        engines: &ENGINES,
        dimensions: Cells::DIMENSIONS,
    });
}

/// A dead take and reading its cause are distinct native operations. Keep the
/// root's final observation aligned even when no channel exposes the offset.
#[test]
fn native_dead_mutex_cause_cross_engine_replay() {
    f1_corpus_matrix_cells(
        "dead-mutex-cause",
        r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let own: Mutex[u32] = mutex.create(tasks, 0)
  let killed: Result[(), Failure] = task.isolate(tasks, fn() -> () {
    mutex.with(tasks, own, fn(mut value: unique u32) -> () uses {} {
      let n: u8 = 255
      let broken = n + 1
    })
  })
  let observed: Result[u32, Failure] = task.isolate(tasks, fn() -> u32 {
    mutex.with(tasks, own, fn(mut value: unique u32) -> u32 uses {} { value })
  })
  Ok(())
}
"#,
        |_| serde_json::json!({"io":{"tasks":"tasks.free@1", "dirs":[]},"args":[]}),
        (0, |_, _, _, _| {}),
        b"",
        || Box::new(|| {}),
        Cells {
            levels: &["record", "durable"],
            engines: &ENGINES,
            dimensions: &[("1", 1)],
        },
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
            assert_eq!(
                canonical(&tape),
                *expected.as_ref().unwrap(),
                "F1 {engine:?}: canonical scalar tape bytes"
            );
        }
    }
}

#[test]
fn f1_atomic_u8_full_projection_and_permit_matrix() {
    f1_atomic_at("atomic-u8", "u8", &["record"]);
}

#[test]
fn f1_atomic_u8_full_projection_and_permit_matrix_durable() {
    f1_atomic_at("atomic-u8", "u8", &["durable"]);
}

#[test]
fn f1_atomic_u64_full_projection_and_permit_matrix() {
    f1_atomic_at("atomic-u64", "u64", &["record"]);
}

#[test]
fn f1_atomic_u64_full_projection_and_permit_matrix_durable() {
    f1_atomic_at("atomic-u64", "u64", &["durable"]);
}

#[test]
fn f1_atomic_u128_full_projection_and_permit_matrix() {
    f1_atomic_at("atomic-u128", "u128", &["record"]);
}

#[test]
fn f1_atomic_u128_full_projection_and_permit_matrix_durable() {
    f1_atomic_at("atomic-u128", "u128", &["durable"]);
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
fn f1_random_and_sleep_requests_replay_through_recorded_attempts_record() {
    f1_random_and_sleep_matrix(Cells::level(false));
}

#[test]
fn f1_random_and_sleep_requests_replay_through_recorded_attempts_durable() {
    f1_random_and_sleep_matrix(Cells::level(true));
}

fn f1_random_and_sleep_matrix(cells: Cells) {
    f1_corpus_matrix_with_exit_cells(
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
        0,
        cells,
    );
}

#[test]
fn f1_failed_root_terminal_outcome_replays_exactly_record() {
    f1_failed_root_terminal_outcome_matrix(Cells::level(false));
}

#[test]
fn f1_failed_root_terminal_outcome_replays_exactly_durable() {
    f1_failed_root_terminal_outcome_matrix(Cells::level(true));
}

fn f1_failed_root_terminal_outcome_matrix(cells: Cells) {
    f1_corpus_matrix_with_exit_cells(
        "failed-root",
        "pub fn main(rt: Io) -> Result[(), GrantError] { let tasks = io.tasks(rt)?; failure.check(false, Arithmetic()); Ok(()) }",
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "dirs":[]}),
        hirsel_run_schema::exit::FAILED,
        cells,
    );
}

#[test]
fn f1_blocked_terminal_outcome_replays_exactly_record() {
    f1_blocked_terminal_outcome_matrix(Cells::level(false));
}

#[test]
fn f1_blocked_terminal_outcome_replays_exactly_durable() {
    f1_blocked_terminal_outcome_matrix(Cells::level(true));
}

fn f1_blocked_terminal_outcome_matrix(cells: Cells) {
    // Full F1 phases are exercised by the qualification configuration. The
    // ordinary admission control checks the shipping legacy-tape path.
    f1_corpus_matrix_with_exit_cells(
        "blocked-root",
        "pub fn main(rt: Io) -> Result[(), GrantError] { let tasks = io.tasks(rt)?; let (tx, rx): (Sender[u32], Receiver[u32]) = channel.open(tasks); let mut sender = take tx; let mut receiver = take rx; let _ = channel.receive(tasks, mut receiver); let _ = channel.send(tasks, mut sender, 1u32); Ok(()) }",
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "dirs":[]}),
        hirsel_run_schema::exit::BLOCKED,
        cells,
    );
}

#[test]
fn f1_terminal_restoration_rejects_forged_root_and_unconsumed_records() {
    f1_terminal_restoration_matrix("record", &[("1", None)], &ENGINES);
}

fn f1_terminal_restoration_matrix(
    level: &str,
    dimensions: &[(&str, Option<usize>)],
    engines: &[&[&str]],
) {
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
    let grants =
        serde_json::json!({"io":{"tasks":"tasks.free@1","dirs":[]},"args":[],"recording":level});
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .args(["run", "--native"])
        .arg(&program)
        .args(["--cores", "8", "--grants", &grants.to_string(), "--record"])
        .arg(&path);
    f1_permits(&mut command, 8);
    let live = bounded_output(&mut command, &program, "root forgery producer");
    assert!(live.status.success(), "{live:?}");
    let original = std::fs::read(&path).unwrap();
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
        for &engine in engines {
            for &(width, permits) in dimensions {
                let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                command
                    .arg("run")
                    .args(engine)
                    .arg(&program)
                    .arg("--replay")
                    .arg(&path)
                    .args(["--cores", width]);
                if let Some(permits) = permits {
                    f1_permits(&mut command, permits);
                }
                let replay = bounded_output(
                    &mut command,
                    &program,
                    &format!("root forgery {mutation} {engine:?} width{width} permits{permits:?}"),
                );
                assert_eq!(
                    replay.status.code(),
                    Some(hirsel_run_schema::exit::REFUSED),
                    "{mutation} {engine:?}: {replay:?}"
                );
                let diagnostic = String::from_utf8_lossy(&replay.stderr);
                assert!(
                    if mutation == "root" {
                        // A contradictory terminal/root pair is independently
                        // invalid before semantic replay begins.
                        replay.stdout.is_empty()
                            && diagnostic.contains("Free terminal outcome differs from root cut")
                    } else {
                        diagnostic.contains("unconsumed") || diagnostic.contains("cut")
                    },
                    "{mutation} {engine:?}: {diagnostic}"
                );
            }
        }
    }
}

#[test]
fn f1_canceled_sleep_completion_is_consumed_before_terminal_restoration() {
    // Full F1 phases are exercised by the qualification configuration. The
    // ordinary admission control checks the shipping legacy-tape path.
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

/// The matrix in `cells`; each recording level is its own case.
fn f1_child_streaming_and_narrowing_replay_without_spawning_in(cells: Cells) {
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
    f1_corpus_matrix_with_exit_cells(
        "child-streaming",
        &source,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "process":{"provider":"process.spawn@1","rules":[{"program":child,"env":[]}]}, "dirs":[{"path":"data","host":host,"access":"read"}]}),
        0,
        cells,
    );
}

#[test]
fn f1_child_streaming_and_narrowing_replay_without_spawning_record() {
    f1_child_streaming_and_narrowing_replay_without_spawning_in(Cells::level(false));
}

#[test]
fn f1_child_streaming_and_narrowing_replay_without_spawning_durable() {
    f1_child_streaming_and_narrowing_replay_without_spawning_in(Cells::level(true));
}

/// The matrix in `cells`; each recording level is its own case.
fn f1_pipeline_and_group_replay_without_spawning_in(cells: Cells) {
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
    f1_corpus_matrix_with_exit_cells(
        "pipeline-group",
        &source,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "process":{"provider":"process.spawn@1","rules":[{"program":child,"env":[]}]}, "dirs":[{"path":"data","host":host,"access":"read"}]}),
        0,
        cells,
    );
}

#[test]
fn f1_pipeline_and_group_replay_without_spawning_record() {
    f1_pipeline_and_group_replay_without_spawning_in(Cells::level(false));
}

#[test]
fn f1_pipeline_and_group_replay_without_spawning_durable() {
    f1_pipeline_and_group_replay_without_spawning_in(Cells::level(true));
}

/// The matrix in `cells`; each recording level is its own case.
fn f1_owned_file_and_child_drops_replay_through_host_phases_in(cells: Cells) {
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
    f1_corpus_matrix_with_exit_cells(
        "owned-drops",
        &source,
        |_| serde_json::json!({"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "process":{"provider":"process.spawn@1","rules":[{"program":child,"env":[]}]}, "dirs":[{"path":"data","host":host,"access":"read_write"}]}),
        0,
        cells,
    );
}

#[test]
fn f1_owned_file_and_child_drops_replay_through_host_phases_record() {
    f1_owned_file_and_child_drops_replay_through_host_phases_in(Cells::level(false));
}

#[test]
fn f1_owned_file_and_child_drops_replay_through_host_phases_durable() {
    f1_owned_file_and_child_drops_replay_through_host_phases_in(Cells::level(true));
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

/// The matrix in `cells`; each recording level is its own case.
fn f1_signal_waits_replay_checked_arrivals_without_a_signal_source_in(cells: Cells) {
    f1_corpus_matrix_with_exit_cells(
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
        0,
        cells,
    );
}

#[test]
fn f1_signal_waits_replay_checked_arrivals_without_a_signal_source_record() {
    f1_signal_waits_replay_checked_arrivals_without_a_signal_source_in(Cells::level(false));
}

#[test]
fn f1_signal_waits_replay_checked_arrivals_without_a_signal_source_durable() {
    f1_signal_waits_replay_checked_arrivals_without_a_signal_source_in(Cells::level(true));
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

/// The matrix in `cells`; each recording level is its own case.
fn f1_network_aliases_replay_without_a_tcp_peer_in(cells: Cells) {
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
        cells,
    );
}

#[test]
fn f1_network_aliases_replay_without_a_tcp_peer_record() {
    f1_network_aliases_replay_without_a_tcp_peer_in(Cells::level(false));
}

#[test]
fn f1_network_aliases_replay_without_a_tcp_peer_durable() {
    f1_network_aliases_replay_without_a_tcp_peer_in(Cells::level(true));
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
        Cells::ALL,
    );
}

#[test]
fn f1_tcp_listener_and_connect_addr_replay_without_sockets() {
    // Full F1 phases are exercised by the qualification configuration. The
    // ordinary admission control checks the shipping legacy-tape path.
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
    f1_tls_client_matrix(
        "tls.connect_with(n, take stream, \"localhost\", client)",
        Cells::ALL,
    );
}

#[test]
fn f1_tls_basic_connect_replays_without_a_tls_peer_record() {
    f1_tls_client_matrix(
        "tls.connect(n, take stream, \"localhost\", Pem(bCA))",
        Cells::level(false),
    );
}

#[test]
fn f1_tls_basic_connect_replays_without_a_tls_peer_durable() {
    f1_tls_client_matrix(
        "tls.connect(n, take stream, \"localhost\", Pem(bCA))",
        Cells::level(true),
    );
}

#[test]
fn f1_tls_identity_connect_replays_without_a_tls_peer_record() {
    f1_tls_client_matrix(
        "tls.connect_as(n, take stream, \"localhost\", Pem(bCA), TlsIdentity(chain: bCHAIN, key: bKEY))",
        Cells::level(false),
    );
}

#[test]
fn f1_tls_identity_connect_replays_without_a_tls_peer_durable() {
    f1_tls_client_matrix(
        "tls.connect_as(n, take stream, \"localhost\", Pem(bCA), TlsIdentity(chain: bCHAIN, key: bKEY))",
        Cells::level(true),
    );
}

fn f1_tls_client_matrix(upgrade: &str, cells: Cells) {
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
        cells,
    );
}

fn f1_tls_server_matrix(cells: Cells) {
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
    let grants = |_: &Scratch| {
        serde_json::json!({"io":{"tasks":"tasks.free@1","dirs":[],
        "net":{"provider":"net.stream@1","rules":[
            {"address":"127.0.0.1","service":port.to_string(),"operation":"listen_tls"},
            {"address":"127.0.0.1","service":port.to_string(),"operation":"connect"}]}},"args":[]})
    };
    f1_corpus_matrix_cells(
        "tls-server",
        &source,
        grants,
        (0, |_, _, _, _| {}),
        b"",
        || Box::new(|| {}),
        cells,
    );
}

#[test]
fn f1_tls_server_mutual_auth_replays_without_handshakes_record_interpreter() {
    f1_tls_server_matrix(Cells::part(false, false));
}

#[test]
fn f1_tls_server_mutual_auth_replays_without_handshakes_record_native() {
    f1_tls_server_matrix(Cells::part(false, true));
}

#[test]
fn f1_tls_server_mutual_auth_replays_without_handshakes_durable_interpreter() {
    f1_tls_server_matrix(Cells::part(true, false));
}

#[test]
fn f1_tls_server_mutual_auth_replays_without_handshakes_durable_native() {
    f1_tls_server_matrix(Cells::part(true, true));
}

fn f1_unix_stream_matrix(cells: Cells) {
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
    let grants = |_: &Scratch| {
        serde_json::json!({"io":{"tasks":"tasks.free@1","dirs":[],
        "net":{"provider":"net.stream@1","rules":[
            {"address":path,"service":"0","operation":"unix_listen"},
            {"address":path,"service":"0","operation":"unix_connect"}]}},"args":[]})
    };
    f1_corpus_matrix_cells(
        "unix-stream",
        &source,
        grants,
        (0, |_, _, _, _| {}),
        b"",
        || Box::new(|| {}),
        cells,
    );
    assert!(!path.exists(), "owned listener must unlink its socket");
}

#[test]
fn f1_unix_streams_replay_without_socket_paths_record_interpreter() {
    f1_unix_stream_matrix(Cells::part(false, false));
}

#[test]
fn f1_unix_streams_replay_without_socket_paths_record_native() {
    f1_unix_stream_matrix(Cells::part(false, true));
}

#[test]
fn f1_unix_streams_replay_without_socket_paths_durable_interpreter() {
    f1_unix_stream_matrix(Cells::part(true, false));
}

#[test]
fn f1_unix_streams_replay_without_socket_paths_durable_native() {
    f1_unix_stream_matrix(Cells::part(true, true));
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

#[test]
fn f1_mapped_callbacks_and_atomic_files_replay_without_the_grant_tree() {
    f1_corpus_matrix_with_io(
        "mapped-callbacks",
        r#"pub type E = Grant(GrantError) | Files(FileError)
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let tasks = io.tasks(rt) ? Grant
  let d = io.dir(rt, path.of("data")) ? Files
  file.write_bytes(d, path.of("mapped"), b"abc") ? Files
  let mapping = file.map(d, path.of("mapped")) ? Files
  let alias = take mapping
  let copied = mapped.with(d, alias, fn(view: Bytes) -> Result[Bytes, FileError] {
    failure.check(view == b"abc", Arithmetic())
    let mut atomic = file.create_atomic(d, path.of("saved"))?
    file.write_all(d, mut atomic, view)?
    file.commit(d, take atomic)?
    Ok(bytes.from_list([view[0u64], view[1u64], view[2u64]]))
  }) ? Files
  failure.check(copied == Ok(b"abc"), Arithmetic())
  let saved = file.read_bytes(d, path.of("saved")) ? Files
  failure.check(saved == b"abc", Arithmetic())
  let checked = mapped.with(d, alias, fn(view: Bytes) -> Result[u64, FileError] {
    let count = bytes.len(view)
    let mut shortened = file.create(d, path.of("mapped"))?
    file.append(d, mut shortened, b"x")?
    Ok(count)
  })
  failure.check(match checked { Error(Truncated()) => true, _ => false }, Arithmetic())
  let mut abandoned = file.create_atomic(d, path.of("abandoned")) ? Files
  file.write_all(d, mut abandoned, b"pending") ? Files
  Ok(())
}
"#,
        |scratch| {
            let host = scratch.path("data");
            std::fs::create_dir(&host).unwrap();
            serde_json::json!({"tasks":"tasks.free@1","dirs":[{"path":"data","host":host,"access":"read_write"}]})
        },
    );
}

#[test]
fn f1_watch_events_and_owned_close_replay_without_a_watcher() {
    f1_corpus_matrix_with_io(
        "watch-events",
        r#"pub type E = Grant(GrantError) | Files(FileError)
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let tasks = io.tasks(rt) ? Grant
  let d = io.dir(rt, path.of("data")) ? Files
  let mut watch = file.watch(d, path.of(""), false) ? Files
  let mut opened = file.create(d, path.of("created")) ? Files
  file.append(d, mut opened, b"x") ? Files
  file.rename(d, path.of("created"), path.of("renamed")) ? Files
  file.remove(d, path.of("renamed")) ? Files
  failure.check(match watcher.next(d, mut watch) ? Files { Created(p) => path.text(p) == "created", _ => false }, Arithmetic())
  failure.check(match watcher.next(d, mut watch) ? Files { Modified(p) => path.text(p) == "created", _ => false }, Arithmetic())
  failure.check(match watcher.next(d, mut watch) ? Files { Renamed(a,b) => path.text(a) == "created" && path.text(b) == "renamed", _ => false }, Arithmetic())
  failure.check(match watcher.next(d, mut watch) ? Files { Removed(p) => path.text(p) == "renamed", _ => false }, Arithmetic())
  task.scope(tasks, fn(s: Scope) -> Result[(), E] {
    let mut owned = take watch
    let creating = task.async(s, fn() -> bool {
      match file.create(d, path.of("wake")) { Ok(_) => true, Error(_) => false }
    })
    failure.check(match watcher.next(d, mut owned) ? Files { Created(p) => path.text(p) == "wake", _ => false }, Arithmetic())
    failure.check(task.await(s, take creating), Arithmetic())
    file.remove(d, path.of("wake")) ? Files
    Ok(())
  })?
  Ok(())
}
"#,
        |scratch| {
            let host = scratch.path("data");
            std::fs::create_dir(&host).unwrap();
            serde_json::json!({"tasks":"tasks.free@1","dirs":[{"path":"data","host":host,"access":"read_write"}]})
        },
    );
}

/// The matrix in `cells`; each recording level is its own case.
fn f1_canceled_watcher_completions_replay_without_notifications_in(cells: Cells) {
    f1_corpus_matrix_with_exit_cells(
        "canceled-watcher",
        r#"pub type E = Grant(GrantError) | Files(FileError)
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let tasks = io.tasks(rt) ? Grant
  let d = io.dir(rt, path.of("data")) ? Files
  task.scope(tasks, fn(s: Scope) {
    let (ready, observed): (Sender[bool], Receiver[bool]) = channel.open(tasks)
    let (release, released): (Sender[bool], Receiver[bool]) = channel.open(tasks)
    let mut rx = take observed
    let mut tx = take release
    let child = task.async(s, fn() -> bool {
      let mut ready_tx = take ready
      let mut release_rx = take released
      let mut watch = match file.watch(d, path.of(""), false) { Ok(value) => take value, Error(_) => return false }
      let _ = channel.send(tasks, mut ready_tx, true)
      let _ = channel.receive(tasks, mut release_rx)
      let first = watcher.next(d, mut watch) == Error(Canceled(Requested()))
      let second = watcher.next(d, mut watch) == Error(Canceled(Requested()))
      let invalid = match file.watch(d, path.of("../outside"), false) { Error(FileFault(_)) => true, _ => false }
      let canceled = match file.watch(d, path.of(""), false) { Error(Canceled(Requested())) => true, _ => false }
      first && second && invalid && canceled
    })
    let _ = channel.receive(tasks, mut rx)
    task.cancel(s, child)
    let _ = channel.send(tasks, mut tx, true)
    failure.check(task.await(s, take child), Arithmetic())
  })
  Ok(())
}
"#,
        |scratch| {
            let host = scratch.path("data");
            std::fs::create_dir(&host).unwrap();
            serde_json::json!({"tasks":"tasks.free@1","dirs":[{"path":"data","host":host,"access":"read_write"}]})
        },
        0,
        cells,
    );
}

#[test]
fn f1_canceled_watcher_completions_replay_without_notifications_record() {
    f1_canceled_watcher_completions_replay_without_notifications_in(Cells::level(false));
}

#[test]
fn f1_canceled_watcher_completions_replay_without_notifications_durable() {
    f1_canceled_watcher_completions_replay_without_notifications_in(Cells::level(true));
}

#[test]
fn f1_telemetry_spans_replay_without_an_identity_or_export_provider() {
    f1_corpus_matrix_with_io(
        "telemetry-spans",
        r#"pub type E = Grant(GrantError) | Capture(TelemetryError) | Output(OutputError)
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let tasks = io.tasks(rt) ? Grant
  let t = io.telemetry(rt) ? Grant
  let out = io.stdout(rt) ? Grant
  let mut zero: List[u8] = []
  for i in 0u64..25u64 { list.push(mut zero, 0u8) }
  let root = capture.begin(t, bytes.from_list(take zero), b"root") ? Capture
  let context = capture.context(t, root)
  failure.check(bytes.len(context) == 25u64, Arithmetic())
  failure.check(match capture.begin(t, b"", b"invalid") { Error(InvalidParent()) => true, _ => false }, Arithmetic())
  capture.event(t, root, b"root event") ? Capture
  task.scope(tasks, fn(s: Scope) -> Result[(), E] {
    let parent = unique context
    let child = task.async(s, fn() -> Result[(), E] {
      let owned_parent = take parent
      let span = capture.begin(t, owned_parent, b"child") ? Capture
      let mine = capture.context(t, span)
      failure.check(bytes.len(mine) == 25u64 && mine != owned_parent, Arithmetic())
      capture.event(t, span, b"child event") ? Capture
      capture.finish(t, take span, b"success") ? Capture
      Ok(())
    })
    capture.log(t, b"interleaved log") ? Capture
    capture.measure(t, b"measurement") ? Capture
    task.await(s, take child)
  })?
  let abandoned = capture.begin(t, context, b"abandoned") ? Capture
  failure.check(bytes.len(capture.context(t, abandoned)) == 25u64, Arithmetic())
  capture.finish(t, take root, b"done") ? Capture
  out.print(out, f"{std@1/hex.encode(context)}\n") ? Output
  Ok(())
}
"#,
        |_| serde_json::json!({"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[],"telemetry":{"provider":"telemetry.capture@1"}}),
    );
}

#[test]
fn f1_telemetry_payload_refusals_and_canceled_calls_replay_exactly() {
    let source = r#"pub type E = Grant(GrantError) | Capture(TelemetryError)
fn parent() -> Bytes uses {} {
  let zero: Inline[u8, 25] = [0u8; 25]
  bytes.from_inline(zero)
}
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let tasks = io.tasks(rt) ? Grant
  let t = io.telemetry(rt) ? Grant
  let initial = parent()
  let span = capture.begin(t, initial, b"root") ? Capture
  let large = b"OVERSIZED"
  failure.check(match capture.event(t, span, large) { Error(TooLarge()) => true, _ => false }, Arithmetic())
  failure.check(match capture.log(t, large) { Error(TooLarge()) => true, _ => false }, Arithmetic())
  failure.check(match capture.measure(t, large) { Error(TooLarge()) => true, _ => false }, Arithmetic())
  failure.check(match capture.begin(t, b"", large) { Error(TooLarge()) => true, _ => false }, Arithmetic())
  failure.check(match capture.finish(t, take span, large) { Error(TooLarge()) => true, _ => false }, Arithmetic())
  let (sender, receiver): (Sender[bool], Receiver[bool]) = channel.open(tasks)
  task.scope(tasks, fn(s: Scope) {
    let mut tx = take sender
    let child = task.async(s, fn() -> bool {
      let mut rx = take receiver
      let _ = channel.receive(tasks, mut rx)
      let mine = match capture.begin(t, parent(), b"after cancellation") { Ok(value) => take value, Error(_) => return false }
      let logged = capture.log(t, b"still observed") == Ok(())
      let ended = capture.finish(t, take mine, b"done") == Ok(())
      logged && ended
    })
    task.cancel(s, child)
    let _ = channel.send(tasks, mut tx, true)
    failure.check(task.await(s, take child), Arithmetic())
  })
  Ok(())
}
"#.replace("OVERSIZED", &"x".repeat(65_537));
    f1_corpus_matrix_with_io(
        "telemetry-refusals",
        &source,
        |_| serde_json::json!({"tasks":"tasks.free@1","dirs":[],"telemetry":{"provider":"telemetry.null@1","seed":0}}),
    );
}

// Every part keeps the native recording and every width/permit replay cell.
// Separate recording levels and engine groups keep each action within RE600.
fn f1_deadline_matrix(future: bool, cells: Cells) {
    let (name, text) = if future {
        (
            "scope-deadline-future",
            include_str!("free/deadline-future.hir"),
        )
    } else {
        ("scope-deadline", include_str!("free/deadline.hir"))
    };
    f1_corpus_matrix_cells(
        name,
        text,
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[]},"args":[]}),
        (0, |_, _, _, _| {}),
        b"",
        || Box::new(|| {}),
        cells,
    );
}

#[test]
fn f1_scope_deadline_phases_replay_with_exact_expiration_causes_record_interpreter() {
    f1_deadline_matrix(false, Cells::part(false, false));
}

#[test]
fn f1_scope_deadline_phases_record_llvm() {
    f1_deadline_matrix(
        false,
        Cells {
            levels: &["record"],
            engines: &[&["--native"]],
            dimensions: Cells::DIMENSIONS,
        },
    );
}

#[test]
fn f1_scope_deadline_phases_record_cranelift_width1_permits1() {
    f1_deadline_matrix(
        false,
        Cells {
            levels: &["record"],
            engines: &[&["--native", "--backend", "cranelift"]],
            dimensions: &[("1", 1)],
        },
    );
}

#[test]
fn f1_scope_deadline_phases_record_cranelift_width1_permits8() {
    f1_deadline_matrix(
        false,
        Cells {
            levels: &["record"],
            engines: &[&["--native", "--backend", "cranelift"]],
            dimensions: &[("1", 8)],
        },
    );
}

#[test]
fn f1_scope_deadline_phases_record_cranelift_width8_permits1() {
    f1_deadline_matrix(
        false,
        Cells {
            levels: &["record"],
            engines: &[&["--native", "--backend", "cranelift"]],
            dimensions: &[("8", 1)],
        },
    );
}

#[test]
fn f1_scope_deadline_phases_record_cranelift_width8_permits8() {
    f1_deadline_matrix(
        false,
        Cells {
            levels: &["record"],
            engines: &[&["--native", "--backend", "cranelift"]],
            dimensions: &[("8", 8)],
        },
    );
}

#[test]
fn f1_scope_deadline_phases_replay_with_exact_expiration_causes_durable_interpreter() {
    f1_deadline_matrix(false, Cells::part(true, false));
}

#[test]
fn f1_scope_deadline_phases_replay_with_exact_expiration_causes_durable_native() {
    f1_deadline_matrix(false, Cells::part(true, true));
}

#[test]
fn f1_future_scope_timers_replay_cancellation_nested_expiry_and_disarm_record_interpreter() {
    f1_deadline_matrix(true, Cells::part(false, false));
}

#[test]
fn f1_future_scope_timers_replay_cancellation_nested_expiry_and_disarm_record_native() {
    f1_deadline_matrix(true, Cells::part(false, true));
}

#[test]
fn f1_future_scope_timers_replay_cancellation_nested_expiry_and_disarm_durable_interpreter() {
    f1_deadline_matrix(true, Cells::part(true, false));
}

#[test]
fn f1_future_scope_timers_replay_cancellation_nested_expiry_and_disarm_durable_native() {
    f1_deadline_matrix(true, Cells::part(true, true));
}

const F1_RUNNER_COMPUTE_SOURCE: &str = r#"pub type E = Grant(GrantError) | Output(OutputError)
fn half(n: u64) -> Result[u64, String] {
  failure.check(n < 100u64, Arithmetic())
  if n % 2u64 == 1u64 { return Error("odd") }
  Ok(n / 2u64)
}
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let out = io.stdout(rt) ? Grant
  let runner = io.runner(rt, "worker") ? Grant
  failure.check(runner.compute(runner, half, 8u64) == Ok(4u64), Arithmetic())
  failure.check(match runner.compute(runner, half, 3u64) { Error(Failed(e)) => e == "odd", _ => false }, Arithmetic())
  failure.check(match runner.compute(runner, half, 200u64) { Error(Faulted(_)) => true, _ => false }, Arithmetic())
  out.print(out, "runner outcomes\n") ? Output
  Ok(())
}
"#;
const F1_RUNNER_COMPUTE_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[],"runners":[{"name":"worker","provider":"local.threads@1","threads":2,"grants":null,"definitions":null}]},"args":[]}"#;

#[test]
fn f1_runner_compute_single_permit_cross_engine_outcomes() {
    let scratch = Scratch::new("f1-runner-compute-single-permit");
    let program = scratch.path("main.hir");
    std::fs::write(&program, F1_RUNNER_COMPUTE_SOURCE).unwrap();
    let grants = F1_RUNNER_COMPUTE_GRANTS;
    let tape = scratch.path("record");
    let report = scratch.path("projection");
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .args(["run", "--native"])
        .arg(&program)
        .args(["--grants", grants, "--cores", "1", "--record"])
        .arg(&tape)
        .env(hirsel_rt::free_replay::Report::VAR, &report);
    f1_permits(&mut command, 1);
    let live = bounded_output(&mut command, &program, "runner compute live width1 permit1");
    assert!(live.status.success(), "{live:?}");
    assert_eq!(live.stdout, b"runner outcomes\n");
    assert!(
        std::fs::read(&tape)
            .unwrap()
            .starts_with(b"hirsel-free-tape 2\n")
    );
    let projection = std::fs::read_to_string(&report).unwrap();
    for engine in ENGINES {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("run")
            .args(engine)
            .arg(&program)
            .arg("--replay")
            .arg(&tape)
            .args(["--cores", "1"])
            .env(hirsel_rt::free_replay::Report::VAR, &report);
        f1_permits(&mut command, 1);
        let replay = bounded_output(
            &mut command,
            &program,
            &format!("runner compute replay {engine:?} width1 permit1"),
        );
        assert!(replay.status.success(), "{engine:?}: {replay:?}");
        assert_eq!(replay.stdout, live.stdout);
        assert_eq!(std::fs::read_to_string(&report).unwrap(), projection);
    }
}

const F1_RUNNER_HOST_WAIT_SOURCE: &str = r#"pub type E = Grant(GrantError) | Output(OutputError)
fn child(rt: Io, n: u64) -> Result[u64, String] {
  let out = match io.stdout(rt) { Ok(v) => v, Error(_) => return Error("stdout") }
  let random = match io.random(rt) { Ok(v) => v, Error(_) => return Error("random") }
  let draw = match random.bytes(random, 8) { Ok(v) => v, Error(_) => return Error("draw") }
  let clock = match io.monotonic_clock(rt) { Ok(v) => v, Error(_) => return Error("clock") }
  let now = match clock.monotonic(clock) { Ok(v) => v, Error(_) => return Error("now") }
  let later = match instant.checked_add(clock, now, 10000000u64) { Some(v) => v, None() => now }
  match clock.sleep_until(clock, later) { Ok(()) => (), Error(_) => return Error("sleep") }
  match out.print(out, "child effect\n") { Ok(()) => (), Error(_) => return Error("print") }
  if bytes.len(draw) == 8 { Ok(n + 1u64) } else { Error("length") }
}
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let out = io.stdout(rt) ? Grant
  let runner = io.runner(rt, "worker") ? Grant
  failure.check(runner.call(runner, child, 8u64) == Ok(9u64), Arithmetic())
  out.print(out, "parent result\n") ? Output
  Ok(())
}
"#;
const F1_RUNNER_HOST_WAIT_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[],"runners":[{"name":"worker","provider":"local.threads@1","threads":2,"grants":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","random":"random.splitmix64@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[]},"definitions":null}]},"args":[]}"#;

#[test]
fn f1_runner_call_child_host_waits_replay_without_child_effects() {
    let scratch = Scratch::new("f1-runner-call-host-waits");
    let program = scratch.path("main.hir");
    std::fs::write(&program, F1_RUNNER_HOST_WAIT_SOURCE).unwrap();
    let grants = F1_RUNNER_HOST_WAIT_GRANTS;
    f1_runner_call_record_replay(
        &scratch,
        &program,
        grants,
        b"child effect\nparent result\n",
        b"parent result\n",
    );
}

fn f1_runner_call_record_replay(
    scratch: &Scratch,
    program: &Path,
    grants: &str,
    live_output: &[u8],
    replay_output: &[u8],
) {
    let tape = scratch.path("record");
    let report = scratch.path("projection");
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .args(["run", "--native"])
        .arg(&program)
        .args(["--grants", grants, "--cores", "1", "--record"])
        .arg(&tape)
        .env(hirsel_rt::free_replay::Report::VAR, &report);
    f1_permits(&mut command, 1);
    let live = bounded_output(&mut command, &program, "runner call child host waits live");
    assert!(live.status.success(), "{live:?}");
    assert_eq!(live.stdout, live_output);
    let projection = std::fs::read_to_string(&report).unwrap();
    for engine in ENGINES {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("run")
            .args(engine)
            .arg(&program)
            .arg("--replay")
            .arg(&tape)
            .args(["--cores", "1"])
            .env(hirsel_rt::free_replay::Report::VAR, &report);
        f1_permits(&mut command, 1);
        let replay = bounded_output(
            &mut command,
            &program,
            &format!("runner call replay {engine:?}"),
        );
        assert!(replay.status.success(), "{engine:?}: {replay:?}");
        // A caller's replay consumes the outcome and never repeats child effects.
        assert_eq!(replay.stdout, replay_output);
        assert_eq!(std::fs::read_to_string(&report).unwrap(), projection);
    }
}

const F1_RUNNER_CHILD_DEADLINE_SOURCE: &str = r#"pub type E = Grant(GrantError) | Output(OutputError)
fn child(rt: Io, n: u64) -> Result[u64, String] {
  let tasks = match io.tasks(rt) { Ok(v) => v, Error(_) => return Error("tasks") }
  let clock = match io.monotonic_clock(rt) { Ok(v) => v, Error(_) => return Error("clock") }
  let now = match clock.monotonic(clock) { Ok(v) => v, Error(_) => return Error("now") }
  let later = match instant.checked_add(clock, now, 10000000u64) { Some(v) => v, None() => now }
  let (sender, receiver): (Sender[u64], Receiver[u64]) = channel.open(tasks)
  let ended = task.scope_until(tasks, Deadline(clock: clock, at: later), fn(s: Scope) -> bool {
    let mut rx = take receiver
    let canceled = match channel.receive(tasks, mut rx) {
      Error(Canceled(DeadlineExceeded())) => true,
      _ => false,
    }
    canceled && match task.check_canceled(s) { Error(Canceled(DeadlineExceeded())) => true, _ => false }
  })
  if match ended { Expired(true) => true, _ => false } { Ok(n + 1u64) } else { Error("deadline") }
}
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let out = io.stdout(rt) ? Grant
  let runner = io.runner(rt, "worker") ? Grant
  failure.check(runner.call(runner, child, 8u64) == Ok(9u64), Arithmetic())
  out.print(out, "child deadline result\n") ? Output
  Ok(())
}
"#;
const F1_RUNNER_CHILD_DEADLINE_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[],"runners":[{"name":"worker","provider":"local.threads@1","threads":2,"grants":{"tasks":"tasks.free@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[]},"definitions":null}]},"args":[]}"#;

const F1_RUNNER_PARENT_DEADLINE_SOURCE: &str = r#"pub type E = Grant(GrantError) | Output(OutputError) | Clock(ClockError)
fn child(rt: Io, n: u64) -> Result[u64, String] {
  let clock = match io.monotonic_clock(rt) { Ok(v) => v, Error(_) => return Error("clock") }
  let now = match clock.monotonic(clock) { Ok(v) => v, Error(_) => return Error("now") }
  let later = match instant.checked_add(clock, now, 3600000000000u64) { Some(v) => v, None() => now }
  match clock.sleep_until(clock, later) {
    Error(Canceled(Parent())) => Ok(n + 1u64),
    _ => Error("parent cancellation not forwarded"),
  }
}
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let out = io.stdout(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let clock = io.monotonic_clock(rt) ? Grant
  let runner = io.runner(rt, "worker") ? Grant
  let now = clock.monotonic(clock) ? Clock
  let later = match instant.checked_add(clock, now, 500000000u64) { Some(v) => v, None() => now }
  let ended = task.scope_until(tasks, Deadline(clock: clock, at: later), fn(s: Scope) -> bool {
    runner.call(runner, child, 8u64) == Ok(9u64)
  })
  failure.check(match ended { Expired(true) => true, _ => false }, Arithmetic())
  out.print(out, "forwarded parent deadline\n") ? Output
  Ok(())
}
"#;
const F1_RUNNER_PARENT_DEADLINE_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[],"runners":[{"name":"worker","provider":"local.threads@1","threads":2,"grants":{"monotonic_clock":"clock.monotonic.ns@1","dirs":[]},"definitions":null}]},"args":[]}"#;

const F1_RUNNER_SPAWN_PENDING_SOURCE: &str = r#"pub type E = Grant(GrantError) | Output(OutputError)
fn child(rt: Io, n: u64) -> Result[u64, String] {
  let clock = match io.monotonic_clock(rt) { Ok(v) => v, Error(_) => return Error("clock") }
  let now = match clock.monotonic(clock) { Ok(v) => v, Error(_) => return Error("now") }
  let ns = if n == 8u64 { 3600000000000u64 } else { 10000000u64 }
  let later = match instant.checked_add(clock, now, ns) { Some(v) => v, None() => now }
  match clock.sleep_until(clock, later) {
    Error(Canceled(Parent())) => if n == 8u64 { Ok(9u64) } else { Error("unexpected cancel") },
    Ok(()) => if n == 20u64 { Ok(21u64) } else { Error("missing cancel") },
    _ => Error("sleep"),
  }
}
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let out = io.stdout(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let runner = io.runner(rt, "worker") ? Grant
  let good = task.scope(tasks, fn(s: Scope) -> bool {
    let first = runner.spawn(s, runner, child, 8u64)
    let second = runner.spawn(s, runner, child, 20u64)
    let _ = out.print(out, "spawn returned while pending\n")
    task.cancel(s, first)
    let right = task.await(s, take second) == Ok(21u64)
    right && task.await(s, take first) == Ok(9u64)
  })
  failure.check(good, Arithmetic())
  out.print(out, "spawn outcomes\n") ? Output
  Ok(())
}
"#;
const F1_RUNNER_SPAWN_PENDING_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[],"runners":[{"name":"worker","provider":"local.threads@1","threads":2,"grants":{"monotonic_clock":"clock.monotonic.ns@1","dirs":[]},"definitions":null}]},"args":[]}"#;

const F1_RUNNER_SPAWN_REFUSAL_SOURCE: &str = r#"pub type E = Grant(GrantError) | Output(OutputError)
fn child(rt: Io, n: u64) -> Result[String, String] { Ok("unused result") }
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let out = io.stdout(rt) ? Grant
  let tasks = io.tasks(rt) ? Grant
  let runner = io.runner(rt, "worker") ? Grant
  let denied = runner.only(runner, []) ? Grant
  let refused = task.scope(tasks, fn(s: Scope) -> bool {
    let waiter = runner.spawn(s, denied, child, 4u64)
    task.await(s, take waiter) == Error(NotRun(Denied()))
  })
  failure.check(refused, Arithmetic())
  task.scope(tasks, fn(s: Scope) {
    let unused = runner.spawn(s, runner, child, 4u64)
    task.detach(s, take unused)
    ()
  })
  out.print(out, "refusal and cleanup\n") ? Output
  Ok(())
}
"#;
const F1_RUNNER_SPAWN_REFUSAL_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[],"runners":[{"name":"worker","provider":"local.threads@1","threads":1,"grants":{"dirs":[]},"definitions":null}]},"args":[]}"#;

const F1_RUNNER_LINKED_SOURCE: &str = r#"pub type E = Grant(GrantError) | Output(OutputError)
fn child(rt: Io, n: u64) -> Result[u64, String] {
  let out = match io.stdout(rt) { Ok(v) => v, Error(_) => return Error("stdout") }
  match out.print(out, "linked child\n") { Ok(()) => Ok(n + 1u64), Error(_) => Error("print") }
}
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let out = io.stdout(rt) ? Grant
  let runner = io.runner(rt, "worker") ? Grant
  failure.check(runner.call(runner, child, 8u64) == Ok(9u64), Arithmetic())
  out.print(out, "linked parent\n") ? Output
  Ok(())
}
"#;
const F1_RUNNER_LINKED_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[],"runners":[{"name":"worker","provider":"local.threads@1","threads":1,"grants":{"stdout":"stdout.bytes@1","dirs":[]},"definitions":null}]},"args":[]}"#;

const F1_RUNNER_CRASH_SOURCE: &str = r#"pub type E = Grant(GrantError) | Output(OutputError)
fn child(rt: Io, n: u64) -> Result[u64, String] {
  let random = match io.random(rt) { Ok(v) => v, Error(_) => return Error("random") }
  let mut i = 0u64
  while i < 32u64 {
    match random.bytes(random, 1u64) { Ok(_) => (), Error(_) => return Error("draw") }
    i = i + 1u64
  }
  Ok(n)
}
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let out = io.stdout(rt) ? Grant
  let runner = io.runner(rt, "worker") ? Grant
  failure.check(runner.call(runner, child, 8u64) == Error(InDoubt(Abandoned())), Arithmetic())
  failure.check(runner.call(runner, child, 9u64) == Error(InDoubt(Abandoned())), Arithmetic())
  out.print(out, "abandoned twice\n") ? Output
  Ok(())
}
"#;
const F1_RUNNER_CRASH_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[],"runners":[{"name":"worker","provider":"sim@1","node":"node","models":"local.process@1","faults":{"crash":{"enable":1000000,"fire":1000000}},"grants":{"random":"random.splitmix64@1","dirs":[]},"definitions":null}]},"args":[]}"#;

fn f1_runner_short_fixture(fault: &str, outcome: &str) -> (String, String) {
    let text = format!(
        r#"pub type E = Grant(GrantError) | Output(OutputError)
fn child(rt: Io, n: u64) -> Result[u64, String] {{ Ok(n) }}
pub fn main(rt: Io, args: Args) -> Result[(), E] {{
  let out = io.stdout(rt) ? Grant
  let runner = io.runner(rt, "worker") ? Grant
  failure.check(runner.call(runner, child, 8u64) == Error(InDoubt({outcome}())), Arithmetic())
  out.print(out, "outcome lost\n") ? Output
  Ok(())
}}
"#
    );
    let grants = format!(
        r#"{{"io":{{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[],"runners":[{{"name":"worker","provider":"sim@1","node":"node","models":"local.process@1","faults":{{"{fault}":{{"enable":1000000,"fire":1000000}}}},"grants":{{"dirs":[]}},"definitions":null}}]}},"args":[]}}"#
    );
    (text, grants)
}

#[test]
fn f1_external_stop_during_host_wait_replays_exact_terminal_and_projection_record() {
    f1_external_stop_matrix("1", 1, &[("1", 1)], Cells::level(false));
}

#[test]
fn f1_external_stop_during_host_wait_replays_exact_terminal_and_projection_durable() {
    f1_external_stop_matrix("1", 1, &[("1", 1)], Cells::level(true));
}

fn f1_external_stop_matrix(
    record_width: &str,
    record_permits: usize,
    replay_dimensions: &[(&str, usize)],
    cells: Cells,
) {
    let scratch = Scratch::new("f1-external-stop-host-wait");
    let program = scratch.path("main.hir");
    std::fs::write(&program, r#"pub type E = Grant(GrantError) | Output(OutputError) | Clock(ClockError) | Wait(WaitError)
pub fn main(rt: Io, args: Args) -> Result[(), E] {
  let out = io.stdout(rt) ? Grant
  let clock = io.monotonic_clock(rt) ? Grant
  let now = clock.monotonic(clock) ? Clock
  out.print(out, "before stop\n") ? Output
  let later = match instant.checked_add(clock, now, 30000000000u64) { Some(v) => v, None() => now }
  clock.sleep_until(clock, later) ? Wait
  out.print(out, "unadmitted suffix\n") ? Output
  Ok(())
}
"#).unwrap();
    // Follow cli/budgets.rs's standalone RunControl stop fixture. The source
    // launcher intentionally forwards only selected environment channels.
    let executable = scratch.path("program");
    let mut build = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    build.arg("build").arg(&program).arg("-o").arg(&executable);
    let built = bounded_output(&mut build, &program, "external stop standalone build");
    assert!(built.status.success(), "{built:?}");
    for &level in cells.levels {
        let tape = scratch.path(level);
        let report = scratch.path("projection");
        let grants = format!(
            r#"{{"io":{{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[]}},"args":[],"placement":{{"cores":{record_width}}},"recording":"{level}"}}"#
        );
        let mut command = Command::new(&executable);
        command
            .arg("--hirsel-record")
            .arg(&tape)
            .args(["--hirsel-grants", &grants])
            .env("HIRSEL_RT_STOP_AFTER_MS", "200")
            .env(hirsel_rt::free_replay::Report::VAR, &report);
        f1_permits(&mut command, record_permits);
        let live = bounded_output(
            &mut command,
            &program,
            &format!("external stop {level} live"),
        );
        assert_eq!(
            live.status.code(),
            Some(hirsel_run_schema::exit::ENVIRONMENTAL),
            "{live:?}"
        );
        assert_eq!(live.stdout, b"before stop\n", "{live:?}");
        assert_eq!(
            live.stderr, b"hirsel-rt: environmental failure: its place stopped the run\n",
            "{live:?}"
        );
        let bytes = std::fs::read(&tape).unwrap();
        let parsed = hirsel_free::tape::parse(&bytes).unwrap();
        assert!(
            parsed
                .end
                .as_ref()
                .unwrap()
                .outcome
                .starts_with("native-stop-v2/")
        );
        let root = parsed.cut[&Vec::new()];
        assert!(
            matches!(root.phase, hirsel_free::phase::Phase::Ended(_)),
            "{root:?}"
        );
        let records = hirsel_free::record::decode(&parsed.logs[&Vec::new()]).unwrap();
        assert!(records.iter().any(
            |r| matches!(r, hirsel_free::record::Record::Prepared { op, .. } if *op == root.op)
        ));
        assert!(!records.iter().any(
            |r| matches!(r, hirsel_free::record::Record::Completion { op, .. } if *op == root.op)
        ));
        let projection = std::fs::read_to_string(&report).unwrap();
        for &engine in cells.engines {
            for &(width, permits) in replay_dimensions {
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
                let replay = bounded_output(
                    &mut command,
                    &program,
                    &format!(
                        "external stop {level} replay {engine:?} width{width} permits{permits}"
                    ),
                );
                assert_eq!(replay.status.code(), live.status.code(), "{replay:?}");
                assert_eq!(replay.stdout, live.stdout, "{replay:?}");
                // These are two public frontends; their exact host diagnostic
                // prefixes differ. Program stderr remains part of the projection.
                assert_eq!(
                    replay.stderr, b"hirsel: environmental failure: its place stopped the run\n",
                    "{replay:?}"
                );
                assert_eq!(std::fs::read_to_string(&report).unwrap(), projection);
            }
        }
        // Reframe exactly this observed host-wait cut as a completed flush.
        // Neither a missing terminal nor a torn later frame may invent a
        // completed host result or admit the output suffix.
        let tasks: Vec<_> = parsed
            .logs
            .iter()
            .map(|(path, log)| hirsel_free::tape::TaskChunk {
                path: path.clone(),
                progress: parsed.cut[path],
                bytes: log.clone(),
            })
            .collect();
        let mut partial = hirsel_free::tape::header(&parsed.binding);
        partial.extend(hirsel_free::tape::flush(&parsed.keys, &tasks));
        let expected = projection.replacen(&parsed.end.as_ref().unwrap().outcome, "truncated", 1);
        assert_ne!(expected, projection);
        let mut partial_reference = None;
        for torn in [false, true] {
            let mut bytes = partial.clone();
            if torn {
                bytes.extend_from_slice(&[20, 0, 0, 0, 0]);
            }
            let parsed_partial = hirsel_free::tape::parse(&bytes).unwrap();
            assert_eq!(parsed_partial.torn, torn);
            assert!(parsed_partial.end.is_none());
            assert_eq!(parsed_partial.cut, parsed.cut);
            let path = scratch.path(if torn { "torn" } else { "partial" });
            std::fs::write(&path, bytes).unwrap();
            // The whole matrix's reference is its first interpreter replay of
            // the untorn cut. A native part replays that cell first too.
            let reference = (!torn && !cells.engines.contains(&Cells::INTERPRETER[0]))
                .then_some((Cells::INTERPRETER[0], replay_dimensions[0]));
            let runs = reference.into_iter().chain(
                cells
                    .engines
                    .iter()
                    .flat_map(|&engine| replay_dimensions.iter().map(move |&cell| (engine, cell))),
            );
            for (engine, (width, permits)) in runs {
                let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                command
                    .arg("run")
                    .args(engine)
                    .arg(&program)
                    .arg("--replay")
                    .arg(&path)
                    .args(["--cores", width])
                    .env(hirsel_rt::free_replay::Report::VAR, &report);
                f1_permits(&mut command, permits);
                let replay = bounded_output(
                    &mut command,
                    &program,
                    &format!(
                        "host wait {level} partial torn={torn} {engine:?} width{width} permits{permits}"
                    ),
                );
                assert_eq!(replay.stdout, b"before stop\n", "{replay:?}");
                assert!(
                    !replay.status.success(),
                    "an unfinished host wait cannot return: {replay:?}"
                );
                assert_eq!(std::fs::read_to_string(&report).unwrap(), expected);
                let result = (replay.status.code(), replay.stderr);
                if let Some(reference) = &partial_reference {
                    assert_eq!(&result, reference);
                } else {
                    partial_reference = Some(result);
                }
            }
        }
    }
}

fn f1_returned_error_resource_matrix(
    record_width: &str,
    record_permits: usize,
    replay_dimensions: &[(&str, usize)],
) {
    let scratch = Scratch::new("f1-returned-error-resource");
    let program = scratch.path("main.hir");
    // Reuse PROG-3's returned-resource fixture and its cleanup expectations.
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/PROG/PROG-3-exit-report-resource-error-run.hir"),
    )
    .unwrap();
    std::fs::write(&program, text).unwrap();
    let report = scratch.path("projection");
    for level in ["record", "durable"] {
        let tape = scratch.path(level);
        let grants = format!(
            r#"{{"io":{{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]}},"args":[],"recording":"{level}"}}"#
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .args(["run", "--native"])
            .arg(&program)
            .args(["--grants", &grants, "--cores", record_width, "--record"])
            .arg(&tape)
            .env(hirsel_rt::free_replay::Report::VAR, &report);
        f1_permits(&mut command, record_permits);
        let live = bounded_output(
            &mut command,
            &program,
            &format!("returned resource {level} live"),
        );
        assert_eq!(live.status.code(), Some(1), "{live:?}");
        assert_eq!(live.stdout, b"beforedrop\n", "{live:?}");
        assert_eq!(live.stderr, b"error: { stdout: <Stdout> }\n", "{live:?}");
        let bytes = std::fs::read(&tape).unwrap();
        let parsed = hirsel_free::tape::parse(&bytes).unwrap();
        assert!(
            parsed
                .end
                .as_ref()
                .unwrap()
                .outcome
                .starts_with("native-outcome-v2/")
        );
        let projection = std::fs::read_to_string(&report).unwrap();
        for engine in ENGINES {
            for &(width, permits) in replay_dimensions {
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
                let replay = bounded_output(
                    &mut command,
                    &program,
                    &format!(
                        "returned resource {level} replay {engine:?} width{width} permits{permits}"
                    ),
                );
                assert_eq!(replay.status.code(), live.status.code(), "{replay:?}");
                assert_eq!(replay.stdout, live.stdout, "{replay:?}");
                assert_eq!(replay.stderr, live.stderr, "{replay:?}");
                assert_eq!(std::fs::read_to_string(&report).unwrap(), projection);
            }
        }
    }
}

#[test]
fn f1_explicit_exit_codes_replay_exact_terminal_and_projection_record() {
    f1_explicit_exit_codes_matrix(
        "1",
        1,
        &[("1", 1)],
        Cells::level(false),
        &F1_EXIT_CODE_FIXTURES,
    );
}

#[test]
fn f1_explicit_exit_codes_replay_exact_terminal_and_projection_durable() {
    f1_explicit_exit_codes_matrix(
        "1",
        1,
        &[("1", 1)],
        Cells::level(true),
        &F1_EXIT_CODE_FIXTURES,
    );
}

const F1_EXIT_CODE_FIXTURES: [&str; 3] = ["direct", "wrapped", "error"];

fn f1_explicit_exit_codes_matrix(
    record_width: &str,
    record_permits: usize,
    replay_dimensions: &[(&str, usize)],
    cells: Cells,
    fixtures: &[&str],
) {
    // Cover all three entry result ABIs using the ordinary ExitCode fixtures'
    // public constructors. Resource-bearing Result[(), E] is covered above.
    for (name, text, code, stderr) in [
        (
            "direct",
            "pub fn main() -> std@1/process.ExitCode { std@1/process.exit_code(37u8) }",
            37,
            "",
        ),
        (
            "wrapped",
            "pub fn main() -> Result[std@1/process.ExitCode, String] { Ok(std@1/process.exit_code(19u8)) }",
            19,
            "",
        ),
        (
            "error",
            "pub fn main() -> Result[std@1/process.ExitCode, String] { Error(unique \"missing data\") }",
            1,
            "error: missing data\n",
        ),
    ] {
        if !fixtures.contains(&name) {
            continue;
        }
        let scratch = Scratch::new(&format!("f1-exit-code-{name}"));
        let program = scratch.path("main.hir");
        std::fs::write(&program, text).unwrap();
        let report = scratch.path("projection");
        for &level in cells.levels {
            let tape = scratch.path(level);
            let grants = format!(
                r#"{{"io":{{"tasks":"tasks.free@1","dirs":[]}},"args":[],"recording":"{level}"}}"#
            );
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command
                .args(["run", "--native"])
                .arg(&program)
                .args(["--grants", &grants, "--cores", record_width, "--record"])
                .arg(&tape)
                .env(hirsel_rt::free_replay::Report::VAR, &report);
            f1_permits(&mut command, record_permits);
            let live = bounded_output(
                &mut command,
                &program,
                &format!("exit code {name} {level} live"),
            );
            assert_eq!(live.status.code(), Some(code), "{name}: {live:?}");
            assert!(live.stdout.is_empty(), "{live:?}");
            assert_eq!(live.stderr, stderr.as_bytes(), "{live:?}");
            let bytes = std::fs::read(&tape).unwrap();
            let parsed = hirsel_free::tape::parse(&bytes).unwrap();
            assert!(
                parsed
                    .end
                    .as_ref()
                    .unwrap()
                    .outcome
                    .starts_with("native-outcome-v2/")
            );
            let projection = std::fs::read_to_string(&report).unwrap();
            for &engine in cells.engines {
                for &(width, permits) in replay_dimensions {
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
                    let replay = bounded_output(
                        &mut command,
                        &program,
                        &format!(
                            "exit code {name} {level} replay {engine:?} width{width} permits{permits}"
                        ),
                    );
                    assert_eq!(replay.status.code(), live.status.code(), "{replay:?}");
                    assert_eq!(replay.stdout, live.stdout, "{replay:?}");
                    assert_eq!(replay.stderr, live.stderr, "{replay:?}");
                    assert_eq!(std::fs::read_to_string(&report).unwrap(), projection);
                }
            }
        }
    }
}

#[test]
fn f1_failed_root_preserves_ordered_cleanup_causes_and_projection() {
    f1_ordered_cleanup_causes_matrix("1", 1, &[("1", 1)], Cells::ALL);
}

fn f1_ordered_cleanup_causes_matrix(
    record_width: &str,
    record_permits: usize,
    replay_dimensions: &[(&str, usize)],
    cells: Cells,
) {
    let scratch = Scratch::new("f1-failed-root-causes");
    let program = scratch.path("main.hir");
    // FAIL-3's resource-unwind shape: failed drops remain causes while the
    // remaining resources still finalize in reverse declaration order.
    let text = r#"resource type First { stdout: Stdout }
drop(x) {
  let _ = out.print(x.stdout, "first\n")
  failure.check(false, Bounds())
}
resource type Second { stdout: Stdout }
drop(x) {
  let _ = out.print(x.stdout, "second\n")
  failure.check(false, TextInvariant())
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let stdout = io.stdout(rt)?
  let first = First(stdout: stdout)
  let second = Second(stdout: stdout)
  failure.check(false, Arithmetic())
  Ok(())
}
"#;
    std::fs::write(&program, text).unwrap();
    let expected_lines: Vec<_> = ["Arithmetic()", "TextInvariant()", "Bounds()"]
        .iter()
        .map(|check| text.lines().position(|line| line.contains(check)).unwrap() + 1)
        .collect();
    let report = scratch.path("projection");
    for &level in cells.levels {
        let tape = scratch.path(level);
        let grants = format!(
            r#"{{"io":{{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]}},"args":[],"recording":"{level}"}}"#
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .args(["run", "--native", "--json"])
            .arg(&program)
            .args(["--grants", &grants, "--cores", record_width, "--record"])
            .arg(&tape)
            .env(hirsel_rt::free_replay::Report::VAR, &report);
        f1_permits(&mut command, record_permits);
        let live = bounded_output(
            &mut command,
            &program,
            &format!("failure causes {level} live"),
        );
        assert_eq!(
            live.status.code(),
            Some(hirsel_run_schema::exit::FAILED),
            "{live:?}"
        );
        assert_eq!(live.stdout, b"second\nfirst\n", "{live:?}");
        let failures: Vec<serde_json::Value> = std::str::from_utf8(&live.stderr)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(failures.len(), 3, "{failures:?}");
        for (failure, &line) in failures.iter().zip(&expected_lines) {
            assert_eq!(failure["schema_version"], 1);
            assert_eq!(failure["runtime_failure"]["site"]["line"], line);
        }
        let rules: std::collections::BTreeSet<_> = failures
            .iter()
            .map(|failure| failure["runtime_failure"]["rule"].as_str().unwrap())
            .collect();
        assert_eq!(
            rules.len(),
            3,
            "distinct primary and ordered cleanup causes: {failures:?}"
        );
        let bytes = std::fs::read(&tape).unwrap();
        let parsed = hirsel_free::tape::parse(&bytes).unwrap();
        assert!(
            parsed
                .end
                .as_ref()
                .unwrap()
                .outcome
                .starts_with("native-outcome-v2/")
        );
        let projection = std::fs::read_to_string(&report).unwrap();
        for &engine in cells.engines {
            for &(width, permits) in replay_dimensions {
                let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                command
                    .args(["run", "--json"])
                    .args(engine)
                    .arg(&program)
                    .arg("--replay")
                    .arg(&tape)
                    .args(["--cores", width])
                    .env(hirsel_rt::free_replay::Report::VAR, &report);
                f1_permits(&mut command, permits);
                let replay = bounded_output(
                    &mut command,
                    &program,
                    &format!(
                        "failure causes {level} replay {engine:?} width{width} permits{permits}"
                    ),
                );
                assert_eq!(replay.status.code(), live.status.code(), "{replay:?}");
                assert_eq!(replay.stdout, live.stdout, "{replay:?}");
                assert_eq!(replay.stderr, live.stderr, "{replay:?}");
                assert_eq!(std::fs::read_to_string(&report).unwrap(), projection);
            }
        }
    }
}

#[test]
fn f1_terminal_metadata_rejects_contradictory_outcomes_and_receipts_before_effects() {
    f1_terminal_metadata_matrix("1", 1, &[("1", 1)], "record", &ENGINES);
}

fn f1_terminal_metadata_matrix(
    record_width: &str,
    record_permits: usize,
    replay_dimensions: &[(&str, usize)],
    level: &str,
    engines: &[&[&str]],
) {
    let scratch = Scratch::new("f1-terminal-metadata");
    let program = scratch.path("main.hir");
    let text = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let out = io.stdout(rt)?
  let _ = out.print(out, "terminal guard\n")
  Ok(())
}
"#;
    std::fs::write(&program, text).unwrap();
    let tape = scratch.path("original");
    let grants = format!(
        r#"{{"io":{{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]}},"args":[],"recording":"{level}"}}"#
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .args(["run", "--native"])
        .arg(&program)
        .args(["--grants", &grants, "--cores", record_width, "--record"])
        .arg(&tape);
    f1_permits(&mut command, record_permits);
    let live = bounded_output(&mut command, &program, "terminal metadata producer");
    assert!(live.status.success(), "{live:?}");
    assert_eq!(live.stdout, b"terminal guard\n");
    assert!(live.stderr.is_empty(), "{live:?}");
    let original = std::fs::read(&tape).unwrap();
    let parsed = hirsel_free::tape::parse(&original).unwrap();
    let end = parsed.end.as_ref().unwrap();
    // Fixed wire tags: Returned(Ok), then two empty Vec<u64> receipts.
    assert_eq!(
        end.outcome,
        format!("native-outcome-v2/{}", "00".repeat(24))
    );
    let tasks: Vec<_> = parsed
        .logs
        .iter()
        .map(|(path, bytes)| hirsel_free::tape::TaskChunk {
            path: path.clone(),
            progress: parsed.cut[path],
            bytes: bytes.clone(),
        })
        .collect();
    let mut failures = Vec::new();
    for (name, withdrawn, in_doubt) in [
        ("unmatched-in-doubt", vec![], vec![u64::MAX]),
        ("overlapping", vec![7u64], vec![7u64]),
        ("duplicate-withdrawn", vec![7u64, 7u64], vec![]),
        ("contradictory-outcome", vec![], vec![]),
        ("consistent-forged-outcome", vec![], vec![]),
    ] {
        let mut payload = vec![0u8; 8];
        if matches!(name, "contradictory-outcome" | "consistent-forged-outcome") {
            payload[4] = 1;
        } // Returned(Error)
        for ids in [&withdrawn, &in_doubt] {
            payload.extend_from_slice(&(ids.len() as u64).to_le_bytes());
            for id in ids {
                payload.extend_from_slice(&id.to_le_bytes());
            }
        }
        let end = hirsel_free::tape::End {
            outcome: format!("native-outcome-v2/{}", hirsel_run_schema::hex(&payload)),
            task: end.task.clone(),
        };
        let mut tasks = tasks.clone();
        if name == "consistent-forged-outcome" {
            // Metadata agrees internally; only executing the admitted root
            // can show that its actual deterministic result is Returned(Ok).
            let root = tasks.iter_mut().find(|task| task.path.is_empty()).unwrap();
            let mut records = hirsel_free::record::decode(&root.bytes).unwrap();
            let outcome = records
                .iter_mut()
                .find_map(|record| match record {
                    hirsel_free::record::Record::Observation { op, bytes }
                        if *op == root.progress.op =>
                    {
                        Some(bytes)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(outcome, &[0u8; 8]);
            outcome[4] = 1;
            root.bytes.clear();
            let mut encoder = hirsel_free::record::Encoder::default();
            for record in records {
                encoder.encode(&record, &mut root.bytes);
            }
        }
        let mut forged = hirsel_free::tape::header(&parsed.binding);
        forged.extend(hirsel_free::tape::stop(&parsed.keys, &tasks, &end));
        let path = scratch.path(name);
        std::fs::write(&path, forged).unwrap();
        for &engine in engines {
            for &(width, permits) in replay_dimensions {
                let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                command
                    .arg("run")
                    .args(engine)
                    .arg(&program)
                    .arg("--replay")
                    .arg(&path)
                    .args(["--cores", width]);
                f1_permits(&mut command, permits);
                let replay = bounded_output(
                    &mut command,
                    &program,
                    &format!("terminal metadata {name} {engine:?} width{width} permits{permits}"),
                );
                let diagnostic = String::from_utf8_lossy(&replay.stderr);
                let (expected, stdout): (&str, &[u8]) = match name {
                    "contradictory-outcome" => ("terminal outcome differs", b""),
                    "consistent-forged-outcome" => (
                        if engine.is_empty() {
                            "Free root outcome differs from recording"
                        } else {
                            "host request differs"
                        },
                        b"terminal guard\n",
                    ),
                    _ => ("terminal request receipt differs from cut", b""),
                };
                if replay.status.code() != Some(hirsel_run_schema::exit::REFUSED)
                    || replay.stdout != stdout
                    || !diagnostic.contains(expected)
                {
                    failures.push(format!("{name} {engine:?}: {replay:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Actual-eight runner acceptance. A caller replay substitutes recorded child
/// results and cannot repeat a callee's host effects.
fn f1_runner_width_and_permit_matrix(
    name: &str,
    text: &str,
    grants: &str,
    live_output: &[u8],
    replay_output: &[u8],
) {
    let scratch = Scratch::new(name);
    f1_runner_matrix_in(
        &scratch,
        text,
        grants,
        live_output,
        replay_output,
        Cells::ALL,
    );
}

/// As [`f1_runner_width_and_permit_matrix`] at one recording level: each
/// level is its own case, inside the action's time.
fn f1_runner_width_and_permit_matrix_at(
    name: &str,
    text: &str,
    grants: &str,
    output: (&[u8], &[u8]),
    durable: bool,
) {
    let cells = Cells::level(durable);
    let scratch = Scratch::new(&format!("{name}-{}", cells.levels[0]));
    f1_runner_matrix_in(&scratch, text, grants, output.0, output.1, cells);
}

fn f1_runner_matrix_in(
    scratch: &Scratch,
    text: &str,
    grants: &str,
    live_output: &[u8],
    replay_output: &[u8],
    cells: Cells,
) {
    let name = scratch.0.display();
    let program = scratch.path("main.hir");
    std::fs::write(&program, text).unwrap();
    let report = scratch.path("projection");
    let mut grants: serde_json::Value = serde_json::from_str(grants).unwrap();
    for &level in cells.levels {
        let tape = scratch.path(level);
        grants["recording"] = level.into();
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .args(["run", "--native"])
            .arg(&program)
            .args(["--grants", &grants.to_string(), "--cores", "8", "--record"])
            .arg(&tape)
            .env(hirsel_rt::free_replay::Report::VAR, &report);
        f1_permits(&mut command, 8);
        let live = bounded_output(
            &mut command,
            &program,
            &format!("{name} {level} native width8 permits8"),
        );
        assert!(live.status.success(), "{live:?}");
        assert_eq!(live.stdout, live_output, "{live:?}");
        assert!(live.stderr.is_empty(), "{live:?}");
        let bytes = std::fs::read(&tape).unwrap();
        assert!(bytes.starts_with(hirsel_free::tape::MAGIC));
        assert!(hirsel_free::tape::parse(&bytes).unwrap().end.is_some());
        let projection = std::fs::read_to_string(&report).unwrap();
        for &engine in cells.engines {
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
                    let replay = bounded_output(
                        &mut command,
                        &program,
                        &format!("{name} {level} {engine:?} width{width} permits{permits}"),
                    );
                    assert_eq!(replay.status.code(), live.status.code(), "{replay:?}");
                    assert_eq!(replay.stdout, replay_output, "{replay:?}");
                    assert_eq!(replay.stderr, live.stderr, "{replay:?}");
                    assert_eq!(std::fs::read_to_string(&report).unwrap(), projection);
                }
            }
        }
    }
}

#[test]
fn f1_runner_compute_full_projection_width_and_permit_matrix() {
    f1_runner_width_and_permit_matrix_at(
        "f1-runner-compute-matrix",
        F1_RUNNER_COMPUTE_SOURCE,
        F1_RUNNER_COMPUTE_GRANTS,
        (b"runner outcomes\n", b"runner outcomes\n"),
        false,
    );
}

#[test]
fn f1_runner_compute_full_projection_width_and_permit_matrix_durable() {
    f1_runner_width_and_permit_matrix_at(
        "f1-runner-compute-matrix",
        F1_RUNNER_COMPUTE_SOURCE,
        F1_RUNNER_COMPUTE_GRANTS,
        (b"runner outcomes\n", b"runner outcomes\n"),
        true,
    );
}

#[test]
fn f1_runner_host_waits_full_projection_width_and_permit_matrix() {
    f1_runner_width_and_permit_matrix(
        "f1-runner-host-wait-matrix",
        F1_RUNNER_HOST_WAIT_SOURCE,
        F1_RUNNER_HOST_WAIT_GRANTS,
        b"child effect\nparent result\n",
        b"parent result\n",
    );
}

#[test]
fn f1_runner_child_deadline_full_projection_width_and_permit_matrix() {
    f1_runner_width_and_permit_matrix(
        "f1-runner-child-deadline-matrix",
        F1_RUNNER_CHILD_DEADLINE_SOURCE,
        F1_RUNNER_CHILD_DEADLINE_GRANTS,
        b"child deadline result\n",
        b"child deadline result\n",
    );
}

#[test]
fn f1_runner_parent_deadline_full_projection_width_and_permit_matrix() {
    f1_runner_width_and_permit_matrix(
        "f1-runner-parent-deadline-matrix",
        F1_RUNNER_PARENT_DEADLINE_SOURCE,
        F1_RUNNER_PARENT_DEADLINE_GRANTS,
        b"forwarded parent deadline\n",
        b"forwarded parent deadline\n",
    );
}

#[test]
fn f1_runner_spawn_pending_full_projection_width_and_permit_matrix() {
    f1_runner_width_and_permit_matrix(
        "f1-runner-spawn-pending-matrix",
        F1_RUNNER_SPAWN_PENDING_SOURCE,
        F1_RUNNER_SPAWN_PENDING_GRANTS,
        b"spawn returned while pending\nspawn outcomes\n",
        b"spawn returned while pending\nspawn outcomes\n",
    );
}

#[test]
fn f1_runner_spawn_refusal_full_projection_width_and_permit_matrix() {
    f1_runner_width_and_permit_matrix(
        "f1-runner-spawn-refusal-matrix",
        F1_RUNNER_SPAWN_REFUSAL_SOURCE,
        F1_RUNNER_SPAWN_REFUSAL_GRANTS,
        b"refusal and cleanup\n",
        b"refusal and cleanup\n",
    );
}

#[test]
fn f1_runner_linked_tapes_width_and_permit_matrix_record_interpreter() {
    f1_runner_linked_tapes_matrix(Cells::at(false, Cells::INTERPRETER));
}

#[test]
fn f1_runner_linked_tapes_width_and_permit_matrix_record_llvm() {
    f1_runner_linked_tapes_matrix(Cells::at(false, Cells::LLVM));
}

#[test]
fn f1_runner_linked_tapes_width_and_permit_matrix_record_cranelift() {
    f1_runner_linked_tapes_matrix(Cells::at(false, Cells::CRANELIFT));
}

#[test]
fn f1_runner_linked_tapes_width_and_permit_matrix_durable_interpreter() {
    f1_runner_linked_tapes_matrix(Cells::at(true, Cells::INTERPRETER));
}

#[test]
fn f1_runner_linked_tapes_width_and_permit_matrix_durable_llvm() {
    f1_runner_linked_tapes_matrix(Cells::at(true, Cells::LLVM));
}

#[test]
fn f1_runner_linked_tapes_width_and_permit_matrix_durable_cranelift() {
    f1_runner_linked_tapes_matrix(Cells::at(true, Cells::CRANELIFT));
}

fn f1_runner_linked_tapes_matrix(cells: Cells) {
    let scratch = Scratch::new("f1-runner-linked-matrix");
    f1_runner_matrix_in(
        &scratch,
        F1_RUNNER_LINKED_SOURCE,
        F1_RUNNER_LINKED_GRANTS,
        b"linked child\nlinked parent\n",
        b"linked parent\n",
        cells,
    );
    let program = scratch.path("main.hir");
    for &level in cells.levels {
        let tape = scratch.path(level);
        let child_tape = scratch.path(&format!("{level}.call-0"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.args(["tapes", "check"]).arg(&tape).arg(&child_tape);
        let joined = bounded_output(&mut command, &program, "actual-eight joined runner tapes");
        assert!(joined.status.success(), "{joined:?}");
        assert!(
            String::from_utf8_lossy(&joined.stdout)
                .ends_with("tapes: 1 links checked, 0 unchecked\n"),
            "{joined:?}"
        );
        assert!(joined.stderr.is_empty(), "{joined:?}");
        f1_runner_child_replay_matrix(
            &program,
            &child_tape,
            false,
            b"linked child\n",
            cells.engines,
        );
    }
}

#[test]
fn f1_runner_crashed_child_width_and_permit_matrix_record_interpreter() {
    f1_runner_crashed_child_matrix(Cells::at(false, Cells::INTERPRETER));
}

#[test]
fn f1_runner_crashed_child_width_and_permit_matrix_record_llvm() {
    f1_runner_crashed_child_matrix(Cells::at(false, Cells::LLVM));
}

#[test]
fn f1_runner_crashed_child_width_and_permit_matrix_record_cranelift() {
    f1_runner_crashed_child_matrix(Cells::at(false, Cells::CRANELIFT));
}

#[test]
fn f1_runner_crashed_child_width_and_permit_matrix_durable_interpreter() {
    f1_runner_crashed_child_matrix(Cells::at(true, Cells::INTERPRETER));
}

#[test]
fn f1_runner_crashed_child_width_and_permit_matrix_durable_llvm() {
    f1_runner_crashed_child_matrix(Cells::at(true, Cells::LLVM));
}

#[test]
fn f1_runner_crashed_child_width_and_permit_matrix_durable_cranelift() {
    f1_runner_crashed_child_matrix(Cells::at(true, Cells::CRANELIFT));
}

fn f1_runner_crashed_child_matrix(cells: Cells) {
    let scratch = Scratch::new("f1-runner-crash-matrix");
    f1_runner_matrix_in(
        &scratch,
        F1_RUNNER_CRASH_SOURCE,
        F1_RUNNER_CRASH_GRANTS,
        b"abandoned twice\n",
        b"abandoned twice\n",
        cells,
    );
    let program = scratch.path("main.hir");
    for &level in cells.levels {
        for ordinal in 0..2 {
            let child_tape = scratch.path(&format!("{level}.call-{ordinal}"));
            let recording = hirsel_rt::host_launch::read_recording(&child_tape).unwrap();
            assert!(recording.events.iter().any(|event| {
                event.request == hirsel_rt::io::Request::Stopped
                    && event.completion == hirsel_rt::io::Completion::Ready
            }));
            assert!(
                !recording.events.iter().any(|event| {
                    matches!(event.request, hirsel_rt::io::Request::CallReturn(_))
                })
            );
            f1_runner_child_replay_matrix(&program, &child_tape, true, b"", cells.engines);
        }
    }
}

#[test]
fn f1_runner_short_crash_width_and_permit_matrix_record_interpreter() {
    f1_runner_short_matrix("crash", "Abandoned", Cells::at(false, Cells::INTERPRETER));
}

#[test]
fn f1_runner_short_crash_width_and_permit_matrix_record_llvm() {
    f1_runner_short_matrix("crash", "Abandoned", Cells::at(false, Cells::LLVM));
}

#[test]
fn f1_runner_short_crash_width_and_permit_matrix_record_cranelift() {
    f1_runner_short_matrix("crash", "Abandoned", Cells::at(false, Cells::CRANELIFT));
}

#[test]
fn f1_runner_short_crash_width_and_permit_matrix_durable_interpreter() {
    f1_runner_short_matrix("crash", "Abandoned", Cells::at(true, Cells::INTERPRETER));
}

#[test]
fn f1_runner_short_crash_width_and_permit_matrix_durable_llvm() {
    f1_runner_short_matrix("crash", "Abandoned", Cells::at(true, Cells::LLVM));
}

#[test]
fn f1_runner_short_crash_width_and_permit_matrix_durable_cranelift() {
    f1_runner_short_matrix("crash", "Abandoned", Cells::at(true, Cells::CRANELIFT));
}

#[test]
fn f1_runner_short_lose_reply_width_and_permit_matrix_record_interpreter() {
    f1_runner_short_matrix("lose_reply", "Lost", Cells::at(false, Cells::INTERPRETER));
}

#[test]
fn f1_runner_short_lose_reply_width_and_permit_matrix_record_llvm() {
    f1_runner_short_matrix("lose_reply", "Lost", Cells::at(false, Cells::LLVM));
}

#[test]
fn f1_runner_short_lose_reply_width_and_permit_matrix_record_cranelift() {
    f1_runner_short_matrix("lose_reply", "Lost", Cells::at(false, Cells::CRANELIFT));
}

#[test]
fn f1_runner_short_lose_reply_width_and_permit_matrix_durable_interpreter() {
    f1_runner_short_matrix("lose_reply", "Lost", Cells::at(true, Cells::INTERPRETER));
}

#[test]
fn f1_runner_short_lose_reply_width_and_permit_matrix_durable_llvm() {
    f1_runner_short_matrix("lose_reply", "Lost", Cells::at(true, Cells::LLVM));
}

#[test]
fn f1_runner_short_lose_reply_width_and_permit_matrix_durable_cranelift() {
    f1_runner_short_matrix("lose_reply", "Lost", Cells::at(true, Cells::CRANELIFT));
}

fn f1_runner_short_matrix(fault: &str, outcome: &str, cells: Cells) {
    {
        let scratch = Scratch::new(&format!("f1-runner-short-{fault}-matrix"));
        let (text, grants) = f1_runner_short_fixture(fault, outcome);
        f1_runner_matrix_in(
            &scratch,
            &text,
            &grants,
            b"outcome lost\n",
            b"outcome lost\n",
            cells,
        );
        let program = scratch.path("main.hir");
        for &level in cells.levels {
            let child_tape = scratch.path(&format!("{level}.call-0"));
            let recording = hirsel_rt::host_launch::read_recording(&child_tape).unwrap();
            assert!(
                !recording
                    .events
                    .iter()
                    .any(|event| event.request == hirsel_rt::io::Request::Stopped)
            );
            assert!(recording.events.iter().any(|event| matches!(
                event.request,
                hirsel_rt::io::Request::CallReturn(hirsel_run_schema::CalleeOutcome::Ok(_))
            )));
            f1_runner_child_replay_matrix(&program, &child_tape, false, b"", cells.engines);
        }
    }
}

/// Child tapes have Host admission rather than Free task logs. Check their
/// exact process outcome independently of the caller's full Free Projection.
fn f1_runner_child_replay_matrix(
    program: &Path,
    tape: &Path,
    stopped: bool,
    stdout: &[u8],
    engines: &[&[&str]],
) {
    let mut reference = None;
    for &engine in engines {
        for width in ["1", "8"] {
            for permits in [1, 8] {
                let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                command
                    .arg("run")
                    .args(engine)
                    .arg(program)
                    .arg("--replay")
                    .arg(tape)
                    .args(["--cores", width]);
                f1_permits(&mut command, permits);
                let replay = bounded_output(
                    &mut command,
                    program,
                    &format!(
                        "child {} {engine:?} width{width} permits{permits}",
                        tape.display()
                    ),
                );
                assert_eq!(replay.stdout, stdout, "{replay:?}");
                if stopped {
                    assert_eq!(
                        replay.status.code(),
                        Some(hirsel_run_schema::exit::ENVIRONMENTAL),
                        "{replay:?}"
                    );
                    assert_eq!(
                        replay.stderr,
                        b"hirsel: environmental failure: its place stopped the run\n",
                        "{replay:?}"
                    );
                } else {
                    assert!(replay.status.success(), "{replay:?}");
                    assert!(replay.stderr.is_empty(), "{replay:?}");
                }
                let observed = (replay.status.code(), replay.stdout, replay.stderr);
                if let Some(reference) = &reference {
                    assert_eq!(&observed, reference);
                } else {
                    reference = Some(observed);
                }
            }
        }
    }
}

#[test]
fn f1_returned_error_resource_width_and_permit_matrix() {
    f1_returned_error_resource_matrix("8", 8, &[("1", 1), ("1", 8), ("8", 1), ("8", 8)]);
}

#[test]
fn f1_explicit_exit_codes_width_and_permit_matrix_record_direct() {
    f1_explicit_exit_codes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::level(false),
        &["direct"],
    );
}

#[test]
fn f1_explicit_exit_codes_width_and_permit_matrix_record_wrapped() {
    f1_explicit_exit_codes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::level(false),
        &["wrapped"],
    );
}

#[test]
fn f1_explicit_exit_codes_width_and_permit_matrix_record_error() {
    f1_explicit_exit_codes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::level(false),
        &["error"],
    );
}

#[test]
fn f1_explicit_exit_codes_width_and_permit_matrix_durable_direct() {
    f1_explicit_exit_codes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::level(true),
        &["direct"],
    );
}

#[test]
fn f1_explicit_exit_codes_width_and_permit_matrix_durable_wrapped() {
    f1_explicit_exit_codes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::level(true),
        &["wrapped"],
    );
}

#[test]
fn f1_explicit_exit_codes_width_and_permit_matrix_durable_error() {
    f1_explicit_exit_codes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::level(true),
        &["error"],
    );
}

#[test]
fn f1_ordered_cleanup_causes_width_and_permit_matrix_record_interpreter() {
    f1_ordered_cleanup_causes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(false, Cells::INTERPRETER),
    );
}

#[test]
fn f1_ordered_cleanup_causes_width_and_permit_matrix_record_llvm() {
    f1_ordered_cleanup_causes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(false, Cells::LLVM),
    );
}

#[test]
fn f1_ordered_cleanup_causes_width_and_permit_matrix_record_cranelift() {
    f1_ordered_cleanup_causes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(false, Cells::CRANELIFT),
    );
}

#[test]
fn f1_ordered_cleanup_causes_width_and_permit_matrix_durable_interpreter() {
    f1_ordered_cleanup_causes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(true, Cells::INTERPRETER),
    );
}

#[test]
fn f1_ordered_cleanup_causes_width_and_permit_matrix_durable_llvm() {
    f1_ordered_cleanup_causes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(true, Cells::LLVM),
    );
}

#[test]
fn f1_ordered_cleanup_causes_width_and_permit_matrix_durable_cranelift() {
    f1_ordered_cleanup_causes_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(true, Cells::CRANELIFT),
    );
}

#[test]
fn f1_external_stop_width_and_permit_matrix_record_interpreter() {
    f1_external_stop_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(false, Cells::INTERPRETER),
    );
}

#[test]
fn f1_external_stop_width_and_permit_matrix_record_llvm() {
    f1_external_stop_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(false, Cells::LLVM),
    );
}

#[test]
fn f1_external_stop_width_and_permit_matrix_record_cranelift() {
    f1_external_stop_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(false, Cells::CRANELIFT),
    );
}

#[test]
fn f1_external_stop_width_and_permit_matrix_durable_interpreter() {
    f1_external_stop_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(true, Cells::INTERPRETER),
    );
}

#[test]
fn f1_external_stop_width_and_permit_matrix_durable_llvm() {
    f1_external_stop_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(true, Cells::LLVM),
    );
}

#[test]
fn f1_external_stop_width_and_permit_matrix_durable_cranelift() {
    f1_external_stop_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        Cells::at(true, Cells::CRANELIFT),
    );
}

#[test]
fn f1_terminal_metadata_width_and_permit_matrix_record_interpreter() {
    f1_terminal_metadata_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        "record",
        Cells::INTERPRETER,
    );
}

#[test]
fn f1_terminal_metadata_width_and_permit_matrix_record_llvm() {
    f1_terminal_metadata_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        "record",
        Cells::LLVM,
    );
}

#[test]
fn f1_terminal_metadata_width_and_permit_matrix_record_cranelift() {
    f1_terminal_metadata_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        "record",
        Cells::CRANELIFT,
    );
}

#[test]
fn f1_terminal_metadata_width_and_permit_matrix_durable_interpreter_width1() {
    // Interpreter replays cost most; one replay width per case fits its action.
    f1_terminal_metadata_matrix("8", 8, &[("1", 1), ("1", 8)], "durable", Cells::INTERPRETER);
}

#[test]
fn f1_terminal_metadata_width_and_permit_matrix_durable_interpreter_width8() {
    // Interpreter replays cost most; one replay width per case fits its action.
    f1_terminal_metadata_matrix("8", 8, &[("8", 1), ("8", 8)], "durable", Cells::INTERPRETER);
}

#[test]
fn f1_terminal_metadata_width_and_permit_matrix_durable_llvm() {
    f1_terminal_metadata_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        "durable",
        Cells::LLVM,
    );
}

#[test]
fn f1_terminal_metadata_width_and_permit_matrix_durable_cranelift() {
    f1_terminal_metadata_matrix(
        "8",
        8,
        &[("1", 1), ("1", 8), ("8", 1), ("8", 8)],
        "durable",
        Cells::CRANELIFT,
    );
}

#[test]
fn f1_terminal_root_and_consumption_width_and_permit_matrix_record_interpreter() {
    f1_terminal_restoration_matrix(
        "record",
        &[
            ("1", Some(1)),
            ("1", Some(8)),
            ("8", Some(1)),
            ("8", Some(8)),
        ],
        Cells::INTERPRETER,
    );
}

#[test]
fn f1_terminal_root_and_consumption_width_and_permit_matrix_record_llvm() {
    f1_terminal_restoration_matrix(
        "record",
        &[
            ("1", Some(1)),
            ("1", Some(8)),
            ("8", Some(1)),
            ("8", Some(8)),
        ],
        Cells::LLVM,
    );
}

#[test]
fn f1_terminal_root_and_consumption_width_and_permit_matrix_record_cranelift() {
    f1_terminal_restoration_matrix(
        "record",
        &[
            ("1", Some(1)),
            ("1", Some(8)),
            ("8", Some(1)),
            ("8", Some(8)),
        ],
        Cells::CRANELIFT,
    );
}

#[test]
fn f1_terminal_root_and_consumption_width_and_permit_matrix_durable_interpreter() {
    f1_terminal_restoration_matrix(
        "durable",
        &[
            ("1", Some(1)),
            ("1", Some(8)),
            ("8", Some(1)),
            ("8", Some(8)),
        ],
        Cells::INTERPRETER,
    );
}

#[test]
fn f1_terminal_root_and_consumption_width_and_permit_matrix_durable_llvm() {
    f1_terminal_restoration_matrix(
        "durable",
        &[
            ("1", Some(1)),
            ("1", Some(8)),
            ("8", Some(1)),
            ("8", Some(8)),
        ],
        Cells::LLVM,
    );
}

#[test]
fn f1_terminal_root_and_consumption_width_and_permit_matrix_durable_cranelift() {
    f1_terminal_restoration_matrix(
        "durable",
        &[
            ("1", Some(1)),
            ("1", Some(8)),
            ("8", Some(1)),
            ("8", Some(8)),
        ],
        Cells::CRANELIFT,
    );
}

#[test]
fn f1_bounded_directory_walk_replays_pages_and_late_error_without_tree_record() {
    f1_bounded_directory_walk_matrix(Cells::level(false));
}

#[test]
fn f1_bounded_directory_walk_replays_pages_and_late_error_without_tree_durable() {
    f1_bounded_directory_walk_matrix(Cells::level(true));
}

fn f1_bounded_directory_walk_matrix(cells: Cells) {
    // Follow cli/walk_stream.rs's empty-directory and invalid-name cases.
    // 129 root files cross the fixed 128-entry page boundary once.
    f1_corpus_matrix_with_exit_cells(
        "bounded-walk",
        r#"pub type E = Grant(GrantError) | Files(FileError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), E] {
  let _ = io.tasks(rt) ? Grant
  let out = io.stdout(rt) ? Grant
  let dir = io.dir(rt, path.of("tree")) ? Files
  let mut paths = file.walk(dir) ? Files
  let mut seen = 0u64
  let mut errors = 0u64
  while true {
    match iter.next(mut paths) {
      None() => break,
      Some(Ok(_)) => { seen += 1u64 },
      Some(Error(_)) => { errors += 1u64 },
    }
  }
  failure.check(seen == 130u64, Bounds())
  failure.check(errors == 1u64, Bounds())
  failure.check(match iter.next(mut paths) { None() => true, _ => false }, Bounds())
  out.print(out, "paged walk\n") ? Output
  Ok(())
}
"#,
        |scratch| {
            let tree = scratch.path("tree");
            std::fs::create_dir_all(tree.join("z-empty-0")).unwrap();
            std::fs::create_dir_all(tree.join("z-empty-1/deep")).unwrap();
            std::fs::create_dir_all(tree.join("z-nested")).unwrap();
            std::fs::create_dir_all(tree.join("zz-invalid")).unwrap();
            for n in 0..129 {
                std::fs::write(tree.join(format!("f{n:03}")), b"").unwrap();
            }
            std::fs::write(tree.join("z-nested/leaf"), b"").unwrap();
            use std::os::unix::ffi::OsStringExt;
            std::fs::write(
                tree.join("zz-invalid")
                    .join(std::ffi::OsString::from_vec(vec![255])),
                b"",
            )
            .unwrap();
            serde_json::json!({"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[{"path":"tree","host":tree,"access":"read"}]})
        },
        0,
        cells,
    );
}

#[test]
fn f1_required_mutex_cycle_cross_engine_matrix() {
    f1_corpus_matrix("required-mutex-cycle", source("mutex_cycle"));
}

#[test]
fn f1_required_parked_receive_cross_engine_matrix() {
    f1_corpus_matrix_with_exit(
        "required-parked-receive",
        source("parked_receive"),
        |_| serde_json::json!({"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]}),
        hirsel_run_schema::exit::BLOCKED,
    );
}

fn f1_required_r7_root_matrix(name: &str, fixture: &str) {
    let scratch = Scratch::new(name);
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/DROP")
            .join(fixture),
    )
    .unwrap();
    f1_runner_matrix_in(
        &scratch,
        &text,
        r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[]}"#,
        b"Err\n",
        b"Err\n",
        Cells::ALL,
    );
    for level in ["record", "durable"] {
        let bytes = std::fs::read(scratch.path(level)).unwrap();
        let tape = hirsel_free::tape::parse(&bytes).unwrap();
        let thaws: Vec<_> = tape
            .logs
            .values()
            .flat_map(|log| hirsel_free::record::decode(log).unwrap())
            .filter_map(|record| match record {
                hirsel_free::record::Record::Thaw { ok, .. } => Some(ok),
                _ => None,
            })
            .collect();
        assert_eq!(thaws, [false], "{name} {level}: {thaws:?}");
    }
}

#[test]
fn f1_required_r7_copy_thaw_cross_engine_matrix() {
    f1_required_r7_root_matrix("required-r7-copy", "DROP-2-root-copy-thaw-run.hir");
}

#[test]
fn f1_required_r7_aggregate_thaw_cross_engine_matrix() {
    f1_required_r7_root_matrix(
        "required-r7-aggregate",
        "DROP-2-root-aggregate-thaw-run.hir",
    );
}

#[test]
fn f1_required_process_wait_allows_other_task_file_progress_cross_engine_matrix_record() {
    f1_required_process_wait_matrix(Cells::level(false));
}

#[test]
fn f1_required_process_wait_allows_other_task_file_progress_cross_engine_matrix_durable() {
    f1_required_process_wait_matrix(Cells::level(true));
}

fn f1_required_process_wait_matrix(cells: Cells) {
    use std::os::unix::fs::PermissionsExt;
    let executable = Scratch::new("required-process-file");
    let host = executable.path("data");
    std::fs::create_dir(&host).unwrap();
    let child = host.join("child");
    // Follow the bounded process/file wait in tasks/free/host/process_tests.rs.
    // The marker proves the process is in flight before Hirsel writes its trigger.
    std::fs::write(&child, "#!/bin/sh\nprintf started > started\ni=0\nwhile [ ! -f trigger ]; do i=$((i + 1)); [ \"$i\" -lt 1000 ] || exit 42; /bin/sleep 0.001; done\n/bin/cat trigger\nprintf observed >&2\n").unwrap();
    std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o700)).unwrap();
    let text = format!(
        r#"pub type E = Grant(GrantError) | Files(FileError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), E] {{
  let tasks = io.tasks(rt) ? Grant
  let out = io.stdout(rt) ? Grant
  let p = io.process(rt) ? Grant
  let clock = io.monotonic_clock(rt) ? Grant
  let d = io.dir(rt, path.of("data")) ? Files
  let _ = file.remove(d, path.of("started"))
  let _ = file.remove(d, path.of("trigger"))
  let ok = task.scope(tasks, fn(s: Scope) -> bool {{
    let child = task.async(s, fn() -> bool {{
      match proc.run(p, Command(program: path.of("{}"), args: [], dir: Some(d), env: None(), own_group: true), b"", 64u64, None()) {{
        Ok(ran) => ran.status == Exited(0) && ran.stdout == b"ready from other task\n" && ran.stderr == b"observed",
        Error(_) => false,
      }}
    }})
    let mut ready = false
    while !ready {{
      ready = match file.read_bytes(d, path.of("started")) {{ Ok(v) => v == b"started", Error(_) => false }}
      if !ready {{
        match clock.monotonic(clock) {{
          Ok(now) => {{
            let at = match instant.checked_add(clock, now, 1000000u64) {{ Some(v) => v, None() => now }}
            let _ = clock.sleep_until(clock, at)
          }},
          Error(_) => failure.check(false, Bounds()),
        }}
      }}
    }}
    let wrote = file.write_bytes(d, path.of("trigger"), b"ready from other task\n") == Ok(())
    let child_ok = task.await(s, take child)
    wrote && child_ok
  }})
  failure.check(ok, Bounds())
  out.print(out, "process and file progressed\n") ? Output
  Ok(())
}}
"#,
        child.display()
    );
    f1_corpus_matrix_with_exit_cells(
        "required-process-file",
        &text,
        |_| serde_json::json!({"stdout":"stdout.bytes@1","tasks":"tasks.free@1","monotonic_clock":"clock.monotonic.ns@1","process":{"provider":"process.spawn@1","rules":[{"program":child,"env":[]}]},"dirs":[{"path":"data","host":host,"access":"read_write"}]}),
        0,
        cells,
    );
}

fn f1_recorded_thaws(tape: &hirsel_free::tape::Tape) -> Vec<bool> {
    tape.logs
        .values()
        .flat_map(|log| hirsel_free::record::decode(log).unwrap())
        .filter_map(|record| match record {
            hirsel_free::record::Record::Thaw { ok, .. } => Some(ok),
            _ => None,
        })
        .collect()
}

#[test]
fn f1_required_finite_release_records_unfinished_child_and_failed_thaw_record_interpreter() {
    f1_required_finite_release_matrix(Cells::part(false, false));
}

#[test]
fn f1_required_finite_release_records_unfinished_child_and_failed_thaw_record_native() {
    f1_required_finite_release_matrix(Cells::part(false, true));
}

#[test]
fn f1_required_finite_release_records_unfinished_child_and_failed_thaw_durable_interpreter() {
    f1_required_finite_release_matrix(Cells::part(true, false));
}

#[test]
fn f1_required_finite_release_records_unfinished_child_and_failed_thaw_durable_native() {
    f1_required_finite_release_matrix(Cells::part(true, true));
}

fn f1_required_finite_release_matrix(cells: Cells) {
    f1_corpus_matrix_cells(
        "required-finite-release-witness",
        source("finite_release"),
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[],"budgets":{"provider":"budgets.limits@2","fuel":200000}}),
        (
            hirsel_run_schema::exit::BUDGET_EXCEEDED,
            |live, tape, _, _| {
                assert_eq!(live.stdout, b"shared\n", "{live:?}");
                assert_eq!(f1_recorded_thaws(tape), [false]);
                let child = tape.cut.get(&vec![0]).expect("admitted child store");
                assert!(child.op > 0, "{child:?}");
                assert_eq!(child.phase, hirsel_free::phase::Phase::Done);
            },
        ),
        b"",
        || Box::new(|| {}),
        cells,
    );
}

#[test]
fn f1_required_nested_root_records_child_end_before_successful_thaw() {
    f1_corpus_matrix_with_witness(
        "required-nested-root-witness",
        source("nested_root"),
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[]}),
        (0, |live, tape, _, _| {
            assert_eq!(live.stdout, b"thawed\n", "{live:?}");
            assert_eq!(f1_recorded_thaws(tape), [true]);
            let child = tape.cut.get(&vec![0]).expect("completed child");
            assert!(child.op > 1, "{child:?}");
            assert_eq!(child.phase, hirsel_free::phase::Phase::Done);
        }),
        b"",
        || Box::new(|| {}),
    );
}

/// S-13's two callbacks, using the same C ABI/manifest shape as
/// hirsel-driver/tests/it/foreign_threads.rs.
fn f1_serial_callback_fixture() -> Scratch {
    let scratch = Scratch::new("required-serial-callbacks");
    let package = scratch.path("serial");
    std::fs::create_dir(&package).unwrap();
    std::fs::write(
        package.join("lib.c"),
        r#"#include <stdint.h>
#include <stdatomic.h>
static _Atomic uint64_t active, overlap;
uint64_t f1_serial_call(void *state, uint64_t (*callback)(void *, uint64_t), uint64_t tag) {
  if (atomic_fetch_add_explicit(&active, 1, memory_order_relaxed))
    atomic_fetch_add_explicit(&overlap, 1, memory_order_relaxed);
  uint64_t result = callback(state, tag);
  atomic_fetch_sub_explicit(&active, 1, memory_order_relaxed);
  return result;
}
uint64_t f1_serial_overlap(void) { return atomic_load_explicit(&overlap, memory_order_relaxed); }
"#,
    )
    .unwrap();
    let target = hirsel_driver::native::TargetSpec::x86_64_unknown_linux_gnu();
    let tools =
        hirsel_driver::native::Toolchain::in_tree(&scratch.path("runtime"), &[&target]).unwrap();
    let mut command = Command::new(&tools.clang);
    command
        .args(["-c", "-O1", "-fPIC"])
        .arg(package.join("lib.c"))
        .arg("-o")
        .arg(package.join("lib.o"));
    let compiled = bounded_output(&mut command, &scratch.path("main.hir"), "serial C fixture");
    assert!(compiled.status.success(), "{compiled:?}");
    std::fs::write(package.join("lib.hir"), r#"type State { tasks: Tasks }
/// Calls callback(state, tag) once, during this call; retains neither pointer.
extern "c" fn f1_serial_call(u: Unsafe, mut ctx: unique State, f: FnPtr[fn(mut state: unique State, item: u64) -> u64], tag: u64) -> u64
/// Returns the overlap counter; retains nothing.
extern "c" fn f1_serial_overlap(u: Unsafe) -> u64
fn respond(mut state: State, tag: u64) -> u64 {
  task.scope(state.tasks, fn(s: Scope) -> u64 {
    let child = task.async(s, fn() -> u64 { tag })
    task.await(s, take child)
  })
}
pub fn call(u: Unsafe, tasks: Tasks, tag: u64) -> u64 {
  let mut state = State(tasks: tasks)
  f1_serial_call(u, mut state, respond, tag)
}
pub fn overlaps(u: Unsafe) -> u64 { f1_serial_overlap(u) }
"#).unwrap();
    std::fs::write(package.join("hirsel.json"), serde_json::json!({"schema":1,"package":{"name":"serial","version":"1"},"sources":[{"path":"lib.hir","module":[]}],"dependencies":[],"unsafe_ffi":{"link":["lib.o"],"headers_checked":false,"threads":"serial","wrappers":[{"module":[],"name":"call","kind":"effect","model":"deterministic"},{"module":[],"name":"overlaps","kind":"effect","model":"deterministic"}]}}).to_string()).unwrap();
    std::fs::write(
        scratch.path("main.hir"),
        r#"pub type E = Grant(GrantError) | Output(OutputError)
pub fn main(rt: Io, u: Unsafe) -> Result[(), E] {
  let tasks = io.tasks(rt) ? Grant
  let out = io.stdout(rt) ? Grant
  let total = task.scope(tasks, fn(s: Scope) -> u64 {
    let a = task.async(s, fn() -> u64 { serial.call(u, tasks, 1u64) })
    let b = task.async(s, fn() -> u64 { serial.call(u, tasks, 2u64) })
    let first = task.await(s, take a)
    let second = task.await(s, take b)
    first + second
  })
  failure.check(total == 3u64 && serial.overlaps(u) == 0u64, Bounds())
  out.print(out, "serial callbacks\n") ? Output
  Ok(())
}
"#,
    )
    .unwrap();
    std::fs::write(scratch.path("hirsel.json"), serde_json::json!({"schema":1,"package":{"name":"serial_app","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":"serial/hirsel.json","package":{"name":"serial","version":"1"},"expose":[{"head":"serial","module":[]}]}]}).to_string()).unwrap();
    scratch
}

const F1_SERIAL_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[],"unsafe":true}"#;

#[test]
fn f1_required_serial_callbacks_interpreter_refuses_extern() {
    let scratch = f1_serial_callback_fixture();
    let program = scratch.path("hirsel.json");
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .arg("run")
        .arg(&program)
        .args(["--grants", F1_SERIAL_GRANTS]);
    f1_permits(&mut command, 1);
    let refused = bounded_output(
        &mut command,
        &scratch.path("main.hir"),
        "PROG-4 interpreter extern refusal",
    );
    assert_eq!(
        refused.status.code(),
        Some(hirsel_run_schema::exit::REFUSED),
        "{refused:?}"
    );
    assert!(refused.stdout.is_empty(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("f1_serial_call"),
        "{refused:?}"
    );
}

#[test]
fn f1_required_serial_callbacks_native_width_and_permit_matrix() {
    let scratch = f1_serial_callback_fixture();
    let program = scratch.path("hirsel.json");
    let capture = scratch.path("main.hir");
    let report = scratch.path("projection");
    let mut grants: serde_json::Value = serde_json::from_str(F1_SERIAL_GRANTS).unwrap();
    for level in ["record", "durable"] {
        grants["recording"] = level.into();
        let tape = scratch.path(level);
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .args(["run", "--native"])
            .arg(&program)
            .args(["--grants", &grants.to_string(), "--cores", "8", "--record"])
            .arg(&tape)
            .env(hirsel_rt::free_replay::Report::VAR, &report);
        f1_permits(&mut command, 8);
        let live = bounded_output(
            &mut command,
            &capture,
            &format!("serial {level} native width8 permits8"),
        );
        assert!(live.status.success(), "{live:?}");
        assert_eq!(live.stdout, b"serial callbacks\n", "{live:?}");
        assert!(live.stderr.is_empty(), "{live:?}");
        let bytes = std::fs::read(&tape).unwrap();
        assert!(bytes.starts_with(hirsel_free::tape::MAGIC));
        assert!(hirsel_free::tape::parse(&bytes).unwrap().end.is_some());
        let projection = std::fs::read_to_string(&report).unwrap();
        assert!(projection.contains("Gate(\"package:"), "{projection}");
        for engine in &ENGINES[1..] {
            for width in ["1", "8"] {
                for permits in [1, 8] {
                    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                    command
                        .arg("run")
                        .args(*engine)
                        .arg(&program)
                        .arg("--replay")
                        .arg(&tape)
                        .args(["--cores", width])
                        .env(hirsel_rt::free_replay::Report::VAR, &report);
                    f1_permits(&mut command, permits);
                    let replay = bounded_output(
                        &mut command,
                        &capture,
                        &format!("serial {level} {engine:?} width{width} permits{permits}"),
                    );
                    assert_eq!(replay.status.code(), live.status.code(), "{replay:?}");
                    assert_eq!(replay.stdout, live.stdout, "{replay:?}");
                    assert_eq!(replay.stderr, live.stderr, "{replay:?}");
                    assert_eq!(std::fs::read_to_string(&report).unwrap(), projection);
                }
            }
        }
    }
}

const F1_REQUIRED_MUTEX_CYCLE_WITNESS: &str = r#"fn discard[T](take x: T) -> () { () }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let out = io.stdout(rt)?
  let built: String = "s"
  let root: Shared[String] = shared.freeze(take built)
  let held = copy root
  let signal_text: String = "locked"
  let signal: Shared[String] = shared.freeze(take signal_text)
  let in_mutex = copy signal
  let roots: unique List[Shared[String]] = unique [take root, take in_mutex]
  let mutex = mutex.create(tasks, take roots)
  let good = task.scope(tasks, fn(scope: Scope) -> bool {
    let a = task.async(scope, fn() -> u64 {
      let mut signals: unique List[Shared[String]] = unique [take signal]
      let mut started = false
      while !started {
        match list.pop(mut signals) {
          Some(marker) => match shared.thaw(take marker) {
            Ok(text) => { discard(take text); started = true },
            Error(retry) => list.push(mut signals, take retry),
          },
          None() => failure.check(false, Bounds()),
        }
      }
      discard(take held)
      mutex.with(tasks, mutex, fn(mut rs: unique List[Shared[String]]) -> u64 uses {} { list.len(rs) })
    })
    let b = task.async(scope, fn() -> bool {
      mutex.with(tasks, mutex, fn(mut rs: unique List[Shared[String]]) -> bool uses {} {
        match list.pop(mut rs) { Some(marker) => discard(take marker), None() => failure.check(false, Bounds()) }
        let mut done = false
        while !done {
          match list.pop(mut rs) {
            Some(mine) => match shared.thaw(take mine) {
              Ok(text) => { discard(take text); done = true },
              Error(retained) => list.push(mut rs, take retained),
            },
            None() => failure.check(false, Bounds()),
          }
        }
        done
      })
    })
    let empty = task.await(scope, take a) == 0u64
    let thawed = task.await(scope, take b)
    empty && thawed
  })
  failure.check(good, Bounds())
  let _ = out.print(out, "mutex cycle thawed\n")
  Ok(())
}
"#;

#[test]
fn f1_required_mutex_cycle_witness_source_checks() {
    let scratch = Scratch::new("required-mutex-witness-check");
    let program = scratch.path("main.hir");
    std::fs::write(&program, F1_REQUIRED_MUTEX_CYCLE_WITNESS).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command.arg("check").arg(&program);
    let checked = bounded_output(&mut command, &program, "mutex cycle witness source check");
    assert!(checked.status.success(), "{checked:?}");
}

#[test]
fn f1_required_mutex_cycle_records_release_on_wait_entry_before_thaw() {
    f1_corpus_matrix_with_witness(
        "required-mutex-cycle-witness",
        F1_REQUIRED_MUTEX_CYCLE_WITNESS,
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[]}),
        (0, |live, tape, _, _| {
            assert_eq!(live.stdout, b"mutex cycle thawed\n", "{live:?}");
            let thaws = f1_recorded_thaws(tape);
            assert_eq!(thaws.last(), Some(&true), "{thaws:?}");
            assert_eq!(thaws.iter().filter(|&&ok| ok).count(), 2, "{thaws:?}");
            let a = hirsel_free::record::decode(&tape.logs[&vec![0]]).unwrap();
            assert!(a.iter().any(|record| matches!(record, hirsel_free::record::Record::Gap { gap, .. } if *gap >= 2)), "{a:?}");
        }),
        b"",
        || Box::new(|| {}),
    );
}

const F1_REQUIRED_PARKED_RECEIVE_WITNESS: &str = r#"fn discard[T](take x: T) -> () { () }
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let out = io.stdout(rt)?
  let built: String = "s"
  let root: Shared[String] = shared.freeze(take built)
  let held = copy root
  let (sender, receiver): (Sender[u32], Receiver[u32]) = channel.open(tasks)
  task.scope(tasks, fn(scope: Scope) {
    let child = task.async(scope, fn() {
      discard(take held)
      let mut rx = take receiver
      let _ = channel.receive(tasks, mut rx)
    })
    let mut roots: unique List[Shared[String]] = unique [take root]
    let mut thawed = false
    while !thawed {
      match list.pop(mut roots) {
        Some(mine) => match shared.thaw(take mine) {
          Ok(text) => { discard(take text); thawed = true },
          Error(retained) => list.push(mut roots, take retained),
        },
        None() => failure.check(false, Bounds()),
      }
    }
    let _ = out.print(out, "parked receive thawed\n")
    task.await(scope, take child)
    discard(take sender)
  })
  Ok(())
}
"#;

#[test]
fn f1_required_parked_receive_records_live_durable_output_cut_and_replays_prefix() {
    f1_corpus_matrix_with_witness(
        "required-parked-receive-witness",
        F1_REQUIRED_PARKED_RECEIVE_WITNESS,
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[]}),
        (
            hirsel_run_schema::exit::BLOCKED,
            |live, tape, scratch, level| {
                assert_eq!(live.stdout, b"parked receive thawed\n", "{live:?}");
                let thaws = f1_recorded_thaws(tape);
                assert_eq!(thaws.last(), Some(&true), "{thaws:?}");
                assert_eq!(thaws.iter().filter(|&&ok| ok).count(), 1, "{thaws:?}");
                assert_eq!(tape.cut[&vec![0]].phase, hirsel_free::phase::Phase::Entered);
                if level != "durable" {
                    return;
                }
                let bytes = std::fs::read(scratch.path(level)).unwrap();
                let mut at = hirsel_free::tape::MAGIC.len();
                let mut output_cut = None;
                // Real flush frame offsets, never a synthetic reframing of the final cut.
                while at + 8 <= bytes.len() {
                    let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
                    let end = at + 8 + len;
                    assert!(end <= bytes.len());
                    if bytes[at + 8] == 2 {
                        // FLUSH
                        let prefix = hirsel_free::tape::parse(&bytes[..end]).unwrap();
                        if prefix
                            .cut
                            .get(&vec![0])
                            .is_some_and(|p| p.phase == hirsel_free::phase::Phase::Entered)
                            && prefix
                                .cut
                                .get(&vec![])
                                .is_some_and(|p| p.phase == hirsel_free::phase::Phase::Entered)
                            && f1_recorded_thaws(&prefix).last() == Some(&true)
                        {
                            output_cut = Some(end);
                            break;
                        }
                    }
                    at = end;
                }
                let end = output_cut.expect(
                    "real durable output flush admits child's receive entry and successful thaw",
                );
                let prefix_path = scratch.path("parked-output-prefix");
                std::fs::write(&prefix_path, &bytes[..end]).unwrap();
                let program = scratch.path("main.hir");
                let report = scratch.path("prefix-projection");
                let mut reference = None;
                for engine in ENGINES {
                    for width in ["1", "8"] {
                        for permits in [1, 8] {
                            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                            command
                                .arg("run")
                                .args(engine)
                                .arg(&program)
                                .arg("--replay")
                                .arg(&prefix_path)
                                .args(["--cores", width])
                                .env(hirsel_rt::free_replay::Report::VAR, &report);
                            f1_permits(&mut command, permits);
                            let replay = bounded_output(
                                &mut command,
                                &program,
                                &format!(
                                    "parked actual durable cut {engine:?} width{width} permits{permits}"
                                ),
                            );
                            assert_eq!(
                                replay.status.code(),
                                Some(hirsel_run_schema::exit::BLOCKED),
                                "{replay:?}"
                            );
                            assert!(
                                replay.stdout.is_empty(),
                                "prepared output cannot be invented: {replay:?}"
                            );
                            let projection = std::fs::read_to_string(&report).unwrap();
                            assert!(projection.contains("truncated"), "{projection}");
                            let observed = (
                                replay.status.code(),
                                replay.stdout,
                                replay.stderr,
                                projection,
                            );
                            if let Some(reference) = &reference {
                                assert_eq!(&observed, reference);
                            } else {
                                reference = Some(observed);
                            }
                        }
                    }
                }
            },
        ),
        b"",
        || Box::new(|| {}),
    );
}

const F1_LATE_SET_SOURCE: &str = r#"fn discard[T](take value: T) -> () { () }
fn wait_release(take marker: Shared[String]) -> () {
  let mut pending: unique List[Shared[String]] = unique [take marker]
  let mut released = false
  while !released {
    match list.pop(mut pending) {
      Some(root) => match shared.thaw(take root) {
        Ok(text) => { discard(take text); released = true },
        Error(retained) => list.push(mut pending, take retained),
      },
      None() => failure.check(false, Bounds()),
    }
  }
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let out = io.stdout(rt)?
  let first_text: String = "first"
  let first_marker: Shared[String] = shared.freeze(take first_text)
  let first_held = copy first_marker
  let second_text: String = "second"
  let second_marker: Shared[String] = shared.freeze(take second_text)
  let second_held = copy second_marker
  task.scope(tasks, fn(scope: Scope) {
    let first = task.async(scope, fn() -> u64 { discard(take first_held); 11u64 })
    wait_release(take first_marker)
    let second = task.async(scope, fn() -> u64 { discard(take second_held); 22u64 })
    wait_release(take second_marker)
    let mut members: TaskSet[u64] of scope = task.set(scope)
    task.add(scope, mut members, take second)
    task.add(scope, mut members, take first)
    let a = match task.await_any(scope, members) { Some(index) => index, None() => 99u64 }
    failure.check(a == 1u64, Bounds())
    let av = task.await_at(scope, mut members, a)
    let b = match task.await_any(scope, members) { Some(index) => index, None() => 99u64 }
    failure.check(b == 0u64, Bounds())
    let bv = task.await_at(scope, mut members, b)
    failure.check(av == 11u64 && bv == 22u64, Bounds())
    let empty = match task.await_any(scope, members) { None() => true, Some(_) => false }
    failure.check(empty, Bounds())
    task.cancel_all(scope, take members)
  })
  let _ = out.print(out, "late set 1 0\n")
  Ok(())
}
"#;

#[test]
fn f1_late_task_set_and_reverse_insertion_full_projection_matrix_record() {
    f1_late_task_set_and_reverse_insertion_matrix(Cells::level(false));
}

#[test]
fn f1_late_task_set_and_reverse_insertion_full_projection_matrix_durable() {
    f1_late_task_set_and_reverse_insertion_matrix(Cells::level(true));
}

fn f1_late_task_set_and_reverse_insertion_matrix(cells: Cells) {
    // The child's pending release attaches to TaskFinish entry. The role
    // covers both entry and result-readiness publication before the parent
    // can observe its successful thaw. Neither child has another body op.
    // This exercises the same completion-rank ordering as the scoped native
    // sets::tests::late_set_creation_and_reverse_insertion control.
    f1_corpus_matrix_cells(
        "late-task-set-reverse-insertion",
        F1_LATE_SET_SOURCE,
        |_| serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[]}),
        (0, |live, tape, _, _| {
            assert_eq!(live.stdout, b"late set 1 0\n", "{live:?}");
            assert_eq!(f1_recorded_thaws(tape).iter().filter(|&&ok| ok).count(), 2);
            for child in [vec![0], vec![1]] {
                assert_eq!(tape.cut[&child].phase, hirsel_free::phase::Phase::Done);
                assert!(tape.cut[&child].op >= 2);
            }
        }),
        b"",
        || Box::new(|| {}),
        cells,
    );
}

/// Placement control's existing one-JSON-line protocol (cli/placement.rs and
/// rt/managed_cores.rs); this observes real worker counts without changing CPU
/// affinity, the process permit allowance, or the recorder's semantic inputs.
fn f1_placement_control(socket: &Path, request: serde_json::Value) -> serde_json::Value {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let mut peer = UnixStream::connect(socket).unwrap();
    peer.set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .unwrap();
    peer.set_write_timeout(Some(std::time::Duration::from_secs(1)))
        .unwrap();
    writeln!(peer, "{request}").unwrap();
    let mut reply = String::new();
    BufReader::new(peer).read_line(&mut reply).unwrap();
    let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert!(value.get("error").is_none(), "{value}");
    value
}

const F1_LIVE_SHRINK_SOURCE: &str = r#"fn pause(c: MonotonicClock) -> () {
  match clock.monotonic(c) {
    Ok(now) => {
      let at = match instant.checked_add(c, now, 1000000u64) { Some(v) => v, None() => now }
      let _ = clock.sleep_until(c, at)
    },
    Error(_) => failure.check(false, Bounds()),
  }
}
pub type E = Grant(GrantError) | Files(FileError)
pub fn main(rt: Io) -> Result[(), E] {
  let tasks = io.tasks(rt) ? Grant
  let out = io.stdout(rt) ? Grant
  let c = io.monotonic_clock(rt) ? Grant
  let d = io.dir(rt, path.of("data")) ? Files
  let ready_a: Atomic[u64] = atomic.create(tasks, 0u64)
  let ready_b: Atomic[u64] = atomic.create(tasks, 0u64)
  let done: Atomic[u64] = atomic.create(tasks, 0u64)
  let valid = task.scope(tasks, fn(scope: Scope) -> bool {
    let a = task.async(scope, fn() -> u64 {
      let items: unique List[u64] = unique [11u64, 1u64]
      atomic.store(tasks, ready_a, 1u64)
      while atomic.load(tasks, done) == 0u64 { pause(c) }
      items[0] + items[1]
    })
    let b = task.async(scope, fn() -> u64 {
      let items: unique List[u64] = unique [22u64, 2u64]
      atomic.store(tasks, ready_b, 1u64)
      while atomic.load(tasks, done) == 0u64 { pause(c) }
      items[0] + items[1]
    })
    while atomic.load(tasks, ready_a) == 0u64 || atomic.load(tasks, ready_b) == 0u64 { pause(c) }
    failure.check(file.write_bytes(d, path.of("ready"), b"both children started") == Ok(()), Bounds())
    let mut shrunk = false
    while !shrunk {
      shrunk = match file.read_bytes(d, path.of("shrunk")) {
        Ok(bytes) => { failure.check(bytes == b"shrunk", Bounds()); true },
        Error(_) => false,
      }
      if !shrunk { pause(c) }
    }
    atomic.store(tasks, done, 1u64)
    let av = task.await(scope, take a)
    let bv = task.await(scope, take b)
    av == 12u64 && bv == 24u64
  })
  failure.check(valid, Bounds())
  let _ = out.print(out, "live shrink 8 1\n")
  Ok(())
}
"#;

#[test]
fn f1_live_worker_shrink_recording_full_projection_matrix_record() {
    f1_worker_shrink_matrix(false, Cells::level(false));
}

#[test]
fn f1_live_worker_shrink_recording_full_projection_matrix_durable() {
    f1_worker_shrink_matrix(false, Cells::level(true));
}

#[test]
fn f1_started_saved_frames_migrate_before_shrink_full_projection_matrix() {
    f1_worker_shrink_matrix(true, Cells::ALL);
}

fn f1_worker_shrink_matrix(observe_migration: bool, cells: Cells) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    let peer_scratch = Scratch::new("live-shrink-control");
    let host = peer_scratch.path("data");
    std::fs::create_dir(&host).unwrap();
    let socket = peer_scratch.path("control");
    let metrics = peer_scratch.path("metrics");
    f1_corpus_matrix_cells(
        if observe_migration {
            "started-frame-migration"
        } else {
            "live-worker-shrink"
        },
        F1_LIVE_SHRINK_SOURCE,
        |_| {
            let mut grants = serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[{"path":"data","host":host,"access":"read_write"}]},"args":[],"placement":{"control":socket}});
            if observe_migration {
                grants["placement"]["metrics"] = metrics.to_string_lossy().as_ref().into();
            }
            grants
        },
        (0, |live, tape, _, _| {
            assert_eq!(live.stdout, b"live shrink 8 1\n", "{live:?}");
            for child in [vec![0], vec![1]] {
                assert_eq!(tape.cut[&child].phase, hirsel_free::phase::Phase::Done);
                assert!(tape.cut[&child].op > 3);
            }
        }),
        b"",
        || {
            let ready = host.join("ready");
            let shrunk = host.join("shrunk");
            let _ = std::fs::remove_file(&ready);
            let _ = std::fs::remove_file(&shrunk);
            let _ = std::fs::remove_file(&socket);
            if observe_migration {
                let _ = std::fs::remove_file(&metrics);
            }
            let metrics = metrics.clone();
            let socket = socket.clone();
            let stopping = Arc::new(AtomicBool::new(false));
            let stopped = stopping.clone();
            let peer = std::thread::spawn(move || {
                // Admission binds the control socket after the native build,
                // which only the child cap bounds. Startup is bounded from then.
                while !socket.exists() {
                    if stopped.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                let deadline = Instant::now() + Duration::from_secs(20);
                let mut last = serde_json::Value::Null;
                loop {
                    if stopped.load(Ordering::Acquire) {
                        return;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "live shrink startup: ready {}, last snapshot {last}",
                        ready.exists()
                    );
                    if ready.exists() {
                        let snapshot =
                            f1_placement_control(&socket, serde_json::json!({"metrics":true}));
                        last = snapshot.clone();
                        if snapshot["width"]["effective"] == 8 && snapshot["width"]["pending"] == 0
                        {
                            eprintln!("Free live shrink observed initial {snapshot}");
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                if observe_migration {
                    // Task::last starts None: first dispatch cannot increment
                    // migrations. Before this file is released, no child has
                    // finished or been joined and no shrink was requested.
                    // A positive count therefore witnesses a previously run
                    // saved frame claimed by another compute worker.
                    loop {
                        if stopped.load(Ordering::Acquire) {
                            return;
                        }
                        assert!(
                            Instant::now() < deadline,
                            "an already-started frame must migrate"
                        );
                        let observed = std::fs::read_to_string(&metrics).unwrap_or_default();
                        let migrated = observed
                            .lines()
                            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                            .find(|row| {
                                row["migrations_per_s"]
                                    .as_f64()
                                    .is_some_and(|value| value > 0.0)
                            });
                        if let Some(row) = migrated {
                            assert_eq!(row["width"]["effective"], 8);
                            assert_eq!(row["width"]["pending"], 0);
                            eprintln!("Free started frame migration observed {row}");
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                let requested =
                    f1_placement_control(&socket, serde_json::json!({"placement":{"cores":1}}));
                assert_eq!(requested["width"]["requested"], 1);
                loop {
                    if stopped.load(Ordering::Acquire) {
                        return;
                    }
                    assert!(Instant::now() < deadline, "live workers must drain to one");
                    let snapshot =
                        f1_placement_control(&socket, serde_json::json!({"metrics":true}));
                    if snapshot["width"]["effective"] == 1 && snapshot["width"]["pending"] == 0 {
                        eprintln!("Free live shrink observed settled {snapshot}");
                        std::fs::write(&shrunk, b"shrunk").unwrap();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            });
            Box::new(move || {
                stopping.store(true, Ordering::Release);
                peer.join().unwrap();
            })
        },
        cells,
    );
}

#[test]
fn f1_queued_never_started_divergent_child_full_projection_matrix() {
    let scratch = Scratch::new("queued-never-started");
    let program = scratch.path("main.hir");
    std::fs::write(
        &program,
        r#"pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let out = io.stdout(rt)?
  let gate: Atomic[u64] = atomic.create(tasks, 0u64)
  task.scope(tasks, fn(scope: Scope) -> () {
    let child = task.async(scope, fn() -> () {
      let _ = out.print(out, "divergent child entered\n")
      while true {}
    })
    let _ = atomic.load(tasks, gate)
    task.await(scope, take child)
  })
  Ok(())
}
"#,
    )
    .unwrap();
    let executable = scratch.path("program");
    let mut build = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    build.arg("build").arg(&program).arg("-o").arg(&executable);
    let built = bounded_output(&mut build, &program, "queued divergent standalone build");
    assert!(built.status.success(), "{built:?}");
    for level in ["record", "durable"] {
        let tape = scratch.path(level);
        let report = scratch.path("projection");
        let witness = scratch.path("queued-witness");
        let _ = std::fs::remove_file(&witness);
        let grants = serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"placement":{"cores":8},"recording":level});
        let mut command = Command::new(&executable);
        command
            .arg("--hirsel-record")
            .arg(&tape)
            .args(["--hirsel-grants", &grants.to_string()])
            .env("HIRSEL_RT_FREE_STOP_BEFORE_FIRST_CHILD", &witness)
            .env(hirsel_rt::free_replay::Report::VAR, &report);
        f1_permits(&mut command, 8);
        let live = bounded_output(
            &mut command,
            &program,
            &format!("queued divergent {level} actual8 live"),
        );
        assert_eq!(
            live.status.code(),
            Some(hirsel_run_schema::exit::ENVIRONMENTAL),
            "{live:?}"
        );
        assert!(live.stdout.is_empty(), "{live:?}");
        assert_eq!(
            live.stderr, b"hirsel-rt: environmental failure: its place stopped the run\n",
            "{live:?}"
        );
        let observed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&witness).unwrap()).unwrap();
        assert_eq!(
            observed,
            serde_json::json!({"first_child":true,"before_token_start":true,"requested":8,"effective":8})
        );
        eprintln!("Free queued child stopped before token start {level} {observed}");
        let bytes = std::fs::read(&tape).unwrap();
        let parsed = hirsel_free::tape::parse(&bytes).unwrap();
        assert_eq!(parsed.cut[&vec![0]], hirsel_free::phase::Progress::START);
        assert!(
            parsed
                .end
                .as_ref()
                .unwrap()
                .outcome
                .starts_with("native-stop-v2/")
        );
        let projection = std::fs::read_to_string(&report).unwrap();
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
                    let replay = bounded_output(
                        &mut command,
                        &program,
                        &format!(
                            "queued divergent {level} {engine:?} width{width} permits{permits}"
                        ),
                    );
                    assert_eq!(replay.status.code(), live.status.code(), "{replay:?}");
                    assert_eq!(replay.stdout, live.stdout, "{replay:?}");
                    assert_eq!(
                        replay.stderr,
                        b"hirsel: environmental failure: its place stopped the run\n",
                        "{replay:?}"
                    );
                    assert_eq!(std::fs::read_to_string(&report).unwrap(), projection);
                }
            }
        }
    }
}

/// Recording-off host epochs must wake namespace waiters just like recorded
/// attempts. The B1 independent-file smoke originally stalled after one create.
#[test]
fn f1_recording_off_independent_files_wake_namespace_waiters() {
    let scratch = Scratch::new("f1-live-namespace-wakes");
    let program = scratch.path("main.hir");
    std::fs::write(
        &program,
        r#"pub type AppError = Grant(GrantError) | Output(OutputError) | Files(FileError)
pub fn main(rt: Io, args: Args) -> Result[(), AppError] {
  let n = match arg.text(args, 0) {
    Ok(t) => match u64.parse(t) { Ok(v) => v, Error(_) => 1000u64, },
    Error(_) => 1000u64,
  }
  let tasks = io.tasks(rt) ? Grant
  let dir = io.dir(rt, path.of(".")) ? Files
  let sum = task.scope(tasks, fn(s: Scope) -> Result[u64, AppError] {
    let mut set: TaskSet[Result[u64, AppError]] of s = task.set(s)
    for lane in 0u64..8u64 {
      task.add(s, mut set, task.async(s, fn() -> Result[u64, AppError] {
        let mut file = file.create(dir, path.of(f"lane-{lane}")) ? Files
        let mut sum = 0u64
        for _ in 0u64..n {
          let written = file.write(dir, mut file, string.as_bytes("x")) ? Files
          sum = sum + written
        }
        Ok(sum)
      }))
    }
    let mut sum = 0u64
    for i in 0u64..8u64 { sum = sum + task.await_at(s, mut set, i)? }
    task.detach_all(s, take set)
    Ok(sum)
  })?
  let stdout = io.stdout(rt) ? Grant
  out.print(stdout, f"{sum}\n") ? Output
  Ok(())
}
"#,
    )
    .unwrap();
    let binary = scratch.path("program");
    let mut build = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    build
        .args(["build", "--release"])
        .arg(&program)
        .arg("-o")
        .arg(&binary);
    let built = bounded_output(&mut build, &program, "live namespace build");
    assert!(built.status.success(), "{built:?}");
    let root = scratch.path("files");
    std::fs::create_dir(&root).unwrap();
    let grants = scratch.path("grants.json");
    std::fs::write(
        &grants,
        serde_json::to_vec(&serde_json::json!({
            "io":{"stdout":"stdout.bytes@1", "tasks":"tasks.free@1", "dirs":[
                {"path":"", "host":root,"access":"read_write"}
            ]},
            "recording":"none", "placement":{"cores":8}
        }))
        .unwrap(),
    )
    .unwrap();
    let mut run = Command::new(&binary);
    f1_permits(&mut run, 8);
    run.arg("--hirsel-grants").arg(&grants).arg("32");
    let output = bounded_output(&mut run, &program, "live namespace file requests");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"256\n");
    for lane in 0..8 {
        assert_eq!(
            std::fs::read(root.join(format!("lane-{lane}"))).unwrap(),
            vec![b'x'; 32]
        );
    }
}

/// Whole-runtime replay identity (runtime-cutover-50): a program records on
/// both native backends at width 8 with eight permits. The LLVM tape replays on
/// the reference interpreter and both backends at widths 1/8 and permits 1/8,
/// the Cranelift tape on every engine at width/permits 1/1 and 8/8, each to the
/// same exit, bytes and full normalized projection (outputs, cuts, per-object
/// sequences and key epochs). Damaged and truncated-invalid tapes refuse before
/// any program output. Each case is one part: an LLVM replay width, the
/// Cranelift tape, or the refusals.
#[derive(Clone, Copy, PartialEq)]
enum Runtime50 {
    LlvmWidth1,
    LlvmWidth8,
    Cranelift,
    Refusals,
}

fn runtime50_matrix(name: &str, text: &str, part: Runtime50) {
    const BACKENDS: [&[&str]; 2] = [&["--native"], &["--native", "--backend", "cranelift"]];
    let grants = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[]}"#;
    let (mut replays, mut refusals) = (0, 0);
    let scratch = Scratch::new(&format!("runtime50-{name}"));
    let program = scratch.path("main.hir");
    std::fs::write(&program, text).unwrap();
    let report = scratch.path("projection");
    for (index, backend) in BACKENDS.iter().enumerate() {
        let llvm = index == 0;
        let records = match part {
            Runtime50::Cranelift => !llvm,
            _ => llvm,
        };
        if !records {
            continue;
        }
        let tape = scratch.path(&format!("tape-{index}"));
        let _ = std::fs::remove_file(&report);
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("run")
            .args(*backend)
            .arg(&program)
            .args(["--grants", grants, "--cores", "8", "--record"])
            .arg(&tape)
            .stdin(std::process::Stdio::null())
            .env(hirsel_rt::free_replay::Report::VAR, &report);
        f1_permits(&mut command, 8);
        let live = bounded_output(
            &mut command,
            &program,
            &format!("runtime50 {name} live {backend:?} width 8 permits 8"),
        );
        assert_eq!(live.status.code(), Some(0), "{name} live: {live:?}");
        let bytes = std::fs::read(&tape).unwrap();
        assert!(
            bytes.starts_with(hirsel_free::tape::MAGIC),
            "{name}: F1 tape"
        );
        let parsed = hirsel_free::tape::parse(&bytes).unwrap();
        assert!(
            !parsed.torn && parsed.end.is_some(),
            "{name}: complete tape"
        );
        assert!(!parsed.cut.is_empty(), "{name}: a terminal cut");
        let projection = std::fs::read_to_string(&report).unwrap();
        let cells: &[(&str, usize)] = match part {
            Runtime50::Refusals => &[],
            Runtime50::LlvmWidth1 => &[("1", 1), ("1", 8)],
            Runtime50::LlvmWidth8 => &[("8", 1), ("8", 8)],
            Runtime50::Cranelift => &[("1", 1), ("8", 8)],
        };
        for engine in ENGINES {
            for &(width, permits) in cells {
                let cell = format!(
                    "runtime50 {name} recorded {backend:?} replay {engine:?} \
                     width {width} permits {permits}"
                );
                let _ = std::fs::remove_file(&report);
                let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                command
                    .arg("run")
                    .args(engine)
                    .arg(&program)
                    .arg("--replay")
                    .arg(&tape)
                    .args(["--cores", width])
                    .stdin(std::process::Stdio::null())
                    .env(hirsel_rt::free_replay::Report::VAR, &report);
                f1_permits(&mut command, permits);
                let replay = bounded_output(&mut command, &program, &cell);
                assert_eq!(
                    replay.status.code(),
                    live.status.code(),
                    "{cell}: {replay:?}"
                );
                assert_eq!(replay.stdout, live.stdout, "{cell}");
                assert_eq!(replay.stderr, live.stderr, "{cell}");
                assert_eq!(
                    std::fs::read_to_string(&report).unwrap(),
                    projection,
                    "{cell}: full normalized projection"
                );
                replays += 1;
            }
        }
        // Refusal is a tape-format property: check it on the LLVM tape.
        if !llvm || !matches!(part, Runtime50::Refusals) {
            continue;
        }
        let header = hirsel_free::tape::MAGIC.len();
        let length = u32::from_le_bytes(bytes[header..header + 4].try_into().unwrap()) as usize;
        assert!(
            bytes.len() > header + 8 + length,
            "{name}: frames after the header"
        );
        let mut damaged = bytes.clone();
        damaged[header + 8 + length / 2] ^= 0x01;
        let invalid = [
            ("truncated magic", bytes[..7].to_vec()),
            ("foreign magic", {
                let mut foreign = bytes.clone();
                foreign[0] ^= 0x20;
                foreign
            }),
            // The header frame itself torn: no binding remains to admit.
            (
                "truncated header frame",
                bytes[..header + 8 + length / 2].to_vec(),
            ),
            // A damaged frame followed by more frames is corruption, not a tear.
            ("damaged header frame", damaged),
        ];
        for (what, damaged) in invalid {
            let path = scratch.path("invalid");
            std::fs::write(&path, &damaged).unwrap();
            // The magic check is engine-independent launch validation; the
            // frame-level refusals run on every engine.
            let engines: &[&[&str]] = if what.ends_with("magic") {
                &ENGINES[..1]
            } else {
                &ENGINES
            };
            for engine in engines {
                let cell = format!("runtime50 {name} {what} replay {engine:?}");
                let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                command
                    .arg("run")
                    .args(*engine)
                    .arg(&program)
                    .arg("--replay")
                    .arg(&path)
                    .stdin(std::process::Stdio::null());
                let refused = bounded_output(&mut command, &program, &cell);
                assert!(!refused.status.success(), "{cell}: {refused:?}");
                assert!(refused.stdout.is_empty(), "{cell}: output before refusal");
                refusals += 1;
            }
        }
    }
    let expected = match part {
        Runtime50::Refusals => (0, 2 + 2 * ENGINES.len()),
        _ => (ENGINES.len() * 2, 0),
    };
    assert_eq!((replays, refusals), expected);
    eprintln!("runtime50 {name} replay matrix: {replays} replays, {refusals} refusals");
}

fn corpus(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_channel_gap_llvm_width1() {
    runtime50_matrix("gap", source("gap"), Runtime50::LlvmWidth1);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_channel_gap_llvm_width8() {
    runtime50_matrix("gap", source("gap"), Runtime50::LlvmWidth8);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_channel_gap_cranelift() {
    runtime50_matrix("gap", source("gap"), Runtime50::Cranelift);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_channel_gap_refusals() {
    runtime50_matrix("gap", source("gap"), Runtime50::Refusals);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_mutex_llvm_width1() {
    runtime50_matrix("mutex", &corpus(MUTEX), Runtime50::LlvmWidth1);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_mutex_llvm_width8() {
    runtime50_matrix("mutex", &corpus(MUTEX), Runtime50::LlvmWidth8);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_mutex_cranelift() {
    runtime50_matrix("mutex", &corpus(MUTEX), Runtime50::Cranelift);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_mutex_refusals() {
    runtime50_matrix("mutex", &corpus(MUTEX), Runtime50::Refusals);
}

const MUTEX: &str = "../../conformance/CLO/CLO-3-child-mutex-capture-run.hir";

const ATOMIC: &str = "../../conformance/CLO/CLO-3-child-atomic-capture-run.hir";
const TASK_SET: &str = "../../conformance/CONC/CONC-2-task-set-first-completed-run.hir";

#[test]
fn runtime50_end_to_end_engine_replay_matrix_atomic_llvm_width1() {
    runtime50_matrix("atomic", &corpus(ATOMIC), Runtime50::LlvmWidth1);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_atomic_llvm_width8() {
    runtime50_matrix("atomic", &corpus(ATOMIC), Runtime50::LlvmWidth8);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_atomic_cranelift() {
    runtime50_matrix("atomic", &corpus(ATOMIC), Runtime50::Cranelift);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_atomic_refusals() {
    runtime50_matrix("atomic", &corpus(ATOMIC), Runtime50::Refusals);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_task_set_llvm_width1() {
    runtime50_matrix("task-set", &corpus(TASK_SET), Runtime50::LlvmWidth1);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_task_set_llvm_width8() {
    runtime50_matrix("task-set", &corpus(TASK_SET), Runtime50::LlvmWidth8);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_task_set_cranelift() {
    runtime50_matrix("task-set", &corpus(TASK_SET), Runtime50::Cranelift);
}

#[test]
fn runtime50_end_to_end_engine_replay_matrix_task_set_refusals() {
    runtime50_matrix("task-set", &corpus(TASK_SET), Runtime50::Refusals);
}

/// The single production route (runtime-cutover-50). Source and build checks:
/// no qualification feature, configuration, admission constant or probe
/// remains, and launch selects production Free exactly for `tasks.free@1`.
/// Trace checks: an ordinary build's native width-8 launch records F1 phases
/// with production host-request evidence and a projected stdout output, and
/// the interpreter replays it without a live host.
/// The `outputs` map of a reported projection: each sink and its bytes, in sink order.
fn projected_outputs(report: &str) -> Vec<(String, Vec<u8>)> {
    let [_, rest] = report.split(", outputs: {").collect::<Vec<_>>()[..] else {
        panic!("one outputs map: {report}");
    };
    let mut rest = rest;
    let mut outputs = Vec::new();
    while let Some(entry) = rest.strip_prefix('"') {
        let (sink, entry) = entry.split_once("\": [").unwrap();
        let (bytes, entry) = entry.split_once(']').unwrap();
        let bytes = bytes
            .split(", ")
            .filter(|byte| !byte.is_empty())
            .map(|byte| byte.parse().unwrap())
            .collect();
        outputs.push((sink.to_owned(), bytes));
        rest = entry.strip_prefix(", ").unwrap_or(entry);
    }
    assert!(
        rest.starts_with("}, observed: {"),
        "outputs map ends: {rest}"
    );
    outputs
}

#[test]
fn runtime50_default_routes_use_r6() {
    let sources = [
        (
            "hirsel-rt Cargo.toml",
            include_str!("../../hirsel-rt/Cargo.toml"),
        ),
        ("launch.rs", include_str!("../../hirsel-rt/src/launch.rs")),
        (
            "free_launch.rs",
            include_str!("../../hirsel-rt/src/free_launch.rs"),
        ),
        (
            "free_driver.rs",
            include_str!("../../hirsel-rt/src/free_driver.rs"),
        ),
        (
            "pool/free.rs",
            include_str!("../../hirsel-rt/src/pool/free.rs"),
        ),
        (
            "network.rs",
            include_str!("../../hirsel-driver/src/network.rs"),
        ),
    ];
    for (name, text) in sources {
        for superseded in [
            "free-qualification",
            "free_qualification",
            "FREE_QUALIFICATION",
            "free_launch::ENABLED",
            "admission_enabled",
        ] {
            assert!(!text.contains(superseded), "{name} retains {superseded}");
        }
    }
    let launch: String = sources[1].1.split_whitespace().collect();
    assert!(
        launch.contains(
            "letproduction_free=requested.as_ref()\
             .and_then(|run|run.record().io.as_ref())\
             .and_then(|io|io.tasks.as_deref())==Some(\"tasks.free@1\");"
        ),
        "the Tasks provider alone selects production Free"
    );

    let scratch = Scratch::new("runtime50-routes");
    let program = scratch.path("main.hir");
    std::fs::write(&program, source("gap")).unwrap();
    let tape = scratch.path("tape");
    let report = scratch.path("projection");
    let grants = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.free@1","dirs":[]},"args":[]}"#;
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .args(["run", "--native"])
        .arg(&program)
        .args(["--grants", grants, "--cores", "8", "--record"])
        .arg(&tape)
        .stdin(std::process::Stdio::null())
        .env(hirsel_rt::free_replay::Report::VAR, &report);
    f1_permits(&mut command, 8);
    let live = bounded_output(&mut command, &program, "runtime50 routes live LLVM width 8");
    assert!(live.status.success(), "{live:?}");
    let bytes = std::fs::read(&tape).unwrap();
    assert!(bytes.starts_with(hirsel_free::tape::MAGIC), "F1 phases");
    let parsed = hirsel_free::tape::parse(&bytes).unwrap();
    assert_eq!(
        parsed.cut[&Vec::new()].phase,
        hirsel_free::phase::Phase::Done,
        "the root's terminal phase"
    );
    assert!(parsed.logs.len() >= 2, "a child task's own phase log");
    let host = parsed
        .logs
        .values()
        .flat_map(|log| hirsel_free::record::decode(log).unwrap())
        .filter(|record| {
            matches!(
                record,
                hirsel_free::record::Record::Prepared { .. }
                    | hirsel_free::record::Record::Completion { .. }
            )
        })
        .count();
    assert!(
        host >= 2,
        "production host requests record prepare and completion"
    );
    let projection = std::fs::read_to_string(&report).unwrap();
    assert!(
        live.stdout == b"sent\n" || live.stdout == b"closed\n",
        "{live:?}"
    );
    // The gap fixture's one output is its stdout print: one exclusive host-resource sink.
    let outputs = projected_outputs(&projection);
    let [(sink, bytes)] = &outputs[..] else {
        panic!("one projected output sink: {projection}");
    };
    assert!(sink.starts_with("host-resource/"), "{sink}: {projection}");
    assert_eq!(bytes, &live.stdout, "the stdout sink's projected bytes");
    let _ = std::fs::remove_file(&report);
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .arg("run")
        .arg(&program)
        .arg("--replay")
        .arg(&tape)
        .args(["--cores", "1"])
        .stdin(std::process::Stdio::null())
        .env(hirsel_rt::free_replay::Report::VAR, &report);
    let replay = bounded_output(
        &mut command,
        &program,
        "runtime50 routes interpreter replay",
    );
    assert_eq!(replay.status.code(), live.status.code(), "{replay:?}");
    assert_eq!(replay.stdout, live.stdout);
    assert_eq!(std::fs::read_to_string(&report).unwrap(), projection);
}
