//! Exact source coverage for cleanup that cannot invoke user code or close a
//! handle. This deliberately refuses every uncertified finalizer; it is not
//! a physical CleanupPlan and grants no compact admission.
use super::*;
use crate::stage::ValidatedUnit;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CleanupOwner {
    Place(AddressId),
    Value(ValueId),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SynchronousCleanup {
    pub at: InstructionPoint,
    pub anchor: CleanupAnchorId,
    pub failure: FailureId,
    pub owner: CleanupOwner,
    pub layout: LayoutId,
    /// The actual transferred closure's environment, never an erased
    /// callable layout's advisory environment or a Reset descriptor claim.
    pub environment: Option<LayoutId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SynchronousCleanupPlan {
    pub body: hirsel_syntax::ContentHash,
    pub digest: hirsel_syntax::ContentHash,
    pub sites: Vec<SynchronousCleanup>,
}

fn original_closure_place(unit: &LirUnit<Ready>, address: AddressId, closure: SlotId) -> bool {
    whole(unit, address).is_some_and(|slot| {
        slot == closure
            || unit
                .slots
                .get(slot.index())
                .is_some_and(|s| s.kind == SlotKind::Parameter(1))
    })
}

fn original_closure_value(unit: &LirUnit<Ready>, value: ValueId, closure: SlotId) -> bool {
    // A release's callable operand is the exact take of the closure place.
    // More general carriers stay unproved instead of borrowing RC's origins.
    unit.instructions.iter().any(|inst| {
        matches!(*inst, Inst::Read { result, address, mode: ReadMode::Take, .. }
            if result == value && original_closure_place(unit, address, closure))
    })
}

impl IsolateProtocol {
    /// Enumerate all Ready cleanup sites, including failure-only disposal.
    /// A callable is accepted only with the independent transferred-closure
    /// identity and an environment whose entire destruction is mechanical.
    pub fn synchronous_cleanup(
        &self,
        wrapper: &ValidatedUnit<Ready>,
        layouts: &LayoutTable,
    ) -> Option<SynchronousCleanupPlan> {
        let u = wrapper.unit();
        if self.identity.wrapper != u.header.instance || self.identity.wrapper_digest != u.digest()
        {
            return None;
        }
        let mut sites = Vec::new();
        for (block, row) in u.blocks.iter().enumerate() {
            for (instruction, inst) in row.instructions.get(&u.instructions)?.iter().enumerate() {
                let (owner, layout, anchor, failure, known) = match *inst {
                    Inst::Drop {
                        address,
                        anchor,
                        failure,
                        ..
                    } => {
                        let layout = u
                            .types
                            .get(u.addresses.get(address.index())?.ty.index())?
                            .layout;
                        (
                            CleanupOwner::Place(address),
                            layout,
                            anchor,
                            failure,
                            original_closure_place(u, address, self.identity.closure),
                        )
                    }
                    Inst::Release {
                        value,
                        layout,
                        anchor,
                        failure,
                        ..
                    }
                    | Inst::Reset {
                        value,
                        layout,
                        anchor,
                        failure,
                        ..
                    } => (
                        CleanupOwner::Value(value),
                        layout,
                        anchor,
                        failure,
                        original_closure_value(u, value, self.identity.closure),
                    ),
                    Inst::FinalizeBegin { .. } => return None,
                    _ => continue,
                };
                let environment =
                    if matches!(layouts.layout(layout)?.kind, LayoutKind::Callable { .. }) {
                        if !known {
                            return None;
                        }
                        Some(self.identity.environment?)
                    } else {
                        None
                    };
                if crate::validate::Layouts(layouts).destructible(environment.unwrap_or(layout)) {
                    return None;
                }
                sites.push(SynchronousCleanup {
                    at: InstructionPoint {
                        block: BlockId(block as u32),
                        instruction: instruction as u32,
                    },
                    anchor,
                    failure,
                    owner,
                    layout,
                    environment,
                });
            }
        }
        Some(SynchronousCleanupPlan {
            body: u.header.instance,
            digest: u.digest(),
            sites,
        })
    }

    /// Independently cover actual instructions and verify their destruction
    /// descriptors. No supplied digest, omitted failure path or equal-layout
    /// owner substitutes for the actual Ready operand and callback identity.
    pub fn check_synchronous_cleanup(
        &self,
        caller: &ValidatedUnit<Ready>,
        wrapper: &ValidatedUnit<Ready>,
        callback: &ValidatedUnit<Ready>,
        layouts: &LayoutTable,
        plan: &SynchronousCleanupPlan,
    ) -> bool {
        if self.validate(caller, wrapper, callback, layouts).is_err() {
            return false;
        }
        let u = wrapper.unit();
        if plan.body != u.header.instance || plan.digest != u.digest() {
            return false;
        }
        let mut seen = BTreeSet::new();
        for site in &plan.sites {
            if !seen.insert((site.at.block, site.at.instruction)) {
                return false;
            }
            let Some(inst) = u
                .blocks
                .get(site.at.block.index())
                .and_then(|b| b.instructions.get(&u.instructions))
                .and_then(|insts| insts.get(site.at.instruction as usize))
            else {
                return false;
            };
            let (owner, layout, anchor, failure, original) = match *inst {
                Inst::Drop {
                    address,
                    anchor,
                    failure,
                    ..
                } => {
                    let Some(layout) = u
                        .addresses
                        .get(address.index())
                        .and_then(|a| u.types.get(a.ty.index()))
                        .map(|ty| ty.layout)
                    else {
                        return false;
                    };
                    (
                        CleanupOwner::Place(address),
                        layout,
                        anchor,
                        failure,
                        whole(u, address).is_some_and(|slot| {
                            slot == self.identity.closure
                                || u.slots
                                    .get(slot.index())
                                    .is_some_and(|s| s.kind == SlotKind::Parameter(1))
                        }),
                    )
                }
                Inst::Release {
                    value,
                    layout,
                    anchor,
                    failure,
                    ..
                }
                | Inst::Reset {
                    value,
                    layout,
                    anchor,
                    failure,
                    ..
                } => {
                    let original = u.instructions.iter().any(|def| match *def {
                        Inst::Read {
                            result,
                            address,
                            mode: ReadMode::Take,
                            ..
                        } if result == value => whole(u, address).is_some_and(|slot| {
                            slot == self.identity.closure
                                || u.slots
                                    .get(slot.index())
                                    .is_some_and(|s| s.kind == SlotKind::Parameter(1))
                        }),
                        _ => false,
                    });
                    (
                        CleanupOwner::Value(value),
                        layout,
                        anchor,
                        failure,
                        original,
                    )
                }
                _ => return false,
            };
            if (site.owner, site.layout, site.anchor, site.failure)
                != (owner, layout, anchor, failure)
            {
                return false;
            }
            let Some(descriptor) = layouts.layout(layout) else {
                return false;
            };
            if matches!(descriptor.kind, LayoutKind::Callable { .. }) {
                if !original
                    || site.environment.is_none()
                    || site.environment != self.identity.environment
                {
                    return false;
                }
            } else if site.environment.is_some() {
                return false;
            }
            if crate::validate::Layouts(layouts).destructible(site.environment.unwrap_or(layout)) {
                return false;
            }
        }
        for (block, row) in u.blocks.iter().enumerate() {
            let Some(instructions) = row.instructions.get(&u.instructions) else {
                return false;
            };
            for (instruction, inst) in instructions.iter().enumerate() {
                match inst {
                    Inst::FinalizeBegin { .. } => return false,
                    Inst::Drop { .. } | Inst::Release { .. } | Inst::Reset { .. }
                        if !seen.contains(&(BlockId(block as u32), instruction as u32)) =>
                    {
                        return false;
                    }
                    _ => {}
                }
            }
        }
        true
    }
}
