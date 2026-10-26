//! Source anchors for the isolated invocation. This evidence does not grant
//! compact admission: entered runtime state, frame custody and the complete
//! reachable cleanup graph still require their physical continuation proofs.
use super::*;
use crate::stage::ValidatedUnit;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallPoint {
    pub block: BlockId,
    pub call: CallId,
    pub normal: Option<EdgeId>,
    pub failure: FailureId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstructionPoint {
    pub block: BlockId,
    pub instruction: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupPoint {
    pub at: InstructionPoint,
    pub anchor: CleanupAnchorId,
    pub failure: FailureId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeldResult {
    pub value: ValueId,
    pub slot: SlotId,
    pub layout: LayoutId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsolateProtocol {
    pub identity: IsolateCallback,
    pub begin: CallPoint,
    pub mark: ValueId,
    pub callback: CallPoint,
    pub admission: InstructionPoint,
    pub admission_failure: FailureId,
    pub pretransfer_failure: Option<FailureId>,
    pub caught_block: BlockId,
    pub caught: EdgeId,
    pub result: Option<HeldResult>,
    /// All instruction cleanup sites, including failure-only result disposal.
    /// An anchor alone never describes its descriptor or suspension protocol.
    pub cleanup: Vec<CleanupPoint>,
    pub ends: Vec<CallPoint>,
    pub terminals: Vec<BlockId>,
}

fn operation(unit: &LirUnit<Ready>, call: CallId, path: &str) -> bool {
    let Some(CallSite {
        callee: Callee::Operation(op),
        ..
    }) = unit.tables.calls.get(call.index())
    else {
        return false;
    };
    unit.tables
        .operations
        .get(op.index())
        .is_some_and(|op| op.definition.is_core(path))
}

fn call_point(unit: &LirUnit<Ready>, id: CallId) -> Option<CallPoint> {
    let mut found = None;
    for (block, term) in unit.terminators.iter().enumerate() {
        if let Terminator::Call {
            call,
            normal,
            failure,
        } = *term
            && call == id
        {
            if found.is_some() {
                return None;
            }
            found = Some(CallPoint {
                block: BlockId(block as u32),
                call,
                normal,
                failure,
            });
        }
    }
    found
}

fn instruction(unit: &LirUnit<Ready>, at: InstructionPoint) -> Option<&Inst> {
    unit.blocks
        .get(at.block.index())?
        .instructions
        .get(&unit.instructions)?
        .get(at.instruction as usize)
}

fn cleanup(inst: &Inst) -> Option<(CleanupAnchorId, FailureId)> {
    match *inst {
        Inst::Drop {
            anchor, failure, ..
        }
        | Inst::Release {
            anchor, failure, ..
        }
        | Inst::Reset {
            anchor, failure, ..
        } => Some((anchor, failure)),
        _ => None,
    }
}

impl IsolateProtocol {
    /// Derive anchors only from independently C-validated Ready units. Neither
    /// optimizer discovery nor a name/digest can substitute for those units.
    pub fn derive(
        caller: &ValidatedUnit<Ready>,
        invocation: CallId,
        wrapper: &ValidatedUnit<Ready>,
        callback: &ValidatedUnit<Ready>,
        environment: Option<LayoutId>,
        layouts: &LayoutTable,
    ) -> Option<Self> {
        let unit = wrapper.unit();
        let identity = IsolateCallback::prove(
            caller.unit(),
            invocation,
            unit,
            callback.unit(),
            environment,
            layouts,
        )?;
        let begins: Vec<_> = (0..unit.tables.calls.len())
            .map(|i| CallId(i as u32))
            .filter(|call| operation(unit, *call, "$intrinsic.IsolateBegin"))
            .collect();
        let &[begin] = begins.as_slice() else {
            return None;
        };
        let mark = unit.tables.calls[begin.index()].result?;
        let callback = call_point(unit, identity.callback)?;
        let mut admissions = Vec::new();
        let mut cleanups = Vec::new();
        let mut stores = Vec::new();
        let result = unit.tables.calls[identity.callback.index()].result;
        for (b, block) in unit.blocks.iter().enumerate() {
            for (i, inst) in block
                .instructions
                .get(&unit.instructions)?
                .iter()
                .enumerate()
            {
                let at = InstructionPoint {
                    block: BlockId(b as u32),
                    instruction: i as u32,
                };
                if let Inst::FrameEnter { failure } = *inst
                    && at.block == callback.block
                {
                    admissions.push((at, failure));
                }
                if let Some((anchor, failure)) = cleanup(inst) {
                    cleanups.push(CleanupPoint {
                        at,
                        anchor,
                        failure,
                    });
                }
                if let Inst::Store { address, value, .. } = *inst
                    && Some(value) == result
                {
                    let slot = whole(unit, address)?;
                    stores.push(HeldResult {
                        value,
                        slot,
                        layout: unit.types[unit.slots[slot.index()].ty.index()].layout,
                    });
                }
            }
        }
        let &[(admission, admission_failure)] = admissions.as_slice() else {
            return None;
        };
        let held = match (result, stores.as_slice()) {
            (None, []) => None,
            (Some(_), [held]) => Some(held.clone()),
            _ => return None,
        };
        let catches: Vec<_> = unit
            .terminators
            .iter()
            .enumerate()
            .filter_map(|(b, term)| {
                if let Terminator::Isolate { start, caught } = *term {
                    (start == mark).then_some((BlockId(b as u32), caught))
                } else {
                    None
                }
            })
            .collect();
        let &[(caught_block, caught)] = catches.as_slice() else {
            return None;
        };
        let ends = (0..unit.tables.calls.len())
            .map(|i| CallId(i as u32))
            .filter(|call| operation(unit, *call, "$intrinsic.IsolateEnd"))
            .map(|call| call_point(unit, call))
            .collect::<Option<Vec<_>>>()?;
        let terminals = unit
            .terminators
            .iter()
            .enumerate()
            .filter_map(|(b, term)| {
                matches!(term, Terminator::Return(_) | Terminator::Propagate)
                    .then_some(BlockId(b as u32))
            })
            .collect();
        Some(Self {
            begin: call_point(unit, begin)?,
            mark,
            callback,
            admission,
            admission_failure,
            pretransfer_failure: unit.tables.calls[identity.callback.index()].pretransfer_failure,
            identity,
            caught_block,
            caught,
            result: held,
            cleanup: cleanups,
            ends,
            terminals,
        })
    }

    /// Check supplied anchors against actual instructions and CFG edges. This
    /// checker does not invoke `derive` or trust a reconstructed plan digest.
    /// Ordinary C supplies ownership and once-only Begin/Catch/End checks;
    /// compact frame offsets/entered and cleanup state remain separate proofs.
    pub fn validate(
        &self,
        caller: &ValidatedUnit<Ready>,
        wrapper: &ValidatedUnit<Ready>,
        callback: &ValidatedUnit<Ready>,
        layouts: &LayoutTable,
    ) -> Result<(), &'static str> {
        let unit = wrapper.unit();
        if IsolateCallback::prove(
            caller.unit(),
            self.identity.invocation,
            unit,
            callback.unit(),
            self.identity.environment,
            layouts,
        )
        .as_ref()
            != Some(&self.identity)
        {
            return Err("isolate callback identity");
        }
        let point_matches = |point: &CallPoint| {
            matches!(unit.terminators.get(point.block.index()), Some(Terminator::Call { call, normal, failure })
                if *call == point.call && *normal == point.normal && *failure == point.failure)
        };
        if !point_matches(&self.begin)
            || !operation(unit, self.begin.call, "$intrinsic.IsolateBegin")
            || unit.tables.calls[self.begin.call.index()].result != Some(self.mark)
        {
            return Err("isolate begin provenance");
        }
        if !point_matches(&self.callback)
            || self.callback.call != self.identity.callback
            || self.pretransfer_failure
                != unit.tables.calls[self.callback.call.index()].pretransfer_failure
        {
            return Err("isolate callback edges");
        }
        if self.admission.block != self.callback.block
            || !matches!(instruction(unit, self.admission), Some(Inst::FrameEnter { failure }) if *failure == self.admission_failure)
        {
            return Err("isolate callback admission");
        }
        if !matches!(unit.terminators.get(self.caught_block.index()), Some(Terminator::Isolate { start, caught }) if *start == self.mark && *caught == self.caught)
        {
            return Err("isolate caught edge");
        }
        let result = unit.tables.calls[self.callback.call.index()].result;
        match (&self.result, result) {
            (None, None) => {}
            (Some(held), Some(value)) if held.value == value => {
                let slot = unit
                    .slots
                    .get(held.slot.index())
                    .ok_or("isolate result slot")?;
                if unit.types[slot.ty.index()].layout != held.layout
                    || !unit.instructions.iter().any(|inst| {
                        matches!(*inst, Inst::Store { address, value: stored, .. }
                        if stored == value && whole(unit, address) == Some(held.slot))
                    })
                {
                    return Err("isolate protected result");
                }
            }
            _ => return Err("isolate protected result"),
        }
        let mut cleanup_seen = BTreeSet::new();
        for site in &self.cleanup {
            if !cleanup_seen.insert((site.at.block, site.at.instruction))
                || instruction(unit, site.at).and_then(cleanup) != Some((site.anchor, site.failure))
            {
                return Err("isolate cleanup site");
            }
        }
        for (b, block) in unit.blocks.iter().enumerate() {
            for (i, inst) in block
                .instructions
                .get(&unit.instructions)
                .ok_or("isolate instruction range")?
                .iter()
                .enumerate()
            {
                if cleanup(inst).is_some() && !cleanup_seen.contains(&(BlockId(b as u32), i as u32))
                {
                    return Err("isolate missing cleanup");
                }
            }
        }
        let mut ends = BTreeSet::new();
        for point in &self.ends {
            if !ends.insert(point.call)
                || !point_matches(point)
                || !operation(unit, point.call, "$intrinsic.IsolateEnd")
            {
                return Err("isolate End site");
            }
        }
        let mut terminals = BTreeSet::new();
        for block in &self.terminals {
            if !terminals.insert(*block)
                || !matches!(
                    unit.terminators.get(block.index()),
                    Some(Terminator::Return(_) | Terminator::Propagate)
                )
            {
                return Err("isolate terminal");
            }
        }
        for (b, term) in unit.terminators.iter().enumerate() {
            match *term {
                Terminator::Call { call, .. }
                    if operation(unit, call, "$intrinsic.IsolateEnd") && !ends.contains(&call) =>
                {
                    return Err("isolate missing End");
                }
                Terminator::Return(_) | Terminator::Propagate
                    if !terminals.contains(&BlockId(b as u32)) =>
                {
                    return Err("isolate missing terminal");
                }
                _ => {}
            }
        }
        Ok(())
    }
}
