//! The task context `TaskCtx`, passed explicitly to every entry: one task's own state, its
//! worker (heap and region pool) and its run's shared state (configuration, scheduler, Io
//! binding). It is not a source capability and conveys no authority. Runs share nothing
//! (OWN-6). The Io adapter belongs to the library; the run holds only its handle close, which
//! handle drops call.

use std::ptr;

#[cfg(any(feature = "diagnostic", feature = "measurement"))]
use crate::alloc::SLAB_SIZE;
use crate::alloc::{Header, Heap, View};
use crate::count::finite_count_max;
use crate::failure::FailureState;
use crate::finalize::CloseThunk;
use crate::region::{REGION_POOL_IDLE_CAP, RegionPool};
use crate::tasks::role::cell::RoleCell;
use crate::{ABI_VERSION, fault};

/// `Config::flags`: diagnostic mode (descriptor, header and state checks; payload poisoning).
pub const CONFIG_DIAGNOSTIC: u32 = 1;

/// Launch status: the context was created.
pub const LAUNCH_OK: u8 = 0;
/// Launch status: the caller was compiled against another interface version.
pub const LAUNCH_ABI_MISMATCH: u8 = 2;
/// Launch status: a configuration value is out of range.
pub const LAUNCH_INVALID_CONFIG: u8 = 3;

/// Context configuration.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Must equal [`ABI_VERSION`].
    pub abi_version: u32,
    pub flags: u32,
    /// The count bound `max` of D's model: finite counts are `1 .. max - 1` and a dup that would
    /// reach `max` saturates. At least 2; 0 selects the full width. Small only in tests.
    pub count_max: u64,
    /// Idle slab bytes kept at each trim point; 0 selects the default (1 MiB).
    pub heap_idle_cap: u64,
    /// The Io adapter's handle close; without one a handle drop is an invariant fault.
    pub close_handle: Option<CloseThunk>,
    /// The semantic hash seed of this run (SEM-5): it keys `hirsel_rt_v2_hash_begin`, so it
    /// moves hash-table placement and nothing a program can observe.
    pub hash_seed: u64,
    /// The linked program's immutable descriptor table; runtime descriptors occupy indices 0/1
    /// (0: the plain pointer-free sequence for Bytes and `List[SocketAddr]`; 1: file walk) and
    /// the index after the table (the runtime's `List[Bytes]`, `text::bytes_list_index`).
    pub layouts: *const *const crate::Layout,
    pub layout_count: u32,
    pub reserved: u32,
}

impl Config {
    /// The production configuration.
    pub const fn new() -> Config {
        Config {
            abi_version: ABI_VERSION,
            flags: 0,
            count_max: 0,
            heap_idle_cap: 0,
            close_handle: None,
            hash_seed: 0,
            layouts: ptr::null(),
            layout_count: 0,
            reserved: 0,
        }
    }
}

impl Default for Config {
    fn default() -> Config {
        Config::new()
    }
}

/// A snapshot of the context's storage.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg(any(feature = "diagnostic", feature = "measurement"))]
pub struct Stats {
    /// Bytes held from the backing allocator: slabs, large blocks and region segments.
    pub backing_bytes: u64,
    /// Backing allocator calls made so far.
    pub backing_allocations: u64,
    pub slab_bytes: u64,
    /// Bytes of slabs holding no live block.
    pub idle_slab_bytes: u64,
    pub large_bytes: u64,
    /// Blocks not on a free list (live, draining or raw tokens) and their capacity bytes.
    pub live_blocks: u64,
    pub live_bytes: u64,
    pub region_active_bytes: u64,
    pub region_pool_bytes: u64,
    pub active_regions: u64,
    /// Blocks made immortal by count saturation (permitted retained allocations).
    pub immortalized_blocks: u64,
}

/// What task exit found and returned.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Audit {
    /// Blocks still allocated at exit (leaks, immortal and static-reachable blocks) and bytes.
    pub live_blocks: u64,
    pub live_bytes: u64,
    pub immortalized_blocks: u64,
    pub active_regions: u64,
    /// Backing bytes still held after exit; always 0.
    pub retained_bytes: u64,
}

/// One task's context: the pointer compiled code receives, one per started task. Opaque to C,
/// except for the words compiled fast paths read at its start (its [`View`] of the worker's
/// `crate::alloc::Compiled` words, runtime ABI 4), which is why it is `repr(C)` with the view
/// first. Behind it: what is the task's own (its failure chain and isolate scratch, finalizer
/// shield, release worklist, innermost region, freeze/thaw stack and foreign scope), the
/// [`Worker`] running it and the run's [`RunShared`] state.
///
/// A compiled frame's context names its task, never its worker, so it stays valid wherever the
/// task runs. Runtime code reaches the worker only through [`TaskCtx::worker`] (and
/// [`TaskCtx::heap`]), which borrows the context: every entry that can suspend the task takes
/// `&mut TaskCtx`, so no worker borrow lives across a suspension.
#[repr(C)]
pub struct TaskCtx {
    pub(crate) view: View,
    /// LANG-10 counters at fixed offsets after the view (runtime ABI 65,
    /// `CTX_FOREGROUND_COUNT_OFFSET`): a depth-leaf callee's room check reads them.
    pub(crate) activations: hirsel_core_contract::depth::Activations,
    pub(crate) cleanup_depth: u32,
    worker: *mut Worker,
    run: *mut RunShared,
    /// Stable scheduler identity and save slot. The task table may grow while
    /// another worker saves this fiber, so a save never points into its Vec.
    pub(crate) task_id: u32,
    pub(crate) task_sp: *mut u8,
    pub(crate) role_depth: u32,
    /// Top of the release worklist (an intrusive stack of dead headers and continuations).
    pub(crate) work: *mut Header,
    pub(crate) shield: u32,
    pub(crate) failure: FailureState,
    /// The innermost region this task entered and has not exited.
    pub(crate) region: *mut crate::region::Region,
    /// The freeze and thaw walks' block stack, kept between walks (MEM-11).
    pub(crate) frozen: Vec<*mut Header>,
    /// The task's root releases that have not taken effect yet (`crate::roots`).
    pub(crate) roots: crate::roots::TaskRoots,
    /// The chain the task's last `task.isolate` caught, which its body reads (`isolate.rs`).
    pub(crate) isolated: Vec<crate::failure::Chained>,
    /// The task's innermost extern call while it is switched out (`crate::foreign`).
    foreign: crate::foreign::ForeignScope,
    /// One bit per active callback of the task, innermost lowest: whether its entry took a
    /// compute permit, which its leave gives back (`crate::foreign`).
    pub(crate) callback_permits: u64,
    pub(crate) callback_depth: u32,
    /// Owner-local native recorder and ordinal. It moves with this task;
    /// cutters read only its stable shared log, never the context.
    pub(crate) recording: Option<Box<crate::tasks::free::record::Task>>,
    /// Meter callbacks reach only these owner-local cells while a heap is
    /// borrowed. Policy accounting and every possible pause happen outside it.
    pub(crate) ordered_allocation: std::cell::Cell<u64>,
    pub(crate) ordered_freed: std::cell::Cell<u64>,
    /// The task's measurement site while it is switched out.
    #[cfg(feature = "measurement")]
    site: u64,
    /// Detached Free object images retained across yielding finalizers and
    /// returned by context destruction when a stop abandons their frames.
    pub(crate) detached_images: crate::tasks::free::cleanup::Images,
    /// A native Free request retains its buffers and late completion even
    /// when its task's host-call frame is abandoned after a terminal cut.
    pub(crate) pending_host: Option<Box<crate::tasks::free::host::Pending>>,
    pub(crate) live_host: Option<Box<crate::tasks::free::host::live::Pending>>,
    pub(crate) free_terminal: Option<Vec<u8>>,
    /// Native Free result across its post-effect quantum, owned outside Rust frames.
    pub(crate) native_host_result: Option<Box<Result<crate::io::Completion, ()>>>,
    /// Replay holds request and transition buffers across admitted-prefix saves.
    pub(crate) replay_host: Option<Box<crate::tasks::free::host::ReplayHost>>,
    pub(crate) ordered_charge_trace: crate::budget::stretch_trace::Local,
    /// Owned Ordered request/result buffers retained across frontier saves.
    pub(crate) pending_ordered_host: Option<Box<crate::tasks::ordered::host::concurrent::Pending>>,
    /// The actual source invocation of a site-carrying host primitive. Task-owned
    /// across a physical save; nested calls restore their caller's site.
    pub(crate) operation_site: u64,
    /// A Free replay op whose compact frame left its stack while it waited
    /// for its version: the reissued op resumes the wait after its entry
    /// (`compact::wait`).
    pub(crate) wait_left: bool,
    /// Free ops begun since the task's last resume point whose operation can
    /// leave ([`crate::tasks::compact::point`]; saturated otherwise): an op
    /// that leaves is that boundary's first, so the reissued operation
    /// repeats no effect (`compact::can_wait`).
    pub(crate) point_ops: u32,
    /// Private execution ownership; never part of the compiled context view.
    pub(crate) ahead: Option<Box<crate::tasks::ordered::ahead::Local>>,
    pub(crate) ahead_physical: *mut crate::tasks::ordered::ahead::native::Physical,
    pub(crate) active_invocations:
        crate::depth::storage::Slots<hirsel_core_contract::depth::Activation>,
    /// User finalizers retained through their field programs. Allocated at task creation.
    pub(crate) cleanup_invocations: crate::depth::storage::Cleanup,
    /// Rejected entries cannot execute code; their nested field scopes need no tokens.
    pub(crate) cleanup_rejections: usize,
    /// Invoking language sites parallel to admitted compiled invocations. Runtime
    /// callback boundaries inherit these sites without introducing an IR selector.
    pub(crate) invocation_sites: crate::depth::storage::Sites,
    /// Initialized prefix of both fixed-capacity native invocation arrays.
    pub(crate) invocation_depth: usize,
    /// Stable pointers into the owned scratch above for the certified naked leaf.
    /// Never serialized or copied to another task; the owner lives with this task.
    pub(crate) invocation_tokens:
        *mut std::mem::MaybeUninit<hirsel_core_contract::depth::Activation>,
    pub(crate) invocation_site_slots: *mut std::mem::MaybeUninit<[u64; 2]>,
    #[cfg(test)]
    pub(crate) test_host_tail: bool,
    /// Free operation state only the running task touches: its quantum,
    /// deferred child and resolved object leases (`tasks::free::objects`).
    pub(crate) free_ops: crate::tasks::free::objects::OpLocal,
    /// A compact frame the shared dispatcher owns off any stack, between its
    /// save and its activation (`crate::continuation::Compact`).
    pub(crate) compact: Option<Box<crate::continuation::Compact>>,
    /// LIFO catches owned by their invocation, including stackful wrappers.
    pub(crate) isolations: crate::isolate::Invocations,
    #[cfg(feature = "ownership-trace")]
    pub(crate) ownership_cleanup: *mut crate::ownership_trace::CleanupScope,
    pub(crate) resume_bindings: crate::continuation::binding::Bindings,
    pub(crate) ordered_inop_leave: Option<bool>,
}

const _: () = assert!(std::mem::offset_of!(TaskCtx, view) == 0);
pub const CTX_FOREGROUND_COUNT_OFFSET: usize = std::mem::offset_of!(TaskCtx, activations);
pub const CTX_CLEANUP_COUNT_OFFSET: usize = CTX_FOREGROUND_COUNT_OFFSET + 2;
pub const CTX_CLEANUP_MODE_OFFSET: usize = std::mem::offset_of!(TaskCtx, cleanup_depth);
const _: () = assert!(
    CTX_FOREGROUND_COUNT_OFFSET == crate::CTX_HEAP_ID_OFFSET + 24
        && CTX_CLEANUP_MODE_OFFSET == CTX_FOREGROUND_COUNT_OFFSET + 4
);

/// One OS thread that runs Hirsel code, and what is its own: its heap (with its region pool),
/// which lives in the run's registry and outlives the worker, and its idle fiber stacks.
pub(crate) struct Worker {
    pub(crate) id: u32,
    heap: *mut Heap,
    run: *mut RunShared,
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) immortalized: u64,
    /// Stacks of completed tasks, for the next spawns.
    pub(crate) idle_stacks: crate::fiber::StackPool,
    /// The stack of a task that completed; the next task to run recycles it.
    pub(crate) retired: Option<crate::fiber::Stack>,
}

impl Worker {
    /// A new worker of `run`, with an orphaned heap it re-adopts or a fresh one from the run's
    /// registry (design §6 Heaps).
    ///
    /// # Safety
    /// `run` is live and outlives the worker.
    pub(crate) unsafe fn create(run: *mut RunShared) -> *mut Worker {
        // SAFETY: caller contract.
        let heaps = unsafe { &(*run).heaps };
        let id = heaps.worker_id();
        let heap = heaps.attach(id);
        Box::into_raw(Box::new(Worker {
            id,
            heap,
            run,
            #[cfg(any(feature = "diagnostic", feature = "measurement"))]
            immortalized: 0,
            idle_stacks: Default::default(),
            retired: None,
        }))
    }

    /// The worker leaves: its heap stays in the registry, orphaned, and any live worker
    /// collects it from then on.
    ///
    /// # Safety
    /// `worker` came from [`Worker::create`], runs no task and is not used again.
    pub(crate) unsafe fn retire(worker: *mut Worker) {
        // SAFETY: caller contract.
        unsafe {
            let worker = Box::from_raw(worker);
            (*worker.run).heaps.retire(worker.heap);
        }
    }

    /// The worker's heap.
    #[inline(always)]
    pub(crate) fn heap(&mut self) -> &mut Heap {
        // SAFETY: the run's registry keeps the heap while the worker lives, and only its
        // worker reaches it through this borrow.
        unsafe { &mut *self.heap }
    }

    /// A scheduling point: take back what other heaps published to this worker's heap, and
    /// collect orphaned heaps and return excess idle slabs.
    #[inline]
    pub(crate) fn collect(&mut self) {
        // mimalloc theap.c:124-147 (31d034d94cdb): collection also visits
        // retired pages. Enforce the idle bound even when a long-lived task
        // never exits a compiler region; trim's under-cap path is constant work.
        self.heap().trim();
        // SAFETY: the run outlives its workers.
        unsafe { (*self.run).heaps.collect_orphans(self.id) };
    }
}

/// What every task of a run shares: its configuration, scheduler, Io binding, failure table and
/// launcher.
pub(crate) struct RunShared {
    pub(crate) role: crate::tasks::role::Role,
    pub(crate) count_max: u32,
    pub(crate) close_handle: Option<CloseThunk>,
    pub(crate) hash_seed: u64,
    io_data: std::sync::atomic::AtomicPtr<std::ffi::c_void>,
    /// The bound Io binding's Path identities and launch arguments (`crate::paths`).
    pub(crate) names: Option<std::sync::Arc<crate::paths::Names>>,
    /// Live private sealed control allocations, registered during preparation.
    /// Check membership before dereferencing an opaque C owner, including after
    /// release. Values bind the exact allocation generation within this run.
    pub(crate) sealed_owners: std::sync::Mutex<crate::sealed_region::Owners>,
    pub(crate) region_owners:
        std::sync::Mutex<std::collections::BTreeMap<usize, crate::region::OwnerRecord>>,
    pub(crate) entry_result: RoleCell<Option<crate::exit_report::Returned>>,
    /// The run's tasks, from its first scope (CONC-2).
    pub(crate) tasks: RoleCell<Option<Box<crate::tasks::Scheduler>>>,
    /// Where a stop of a width-1 run returns (`crate::stop`).
    pub(crate) driver: RoleCell<crate::stop::Driver>,
    /// The root run's execution of runner calls and the callee runs they start (RUN-10).
    pub(crate) runners: RoleCell<Option<Box<crate::runners::Execution>>>,
    /// Immutable child-local runner data. Published before dispatch; workers
    /// never follow a pointer into a parent's home-owned execution.
    pub(crate) runner_call: Option<std::sync::Arc<crate::runners::native::CallState>>,
    /// Send-only runner transport and a home-only service location. The actual
    /// service remains on admission, so detached workers never destroy its Io.
    pub(crate) runner_home: Option<std::sync::Arc<crate::runners::native::Slot>>,
    /// A callee run's execution root, whose launcher a stop returns to; null for a root.
    pub(crate) root: *mut RunShared,
    /// The run's stop word and what its host asks of it (`crate::stop`).
    pub(crate) control: std::sync::Arc<crate::stop::Control>,
    /// The Io binding's parked requests and armed deadlines, mirrored where every worker reads
    /// it (`crate::host_waits`).
    pub(crate) io_facade: RoleCell<Option<Box<crate::io::Binding>>>,
    pub(crate) host_waits: std::sync::atomic::AtomicU32,
    pub(crate) timer_hint: crate::host_waits::TimerHint,
    /// The root task's entry, words and stack, owned by the run (`crate::stop`).
    pub(crate) launch: RoleCell<Option<Box<crate::stop::LaunchFrame>>>,
    /// The run's root context and its first worker (the home thread's), which a detached
    /// run's last thread frees ([`reclaim`]).
    root_ctx: *mut TaskCtx,
    home: *mut Worker,
    /// The running program's failure table (`crate::isolate`).
    pub(crate) failure_table: Option<crate::isolate::FailureTable>,
    /// The scheduler policy the run's tasks start with (`inputs.scheduler`).
    pub(crate) schedule: hirsel_kernels::schedule::Schedule,
    /// Free mode's session and test channel (`crate::free`).
    pub(crate) free: RoleCell<Option<Box<crate::free::Free>>>,
    /// The admitted tasks.free@1 policy, independently of recording or
    /// physical dispatch; false for Ordered and the cooperative test harness.
    pub(crate) free_mode: bool,
    /// The Free logical policy without replay, fixed at admission: immediate
    /// object operations may then act on resolved leases without the role.
    pub(crate) free_objects: std::sync::atomic::AtomicBool,
    /// Fixed at native replay admission before any task starts.
    pub(crate) free_replay: bool,
    /// Ordinary configured native Free admission, distinct from F0/BodyRun tests.
    pub(crate) free_production: bool,
    pub(crate) free_root_env: bool,
    /// The run's native recording, set at root admission before any executor starts: each
    /// executor registers its slot here when it starts serving the run.
    pub(crate) recording:
        std::sync::OnceLock<std::sync::Arc<crate::tasks::free::record::Recording>>,
    pub(crate) free_namespace_coupled: std::sync::atomic::AtomicBool,
    pub(crate) free_return: (u64, u64),
    /// Published at ordered admission, read without borrowing policy state.
    pub(crate) ordered_mode: std::sync::atomic::AtomicBool,
    /// Fast allocation preflight selector; MAX means no physical allowance cap.
    pub(crate) ordered_allowance: std::sync::atomic::AtomicU64,
    pub(crate) ordered_memory: std::sync::atomic::AtomicBool,
    /// Immutable compiler code identities, installed before dispatch. No
    /// pointer value is ever included in canonical initialized-field output.
    pub(crate) value_codes: std::sync::Arc<std::collections::BTreeMap<usize, String>>,
    /// The exact counts of roots past the finite maximum (`crate::roots`).
    pub(crate) root_overflow: crate::roots::Overflow,
    /// Every worker's heap, until the run ends (`crate::heaps`).
    pub(crate) heaps: Box<crate::heaps::HeapRegistry>,
    /// The fuel not yet handed out as chunks (`crate::budget`).
    pub(crate) fuel: crate::budget::FuelPool,
    /// The home thread and blocking-pool threads, from the first pool dispatch
    /// (`crate::pool`).
    pub(crate) execs: RoleCell<Option<Box<crate::pool::Execs>>>,
    /// The extern row's headroom counters, when metrics are enabled (`crate::pool`).
    pub(crate) extern_metrics: Option<std::sync::Arc<crate::pool::ExternMetrics>>,
    /// The scheduling rows' headroom counters, when metrics are enabled (`crate::pool`).
    pub(crate) worker_metrics: Option<std::sync::Arc<crate::pool::WorkerMetrics>>,
    #[cfg(feature = "measurement")]
    pub(crate) measurement_output: RoleCell<Option<std::fs::File>>,
    #[cfg(feature = "measurement")]
    pub(crate) mc_profile: RoleCell<Option<crate::tasks::profile::Profile>>,
    #[cfg(feature = "measurement")]
    pub(crate) profile_output: RoleCell<Option<std::fs::File>>,
    /// Trace admission only: concurrent Shared-root provenance, never a role lookup.
    pub(crate) ordered_roots: std::sync::OnceLock<crate::ordered::root_ids::Registry>,
    /// Ordered concurrent requests retain their adapter through saved waits.
    pub(crate) ordered_io:
        std::sync::OnceLock<std::sync::Arc<crate::tasks::ordered::host::concurrent::Service>>,
    /// Optional fixed Ordered commit thread. Its callback loans are synchronous.
    pub(crate) ordered_commit: std::sync::OnceLock<crate::pool::ordered_commit::Service>,
    /// Trace-only bounded home results that have not transferred to their issuer.
    pub(crate) ordered_home_results:
        std::sync::Mutex<std::collections::BTreeMap<u32, crate::io_home::Snapshot>>,
    /// Explicit Free host admission: owned concurrent requests and driver wake.
    pub(crate) free_io:
        std::sync::OnceLock<std::sync::Arc<crate::tasks::free::host::concurrent::Service>>,
    /// Explicit native Free host routing; production admission remains gated.
    pub(crate) free_native_io: std::sync::atomic::AtomicBool,
    /// Native telemetry context derives from admitted span creation, without host calls.
    pub(crate) free_native_spans: crate::tasks::free::host::native::Spans,
    #[cfg(feature = "operation-latency")]
    pub(crate) operation_latency: Option<crate::operation_latency::Observer>,
    #[cfg(feature = "ownership-trace")]
    pub(crate) ownership_trace: Option<crate::ownership_trace::Observer>,
}

impl Drop for RunShared {
    fn drop(&mut self) {
        #[cfg(feature = "ownership-trace")]
        if let Some(observer) = self.ownership_trace.take() {
            observer.write();
        }
        #[cfg(feature = "operation-latency")]
        if !self.free_replay {
            if let Some(observer) = &self.operation_latency {
                observer.write();
            }
        }
        // The detached reclaim path also drops the heaps wholesale without
        // TaskCtx::exit. Its last worker owns quiescent metadata at this point.
        if let Some(scheduler) = self.tasks.as_deref_mut() {
            scheduler.restore_reserved_custody();
        }
    }
}

impl RunShared {
    pub(crate) fn root_context(&self) -> *mut TaskCtx {
        self.root_ctx
    }
    pub(crate) fn io_binding(&self) -> *mut std::ffi::c_void {
        self.io_data.load(std::sync::atomic::Ordering::Acquire)
    }

    /// The home thread's worker, the run's first.
    pub(crate) fn home_worker(&self) -> *mut Worker {
        self.home
    }
}

impl TaskCtx {
    /// A fresh task's context on `worker`, in `run`; nothing binds it yet.
    pub(crate) fn new(worker: *mut Worker, run: *mut RunShared) -> TaskCtx {
        // SAFETY: all extents move and drop with this context. The Sites owner
        // remains allocated until no extent can be used, including on stop.
        let (mut active_invocations, cleanup_invocations, mut invocation_sites) =
            unsafe { crate::depth::storage::Sites::new() };
        let invocation_tokens = active_invocations.as_mut_ptr();
        let invocation_site_slots = invocation_sites.as_mut_ptr();
        TaskCtx {
            view: View::new(),
            worker,
            run,
            task_id: 0,
            task_sp: ptr::null_mut(),
            role_depth: 0,
            work: ptr::null_mut(),
            shield: 0,
            failure: FailureState::default(),
            region: ptr::null_mut(),
            frozen: Vec::new(),
            roots: Default::default(),
            isolated: Vec::new(),
            isolations: Default::default(),
            foreign: crate::foreign::ForeignScope::NONE,
            callback_permits: 0,
            callback_depth: 0,
            recording: None,
            ordered_allocation: Default::default(),
            ordered_freed: Default::default(),
            #[cfg(feature = "measurement")]
            site: 0,
            detached_images: Default::default(),
            pending_host: None,
            live_host: None,
            free_terminal: None,
            pending_ordered_host: None,
            operation_site: 0,
            ahead: None,
            ahead_physical: std::ptr::null_mut(),
            #[cfg(test)]
            test_host_tail: false,
            native_host_result: None,
            wait_left: false,
            point_ops: u32::MAX,
            replay_host: None,
            ordered_charge_trace: Default::default(),
            activations: Default::default(),
            active_invocations,
            cleanup_depth: 0,
            cleanup_invocations,
            cleanup_rejections: 0,
            free_ops: Default::default(),
            compact: None,
            #[cfg(feature = "ownership-trace")]
            ownership_cleanup: std::ptr::null_mut(),
            resume_bindings: Default::default(),
            ordered_inop_leave: None,
            invocation_sites,
            invocation_depth: 0,
            invocation_tokens,
            invocation_site_slots,
        }
    }

    /// The worker running this task.
    #[inline(always)]
    pub(crate) fn worker(&mut self) -> &mut Worker {
        // SAFETY: a task's worker outlives its run on that worker, and only the task it runs
        // reaches it through this borrow of the task's context.
        unsafe { &mut *self.worker }
    }
    /// The running worker's heap.
    #[inline(always)]
    pub(crate) fn heap(&mut self) -> &mut Heap {
        self.worker().heap()
    }
    /// The running worker's heap, read only.
    #[inline(always)]
    pub(crate) fn heap_ref(&self) -> &Heap {
        // SAFETY: as for `worker`.
        unsafe { &*(*self.worker).heap }
    }
    /// The running worker's region pool.
    #[inline(always)]
    pub(crate) fn pool(&mut self) -> &mut RegionPool {
        &mut self.heap().regions
    }

    /// Return a segment of a region that exited on this task's worker to the pool that lent
    /// it (`crate::region`).
    ///
    /// # Safety
    /// `segment` is lent to a region of this run that exited.
    pub(crate) unsafe fn return_segment(&mut self, segment: *mut crate::region::Segment) {
        let run = self.run;
        // SAFETY: caller contract; the run's registry holds every heap of the run.
        unsafe {
            self.pool()
                .return_segment(segment, |id| &raw mut (*(*run).heaps.heap(id)).regions)
        }
    }
    /// Exclusive access during admission or after every worker has quiesced.
    /// Runtime entries must use `run_ref` and project role-owned fields through
    /// `RoleCell::get`; owning the scheduler role alone does not permit an
    /// exclusive borrow of the whole run while other services read it.
    #[inline(always)]
    pub(crate) fn run(&mut self) -> &mut RunShared {
        // SAFETY: callers establish admission or quiescence as described above;
        // the run outlives each of its tasks.
        unsafe { &mut *self.run }
    }
    /// The run this task belongs to, read only.
    #[inline(always)]
    pub(crate) fn run_ref(&self) -> &RunShared {
        // SAFETY: the run outlives this task; live mutations use interior mutability.
        unsafe { &*self.run }
    }
    /// The run's state, without borrowing the task's context.
    pub(crate) fn run_ptr(&self) -> *mut RunShared {
        self.run
    }
    /// The worker running this task, without borrowing the task's context: what a switch
    /// hands to the next task.
    pub(crate) fn worker_ptr(&self) -> *mut Worker {
        self.worker
    }

    /// Switch-in: this task runs on `worker` from here on. Its view takes the worker's pages
    /// (when stale) and counters, and the thread's running context and foreign scope become
    /// its own.
    ///
    /// # Safety
    /// `ctx` and `worker` are live; no other task is bound to `worker`.
    #[inline(never)]
    pub(crate) unsafe fn switch_in(ctx: *mut TaskCtx, worker: *mut Worker) {
        // SAFETY: caller contract.
        unsafe {
            TaskCtx::attach(ctx, worker);
            if (*ctx)
                .run_ref()
                .ordered_mode
                .load(std::sync::atomic::Ordering::Acquire)
            {
                crate::tasks::ordered::engine::fuel_in(ctx);
            } else if crate::pool::wide((*ctx).run_ref()) && (*ctx).run_ref().fuel.active() {
                let chunk = (*ctx).run_ref().fuel.take();
                (*ctx).heap().set_fuel(chunk as i64);
            }
            crate::stop::set_running(ctx);
            crate::foreign::restore((*ctx).foreign);
            (*worker).collect();
        }
    }

    /// Bind `ctx` to `worker`'s heap, as [`TaskCtx::switch_in`] does, leaving the thread's
    /// running context and foreign scope alone: a context created outside its run's launcher.
    ///
    /// # Safety
    /// As for [`TaskCtx::switch_in`].
    pub(crate) unsafe fn attach(ctx: *mut TaskCtx, worker: *mut Worker) {
        // SAFETY: caller contract.
        unsafe {
            (*ctx).worker = worker;
            match (*(*ctx).run).worker_metrics.as_deref() {
                Some(metrics) if (*worker).heap().stale(&(*ctx).view) => {
                    let started = std::time::Instant::now();
                    (*worker).heap().bind(&raw mut (*ctx).view);
                    metrics.refresh(started.elapsed());
                }
                _ => (*worker).heap().bind(&raw mut (*ctx).view),
            }
            #[cfg(feature = "measurement")]
            {
                (*worker).heap().measurement.active_site = (*ctx).site;
            }
        }
    }

    /// Switch-out: the running task leaves its worker. Its view's counters fold back into the
    /// worker, and it keeps the thread's foreign scope until it is switched in again.
    #[inline(never)]
    pub(crate) fn switch_out(&mut self) {
        crate::tasks::role::unlent("a fiber switch");
        if self
            .run_ref()
            .ordered_mode
            .load(std::sync::atomic::Ordering::Acquire)
        {
            crate::tasks::ordered::engine::fuel_out(self);
        } else if crate::pool::wide(self.run_ref()) && self.run_ref().fuel.active() {
            let points = self.view.compiled.fuel.max(0) as u64;
            self.run_ref().fuel.put(points);
            self.heap().set_fuel(0);
        }
        self.heap().unbind();
        #[cfg(feature = "measurement")]
        {
            self.site = std::mem::take(&mut self.heap().measurement.active_site);
        }
        self.foreign = crate::foreign::save();
    }

    /// Rebind a root context whose bytes a harness moved (swapped with another context's)
    /// before its run started: its worker's binding names the old place.
    ///
    /// # Safety
    /// `ctx` is a live root context of no started run.
    #[doc(hidden)]
    pub unsafe fn moved(ctx: *mut TaskCtx) {
        // SAFETY: caller contract.
        unsafe { TaskCtx::attach(ctx, (*ctx).worker) };
    }

    /// The task bound to this task's worker: the running one.
    pub(crate) fn running(&self) -> *mut TaskCtx {
        self.heap_ref().bound().cast()
    }

    /// Bind the run's root to the home worker after a width > 1 run's threads left
    /// (`crate::pool::shutdown`): the root was switched out on whichever thread ran it last.
    pub(crate) fn home_again(&mut self) {
        let home = self.run_ref().home_worker();
        // SAFETY: the run's threads left, so nothing runs on the home worker.
        unsafe {
            if (*home).heap().bound() != (&raw mut self.view) {
                TaskCtx::attach(self, home);
            } else {
                // Its view is home's already, but its worker may be a compute worker, which
                // the run's exit retires.
                self.worker = home;
            }
        }
    }

    /// Make this task the running one on its worker again, after a stop left another bound.
    pub(crate) fn rebind(&mut self) {
        let running = self.running();
        if !std::ptr::eq(running, self) {
            // SAFETY: the bound task's context is live until the run's tasks are dropped, and
            // this one is the run's root.
            unsafe {
                (*running).switch_out();
                let worker = self.worker;
                TaskCtx::switch_in(self, worker);
            }
        }
    }

    #[inline]
    pub(crate) unsafe fn descriptor(&self, index: u32) -> *const crate::Layout {
        unsafe { self.heap_ref().descriptor(index) }
    }
    /// Bind one explicit adapter to this context's run; null unbinds it.
    ///
    /// # Safety
    /// The adapter outlives every release on this run. Close-handle may run only
    /// while the adapter is bound. No simultaneous access to the adapter is allowed.
    pub unsafe fn bind_io_data(&mut self, data: *mut std::ffi::c_void) {
        self.run_ref()
            .io_data
            .store(data, std::sync::atomic::Ordering::Release);
    }
    /// Apply the run's budgets (`budgets.limits@2`) before its entry runs, after its workers
    /// start. At width 1 without a host's stop handle the fuel pool hands the whole budget out
    /// as one chunk; the running
    /// task's word is emptied, so its next fuel point takes that chunk. Under a memory budget
    /// every heap of the run stops allocating and freeing inline, so the meter sees each one.
    pub(crate) fn set_budgets(&mut self, fuel: Option<u64>, memory_bytes: Option<u64>) {
        // A refill is where a loop observes a stop: a run with compute workers or a host's
        // stop handle takes its fuel in chunks.
        let whole = !crate::pool::wide(self.run_ref()) && !self.run_ref().control.hosted();
        self.run().fuel = crate::budget::FuelPool::new(fuel, whole);
        self.heap().set_fuel(0);
        let limit = memory_bytes.unwrap_or(0);
        self.heap().meter.set_limit(limit);
        if limit != 0 {
            let heaps: Vec<_> = self.run_ref().heaps.heaps().collect();
            for heap in heaps {
                // SAFETY: the run's heaps are live, and no other task runs before the entry.
                unsafe { (*heap).set_metered() };
            }
        }
    }
    /// Retrieve the explicitly bound adapter; there is no ambient fallback.
    pub(crate) fn io_data(&self) -> *mut std::ffi::c_void {
        self.run_ref()
            .io_data
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Read the diagnostic side state of a block owned by this context.
    /// # Safety
    /// The pointer names storage of this live diagnostic context (including free storage).
    pub unsafe fn diagnostic_state(&self, block: *mut crate::Header) -> u8 {
        if !self.heap_ref().diagnostic {
            crate::fault("state inspection requires diagnostic mode");
        }
        if let Some(owner) = self
            .run_ref()
            .region_owners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&block.addr())
        {
            if !owner.active {
                crate::fault("count operation after a regional owner expired");
            }
            return crate::STATE_LIVE;
        }
        unsafe { self.heap_ref().state(block) }
    }

    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) fn stats(&mut self) -> Stats {
        let mut stats = Stats::default();
        let heaps: Vec<_> = self.run_ref().heaps.heaps().collect();
        for heap in heaps {
            // SAFETY: the caller holds the cooperative run token (or every
            // worker is idle at teardown). No heap is concurrently mutated.
            let heap = unsafe { &mut *heap };
            let (blocks, bytes) = heap.live();
            stats.backing_bytes +=
                heap.backing_bytes() + heap.regions.owned_bytes() + heap.regions.idle_bytes;
            stats.backing_allocations +=
                heap.backing_allocations + heap.regions.backing_allocations;
            stats.slab_bytes += heap.slab_count * SLAB_SIZE as u64;
            stats.idle_slab_bytes += heap.view().idle_slabs * SLAB_SIZE as u64;
            stats.large_bytes += heap.large_bytes;
            stats.live_blocks += blocks;
            stats.live_bytes += bytes;
            stats.region_active_bytes += heap.regions.active_bytes();
            stats.region_pool_bytes += heap.regions.idle_bytes;
            stats.active_regions = stats.active_regions.wrapping_add(heap.regions.active_count);
        }
        let mut workers = crate::pool::workers(self.run_ref());
        workers.push(self.run_ref().home_worker());
        stats.immortalized_blocks = workers
            .into_iter()
            .map(|worker| unsafe { (*worker).immortalized })
            .sum();
        stats
    }

    /// A trim point: idle slabs and pooled region segments above their caps go back.
    pub(crate) fn trim(&mut self) {
        let heap = self.heap();
        heap.regions.trim(REGION_POOL_IDLE_CAP);
        heap.trim();
    }

    /// Task exit: return all storage, live or idle, and report what was still held.
    pub(crate) fn exit(&mut self) -> Audit {
        if !self.work.is_null() {
            fault("task exit with release work pending");
        }
        #[cfg(feature = "measurement")]
        crate::tasks::profile::finish(self);
        crate::tasks::end_run(self);
        // end_run acknowledged every physical owner. Restore unpublished
        // callable custody before release_all can unmap its live words.
        if let Some(scheduler) = self.run().tasks.as_deref_mut() {
            scheduler.restore_reserved_custody();
        }
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        let audit = self.exit_audit();
        #[cfg(not(any(feature = "diagnostic", feature = "measurement")))]
        let audit = Audit::default();
        #[cfg(feature = "measurement")]
        if let Some(mut output) = self.run().measurement_output.take() {
            let budget = self.budget_counters();
            let measurement = &mut self.heap().measurement;
            measurement.meter_ops = budget.meter_ops;
            measurement.metered_allocations = budget.allocations;
            measurement.fuel_refills = budget.fuel_refills;
            // Snapshot before forced teardown, so leaks remain visible in the counters.
            self.heap()
                .measurement
                .write(&mut output)
                .unwrap_or_else(|_| crate::environmental("could not write the measurement report"));
            if let Some(profile) = self.run_ref().mc_profile.as_ref() {
                profile.write(output).unwrap_or_else(|_| {
                    crate::environmental("could not write the segment profile")
                });
            }
        }
        #[cfg(feature = "measurement")]
        if let Some(output) = self.run().profile_output.take() {
            self.heap()
                .measurement
                .write_sites(output)
                .unwrap_or_else(|_| crate::environmental("could not write the source profile"));
        }
        {
            let mut owners = self
                .run_ref()
                .region_owners
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for owner in owners.values_mut() {
                owner.active = false;
            }
        }
        self.region = ptr::null_mut();
        // Every heap of the run: tasks that ran on other workers allocated from theirs, and the
        // run's threads all left (`crate::tasks::end_run`).
        let heaps: Vec<_> = self.run_ref().heaps.heaps().collect();
        for &heap in &heaps {
            // SAFETY: the run's heaps are live and no other thread runs the run.
            unsafe {
                (*heap).regions.release_all();
                (*heap).release_all();
            }
        }
        self.heap().meter.clear();
        self.failure.clear();
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.worker().immortalized = 0;
            let retained_bytes = heaps
                .iter()
                // SAFETY: as above.
                .map(|&heap| unsafe {
                    (*heap).backing_bytes()
                        + (*heap).regions.owned_bytes()
                        + (*heap).regions.idle_bytes
                })
                .sum();
            return Audit {
                retained_bytes,
                ..audit
            };
        }
        #[cfg(not(any(feature = "diagnostic", feature = "measurement")))]
        audit
    }

    /// A stopped run (`crate::stop`): forget its pending release work, failure chain, shield
    /// and active region, which belonged to frames that never resume, so the exit returns all
    /// of its storage without running anything of the program. The root becomes the running
    /// task again, whichever task the stop left bound.
    pub(crate) fn abandon(&mut self) {
        if crate::pool::wide(self.run_ref()) {
            // Every thread of the run is idle: they leave, and the root, switched out where it
            // last ran, goes back to the home worker.
            crate::pool::shutdown(self.run_ref());
            self.home_again();
        } else {
            self.rebind();
        }
        self.work = ptr::null_mut();
        self.shield = 0;
        self.failure.clear();
        self.region = ptr::null_mut();
    }

    /// Separately built correctness runtimes audit before forced storage reclamation.
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    #[inline(never)]
    pub(crate) fn exit_audit(&self) -> Audit {
        // SAFETY: as for `worker`.
        let worker = unsafe { &*self.worker };
        let mut audit = Audit {
            immortalized_blocks: worker.immortalized,
            ..Audit::default()
        };
        for heap in self.run_ref().heaps.heaps() {
            // SAFETY: the run's heaps are live and no other thread runs the run.
            let heap = unsafe { &*heap };
            let (blocks, bytes) = heap.live();
            audit.live_blocks += blocks;
            audit.live_bytes += bytes;
            audit.active_regions = audit.active_regions.wrapping_add(heap.regions.active_count);
        }
        audit
    }
}

/// Create a run context. Returns a launch status; on success `*out` owns the new context.
///
/// # Safety
/// `config` is readable and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ctx_create(
    config: *const Config,
    out: *mut *mut TaskCtx,
) -> u8 {
    // SAFETY: caller contract.
    let config = unsafe { *config };
    unsafe { out.write(ptr::null_mut()) };
    if config.abi_version != ABI_VERSION {
        return LAUNCH_ABI_MISMATCH;
    }
    if config.flags & !CONFIG_DIAGNOSTIC != 0
        || config.count_max == 1
        || config.count_max > u32::MAX as u64
        || config.reserved != 0
        || config.layout_count > crate::alloc::INDEX_MASK - 2
        || (config.layout_count != 0 && config.layouts.is_null())
    {
        return LAUNCH_INVALID_CONFIG;
    }
    let idle_cap = if config.heap_idle_cap == 0 {
        1024 * 1024
    } else {
        config.heap_idle_cap
    };
    let mut layouts = vec![
        &crate::text::hirsel_rt_v2_bytes_descriptor.0 as *const crate::Layout,
        &crate::io::hirsel_rt_v2_file_walk_descriptor.0 as *const crate::Layout,
    ];
    if config.layout_count != 0 {
        layouts.extend_from_slice(unsafe {
            std::slice::from_raw_parts(config.layouts, config.layout_count as usize)
        });
    }
    layouts.push(&crate::text::BYTES_LIST_DESCRIPTOR.0 as *const crate::Layout);
    for &layout in &layouts {
        unsafe {
            crate::release::check_layout(layout);
        }
    }
    let diagnostic = config.flags & CONFIG_DIAGNOSTIC != 0;
    let count_max = finite_count_max(config.count_max);
    // Diagnostic mode checks every dup and measurement counts it, so neither adds holders inline.
    let dup_max = if !diagnostic && !cfg!(feature = "measurement") {
        count_max
    } else {
        0
    };
    let heaps = crate::heaps::HeapRegistry::new(crate::heaps::Template {
        layouts: layouts.into_boxed_slice(),
        diagnostic,
        idle_cap,
        dup_max,
        meter: Default::default(),
    });
    let run = Box::into_raw(Box::new(RunShared {
        role: Default::default(),
        count_max,
        close_handle: config.close_handle,
        hash_seed: config.hash_seed,
        io_data: Default::default(),
        names: None,
        region_owners: Default::default(),
        sealed_owners: Default::default(),
        entry_result: RoleCell::new(None),
        tasks: RoleCell::new(None),
        driver: RoleCell::new(crate::stop::Driver::default()),
        runners: RoleCell::new(None),
        runner_call: None,
        runner_home: None,
        root: ptr::null_mut(),
        control: Default::default(),
        io_facade: RoleCell::new(None),
        host_waits: Default::default(),
        timer_hint: Default::default(),
        launch: RoleCell::new(None),
        root_ctx: ptr::null_mut(),
        home: ptr::null_mut(),
        failure_table: None,
        schedule: Default::default(),
        free: RoleCell::new(None),
        free_mode: false,
        free_objects: Default::default(),
        free_replay: false,
        free_production: false,
        free_root_env: false,
        recording: Default::default(),
        free_namespace_coupled: Default::default(),
        free_return: (0, 0),
        ordered_mode: Default::default(),
        ordered_allowance: std::sync::atomic::AtomicU64::new(u64::MAX),
        ordered_memory: Default::default(),
        value_codes: Default::default(),
        root_overflow: Default::default(),
        heaps,
        fuel: Default::default(),
        execs: RoleCell::new(None),
        extern_metrics: None,
        worker_metrics: None,
        #[cfg(feature = "measurement")]
        measurement_output: RoleCell::new(None),
        #[cfg(feature = "measurement")]
        mc_profile: RoleCell::new(None),
        #[cfg(feature = "measurement")]
        profile_output: RoleCell::new(None),
        ordered_roots: Default::default(),
        ordered_io: Default::default(),
        ordered_commit: Default::default(),
        ordered_home_results: Default::default(),
        free_io: Default::default(),
        free_native_io: Default::default(),
        free_native_spans: Default::default(),
        #[cfg(feature = "operation-latency")]
        operation_latency: None,
        #[cfg(feature = "ownership-trace")]
        ownership_trace: None,
    }));
    // SAFETY: the run was just created; its root context owns it and the worker.
    let worker = unsafe { Worker::create(run) };
    // The root task's context, which owns the worker and the run.
    let ctx = Box::into_raw(Box::new(TaskCtx::new(worker, run)));
    // SAFETY: the run was just created.
    unsafe {
        (*run).root_ctx = ctx;
        (*run).home = worker;
    }
    // SAFETY: both were just created, and the root is the worker's first task.
    unsafe { TaskCtx::attach(ctx, worker) };
    unsafe { out.write(ctx) };
    LAUNCH_OK
}

/// A context for a new worker of `ctx`'s run, on a heap of its own from the run's registry
/// (design §6 Heaps): the substrate W1's pool runs tasks on, and how the two-thread heap tests
/// put a second worker in one run.
///
/// # Safety
/// `ctx` is a live context of a started or created run, which outlives the new context; the
/// new context is used by one thread at a time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_worker_create(ctx: *mut TaskCtx) -> *mut TaskCtx {
    // SAFETY: caller contract.
    unsafe {
        let run = (*ctx).run;
        let worker = Worker::create(run);
        let fresh = Box::into_raw(Box::new(TaskCtx::new(worker, run)));
        TaskCtx::attach(fresh, worker);
        fresh
    }
}

/// Retire the worker of a context from [`hirsel_rt_v2_worker_create`] and free the context:
/// its heap stays in the run's registry, orphaned, collected by the run's live workers.
///
/// # Safety
/// `ctx` came from `hirsel_rt_v2_worker_create`, has no release in progress and is not used
/// again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_worker_retire(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    unsafe {
        let ctx = Box::from_raw(ctx);
        if !ctx.work.is_null() {
            fault("a worker retires with release work pending");
        }
        Worker::retire(ctx.worker);
    }
}

/// A scheduling point of `ctx`'s worker: take back what other workers freed to its heap, and
/// collect orphaned heaps.
///
/// # Safety
/// `ctx` is live and its worker's thread calls this.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_worker_collect(ctx: *mut TaskCtx) {
    unsafe { (*ctx).worker().collect() };
}

/// Write a storage snapshot to `out`.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
#[cfg(any(feature = "diagnostic", feature = "measurement"))]
pub unsafe extern "C" fn hirsel_rt_v2_ctx_stats(ctx: *mut TaskCtx, out: *mut Stats) {
    unsafe { out.write((*ctx).stats()) };
}

/// An explicit trim point, as at region exit.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_trim(ctx: *mut TaskCtx) {
    unsafe { (*ctx).trim() };
}

/// Task exit: return every slab, large block and region segment, and write the audit. The
/// context stays usable as an empty task.
///
/// # Safety
/// `ctx` is live with no release in progress; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_exit(ctx: *mut TaskCtx, out: *mut Audit) {
    let audit = unsafe { (*ctx).exit() };
    if !out.is_null() {
        unsafe { out.write(audit) };
    }
}

/// Exit the task and free the context.
///
/// # Safety
/// `ctx` was created by `hirsel_rt_v2_ctx_create` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ctx_destroy(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    let mut ctx = unsafe { Box::from_raw(ctx) };
    ctx.exit();
    // SAFETY: the root context owns its run and worker, which nothing else uses any more. The
    // run goes first: its tasks' contexts lie in the worker's stacks.
    unsafe {
        let home = (*ctx.run).home;
        // All workers are quiescent; finish concurrent registrations while
        // role and task metadata still exist for bounded adapter callbacks.
        drop((*ctx.run).ordered_io.take());
        drop(Box::from_raw(ctx.run));
        drop(Box::from_raw(home));
    }
}

/// Leave a stopped run whose workers still run its code (`crate::pool::detach`): its last
/// thread to leave frees it, its root context and its first worker ([`reclaim`]); when none is
/// left, they are freed now. Nothing of the program runs, and its storage returns wholesale.
///
/// # Safety
/// `ctx` is a run's root context, stopped, whose launcher uses neither again.
pub(crate) unsafe fn detach(ctx: *mut TaskCtx) {
    // SAFETY: caller contract.
    unsafe {
        let run = (*ctx).run;
        if !crate::pool::detach(&*run) {
            reclaim(run);
        }
    }
}

/// Free a detached run: its workers, its storage (every heap, wholesale, with the registry),
/// its tasks' rows and stacks, its root context and its first worker.
///
/// # Safety
/// The run was detached and no thread runs any of it.
pub(crate) unsafe fn reclaim(run: *mut RunShared) {
    // SAFETY: caller contract.
    unsafe {
        // This Arc outlives execs and the run. A status reader must not see
        // reclamation complete while their task-local owners still exist.
        let holders = crate::pool::holders(&*run);
        crate::pool::forget(&mut *run);
        let (root, home) = ((*run).root_ctx, (*run).home);
        drop((*run).ordered_io.take());
        drop(Box::from_raw(run));
        drop(Box::from_raw(root));
        drop(Box::from_raw(home));
        if let Some(holders) = holders {
            holders.reclaimed();
        }
    }
}
