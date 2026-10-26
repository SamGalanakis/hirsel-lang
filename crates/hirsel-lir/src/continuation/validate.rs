//! Import validation. Every plan is recomputed here with this module's own
//! liveness, storage and cleanup walks; the producer's `crate::liveness` is
//! not consulted, so an error in either analysis shows up as a refusal.
use super::*;
use crate::ahead::{AheadPlan, MachineEvidence};
#[path = "validate_scope.rs"]
mod scope;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContinuationError {
    /// A suspending body has no plan, or a plan names a body that cannot suspend.
    Coverage(ContentHash),
    Body(ContentHash),
    Representation(ContentHash),
    States(ContentHash),
    Boundary(ContentHash, u32),
    LiveOwner(ContentHash, u32),
    /// A declared parameter's record (by parameter index).
    Parameter(ContentHash, u32),
    /// A region's frame record (by region index).
    RegionRecord(ContentHash, u32),
    Storage(ContentHash, u32),
    Loan(ContentHash, u32),
    RegionToken(ContentHash, u32),
    /// A constructor-tail hole is live across the boundary, as a value or in
    /// storage: no frame saves one (LANG-10, LIR schema revision 59).
    Hole(ContentHash, u32),
    CleanupAnchor(ContentHash, u32),
    ResumeLabel(ContentHash, u32),
    /// Missing, forged or currently unsupported entered-operation evidence.
    RuntimeOperation(ContentHash, u32),
    /// A nested callee's exclusive parameter without its call-edge proof, or
    /// with a proof this body does not establish (by state).
    Exclusive(ContentHash, u32),
    Frame(ContentHash),
    Certificate(ContentHash),
    Identity,
}

impl std::fmt::Display for ContinuationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid continuation plan: {self:?}")
    }
}

/// Backward value liveness, recomputed with sets.
struct Live {
    live_in: Vec<BTreeSet<ValueId>>,
}

fn params(u: &LirUnit<Ready>, b: BlockId) -> &[ValueId] {
    u.blocks[b.index()]
        .parameters
        .get(&u.pools.values)
        .unwrap_or(&[])
}

fn uses(u: &LirUnit<Ready>, inst: &Inst) -> Vec<ValueId> {
    let defs = inst.defs();
    let mut out = Vec::new();
    inst.visit(&mut |r| {
        if let Ref::Value(v) = r
            && !defs.contains(&v)
        {
            out.push(v);
        }
    });
    match *inst {
        Inst::Address { address, .. } => {
            let a = &u.addresses[address.index()];
            if let AddressBase::Value(v) = a.base {
                out.push(v);
            }
            for p in a.projection.get(&u.pools.projections).unwrap_or(&[]) {
                match *p {
                    ProjectionStep::Index(v) | ProjectionStep::Slot(v) => out.push(v),
                    ProjectionStep::FlatAt { index, offset } => out.extend([index, offset]),
                    _ => {}
                }
            }
        }
        Inst::Construct { construct, .. }
        | Inst::Reuse { construct, .. }
        | Inst::UniqueCopy { construct, .. }
        | Inst::FlatAppend { construct, .. }
        | Inst::FlatClose { construct, .. } => out.extend(
            u.tables.constructs[construct.index()]
                .fields
                .get(&u.pools.fields)
                .unwrap_or(&[])
                .iter()
                .map(|f| f.value),
        ),
        Inst::Closure { closure, .. } => out.extend(
            u.tables.closures[closure.index()]
                .captures
                .get(&u.pools.captures)
                .unwrap_or(&[])
                .iter()
                .map(|c| c.value),
        ),
        Inst::Release { live, .. } | Inst::Reset { live, .. } => {
            if let MaskRef::Dynamic(v) = live {
                out.push(v);
            }
        }
        _ => {}
    }
    out
}

fn call_values(u: &LirUnit<Ready>, call: CallId) -> (Vec<ValueId>, Vec<ValueId>) {
    let site = &u.tables.calls[call.index()];
    let mut defs: Vec<_> = site.result.into_iter().collect();
    let mut reads: Vec<_> = site.callee.value().into_iter().collect();
    for a in site.arguments.get(&u.pools.call_arguments).unwrap_or(&[]) {
        match a.value {
            ArgumentValue::Value(v) => reads.push(v),
            ArgumentValue::Address { version, .. } => defs.push(version),
        }
    }
    (defs, reads)
}

impl Live {
    fn edge(&self, u: &LirUnit<Ready>, e: EdgeId) -> BTreeSet<ValueId> {
        let edge = &u.tables.edges[e.index()];
        let mut out = self.live_in[edge.target.index()].clone();
        for p in params(u, edge.target) {
            out.remove(p);
        }
        out.extend(edge.arguments.get(&u.pools.values).unwrap_or(&[]));
        out
    }
    fn failure(&self, u: &LirUnit<Ready>, f: FailureId) -> BTreeSet<ValueId> {
        let failure = &u.tables.failures[f.index()];
        let mut out = self.edge(u, failure.cleanup);
        out.extend(failure.dispose.get(&u.pools.values).unwrap_or(&[]));
        out
    }
    /// Everything the terminator's successors need, before its own defs.
    fn after(&self, u: &LirUnit<Ready>, b: BlockId) -> BTreeSet<ValueId> {
        let term = &u.terminators[b.index()];
        let mut out = BTreeSet::new();
        for e in term.normal_edges(&u.pools) {
            out.extend(self.edge(u, e));
        }
        for f in term.failure_edges(&u.tables) {
            out.extend(self.failure(u, f));
        }
        out
    }
    fn compute(u: &LirUnit<Ready>) -> Self {
        let mut l = Live {
            live_in: vec![BTreeSet::new(); u.blocks.len()],
        };
        loop {
            let mut changed = false;
            for b in (0..u.blocks.len()).rev() {
                let block = BlockId(b as u32);
                let mut live = l.after(u, block);
                match u.terminators[b] {
                    Terminator::Call { call, .. } => {
                        let (defs, reads) = call_values(u, call);
                        for d in defs {
                            live.remove(&d);
                        }
                        live.extend(reads);
                    }
                    Terminator::Branch { condition: v, .. }
                    | Terminator::Switch { value: v, .. }
                    | Terminator::Return(Some(v)) => {
                        live.insert(v);
                    }
                    _ => {}
                }
                for inst in u.blocks[b]
                    .instructions
                    .get(&u.instructions)
                    .unwrap_or(&[])
                    .iter()
                    .rev()
                {
                    for d in inst.defs() {
                        live.remove(&d);
                    }
                    live.extend(uses(u, inst));
                    if let Some(f) = inst.failure() {
                        live.extend(l.failure(u, f));
                    }
                }
                for p in params(u, block) {
                    live.remove(p);
                }
                if live != l.live_in[b] {
                    l.live_in[b].extend(live);
                    changed = true;
                }
            }
            if !changed {
                return l;
            }
        }
    }
}

fn successors(u: &LirUnit<Ready>, b: BlockId) -> Vec<BlockId> {
    super::successors_of(u, b)
}

/// Storage and loans live on reaching `at`: a fixpoint over block states.
fn storage(u: &LirUnit<Ready>, at: BlockId) -> (BTreeSet<SlotId>, BTreeSet<LoanId>) {
    storage_before_instruction(u, at, usize::MAX)
}
fn storage_before_instruction(
    u: &LirUnit<Ready>,
    at: BlockId,
    before: usize,
) -> (BTreeSet<SlotId>, BTreeSet<LoanId>) {
    type State = (BTreeSet<SlotId>, BTreeSet<LoanId>);
    let mut states: BTreeMap<BlockId, State> = BTreeMap::new();
    let start: BTreeSet<SlotId> = (0..u.slots.len() as u32)
        .map(SlotId)
        .filter(|s| {
            matches!(
                u.slots[s.index()].kind,
                SlotKind::Parameter(_) | SlotKind::Capture(_) | SlotKind::ReturnSlot
            )
        })
        .collect();
    states.insert(u.entry, (start, BTreeSet::new()));
    let mut queue = std::collections::VecDeque::from([u.entry]);
    let mut at_state: State = Default::default();
    while let Some(b) = queue.pop_front() {
        let (mut slots, mut loans) = states[&b].clone();
        let mut leaving: Vec<(BlockId, State)> = Vec::new();
        for (i, inst) in u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            if b == at && i == before {
                at_state.0.extend(slots.iter().copied());
                at_state.1.extend(loans.iter().copied());
            }
            if let Some(f) = inst.failure() {
                let t = u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target;
                leaving.push((t, (slots.clone(), loans.clone())));
            }
            match *inst {
                Inst::StorageLive { slot } => {
                    slots.insert(slot);
                }
                Inst::StorageDead { slot, .. } => {
                    slots.remove(&slot);
                }
                Inst::BeginLoan { loan, .. } => {
                    loans.insert(loan);
                }
                Inst::EndLoan { loan, .. } => {
                    loans.remove(&loan);
                }
                _ => {}
            }
        }
        if b == at && before == usize::MAX {
            at_state.0.extend(slots.iter().copied());
            at_state.1.extend(loans.iter().copied());
        }
        let term = &u.terminators[b.index()];
        for e in term.normal_edges(&u.pools) {
            leaving.push((
                u.tables.edges[e.index()].target,
                (slots.clone(), loans.clone()),
            ));
        }
        for f in term.failure_edges(&u.tables) {
            let t = u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target;
            leaving.push((t, (slots.clone(), loans.clone())));
        }
        for (t, (s, l)) in leaving {
            match states.get_mut(&t) {
                None => {
                    states.insert(t, (s, l));
                    queue.push_back(t);
                }
                Some(old) => {
                    let size = (old.0.len(), old.1.len());
                    old.0.extend(s);
                    old.1.extend(l);
                    if size != (old.0.len(), old.1.len()) {
                        queue.push_back(t);
                    }
                }
            }
        }
    }
    at_state
}

fn anchors(u: &LirUnit<Ready>, entry: BlockId) -> Vec<CleanupAnchorId> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![entry];
    let mut out = BTreeSet::new();
    while let Some(b) = stack.pop() {
        if !seen.insert(b) {
            continue;
        }
        for inst in u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
        {
            if let Inst::Drop { anchor, .. }
            | Inst::Release { anchor, .. }
            | Inst::Reset { anchor, .. }
            | Inst::FinalizeBegin { anchor, .. }
            | Inst::FinalizeEnd { anchor, .. } = *inst
            {
                out.insert(anchor);
            }
        }
        stack.extend(successors(u, b));
    }
    out.into_iter().collect()
}

/// The slot an address finally names, through its chain of bases.
fn root_slot(u: &LirUnit<Ready>, address: AddressId) -> Option<SlotId> {
    let mut seen = BTreeSet::new();
    let mut a = address;
    while seen.insert(a) {
        match u.addresses.get(a.index())?.base {
            AddressBase::Slot(s) => return Some(s),
            AddressBase::Address(b) => a = b,
            AddressBase::Value(_) => return None,
        }
    }
    None
}

/// Recompute a nested call edge's exclusive-slot proof from the body: the
/// argument for the callee's exclusive parameter `record` must be the whole
/// place of one local slot whose layout is the parameter's non-owning scalar
/// layout, held at an aligned offset of this frame's storage while the call
/// runs, and the only active loan reaching that slot must be the argument's,
/// mutable. A direct call without a pretransfer gate only.
#[allow(clippy::too_many_arguments)]
fn exclusive_proof(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    block: BlockId,
    call: CallId,
    body: ContentHash,
    record: &ExclusiveParameter,
    slots: &BTreeSet<SlotId>,
    loans: &BTreeSet<LoanId>,
    frame: &[LiveStorage],
) -> Option<ExclusiveSlot> {
    let Terminator::Call {
        call: at,
        normal: Some(normal),
        failure,
    } = u.terminators[block.index()]
    else {
        return None;
    };
    let site = u.tables.calls.get(call.index())?;
    if at != call || site.pretransfer_failure.is_some() {
        return None;
    }
    let Callee::Direct(_) = site.callee else {
        return None;
    };
    let arguments = site.arguments.get(&u.pools.call_arguments)?;
    let passed: Vec<_> = arguments
        .iter()
        .filter(|a| a.parameter == record.source)
        .collect();
    let [argument] = passed[..] else {
        return None;
    };
    let ArgumentValue::Address { address, .. } = argument.value else {
        return None;
    };
    let place = u.addresses.get(address.index())?;
    let AddressBase::Slot(slot) = place.base else {
        return None;
    };
    let layout = table.layout(record.layout)?;
    let LayoutKind::Scalar(kind) = layout.kind else {
        return None;
    };
    let SizeSpec::Fixed(size) = layout.size else {
        return None;
    };
    let align = u64::from(layout.align.max(1));
    let info = u.slots.get(slot.index())?;
    if kind == ScalarKind::Pointer
        || !layout.scan.is_empty()
        || !layout.drop.is_empty()
        || layout.finalizer.is_some()
        || !matches!(u.types[info.ty.index()].repr, ValueRepr::Scalar(_))
        || place.projection.len != 0
        || !place.unchecked.is_empty()
        || !matches!(info.kind, SlotKind::Local | SlotKind::Temporary)
        || u.types[info.ty.index()].layout != record.layout
        || u.types[place.ty.index()].layout != record.layout
        || !slots.contains(&slot)
    {
        return None;
    }
    let stored: Vec<_> = frame.iter().filter(|s| s.slot == slot).collect();
    let [stored] = stored[..] else {
        return None;
    };
    if stored.layout != record.layout || stored.offset % align != 0 {
        return None;
    }
    let aliased = arguments.iter().any(|other| {
        other.parameter != record.source
            && matches!(other.value, ArgumentValue::Address { address, .. }
                if root_slot(u, address) == Some(slot))
    });
    if aliased {
        return None;
    }
    let active: Vec<LoanId> = loans
        .iter()
        .copied()
        .filter(|l| root_slot(u, u.tables.loans[l.index()].address) == Some(slot))
        .collect();
    let [loan] = active[..] else {
        return None;
    };
    let lent = &u.tables.loans[loan.index()];
    if !lent.mutable || lent.address != address {
        return None;
    }
    Some(ExclusiveSlot {
        callee: body,
        call,
        parameter: record.source,
        address,
        loan,
        slot,
        offset: stored.offset,
        layout: record.layout,
        size,
        align,
        normal,
        failure,
    })
}

#[allow(clippy::too_many_arguments)]
fn check_state(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    units: &BTreeMap<ContentHash, &LirUnit<Ready>>,
    live: &Live,
    ordinal: u32,
    state: &ResumeState,
    expected: &ExistingBoundary,
    callee: Option<(ContentHash, u64)>,
    exclusive: Option<&ExclusiveParameter>,
    frame: &[LiveStorage],
    base: u64,
    isolate: Option<(&isolate::IsolateProtocol, &ValidatedUnit<Ready>)>,
) -> Result<(), ContinuationError> {
    let id = u.header.instance;
    let n = ordinal + 1;
    if state.boundary != *expected {
        return Err(ContinuationError::Boundary(id, n));
    }
    let b = expected.block;
    let Terminator::Call {
        call,
        normal: Some(normal),
        failure,
    } = u.terminators[b.index()]
    else {
        return Err(ContinuationError::Boundary(id, n));
    };
    if call != expected.call {
        return Err(ContinuationError::Boundary(id, n));
    }
    // Live across the boundary: what successors need, minus what the call
    // defines, plus what it reads (it has not run while the frame is saved).
    let (defs, reads) = call_values(u, call);
    let mut values = live.after(u, b);
    for d in &defs {
        values.remove(d);
    }
    values.extend(reads);
    for a in u.tables.calls[call.index()]
        .arguments
        .get(&u.pools.call_arguments)
        .unwrap_or(&[])
    {
        if let ArgumentValue::Address { address, .. } = a.value
            && let AddressBase::Value(v) = u.addresses[address.index()].base
        {
            values.insert(v);
        }
    }
    values.extend(super::carried(u, b).1);
    let ty = |v: ValueId| &u.types[u.values[v.index()].ty.index()];
    if values.iter().any(|&v| ty(v).repr == ValueRepr::Hole) {
        return Err(ContinuationError::Hole(id, n));
    }
    // Owners: same set, each with its exact layout, representation and holding.
    let planned: Vec<_> = state.live.iter().map(|l| l.value).collect();
    if planned != values.iter().copied().collect::<Vec<_>>()
        || state.live.iter().any(|l| {
            l.layout != ty(l.value).layout
                || l.repr != ty(l.value).repr
                || l.holding != u.values[l.value.index()].holding
        })
    {
        return Err(ContinuationError::LiveOwner(id, n));
    }
    let (mut slots, loans) = storage(u, b);
    if slots
        .iter()
        .any(|s| u.types[u.slots[s.index()].ty.index()].repr == ValueRepr::Hole)
    {
        return Err(ContinuationError::Hole(id, n));
    }
    let proof = match (exclusive, callee) {
        (None, _) => None,
        (Some(record), Some((body, _))) => Some(
            exclusive_proof(u, table, b, call, body, record, &slots, &loans, frame)
                .ok_or(ContinuationError::Exclusive(id, n))?,
        ),
        (Some(_), None) => return Err(ContinuationError::Exclusive(id, n)),
    };
    if state.exclusive != proof {
        return Err(ContinuationError::Exclusive(id, n));
    }
    slots.retain(|s| !matches!(u.slots[s.index()].kind, SlotKind::Parameter(_)));
    let expected_storage: Vec<_> = frame
        .iter()
        .filter(|s| slots.contains(&s.slot))
        .cloned()
        .collect();
    if state.storage != expected_storage || expected_storage.len() != slots.len() {
        return Err(ContinuationError::Storage(id, n));
    }
    check_tail(u, table, b, call, state, &slots, &loans, frame)?;
    if state.loans != loans.into_iter().collect::<Vec<_>>() {
        return Err(ContinuationError::Loan(id, n));
    }
    let regions: Vec<_> = state
        .live
        .iter()
        .filter_map(|l| match ty(l.value).repr {
            ValueRepr::RegionMark => Some((l.value, l.layout, RegionKind::Mark)),
            ValueRepr::Arena => Some((l.value, l.layout, RegionKind::Arena)),
            _ => None,
        })
        .collect();
    if state
        .regions
        .iter()
        .map(|r| (r.value, r.layout, r.kind))
        .collect::<Vec<_>>()
        != regions
    {
        return Err(ContinuationError::RegionToken(id, n));
    }
    let entry = u.tables.edges[u.tables.failures[failure.index()].cleanup.index()].target;
    if state.cleanup.failure != failure
        || state.cleanup.entry != entry
        || state.cleanup.anchors != anchors(u, entry)
    {
        return Err(ContinuationError::CleanupAnchor(id, n));
    }
    if state.target.state != n
        || state.target.edge != normal
        || state.target.block != u.tables.edges[normal.index()].target
    {
        return Err(ContinuationError::ResumeLabel(id, n));
    }
    // Value offsets are increasing, aligned and disjoint, after the storage.
    let mut end = base;
    for l in &state.live {
        let Some((size, align)) = fixed(table, l.layout) else {
            return Err(ContinuationError::Frame(id));
        };
        if l.offset < end || l.offset % align != 0 {
            return Err(ContinuationError::Frame(id));
        }
        end = l.offset + size;
    }
    end = end.next_multiple_of(8);
    match (state.callee, callee) {
        (None, None) => {}
        (Some(nested), Some((body, bytes)))
            if nested.body == body
                && nested.bytes == bytes
                && nested.offset >= end
                && nested.offset % 16 == 0 =>
        {
            end = (nested.offset + nested.bytes).next_multiple_of(8);
        }
        _ => return Err(ContinuationError::Frame(id)),
    }
    let runtime_base = end;
    if let Some(runtime) = &state.runtime {
        for slot in &runtime.results {
            let Some((bytes, align)) = fixed(table, slot.layout) else {
                return Err(ContinuationError::Frame(id));
            };
            if slot.bytes != bytes
                || slot.align != align
                || slot.offset % align != 0
                || slot
                    .offset
                    .checked_add(bytes)
                    .is_none_or(|end| end > MAX_FRAME_BYTES)
            {
                return Err(ContinuationError::Frame(id));
            }
            match slot.source {
                RuntimeResultSource::CallResult(_) => {
                    if slot.offset < end {
                        return Err(ContinuationError::Frame(id));
                    }
                    end = (slot.offset + bytes).next_multiple_of(8);
                }
                RuntimeResultSource::ArgumentSlot { slot: place, .. } => {
                    if !state.storage.iter().any(|stored| {
                        stored.slot == place
                            && stored.layout == slot.layout
                            && stored.offset == slot.offset
                    }) {
                        return Err(ContinuationError::Frame(id));
                    }
                }
            }
        }
    }
    if state.bytes != end || state.bytes > MAX_FRAME_BYTES {
        return Err(ContinuationError::Frame(id));
    }
    if ordered_await::candidate(u) {
        if !ordered_await::check(u, table, frame, state) || runtime_base != end {
            return Err(ContinuationError::RuntimeOperation(id, n));
        }
    } else if let Some((protocol, wrapper)) = isolate {
        let runtime = state
            .runtime
            .as_ref()
            .ok_or(ContinuationError::RuntimeOperation(id, n))?;
        if expected.call != protocol.begin.call
            || state.callee.is_some()
            || state.tail.is_some()
            || !protocol.check_begin_operation(wrapper, table, runtime_base, runtime)
        {
            return Err(ContinuationError::RuntimeOperation(id, n));
        }
    } else if ordered::sleep(u, expected.call) {
        if !ordered::check_sleep(u, table, state, runtime_base) {
            return Err(ContinuationError::RuntimeOperation(id, n));
        }
    } else if scope::check(u, table, units, state, runtime_base)? != end {
        return Err(ContinuationError::RuntimeOperation(id, n));
    }
    // Check the action only after independently checking the nested frame,
    // so a forged frame retains its structural rejection.
    let action = if state.runtime.is_some() {
        ResumeAction::ResumeRuntime(expected.call)
    } else {
        match state.callee {
            Some(frame) => ResumeAction::ResumeNested(expected.call, frame),
            None if state.tail.is_some() => ResumeAction::AfterEnteredCall(expected.call, normal),
            None => ResumeAction::RetryUnenteredCall(expected.call),
        }
    };
    if state.action != action {
        return Err(ContinuationError::ResumeLabel(id, n));
    }
    Ok(())
}

/// Rebuild entered-tail custody from the Ready arguments and the independently
/// computed live slots/loans. The producer's transfer proof is not consulted.
#[allow(clippy::too_many_arguments)]
fn check_tail(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    block: BlockId,
    call: CallId,
    state: &ResumeState,
    slots: &BTreeSet<SlotId>,
    loans: &BTreeSet<LoanId>,
    frame: &[LiveStorage],
) -> Result<(), ContinuationError> {
    let refused = ContinuationError::LiveOwner(u.header.instance, state.target.state);
    let operation = match u.tables.calls[call.index()].callee {
        Callee::Operation(op) => core_name(&u.tables.operations[op.index()].definition),
        _ => None,
    };
    let kind = match operation.as_deref() {
        Some("$intrinsic.MutexPut") => Some(TailKind::Put),
        Some("$intrinsic.MutexKill") => Some(TailKind::Kill),
        Some("$intrinsic.MutexCause") => Some(TailKind::Cause),
        _ => None,
    };
    let known = core_name(&u.header.definition).as_deref() == Some("mutex.with")
        && u.tables
            .calls
            .iter()
            .any(|c| matches!(c.callee, Callee::Direct(_) | Callee::Bound { .. }))
        && u.tables
            .calls
            .iter()
            .all(|c| !matches!(c.callee, Callee::Indirect(_)));
    if !known || kind.is_none() {
        return if state.tail.is_none() {
            Ok(())
        } else {
            Err(refused)
        };
    }
    let kind = kind.unwrap();
    let proof = state.tail.as_ref().ok_or(refused.clone())?;
    let Terminator::Call {
        normal: Some(normal),
        failure,
        ..
    } = u.terminators[block.index()]
    else {
        return Err(refused);
    };
    if proof.kind != kind
        || proof.normal != normal
        || proof.failure != failure
        || state.callee.is_some()
    {
        return Err(refused);
    }
    // A callback failure cannot release payload/captures before the kill.
    for term in &u.terminators {
        let Terminator::Call { call, failure, .. } = *term else {
            continue;
        };
        if !matches!(
            u.tables.calls[call.index()].callee,
            Callee::Direct(_) | Callee::Bound { .. }
        ) {
            continue;
        }
        let mut todo =
            vec![u.tables.edges[u.tables.failures[failure.index()].cleanup.index()].target];
        let mut seen = BTreeSet::new();
        while let Some(b) = todo.pop() {
            if !seen.insert(b) {
                continue;
            }
            if u.blocks[b.index()]
                .instructions
                .get(&u.instructions)
                .unwrap_or(&[])
                .iter()
                .any(|i| {
                    matches!(
                        i,
                        Inst::Drop { .. } | Inst::Release { .. } | Inst::FinalizeBegin { .. }
                    )
                })
            {
                return Err(refused);
            }
            if let Terminator::Call { call, .. } = u.terminators[b.index()] {
                let Callee::Operation(op) = u.tables.calls[call.index()].callee else {
                    return Err(refused);
                };
                if core_name(&u.tables.operations[op.index()].definition).as_deref()
                    != Some("$intrinsic.MutexKill")
                {
                    return Err(refused);
                }
                continue;
            }
            let next = successors_of(u, b);
            if next.is_empty() {
                return Err(refused);
            }
            todo.extend(next);
        }
    }
    if kind != TailKind::Put {
        return if proof.carrier.is_none() {
            Ok(())
        } else {
            Err(refused)
        };
    }
    let carrier = proof.carrier.as_ref().ok_or(refused.clone())?;
    let args = u.tables.calls[call.index()]
        .arguments
        .get(&u.pools.call_arguments)
        .unwrap_or(&[]);
    let passed: Vec<_> = args.iter().filter(|a| a.parameter == 1).collect();
    let [arg] = passed[..] else {
        return Err(refused);
    };
    let ArgumentValue::Address { address, .. } = arg.value else {
        return Err(refused);
    };
    let at = &u.addresses[address.index()];
    let AddressBase::Slot(slot) = at.base else {
        return Err(refused);
    };
    let layout = u.types[u.slots[slot.index()].ty.index()].layout;
    let (size, align) = fixed(table, layout).ok_or(refused.clone())?;
    let stored: Vec<_> = frame.iter().filter(|s| s.slot == slot).collect();
    let [stored] = stored[..] else {
        return Err(refused);
    };
    let active: Vec<_> = loans
        .iter()
        .copied()
        .filter(|l| root_slot(u, u.tables.loans[l.index()].address) == Some(slot))
        .collect();
    let loan = match active.as_slice() {
        [] => None,
        [loan] => Some(*loan),
        _ => return Err(refused),
    };
    let expected = TailCarrier {
        slot,
        address,
        loan,
        layout,
        offset: stored.offset,
        size,
        align,
    };
    if *carrier != expected || !slots.contains(&slot) || stored.layout != layout || stored.offset % align != 0
        || at.projection.len != 0 || !at.unchecked.is_empty() || u.types[at.ty.index()].layout != layout
        || !matches!(u.slots[slot.index()].kind, SlotKind::Local | SlotKind::Temporary)
        || arg.passing != Passing::Mut || align > 16
        || loan.is_some_and(|loan| !u.tables.loans[loan.index()].mutable || u.tables.loans[loan.index()].address != address)
        || args.iter().any(|a| a.parameter != 1 && matches!(a.value, ArgumentValue::Address { address, .. } if root_slot(u, address) == Some(slot)))
    { return Err(refused); }
    Ok(())
}

/// Every declared parameter's record against the body's own ABI, then every
/// region's record, packed from the header on; where storage may start.
fn check_fixed(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    target: &TargetSpec,
    plan: &ContinuationPlan,
) -> Result<u64, ContinuationError> {
    let id = u.header.instance;
    let Some(abi) = table.fn_abis.get(u.header.abi.0 as usize) else {
        return Err(ContinuationError::Representation(id));
    };
    if abi.parameters.len() != u.parameters.len() || plan.parameters.len() != u.parameters.len() {
        return Err(ContinuationError::Parameter(
            id,
            plan.parameters.len() as u32,
        ));
    }
    let mut end = FRAME_HEADER_BYTES;
    let mut exclusive = 0;
    for (i, ((p, &value), record)) in abi
        .parameters
        .iter()
        .zip(&u.parameters)
        .zip(&plan.parameters)
        .enumerate()
    {
        let refused = ContinuationError::Parameter(id, i as u32);
        let slots: Vec<_> = (0..u.slots.len() as u32)
            .map(SlotId)
            .filter(|s| u.slots[s.index()].kind == SlotKind::Parameter(p.source))
            .collect();
        // Only a parameter received as bytes the frame can hold, or one whole
        // non-owning scalar `mut` place whose record is the caller's address
        // (its edges carry `ExclusiveSlot` proofs): never other caller
        // storage or a dynamic extent.
        let (size, align) = match p.passing {
            ParameterPassing::ByValue
            | ParameterPassing::ReadOnlyAddress
            | ParameterPassing::TransferAddress => fixed(table, p.layout).ok_or(refused.clone())?,
            ParameterPassing::ExclusiveAddress => {
                exclusive += 1;
                let scalar = table.layout(p.layout).is_some_and(|l| {
                    matches!(l.kind, LayoutKind::Scalar(k) if k != ScalarKind::Pointer)
                        && matches!(l.size, SizeSpec::Fixed(_))
                        && l.scan.is_empty()
                        && l.drop.is_empty()
                        && l.finalizer.is_none()
                });
                let received = &u.values[value.index()];
                let plain = matches!(u.types[received.ty.index()].repr, ValueRepr::Scalar(_))
                    && received.holding == Holding::Plain;
                if exclusive > 1
                    || p.convention != Passing::Mut
                    || !scalar
                    || !plain
                    || slots.len() != 1
                {
                    return Err(refused);
                }
                let pointer = u64::from(target.pointer_bytes);
                (pointer, pointer)
            }
            ParameterPassing::PointerLength => return Err(refused),
        };
        if record.value != value
            || record.source != p.source
            || record.convention != p.convention
            || record.passing != p.passing
            || record.holding != u.values[value.index()].holding
            || record.layout != p.layout
            || record.slot != slots.first().copied()
            || slots.len() > 1
            || record
                .slot
                .is_some_and(|s| u.types[u.slots[s.index()].ty.index()].layout != p.layout)
            || record.offset < end
            || record.offset % align != 0
        {
            return Err(refused);
        }
        end = record.offset + size;
    }
    end = end.next_multiple_of(8);
    if plan.records.len() != u.tables.regions.len() {
        return Err(ContinuationError::RegionRecord(
            id,
            plan.records.len() as u32,
        ));
    }
    for (r, record) in plan.records.iter().enumerate() {
        if record.region != RegionInstanceId(r as u32)
            || record.offset < end
            || record.offset % 8 != 0
        {
            return Err(ContinuationError::RegionRecord(id, r as u32));
        }
        end = record.offset + u.tables.regions[r].record_bytes();
    }
    Ok(end)
}

/// Recompute and check every plan against the complete Ready bodies.
pub fn validate(
    units: &[&ValidatedUnit<Ready>],
    inputs: &PlanInputs<'_>,
    plans: &ContinuationPlans,
) -> Result<(), ContinuationError> {
    let index: BTreeMap<_, _> = units
        .iter()
        .map(|u| (u.unit().header.instance, u.unit()))
        .collect();
    let digests: BTreeMap<_, _> = index.iter().map(|(k, u)| (*k, u.digest())).collect();
    // Suspension propagates from callees to callers over reverse call edges.
    let mut callers: BTreeMap<ContentHash, BTreeSet<ContentHash>> = BTreeMap::new();
    let mut suspends = BTreeSet::new();
    let none = BTreeSet::new();
    for (id, u) in &index {
        if !root_instruction_sites(u, inputs.table).is_empty() {
            suspends.insert(*id);
        }
        for call in &u.tables.calls {
            match call_effect(u, call, &index, &none, inputs.classify) {
                CallEffect::Local => {}
                CallEffect::Nested(callee) => {
                    callers.entry(callee).or_default().insert(*id);
                }
                CallEffect::Boundary(_) | CallEffect::Unknown(_) => {
                    suspends.insert(*id);
                }
            }
            if let Some(symbol) = callee_symbol(u, call)
                && let Some(callee) = symbol.instance
                && index.contains_key(&callee)
            {
                callers.entry(callee).or_default().insert(*id);
            }
        }
    }
    let mut work: Vec<_> = suspends.iter().copied().collect();
    while let Some(id) = work.pop() {
        for caller in callers.get(&id).into_iter().flatten() {
            if suspends.insert(*caller) {
                work.push(*caller);
            }
        }
    }
    if let Some(id) = suspends
        .symmetric_difference(&plans.plans.keys().copied().collect())
        .next()
    {
        return Err(ContinuationError::Coverage(*id));
    }
    // Callers before their nested callees: a caller's refusal to nest a
    // forged callee is reported at the caller, whatever the bodies' hashes.
    fn post(
        id: ContentHash,
        nested: &BTreeMap<ContentHash, Vec<ContentHash>>,
        seen: &mut BTreeSet<ContentHash>,
        out: &mut Vec<ContentHash>,
    ) {
        if !seen.insert(id) {
            return;
        }
        for callee in nested.get(&id).into_iter().flatten() {
            post(*callee, nested, seen, out);
        }
        out.push(id);
    }
    let nested: BTreeMap<ContentHash, Vec<ContentHash>> = plans
        .plans
        .keys()
        .map(|id| {
            let u = index[id];
            let callees = u
                .tables
                .calls
                .iter()
                .filter_map(
                    |c| match call_effect(u, c, &index, &suspends, inputs.classify) {
                        CallEffect::Nested(callee) if plans.plans.contains_key(&callee) => {
                            Some(callee)
                        }
                        _ => None,
                    },
                )
                .collect();
            (*id, callees)
        })
        .collect();
    let mut order = Vec::new();
    let mut seen = BTreeSet::new();
    for id in plans.plans.keys() {
        post(*id, &nested, &mut seen, &mut order);
    }
    for id in order.iter().rev() {
        let (id, plan) = (id, &plans.plans[id]);
        let u = index[id];
        if plan.body != *id || plan.digest != u.digest() {
            return Err(ContinuationError::Body(*id));
        }
        let effects: Vec<_> = u
            .tables
            .calls
            .iter()
            .map(|c| call_effect(u, c, &index, &suspends, inputs.classify))
            .collect();
        let sites = super::sites(u, inputs.table, &effects, &plans.plans);
        let cleanup_sites = root_instruction_sites(u, inputs.table);
        match &plan.representation {
            Representation::Resumable => {
                let Ok(sites) = sites else {
                    return Err(ContinuationError::Representation(*id));
                };
                if sites.is_empty() && cleanup_sites.is_empty() {
                    return Err(ContinuationError::Representation(*id));
                }
                if plan.states.len() != sites.len() || plan.cleanups.len() != cleanup_sites.len() {
                    return Err(ContinuationError::States(*id));
                }
                // No resume label may reach its own boundary again.
                for site in &sites {
                    let Terminator::Call {
                        normal: Some(normal),
                        ..
                    } = u.terminators[site.block.index()]
                    else {
                        return Err(ContinuationError::Representation(*id));
                    };
                    let mut seen = BTreeSet::new();
                    let mut stack = vec![u.tables.edges[normal.index()].target];
                    while let Some(b) = stack.pop() {
                        if b == site.block {
                            return Err(ContinuationError::Representation(*id));
                        }
                        if seen.insert(b) {
                            stack.extend(successors(u, b));
                        }
                    }
                }
                let mut end = check_fixed(u, inputs.table, inputs.target, plan)?;
                // Fixed storage: every slot live at some boundary, in slot
                // order, but a parameter's, which its record holds.
                let union: BTreeSet<SlotId> = sites
                    .iter()
                    .flat_map(|s| storage(u, s.block).0)
                    .chain(
                        cleanup_sites
                            .iter()
                            .flat_map(|(b, i)| storage_before_instruction(u, *b, *i).0),
                    )
                    .filter(|s| !matches!(u.slots[s.index()].kind, SlotKind::Parameter(_)))
                    .collect();
                if plan.storage.iter().map(|s| s.slot).collect::<Vec<_>>()
                    != union.iter().copied().collect::<Vec<_>>()
                {
                    return Err(ContinuationError::Storage(*id, 0));
                }
                for s in &plan.storage {
                    let ty = &u.types[u.slots[s.slot.index()].ty.index()];
                    let Some((size, align)) = fixed(inputs.table, s.layout) else {
                        return Err(ContinuationError::Frame(*id));
                    };
                    if s.layout != ty.layout
                        || s.repr != ty.repr
                        || s.offset < end
                        || s.offset % align != 0
                    {
                        return Err(ContinuationError::Storage(*id, 0));
                    }
                    end = s.offset + size;
                }
                let base = end.next_multiple_of(8);
                let live = Live::compute(u);
                for (i, (state, site)) in plan.states.iter().zip(&sites).enumerate() {
                    let boundary = ExistingBoundary {
                        block: site.block,
                        call: site.call,
                        operation: site.operation.clone(),
                    };
                    check_state(
                        u,
                        inputs.table,
                        &index,
                        &live,
                        i as u32,
                        state,
                        &boundary,
                        site.callee,
                        site.exclusive.as_ref(),
                        &plan.storage,
                        base,
                        None,
                    )?;
                }
                for (i, (state, &(block, instruction))) in
                    plan.cleanups.iter().zip(&cleanup_sites).enumerate()
                {
                    check_cleanup_instruction(
                        u,
                        inputs.table,
                        &live,
                        plan,
                        state,
                        block,
                        instruction,
                        plan.states.len() as u32 + i as u32 + 1,
                        base,
                    )?;
                }
            }
            // A refused compact form is always safe.
            Representation::Stackful(_) => {
                if !plan.states.is_empty()
                    || !plan.cleanups.is_empty()
                    || !plan.storage.is_empty()
                    || !plan.parameters.is_empty()
                    || !plan.records.is_empty()
                {
                    return Err(ContinuationError::States(*id));
                }
            }
            Representation::Direct => {
                if !matches!(sites, Ok(ref s) if s.is_empty())
                    || !cleanup_sites.is_empty()
                    || !plan.cleanups.is_empty()
                    || !plan.states.is_empty()
                    || !plan.parameters.is_empty()
                    || !plan.records.is_empty()
                {
                    return Err(ContinuationError::Representation(*id));
                }
            }
        }
        let captures = inputs.environments.get(id).copied().flatten();
        let frame = plan
            .states
            .iter()
            .map(|s| s.bytes)
            .chain(plan.cleanups.iter().map(|s| s.bytes))
            .max()
            .unwrap_or(0);
        if plan.captures != captures
            || plan.cost.boundaries != (plan.states.len() + plan.cleanups.len()) as u32
            || plan.cost.frame_bytes != frame
        {
            return Err(ContinuationError::Frame(*id));
        }
        let certificate = certify(
            u,
            inputs,
            &effects,
            &digests,
            &plan.representation,
            (&plan.parameters, &plan.records),
            &plan.storage,
            &plan.states,
            &plan.cleanups,
            plan.captures,
        );
        if certificate != plan.certificate {
            return Err(ContinuationError::Certificate(*id));
        }
    }
    if plans.identity != ContinuationPlans::identity_of(&plans.plans, inputs) {
        return Err(ContinuationError::Identity);
    }
    Ok(())
}

/// The exact final emitted frame graph of an ahead body.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FrameDag {
    /// (body, final frame bytes, direct callees), in body order.
    pub nodes: Vec<(ContentHash, u64, Vec<ContentHash>)>,
}

/// The active caller a private segment can return into: none for a scalar
/// spawn, the digest of the instrumented caller spines for a host return.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CallerProof {
    pub spines: Option<ContentHash>,
}

/// The selected original result descriptor of every host-return read:
/// (unit, call, descriptor, buffer layout, maximum).
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct ResultCleanupProof {
    pub reads: Vec<(ContentHash, CallId, LayoutId, LayoutId, u32)>,
}

/// Final-frame certificate of one ahead root, separate from the ordinary
/// entry's frames. Helpers are (startup, getter, handoff, depth-leaf) bytes.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AheadCertificate {
    pub root: ContentHash,
    pub plan: ContentHash,
    pub ordinary_symbol: String,
    pub ahead_symbol: String,
    pub final_frames: FrameDag,
    pub helpers: [u64; 4],
    pub max_stack_bytes: u32,
    pub caller: CallerProof,
    pub result: ResultCleanupProof,
    /// A host-return root whose body plans Resumable enters its resumable
    /// ahead code; a save continues in its ordinary dispatcher.
    pub resume: Option<ResumeProof>,
}

/// The resumable ahead entry of a root, bound to its ordinary plan: the
/// entry and dispatcher symbols, the frame both run over and that plan's
/// certificate. The frame DAG above already measures the entry's final frame.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ResumeProof {
    pub symbol: String,
    pub dispatcher: String,
    pub frame_bytes: u32,
    pub plan: ContentHash,
}

fn resume_proof(
    plan: &AheadPlan,
    plans: &ContinuationPlans,
    root: ContentHash,
) -> Option<ResumeProof> {
    let r = plan.roots.get(&root)?;
    r.host.as_ref()?;
    let symbol = plan.symbols.get(&root)?;
    let ordinary = plans
        .get(root)
        .filter(|p| p.representation == Representation::Resumable)?;
    Some(ResumeProof {
        symbol: crate::ahead::alternate_resume_name(&symbol.definition, root),
        dispatcher: super::resume_name(&symbol.definition, root, None),
        frame_bytes: u32::try_from(ordinary.cost.frame_bytes).ok()?,
        plan: ordinary.certificate.identity,
    })
}

fn spines_digest(plan: &AheadPlan, root: ContentHash) -> Option<ContentHash> {
    plan.roots[&root].host.as_ref().map(|h| {
        ContentHash::of_parts(
            "hirsel.continuation.ahead-callers.v1",
            [format!("{:?}", h.spines).as_bytes()],
        )
    })
}

fn reads(plan: &AheadPlan, root: ContentHash) -> ResultCleanupProof {
    ResultCleanupProof {
        reads: plan.roots[&root]
            .host
            .iter()
            .flat_map(|h| &h.reads)
            .map(|((unit, call), r)| (*unit, *call, r.descriptor, r.buffer_layout, r.maximum))
            .collect(),
    }
}

impl AheadCertificate {
    /// Derive from the plan and the final object's machine evidence.
    pub fn derive(
        plan: &AheadPlan,
        plans: &ContinuationPlans,
        root: ContentHash,
        evidence: &MachineEvidence,
        helpers: [u64; 4],
    ) -> Option<Self> {
        let r = plan.roots.get(&root)?;
        let symbol = plan.symbols.get(&root)?;
        let frames: BTreeMap<_, _> = evidence.frames.iter().copied().collect();
        let nodes = r
            .graph
            .bodies
            .iter()
            .map(|(id, body)| Some((*id, *frames.get(id)?, body.calls.iter().copied().collect())))
            .collect::<Option<Vec<_>>>()?;
        let bound = evidence.bounds.iter().find(|(id, _)| *id == root)?.1;
        Some(Self {
            root,
            plan: plan.identity,
            ordinary_symbol: crate::emission::symbol_name(&symbol.definition, Some(&root)),
            ahead_symbol: crate::ahead::alternate_name(&symbol.definition, root),
            final_frames: FrameDag { nodes },
            helpers,
            max_stack_bytes: u32::try_from(bound).ok()?,
            caller: CallerProof {
                spines: spines_digest(plan, root),
            },
            result: reads(plan, root),
            resume: resume_proof(plan, plans, root),
        })
    }

    /// Independently check every field against the plan and the evidence.
    pub fn check(
        &self,
        plan: &AheadPlan,
        plans: &ContinuationPlans,
        evidence: &MachineEvidence,
    ) -> Result<(), String> {
        let r = plan.roots.get(&self.root).ok_or("unknown ahead root")?;
        let symbol = plan.symbols.get(&self.root).ok_or("unknown ahead symbol")?;
        if self.plan != plan.identity || evidence.plan != plan.identity {
            return Err("plan identity".into());
        }
        let ordinary = crate::emission::symbol_name(&symbol.definition, Some(&self.root));
        let ahead = crate::ahead::alternate_name(&symbol.definition, self.root);
        if self.ordinary_symbol != ordinary
            || self.ahead_symbol != ahead
            || self.ordinary_symbol == self.ahead_symbol
        {
            return Err("ordinary/ahead symbols".into());
        }
        let measured: BTreeMap<_, _> = evidence.frames.iter().copied().collect();
        let mut frames = BTreeMap::new();
        if self.final_frames.nodes.len() != r.graph.bodies.len() {
            return Err("frame DAG bodies".into());
        }
        for (id, bytes, calls) in &self.final_frames.nodes {
            let body = r.graph.bodies.get(id).ok_or("frame DAG body")?;
            if measured.get(id) != Some(bytes)
                || calls.iter().copied().collect::<BTreeSet<_>>() != body.calls
            {
                return Err("frame DAG edge or extent".into());
            }
            frames.insert(*id, *bytes);
        }
        let [startup, getter, handoff, depth] = self.helpers;
        let bound = crate::ahead::stack_bound_with_depth(
            &r.graph, &frames, startup, getter, handoff, depth,
        )
        .ok_or("frame bound")?;
        if bound != u64::from(self.max_stack_bytes)
            || bound > crate::ahead::MAX_STACK_BYTES
            || !evidence.bounds.contains(&(self.root, bound))
        {
            return Err("final frame bound".into());
        }
        if self.caller.spines != spines_digest(plan, self.root) {
            return Err("caller proof".into());
        }
        if self.result != reads(plan, self.root) {
            return Err("selected result descriptor".into());
        }
        let expected = resume_proof(plan, plans, self.root);
        if self.resume != expected
            || self.resume.as_ref().is_some_and(|r| {
                u64::from(r.frame_bytes) > super::MAX_FRAME_BYTES
                    || r.symbol == self.ahead_symbol
                    || r.symbol == self.ordinary_symbol
                    || r.dispatcher == r.symbol
            })
        {
            return Err("resumable ahead entry".into());
        }
        Ok(())
    }
}

/// Recheck entered-runtime evidence at each native emitter's public entry.
/// Callers supply complete Ready content, including bodies that stay stackful,
/// because incoming callable provenance cannot be inferred from a plan hash.
pub fn validate_runtime_operations(
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    target: &TargetSpec,
    plans: &ContinuationPlans,
) -> Result<(), ContinuationError> {
    let index: BTreeMap<_, _> = units
        .iter()
        .map(|u| (u.unit().header.instance, u.unit()))
        .collect();
    for (id, plan) in &plans.plans {
        if !plan.states.iter().any(|s| s.runtime.is_some()) {
            continue;
        }
        let u = index.get(id).ok_or(ContinuationError::Body(*id))?;
        if plan.digest != u.digest() || plan.body != *id {
            return Err(ContinuationError::Body(*id));
        }
        // The dispatcher allocates from this claimed extent, not from any
        // individual state's size. Reject a forged allocation before emission
        // can materialize owners or pass a runtime helper its frame address.
        let frame = plan
            .states
            .iter()
            .map(|state| state.bytes)
            .chain(plan.cleanups.iter().map(|state| state.bytes))
            .max()
            .unwrap_or(0);
        if frame > MAX_FRAME_BYTES || plan.cost.frame_bytes != frame {
            return Err(ContinuationError::Frame(*id));
        }
        let mut end = check_fixed(u, table, target, plan)?;
        for slot in &plan.storage {
            let row = u
                .slots
                .get(slot.slot.index())
                .ok_or(ContinuationError::Storage(*id, 0))?;
            let ty = &u.types[row.ty.index()];
            let (bytes, align) = fixed(table, slot.layout).ok_or(ContinuationError::Frame(*id))?;
            if slot.layout != ty.layout
                || slot.repr != ty.repr
                || slot.offset < end
                || slot.offset % align != 0
            {
                return Err(ContinuationError::Storage(*id, 0));
            }
            end = slot
                .offset
                .checked_add(bytes)
                .filter(|end| *end <= MAX_FRAME_BYTES)
                .ok_or(ContinuationError::Frame(*id))?;
        }
        let base = end.next_multiple_of(8);
        let live = Live::compute(u);
        for (ordinal, state) in plan
            .states
            .iter()
            .enumerate()
            .filter(|(_, s)| s.runtime.is_some())
        {
            let Some(Terminator::Call { call, .. }) =
                u.terminators.get(state.boundary.block.index())
            else {
                return Err(ContinuationError::Boundary(*id, ordinal as u32 + 1));
            };
            let call = *call;
            let site = &u.tables.calls[call.index()];
            let Callee::Operation(op) = site.callee else {
                return Err(ContinuationError::RuntimeOperation(*id, ordinal as u32 + 1));
            };
            let expected = ExistingBoundary {
                block: state.boundary.block,
                call,
                operation: u.tables.operations[op.index()].definition.clone(),
            };
            check_state(
                u,
                table,
                &index,
                &live,
                ordinal as u32,
                state,
                &expected,
                None,
                None,
                &plan.storage,
                base,
                None,
            )?;
        }
    }
    Ok(())
}

/// Identify releases from Ready instruction operands, independent of producer
/// flags and metadata. A call coordinate is never accepted here.
fn root_instruction_sites(u: &LirUnit<Ready>, table: &LayoutTable) -> Vec<(BlockId, usize)> {
    let mut sites = Vec::new();
    for (b, block) in u.blocks.iter().enumerate() {
        for (i, inst) in block
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            if let Inst::Release {
                value,
                layout,
                live: MaskRef::All,
                ..
            } = inst
                && (table
                    .layout(u.types[u.values[value.index()].ty.index()].layout)
                    .is_some_and(|r| {
                        matches!(
                            r.kind,
                            LayoutKind::Shared { .. }
                                | LayoutKind::Block { .. }
                                | LayoutKind::Box { .. }
                        )
                    })
                    || table.layout(*layout).is_some_and(|r| {
                        matches!(r.kind, LayoutKind::Resource | LayoutKind::Handle)
                            && r.size.storage().is_some()
                    }))
            {
                if !instruction_reaches_close(table, *layout) {
                    continue;
                }
                sites.push((BlockId(b as u32), i));
            }
        }
    }
    sites
}

#[allow(clippy::too_many_arguments)]
fn check_cleanup_instruction(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    live: &Live,
    plan: &ContinuationPlan,
    state: &CleanupResumeState,
    block: BlockId,
    instruction: usize,
    number: u32,
    base: u64,
) -> Result<(), ContinuationError> {
    let id = u.header.instance;
    let bad = ContinuationError::LiveOwner(id, number);
    let inst = &u.blocks[block.index()]
        .instructions
        .get(&u.instructions)
        .unwrap_or(&[])[instruction];
    let Inst::Release {
        value,
        layout,
        live: MaskRef::All,
        anchor,
        failure,
    } = *inst
    else {
        return Err(bad);
    };
    if state.block != block
        || state.instruction != instruction as u32
        || state.value != value
        || state.layout != layout
        || state.anchor != anchor
        || state.failure != failure
        || state.state != number
        || state.action
            != (ResumeAction::ResumeCleanup {
                block,
                instruction: instruction as u32,
                anchor,
            })
        || u.values[value.index()].holding != Holding::Owned
    {
        return Err(ContinuationError::Boundary(id, number));
    }
    let mut seen = BTreeSet::new();
    let mut todo = successors(u, block);
    while let Some(b) = todo.pop() {
        if b == block {
            return Err(ContinuationError::Representation(id));
        }
        if seen.insert(b) {
            todo.extend(successors(u, b));
        }
    }
    let (mut slots, loans) = storage_before_instruction(u, block, instruction);
    if !loans.is_empty() {
        return Err(ContinuationError::Loan(id, number));
    }
    slots.retain(|s| !matches!(u.slots[s.index()].kind, SlotKind::Parameter(_)));
    let retained: Vec<_> = plan
        .storage
        .iter()
        .filter(|s| slots.contains(&s.slot))
        .cloned()
        .collect();
    if state.storage != retained || retained.len() != slots.len() {
        return Err(ContinuationError::Storage(id, number));
    }
    let mut values = live.after(u, block);
    match u.terminators[block.index()] {
        Terminator::Call { call, .. } => {
            let (defs, reads) = call_values(u, call);
            for d in defs {
                values.remove(&d);
            }
            values.extend(reads);
        }
        Terminator::Branch { condition: v, .. }
        | Terminator::Switch { value: v, .. }
        | Terminator::Return(Some(v)) => {
            values.insert(v);
        }
        _ => {}
    }
    for inst in u.blocks[block.index()]
        .instructions
        .get(&u.instructions)
        .unwrap_or(&[])[instruction..]
        .iter()
        .rev()
    {
        for d in inst.defs() {
            values.remove(&d);
        }
        values.extend(uses(u, inst));
        if let Some(f) = inst.failure() {
            values.extend(live.failure(u, f));
        }
    }
    values.extend(super::carried_from(u, block, Some(instruction)).1);
    if state.live.iter().map(|v| v.value).collect::<Vec<_>>()
        != values.iter().copied().collect::<Vec<_>>()
    {
        return Err(bad);
    }
    let mut end = base;
    for retained in &state.live {
        let ty = &u.types[u.values[retained.value.index()].ty.index()];
        let holding = u.values[retained.value.index()].holding;
        let Some((size, align)) = fixed(table, ty.layout) else {
            return Err(bad);
        };
        if retained.layout != ty.layout
            || retained.repr != ty.repr
            || retained.holding != holding
            || align > 16
            || retained.offset < end
            || retained.offset % align != 0
            || matches!(holding, Holding::Borrowed(_))
            || matches!(
                ty.repr,
                ValueRepr::Hole | ValueRepr::Token | ValueRepr::Uncounted
            )
        {
            return Err(bad);
        }
        end = retained.offset.checked_add(size).ok_or(bad.clone())?;
    }
    if state.bytes != end.next_multiple_of(8) || state.bytes > MAX_FRAME_BYTES {
        return Err(ContinuationError::Frame(id));
    }
    Ok(())
}

// Independently traverse the descriptor graph before admitting an instruction.
fn instruction_reaches_close(table: &LayoutTable, layout: LayoutId) -> bool {
    use crate::layout_types::DropStep;
    let mut pending = vec![layout];
    let mut visited = BTreeSet::new();
    while let Some(layout) = pending.pop() {
        if !visited.insert(layout) {
            continue;
        }
        let Some(record) = table.layout(layout) else {
            return false;
        };
        let mut programs = vec![record.drop.as_slice()];
        while let Some(steps) = programs.pop() {
            for step in steps {
                match step {
                    DropStep::CloseHandle { .. } => return true,
                    DropStep::ActiveVariant { variants, .. } => {
                        programs.extend(variants.iter().map(|(_, steps)| steps.as_slice()))
                    }
                    _ => {}
                }
            }
        }
        match record.kind {
            LayoutKind::Shared { payload }
            | LayoutKind::Block { payload }
            | LayoutKind::Box { payload } => pending.push(payload),
            LayoutKind::Frozen { value } => pending.push(value),
            LayoutKind::Inline { element, .. }
            | LayoutKind::Sequence { element }
            | LayoutKind::Sequential { element }
            | LayoutKind::SoaColumns { element }
            | LayoutKind::Slice { element }
            | LayoutKind::Iterator { element } => pending.push(element),
            LayoutKind::Callable {
                environment: Some(layout),
            } => pending.push(layout),
            _ => {}
        }
        pending.extend(record.fields.iter().map(|f| f.layout));
        pending.extend(
            record
                .tag
                .iter()
                .flat_map(|t| &t.variants)
                .flat_map(|v| &v.fields)
                .map(|f| f.layout),
        );
    }
    false
}

#[cfg(test)]
mod region_width_tests {
    use super::*;
    use crate::abi_types::{CallMultiplicity, CallableShape, FnAbi, FnAbiId, FnAbiKey};
    use crate::objects::UnitKey;
    use crate::stage::StageKind;

    #[test]
    fn demanded_region_frame_widths_keep_mixed_records_disjoint_and_refuse_overlap() {
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let digest = ContentHash([1; 32]);
        let mut table = LayoutTable::new(&target);
        table.fn_abis.push(FnAbi {
            key: FnAbiKey {
                callable: digest,
                abi_version: crate::ABI_VERSION,
                receipt_contract: None,
            },
            shape: CallableShape::Definition,
            multiplicity: CallMultiplicity::Many,
            context: true,
            environment: false,
            caller_site: false,
            destination: false,
            result: None,
            diverges: false,
            result_arena: None,
            parameters: vec![],
            version: crate::ABI_VERSION,
            nonretaining: vec![],
            nonretaining_captures: vec![],
            scoped_contract: crate::abi::scoped_contract_digest(&[], &[]),
            scoped_entries: vec![],
            receipts: None,
        });
        let mut unit = LirUnit::<Ready>::new(UnitHeader {
            source_count: 0,
            key: UnitKey(digest),
            track_caller: false,
            stage: StageKind::Ready,
            lir_version: crate::LIR_VERSION,
            origin: UnitOrigin::Source,
            definition: DefPath::core("mixed_region_width"),
            implementation_revision: digest,
            instance: digest,
            target: table.target,
            abi: FnAbiId(0),
        });
        unit.tables.regions = [true, false, true]
            .into_iter()
            .map(|seal_demand| RegionInstance {
                scope: ScopeId(0),
                anchor: SourceAnchorId(0),
                seal_demand,
            })
            .collect();
        let (parameters, records, end) = super::super::fixed_area(&unit, &table, &target).unwrap();
        assert_eq!(
            records.iter().map(|r| r.offset).collect::<Vec<_>>(),
            [8, 88, 152]
        );
        assert_eq!(end, 232);
        let mut plan = ContinuationPlan {
            body: digest,
            digest,
            representation: Representation::Resumable,
            parameters,
            records,
            storage: vec![],
            states: vec![],
            cleanups: vec![],
            captures: None,
            cost: PlacementCost::default(),
            certificate: ValidatedContinuation { identity: digest },
        };
        assert_eq!(check_fixed(&unit, &table, &target, &plan).unwrap(), 232);
        plan.records[1].offset = 72;
        assert!(matches!(
            check_fixed(&unit, &table, &target, &plan),
            Err(ContinuationError::RegionRecord(_, 1))
        ));
    }
}

/// Physical checker for an inactive isolate proposal. Ordinary plans continue
/// to refuse the exception until source/emitter admission is qualified.
pub(super) fn isolate_frame(
    units: &[&ValidatedUnit<Ready>],
    inputs: &PlanInputs<'_>,
    ordinary: &ContinuationPlans,
    protocol: &isolate::IsolateProtocol,
    plan: &ContinuationPlan,
) -> Result<(), ContinuationError> {
    let id = protocol.identity.wrapper;
    let wrapper = units
        .iter()
        .copied()
        .find(|u| u.unit().header.instance == id)
        .ok_or(ContinuationError::Body(id))?;
    let u = wrapper.unit();
    let index: BTreeMap<_, _> = units
        .iter()
        .map(|u| (u.unit().header.instance, u.unit()))
        .collect();
    if plan.body != id || plan.digest != u.digest() {
        return Err(ContinuationError::Body(id));
    }
    if plan.representation != Representation::Resumable
        || !plan.cleanups.is_empty()
        || !root_instruction_sites(u, inputs.table).is_empty()
    {
        return Err(ContinuationError::Representation(id));
    }
    let suspends = ordinary.plans.keys().copied().collect();
    let effects: Vec<_> = u
        .tables
        .calls
        .iter()
        .enumerate()
        .map(|(id, call)| {
            if CallId(id as u32) == protocol.begin.call {
                if let Callee::Operation(op) = call.callee
                    && u.tables.operations[op.index()]
                        .definition
                        .is_core("$intrinsic.IsolateBegin")
                {
                    return CallEffect::Boundary(
                        u.tables.operations[op.index()].definition.clone(),
                    );
                }
                return CallEffect::Unknown("wrong isolate begin".into());
            }
            if let Callee::Operation(op) = call.callee {
                let definition = &u.tables.operations[op.index()].definition;
                if [
                    "$intrinsic.IsolateEnd",
                    "$intrinsic.IsolateCount",
                    "$intrinsic.IsolateKind",
                    "$intrinsic.IsolateRule",
                    "$intrinsic.IsolateDefinition",
                    "$intrinsic.IsolateLine",
                    "$intrinsic.IsolateColumn",
                    "$intrinsic.IsolateSpawn",
                ]
                .iter()
                .any(|path| definition.is_core(path))
                {
                    return CallEffect::Local;
                }
            }
            call_effect(u, call, &index, &suspends, inputs.classify)
        })
        .collect();
    // Enumerate actual terminators rather than using the producer's sites.
    let mut boundaries = Vec::new();
    for (b, term) in u.terminators.iter().enumerate() {
        let Terminator::Call {
            call,
            normal: Some(normal),
            ..
        } = *term
        else {
            continue;
        };
        let (operation, callee) = match &effects[call.index()] {
            CallEffect::Local => continue,
            CallEffect::Unknown(_) => return Err(ContinuationError::Representation(id)),
            CallEffect::Boundary(op) => (op.clone(), None),
            CallEffect::Nested(body) => {
                let nested = ordinary
                    .get(*body)
                    .filter(|p| p.representation == Representation::Resumable)
                    .ok_or(ContinuationError::Representation(id))?;
                let abi = &inputs.table.fn_abis[u.tables.calls[call.index()].abi.0 as usize];
                if abi
                    .parameters
                    .iter()
                    .any(|p| p.passing == ParameterPassing::ExclusiveAddress)
                {
                    return Err(ContinuationError::Exclusive(
                        id,
                        boundaries.len() as u32 + 1,
                    ));
                }
                (
                    callee_symbol(u, &u.tables.calls[call.index()])
                        .unwrap()
                        .definition,
                    Some((*body, nested.cost.frame_bytes)),
                )
            }
        };
        let block = BlockId(b as u32);
        let mut seen = BTreeSet::new();
        let mut pending = vec![u.tables.edges[normal.index()].target];
        while let Some(at) = pending.pop() {
            if at == block {
                return Err(ContinuationError::Representation(id));
            }
            if seen.insert(at) {
                pending.extend(successors(u, at));
            }
        }
        boundaries.push((
            ExistingBoundary {
                block,
                call,
                operation,
            },
            callee,
        ));
    }
    if boundaries.len() != plan.states.len() || boundaries.is_empty() {
        return Err(ContinuationError::States(id));
    }
    let mut end = check_fixed(u, inputs.table, inputs.target, plan)?;
    let slots: BTreeSet<_> = boundaries
        .iter()
        .flat_map(|(site, _)| storage(u, site.block).0)
        .filter(|s| !matches!(u.slots[s.index()].kind, SlotKind::Parameter(_)))
        .collect();
    if plan.storage.iter().map(|s| s.slot).collect::<Vec<_>>()
        != slots.iter().copied().collect::<Vec<_>>()
    {
        return Err(ContinuationError::Storage(id, 0));
    }
    for slot in &plan.storage {
        let ty = &u.types[u.slots[slot.slot.index()].ty.index()];
        let (bytes, align) =
            fixed(inputs.table, slot.layout).ok_or(ContinuationError::Frame(id))?;
        if slot.layout != ty.layout
            || slot.repr != ty.repr
            || slot.offset < end
            || slot.offset % align != 0
        {
            return Err(ContinuationError::Storage(id, 0));
        }
        end = slot
            .offset
            .checked_add(bytes)
            .filter(|e| *e <= MAX_FRAME_BYTES)
            .ok_or(ContinuationError::Frame(id))?;
    }
    let base = end.next_multiple_of(8);
    let live = Live::compute(u);
    let mut begins = 0;
    for (ordinal, (state, (site, callee))) in plan.states.iter().zip(&boundaries).enumerate() {
        let begin = site.call == protocol.begin.call;
        begins += usize::from(begin);
        check_state(
            u,
            inputs.table,
            &index,
            &live,
            ordinal as u32,
            state,
            site,
            *callee,
            None,
            &plan.storage,
            base,
            begin.then_some((protocol, wrapper)),
        )?;
    }
    let frame = plan.states.iter().map(|s| s.bytes).max().unwrap_or(0);
    if begins != 1
        || plan.cost.frame_bytes != frame
        || frame > MAX_FRAME_BYTES
        || plan.cost.boundaries as usize != plan.states.len()
        || plan.captures != inputs.environments.get(&id).copied().flatten()
    {
        return Err(ContinuationError::Frame(id));
    }
    let digests = index.iter().map(|(k, u)| (*k, u.digest())).collect();
    let certificate = certify(
        u,
        inputs,
        &effects,
        &digests,
        &plan.representation,
        (&plan.parameters, &plan.records),
        &plan.storage,
        &plan.states,
        &plan.cleanups,
        plan.captures,
    );
    if plan.certificate != certificate {
        return Err(ContinuationError::Certificate(id));
    }
    Ok(())
}
