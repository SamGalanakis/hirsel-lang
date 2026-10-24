//! Runtime r6 Ordered acceptance model (runtime-ordered-50). A deterministic
//! object world drives the production [`Frontier`] at width 1 and at jittered
//! widths. The width-1 strict cell is the serial reference: every other cell
//! must equal it once physical fields (early flags, worker choice, publication
//! order) are removed, and each object's commit history must stay a prefix of
//! the unstopped serial history, including early commits a stop later hides.

use super::*;
use crate::budget::stretch::{self, Event};
use crate::budget::stretch_trace::{Charge, Observer};
use footprint::{Item, ObjectId};
use hirsel_rt_operations::descriptors::op_descriptor;
use std::collections::BTreeSet;

/// One language operation of a model task. Waiting operations that park are
/// reissued by the world, as a native frame retries its saved operation.
#[derive(Clone, Debug)]
pub(crate) enum Act {
    Store(u64, u64),
    Load(u64),
    Take(u64),
    Put(u64, u64),
    Send(u64, u64),
    Receive(u64),
    Thaw(u64),
    /// A host request whose completion becomes logical at this poll count.
    Host(u64),
    Cancel(TaskId),
    /// A failing operation; an unstopped reference treats it as a yield.
    Fail,
    Step(&'static str, Next, u64),
}

impl Act {
    fn name(&self) -> &'static str {
        match self {
            Act::Store(..) => "atomic.store",
            Act::Load(_) => "atomic.load",
            Act::Take(_) => "mutex.take",
            Act::Put(..) => "mutex.put",
            Act::Send(..) => "channel.send",
            Act::Receive(_) => "channel.receive",
            Act::Thaw(_) => "shared.thaw",
            Act::Host(_) => "host.request",
            Act::Cancel(_) => "task.cancel",
            Act::Fail => "stop",
            Act::Step(name, ..) => name,
        }
    }

    fn object(&self) -> Option<u64> {
        match *self {
            Act::Store(n, _)
            | Act::Load(n)
            | Act::Take(n)
            | Act::Put(n, _)
            | Act::Send(n, _)
            | Act::Receive(n)
            | Act::Thaw(n) => Some(n),
            _ => None,
        }
    }

    fn footprint(&self) -> Footprint {
        let mut footprint = Footprint::default();
        match self {
            Act::Load(n) => {
                footprint.reads.insert(Item::Object(object(*n)));
            }
            Act::Host(_) => {
                footprint.writes.insert(Item::Host);
            }
            Act::Cancel(task) => {
                footprint.writes.insert(Item::Cancel(*task));
            }
            act => {
                if let Some(n) = act.object() {
                    footprint.writes.insert(Item::Object(object(n)));
                }
            }
        }
        footprint
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Op {
    pub(crate) act: Act,
    pub(crate) charges: Vec<Event>,
}

pub(crate) fn op(act: Act) -> Op {
    Op {
        act,
        charges: Vec::new(),
    }
}

pub(crate) fn charged(act: Act, charges: &[Event]) -> Op {
    Op {
        act,
        charges: charges.to_vec(),
    }
}

pub(crate) fn object(n: u64) -> ObjectId {
    ObjectId {
        creator: vec![0],
        ordinal: n,
    }
}

pub(crate) fn yields() -> Act {
    Act::Step("task.check_canceled", Next::Yield, 0)
}

pub(crate) fn ends() -> Act {
    Act::Step("task.end", Next::End, 0)
}

pub(crate) fn holds() -> Act {
    Act::Step("task.end", Next::Hold, 0)
}

fn operation(act: Act) -> Operation<Act> {
    Operation {
        descriptor: op_descriptor(act.name()).expect("a model operation descriptor"),
        footprint: act.footprint(),
        payload: act,
    }
}

#[derive(Clone)]
pub(crate) struct Scenario {
    /// Root (task 0) first, then every task in its initial queue position.
    pub(crate) programs: Vec<(TaskId, Vec<Op>)>,
    pub(crate) holding: Vec<(TaskId, Vec<TaskId>)>,
    pub(crate) settled: Settled,
    pub(crate) budgets: stretch::Budgets,
    pub(crate) failures: bool,
}

impl Scenario {
    pub(crate) fn new(programs: Vec<(TaskId, Vec<Op>)>) -> Self {
        Self {
            programs,
            holding: Vec::new(),
            settled: Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
            budgets: stretch::Budgets {
                fuel: false,
                memory: false,
            },
            failures: false,
        }
    }

    /// The internal history an early commit may extend: no budget, no failure.
    fn unstopped(&self) -> Self {
        Self {
            settled: Settled {
                level: self.settled.level,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
            failures: false,
            ..self.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Wait {
    Receive(u64),
    Host(u64),
}

/// One retired operation or waiting continuation, without physical fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Row {
    pub(crate) index: u64,
    pub(crate) task: TaskId,
    pub(crate) op: &'static str,
    pub(crate) result: u64,
    pub(crate) next: Next,
    pub(crate) credit: u64,
}

type History = BTreeMap<u64, Vec<(TaskId, &'static str, u64)>>;

#[derive(Default)]
struct World {
    failures: bool,
    atomics: BTreeMap<u64, u64>,
    taken: BTreeSet<u64>,
    mutexes: BTreeMap<u64, u64>,
    channels: BTreeMap<u64, VecDeque<u64>>,
    pending: BTreeMap<TaskId, VecDeque<(u64, u64)>>,
    parked: Vec<(TaskId, Wait)>,
    canceled: BTreeSet<TaskId>,
    completed: BTreeSet<TaskId>,
    polls: u64,
    reissue: Vec<(TaskId, Act)>,
    rows: Vec<Row>,
    wakes: Vec<(u64, Vec<TaskId>)>,
    history: History,
    reference: Option<History>,
    early: Vec<(TaskId, &'static str)>,
    published: Vec<TaskId>,
    calls: Vec<(TaskId, TaskId)>,
    members: Vec<(TaskId, TaskId)>,
    returned: Vec<TaskId>,
    stop: Option<(TaskId, u64, BudgetKind)>,
    stream: Vec<Charge>,
    failing: Vec<Charge>,
    observer: Option<Observer>,
}

impl World {
    fn touch(&mut self, n: u64, task: TaskId, name: &'static str, value: u64) {
        let history = self.history.entry(n).or_default();
        history.push((task, name, value));
        if let Some(reference) = &self.reference {
            let serial = reference.get(&n).map_or(&[][..], |events| &events[..]);
            assert!(
                serial.starts_with(history),
                "object {n} history {history:?} left the serial prefix {serial:?}"
            );
        }
    }

    fn park(&mut self, task: TaskId, wait: Wait, act: Act) -> (u64, Next, u64) {
        self.parked.push((task, wait));
        self.reissue.push((task, act));
        (0, Next::Park, 0)
    }

    fn idle(&mut self) -> Vec<TaskId> {
        let mut woken = Vec::new();
        self.parked.retain(|&(task, wait)| {
            let host = matches!(wait, Wait::Host(_));
            if host {
                woken.push(task);
            }
            !host
        });
        self.completed.extend(woken.iter().copied());
        woken
    }
}

impl Backend<Act> for World {
    type Result = u64;

    fn can_commit_early(&mut self, op: &Operation<Act>) -> bool {
        match op.payload {
            Act::Take(m) => !self.taken.contains(&m),
            _ => true,
        }
    }

    fn commit(&mut self, task: TaskId, op: &Operation<Act>, early: bool) -> Receipt<u64> {
        let name = op.descriptor.name;
        let (result, next, credit) = match op.payload.clone() {
            Act::Store(n, value) => {
                self.atomics.insert(n, value);
                self.touch(n, task, name, value);
                (0, Next::Yield, 0)
            }
            Act::Load(n) => {
                let value = self.atomics.get(&n).copied().unwrap_or(0);
                self.touch(n, task, name, value);
                (value, Next::Yield, 0)
            }
            Act::Take(m) => {
                // As production (sync.rs take): an Ordered mutex body holds no
                // scheduling point, so a take never finds its mutex held.
                assert!(
                    self.taken.insert(m),
                    "a mutex acquired while a body holds it"
                );
                let value = self.mutexes.get(&m).copied().unwrap_or(0);
                self.touch(m, task, name, value);
                (value, Next::Keep, 0)
            }
            Act::Put(m, value) => {
                assert!(self.taken.remove(&m), "put of an untaken mutex {m}");
                self.mutexes.insert(m, value);
                self.touch(m, task, name, value);
                (0, Next::Yield, 0)
            }
            Act::Send(c, value) => {
                self.touch(c, task, name, value);
                if early {
                    // Rule 4: invisible to every receive poll until retirement.
                    self.pending.entry(task).or_default().push_back((c, value));
                } else {
                    self.channels.entry(c).or_default().push_back(value);
                }
                (0, Next::Yield, 0)
            }
            Act::Receive(c) => match self.channels.entry(c).or_default().pop_front() {
                Some(value) => {
                    self.touch(c, task, name, value);
                    (value, Next::Yield, 0)
                }
                None => self.park(task, Wait::Receive(c), Act::Receive(c)),
            },
            Act::Thaw(n) => {
                assert!(!early, "thaw is outside E");
                self.touch(n, task, name, 1);
                (1, Next::Keep, 0)
            }
            Act::Host(at) => {
                assert!(!early, "host requests are outside E");
                if self.canceled.contains(&task) {
                    (2, Next::Keep, 0)
                } else if self.polls >= at || self.completed.remove(&task) {
                    (1, Next::Keep, 0)
                } else {
                    self.park(task, Wait::Host(at), Act::Host(at))
                }
            }
            Act::Cancel(target) => {
                assert!(!early, "cancellation is outside E");
                self.canceled.insert(target);
                (0, Next::Yield, 0)
            }
            Act::Fail => (
                0,
                if self.failures {
                    Next::Stop
                } else {
                    Next::Yield
                },
                0,
            ),
            Act::Step(_, next, credit) => (0, next, credit),
        };
        if early {
            self.early.push((task, name));
        }
        Receipt {
            result,
            next,
            credit,
        }
    }

    fn record(&mut self, _: TaskId, _: &Operation<Act>, _: &u64, _: u64) {
        unreachable!("the model records complete retirement metadata")
    }

    fn record_retirement(
        &mut self,
        task: TaskId,
        op: &Operation<Act>,
        result: &u64,
        retirement: Retirement<'_>,
    ) {
        self.rows.push(Row {
            index: retirement.index,
            task,
            op: op.descriptor.name,
            result: *result,
            next: retirement.transition.clone(),
            credit: retirement.credit,
        });
        self.stream.extend(retirement.before.exact.iter().copied());
        if retirement.credit != 0 {
            self.stream.push(Charge::Credit(retirement.credit));
        }
        if let Some(observer) = &mut self.observer {
            assert_eq!(observer.settle(task, retirement.before), Ok(()));
            observer.credit(retirement.credit);
        }
    }

    fn record_budget_stop(
        &mut self,
        task: TaskId,
        before: &Stretch,
        retirement: u64,
        kind: BudgetKind,
    ) {
        assert!(self.stop.is_none(), "one first stop");
        self.stop = Some((task, retirement, kind));
        self.failing = before.exact.clone();
        if let Some(observer) = &mut self.observer {
            assert_eq!(observer.settle(task, before), Err(kind));
        }
    }

    fn publish(&mut self, task: TaskId, op: &Operation<Act>, _: &u64) {
        if let Act::Send(..) = op.payload {
            let (c, value) = self
                .pending
                .get_mut(&task)
                .and_then(VecDeque::pop_front)
                .expect("an early send owns its pending message");
            self.channels.entry(c).or_default().push_back(value);
        }
        self.published.push(task);
    }

    fn call(&mut self, caller: TaskId, child: TaskId) {
        self.calls.push((caller, child));
    }

    fn call_member(&mut self, caller: TaskId, child: TaskId) {
        self.members.push((caller, child));
    }

    fn call_returned(&mut self, caller: TaskId) {
        self.returned.push(caller);
    }

    fn poll(&mut self) -> Vec<TaskId> {
        self.polls += 1;
        let mut woken = Vec::new();
        let parked = std::mem::take(&mut self.parked);
        for (task, wait) in parked {
            let ready = match wait {
                Wait::Receive(c) => self.channels.get(&c).is_some_and(|q| !q.is_empty()),
                Wait::Host(at) => self.polls >= at || self.canceled.contains(&task),
            };
            if ready {
                woken.push(task);
            } else {
                self.parked.push((task, wait));
            }
        }
        self.wakes.push((self.polls, woken.clone()));
        woken
    }
}

/// The canonical projection of one cell: everything but physical choices.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Canonical {
    pub(crate) rows: Vec<Row>,
    pub(crate) wakes: Vec<(u64, Vec<TaskId>)>,
    pub(crate) idle: Vec<Vec<TaskId>>,
    pub(crate) queues: Vec<(Option<TaskId>, Vec<TaskId>)>,
    pub(crate) calls: Vec<(TaskId, TaskId)>,
    pub(crate) members: Vec<(TaskId, TaskId)>,
    pub(crate) returned: Vec<TaskId>,
    pub(crate) stop: Option<(TaskId, u64, BudgetKind)>,
    pub(crate) stopped: bool,
    pub(crate) parked: Vec<TaskId>,
    pub(crate) exact: BTreeMap<TaskId, Vec<Charge>>,
    pub(crate) stream: Vec<Charge>,
    pub(crate) failing: Vec<Charge>,
    pub(crate) settled: Settled,
    /// Final object state, only when no stop hides younger early commits.
    pub(crate) objects: Option<(
        BTreeMap<u64, u64>,
        BTreeMap<u64, Vec<u64>>,
        BTreeMap<u64, u64>,
    )>,
}

pub(crate) struct Cell {
    pub(crate) canonical: Canonical,
    pub(crate) early: Vec<(TaskId, &'static str)>,
    pub(crate) published: Vec<TaskId>,
    pub(crate) history: History,
}

fn lcg(state: u64) -> u64 {
    state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
}

fn held(program: &[Op]) -> Held {
    Held {
        objects: program
            .iter()
            .filter_map(|op| op.act.object().map(object))
            .collect(),
        ..Held::empty()
    }
}

/// Run one cell. Width 1 offers nothing ahead; wider cells let up to
/// `width - 1` queued tasks offer a seeded number of operations per head step.
pub(crate) fn cell(
    scenario: &Scenario,
    width: usize,
    seed: u64,
    reorder: Reorder,
    reference: Option<&History>,
) -> Cell {
    let mut programs: BTreeMap<TaskId, VecDeque<Op>> = scenario
        .programs
        .iter()
        .map(|(task, program)| (*task, program.iter().cloned().collect()))
        .collect();
    let (root, root_program) = &scenario.programs[0];
    assert_eq!(*root, 0);
    let mut f: Frontier<Act, u64> = Frontier::new(
        0,
        Policy::admit(
            &Placement {
                reorder: Some(reorder),
                ..Placement::default()
            },
            None,
        ),
        scenario.settled,
    );
    f.held.insert(0, held(root_program));
    for (task, program) in &scenario.programs[1..] {
        f.enqueue(*task, held(program));
    }
    for (parent, children) in &scenario.holding {
        f.held
            .get_mut(parent)
            .unwrap()
            .holding
            .extend(children.iter().copied());
    }
    let mut w = World {
        failures: scenario.failures,
        reference: reference.cloned(),
        observer: Some(Observer::new(scenario.settled, scenario.budgets)),
        ..World::default()
    };
    let summary = |op: &Op| stretch::summarise(scenario.budgets, &op.charges).unwrap();
    let mut queues = Vec::new();
    let mut idle = Vec::new();
    let mut random = lcg(seed.wrapping_add(0x5eed));
    let mut steps = 0u32;
    let mut suspended = BTreeSet::new();
    loop {
        steps += 1;
        assert!(steps < 200_000, "model livelock");
        if width > 1 && !f.is_stopped() {
            random = lcg(random);
            let mut ready: Vec<_> = f.queue().collect();
            ready.sort_by_key(|&t| (t ^ random).rotate_left((random % 63) as u32));
            for task in ready.into_iter().take(width - 1) {
                // A worker that offered a pending (or refused) operation stays
                // suspended until the head absorbs its log, even after an
                // earlier logged turn retires and the task is logically queued.
                if suspended.contains(&task) {
                    if !f.tasks[&task].log.is_empty() {
                        continue;
                    }
                    suspended.remove(&task);
                }
                random = lcg(random);
                for _ in 0..(random >> 33) % 4 {
                    let Some(next) = programs.get_mut(&task).unwrap().pop_front() else {
                        break;
                    };
                    let before = summary(&next);
                    let (offered, returned) =
                        f.offer(task, before, operation(next.act.clone()), &mut w);
                    if returned.is_some() {
                        programs.get_mut(&task).unwrap().push_front(next);
                    }
                    if matches!(offered, Offer::Pending | Offer::Full | Offer::Stopped) {
                        suspended.insert(task);
                        break;
                    }
                }
            }
        }
        let progress = match f.absorb(&mut w) {
            Ok(Progress::NeedSegment(task)) => {
                let next = programs
                    .get_mut(&task)
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| panic!("head {task} has no remaining operation"));
                f.apply_head(summary(&next), operation(next.act), &mut w)
            }
            other => other,
        };
        for (task, act) in std::mem::take(&mut w.reissue) {
            programs.get_mut(&task).unwrap().push_front(op(act));
        }
        match progress {
            Ok(Progress::Retired) => queues.push((f.head(), f.queue().collect())),
            Ok(Progress::Idle) => {
                let woken = w.idle();
                if woken.is_empty() {
                    break;
                }
                idle.push(woken.clone());
                f.resume_idle(woken);
                queues.push((f.head(), f.queue().collect()));
            }
            Ok(Progress::Stopped) | Err(_) => break,
            Ok(Progress::NeedSave(_) | Progress::Poll) => {
                panic!("the model owns no physical frames and polls inline")
            }
            Ok(_) => {}
        }
    }
    let stopped = f.is_stopped();
    if stopped {
        // The terminal prefix is closed: an offered operation stays unowned.
        let queued = f.queue().next();
        if let Some(task) = queued {
            let (offered, returned) = f.offer(
                task,
                Stretch::new(scenario.budgets),
                operation(yields()),
                &mut w,
            );
            assert_eq!(offered, Offer::Stopped);
            assert!(returned.is_some());
        }
    }
    let mut observer = w.observer.take().unwrap();
    let exact = scenario
        .programs
        .iter()
        .map(|(task, _)| (*task, observer.take(*task).exact))
        .filter(|(_, exact)| !exact.is_empty())
        .collect();
    let objects = (!stopped).then(|| {
        (
            w.atomics.clone(),
            w.channels
                .iter()
                .map(|(c, q)| (*c, q.iter().copied().collect()))
                .collect(),
            w.mutexes.clone(),
        )
    });
    Cell {
        canonical: Canonical {
            rows: w.rows,
            wakes: w.wakes,
            idle,
            queues,
            calls: w.calls,
            members: w.members,
            returned: w.returned,
            stop: w.stop,
            stopped,
            parked: w.parked.iter().map(|(task, _)| *task).collect(),
            exact,
            stream: w.stream,
            failing: w.failing,
            settled: f.settled,
            objects,
        },
        early: w.early,
        published: w.published,
        history: w.history,
    }
}

pub(crate) struct Matrix {
    pub(crate) serial: Cell,
    pub(crate) unstopped: Cell,
    /// Early commits observed per (width, reorder auto) over every seed.
    pub(crate) early: BTreeMap<(usize, bool), usize>,
    pub(crate) cells: usize,
}

pub(crate) const WIDTHS: [usize; 4] = [1, 2, 4, 8];

/// Every width/seed/reorder cell equals the strict width-1 serial cell, and
/// every object history stays a prefix of the unstopped serial history.
pub(crate) fn run_matrix(scenario: &Scenario, seeds: u64) -> Matrix {
    let unstopped = cell(&scenario.unstopped(), 1, 0, Reorder::Off, None);
    assert!(unstopped.early.is_empty());
    let serial = cell(scenario, 1, 0, Reorder::Off, Some(&unstopped.history));
    assert!(serial.early.is_empty());
    assert!(serial.published.is_empty());
    let mut early = BTreeMap::new();
    let mut cells = 0;
    for width in WIDTHS {
        for seed in 0..seeds {
            for reorder in [Reorder::Off, Reorder::Auto] {
                let observed = cell(scenario, width, seed, reorder, Some(&unstopped.history));
                cells += 1;
                assert_eq!(
                    observed.canonical, serial.canonical,
                    "width {width} seed {seed} {reorder:?}"
                );
                if reorder == Reorder::Off || width == 1 {
                    assert!(observed.early.is_empty(), "{width} {seed} {reorder:?}");
                }
                // Publication is retirement: every published task has a
                // retired row, and no early op outlives its terminal prefix.
                for task in &observed.published {
                    assert!(
                        observed.canonical.rows.iter().any(|row| row.task == *task),
                        "published task {task} has no retired row"
                    );
                }
                *early.entry((width, reorder == Reorder::Auto)).or_insert(0) +=
                    observed.early.len();
            }
        }
    }
    Matrix {
        serial,
        unstopped,
        early,
        cells,
    }
}

/// The exact serial reference meter over the retired charge stream: the
/// retired prefix succeeds, and the failing stretch stops with the same kind.
pub(crate) fn reference_stop(scenario: &Scenario, canonical: &Canonical) -> Option<BudgetKind> {
    let events = |charges: &[Charge]| {
        charges
            .iter()
            .flat_map(|charge| match *charge {
                Charge::Fuel(points) => vec![Event::Fuel; points as usize],
                Charge::Alloc(bytes) => vec![Event::Alloc(bytes)],
                Charge::Free(bytes) => vec![Event::Free(bytes)],
                Charge::Credit(bytes) => vec![Event::Credit(bytes)],
            })
            .collect::<Vec<_>>()
    };
    let mut state = scenario.settled;
    assert_eq!(
        stretch::reference(&mut state, &events(&canonical.stream)),
        Ok(())
    );
    canonical.stop.as_ref()?;
    stretch::reference(&mut state, &events(&canonical.failing)).err()
}

/// Seeded mixed programs: shared and private atomics, mutex bodies, channels,
/// scheduling points, credits and optional exact charges.
pub(crate) fn generated(seed: u64, charged_ops: bool) -> Scenario {
    let mut random = lcg(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut draw = |n: u64| {
        random = lcg(random);
        (random >> 33) % n
    };
    let tasks = 3 + draw(4);
    let mut programs = vec![(0, vec![op(yields()), op(ends())])];
    for task in 1..=tasks {
        let mut program = Vec::new();
        for _ in 0..2 + draw(5) {
            let mut acts = match draw(9) {
                0 => vec![Act::Store(1 + draw(3), draw(100))],
                1 => vec![Act::Load(1 + draw(3))],
                2 => vec![Act::Store(100 + task, draw(100)), Act::Load(100 + task)],
                3 => {
                    let m = 20 + draw(2);
                    vec![Act::Take(m), Act::Thaw(40 + m), Act::Put(m, draw(100))]
                }
                4 => vec![Act::Send(30 + draw(2), draw(100))],
                5 if task % 2 == 0 => vec![Act::Receive(30 + draw(2))],
                6 => vec![Act::Step("drop.trivial", Next::Keep, draw(3) * 8)],
                _ => vec![yields()],
            };
            for act in acts.drain(..) {
                let mut charges = Vec::new();
                let mut live = 0;
                for _ in 0..if charged_ops { draw(4) } else { 0 } {
                    charges.push(match draw(4) {
                        0 | 1 => Event::Fuel,
                        2 => {
                            let bytes = 8 + draw(24);
                            live += bytes;
                            Event::Alloc(bytes)
                        }
                        _ => {
                            let bytes = draw(live + 1);
                            live -= bytes;
                            Event::Free(bytes)
                        }
                    });
                }
                program.push(Op { act, charges });
            }
        }
        program.push(op(ends()));
        programs.push((task, program));
    }
    Scenario::new(programs)
}
