//! Import validation. Every plan is recomputed here with this module's own
//! liveness, storage and cleanup walks; the producer's `crate::liveness` is
//! not consulted, so an error in either analysis shows up as a refusal.
use super::*;
use crate::ahead::{AheadPlan, MachineEvidence};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContinuationError {
    /// A suspending body has no plan, or a plan names a body that cannot suspend.
    Coverage(ContentHash),
    Body(ContentHash),
    Representation(ContentHash),
    States(ContentHash),
    Boundary(ContentHash, u32),
    LiveOwner(ContentHash, u32),
    Storage(ContentHash, u32),
    Loan(ContentHash, u32),
    RegionToken(ContentHash, u32),
    CleanupAnchor(ContentHash, u32),
    ResumeLabel(ContentHash, u32),
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
        for inst in u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
        {
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
        if b == at {
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

fn check_state(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    live: &Live,
    ordinal: u32,
    state: &ResumeState,
    expected: &ExistingBoundary,
    callee: Option<(ContentHash, u64)>,
    frame: &[LiveStorage],
    base: u64,
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
    let (slots, loans) = storage(u, b);
    let expected_storage: Vec<_> = frame
        .iter()
        .filter(|s| slots.contains(&s.slot))
        .cloned()
        .collect();
    if state.storage != expected_storage || expected_storage.len() != slots.len() {
        return Err(ContinuationError::Storage(id, n));
    }
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
    if state.bytes != end || state.bytes > MAX_FRAME_BYTES {
        return Err(ContinuationError::Frame(id));
    }
    Ok(())
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
    for (id, plan) in &plans.plans {
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
        let sites = super::sites(u, &effects, &plans.plans);
        match &plan.representation {
            Representation::Resumable => {
                let Ok(sites) = sites else {
                    return Err(ContinuationError::Representation(*id));
                };
                if sites.is_empty() || !u.parameters.is_empty() {
                    return Err(ContinuationError::Representation(*id));
                }
                if plan.states.len() != sites.len() {
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
                // Fixed storage: every slot live at some boundary, in slot order.
                let union: BTreeSet<SlotId> =
                    sites.iter().flat_map(|s| storage(u, s.block).0).collect();
                if plan.storage.iter().map(|s| s.slot).collect::<Vec<_>>()
                    != union.iter().copied().collect::<Vec<_>>()
                {
                    return Err(ContinuationError::Storage(*id, 0));
                }
                let mut end = FRAME_HEADER_BYTES;
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
                        &live,
                        i as u32,
                        state,
                        &boundary,
                        site.callee,
                        &plan.storage,
                        base,
                    )?;
                    if !state.regions.is_empty() {
                        return Err(ContinuationError::RegionToken(*id, i as u32));
                    }
                }
            }
            // A refused compact form is always safe.
            Representation::Stackful(_) => {
                if !plan.states.is_empty() || !plan.storage.is_empty() {
                    return Err(ContinuationError::States(*id));
                }
            }
            Representation::Direct => {
                if !matches!(sites, Ok(ref s) if s.is_empty()) || !plan.states.is_empty() {
                    return Err(ContinuationError::Representation(*id));
                }
            }
        }
        let captures = inputs.environments.get(id).copied().flatten();
        let frame = plan.states.iter().map(|s| s.bytes).max().unwrap_or(0);
        if plan.captures != captures
            || plan.cost.boundaries != plan.states.len() as u32
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
            &plan.storage,
            &plan.states,
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
        })
    }

    /// Independently check every field against the plan and the evidence.
    pub fn check(&self, plan: &AheadPlan, evidence: &MachineEvidence) -> Result<(), String> {
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
        Ok(())
    }
}
