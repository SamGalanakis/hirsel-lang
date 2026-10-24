//! Compiler-owned continuation plans (r6 S7): emission metadata derived from
//! validated Ready bodies and layouts, never a new LIR construct.
//!
//! A body whose calls are all classified is *closed*. A closed body with at
//! least one existing operation boundary gets one [`ResumeState`] per boundary:
//! the values and storage live across it, its region tokens, the cleanup that a
//! stop or cancellation runs from it and the label execution resumes at. A body
//! reaching an unknown call, or suspending inside a nested frame, keeps the
//! general stackful representation. Both use the same Saved/Active ownership
//! protocol in the runtime; the representation never changes semantics.
//!
//! Cost classes only choose representation and placement. Like Rayon's join
//! (`rayon-core/src/join/mod.rs:132-185`), a local first execution must have
//! materialized everything a stolen continuation needs before it can wait.
//!
//! [`validate`] recomputes every plan with its own analyses: a plan imported
//! from a cache or another partition is never trusted by its identity alone.
use std::collections::{BTreeMap, BTreeSet};

use hirsel_syntax::{ContentHash, DefPath};

use crate::ir::*;
use crate::layout_types::{LayoutId, LayoutTable, SizeSpec, TargetSpec};
use crate::stage::{Ready, ValidatedUnit};

mod validate;
pub use validate::{
    AheadCertificate, CallerProof, ContinuationError, FrameDag, ResultCleanupProof, validate,
};

/// The continuation alternates of a resumable body: its dispatcher (`None`),
/// which enters the state its frame header names, or one state's entry.
pub fn resume_name(definition: &DefPath, instance: ContentHash, state: Option<u32>) -> String {
    let base = crate::emission::symbol_name(definition, Some(&instance));
    match state {
        None => format!("{base}_resume"),
        Some(k) => format!("{base}_resume{k}"),
    }
}

/// The address preparations state `state` (1-based) re-runs before reissuing
/// its boundary: each `Inst::Address` in a block that state does not emit
/// (`skipped`) whose root base is frame-resident storage or a reloaded live
/// value, bases first. Their checks passed before the suspension and read
/// only state the frame retains, so re-running them observes nothing.
pub fn resume_preparations(
    u: &LirUnit<Ready>,
    plan: &ContinuationPlan,
    state: u32,
    skipped: &[bool],
) -> Vec<(BlockId, usize)> {
    let Some(resume) = state.checked_sub(1).and_then(|i| plan.states.get(i as usize)) else {
        return Vec::new();
    };
    let mut at = BTreeMap::new();
    for (b, block) in u.blocks.iter().enumerate() {
        if !skipped.get(b).copied().unwrap_or(false) {
            continue;
        }
        for (i, inst) in block.instructions.get(&u.instructions).unwrap_or(&[]).iter().enumerate() {
            if let Inst::Address { address, .. } = *inst {
                at.insert(address, (BlockId(b as u32), i));
            }
        }
    }
    fn root(u: &LirUnit<Ready>, a: AddressId) -> AddressBase {
        match u.addresses[a.index()].base {
            AddressBase::Address(b) => root(u, b),
            base => base,
        }
    }
    let eligible = |a: AddressId| match root(u, a) {
        AddressBase::Slot(s) => plan.storage.iter().any(|f| f.slot == s),
        AddressBase::Value(v) => resume.live.iter().any(|l| l.value == v),
        AddressBase::Address(_) => false,
    };
    fn visit(
        u: &LirUnit<Ready>,
        a: AddressId,
        at: &BTreeMap<AddressId, (BlockId, usize)>,
        done: &mut BTreeSet<AddressId>,
        out: &mut Vec<(BlockId, usize)>,
    ) {
        if !done.insert(a) {
            return;
        }
        if let AddressBase::Address(b) = u.addresses[a.index()].base
            && at.contains_key(&b)
        {
            visit(u, b, at, done, out);
        }
        out.push(at[&a]);
    }
    let mut done = BTreeSet::new();
    let mut out = Vec::new();
    for &a in at.keys() {
        if eligible(a) {
            visit(u, a, &at, &mut done, &mut out);
        }
    }
    out
}

/// Fixed header of a resumable frame: the resume state and a reserved word.
pub const FRAME_HEADER_BYTES: u64 = 8;
/// Larger live images stay stackful: the frame must stay compact.
pub const MAX_FRAME_BYTES: u64 = 4096;

/// What a call does to its caller's continuation, as the runtime's reviewed
/// operation descriptors classify it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CalleeClass {
    /// Completes without a scheduling point.
    Local,
    /// An existing operation boundary: a scheduling point may follow.
    Boundary,
    /// No classification: only the general stackful form is valid.
    Unknown,
}

#[derive(Clone, Copy)]
pub struct PlanInputs<'a> {
    pub table: &'a LayoutTable,
    pub target: &'a TargetSpec,
    /// Environment layout of each known code (`ahead::environments`).
    pub environments: &'a BTreeMap<ContentHash, Option<LayoutId>>,
    /// Exact runtime/helper bytes the resume protocol is bound to.
    pub runtime: ContentHash,
    /// The reviewed classification of a runtime operation; None defers to
    /// the callee's Ready body, and a callee with neither is Unknown.
    pub classify: &'a dyn Fn(&DefPath) -> Option<CalleeClass>,
}

/// One existing call boundary of the body.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ExistingBoundary {
    pub block: BlockId,
    pub call: CallId,
    pub operation: DefPath,
}

/// A value the frame retains across a boundary, at its frame offset.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct LiveOwnedSlot {
    pub value: ValueId,
    pub layout: LayoutId,
    pub repr: ValueRepr,
    pub holding: Holding,
    pub offset: u64,
}

/// Body storage (a slot) whose bytes the frame retains.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct LiveStorage {
    pub slot: SlotId,
    pub layout: LayoutId,
    pub repr: ValueRepr,
    pub offset: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RegionKind {
    /// A compiler-local region mark.
    Mark,
    /// An open arena's token.
    Arena,
}

/// A live region token: the resumed body still owns that region.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RegionTokenLayout {
    pub value: ValueId,
    pub layout: LayoutId,
    pub kind: RegionKind,
}

/// Where a stop or cancellation observed at the boundary enters cleanup, and
/// every cleanup anchor that cleanup can discharge.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CleanupAnchor {
    pub failure: FailureId,
    pub entry: BlockId,
    pub anchors: Vec<CleanupAnchorId>,
}

/// The block a resumed frame continues in; state 0 is the body's entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ResumeLabel {
    pub state: u32,
    pub edge: EdgeId,
    pub block: BlockId,
}

/// A resumable callee's own frame, nested in its caller's at the call.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct NestedFrame {
    pub body: ContentHash,
    pub offset: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ResumeState {
    pub boundary: ExistingBoundary,
    /// Set when the boundary is a call of a resumable body.
    pub callee: Option<NestedFrame>,
    pub live: Vec<LiveOwnedSlot>,
    /// The frame-resident storage live here, at the plan's fixed offsets.
    pub storage: Vec<LiveStorage>,
    pub loans: Vec<LoanId>,
    pub cleanup: CleanupAnchor,
    pub regions: Vec<RegionTokenLayout>,
    pub target: ResumeLabel,
    /// Bytes of this state's image, header included.
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Representation {
    /// No boundary: run to completion where it starts.
    Direct,
    /// Compact live-state frame at each boundary.
    Resumable,
    /// General stackful frame, with the reason the compact form is refused.
    Stackful(String),
}

/// Placement estimate. It selects representation and placement only.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct PlacementCost {
    pub boundaries: u32,
    pub frame_bytes: u64,
    pub owned: u32,
    pub instructions: u32,
}

/// The plan's identity over the body, layouts, callees, target and runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ValidatedContinuation {
    pub identity: ContentHash,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ContinuationPlan {
    pub body: ContentHash,
    pub digest: ContentHash,
    pub representation: Representation,
    /// Storage live at any boundary. It lives in the frame at fixed offsets
    /// for the whole body, so addresses into it stay valid across a suspension.
    pub storage: Vec<LiveStorage>,
    pub states: Vec<ResumeState>,
    pub captures: Option<LayoutId>,
    pub cost: PlacementCost,
    pub certificate: ValidatedContinuation,
}

/// Plans of every body that reaches a boundary; a body absent here is Direct.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContinuationPlans {
    pub plans: BTreeMap<ContentHash, ContinuationPlan>,
    pub identity: ContentHash,
}

/// How each call of a body affects its continuation.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum CallEffect {
    Local,
    Boundary(DefPath),
    /// A known callee that itself suspends: its frame is part of the stack.
    Nested(ContentHash),
    Unknown(String),
}

pub(crate) fn callee_symbol(u: &LirUnit<Ready>, call: &CallSite) -> Option<Symbol> {
    match call.callee {
        Callee::Direct(s) | Callee::Bound { code: s, .. } => {
            Some(u.tables.symbols[s.index()].clone())
        }
        _ => None,
    }
}

/// Which known bodies may reach a boundary, to a fixpoint over the call graph.
fn suspending(
    units: &BTreeMap<ContentHash, &LirUnit<Ready>>,
    classify: &dyn Fn(&DefPath) -> Option<CalleeClass>,
) -> BTreeSet<ContentHash> {
    let mut out = BTreeSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        for (id, u) in units {
            if out.contains(id) {
                continue;
            }
            let suspends = u.tables.calls.iter().any(|call| {
                match call_effect(u, call, units, &out, classify) {
                    CallEffect::Local => false,
                    CallEffect::Boundary(_) | CallEffect::Nested(_) | CallEffect::Unknown(_) => {
                        true
                    }
                }
            });
            if suspends {
                out.insert(*id);
                changed = true;
            }
        }
    }
    out
}

pub(crate) fn call_effect(
    u: &LirUnit<Ready>,
    call: &CallSite,
    units: &BTreeMap<ContentHash, &LirUnit<Ready>>,
    suspending: &BTreeSet<ContentHash>,
    classify: &dyn Fn(&DefPath) -> Option<CalleeClass>,
) -> CallEffect {
    let (definition, body) = match call.callee {
        Callee::Operation(op) => (u.tables.operations[op.index()].definition.clone(), None),
        Callee::Indirect(_) => return CallEffect::Unknown("indirect call".into()),
        Callee::Direct(_) | Callee::Bound { .. } => {
            let symbol = callee_symbol(u, call).unwrap();
            let body = symbol.instance.filter(|id| units.contains_key(id));
            (symbol.definition, body)
        }
    };
    // A classified operation is the boundary itself, even when its native
    // disposition is a library body: that body is the operation's implementation.
    match (classify(&definition), body) {
        (Some(CalleeClass::Local), _) => CallEffect::Local,
        (Some(CalleeClass::Boundary), _) => CallEffect::Boundary(definition),
        (Some(CalleeClass::Unknown), _) => {
            CallEffect::Unknown(format!("unclassified callee {definition}"))
        }
        (None, Some(id)) if suspending.contains(&id) => CallEffect::Nested(id),
        (None, Some(_)) => CallEffect::Local,
        (None, None) => CallEffect::Unknown(format!("unclassified callee {definition}")),
    }
}

fn fixed(table: &LayoutTable, id: LayoutId) -> Option<(u64, u64)> {
    let layout = table.layout(id)?;
    match layout.size {
        SizeSpec::Fixed(n) => Some((n, u64::from(layout.align.max(1)))),
        SizeSpec::Dynamic(_) => None,
    }
}

/// Forward may-live storage and active loans on entry to `at`'s terminator.
fn storage_at(u: &LirUnit<Ready>, at: BlockId) -> (BTreeSet<SlotId>, BTreeSet<LoanId>) {
    let n = u.blocks.len();
    let mut slots_in: Vec<BTreeSet<SlotId>> = vec![BTreeSet::new(); n];
    let mut loans_in: Vec<BTreeSet<LoanId>> = vec![BTreeSet::new(); n];
    slots_in[u.entry.index()].extend(
        u.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                matches!(
                    s.kind,
                    SlotKind::Parameter(_) | SlotKind::Capture(_) | SlotKind::ReturnSlot
                )
            })
            .map(|(i, _)| SlotId(i as u32)),
    );
    let mut reached = BTreeSet::from([u.entry]);
    let mut pending = vec![u.entry];
    let mut result = (BTreeSet::new(), BTreeSet::new());
    while let Some(b) = pending.pop() {
        let mut slots = slots_in[b.index()].clone();
        let mut loans = loans_in[b.index()].clone();
        let mut out = Vec::new();
        for inst in u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
        {
            if let Some(f) = inst.failure() {
                out.push((
                    u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target,
                    slots.clone(),
                    loans.clone(),
                ));
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
            result.0.extend(slots.iter().copied());
            result.1.extend(loans.iter().copied());
        }
        let term = &u.terminators[b.index()];
        let mut edges = term.normal_edges(&u.pools);
        edges.extend(
            term.failure_edges(&u.tables)
                .into_iter()
                .map(|f| u.tables.failures[f.index()].cleanup),
        );
        out.extend(edges.into_iter().map(|e| {
            (
                u.tables.edges[e.index()].target,
                slots.clone(),
                loans.clone(),
            )
        }));
        for (next, slots, loans) in out {
            let before = (slots_in[next.index()].len(), loans_in[next.index()].len());
            slots_in[next.index()].extend(slots);
            loans_in[next.index()].extend(loans);
            let grew = before != (slots_in[next.index()].len(), loans_in[next.index()].len());
            if reached.insert(next) || grew {
                pending.push(next);
            }
        }
    }
    result
}

/// Every cleanup anchor reachable from a cleanup entry.
fn anchors_from(u: &LirUnit<Ready>, entry: BlockId) -> Vec<CleanupAnchorId> {
    let mut seen = BTreeSet::from([entry]);
    let mut pending = vec![entry];
    let mut anchors = BTreeSet::new();
    while let Some(b) = pending.pop() {
        for inst in u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
        {
            match *inst {
                Inst::Drop { anchor, .. }
                | Inst::Release { anchor, .. }
                | Inst::Reset { anchor, .. }
                | Inst::FinalizeBegin { anchor, .. }
                | Inst::FinalizeEnd { anchor, .. } => {
                    anchors.insert(anchor);
                }
                _ => {}
            }
            if let Some(f) = inst.failure() {
                let next = u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target;
                if seen.insert(next) {
                    pending.push(next);
                }
            }
        }
        let term = &u.terminators[b.index()];
        let mut edges = term.normal_edges(&u.pools);
        edges.extend(
            term.failure_edges(&u.tables)
                .into_iter()
                .map(|f| u.tables.failures[f.index()].cleanup),
        );
        for e in edges {
            let next = u.tables.edges[e.index()].target;
            if seen.insert(next) {
                pending.push(next);
            }
        }
    }
    anchors.into_iter().collect()
}

fn slot_layout(u: &LirUnit<Ready>, s: SlotId) -> (LayoutId, ValueRepr) {
    let ty = &u.types[u.slots[s.index()].ty.index()];
    (ty.layout, ty.repr)
}

/// Place items in order from `at`; the end, rounded to 8.
fn place(table: &LayoutTable, at: u64, layouts: &[LayoutId]) -> Option<(Vec<u64>, u64)> {
    let mut at = at;
    let mut out = Vec::with_capacity(layouts.len());
    for &layout in layouts {
        let (size, align) = fixed(table, layout)?;
        at = at.checked_next_multiple_of(align)?;
        out.push(at);
        at = at.checked_add(size)?;
    }
    Some((out, at.checked_next_multiple_of(8)?))
}

/// Values live across the boundary call at `block`: what its successors
/// need, minus what it defines, plus what it reads (it has not yet run).
fn live_across(
    u: &LirUnit<Ready>,
    live: &crate::liveness::Liveness,
    block: BlockId,
    call: CallId,
) -> Vec<ValueId> {
    let mut values = live.successor_needs(u, block);
    for d in crate::liveness::terminator_defs(u, &u.terminators[block.index()]) {
        values.remove(d);
    }
    for v in crate::liveness::terminator_uses(u, &u.terminators[block.index()]) {
        values.insert(v);
    }
    let site = &u.tables.calls[call.index()];
    for a in site.arguments.get(&u.pools.call_arguments).unwrap_or(&[]) {
        if let ArgumentValue::Address { address, .. } = a.value
            && let AddressBase::Value(v) = u.addresses[address.index()].base
        {
            values.insert(v);
        }
    }
    values.iter().collect()
}

/// Whether `to` is reachable from `from` along any edge.
fn reaches(u: &LirUnit<Ready>, from: BlockId, to: BlockId) -> bool {
    let mut seen = BTreeSet::from([from]);
    let mut pending = vec![from];
    while let Some(b) = pending.pop() {
        if b == to {
            return true;
        }
        let mut next = Vec::new();
        for inst in u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
        {
            if let Some(f) = inst.failure() {
                next.push(u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target);
            }
        }
        let term = &u.terminators[b.index()];
        next.extend(
            term.normal_edges(&u.pools)
                .into_iter()
                .map(|e| u.tables.edges[e.index()].target),
        );
        next.extend(
            term.failure_edges(&u.tables)
                .into_iter()
                .map(|f| u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target),
        );
        for n in next {
            if seen.insert(n) {
                pending.push(n);
            }
        }
    }
    false
}

/// One boundary: its call, operation and, for a resumable callee, its frame.
pub(crate) struct Site {
    pub(crate) block: BlockId,
    pub(crate) call: CallId,
    pub(crate) operation: DefPath,
    pub(crate) callee: Option<(ContentHash, u64)>,
}

/// Lay out every state over one fixed storage area.
fn states(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    sites: &[Site],
) -> Result<(Vec<LiveStorage>, Vec<ResumeState>), String> {
    let live = crate::liveness::liveness(u);
    let mut at_sites = Vec::new();
    let mut union = BTreeSet::new();
    for site in sites {
        let Terminator::Call {
            normal: Some(normal),
            ..
        } = u.terminators[site.block.index()]
        else {
            return Err("diverging boundary".into());
        };
        if reaches(u, u.tables.edges[normal.index()].target, site.block) {
            return Err("boundary on a cycle".into());
        }
        let (slots, loans) = storage_at(u, site.block);
        union.extend(slots.iter().copied());
        at_sites.push((slots, loans));
    }
    let slots: Vec<_> = union.into_iter().collect();
    let (offsets, base) = place(
        table,
        FRAME_HEADER_BYTES,
        &slots
            .iter()
            .map(|s| slot_layout(u, *s).0)
            .collect::<Vec<_>>(),
    )
    .ok_or("dynamic live storage")?;
    let storage: Vec<_> = slots
        .iter()
        .zip(&offsets)
        .map(|(&slot, &offset)| {
            let (layout, repr) = slot_layout(u, slot);
            LiveStorage {
                slot,
                layout,
                repr,
                offset,
            }
        })
        .collect();
    let mut out = Vec::new();
    for (i, (site, (slots, loans))) in sites.iter().zip(at_sites).enumerate() {
        let Terminator::Call {
            normal: Some(normal),
            failure,
            ..
        } = u.terminators[site.block.index()]
        else {
            unreachable!()
        };
        let values = live_across(u, &live, site.block, site.call);
        let layouts: Vec<_> = values
            .iter()
            .map(|v| u.types[u.values[v.index()].ty.index()].layout)
            .collect();
        let (voffsets, mut bytes) = place(table, base, &layouts).ok_or("dynamic live layout")?;
        let callee = site.callee.map(|(body, size)| {
            let offset = bytes.next_multiple_of(16);
            bytes = (offset + size).next_multiple_of(8);
            NestedFrame {
                body,
                offset,
                bytes: size,
            }
        });
        let entry = u.tables.edges[u.tables.failures[failure.index()].cleanup.index()].target;
        let ty = |v: ValueId| &u.types[u.values[v.index()].ty.index()];
        out.push(ResumeState {
            boundary: ExistingBoundary {
                block: site.block,
                call: site.call,
                operation: site.operation.clone(),
            },
            callee,
            live: values
                .iter()
                .zip(voffsets)
                .map(|(&value, offset)| LiveOwnedSlot {
                    value,
                    layout: ty(value).layout,
                    repr: ty(value).repr,
                    holding: u.values[value.index()].holding,
                    offset,
                })
                .collect(),
            storage: storage
                .iter()
                .filter(|s| slots.contains(&s.slot))
                .cloned()
                .collect(),
            loans: loans.into_iter().collect(),
            cleanup: CleanupAnchor {
                failure,
                entry,
                anchors: anchors_from(u, entry),
            },
            regions: values
                .iter()
                .filter_map(|&value| {
                    match ty(value).repr {
                        ValueRepr::RegionMark => Some(RegionKind::Mark),
                        ValueRepr::Arena => Some(RegionKind::Arena),
                        _ => None,
                    }
                    .map(|kind| RegionTokenLayout {
                        value,
                        layout: ty(value).layout,
                        kind,
                    })
                })
                .collect(),
            target: ResumeLabel {
                state: i as u32 + 1,
                edge: normal,
                block: u.tables.edges[normal.index()].target,
            },
            bytes,
        });
    }
    Ok((storage, out))
}

/// The certificate binds everything the plan was derived from.
pub(crate) fn certify(
    u: &LirUnit<Ready>,
    inputs: &PlanInputs<'_>,
    effects: &[CallEffect],
    callee_digests: &BTreeMap<ContentHash, ContentHash>,
    representation: &Representation,
    storage: &[LiveStorage],
    states: &[ResumeState],
    captures: Option<LayoutId>,
) -> ValidatedContinuation {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&u.header.instance.0);
    bytes.extend_from_slice(&u.digest().0);
    bytes.extend_from_slice(&inputs.table.digest().0);
    bytes.extend_from_slice(&inputs.target.digest().0);
    bytes.extend_from_slice(&inputs.runtime.0);
    for effect in effects {
        let text = match effect {
            CallEffect::Nested(id) => format!(
                "nested {:?} {:?}",
                id,
                callee_digests.get(id).copied().unwrap_or_default()
            ),
            other => format!("{other:?}"),
        };
        bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
        bytes.extend_from_slice(text.as_bytes());
    }
    let tail = format!("{representation:?}{storage:?}{states:?}{captures:?}");
    bytes.extend_from_slice(&(tail.len() as u64).to_le_bytes());
    bytes.extend_from_slice(tail.as_bytes());
    ValidatedContinuation {
        identity: ContentHash::of_parts("hirsel.continuation.plan.v2", [bytes.as_slice()]),
    }
}

/// The boundaries of a closed body, in block order; Err refuses the compact form.
pub(crate) fn sites(
    u: &LirUnit<Ready>,
    effects: &[CallEffect],
    plans: &BTreeMap<ContentHash, ContinuationPlan>,
) -> Result<Vec<Site>, String> {
    let mut out = Vec::new();
    for (b, term) in u.terminators.iter().enumerate() {
        let Terminator::Call { call, .. } = term else {
            continue;
        };
        let (operation, callee) = match &effects[call.index()] {
            CallEffect::Local => continue,
            CallEffect::Unknown(why) => return Err(why.clone()),
            CallEffect::Boundary(op) => (op.clone(), None),
            CallEffect::Nested(id) => match plans.get(id) {
                Some(p) if p.representation == Representation::Resumable => (
                    callee_symbol(u, &u.tables.calls[call.index()])
                        .unwrap()
                        .definition,
                    Some((*id, p.cost.frame_bytes)),
                ),
                _ => return Err(format!("suspends inside stackful callee {id:?}")),
            },
        };
        out.push(Site {
            block: BlockId(b as u32),
            call: *call,
            operation,
            callee,
        });
    }
    Ok(out)
}

fn plan_unit(
    u: &LirUnit<Ready>,
    inputs: &PlanInputs<'_>,
    units: &BTreeMap<ContentHash, &LirUnit<Ready>>,
    suspends: &BTreeSet<ContentHash>,
    digests: &BTreeMap<ContentHash, ContentHash>,
    done: &BTreeMap<ContentHash, ContinuationPlan>,
) -> ContinuationPlan {
    let effects: Vec<_> = u
        .tables
        .calls
        .iter()
        .map(|c| call_effect(u, c, units, suspends, inputs.classify))
        .collect();
    let captures = inputs
        .environments
        .get(&u.header.instance)
        .copied()
        .flatten();
    let built = sites(u, &effects, done).and_then(|sites| states(u, inputs.table, &sites));
    let (representation, storage, states) = match built {
        Err(why) => (Representation::Stackful(why), Vec::new(), Vec::new()),
        Ok((_, states)) if states.is_empty() => (Representation::Direct, Vec::new(), states),
        Ok((_, states)) if states.iter().any(|s| s.bytes > MAX_FRAME_BYTES) => (
            Representation::Stackful("oversize live image".into()),
            Vec::new(),
            Vec::new(),
        ),
        // Each re-entry passes a body's declared parameters again; until the
        // alternates take them from the frame instead, such a body stays stackful.
        Ok(_) if !u.parameters.is_empty() => (
            Representation::Stackful("declared parameters".into()),
            Vec::new(),
            Vec::new(),
        ),
        // A compiler-local region's record is a stack slot of the frame that
        // entered it; until records are frame-resident its body stays stackful.
        Ok((_, states)) if states.iter().any(|s| !s.regions.is_empty()) => (
            Representation::Stackful("region record live across a boundary".into()),
            Vec::new(),
            Vec::new(),
        ),
        Ok((storage, states)) => (Representation::Resumable, storage, states),
    };
    let cost = PlacementCost {
        instructions: (u.instructions.len() + u.terminators.len()) as u32,
        boundaries: states.len() as u32,
        frame_bytes: states.iter().map(|s| s.bytes).max().unwrap_or(0),
        owned: states
            .iter()
            .map(|s| {
                s.live
                    .iter()
                    .filter(|l| l.holding == Holding::Owned)
                    .count() as u32
            })
            .max()
            .unwrap_or(0),
    };
    let certificate = certify(
        u,
        inputs,
        &effects,
        digests,
        &representation,
        &storage,
        &states,
        captures,
    );
    ContinuationPlan {
        body: u.header.instance,
        digest: u.digest(),
        representation,
        storage,
        states,
        captures,
        cost,
        certificate,
    }
}

/// Callees first, so a caller sees each nested callee's final plan. A body
/// reached again while being planned is recursive and keeps its stack.
fn plan_in_order(
    id: ContentHash,
    inputs: &PlanInputs<'_>,
    units: &BTreeMap<ContentHash, &LirUnit<Ready>>,
    suspends: &BTreeSet<ContentHash>,
    digests: &BTreeMap<ContentHash, ContentHash>,
    active: &mut BTreeSet<ContentHash>,
    done: &mut BTreeMap<ContentHash, ContinuationPlan>,
) {
    if done.contains_key(&id) || !active.insert(id) {
        return;
    }
    let u = units[&id];
    for call in &u.tables.calls {
        if let CallEffect::Nested(callee) = call_effect(u, call, units, suspends, inputs.classify) {
            plan_in_order(callee, inputs, units, suspends, digests, active, done);
        }
    }
    let plan = plan_unit(u, inputs, units, suspends, digests, done);
    active.remove(&id);
    done.insert(id, plan);
}

impl ContinuationPlans {
    /// Plan every body that can reach a boundary. Partitioning cannot change
    /// a plan: it reads only the complete Ready bodies given here.
    pub fn build(units: &[&ValidatedUnit<Ready>], inputs: &PlanInputs<'_>) -> Self {
        let index: BTreeMap<_, _> = units
            .iter()
            .map(|u| (u.unit().header.instance, u.unit()))
            .collect();
        let digests: BTreeMap<_, _> = index.iter().map(|(k, u)| (*k, u.digest())).collect();
        let suspends = suspending(&index, inputs.classify);
        let mut plans = BTreeMap::new();
        for id in &suspends {
            plan_in_order(
                *id,
                inputs,
                &index,
                &suspends,
                &digests,
                &mut BTreeSet::new(),
                &mut plans,
            );
        }
        let identity = Self::identity_of(&plans, inputs);
        Self { plans, identity }
    }

    pub(crate) fn identity_of(
        plans: &BTreeMap<ContentHash, ContinuationPlan>,
        inputs: &PlanInputs<'_>,
    ) -> ContentHash {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&inputs.runtime.0);
        bytes.extend_from_slice(&(plans.len() as u64).to_le_bytes());
        for (id, plan) in plans {
            bytes.extend_from_slice(&id.0);
            bytes.extend_from_slice(&plan.certificate.identity.0);
        }
        ContentHash::of_parts("hirsel.continuation.plans.v1", [bytes.as_slice()])
    }

    pub fn get(&self, body: ContentHash) -> Option<&ContinuationPlan> {
        self.plans.get(&body)
    }

    /// Whether any body runs through its resumable alternates.
    pub fn resumable(&self) -> bool {
        self.plans
            .values()
            .any(|p| p.representation == Representation::Resumable)
    }

    /// The plan of a task body that enters through its dispatcher.
    pub fn task_entry(&self, unit: &LirUnit<Ready>) -> Option<&ContinuationPlan> {
        self.get(unit.header.instance).filter(|p| {
            p.representation == Representation::Resumable
                && unit.header.definition.is_core("$native.task_main")
        })
    }
}
