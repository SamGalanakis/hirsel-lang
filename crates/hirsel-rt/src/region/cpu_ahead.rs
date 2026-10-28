//! Idle-side regional allocation retains the ordinary arena's segment rules.
use super::*;

pub(crate) struct Plan {
    pub(crate) region: *mut Region,
    index: u32,
    extent: u64,
    align: u64,
    flags: u32,
    need: Option<u64>,
    schedule: bool,
}

impl Plan {
    pub(crate) fn charge(&self) -> u64 {
        self.need.map_or(0, canonical_size)
    }

    /// The exact saved mapper still owns this active region. Nothing mutates
    /// the region or pool until backing is admitted; refusal leaves it intact.
    pub(crate) unsafe fn allocate(
        &self,
        ctx: *mut TaskCtx,
    ) -> Result<*mut Header, crate::pool::cpu_scoped::ledger::BackingFull> {
        unsafe {
            if let Some(need) = self.need {
                (*ctx).pool().reserve_cpu_backing(need)?;
            }
            Ok(arena_alloc(
                ctx,
                self.region,
                self.index,
                self.extent,
                self.align,
                self.flags,
                true,
                self.schedule,
            ))
        }
    }
}

/// Only an active region in the saved task's bounded region chain is accepted.
/// This checks addresses before reading the caller-supplied region pointer.
pub(crate) unsafe fn plan(
    ctx: *mut TaskCtx,
    region: *mut Region,
    index: u32,
    extent: u64,
    align: u64,
    flags: u32,
    schedule: bool,
) -> Option<Plan> {
    unsafe {
        if region.is_null()
            || index >= (*ctx).heap_ref().layout_count()
            || extent > crate::alloc::EXTENT_MAX
            || align == 0
            || !align.is_power_of_two()
            || align > 1 << 30
            || flags & !ALLOC_TRACKED != 0
        {
            return None;
        }
        let mut active = (*ctx).region;
        let mut found = false;
        for _ in 0..crate::budget::stretch::ENVELOPE_MAX {
            if active.is_null() {
                break;
            }
            if active == region {
                found = true;
                break;
            }
            active = (*active).parent;
        }
        if !found || (*region).state != REGION_ACTIVE {
            return None;
        }
        let layout = &*(*ctx).descriptor(index);
        let bare = layout.flags & LAYOUT_NO_EXTENT != 0;
        if layout.size > extent || (bare && (extent != layout.size || align != layout.align)) {
            return None;
        }
        let physical = align.max(8);
        let prefix = if flags & ALLOC_TRACKED != 0 {
            TRACKED_PREFIX_BYTES
        } else if bare {
            8
        } else {
            24
        };
        let reserved = prefix.checked_add(extent)?.checked_add(physical - 8)?;
        let schedule = schedule && layout.step_count != 0;
        if reserved > crate::alloc::EXTENT_MAX {
            return None;
        }
        let (need, remaining) = match reservation(&*region, reserved, 8, true) {
            Ok(start) => (
                None,
                (segment_end(&*region).addr() - start.addr()) as u64 - reserved,
            ),
            Err(need) => (
                Some(need),
                canonical_size(need) - SEGMENT_HEADER as u64 - reserved,
            ),
        };
        if need.is_some_and(|need| need > crate::alloc::EXTENT_MAX) {
            return None;
        }
        // Entry's size and the segment end are word aligned, so this is also
        // the fit test after aligning the post-block cursor for record().
        let need = if schedule && remaining < size_of::<Entry>() as u64 {
            // Regrowth of an existing segment leaves geometric slack for its
            // cleanup. Only a block that fits the old segment can need a new
            // standard segment for record(), so one preflight still suffices.
            if need.is_some() {
                return None;
            }
            Some(SEGMENT_HEADER as u64 + size_of::<Entry>() as u64 + align_of::<Entry>() as u64 - 1)
        } else {
            need
        };
        Some(Plan {
            region,
            index,
            extent,
            align,
            flags,
            need,
            schedule,
        })
    }
}

/// A compiler-local reservation has no Header and no counted generation.
/// Its active region and the saved frame retain the raw storage.
pub(crate) struct RawPlan {
    region: *mut Region,
    extent: u64,
    align: u64,
    need: Option<u64>,
}

impl RawPlan {
    pub(crate) fn charge(&self) -> u64 {
        self.need.map_or(0, canonical_size)
    }

    /// # Safety
    /// The checked saved frame still owns the region accepted by raw_plan.
    pub(crate) unsafe fn allocate(
        &self,
        ctx: *mut TaskCtx,
    ) -> Result<*mut u8, crate::pool::cpu_scoped::ledger::BackingFull> {
        // SAFETY: the idle service prevents resumption and owns this region.
        unsafe {
            if let Some(need) = self.need {
                (*ctx).pool().reserve_cpu_backing(need)?;
            }
            Ok(reserve_in_mode(
                &mut *ctx,
                self.region,
                self.extent,
                self.align,
                false,
                true,
            ))
        }
    }
}

/// # Safety
/// ctx is the saving task's live context; the region address is untrusted.
pub(crate) unsafe fn raw_plan(
    ctx: *mut TaskCtx,
    region: *mut Region,
    extent: u64,
    align: u64,
) -> Option<RawPlan> {
    if region.is_null()
        || extent > crate::alloc::EXTENT_MAX
        || align == 0
        || !align.is_power_of_two()
        || align > 1 << 30
    {
        return None;
    }
    // SAFETY: only the trusted context chain is read until address membership
    // is proved. The bound is the existing regional custody cap.
    unsafe {
        let mut active = (*ctx).region;
        let mut found = false;
        for _ in 0..crate::budget::stretch::ENVELOPE_MAX {
            if active.is_null() {
                break;
            }
            if active == region {
                found = true;
                break;
            }
            active = (*active).parent;
        }
        if !found || (*region).state != REGION_ACTIVE {
            return None;
        }
        let need = reservation(&*region, extent, align, false).err();
        if need.is_some_and(|need| need > crate::alloc::EXTENT_MAX) {
            return None;
        }
        Some(RawPlan {
            region,
            extent,
            align,
            need,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    fn growth_charge(saved: bool, extent: u64, diagnostic: bool) -> u64 {
        let observed = Arc::new(AtomicU64::new(0));
        let output = observed.clone();
        let mut run = crate::host::BodyRun::new(1, false);
        if diagnostic {
            run.config.flags |= crate::context::CONFIG_DIAGNOSTIC;
        }
        let outcome = run.run(
            |ctx| unsafe { (*ctx).set_budgets(None, Some(2 << 20)) },
            move |ctx| unsafe {
                let mut region = std::mem::MaybeUninit::<Region>::uninit();
                let region = region.as_mut_ptr();
                hirsel_rt_v2_region_enter(ctx, region);
                let index = crate::text::bytes_list_index(ctx);
                let old = hirsel_rt_v2_arena_alloc(ctx, region, index, 64, 64, ALLOC_TRACKED);
                ptr::write_bytes(crate::alloc::payload(old), 0, 64);
                hirsel_rt_v2_region_schedule(ctx, region, index, crate::alloc::payload(old));
                let before = (*ctx).pool().active_bytes();
                if saved {
                    let plan = plan(ctx, region, index, extent, 64, ALLOC_TRACKED, true).unwrap();
                    let fresh = plan.allocate(ctx).unwrap();
                    ptr::copy_nonoverlapping(
                        crate::alloc::payload(old),
                        crate::alloc::payload(fresh),
                        64,
                    );
                    assert_eq!((*ctx).pool().active_bytes() - before, plan.charge());
                } else {
                    crate::alloc::hirsel_rt_v2_block_grow(ctx, old, 64, extent, 64);
                }
                output.store((*ctx).pool().active_bytes() - before, Ordering::Relaxed);
                assert_eq!(hirsel_rt_v2_region_exit(ctx, region, 0, 0), 0);
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        observed.load(Ordering::Relaxed)
    }

    #[test]
    fn saved_tracked_growth_keeps_canonical_block_and_cleanup_segment_charges() {
        for diagnostic in [false, true] {
            for extent in [256, 65184, 65536] {
                assert_eq!(
                    growth_charge(true, extent, diagnostic),
                    growth_charge(false, extent, diagnostic),
                    "canonical regrow parity at extent {extent}, diagnostic {diagnostic}"
                );
            }
        }
    }

    fn raw_charge(saved: bool, extent: u64, diagnostic: bool) -> u64 {
        let observed = Arc::new(AtomicU64::new(0));
        let output = observed.clone();
        let mut run = crate::host::BodyRun::new(1, false);
        if diagnostic {
            run.config.flags |= crate::context::CONFIG_DIAGNOSTIC;
        }
        let outcome = run.run(
            |ctx| unsafe { (*ctx).set_budgets(None, Some(2 << 20)) },
            move |ctx| unsafe {
                let mut storage = std::mem::MaybeUninit::<Region>::uninit();
                let region = storage.as_mut_ptr();
                hirsel_rt_v2_region_enter(ctx, region);
                let mut first = ptr::null_mut();
                assert_eq!(
                    hirsel_rt_v2_region_reserve(ctx, region, 64, 64, &mut first),
                    0
                );
                assert!(raw_plan(ctx, ptr::dangling_mut(), extent, 64).is_none());
                assert!(raw_plan(ctx, region, extent, 3).is_none());
                let before = (*ctx).pool().active_bytes();
                let fresh = if saved {
                    let plan = raw_plan(ctx, region, extent, 64).unwrap();
                    let fresh = plan.allocate(ctx).unwrap();
                    assert_eq!((*ctx).pool().active_bytes() - before, plan.charge());
                    fresh
                } else {
                    let mut fresh = ptr::null_mut();
                    assert_eq!(
                        hirsel_rt_v2_region_reserve(ctx, region, extent, 64, &mut fresh),
                        0
                    );
                    fresh
                };
                ptr::write_bytes(fresh, 0x5a, extent as usize);
                assert_eq!(fresh.addr() % 64, 0);
                assert_eq!(*fresh.add(extent as usize - 1), 0x5a);
                output.store((*ctx).pool().active_bytes() - before, Ordering::Relaxed);
                assert_eq!(hirsel_rt_v2_region_exit(ctx, region, 0, 0), 0);
                assert!(raw_plan(ctx, region, extent, 64).is_none());
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        observed.load(Ordering::Relaxed)
    }

    #[test]
    fn saved_raw_regions_keep_ordinary_tail_and_segment_charges() {
        for diagnostic in [false, true] {
            for extent in [64, 65536] {
                assert_eq!(
                    raw_charge(true, extent, diagnostic),
                    raw_charge(false, extent, diagnostic)
                );
            }
        }
    }
}
