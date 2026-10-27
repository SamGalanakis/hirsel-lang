//! A CPU allocation leaves only naked, fixed-size frames on its task stack.
//! The idle owner admits backing and allocates without retaining a heap borrow
//! across save. A refused request, including its logical admission, stays with
//! the task. No compiler emits this helper until allocating frames are proved.
use super::native::{Native, Physical};
use crate::stop::Stopped;
use crate::{Header, TaskCtx};

const IDLE: u32 = 0;
const REQUESTED: u32 = 1;
const SUMMARIZED: u32 = 2;
const CHARGED: u32 = 3;
pub(super) const REASON: u32 = 8;

#[repr(C)]
pub(super) struct Request {
    phase: u32,
    index: u32,
    extent: u64,
    align: u64,
    result: *mut Header,
}

impl Request {
    pub(super) fn new() -> Self {
        Self {
            phase: IDLE,
            index: 0,
            extent: 0,
            align: 0,
            result: std::ptr::null_mut(),
        }
    }
    pub(super) fn begin(&mut self, index: u32, extent: u64, align: u64) {
        assert_eq!(self.phase, IDLE);
        assert!(self.result.is_null());
        self.index = index;
        self.extent = extent;
        self.align = align;
        self.phase = REQUESTED;
    }
    pub(super) fn take_result(&mut self) -> *mut Header {
        assert_eq!(self.phase, IDLE);
        let result = std::mem::replace(&mut self.result, std::ptr::null_mut());
        assert!(!result.is_null());
        result
    }
    /// Logical admission is independent of the storage provider. Idle only;
    /// an admitted charge survives any number of physical backing refusals.
    pub(super) fn admit(&mut self, ctx: *mut TaskCtx, bytes: u64) -> Stopped<bool> {
        if self.phase == REQUESTED {
            self.phase = match super::super::engine::ahead_allocation(ctx, bytes)? {
                super::super::engine::CpuAllocation::Paused => return Ok(false),
                super::super::engine::CpuAllocation::Summarized => SUMMARIZED,
                super::super::engine::CpuAllocation::Settled => CHARGED,
            };
        }
        if self.phase == SUMMARIZED {
            let task = super::super::engine::current(ctx);
            if super::super::engine::with_driver(ctx, |d| d.head_ready(task)) {
                assert!(super::super::engine::host_segment_barrier(ctx)?);
                self.phase = CHARGED;
            }
        }
        assert!(matches!(self.phase, SUMMARIZED | CHARGED));
        Ok(true)
    }
    pub(super) fn complete(&mut self) {
        self.phase = IDLE;
    }
    pub(super) fn pending(&self) -> bool {
        self.phase != IDLE
    }
}

/// # Safety
/// Only a checked CPU mapper alternate, after its canonical job claim, calls
/// this helper. `state` is its exclusively active Native record. The requested
/// descriptor/extent/alignment satisfy T01's contract. The allocation is an
/// unpublished, task-owned heap block. No Rust frame runs on the task stack.
/// Its stack extent is 16 bytes plus AheadHandoff's 64-byte save.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_ahead_alloc(
    _state: *mut Physical,
    _index: u32,
    _extent: u64,
    _align: u64,
) -> *mut Header {
    core::arch::naked_asm!(
        "push rbx", "mov rbx, rdi",
        "cmp byte ptr [rbx + {cpu}], 0", "je 2f",
        "cmp dword ptr [rbx + {cpu_phase}], 2", "jne 2f",
        "cmp dword ptr [rbx + {request_phase}], 0", "jne 2f",
        "mov [rbx + {index}], esi", "mov [rbx + {extent}], rdx", "mov [rbx + {align}], rcx",
        "mov qword ptr [rbx + {result}], 0", "mov dword ptr [rbx + {request_phase}], {requested}",
        "mov esi, {reason}", "call {handoff}",
        "cmp dword ptr [rbx + {request_phase}], 0", "jne 2f",
        "mov rax, [rbx + {result}]", "test rax, rax", "je 2f",
        "mov qword ptr [rbx + {result}], 0", "pop rbx", "ret", "2:", "ud2",
        cpu = const std::mem::offset_of!(Native, cpu),
        cpu_phase = const std::mem::offset_of!(Native, cpu_phase),
        request_phase = const std::mem::offset_of!(Native, allocation) + std::mem::offset_of!(Request, phase),
        index = const std::mem::offset_of!(Native, allocation) + std::mem::offset_of!(Request, index),
        extent = const std::mem::offset_of!(Native, allocation) + std::mem::offset_of!(Request, extent),
        align = const std::mem::offset_of!(Native, allocation) + std::mem::offset_of!(Request, align),
        result = const std::mem::offset_of!(Native, allocation) + std::mem::offset_of!(Request, result),
        requested = const REQUESTED,
        reason = const REASON,
        handoff = sym super::native::hirsel_rt_v2_ahead_handoff,
    );
}

/// Run only on idle, with the role held and the active task's stack saved.
/// No stop or suspension occurs under a heap borrow. SUMMARIZED and CHARGED
/// survive repeated BackingFull responses and physical migration.
pub(super) unsafe fn serve(ctx: *mut TaskCtx, native: *mut Native) -> Stopped<bool> {
    assert!(
        crate::stop::running().is_null(),
        "CPU allocation runs on idle"
    );
    assert!(
        !super::private(ctx),
        "CPU allocation follows a canonical claim"
    );
    assert!(unsafe { (*native).cpu && (*native).cpu_phase == 2 });
    let request = unsafe { &mut (*native).allocation };
    assert!(request.pending());
    crate::alloc::check_request(request.extent, request.align);
    let bytes = unsafe {
        crate::alloc::allocation_charge(ctx, request.index, request.extent, request.align)
    };
    if !request.admit(ctx, bytes)? {
        return Ok(false);
    }
    if !super::cpu_withdrawal::acknowledged(ctx) {
        return Ok(false);
    }
    if unsafe { crate::alloc::cpu_preflight(ctx, request.index, request.extent, request.align) }
        .is_err()
    {
        super::cpu_withdrawal::request(ctx);
        return Ok(false);
    }
    // Idle Meter callbacks see no running task. Logical admission already
    // settled above; no callback is allowed to reenter or save this idle frame.
    let result = unsafe {
        crate::alloc::alloc_prepared(
            ctx,
            request.index,
            request.extent,
            request.align,
            crate::alloc::DOMAIN_HEAP,
            0,
        )
    };
    unsafe { crate::alloc::cpu_allocation_done(ctx) };
    super::cpu_withdrawal::release(ctx);
    if let Some(ledger) = unsafe { (*ctx).heap_ref().meter.cpu.get() } {
        let task = super::super::engine::current(ctx);
        let owner = super::super::engine::with_driver(ctx, |driver| {
            driver
                .cpu_batch
                .as_ref()
                .and_then(|batch| batch.block_owner(task))
        })
        .expect("a CPU allocation belongs to its claimed ordinal");
        ledger.blocks.lock().unwrap().adopt(owner, result);
    }
    request.result = result;
    request.phase = IDLE;
    Ok(true)
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests;
