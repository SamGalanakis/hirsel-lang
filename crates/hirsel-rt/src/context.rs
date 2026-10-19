//! The run context `RunCtx`: one task's heap domain, release worklist, region pool,
//! failure state and semantic configuration, passed explicitly to every entry. It is not a source
//! capability and conveys no authority. Contexts share nothing (OWN-6). The Io adapter belongs to
//! the library; the context holds only its handle close, which handle drops call.

use std::ptr;

#[cfg(any(feature = "diagnostic", feature = "measurement"))]
use crate::alloc::SLAB_SIZE;
use crate::alloc::{Header, Heap};
use crate::count::finite_count_max;
use crate::failure::FailureState;
use crate::finalize::CloseThunk;
use crate::region::{REGION_POOL_IDLE_CAP, RegionPool};
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
    /// Idle slab bytes kept at each trim point; 0 selects the default (8 MiB, as MEM-10).
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

/// One task's run context. Opaque to C, except for the words compiled fast paths read at its
/// start (`crate::alloc::Compiled`, runtime ABI 4), which is why it is `repr(C)` with the heap
/// first.
#[repr(C)]
pub struct RunCtx {
    pub(crate) heap: Heap,
    /// Top of the release worklist (an intrusive stack of dead headers and continuations).
    pub(crate) work: *mut Header,
    pub(crate) count_max: u32,
    pub(crate) shield: u32,
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) immortalized: u64,
    pub(crate) failure: FailureState,
    pub(crate) entry_result: Option<crate::exit_report::Returned>,
    pub(crate) regions: RegionPool,
    pub(crate) close_handle: Option<CloseThunk>,
    pub(crate) hash_seed: u64,
    io_data: *mut std::ffi::c_void,
    pub(crate) region_owners: Option<std::collections::BTreeMap<usize, crate::region::OwnerRecord>>,
    /// The freeze and thaw walks' block stack, kept between walks (MEM-11).
    pub(crate) frozen: Vec<*mut Header>,
    /// The run's tasks, from its first scope (CONC-2).
    pub(crate) tasks: Option<Box<crate::tasks::Scheduler>>,
    /// Where a stop of the run returns (`crate::stop`).
    pub(crate) driver: crate::stop::Driver,
    /// The running program's failure table (`crate::isolate`).
    pub(crate) failure_table: Option<std::sync::Arc<crate::isolate::FailureTable>>,
    /// The scheduler policy the run's tasks start with (`inputs.scheduler`).
    pub(crate) schedule: hirsel_kernels::schedule::Schedule,
    #[cfg(feature = "measurement")]
    pub(crate) measurement_output: Option<std::fs::File>,
    #[cfg(feature = "measurement")]
    pub(crate) mc_profile: Option<crate::tasks::profile::Profile>,
    #[cfg(feature = "measurement")]
    pub(crate) profile_output: Option<std::fs::File>,
}

const _: () = assert!(std::mem::offset_of!(RunCtx, heap) == 0);

impl RunCtx {
    #[inline]
    pub(crate) unsafe fn descriptor(&self, index: u32) -> *const crate::Layout {
        unsafe { self.heap.descriptor(index) }
    }
    /// Bind one explicit adapter to this context; null unbinds it.
    ///
    /// # Safety
    /// The adapter outlives every release on this context. Close-handle may run only
    /// while the adapter is bound. No simultaneous access to the adapter is allowed.
    pub unsafe fn bind_io_data(&mut self, data: *mut std::ffi::c_void) {
        self.io_data = data;
    }
    /// Apply the run's budgets (`budgets.limits@1`) before its entry runs.
    pub(crate) fn set_budgets(&mut self, fuel: Option<u64>, memory_bytes: Option<u64>) {
        self.heap.compiled.fuel = fuel.map_or(i64::MAX, |f| i64::try_from(f).unwrap_or(i64::MAX));
        self.heap.meter.set_limit(memory_bytes.unwrap_or(0));
    }
    /// Retrieve the explicitly bound adapter; there is no ambient fallback.
    pub(crate) fn io_data(&self) -> *mut std::ffi::c_void {
        self.io_data
    }

    /// Read the diagnostic side state of a block owned by this context.
    /// # Safety
    /// The pointer names storage of this live diagnostic context (including free storage).
    pub unsafe fn diagnostic_state(&self, block: *mut crate::Header) -> u8 {
        if !self.heap.diagnostic {
            crate::fault("state inspection requires diagnostic mode");
        }
        if let Some(owner) = self
            .region_owners
            .as_ref()
            .and_then(|owners| owners.get(&block.addr()))
        {
            if !owner.active {
                crate::fault("count operation after a regional owner expired");
            }
            return crate::STATE_LIVE;
        }
        unsafe { self.heap.state(block) }
    }

    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    pub(crate) fn stats(&self) -> Stats {
        let (live_blocks, live_bytes) = self.heap.live();
        Stats {
            backing_bytes: self.heap.backing_bytes()
                + self.regions.active_bytes
                + self.regions.idle_bytes,
            backing_allocations: self.heap.backing_allocations + self.regions.backing_allocations,
            slab_bytes: self.heap.slab_count * SLAB_SIZE as u64,
            idle_slab_bytes: self.heap.compiled.idle_slabs * SLAB_SIZE as u64,
            large_bytes: self.heap.large_bytes,
            live_blocks,
            live_bytes,
            region_active_bytes: self.regions.active_bytes,
            region_pool_bytes: self.regions.idle_bytes,
            active_regions: self.regions.active_count,
            immortalized_blocks: self.immortalized,
        }
    }

    /// A trim point: idle slabs and pooled region segments above their caps go back.
    pub(crate) fn trim(&mut self) {
        self.regions.trim(REGION_POOL_IDLE_CAP);
        self.heap.trim();
    }

    /// Task exit: return all storage, live or idle, and report what was still held.
    pub(crate) fn exit(&mut self) -> Audit {
        if !self.work.is_null() {
            fault("task exit with release work pending");
        }
        #[cfg(feature = "measurement")]
        crate::tasks::profile::finish(self);
        crate::tasks::end_run(self);
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        let audit = self.exit_audit();
        #[cfg(not(any(feature = "diagnostic", feature = "measurement")))]
        let audit = Audit::default();
        #[cfg(feature = "measurement")]
        if let Some(mut output) = self.measurement_output.take() {
            // Snapshot before forced teardown, so leaks remain visible in the counters.
            self.heap
                .measurement
                .write(&mut output)
                .unwrap_or_else(|_| crate::environmental("could not write the measurement report"));
            if let Some(profile) = &self.mc_profile {
                profile.write(output).unwrap_or_else(|_| {
                    crate::environmental("could not write the segment profile")
                });
            }
        }
        #[cfg(feature = "measurement")]
        if let Some(output) = self.profile_output.take() {
            self.heap
                .measurement
                .write_sites(output)
                .unwrap_or_else(|_| crate::environmental("could not write the source profile"));
        }
        if let Some(owners) = &mut self.region_owners {
            for owner in owners.values_mut() {
                owner.active = false;
            }
        }
        self.regions.release_all();
        self.heap.release_all();
        self.failure.clear();
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        {
            self.immortalized = 0;
            return Audit {
                retained_bytes: self.heap.backing_bytes()
                    + self.regions.active_bytes
                    + self.regions.idle_bytes,
                ..audit
            };
        }
        #[cfg(not(any(feature = "diagnostic", feature = "measurement")))]
        audit
    }

    /// A stopped run (`crate::stop`): forget its pending release work, failure chain, shield
    /// and active region, which belonged to frames that never resume, so the exit returns all
    /// of its storage without running anything of the program.
    pub(crate) fn abandon(&mut self) {
        self.work = ptr::null_mut();
        self.shield = 0;
        self.failure.clear();
        self.regions.active = ptr::null_mut();
    }

    /// Separately built correctness runtimes audit before forced storage reclamation.
    #[cfg(any(feature = "diagnostic", feature = "measurement"))]
    #[inline(never)]
    pub(crate) fn exit_audit(&self) -> Audit {
        let (live_blocks, live_bytes) = self.heap.live();
        Audit {
            live_blocks,
            live_bytes,
            immortalized_blocks: self.immortalized,
            active_regions: self.regions.active_count,
            retained_bytes: 0,
        }
    }
}

/// Create a run context. Returns a launch status; on success `*out` owns the new context.
///
/// # Safety
/// `config` is readable and `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ctx_create(
    config: *const Config,
    out: *mut *mut RunCtx,
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
        REGION_POOL_IDLE_CAP
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
    let mut heap = Heap::new(layouts.into_boxed_slice(), diagnostic, idle_cap);
    // Diagnostic mode checks every dup and measurement counts it, so neither adds holders inline.
    if !diagnostic && !cfg!(feature = "measurement") {
        heap.compiled.dup_max = count_max;
    }
    let meter = heap.meter.clone();
    let ctx = Box::new(RunCtx {
        heap,
        work: ptr::null_mut(),
        count_max,
        shield: 0,
        #[cfg(any(feature = "diagnostic", feature = "measurement"))]
        immortalized: 0,
        failure: FailureState::default(),
        entry_result: None,
        regions: RegionPool::new(meter),
        close_handle: config.close_handle,
        hash_seed: config.hash_seed,
        io_data: ptr::null_mut(),
        region_owners: None,
        frozen: Vec::new(),
        tasks: None,
        driver: crate::stop::Driver::default(),
        failure_table: None,
        schedule: Default::default(),
        #[cfg(feature = "measurement")]
        measurement_output: None,
        #[cfg(feature = "measurement")]
        mc_profile: None,
        #[cfg(feature = "measurement")]
        profile_output: None,
    });
    unsafe { out.write(Box::into_raw(ctx)) };
    LAUNCH_OK
}

/// Write a storage snapshot to `out`.
///
/// # Safety
/// `ctx` is live and `out` writable.
#[unsafe(no_mangle)]
#[cfg(any(feature = "diagnostic", feature = "measurement"))]
pub unsafe extern "C" fn hirsel_rt_v2_ctx_stats(ctx: *mut RunCtx, out: *mut Stats) {
    unsafe { out.write((*ctx).stats()) };
}

/// An explicit trim point, as at region exit.
///
/// # Safety
/// `ctx` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_trim(ctx: *mut RunCtx) {
    unsafe { (*ctx).trim() };
}

/// Task exit: return every slab, large block and region segment, and write the audit. The
/// context stays usable as an empty task.
///
/// # Safety
/// `ctx` is live with no release in progress; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_task_exit(ctx: *mut RunCtx, out: *mut Audit) {
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
pub unsafe extern "C" fn hirsel_rt_v2_ctx_destroy(ctx: *mut RunCtx) {
    // SAFETY: caller contract.
    let mut ctx = unsafe { Box::from_raw(ctx) };
    ctx.exit();
}
