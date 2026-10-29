//! A claimed job's private allocation lists, backed by ordinary worker heaps.
//! Like Go's mcache, a hit uses only the current owner's lists; shared policy
//! and backing admission run on idle misses. Ordinary headers let escape
//! transfer an existing block without copying or changing its logical charge.
use super::native::Native;
use crate::{Header, TaskCtx};

pub(super) const SLOTS: usize = 16;
pub(super) const EVENTS: usize = 128;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Entry {
    pub pointer: *mut Header,
    pub extent: u64,
    pub align: u64,
    pub charge: u64,
    pub capacity: u64,
    pub index: u32,
    pub live: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Event {
    pub pointer: *mut Header,
    pub charge: u64,
    pub allocation: u64,
}

#[repr(C)]
pub(super) struct Arena {
    pub enabled: u64,
    pub ordinal: u64,
    pub traced: u64,
    pub reuses: u64,
    pub count: u64,
    cursor: usize,
    pub entries: [Entry; SLOTS],
    pub events: [Event; EVENTS],
}

impl Arena {
    pub(super) fn new() -> Self {
        Self {
            enabled: 0,
            ordinal: 0,
            traced: 0,
            reuses: 0,
            count: 0,
            cursor: 0,
            entries: [Entry::default(); SLOTS],
            events: [Event::default(); EVENTS],
        }
    }
}

/// The native scan also carries type edges and value observations. These do
/// not read holders; rejecting every nonempty program would reject scalar
/// sequence payloads too. Actual holder instructions and unknown scans refuse.
/// # Safety
/// The descriptor is checked and its scan words are readable and immutable.
pub(super) unsafe fn inert_layout(layout: &crate::Layout) -> bool {
    use hirsel_core_contract::handle_scan::{TYPE_EDGE, VALUE, VALUE_COLUMN, VALUE_FIELD, WORDS};
    if layout.finalizer.is_some() || layout.step_count != 0 {
        return false;
    }
    if layout.scan_words == 0 {
        return true;
    }
    let Ok(words) = usize::try_from(layout.scan_words) else {
        return false;
    };
    if !words.is_multiple_of(WORDS)
        || words > isize::MAX as usize / size_of::<u64>()
        || layout.scan.is_null()
        || !layout.scan.addr().is_multiple_of(align_of::<u64>())
    {
        return false;
    }
    // SAFETY: the caller supplies a checked native descriptor and immutable
    // readable scan words. Only non-holder instructions are admitted.
    unsafe { std::slice::from_raw_parts(layout.scan, words) }
        .chunks_exact(WORDS)
        .all(|op| matches!(op[0], TYPE_EDGE | VALUE | VALUE_FIELD | VALUE_COLUMN))
}

/// Register fresh canonical backing, without changing its already admitted
/// logical allocation. Only layouts with no child owners or finalizers enter
/// these lists. Fuel-bearing jobs retain their exact canonical interleaving.
/// # Safety
/// Idle owns the role, this Native and the fresh, initialized header.
pub(super) unsafe fn adopt(
    ctx: *mut TaskCtx,
    native: *mut Native,
    pointer: *mut Header,
    extent: u64,
    align: u64,
) {
    // SAFETY: the canonical service retains exclusive ownership until resume.
    unsafe {
        if cfg!(feature = "memcheck")
            || cfg!(feature = "measurement")
            || cfg!(feature = "ownership-trace")
            || !(*native).cpu
            || !matches!((*native).cpu_phase, 2 | 3)
        {
            return;
        }
        if super::super::engine::with_driver(ctx, |driver| driver.budgets.fuel) {
            return;
        }
        let Some(owner) = (unsafe { (*native).cpu_owner() }) else {
            return;
        };
        if (*native).arena.enabled != 0 {
            assert_eq!((*native).arena.ordinal, owner.ordinal);
        }
        (*native).arena.ordinal = owner.ordinal;
        (*native).arena.traced = u64::from(super::super::events::observing(ctx));
        let index = (*pointer).index();
        let layout = &*(*ctx).descriptor(index);
        if !inert_layout(layout) {
            return;
        }
        let Some(entry) = (*native)
            .arena
            .entries
            .iter_mut()
            .find(|e| e.pointer.is_null())
        else {
            return;
        };
        *entry = Entry {
            pointer,
            extent,
            align,
            charge: crate::alloc::allocation_charge(ctx, index, extent, align),
            capacity: (*ctx).heap_ref().capacity(pointer),
            index,
            live: 1,
        };
        (*native).arena.enabled = 1;
    }
}

/// Remove backing that a canonical service will consume, while it is still
/// live. Shared detach and in-place reserve keep their registered original.
pub(super) unsafe fn forget(native: *mut Native, pointer: *mut Header) {
    // SAFETY: idle owns this job; no mapper instruction runs concurrently.
    unsafe {
        for entry in &mut (*native).arena.entries {
            if entry.pointer == pointer {
                assert_eq!(entry.live, 1);
                *entry = Entry::default();
            }
        }
    }
}

/// Apply the exact allocation/free order at the next canonical boundary.
/// A physical refusal retains the unconsumed event and all arena backing.
/// Nothing here creates a semantic operation or a receipt boundary.
/// # Safety
/// Idle holds the scheduler role and the exclusive saved Native image.
pub(super) unsafe fn flush(ctx: *mut TaskCtx, native: *mut Native) -> crate::stop::Stopped<bool> {
    // SAFETY: each event and cursor belongs to this saved job. No arena borrow
    // crosses driver admission, which may report a stop or physical pause.
    unsafe {
        if (*native).arena.enabled == 0 {
            return Ok(true);
        }
        if (*native).arena.traced == 0 {
            // Every reusable block keeps its already-admitted logical charge.
            // Reuse cannot exceed the peak at which all current slots were
            // admitted live. Before any new canonical allocation/boundary,
            // return the currently dead slots and their charges. Thus the
            // memory-only envelope and net are exact without a per-use log.
            let reuses = std::mem::take(&mut (*native).arena.reuses);
            (*ctx).heap_ref().meter.cpu_reuses(reuses);
            for entry in &mut (*native).arena.entries {
                if !entry.pointer.is_null() && entry.live == 0 {
                    super::super::engine::memory_freed(ctx, entry.charge);
                    (*ctx).heap().free(entry.pointer);
                    *entry = Entry::default();
                }
            }
            return Ok(true);
        }
        while (*native).arena.cursor < (*native).arena.count as usize {
            let event = (*native).arena.events[(*native).arena.cursor];
            if event.allocation != 0 {
                if matches!(
                    super::super::engine::ahead_allocation(ctx, event.charge)?,
                    super::super::engine::CpuAllocation::Paused
                ) {
                    return Ok(false);
                }
                (*ctx).heap_ref().meter.charge_allocation(event.charge);
                if let Some(ledger) = (*ctx).heap_ref().meter.cpu.get() {
                    let task = super::super::engine::current(ctx);
                    let owner = super::super::engine::with_driver(ctx, |driver| {
                        driver
                            .cpu_batch
                            .as_ref()
                            .and_then(|batch| batch.block_owner(task))
                    })
                    .expect("a CPU arena retains its claim");
                    ledger.blocks.lock().unwrap().adopt(owner, event.pointer);
                }
            } else {
                super::super::engine::memory_freed(ctx, event.charge);
                (*ctx).heap_ref().meter.credit(event.charge);
                if let Some(ledger) = (*ctx).heap_ref().meter.cpu.get() {
                    ledger.blocks.lock().unwrap().release(event.pointer);
                }
            }
            (*native).arena.cursor += 1;
        }
        (*native).arena.count = 0;
        (*native).arena.cursor = 0;
        // A miss must return dead physical backing before requesting another
        // class. Otherwise cached, already-credited blocks can prevent the
        // current job from satisfying its own backing admission.
        if (*native).allocation.pending() || !(*native).growth.is_null() {
            for entry in &mut (*native).arena.entries {
                if !entry.pointer.is_null() && entry.live == 0 {
                    (*ctx).heap().free_cached(entry.pointer);
                    *entry = Entry::default();
                }
            }
        }
        Ok(true)
    }
}

/// End private custody at a runtime escape, publication or return.
/// Live values already have canonical headers/backing and keep their holders.
/// Dead cache entries return physical storage without a second logical free.
/// # Safety
/// Idle owns this image, and its event journal has been fully flushed.
pub(super) unsafe fn canonical(ctx: *mut TaskCtx, native: *mut Native) {
    // SAFETY: clearing an entry transfers a live block or frees dead backing
    // exactly once. Normal heap teardown owns all blocks if the run stops.
    unsafe {
        assert_eq!((*native).arena.count, 0);
        for entry in &mut (*native).arena.entries {
            if !entry.pointer.is_null() && entry.live == 0 {
                (*ctx).heap().free_cached(entry.pointer);
            }
            *entry = Entry::default();
        }
        (*native).arena.enabled = 0;
    }
}

/// Bind the complete arena, including physically retained dead cache blocks,
/// to the same saved frame and ordinal as ordinary CPU allocations.
pub(super) fn snapshot(native: &Native) -> [Entry; SLOTS] {
    native.arena.entries
}

/// Fixed private leaf. A miss preserves the request arguments and saves no
/// frame; the caller takes its ordinary canonical handoff after this returns.
/// # Safety
/// The caller owns the checked CPU Native and supplies a valid T01 request.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
pub(super) unsafe extern "C" fn allocate(
    _native: *mut Native,
    _index: u32,
    _extent: u64,
    _align: u64,
) -> *mut Header {
    core::arch::naked_asm!(
        "xor eax, eax",
        "cmp qword ptr [rdi + {enabled}], 0", "je 5f",
        "mov r9, [rdi + {count}]", "cmp r9, {events}", "jae 5f",
        "lea r8, [rdi + {entries}]", "mov r10d, {slots}",
        "2:", "cmp qword ptr [r8], 0", "je 4f",
        "cmp dword ptr [r8 + {live}], 0", "jne 4f",
        "cmp [r8 + {index}], esi", "jne 4f",
        "cmp [r8 + {extent}], rdx", "jne 4f",
        "cmp [r8 + {align}], rcx", "jne 4f",
        "mov rax, [r8]", "mov dword ptr [r8 + {live}], 1",
        // The descriptor word retains its canonical small/large storage kind.
        "mov dword ptr [rax], 1",
        "cmp qword ptr [rdi + {traced}], 0", "jne 6f",
        "inc qword ptr [rdi + {reuses}]", "ret", "6:",
        "imul r9, r9, {event_bytes}", "lea r9, [rdi + r9 + {journal}]",
        "mov [r9], rax", "mov r11, [r8 + {charge}]", "mov [r9 + 8], r11",
        "mov qword ptr [r9 + 16], 1", "inc qword ptr [rdi + {count}]", "ret",
        "4:", "add r8, {entry_bytes}", "dec r10d", "jne 2b",
        "5:", "ret",
        traced = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, traced),
        reuses = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, reuses),
        enabled = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, enabled),
        count = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, count),
        entries = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, entries),
        journal = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, events),
        live = const std::mem::offset_of!(Entry, live),
        index = const std::mem::offset_of!(Entry, index),
        extent = const std::mem::offset_of!(Entry, extent),
        align = const std::mem::offset_of!(Entry, align),
        charge = const std::mem::offset_of!(Entry, charge),
        entry_bytes = const size_of::<Entry>(),
        event_bytes = const size_of::<Event>(),
        slots = const SLOTS,
        events = const EVENTS,
    );
}

/// # Safety
/// The checked caller owns one of the block's credits. The leaf accepts only a
/// pointer-free block already registered in this exact job's arena.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
pub(super) unsafe extern "C" fn release(_native: *mut Native, _block: *mut Header) -> u8 {
    core::arch::naked_asm!(
        "xor eax, eax",
        "cmp qword ptr [rdi + {enabled}], 0", "je 5f",
        "mov r9, [rdi + {count}]", "cmp r9, {events}", "jae 5f",
        "lea r8, [rdi + {entries}]", "mov r10d, {slots}",
        "2:", "cmp [r8], rsi", "jne 4f",
        "cmp dword ptr [r8 + {live}], 1", "jne 5f",
        "cmp dword ptr [rsi], 1", "je 7f",
        "cmp dword ptr [rsi], {arena_shared}", "jae 5f",
        "cmp dword ptr [rsi], 1", "jbe 5f",
        "dec dword ptr [rsi]", "mov eax, 1", "ret", "7:",
        "mov dword ptr [r8 + {live}], 0",
        "cmp qword ptr [rdi + {traced}], 0", "jne 6f", "mov eax, 1", "ret", "6:",
        "imul r9, r9, {event_bytes}", "lea r9, [rdi + r9 + {journal}]",
        "mov [r9], rsi", "mov r11, [r8 + {charge}]", "mov [r9 + 8], r11",
        "mov qword ptr [r9 + 16], 0", "inc qword ptr [rdi + {count}]",
        "mov eax, 1", "ret",
        "4:", "add r8, {entry_bytes}", "dec r10d", "jne 2b",
        "5:", "ret",
        arena_shared = const crate::count::COUNT_ARENA_SHARED,
        traced = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, traced),
        enabled = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, enabled),
        count = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, count),
        entries = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, entries),
        journal = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, events),
        live = const std::mem::offset_of!(Entry, live),
        charge = const std::mem::offset_of!(Entry, charge),
        entry_bytes = const size_of::<Entry>(),
        event_bytes = const size_of::<Event>(),
        slots = const SLOTS,
        events = const EVENTS,
    );
}

/// Fixed growth/copy/cleanup leaf. Every nested call returns before a miss can
/// save, and its maximum live stack footprint is below the existing 80 bytes.
/// # Safety
/// The checked CPU caller owns this request and all its borrowed input credits.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
pub(super) unsafe extern "C" fn growth(
    _native: *mut Native,
    _request: *mut super::cpu_handoff::Growth,
) -> u8 {
    core::arch::naked_asm!(
        "push rbx", "push r12", "push r13", "push r14", "mov rbx, rdi", "mov r12, rsi",
        "xor eax, eax", "cmp qword ptr [rbx + {enabled}], 0", "je 9f",
        "cmp dword ptr [r12], 1", "jne 9f",
        "mov rdx, [r12 + 8]", "cmp [rbx + {ordinal}], rdx", "jne 9f",
        "cmp dword ptr [r12 + 4], 6", "je 6f",
        "cmp dword ptr [r12 + 4], 7", "je 10f",
        "cmp dword ptr [r12 + 4], 1", "je 3f",
        "cmp dword ptr [r12 + 4], 5", "jne 9f",
        // Copy only a registered owner; ordinary inputs stay canonical.
        "mov rdx, [r12 + 16]", "lea r8, [rbx + {entries}]", "mov r10d, {slots}",
        "2:", "cmp [r8], rdx", "je 7f", "add r8, {entry_bytes}", "dec r10d", "jne 2b", "jmp 9f",
        "7:", "cmp dword ptr [r8 + {live}], 1", "jne 9f",
        "mov r13, [r12 + 16]", "mov r14, [r8 + {capacity}]", "mov rdx, r14", "mov rcx, [r8 + {align}]",
        "mov esi, [r8 + {index}]", "cmp [r12 + 32], esi", "jne 8f",
        "mov rdi, rbx", "call {allocate}", "test rax, rax", "je 9f",
        "mov [r12 + 16], rax", "lea rdi, [rax + 8]",
        "lea rsi, [r13 + 8]", "mov rcx, r14", "rep movsb",
        "mov eax, 1", "jmp 9f",
        // Reserve an empty static list. The canonical allocator determined
        // the cached extent, capacity and charge at this exact request size.
        "3:", "mov rdx, [r12 + 16]", "test rdx, rdx", "je 9f",
        "cmp dword ptr [rdx], -1", "jne 9f",
        "mov ecx, [rdx + 4]", "shr ecx, 29", "cmp ecx, 2", "jne 9f",
        "cmp qword ptr [rdx + 8], 0", "jne 9f",
        "mov rcx, [r12 + 72]", "cmp rcx, 16", "ja 9f", "test rcx, rcx", "je 9f",
        "lea rdx, [rcx - 1]", "test rcx, rdx", "jne 9f",
        "mov r13, [r12 + 64]", "test r13, r13", "je 9f",
        "mov rax, [r12 + 80]", "mul r13", "test rdx, rdx", "jne 8f",
        "add rax, 16", "jc 8f", "mov rdx, rax",
        "cmp rcx, 8", "jae 4f", "mov ecx, 8", "4:",
        "mov esi, [r12 + 32]", "mov rdi, rbx", "call {allocate}", "test rax, rax", "je 9f",
        "mov [r12 + 16], rax", "mov qword ptr [rax + 8], 0",
        "mov rcx, rax", "mov rax, [r8 + {capacity}]", "sub rax, 16", "xor edx, edx", "div r13",
        "mov [rcx + 16], rax", "mov eax, 1", "jmp 9f",
        "6:", "mov rsi, [r12 + 16]", "test rsi, rsi", "je 9f",
        "mov edx, [rsi + 4]", "and edx, {index_mask}", "cmp [r12 + 32], edx", "jne 9f",
        "call {release}", "jmp 9f",
        "10:", "mov rdx, [r12 + 16]", "test rdx, rdx", "je 9f",
        "lea r8, [rbx + {entries}]", "mov r10d, {slots}",
        "11:", "cmp [r8], rdx", "je 12f",
        "add r8, {entry_bytes}", "dec r10d", "jne 11b", "jmp 9f",
        "12:", "cmp dword ptr [r8 + {live}], 1", "jne 9f",
        "mov eax, [r8 + {index}]", "cmp [r12 + 32], eax", "jne 8f",
        "mov eax, [rdx]", "test eax, eax", "je 8f",
        "mov rcx, [rbx + {ctx}]", "cmp eax, [rcx + {dup_max}]", "jae 8f",
        "inc dword ptr [rdx]", "mov eax, 1", "jmp 9f",
        "8:", "xor eax, eax",
        "9:", "pop r14", "pop r13", "pop r12", "pop rbx", "ret",
        ctx = const std::mem::offset_of!(Native, ctx),
        dup_max = const crate::alloc::CTX_DUP_MAX_OFFSET,
        enabled = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, enabled),
        entries = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, entries),
        live = const std::mem::offset_of!(Entry, live),
        index = const std::mem::offset_of!(Entry, index),
        align = const std::mem::offset_of!(Entry, align),
        capacity = const std::mem::offset_of!(Entry, capacity),
        entry_bytes = const size_of::<Entry>(),
        slots = const SLOTS,
        ordinal = const std::mem::offset_of!(Native, arena) + std::mem::offset_of!(Arena, ordinal),
        index_mask = const crate::alloc::INDEX_MASK,
        allocate = sym allocate,
        release = sym release,
    );
}

#[cfg(test)]
mod tests {
    use super::inert_layout;
    use hirsel_core_contract::handle_scan::*;

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn arena_aliases_detach_release_and_refuse_dup_without_losing_custody() {
        use super::*;
        use crate::tasks::ordered::ahead::{cpu_handoff::Growth, native::Certificate};
        unsafe extern "C" fn unused(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
            crate::STATUS_OK
        }
        let outcome = crate::host::BodyRun::new(1, false).run(
            |_| {},
            |ctx| {
                // SAFETY: this BodyRun owns its context and all initialized
                // heap buffers until their explicit return below. The fixture
                // models one checked ordinal without entering scheduler code.
                unsafe {
                    let source = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        0,
                        32,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    let target = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        0,
                        32,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    let bytes = (*ctx).heap_ref().capacity(source);
                    assert_eq!(bytes, (*ctx).heap_ref().capacity(target));
                    std::ptr::write_bytes(crate::alloc::payload(source), 7, bytes as usize);
                    let mut live = 1;
                    let mut native = Native::new(
                        ctx,
                        std::ptr::null_mut(),
                        0,
                        Certificate {
                            entry: unused,
                            stack_bytes: 256,
                            host: std::ptr::null(),
                            resume: None,
                            cpu: false,
                            preclaimed: false,
                            live: &raw mut live,
                            mask: 1,
                            source: std::ptr::null_mut(),
                        },
                    );
                    native.cpu = true;
                    native.cpu_phase = 2;
                    native.arena.enabled = 1;
                    native.arena.ordinal = 9;
                    for (i, pointer) in [source, target].into_iter().enumerate() {
                        native.arena.entries[i] = Entry {
                            pointer,
                            extent: bytes,
                            capacity: bytes,
                            align: 8,
                            charge: 0,
                            index: 0,
                            live: u32::from(i == 0),
                        };
                    }
                    let dup_bound = (*ctx).view.compiled.dup_max;
                    (*ctx).view.compiled.dup_max = 8;
                    let mut request = Growth {
                        version: 1,
                        kind: 7,
                        ordinal: 9,
                        block: source,
                        region: std::ptr::null_mut(),
                        index: 0,
                        flags: 0,
                        extent: 0,
                        align: 0,
                        keep: 0,
                        element_size: 0,
                        element_align: 0,
                        capacity: 0,
                    };
                    assert_eq!(growth(&raw mut *native, &raw mut request), 1);
                    assert_eq!((*source).count, 2);
                    request.kind = 5;
                    assert_eq!(growth(&raw mut *native, &raw mut request), 1);
                    assert_eq!(request.block, target);
                    assert_eq!((*target).count, 1);
                    crate::alloc::payload(target).write(99);
                    assert_eq!(crate::alloc::payload(source).read(), 7);
                    assert_eq!(release(&raw mut *native, source), 1);
                    assert_eq!((*source).count, 1);
                    assert_eq!(native.arena.entries[0].live, 1);
                    assert_eq!(release(&raw mut *native, target), 1);
                    assert_eq!(native.arena.entries[1].live, 0);
                    request.kind = 7;
                    request.block = source;
                    (*source).count = 8;
                    assert_eq!(growth(&raw mut *native, &raw mut request), 0);
                    assert_eq!((*source).count, 8);
                    assert_eq!(native.arena.entries[0].live, 1);
                    (*source).count = 1;
                    (*ctx).view.compiled.dup_max = 0;
                    assert_eq!(growth(&raw mut *native, &raw mut request), 0);
                    assert_eq!((*source).count, 1);
                    (*ctx).view.compiled.dup_max = 8;
                    request.ordinal += 1;
                    assert_eq!(growth(&raw mut *native, &raw mut request), 0);
                    assert_eq!((*source).count, 1);
                    assert_eq!(native.arena.enabled, 1);
                    (*ctx).view.compiled.dup_max = dup_bound;
                    native.arena = Arena::new();
                    (*ctx).heap().free(source);
                    (*ctx).heap().free(target);
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    }

    #[test]
    fn arena_layout_ignores_observations_and_rejects_holder_or_unknown_scans() {
        let mut layout = crate::text::hirsel_rt_v2_bytes_descriptor.0;
        let mut scan = [
            TYPE_EDGE,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            TYPE_EDGE,
            0,
            2,
            0,
            0,
            0,
            0,
            0,
            VALUE,
            VALUE_SEQUENCE,
            16,
            8,
            16,
            2,
            0,
            0,
        ];
        layout.scan = scan.as_ptr();
        layout.scan_words = scan.len() as u64;
        // SAFETY: this local descriptor names the initialized array above.
        assert!(unsafe { inert_layout(&layout) });
        for opcode in [
            HANDLE,
            BLOCK,
            ROOT,
            INLINE,
            UNKNOWN,
            TYPE_UNKNOWN,
            TYPED_HANDLE,
        ] {
            scan[0] = opcode;
            // SAFETY: only the opcode changes, with the same readable words.
            assert!(!unsafe { inert_layout(&layout) }, "opcode {opcode}");
        }
        scan[0] = TYPE_EDGE;
        layout.step_count = 1;
        // SAFETY: the scan remains initialized; steps are never read here.
        assert!(!unsafe { inert_layout(&layout) });
        layout.step_count = 0;
        layout.scan_words -= 1;
        // SAFETY: a malformed length must refuse before reading any words.
        assert!(!unsafe { inert_layout(&layout) });
        layout.scan_words = u64::MAX;
        // SAFETY: unknown metadata must refuse before reading any words.
        assert!(!unsafe { inert_layout(&layout) });
    }
}
