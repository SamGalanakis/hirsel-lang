//! Read-only typed graph planning for the existing seal transaction.
//!
//! Like copy.rs and ordered/capture.rs, this uses explicit worklists and the
//! compiler's linked descriptor programs. Value metadata establishes typed
//! coverage independently of cleanup (arena-local edges can have no release).
//! Semantic receipts retain occurrence multiplicity; only physical copies are
//! deduplicated. This Rust seam does not extend native seal admission.
use super::guard::DemandedGuard;
use super::receipt::semantic::{AllocationId, Origin, Record};
use super::receipt::{Receipt, View};
use super::{Block, BlockId, Patch, Placement, Plan, Root};
use crate::alloc::{Header, payload};
use crate::{Layout, TaskCtx};
use std::collections::BTreeMap;

mod descriptors;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Guard,
    Semantic,
    Descriptor,
    Image,
    Coverage,
    Foreign,
}

/// One checked binding of a retained semantic allocation to its typed payload.
/// An elided payload has no physical block; its initialized receipt image still
/// describes all its retained outgoing occurrences. Every inventory ID is
/// required, including dead, empty and zero-charge entries.
pub struct Retained {
    pub allocation: AllocationId,
    pub descriptor: u32,
    pub block: Option<*mut Header>,
    pub extent: usize,
}

/// Build a complete existing Plan without acquiring or consuming source credits.
/// Root receipts describe inline holder bytes. Ordinary child and retained
/// receipts describe initialized full owner payloads, with one child per typed
/// outgoing holder in address order. Shared leaves describe their owner word.
/// Repeated nodes are repeated semantic occurrences, not additional storage
/// permissions. Arena child records must be the current inventory snapshot.
///
/// # Safety
/// The entered invocation exclusively owns this exact live guard and root;
/// external arena loans have ended. All descriptor tables/programs are live,
/// independently checked compiler data; all source pointers and initialized
/// images are paired with their original receipts and live resource-free
/// storage of this run. Retained bindings cover every semantic allocation and
/// its current physical representation. Journal/root/physical cleanup own
/// their exact, disjoint source credits as required by DemandedGuard::seal.
/// Sources stay live and unchanged through planning and the subsequent seal.
/// A returned Plan borrows those sources by raw pointer; it grants no custody.
pub unsafe fn plan(
    guard: &DemandedGuard<'_>,
    descriptor: u32,
    value: *mut u8,
    root: &Receipt<Record>,
    retained: &[Retained],
) -> Result<Plan, Error> {
    unsafe {
        plan_inventory(
            guard.context(),
            guard.base(),
            guard.inventory().map_err(|_| Error::Guard)?,
            descriptor,
            value,
            root,
            retained,
        )
    }
}

/// The native entry borrows the same live journal instead of manufacturing a
/// second guard owner. Source custody and initialized images obey plan's contract.
pub(super) unsafe fn plan_inventory(
    ctx: *mut TaskCtx,
    base: *mut crate::region::Region,
    inventory: &super::receipt::semantic::Inventory,
    descriptor: u32,
    value: *mut u8,
    root: &Receipt<Record>,
    retained: &[Retained],
) -> Result<Plan, Error> {
    let grant = inventory.grant();
    let charge = inventory.seal_charge(root).map_err(|_| Error::Semantic)?;
    if root.record().origin() != Origin::Inline {
        return Err(Error::Image);
    }
    let mut active = unsafe { (*ctx).region };
    while !active.is_null() && active != base {
        active = unsafe { (*active).parent };
    }
    if active.is_null() {
        return Err(Error::Guard);
    }
    let layout = unsafe { descriptors::layout(ctx, descriptor) }?;
    if layout.flags & crate::LAYOUT_DYNAMIC != 0 {
        return Err(Error::Descriptor);
    }
    let extent = usize::try_from(layout.size).map_err(|_| Error::Descriptor)?;
    let align = layout.align as usize;
    if root.alignment().is_none_or(|alignment| alignment < align) {
        return Err(Error::Image);
    }
    if extent != 0 && (value.is_null() || !value.addr().is_multiple_of(align)) {
        return Err(Error::Image);
    }
    if retained.len() != inventory.entries().len() {
        return Err(Error::Coverage);
    }
    let mut entries = BTreeMap::new();
    for binding in retained {
        if entries.insert(binding.allocation, binding).is_some() {
            return Err(Error::Coverage);
        }
    }
    let mut arena_nodes = BTreeMap::new();
    let mut pending = Vec::new();
    for (id, receipt) in inventory.entries() {
        let binding = entries.get(&id).ok_or(Error::Coverage)?;
        if receipt
            .bytes()
            .is_none_or(|image| image.len() != binding.extent)
        {
            return Err(Error::Image);
        }
        if let Some(block) = binding.block {
            if block.is_null()
                || arena_nodes
                    .insert(block.addr(), receipt.generation())
                    .is_some()
            {
                return Err(Error::Coverage);
            }
        }
        pending.push(Work::Retained(binding, receipt.view()));
    }
    pending.push(Work::Root(root.view()));
    let mut result = Plan {
        root: Root {
            descriptor,
            source: value,
            extent,
            align,
            patches: Vec::new(),
        },
        blocks: Vec::new(),
        shared: Vec::new(),
        charge,
    };
    let mut physical = BTreeMap::new();
    while let Some(work) = pending.pop() {
        let (index, source, size, receipt, destination) = match work {
            Work::Root(receipt) => (descriptor, value, extent, receipt, Destination::Root),
            Work::Retained(binding, receipt) => match binding.block {
                Some(block) => {
                    let id = unsafe {
                        block_plan(
                            ctx,
                            base,
                            block,
                            binding.descriptor,
                            receipt,
                            &mut result,
                            &mut physical,
                        )
                    }?;
                    (
                        binding.descriptor,
                        payload(block),
                        result.blocks[id.0].extent,
                        receipt,
                        Destination::Block(id),
                    )
                }
                None => {
                    let image = receipt.bytes().ok_or(Error::Image)?;
                    (
                        binding.descriptor,
                        image.as_ptr().cast_mut(),
                        image.len(),
                        receipt,
                        Destination::Elided,
                    )
                }
            },
            Work::Block(block, index, receipt) => {
                if receipt.record().origin() == Origin::Arena(grant) {
                    if arena_nodes.get(&block.addr()) != Some(&receipt.generation()) {
                        return Err(Error::Coverage);
                    }
                    // Its owned outgoing occurrences are counted at its sole
                    // retained entry, never again at an arena reference.
                    continue;
                }
                let id = unsafe {
                    block_plan(ctx, base, block, index, receipt, &mut result, &mut physical)
                }?;
                (
                    index,
                    payload(block),
                    result.blocks[id.0].extent,
                    receipt,
                    Destination::Block(id),
                )
            }
        };
        unsafe { image_matches(receipt, source, size) }?;
        let edges = unsafe { descriptors::edges(ctx, index, source, size) }?;
        let children: Vec<_> = receipt.children().collect();
        if children.len() != edges.len() {
            return Err(Error::Coverage);
        }
        let mut patches = Vec::new();
        for (edge, child) in edges.into_iter().zip(children) {
            let block = unsafe {
                source
                    .add(edge.offset)
                    .cast::<*mut Header>()
                    .read_unaligned()
            };
            if block.is_null() {
                return Err(Error::Image);
            }
            let actual = unsafe { descriptors::layout(ctx, (*block).index()) }?;
            let shared = actual.flags & crate::LAYOUT_FROZEN != 0;
            if shared != edge.shared || shared != (child.record().origin() == Origin::Shared) {
                return Err(Error::Descriptor);
            }
            if let Some(expected) = edge.payload {
                if unsafe { (*block).index() } != expected {
                    return Err(Error::Descriptor);
                }
            }
            if shared {
                let word = block.addr().to_ne_bytes();
                if child.bytes() != Some(word.as_slice()) || child.children().len() != 0 {
                    return Err(Error::Image);
                }
                result.shared.push(block);
                continue;
            }
            let child_index = unsafe { (*block).index() };
            if edge.slice {
                let start = unsafe { source.add(edge.offset + 8).cast::<u64>().read_unaligned() };
                let length = unsafe { source.add(edge.offset + 16).cast::<u64>().read_unaligned() };
                let initialized = child.bytes().ok_or(Error::Image)?.len() as u64;
                if start
                    .checked_add(8)
                    .and_then(|n| n.checked_add(length))
                    .is_none_or(|end| end > initialized)
                {
                    return Err(Error::Image);
                }
            }
            match child.record().origin() {
                Origin::Arena(id) if id == grant => {
                    if arena_nodes.get(&block.addr()) != Some(&child.generation()) {
                        return Err(Error::Coverage);
                    }
                }
                Origin::Heap | Origin::Static => {}
                _ => return Err(Error::Foreign),
            }
            let id = unsafe {
                block_plan(
                    ctx,
                    base,
                    block,
                    child_index,
                    child,
                    &mut result,
                    &mut physical,
                )
            }?;
            patches.push(Patch {
                offset: edge.offset,
                target: id,
            });
            pending.push(Work::Block(block, child_index, child));
        }
        match destination {
            Destination::Root => result.root.patches = patches,
            Destination::Block(id) => {
                let previous = &mut result.blocks[id.0].patches;
                if !previous.is_empty() && !same_patches(previous, &patches) {
                    return Err(Error::Coverage);
                }
                *previous = patches;
            }
            Destination::Elided => {}
        }
    }
    Ok(result)
}

fn same_patches(a: &[Patch], b: &[Patch]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.offset == b.offset && a.target == b.target)
}

enum Destination {
    Root,
    Block(BlockId),
    Elided,
}
enum Work<'a> {
    Root(View<'a, Record>),
    Retained(&'a Retained, View<'a, Record>),
    Block(*mut Header, u32, View<'a, Record>),
}

unsafe fn image_matches(
    receipt: View<'_, Record>,
    source: *mut u8,
    size: usize,
) -> Result<(), Error> {
    let image = receipt.bytes().ok_or(Error::Image)?;
    if image.len() != size
        || (size != 0 && image != unsafe { std::slice::from_raw_parts(source, size) })
    {
        return Err(Error::Image);
    }
    Ok(())
}

unsafe fn block_plan(
    ctx: *mut TaskCtx,
    arena: *mut crate::region::Region,
    source: *mut Header,
    descriptor: u32,
    receipt: View<'_, Record>,
    plan: &mut Plan,
    physical: &mut BTreeMap<usize, BlockId>,
) -> Result<BlockId, Error> {
    if source.is_null() || unsafe { (*source).index() } != descriptor {
        return Err(Error::Descriptor);
    }
    let layout = unsafe { descriptors::layout(ctx, descriptor) }?;
    let (capacity, align) = unsafe { (*ctx).heap_ref().usable(source) };
    let extent = receipt.bytes().ok_or(Error::Image)?.len();
    let align = usize::try_from(align).map_err(|_| Error::Image)?;
    if extent as u64 > capacity
        || extent < layout.size as usize
        || (layout.flags & crate::LAYOUT_DYNAMIC == 0 && extent != layout.size as usize)
        || !align.is_power_of_two()
        || align < layout.align as usize
        || receipt
            .alignment()
            .is_none_or(|alignment| alignment < layout.align as usize)
    {
        return Err(Error::Image);
    }
    unsafe { image_matches(receipt, payload(source), extent) }?;
    let adopt = matches!(receipt.record().origin(), Origin::Arena(_));
    let belongs = unsafe { crate::region::contains_storage(arena, source, extent) };
    if adopt != belongs {
        return Err(Error::Foreign);
    }
    if let Some(&id) = physical.get(&source.addr()) {
        if plan.blocks[id.0].extent != extent
            || plan.blocks[id.0].placement
                != if adopt {
                    Placement::Adopt
                } else {
                    Placement::Evacuate
                }
        {
            return Err(Error::Image);
        }
        return Ok(id);
    }
    let id = BlockId(plan.blocks.len());
    plan.blocks.push(Block {
        source,
        descriptor,
        extent,
        align,
        placement: if adopt {
            Placement::Adopt
        } else {
            Placement::Evacuate
        },
        patches: Vec::new(),
    });
    physical.insert(source.addr(), id);
    Ok(id)
}

#[cfg(all(test, feature = "diagnostic"))]
pub(in crate::sealed_region) mod tests;
