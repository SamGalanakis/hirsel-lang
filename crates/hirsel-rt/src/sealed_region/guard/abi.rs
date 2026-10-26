//! Demanded guard and entered seal C boundary. Ordinary scopes never call these
//! helpers, reserve the extension or construct receipt metadata.
use super::*;
use crate::sealed_region::receipt::semantic::{self, Origin};
use crate::sealed_region::{Charge, SealError, into_native_owner};
use std::ffi::c_void;
use std::mem::ManuallyDrop;

unsafe fn metadata_run(ctx: *mut TaskCtx) -> MetadataRun {
    unsafe { &*ctx }
        .run_ref()
        .sealed_owners
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .metadata_run()
}

fn guard(raw: *mut c_void) -> *mut ReceiptArenaGuard<'static> {
    if raw.is_null()
        || !raw
            .addr()
            .is_multiple_of(align_of::<ReceiptArenaGuard<'_>>())
    {
        crate::fault("invalid demanded arena guard storage");
    }
    raw.cast()
}

/// Initialize the approved 80-byte demanded record, keeping its ordinary
/// 64-byte region prefix and offsets. This creates no user-visible owner.
///
/// # Safety
/// `ctx` is current and `raw` is exclusive aligned writable storage of 80 bytes,
/// stable through its matching exit. C independently proved the demanded scope
/// entry; an ordinary 64-byte region may never be cast to this extension.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_receipt_region_enter(ctx: *mut TaskCtx, raw: *mut c_void) {
    if ctx.is_null() {
        crate::fault("null demanded arena context");
    }
    let record = guard(raw);
    let run = unsafe {
        let mut owners = (*ctx)
            .run_ref()
            .sealed_owners
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        owners.enter_guard(raw.addr(), ctx.addr());
        owners.metadata_run()
    };
    unsafe { enter_record(ctx, record, run) };
}

/// Consume lexical scope cleanup. After seal the disarm marker is never
/// followed, even if the affine sealed owner was already released.
///
/// # Safety
/// `ctx` and `raw` name their matching live demanded entry. Lexical nesting and
/// original cleanup/attribution contracts are unchanged. This consumes entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_receipt_region_exit(
    ctx: *mut TaskCtx,
    raw: *mut c_void,
    anchor: u64,
    site: u64,
) -> u8 {
    if ctx.is_null() {
        crate::fault("null demanded arena context");
    }
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let record = guard(raw);
    unsafe { &*ctx }
        .run_ref()
        .sealed_owners
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .exit_guard(raw.addr(), ctx.addr());
    unsafe { close_record(ctx, record, anchor, site) }
}

/// Snapshot the independently checked pointer-free original source value.
/// The returned nonnull handle owns one metadata holder, no source credit.
///
/// # Safety
/// `ctx` is current; `index` describes `value` exactly, with every byte including
/// padding initialized. Original source proof excludes pointer-valued scalars,
/// hidden resources/holders and borrowed provenance. Descriptor scan absence
/// alone cannot mint that proof. Null `value` is valid only for a zero-size root.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_receipt_bits(
    ctx: *mut TaskCtx,
    index: u32,
    value: *const c_void,
) -> *mut c_void {
    if ctx.is_null() {
        crate::fault("null demanded arena context");
    }
    let layout = unsafe { &*(*ctx).descriptor(index) };
    if layout.finalizer.is_some()
        || layout.step_count != 0
        || layout.scan_words != 0
        || layout.flags != 0
        || !layout.logical_count.is_null()
    {
        crate::fault("receipt bits require a certified pointer-free descriptor");
    }
    let size = usize::try_from(layout.size)
        .unwrap_or_else(|_| crate::fault("receipt bits extent overflow"));
    let align = usize::try_from(layout.align)
        .unwrap_or_else(|_| crate::fault("receipt bits alignment overflow"));
    if !align.is_power_of_two()
        || (size != 0 && (value.is_null() || !value.addr().is_multiple_of(align)))
    {
        crate::fault("invalid receipt bits source storage");
    }
    let bytes = if size == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(value.cast::<u8>(), size) }
    };
    semantic::snapshot(
        unsafe { metadata_run(ctx) },
        Origin::Inline,
        Charge::Bytes(0),
        bytes,
        align,
        &[],
    )
    .unwrap_or_else(|_| crate::fault("invalid receipt bits image"))
    .into_raw()
}

/// Entered prepare/commit. Success publishes one opaque generation; exhausted
/// returns 2 with null output and unchanged source custody. Invalid/incomplete
/// checked contracts trap. Currently only actual-empty pointer-free plans are
/// linked; a retained inventory is never treated as empty. Public bodies stay off.
///
/// # Safety
/// The independently validated entered call owns grant/root, and `receipt` is
/// their live paired original semantic Record handle. `out_owner` is exclusive
/// writable storage, disjoint from the guard/root. All root bytes are initialized
/// and no root/grant view is live. Caller keeps input cleanup on nonzero status,
/// disarms it only on success, and retains its receipt holder on every outcome.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_seal_prepare_commit(
    ctx: *mut TaskCtx,
    raw: *mut c_void,
    value: *const c_void,
    index: u32,
    receipt: *const c_void,
    _site: u64,
    out_owner: *mut *mut c_void,
) -> u8 {
    if ctx.is_null() {
        crate::fault("null demanded arena context");
    }
    if out_owner.is_null() {
        crate::fault("null sealed owner output");
    }
    unsafe { out_owner.write(ptr::null_mut()) };
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let record = guard(raw);
    unsafe { &*ctx }
        .run_ref()
        .sealed_owners
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .check_guard(raw.addr(), ctx.addr());
    if receipt.is_null() {
        crate::fault("null seal root receipt");
    }
    // Borrow the caller's metadata holder without acquiring or consuming one.
    let root = ManuallyDrop::new(unsafe { Receipt::<Record>::from_raw(receipt.cast_mut()) });
    if root.run() != unsafe { metadata_run(ctx) } {
        crate::fault("foreign seal root receipt");
    }
    let plan =
        unsafe { super::bits::bits_plan(ctx, record, index, value.cast_mut().cast(), &root) }
            .unwrap_or_else(|_| crate::fault("invalid or incomplete seal plan contract"));
    match unsafe { seal_record(ctx, record, &root, &plan) } {
        Ok(owner) => {
            unsafe { out_owner.write(into_native_owner(ctx, owner)) };
            crate::STATUS_OK
        }
        Err(Error::Seal(journal::SealError::Transaction(SealError::Exhausted))) => {
            crate::STATUS_EXHAUSTED
        }
        Err(_) => crate::fault("invalid entered seal transaction"),
    }
}

#[cfg(all(test, feature = "diagnostic"))]
mod tests;
