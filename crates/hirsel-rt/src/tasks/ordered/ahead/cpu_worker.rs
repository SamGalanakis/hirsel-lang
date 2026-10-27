//! A CPU worker enters ahead work only after its canonical job claim. The
//! Shared mapper and job list retain their ordinary owners throughout.
use super::{SpawnAheadInfo, native};
use crate::{Header, TaskCtx};

#[repr(C)]
pub struct Info {
    pub prefix: native::Info,
    pub shared_descriptor: u32,
    pub shared_payload_descriptor: u32,
    pub jobs_descriptor: u32,
    pub jobs_offset: u32,
}

#[repr(C)]
pub struct PreclaimedInfo {
    pub prefix: Info,
    pub width_offset: u32,
    pub ordinal_offset: u32,
}

pub(super) unsafe fn validate_preclaimed(
    ctx: *mut TaskCtx,
    env: *mut Header,
    info: *const PreclaimedInfo,
) -> Option<native::Certificate> {
    unsafe {
        let row = &*info;
        let mut certificate = validate(ctx, env, &row.prefix)?;
        let recipe = row.prefix.prefix.recipe.as_ref()?;
        for (offset, size) in [(row.width_offset, 4), (row.ordinal_offset, 8)] {
            if !offset.is_multiple_of(size) || offset.checked_add(size)? > recipe.wrapper_extent {
                return None;
            }
        }
        let payload = crate::alloc::payload(env);
        let width = payload.add(row.width_offset as usize).cast::<u32>().read();
        let ordinal = payload
            .add(row.ordinal_offset as usize)
            .cast::<u64>()
            .read();
        let parent = super::super::engine::current(ctx);
        if !super::super::engine::with_driver(ctx, |d| {
            d.cpu_batch
                .as_ref()
                .is_some_and(|batch| batch.pending(parent, width, ordinal))
        }) {
            return None;
        }
        certificate.preclaimed = true;
        Some(certificate)
    }
}

pub(super) unsafe fn validate(
    ctx: *mut TaskCtx,
    env: *mut Header,
    info: *const Info,
) -> Option<native::Certificate> {
    unsafe {
        let info = &*info;
        let recipe = info.prefix.recipe.as_ref()?;
        let entry = info.prefix.prefix.entry?;
        let count = (*ctx).heap_ref().layout_count();
        if env.is_null()
            || (*env).count != crate::count::COUNT_UNIQUE
            || (*env).index() != recipe.wrapper_descriptor
            || !(80..=65536).contains(&info.prefix.stack_bytes)
            || recipe.wrapper_extent > 256
            || recipe.source_extent > 256
            || recipe.live_mask != 1
            || [
                recipe.wrapper_descriptor,
                recipe.source_descriptor,
                info.shared_descriptor,
                info.shared_payload_descriptor,
                info.jobs_descriptor,
            ]
            .iter()
            .any(|index| *index >= count)
            || ![
                recipe.callable_offset,
                recipe.live_word_offset,
                info.jobs_offset,
            ]
            .into_iter()
            .all(|offset| {
                offset.is_multiple_of(8)
                    && offset
                        .checked_add(8)
                        .is_some_and(|end| end <= recipe.wrapper_extent)
            })
            || recipe.callable_offset == recipe.live_word_offset
            || recipe.callable_offset == info.jobs_offset
            || info.jobs_offset == recipe.live_word_offset
        {
            return None;
        }
        let wrapper = &*(*ctx).descriptor(recipe.wrapper_descriptor);
        if wrapper.size != u64::from(recipe.wrapper_extent)
            || wrapper.align > 16
            || wrapper.finalizer.is_some()
        {
            return None;
        }
        let payload = crate::alloc::payload(env);
        let live = payload.add(recipe.live_word_offset as usize).cast::<u64>();
        if live.read() != 1 {
            return None;
        }
        let shared = payload
            .add(recipe.callable_offset as usize)
            .cast::<*mut Header>()
            .read();
        if shared.is_null()
            || (*shared).index() != info.shared_payload_descriptor
            || crate::region::is_tracked(shared)
        {
            return None;
        }
        if info.prefix.prefix.version == 5 {
            let jobs = payload
                .add(info.jobs_offset as usize)
                .cast::<*mut Header>()
                .read();
            if jobs.is_null()
                || (*jobs).index() != info.jobs_descriptor
                || !matches!(
                    (*jobs).count,
                    crate::count::COUNT_UNIQUE | crate::count::COUNT_IMMORTAL
                )
                || crate::region::is_tracked(jobs)
            {
                return None;
            }
        } else if !native::inert_source(ctx, info.jobs_descriptor) {
            return None;
        }
        let frozen = &*(*ctx).descriptor(info.shared_payload_descriptor);
        if frozen.size != 16 || frozen.align > 8 || frozen.flags & crate::LAYOUT_FROZEN == 0 {
            return None;
        }
        let callable = crate::alloc::payload(shared).cast::<usize>();
        if recipe.source_code.is_null() || callable.read() != recipe.source_code as usize {
            return None;
        }
        let source = callable.add(1).read() as *mut Header;
        if source.is_null() || (*source).index() != recipe.source_descriptor {
            return None;
        }
        let layout = &*(*ctx).descriptor(recipe.source_descriptor);
        if layout.size != u64::from(recipe.source_extent)
            || layout.align > 16
            || layout.finalizer.is_some()
            || layout.step_count != 0
            || !native::inert_source(ctx, recipe.source_descriptor)
        {
            return None;
        }
        Some(native::Certificate {
            entry,
            stack_bytes: info.prefix.stack_bytes,
            host: std::ptr::null(),
            resume: None,
            cpu: true,
            preclaimed: false,
            live,
            mask: 1,
            source,
        })
    }
}

pub(crate) fn armed(ctx: *mut TaskCtx) -> bool {
    let native = unsafe { (*ctx).ahead_physical }.cast::<native::Native>();
    !native.is_null() && unsafe { (*native).cpu && (*native).cpu_phase == 1 }
}

pub(super) unsafe fn admit(native: *mut native::Native) {
    unsafe {
        assert_eq!((*native).cpu_phase, 2);
        (*native).admitted = Some(crate::continuation::PrivateSave {
            pc: crate::continuation::SavedPc::Fiber(crate::continuation::SavedFiber {
                sp: (*native).physical.saved_sp,
            }),
            generation: 0,
            fuel: 0,
            descriptor: None,
        });
        (*native).cpu_phase = 0;
    }
}

/// # Safety
/// The current compiled worker has the checked CPU certificate and is about
/// to issue its canonical claim, before invoking its immutable mapper.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_ahead_arm(ctx: *mut TaskCtx) {
    let native = unsafe { (*ctx).ahead_physical }.cast::<native::Native>();
    if !native.is_null() && unsafe { (*native).cpu } {
        assert_eq!(unsafe { (*native).cpu_phase }, 0);
        unsafe {
            (*native).cpu_phase = 1;
        }
    }
}

/// # Safety
/// Only the certified worker calls this fixed leaf immediately after its
/// canonical claim. The ordinary claim result branch still gates the mapper.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_ahead_enter(_state: *mut native::Physical) {
    core::arch::naked_asm!(
        "cmp byte ptr [rdi + {cpu}], 0", "je 2f",
        "cmp dword ptr [rdi + {phase}], 1", "jne 2f",
        "mov dword ptr [rdi + {phase}], 2", "mov qword ptr [rdi], 1024", "2:", "ret",
        cpu = const std::mem::offset_of!(native::Native, cpu),
        phase = const std::mem::offset_of!(native::Native, cpu_phase),
    );
}

const _: () = {
    assert!(std::mem::size_of::<Info>() == 48);
    assert!(std::mem::size_of::<PreclaimedInfo>() == 56);
    assert!(std::mem::offset_of!(Info, prefix) == 0);
    assert!(std::mem::offset_of!(Info, shared_descriptor) == 32);
    assert!(std::mem::size_of::<SpawnAheadInfo>() == 16);
};
