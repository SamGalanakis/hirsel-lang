//! ABI records borrowed from a checked CPU mapper's saved frame.
use super::native::{Native, Physical};
use crate::{Header, TaskCtx, region::Region};

pub(super) mod growth;

pub const VERSION: u32 = 1;
pub const ACCEPTED: u8 = 0;
pub const NOT_CERTIFIED: u8 = 1;
pub const SUMMARY_PENDING: u32 = 0;
pub const SUMMARY_PUBLISHED: u32 = 1;
pub const SUMMARY_RETIRED: u32 = 2;
pub const SUMMARY_WITHDRAWN: u32 = 3;
pub const GROW_BLOCK: u32 = 0;
pub const GROW_SEQUENCE: u32 = 1;
pub const ALLOC_REGION: u32 = 2;
/// Headerless compiler-local storage, with the ordinary non-geometric segment charge.
pub const ALLOC_RAW_REGION: u32 = 3;
pub const STACK_BYTES: u64 = 80;

/// The value and retained-block table remain owned by the saved mapper until
/// retirement. Publication borrows them; it never adds counted holders.
#[repr(C)]
pub struct Summary {
    pub version: u32,
    pub phase: u32,
    pub ordinal: u64,
    pub value: *const u8,
    pub value_bytes: u64,
    pub descriptor: u32,
    pub retained_count: u32,
    pub retained: *const *mut Header,
}

/// A saved storage call. `block` is both the old block and the successful
/// result. All input fields survive refusal unchanged. No Rust frame is saved.
#[repr(C)]
pub struct Growth {
    pub version: u32,
    pub kind: u32,
    pub ordinal: u64,
    pub block: *mut Header,
    pub region: *mut Region,
    pub index: u32,
    pub flags: u32,
    pub extent: u64,
    pub align: u64,
    pub keep: u64,
    pub element_size: u64,
    pub element_align: u64,
    pub capacity: u64,
}

const _: () = {
    assert!(size_of::<Summary>() == 48);
    assert!(std::mem::offset_of!(Summary, ordinal) == 8);
    assert!(std::mem::offset_of!(Summary, retained) == 40);
    assert!(size_of::<Growth>() == 88);
    assert!(std::mem::offset_of!(Growth, block) == 16);
    assert!(std::mem::offset_of!(Growth, capacity) == 80);
};

/// # Safety
/// Called only by a checked CPU alternate. Once supported, the summary and
/// its pointed-to value/table must remain in that mapper's owned saved frame
/// until this call returns. It uses 16 stack bytes plus the 64-byte handoff.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_ahead_publish(
    _state: *mut Physical,
    _summary: *mut Summary,
) -> u8 {
    core::arch::naked_asm!(
        "test rdi, rdi", "je 2f", "test rsi, rsi", "je 2f",
        "cmp byte ptr [rdi + {cpu}], 0", "je 2f",
        "cmp dword ptr [rdi + {cpu_phase}], 2", "jne 2f",
        "cmp qword ptr [rdi + {summary}], 0", "jne 3f",
        "push rbx", "mov rbx, rdi", "mov [rbx + {summary}], rsi",
        "mov dword ptr [rbx + {status}], {refused}",
        "mov esi, 16", "call {handoff}",
        "cmp qword ptr [rbx + {summary}], 0", "jne 3f",
        "mov eax, [rbx + {status}]", "pop rbx", "ret",
        "2:", "mov eax, {refused}", "ret", "3:", "ud2",
        cpu = const std::mem::offset_of!(Native, cpu),
        cpu_phase = const std::mem::offset_of!(Native, cpu_phase),
        summary = const std::mem::offset_of!(Native, summary),
        status = const std::mem::offset_of!(Native, summary_status),
        refused = const NOT_CERTIFIED,
        handoff = sym super::native::hirsel_rt_v2_ahead_handoff,
    );
}

/// # Safety
/// The same contract as the x86 handoff. Unsupported targets refuse it.
#[cfg(not(target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_ahead_publish(
    _state: *mut Physical,
    _summary: *mut Summary,
) -> u8 {
    NOT_CERTIFIED
}

/// Idle and role-owned, with the caller's exact stack saved. Publication is
/// a physical transfer of its charge prefix, never a canonical operation.
pub(super) unsafe fn serve(ctx: *mut TaskCtx, native: *mut Native) -> crate::stop::Stopped<bool> {
    assert!(
        crate::stop::running().is_null(),
        "CPU publication runs on idle"
    );
    if super::private(ctx) {
        return Ok(false);
    }
    // SAFETY: the naked entry saved this exclusive caller-owned record. It
    // stays live until serve clears the pointer and resumes the exact call.
    let pointer = unsafe { (*native).summary };
    // Inspect the version before borrowing a complete record or any pointee.
    if unsafe { pointer.cast::<u32>().read() } != VERSION {
        unsafe {
            (*native).summary_status = u32::from(NOT_CERTIFIED);
            (*native).summary = std::ptr::null_mut();
        }
        return Ok(true);
    }
    // Retirement writes this record through the batch's retained raw pointer.
    // No reference to the record may span either publication or retirement.
    if unsafe { (*pointer).phase } == SUMMARY_PENDING {
        let (value, bytes, descriptor, count, retained, ordinal) = unsafe {
            (
                (*pointer).value,
                (*pointer).value_bytes,
                (*pointer).descriptor,
                (*pointer).retained_count,
                (*pointer).retained,
                (*pointer).ordinal,
            )
        };
        let task = super::super::engine::current(ctx);
        let valid = bytes <= 256
            && (bytes == 0 || !value.is_null())
            && count <= crate::budget::stretch::ENVELOPE_MAX as u32
            && (count == 0 || !retained.is_null())
            && descriptor < unsafe { (*ctx).heap_ref().layout_count() }
            && super::super::engine::with_driver(ctx, |driver| {
                driver
                    .cpu_batch
                    .as_ref()
                    .is_some_and(|batch| batch.owns(task, ordinal))
            });
        if !valid {
            unsafe {
                (*native).summary_status = u32::from(NOT_CERTIFIED);
                (*native).summary = std::ptr::null_mut();
            }
            return Ok(true);
        }
        if let Some(ledger) = unsafe { (*ctx).heap_ref().meter.cpu.get() } {
            // The checked saved frame owns this complete borrowed root table.
            // Empty tables may use null, which from_raw_parts does not accept.
            let retained = if count == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(retained, count as usize) }
            };
            let owner = crate::pool::cpu_scoped::ledger::blocks::Owner { task, ordinal };
            let Some(blocks) = ledger.blocks.lock().unwrap().retain(owner, retained) else {
                unsafe {
                    (*native).summary_status = u32::from(NOT_CERTIFIED);
                    (*native).summary = std::ptr::null_mut();
                }
                return Ok(true);
            };
            unsafe { (*native).summary_blocks = blocks };
        }
        super::super::engine::ahead_summary(ctx, pointer)?;
        unsafe { (*pointer).phase = SUMMARY_PUBLISHED };
    }
    assert_eq!(
        unsafe { (*pointer).phase },
        SUMMARY_PUBLISHED,
        "a CPU summary publishes once"
    );
    if let Some(ledger) = unsafe { (*ctx).heap_ref().meter.cpu.get() } {
        assert!(
            ledger
                .blocks
                .lock()
                .unwrap()
                .valid(unsafe { &(*native).summary_blocks }),
            "saved CPU summary retains its exact allocation generations"
        );
    }
    if !super::super::engine::host_segment_barrier(ctx)? {
        return Ok(false);
    }
    assert_eq!(unsafe { (*pointer).phase }, SUMMARY_RETIRED);
    unsafe {
        (*native).summary_status = u32::from(ACCEPTED);
        (*native).summary = std::ptr::null_mut();
        (*native).summary_blocks.clear();
    }
    Ok(true)
}

/// # Safety
/// Only a checked CPU alternate calls this helper. The request, its input
/// credit and its active region remain in that ordinal's saved custody through
/// return. Shared sequences retain their other holders and every copied child
/// during detachment. Tracked growth moves holders into region-owned storage.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_ahead_grow(
    _state: *mut Physical,
    _request: *mut Growth,
) -> u8 {
    core::arch::naked_asm!(
        "test rdi, rdi", "je 2f", "test rsi, rsi", "je 2f",
        "cmp byte ptr [rdi + {cpu}], 0", "je 2f",
        "cmp dword ptr [rdi + {cpu_phase}], 2", "jne 2f",
        "cmp qword ptr [rdi + {growth}], 0", "jne 3f",
        "push rbx", "mov rbx, rdi", "mov [rbx + {growth}], rsi",
        "mov dword ptr [rbx + {status}], {refused}",
        "mov esi, 32", "call {handoff}",
        "cmp qword ptr [rbx + {growth}], 0", "jne 3f",
        "mov eax, [rbx + {status}]", "pop rbx", "ret",
        "2:", "mov eax, {refused}", "ret", "3:", "ud2",
        cpu = const std::mem::offset_of!(Native, cpu),
        cpu_phase = const std::mem::offset_of!(Native, cpu_phase),
        growth = const std::mem::offset_of!(Native, growth),
        status = const std::mem::offset_of!(Native, growth_status),
        refused = const NOT_CERTIFIED,
        handoff = sym super::native::hirsel_rt_v2_ahead_handoff,
    );
}

/// # Safety
/// The same contract as the x86 handoff. Unsupported targets refuse it.
#[cfg(not(target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_ahead_grow(
    _state: *mut Physical,
    _request: *mut Growth,
) -> u8 {
    NOT_CERTIFIED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncertified_handoffs_refuse_before_reading_the_saved_frame() {
        // SAFETY: these declaration-only entries never dereference arguments.
        unsafe {
            assert_eq!(
                hirsel_rt_v2_cpu_ahead_publish(std::ptr::null_mut(), std::ptr::null_mut()),
                NOT_CERTIFIED
            );
            assert_eq!(
                hirsel_rt_v2_cpu_ahead_grow(std::ptr::null_mut(), std::ptr::null_mut()),
                NOT_CERTIFIED
            );
        }
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod naked_tests;

#[cfg(all(test, target_arch = "x86_64"))]
mod growth_tests;
