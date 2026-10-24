//! r6 S10: a deterministic driver over the production Free transitions.
//!
//! One thread fires one transition per step, chosen by a seeded controller:
//! a worker's dispatcher search (`next`), a running segment's operation, its
//! idle save (`Running::save` then `publish`), a host claim or delayed
//! completion, a per-domain virtual-clock step, a cut, a stop, a permit or
//! width change, or an injected cancellation, spurious or stale wake. The
//! queues, frame tokens, permits, claim words, section gate, mutex permit,
//! cancellation flags, pending releases and virtual timers are the runtime's
//! own; the driver only decides which transition fires and checks custody
//! after each one. It adds no production scheduling point.
//!
//! Prior art: FDB runs production actors on a seeded single-thread scheduler
//! (fdbrpc/sim2.cpp:1588-1616); MadSim checks a repeated seed's RNG
//! transcript (madsim/src/sim/runtime/mod.rs:178-200); Shuttle replays the
//! recorded schedule and fails at the first decision that differs
//! (shuttle-schedulers/src/uncontrolled_nondeterminism.rs:61-97). The
//! transcript here holds every RNG draw, every runnable set with its selected
//! member, and every yield site. This proves neither weak memory
//! (proofs/free, `models`) nor the native switch ABI (`progress`).
use super::super::dispatch_tests::{Topology, topology};
use super::super::*;
use crate::readiness::{Readiness, WaitSource};
use hirsel_free::cancel::Flag;
use hirsel_free::claim::{Arbiter, Claim, Request, Requests};
use hirsel_free::mutex::{Acquired, Permit as ObjectPermit};
use hirsel_free::pending::Pending;
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::section::{Cut, Section, TaskLog};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

mod models;
mod progress;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Entry {
    Random {
        site: &'static str,
        bound: u64,
        value: u64,
    },
    Decision {
        site: &'static str,
        runnable: Vec<u32>,
        selected: Option<u32>,
    },
    Yield {
        task: u32,
        generation: u32,
        op: usize,
        site: &'static str,
    },
}

#[derive(Debug)]
pub(super) struct Divergence {
    pub(super) index: usize,
    pub(super) expected: Option<Entry>,
    pub(super) actual: Entry,
}

/// Seeded choices and their checked transcript. With `expected`, the first
/// entry that differs is retained and the run ends there.
pub(super) struct Controller {
    rng: u64,
    transcript: Vec<Entry>,
    expected: Option<Vec<Entry>>,
    divergence: Option<Divergence>,
    /// Mutation only: process state outside the seed, read at every choice.
    uncontrolled: Option<u64>,
    first_uncontrolled: Option<usize>,
}

impl Controller {
    pub(super) fn new(
        seed: u64,
        expected: Option<Vec<Entry>>,
        uncontrolled: Option<&AtomicU64>,
    ) -> Self {
        Self {
            rng: seed ^ 0x5851_f42d_4c95_7f2d,
            transcript: Vec::new(),
            expected,
            divergence: None,
            uncontrolled: uncontrolled.map(|clock| clock.fetch_add(1, Ordering::Relaxed)),
            first_uncontrolled: None,
        }
    }

    fn draw(&mut self) -> u64 {
        // SplitMix64, as MadSim's GlobalRng seeds its generator.
        self.rng = self.rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn push(&mut self, entry: Entry) {
        if self.divergence.is_none()
            && let Some(expected) = &self.expected
            && expected.get(self.transcript.len()) != Some(&entry)
        {
            self.divergence = Some(Divergence {
                index: self.transcript.len(),
                expected: expected.get(self.transcript.len()).cloned(),
                actual: entry.clone(),
            });
        }
        self.transcript.push(entry);
    }

    pub(super) fn diverged(&self) -> bool {
        self.divergence.is_some()
    }

    pub(super) fn random(&mut self, site: &'static str, bound: u64) -> u64 {
        let value = self.draw() % bound;
        self.push(Entry::Random { site, bound, value });
        value
    }

    /// Choose one member of a canonical runnable set.
    pub(super) fn decide(&mut self, site: &'static str, mut runnable: Vec<u32>) -> Option<u32> {
        runnable.sort_unstable();
        if let Some(epoch) = self.uncontrolled
            && runnable.len() > 1
        {
            // Like iterating a randomly keyed map: an order the seed never chose.
            let by = (epoch % runnable.len() as u64) as usize;
            runnable.rotate_left(by);
            self.first_uncontrolled.get_or_insert(self.transcript.len());
        }
        let selected = (!runnable.is_empty())
            .then(|| runnable[(self.draw() % runnable.len() as u64) as usize]);
        self.push(Entry::Decision {
            site,
            runnable,
            selected,
        });
        selected
    }

    /// Record a choice the production code made (the dispatcher's selection).
    pub(super) fn observe(
        &mut self,
        site: &'static str,
        mut runnable: Vec<u32>,
        selected: Option<u32>,
    ) {
        runnable.sort_unstable();
        self.push(Entry::Decision {
            site,
            runnable,
            selected,
        });
    }
}

const WIDTH: u16 = 3;
const DOMAINS: usize = 2;
const STEPS: u64 = 20_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Compute,
    Yield,
    Lock,
    Unlock,
    Host { shielded: bool },
    Sleep { domain: usize, after: u64 },
    Dup,
    Release,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Custody {
    Queued,
    Running(usize),
    Parked,
    Done,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Wait {
    Mutex,
    Host(u64),
    Timer { domain: usize, token: u64, at: u64 },
}

enum Mail {
    /// A host completion: an initialized handle, or a failed call.
    Result(Option<Handle>),
    Canceled,
    Due,
}

/// An initialized host result or a request's owned buffer. Drop is its only close.
struct Handle {
    id: u64,
    drops: Arc<Mutex<BTreeMap<u64, u32>>>,
}
impl Drop for Handle {
    fn drop(&mut self) {
        *self.drops.lock().unwrap().entry(self.id).or_default() += 1;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    Posted,
    /// Delayed: the home polls it `remaining` more times before it completes.
    Claimed {
        remaining: u64,
        fail: bool,
    },
    Returned,
}

struct Job {
    request: Arc<Request<Handle>>,
    slot: u32,
    generation: u32,
    shielded: bool,
    stage: Stage,
}

struct Task {
    slot: u32,
    generation: u32,
    frame: Arc<Frame>,
    script: Vec<Op>,
    pc: usize,
    custody: Custody,
    /// A wake that found the segment running, before its idle save.
    remembered: bool,
    log: Arc<TaskLog>,
    cancel: Flag,
    wait: Option<Wait>,
    job: Option<u64>,
    mail: Option<Mail>,
    mutex_queued: bool,
    pending: Pending<usize>,
    handles: Vec<Handle>,
    reuse: u32,
    /// The reused row's next generation and its first queue token.
    successor: Option<(Box<Task>, Queued<Image>)>,
}

struct Slot {
    exec: Arc<Exec>,
    active: Option<(Running<Image>, Arc<Frame>, Lease)>,
    leaving: Option<Leave>,
    permit_wait: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Config {
    pub(super) seed: u64,
    pub(super) tasks: u32,
    pub(super) reuse: u32,
    pub(super) stop: bool,
    /// Fixed scripts by slot instead of seeded ones.
    scripts: Option<&'static [&'static [Op]]>,
    /// A domain's clock steps only after this many of its timers were armed.
    hold_clock: u64,
}

impl Config {
    pub(super) fn of(seed: u64) -> Self {
        Self {
            seed,
            tasks: 3 + (seed % 3) as u32,
            reuse: (seed / 3 % 2) as u32 + 1,
            stop: seed % 2 == 0,
            scripts: None,
            hold_clock: 0,
        }
    }

    /// Two tasks arm one-tick sleeps in one domain at the same virtual
    /// instant; its clock steps only once both are armed, so the driver's
    /// clock pump delivers two equal deadlines in one step.
    pub(super) fn equal_sleeps(seed: u64) -> Self {
        const SLEEP: &[Op] = &[
            Op::Sleep {
                domain: 0,
                after: 1,
            },
            Op::Release,
        ];
        Self {
            seed,
            tasks: 2,
            reuse: 0,
            stop: false,
            scripts: Some(&[SLEEP, SLEEP]),
            hold_clock: 2,
        }
    }
}

/// How often each matrix cell happened; the matrix test requires every one.
#[derive(Clone, Debug, Default)]
pub(super) struct Coverage {
    pub(super) wake_before_save: u64,
    pub(super) coalesced: u64,
    pub(super) completed_wake_inert: u64,
    pub(super) stale_generation_inert: u64,
    pub(super) spurious_repark: u64,
    pub(super) reused_rows: u64,
    pub(super) yields: u64,
    pub(super) mutex_handoffs: u64,
    pub(super) cancel_withdrew: u64,
    pub(super) claim_beat_cancel: u64,
    pub(super) shield_kept_request: u64,
    pub(super) claim_lost: u64,
    pub(super) shielded_delivery: u64,
    pub(super) delayed_delivery: u64,
    pub(super) failed_calls: u64,
    pub(super) timer_equal_now: u64,
    pub(super) timer_equal_peers: u64,
    pub(super) timer_exit: u64,
    pub(super) clock_steps: u64,
    pub(super) nested_tail: u64,
    pub(super) deferred_release: u64,
    pub(super) flush_cuts: u64,
    pub(super) stop_prepared: u64,
    pub(super) stop_claimed: u64,
    pub(super) stop_returned: u64,
    pub(super) in_doubt: u64,
    pub(super) quarantined: u64,
    pub(super) reclaimed: u64,
    pub(super) discarded_after_stop: u64,
    pub(super) contraction_while_held: u64,
    pub(super) permit_waits: u64,
    pub(super) shrink_drains: u64,
    pub(super) stopped_mutex_owner: u64,
}

impl Coverage {
    pub(super) fn add(&mut self, other: &Coverage) {
        macro_rules! sum {
            ($($field:ident),*) => { $(self.$field += other.$field;)* };
        }
        sum!(
            wake_before_save,
            coalesced,
            completed_wake_inert,
            stale_generation_inert,
            spurious_repark,
            reused_rows,
            yields,
            mutex_handoffs,
            cancel_withdrew,
            claim_beat_cancel,
            shield_kept_request,
            claim_lost,
            shielded_delivery,
            delayed_delivery,
            failed_calls,
            timer_equal_now,
            timer_equal_peers,
            timer_exit,
            clock_steps,
            nested_tail,
            deferred_release,
            flush_cuts,
            stop_prepared,
            stop_claimed,
            stop_returned,
            in_doubt,
            quarantined,
            reclaimed,
            discarded_after_stop,
            contraction_while_held,
            permit_waits,
            shrink_drains,
            stopped_mutex_owner
        );
    }
}

pub(super) struct Outcome {
    pub(super) transcript: Vec<Entry>,
    pub(super) projection: String,
    pub(super) divergence: Option<Divergence>,
    pub(super) first_uncontrolled: Option<usize>,
    pub(super) coverage: Coverage,
}

struct World {
    c: Controller,
    config: Config,
    execs: Topology,
    arbiter: Arbiter,
    requests: Requests<Handle>,
    tasks: Vec<Task>,
    slots: Vec<Slot>,
    mutex: ObjectPermit,
    mutex_owner: Option<u32>,
    domains: Vec<Readiness>,
    armed: [u64; DOMAINS],
    jobs: BTreeMap<u64, Job>,
    claimed: BTreeSet<u64>,
    withdrawn: BTreeSet<u64>,
    next_id: u64,
    roots: [u32; 2],
    freed: [u32; 2],
    stale: Vec<Notification<Image>>,
    stopped: bool,
    stopped_owner: Option<u32>,
    step: u64,
    handle_drops: Arc<Mutex<BTreeMap<u64, u32>>>,
    buffer_drops: Arc<Mutex<BTreeMap<u64, u32>>>,
    handles: BTreeSet<u64>,
    buffers: BTreeSet<u64>,
    closed: u64,
    quarantined: u64,
    projection: String,
    coverage: Coverage,
}

fn with_exec<R>(exec: &Arc<Exec>, f: impl FnOnce() -> R) -> R {
    EXEC.with(|current| {
        let old = current.replace(Arc::as_ptr(exec));
        let result = f();
        current.set(old);
        result
    })
}

const ENTERED: u8 = 0;
const DONE: u8 = 1;
const TAIL: u8 = 2;

/// One phase's record and its progress, in the same section.
fn effect(section: &Section<'_>, log: &Arc<TaskLog>, op: u64, phase: Phase, tag: u8) {
    let mut bytes = op.to_le_bytes().to_vec();
    bytes.push(tag);
    section.append(log, &bytes);
    section.publish(log, Progress::new(op, phase));
}

/// Apply `n` releases of `root`. A root brought to zero is walked as the
/// tail of this phase: its nested release applies at once (hirsel-free
/// pending.rs, Walks), recorded in the same section as the phase.
fn release(
    roots: &mut [u32; 2],
    freed: &mut [u32; 2],
    coverage: &mut Coverage,
    section: &Section<'_>,
    log: &Arc<TaskLog>,
    op: u64,
    root: usize,
    n: u32,
) {
    roots[root] = roots[root]
        .checked_sub(n)
        .expect("a root released below zero");
    if roots[root] == 0 {
        freed[root] += 1;
        let mut bytes = op.to_le_bytes().to_vec();
        bytes.push(TAIL);
        section.append(log, &bytes);
        if root == 0 {
            coverage.nested_tail += 1;
            release(roots, freed, coverage, section, log, op, 1, 1);
        }
    }
}

fn script(c: &mut Controller) -> Vec<Op> {
    let mut script = Vec::new();
    for _ in 0..2 + c.random("script-len", 4) {
        match c.random("script-op", 7) {
            0 => script.push(Op::Compute),
            1 => script.push(Op::Yield),
            2 => script.extend([Op::Lock, Op::Yield, Op::Unlock]),
            3 => script.push(Op::Host {
                shielded: c.random("shield", 3) == 0,
            }),
            4 => script.push(Op::Sleep {
                domain: c.random("domain", DOMAINS as u64) as usize,
                after: c.random("after", 3),
            }),
            5 => script.extend([Op::Dup, Op::Release]),
            _ => script.extend([Op::Lock, Op::Unlock]),
        }
    }
    script.push(Op::Release);
    script
}

/// A fresh row generation and its first queue token (`frame::prepared`).
fn prepare(slot: u32, generation: u32, script: Vec<Op>, reuse: u32) -> (Task, Queued<Image>) {
    let (notification, queued) = frame::prepared(Image {
        ctx: std::ptr::null_mut(),
        task: slot,
        sp: std::ptr::null_mut(),
    });
    let frame = Arc::new(Frame {
        notification,
        owner: AtomicUsize::new(owner_word(Owner::Any)),
        saved: AtomicBool::new(true),
        #[cfg(feature = "free-qualification")]
        stop_before_start: false,
    });
    let task = Task {
        slot,
        generation,
        frame,
        script,
        pc: 0,
        custody: Custody::Queued,
        remembered: false,
        log: TaskLog::new(vec![slot, generation]),
        cancel: Flag::default(),
        wait: None,
        job: None,
        mail: None,
        mutex_queued: false,
        pending: Pending::default(),
        handles: Vec::new(),
        reuse,
        successor: None,
    };
    (task, queued)
}

fn validate(projection: &mut String, scanned: &[hirsel_free::section::Scanned]) {
    for task in scanned {
        assert_eq!(task.bytes.len() % 9, 0, "whole records only");
        let ops: Vec<(u64, u8)> = task
            .bytes
            .chunks(9)
            .map(|r| (u64::from_le_bytes(r[..8].try_into().unwrap()), r[8]))
            .collect();
        // A cut holds a phase's records exactly when it admits the phase.
        if let Some(&(op, _)) = ops.last() {
            assert_eq!(op, task.progress.op, "cut split a phase from its records");
        }
        assert!(ops.iter().all(|&(op, _)| op <= task.progress.op));
        let _ = writeln!(
            projection,
            "  {:?} {} {:?}",
            task.path,
            task.progress,
            ops.iter().map(|r| r.1).collect::<Vec<_>>()
        );
    }
}

/// The phase's section, or the segment ends: the stop's cut stays closed.
macro_rules! section {
    ($world:ident, $w:expr) => {{
        if $world.arbiter.gate().unwrap().stopped() {
            return $world.terminate($w);
        }
        $world
            .arbiter
            .section($w)
            .expect("an open gate admits the section")
            .expect("the driver records")
    }};
}

impl World {
    fn new(config: Config, mut c: Controller) -> Self {
        let execs = topology(WIDTH);
        let slots = execs
            .compute
            .lock()
            .unwrap()
            .active
            .iter()
            .map(|exec| Slot {
                exec: exec.clone(),
                active: None,
                leaving: None,
                permit_wait: false,
            })
            .collect();
        let mut firsts = Vec::new();
        let mut tasks = Vec::new();
        for slot in 0..config.tasks {
            let script = match config.scripts {
                Some(scripts) => scripts[slot as usize].to_vec(),
                None => script(&mut c),
            };
            let (task, queued) = prepare(slot, 0, script, config.reuse);
            tasks.push(task);
            firsts.push(queued);
        }
        let mut world = Self {
            c,
            config,
            execs,
            arbiter: Arbiter::new(true, usize::from(WIDTH)),
            requests: Requests::default(),
            tasks,
            slots,
            mutex: ObjectPermit::new(),
            mutex_owner: None,
            domains: (0..DOMAINS).map(|_| Readiness::virtual_clock(0)).collect(),
            armed: [0; DOMAINS],
            jobs: BTreeMap::new(),
            claimed: BTreeSet::new(),
            withdrawn: BTreeSet::new(),
            next_id: 0,
            // Every generation, reused rows included, holds root 0 from
            // admission; root 0 holds nested root 1.
            roots: [config.tasks * (config.reuse + 1), 1],
            freed: [0, 0],
            stale: Vec::new(),
            stopped: false,
            stopped_owner: None,
            step: 0,
            handle_drops: Default::default(),
            buffer_drops: Default::default(),
            handles: BTreeSet::new(),
            buffers: BTreeSet::new(),
            closed: 0,
            quarantined: 0,
            projection: String::new(),
            coverage: Coverage::default(),
        };
        assert_eq!(
            world.mutex.release(),
            None,
            "the mutex starts with its payload"
        );
        {
            let section = world.arbiter.gate().unwrap().enter(0).unwrap();
            for task in &world.tasks {
                section.register(&task.log);
            }
        }
        for (slot, queued) in firsts.into_iter().enumerate() {
            world.publish_first(slot, queued);
        }
        world
    }

    fn dispatch(&self) -> &Dispatch {
        self.execs.dispatch.get().unwrap()
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// schedule_saved's prepared branch: the first token of a new row.
    fn publish_first(&mut self, slot: usize, queued: Queued<Image>) {
        assert_eq!(self.tasks[slot].custody, Custody::Queued);
        let saved = Saved {
            queued,
            frame: self.tasks[slot].frame.clone(),
            lease: Lease::new(&self.dispatch().count),
            chained: false,
        };
        publish(&self.execs, saved, false, false);
    }

    /// schedule_saved's notification branch.
    fn notify(&mut self, slot: usize) {
        let queued = self.tasks[slot].frame.notification.notify();
        let task = &mut self.tasks[slot];
        match queued {
            Some(queued) => {
                assert_eq!(task.custody, Custody::Parked, "a wake duplicated custody");
                task.custody = Custody::Queued;
                let saved = Saved {
                    queued,
                    frame: task.frame.clone(),
                    lease: Lease::new(&self.execs.dispatch.get().unwrap().count),
                    chained: false,
                };
                publish(&self.execs, saved, false, false);
            }
            None => match task.custody {
                Custody::Running(_) => {
                    task.remembered = true;
                    self.coverage.wake_before_save += 1;
                }
                Custody::Queued => self.coverage.coalesced += 1,
                Custody::Done => self.coverage.completed_wake_inert += 1,
                Custody::Parked => panic!("a saved idle frame refused its wake"),
            },
        }
    }

    fn queued(&self) -> Vec<u32> {
        self.tasks
            .iter()
            .filter(|task| task.custody == Custody::Queued)
            .map(|task| task.slot)
            .collect()
    }

    fn events(&self) -> Vec<u32> {
        let queued = self.dispatch().queued.load(Ordering::SeqCst) != 0;
        let mut events = Vec::new();
        for (w, slot) in self.slots.iter().enumerate() {
            let enabled = if slot.leaving.is_some() {
                true
            } else if slot.active.is_some() {
                !slot.permit_wait || self.execs.permits.available() > 0
            } else {
                queued
            };
            if enabled {
                events.push(w as u32);
            }
        }
        for (&id, job) in &self.jobs {
            match job.stage {
                Stage::Posted => events.push(1 << 24 | id as u32),
                Stage::Claimed { .. } => events.push(2 << 24 | id as u32),
                Stage::Returned => {}
            }
        }
        for domain in 0..DOMAINS {
            if self.armed[domain] >= self.config.hold_clock
                && self
                    .tasks
                    .iter()
                    .any(|t| matches!(t.wait, Some(Wait::Timer { domain: d, .. }) if d == domain))
            {
                events.push(3 << 24 | domain as u32);
            }
        }
        events
    }

    fn run(mut self) -> Outcome {
        loop {
            self.step += 1;
            assert!(self.step < STEPS, "the composed run did not quiesce");
            self.inject();
            let events = self.events();
            let event = self.c.decide("event", events);
            if self.c.diverged() {
                break;
            }
            let Some(event) = event else { break };
            let arg = (event & 0xff_ffff) as usize;
            match event >> 24 {
                0 => self.worker(arg),
                1 => self.claim(arg as u64),
                2 => self.complete(arg as u64),
                _ => self.clock(arg),
            }
            if self.c.diverged() {
                break;
            }
            self.check();
        }
        self.finish()
    }

    fn inject(&mut self) {
        let tasks = self.tasks.len() as u64;
        match self.c.random("inject", 40) {
            0 if !self.stopped => self.flush(),
            1 if self.config.stop && !self.stopped && self.step > 8 => self.stop(),
            2 => {
                let slot = self.c.random("cancel", tasks) as usize;
                self.cancel(slot);
            }
            3 => {
                let slot = self.c.random("spurious", tasks) as usize;
                self.notify(slot);
            }
            4 if !self.stale.is_empty() => {
                let at = self.c.random("stale", self.stale.len() as u64) as usize;
                assert!(
                    self.stale[at].notify().is_none(),
                    "a reused row's old notification published"
                );
                self.coverage.stale_generation_inert += 1;
            }
            5 => {
                let limit = 1 + self.c.random("permits", u64::from(WIDTH)) as u32;
                let mut state = self.execs.permits.state.lock().unwrap();
                if state.held > limit {
                    self.coverage.contraction_while_held += 1;
                }
                state.limit = limit;
                drop(state);
                self.execs.permits.freed.notify_all();
            }
            6 => {
                let width = 1 + self.c.random("width", u64::from(WIDTH)) as u32;
                let ready = &self.execs.dispatch.get().unwrap().ready;
                if width < ready.width() {
                    self.coverage.shrink_drains += 1;
                }
                ready.set_width(width);
            }
            _ => {}
        }
    }

    fn worker(&mut self, w: usize) {
        if self.slots[w].leaving.is_some() {
            self.idle_save(w);
        } else if self.slots[w].active.is_none() {
            self.search(w);
        } else if self.slots[w].permit_wait {
            self.acquire(w);
        } else {
            self.operate(w);
        }
    }

    fn acquire(&mut self, w: usize) {
        let slot = &mut self.slots[w];
        if !slot.exec.held.load(Ordering::Acquire) && self.execs.permits.available() == 0 {
            if !slot.permit_wait {
                self.coverage.permit_waits += 1;
            }
            slot.permit_wait = true;
            return;
        }
        assert!(self.execs.permits.acquire(&slot.exec, None, None).is_ok());
        slot.permit_wait = false;
    }

    /// pool::free::next, then resume's token start and stop check.
    fn search(&mut self, w: usize) {
        let runnable = self.queued();
        let exec = self.slots[w].exec.clone();
        let saved = with_exec(&exec, || next(&self.execs, &exec));
        let Some(Saved {
            queued,
            frame,
            lease,
            ..
        }) = saved
        else {
            self.c.observe("dispatch", runnable, None);
            return;
        };
        let running = queued.start();
        let slot = running.value().task;
        let task = &mut self.tasks[slot as usize];
        assert!(
            Arc::ptr_eq(&frame, &task.frame),
            "an old generation's token reached a worker"
        );
        assert_eq!(task.custody, Custody::Queued, "dispatch duplicated custody");
        self.c.observe("dispatch", runnable, Some(slot));
        if self.stopped {
            drop(running);
            drop(lease);
            exec.busy.store(false, Ordering::SeqCst);
            task.custody = Custody::Done;
            self.coverage.discarded_after_stop += 1;
            return;
        }
        task.custody = Custody::Running(w);
        frame.saved.store(false, Ordering::Release);
        self.slots[w].active = Some((running, frame, lease));
        self.acquire(w);
    }

    fn running(&self, w: usize) -> usize {
        self.slots[w].active.as_ref().unwrap().0.value().task as usize
    }

    fn leave(&mut self, w: usize, yielded: bool, site: &'static str) {
        let t = self.running(w);
        let task = &self.tasks[t];
        self.c.push(Entry::Yield {
            task: task.slot,
            generation: task.generation,
            op: task.pc,
            site,
        });
        if yielded {
            self.coverage.yields += 1;
        }
        self.slots[w].leaving = Some(Leave::Suspend {
            owner: Owner::Any,
            yielded,
        });
    }

    /// A stopped run's segment ends at its next effect.
    fn terminate(&mut self, w: usize) {
        assert!(self.stopped, "only the stop refuses a section");
        self.slots[w].leaving = Some(Leave::Complete);
    }

    /// pool::free::saved: the idle continuation after the switch.
    fn idle_save(&mut self, w: usize) {
        let leaving = self.slots[w].leaving.take().unwrap();
        let (running, frame, lease) = self.slots[w].active.take().unwrap();
        let exec = self.slots[w].exec.clone();
        self.execs.permits.release(&exec);
        let t = running.value().task as usize;
        match leaving {
            Leave::Complete => {
                drop(running);
                exec.busy.store(false, Ordering::SeqCst);
                drop(lease);
                self.tasks[t].custody = Custody::Done;
                // Completion invalidates every later wake of this generation.
                assert!(self.tasks[t].frame.notification.notify().is_none());
                self.coverage.completed_wake_inert += 1;
                if let Some((next, queued)) = self.tasks[t].successor.take() {
                    let old = std::mem::replace(&mut self.tasks[t], *next);
                    self.stale.push(old.frame.notification.clone());
                    self.coverage.reused_rows += 1;
                    self.publish_first(t, queued);
                }
            }
            Leave::Suspend { yielded, .. } => {
                frame.owner.store(owner_word(Owner::Any), Ordering::Release);
                frame.mark_saved();
                let remembered = std::mem::take(&mut self.tasks[t].remembered);
                let queued = running.save(yielded);
                assert_eq!(
                    queued.is_some(),
                    yielded || remembered,
                    "the idle save lost or invented a wake"
                );
                exec.busy.store(false, Ordering::SeqCst);
                if let Some(Wait::Timer { domain, token, .. }) = self.tasks[t].wait {
                    self.domains[domain].parked(token);
                }
                if let Some(queued) = queued {
                    self.tasks[t].custody = Custody::Queued;
                    let saved = Saved {
                        queued,
                        frame,
                        lease,
                        chained: false,
                    };
                    with_exec(&exec, || publish(&self.execs, saved, yielded, true));
                } else {
                    self.tasks[t].custody = Custody::Parked;
                    drop(lease);
                }
            }
        }
    }

    /// One operation of the running segment.
    fn operate(&mut self, w: usize) {
        let t = self.running(w);
        let slot = self.tasks[t].slot;
        if self.tasks[t].pc == self.tasks[t].script.len() {
            return self.end(w, t);
        }
        let op = self.tasks[t].script[self.tasks[t].pc];
        let index = self.tasks[t].pc as u64 + 1;
        match op {
            Op::Compute => self.tasks[t].pc += 1,
            Op::Yield => {
                self.tasks[t].pc += 1;
                self.leave(w, true, "yield");
            }
            Op::Lock => {
                let section = section!(self, w);
                let task = &mut self.tasks[t];
                match self.mutex.acquire(slot, &mut task.mutex_queued) {
                    Acquired::Payload => {
                        assert!(self.mutex_owner.replace(slot).is_none(), "two mutex owners");
                        effect(&section, &task.log, index, Phase::Done, DONE);
                        task.pc += 1;
                        task.wait = None;
                    }
                    Acquired::Pending => {
                        task.wait = Some(Wait::Mutex);
                        drop(section);
                        self.leave(w, false, "mutex");
                    }
                    Acquired::Dead => unreachable!("the driver never kills its mutex"),
                }
            }
            Op::Unlock => {
                let section = section!(self, w);
                let successor = self.mutex.release();
                assert_eq!(self.mutex_owner.take(), Some(slot));
                effect(&section, &self.tasks[t].log, index, Phase::Done, DONE);
                drop(section);
                self.tasks[t].pc += 1;
                // Direct custody notification after leaving the object's section.
                if let Some(successor) = successor {
                    self.coverage.mutex_handoffs += 1;
                    self.notify(successor as usize);
                }
            }
            Op::Host { shielded } => self.host(w, t, index, shielded),
            Op::Sleep { domain, after } => self.sleep(w, t, index, domain, after),
            Op::Dup => {
                let section = section!(self, w);
                let task = &mut self.tasks[t];
                if !task.pending.dup(&0) {
                    self.roots[0] += 1;
                }
                effect(&section, &task.log, index, Phase::Done, DONE);
                task.pc += 1;
            }
            Op::Release => {
                let section = section!(self, w);
                let task = &mut self.tasks[t];
                if self.roots[0] == 1 {
                    // The segment frees the root and walks it here.
                    release(
                        &mut self.roots,
                        &mut self.freed,
                        &mut self.coverage,
                        &section,
                        &task.log,
                        index,
                        0,
                        1,
                    );
                } else {
                    task.pending.defer(0);
                    self.coverage.deferred_release += 1;
                }
                effect(&section, &task.log, index, Phase::Done, DONE);
                task.pc += 1;
            }
        }
    }

    fn host(&mut self, w: usize, t: usize, index: u64, shielded: bool) {
        if let Some(mail) = self.tasks[t].mail.take() {
            if self.arbiter.gate().unwrap().stopped() {
                // A completion after the terminal cut is never delivered.
                if let Mail::Result(Some(handle)) = mail {
                    drop(handle);
                    self.quarantined += 1;
                    self.coverage.quarantined += 1;
                }
                return self.terminate(w);
            }
            let section = section!(self, w);
            let task = &mut self.tasks[t];
            let id = task.job.take().unwrap();
            match mail {
                Mail::Result(handle) => {
                    let job = self.jobs.remove(&id).unwrap();
                    assert_eq!(job.stage, Stage::Returned);
                    job.request.finish();
                    self.requests.retire(id);
                    match handle {
                        Some(handle) => task.handles.push(handle),
                        None => self.coverage.failed_calls += 1,
                    }
                    if shielded && task.cancel.cause().is_some() {
                        self.coverage.shielded_delivery += 1;
                    }
                }
                Mail::Canceled => assert!(self.withdrawn.contains(&id)),
                Mail::Due => unreachable!("a host wait received a timer"),
            }
            effect(&section, &task.log, index, Phase::Done, DONE);
            task.pc += 1;
            return;
        }
        if self.tasks[t].wait.is_some() {
            self.coverage.spurious_repark += 1;
            return self.leave(w, false, "host-repark");
        }
        if !shielded && self.tasks[t].cancel.cause().is_some() {
            self.tasks[t].pc += 1;
            return;
        }
        let section = section!(self, w);
        let id = self.next_id + 1;
        self.next_id = id;
        self.buffers.insert(id);
        let request = self.requests.prepare(
            id,
            Handle {
                id,
                drops: self.buffer_drops.clone(),
            },
        );
        assert_eq!(request.state(), Claim::Prepared);
        let task = &mut self.tasks[t];
        effect(&section, &task.log, index, Phase::Entered, ENTERED);
        drop(section);
        self.jobs.insert(
            id,
            Job {
                request,
                slot: task.slot,
                generation: task.generation,
                shielded,
                stage: Stage::Posted,
            },
        );
        task.wait = Some(Wait::Host(id));
        task.job = Some(id);
        self.leave(w, false, "host");
    }

    fn sleep(&mut self, w: usize, t: usize, index: u64, domain: usize, after: u64) {
        if let Some(mail) = self.tasks[t].mail.take() {
            assert!(matches!(mail, Mail::Due | Mail::Canceled));
            let section = section!(self, w);
            effect(&section, &self.tasks[t].log, index, Phase::Done, DONE);
            self.tasks[t].pc += 1;
            return;
        }
        if self.tasks[t].wait.is_some() {
            self.coverage.spurious_repark += 1;
            return self.leave(w, false, "timer-repark");
        }
        if self.tasks[t].cancel.cause().is_some() {
            self.tasks[t].pc += 1;
            return;
        }
        let token = self.id();
        let readiness = &self.domains[domain];
        let at = readiness.virtual_time().unwrap() + after;
        readiness.enter(token);
        if readiness.timer(at).unwrap() {
            // A deadline equal to the domain's now is already due.
            assert_eq!(after, 0);
            self.coverage.timer_equal_now += 1;
            let section = section!(self, w);
            effect(&section, &self.tasks[t].log, index, Phase::Done, DONE);
            self.tasks[t].pc += 1;
        } else {
            self.armed[domain] += 1;
            self.tasks[t].wait = Some(Wait::Timer { domain, token, at });
            self.leave(w, false, "timer");
        }
    }

    /// The end's cleanup phase: handles close, deferred releases apply with
    /// their walks as this phase's tail, and a reused row is registered.
    fn end(&mut self, w: usize, t: usize) {
        let section = section!(self, w);
        let index = self.tasks[t].script.len() as u64 + 1;
        let task = &mut self.tasks[t];
        for handle in task.handles.drain(..) {
            drop(handle);
            self.closed += 1;
        }
        let successor = (task.reuse > 0).then(|| {
            let script = script(&mut self.c);
            let (next, queued) = prepare(task.slot, task.generation + 1, script, task.reuse - 1);
            section.register(&next.log);
            (Box::new(next), queued)
        });
        for (root, n) in task.pending.take() {
            release(
                &mut self.roots,
                &mut self.freed,
                &mut self.coverage,
                &section,
                &task.log,
                index,
                root,
                n,
            );
        }
        effect(&section, &task.log, index, Phase::Done, DONE);
        task.successor = successor;
        drop(section);
        self.slots[w].leaving = Some(Leave::Complete);
    }

    /// The home claims a posted request by CAS (claim.rs).
    fn claim(&mut self, id: u64) {
        let job = self.jobs.get_mut(&id).unwrap();
        if job.request.claim() {
            assert!(self.claimed.insert(id));
            let fail = self.c.random("host-fail", 6) == 0;
            let delay = self.c.random("host-delay", 5);
            if delay > 0 {
                self.coverage.delayed_delivery += 1;
            }
            job.stage = Stage::Claimed {
                remaining: delay,
                fail,
            };
        } else {
            assert_eq!(job.request.state(), Claim::Withdrawn);
            assert!(self.withdrawn.contains(&id));
            self.jobs.remove(&id);
            self.coverage.claim_lost += 1;
        }
    }

    /// A delayed host completion with an initialized handle.
    fn complete(&mut self, id: u64) {
        let Some(Job {
            stage: Stage::Claimed { remaining, fail },
            ..
        }) = self.jobs.get_mut(&id)
        else {
            unreachable!()
        };
        if *remaining > 0 {
            *remaining -= 1;
            return;
        }
        let fail = *fail;
        let handle = (!fail).then(|| {
            let handle = self.id();
            self.handles.insert(handle);
            Handle {
                id: handle,
                drops: self.handle_drops.clone(),
            }
        });
        if self.stopped {
            if let Some(handle) = handle {
                drop(handle);
                self.quarantined += 1;
            }
            self.coverage.quarantined += 1;
            self.jobs.remove(&id);
            return;
        }
        let job = self.jobs.get_mut(&id).unwrap();
        let t = job.slot as usize;
        let task = &mut self.tasks[t];
        assert_eq!(
            task.generation, job.generation,
            "a completion reached a reused row"
        );
        assert_eq!(task.wait, Some(Wait::Host(id)));
        task.wait = None;
        task.mail = Some(Mail::Result(handle));
        job.stage = Stage::Returned;
        self.notify(t);
    }

    /// Housekeeping by steps: one domain's scheduling point, or its clock
    /// advancing to the next live deadline. Other domains keep their time.
    fn clock(&mut self, domain: usize) {
        let others: Vec<_> = self.domains.iter().map(Readiness::virtual_time).collect();
        let ready = self.domains[domain].ready().unwrap();
        if ready.is_empty() {
            match self.domains[domain].wait_source(false).unwrap() {
                WaitSource::Ready => self.coverage.clock_steps += 1,
                // A passed deadline stays latched until its task's idle save parks it.
                WaitSource::Empty => assert!(self.tasks.iter().any(|task| {
                    matches!(task.wait, Some(Wait::Timer { domain: d, .. }) if d == domain)
                        && matches!(task.custody, Custody::Running(_))
                })),
                WaitSource::Driver(..) => panic!("a virtual domain waited on its driver"),
            }
        }
        for (d, before) in others.into_iter().enumerate() {
            if d != domain {
                assert_eq!(self.domains[d].virtual_time(), before);
            }
        }
        let mut ats = Vec::new();
        for token in ready {
            let t = self
                .tasks
                .iter()
                .position(|task| {
                    matches!(task.wait, Some(Wait::Timer { domain: d, token: k, .. }) if d == domain && k == token)
                })
                .expect("a timer reported for no current wait");
            let Some(Wait::Timer { at, .. }) = self.tasks[t].wait else {
                unreachable!()
            };
            assert!(at <= self.domains[domain].virtual_time().unwrap());
            ats.push(at);
            self.domains[domain].finish(token);
            self.tasks[t].wait = None;
            self.tasks[t].mail = Some(Mail::Due);
            self.notify(t);
        }
        ats.sort_unstable();
        if ats.windows(2).any(|pair| pair[0] == pair[1]) {
            self.coverage.timer_equal_peers += 1;
        }
    }

    fn cancel(&mut self, t: usize) {
        if self.tasks[t].custody == Custody::Done || !self.tasks[t].cancel.cancel(1) {
            return;
        }
        match self.tasks[t].wait {
            Some(Wait::Host(id)) => {
                let Some(job) = self.jobs.get(&id) else {
                    // Swept or quarantined at the stop; its row never runs again.
                    assert!(self.stopped);
                    return;
                };
                if job.shielded {
                    self.coverage.shield_kept_request += 1;
                } else if job.stage == Stage::Posted && job.request.withdraw() {
                    // Exactly one of the home's claim and this withdraw wins.
                    assert!(self.withdrawn.insert(id));
                    self.requests.retire(id);
                    self.jobs.remove(&id);
                    self.tasks[t].wait = None;
                    self.tasks[t].mail = Some(Mail::Canceled);
                    self.coverage.cancel_withdrew += 1;
                    self.notify(t);
                } else {
                    self.coverage.claim_beat_cancel += 1;
                }
            }
            Some(Wait::Timer { domain, token, .. }) => {
                // The timer exits: a later clock step never reports it.
                self.domains[domain].finish(token);
                self.tasks[t].wait = None;
                self.tasks[t].mail = Some(Mail::Canceled);
                self.coverage.timer_exit += 1;
                self.notify(t);
            }
            // Mutex take is not a cancellation point (CONC-3).
            Some(Wait::Mutex) | None => {}
        }
    }

    fn flush(&mut self) {
        let Some(mut cut) = self.arbiter.gate().unwrap().flush_cut() else {
            return;
        };
        let scanned = cut.scan();
        drop(cut);
        let _ = writeln!(self.projection, "flush {}", self.step);
        validate(&mut self.projection, &scanned);
        self.coverage.flush_cuts += 1;
    }

    fn stop(&mut self) {
        for job in self.jobs.values() {
            match job.stage {
                Stage::Posted => self.coverage.stop_prepared += 1,
                Stage::Claimed { .. } => self.coverage.stop_claimed += 1,
                Stage::Returned => self.coverage.stop_returned += 1,
            }
        }
        let terminal = self.arbiter.stop(0, &[&self.requests]).expect("one stop");
        let mut cut: Cut<'_> = terminal.cut.expect("a recorded stop cuts");
        let scanned = cut.scan();
        let progress = cut.progress();
        drop(cut);
        let sweep = terminal.sweep;
        assert!(
            self.arbiter.section(0).is_err(),
            "no section opens after the stop"
        );
        let _ = writeln!(self.projection, "stop {} {:?}", self.step, progress);
        validate(&mut self.projection, &scanned);
        for id in &sweep.withdrawn {
            assert_eq!(self.jobs[id].stage, Stage::Posted);
            assert!(self.withdrawn.insert(*id));
        }
        for id in &sweep.in_doubt {
            assert!(matches!(
                self.jobs[id].stage,
                Stage::Claimed { .. } | Stage::Returned
            ));
        }
        self.coverage.in_doubt += sweep.in_doubt.len() as u64;
        let _ = writeln!(self.projection, "sweep {sweep:?}");
        self.stopped = true;
        self.stopped_owner = self.mutex_owner;
        if self.mutex_owner.is_some() {
            self.coverage.stopped_mutex_owner += 1;
        }
    }

    /// Exactly-one custody after every transition.
    fn check(&self) {
        let live = self.dispatch().count.live.load(Ordering::Acquire) as usize;
        let queued = self.queued().len();
        let active = self.slots.iter().filter(|s| s.active.is_some()).count();
        assert_eq!(
            live,
            queued + active,
            "reservations equal queue and running tokens"
        );
        for (w, slot) in self.slots.iter().enumerate() {
            if let Some((running, frame, _)) = &slot.active {
                let task = &self.tasks[running.value().task as usize];
                assert_eq!(task.custody, Custody::Running(w));
                assert!(Arc::ptr_eq(frame, &task.frame));
            }
        }
        for task in &self.tasks {
            if let Custody::Running(w) = task.custody {
                assert!(self.slots[w].active.is_some());
            }
        }
        let held = self
            .slots
            .iter()
            .filter(|s| s.exec.held.load(Ordering::Acquire))
            .count() as u32;
        assert_eq!(self.execs.permits.state.lock().unwrap().held, held);
        match (self.stopped, self.mutex_owner) {
            (false, Some(owner)) => {
                assert_ne!(self.tasks[owner as usize].custody, Custody::Done);
            }
            // After the cut no section opens, so no acquire, release or
            // handoff: the stopped owner keeps the payload until teardown.
            (true, owner) => {
                assert_eq!(
                    owner, self.stopped_owner,
                    "mutex custody moved after the stop"
                );
                assert_eq!(self.mutex.available(), owner.is_none());
            }
            (false, None) => {}
        }
    }

    fn finish(mut self) -> Outcome {
        let divergence = self.c.divergence.take();
        if divergence.is_none() {
            if self.stopped {
                assert!(self.arbiter.gate().unwrap().stopped());
            } else {
                for task in &self.tasks {
                    assert_eq!(task.custody, Custody::Done, "task {} stranded", task.slot);
                    assert_eq!(task.reuse, 0);
                }
                assert_eq!(self.roots, [0, 0]);
                assert_eq!(self.freed, [1, 1], "each root walked exactly once");
                assert_eq!(self.mutex_owner, None);
                assert!(self.jobs.is_empty());
                assert_eq!(self.quarantined, 0);
            }
            assert_eq!(self.dispatch().queued.load(Ordering::SeqCst), 0);
        }
        let _ = writeln!(
            self.projection,
            "end roots {:?} freed {:?} closed {} quarantined {} claimed {:?} withdrawn {:?}",
            self.roots, self.freed, self.closed, self.quarantined, self.claimed, self.withdrawn
        );
        for task in &self.tasks {
            let _ = writeln!(
                self.projection,
                "task {} gen {} {:?} pc {}",
                task.slot, task.generation, task.custody, task.pc
            );
        }
        // Teardown: running owners complete, queued tokens are discarded,
        // parked rows and undelivered mail are reclaimed.
        let reclaimable = self.handles.len() as u64 - self.closed - self.quarantined;
        for slot in &mut self.slots {
            slot.active = None;
        }
        self.coverage.reclaimed += reclaimable;
        let World {
            c,
            execs,
            tasks,
            jobs,
            requests,
            handle_drops,
            buffer_drops,
            handles,
            buffers,
            claimed,
            withdrawn,
            slots,
            projection,
            coverage,
            ..
        } = self;
        drop(slots);
        drop(tasks);
        drop(jobs);
        drop(requests);
        drop(execs);
        if divergence.is_none() {
            let handle_drops = handle_drops.lock().unwrap();
            for id in &handles {
                assert_eq!(
                    handle_drops.get(id),
                    Some(&1),
                    "handle {id} closed exactly once"
                );
            }
            assert_eq!(handle_drops.len(), handles.len());
            let buffer_drops = buffer_drops.lock().unwrap();
            for id in &buffers {
                assert_eq!(
                    buffer_drops.get(id),
                    Some(&1),
                    "buffer {id} freed exactly once"
                );
            }
            assert!(
                claimed.is_disjoint(&withdrawn),
                "claim and withdraw both won"
            );
            let decided: BTreeSet<u64> = claimed.union(&withdrawn).copied().collect();
            assert_eq!(decided, buffers, "every request was claimed or withdrawn");
        }
        Outcome {
            first_uncontrolled: c.first_uncontrolled,
            transcript: c.transcript,
            projection,
            divergence,
            coverage,
        }
    }
}

pub(super) fn run(
    config: Config,
    expected: Option<Vec<Entry>>,
    uncontrolled: Option<&AtomicU64>,
) -> Outcome {
    World::new(config, Controller::new(config.seed, expected, uncontrolled)).run()
}

const SEEDS: u64 = 96;

#[test]
fn runtime50_composed_seed_transcript() {
    let mut projections = BTreeSet::new();
    let mut entries = 0;
    for seed in 0..SEEDS {
        let config = Config::of(seed);
        let first = run(config, None, None);
        assert!(first.first_uncontrolled.is_none());
        let again = run(config, Some(first.transcript.clone()), None);
        assert!(
            again.divergence.is_none(),
            "seed {seed} repeated differently: {:?}",
            again.divergence
        );
        assert_eq!(first.transcript, again.transcript);
        assert_eq!(first.projection, again.projection);
        assert!(
            first.transcript.iter().any(|e| matches!(
                e,
                Entry::Decision {
                    site: "dispatch",
                    selected: Some(_),
                    ..
                }
            )) && first
                .transcript
                .iter()
                .any(|e| matches!(e, Entry::Yield { .. }))
                && first
                    .transcript
                    .iter()
                    .any(|e| matches!(e, Entry::Random { .. }))
        );
        entries += first.transcript.len();
        projections.insert(first.projection);
    }
    assert!(
        projections.len() as u64 > SEEDS * 3 / 4,
        "seeds explore distinct schedules"
    );
    // A checked transcript is not vacuous: another seed's schedule diverges.
    let other = run(Config::of(1), None, None);
    let checked = run(Config::of(7), Some(other.transcript), None);
    assert!(checked.divergence.is_some());

    // Mutation: one choice reads process state the seed does not control.
    // The checked repeat must fail at exactly that first decision.
    let clock = AtomicU64::new(0);
    let mut smallest: Option<(u64, Divergence)> = None;
    for seed in 0..SEEDS {
        let config = Config::of(seed);
        let first = run(config, None, Some(&clock));
        let at = first.first_uncontrolled.expect("a choice among several");
        let again = run(config, Some(first.transcript.clone()), Some(&clock));
        let divergence = again
            .divergence
            .expect("an uncontrolled choice must be detected");
        assert_eq!(
            divergence.index, at,
            "seed {seed} failed after its first decision"
        );
        assert_eq!(again.transcript[..at], first.transcript[..at]);
        assert!(matches!(
            divergence.actual,
            Entry::Decision { site: "event", .. }
        ));
        if smallest
            .as_ref()
            .is_none_or(|(_, d)| divergence.index < d.index)
        {
            smallest = Some((seed, divergence));
        }
    }
    let (seed, divergence) = smallest.unwrap();
    eprintln!(
        "RUNTIME50 seeds {SEEDS} repeated exactly ({entries} transcript entries, {} distinct projections); \
         smallest divergent trace: seed {seed}, entry {}, expected {:?}, actual {:?}",
        projections.len(),
        divergence.index,
        divergence.expected,
        divergence.actual
    );
}

#[test]
fn runtime50_composed_ownership_cut_matrix() {
    // Direct vectors on the production frame and dispatcher.
    {
        let execs = topology(1);
        let exec = execs.compute.lock().unwrap().active[0].clone();
        let (task, first) = prepare(0, 0, Vec::new(), 0);
        let lease = || Lease::new(&execs.dispatch.get().unwrap().count);
        publish(
            &execs,
            Saved {
                queued: first,
                frame: task.frame.clone(),
                lease: lease(),
                chained: false,
            },
            false,
            false,
        );
        // A queued wake coalesces into the one token.
        assert!(task.frame.notification.notify().is_none());
        let saved = next(&execs, &exec).expect("the published token");
        let running = saved.queued.start();
        // Wake before save: remembered, then the save returns the token.
        assert!(task.frame.notification.notify().is_none());
        let queued = running.save(false).expect("the remembered wake");
        publish(
            &execs,
            Saved {
                queued,
                frame: saved.frame,
                lease: saved.lease,
                chained: false,
            },
            false,
            false,
        );
        assert!(next(&execs, &exec).is_some_and(|saved| {
            // Close while queued: discarding invalidates later wakes.
            drop(saved);
            true
        }));
        assert!(task.frame.notification.notify().is_none());
        assert_eq!(
            execs
                .dispatch
                .get()
                .unwrap()
                .count
                .live
                .load(Ordering::Acquire),
            0
        );
        // Row reuse: the old generation's notification stays inert.
        let (reused, queued) = prepare(0, 1, Vec::new(), 0);
        assert!(task.frame.notification.notify().is_none());
        drop(queued.start());
        assert!(reused.frame.notification.notify().is_none());
    }
    let mut total = Coverage::default();
    let mut stops = 0;
    for seed in 0..4 * SEEDS {
        let outcome = run(Config::of(seed), None, None);
        stops += u64::from(outcome.projection.lines().any(|l| l.starts_with("stop ")));
        total.add(&outcome.coverage);
    }
    eprintln!(
        "RUNTIME50 matrix over {} seeds, {stops} stops: {total:#?}",
        4 * SEEDS
    );
    let cells = [
        ("wake before save", total.wake_before_save),
        ("coalesced wake", total.coalesced),
        ("completed wake inert", total.completed_wake_inert),
        ("stale generation inert", total.stale_generation_inert),
        ("spurious wake re-parks", total.spurious_repark),
        ("row reuse", total.reused_rows),
        ("mutex custody handoff", total.mutex_handoffs),
        ("cancel withdraws Prepared", total.cancel_withdrew),
        ("claim beats cancel", total.claim_beat_cancel),
        ("shield keeps request", total.shield_kept_request),
        ("withdrawn claim loses", total.claim_lost),
        ("shielded delivery after cancel", total.shielded_delivery),
        ("delayed initialized handle", total.delayed_delivery),
        ("failed host call", total.failed_calls),
        ("timer equal to now", total.timer_equal_now),
        ("timers equal to each other", total.timer_equal_peers),
        ("timer exit", total.timer_exit),
        ("clock step housekeeping", total.clock_steps),
        ("phase-root nested tail", total.nested_tail),
        ("deferred release", total.deferred_release),
        ("flush cut", total.flush_cuts),
        ("stop at Prepared", total.stop_prepared),
        ("stop at claimed call", total.stop_claimed),
        ("stop at ended, not done", total.stop_returned),
        ("in doubt", total.in_doubt),
        ("quarantine", total.quarantined),
        ("reclaimed at teardown", total.reclaimed),
        ("discarded after stop", total.discarded_after_stop),
        ("contraction while held", total.contraction_while_held),
        ("permit wait", total.permit_waits),
        ("shrink drain", total.shrink_drains),
        (
            "mutex payload held through the stop",
            total.stopped_mutex_owner,
        ),
    ];
    let missing: Vec<_> = cells.iter().filter(|c| c.1 == 0).map(|c| c.0).collect();
    assert!(missing.is_empty(), "uncovered matrix cells: {missing:?}");
    assert!(stops > SEEDS);
}
