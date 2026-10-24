//! Typed evacuation and publication, reusing region allocation/remote custody
//! and the runtime's existing source release engine. A plan is compiler data,
//! not proof: callers must obtain complete graph coverage from validated source
//! and storage descriptors. Public native operations remain unavailable until
//! that producer and both emitters are connected.
//!
//! Charge is a semantic summary supplied independently of physical blocks.
//! In particular source heap placement is never inferred from Header.domain:
//! a compiler-local arena may physically contain a semantic heap value.
use super::{PreparedSeal, SealedRegion};
use crate::alloc::{Header, payload};
use crate::region::sealed_cleanup::Credits;
use crate::region::{
    Region, detach_storage, hirsel_rt_v2_arena_alloc, hirsel_rt_v2_region_enter,
    hirsel_rt_v2_region_exit,
};
use crate::{STATUS_OK, TaskCtx, fault};
use std::collections::BTreeSet;
use std::mem::MaybeUninit;
use std::ptr;

/// Overflow remains greater than every possible bound, including u64::MAX.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Charge {
    Bytes(u64),
    Overflow,
}
impl Charge {
    pub fn plus(self, other: Self) -> Self {
        match (self, other) {
            (Self::Bytes(a), Self::Bytes(b)) => {
                a.checked_add(b).map_or(Self::Overflow, Self::Bytes)
            }
            _ => Self::Overflow,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockId(pub usize);

/// Physical custody only; it does not select a language charge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Placement {
    /// Already in the consumed explicit arena; preserve its address.
    Adopt,
    /// Copy privately, including semantic heap data physically in a temporary
    /// compiler region, static data, and ordinary counted heap blocks.
    Evacuate,
}

/// Replace only the owner word. Slice offsets/lengths beside it stay unchanged,
/// since evacuation retains the full physical byte owner and its extent.
#[derive(Clone, Copy, Debug)]
pub struct Patch {
    pub offset: usize,
    pub target: BlockId,
}

pub struct Block {
    pub source: *mut Header,
    pub descriptor: u32,
    pub extent: usize,
    pub align: usize,
    pub placement: Placement,
    pub patches: Vec<Patch>,
}

pub struct Root {
    /// Inline holder/value descriptor, not its referenced block's descriptor.
    pub descriptor: u32,
    pub source: *mut u8,
    pub extent: usize,
    pub align: usize,
    pub patches: Vec<Patch>,
}

/// Complete physical graph plus independent semantic obligations. A dead
/// retained arena block stays in `blocks` even when root does not reach it.
/// Equal physical identities occur once; equal bytes at distinct identities
/// remain separate. `shared` is per semantic owned occurrence, including
/// credits whose physical holder was elided. Snapshots acquire no credits.
pub struct Plan {
    pub root: Root,
    pub blocks: Vec<Block>,
    pub shared: Vec<*mut Header>,
    /// Premaintained retained inventory total plus the root's extra obligation;
    /// never recomputed from the number of physically copied blocks.
    pub charge: Charge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Exhausted,
    InvalidPlan,
    /// Recoverable staging interruption used by the mechanical test harness.
    /// Real host allocation failure retains the existing environmental policy.
    Interrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Event {
    Owner,
    Snapshot,
    Extension,
    Block(usize),
    Shared(usize),
    Ready,
}

/// Actual journal holder batches, additional to root/physical guard credits.
/// Snapshot is nonowning until disarm at commit; abort keeps the journal live.
pub(super) trait JournalCredits {
    fn snapshot(&self, credits: &mut Credits);
    fn disarm(&mut self);
}

fn extent_valid(extent: usize, align: usize) -> bool {
    std::alloc::Layout::from_size_align(extent.max(1), align).is_ok()
}

impl Plan {
    fn check(&self) -> Result<(), Error> {
        let patches = |extent: usize, rows: &[Patch]| {
            let mut words = BTreeSet::new();
            rows.iter().all(|patch| {
                patch.target.0 < self.blocks.len()
                    && patch
                        .offset
                        .checked_add(size_of::<*mut Header>())
                        .is_some_and(|end| end <= extent)
                    && words.insert(patch.offset)
            }) && words
                .iter()
                .zip(words.iter().skip(1))
                .all(|(left, right)| right - left >= size_of::<*mut Header>())
        };
        let mut sources = BTreeSet::new();
        if !extent_valid(self.root.extent, self.root.align)
            || (self.root.source.is_null() && self.root.extent != 0)
            || !patches(self.root.extent, &self.root.patches)
            || self.shared.iter().any(|root| root.is_null())
            || self.blocks.iter().any(|block| {
                block.source.is_null()
                    || !sources.insert(block.source.addr())
                    || !extent_valid(block.extent, block.align)
                    || !patches(block.extent, &block.patches)
            })
        {
            return Err(Error::InvalidPlan);
        }
        Ok(())
    }
}

/// A private extension guard always exits, including recoverable interruption.
/// It holds no source credits and schedules no payload destructor.
struct Extension {
    ctx: *mut TaskCtx,
    region: Box<Region>,
}
impl Extension {
    unsafe fn new(ctx: *mut TaskCtx) -> Self {
        let mut region = Box::new(MaybeUninit::<Region>::uninit());
        unsafe {
            hirsel_rt_v2_region_enter(ctx, region.as_mut_ptr());
            Self {
                ctx,
                region: Box::from_raw(Box::into_raw(region).cast()),
            }
        }
    }
}
impl Drop for Extension {
    fn drop(&mut self) {
        unsafe {
            if hirsel_rt_v2_region_exit(self.ctx, &mut *self.region, 0, 0) != STATUS_OK {
                fault("private seal extension unexpectedly ran a failing destructor");
            }
        }
    }
}

/// Preflight, evacuate, retire old credits, adopt segments, then publish one
/// owner. On failure the source root and arena cleanup remain untouched and
/// no owner is returned. Successful publication consumes their credits.
/// The matching library body records an Exhausted result at the seal site.
///
/// # Safety
/// `ctx` and `arena` are live in the same run; the exact explicit grant is
/// exclusively consumed and all external arena views have ended. `plan` is
/// derived from independently checked complete typed storage/source evidence:
/// every retained ordinary outgoing edge is covered exactly once by its
/// source cleanup and physical patch; Shared multiplicities are exact; all
/// source storage is live, readable at the stated descriptor/extent/alignment,
/// and resource-free. Adopt blocks belong to `arena`, evacuate blocks are not
/// externally mutated. Root's inline descriptor covers its actual owned
/// source credits. No plan/snapshot pointer may survive this call. The run
/// outlives the returned affine owner, destroyed once through `drop_owner`.
pub unsafe fn prepare_commit(
    ctx: *mut TaskCtx,
    arena: *mut Region,
    plan: &Plan,
) -> Result<*mut SealedRegion, Error> {
    unsafe { run(ctx, arena, plan, &mut |_| Ok(())) }
}

unsafe fn run(
    ctx: *mut TaskCtx,
    arena: *mut Region,
    plan: &Plan,
    checkpoint: &mut impl FnMut(Event) -> Result<(), Error>,
) -> Result<*mut SealedRegion, Error> {
    unsafe { run_with_journal(ctx, arena, plan, checkpoint, None) }
}

/// The same transaction with separately owned source-journal credits.
/// # Safety
/// prepare_commit's contract plus exact, nonduplicated journal custody applies. No journal or source cleanup is disarmed before Ready succeeds.
pub(super) unsafe fn run_with_journal(
    ctx: *mut TaskCtx,
    arena: *mut Region,
    plan: &Plan,
    checkpoint: &mut impl FnMut(Event) -> Result<(), Error>,
    mut journal: Option<&mut dyn JournalCredits>,
) -> Result<*mut SealedRegion, Error> {
    plan.check()?;
    let Charge::Bytes(charge) = plan.charge else {
        return Err(Error::Exhausted);
    };
    unsafe {
        // No logical charge and no source mutation happens on a failed prepare.
        if charge > (*arena).remaining {
            return Err(Error::Exhausted);
        }
        checkpoint(Event::Owner)?;
        let mut prepared = PreparedSeal::new(
            &mut *ctx,
            plan.root.source,
            plan.root.extent,
            plan.root.align,
        );
        checkpoint(Event::Snapshot)?;
        let mut credits = Credits::new();
        credits.value(prepared.ctx, plan.root.descriptor, plan.root.source);
        credits.arena(prepared.ctx, arena);
        if let Some(journal) = journal.as_deref_mut() {
            journal.snapshot(&mut credits);
        }
        let mut addresses = Vec::with_capacity(plan.blocks.len());
        checkpoint(Event::Extension)?;
        let mut extension = Extension::new(ctx);
        // First copy every block without touching an original arena cell. All
        // pointers in private bytes may temporarily still name source data.
        for (ordinal, block) in plan.blocks.iter().enumerate() {
            let destination = match block.placement {
                Placement::Adopt => block.source,
                Placement::Evacuate => {
                    checkpoint(Event::Block(ordinal))?;
                    let destination = hirsel_rt_v2_arena_alloc(
                        ctx,
                        &mut *extension.region,
                        block.descriptor,
                        block.extent as u64,
                        block.align as u64,
                        0,
                    );
                    ptr::copy_nonoverlapping(
                        payload(block.source),
                        payload(destination),
                        block.extent,
                    );
                    destination
                }
            };
            addresses.push(destination);
        }
        for (ordinal, &root) in plan.shared.iter().enumerate() {
            checkpoint(Event::Shared(ordinal))?;
            prepared.retain_shared(root);
        }
        checkpoint(Event::Ready)?;

        // Commit begins. Through publication nothing reserves scratch, calls
        // language code, observes cancellation, or charges again.
        if (*arena).bound != 0 {
            (*arena).remaining -= charge;
        }
        credits.disarm(arena);
        if let Some(journal) = journal {
            journal.disarm();
        }
        for (block, &destination) in plan.blocks.iter().zip(&addresses) {
            if block.placement == Placement::Adopt && crate::region::is_tracked(destination) {
                // A sealed payload is immutable, never an open tracked builder.
                // Remove the only prefix pointer back into the source frame.
                destination
                    .cast::<u8>()
                    .sub(crate::region::TRACKED_REGION_BACK as usize)
                    .cast::<*mut Region>()
                    .write(ptr::null_mut());
                (*destination).count = crate::COUNT_IMMORTAL;
            }
            for patch in &block.patches {
                payload(destination)
                    .add(patch.offset)
                    .cast::<*mut Header>()
                    .write_unaligned(addresses[patch.target.0]);
            }
        }
        for patch in &plan.root.patches {
            prepared
                .root_mut()
                .add(patch.offset)
                .cast::<*mut Header>()
                .write_unaligned(addresses[patch.target.0]);
        }
        let storage = detach_storage(&mut *extension.region);
        storage.transfer_diagnostic(prepared.ctx, &mut *extension.region);
        prepared.control.as_mut().expect("private owner").extensions = Some(storage);
        let owner = prepared.commit(arena);
        // Publication is complete. Source retirement follows the existing root
        // deferral policy; it may reserve runtime bookkeeping but cannot call
        // a user destructor, fail in the language, or suspend.
        credits.release(&mut *ctx);
        // Close only the private extension's lexical guard, preserving any
        // nested scope active over the explicitly named consumed arena.
        drop(extension);
        Ok(owner)
    }
}

#[cfg(all(test, feature = "diagnostic"))]
mod tests;
