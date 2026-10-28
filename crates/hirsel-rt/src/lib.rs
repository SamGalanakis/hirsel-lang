//! The Hirsel native runtime substrate behind the versioned C interface `hirsel_rt_v2`
//! (`include/hirsel_rt_v2.h`).
//!
//! Every export is a non-unwinding `extern "C"` function, so a Rust panic aborts the process at
//! the boundary and never crosses into C. Statuses are `u8`: [`STATUS_OK`] or
//! [`STATUS_FAILURE`] (a language failure; a library row may ask the caller to record its rule). Broken invariants
//! (malformed descriptors, double release, header/layout mismatch) are implementation faults:
//! the runtime prints the fault and aborts, and never reports one as a language failure.
//! Host allocation exhaustion is environmental and also terminates.
//!
//! Each primitive cites the transition of D's Lean model (`proofs/rc`) it implements
//! by its ID and name (`T04 DropLast`); `proofs/rc/README.md` defines the IDs and the README here
//! lists the entries. `tests/it/model.rs` runs the model's `rc-trace v1` vectors on the runtime.

#[cfg(all(test, not(feature = "tsan")))]
hirsel_guard::install!(crate::io_real::borrowed_net_tests::Count: crate::io_real::borrowed_net_tests::Count);

mod base64;
mod continuation;
mod deadlines;
mod depth;
pub mod handles;
mod host_child;
pub mod host_launch;
pub mod sha;

pub mod exit_report;
mod host_io_tape;
pub mod host_paths;
mod host_stages;
pub mod host_stdin;
mod host_waits;
mod id_map;
pub mod io;
pub mod io_bridge;
pub mod io_checks;
pub mod io_concurrent;
mod io_home;
pub mod io_keys;
pub mod io_metrics;
pub mod io_net_script;
mod io_observation;
pub mod io_real;
mod io_request_observation;
pub mod io_sim;
pub mod isolate;
pub mod journal;
pub mod kernels;
pub mod launch;
pub mod managed_cores;
mod mapping;
pub mod net;
pub mod net_abi;
pub mod net_host;
pub mod net_tls;
mod observation_timing;
#[cfg(feature = "operation-latency")]
mod operation_latency;
#[cfg(feature = "ownership-trace")]
pub mod ownership_trace;
pub mod paths;
pub mod pool;
pub mod process;
pub mod program;
pub mod readiness;
pub mod runner;
pub mod runners;
pub mod signals;
mod standalone;
pub mod telemetry;
pub mod test_net_abi;
pub mod text;
pub mod tls_abi;
pub mod views;
pub mod watch;

pub mod alloc;
pub mod budget;
mod channels;
pub mod codec;
pub mod codec_text;
pub mod context;
pub mod copy;
pub mod count;
pub mod failure;
mod fiber;
#[cfg(all(feature = "tsan", target_arch = "x86_64", target_os = "linux"))]
mod tsan_callbacks;
/// Exact implementation bound into diagnostic private-stack certificates.
pub const TSAN_CALLBACK_BRIDGES: &[u8] = include_bytes!("tsan_callbacks.rs");
pub mod finalize;
pub mod foreign;
mod free;
pub mod free_driver;
/// Engine-facing production Free replay using the native host codec.
pub use tasks::free::host::engine as free_replay;
pub use tasks::ordered::ahead::cpu_handoff;
mod ahead_trace;
#[cfg(feature = "diagnostic")]
mod compact_trace;
mod free_launch;
mod heaps;
pub mod host;
#[cfg(feature = "measurement")]
pub mod measurement;
mod memcheck;
pub mod multicore_policy;
pub mod ordered;
pub mod region;
pub mod release;
mod remote;
mod remote_sync;
#[cfg(feature = "diagnostic")]
mod root_trace;
pub mod roots;
pub mod sealed_region;
pub mod shared;
mod slots;
pub mod stop;
mod sync;
pub mod tape_binary;
pub mod tape_links;
mod tape_wire;
pub mod tasks;
mod trace_events;

pub use alloc::{
    CLASS_COUNT, CLASS_SIZES, CTX_DUP_MAX_OFFSET, CTX_FREE_INLINE_OFFSET, CTX_FUEL_OFFSET,
    CTX_HEAP_ID_OFFSET, CTX_IDLE_SLABS_OFFSET, CTX_PAGES_OFFSET, DOMAIN_HEAP, DOMAIN_REGION,
    DOMAIN_STATIC, HEADER_SIZE, Header, LARGE_CLASS, PAGE_FREE_OFFSET, PAGE_LIVE_OFFSET,
    PAGE_LOCAL_FREE_OFFSET, PAGE_OWNER_OFFSET, PAGE_XTHREAD_FREE_OFFSET, POOL_COUNT, SLAB_SIZE,
    SMALL_ALIGN_MAX, SMALL_MAX, STATE_FIELDS_PENDING, STATE_FINALIZE_PENDING, STATE_FINALIZING,
    STATE_FREE, STATE_LIVE, STATE_RAW, class_of,
};
#[cfg(any(feature = "diagnostic", feature = "measurement"))]
pub use context::Stats;
pub use context::{
    Audit, CTX_CLEANUP_COUNT_OFFSET, CTX_CLEANUP_MODE_OFFSET, CTX_FOREGROUND_COUNT_OFFSET, Config,
    TaskCtx,
};
pub use count::{COUNT_ARENA_SHARED, COUNT_ARENA_UNIQUE, COUNT_IMMORTAL, finite_count_max};
pub use failure::FailureRecord;
pub use finalize::{Cleanup, CloseThunk, DropThunk};
pub use region::{Builder, REGION_POOL_IDLE_CAP, Region, SEGMENT_SIZE};
pub use release::{
    LAYOUT_COLUMNS, LAYOUT_DYNAMIC, LAYOUT_FROZEN, LAYOUT_IMPLEMENTATION_FINALIZER, LAYOUT_KEYED,
    LAYOUT_LOGICAL_PER_ELEMENT, LAYOUT_MAGIC, LAYOUT_MUTEX_ROOT, LAYOUT_NO_EXTENT, LAYOUT_SEQUENCE,
    LAYOUT_SEQUENTIAL, LAYOUT_UNSAFE_FFI, LAYOUT_UNTRACKED_BELOW, LAYOUT_VERSION,
    LOGICAL_UNSUPPORTED, Layout, NO_OFFSET, STEP_CLOSE_HANDLE, STEP_COLUMN, STEP_CURSOR_BITS,
    STEP_END_FINALIZE, STEP_FINALIZE, STEP_LIVE_BIT, STEP_LOOP, STEP_NONNULL, STEP_RELEASE,
    STEP_RELEASE_LAST, STEP_RELEASE_RECEIPT, STEP_RELEASE_ROOT, STEP_RELEASE_SEALED, STEP_TAG,
    STEP_THAW, Step, WHOLE_VALUE,
};
pub use roots::ROOT_OVERFLOW;

/// The interface version a caller must request in [`Config::abi_version`].
pub const ABI_VERSION: u32 = 95;
/// Success status.
pub const STATUS_OK: u8 = 0;
/// A language failure. Substrate entries record it in the run context; a library
/// primitive with a manifest `failure` rule asks the caller to record it at the call site.
pub const STATUS_FAILURE: u8 = 1;
/// A deep copy into a bounded arena its budget does not cover (MEM-6): nothing is copied or
/// recorded; the caller records the arena's `OutOfSpace`.
pub const STATUS_EXHAUSTED: u8 = 2;
pub const STATUS_PRETRANSFER_FAILURE: u8 = 3;
/// A resumable continuation alternate saved its frame at a boundary whose call has not run.
pub const STATUS_SUSPENDED: u8 = 4;
/// Set in a resume point's state when a non-head is logged for the boundary's operation
/// (`hirsel_lir::runtime_abi::RESUME_LOGGED`).
pub const RESUME_LOGGED: u32 = 1 << 31;
/// Set when a non-head may commit the boundary's operation early
/// (`hirsel_lir::runtime_abi::RESUME_EARLY`).
pub const RESUME_EARLY: u32 = 1 << 30;
/// Set when the boundary's operation may leave its stack from its first op
/// (`hirsel_lir::runtime_abi::RESUME_LEAVES`).
pub const RESUME_LEAVES: u32 = 1 << 29;

/// Report a broken runtime invariant and abort. Never a language failure.
#[cold]
pub(crate) fn fault(message: &str) -> ! {
    eprintln!("hirsel-rt: invariant violated: {message}");
    std::process::abort()
}

/// Report environmental exhaustion (host allocation failure) and terminate.
#[cold]
pub(crate) fn environmental(message: &str) -> ! {
    eprintln!("hirsel-rt: environmental failure: {message}");
    std::process::abort()
}

/// Diagnostic probe: panics inside the runtime so a harness can show that the panic aborts at
/// the C boundary instead of unwinding into foreign frames.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_diag_panic() {
    panic!("hirsel-rt: diagnostic panic probe");
}

/// The keyed state every `Inst::Hash` starts from (LIR schema revision 7): the run context's
/// seed. SEM-5: the seed moves hash-table placement and nothing a program can observe.
///
/// # Safety
/// `ctx` is a live run context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_hash_begin(ctx: *mut TaskCtx) -> u64 {
    // SAFETY: caller contract.
    unsafe { (*ctx).run_ref().hash_seed }
}

/// SipHash-1-3 of a single scalar leaf, keyed by the run seed. Composite
/// values use one `hash_init`/`hash_update`/`hash_finish` stream.
///
/// # Safety
/// `bytes` is readable for `length` bytes (it may dangle when `length` is 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_hash_bytes(state: u64, bytes: *const u8, length: u64) -> u64 {
    let bytes = if length == 0 {
        &[][..]
    } else {
        // SAFETY: caller contract.
        unsafe { std::slice::from_raw_parts(bytes, length as usize) }
    };
    kernels::siphash::siphash13(bytes, state, HASH_KEY_1)
}

/// Initialize caller-owned, eight-byte-aligned 48-byte SipHash storage.
/// # Safety
/// `out` names writable storage of that size and alignment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_hash_init(out: *mut std::ffi::c_void, seed: u64) {
    const {
        assert!(std::mem::size_of::<kernels::siphash::SipHasher13>() == 48);
    }
    const {
        assert!(std::mem::align_of::<kernels::siphash::SipHasher13>() == 8);
    }
    unsafe {
        out.cast::<kernels::siphash::SipHasher13>()
            .write(kernels::siphash::SipHasher13::new(seed, HASH_KEY_1));
    }
}
/// Feed another leaf without finalizing. The state address travels in an
/// opaque u64 carrier through generated layout hash helpers.
/// # Safety
/// `state` is initialized streaming scratch; `bytes` is readable for `length`
/// bytes and does not overlap it. An empty range may dangle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_hash_update(
    state: u64,
    bytes: *const u8,
    length: u64,
) -> u64 {
    let bytes = if length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(bytes, length as usize) }
    };
    unsafe {
        (*(state as *mut kernels::siphash::SipHasher13)).write(bytes);
    }
    state
}
/// Finalize one complete structural key, independent of leaf boundaries.
/// # Safety
/// `state` names initialized streaming scratch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_hash_finish(state: u64) -> u64 {
    unsafe { (*(state as *const kernels::siphash::SipHasher13)).finish() }
}

/// The second SipHash key word of [`hirsel_rt_v2_hash_bytes`]; the state is the first.
pub(crate) const HASH_KEY_1: u64 = 0x6869_7273_656c_2d37;

/// The interface version this library implements.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_abi_version() -> u32 {
    ABI_VERSION
}

/// Terminate on a compiled implementation invariant, through the same fatal
/// path as broken runtime invariants. It never reports a language failure.
#[unsafe(no_mangle)]
pub extern "C" fn hirsel_rt_v2_invariant_fault() -> ! {
    fault("compiled implementation invariant")
}

/// End a run whose unchecked integer sum violates its MEM-9 precondition.
///
/// # Safety
/// `ctx` is live and `site` belongs to its program's source table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_unchecked_add_overflow(ctx: *mut TaskCtx, site: u64) -> ! {
    // SAFETY: caller contract; release the role before abandoning this fiber.
    let role = unsafe { tasks::role::Guard::new(ctx) };
    let site = isolate::source_site(ctx, site);
    let message = format!(
        "undefined behaviour detected: int.unchecked_add at line {}: mathematical sum must fit the destination integer type",
        site.line
    );
    let diagnostic = hirsel_run_schema::ExecutionDiagnostic {
        rule: "undefined behaviour detected".into(),
        site,
        message,
    };
    drop(role);
    // SAFETY: this entry transfers every owned local into the stop, and compiled
    // callers hold plain values only. The launcher owns the abandoned storage.
    unsafe { stop::raise(ctx, stop::Stop::UndefinedBehavior(diagnostic)) }
}

#[cfg(test)]
mod header_tests;

/// Compiler/runtime certificate identity, including the exact bounded helpers.
#[doc(hidden)]
pub const AHEAD_HELPERS: &[u8] = include_bytes!("tasks/ordered/ahead/native.rs");
/// Exact task-local depth leaf source, bound into the native ahead certificate.
#[doc(hidden)]
pub const DEPTH_HELPERS: &[u8] = include_bytes!("depth/native.rs");
/// x86 SysV full depth-leaf call extent, including the return address.
#[doc(hidden)]
pub const DEPTH_LEAF_STACK_BYTES: u64 = 8;
#[doc(hidden)]
pub const AHEAD_HOST_HELPERS: &[u8] = include_bytes!("tasks/ordered/ahead/host_return.rs");
#[doc(hidden)]
pub const AHEAD_CPU_HELPERS: &[u8] = include_bytes!("tasks/ordered/ahead/cpu_worker.rs");
#[doc(hidden)]
pub const AHEAD_CPU_ALLOCATION_HELPERS: &[u8] =
    include_bytes!("tasks/ordered/ahead/cpu_allocation.rs");
#[doc(hidden)]
pub const AHEAD_CPU_HANDOFF_HELPERS: &[u8] = include_bytes!("tasks/ordered/ahead/cpu_handoff.rs");
#[doc(hidden)]
pub const AHEAD_SWITCH: &[u8] = include_bytes!("fiber.rs");
/// The continuation ownership protocol compiler plans are bound to.
#[doc(hidden)]
pub const CONTINUATION_PROTOCOL: &[u8] = include_bytes!("continuation.rs");
