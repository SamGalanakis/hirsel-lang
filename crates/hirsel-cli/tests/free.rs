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
