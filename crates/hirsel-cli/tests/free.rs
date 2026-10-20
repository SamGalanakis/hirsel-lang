//! Free mode at width 1 and one permit (lane F0), through its test channel
//! (`HIRSEL_RT_FREE_TEST`), on the interpreter and both native backends: each trace program
//! records under seeded schedules with injected flushes, terminal and durable, then replays
//! to its live projection under other seeds, and from every cut its report names, truncated
//! there and torn five bytes past it, to the live projection at that cut.
use std::path::{Path, PathBuf};
use std::process::Command;

hirsel_guard::install!();

static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The interpreter, then each native backend.
const ENGINES: [&[&str]; 3] = [&[], &["--native"], &["--native", "--backend", "cranelift"]];

const GRANTS: &str =
    r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"args":[]}"#;
/// `finite_release` spends its fuel inside a releasing walk.
const FUEL_GRANTS: &str = r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"args":[],"budgets":{"provider":"budgets.limits@1","fuel":200000}}"#;

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

/// One run of `program` under the test channel `hook`; its stdout.
fn run(engine: &[&str], program: &Path, grants: &str, hook: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .args(engine)
        .arg(program)
        .arg("--grants")
        .arg(grants)
        .env("HIRSEL_RT_FREE_TEST", hook)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A replay of `tape` under `seed`: the projection it reports.
fn replayed(engine: &[&str], program: &Path, grants: &str, tape: &Path, seed: u64) -> String {
    let report = tape.with_extension("replayed");
    let _ = std::fs::remove_file(&report);
    run(
        engine,
        program,
        grants,
        &format!(
            "mode=replay,tape={},report={},seed={seed}",
            tape.display(),
            report.display()
        ),
    );
    let text = std::fs::read_to_string(&report).unwrap_or_default();
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
    let stdout = run(
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
    let report = std::fs::read_to_string(&report).unwrap_or_else(|e| panic!("{what}: {e}"));
    let cuts = report
        .lines()
        .map(|line| {
            let rest = line.strip_prefix("cut ").unwrap();
            let (offset, projection) = rest.split_once(' ').unwrap();
            (offset.parse().unwrap(), projection.to_owned())
        })
        .collect::<Vec<_>>();
    assert!(!cuts.is_empty(), "{what}: no cut");
    Recorded {
        scratch,
        what,
        stdout,
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
    std::thread::scope(|scope| {
        for (name, outcomes) in [
            ("mutex_cycle", ["thawed\n", "shared\n"]),
            ("acquire_reply", ["a first\n", "b first\n"]),
            ("thaw_race", ["thawed\n", "shared\n"]),
        ] {
            for engine in ENGINES {
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
