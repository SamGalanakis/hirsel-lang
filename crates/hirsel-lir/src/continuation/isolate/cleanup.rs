//! Exact Ready cleanup operands. Source coverage and retained cursor/frame
//! custody are separate proofs; enumerating a destructor grants no admission.
use super::*;
use crate::stage::ValidatedUnit;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CleanupOwner {
    Place(AddressId),
    Value(ValueId),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SourceCleanup {
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
pub struct SourceCleanupPlan {
    pub body: hirsel_syntax::ContentHash,
    pub digest: hirsel_syntax::ContentHash,
    pub sites: Vec<SourceCleanup>,
    pub finalizers: Vec<InstructionPoint>,
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
    ) -> Option<SourceCleanupPlan> {
        let plan = self.source_cleanup(wrapper, layouts)?;
        (plan.finalizers.is_empty()
            && plan.sites.iter().all(|site| {
                !crate::validate::Layouts(layouts)
                    .destructible(site.environment.unwrap_or(site.layout))
            }))
        .then_some(plan)
    }

    pub(crate) fn source_cleanup(
        &self,
        wrapper: &ValidatedUnit<Ready>,
        layouts: &LayoutTable,
    ) -> Option<SourceCleanupPlan> {
        let u = wrapper.unit();
        if self.identity.wrapper != u.header.instance || self.identity.wrapper_digest != u.digest()
        {
            return None;
        }
        let mut sites = Vec::new();
        let mut finalizers = Vec::new();
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
                    Inst::FinalizeBegin { .. } => {
                        finalizers.push(InstructionPoint {
                            block: BlockId(block as u32),
                            instruction: instruction as u32,
                        });
                        continue;
                    }
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
                sites.push(SourceCleanup {
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
        Some(SourceCleanupPlan {
            body: u.header.instance,
            digest: u.digest(),
            sites,
            finalizers,
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
        plan: &SourceCleanupPlan,
    ) -> bool {
        plan.finalizers.is_empty()
            && plan.sites.iter().all(|site| {
                !crate::validate::Layouts(layouts)
                    .destructible(site.environment.unwrap_or(site.layout))
            })
            && self.check_source_cleanup(caller, wrapper, callback, layouts, plan)
    }

    pub(crate) fn check_source_cleanup(
        &self,
        caller: &ValidatedUnit<Ready>,
        wrapper: &ValidatedUnit<Ready>,
        callback: &ValidatedUnit<Ready>,
        layouts: &LayoutTable,
        plan: &SourceCleanupPlan,
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
        }
        let mut finalizers = BTreeSet::new();
        for point in &plan.finalizers {
            if !finalizers.insert((point.block, point.instruction))
                || !matches!(
                    u.blocks
                        .get(point.block.index())
                        .and_then(|b| b.instructions.get(&u.instructions))
                        .and_then(|i| i.get(point.instruction as usize)),
                    Some(Inst::FinalizeBegin { .. })
                )
            {
                return false;
            }
        }
        for (block, row) in u.blocks.iter().enumerate() {
            let Some(instructions) = row.instructions.get(&u.instructions) else {
                return false;
            };
            for (instruction, inst) in instructions.iter().enumerate() {
                match inst {
                    Inst::FinalizeBegin { .. }
                        if !finalizers.contains(&(BlockId(block as u32), instruction as u32)) =>
                    {
                        return false;
                    }
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

/// C already proves the finalization lifecycle. Recheck that its actual call
/// is the descriptor's source body, borrowing exactly the started owner.
pub(crate) fn explicit_finalizer(
    unit: &LirUnit<Ready>,
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    point: InstructionPoint,
) -> Result<hirsel_syntax::ContentHash, String> {
    let block = unit
        .blocks
        .get(point.block.index())
        .ok_or("finalizer block")?;
    let Some(Inst::FinalizeBegin { address, .. }) = block
        .instructions
        .get(&unit.instructions)
        .and_then(|code| code.get(point.instruction as usize))
    else {
        return Err("finalizer begin".into());
    };
    let Some(Terminator::Call { call, .. }) = unit.terminators.get(point.block.index()) else {
        return Err("finalizer call".into());
    };
    let site = &unit.tables.calls[call.index()];
    let Callee::Direct(symbol) = site.callee else {
        return Err("indirect finalizer".into());
    };
    let target = &unit.tables.symbols[symbol.index()];
    let body = target.instance.ok_or("finalizer body identity")?;
    let layout = unit.types[unit.addresses[address.index()].ty.index()].layout;
    let source = FinalizerSource::derive(units, table, layout, body)?;
    if site.abi != source.abi
        || target.definition
            != table
                .layout(layout)
                .unwrap()
                .finalizer
                .as_ref()
                .unwrap()
                .definition
    {
        return Err("finalizer call ABI".into());
    }
    let args = site
        .arguments
        .get(&unit.pools.call_arguments)
        .ok_or("finalizer arguments")?;
    let [argument] = args else {
        return Err("finalizer arguments".into());
    };
    let ArgumentValue::Value(value) = argument.value else {
        return Err("finalizer self operand".into());
    };
    let Holding::Borrowed(loan) = unit.values[value.index()].holding else {
        return Err("finalizer self loan".into());
    };
    if argument.parameter != 0
        || argument.passing != hirsel_core_contract::Passing::Borrow
        || unit
            .tables
            .loans
            .get(loan.index())
            .is_none_or(|l| l.mutable || l.address != *address)
    {
        return Err("finalizer borrowed owner".into());
    }
    Ok(body)
}

/// An erased child cannot borrow its enclosing source proof. Every branch
/// of a typed cleanup graph must remain visible, including inactive variants.
pub(crate) fn known_cleanup(table: &LayoutTable, root: LayoutId) -> bool {
    let mut seen = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(record) = table.layout(id) else {
            return false;
        };
        if matches!(
            record.kind,
            LayoutKind::Callable { environment: None }
                | LayoutKind::Iterator { .. }
                | LayoutKind::Opaque { .. }
        ) {
            return false;
        }
        pending.extend(record.references());
    }
    true
}

/// A callback may own a temporary closure on a rejected nested invocation.
/// Prove its actual SSA source before treating its erased callable release as
/// mechanical. Any destructor or opaque child still needs retained custody.
pub(super) fn mechanical_callable_release(
    unit: &LirUnit<Ready>,
    units: &std::collections::BTreeMap<hirsel_syntax::ContentHash, &LirUnit<Ready>>,
    table: &LayoutTable,
    inst: &Inst,
) -> bool {
    let Inst::Release {
        value,
        layout,
        live: MaskRef::All,
        ..
    } = *inst
    else {
        return false;
    };
    if !table
        .layout(layout)
        .is_some_and(|r| matches!(r.kind, LayoutKind::Callable { .. }))
    {
        return false;
    }
    units.values().any(|body| {
        let Some(source) = target(unit, value, body, None, &mut BTreeSet::new()) else {
            return false;
        };
        source
            .symbol
            .instance
            .is_some_and(|id| units.contains_key(&id))
            && source.environment.is_none_or(|root| {
                known_cleanup(table, root) && !crate::validate::Layouts(table).destructible(root)
            })
    })
}
