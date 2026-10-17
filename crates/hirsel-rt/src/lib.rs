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

pub mod handles;
pub mod host_launch;

mod host_io_tape;
pub mod host_paths;
pub mod host_stdin;
pub mod io;
pub mod io_net_script;
pub mod io_real;
pub mod io_sim;
pub mod kernels;
pub mod launch;
pub mod net;
pub mod net_abi;
pub mod net_host;
pub mod net_tls;
pub mod operations;
pub mod process;
pub mod readiness;
mod standalone;
pub mod test_net_abi;
pub mod text;
pub mod tls_abi;
pub mod views;

pub mod alloc;
pub mod context;
pub mod copy;
pub mod count;
pub mod failure;
mod fiber;
pub mod finalize;
#[cfg(feature = "measurement")]
pub mod measurement;
mod memcheck;
pub mod region;
pub mod release;
pub mod shared;
pub mod tasks;

pub use alloc::{
    CLASS_COUNT, CLASS_SIZES, CTX_DUP_MAX_OFFSET, CTX_FREE_INLINE_OFFSET, CTX_IDLE_SLABS_OFFSET,
    CTX_PAGES_OFFSET, DOMAIN_HEAP, DOMAIN_REGION, DOMAIN_STATIC, HEADER_SIZE, Header, LARGE_CLASS,
    PAGE_FREE_OFFSET, PAGE_LIVE_OFFSET, PAGE_LOCAL_FREE_OFFSET, POOL_COUNT, SLAB_SIZE,
    SMALL_ALIGN_MAX, SMALL_MAX, STATE_FIELDS_PENDING, STATE_FINALIZE_PENDING, STATE_FINALIZING,
    STATE_FREE, STATE_LIVE, STATE_RAW, class_of,
};
#[cfg(any(feature = "diagnostic", feature = "measurement"))]
pub use context::Stats;
pub use context::{Audit, Config, RunCtx};
pub use count::{COUNT_IMMORTAL, finite_count_max};
pub use failure::FailureRecord;
pub use finalize::{Cleanup, CloseThunk, DropThunk};
pub use region::{Builder, REGION_POOL_IDLE_CAP, Region, SEGMENT_SIZE};
pub use release::{
    LAYOUT_DYNAMIC, LAYOUT_FROZEN, LAYOUT_LOGICAL_PER_ELEMENT, LAYOUT_MAGIC, LAYOUT_NO_EXTENT,
    LAYOUT_UNSAFE_FFI, LAYOUT_VERSION, LOGICAL_UNSUPPORTED, Layout, NO_OFFSET, STEP_CLOSE_HANDLE,
    STEP_COLUMN, STEP_CURSOR_BITS, STEP_FINALIZE, STEP_LIVE_BIT, STEP_LOOP, STEP_NONNULL,
    STEP_RELEASE, STEP_TAG, STEP_THAW, Step, WHOLE_VALUE,
};

/// The interface version a caller must request in [`Config::abi_version`].
pub const ABI_VERSION: u32 = 8;
/// Success status.
pub const STATUS_OK: u8 = 0;
/// A language failure. Substrate entries record it in the run context; a library
/// primitive with a manifest `failure` rule asks the caller to record it at the call site.
pub const STATUS_FAILURE: u8 = 1;
/// A deep copy into a bounded arena its budget does not cover (MEM-6): nothing is copied or
/// recorded; the caller records the arena's `OutOfSpace`.
pub const STATUS_EXHAUSTED: u8 = 2;

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
pub unsafe extern "C" fn hirsel_rt_v2_hash_begin(ctx: *mut RunCtx) -> u64 {
    // SAFETY: caller contract.
    unsafe { (*ctx).hash_seed }
}

/// Fold `length` bytes into a hash state: SipHash-1-3 of the bytes keyed by the state, so a
/// value hashed leaf by leaf chains one keyed hash per leaf.
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

#[cfg(test)]
mod header_tests;
