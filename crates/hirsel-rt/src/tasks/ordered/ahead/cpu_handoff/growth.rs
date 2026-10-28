//! Saved growth uses the existing heap classes, holder walk and arena segments.
use super::*;
use crate::alloc;

pub(in crate::tasks::ordered::ahead) enum Plan {
    RawRegion(crate::region::cpu_ahead::RawPlan),
    Heap(HeapPlan),
    Region {
        allocation: crate::region::cpu_ahead::Plan,
        moved: Option<(*mut Header, u64)>,
    },
}

pub(in crate::tasks::ordered::ahead) struct HeapPlan {
    old: *mut Header,
    keep: u64,
    elements: Option<(u64, u64)>,
    detach: bool,
    copy: bool,
}

/// Compute the same class extent as block_grow/seq_grow without allocating,
/// freeing, or changing a saved input. Shared sequences detach at the
/// requested capacity, as alloc::detach_reserved does.
unsafe fn plan(ctx: *mut TaskCtx, request: &Growth) -> Option<(HeapPlan, u64, u64)> {
    if !matches!(request.kind, GROW_BLOCK | GROW_SEQUENCE | COPY_SEQUENCE)
        || request.block.is_null()
    {
        return None;
    }
    // SAFETY: the checked Growth caller retains its live unique block.
    unsafe {
        let old = request.block;
        if (*old).index() >= (*ctx).heap_ref().layout_count() {
            return None;
        }
        if request.kind == COPY_SEQUENCE {
            let layout = &*(*ctx).descriptor((*old).index());
            if (*old).count == 0
                || (*old).index() != request.index
                || !matches!((*old).domain(), alloc::DOMAIN_HEAP | alloc::DOMAIN_STATIC)
                || layout.finalizer.is_some()
            {
                return None;
            }
            // copy::clone_block uses this exact extent and alignment, then
            // copy_held retains children without consuming the borrowed input.
            let (bytes, align) = (*ctx).heap().usable(old);
            return Some((
                HeapPlan {
                    old,
                    keep: bytes,
                    elements: None,
                    detach: true,
                    copy: true,
                },
                bytes,
                align,
            ));
        }
        let unique =
            (*old).count == crate::count::COUNT_UNIQUE && (*old).domain() == alloc::DOMAIN_HEAP;
        let detach = !unique;
        if detach {
            let layout = &*(*ctx).descriptor((*old).index());
            if request.kind != GROW_SEQUENCE
                || (*old).count == 0
                || ((*old).domain() == alloc::DOMAIN_STATIC
                    && (*old).count != crate::COUNT_IMMORTAL)
                || !matches!((*old).domain(), alloc::DOMAIN_HEAP | alloc::DOMAIN_STATIC)
                || layout.finalizer.is_some()
            {
                return None;
            }
        }
        if request.kind == GROW_BLOCK {
            let bytes = (*ctx).heap_ref().capacity(old);
            if request.keep > bytes || request.keep > request.extent {
                return None;
            }
            return Some((
                HeapPlan {
                    old,
                    keep: request.keep,
                    elements: None,
                    detach: false,
                    copy: false,
                },
                request.extent,
                request.align,
            ));
        }
        let align = request.element_align;
        if align == 0 || !align.is_power_of_two() || align > 1 << 30 {
            return None;
        }
        let offset = alloc::sequence_elements_offset(align);
        let count = alloc::payload(old).cast::<u64>().read();
        // Empty static literals need no extent metadata read.
        let empty_static = count == 0 && (*old).domain() == alloc::DOMAIN_STATIC;
        let bytes = if empty_static {
            offset
        } else {
            (*ctx).heap_ref().capacity(old)
        };
        if bytes < offset {
            return None;
        }
        let keep = count
            .checked_mul(request.element_size)?
            .checked_add(offset)?;
        if keep > bytes {
            return None;
        }
        let slots = if detach {
            0
        } else {
            alloc::sequence_slots((*ctx).heap_ref(), old, offset, request.element_size)
        };
        let extent = if detach {
            request
                .capacity
                .max(count)
                .checked_mul(request.element_size)?
                .checked_add(offset)?
        } else if slots >= request.capacity {
            bytes
        } else {
            let extent = request
                .capacity
                .checked_mul(request.element_size)?
                .checked_add(offset)?;
            let geometric = slots
                .checked_mul(2)
                .and_then(|n| n.checked_mul(request.element_size))
                .and_then(|n| n.checked_add(offset))
                .filter(|&n| n <= alloc::EXTENT_MAX)
                .unwrap_or(extent);
            extent.max(geometric)
        };
        Some((
            HeapPlan {
                old,
                keep: if empty_static { 0 } else { keep },
                elements: Some((offset, request.element_size)),
                detach,
                copy: false,
            },
            extent,
            align,
        ))
    }
}

unsafe fn finish(native: *mut Native, block: *mut Header, status: u8) {
    // SAFETY: idle exclusively owns this saved call and its borrowed request.
    unsafe {
        if status == ACCEPTED {
            (*(*native).growth).block = block;
        }
        (*native).growth_status = u32::from(status);
        (*native).growth = std::ptr::null_mut();
        (*native).growth_plan = None;
    }
}

pub(in crate::tasks::ordered::ahead) unsafe fn serve(
    ctx: *mut TaskCtx,
    native: *mut Native,
) -> crate::stop::Stopped<bool> {
    assert!(crate::stop::running().is_null(), "CPU growth runs on idle");
    assert!(!super::super::private(ctx));
    let pointer = unsafe { (*native).growth };
    if unsafe { pointer.cast::<u32>().read() } != VERSION {
        unsafe { finish(native, std::ptr::null_mut(), NOT_CERTIFIED) };
        return Ok(true);
    }
    if unsafe { (*pointer).kind == RELEASE_SEQUENCE } {
        let block = unsafe { (*pointer).block };
        let task = super::super::super::engine::current(ctx);
        let owned = super::super::super::engine::with_driver(ctx, |driver| {
            driver
                .cpu_batch
                .as_ref()
                .is_some_and(|batch| batch.owns(task, unsafe { (*pointer).ordinal }))
        });
        let valid = owned
            && !block.is_null()
            && unsafe {
                let descriptor = (*block).index();
                descriptor == (*pointer).index
                    && descriptor < (*ctx).heap_ref().layout_count()
                    && (*(*ctx).descriptor(descriptor)).finalizer.is_none()
                    && (*(*ctx).descriptor(descriptor)).step_count == 0
                    && (*(*ctx).descriptor(descriptor)).scan_words == 0
            };
        let released = valid
            && unsafe {
                // The checked descriptor has neither child credits nor a user
                // finalizer. Keep this path structurally separate from the general
                // release walker, which can submit semantic operations.
                let count = (*block).count;
                if count >= crate::count::COUNT_ARENA_SHARED {
                    true
                } else if count == 1 && (*block).domain() == alloc::DOMAIN_HEAP {
                    let capacity = (*ctx).heap_ref().capacity(block);
                    let charge = if (*block).kind() == alloc::KIND_LARGE {
                        crate::budget::large_charge(capacity)
                    } else {
                        crate::budget::small_charge(capacity)
                    };
                    (*ctx).heap().free(block);
                    super::super::super::engine::memory_freed(ctx, charge);
                    true
                } else if count > 1 {
                    (*block).count = count - 1;
                    true
                } else {
                    false
                }
            };
        unsafe {
            finish(
                native,
                block,
                if released { ACCEPTED } else { NOT_CERTIFIED },
            )
        };
        return Ok(true);
    }
    if unsafe { (*native).growth_plan.is_none() && (*pointer).kind == ALLOC_RAW_REGION } {
        let request = unsafe { &*pointer };
        let task = super::super::super::engine::current(ctx);
        let owner = super::super::super::engine::with_driver(ctx, |d| {
            d.cpu_batch
                .as_ref()
                .and_then(|batch| batch.block_owner(task))
        });
        let certified = owner.is_some_and(|owner| {
            owner.ordinal == request.ordinal
                && request.block.is_null()
                && request.flags == 0
                && unsafe { (*ctx).heap_ref().meter.cpu.get() }.is_none_or(|ledger| {
                    ledger
                        .blocks
                        .lock()
                        .unwrap()
                        .region_owned(owner, request.region.addr())
                })
        });
        let plan = certified
            .then(|| unsafe {
                crate::region::cpu_ahead::raw_plan(
                    ctx,
                    request.region,
                    request.extent,
                    request.align,
                )
            })
            .flatten();
        let Some(plan) = plan else {
            unsafe { finish(native, std::ptr::null_mut(), NOT_CERTIFIED) };
            return Ok(true);
        };
        unsafe {
            (*native)
                .allocation
                .begin(request.index, request.extent, request.align);
            (*native).growth_plan = Some(Plan::RawRegion(plan));
        }
    }
    if let Some(Plan::RawRegion(plan)) = unsafe { (*native).growth_plan.as_ref() } {
        if !unsafe { (*native).allocation.admit(ctx, plan.charge())? } {
            return Ok(false);
        }
        if !super::super::cpu_withdrawal::acknowledged(ctx) {
            return Ok(false);
        }
        let Ok(storage) = (unsafe { plan.allocate(ctx) }) else {
            super::super::cpu_withdrawal::request(ctx);
            return Ok(false);
        };
        super::super::cpu_withdrawal::release(ctx);
        // SAFETY: storage is a raw output, never registered or read as Header.
        // Only this success replaces the borrowed record's null output word.
        unsafe {
            (*native).allocation.complete();
            finish(native, storage.cast(), ACCEPTED);
        }
        return Ok(true);
    }
    if unsafe { (*native).growth_plan.is_none() && (*pointer).kind == ALLOC_REGION } {
        let request = unsafe { &*pointer };
        let task = super::super::super::engine::current(ctx);
        let owner = super::super::super::engine::with_driver(ctx, |d| {
            d.cpu_batch
                .as_ref()
                .and_then(|batch| batch.block_owner(task))
        });
        let certified = owner.is_some_and(|owner| {
            owner.ordinal == request.ordinal
                && request.block.is_null()
                && unsafe { (*ctx).heap_ref().meter.cpu.get() }.is_none_or(|ledger| {
                    ledger
                        .blocks
                        .lock()
                        .unwrap()
                        .region_owned(owner, request.region.addr())
                })
        });
        let prepared = certified
            .then(|| unsafe {
                crate::region::cpu_ahead::plan(
                    ctx,
                    request.region,
                    request.index,
                    request.extent,
                    request.align,
                    request.flags,
                    false,
                )
            })
            .flatten();
        let Some(plan) = prepared else {
            unsafe { finish(native, std::ptr::null_mut(), NOT_CERTIFIED) };
            return Ok(true);
        };
        unsafe {
            (*native)
                .allocation
                .begin(request.index, request.extent, request.align);
            (*native).growth_plan = Some(Plan::Region {
                allocation: plan,
                moved: None,
            });
        }
    }
    if unsafe { (*native).growth_plan.is_none() && (*pointer).kind == GROW_BLOCK } {
        let request = unsafe { &*pointer };
        let task = super::super::super::engine::current(ctx);
        let owner = super::super::super::engine::with_driver(ctx, |d| {
            d.cpu_batch
                .as_ref()
                .and_then(|batch| batch.block_owner(task))
        });
        // Check the allocation's task generation and ordinal before its header.
        let certified = owner.is_some_and(|owner| {
            owner.ordinal == request.ordinal
                && !request.block.is_null()
                && unsafe { (*ctx).heap_ref().meter.cpu.get() }.is_none_or(|ledger| {
                    ledger
                        .blocks
                        .lock()
                        .unwrap()
                        .retain(owner, &[request.block])
                        .is_some()
                })
        });
        if certified && unsafe { crate::region::is_tracked(request.block) } {
            let old = request.block;
            let region = unsafe { crate::region::tracked_region(old) };
            let owned = unsafe { (*ctx).heap_ref().meter.cpu.get() }.is_none_or(|ledger| {
                ledger
                    .blocks
                    .lock()
                    .unwrap()
                    .region_owned(owner.unwrap(), region.addr())
            });
            let valid = owned
                && unsafe { (*old).count == crate::COUNT_ARENA_UNIQUE }
                && request.align != 0
                && request.align.is_power_of_two()
                && request.align <= 1 << 30
                && request.extent <= alloc::EXTENT_MAX;
            let prepared = valid
                .then(|| unsafe {
                    crate::region::cpu_ahead::plan(
                        ctx,
                        region,
                        (*old).index(),
                        request.extent,
                        request.align.max(8),
                        crate::region::ALLOC_TRACKED,
                        true,
                    )
                })
                .flatten();
            let Some(plan) = prepared else {
                unsafe { finish(native, std::ptr::null_mut(), NOT_CERTIFIED) };
                return Ok(true);
            };
            let bytes = unsafe { (*ctx).heap_ref().capacity(old) };
            if request.keep > bytes || request.keep > request.extent {
                unsafe { finish(native, std::ptr::null_mut(), NOT_CERTIFIED) };
                return Ok(true);
            }
            if bytes >= request.extent {
                unsafe { finish(native, old, ACCEPTED) };
                return Ok(true);
            }
            unsafe {
                (*native)
                    .allocation
                    .begin((*old).index(), request.extent, request.align.max(8));
                (*native).growth_plan = Some(Plan::Region {
                    allocation: plan,
                    moved: Some((old, request.keep)),
                });
            }
        }
    }
    if let Some(Plan::Region {
        allocation: plan,
        moved,
    }) = unsafe { (*native).growth_plan.as_ref() }
    {
        if !unsafe { (*native).allocation.admit(ctx, plan.charge())? } {
            return Ok(false);
        }
        if !super::super::cpu_withdrawal::acknowledged(ctx) {
            return Ok(false);
        }
        let Ok(fresh) = (unsafe { plan.allocate(ctx) }) else {
            super::super::cpu_withdrawal::request(ctx);
            return Ok(false);
        };
        super::super::cpu_withdrawal::release(ctx);
        unsafe {
            if let Some((old, keep)) = moved {
                std::ptr::copy_nonoverlapping(
                    alloc::payload(*old),
                    alloc::payload(fresh),
                    *keep as usize,
                );
                // The new cleanup owns the moved holders; the abandoned block
                // remains scheduled with an empty sequence until region exit.
                alloc::payload(*old).cast::<u64>().write(0);
            }
            if let Some(ledger) = (*ctx).heap_ref().meter.cpu.get() {
                let task = super::super::super::engine::current(ctx);
                let owner = super::super::super::engine::with_driver(ctx, |d| {
                    d.cpu_batch.as_ref().unwrap().block_owner(task).unwrap()
                });
                ledger
                    .blocks
                    .lock()
                    .unwrap()
                    .adopt_in(owner, fresh, plan.region.addr());
            }
            (*native).allocation.complete();
            finish(native, fresh, ACCEPTED);
        }
        return Ok(true);
    }
    if unsafe { (*native).growth_plan.is_none() } {
        let request = unsafe { &*pointer };
        let task = super::super::super::engine::current(ctx);
        let owner = super::super::super::engine::with_driver(ctx, |d| {
            d.cpu_batch
                .as_ref()
                .and_then(|batch| batch.block_owner(task))
        });
        let certified = owner.is_some_and(|owner| {
            owner.ordinal == request.ordinal
                && unsafe { (*ctx).heap_ref().meter.cpu.get() }.is_none_or(|ledger| {
                    ledger
                        .blocks
                        .lock()
                        .unwrap()
                        .retain(owner, &[request.block])
                        .is_some()
                })
        });
        if request.kind == CHECK_UNTRACKED_SEQUENCE {
            // ArenaPush leaves heap/static blocks unchanged. Tracked arena
            // inputs have no admitted mapper custody and must still refuse.
            let untracked = certified
                && !request.block.is_null()
                && unsafe {
                    let block = request.block;
                    (*block).count != 0
                        && matches!((*block).domain(), alloc::DOMAIN_HEAP | alloc::DOMAIN_STATIC)
                        && (*block).index() == request.index
                        && (*block).index() < (*ctx).heap_ref().layout_count()
                };
            unsafe {
                finish(
                    native,
                    request.block,
                    if untracked { ACCEPTED } else { NOT_CERTIFIED },
                )
            };
            return Ok(true);
        }
        let prepared = certified.then(|| unsafe { plan(ctx, request) }).flatten();
        let Some((plan, extent, align)) = prepared.filter(|(_, extent, align)| {
            *extent <= alloc::EXTENT_MAX
                && *align != 0
                && align.is_power_of_two()
                && *align <= 1 << 30
        }) else {
            unsafe { finish(native, std::ptr::null_mut(), NOT_CERTIFIED) };
            return Ok(true);
        };
        if !plan.detach && unsafe { (*ctx).heap_ref().capacity(plan.old) } >= extent {
            if let Some((offset, size)) = plan.elements {
                let slots =
                    unsafe { alloc::sequence_slots((*ctx).heap_ref(), plan.old, offset, size) };
                unsafe { alloc::set_sequence_capacity(plan.old, slots) };
            }
            unsafe { finish(native, plan.old, ACCEPTED) };
            return Ok(true);
        }
        unsafe {
            (*native)
                .allocation
                .begin((*plan.old).index(), extent, align.max(8));
            (*native).growth_plan = Some(Plan::Heap(plan));
        }
    }
    if unsafe { (*native).allocation.pending() }
        && !unsafe { super::super::cpu_allocation::serve(ctx, native)? }
    {
        return Ok(false);
    }
    // SAFETY: the saved request pins the input until its one new allocation
    // succeeds. No heap reference crosses admission, save, or migration.
    unsafe {
        let fresh = (*native).allocation.take_result();
        let Plan::Heap(plan) = (*native).growth_plan.take().unwrap() else {
            unreachable!()
        };
        std::ptr::copy_nonoverlapping(
            alloc::payload(plan.old),
            alloc::payload(fresh),
            plan.keep as usize,
        );
        if let Some((offset, size)) = plan.elements {
            if plan.keep == 0 {
                alloc::payload(fresh).cast::<u64>().write(0);
            }
            let slots = alloc::sequence_slots((*ctx).heap_ref(), fresh, offset, size);
            alloc::set_sequence_capacity(fresh, slots);
        }
        if plan.detach {
            crate::copy::retain_children(ctx, fresh);
            // The original retains its other credits; immortal/static storage
            // keeps none. A shared sequence cannot be freed by this return.
            if !plan.copy && (*plan.old).count != crate::COUNT_IMMORTAL {
                (*plan.old).count -= 1;
            }
        } else {
            let capacity = (*ctx).heap_ref().capacity(plan.old);
            let charge = if (*plan.old).kind() == alloc::KIND_LARGE {
                crate::budget::large_charge(capacity)
            } else {
                crate::budget::small_charge(capacity)
            };
            (*ctx).heap().free(plan.old);
            super::super::super::engine::memory_freed(ctx, charge);
        }
        finish(native, fresh, ACCEPTED);
    }
    Ok(true)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn empty_static_growth_reads_only_the_literal_count_before_detaching() {
        let outcome = crate::host::BodyRun::new(1, false).run(
            |_| {},
            |ctx| unsafe {
                let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
                let mapping = libc::mmap(
                    std::ptr::null_mut(),
                    page * 2,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                );
                assert_ne!(mapping, libc::MAP_FAILED);
                assert_eq!(
                    libc::mprotect(mapping.cast::<u8>().add(page).cast(), page, libc::PROT_NONE),
                    0
                );
                let old = mapping.cast::<u8>().add(page - 16).cast::<Header>();
                old.write(Header {
                    count: crate::COUNT_IMMORTAL,
                    descriptor: alloc::KIND_STATIC << alloc::KIND_SHIFT,
                });
                alloc::payload(old).cast::<u64>().write(0);
                let request = Growth {
                    version: VERSION,
                    kind: GROW_SEQUENCE,
                    ordinal: 0,
                    block: old,
                    region: std::ptr::null_mut(),
                    index: 0,
                    flags: 0,
                    extent: 0,
                    align: 0,
                    keep: 0,
                    element_size: 8,
                    element_align: 64,
                    capacity: 4,
                };
                let (plan, extent, align) = plan(ctx, &request).unwrap();
                assert!(plan.detach);
                assert_eq!(
                    plan.keep, 0,
                    "an empty literal lends no capacity or elements to copy"
                );
                assert_eq!((extent, align), (96, 64));
                assert_eq!(libc::munmap(mapping, page * 2), 0);
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    }
}
