//! A selected read permits one certified continuation, independently of spawn
//! admission. Pending inert cleanup and its old charge position stay task-owned.
use super::native::{Certificate, Info, Native, Physical};
use crate::{Header, TaskCtx};
use std::mem::size_of;

#[repr(C)]
pub struct ReadRecipe {
    pub buffer_descriptor: u32,
    pub maximum: u32,
    pub cleanup_slots: u32,
    pub flags: u32,
}
#[repr(C)]
pub struct Root {
    pub version: u32,
    pub flags: u32,
    pub read_count: u64,
    pub reads: *const *const ReadRecipe,
    pub identity: [u8; 32],
}
#[repr(C)]
pub struct HostInfo {
    pub prefix: Info,
    pub host: *const Root,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Pending {
    block: *mut Header,
    fuel: i64,
    charged: u8,
}
const EMPTY: Pending = Pending {
    block: std::ptr::null_mut(),
    fuel: 0,
    charged: 0,
};
const IDLE: u32 = 0;
const ARMED: u32 = 1;
const SELECTED: u32 = 2;
const ACTIVE: u32 = 3;
#[repr(C)]
pub(super) struct State {
    pub root: *const Root,
    armed: *const ReadRecipe,
    buffer: *mut *mut Header,
    phase: u32,
    task: u64,
    selected: Option<crate::tasks::ordered::host::CompletedRead>,
    pending: [Pending; 8],
}
const _: () = {
    assert!(size_of::<ReadRecipe>() == 16);
    assert!(size_of::<Root>() == 56);
    assert!(size_of::<HostInfo>() == 40);
    assert!(size_of::<Pending>() == 24);
};
impl State {
    pub(super) fn new(root: *const Root) -> Self {
        Self {
            root,
            armed: std::ptr::null(),
            buffer: std::ptr::null_mut(),
            phase: IDLE,
            task: 0,
            selected: None,
            pending: [EMPTY; 8],
        }
    }
    pub(super) fn active(&self) -> bool {
        self.phase == ACTIVE
    }
}
unsafe fn native(ctx: *mut TaskCtx) -> Option<*mut Native> {
    let p = unsafe { (*ctx).ahead_physical }.cast::<Native>();
    (!p.is_null() && !unsafe { (*p).host.root }.is_null()).then_some(p)
}
/// The v3 prefix remains the exact v2 prefix. No scalar v1/v2 record grants this capability.
pub(super) unsafe fn validate(
    ctx: *mut TaskCtx,
    env: *mut Header,
    info: *const HostInfo,
) -> Option<Certificate> {
    let _entry = unsafe { (*info).prefix.prefix.entry }?;
    let root = unsafe { (*info).host };
    if root.is_null() {
        return None;
    }
    let r = unsafe { &*root };
    if r.version != 1 || r.flags != 0 || !(1..=64).contains(&r.read_count) || r.reads.is_null() {
        return None;
    }
    let count = unsafe { (*ctx).heap_ref().layout_count() };
    for p in unsafe { std::slice::from_raw_parts(r.reads, r.read_count as usize) } {
        if p.is_null() {
            return None;
        }
        let recipe = unsafe { &**p };
        if recipe.flags != 0
            || !(1..=256).contains(&recipe.maximum)
            || !(1..=8).contains(&recipe.cleanup_slots)
            || recipe.buffer_descriptor >= count
        {
            return None;
        }
        if !unsafe { inert_buffer_layout(ctx, recipe.buffer_descriptor) } {
            return None;
        }
    }
    let mut cert = unsafe { super::native::validate_capture(ctx, env, &(*info).prefix, false) }?;
    cert.host = root;
    Some(cert)
}

/// Certify a byte sequence, including its holder metadata, before using the
/// shallow storage release. An empty scan explicitly certifies no holders.
unsafe fn inert_buffer_layout(ctx: *mut TaskCtx, index: u32) -> bool {
    use hirsel_core_contract::handle_scan::*;
    let layout = unsafe { &*(*ctx).descriptor(index) };
    if layout.magic != crate::LAYOUT_MAGIC
        || layout.version != crate::LAYOUT_VERSION
        || layout.size != 16
        || layout.align != 8
        || layout.finalizer.is_some()
        || layout.step_count != 0
        || layout.flags & (crate::LAYOUT_DYNAMIC | crate::LAYOUT_SEQUENCE)
            != crate::LAYOUT_DYNAMIC | crate::LAYOUT_SEQUENCE
        || layout.flags
            & !(crate::LAYOUT_DYNAMIC | crate::LAYOUT_SEQUENCE | crate::LAYOUT_LOGICAL_PER_ELEMENT)
            != 0
    {
        return false;
    }
    if layout.scan_words == 0 {
        return true;
    }
    if layout.scan_words != 2 * WORDS as u64 || layout.scan.is_null() {
        return false;
    }
    let words = unsafe { std::slice::from_raw_parts(layout.scan, 2 * WORDS) };
    let Ok(child) = u32::try_from(words[2]) else {
        return false;
    };
    if child >= unsafe { (*ctx).heap_ref().layout_count() }
        || words[..WORDS] != [TYPE_EDGE, 0, u64::from(child), 0, 0, 0, 0, 0]
        || words[WORDS..] != [VALUE, VALUE_SEQUENCE, 16, 1, 16, u64::from(child), 0, 0]
    {
        return false;
    }
    let element = unsafe { &*(*ctx).descriptor(child) };
    if element.magic != crate::LAYOUT_MAGIC
        || element.version != crate::LAYOUT_VERSION
        || element.size != 1
        || element.align != 1
        || element.finalizer.is_some()
        || element.step_count != 0
        || element.flags & !crate::LAYOUT_NO_EXTENT != 0
    {
        return false;
    }
    element.scan_words == 0
        || (element.scan_words == WORDS as u64
            && !element.scan.is_null()
            && unsafe { std::slice::from_raw_parts(element.scan, WORDS) }
                == [VALUE, VALUE_BITS, 1, 0, 0, 0, 0, 0])
}

/// Check the entire custody record before accounting or destroying any owner.
/// Like Tokio's task stage, only its admitted lifecycle owner consumes storage
/// (tokio runtime/task/core.rs:391-400, harness.rs:236-250,501-509).
unsafe fn valid_pending(ctx: *mut TaskCtx, state: &State) -> bool {
    if state.pending.iter().all(|p| p.block.is_null()) {
        return true;
    }
    if state.armed.is_null() || state.root.is_null() {
        return false;
    }
    let recipe = unsafe { &*state.armed };
    if !(1..=8).contains(&recipe.cleanup_slots)
        || recipe.buffer_descriptor >= unsafe { (*ctx).heap_ref().layout_count() }
        || !unsafe { inert_buffer_layout(ctx, recipe.buffer_descriptor) }
    {
        return false;
    }
    for (slot, pending) in state.pending.iter().enumerate() {
        let block = pending.block;
        if block.is_null() {
            continue;
        }
        if slot >= recipe.cleanup_slots as usize
            || unsafe { (*block).index() } != recipe.buffer_descriptor
            || !matches!(unsafe { (*block).count }, 1 | crate::count::COUNT_IMMORTAL)
            || unsafe { crate::region::is_tracked(block) }
            || (unsafe { (*block).count } == 1
                && (unsafe { (*block).domain() } != crate::alloc::DOMAIN_HEAP
                    || state.pending[..slot].iter().any(|p| p.block == block)))
        {
            return false;
        }
    }
    true
}

/// Only the explicit entry recipe, armed before this exact host read, permits
/// an early return. A selected result is still a separate custody certificate.
pub(crate) fn armed(ctx: *mut TaskCtx) -> bool {
    #[cfg(test)]
    if unsafe { (*ctx).test_host_tail } {
        return true;
    }
    unsafe { native(ctx).is_some_and(|n| matches!((*n).host.phase, ARMED | SELECTED)) }
}

pub(crate) fn selected(ctx: *mut TaskCtx) -> bool {
    #[cfg(test)]
    if unsafe { (*ctx).test_host_tail } {
        return true;
    }
    unsafe { native(ctx).is_some_and(|n| (*n).host.phase == SELECTED) }
}

/// # Safety
/// The immutable recipe belongs to the current compiled host root; buffer is
/// its exclusive List[u8] local, immediately before the selected read operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_host_arm(
    ctx: *mut TaskCtx,
    recipe: *const ReadRecipe,
    buffer: *mut *mut Header,
) {
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let Some(n) = (unsafe { native(ctx) }) else {
        return;
    };
    let state = unsafe { &mut (*n).host };
    assert_eq!(state.phase, IDLE);
    assert!(state.pending.iter().all(|p| p.block.is_null()));
    let root = unsafe { &*state.root };
    let recipes = unsafe { std::slice::from_raw_parts(root.reads, root.read_count as usize) };
    if !recipes.contains(&recipe) || buffer.is_null() {
        return;
    }
    let block = unsafe { buffer.read() };
    if block.is_null()
        || unsafe { (*block).index() } != unsafe { (*recipe).buffer_descriptor }
        || unsafe { crate::region::is_tracked(block) }
        || !matches!(unsafe { (*block).count }, 1 | crate::count::COUNT_IMMORTAL)
        || unsafe { crate::alloc::payload(block).cast::<u64>().read() } != 0
    {
        return;
    }
    state.task = crate::tasks::ordered::engine::current(ctx);
    state.armed = recipe;
    state.buffer = buffer;
    state.phase = ARMED;
}

/// Runtime conversion still owns the prefix. The selected payload, token and
/// task generation survive independently of its Rust frames and original wait row.
pub(crate) fn select(
    ctx: *mut TaskCtx,
    token: u64,
    request: &crate::io::Request,
    result: &Result<crate::io::Completion, crate::io::AdapterError>,
) {
    let Some(n) = (unsafe { native(ctx) }) else {
        return;
    };
    let state = unsafe { &mut (*n).host };
    if state.phase != ARMED {
        return;
    }
    let crate::io::Request::NetRead { max, .. } = request else {
        return;
    };
    if *max != u64::from(unsafe { (*state.armed).maximum }) {
        return;
    }
    let Some(selected) = crate::tasks::ordered::host::CompletedRead::new(token, request, result)
    else {
        return;
    };
    assert_eq!(state.task, crate::tasks::ordered::engine::current(ctx));
    state.selected = Some(selected);
    state.phase = SELECTED;
}

// Eight-byte leaf calls. The fixed metadata/assembly contract is included in
// helper identity, and every simultaneously active caller is in the frame DAG.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_host_enter(
    _state: *mut Physical,
    _recipe: *const ReadRecipe,
) {
    core::arch::naked_asm!(
        "cmp [rdi + {armed}], rsi", "jne 2f",
        "cmp dword ptr [rdi + {phase}], 1", "jne 3f", "mov dword ptr [rdi + {phase}], 0", "jmp 2f", "3:",
        "cmp dword ptr [rdi + {phase}], 2", "jne 2f",
        "mov dword ptr [rdi + {phase}], 3", "mov qword ptr [rdi], 1024", "2:", "ret",
        armed = const std::mem::offset_of!(Native, host) + std::mem::offset_of!(State, armed),
        phase = const std::mem::offset_of!(Native, host) + std::mem::offset_of!(State, phase),
    );
}
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_host_active(_state: *mut Physical) -> u8 {
    core::arch::naked_asm!("xor eax, eax", "cmp dword ptr [rdi + {phase}], 3", "sete al", "ret",
        phase = const std::mem::offset_of!(Native, host) + std::mem::offset_of!(State, phase));
}
/// # Safety
/// The certified acyclic continuation consumes its inert byte-buffer credit
/// once at this bounded slot. No release, allocator, destructor or observer runs.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_host_defer(
    _state: *mut Physical,
    _slot: u32,
    _block: *mut Header,
    _fuel: u64,
) {
    core::arch::naked_asm!("mov esi, esi", "imul rsi, rsi, 24", "add rsi, rdi",
        "mov [rsi + {pending}], rdx", "mov [rsi + {pending} + 8], rcx", "mov byte ptr [rsi + {pending} + 16], 0", "ret",
        pending = const std::mem::offset_of!(Native, host) + std::mem::offset_of!(State, pending));
}

/// Idle, before capturing the current fuel word. Insert each inert free at
/// its original word, then normal capture accounts the intervening pure points.
/// Physical storage remains owned by this fixed task record until admission.
pub(super) unsafe fn flush_logical(ctx: *mut TaskCtx, n: *mut Native) {
    let state = unsafe { &mut (*n).host };
    if state.root.is_null() {
        return;
    }
    assert!(
        unsafe { valid_pending(ctx, state) },
        "invalid host cleanup custody"
    );
    for pending in &mut state.pending {
        if pending.block.is_null() || pending.charged != 0 {
            continue;
        }
        let block = pending.block;
        if unsafe { (*block).count } == 1
            && unsafe {
                (*ctx)
                    .run_ref()
                    .ordered_memory
                    .load(std::sync::atomic::Ordering::Acquire)
            }
        {
            let heap = unsafe { (*ctx).heap_ref() };
            if !heap.meter.metered() {
                pending.charged = 1;
                continue;
            }
            let capacity = unsafe { heap.capacity(block) };
            let bytes = if unsafe { (*block).kind() } == crate::alloc::KIND_LARGE {
                crate::budget::large_charge(capacity)
            } else {
                crate::budget::small_charge(capacity)
            };
            unsafe { (*ctx).ordered_charge_trace.free(pending.fuel, bytes) };
            unsafe {
                (*ctx)
                    .ordered_freed
                    .set((*ctx).ordered_freed.get().saturating_add(bytes))
            };
        }
        pending.charged = 1;
    }
}

/// At an admitted canonical barrier, never on the task stack. Meter::credit
/// sees no running TLS task here, so the already journaled free is not duplicated.
pub(super) unsafe fn admit(ctx: *mut TaskCtx, n: *mut Native) {
    assert!(crate::stop::running().is_null());
    let state = unsafe { &mut (*n).host };
    assert_eq!(state.task, crate::tasks::ordered::engine::current(ctx));
    assert!(
        unsafe { valid_pending(ctx, state) },
        "invalid host cleanup custody"
    );
    assert!(
        state
            .pending
            .iter()
            .all(|p| p.block.is_null() || p.charged == 1)
    );
    // An active segment's exact resume point and selected original
    // descriptor, recorded once; a repeated admission adopts nothing. Its
    // fuel is logical throughout, so nothing private moves with it.
    if state.phase == ACTIVE {
        unsafe {
            (*n).admitted = Some(crate::continuation::PrivateSave {
                pc: crate::continuation::SavedFiber {
                    sp: (*n).physical.saved_sp,
                },
                generation: 0,
                fuel: 0,
                descriptor: Some((*state.armed).buffer_descriptor),
            })
        };
    }
    for pending in &mut state.pending {
        if pending.block.is_null() {
            continue;
        }
        assert_ne!(pending.charged, 0);
        let block = pending.block;
        // The complete record is inert and exclusively owned. Reuse the same
        // T16 storage primitive as release::drop_last's empty-program path;
        // admitting physical cleanup must never enter the general drop walker.
        if unsafe { (*block).count } != crate::count::COUNT_IMMORTAL {
            unsafe { (*ctx).heap().free(block) };
        }
        *pending = EMPTY;
    }
    state.phase = IDLE;
    state.armed = std::ptr::null();
    state.buffer = std::ptr::null_mut();
    state.selected = None;
}

#[cfg(all(test, target_arch = "x86_64"))]
pub(super) mod tests;
