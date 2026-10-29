use super::*;
use crate::budget::stretch::{Budgets, Event, summarise};
use footprint::{Item, ObjectId};
use hirsel_rt_operations::descriptors::op_descriptor;

#[derive(Clone, Debug)]
enum Command {
    Store(ObjectId, u64),
    Load(ObjectId),
    Step(Next, u64),
}

#[derive(Default)]
struct Objects {
    values: BTreeMap<ObjectId, u64>,
    committed: Vec<(TaskId, &'static str, bool)>,
    trace: Vec<(u64, TaskId, &'static str, u64)>,
    published: Vec<TaskId>,
    polls: u64,
    wakes: Vec<TaskId>,
    deferred: bool,
    block_retirement: bool,
    calls: Vec<(TaskId, TaskId)>,
    members: Vec<(TaskId, TaskId)>,
    returned_calls: Vec<TaskId>,
    retirements: Vec<(TaskId, bool, u64, Stretch, Next, u64)>,
}

impl Backend<Command> for Objects {
    type Result = u64;
    fn can_retire(&mut self, _: TaskId, _: &Operation<Command>) -> bool {
        !self.block_retirement
    }
    fn commit(&mut self, task: TaskId, op: &Operation<Command>, early: bool) -> Receipt<u64> {
        self.committed.push((task, op.descriptor.name, early));
        let (result, next, credit) = match &op.payload {
            Command::Store(object, value) => {
                self.values.insert(object.clone(), *value);
                (0, Next::Yield, 0)
            }
            Command::Load(object) => (
                self.values.get(object).copied().unwrap_or(0),
                Next::Yield,
                0,
            ),
            Command::Step(next, credit) => (0, next.clone(), *credit),
        };
        Receipt {
            result,
            next,
            credit,
        }
    }
    fn record(&mut self, task: TaskId, op: &Operation<Command>, result: &u64, retirement: u64) {
        self.trace
            .push((retirement, task, op.descriptor.name, *result));
    }
    fn record_retirement(
        &mut self,
        task: TaskId,
        op: &Operation<Command>,
        result: &u64,
        record: Retirement<'_>,
    ) {
        self.record(task, op, result, record.index);
        self.retirements.push((
            task,
            false,
            record.index,
            record.before.clone(),
            record.transition.clone(),
            record.credit,
        ));
    }
    fn record_continuation(&mut self, task: TaskId, record: Retirement<'_>) {
        self.retirements.push((
            task,
            true,
            record.index,
            record.before.clone(),
            record.transition.clone(),
            record.credit,
        ));
    }
    fn publish(&mut self, task: TaskId, _: &Operation<Command>, _: &u64) {
        self.published.push(task);
    }
    fn defer_poll(&self) -> bool {
        self.deferred
    }
    fn call(&mut self, caller: TaskId, child: TaskId) {
        self.calls.push((caller, child));
    }
    fn call_member(&mut self, caller: TaskId, child: TaskId) {
        self.members.push((caller, child));
    }
    fn call_returned(&mut self, caller: TaskId) {
        self.returned_calls.push(caller);
    }
    fn poll(&mut self) -> Vec<TaskId> {
        self.polls += 1;
        std::mem::take(&mut self.wakes)
    }
}

const BOTH: Budgets = Budgets {
    fuel: true,
    memory: true,
};
fn stretch(events: &[Event]) -> Stretch {
    summarise(BOTH, events).unwrap()
}
fn empty() -> Stretch {
    stretch(&[])
}
fn object(n: u64) -> ObjectId {
    ObjectId {
        creator: vec![0],
        ordinal: n,
    }
}
fn operation(name: &str, payload: Command, reads: &[u64], writes: &[u64]) -> Operation<Command> {
    Operation {
        descriptor: op_descriptor(name).expect("an operation descriptor"),
        footprint: Footprint {
            reads: reads.iter().map(|&n| Item::Object(object(n))).collect(),
            writes: writes.iter().map(|&n| Item::Object(object(n))).collect(),
        },
        payload,
    }
}
fn step(name: &str, next: Next) -> Operation<Command> {
    operation(name, Command::Step(next, 0), &[], &[])
}
fn store(n: u64, value: u64) -> Operation<Command> {
    operation("atomic.store", Command::Store(object(n), value), &[], &[n])
}
fn load(n: u64) -> Operation<Command> {
    operation("atomic.load", Command::Load(object(n)), &[n], &[])
}
fn frontier() -> (Frontier<Command, u64>, Objects) {
    let mut f = Frontier::new(
        0,
        Policy::admit(&Placement::default(), None),
        Settled {
            level: 0,
            limit: u64::MAX,
            fuel: u64::MAX,
        },
    );
    let mut b = Objects::default();
    f.held.insert(0, Held::empty());
    f.enqueue(1, Held::empty());
    f.enqueue(2, Held::empty());
    f.apply_head(empty(), step("task.check_canceled", Next::Park), &mut b)
        .unwrap();
    assert_eq!(f.head(), Some(1));
    b.trace.clear();
    b.committed.clear();
    b.polls = 0;
    (f, b)
}

#[test]
fn admission_preserves_explicit_zero_and_policy_defaults() {
    let p = Policy::admit(&Placement::default(), Some(1024));
    assert_eq!(p.run_ahead, 32);
    assert_eq!(p.speculation_bytes, 64);
    assert_eq!(p.reorder, Reorder::Auto);
    let explicit = Placement {
        run_ahead: Some(0),
        reorder: Some(Reorder::Off),
        speculation_bytes: Some(0),
        ..Placement::default()
    };
    let p = Policy::admit(&explicit, None);
    assert_eq!(p.run_ahead, 0);
    assert_eq!(p.speculation_bytes, 0);
    assert_eq!(p.reorder, Reorder::Off);
}

#[test]
fn independent_sibling_ops_pass_a_long_head_and_retire_once() {
    let (mut f, mut b) = frontier();
    f.held.get_mut(&1).unwrap().objects.insert(object(1));
    f.held.get_mut(&2).unwrap().objects.insert(object(2));
    assert_eq!(f.offer(2, empty(), store(2, 42), &mut b).0, Offer::Early);
    assert_eq!(f.early_result(2), Some(&0));
    assert_eq!(b.values[&object(2)], 42);
    assert!(b.trace.is_empty());
    assert_eq!(b.polls, 0);
    f.apply_head(empty(), load(1), &mut b).unwrap();
    assert_eq!(f.head(), Some(2));
    f.absorb(&mut b).unwrap();
    assert_eq!(
        b.committed.iter().filter(|(task, _, _)| *task == 2).count(),
        1
    );
    assert_eq!(b.published, [2]);
    assert_eq!(
        b.trace.iter().map(|(_, t, _, _)| *t).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(b.polls, 2);
}

#[test]
fn early_ops_are_bounded_to_the_first_queued_turn_per_head() {
    let (mut f, mut b) = frontier();
    assert_eq!(f.offer(2, empty(), store(7, 1), &mut b).0, Offer::Early);
    assert!(!f.eligible(2, &store(7, 2)));
    assert_eq!(f.offer(2, empty(), store(7, 2), &mut b).0, Offer::Logged);
    assert_eq!(b.values[&object(7)], 1);
    f.apply_head(empty(), load(7), &mut b).unwrap();
    assert_eq!(b.trace.last().unwrap().3, 1);
    f.absorb(&mut b).unwrap();
    // The second speculative turn has not committed at the first takeover.
    assert_eq!(b.values[&object(7)], 1);
}

#[test]
fn newly_enqueued_tasks_cannot_pass_the_current_head() {
    let (mut f, _) = frontier();
    f.enqueue(3, Held::empty());
    assert!(!f.eligible(3, &store(7, 1)));
}

#[test]
fn logged_keep_conflict_blocks_an_early_op_in_the_same_turn() {
    let (mut f, mut b) = frontier();
    let close = operation("drop.trivial", Command::Step(Next::Keep, 0), &[], &[7]);
    assert_eq!(f.offer(2, empty(), close, &mut b).0, Offer::Logged);
    assert!(!f.eligible(2, &store(7, 2)));
    assert!(f.eligible(2, &store(8, 2)));
    assert!(b.committed.is_empty());
}

#[test]
fn every_unresolved_predecessor_and_not_just_the_head_counts() {
    let (mut f, _) = frontier();
    f.enqueue(3, Held::empty());
    // Start the next turn with 2 and 3 in the head's captured queue.
    f.queue.push_front(1);
    f.head = Some(0);
    f.eligible_turns = [(1, 0), (2, 0), (3, 0)].into();
    f.held.get_mut(&2).unwrap().objects.insert(object(7));
    assert!(!f.eligible(3, &store(7, 1)));
    assert!(f.eligible(3, &store(8, 1)));
}

#[test]
fn unknown_capture_or_type_metadata_is_a_conflict() {
    let (mut f, _) = frontier();
    f.held.get_mut(&1).unwrap().unknown = true;
    assert!(!f.eligible(2, &store(7, 1)));
    f.held.get_mut(&1).unwrap().unknown = false;
    f.held.get_mut(&1).unwrap().obtains.insert(10);
    assert!(!f.eligible(2, &store(7, 1)));
    f.carries.define(10, Default::default());
    assert!(f.eligible(2, &store(7, 1)));
}

#[test]
fn type_closure_covers_leftover_finalizers_and_task_set_take() {
    let (mut f, _) = frontier();
    f.held.get_mut(&1).unwrap().obtains.insert(10);
    f.carries.define(10, [11].into());
    f.carries.define(11, [12].into());
    f.carries.define(12, [10].into()); // recursive handle types terminate
    f.carries.register(12, object(7));
    assert!(!f.eligible(2, &store(7, 1)));
    assert!(f.eligible(2, &store(8, 1)));
}

#[test]
fn type_and_object_updates_preserve_a_cloned_frontiers_conflicts() {
    let (mut f, _) = frontier();
    f.held.get_mut(&1).unwrap().obtains.insert(10);
    f.carries.define(10, [11].into());
    let mut updated = f.carries.clone();
    updated.define(11, Default::default());
    assert!(!f.eligible(2, &store(7, 1)));
    std::mem::swap(&mut f.carries, &mut updated);
    assert!(f.eligible(2, &store(7, 1)));
    updated = f.carries.clone();
    f.carries.register(11, object(7));
    assert!(!f.eligible(2, &store(7, 1)));
    assert!(f.eligible(2, &store(8, 1)));
    f.carries = updated;
    assert!(f.eligible(2, &store(7, 1)));
}

#[test]
fn call_footprints_are_transitive_and_include_callers_remaining_members() {
    let (mut f, _) = frontier();
    f.enqueue(3, Held::empty());
    f.enqueue(4, Held::empty());
    f.enqueue(5, Held::empty());
    f.queue.retain(|&task| task < 3);
    f.held.get_mut(&1).unwrap().holding.extend([3, 5]);
    f.held.get_mut(&3).unwrap().holding.insert(4);
    f.held.get_mut(&4).unwrap().objects.insert(object(7));
    f.held.get_mut(&5).unwrap().objects.insert(object(8));
    assert!(!f.eligible(2, &store(7, 1)));
    f.head = Some(3);
    f.held.get_mut(&3).unwrap().caller = Some(1);
    assert!(!f.eligible(2, &store(8, 1)));
    assert!(f.eligible(2, &store(9, 1)));
}

#[test]
fn cancellation_credits_task_ops_and_thaw_never_commit_early() {
    let (f, _) = frontier();
    for name in [
        "task.cancel",
        "task.set_take",
        "drop.trivial",
        "drop.leftovers",
        "mutex.kill",
        "shared.thaw",
        "stop",
    ] {
        assert!(!f.eligible(2, &step(name, Next::Keep)), "{name}");
    }
    let mut injected = store(7, 1);
    injected.footprint.writes.insert(Item::Cancel(1));
    assert!(!f.eligible(2, &injected));
}

#[test]
fn a_zero_log_cap_parks_the_first_boundary_without_losing_it() {
    let (mut f, mut b) = frontier();
    f.policy.run_ahead = 0;
    f.policy.reorder = Reorder::Off;
    assert_eq!(f.offer(2, empty(), store(7, 1), &mut b).0, Offer::Pending);
    assert!(b.committed.is_empty());
    f.apply_head(empty(), step("task.check_canceled", Next::Park), &mut b)
        .unwrap();
    f.absorb(&mut b).unwrap();
    assert_eq!(b.values[&object(7)], 1);
}

#[test]
fn logged_turns_are_absorbed_in_program_order_before_new_head_work() {
    let (mut f, mut b) = frontier();
    f.policy.reorder = Reorder::Off;
    assert_eq!(f.offer(2, empty(), store(7, 1), &mut b).0, Offer::Logged);
    assert_eq!(f.offer(2, empty(), store(7, 2), &mut b).0, Offer::Logged);
    f.apply_head(empty(), step("task.check_canceled", Next::Yield), &mut b)
        .unwrap();
    f.absorb(&mut b).unwrap();
    assert_eq!(b.values[&object(7)], 1);
    assert_eq!(f.head(), Some(1));
    f.apply_head(empty(), load(7), &mut b).unwrap();
    assert_eq!(b.trace.last().unwrap().3, 1);
    f.absorb(&mut b).unwrap();
    assert_eq!(b.values[&object(7)], 2);
}

#[test]
fn a_logged_detach_all_calls_holding_members_in_order_before_its_later_log() {
    let (mut f, mut b) = frontier();
    for child in [3, 4] {
        f.enqueue(child, Held::empty());
        f.tasks.get_mut(&child).unwrap().state = State::Holding;
        f.held.get_mut(&2).unwrap().holding.insert(child);
    }
    f.queue.retain(|&task| task < 3);
    let detach = step(
        "task.detach_all",
        Next::Call {
            members: vec![3, 4],
            then_yield: true,
        },
    );
    assert_eq!(f.offer(2, empty(), detach, &mut b).0, Offer::Logged);
    assert_eq!(f.offer(2, empty(), store(7, 1), &mut b).0, Offer::Logged);
    f.apply_head(empty(), step("task.check_canceled", Next::Yield), &mut b)
        .unwrap();
    assert_eq!(f.absorb(&mut b).unwrap(), Progress::Called(3));
    assert_eq!(b.calls, [(2, 3)]);
    assert_eq!(f.offer(2, empty(), store(8, 2), &mut b).0, Offer::Logged);
    assert_eq!(
        f.state(2),
        State::Parked,
        "producing a log cannot end a call"
    );
    let polls = b.polls;
    assert_eq!(
        f.apply_head(empty(), step("task.end", Next::End), &mut b)
            .unwrap(),
        Progress::Called(4)
    );
    assert_eq!(b.calls, [(2, 3), (2, 4)]);
    assert_eq!(b.members, [(2, 3), (2, 4)]);
    assert!(b.returned_calls.is_empty());
    assert_eq!(b.polls, polls);
    assert_eq!(
        f.apply_head(empty(), step("task.end", Next::End), &mut b)
            .unwrap(),
        Progress::Retired
    );
    assert_eq!(f.head(), Some(1));
    assert_eq!(b.returned_calls, [2]);
    assert!(!b.values.contains_key(&object(7)));
    f.apply_head(empty(), step("task.check_canceled", Next::Yield), &mut b)
        .unwrap();
    f.absorb(&mut b).unwrap();
    assert_eq!(b.values[&object(7)], 1);
}

#[test]
fn credit_events_split_stretches_at_the_committing_op() {
    let (mut f, mut b) = frontier();
    f.policy.reorder = Reorder::Off;
    f.settled = Settled {
        level: 90,
        limit: 100,
        fuel: 1,
    };
    let close = operation("drop.trivial", Command::Step(Next::Keep, 80), &[], &[7]);
    f.offer(2, empty(), close, &mut b);
    f.offer(2, stretch(&[Event::Alloc(20)]), store(8, 1), &mut b);
    f.apply_head(empty(), step("task.check_canceled", Next::Park), &mut b)
        .unwrap();
    f.absorb(&mut b).unwrap();
    f.absorb(&mut b).unwrap();
    assert_eq!(f.settled.level, 30);
}

#[test]
fn speculative_budget_stops_expose_only_the_retired_prefix() {
    for (events, expected) in [
        (
            vec![Event::Alloc(20), Event::Fuel, Event::Fuel, Event::Free(20)],
            BudgetKind::Memory,
        ),
        (
            vec![Event::Fuel, Event::Fuel, Event::Alloc(20), Event::Free(20)],
            BudgetKind::Fuel,
        ),
    ] {
        let (mut f, mut b) = frontier();
        f.settled = Settled {
            level: 90,
            limit: 100,
            fuel: 1,
        };
        assert_eq!(
            f.offer(2, stretch(&events), store(7, 1), &mut b).0,
            Offer::Early
        );
        f.apply_head(empty(), step("task.check_canceled", Next::Park), &mut b)
            .unwrap();
        let prefix = b.trace.clone();
        assert_eq!(f.absorb(&mut b), Err(expected));
        assert_eq!(b.trace, prefix);
        assert!(b.published.is_empty());
        assert_eq!(f.absorb(&mut b), Ok(Progress::Stopped));
    }
}

#[test]
fn speculation_allowance_does_not_charge_or_block_the_head() {
    let (mut f, mut b) = frontier();
    f.policy.speculation_bytes = 16;
    f.settled.fuel = 10;
    assert_eq!(f.reserve(2, 16, 10), Ok(()));
    assert_eq!(f.reserve(2, 1, 10), Err(Pause::Allowance));
    assert_eq!(f.reserve(2, 0, 11), Err(Pause::Fuel));
    assert_eq!(f.reserve(1, 1024, 100), Ok(()));
    assert_eq!(f.settled.level, 0);
    f.free_speculative(2, 8);
    assert_eq!(f.reserve(2, 8, 10), Ok(()));
    f.offer(2, empty(), store(7, 1), &mut b);
    f.apply_head(empty(), step("task.check_canceled", Next::Park), &mut b)
        .unwrap();
    f.absorb(&mut b).unwrap();
    assert_eq!(f.physical, 0);
}

#[test]
fn a_full_command_keeps_its_open_allocation_reservation_after_the_last_logged_turn() {
    let (mut f, mut b) = frontier();
    f.policy.speculation_bytes = 16;
    b.deferred = true;
    f.enqueue(3, Held::empty());
    f.reserve(2, 8, 0).unwrap();
    assert_eq!(f.offer(2, empty(), store(7, 1), &mut b).0, Offer::Early);
    f.reserve(2, 8, 0).unwrap();
    // This may free older logged storage; it does not prove that any of the
    // eight bytes in the open stretch have died.
    f.credit_speculative(2, 4);
    assert_eq!(
        f.apply_head(empty(), step("task.check_canceled", Next::Park), &mut b),
        Ok(Progress::Poll)
    );
    let (offer, retained) = f.offer(2, empty(), store(8, 1), &mut b);
    assert_eq!(offer, Offer::Full);
    f.complete_poll(Vec::new());
    assert_eq!(f.absorb(&mut b), Ok(Progress::Poll));
    assert_eq!(f.physical, 8, "unsubmitted stretch still owns its storage");
    f.complete_poll(Vec::new());
    assert_eq!(f.head(), Some(3));
    assert_eq!(f.reserve(2, 9, 0), Err(Pause::Allowance));
    assert_eq!(
        f.apply_head(empty(), step("task.check_canceled", Next::Park), &mut b),
        Ok(Progress::Poll)
    );
    f.complete_poll(Vec::new());
    assert_eq!(f.head(), Some(2));
    let (before, operation) = retained.unwrap();
    assert_eq!(f.apply_head(before, operation, &mut b), Ok(Progress::Poll));
    assert_eq!(f.physical, 0);
}

#[test]
fn measured_losing_windows_fall_back_and_periodic_probes_recover() {
    let (mut f, _) = frontier();
    f.tune(AutoSample::default());
    assert!(!f.eligible(2, &store(7, 1)));
    f.probe();
    assert!(f.eligible(2, &store(7, 1)));
    f.tune(AutoSample {
        segment_ns: 100_000,
        probes: 100,
        eligible: 100,
        saved_ns: 100_000,
        overhead_ns: 100,
    });
    assert!(f.eligible(2, &store(7, 1)));
    f.policy.reorder = Reorder::Off;
    f.probe();
    assert!(!f.eligible(2, &store(7, 1)));
}

/// Deterministic perturbation of worker arrival order. The reference does no
/// off-frontier work; every tested width/mode has the same canonical trace.
#[test]
fn differential_width_jitter_reorder_matrix() {
    fn run(width: usize, seed: u64, reorder: Reorder) -> Vec<(u64, TaskId, &'static str, u64)> {
        let mut f = Frontier::new(
            0,
            Policy::admit(
                &Placement {
                    reorder: Some(reorder),
                    ..Placement::default()
                },
                None,
            ),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        let mut b = Objects::default();
        f.held.insert(0, Held::empty());
        let mut programs: BTreeMap<TaskId, VecDeque<Operation<Command>>> = BTreeMap::new();
        for task in 1..=8 {
            f.enqueue(
                task,
                Held {
                    objects: [object(task)].into(),
                    ..Held::empty()
                },
            );
            programs.insert(
                task,
                [
                    store(task, task * 3),
                    load(task),
                    step("task.end", Next::End),
                ]
                .into(),
            );
        }
        f.apply_head(empty(), step("task.end", Next::End), &mut b)
            .unwrap();
        let mut random = seed;
        while let Some(head) = f.head() {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut ready: Vec<_> = f.queue().collect();
            ready.sort_by_key(|&t| t.wrapping_mul(random).rotate_left((random % 63) as u32));
            for task in ready.into_iter().take(width - 1) {
                if !matches!(f.state(task), State::Pending) {
                    for _ in 0..(random % 4) {
                        let Some(op) = programs.get_mut(&task).unwrap().pop_front() else {
                            break;
                        };
                        let (offered, returned) = f.offer(task, empty(), op, &mut b);
                        if let Some((_, op)) = returned {
                            programs.get_mut(&task).unwrap().push_front(op);
                        }
                        if matches!(offered, Offer::Pending | Offer::Full) {
                            break;
                        }
                    }
                }
            }
            match f.absorb(&mut b).unwrap() {
                Progress::NeedSave(_) => panic!("the model backend owns no physical frames"),
                Progress::NeedSegment(task) => {
                    assert_eq!(task, head);
                    let op = programs.get_mut(&head).unwrap().pop_front().unwrap();
                    f.apply_head(empty(), op, &mut b).unwrap();
                }
                Progress::Idle | Progress::Stopped => break,
                _ => {}
            }
        }
        b.trace
    }
    let expected = run(1, 0, Reorder::Off);
    for width in [1, 2, 4, 8] {
        for seed in 0..32 {
            for reorder in [Reorder::Off, Reorder::Auto] {
                assert_eq!(
                    run(width, seed, reorder),
                    expected,
                    "{width} {seed} {reorder:?}"
                );
            }
        }
    }
}

#[test]
fn a_deferred_poll_selects_no_head_and_preserves_wake_before_requeue_order() {
    let (mut f, mut b) = frontier();
    b.deferred = true;
    assert_eq!(f.offer(2, empty(), store(7, 1), &mut b).0, Offer::Early);
    assert_eq!(
        f.apply_head(empty(), step("task.check_canceled", Next::Yield), &mut b),
        Ok(Progress::Poll)
    );
    assert_eq!(f.head(), None);
    assert_eq!(f.retired(), 1);
    assert_eq!(b.polls, 0);
    let committed = b.committed.len();
    assert_eq!(f.absorb(&mut b), Ok(Progress::Poll));
    let (offer, retained) = f.offer(2, empty(), store(8, 1), &mut b);
    assert_eq!(offer, Offer::Full);
    assert!(retained.is_some());
    assert_eq!(b.committed.len(), committed);
    assert_eq!(f.complete_poll(vec![0]), Progress::Retired);
    assert_eq!(f.head(), Some(2));
    assert_eq!(f.queue().collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(f.absorb(&mut b), Ok(Progress::Poll));
    assert_eq!(b.published, vec![2], "publish precedes the deferred poll");
    assert_eq!(b.polls, 0);
    assert_eq!(f.complete_poll(vec![]), Progress::Retired);
    assert_eq!(f.head(), Some(0));
    assert_eq!(f.queue().collect::<Vec<_>>(), vec![1, 2]);
}

#[test]
fn a_pending_keep_can_resume_its_saved_frame_after_absorption() {
    let (mut f, mut b) = frontier();
    assert_eq!(
        f.offer(2, empty(), step("shared.thaw", Next::Keep), &mut b)
            .0,
        Offer::Pending
    );
    f.apply_head(empty(), step("task.check_canceled", Next::Park), &mut b)
        .unwrap();
    assert_eq!(f.head(), Some(2));
    assert_eq!(f.absorb(&mut b), Ok(Progress::Absorbed(2)));
    f.start(2, 3);
    assert_eq!(f.state(2), State::Running(3));
}

#[test]
fn retirement_metadata_keeps_early_stretches_and_positional_credits() {
    let (mut f, mut b) = frontier();
    let before = stretch(&[Event::Alloc(24), Event::Fuel, Event::Free(8)]);
    let prior = b.retirements.len();
    assert_eq!(
        f.offer(2, before.clone(), store(7, 9), &mut b).0,
        Offer::Early
    );
    assert_eq!(
        b.retirements.len(),
        prior,
        "commit must not emit a retirement"
    );
    f.apply_head(empty(), step("task.check_canceled", Next::Park), &mut b)
        .unwrap();
    f.absorb(&mut b).unwrap();
    assert_eq!(
        b.retirements.last().unwrap(),
        &(2, false, 2, before, Next::Yield, 0)
    );
    let credit = operation("root.release", Command::Step(Next::Keep, 16), &[], &[]);
    f.apply_head(empty(), credit, &mut b).unwrap();
    assert_eq!(b.retirements.last().unwrap().5, 16);
    assert_eq!(f.settled.level, 0);
}

#[test]
fn waiting_continuations_record_their_stretch_without_inventing_an_op() {
    let (mut f, mut b) = frontier();
    f.apply_head(empty(), step("host.request", Next::Keep), &mut b)
        .unwrap();
    let operations = b.trace.len();
    let before = stretch(&[Event::Fuel, Event::Alloc(8)]);
    f.continue_head(before.clone(), &mut b, || Next::Yield)
        .unwrap();
    assert_eq!(b.trace.len(), operations);
    assert_eq!(
        b.retirements.last().unwrap(),
        &(1, true, 1, before, Next::Yield, 0)
    );
}

#[test]
fn a_budget_stop_does_not_publish_metadata_for_the_rejected_operation() {
    let (mut f, mut b) = frontier();
    f.settled.limit = 8;
    let prior = (b.trace.len(), b.retirements.len());
    assert_eq!(
        f.apply_head(stretch(&[Event::Alloc(16)]), store(7, 9), &mut b),
        Err(BudgetKind::Memory)
    );
    assert_eq!((b.trace.len(), b.retirements.len()), prior);
    assert!(b.values.is_empty());
}

#[test]
fn a_physical_save_wait_retains_the_exact_operation_and_stretch_without_polling() {
    let policy = Policy::admit(&Default::default(), None);
    let settled = Settled {
        level: 0,
        limit: 100,
        fuel: 100,
    };
    let mut frontier = Frontier::new(0, policy, settled);
    frontier.held.insert(0, Held::empty());
    assert!(frontier.head_ready(0));
    let mut backend = Objects {
        block_retirement: true,
        ..Default::default()
    };
    let mut before = Stretch::new(Budgets {
        fuel: true,
        memory: true,
    });
    before.fuel_points(7);
    before.alloc(12).unwrap();
    let op = Operation {
        descriptor: op_descriptor("task.async").unwrap(),
        footprint: Default::default(),
        payload: Command::Step(Next::Yield, 0),
    };
    assert_eq!(
        frontier.apply_head(before, op, &mut backend),
        Ok(Progress::NeedSave(0))
    );
    assert!(!frontier.head_ready(0));
    assert_eq!(frontier.settled.fuel, 100);
    assert_eq!(frontier.settled.level, 0);
    assert_eq!(frontier.retired(), 0);
    assert!(backend.committed.is_empty());
    assert_eq!(backend.polls, 0);
    assert_eq!(frontier.absorb(&mut backend), Ok(Progress::NeedSave(0)));
    assert!(backend.trace.is_empty());
    assert!(!frontier.head_ready(0));
    backend.block_retirement = false;
    assert_eq!(frontier.absorb(&mut backend), Ok(Progress::Retired));
    assert_eq!(frontier.settled.fuel, 93);
    assert_eq!(frontier.settled.level, 12);
    assert_eq!(frontier.retired(), 1);
    assert_eq!(backend.committed.len(), 1);
    assert_eq!(backend.trace.len(), 1);
    assert_eq!(backend.polls, 1);
    assert!(frontier.head_ready(0));
}
