//! The bounded x86 SysV boundary. Only these naked instructions run on a
//! private stack; accounting, roles and stop handling run on idle.
use super::super::super::TaskCode;
use super::*;
use crate::{Header, TaskCtx};

#[repr(C)]
pub struct Physical {
    pub points_left: i64,
    pub saved_sp: *mut u8,
    pub idle_sp: *mut u8,
    pub reason: u32,
    pub reserved_zero: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Recipe {
    pub wrapper_descriptor: u32,
    pub source_descriptor: u32,
    pub wrapper_extent: u32,
    pub callable_offset: u32,
    pub live_word_offset: u32,
    pub source_extent: u32,
    pub live_mask: u64,
    pub source_code: *const u8,
}
#[repr(C)]
pub struct Info {
    pub prefix: SpawnAheadInfo,
    pub stack_bytes: u64,
    pub recipe: *const Recipe,
}

pub(crate) struct Certificate {
    pub entry: TaskCode,
    pub stack_bytes: u64,
    pub(crate) host: *const super::host_return::Root,
    /// A v4 root: the ordinary dispatcher a save continues in, and its frame.
    pub(crate) resume: Option<(crate::continuation::ResumeDispatcher, u32)>,
    pub(crate) cpu: bool,
    pub(crate) preclaimed: bool,
    pub(super) live: *mut u64,
    pub(super) mask: u64,
    pub(super) source: *mut Header,
}
#[repr(C)]
pub(crate) struct Native {
    pub physical: Physical,
    entry: TaskCode,
    pub(super) environment: *mut Header,
    operation_site: u64,
    status: u8,
    pub pending_refill: bool,
    pub(super) ctx: *mut TaskCtx,
    live: *mut u64,
    original_mask: u64,
    pub(super) source: *mut Header,
    pub(super) host: super::host_return::State,
    /// The last admitted host-return segment.
    pub(crate) admitted: Option<crate::continuation::PrivateSave>,
    /// The frame a v4 entry runs over (null before v4); `start` passes it.
    frame: *mut u8,
    resume: Option<(
        crate::continuation::ResumeDispatcher,
        crate::continuation::OwnedResumeFrame,
    )>,
    /// The saved frame left for its ordinary dispatcher: this physical record
    /// no longer owns any of the body's state.
    continued: bool,
    pub(super) cpu: bool,
    pub(super) cpu_phase: u32,
    pub(super) allocation: super::cpu_allocation::Request,
    pub(super) summary: *mut super::cpu_handoff::Summary,
    pub(super) summary_status: u32,
    pub(super) summary_blocks: Vec<crate::pool::cpu_scoped::ledger::blocks::Block>,
    pub(super) growth: *mut super::cpu_handoff::Growth,
    pub(super) growth_status: u32,
    pub(super) growth_plan: Option<super::cpu_handoff::growth::Plan>,
    pub(crate) cpu_job: Option<std::sync::Arc<crate::pool::cpu_scoped::probe::Job>>,
    pub(super) saved_roots: Option<super::cpu_roots::Saved>,
    pub(super) input_blocks: Vec<crate::pool::cpu_scoped::ledger::blocks::Observed>,
    pub(super) arena: super::cpu_arena::Arena,
    pub(crate) cpu_claim: Option<std::sync::Arc<super::super::cpu::ClaimIdentity>>,
    pub(crate) cpu_fuel: super::cpu_fuel::Fuel,
    pub(super) restart: super::cpu_restart::State,
}
impl Drop for Native {
    fn drop(&mut self) {
        self.restart.discard(&self.arena);
        // Normal cleanup has left this physical record. A stopped, retained
        // native worker never gets here until its actual native owner exits.
        if let Some(identity) = &self.cpu_claim
            && identity.lease().publication.verdict().is_some()
        {
            identity.lease().publication.drained();
        }
    }
}

const _: () = {
    assert!(std::mem::offset_of!(Physical, points_left) == 0);
    assert!(std::mem::offset_of!(Physical, reserved_zero) == 28);
    assert!(std::mem::offset_of!(Recipe, wrapper_descriptor) == 0);
    assert!(std::mem::offset_of!(Recipe, source_descriptor) == 4);
    assert!(std::mem::offset_of!(Recipe, wrapper_extent) == 8);
    assert!(std::mem::offset_of!(Recipe, callable_offset) == 12);
    assert!(std::mem::offset_of!(Recipe, live_word_offset) == 16);
    assert!(std::mem::offset_of!(Recipe, source_extent) == 20);
    assert!(std::mem::offset_of!(Info, prefix) == 0);
    assert!(size_of::<Physical>() == 32 && align_of::<Physical>() == 8);
    assert!(std::mem::offset_of!(Physical, saved_sp) == 8);
    assert!(std::mem::offset_of!(Physical, idle_sp) == 16);
    assert!(std::mem::offset_of!(Physical, reason) == 24);
    assert!(size_of::<Recipe>() == 40 && align_of::<Recipe>() == 8);
    assert!(std::mem::offset_of!(Recipe, live_mask) == 24);
    assert!(std::mem::offset_of!(Recipe, source_code) == 32);
    assert!(size_of::<Info>() == 32 && align_of::<Info>() == 8);
    assert!(std::mem::offset_of!(Info, stack_bytes) == 16);
    assert!(std::mem::offset_of!(Info, recipe) == 24);
    assert!(std::mem::offset_of!(Native, entry) == 32);
    assert!(std::mem::offset_of!(Native, environment) == 40);
    assert!(std::mem::offset_of!(Native, operation_site) == 48);
    assert!(std::mem::offset_of!(Native, status) == 56);
    assert!(std::mem::offset_of!(Native, ctx) == 64);
};

/// Validate the narrow two-block capture image without weakening plain_layouts.
/// # Safety
/// A v2 record and the ordinary owned closure/environment are live and immutable.
pub(crate) unsafe fn validate(
    ctx: *mut TaskCtx,
    environment: *mut Header,
    info: *const Info,
) -> Option<Certificate> {
    unsafe { validate_capture(ctx, environment, info, true) }
}

pub(super) unsafe fn validate_capture(
    ctx: *mut TaskCtx,
    environment: *mut Header,
    info: *const Info,
    inert: bool,
) -> Option<Certificate> {
    unsafe {
        let info = &*info;
        let entry = info.prefix.entry?;
        if !(80..=65536).contains(&info.stack_bytes)
            || info.recipe.is_null()
            || environment.is_null()
        {
            return None;
        }
        let r = &*info.recipe;
        let count = (*ctx).heap_ref().layout_count();
        if r.wrapper_descriptor >= count
            || (r.source_descriptor != u32::MAX && r.source_descriptor >= count)
            || r.live_mask != 1
            || r.wrapper_extent > 256
            || r.source_extent > 256
            || r.callable_offset
                .checked_add(16)
                .is_none_or(|end| end > r.wrapper_extent)
            || r.live_word_offset
                .checked_add(8)
                .is_none_or(|end| end > r.wrapper_extent)
            || !(r.callable_offset + 16 <= r.live_word_offset
                || r.live_word_offset + 8 <= r.callable_offset)
            || r.callable_offset % 8 != 0
            || r.live_word_offset % 8 != 0
            || (*environment).count != crate::count::COUNT_UNIQUE
            || (*environment).index() != r.wrapper_descriptor
        {
            return None;
        }
        let wrapper = &*(*ctx).descriptor(r.wrapper_descriptor);
        if wrapper.size != u64::from(r.wrapper_extent)
            || wrapper.align > 16
            || wrapper.finalizer.is_some()
            || wrapper.flags
                & (crate::LAYOUT_DYNAMIC | crate::LAYOUT_UNSAFE_FFI | crate::LAYOUT_MUTEX_ROOT)
                != 0
        {
            return None;
        }
        let payload = crate::alloc::payload(environment);
        let live = payload.add(r.live_word_offset as usize).cast::<u64>();
        let mask = live.read();
        if mask != r.live_mask {
            return None;
        }
        let callable = payload.add(r.callable_offset as usize).cast::<usize>();
        let source_code = r.source_code as usize;
        if source_code == 0 {
            return None;
        }
        if callable.read() != source_code {
            return None;
        }
        let source = callable.add(1).read() as *mut Header;
        if source.is_null() {
            if r.source_descriptor != u32::MAX || r.source_extent != 0 {
                return None;
            }
        } else {
            if r.source_descriptor == u32::MAX || (*source).index() != r.source_descriptor {
                return None;
            }
            let layout = &*(*ctx).descriptor(r.source_descriptor);
            if layout.size != u64::from(r.source_extent)
                || layout.align > 16
                || layout.finalizer.is_some()
                || (inert && (layout.step_count != 0 || !inert_source(ctx, r.source_descriptor)))
                || layout.flags
                    & (crate::LAYOUT_DYNAMIC | crate::LAYOUT_UNSAFE_FFI | crate::LAYOUT_MUTEX_ROOT)
                    != 0
            {
                return None;
            }
        }
        Some(Certificate {
            entry,
            stack_bytes: info.stack_bytes,
            host: std::ptr::null(),
            resume: None,
            cpu: false,
            preclaimed: false,
            live,
            mask,
            source,
        })
    }
}
/// A descriptor may contain canonical value/type metadata without scanning any
/// owner. Follow only fixed inert field edges; general plain_layouts and the
/// wrapper's TYPE_UNKNOWN remain unchanged. This runs before dispatch.
pub(super) unsafe fn inert_source(ctx: *mut TaskCtx, root: u32) -> bool {
    use hirsel_core_contract::handle_scan::*;
    let mut pending = vec![root];
    let mut seen = std::collections::BTreeSet::new();
    let count = unsafe { (*ctx).heap_ref().layout_count() };
    let mut work = 0usize;
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        if id >= count || seen.len() > 64 {
            return false;
        }
        let layout = unsafe { &*(*ctx).descriptor(id) };
        if layout.size > 256
            || layout.align > 16
            || layout.finalizer.is_some()
            || layout.step_count != 0
            || layout.flags
                & (crate::LAYOUT_DYNAMIC | crate::LAYOUT_UNSAFE_FFI | crate::LAYOUT_MUTEX_ROOT)
                != 0
            || layout.scan_words > 4096
            || !layout.scan_words.is_multiple_of(WORDS as u64)
            || (layout.scan_words != 0 && layout.scan.is_null())
        {
            return false;
        }
        let words = if layout.scan_words == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(layout.scan, layout.scan_words as usize) }
        };
        for op in words.chunks_exact(WORDS) {
            work += 1;
            if work > 4096 {
                return false;
            }
            match op[0] {
                TYPE_EDGE | VALUE_FIELD => {
                    let Ok(child) = u32::try_from(op[2]) else {
                        return false;
                    };
                    if child >= count {
                        return false;
                    }
                    if op[0] == VALUE_FIELD {
                        let size = unsafe { (*(*ctx).descriptor(child)).size };
                        if op[1].checked_add(size).is_none_or(|end| end > layout.size) {
                            return false;
                        }
                    }
                    pending.push(child);
                }
                VALUE
                    if matches!(
                        op[1],
                        VALUE_BITS | VALUE_BOOL | VALUE_FIELDS | VALUE_VARIANT
                    ) && op[2] == layout.size => {}
                _ => return false,
            }
        }
    }
    true
}

impl Native {
    pub(crate) fn cpu_owner(&self) -> Option<crate::pool::cpu_scoped::ledger::blocks::Owner> {
        let claim = self.cpu_claim.as_ref()?;
        if !claim.active() {
            return None;
        }
        let key = claim.key();
        Some(crate::pool::cpu_scoped::ledger::blocks::Owner {
            task: (u64::from(key.generation) << 32) | u64::from(key.slot),
            ordinal: claim.ordinal() as u64,
        })
    }

    pub(crate) fn new(
        ctx: *mut TaskCtx,
        environment: *mut Header,
        operation_site: u64,
        certificate: Certificate,
    ) -> Box<Self> {
        let mut resume = certificate.resume.map(|(dispatcher, bytes)| {
            (
                dispatcher,
                crate::continuation::OwnedResumeFrame::new(bytes),
            )
        });
        let frame = resume
            .as_mut()
            .map_or(std::ptr::null_mut(), |(_, frame)| frame.as_mut_ptr());
        let input_blocks = if certificate.cpu {
            // SAFETY: the checked capture is initialized and still exclusively
            // owned by its launcher; mapper execution has not begun.
            unsafe { super::cpu_roots::inputs(ctx, [environment, certificate.source]) }
        } else {
            Vec::new()
        };
        Box::new(Self {
            physical: Physical {
                points_left: if certificate.host.is_null()
                    && (!certificate.cpu || certificate.preclaimed)
                {
                    1024
                } else {
                    i64::MAX
                },
                saved_sp: std::ptr::null_mut(),
                idle_sp: std::ptr::null_mut(),
                reason: 0,
                reserved_zero: 0,
            },
            entry: certificate.entry,
            environment,
            operation_site,
            status: 0,
            pending_refill: false,
            ctx,
            live: certificate.live,
            original_mask: certificate.mask,
            source: certificate.source,
            host: super::host_return::State::new(certificate.host),
            admitted: None,
            frame,
            resume,
            continued: false,
            cpu: certificate.cpu,
            cpu_phase: if certificate.preclaimed { 2 } else { 0 },
            allocation: super::cpu_allocation::Request::new(),
            summary: std::ptr::null_mut(),
            summary_status: 0,
            summary_blocks: Vec::new(),
            growth: std::ptr::null_mut(),
            growth_status: 0,
            growth_plan: None,
            cpu_job: None,
            saved_roots: None,
            input_blocks,
            arena: super::cpu_arena::Arena::new(),
            cpu_claim: None,
            cpu_fuel: super::cpu_fuel::Fuel::default(),
            restart: super::cpu_restart::State::new(certificate.preclaimed),
        })
    }
    /// A v4 body saved at a boundary: its exact saved frame and the ordinary
    /// dispatcher that continues it, taken once.
    pub(crate) fn take_resume(
        &mut self,
    ) -> (
        crate::continuation::ResumeDispatcher,
        crate::continuation::OwnedResumeFrame,
    ) {
        let (dispatcher, frame) = self
            .resume
            .take()
            .unwrap_or_else(|| crate::fault("an ahead save without its resumable frame"));
        self.frame = std::ptr::null_mut();
        self.continued = true;
        (dispatcher, frame)
    }
    /// The logical owner now drives this physical record without private quanta.
    pub(super) fn adopt(&mut self) {
        self.physical.points_left = i64::MAX;
    }
    /// Withdrawal only (`Local::restore`): the original live mask returns to
    /// the environment, whose ordinary drop then releases the source capture.
    pub(super) unsafe fn restore_mask(&mut self) {
        unsafe { self.live.write(self.original_mask) };
    }
}

/// # Safety
/// Only compiler-certified x86 SysV alternates call this fixed-load leaf.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ahead_state(_ctx: *mut TaskCtx) -> *mut Physical {
    core::arch::naked_asm!("mov rax, [rdi + {offset}]","ret",
        offset=const std::mem::offset_of!(TaskCtx,ahead_physical));
}
/// Exactly 64 bytes: return address, six callee saves, eight control bytes.
/// # Safety
/// The exclusive running owner initialized state and its idle continuation.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ahead_handoff(_state: *mut Physical, _reason: u32) {
    core::arch::naked_asm!("mov [rdi + 24], esi","mov rsi, [rdi + 16]","add rdi, 8",
        "jmp {switch}",switch=sym crate::fiber::fiber_switch);
}
/// fiber_start's call plus the one saved register: 16 bytes of startup extent.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn start(_ctx: *mut u8, _native: *mut u8) -> ! {
    // A v4 entry also takes its frame; a suspended one continues in its
    // ordinary dispatcher instead of completing.
    core::arch::naked_asm!(
        "push rbx","mov rbx, rsi","mov rsi, [rbx + 40]",
        "mov rdx, [rbx + 48]","mov rcx, rdx","mov r8, [rbx + {frame}]","call [rbx + 32]",
        "mov [rbx + 56], al","cmp al, {suspended}","je 2f",
        "mov rdi, rbx","mov esi, 4","call {handoff}",
        "mov rdi, [rbx + 64]","movzx esi, byte ptr [rbx + 56]","pop rbx","jmp {complete}",
        "2:","mov rdi, [rbx + 64]","mov rsi, rbx","pop rbx","jmp {suspended_entry}",
        frame=const std::mem::offset_of!(Native, frame),
        suspended=const crate::STATUS_SUSPENDED,
        handoff=sym hirsel_rt_v2_ahead_handoff,complete=sym super::super::super::ahead_complete,
        suspended_entry=sym super::super::super::ahead_suspended,
    );
}

/// Whether the running task's admitted ahead record runs a v4 resumable body:
/// its ahead code saves where an ordinary frame would, and once it handed the
/// saved frame to the ordinary dispatcher that frame leaves its stack as any
/// compact frame.
pub(crate) fn resumable(ctx: *mut TaskCtx) -> bool {
    let p = unsafe { (*ctx).ahead_physical }.cast::<Native>();
    !p.is_null() && unsafe { (*p).continued || (*p).resume.is_some() } && !super::private(ctx)
}

/// The dispatcher owns Running and its permit for this entire call. No
/// physical save is published until the normal idle callback acknowledges it.
/// # Safety
/// Idle only, with the installed exclusive image and no role borrow retained.
pub(crate) unsafe fn run(ctx: *mut TaskCtx, exec: &crate::pool::Exec, mut sp: *mut u8) {
    unsafe {
        let native = (*ctx)
            .ahead
            .as_mut()
            .unwrap()
            .native
            .as_mut()
            .unwrap()
            .as_mut() as *mut Native;
        let mut returned = false;
        loop {
            crate::stop::set_running(std::ptr::null_mut());
            super::super::super::role::enter(ctx);
            if let Some(claim) = &(*native).cpu_claim {
                assert!(
                    claim.lease().frame_bound(),
                    "CPU adoption retains its actual saved-frame allocation"
                );
            }
            if let Some(saved) = (*native).saved_roots.take() {
                let owner = (*native)
                    .cpu_owner()
                    .expect("a saved CPU call retains its ordinal");
                saved.check(ctx, &*native, owner);
            }
            let run = (*ctx).run_ptr();
            super::cpu_restart::prepare(ctx, native, sp);
            let revoked = super::cpu_restart::must_revoke(ctx, native);
            if revoked {
                super::cpu_restart::rollback(ctx, native);
            }
            let arena_ready = if revoked || (*run).control.stopped().is_some() {
                false
            } else {
                match super::cpu_arena::flush(ctx, native) {
                    Ok(ready) => ready,
                    Err(stop) => {
                        (*run).control.request_inside(stop);
                        false
                    }
                }
            };
            if arena_ready && (*native).physical.reason & (4 | 16) != 0 {
                super::cpu_arena::canonical(ctx, native);
            }
            super::host_return::flush_logical(ctx, native);
            let mut pause = !arena_ready || (*run).control.stopped().is_some();
            if !private(ctx) {
                pause |= (*ctx)
                    .ahead
                    .as_ref()
                    .unwrap()
                    .save_requested
                    .load(Ordering::Acquire);
            }
            if !returned && !pause && (*native).pending_refill && !private(ctx) {
                match super::super::engine::ahead_refill(ctx) {
                    Ok(ready) => {
                        pause = !ready;
                        (*native).pending_refill = !ready;
                    }
                    Err(stop) => {
                        (*run).control.request_inside(stop);
                        pause = true;
                    }
                }
            }
            if !pause
                && ((*native).host.active() || (*native).cpu_phase == 2)
                && (*native).physical.reason & 4 != 0
            {
                match super::super::engine::host_segment_barrier(ctx) {
                    Ok(true) => {
                        if (*native).cpu_phase == 2 {
                            super::cpu_worker::admit(native);
                        } else {
                            super::host_return::admit(ctx, native);
                        }
                    }
                    Ok(false) => pause = true,
                    Err(stop) => {
                        (*run).control.request_inside(stop);
                        pause = true;
                    }
                }
            }
            if returned && !pause {
                let reason = (*native).physical.reason;
                if private(ctx) {
                    if reason & 2 != 0 {
                        fuel_out(ctx);
                        let local = (*ctx).ahead.as_ref().unwrap();
                        if local.fuel.get() > (*ctx).run_ref().fuel.remaining() {
                            pause = true;
                            (*native).pending_refill = true;
                        } else {
                            fuel_in(ctx);
                        }
                    }
                    let local = (*ctx).ahead.as_ref().unwrap();
                    pause |= reason & 4 != 0 || local.save_requested.load(Ordering::Acquire);
                    (*native).physical.points_left = 1024;
                } else {
                    if reason & 2 != 0 && !(*native).pending_refill {
                        match super::super::engine::ahead_refill(ctx) {
                            Ok(ready) => {
                                pause = !ready;
                                (*native).pending_refill = !ready;
                            }
                            Err(stop) => {
                                (*run).control.request_inside(stop);
                                pause = true;
                            }
                        }
                    }
                    (*native).physical.points_left =
                        if (*native).host.active() || (*native).cpu_phase == 2 {
                            1024
                        } else {
                            i64::MAX
                        };
                }
            }
            if !pause && (*native).growth.is_null() && (*native).allocation.pending() {
                match super::cpu_allocation::serve(ctx, native) {
                    Ok(ready) => pause = !ready,
                    Err(stop) => {
                        (*run).control.request_inside(stop);
                        pause = true;
                    }
                }
            }
            if !pause && !(*native).summary.is_null() {
                match super::cpu_handoff::serve(ctx, native) {
                    Ok(ready) => pause = !ready,
                    Err(stop) => {
                        (*run).control.request_inside(stop);
                        pause = true;
                    }
                }
            }
            if !pause && !(*native).growth.is_null() {
                match super::cpu_handoff::growth::serve(ctx, native) {
                    Ok(ready) => pause = !ready,
                    Err(stop) => {
                        (*run).control.request_inside(stop);
                        pause = true;
                    }
                }
            }
            if pause {
                super::cpu_restart::rollback(ctx, native);
                if !private(ctx) && (*run).control.stopped().is_none() {
                    let task = super::super::engine::current(ctx);
                    super::super::engine::with_driver(ctx, |d| d.suspending(task, true));
                }
                let owner = super::super::super::dispatch_owner(ctx);
                (*ctx).switch_out();
                assert!(
                    exec.leaving
                        .replace(Some(crate::pool::free::Leave::Suspend {
                            owner,
                            yielded: false
                        }))
                        .is_none()
                );
                super::super::super::role::release(ctx);
                return;
            }
            (*native).physical.reason = 0;
            super::super::super::role::release(ctx);
            crate::stop::set_running(ctx);
            // Save exactly this idle continuation. Normal post-admission runtime
            // suspensions read the same pointer through pool::idle_of.
            crate::pool::cpu_scoped::probe::resume(ctx);
            loop {
                #[cfg(all(feature = "tsan", target_arch = "x86_64", target_os = "linux"))]
                crate::tsan_callbacks::bind(&raw mut (*native).physical.idle_sp);
                crate::fiber::switch(&raw mut (*native).physical.idle_sp, sp);
                #[cfg(all(feature = "tsan", target_arch = "x86_64", target_os = "linux"))]
                crate::tsan_callbacks::clear();
                // A scalar private or CPU quantum owns no shared state. Keep the same
                // 1024-point checkpoint and every stop/save/fuel/barrier test,
                // without taking the scheduler role for a quota-only return.
                if (*native).cpu
                    && (*native).cpu_phase == 2
                    && !private(ctx)
                    && (*native).physical.reason == 2
                    && (*run).control.stopped().is_none()
                    && !(*ctx)
                        .ahead
                        .as_ref()
                        .unwrap()
                        .save_requested
                        .load(Ordering::Acquire)
                    && !(*native).pending_refill
                    && !(*native).allocation.pending()
                    && (*native).summary.is_null()
                    && (*native).growth.is_null()
                    && exec.leaving.get().is_none()
                    && let Some(grant) = (*native)
                        .cpu_fuel
                        .refill((*ctx).view.compiled.fuel, crate::budget::FUEL_CHUNK)
                {
                    (*ctx)
                        .ordered_charge_trace
                        .capture((*ctx).view.compiled.fuel);
                    (*ctx).heap().set_fuel(grant as i64);
                    (*ctx).ordered_charge_trace.grant(grant as i64);
                    (*native).physical.points_left = 1024;
                    (*native).physical.reason = 0;
                    sp = (*native).physical.saved_sp;
                    #[cfg(all(feature = "tsan", target_arch = "x86_64"))]
                    crate::pool::free::double_claim::mapper(ctx, sp);
                    continue;
                }
                if (((*native).cpu && (*native).cpu_phase == 2 && !private(ctx))
                    || (!(*native).cpu && private(ctx) && (*native).host.root.is_null()))
                    && (*native).physical.reason == 1
                    && (*run).control.stopped().is_none()
                    && !(*ctx)
                        .ahead
                        .as_ref()
                        .unwrap()
                        .save_requested
                        .load(Ordering::Acquire)
                    && !(*native).pending_refill
                    && !(*native).allocation.pending()
                    && (*native).summary.is_null()
                    && (*native).growth.is_null()
                    && exec.leaving.get().is_none()
                {
                    (*native).physical.points_left = 1024;
                    (*native).physical.reason = 0;
                    sp = (*native).physical.saved_sp;
                    #[cfg(all(feature = "tsan", target_arch = "x86_64"))]
                    crate::pool::free::double_claim::mapper(ctx, sp);
                    continue;
                }
                break;
            }
            if (*native).cpu && (*native).physical.reason & 4 != 0 {
                crate::pool::cpu_scoped::probe::boundary(ctx, (*native).physical.reason);
            }
            crate::pool::cpu_scoped::probe::checkpoint(ctx);
            exec.idle.set((*native).physical.idle_sp);
            crate::stop::set_running(std::ptr::null_mut());
            if exec.leaving.get().is_some() {
                return;
            }
            (*ctx).task_sp = (*native).physical.saved_sp;
            sp = (*ctx).task_sp;
            returned = true;
        }
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub(crate) unsafe extern "C" fn start(_ctx: *mut u8, _native: *mut u8) -> ! {
    crate::fault("ahead entry on an unsupported architecture")
}

#[cfg(test)]
pub(crate) mod tests;
