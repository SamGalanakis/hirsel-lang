//! Idle-side regional allocation retains the ordinary arena's segment rules.
use super::*;

pub(crate) struct Plan {
    region: *mut Region,
    index: u32,
    extent: u64,
    align: u64,
    flags: u32,
    need: Option<u64>,
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
        if reserved > crate::alloc::EXTENT_MAX {
            return None;
        }
        let need = reservation(&*region, reserved, 8, true).err();
        if need.is_some_and(|need| need > crate::alloc::EXTENT_MAX) {
            return None;
        }
        Some(Plan {
            region,
            index,
            extent,
            align,
            flags,
            need,
        })
    }
}
