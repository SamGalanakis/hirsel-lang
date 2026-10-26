//! Operation descriptors (multicore design §4.1): what every runtime entry, every inline effect
//! the code generators emit and every scheduling point's effect may touch, so an executor can
//! tell which work runs in parallel, which ops commit early and which run only at the head.
//!
//! - [`ENTRIES`] gives every `hirsel_rt_v2_*` entry its [`Locality`], and an [`Locality::Op`]
//!   entry the ops it performs.
//! - [`OPS`] describes each op: its transitions, outcome, footprint, visibility and the free-mode
//!   phases it runs (§5 Op phases). [`OpDescriptor::class`] derives its ordered-mode class.
//! - [`E`] is the closed set of ops refinement 2 may commit early (§4.3).
//! - [`INLINE`] classifies the effects compiled code performs without a call, by the run-context
//!   and block words it touches; [`SCHEDULING_POINT`] the effects a scheduling point performs
//!   for parked tasks.
//! - [`REGISTRY`] names, for each task, channel, mutex, atomic and `Shared` operation of the core
//!   registry, the ops its native lowering performs.
//!
//! hirsel-rt's `descriptors` test checks the runtime source against these tables: an
//! unclassified entry or registry operation fails, and so does an entry marked
//! [`Locality::Local`], [`Locality::Immutable`] or [`Locality::Concurrent`] whose call graph
//! reaches the scheduler, the Io binding or an entry of an incompatible locality.
//! Task-local work may call an unobservable concurrent service, checked independently.

/// Where an entry's work may run (§4.1 Locality).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Locality {
    /// Task-local: the task's own blocks, text, copy and codec work, the regions it owns, its
    /// failure chain and isolate scratch, its freeze/thaw stack, its fuel. Runs on any worker
    /// in parallel with every other task; may use unobservable concurrent services
    /// (for example, copying a value adds a `Shared` root). Ordered allocation
    /// and fuel preflight may physically save until head for exact budgets;
    /// this does not create an op or a scheduling point for the local entry.
    Local,
    /// Reads only immutable run data: layouts, the failure table, program tables, seeds,
    /// launch arguments.
    Immutable,
    /// A concurrent service whose effect is unobservable: path interning, handle-row
    /// allocation, remote free, root increments, per-worker counters.
    Concurrent,
    /// A boundary: performs the ops it names.
    Op,
    /// Runs on the admission thread outside every task: creating, launching, reporting on and
    /// destroying a run (§6 Stops and worker lifetime).
    Admission,
    /// Role-owned physical bookkeeping for a task's certified continuation.
    /// May inspect its scheduler identity but creates no semantic operation.
    /// Called on a task, unlike Admission; audited separately from local work.
    Physical,
}

/// What follows an op's effect (§4.1 Transition).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transition {
    /// No scheduling point: the task continues its turn.
    Keep,
    /// A scheduling point follows the effect.
    Yield,
    /// A scheduling point; the task resumes when a poll ends its wait.
    Park,
    /// The task parks and a holding child's rest runs; the child's end returns to the caller
    /// with no scheduling point.
    Call,
    /// Task completion: a scheduling point unless it returns to a caller.
    End,
    /// The run stops.
    Terminal,
}

/// What an op returns (§4.1 Outcome).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    None,
    /// An identity minted by a concurrent allocator and initialised at commit.
    Provisional,
    /// A value that depends on shared state.
    Value,
}

/// An object an op touches, named by (creator path, creator op ordinal) in canonical events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    Channel,
    Mutex,
    Atomic,
    TaskSet,
    /// A task's row: its state, result slot and holder.
    Task,
    Root,
    /// A `serial` package's foreign gate (S-13).
    Gate,
}

/// A footprint item (§4.1 Footprint).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Item {
    Object(ObjectKind),
    /// The running task's cancel state.
    CancelState,
    /// The cancel states of a subtree (`cancel`, a failing end, deadline expiry).
    SubtreeCancel,
    /// The task tree's structure: scopes, their member lists and tasks' scope links.
    TaskTree,
    /// A scope's completion rank (written by an end, read by `await_any`).
    CompletionRank,
    /// Whether the running task holds a root's last reference, which every holder's
    /// releases decide.
    RootHolders,
    /// Host resource keys (§6 Io).
    HostKeys,
    /// Counted storage an op's commit frees (a close dropping leftovers).
    StorageCredits,
    /// The cancel states of every parked task a poll polls.
    ParkedCancelStates,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    Read(Item),
    Write(Item),
}

impl Access {
    pub const fn item(self) -> Item {
        match self {
            Access::Read(item) | Access::Write(item) => item,
        }
    }
}

/// §4.1 Visibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visibility {
    Internal,
    /// Host requests, extern calls, tape lines, stops.
    External,
}

/// The free-mode phases an op runs (§5 Op phases). Ordered mode retires turns and uses none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phases {
    /// Entry and action in one section.
    Single,
    /// Entered, then a done phase retried after each wake under the same ordinal.
    Waiting,
    /// Entered and prepared, claimed, one or more key transitions, then done.
    Request,
    /// Entered, cleanup batches until no release is pending, then done.
    End,
}

/// The ordered-mode class (§4.1), derived from the other fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    /// Deferrable: outcome none or provisional, status statically OK, internal.
    D,
    /// Its outcome is a value; internal.
    R,
    /// Head-only: external, or it reads who holds a root.
    H,
}

#[derive(Clone, Copy, Debug)]
pub struct OpDescriptor {
    pub name: &'static str,
    /// Every transition the op can take, the first being its usual one.
    pub transitions: &'static [Transition],
    pub outcome: Outcome,
    /// Whether its status can be a failure.
    pub fallible: bool,
    pub footprint: &'static [Access],
    pub visibility: Visibility,
    pub phases: Phases,
    /// Where the design (§4.1) or the runtime says so.
    pub note: &'static str,
}

impl OpDescriptor {
    pub fn class(&self) -> Class {
        if self.visibility == Visibility::External
            || self.footprint.iter().any(|a| a.item() == Item::RootHolders)
        {
            Class::H
        } else if self.outcome == Outcome::Value || self.fallible {
            Class::R
        } else {
            Class::D
        }
    }

    /// Whether every footprint item is an object: the precondition of [`E`] membership.
    pub fn objects_only(&self) -> bool {
        self.footprint
            .iter()
            .all(|a| matches!(a.item(), Item::Object(_)))
    }
}

/// A runtime entry: its exported symbol, locality and, for an op boundary, its ops.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub symbol: &'static str,
    pub locality: Locality,
    pub ops: &'static [&'static str],
}

/// An inline effect of compiled code, named by the run-context or block word it touches
/// (`hirsel_lir::runtime_abi`).
#[derive(Clone, Copy, Debug)]
pub struct Inline {
    pub word: &'static str,
    pub locality: Locality,
    pub effect: &'static str,
}

use Access::{Read, Write};
use Item::*;
use Locality::{Admission, Concurrent, Immutable, Local, Op, Physical};
use ObjectKind::*;
use Transition::{Call, End, Keep, Park, Terminal, Yield};

const fn op(
    name: &'static str,
    transitions: &'static [Transition],
    outcome: Outcome,
    footprint: &'static [Access],
    phases: Phases,
) -> OpDescriptor {
    OpDescriptor {
        name,
        transitions,
        outcome,
        fallible: false,
        footprint,
        visibility: Visibility::Internal,
        phases,
        note: "",
    }
}

const fn external(mut d: OpDescriptor) -> OpDescriptor {
    d.visibility = Visibility::External;
    d
}

const fn fallible(mut d: OpDescriptor) -> OpDescriptor {
    d.fallible = true;
    d
}

const fn note(mut d: OpDescriptor, note: &'static str) -> OpDescriptor {
    d.note = note;
    d
}

/// Every op (§4.1's op table, plus scope entry, a task's end and the drops it names).
pub const OPS: &[OpDescriptor] = &[
    // Creations: a provisional identity, initialised at commit.
    op(
        "atomic.create",
        &[Yield],
        Outcome::Provisional,
        &[Write(Object(Atomic))],
        Phases::Single,
    ),
    op(
        "mutex.create",
        &[Yield],
        Outcome::Provisional,
        &[Write(Object(Mutex))],
        Phases::Single,
    ),
    op(
        "channel.open",
        &[Yield],
        Outcome::Provisional,
        &[Write(Object(Channel))],
        Phases::Single,
    ),
    op(
        "sender.dup",
        &[Yield],
        Outcome::Provisional,
        &[Write(Object(Channel))],
        Phases::Single,
    ),
    op(
        "task.set",
        &[Yield],
        Outcome::Provisional,
        &[Write(Object(TaskSet)), Write(TaskTree)],
        Phases::Single,
    ),
    note(
        op(
            "task.async",
            &[Yield],
            Outcome::Provisional,
            &[Write(Object(Task)), Write(TaskTree), Read(CancelState)],
            Phases::Single,
        ),
        "the cancel inheritance is read at commit (tasks.rs spawn)",
    ),
    op(
        "task.scope_enter",
        &[Yield],
        Outcome::Provisional,
        &[Write(TaskTree)],
        Phases::Single,
    ),
    // Stores and edges with no outcome.
    op(
        "atomic.store",
        &[Yield],
        Outcome::None,
        &[Write(Object(Atomic))],
        Phases::Single,
    ),
    op(
        "mutex.put",
        &[Yield],
        Outcome::None,
        &[Write(Object(Mutex))],
        Phases::Single,
    ),
    note(
        op(
            "mutex.kill",
            &[Yield],
            Outcome::None,
            &[Write(Object(Mutex)), Write(StorageCredits)],
            Phases::Single,
        ),
        "a failure path (tasks.rs mutex_kill); outside E",
    ),
    op(
        "task.add",
        &[Yield],
        Outcome::None,
        &[Write(Object(TaskSet)), Write(Object(Task)), Write(TaskTree)],
        Phases::Single,
    ),
    op(
        "task.cancel",
        &[Yield],
        Outcome::None,
        &[Write(Object(Task)), Write(SubtreeCancel)],
        Phases::Single,
    ),
    note(
        op(
            "task.detach",
            &[Call, Yield],
            Outcome::None,
            &[Write(Object(Task)), Write(TaskTree), Write(StorageCredits)],
            Phases::Single,
        ),
        "a call per holding member, then yield",
    ),
    note(
        op(
            "task.detach_all",
            &[Call, Yield],
            Outcome::None,
            &[
                Write(Object(TaskSet)),
                Write(Object(Task)),
                Write(TaskTree),
                Write(StorageCredits),
            ],
            Phases::Single,
        ),
        "detaches member by member",
    ),
    // Drops.
    note(
        op(
            "root.release",
            &[Keep],
            Outcome::None,
            &[Write(Object(Root)), Write(StorageCredits)],
            Phases::Single,
        ),
        "roots::consume defers non-last releases to retirement or the next entered phase; a sole holder frees its frozen graph without finalizers or host handles",
    ),
    note(
        op(
            "drop.trivial",
            &[Keep],
            Outcome::None,
            &[
                Write(Object(Channel)),
                Write(Object(TaskSet)),
                Write(Object(Task)),
                Write(Object(Atomic)),
                Write(Object(Mutex)),
            ],
            Phases::Single,
        ),
        "a Sender, TaskSet, Task or atomic; a Receiver or mutex whose payload is trivially droppable",
    ),
    note(
        op(
            "drop.leftovers",
            &[Keep],
            Outcome::Value,
            &[
                Write(Object(Channel)),
                Write(Object(Mutex)),
                Write(StorageCredits),
            ],
            Phases::Single,
        ),
        "close detaches the leftovers, which the dropper releases in its own continuation",
    ),
    // Values without a scheduling point.
    op(
        "mutex.take",
        &[Keep, Park],
        Outcome::Value,
        &[Write(Object(Mutex))],
        Phases::Waiting,
    ),
    op(
        "task.set_has",
        &[Keep],
        Outcome::Value,
        &[Read(Object(TaskSet)), Read(Object(Task))],
        Phases::Single,
    ),
    op(
        "task.set_take",
        &[Keep],
        Outcome::Value,
        &[Write(Object(TaskSet)), Read(Object(Task))],
        Phases::Single,
    ),
    fallible(op(
        "mutex.cause",
        &[Keep],
        Outcome::Value,
        &[Read(Object(Mutex))],
        Phases::Single,
    )),
    // Values followed by a scheduling point.
    op(
        "atomic.load",
        &[Yield],
        Outcome::Value,
        &[Read(Object(Atomic))],
        Phases::Single,
    ),
    note(
        op(
            "channel.send",
            &[Yield],
            Outcome::Value,
            &[Write(Object(Channel))],
            Phases::Single,
        ),
        "never checks cancellation (S-11, ruling (b))",
    ),
    op(
        "task.check_canceled",
        &[Yield],
        Outcome::Value,
        &[Read(CancelState)],
        Phases::Single,
    ),
    op(
        "channel.receive",
        &[Yield, Park],
        Outcome::Value,
        &[Write(Object(Channel)), Read(CancelState)],
        Phases::Waiting,
    ),
    op(
        "task.await_any",
        &[Yield, Park, Call],
        Outcome::Value,
        &[
            Write(Object(TaskSet)),
            Read(Object(Task)),
            Read(CompletionRank),
            Read(CancelState),
        ],
        Phases::Waiting,
    ),
    op(
        "task.join",
        &[Yield, Park, Call],
        Outcome::Value,
        &[Write(Object(Task)), Write(TaskTree), Read(CancelState)],
        Phases::Waiting,
    ),
    op(
        "task.cancel_all",
        &[Yield, Park, Call],
        Outcome::Value,
        &[
            Write(Object(TaskSet)),
            Write(Object(Task)),
            Write(SubtreeCancel),
            Write(TaskTree),
        ],
        Phases::Waiting,
    ),
    note(
        op(
            "task.scope_exit",
            &[Yield, Park, Call],
            Outcome::Value,
            &[Write(Object(Task)), Write(TaskTree), Read(CancelState)],
            Phases::Waiting,
        ),
        "waits for, and calls into, children in reverse order",
    ),
    note(
        op(
            "task.end",
            &[End],
            Outcome::None,
            &[
                Write(Object(Task)),
                Write(CompletionRank),
                Write(SubtreeCancel),
                Write(TaskTree),
            ],
            Phases::End,
        ),
        "a failing end cancels its subtree",
    ),
    note(
        op(
            "task.isolate",
            &[Yield, Keep],
            Outcome::Value,
            &[],
            Phases::Single,
        ),
        "its entry yields; its catch reads the task's own spawn path for the caught chain",
    ),
    note(
        op("arg.read", &[Yield], Outcome::Value, &[], Phases::Single),
        "a launch argument read: immutable, but a capability call and so a scheduling point",
    ),
    // Head-only.
    op(
        "shared.thaw",
        &[Keep],
        Outcome::Value,
        &[Read(RootHolders), Write(Object(Root))],
        Phases::Single,
    ),
    external(op(
        "host.request",
        &[Yield, Park, Keep],
        Outcome::Value,
        &[Write(HostKeys), Read(CancelState)],
        Phases::Request,
    )),
    external(op(
        "host.handle_drop",
        &[Keep],
        Outcome::None,
        &[Write(HostKeys)],
        Phases::Request,
    )),
    external(note(
        op(
            "task.scope_until",
            &[Yield],
            Outcome::Provisional,
            &[Write(TaskTree), Write(HostKeys), Write(SubtreeCancel)],
            Phases::Single,
        ),
        "arms the scope's deadline on its clock",
    )),
    external(op(
        "extern.call",
        &[Keep],
        Outcome::Value,
        &[],
        Phases::Single,
    )),
    external(note(
        op(
            "gate.enter",
            &[Park, Keep],
            Outcome::None,
            &[Write(Object(Gate))],
            Phases::Waiting,
        ),
        "the outermost entry of a serial package's gate (S-13)",
    )),
    external(note(
        op(
            "gate.leave",
            &[Keep],
            Outcome::None,
            &[Write(Object(Gate))],
            Phases::Single,
        ),
        "the outermost leave of a serial package's gate (S-13)",
    )),
    external(op("stop", &[Terminal], Outcome::None, &[], Phases::Single)),
];

/// The ops refinement 2 may commit early (§4.1 E): footprint objects only, internal.
pub const E: &[&str] = &[
    "atomic.create",
    "atomic.load",
    "atomic.store",
    "mutex.create",
    "mutex.take",
    "mutex.put",
    "mutex.cause",
    "channel.open",
    "sender.dup",
    "channel.send",
];

/// Ops whose footprint is objects only but which stay outside [`E`], with the reason.
pub const OUTSIDE_E: &[(&str, &str)] = &[
    (
        "drop.trivial",
        "a drop closes an endpoint, which ends receivers' waits at a poll",
    ),
    ("task.set_has", "every task.* op commits at retirement"),
    ("task.set_take", "every task.* op commits at retirement"),
    ("task.isolate", "touches no shared item: nothing to commit"),
    ("arg.read", "touches no shared item: nothing to commit"),
];

/// The descriptor of `name`.
pub fn op_descriptor(name: &str) -> Option<&'static OpDescriptor> {
    OPS.iter().find(|d| d.name == name)
}

const fn entry(symbol: &'static str, locality: Locality) -> Entry {
    Entry {
        symbol,
        locality,
        ops: &[],
    }
}

const fn ops(symbol: &'static str, ops: &'static [&'static str]) -> Entry {
    Entry {
        symbol,
        locality: Op,
        ops,
    }
}

const HOST: &[&str] = &["host.request"];
/// The release walk: a dropped value's finalizers, endpoint closes and handle drops.
const DROPS: &[&str] = &["drop.trivial", "drop.leftovers", "host.handle_drop"];

/// Every exported runtime entry.
pub const ENTRIES: &[Entry] = &[
    // alloc.rs
    entry("hirsel_rt_v2_alloc", Local),
    ops("hirsel_rt_v2_dispose", DROPS),
    ops("hirsel_rt_v2_reuse", DROPS),
    entry("hirsel_rt_v2_seq_grow", Local),
    entry("hirsel_rt_v2_block_grow", Local),
    entry("hirsel_rt_v2_payload", Local),
    entry("hirsel_rt_v2_capacity", Local),
    // depth.rs: semantic counters belong exclusively to the running task.
    entry("hirsel_rt_v2_depth_enter", Local),
    entry("hirsel_rt_v2_depth_enter_at_site", Local),
    entry("hirsel_rt_v2_depth_leave", Local),
    entry("hirsel_rt_v2_depth_preflight", Local),
    entry("hirsel_rt_v2_structural_order", Local),
    entry("hirsel_rt_v2_structural_hash", Local),
    // budget.rs
    entry("hirsel_rt_v2_fuel_refill", Local),
    // Ahead execution is physical progress, not a semantic operation.
    entry("hirsel_rt_v2_ahead_barrier", Local),
    entry("hirsel_rt_v2_ahead_state", Local),
    entry("hirsel_rt_v2_resume_point", Local),
    entry("hirsel_rt_v2_resume_tail", Local),
    entry("hirsel_rt_v2_resume_cleanup", Local),
    ops("hirsel_rt_v2_ordered_join", &["task.join"]),
    ops("hirsel_rt_v2_ordered_sleep_until", HOST),
    ops("hirsel_rt_v2_ordered_print", HOST),
    entry("hirsel_rt_v2_resume_run", Physical),
    entry("hirsel_rt_v2_ahead_handoff", Local),
    // Arm validates task-owned recipe/buffer custody under the scheduler role;
    // it creates neither a read nor an ordinal. Private leaf helpers inspect
    // or update only that task's fixed continuation/cleanup record. Inert
    // credits are actually released later, at canonical admission.
    entry("hirsel_rt_v2_host_arm", Physical),
    entry("hirsel_rt_v2_host_enter", Local),
    entry("hirsel_rt_v2_host_active", Local),
    entry("hirsel_rt_v2_host_defer", Local),
    // codec.rs
    entry("hirsel_rt_v2_codec_encode", Local),
    // A failed decode drops what it built.
    ops("hirsel_rt_v2_codec_decode", DROPS),
    // context.rs
    entry("hirsel_rt_v2_ctx_create", Admission),
    entry("hirsel_rt_v2_ctx_stats", Admission),
    entry("hirsel_rt_v2_ctx_destroy", Admission),
    entry("hirsel_rt_v2_task_exit", Admission),
    entry("hirsel_rt_v2_trim", Local),
    // A worker's creation and retirement (design §6 Heaps) run outside every task, where W1's
    // pool starts and stops threads; a scheduling point's collection of storage other workers
    // freed is a concurrent, unobservable service.
    entry("hirsel_rt_v2_worker_create", Admission),
    entry("hirsel_rt_v2_worker_retire", Admission),
    entry("hirsel_rt_v2_worker_collect", Concurrent),
    // copy.rs
    entry("hirsel_rt_v2_unique_copy", Local),
    entry("hirsel_rt_v2_unique_copy_value", Local),
    entry("hirsel_rt_v2_detach", Local),
    // count.rs
    entry("hirsel_rt_v2_dup", Local),
    // exit_report.rs
    entry("hirsel_rt_v2_exit_report", Admission),
    // failure.rs
    entry("hirsel_rt_v2_failure_begin", Local),
    entry("hirsel_rt_v2_failure_append", Local),
    entry("hirsel_rt_v2_failure_active", Local),
    entry("hirsel_rt_v2_failure_primary", Local),
    entry("hirsel_rt_v2_failure_cause_count", Local),
    entry("hirsel_rt_v2_failure_cause", Local),
    entry("hirsel_rt_v2_failure_clear", Local),
    entry("hirsel_rt_v2_failure_reanchor", Local),
    // finalize.rs
    ops("hirsel_rt_v2_finalize", DROPS),
    ops("hirsel_rt_v2_finalize_begin", DROPS),
    ops("hirsel_rt_v2_finalize_end", DROPS),
    ops("hirsel_rt_v2_close_handle", DROPS),
    entry("hirsel_rt_v2_shielded", Local),
    // foreign.rs
    ops("hirsel_rt_v2_foreign_enter", &["extern.call", "gate.enter"]),
    ops("hirsel_rt_v2_foreign_leave", &["extern.call", "gate.leave"]),
    ops("hirsel_rt_v2_foreign_violation", &["stop"]),
    // The foreign interval owns this task exclusively on its bound thread.
    // Admission/retirement are task-local; permit services remain concurrent.
    entry("hirsel_rt_v2_callback_context", Local),
    entry("hirsel_rt_v2_callback_leave", Local),
    ops("hirsel_rt_v2_callback_failed", &["stop"]),
    // io.rs
    ops("hirsel_rt_v2_io_close", HOST),
    ops("hirsel_rt_v2_capture_begin", HOST),
    ops("hirsel_rt_v2_capture_context", HOST),
    ops("hirsel_rt_v2_capture_event", HOST),
    ops("hirsel_rt_v2_capture_finish", HOST),
    ops("hirsel_rt_v2_signal_wait", HOST),
    ops("hirsel_rt_v2_out_print", HOST),
    ops("hirsel_rt_v2_out_print_at_site", HOST),
    ops("hirsel_rt_v2_out_write", HOST),
    ops("hirsel_rt_v2_err_print", HOST),
    ops("hirsel_rt_v2_err_write", HOST),
    ops("hirsel_rt_v2_dir_read_only", HOST),
    ops("hirsel_rt_v2_mapped_view", HOST),
    ops("hirsel_rt_v2_mapped_check", HOST),
    ops("hirsel_rt_v2_file_watch", HOST),
    ops("hirsel_rt_v2_watcher_next", HOST),
    ops("hirsel_rt_v2_test_dir_event", HOST),
    ops("hirsel_rt_v2_file_narrow", HOST),
    ops("hirsel_rt_v2_file_commit", HOST),
    ops("hirsel_rt_v2_file_read", HOST),
    ops("hirsel_rt_v2_file_seek", HOST),
    ops("hirsel_rt_v2_file_write", HOST),
    ops("hirsel_rt_v2_file_append", HOST),
    ops("hirsel_rt_v2_file_write_bytes", HOST),
    ops("hirsel_rt_v2_file_sync", HOST),
    ops("hirsel_rt_v2_file_put_new", HOST),
    ops("hirsel_rt_v2_file_write_all", HOST),
    entry("hirsel_rt_v2_path_of", Concurrent),
    entry("hirsel_rt_v2_path_text", Local),
    entry("hirsel_rt_v2_path_compare", Concurrent),
    ops("hirsel_rt_v2_arg_path", &["arg.read"]),
    ops("hirsel_rt_v2_arg_len", &["arg.read"]),
    ops("hirsel_rt_v2_arg_text", &["arg.read"]),
    entry("hirsel_rt_v2_instant_checked_add", Local),
    ops("hirsel_rt_v2_clock_sleep_until", HOST),
    ops("hirsel_rt_v2_clock_sleep_until_at_site", HOST),
    ops("hirsel_rt_v2_random_bytes", HOST),
    ops("hirsel_rt_v2_instant_ticks", HOST),
    ops("hirsel_rt_v2_test_check", HOST),
    ops("hirsel_rt_v2_test_check_case", HOST),
    ops("hirsel_rt_v2_test_fail", HOST),
    ops("hirsel_rt_v2_test_crash", HOST),
    ops("hirsel_rt_v2_test_seed", HOST),
    ops("hirsel_rt_v2_test_tasks", HOST),
    ops("hirsel_rt_v2_test_clock", HOST),
    ops("hirsel_rt_v2_test_log", HOST),
    ops("hirsel_rt_v2_test_expect_snapshot", HOST),
    ops("hirsel_rt_v2_test_dir", HOST),
    ops("hirsel_rt_v2_file_read_bytes", HOST),
    ops("hirsel_rt_v2_file_walk", HOST),
    ops("hirsel_rt_v2_file_walk_step", HOST),
    ops("hirsel_rt_v2_file_read_dir_step", HOST),
    ops("hirsel_rt_v2_stdin_read", HOST),
    ops("hirsel_rt_v2_ordered_stdin_read", HOST),
    ops("hirsel_rt_v2_ordered_watcher_next", HOST),
    ops("hirsel_rt_v2_env_get", HOST),
    ops("hirsel_rt_v2_env_narrow", HOST),
    ops("hirsel_rt_v2_file_set_permissions", HOST),
    ops("hirsel_rt_v2_file_rename", HOST),
    ops("hirsel_rt_v2_file_metadata", HOST),
    ops("hirsel_rt_v2_file_read_dir", HOST),
    // isolate.rs
    ops("hirsel_rt_v2_isolate_begin", &["task.isolate"]),
    entry("hirsel_rt_v2_isolate_end", Local),
    ops("hirsel_rt_v2_isolate_catch", &["task.isolate"]),
    entry("hirsel_rt_v2_isolate_count", Local),
    entry("hirsel_rt_v2_isolate_kind", Local),
    entry("hirsel_rt_v2_isolate_rule", Local),
    entry("hirsel_rt_v2_isolate_definition", Local),
    entry("hirsel_rt_v2_isolate_line", Local),
    entry("hirsel_rt_v2_isolate_column", Local),
    entry("hirsel_rt_v2_isolate_spawn", Local),
    // runners.rs: runner handles and calls reach the run's execution, its callee runs and
    // their Io bindings.
    ops("hirsel_rt_v2_io_runner", HOST),
    ops("hirsel_rt_v2_test_runner", HOST),
    ops("hirsel_rt_v2_runner_narrow", HOST),
    ops("hirsel_rt_v2_runner_only", HOST),
    ops("hirsel_rt_v2_runner_grants", HOST),
    ops("hirsel_rt_v2_runner_entry_path", HOST),
    ops("hirsel_rt_v2_runner_send", HOST),
    ops("hirsel_rt_v2_runner_wait", HOST),
    ops("hirsel_rt_v2_runner_take", HOST),
    ops("hirsel_rt_v2_runner_entry", HOST),
    ops("hirsel_rt_v2_runner_argument", HOST),
    ops("hirsel_rt_v2_runner_io", HOST),
    ops("hirsel_rt_v2_runner_tasks", HOST),
    ops("hirsel_rt_v2_runner_reply", HOST),
    // launch.rs
    entry("hirsel_rt_v2_launch_zero", Admission),
    entry("hirsel_rt_v2_launch", Admission),
    entry("hirsel_rt_v2_launch_test", Admission),
    entry("hirsel_rt_v2_main", Admission),
    // lib.rs
    ops("hirsel_rt_v2_diag_panic", &["stop"]),
    entry("hirsel_rt_v2_hash_begin", Local),
    entry("hirsel_rt_v2_hash_bytes", Local),
    entry("hirsel_rt_v2_hash_init", Local),
    entry("hirsel_rt_v2_hash_update", Local),
    entry("hirsel_rt_v2_hash_finish", Local),
    entry("hirsel_rt_v2_abi_version", Immutable),
    ops("hirsel_rt_v2_invariant_fault", &["stop"]),
    ops("hirsel_rt_v2_unchecked_add_overflow", &["stop"]),
    // measurement.rs
    entry("hirsel_rt_v2_measure_release", Concurrent),
    entry("hirsel_rt_v2_profile_enter", Concurrent),
    entry("hirsel_rt_v2_profile_leave", Concurrent),
    // ownership_trace.rs: optional diagnostics, without semantic authority.
    entry("hirsel_rt_v2_ownership_enter", Concurrent),
    entry("hirsel_rt_v2_ownership_event", Concurrent),
    entry("hirsel_rt_v2_ownership_cleanup_begin", Concurrent),
    entry("hirsel_rt_v2_ownership_cleanup_end", Concurrent),
    entry("hirsel_rt_v2_ownership_alias", Concurrent),
    entry("hirsel_rt_v2_ownership_leave", Concurrent),
    // net_abi.rs
    ops("hirsel_rt_v2_net_narrow", HOST),
    ops("hirsel_rt_v2_net_connect", HOST),
    ops("hirsel_rt_v2_net_connect_addr", HOST),
    ops("hirsel_rt_v2_net_resolve", HOST),
    ops("hirsel_rt_v2_net_read", HOST),
    ops("hirsel_rt_v2_net_read_into", HOST),
    ops("hirsel_rt_v2_net_write", HOST),
    ops("hirsel_rt_v2_net_write_all", HOST),
    ops("hirsel_rt_v2_net_shutdown", HOST),
    ops("hirsel_rt_v2_net_split", HOST),
    ops("hirsel_rt_v2_net_join", HOST),
    ops("hirsel_rt_v2_write_half_shutdown", HOST),
    ops("hirsel_rt_v2_net_peer_addr", HOST),
    ops("hirsel_rt_v2_net_close", HOST),
    ops("hirsel_rt_v2_net_listen", HOST),
    ops("hirsel_rt_v2_net_unix_connect", HOST),
    ops("hirsel_rt_v2_net_unix_listen", HOST),
    ops("hirsel_rt_v2_net_local_addr", HOST),
    ops("hirsel_rt_v2_net_accept", HOST),
    ops("hirsel_rt_v2_net_udp_bind", HOST),
    ops("hirsel_rt_v2_udp_send_to", HOST),
    ops("hirsel_rt_v2_udp_recv_from", HOST),
    ops("hirsel_rt_v2_udp_close", HOST),
    // process.rs
    ops("hirsel_rt_v2_proc_run", HOST),
    ops("hirsel_rt_v2_proc_pipeline", HOST),
    ops("hirsel_rt_v2_test_process", HOST),
    ops("hirsel_rt_v2_proc_narrow", HOST),
    ops("hirsel_rt_v2_proc_spawn", HOST),
    ops("hirsel_rt_v2_proc_spawn_at_site", HOST),
    ops("hirsel_rt_v2_child_write", HOST),
    ops("hirsel_rt_v2_child_write_at_site", HOST),
    ops("hirsel_rt_v2_child_close_stdin", HOST),
    ops("hirsel_rt_v2_child_close_stdin_at_site", HOST),
    ops("hirsel_rt_v2_child_read_stdout", HOST),
    ops("hirsel_rt_v2_child_read_stderr", HOST),
    ops("hirsel_rt_v2_child_wait", HOST),
    ops("hirsel_rt_v2_child_wait_at_site", HOST),
    ops("hirsel_rt_v2_child_kill", HOST),
    ops("hirsel_rt_v2_proc_spawn_pipeline", HOST),
    ops("hirsel_rt_v2_proc_spawn_pipeline_at_site", HOST),
    ops("hirsel_rt_v2_proc_wait_group", HOST),
    ops("hirsel_rt_v2_proc_wait_group_at_site", HOST),
    // region.rs
    entry("hirsel_rt_v2_region_owner", Local),
    entry("hirsel_rt_v2_arena_alloc", Local),
    entry("hirsel_rt_v2_arena_push", Local),
    entry("hirsel_rt_v2_arena_reserve", Local),
    entry("hirsel_rt_v2_arena_regrow", Local),
    entry("hirsel_rt_v2_region_enter", Local),
    entry("hirsel_rt_v2_arena_enter", Local),
    entry("hirsel_rt_v2_arena_bound", Local),
    entry("hirsel_rt_v2_arena_exhausted", Local),
    entry("hirsel_rt_v2_arena_recover", Local),
    entry("hirsel_rt_v2_region_reserve", Local),
    entry("hirsel_rt_v2_region_hold", Local),
    entry("hirsel_rt_v2_region_schedule", Local),
    ops("hirsel_rt_v2_region_exit", DROPS),
    entry("hirsel_rt_v2_builder_begin", Local),
    entry("hirsel_rt_v2_builder_grow", Local),
    entry("hirsel_rt_v2_builder_publish", Local),
    // Immutable semantic metadata has its own atomic custody, with no scheduler
    // operation, source credit, task context or user finalizer.
    entry("hirsel_rt_v2_receipt_retain", Concurrent),
    entry("hirsel_rt_v2_receipt_release", Concurrent),
    // Native sealed reads borrow a live owner. Releasing it follows the exact
    // existing Shared-root release policy for each retained occurrence.
    entry("hirsel_rt_v2_sealed_root", Concurrent),
    ops("hirsel_rt_v2_sealed_release", &["root.release"]),
    // Demand-only scope state and bit snapshots are task-local construction.
    // Entered seal retires source credits; exit also closes ordinary cleanup.
    entry("hirsel_rt_v2_receipt_region_enter", Local),
    entry("hirsel_rt_v2_receipt_bits", Local),
    ops("hirsel_rt_v2_seal_prepare_commit", &["root.release"]),
    ops(
        "hirsel_rt_v2_receipt_region_exit",
        &[
            "drop.trivial",
            "drop.leftovers",
            "host.handle_drop",
            "root.release",
        ],
    ),
    // roots.rs: increments commute; releases can free counted frozen storage.
    entry("hirsel_rt_v2_root_dup", Concurrent),
    ops("hirsel_rt_v2_root_release", &["root.release"]),
    ops(
        "hirsel_rt_v2_root_drop",
        &[
            "root.release",
            "drop.trivial",
            "drop.leftovers",
            "host.handle_drop",
        ],
    ),
    // release.rs
    ops("hirsel_rt_v2_free", DROPS),
    ops("hirsel_rt_v2_release", DROPS),
    ops("hirsel_rt_v2_reset", DROPS),
    // A constructor-tail block's header takes its box layout (LANG-10).
    entry("hirsel_rt_v2_hole_close", Local),
    // shared.rs
    entry("hirsel_rt_v2_freeze", Local),
    entry("hirsel_rt_v2_thaw", Local),
    ops("hirsel_rt_v2_thaw_turn", &["shared.thaw"]),
    // tasks.rs
    ops("hirsel_rt_v2_task_scope_enter", &["task.scope_enter"]),
    ops("hirsel_rt_v2_task_scope_enter_until", &["task.scope_until"]),
    ops("hirsel_rt_v2_task_scope_exit", &["task.scope_exit"]),
    ops("hirsel_rt_v2_task_scope_exit_until", &["task.scope_exit"]),
    ops("hirsel_rt_v2_task_spawn", &["task.async"]),
    ops("hirsel_rt_v2_task_spawn_ahead", &["task.async"]),
    ops("hirsel_rt_v2_task_finish", &["task.end"]),
    ops("hirsel_rt_v2_task_join", &["task.join"]),
    ops("hirsel_rt_v2_task_cancel", &["task.cancel"]),
    ops("hirsel_rt_v2_task_detach", &["task.detach"]),
    ops("hirsel_rt_v2_task_set", &["task.set"]),
    ops("hirsel_rt_v2_task_add", &["task.add"]),
    ops("hirsel_rt_v2_task_await_any", &["task.await_any"]),
    ops("hirsel_rt_v2_task_set_has", &["task.set_has"]),
    ops("hirsel_rt_v2_task_set_take", &["task.set_take"]),
    ops("hirsel_rt_v2_task_cancel_all", &["task.cancel_all"]),
    ops("hirsel_rt_v2_task_detach_all", &["task.detach_all"]),
    ops("hirsel_rt_v2_task_canceled", &["task.check_canceled"]),
    ops("hirsel_rt_v2_channel_open", &["channel.open"]),
    ops("hirsel_rt_v2_sender_dup", &["sender.dup"]),
    ops("hirsel_rt_v2_channel_send", &["channel.send"]),
    ops("hirsel_rt_v2_channel_receive", &["channel.receive"]),
    ops("hirsel_rt_v2_mutex_create", &["mutex.create"]),
    // Mutex wrappers close through finalize::close_handle. A last release
    // detaches the inline image before drop.leftovers invokes user cleanup.
    ops("hirsel_rt_v2_mutex_take", &["mutex.take"]),
    ops("hirsel_rt_v2_mutex_put", &["mutex.put"]),
    ops("hirsel_rt_v2_mutex_kill", &["mutex.kill"]),
    ops("hirsel_rt_v2_mutex_cause", &["mutex.cause"]),
    ops("hirsel_rt_v2_atomic_create", &["atomic.create"]),
    ops("hirsel_rt_v2_atomic_load", &["atomic.load"]),
    ops("hirsel_rt_v2_atomic_store", &["atomic.store"]),
    // test_net_abi.rs
    ops("hirsel_rt_v2_test_net", HOST),
    // text.rs
    entry("hirsel_rt_v2_bytes_json_number_end", Local),
    entry("hirsel_rt_v2_bytes_integer", Local),
    entry("hirsel_rt_v2_bytes_decimal_word", Local),
    entry("hirsel_rt_v2_bytes_find_any", Local),
    entry("hirsel_rt_v2_bytes_base64_encode", Local),
    entry("hirsel_rt_v2_bytes_base64_decode", Local),
    entry("hirsel_rt_v2_bytes_sha1", Local),
    entry("hirsel_rt_v2_bytes_sha224", Local),
    entry("hirsel_rt_v2_bytes_sha256", Local),
    entry("hirsel_rt_v2_bytes_sha384", Local),
    entry("hirsel_rt_v2_bytes_sha512", Local),
    entry("hirsel_rt_v2_bytes_sha512_256", Local),
    entry("hirsel_rt_v2_string_from_bytes", Local),
    entry("hirsel_rt_v2_string_slice", Local),
    entry("hirsel_rt_v2_bytes_append", Local),
    entry("hirsel_rt_v2_bytes_replace_range", Local),
    entry("hirsel_rt_v2_bytes_append_from_history", Local),
    entry("hirsel_rt_v2_bytes_slice", Local),
    entry("hirsel_rt_v2_string_position", Local),
    entry("hirsel_rt_v2_string_byte_offset", Local),
    // tls_abi.rs
    ops("hirsel_rt_v2_tls_connect", HOST),
    ops("hirsel_rt_v2_tls_connect_as", HOST),
    ops("hirsel_rt_v2_tls_connect_with", HOST),
    ops("hirsel_rt_v2_tls_negotiated_protocol", HOST),
    ops("hirsel_rt_v2_tls_accept", HOST),
    ops("hirsel_rt_v2_tls_peer", HOST),
    ops("hirsel_rt_v2_tls_close", HOST),
    // views.rs
    entry("hirsel_rt_v2_unicode_scalars_next", Local),
    entry("hirsel_rt_v2_scalar_positions_next", Local),
];

/// The descriptor of the entry exported as `symbol`.
pub fn entry_descriptor(symbol: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.symbol == symbol)
}

/// Every inline effect of compiled code, by the `runtime_abi` word it touches.
pub const INLINE: &[Inline] = &[
    Inline {
        word: "CTX_DUP_MAX_OFFSET",
        locality: Local,
        effect: "inline dup of the task's own blocks",
    },
    Inline {
        word: "CTX_FREE_INLINE_OFFSET",
        locality: Local,
        effect: "inline free of the task's own blocks",
    },
    Inline {
        word: "CTX_IDLE_SLABS_OFFSET",
        locality: Local,
        effect: "inline free's idle-slab delta, folded back into the worker at switch-out",
    },
    Inline {
        word: "CTX_PAGES_OFFSET",
        locality: Local,
        effect: "inline small allocation and free from the task's view of its worker's pages",
    },
    Inline {
        word: "CTX_FUEL_OFFSET",
        locality: Local,
        effect: "the fuel decrement",
    },
    Inline {
        word: "CTX_HEAP_ID_OFFSET",
        locality: Local,
        effect: "inline free's owner check: a block of another worker's heap goes to the runtime",
    },
    Inline {
        word: "CTX_FOREGROUND_COUNT_OFFSET",
        locality: Local,
        effect: "a depth-leaf call's read-only room test of the task's foreground depth",
    },
    Inline {
        word: "CTX_CLEANUP_COUNT_OFFSET",
        locality: Local,
        effect: "a depth-leaf call's read-only room test of the task's cleanup depth",
    },
    Inline {
        word: "CTX_CLEANUP_MODE_OFFSET",
        locality: Local,
        effect: "a depth-leaf call's read-only test of the task's finalizer nesting",
    },
    Inline {
        word: "COUNT_OFFSET",
        locality: Local,
        effect: "inline count increment and decrement of a block the task holds",
    },
    Inline {
        word: "REGION_CURSOR_OFFSET",
        locality: Local,
        effect: "inline bump allocation in a region the task owns",
    },
    Inline {
        word: "REGION_END_OFFSET",
        locality: Local,
        effect: "inline bump allocation in a region the task owns",
    },
    Inline {
        word: "REGION_REMAINING_OFFSET",
        locality: Local,
        effect: "a bounded arena's remaining logical bytes",
    },
];

/// The effects a scheduling point performs for parked tasks (§4.1 Footprint): its polls.
pub const SCHEDULING_POINT: &[Access] = &[
    Read(Object(Channel)),
    Read(Object(Mutex)),
    Read(Object(Task)),
    Read(Object(TaskSet)),
    Read(CompletionRank),
    Read(ParkedCancelStates),
    Write(HostKeys),
];

/// The ops each task, channel, mutex, atomic and `Shared` operation of the core registry
/// performs natively. An empty list is task-local work.
pub const REGISTRY: &[(&str, &[&str])] = &[
    ("atomic.create", &["atomic.create"]),
    ("atomic.load", &["atomic.load"]),
    ("atomic.store", &["atomic.store"]),
    ("channel.open", &["channel.open"]),
    ("channel.receive", &["channel.receive"]),
    ("channel.send", &["channel.send"]),
    ("mutex.create", &["mutex.create"]),
    (
        "mutex.with",
        &["mutex.take", "mutex.put", "mutex.kill", "mutex.cause"],
    ),
    ("sender.dup", &["sender.dup"]),
    ("shared.freeze", &[]),
    ("shared.thaw", &["shared.thaw"]),
    ("shared.with", &[]),
    ("task.add", &["task.add"]),
    ("task.async", &["task.async"]),
    ("task.await", &["task.join"]),
    ("task.await_any", &["task.await_any"]),
    ("task.await_at", &["task.join"]),
    ("task.cancel", &["task.cancel"]),
    ("task.cancel_all", &["task.cancel_all"]),
    ("task.check_canceled", &["task.check_canceled"]),
    ("task.detach", &["task.detach"]),
    ("task.detach_all", &["task.detach_all"]),
    ("task.isolate", &["task.isolate"]),
    ("task.scope", &["task.scope_enter", "task.scope_exit"]),
    ("task.scope_until", &["task.scope_until", "task.scope_exit"]),
    ("task.set", &["task.set"]),
    ("$intrinsic.TaskScopeEnter", &["task.scope_enter"]),
    ("$intrinsic.TaskScopeExit", &["task.scope_exit"]),
    ("$intrinsic.TaskScopeEnterUntil", &["task.scope_until"]),
    ("$intrinsic.TaskScopeExitUntil", &["task.scope_exit"]),
    ("$intrinsic.TaskSpawn", &["task.async"]),
    ("$intrinsic.TaskFinish", &["task.end"]),
    ("$intrinsic.TaskJoin", &["task.join"]),
    ("$intrinsic.TaskSetHas", &["task.set_has"]),
    ("$intrinsic.TaskSetTake", &["task.set_take"]),
    ("$intrinsic.TaskCanceled", &["task.check_canceled"]),
    ("$intrinsic.ChannelOpen", &["channel.open"]),
    ("$intrinsic.ChannelSend", &["channel.send"]),
    ("$intrinsic.ChannelReceive", &["channel.receive"]),
    ("$intrinsic.MutexCreate", &["mutex.create"]),
    ("$intrinsic.MutexTake", &["mutex.take"]),
    ("$intrinsic.MutexPut", &["mutex.put"]),
    ("$intrinsic.MutexKill", &["mutex.kill"]),
    ("$intrinsic.MutexCause", &["mutex.cause"]),
    ("$intrinsic.AtomicCreate", &["atomic.create"]),
    ("$intrinsic.AtomicLoad", &["atomic.load"]),
    ("$intrinsic.AtomicStore", &["atomic.store"]),
    ("$intrinsic.SharedThawTurn", &["shared.thaw"]),
];

/// The registry families [`REGISTRY`] must cover.
pub const REGISTRY_FAMILIES: &[&str] = &[
    "task.",
    "channel.",
    "sender.",
    "receiver.",
    "mutex.",
    "atomic.",
    "shared.",
    "$intrinsic.Task",
    "$intrinsic.Channel",
    "$intrinsic.Mutex",
    "$intrinsic.Atomic",
    "$intrinsic.Shared",
];
