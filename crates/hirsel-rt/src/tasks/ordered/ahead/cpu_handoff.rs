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
/// ArenaPush is a no-op for admitted heap/static sequence owners.
pub const CHECK_UNTRACKED_SEQUENCE: u32 = 4;
/// A shallow sequence copy borrows its original and keeps every original credit.
pub const COPY_SEQUENCE: u32 = 5;
/// Release of a checked pointer-free sequence owned by this job.
pub const RELEASE_SEQUENCE: u32 = 6;
pub const STACK_BYTES: u64 = 96;

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

/// Publish the same checked result from ordinary serial execution and replay.
/// # Safety
/// `ctx` is current. The checked worker owns this record, its initialized
/// result and its retained roots until return; the descriptor matches the SSA
/// result layout. This entry is not called from a private alternate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_publish(ctx: *mut TaskCtx, summary: *mut Summary) -> u8 {
    if !super::super::engine::on(ctx) {
        return ACCEPTED;
    }
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let task = super::super::engine::current(ctx);
    let owned = super::super::engine::with_driver(ctx, |d| {
        d.cpu_batch.as_ref().is_some_and(|batch| {
            !summary.is_null() && batch.owns(task, unsafe { (*summary).ordinal })
        })
    });
    if !owned {
        return NOT_CERTIFIED;
    }
    if unsafe {
        (*summary).version != VERSION
            || (*summary).phase != SUMMARY_PENDING
            || (*summary).value_bytes > 256
            || (*summary).retained_count > crate::budget::stretch::ENVELOPE_MAX as u32
    } {
        return NOT_CERTIFIED;
    }
    unsafe { crate::stop::check(ctx, super::super::engine::ahead_summary(ctx, summary)) };
    unsafe { (*summary).phase = SUMMARY_PUBLISHED };
    unsafe { crate::stop::check(ctx, super::super::engine::at_head(ctx)) };
    assert_eq!(unsafe { (*summary).phase }, SUMMARY_RETIRED);
    ACCEPTED
}

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
        "mov eax, [rdi + {cpu_phase}]", "sub eax, 2", "cmp eax, 1", "ja 2f",
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
        "mov eax, [rdi + {cpu_phase}]", "sub eax, 2", "cmp eax, 1", "ja 2f",
        "5:", "push rdi", "push rsi", "call {cached}", "pop rsi", "pop rdi",
        "test al, al", "je 4f", "xor eax, eax", "ret", "4:",
        "cmp qword ptr [rdi + {arena_count}], {arena_events}", "jb 6f",
        "push rdi", "push rsi", "push rbx", "mov rbx, rdi", "mov esi, 64", "call {handoff}",
        "pop rbx", "pop rsi", "pop rdi", "jmp 5b", "6:",
        "cmp qword ptr [rdi + {growth}], 0", "jne 3f",
        "push rbx", "mov rbx, rdi", "mov [rbx + {growth}], rsi",
        "mov dword ptr [rbx + {status}], {refused}",
        "mov esi, 32", "call {handoff}",
        "cmp qword ptr [rbx + {growth}], 0", "jne 3f",
        "mov eax, [rbx + {status}]", "pop rbx", "ret",
        "2:", "mov eax, {refused}", "ret", "3:", "ud2",
        cpu = const std::mem::offset_of!(Native, cpu),
        cpu_phase = const std::mem::offset_of!(Native, cpu_phase),
        arena_count = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(super::cpu_arena::Arena, count),
        arena_events = const super::cpu_arena::EVENTS,
        cached = sym super::cpu_arena::growth,
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
