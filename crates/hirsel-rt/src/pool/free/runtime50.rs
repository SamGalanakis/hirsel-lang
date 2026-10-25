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
use crate::io::{AdapterError, Completion};
use crate::readiness::{Readiness, WaitSource};
use crate::tasks::free::host::concurrent::timer_hooks::Deadlines;
use crate::tasks::free::host::jobs::{self, Jobs, Owned, Taken};
use crate::tasks::registry::{ScopeKey, TaskKey};
use crate::tasks::role::Role;
use crate::tasks::runtime50::hooks::Rows;
use hirsel_free::claim::{Arbiter, Claim, Request, Requests};
use hirsel_free::mutex::{Acquired, Permit as ObjectPermit};
use hirsel_free::pending::Pending;
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::section::{Cut, Section, TaskLog};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

mod models;
mod progress;
mod reduce;

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
        generation: u64,
        op: usize,
        site: &'static str,
    },
    /// The dispatched token's concrete registry row.
    Selected {
        task: u32,
        slot: u32,
        generation: u64,
    },
    /// A scheduler-role, home or permit transition at `site`, and the
    /// holder it leaves (a role owner word, or a worker for permits).
    Handoff {
        site: &'static str,
        owner: Option<u64>,
    },
}

#[derive(Debug)]
pub(super) struct Divergence {
    pub(super) index: usize,
    pub(super) expected: Option<Entry>,
    pub(super) actual: Entry,
}

/// Where choices come from: the seed's generator, or an explicit schedule.
enum Source {
    Seeded(u64),
    /// One value per choice, in order: a random draw's value, or the index of
    /// an `event` decision's selection in its (possibly mutated) runnable
    /// order. Past its end every draw is 0 and decisions go round-robin: the
    /// count of filled decisions modulo the runnable count, so a filled tail
    /// never starves a transition that stays enabled.
    Schedule {
        choices: Vec<u64>,
        at: usize,
        filled: u64,
    },
}

/// Mutation only: a choice reads process state outside the seed, and the
/// runnable order it sees is rotated by `epoch`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Mutation {
    pub(super) epoch: u64,
    /// Only `event` decisions where a running pool host call competes.
    pub(super) host_only: bool,
}

/// Seeded choices and their checked transcript. With `expected`, the first
/// entry that differs is retained and the run ends there.
pub(super) struct Controller {
    source: Source,
    transcript: Vec<Entry>,
    expected: Option<Vec<Entry>>,
    divergence: Option<Divergence>,
    /// A scheduled choice out of its bound, at this transcript index.
    invalid: Option<usize>,
    uncontrolled: Option<Mutation>,
    first_uncontrolled: Option<usize>,
}

impl Controller {
    pub(super) fn new(
        seed: u64,
        expected: Option<Vec<Entry>>,
        uncontrolled: Option<&AtomicU64>,
    ) -> Self {
        let mutation = uncontrolled.map(|clock| Mutation {
            epoch: clock.fetch_add(1, Ordering::Relaxed),
            host_only: false,
        });
        Self::with(
            Source::Seeded(seed ^ 0x5851_f42d_4c95_7f2d),
            expected,
            mutation,
        )
    }

    /// Replay explicit choices instead of drawing them.
    pub(super) fn schedule(
        choices: &[u64],
        expected: Option<Vec<Entry>>,
        mutation: Option<Mutation>,
    ) -> Self {
        Self::with(
            Source::Schedule {
                choices: choices.to_vec(),
                at: 0,
                filled: 0,
            },
            expected,
            mutation,
        )
    }

    fn with(source: Source, expected: Option<Vec<Entry>>, uncontrolled: Option<Mutation>) -> Self {
        Self {
            source,
            transcript: Vec::new(),
            expected,
            divergence: None,
            invalid: None,
            uncontrolled,
            first_uncontrolled: None,
        }
    }

    /// The next choice below `bound`, for a decision or a random draw.
    fn draw(&mut self, bound: u64, decision: bool) -> u64 {
        match &mut self.source {
            Source::Seeded(rng) => {
                // SplitMix64, as MadSim's GlobalRng seeds its generator.
                *rng = rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
                let mut z = *rng;
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                (z ^ (z >> 31)) % bound
            }
            Source::Schedule {
                choices,
                at,
                filled,
            } => {
                let Some(&value) = choices.get(*at) else {
                    *filled += u64::from(decision);
                    return if decision { (*filled - 1) % bound } else { 0 };
                };
                *at += 1;
                if value < bound {
                    value
                } else {
                    self.invalid.get_or_insert(self.transcript.len());
                    0
                }
            }
        }
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

    /// The run ends: a checked entry differed or a scheduled choice was invalid.
    pub(super) fn halted(&self) -> bool {
        self.divergence.is_some() || self.invalid.is_some()
    }

    pub(super) fn random(&mut self, site: &'static str, bound: u64) -> u64 {
        let value = self.draw(bound, false);
        self.push(Entry::Random { site, bound, value });
        value
    }

    /// Choose one member of a canonical runnable set.
    pub(super) fn decide(&mut self, site: &'static str, mut runnable: Vec<u32>) -> Option<u32> {
        runnable.sort_unstable();
        if let Some(mutation) = self.uncontrolled
            && runnable.len() > 1
            && (!mutation.host_only || runnable.iter().any(|&e| e >> 24 == 7))
        {
            // Like iterating a randomly keyed map: an order the seed never chose.
            let by = (mutation.epoch % runnable.len() as u64) as usize;
            runnable.rotate_left(by);
            self.first_uncontrolled.get_or_insert(self.transcript.len());
        }
        let selected = (!runnable.is_empty())
            .then(|| runnable[self.draw(runnable.len() as u64, true) as usize]);
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
    Host {
        shielded: bool,
    },
    Sleep {
        domain: usize,
        after: u64,
    },
    Dup,
    Release,
    /// A synchronous home bridge while the task holds the scheduler role
    /// (`call_home_in`): home serves it by re-entering the caller's role.
    Loan,
    /// A scope whose deadline, `after` ticks on, the concrete timer index
    /// keeps; the task waits in it for expiry or cancellation.
    Deadline {
        after: u64,
    },
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
    Deadline,
}

enum Mail {
    /// The job returned: its owner takes the result from the job's slot.
    Returned,
    Canceled,
    Due,
}

/// The scope-timer index's clock id for every deadline scope.
const CLOCK: u64 = 11;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Deadline {
    scope: ScopeKey,
    at: u64,
    stage: Arm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Arm {
    /// Entered; home has not armed its timer.
    Requested,
    /// Due already at arming (a deadline equal to now).
    DueAtArm,
    /// Armed with this token, which the scope has not published.
    Armed(u64),
    Published(u64),
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
    /// Claimed: its job credit reservation waits in the pool's FIFO.
    Reserved,
    /// The credit moved into a pool job at step `at`.
    Submitted {
        at: u64,
    },
    Returned,
}

struct Job {
    request: Arc<Request<Handle>>,
    slot: u32,
    key: TaskKey,
    /// The concrete job ownership: reservation, result slot and the lease
    /// on the destination row.
    owned: Owned,
    fail: bool,
    shielded: bool,
    stage: Stage,
}

struct Task {
    /// The driver's lineage index: the frame image's task and the mutex
    /// waiter id. Each generation of the lineage is a concrete registry row.
    slot: u32,
    key: TaskKey,
    /// Which generation of its lineage, from 0.
    ordinal: u32,
    frame: Arc<Frame>,
    script: Vec<Op>,
    pc: usize,
    custody: Custody,
    /// A wake that found the segment running, before its idle save.
    remembered: bool,
    log: Arc<TaskLog>,
    wait: Option<Wait>,
    deadline: Option<Deadline>,
    /// A completion the save gate held back: the frame had not acknowledged
    /// its save, so the policy may not resume the row. The idle save's
    /// after-save callback delivers it.
    deferred: Option<(Mail, bool)>,
    job: Option<u64>,
    mail: Option<Mail>,
    mutex_queued: bool,
    pending: Pending<usize>,
    handles: Vec<Handle>,
    reuse: u32,
    /// The lineage's next generation, registered by this one's end phase
    /// and spawned into a concrete row once this one retired.
    successor: Option<Successor>,
}

struct Successor {
    script: Vec<Op>,
    log: Arc<TaskLog>,
    reuse: u32,
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
    pub(super) mutant: Option<Mutant>,
}

/// A deliberate protocol defect; the run reports what it broke.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mutant {
    /// Lineage 0's first generation never acknowledges a save.
    MissingSaveAck,
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
            mutant: None,
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
            mutant: None,
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
    /// A successor took its predecessor's retired slot at a later generation.
    pub(super) concrete_reuse: u64,
    /// A successor took another slot: the retired one still had an owner.
    pub(super) retained_reuse: u64,
    pub(super) deferred_until_save: u64,
    pub(super) loans: u64,
    pub(super) home_reentry: u64,
    pub(super) home_refused: u64,
    /// A role-needing transition waited while a loan held the role.
    pub(super) role_waits: u64,
    pub(super) cancel_at_registration: u64,
    pub(super) stale_notices: u64,
    /// A claimed request's job credit waited in the pool's FIFO.
    pub(super) credit_waits: u64,
    /// A job returned after its owner abandoned it and quarantined its result.
    pub(super) late_handles: u64,
    /// An abandoned running job kept its credit until it returned.
    pub(super) credit_held_after_abandon: u64,
    pub(super) index_equal_now: u64,
    pub(super) index_equal_peers: u64,
    /// A timer was due before its scope published the token: latched.
    pub(super) index_latched: u64,
    pub(super) index_expired: u64,
    pub(super) deadline_exit: u64,
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
            stopped_mutex_owner,
            concrete_reuse,
            retained_reuse,
            deferred_until_save,
            loans,
            home_reentry,
            home_refused,
            role_waits,
            cancel_at_registration,
            stale_notices,
            credit_waits,
            late_handles,
            credit_held_after_abandon,
            index_equal_now,
            index_equal_peers,
            index_latched,
            index_expired,
            deadline_exit
        );
    }
}

pub(super) struct Outcome {
    /// Lineages left unfinished: whether a completion waited on the save
    /// gate, and whether the row's frame had acknowledged its save.
    pub(super) stranded: Vec<(u32, bool, bool)>,
    pub(super) transcript: Vec<Entry>,
    pub(super) projection: String,
    pub(super) divergence: Option<Divergence>,
    pub(super) invalid: Option<usize>,
    pub(super) first_uncontrolled: Option<usize>,
    pub(super) coverage: Coverage,
}

struct World {
    c: Controller,
    config: Config,
    execs: Topology,
    /// The concrete registry: every generation's row, key and subscriptions.
    rows: Rows,
    /// The concrete scheduler role.
    role: Role,
    /// The worker whose home loan holds the role, and whether home served it.
    loan: Option<(usize, bool)>,
    /// The concrete owned job pool, run by the driver's pool threads.
    pool: Jobs,
    /// Jobs a pool thread took, by token, until their call returns.
    running: BTreeMap<u64, Taken>,
    /// The pool's waker fired: home has ready tokens to merge.
    pool_woken: Arc<AtomicBool>,
    /// The concrete scope-timer index.
    deadlines: Deadlines,
    armed_tokens: BTreeSet<u64>,
    due_at_arm: u64,
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

/// A task's role owner word: even and nonzero, like a context address.
fn role_owner(t: usize) -> usize {
    (t + 1) << 4
}

/// Host request tokens, as the concurrent service tags its own.
const HOST: u64 = 1 << 63;

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
        match c.random("script-op", 9) {
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
            6 => script.push(Op::Loan),
            7 => script.push(Op::Deadline {
                after: c.random("deadline", 3),
            }),
            _ => script.extend([Op::Lock, Op::Unlock]),
        }
    }
    script.push(Op::Release);
    script
}

/// A fresh row generation and its first queue token (`frame::prepared`).
fn prepare(
    slot: u32,
    key: TaskKey,
    ordinal: u32,
    script: Vec<Op>,
    log: Arc<TaskLog>,
    reuse: u32,
) -> (Task, Queued<Image>) {
    // The model's image names its lineage, not a registry row: `rows` holds
    // the concrete keys, and generation 0 is never a row's.
    let (notification, queued) = frame::prepared(Image {
        ctx: std::ptr::null_mut(),
        task: TaskKey::new(slot, 0),
        sp: std::ptr::null_mut(),
    });
    let frame = Arc::new(Frame {
        notification,
        placement: Placement::new(owner_word(Owner::Any)),
        saved: AtomicBool::new(true),
        stop_before_start: false,
    });
    let task = Task {
        slot,
        key,
        ordinal,
        frame,
        script,
        pc: 0,
        custody: Custody::Queued,
        remembered: false,
        log,
        wait: None,
        deadline: None,
        deferred: None,
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

/// Log room any one transition writes: a phase record and two root tails.
const TRANSITION: usize = 3 * 9;

/// The phase's section, or the segment ends: the stop's cut stays closed. Every task's log
/// has a transition's room reserved first, so the section allocates nothing.
macro_rules! section {
    ($world:ident, $w:expr) => {{
        if $world.arbiter.gate().unwrap().stopped() {
            return $world.terminate($w);
        }
        for task in &$world.tasks {
            // SAFETY: the driver's one thread owns every log and opens no section here.
            unsafe { task.log.reserve(TRANSITION) };
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
        let mut rows = Rows::new();
        let role = Role::default();
        role.enable();
        // Fewer credits than tasks, so reservations queue FIFO.
        let pool = Jobs::manual(2);
        let pool_woken = Arc::new(AtomicBool::new(false));
        let woken = pool_woken.clone();
        pool.set_waker(Arc::new(move || woken.store(true, Ordering::Release)));
        for slot in 0..config.tasks {
            let script = match config.scripts {
                Some(scripts) => scripts[slot as usize].to_vec(),
                None => script(&mut c),
            };
            let key = rows.spawn(slot);
            let log = TaskLog::new(&[slot, 0]);
            let (task, queued) = prepare(slot, key, 0, script, log, config.reuse);
            rows.attach(key, task.frame.clone());
            tasks.push(task);
            firsts.push(queued);
        }
        let mut world = Self {
            c,
            config,
            execs,
            rows,
            role,
            loan: None,
            pool,
            running: BTreeMap::new(),
            pool_woken,
            deadlines: Deadlines::new(),
            armed_tokens: BTreeSet::new(),
            due_at_arm: 0,
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

    fn events(&mut self) -> Vec<u32> {
        let queued = self.dispatch().queued.load(Ordering::SeqCst) != 0;
        let mut events = Vec::new();
        for (w, slot) in self.slots.iter().enumerate() {
            // An idle save and a segment's operation take the role; while a
            // loan holds it they wait. Search and permits never take it.
            let needs_role = slot.leaving.is_some() || (slot.active.is_some() && !slot.permit_wait);
            let enabled = match self.loan {
                Some((loaner, served)) if loaner == w => served,
                Some(_) if needs_role => {
                    self.coverage.role_waits += 1;
                    false
                }
                _ if slot.leaving.is_some() => true,
                _ if slot.active.is_some() => {
                    !slot.permit_wait || self.execs.permits.available() > 0
                }
                _ => queued,
            };
            if enabled {
                events.push(w as u32);
            }
        }
        for (&id, job) in &self.jobs {
            if job.stage == Stage::Posted {
                events.push(1 << 24 | id as u32);
            }
        }
        if self.pool_woken.load(Ordering::Acquire) {
            events.push(2 << 24);
        }
        if self.pool.queued() > 0 {
            events.push(6 << 24);
        }
        for &token in self.running.keys() {
            events.push(7 << 24 | token as u32);
        }
        // After the terminal cut home merges no timer: teardown clears them,
        // and a stopped segment may retire inside its deadline scope.
        for task in self.tasks.iter().filter(|_| !self.stopped) {
            match task.deadline.map(|d| d.stage) {
                Some(Arm::Requested) => events.push(9 << 24 | task.slot),
                Some(Arm::Armed(_)) => events.push(10 << 24 | task.slot),
                _ => {}
            }
        }
        if self.deadlines.watching() && !self.stopped {
            events.push(11 << 24);
        }
        if !self.stopped
            && (self.deadlines.due() > 0
                || self
                    .tasks
                    .iter()
                    .any(|t| t.deadline.is_some_and(|d| d.stage == Arm::DueAtArm)))
        {
            events.push(12 << 24);
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
        if self.rows.notices() > 0 {
            events.push(4 << 24);
        }
        if matches!(self.loan, Some((_, false))) {
            events.push(5 << 24);
        }
        // The stop's teardown may begin while pool calls still run.
        if self.stopped
            && (!self.running.is_empty() || self.pool.queued() > 0)
            && !events.iter().any(|e| e >> 24 == 0)
        {
            events.push(13 << 24);
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
            if self.c.halted() {
                break;
            }
            let Some(event) = event else { break };
            if event >> 24 == 13 {
                break;
            }
            let arg = (event & 0xff_ffff) as usize;
            match event >> 24 {
                0 => self.worker(arg),
                5 => self.serve_loan(),
                // Pool threads and home's adapter calls run outside the role.
                6 => {
                    let taken = self.pool.take().unwrap();
                    assert!(self.running.insert(taken.token(), taken).is_none());
                }
                7 => self.running.remove(&(arg as u64)).unwrap().run(),
                9 => self.arm(arg),
                11 => self.poll_deadlines(),
                home => {
                    // Home's merges take the role by its nonblocking idle claim.
                    if !self.role.driven_try_idle() {
                        self.coverage.home_refused += 1;
                        let owner = self.role.driven_owner().map(|o| o as u64);
                        self.c.push(Entry::Handoff {
                            site: "home-refused",
                            owner,
                        });
                    } else {
                        self.handoff("home", Some(Role::IDLE));
                        match home {
                            1 => self.claim(arg as u64),
                            2 => self.merge_jobs(),
                            3 => self.clock(arg),
                            4 => self.cancellations(),
                            10 => self.publish_deadline(arg),
                            _ => self.expire_deadlines(),
                        }
                        self.release(Role::IDLE);
                    }
                }
            }
            if self.c.halted() {
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
            // Another task's cancellation runs under its own role entry.
            2 if self.loan.is_none() => {
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
        } else if let Some((_, served)) = self.loan.take_if(|l| l.0 == w) {
            // The bridge returned: its caller leaves the role it held.
            assert!(served);
            let t = self.running(w);
            self.tasks[t].pc += 1;
            self.release(role_owner(t));
        } else {
            // One semantic entry: the operation holds the role throughout.
            let owner = role_owner(self.running(w));
            self.role.driven_enter(owner);
            self.handoff("enter", Some(owner));
            self.operate(w);
            if self.loan.is_none() {
                self.release(owner);
            }
        }
    }

    fn handoff(&mut self, site: &'static str, owner: Option<usize>) {
        self.c.push(Entry::Handoff {
            site,
            owner: owner.map(|o| o as u64),
        });
    }

    fn release(&mut self, owner: usize) {
        assert!(
            self.role.driven_release(owner),
            "{owner:#x} did not hold the role"
        );
        self.handoff("release", None);
    }

    /// Home serves the loan: it re-enters its caller's role without an
    /// acquisition and reads the caller's row, as `call_home_in` does.
    fn serve_loan(&mut self) {
        let (w, served) = self.loan.as_mut().unwrap();
        assert!(!*served);
        *served = true;
        let t = self.slots[*w].active.as_ref().unwrap().0.value().task.slot as usize;
        let owner = role_owner(t);
        let before = self.role.acquisitions();
        self.role.driven_enter(owner);
        assert_eq!(
            self.role.acquisitions(),
            before,
            "re-entry is no acquisition"
        );
        assert!(
            !self.role.driven_try_idle(),
            "home's idle claim waits for the loan"
        );
        assert!(self.rows.live(self.tasks[t].key));
        self.coverage.home_reentry += 1;
        self.handoff("home-reenter", Some(owner));
    }

    fn acquire(&mut self, w: usize) {
        let slot = &mut self.slots[w];
        if !slot.exec.held.load(Ordering::Acquire) && self.execs.permits.available() == 0 {
            if !slot.permit_wait {
                self.coverage.permit_waits += 1;
            }
            slot.permit_wait = true;
            self.handoff("permit-wait", Some(w));
            return;
        }
        assert!(self.execs.permits.acquire(&slot.exec, None, None).is_ok());
        slot.permit_wait = false;
        self.handoff("permit", Some(w));
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
        let slot = running.value().task.slot;
        let task = &mut self.tasks[slot as usize];
        assert!(
            Arc::ptr_eq(&frame, &task.frame),
            "an old generation's token reached a worker"
        );
        assert_eq!(task.custody, Custody::Queued, "dispatch duplicated custody");
        assert!(self.rows.live(task.key), "a token outlived its row");
        self.c.observe("dispatch", runnable, Some(slot));
        self.c.push(Entry::Selected {
            task: slot,
            slot: task.key.slot,
            generation: task.key.generation,
        });
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
        self.slots[w].active.as_ref().unwrap().0.value().task.slot as usize
    }

    fn leave(&mut self, w: usize, yielded: bool, site: &'static str) {
        let t = self.running(w);
        let task = &self.tasks[t];
        self.c.push(Entry::Yield {
            task: task.slot,
            generation: task.key.generation,
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

    /// pool::free::saved: the idle continuation after the switch. Its
    /// after-save callback runs under the role's idle guard.
    fn idle_save(&mut self, w: usize) {
        let leaving = self.slots[w].leaving.take().unwrap();
        let (running, frame, lease) = self.slots[w].active.take().unwrap();
        let exec = self.slots[w].exec.clone();
        self.execs.permits.release(&exec);
        self.handoff("permit-release", Some(w));
        let t = running.value().task.slot as usize;
        let key = self.tasks[t].key;
        self.role.driven_idle();
        self.handoff("idle", Some(Role::IDLE));
        match leaving {
            Leave::Complete => {
                drop(running);
                // after_save(completed): the save acknowledgement, then the
                // policy retires the row. Its slot waits for every owner.
                frame.mark_saved();
                self.rows.retire(key);
                assert!(!self.rows.live(key));
                self.release(Role::IDLE);
                exec.busy.store(false, Ordering::SeqCst);
                drop(lease);
                drop(frame);
                self.tasks[t].custody = Custody::Done;
                // Completion invalidates every later wake of this generation.
                assert!(self.tasks[t].frame.notification.notify().is_none());
                self.coverage.completed_wake_inert += 1;
                if let Some(next) = self.tasks[t].successor.take() {
                    self.spawn_successor(t, next);
                }
            }
            Leave::Compact => unreachable!("the protocol model saves no compact frame"),
            Leave::Suspend { yielded, .. } => {
                frame.placement.set_owner(owner_word(Owner::Any));
                let unacknowledged = self.config.mutant == Some(Mutant::MissingSaveAck)
                    && t == 0
                    && self.tasks[t].ordinal == 0;
                if !unacknowledged {
                    frame.mark_saved();
                }
                // after_save re-polls what the save gate deferred. The frame
                // is still running, so the wake is remembered for the save.
                if self.rows.saved(key)
                    && let Some((mail, host)) = self.tasks[t].deferred.take()
                {
                    if host {
                        self.rows.host_done(key);
                    }
                    self.tasks[t].mail = Some(mail);
                    self.notify(t);
                }
                self.release(Role::IDLE);
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

    /// The lineage's next generation takes a concrete row: its
    /// predecessor's slot at a later generation once no owner retains it.
    fn spawn_successor(&mut self, t: usize, next: Successor) {
        let old = self.tasks[t].key;
        let ordinal = self.tasks[t].ordinal + 1;
        let (task, queued) = prepare(t as u32, old, ordinal, next.script, next.log, next.reuse);
        let previous = std::mem::replace(&mut self.tasks[t], task);
        self.stale.push(previous.frame.notification.clone());
        drop(previous);
        let key = self.rows.spawn(t as u32);
        assert!(!self.rows.live(old), "a retired key names a later occupant");
        if key.slot == old.slot {
            assert!(key.generation > old.generation);
            self.coverage.concrete_reuse += 1;
        } else if self.rows.retained().contains(&old.slot) {
            self.coverage.retained_reuse += 1;
        }
        self.tasks[t].key = key;
        self.rows.attach(key, self.tasks[t].frame.clone());
        self.coverage.reused_rows += 1;
        self.publish_first(t, queued);
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
            Op::Deadline { after } => self.deadline(w, t, index, after),
            Op::Loan => {
                self.loan = Some((w, false));
                self.coverage.loans += 1;
                self.handoff("loan", Some(role_owner(t)));
            }
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
            // After the terminal cut the owner never takes the result: the
            // stop's teardown abandons the job, which quarantines it.
            let section = section!(self, w);
            let canceled = self.rows.canceled(self.tasks[t].key);
            let id = self.tasks[t].job.take().unwrap();
            match mail {
                Mail::Returned => {
                    let mut job = self.jobs.remove(&id).unwrap();
                    assert_eq!(job.stage, Stage::Returned);
                    let slot = job.owned.slot.take().unwrap();
                    let output = slot.deliver().expect("the owner wins delivery");
                    job.request.finish();
                    self.requests.retire(id);
                    match output {
                        (Ok(Completion::Unit(Ok(()))), Some(handle)) => {
                            assert!(self.handles.insert(handle));
                            self.tasks[t].handles.push(Handle {
                                id: handle,
                                drops: self.handle_drops.clone(),
                            });
                        }
                        (Err(AdapterError::Blocked), None) => self.coverage.failed_calls += 1,
                        _ => unreachable!("the job returned what it was given"),
                    }
                    if shielded && canceled {
                        self.coverage.shielded_delivery += 1;
                    }
                }
                Mail::Canceled => assert!(self.withdrawn.contains(&id)),
                Mail::Due => unreachable!("a host wait received a timer"),
            }
            effect(&section, &self.tasks[t].log, index, Phase::Done, DONE);
            self.tasks[t].pc += 1;
            return;
        }
        if self.tasks[t].wait.is_some() || self.tasks[t].deferred.is_some() {
            self.coverage.spurious_repark += 1;
            return self.leave(w, false, "host-repark");
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
        let key = self.tasks[t].key;
        effect(&section, &self.tasks[t].log, index, Phase::Entered, ENTERED);
        // Parking subscribes the request to the task's cancellation, with
        // the post-registration recheck.
        let registration = self.rows.host_wait(key, HOST | id, shielded);
        assert_eq!(registration.key, key);
        if registration.canceled.is_some() {
            // Canceled before it parked: the owner withdraws its own
            // Prepared request, which no home can have claimed.
            assert!(!shielded && request.withdraw());
            assert!(self.withdrawn.insert(id));
            self.requests.retire(id);
            self.rows.host_done(key);
            self.coverage.cancel_at_registration += 1;
            effect(&section, &self.tasks[t].log, index, Phase::Done, DONE);
            self.tasks[t].pc += 1;
            return;
        }
        drop(section);
        let owned = Owned {
            reservation: None,
            slot: None,
            destination: self.rows.lease(key),
        };
        let task = &mut self.tasks[t];
        self.jobs.insert(
            id,
            Job {
                request,
                slot: task.slot,
                key,
                owned,
                fail: false,
                shielded,
                stage: Stage::Posted,
            },
        );
        task.wait = Some(Wait::Host(id));
        task.job = Some(id);
        self.leave(w, false, "host");
    }

    /// The policy's delivery of an ended wait. A parked row whose frame has
    /// not acknowledged its save is not resumable (`readiness::saved`): the
    /// completion waits for the idle save's after-save callback.
    fn deliver(&mut self, t: usize, mail: Mail) {
        let key = self.tasks[t].key;
        let host = matches!(self.tasks[t].wait.take(), Some(Wait::Host(_)));
        if !self.rows.saved(key) {
            assert!(self.tasks[t].deferred.replace((mail, host)).is_none());
            self.coverage.deferred_until_save += 1;
            return;
        }
        if host {
            self.rows.host_done(key);
        }
        self.tasks[t].mail = Some(mail);
        self.notify(t);
    }

    fn sleep(&mut self, w: usize, t: usize, index: u64, domain: usize, after: u64) {
        if let Some(mail) = self.tasks[t].mail.take() {
            assert!(matches!(mail, Mail::Due | Mail::Canceled));
            let section = section!(self, w);
            effect(&section, &self.tasks[t].log, index, Phase::Done, DONE);
            self.tasks[t].pc += 1;
            return;
        }
        if self.tasks[t].wait.is_some() || self.tasks[t].deferred.is_some() {
            self.coverage.spurious_repark += 1;
            return self.leave(w, false, "timer-repark");
        }
        if self.rows.canceled(self.tasks[t].key) {
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
            let log = TaskLog::new(&[task.slot, task.ordinal + 1]);
            section.register(&log);
            Successor {
                script,
                log,
                reuse: task.reuse - 1,
            }
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

    /// The home claims a posted request by CAS (claim.rs); the claim winner
    /// reserves one job credit before the call is issued.
    fn claim(&mut self, id: u64) {
        let job = self.jobs.get_mut(&id).unwrap();
        if job.request.claim() {
            assert!(self.claimed.insert(id));
            job.fail = self.c.random("host-fail", 6) == 0;
            job.owned.reservation = Some(self.pool.reserve(id));
            job.stage = Stage::Reserved;
            self.submit(id);
        } else {
            assert_eq!(job.request.state(), Claim::Withdrawn);
            assert!(self.withdrawn.contains(&id));
            self.jobs.remove(&id);
            self.coverage.claim_lost += 1;
        }
    }

    /// A granted credit moves into the job a pool thread will run; its
    /// result is an initialized handle, or a failed call.
    fn submit(&mut self, id: u64) {
        let job = self.jobs.get_mut(&id).unwrap();
        if !job.owned.reservation.as_mut().unwrap().granted() {
            self.coverage.credit_waits += 1;
            return;
        }
        let credit = job.owned.reservation.take().unwrap().take().unwrap();
        let output: jobs::Output = if job.fail {
            (Err(AdapterError::Blocked), None)
        } else {
            self.next_id += 1;
            (Ok(Completion::Unit(Ok(()))), Some(self.next_id))
        };
        job.owned.slot = Some(self.pool.submit(credit, id, move || output));
        job.stage = Stage::Submitted { at: self.step };
    }

    /// Home merges the pool's ready tokens in publication order: a granted
    /// reservation submits; a returned call wakes its owner, which takes the
    /// result itself (the single delivery winner).
    fn merge_jobs(&mut self) {
        self.pool_woken.store(false, Ordering::Release);
        for id in self.pool.drain() {
            let Some(job) = self.jobs.get_mut(&id) else {
                continue;
            };
            match job.stage {
                // After the stop no new call is issued; teardown returns it.
                Stage::Reserved if !self.stopped => self.submit(id),
                Stage::Submitted { at } if job.owned.slot.as_ref().unwrap().returned() => {
                    if self.step > at + 2 {
                        self.coverage.delayed_delivery += 1;
                    }
                    job.stage = Stage::Returned;
                    let t = job.slot as usize;
                    assert_eq!(
                        self.tasks[t].key, job.key,
                        "a completion reached a reused row"
                    );
                    assert!(self.rows.live(job.key));
                    assert_eq!(self.tasks[t].wait, Some(Wait::Host(id)));
                    if !self.stopped {
                        self.deliver(t, Mail::Returned);
                    }
                }
                _ => {}
            }
        }
    }

    /// The task enters a deadline scope and waits in it; home arms the
    /// concrete timer index outside the role.
    fn deadline(&mut self, w: usize, t: usize, index: u64, after: u64) {
        if let Some(mail) = self.tasks[t].mail.take() {
            assert!(matches!(mail, Mail::Due | Mail::Canceled));
            let section = section!(self, w);
            // The scope exits: its timer is disarmed unless it already fired.
            let Deadline { scope, stage, .. } = self.tasks[t].deadline.take().unwrap();
            if let Arm::Armed(token) | Arm::Published(token) = stage {
                self.deadlines.disarm(token);
            }
            self.rows.close_deadline(self.tasks[t].key, scope);
            effect(&section, &self.tasks[t].log, index, Phase::Done, DONE);
            self.tasks[t].pc += 1;
            return;
        }
        if self.tasks[t].wait.is_some() || self.tasks[t].deferred.is_some() {
            self.coverage.spurious_repark += 1;
            return self.leave(w, false, "deadline-repark");
        }
        let key = self.tasks[t].key;
        if self.rows.canceled(key) {
            self.tasks[t].pc += 1;
            return;
        }
        let section = section!(self, w);
        let scope = self.rows.open_deadline(key);
        effect(&section, &self.tasks[t].log, index, Phase::Entered, ENTERED);
        drop(section);
        self.tasks[t].deadline = Some(Deadline {
            scope,
            at: self.deadlines.now() + after,
            stage: Arm::Requested,
        });
        self.tasks[t].wait = Some(Wait::Deadline);
        self.leave(w, false, "deadline");
    }

    /// Home arms the scope's timer (`Service::arm`).
    fn arm(&mut self, t: usize) {
        let deadline = self.tasks[t].deadline.as_mut().unwrap();
        deadline.stage = match self.deadlines.arm(deadline.scope, CLOCK, deadline.at) {
            Some(token) => {
                assert!(self.armed_tokens.insert(token));
                Arm::Armed(token)
            }
            None => {
                // Inclusive equality: a deadline at or before now is due.
                assert!(deadline.at <= self.deadlines.now());
                if deadline.at == self.deadlines.now() {
                    self.coverage.index_equal_now += 1;
                }
                self.due_at_arm += 1;
                Arm::DueAtArm
            }
        };
    }

    /// The scope publishes its armed token under the role.
    fn publish_deadline(&mut self, t: usize) {
        let deadline = self.tasks[t].deadline.as_mut().unwrap();
        let Arm::Armed(token) = deadline.stage else {
            unreachable!()
        };
        self.rows.publish_deadline(deadline.scope, token);
        deadline.stage = Arm::Published(token);
    }

    /// Home's readiness poll: the index latches each due timer.
    fn poll_deadlines(&mut self) {
        let ready = self.deadlines.step();
        let mut ats: Vec<u64> = ready.iter().map(|&(_, at)| at).collect();
        ats.sort_unstable();
        if ats.windows(2).any(|pair| pair[0] == pair[1]) {
            self.coverage.index_equal_peers += 1;
        }
        for (token, _) in ready {
            if self
                .tasks
                .iter()
                .any(|task| task.deadline.is_some_and(|d| d.stage == Arm::Armed(token)))
            {
                self.coverage.index_latched += 1;
            }
        }
    }

    /// Home's expiry merge under the role: published due timers expire
    /// their scopes, which wakes each owner; unpublished ones stay latched.
    fn expire_deadlines(&mut self) {
        for t in 0..self.tasks.len() {
            if let Some(deadline) = self.tasks[t].deadline.as_mut()
                && deadline.stage == Arm::DueAtArm
            {
                deadline.stage = Arm::Due;
                let scope = deadline.scope;
                self.rows.expire_now(scope);
                self.wake_expired(t);
            }
        }
        let deadlines = &self.deadlines;
        self.rows.merge(|s| deadlines.expire(s));
        for t in 0..self.tasks.len() {
            if let Some(deadline) = self.tasks[t].deadline.as_mut()
                && let Arm::Published(_) = deadline.stage
                && self.rows.expired(deadline.scope)
            {
                deadline.stage = Arm::Due;
                self.coverage.index_expired += 1;
                self.wake_expired(t);
            }
        }
    }

    /// Expiry wakes an owner still waiting in its scope; a canceled one
    /// already left the wait and only exits.
    fn wake_expired(&mut self, t: usize) {
        if self.tasks[t].wait == Some(Wait::Deadline) {
            self.deliver(t, Mail::Due);
        }
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
            self.deliver(t, Mail::Due);
        }
        ats.sort_unstable();
        if ats.windows(2).any(|pair| pair[0] == pair[1]) {
            self.coverage.timer_equal_peers += 1;
        }
    }

    /// A cancellation through the concrete row: its host subscription
    /// queues a notice for home's merge; a timer wait exits at once.
    fn cancel(&mut self, t: usize) {
        let key = self.tasks[t].key;
        if !self.rows.live(key) || !self.rows.cancel(key) {
            return;
        }
        match self.tasks[t].wait {
            Some(Wait::Host(id)) => {
                if self.jobs.get(&id).is_some_and(|job| job.shielded) {
                    self.coverage.shield_kept_request += 1;
                }
            }
            Some(Wait::Timer { domain, token, .. }) => {
                // The timer exits: a later clock step never reports it.
                self.domains[domain].finish(token);
                self.coverage.timer_exit += 1;
                self.deliver(t, Mail::Canceled);
            }
            Some(Wait::Deadline) => {
                self.coverage.deadline_exit += 1;
                self.deliver(t, Mail::Canceled);
            }
            // Mutex take is not a cancellation point (CONC-3).
            Some(Wait::Mutex) | None => {}
        }
    }

    /// Home drains the notices addressed to host requests. The concrete
    /// registration check drops one after delivery, retry or row reuse.
    fn cancellations(&mut self) {
        let queued = self.rows.notices();
        let tokens = self.rows.host_cancellations(HOST, HOST);
        self.coverage.stale_notices += (queued - tokens.len()) as u64;
        if self.stopped {
            // The stop's sweep already decided every request.
            return;
        }
        for token in tokens {
            let id = token & !HOST;
            let job = &self.jobs[&id];
            let t = job.slot as usize;
            assert_eq!(self.tasks[t].key, job.key);
            assert!(!job.shielded, "a shielded wait is not canceled");
            if job.stage == Stage::Posted && job.request.withdraw() {
                // Exactly one of the home's claim and this withdraw wins.
                assert!(self.withdrawn.insert(id));
                self.requests.retire(id);
                self.jobs.remove(&id);
                self.coverage.cancel_withdrew += 1;
                self.deliver(t, Mail::Canceled);
            } else {
                self.coverage.claim_beat_cancel += 1;
            }
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
                Stage::Reserved | Stage::Submitted { .. } => self.coverage.stop_claimed += 1,
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
                Stage::Reserved | Stage::Submitted { .. } | Stage::Returned
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
                let task = &self.tasks[running.value().task.slot as usize];
                assert_eq!(task.custody, Custody::Running(w));
                assert!(Arc::ptr_eq(frame, &task.frame));
            }
        }
        for task in &self.tasks {
            if let Custody::Running(w) = task.custody {
                assert!(self.slots[w].active.is_some());
            }
            // A token discarded after the stop never retires its row.
            if task.custody != Custody::Done || !self.stopped {
                assert_eq!(
                    self.rows.live(task.key),
                    task.custody != Custody::Done,
                    "a lineage's current generation is exactly its live row"
                );
            }
        }
        // One role owner: between transitions only a loan's caller holds it.
        let loaner = self.loan.map(|(w, _)| {
            role_owner(self.slots[w].active.as_ref().unwrap().0.value().task.slot as usize)
        });
        assert_eq!(
            self.role.driven_owner(),
            loaner,
            "the role outlived its entry"
        );
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
        let complete = divergence.is_none() && self.c.invalid.is_none();
        if complete {
            if self.stopped {
                assert!(self.arbiter.gate().unwrap().stopped());
            } else if self.config.mutant.is_some() {
                // The defect's report: what it stranded.
            } else {
                for task in &self.tasks {
                    assert_eq!(task.custody, Custody::Done, "task {} stranded", task.slot);
                    assert_eq!(task.reuse, 0);
                }
                assert_eq!(self.roots, [0, 0]);
                assert_eq!(self.freed, [1, 1], "each root walked exactly once");
                assert_eq!(self.mutex_owner, None);
                assert!(self.jobs.is_empty());
                assert_eq!(self.pool.stats().quarantined.load(Ordering::Relaxed), 0);
                assert_eq!(self.loan, None);
                assert!(!self.deadlines.pending());
                // Each timer finished exactly once: at arming when already
                // due, else by its expiry or its disarm.
                let finishes = self.deadlines.finishes();
                let finished: BTreeSet<u64> = finishes.iter().copied().collect();
                assert_eq!(finished.len(), finishes.len(), "a timer finished twice");
                assert!(self.armed_tokens.is_subset(&finished));
                assert_eq!(
                    (finished.len() - self.armed_tokens.len()) as u64,
                    self.due_at_arm
                );
            }
            assert_eq!(self.dispatch().queued.load(Ordering::SeqCst), 0);
        }
        let _ = writeln!(
            self.projection,
            "end roots {:?} freed {:?} closed {} claimed {:?} withdrawn {:?}",
            self.roots, self.freed, self.closed, self.claimed, self.withdrawn
        );
        for task in &self.tasks {
            let _ = writeln!(
                self.projection,
                "task {} gen {} {:?} pc {}",
                task.slot, task.ordinal, task.custody, task.pc
            );
        }
        // Teardown: running owners complete, queued tokens are discarded,
        // parked rows and undelivered mail are reclaimed.
        let stranded = self
            .tasks
            .iter()
            .filter(|task| task.custody != Custody::Done)
            .map(|task| {
                (
                    task.slot,
                    task.deferred.is_some(),
                    self.rows.saved(task.key),
                )
            })
            .collect();
        let reclaimable = self.handles.len() as u64 - self.closed;
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
            mut coverage,
            pool,
            running,
            ..
        } = self;
        drop(slots);
        drop(tasks);
        // The owners leave: each undelivered job is abandoned, quarantining
        // a returned result. A running call keeps its credit until it
        // returns and then quarantines its own late result.
        let quarantined = |pool: &Jobs| pool.stats().quarantined.load(Ordering::Relaxed);
        drop(jobs);
        let abandoned = quarantined(&pool);
        if pool.held() > 0 && !running.is_empty() {
            coverage.credit_held_after_abandon += 1;
        }
        for (_, taken) in running {
            taken.run();
        }
        while let Some(taken) = pool.take() {
            taken.run();
        }
        let after = quarantined(&pool);
        coverage.quarantined += after;
        coverage.late_handles += after - abandoned;
        drop(requests);
        drop(execs);
        if complete {
            assert_eq!(pool.held(), 0, "a credit outlived its call");
            let stats = pool.stats();
            let count = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
            assert_eq!(
                count(&stats.returned),
                count(&stats.delivered) + count(&stats.quarantined),
                "every returned result was delivered or quarantined once"
            );
            assert_eq!(count(&stats.reclaimed), count(&stats.quarantined));
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
            stranded,
            invalid: c.invalid,
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
    // The simulated world keeps its own tables beside the real sections it drives from this
    // one thread; the production section paths keep the trap (hirsel-free, record.rs tests).
    hirsel_guard::simulated_sections(|| {
        World::new(config, Controller::new(config.seed, expected, uncontrolled)).run()
    })
}

/// Run `config` from explicit choices (`Controller::schedule`) instead of its seed.
pub(super) fn replay(
    config: Config,
    choices: &[u64],
    expected: Option<Vec<Entry>>,
    mutation: Option<Mutation>,
) -> Outcome {
    // As `run`: the simulation's own bookkeeping sits beside the real sections it drives.
    hirsel_guard::simulated_sections(|| {
        World::new(config, Controller::schedule(choices, expected, mutation)).run()
    })
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
                && first
                    .transcript
                    .iter()
                    .any(|e| matches!(e, Entry::Selected { .. }))
                && first
                    .transcript
                    .iter()
                    .any(|e| matches!(e, Entry::Handoff { site: "idle", .. }))
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
        let mut rows = Rows::new();
        let key = rows.spawn(0);
        let (task, first) = prepare(0, key, 0, Vec::new(), TaskLog::new(&[0, 0]), 0);
        rows.attach(key, task.frame.clone());
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
        let (reused, queued) = prepare(0, key, 1, Vec::new(), TaskLog::new(&[0, 1]), 0);
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

#[test]
fn runtime50_missing_save_ack_makes_resumption_impossible() {
    // Lineage 0's first generation parks without acknowledging its save.
    // Every completion addressed to it waits on the concrete save gate and
    // is never delivered: the row stays live, unsaved and unfinished.
    let (mut gated, mut runs) = (0, 0);
    for seed in 0..SEEDS {
        let config = Config {
            stop: false,
            mutant: Some(Mutant::MissingSaveAck),
            ..Config::of(seed)
        };
        let outcome = run(config, None, None);
        let Some(&(_, deferred, saved)) = outcome.stranded.iter().find(|s| s.0 == 0) else {
            continue;
        };
        runs += 1;
        if deferred {
            assert!(
                !saved,
                "seed {seed}: a deferred completion's row acknowledged its save"
            );
            gated += 1;
        }
        // The same seed without the defect finishes every lineage.
        let positive = run(
            Config {
                stop: false,
                ..Config::of(seed)
            },
            None,
            None,
        );
        assert!(positive.stranded.is_empty(), "seed {seed}");
    }
    assert!(gated > 0, "no completion reached the unacknowledged row");
    eprintln!("RUNTIME50 missing save ack: {runs} stranded runs, {gated} with a gated completion");
}

#[test]
fn runtime50_forced_concrete_vectors() {
    // Each race has a scripted topology. The first seed whose schedule hits
    // its concrete hook is replayed against its checked transcript; every
    // step of both runs checks single custody of rows, tokens, permits, the
    // role and the mutex.
    const HOST: &[Op] = &[Op::Host { shielded: false }, Op::Release];
    const HOSTS: &[Op] = &[
        Op::Host { shielded: false },
        Op::Host { shielded: false },
        Op::Release,
    ];
    const SHORT: &[Op] = &[Op::Compute, Op::Release];
    const DEADLINE: &[Op] = &[Op::Deadline { after: 1 }, Op::Release];
    const NOW: &[Op] = &[Op::Deadline { after: 0 }, Op::Release];
    let scripted = |scripts: &'static [&'static [Op]], reuse, stop| {
        move |seed| Config {
            seed,
            tasks: scripts.len() as u32,
            reuse,
            stop,
            scripts: Some(scripts),
            hold_clock: 0,
            mutant: None,
        }
    };
    type Hit = fn(&Coverage) -> bool;
    let vectors: [(&str, Box<dyn Fn(u64) -> Config>, Hit); 8] = [
        (
            "wake before save: the save gate defers a completion",
            Box::new(scripted(&[HOST, HOST, HOST], 0, false)),
            |c| c.deferred_until_save > 0,
        ),
        (
            "reused row: a successor takes the retired slot",
            Box::new(scripted(&[SHORT, SHORT], 2, false)),
            |c| c.concrete_reuse > 0 && c.stale_generation_inert > 0,
        ),
        (
            "claim cut: the stop leaves a claimed call in doubt",
            Box::new(scripted(&[HOST, HOST, HOST], 0, true)),
            |c| c.stop_claimed > 0 && c.in_doubt > 0,
        ),
        (
            "late initialized handle: an abandoned call returns into quarantine",
            Box::new(scripted(&[HOST, HOST, HOST], 0, true)),
            |c| c.late_handles > 0 && c.credit_held_after_abandon > 0,
        ),
        (
            "timer equality: equal deadlines expire in one index poll",
            Box::new(scripted(&[DEADLINE, DEADLINE], 0, false)),
            |c| c.index_equal_peers > 0 && c.index_expired > 1,
        ),
        (
            "timer equality: a deadline equal to now is due at arming",
            Box::new(scripted(&[NOW, SHORT], 0, false)),
            |c| c.index_equal_now > 0,
        ),
        (
            "subscription race: cancellation before registration",
            Box::new(scripted(&[HOSTS, HOSTS], 0, false)),
            |c| c.cancel_at_registration > 0,
        ),
        (
            "subscription race: a delayed notice after the wait ended",
            Box::new(scripted(&[HOSTS, HOSTS], 0, false)),
            |c| c.stale_notices > 0 && c.cancel_withdrew + c.claim_beat_cancel > 0,
        ),
    ];
    for (name, config, hit) in vectors {
        let (seed, first) = (0..512)
            .map(|seed| (seed, run(config(seed), None, None)))
            .find(|(_, outcome)| hit(&outcome.coverage))
            .unwrap_or_else(|| panic!("no schedule forced {name}"));
        let again = run(config(seed), Some(first.transcript.clone()), None);
        assert!(again.divergence.is_none(), "{name}: {:?}", again.divergence);
        assert!(hit(&again.coverage), "{name}");
        assert_eq!(first.projection, again.projection);
        eprintln!(
            "RUNTIME50 forced {name}: seed {seed}, {} entries",
            first.transcript.len()
        );
    }
}
