//! A physical CPU save binds the whole ordinal's storage custody, not just
//! its successful result. The allocation ledger includes parent helper frames
//! and uninitialized allocation results; binding never scans their payloads.
use super::{cpu_handoff, native::Native};
use crate::pool::cpu_scoped::ledger::blocks::{Block, Observed, Owner};
use crate::{Header, TaskCtx};

pub(super) struct Saved {
    owner: Owner,
    sp: *mut u8,
    input: *mut Header,
    mapper: *mut Header,
    growth: *mut cpu_handoff::Growth,
    growth_input: *mut Header,
    summary: *mut cpu_handoff::Summary,
    result: *const u8,
    retained: Vec<*mut Header>,
    allocations: Vec<Block>,
    inputs: Vec<Observed>,
}

/// Record the initialized input graph before its credits can move into mapper
/// and helper locals. The ordinary holder walker supplies active arms, masks,
/// slices and sequence elements. Shared mapper roots retain their own graph.
/// # Safety
/// The checked input and mapper captures are initialized and immutable here.
pub(super) unsafe fn inputs(ctx: *mut TaskCtx, roots: [*mut Header; 2]) -> Vec<Observed> {
    unsafe {
        let Some(ledger) = (*ctx).heap_ref().meter.cpu.get() else {
            return Vec::new();
        };
        let mut visited = std::collections::BTreeSet::new();
        let mut pending = Vec::from(roots);
        let mut inputs = Vec::new();
        while let Some(block) = pending.pop() {
            if block.is_null() || !visited.insert(block.addr()) {
                continue;
            }
            if matches!(
                (*block).kind(),
                crate::alloc::KIND_SMALL | crate::alloc::KIND_LARGE
            ) {
                inputs.push(ledger.blocks.lock().unwrap().observe(block));
            }
            let layout = (*ctx).descriptor((*block).index());
            if (*layout).flags & crate::LAYOUT_FROZEN == 0 {
                crate::copy::holders(layout, crate::alloc::payload(block), &mut |slot| {
                    pending.push(slot.cast::<*mut Header>().read_unaligned());
                });
            }
        }
        inputs
    }
}

impl Saved {
    /// # Safety
    /// The dispatch owner has physically saved `native` and prevents resumption
    /// until this binding is installed. Its input, mapper and borrowed records
    /// remain owned by that saved frame. No other task runs on `ctx`'s worker.
    pub(super) unsafe fn bind(ctx: *mut TaskCtx, native: &Native, owner: Owner) -> Self {
        // SAFETY: the physical save owns all these records. Version checks
        // precede reads of record suffixes, including in refusal cases.
        unsafe {
            let growth = native.growth;
            let growth_input = if !growth.is_null() && (*growth).version == cpu_handoff::VERSION {
                (*growth).block
            } else {
                std::ptr::null_mut()
            };
            let summary = native.summary;
            let mut retained = vec![native.environment, native.source, growth_input];
            let result = if !summary.is_null() && (*summary).version == cpu_handoff::VERSION {
                let count = (*summary).retained_count as usize;
                if count <= crate::budget::stretch::ENVELOPE_MAX && count != 0 {
                    assert!(!(*summary).retained.is_null(), "a saved CPU root table");
                    retained
                        .extend_from_slice(std::slice::from_raw_parts((*summary).retained, count));
                }
                (*summary).value
            } else {
                std::ptr::null()
            };
            retained.retain(|root| !root.is_null());
            let (allocations, inputs) = (*ctx).heap_ref().meter.cpu.get().map_or_else(
                || (Vec::new(), Vec::new()),
                |ledger| {
                    let blocks = ledger.blocks.lock().unwrap();
                    (blocks.owned(owner), blocks.surviving(&native.input_blocks))
                },
            );
            Self {
                owner,
                sp: native.physical.saved_sp,
                input: native.environment,
                mapper: native.source,
                growth,
                growth_input,
                summary,
                result,
                retained,
                allocations,
                inputs,
            }
        }
    }

    /// No RC operation is performed on a borrowed root. Allocation generations
    /// distinguish a still-held block from reuse at the same address.
    pub(super) unsafe fn check(&self, ctx: *mut TaskCtx, native: &Native, owner: Owner) {
        // SAFETY: the same exclusive saved frame still owns its records.
        unsafe {
            assert_eq!(self.owner, owner, "a saved CPU ordinal and task generation");
            assert_eq!(
                self.sp, native.physical.saved_sp,
                "the exact saved CPU call"
            );
            assert_eq!(self.input, native.environment);
            assert_eq!(self.mapper, native.source);
            assert_eq!(self.growth, native.growth);
            if !self.growth.is_null() && (*self.growth).version == cpu_handoff::VERSION {
                assert_eq!(self.growth_input, (*self.growth).block);
            }
            assert_eq!(self.summary, native.summary);
            if !self.summary.is_null() && (*self.summary).version == cpu_handoff::VERSION {
                assert_eq!(self.result, (*self.summary).value);
                let mut retained = vec![self.input, self.mapper, self.growth_input];
                let count = (*self.summary).retained_count as usize;
                if count <= crate::budget::stretch::ENVELOPE_MAX && count != 0 {
                    retained.extend_from_slice(std::slice::from_raw_parts(
                        (*self.summary).retained,
                        count,
                    ));
                }
                retained.retain(|root| !root.is_null());
                assert_eq!(self.retained, retained, "the complete borrowed CPU roots");
            }
            if let Some(ledger) = (*ctx).heap_ref().meter.cpu.get() {
                assert!(
                    ledger.blocks.lock().unwrap().valid_roots(
                        &self.allocations,
                        &self.inputs,
                        &self.retained
                    ),
                    "a saved CPU root keeps its allocation generation"
                );
            }
        }
    }
}

/// Bind before publishing the dispatcher acknowledgement, on the saving
/// worker. Only its own heap may collect returns and release idle backing.
/// Live blocks and lent region segments remain under ordinary holder custody.
/// # Safety
/// Called by the actual saving worker while it exclusively owns this image.
/// The scheduler role is held and the physical frame cannot resume yet.
pub(in crate::tasks::ordered) unsafe fn save(ctx: *mut TaskCtx, owner: Owner) {
    // SAFETY: the caller is the physical save callback, not the requesting
    // head. It owns both this image and its worker's heap until it returns.
    unsafe {
        let Some(native) = (*ctx)
            .ahead
            .as_mut()
            .and_then(|local| local.native.as_mut())
        else {
            return;
        };
        if !native.cpu {
            return;
        }
        let native = &raw mut **native;
        let saved = Saved::bind(ctx, &*native, owner);
        let heap = (*ctx).heap();
        let cap = heap.idle_cap;
        heap.idle_cap = 0;
        heap.trim();
        heap.idle_cap = cap;
        heap.regions.collect();
        heap.regions.trim(0);
        if let Some(ledger) = (*ctx).heap_ref().meter.cpu.get() {
            let blocks = ledger.blocks.lock().unwrap();
            assert!(
                blocks.valid(&saved.allocations) && blocks.valid_observed(&saved.inputs),
                "owner collection preserves every live allocation and external holder"
            );
        }
        saved.check(ctx, &*native, owner);
        (*native).saved_roots = Some(saved);
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests;
