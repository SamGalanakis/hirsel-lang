//! The F0 traces (design §5 Required traces, §9 F0) under the reference cooperative executor
//! at width 1 and one permit: every seed's live run records a tape whose replay, under other
//! seeds, has the live run's projection, terminal and durable, and every truncated prefix of a
//! durable tape replays to the live projection at that flush's cut.

use crate::executor::{Drive, Ins::*, Program, record, replay};
use hirsel_free::projection::Observed;

const SEEDS: u64 = 64;
const REPLAY_SEEDS: [u64; 3] = [1, 7, 1 << 40];

/// Record under `drive`, then replay the tape and every truncated or torn prefix of it.
fn agree(name: &str, program: &Program, drive: Drive) -> crate::executor::Recorded {
    let recorded = record(program, drive);
    for seed in REPLAY_SEEDS {
        let replayed = replay(program, &recorded.tape, seed)
            .unwrap_or_else(|e| panic!("{name} {drive:?}, replay seed {seed}: {e}"));
        assert_eq!(
            replayed, recorded.live,
            "{name} {drive:?}, replay seed {seed}"
        );
    }
    // Each cut but the last has a barrier mark at the end of its chunk.
    let first = recorded.marks.len() - recorded.cuts.len();
    for (i, live) in recorded.at_cut.iter().enumerate() {
        let end = recorded.marks[first + i];
        let replayed = replay(program, &recorded.tape[..end], 3)
            .unwrap_or_else(|e| panic!("{name} {drive:?}, truncated at cut {i}: {e}"));
        assert_eq!(&replayed, live, "{name} {drive:?}, truncated at cut {i}");
        // A tape torn inside the next chunk reads as the same cut.
        let torn = replay(program, &recorded.tape[..end + 5], 3)
            .unwrap_or_else(|e| panic!("{name} {drive:?}, torn after cut {i}: {e}"));
        assert_eq!(&torn, live, "{name} {drive:?}, torn after cut {i}");
    }
    recorded
}

/// Every seed, terminal (no flush) and durable (random flushes besides the durability waits).
fn all(name: &str, program: &Program) -> Vec<crate::executor::Recorded> {
    let mut out = Vec::new();
    for seed in 0..SEEDS {
        out.push(agree(name, program, Drive::new(seed)));
        let durable = Drive {
            durable: true,
            flush_one_in: 3,
            ..Drive::new(seed)
        };
        out.push(agree(name, program, durable));
    }
    out
}

fn thaws(recorded: &crate::executor::Recorded) -> Vec<bool> {
    recorded
        .live
        .observed
        .values()
        .flatten()
        .filter_map(|o| match o {
            Observed::Thaw(ok) => Some(*ok),
            _ => None,
        })
        .collect()
}

#[test]
fn gap_boundary_on_a_fresh_object() {
    let program = Program(vec![
        vec![
            Open("c", 2),
            Spawn(1, vec![]),
            Send("c", 1),
            Close("c"),
            Recv("c"),
            Recv("c"),
            Recv("c"),
            Join(1),
        ],
        vec![Send("c", 2), Close("c")],
    ]);
    let runs = all("gap boundary", &program);
    // Both orders of the first touches happen: the child's first send at version 1 (gap 0,
    // no record) and after the creator's (gap 1 from the default p = 1).
    let first: std::collections::BTreeSet<_> = runs
        .iter()
        .map(|r| {
            r.live.objects.values().next().unwrap().versions[&2]
                .task
                .clone()
        })
        .collect();
    assert_eq!(
        first.len(),
        2,
        "both tasks touched the fresh channel first in some run"
    );
}

#[test]
fn finite_release_infinite_suffix() {
    let program = Program(vec![
        vec![
            Freeze("s", vec![]),
            Dup("s"),
            Atomic("x"),
            Spawn(1, vec!["s"]),
            Load("x"),
            Thaw("s"),
        ],
        vec![Store("x", 1), Release("s"), Loop],
    ]);
    for run in all("finite release", &program) {
        assert_eq!(run.live.outcome, "fuel");
        // The release after A's last op stays pending: B's thaw never sees count 1.
        assert_eq!(thaws(&run), vec![false]);
    }
}

#[test]
fn finite_release_needs_deferral() {
    // The mutation that applies releases at once: some live thaw sees count 1 that a replay,
    // which never runs A's trailing segment, cannot reproduce.
    let program = Program(vec![
        vec![
            Freeze("s", vec![]),
            Dup("s"),
            Atomic("x"),
            Spawn(1, vec!["s"]),
            Load("x"),
            Thaw("s"),
        ],
        vec![Store("x", 1), Release("s"), Loop],
    ]);
    let diverged = (0..SEEDS).any(|seed| {
        let recorded = record(
            &program,
            Drive {
                immediate_releases: true,
                ..Drive::new(seed)
            },
        );
        replay(&program, &recorded.tape, 1).is_err()
    });
    assert!(
        diverged,
        "without deferral the finite-release trace must fail to replay"
    );
}

#[test]
fn mutex_cycle() {
    let program = Program(vec![
        vec![
            Mutex("m"),
            Freeze("s", vec![]),
            Dup("s"),
            Take("m"),
            Spawn(1, vec!["s"]),
            Atomic("y"),
            Store("y", 1),
            Thaw("s"),
            Put("m"),
            Join(1),
        ],
        vec![Release("s"), Take("m"), Put("m")],
    ]);
    let runs = all("mutex cycle", &program);
    // The cycle itself: A's entry released S before B's thaw, which then saw count 1, while A's
    // take waited for B's put.
    assert!(
        runs.iter().any(|r| thaws(r) == vec![true]),
        "some run thaws Ok through A's entry"
    );
    assert!(runs.iter().any(|r| thaws(r) == vec![false]));
}

#[test]
fn parked_receive_durable_cut() {
    let program = Program(vec![
        vec![
            Open("c", 1),
            Freeze("s", vec![]),
            Dup("s"),
            Spawn(1, vec!["s"]),
            Atomic("y"),
            Thaw("s"),
            Write("out", "x"),
            Loop,
        ],
        vec![Release("s"), Recv("c")],
    ]);
    let mut held = false;
    for seed in 0..SEEDS {
        for drive in [
            Drive::new(seed),
            Drive {
                durable: true,
                ..Drive::new(seed)
            },
        ] {
            let run = agree("parked receive", &program, drive);
            let a = run.live.cut[&vec![0]];
            // A waits forever on the empty receive, its entry applied.
            assert_eq!((a.op, a.phase), (1, hirsel_free::phase::Phase::Entered));
            if drive.durable && thaws(&run) == vec![true] {
                // The durable cut of the write's Prepared holds A at (1, entered).
                held |= run
                    .cuts
                    .iter()
                    .any(|cut| cut.get(&vec![0]) == Some(&a) && cut.len() == 2);
            }
        }
    }
    assert!(
        held,
        "some durable cut holds A at its entered phase after B's thaw Ok"
    );
}

#[test]
fn nested_root_at_task_end() {
    let program = Program(vec![
        vec![
            Freeze("r2", vec![]),
            Dup("r2"),
            Freeze("r1", vec!["r2"]),
            Dup("r1"),
            Spawn(1, vec!["r1"]),
            Release("r1"),
            Join(1),
            Thaw("r2"),
        ],
        vec![Atomic("z")],
    ]);
    let runs = all("nested root at end", &program);
    for run in &runs {
        // The joiner's thaw after the join sees count 1, whoever freed the outer root.
        assert_eq!(thaws(run), vec![true]);
    }
    let _ = runs;
}

/// X holds Y in its frozen graph; T and U hold X; the root holds Y and thaws it. Whichever of
/// T's and U's entries applies X's last release walks X, and that order is not on the tape.
fn walker_program() -> Program {
    Program(vec![
        vec![
            Freeze("y", vec![]),
            Dup("y"),
            Freeze("x", vec!["y"]),
            Dup("x"),
            Spawn(1, vec!["x"]),
            Spawn(2, vec!["x"]),
            Atomic("w"),
            Store("w", 1),
            Store("w", 2),
            Store("w", 3),
            Thaw("y"),
        ],
        vec![Release("x"), Atomic("a"), Store("a", 1)],
        vec![Release("x"), Atomic("c"), Loop],
    ])
}

#[test]
fn a_phase_walk_releases_nested_roots_in_that_phase() {
    let runs = all("walker", &walker_program());
    assert!(
        runs.iter().any(|r| thaws(r) == vec![true]),
        "some thaw sees the nested release"
    );
}

#[test]
fn deferring_a_phase_walk_strands_nested_releases() {
    // r5's rule defers the walk's nested releases to the walker's next phase. When the replay
    // orders U's entry after T's, U walks X, and U never reaches another admitted phase, so a
    // recorded thaw Ok of Y can never be replayed.
    let program = walker_program();
    let stuck = (0..SEEDS).any(|seed| {
        let recorded = record(
            &program,
            Drive {
                deferred_walks: true,
                ..Drive::new(seed)
            },
        );
        (0..16).any(|replay_seed| replay_deferred(&program, &recorded.tape, replay_seed).is_err())
    });
    assert!(
        stuck,
        "the r5 walk rule must strand a nested release in some replay order"
    );
}

fn replay_deferred(
    program: &Program,
    tape: &[u8],
    seed: u64,
) -> Result<(), hirsel_free::replay::Diverged> {
    crate::executor::replay_with(
        program,
        tape,
        Drive {
            deferred_walks: true,
            ..Drive::new(seed)
        },
    )
    .map(drop)
}

#[test]
fn acquire_wait_acquire_reply() {
    let program = Program(vec![
        vec![
            Mutex("m"),
            Open("c", 1),
            Spawn(1, vec![]),
            Take("m"),
            Put("m"),
            Recv("c"),
            Join(1),
        ],
        vec![Take("m"), Put("m"), Send("c", 7), Close("c")],
    ]);
    for run in all("acquire, wait, acquire, reply", &program) {
        assert_eq!(run.live.outcome, "returned");
    }
}

#[test]
fn stops_at_every_step_with_requests_in_flight() {
    // The claim matrix under the executor: a stop before a claim withdraws the request, a stop
    // after it leaves it in doubt, and either way the tape replays to the live projection.
    let program = Program(vec![
        vec![
            Spawn(1, vec![]),
            Write("out", "a"),
            Write("out", "b"),
            Join(1),
        ],
        vec![Write("out", "c"), Write("err", "d")],
    ]);
    let (mut withdrawn, mut in_doubt) = (0, 0);
    for seed in 0..16 {
        for stop in 0..20 {
            for durable in [false, true] {
                let drive = Drive {
                    durable,
                    stop_at: Some(stop),
                    ..Drive::new(seed)
                };
                let run = agree("stops", &program, drive);
                withdrawn += run.withdrawn;
                in_doubt += run.in_doubt;
            }
        }
    }
    assert!(
        withdrawn > 0 && in_doubt > 0,
        "stops landed both before and after claims"
    );
}
