//! Runtime r6 Ordered acceptance (runtime-ordered-50). Each test drives the
//! production Frontier through the model world at widths 1/2/4/8 with seeded
//! scheduling jitter, then the real native runtime through the same seams.

use crate::budget::stretch::{self, ENVELOPE_MAX, Event, PIECES_MAX, Settled, Stretch};
use crate::ordered::footprint::{Held, TaskId};
use crate::ordered::runtime50_model::*;
use crate::ordered::{Frontier, Next, Pause, Policy};
use hirsel_run_config::{Commit, Placement, Reorder};
use hirsel_run_schema::BudgetKind;

const SEEDS: u64 = 12;

fn rows_of<'a>(matrix: &'a Matrix, task: TaskId) -> impl Iterator<Item = &'a Row> {
    matrix
        .serial
        .canonical
        .rows
        .iter()
        .filter(move |row| row.task == task)
}

fn nth_row<'a>(matrix: &'a Matrix, task: TaskId, op: &str, nth: usize) -> (usize, &'a Row) {
    matrix
        .serial
        .canonical
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.task == task && row.op == op)
        .nth(nth)
        .unwrap_or_else(|| panic!("task {task} retired no {op} #{nth}"))
}

/// Program-order position among all retired rows.
fn position(matrix: &Matrix, task: TaskId, op: &str, nth: usize) -> usize {
    nth_row(matrix, task, op, nth).0
}

/// The retirement index of the turn containing this row; a call's rests
/// share their caller's turn.
fn turn(matrix: &Matrix, task: TaskId, op: &str, nth: usize) -> u64 {
    nth_row(matrix, task, op, nth).1.index
}

/// The poll that ran when the turn ending at retirement `index` retired.
fn woken_at(matrix: &Matrix, index: u64) -> &[TaskId] {
    &matrix.serial.canonical.wakes[index as usize].1
}

fn early_somewhere(matrix: &Matrix) -> usize {
    matrix
        .early
        .iter()
        .filter(|((width, auto), _)| *width > 1 && *auto)
        .map(|(_, count)| count)
        .sum()
}

#[test]
fn runtime50_ordered_serial_prefix_matrix() {
    // Seeded mixed programs: every cell's canonical projection, logical
    // queue after each scheduling point, wake merge and per-object history
    // equal the width-1 serial run (checked inside matrix and cell).
    let mut cells = 0;
    let mut early = 0;
    let mut blocked = 0;
    for seed in 0..48 {
        let scenario = generated(seed, false);
        let matrix = run_matrix(&scenario, SEEDS);
        cells += matrix.cells;
        early += early_somewhere(&matrix);
        blocked += usize::from(!matrix.serial.canonical.parked.is_empty());
        let serial = &matrix.serial.canonical;
        // Retirement indices are assigned at retirement: consecutive and
        // never preassigned to a queue slot.
        for (expected, row) in serial.rows.iter().enumerate() {
            assert!(row.index <= expected as u64);
        }
        assert!(serial.rows.windows(2).all(|w| w[0].index <= w[1].index));
        assert_eq!(serial.objects, matrix.unstopped.canonical.objects);
        assert_eq!(matrix.serial.history, matrix.unstopped.history);
    }
    assert_eq!(cells, 48 * 4 * SEEDS as usize * 2);
    assert!(
        early > 100,
        "auto must actually commit E ops early: {early}"
    );
    assert!(
        blocked > 0 && blocked < 48,
        "both completing and blocked programs"
    );
    eprintln!("model serial-prefix cells: {cells}, early commits: {early}, blocked: {blocked}");
    native::serial_prefix_matrix();
}

#[test]
fn runtime50_ordered_call_and_early_wake() {
    // Yielding finalizers, detach_all and inserted holding-child rests.
    let mut calls = Scenario::new(vec![
        (0, vec![op(ends())]),
        (
            1,
            vec![
                op(Act::Store(110, 1)),
                op(yields()),
                op(Act::Step(
                    "task.detach_all",
                    Next::Call {
                        members: vec![3, 4],
                        then_yield: true,
                    },
                    0,
                )),
                op(Act::Store(110, 2)),
                op(ends()),
            ],
        ),
        (
            2,
            vec![
                op(Act::Store(120, 1)),
                op(yields()),
                op(Act::Store(130, 9)),
                op(Act::Load(130)),
                op(Act::Store(120, 2)),
                op(ends()),
            ],
        ),
        (
            3,
            vec![
                op(Act::Store(130, 1)),
                op(holds()),
                // The finalizer rest yields behind the queued sibling.
                op(yields()),
                op(Act::Store(130, 2)),
                op(ends()),
            ],
        ),
        (4, vec![op(holds()), op(Act::Store(140, 1)), op(ends())]),
    ]);
    calls.holding = vec![(1, vec![3, 4])];
    let matrix = run_matrix(&calls, SEEDS);
    let serial = &matrix.serial.canonical;
    assert_eq!(serial.calls, [(1, 3), (1, 4)]);
    assert_eq!(serial.members, [(1, 3), (1, 4)]);
    assert_eq!(serial.returned, [1]);
    let detach = position(&matrix, 1, "task.detach_all", 0);
    let rest_yield = position(&matrix, 3, "task.check_canceled", 0);
    let child_end = position(&matrix, 3, "task.end", 1);
    let second_end = position(&matrix, 4, "task.end", 1);
    let caller_continues = position(&matrix, 1, "atomic.store", 1);
    assert!(detach < rest_yield && rest_yield < child_end);
    assert!(child_end < second_end && second_end < caller_continues);
    // The called rest enters the caller's turn without a new retirement
    // index; queued turns keep their logical links, not renumbered slots.
    assert_eq!(
        turn(&matrix, 1, "task.detach_all", 0),
        turn(&matrix, 3, "task.check_canceled", 0)
    );
    // The yielding rest let the queued sibling run before the rest resumed.
    let sibling = serial
        .rows
        .iter()
        .enumerate()
        .any(|(at, row)| row.task == 2 && at > rest_yield && at < child_end);
    assert!(
        sibling,
        "a queued sibling retires between a yielding finalizer's turns"
    );
    assert!(early_somewhere(&matrix) > 0);

    // An early send to a parked receiver stays pending until its turn retires.
    let send = Scenario::new(vec![
        (0, vec![op(ends())]),
        (
            1,
            vec![op(Act::Receive(30)), op(Act::Store(101, 1)), op(ends())],
        ),
        (
            2,
            vec![
                op(Act::Store(102, 1)),
                op(Act::Store(102, 2)),
                op(Act::Store(102, 3)),
                op(ends()),
            ],
        ),
        (
            3,
            vec![op(Act::Send(30, 7)), op(Act::Store(103, 1)), op(ends())],
        ),
    ]);
    let matrix = run_matrix(&send, SEEDS * 2);
    let serial = &matrix.serial.canonical;
    let sent = turn(&matrix, 3, "channel.send", 0);
    assert_eq!(woken_at(&matrix, sent), [1]);
    for index in 0..sent {
        assert!(!woken_at(&matrix, index).contains(&1));
    }
    let received = rows_of(&matrix, 1)
        .filter(|row| row.op == "channel.receive")
        .collect::<Vec<_>>();
    assert_eq!(received.len(), 2);
    assert_eq!(
        (received[0].next.clone(), received[1].result),
        (Next::Park, 7)
    );
    assert!(serial.parked.is_empty());
    let early_sends = (0..SEEDS * 2)
        .flat_map(|seed| WIDTHS.map(|width| (seed, width)))
        .filter(|&(seed, width)| {
            cell(
                &send,
                width,
                seed,
                Reorder::Auto,
                Some(&matrix.unstopped.history),
            )
            .early
            .contains(&(3, "channel.send"))
        })
        .count();
    assert!(early_sends > 0, "the send must commit early in some cell");

    // Cancellation of a host waiter wakes it at the cancel's retirement; an
    // uncancelled waiter completes at its logical host time.
    let host = Scenario::new(vec![
        (0, vec![op(ends())]),
        (
            1,
            vec![op(Act::Host(1_000)), op(Act::Store(201, 1)), op(ends())],
        ),
        (
            2,
            vec![op(Act::Host(4)), op(Act::Store(202, 1)), op(ends())],
        ),
        (
            3,
            vec![
                op(Act::Store(203, 1)),
                op(Act::Cancel(1)),
                op(Act::Store(203, 2)),
                op(ends()),
            ],
        ),
        (
            4,
            vec![op(Act::Store(204, 1)), op(Act::Store(204, 2)), op(ends())],
        ),
    ]);
    let matrix = run_matrix(&host, SEEDS);
    let canceled = turn(&matrix, 3, "task.cancel", 0);
    assert!(woken_at(&matrix, canceled).contains(&1));
    let results = |task| {
        rows_of(&matrix, task)
            .filter(|row| row.op == "host.request")
            .map(|row| (row.result, row.next.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(results(1), [(0, Next::Park), (2, Next::Keep)]);
    assert_eq!(results(2), [(0, Next::Park), (1, Next::Keep)]);
    assert!(early_somewhere(&matrix) > 0);

    // A thaw inside a mutex body is strict and the body holds no scheduling
    // point; the second body observes the first body's put.
    let mutex = Scenario::new(vec![
        (0, vec![op(ends())]),
        (
            1,
            vec![
                op(Act::Take(300)),
                op(Act::Thaw(301)),
                op(Act::Put(300, 5)),
                op(Act::Store(302, 1)),
                op(ends()),
            ],
        ),
        (
            2,
            vec![op(Act::Take(300)), op(Act::Put(300, 6)), op(ends())],
        ),
        (
            3,
            vec![
                op(Act::Store(303, 1)),
                op(Act::Take(304)),
                op(Act::Put(304, 1)),
                op(ends()),
            ],
        ),
    ]);
    let matrix = run_matrix(&mutex, SEEDS);
    let takes = rows_of(&matrix, 2)
        .filter(|row| row.op == "mutex.take")
        .map(|row| (row.result, row.next.clone()))
        .collect::<Vec<_>>();
    assert_eq!(takes, [(5, Next::Keep)]);
    assert!(position(&matrix, 1, "mutex.put", 0) < position(&matrix, 2, "mutex.take", 0));
    let mut early_takes = 0;
    for width in WIDTHS {
        for seed in 0..SEEDS {
            let observed = cell(
                &mutex,
                width,
                seed,
                Reorder::Auto,
                Some(&matrix.unstopped.history),
            );
            assert!(!observed.early.iter().any(|(_, op)| *op == "shared.thaw"));
            early_takes += observed
                .early
                .iter()
                .filter(|(_, op)| *op == "mutex.take")
                .count();
        }
    }
    assert!(early_takes > 0, "an independent mutex body commits early");

    // A sibling failure closes the retired prefix: younger early commits are
    // internal, never retired, published past the stop or extended after it.
    let mut failure = Scenario::new(vec![
        (0, vec![op(ends())]),
        (
            1,
            vec![op(Act::Store(401, 1)), op(Act::Store(401, 2)), op(ends())],
        ),
        (
            2,
            vec![
                op(Act::Store(402, 1)),
                op(Act::Fail),
                op(Act::Store(402, 2)),
                op(ends()),
            ],
        ),
        (
            3,
            vec![
                op(Act::Store(403, 1)),
                op(Act::Store(403, 2)),
                op(Act::Send(404, 1)),
                op(ends()),
            ],
        ),
    ]);
    failure.failures = true;
    let matrix = run_matrix(&failure, SEEDS * 2);
    let serial = &matrix.serial.canonical;
    assert!(serial.stopped && serial.stop.is_none());
    let last = serial.rows.last().unwrap();
    assert_eq!((last.task, last.op, &last.next), (2, "stop", &Next::Stop));
    assert!(serial.objects.is_none());
    assert!(
        matrix.unstopped.canonical.rows.len() > serial.rows.len(),
        "the failure hides a longer unstopped history"
    );
    native::call_and_early_wake();
}

#[test]
fn runtime50_budget_first_stop_and_credit() {
    let both = stretch::Budgets {
        fuel: true,
        memory: true,
    };
    // Memory-before-fuel and fuel-before-memory, settled 90/100 with one fuel
    // point, in a younger sibling's speculative stretch behind a long head.
    for (events, kind) in [
        (
            [Event::Alloc(20), Event::Fuel, Event::Fuel, Event::Free(20)],
            BudgetKind::Memory,
        ),
        (
            [Event::Fuel, Event::Fuel, Event::Alloc(20), Event::Free(20)],
            BudgetKind::Fuel,
        ),
    ] {
        let mut scenario = Scenario::new(vec![
            (0, vec![op(ends())]),
            (
                1,
                vec![
                    op(Act::Store(1, 1)),
                    op(Act::Store(1, 2)),
                    op(Act::Store(1, 3)),
                    op(ends()),
                ],
            ),
            (
                2,
                vec![
                    charged(Act::Store(2, 1), &events),
                    op(Act::Store(2, 2)),
                    op(ends()),
                ],
            ),
            (
                3,
                vec![op(Act::Store(3, 1)), op(Act::Store(3, 2)), op(ends())],
            ),
        ]);
        scenario.settled = Settled {
            level: 90,
            limit: 100,
            fuel: 1,
        };
        scenario.budgets = both;
        let matrix = run_matrix(&scenario, SEEDS);
        let serial = &matrix.serial.canonical;
        assert_eq!(
            serial.stop.map(|(task, _, kind)| (task, kind)),
            Some((2, kind))
        );
        assert_eq!(reference_stop(&scenario, serial), Some(kind));
        // The terminal prefix truncates the younger suffix in every cell.
        assert!(
            serial
                .rows
                .iter()
                .all(|row| row.task != 3 || row.index < serial.stop.unwrap().1)
        );
        assert!(early_somewhere(&matrix) > 0);
    }

    // A commit credit appears at its operation's position: 90 - 80 + 20.
    for credit in [80, 0] {
        let mut scenario = Scenario::new(vec![
            (0, vec![op(ends())]),
            (
                1,
                vec![
                    op(Act::Step("drop.trivial", Next::Keep, credit)),
                    charged(Act::Store(1, 1), &[Event::Alloc(20)]),
                    op(ends()),
                ],
            ),
            (
                2,
                vec![
                    charged(Act::Store(2, 1), &[Event::Alloc(5), Event::Free(5)]),
                    op(ends()),
                ],
            ),
        ]);
        scenario.settled = Settled {
            level: 90,
            limit: 100,
            fuel: u64::MAX,
        };
        scenario.budgets = stretch::Budgets {
            fuel: false,
            memory: true,
        };
        let matrix = run_matrix(&scenario, SEEDS);
        let serial = &matrix.serial.canonical;
        if credit == 80 {
            assert_eq!(serial.stop, None);
            assert_eq!(serial.settled.level, 30);
            assert_eq!(reference_stop(&scenario, serial), None);
        } else {
            assert_eq!(
                serial.stop.map(|(_, _, kind)| kind),
                Some(BudgetKind::Memory)
            );
            assert_eq!(reference_stop(&scenario, serial), Some(BudgetKind::Memory));
        }
    }

    // Seeded charged corpus at tight budgets: the first stop, its kind, the
    // exact charge witness and the retired prefix match the serial meter.
    let mut stops = [0usize; 3];
    for seed in 0..40 {
        let mut scenario = generated(seed, true);
        scenario.budgets = match seed % 3 {
            0 => both,
            1 => stretch::Budgets {
                fuel: true,
                memory: false,
            },
            _ => stretch::Budgets {
                fuel: false,
                memory: true,
            },
        };
        scenario.settled = Settled {
            level: 0,
            limit: if scenario.budgets.memory {
                40 + seed * 3
            } else {
                u64::MAX
            },
            fuel: if scenario.budgets.fuel {
                4 + seed % 9
            } else {
                u64::MAX
            },
        };
        let matrix = run_matrix(&scenario, 6);
        let serial = &matrix.serial.canonical;
        assert_eq!(
            reference_stop(&scenario, serial),
            serial.stop.map(|(_, _, kind)| kind),
            "seed {seed}"
        );
        stops[match serial.stop.map(|(_, _, kind)| kind) {
            None => 0,
            Some(BudgetKind::Fuel) => 1,
            Some(_) => 2,
        }] += 1;
    }
    assert!(
        stops.iter().all(|&count| count > 0),
        "fuel, memory and no stop: {stops:?}"
    );

    // 1024-entry envelopes and 64-piece stretches pause; the pause leaves
    // the stretch unchanged, and settling the paused prefix then the head's
    // live remainder equals the reference over the whole stream.
    let mut long = Stretch::new(both);
    for _ in 0..ENVELOPE_MAX {
        long.fuel();
        assert!(!long.allocation_pauses(1));
        long.alloc(1).unwrap();
    }
    long.fuel();
    assert!(long.allocation_pauses(1));
    let before = long.clone();
    assert_eq!(long.alloc(1), Err(stretch::Pause::Envelope));
    assert_eq!(long, before);
    let mut pieces = Stretch::new(both);
    for _ in 1..PIECES_MAX {
        pieces.credit(0).unwrap();
    }
    let before = pieces.clone();
    assert_eq!(pieces.credit(0), Err(stretch::Pause::Pieces));
    assert_eq!(pieces, before);
    let state = Settled {
        level: 0,
        limit: 2_000,
        fuel: 1_500,
    };
    let mut whole: Vec<Event> = (0..ENVELOPE_MAX + 8)
        .flat_map(|_| [Event::Fuel, Event::Alloc(1)])
        .collect();
    whole.push(Event::Alloc(990));
    let mut expected = state;
    let expected = (stretch::reference(&mut expected, &whole), expected);
    let split = 2 * ENVELOPE_MAX;
    let prefix = stretch::summarise(both, &whole[..split]).unwrap();
    assert_eq!(
        stretch::summarise(both, &whole[..split + 2]).err(),
        Some(stretch::Pause::Envelope)
    );
    let mut settled = state;
    let mut result = settled.settle(&prefix);
    if result.is_ok() {
        result = stretch::reference(&mut settled, &whole[split..]);
    }
    assert_eq!((result, settled), expected);

    // The head checks live and never waits on the speculation allowance;
    // a non-head reservation pauses for fuel or allowance without effect.
    let mut f: Frontier<Act, u64> = Frontier::new(
        0,
        Policy {
            speculation_bytes: 64,
            ..Policy::admit(&Placement::default(), Some(1024))
        },
        Settled {
            level: 0,
            limit: 1024,
            fuel: 10,
        },
    );
    f.held.insert(0, Held::empty());
    f.enqueue(1, Held::empty());
    assert_eq!(f.reserve(0, 1 << 20, 1 << 20), Ok(()));
    assert_eq!(f.reserve(1, 1, 11), Err(Pause::Fuel));
    assert_eq!(f.reserve(1, 65, 0), Err(Pause::Allowance));
    assert_eq!(f.reserve(1, 64, 10), Ok(()));
    assert_eq!(f.reserve(1, 1, 0), Err(Pause::Allowance));
    native::budget_first_stop_and_credit();
}

#[test]
fn runtime50_ordered_ahead_saved_custody() {
    native::ahead_saved_custody();
}

mod native {
    use super::super::{ahead, drops, engine, events, host, lifecycle, sets, terminal_tests};
    use super::{Commit, Reorder};
    use crate::tasks::{self, role};
    use hirsel_core_contract::handle_scan::{TYPED_HANDLE, WORDS};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    const SIBLINGS: u64 = 4;

    fn handle(offset: u64) -> [u64; WORDS] {
        let mut word = [0; WORDS];
        word[..4].copy_from_slice(&[
            TYPED_HANDLE,
            offset,
            crate::ordered::objects::Kind::Atomic.code(),
            2,
        ]);
        word
    }

    fn layout(size: u64, scan: &[u64]) -> crate::Layout {
        crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size,
            align: 8,
            finalizer: None,
            steps: std::ptr::null(),
            step_count: 0,
            field_count: 0,
            flags: 0,
            logical: 0,
            logical_count: std::ptr::null(),
            scan: if scan.is_empty() {
                std::ptr::null()
            } else {
                scan.as_ptr()
            },
            scan_words: scan.len() as u64,
        }
    }

    unsafe fn install_result(ctx: *mut crate::TaskCtx) {
        use super::super::values::{Field, Node, Program, Tag};
        unsafe {
            events::install_value(
                ctx,
                3,
                Program::new(
                    vec![
                        Node::Variant {
                            extent: 16,
                            tag: Tag::Ordinal { offset: 0 },
                            arms: vec![vec![], vec![Field { offset: 8, node: 1 }]],
                        },
                        Node::Bits(8),
                    ],
                    0,
                )
                .unwrap(),
            )
        };
    }

    unsafe fn finish(ctx: *mut crate::TaskCtx, identity: u64) -> u8 {
        let mut result = [1u64, identity];
        unsafe {
            tasks::hirsel_rt_v2_task_finish(
                ctx,
                result.as_mut_ptr().cast(),
                [0u64; 2].as_ptr().cast(),
                16,
                3,
            )
        }
    }

    /// Own and shared atomics with seeded physical jitter between operations;
    /// the jitter is a spin, never a language operation.
    unsafe extern "C" fn sibling(
        ctx: *mut crate::TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        let [own, shared, identity, seed] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 4]>().read() };
        let mut jitter = seed ^ identity.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for step in 0..4 {
            jitter = jitter
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            for _ in 0..(jitter >> 61) {
                std::thread::yield_now();
            }
            let value = identity * 100 + step;
            let mut seen = 0u64;
            unsafe {
                tasks::hirsel_rt_v2_atomic_store(ctx, &own, (&raw const value).cast(), 8);
                tasks::hirsel_rt_v2_atomic_load(ctx, &own, (&raw mut seen).cast(), 8);
            }
            assert_eq!(seen, value);
        }
        let mut observed = 0u64;
        unsafe {
            if identity % 2 == 0 {
                tasks::hirsel_rt_v2_atomic_store(ctx, &shared, (&raw const identity).cast(), 8);
            } else {
                tasks::hirsel_rt_v2_atomic_load(ctx, &shared, (&raw mut observed).cast(), 8);
            }
            finish(ctx, identity * 1000 + observed)
        }
    }

    /// Same-engine canonical bytes at widths 1/2/4/8, reorder off/auto and
    /// forced migration with seeded jitter all equal the width-1 trace.
    pub(super) fn serial_prefix_matrix() {
        let scan: Vec<u64> = [handle(0), handle(8)].concat();
        let environment = layout(32, &scan);
        let result = layout(16, &[]);
        let layouts = [&environment as *const crate::Layout, &result];
        let mut reference: Option<Vec<u8>> = None;
        let mut cells = 0;
        for width in super::WIDTHS {
            for reorder in [Reorder::Off, Reorder::Auto] {
                for (migrate, seed) in [(false, 0), (true, 1), (true, 2)] {
                    let mut run = crate::host::BodyRun::new(width as u32, migrate);
                    run.config.layouts = layouts.as_ptr();
                    run.config.layout_count = 2;
                    unsafe { run.use_physical_dispatch() };
                    let (outcome, trace) = run.run_traced(
                        move |ctx| unsafe {
                            let mut policy =
                                crate::ordered::Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            engine::enable(ctx, policy);
                            events::enable(ctx);
                            install_result(ctx);
                        },
                        move |ctx| unsafe {
                            let mut scope = 0;
                            tasks::hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0,
                            );
                            let mut atomics = [0u64; SIBLINGS as usize + 1];
                            for atomic in &mut atomics {
                                tasks::hirsel_rt_v2_atomic_create(
                                    ctx,
                                    std::ptr::null_mut(),
                                    atomic,
                                    (&0u64 as *const u64).cast(),
                                    8,
                                    0,
                                    8,
                                );
                            }
                            for identity in 0..SIBLINGS {
                                let environment = crate::alloc::hirsel_rt_v2_alloc(
                                    ctx,
                                    2,
                                    32,
                                    8,
                                    crate::alloc::DOMAIN_HEAP,
                                    0,
                                );
                                crate::alloc::payload(environment)
                                    .cast::<[u64; 4]>()
                                    .write([
                                        atomics[identity as usize],
                                        atomics[SIBLINGS as usize],
                                        identity,
                                        seed,
                                    ]);
                                let mut task = 0;
                                tasks::hirsel_rt_v2_task_spawn(
                                    ctx,
                                    0,
                                    0,
                                    std::ptr::null_mut(),
                                    &mut task,
                                    &scope,
                                    sibling,
                                    environment,
                                    0,
                                );
                            }
                            tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                            for (identity, atomic) in
                                atomics[..SIBLINGS as usize].iter().enumerate()
                            {
                                let mut value = 0u64;
                                tasks::hirsel_rt_v2_atomic_load(
                                    ctx,
                                    atomic,
                                    (&raw mut value).cast(),
                                    8,
                                );
                                assert_eq!(value, identity as u64 * 100 + 3);
                            }
                            crate::STATUS_OK
                        },
                    );
                    cells += 1;
                    let cell = format!("width {width} {reorder:?} migrate {migrate} seed {seed}");
                    assert_eq!(outcome.ended, Ok(crate::STATUS_OK), "{cell}");
                    assert_eq!(outcome.pending, 0, "{cell}");
                    let trace = trace.unwrap_or_else(|error| panic!("{cell}: {error}"));
                    assert!(!trace.bytes().is_empty());
                    match &reference {
                        Some(reference) => assert_eq!(trace.bytes(), &reference[..], "{cell}"),
                        None => reference = Some(trace.bytes().to_vec()),
                    }
                }
            }
        }
        assert_eq!(cells, 24);
    }

    pub(super) fn call_and_early_wake() {
        // Inserted holding-child rests before later caller logs, singly and
        // in bulk detach order.
        lifecycle::tests::run_logged_detach(false);
        lifecycle::tests::run_logged_detach(true);
        for width in [2, 4, 8] {
            // detach_all calls holding rests in add order, yields once.
            sets::tests::run_mode(width, 1);
            // Receiver leftovers and last mutex roots run yielding finalizers.
            drops::tests::run_drops(width);
        }
        // A send retired ahead of a parked receiver wakes it at its serial
        // poll; saved-receive evidence is identical at every width/policy.
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [Reorder::Off, Reorder::Auto] {
                let observed = lifecycle::tests::receive_run(width, true, true, reorder)
                    .expect("receive evidence");
                assert!(!observed.1.is_empty());
                match &reference {
                    Some(reference) => assert_eq!(&observed, reference, "{width} {reorder:?}"),
                    None => reference = Some(observed),
                }
            }
        }
        // Host cancellation of a saved waiter and idle completion.
        host::tests::owned_host_sleep_completes_from_idle_and_deadline_cancels_a_saved_request_commit(
            Commit::Shared,
        );
        // Sibling failure: a younger terminal condition and a root failure
        // wait for the older retired prefix.
        terminal_tests::terminal_run(0);
        terminal_tests::terminal_run(5);
    }

    pub(super) fn budget_first_stop_and_credit() {
        // Younger fuel exhaustion, heap/region allowance pauses, speculative
        // envelopes choosing memory or fuel first and envelope overflow.
        engine::tests::younger_budget(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        engine::tests::ordered_native_region_credits_and_first_stop_match_warm_and_cold_pools();
        terminal_tests::budget_terminal_trace_truncates_actual_younger_suffix_and_accumulates_live_head_charges_commit(
            Commit::Shared,
        );
        terminal_tests::exact_charge_trace_preserves_fuel_before_and_after_a_free_across_head_settlements_commit(
            Commit::Shared,
        );
    }

    #[derive(Default)]
    struct Entries {
        entered: [AtomicU32; SIBLINGS as usize],
        private: AtomicU32,
        finished: AtomicU64,
    }

    /// Counts physical entries. A private body may start before its spawn
    /// retires; its end (a semantic operation) returns only after adoption.
    unsafe extern "C" fn counted(
        ctx: *mut crate::TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        let [probe, ordinal] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 2]>().read() };
        let probe = unsafe { &*(probe as *const Entries) };
        probe.entered[ordinal as usize].fetch_add(1, Ordering::AcqRel);
        if ahead::private(ctx) {
            probe.private.fetch_add(1, Ordering::AcqRel);
            // Let a later sibling's private start overlap this one.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(50);
            while std::time::Instant::now() < until && ahead::private(ctx) {
                std::thread::yield_now();
            }
        }
        let status = unsafe { finish(ctx, ordinal) };
        assert!(
            !ahead::private(ctx),
            "a semantic end returned before its spawn was adopted"
        );
        probe.finished.fetch_or(1 << ordinal, Ordering::AcqRel);
        status
    }

    pub(super) fn ahead_saved_custody() {
        use ahead::tests::{Boundary, POINTS, checkpoint_case};
        // Saved fuel adopted exactly once and matching width one, and head
        // work waiting for the running private owner's save at every boundary.
        for width in [1, 2, 4] {
            checkpoint_case(width, 4, false, POINTS, Boundary::Exit);
        }
        for boundary in [
            Boundary::Exit,
            Boundary::Submit,
            Boundary::Refill,
            Boundary::Allocation,
        ] {
            checkpoint_case(2, 2, true, 1, boundary);
        }
        ahead::tests::a_second_real_frame_runs_before_its_spawn_is_admitted_and_is_adopted_once();
        // Custody restores once, adopts the actual moved record, refuses a
        // stale generation and is restored before a stopped heap is reclaimed.
        ahead::native::tests::saved_custody_restores_only_the_mask_once_without_releasing_the_source();
        ahead::native::tests::promotion_adopts_the_actual_moved_mask_and_same_physical_record();
        ahead::native::tests::stale_generation_cannot_restore_or_adopt_custody();
        ahead::native::tests::stopped_run_restores_saved_custody_before_reclaiming_its_heap();
        // Every private start is adopted, never restarted, and exposes no
        // semantic effect before adoption; traces equal width one.
        let environment = layout(16, &[]);
        let result = layout(16, &[]);
        let layouts = [&environment as *const crate::Layout, &result];
        let mut reference: Option<Vec<u8>> = None;
        let mut private = 0;
        for width in [1, 2, 4, 8] {
            for reorder in [Reorder::Off, Reorder::Auto] {
                let probe = Arc::new(Entries::default());
                let seen = probe.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 2;
                unsafe { run.use_physical_dispatch() };
                let (outcome, trace) = run.run_traced(
                    move |ctx| unsafe {
                        let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        engine::enable(ctx, policy);
                        events::enable(ctx);
                        install_result(ctx);
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        tasks::hirsel_rt_v2_task_scope_enter(
                            ctx,
                            std::ptr::null_mut(),
                            &mut scope,
                            &0,
                        );
                        for ordinal in 0..SIBLINGS {
                            let env = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                16,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            crate::alloc::payload(env)
                                .cast::<[u64; 2]>()
                                .write([Arc::as_ptr(&seen) as u64, ordinal]);
                            let mut task = 0;
                            let _role = role::Guard::new(ctx);
                            assert_eq!(
                                lifecycle::spawn_with_entry(
                                    ctx,
                                    &mut task,
                                    &scope,
                                    counted,
                                    env,
                                    0,
                                    Some(counted)
                                ),
                                Ok(crate::STATUS_OK)
                            );
                        }
                        tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                        crate::STATUS_OK
                    },
                );
                let cell = format!("width {width} {reorder:?}");
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK), "{cell}");
                assert_eq!(outcome.pending, 0, "{cell}");
                for entered in &probe.entered {
                    assert_eq!(entered.load(Ordering::Acquire), 1, "{cell}: restarted body");
                }
                assert_eq!(probe.finished.load(Ordering::Acquire), (1 << SIBLINGS) - 1);
                private += probe.private.load(Ordering::Acquire);
                let trace = trace.unwrap_or_else(|error| panic!("{cell}: {error}"));
                match &reference {
                    Some(reference) => assert_eq!(trace.bytes(), &reference[..], "{cell}"),
                    None => reference = Some(trace.bytes().to_vec()),
                }
            }
        }
        assert!(
            private > 0,
            "some child started privately ahead of its spawn"
        );
        eprintln!("private entries adopted without restart: {private}");
    }
}
