//! Compile timings for `--timings` (docs/building.md, "Compile timings").
//! Three measures, kept apart:
//! - [`Interval`]: wall time of the driver's non-overlapping intervals. A
//!   nested interval pauses the one it interrupts, so intervals add up to the
//!   wall time they cover. Only intervals are additive.
//! - [`Phase`]: work time (wall) and CPU time inside each phase, summed over
//!   every thread. A nested phase's time is its own, not its parent's.
//! - Wait time: time a thread spends waiting for others ([`wait`]).
//!
//! Off by default: every guard then costs one relaxed atomic load and reads
//! no clock. Totals are process-wide atomics, so worker threads need no flush.
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// A driver interval. Today's driver maps onto them as docs/building.md
/// describes; an interval it has no step for stays 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Interval {
    S0,
    Parallel,
    S1,
    Parallel2,
    Merge,
    Plan,
    Units,
    Link,
    ImageOpen,
    /// Serial preparation of immutable per-definition worker inputs.
    FrontPrepare,
    /// Whole-program generic-instance growth checking.
    GenericGraph,
}

impl Interval {
    /// Every interval, in pipeline order.
    pub const ALL: [Interval; 11] = [
        Interval::ImageOpen,
        Interval::S0,
        Interval::FrontPrepare,
        Interval::Parallel,
        Interval::GenericGraph,
        Interval::S1,
        Interval::Parallel2,
        Interval::Merge,
        Interval::Plan,
        Interval::Units,
        Interval::Link,
    ];
    /// The stable name `--timings` prints.
    pub fn name(self) -> &'static str {
        match self {
            Interval::S0 => "S0",
            Interval::Parallel => "parallel",
            Interval::S1 => "S1",
            Interval::Parallel2 => "parallel2",
            Interval::Merge => "merge",
            Interval::Plan => "plan",
            Interval::Units => "units",
            Interval::Link => "link",
            Interval::ImageOpen => "image open",
            Interval::FrontPrepare => "front prepare",
            Interval::GenericGraph => "GEN-5",
        }
    }
}

/// A unit of compiler work, timed on whichever thread runs it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Phase {
    Parse,
    Resolve,
    Signatures,
    Check,
    Modes,
    Lower,
    Verify,
    Comptime,
    Plan,
    Import,
    Place,
    Rc,
    Reuse,
    Ready,
    LirValidate,
    Codegen,
    StoreLookup,
    /// The link's debug object (line tables from the objects' debug intervals).
    DebugObject,
    /// Tokenization alone, excluding parser and declaration publication.
    Lex,
    /// Declaration indexing, including its signature demands.
    Declarations,
}

impl Phase {
    /// Every phase, in declaration order.
    pub const ALL: [Phase; 20] = [
        Phase::Parse,
        Phase::Resolve,
        Phase::Signatures,
        Phase::Check,
        Phase::Modes,
        Phase::Lower,
        Phase::Verify,
        Phase::Comptime,
        Phase::Plan,
        Phase::Import,
        Phase::Place,
        Phase::Rc,
        Phase::Reuse,
        Phase::Ready,
        Phase::LirValidate,
        Phase::Codegen,
        Phase::StoreLookup,
        Phase::DebugObject,
        Phase::Lex,
        Phase::Declarations,
    ];
    /// The stable name `--timings` prints.
    pub fn name(self) -> &'static str {
        match self {
            Phase::Parse => "parse",
            Phase::Resolve => "resolve",
            Phase::Signatures => "signatures",
            Phase::Check => "check",
            Phase::Modes => "modes",
            Phase::Lower => "lower",
            Phase::Verify => "verify",
            Phase::Comptime => "comptime",
            Phase::Plan => "plan",
            Phase::Import => "import",
            Phase::Place => "place",
            Phase::Rc => "rc",
            Phase::Reuse => "reuse",
            Phase::Ready => "ready",
            Phase::LirValidate => "lir validate",
            Phase::Codegen => "codegen",
            Phase::StoreLookup => "store lookup",
            Phase::DebugObject => "debug object",
            Phase::Lex => "lex",
            Phase::Declarations => "declarations",
        }
    }
}

/// A counted event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Count {
    /// Definition bodies checked (H ran).
    Definitions,
    /// Native LIR units built.
    Units,
    /// Full V runs (a definition V already passed is not counted).
    VRuns,
    /// Result-store hits (none until the store exists).
    StoreHits,
    /// Result-store misses (none until the store exists).
    StoreMisses,
    /// Object-cache hits (none until the unit cache exists).
    ObjectHits,
    /// Object-cache misses (none until the unit cache exists).
    ObjectMisses,
    /// Native units imported instead of replayed.
    NativeImports,
    /// Native imports replayed from stored effects and Value LIR.
    ImportHits,
    /// Native import lookups that need fresh importing.
    ImportMisses,
    /// Bodies lowered to TIR; in `check`, the bodies lowered at check time.
    Lowered,
    /// Dependency package sources parsed; none for a package a std image holds
    /// until a query needs its syntax.
    DependencyParses,
    /// Per-definition queries computed rather than reused.
    Recomputes,
    /// Memos invalidated by a changed signature read.
    InvalidatedSignature,
    /// Memos invalidated by a changed failed signature read.
    InvalidatedFailedSignature,
    /// Memos invalidated by a changed lookup read.
    InvalidatedLookup,
    /// Memos invalidated by a changed name set read.
    InvalidatedNameSet,
    /// Memos invalidated by a changed representation read.
    InvalidatedRepresentation,
    /// Memos invalidated by a changed executed body read.
    InvalidatedExecutedBody,
    /// Memos invalidated by a changed executed signature read.
    InvalidatedExecutedSignature,
    /// Memos invalidated by a changed cited text read.
    InvalidatedCitedText,
    /// Memos invalidated by a changed declaration candidates read.
    InvalidatedDeclarationCandidates,
    /// Memos invalidated by a changed validation view read.
    InvalidatedValidationView,
    /// Memos invalidated by a changed universe errors read.
    InvalidatedUniverseErrors,
    /// Memos invalidated by a changed first heap read.
    InvalidatedFirstHeap,
    /// Memos invalidated by a changed environment read.
    InvalidatedEnvironment,
    /// Proofs and comparisons the mode passes asked their query's `ContextMemo`.
    ModeMemoQueries,
    /// Entries those queries reused, interior sub-proofs included.
    ModeMemoHits,
    /// S1 keys first claimed by a worker.
    S1Claims,
    /// S1 requests that park on another worker's key.
    S1Parks,
    /// S1 requests run inline to avoid a wait-for cycle.
    S1Duplicates,
    /// Nanoseconds acquiring the S1 coordinator's lock (sum over workers).
    S1LockWaitNs,
    /// Nanoseconds parked on S1 dependencies (sum over workers).
    S1ParkNs,
    /// Memos invalidated by a changed package-view universe input.
    InvalidatedValidationUniverse,
    /// S1 worker helper lowerings evicted by the retention budget.
    S1LowerHoldEvictions,
    /// Evicted worker helper lowerings demanded again.
    S1LowerHoldRecomputes,
    /// Sum of each worker's insertion-before-eviction peak retained bytes.
    S1LowerHoldPeakBytes,
    /// Physical worker Lower misses, including helpers and discarded attempts.
    S1LowerQueryAttempts,
    /// Actual worker LoweringPhase::lower calls, including discarded work.
    S1LowerAdapterCalls,
    /// Root tasks dispatched, including registration retries and suffix work.
    S1RootTasks,
    /// Dispatched root outcomes excluded by an earlier publication barrier.
    S1DiscardedRoots,
    /// Rounds repeated after publishing required declarations or generated text.
    S1RegistrationRounds,
    /// Root outcomes obtained through the scalar compatibility path.
    S1ScalarRoots,
    /// Maximum sum of Value row lengths at native admission (not capacities).
    NativeValueBytes,
    /// Maximum estimated bytes admitted simultaneously, including backend contexts.
    NativeActiveBytes,
    /// Maximum native admission allowance selected for a build.
    NativeBudgetBytes,
    /// Maximum effective native workers, after byte and stack limits.
    NativeWorkers,
    /// Stack virtual reservations of additional native workers, not RSS.
    NativeStackBytes,
    /// Maximum Ready row bytes retained for uncompleted native consumers.
    NativeReadyBytes,
    ValueDecodes,
    ValueDecodeBytes,
    ValueRebinds,
    ImportEffectReplays,
    NativeAppendExpand,
    ClosureBodyScans,
    ArenaBodyScans,
    LateKeys,
}

impl Count {
    /// Every count, in declaration order.
    pub const ALL: [Count; 57] = [
        Count::Definitions,
        Count::Units,
        Count::VRuns,
        Count::StoreHits,
        Count::StoreMisses,
        Count::ObjectHits,
        Count::ObjectMisses,
        Count::NativeImports,
        Count::ImportHits,
        Count::ImportMisses,
        Count::Lowered,
        Count::DependencyParses,
        Count::Recomputes,
        Count::InvalidatedSignature,
        Count::InvalidatedFailedSignature,
        Count::InvalidatedLookup,
        Count::InvalidatedNameSet,
        Count::InvalidatedRepresentation,
        Count::InvalidatedExecutedBody,
        Count::InvalidatedExecutedSignature,
        Count::InvalidatedCitedText,
        Count::InvalidatedDeclarationCandidates,
        Count::InvalidatedValidationView,
        Count::InvalidatedUniverseErrors,
        Count::InvalidatedFirstHeap,
        Count::InvalidatedEnvironment,
        Count::ModeMemoQueries,
        Count::ModeMemoHits,
        Count::S1Claims,
        Count::S1Parks,
        Count::S1Duplicates,
        Count::S1LockWaitNs,
        Count::S1ParkNs,
        Count::InvalidatedValidationUniverse,
        Count::S1LowerHoldEvictions,
        Count::S1LowerHoldRecomputes,
        Count::S1LowerHoldPeakBytes,
        Count::S1LowerQueryAttempts,
        Count::S1LowerAdapterCalls,
        Count::S1RootTasks,
        Count::S1DiscardedRoots,
        Count::S1RegistrationRounds,
        Count::S1ScalarRoots,
        Count::NativeValueBytes,
        Count::NativeActiveBytes,
        Count::NativeBudgetBytes,
        Count::NativeWorkers,
        Count::NativeStackBytes,
        Count::NativeReadyBytes,
        Count::ValueDecodes,
        Count::ValueDecodeBytes,
        Count::ValueRebinds,
        Count::ImportEffectReplays,
        Count::NativeAppendExpand,
        Count::ClosureBodyScans,
        Count::ArenaBodyScans,
        Count::LateKeys,
    ];
    /// The stable name `--timings` prints.
    pub fn name(self) -> &'static str {
        match self {
            Count::ValueDecodes => "native Value decodes",
            Count::ValueDecodeBytes => "native Value decode bytes",
            Count::ValueRebinds => "native Value rebinds",
            Count::ImportEffectReplays => "native effect replays",
            Count::NativeAppendExpand => "native append/expand",
            Count::ClosureBodyScans => "closure body scans",
            Count::ArenaBodyScans => "arena body scans",
            Count::LateKeys => "native late keys",
            Count::Definitions => "definitions",
            Count::Units => "units",
            Count::VRuns => "V runs",
            Count::StoreHits => "store hits",
            Count::StoreMisses => "store misses",
            Count::ObjectHits => "object hits",
            Count::ObjectMisses => "object misses",
            Count::NativeImports => "native imports",
            Count::ImportHits => "import hits",
            Count::ImportMisses => "import misses",
            Count::Lowered => "lowered",
            Count::DependencyParses => "dependency parses",
            Count::Recomputes => "recomputes",
            Count::InvalidatedSignature => "invalidated by signature",
            Count::InvalidatedFailedSignature => "invalidated by failed signature",
            Count::InvalidatedLookup => "invalidated by lookup",
            Count::InvalidatedNameSet => "invalidated by name set",
            Count::InvalidatedRepresentation => "invalidated by representation",
            Count::InvalidatedExecutedBody => "invalidated by executed body",
            Count::InvalidatedExecutedSignature => "invalidated by executed signature",
            Count::InvalidatedCitedText => "invalidated by cited text",
            Count::InvalidatedDeclarationCandidates => "invalidated by declaration candidates",
            Count::InvalidatedValidationView => "invalidated by validation view",
            Count::InvalidatedUniverseErrors => "invalidated by universe errors",
            Count::InvalidatedFirstHeap => "invalidated by first heap",
            Count::InvalidatedEnvironment => "invalidated by environment",
            Count::ModeMemoQueries => "mode memo queries",
            Count::ModeMemoHits => "mode memo hits",
            Count::S1Claims => "S1 claims",
            Count::S1Parks => "S1 parks",
            Count::S1Duplicates => "S1 duplicates",
            Count::S1LockWaitNs => "S1 lock wait ns",
            Count::S1ParkNs => "S1 park ns",
            Count::InvalidatedValidationUniverse => "invalidated by validation universe",
            Count::S1LowerHoldEvictions => "S1 lower hold evictions",
            Count::S1LowerHoldRecomputes => "S1 lower hold recomputes",
            Count::S1LowerHoldPeakBytes => "S1 lower hold peak bytes sum",
            Count::S1LowerQueryAttempts => "S1 lower query attempts",
            Count::S1LowerAdapterCalls => "S1 lower adapter calls",
            Count::S1RootTasks => "S1 root tasks",
            Count::S1DiscardedRoots => "S1 discarded roots",
            Count::S1RegistrationRounds => "S1 registration rounds",
            Count::S1ScalarRoots => "S1 scalar roots",
            Count::NativeValueBytes => "native Value structural bytes peak",
            Count::NativeActiveBytes => "native active estimated bytes peak",
            Count::NativeBudgetBytes => "native admission budget bytes",
            Count::NativeWorkers => "native effective workers peak",
            Count::NativeStackBytes => "native worker stack reservation bytes peak",
            Count::NativeReadyBytes => "native retained Ready structural bytes peak",
        }
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);

struct Totals {
    intervals: [AtomicU64; Interval::ALL.len()],
    /// Per phase: work nanoseconds, CPU nanoseconds, entries.
    phases: [[AtomicU64; 3]; Phase::ALL.len()],
    counts: [AtomicU64; Count::ALL.len()],
    wait: AtomicU64,
    /// Verify work, CPU and entries inside a comptime phase (G1's split).
    comptime_verify: [AtomicU64; 3],
}

static TOTALS: Totals = Totals {
    intervals: [const { AtomicU64::new(0) }; Interval::ALL.len()],
    phases: [const { [const { AtomicU64::new(0) }; 3] }; Phase::ALL.len()],
    counts: [const { AtomicU64::new(0) }; Count::ALL.len()],
    wait: AtomicU64::new(0),
    comptime_verify: [const { AtomicU64::new(0) }; 3],
};

struct PhaseFrame {
    phase: Phase,
    start: Instant,
    cpu: Option<Duration>,
    child_wall: Duration,
    child_cpu: Duration,
}

struct IntervalFrame {
    interval: Interval,
    resumed: Instant,
}

#[derive(Default)]
struct Stacks {
    phases: Vec<PhaseFrame>,
    intervals: Vec<IntervalFrame>,
}

thread_local! {
    static STACKS: RefCell<Stacks> = RefCell::new(Stacks::default());
}

/// Start recording, for the whole process.
pub fn enable() {
    ENABLED.store(true, Relaxed);
}

/// Whether timings are being recorded.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Relaxed)
}

fn add(cell: &AtomicU64, duration: Duration) {
    cell.fetch_add(duration.as_nanos() as u64, Relaxed);
}

/// This thread's CPU time, where the OS reports it.
fn thread_cpu() -> Option<Duration> {
    #[cfg(target_os = "linux")]
    {
        #[repr(C)]
        struct Timespec {
            seconds: i64,
            nanoseconds: i64,
        }
        unsafe extern "C" {
            fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
        }
        const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
        let mut time = Timespec {
            seconds: 0,
            nanoseconds: 0,
        };
        // SAFETY: `time` is a valid, writable timespec for the call.
        if unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut time) } == 0 {
            return Some(Duration::new(time.seconds as u64, time.nanoseconds as u32));
        }
    }
    None
}

/// Attribute wall time to `interval` until the guard drops, pausing the
/// interval it interrupts on this thread.
#[inline]
#[must_use = "the interval ends when the guard drops"]
pub fn interval(interval: Interval) -> IntervalGuard {
    if !enabled() {
        return IntervalGuard(false);
    }
    let now = Instant::now();
    STACKS.with(|s| {
        let stack = &mut s.borrow_mut().intervals;
        if let Some(outer) = stack.last() {
            add(
                &TOTALS.intervals[outer.interval as usize],
                now - outer.resumed,
            );
        }
        stack.push(IntervalFrame {
            interval,
            resumed: now,
        });
    });
    IntervalGuard(true)
}

/// Time `phase` until the guard drops.
#[inline]
#[must_use = "the phase ends when the guard drops"]
pub fn phase(phase: Phase) -> PhaseGuard {
    if !enabled() {
        return PhaseGuard(false);
    }
    let frame = PhaseFrame {
        phase,
        start: Instant::now(),
        cpu: thread_cpu(),
        child_wall: Duration::ZERO,
        child_cpu: Duration::ZERO,
    };
    STACKS.with(|s| s.borrow_mut().phases.push(frame));
    PhaseGuard(true)
}

/// Record time this thread spends waiting for other threads, until the
/// guard drops. It is reported apart from intervals and phases, and the
/// phase this thread is in does not count it as its own work.
#[inline]
#[must_use = "the wait ends when the guard drops"]
pub fn wait() -> WaitGuard {
    WaitGuard(enabled().then(Instant::now))
}

/// Count one `count` event.
#[inline]
pub fn count(count: Count) {
    if enabled() {
        TOTALS.counts[count as usize].fetch_add(1, Relaxed);
    }
}

/// Count `n` `count` events.
#[inline]
pub fn count_by(count: Count, n: u64) {
    if enabled() && n > 0 {
        TOTALS.counts[count as usize].fetch_add(n, Relaxed);
    }
}

/// Retain the maximum observation for a peak counter. Unlike `count_by`,
/// separate builds in one resident session do not add their byte peaks.
#[inline]
pub fn count_peak(count: Count, n: u64) {
    if enabled() {
        TOTALS.counts[count as usize].fetch_max(n, Relaxed);
    }
}

/// An active interval; dropping it resumes the interval it interrupted.
pub struct IntervalGuard(bool);

impl Drop for IntervalGuard {
    #[inline]
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let now = Instant::now();
        STACKS.with(|s| {
            let stack = &mut s.borrow_mut().intervals;
            let frame = stack.pop().expect("timings: interval exit without entry");
            add(
                &TOTALS.intervals[frame.interval as usize],
                now - frame.resumed,
            );
            if let Some(outer) = stack.last_mut() {
                outer.resumed = now;
            }
        });
    }
}

/// An active phase; dropping it ends the phase.
pub struct PhaseGuard(bool);

impl Drop for PhaseGuard {
    #[inline]
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let (wall, cpu) = (Instant::now(), thread_cpu());
        STACKS.with(|s| {
            let stack = &mut s.borrow_mut().phases;
            let frame = stack.pop().expect("timings: phase exit without entry");
            let wall = wall - frame.start;
            let cpu = cpu
                .zip(frame.cpu)
                .map_or(Duration::ZERO, |(end, start)| end.saturating_sub(start));
            let (work, cpu_own) = (
                wall.saturating_sub(frame.child_wall),
                cpu.saturating_sub(frame.child_cpu),
            );
            let record = |totals: &[AtomicU64; 3]| {
                add(&totals[0], work);
                add(&totals[1], cpu_own);
                totals[2].fetch_add(1, Relaxed);
            };
            record(&TOTALS.phases[frame.phase as usize]);
            if frame.phase == Phase::Verify && stack.iter().any(|f| f.phase == Phase::Comptime) {
                record(&TOTALS.comptime_verify);
            }
            if let Some(parent) = stack.last_mut() {
                parent.child_wall += wall;
                parent.child_cpu += cpu;
            }
        });
    }
}

/// An active wait; dropping it ends the wait.
pub struct WaitGuard(Option<Instant>);

impl Drop for WaitGuard {
    #[inline]
    fn drop(&mut self) {
        if let Some(start) = self.0 {
            let waited = start.elapsed();
            add(&TOTALS.wait, waited);
            STACKS.with(|s| {
                if let Some(phase) = s.borrow_mut().phases.last_mut() {
                    phase.child_wall += waited;
                }
            });
        }
    }
}

/// One phase's totals over every thread.
#[derive(Clone, Debug, PartialEq)]
pub struct PhaseRow {
    pub phase: Phase,
    /// Wall time inside the phase, excluding nested phases, summed over threads.
    pub work: Duration,
    /// CPU time inside the phase, excluding nested phases, summed over threads.
    pub cpu: Duration,
    pub calls: u64,
}

/// What `--timings` prints. Every interval, phase and count appears, in
/// `ALL` order, including those still 0.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub intervals: Vec<(Interval, Duration)>,
    pub phases: Vec<PhaseRow>,
    /// The part of [`Phase::Verify`] run inside a [`Phase::Comptime`]: V on
    /// the bodies compile-time evaluation executes.
    pub comptime_verify: PhaseRow,
    pub wait: Duration,
    pub counts: Vec<(Count, u64)>,
}

/// Every interval and phase that has ended so far, on every thread.
pub fn report() -> Report {
    let nanos = |cell: &AtomicU64| Duration::from_nanos(cell.load(Relaxed));
    Report {
        intervals: Interval::ALL
            .iter()
            .map(|&i| (i, nanos(&TOTALS.intervals[i as usize])))
            .collect(),
        phases: Phase::ALL
            .iter()
            .map(|&phase| {
                let totals = &TOTALS.phases[phase as usize];
                PhaseRow {
                    phase,
                    work: nanos(&totals[0]),
                    cpu: nanos(&totals[1]),
                    calls: totals[2].load(Relaxed),
                }
            })
            .collect(),
        comptime_verify: PhaseRow {
            phase: Phase::Verify,
            work: nanos(&TOTALS.comptime_verify[0]),
            cpu: nanos(&TOTALS.comptime_verify[1]),
            calls: TOTALS.comptime_verify[2].load(Relaxed),
        },
        wait: nanos(&TOTALS.wait),
        counts: Count::ALL
            .iter()
            .map(|&c| (c, TOTALS.counts[c as usize].load(Relaxed)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spin until this thread has used `cpu` of CPU time.
    fn burn(cpu: Duration) {
        let start = thread_cpu().unwrap();
        while thread_cpu().unwrap() - start < cpu {}
    }

    fn phase_row(phase: Phase) -> PhaseRow {
        report()
            .phases
            .into_iter()
            .find(|r| r.phase == phase)
            .unwrap()
    }

    fn interval_wall(interval: Interval) -> Duration {
        report()
            .intervals
            .into_iter()
            .find(|r| r.0 == interval)
            .unwrap()
            .1
    }

    // One test owns the process-wide totals: separate tests would race on them.
    #[test]
    fn intervals_add_up_and_phases_sum_work_and_cpu_over_threads() {
        enable();
        // A nested interval pauses its outer one: together they cover the
        // wall time once.
        let start = Instant::now();
        let outer = interval(Interval::S0);
        burn(Duration::from_millis(5));
        let inner = interval(Interval::Merge);
        burn(Duration::from_millis(5));
        drop(inner);
        drop(outer);
        let covered = start.elapsed();
        let (s0, merge) = (interval_wall(Interval::S0), interval_wall(Interval::Merge));
        assert!(s0 + merge <= covered, "{s0:?} + {merge:?} > {covered:?}");
        assert!(merge >= Duration::from_millis(5) && s0 >= Duration::from_millis(5));

        // A nested phase's time is its own; the parent keeps only the rest.
        let lower = phase(Phase::Lower);
        burn(Duration::from_millis(5));
        let verify = phase(Phase::Verify);
        burn(Duration::from_millis(10));
        drop(verify);
        drop(lower);
        let comptime = phase(Phase::Comptime);
        let inner = phase(Phase::Verify);
        burn(Duration::from_millis(5));
        drop(inner);
        drop(comptime);
        let split = report().comptime_verify;
        assert_eq!(split.calls, 1);
        assert!(split.cpu >= Duration::from_millis(5), "{split:?}");
        let (lower, verify) = (phase_row(Phase::Lower), phase_row(Phase::Verify));
        assert!(verify.cpu >= Duration::from_millis(10), "{verify:?}");
        assert!(lower.cpu >= Duration::from_millis(5), "{lower:?}");
        assert!(lower.cpu < verify.cpu, "{lower:?}");

        // A phase run by four jobs::map workers at once sums each worker's
        // work and CPU: its work is at least the interval's wall, and CPU at
        // most work. The caller's wait for them is wait, not its phase's work.
        const THREADS: u32 = 4;
        const EACH: Duration = Duration::from_millis(20);
        let parallel = interval(Interval::Parallel);
        let modes = phase(Phase::Modes);
        crate::jobs::map(
            crate::jobs::Jobs::new(THREADS as usize),
            vec![(); THREADS as usize],
            |_| 1,
            |()| {
                let _check = phase(Phase::Check);
                burn(EACH);
            },
        );
        drop(modes);
        drop(parallel);
        let check = phase_row(Phase::Check);
        let wall = interval_wall(Interval::Parallel);
        assert_eq!(check.calls, u64::from(THREADS));
        assert!(check.work >= wall, "{check:?} against {wall:?}");
        let (modes, waited) = (phase_row(Phase::Modes), report().wait);
        assert!(
            waited >= EACH && modes.work < EACH,
            "{modes:?}, waited {waited:?}"
        );
        assert!(
            check.cpu >= EACH * THREADS && check.cpu <= check.work,
            "{check:?}"
        );
    }
}
