//! Snapshot the existing guard's actual outgoing credits before a seal patches
//! any arena cell. A snapshot acquires nothing. The complete typed transaction
//! consumes this list only at commit; abort leaves the guard unchanged.
use super::{Entry, Region, check_active};
use crate::alloc::Header;
use crate::{TaskCtx, fault};
use std::ptr;

/// One actual source holder, distinct from semantic occurrence multiplicity.
pub(crate) struct Credits(Vec<*mut Header>);

impl Credits {
    pub(crate) fn new() -> Self {
        Self(Vec::new())
    }

    /// Accept actual source holder credits without duplicating them. The
    /// caller transfers custody only after its source operation commits.
    ///
    /// # Safety
    /// Every nonnull holder is live, resource-free and owned once by this
    /// batch, with multiplicity preserved. No other cleanup owns these credits.
    pub(crate) unsafe fn from_owned(holders: Vec<*mut Header>) -> Self {
        Self(holders)
    }

    /// # Safety
    /// `value` is a live, resource-free inline value of the named descriptor.
    pub(crate) unsafe fn value(&mut self, ctx: &TaskCtx, index: u32, value: *mut u8) {
        unsafe {
            let layout = ctx.descriptor(index);
            if (*layout).finalizer.is_some() {
                fault("seal cleanup contains a resource");
            }
            // holders also refuses nested finalizers, handles, and thaw steps.
            crate::copy::holders(layout, value, &mut |slot| {
                self.0.push(slot.cast::<*mut Header>().read_unaligned());
            });
        }
    }

    /// # Safety
    /// The active region and every scheduled value are live and exclusively
    /// owned by this entered transaction, whose complete graph has no resources.
    pub(crate) unsafe fn arena(&mut self, ctx: &TaskCtx, region: *mut Region) {
        unsafe {
            check_active(region);
            let mut entry = (*region).cleanup.cast::<Entry>();
            while !entry.is_null() {
                if (*entry).block.is_null() {
                    self.value(ctx, (*entry).cleanup.index, (*entry).cleanup.value);
                } else {
                    self.0.push((*entry).block);
                }
                entry = (*entry).prev;
            }
        }
    }

    /// Transfer guard custody to the private source-retirement snapshot. Its
    /// release no longer reads old cells, which publication may now patch.
    ///
    /// # Safety
    /// The snapshot exactly covers this guard's cleanup obligations.
    pub(crate) unsafe fn disarm(&self, region: *mut Region) {
        unsafe { (*region).cleanup = ptr::null_mut() };
    }

    /// Retire source credits after publication. Destination Shared credits
    /// already exist, so there is no transient sole-root count. Existing root
    /// deferral may reserve bookkeeping; it is outside the mechanical commit.
    ///
    /// # Safety
    /// The snapshot remains exact, each holder is owned once by root/guard,
    /// and all destination storage and Shared acquisitions are complete.
    pub(crate) unsafe fn release(self, ctx: &mut TaskCtx) {
        unsafe {
            for block in self.0 {
                if (*block).domain() == crate::DOMAIN_HEAP && crate::shared::is_root(ctx, block) {
                    crate::roots::hirsel_rt_v2_root_release(ctx, block);
                } else if crate::release::release_block(ctx, block, ptr::null(), 0, 0)
                    != crate::STATUS_OK
                {
                    fault("seal cleanup ran a failing resource destructor");
                }
            }
        }
    }
}
