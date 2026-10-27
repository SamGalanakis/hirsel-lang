//! Saved unique growth uses the allocator's existing classes and RawVec rule.
use super::*;
use crate::alloc;

pub(in crate::tasks::ordered::ahead) enum Plan {
    Heap(HeapPlan),
    Region(crate::region::cpu_ahead::Plan),
}

pub(in crate::tasks::ordered::ahead) struct HeapPlan {
    old: *mut Header,
    keep: u64,
    elements: Option<(u64, u64)>,
    detach: bool,
}

/// Compute the same class extent as block_grow/seq_grow without allocating,
/// freeing, or changing a saved input. Shared flat sequences detach at the
/// requested capacity, as alloc::detach_reserved does.
unsafe fn plan(ctx: *mut TaskCtx, request: &Growth) -> Option<(HeapPlan, u64, u64)> {
    if !matches!(request.kind, GROW_BLOCK | GROW_SEQUENCE) || request.block.is_null() {
        return None;
    }
    // SAFETY: the checked Growth caller retains its live unique block.
    unsafe {
        let old = request.block;
        if (*old).index() >= (*ctx).heap_ref().layout_count() {
            return None;
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
                || layout.step_count != 0
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
            (*native).growth_plan = Some(Plan::Region(plan));
        }
    }
    if let Some(Plan::Region(plan)) = unsafe { (*native).growth_plan.as_ref() } {
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
            if let Some(ledger) = (*ctx).heap_ref().meter.cpu.get() {
                let task = super::super::super::engine::current(ctx);
                let owner = super::super::super::engine::with_driver(ctx, |d| {
                    d.cpu_batch.as_ref().unwrap().block_owner(task).unwrap()
                });
                ledger
                    .blocks
                    .lock()
                    .unwrap()
                    .adopt_in(owner, fresh, (*pointer).region.addr());
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
            // Flat layouts have no child holders to duplicate. The original
            // retains its other credits; immortal/static storage keeps none.
            if (*plan.old).count != crate::COUNT_IMMORTAL {
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
