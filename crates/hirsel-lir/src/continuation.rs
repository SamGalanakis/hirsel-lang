//! Compiler-owned continuation plans (r6 S7): emission metadata derived from
//! validated Ready bodies and layouts, never a new LIR construct.
//!
//! A body whose calls are all classified is *closed*. A closed body with at
//! least one existing operation boundary gets one [`ResumeState`] per boundary:
//! the values and storage live across it, its region tokens, the cleanup that a
//! stop or cancellation runs from it and the label execution resumes at. Its
//! declared parameters and region records live in the frame for the whole
//! body: a re-entry binds parameters from their records instead of its
//! arguments, and a region token, its record's address, survives a save. A body
//! reaching an unknown call, or suspending inside a stackful nested frame, keeps
//! the general stackful representation. One aligned, non-owning scalar `mut`
//! parameter is held as its caller place's address; only a call edge proved to
//! pass a slot of the caller's own frame ([`ExclusiveSlot`]) enters that body
//! compactly. Both use the same Saved/Active ownership
//! protocol in the runtime; the representation never changes semantics.
//!
//! Cost classes only choose representation and placement. Like Rayon's join
//! (`rayon-core/src/join/mod.rs:132-185`), a local first execution must have
//! materialized everything a stolen continuation needs before it can wait.
//!
//! [`validate`] recomputes every plan with its own analyses: a plan imported
//! from a cache or another partition is never trusted by its identity alone.
use std::collections::{BTreeMap, BTreeSet};

use hirsel_core_contract::Passing;
use hirsel_syntax::{ContentHash, DefPath};

use crate::abi_types::ParameterPassing;
use crate::ir::*;
use crate::layout_types::{LayoutId, LayoutKind, LayoutTable, ScalarKind, SizeSpec, TargetSpec};
use crate::runtime_abi::REGION_RECORD_BYTES;
use crate::stage::{Ready, ValidatedUnit};

pub mod isolate;
mod ordered;
mod ordered_await;
mod ordered_join;
mod runtime;
mod scope;
pub use ordered::{check_ordered_host_result, ordered_host_result};
pub use ordered_await::{AwaitCleanup, OrderedAwaitProtocol};
pub use ordered_join::{check_ordered_join_result, ordered_join_result};
pub use runtime::{RuntimeOpKind, RuntimeOpPlan, RuntimeResultSlot, RuntimeResultSource};
pub use scope::ScopeOpPlan;
mod tail;
mod validate;
pub use tail::{OwnedTail, TailCarrier, TailKind};
mod cleanup;
pub use validate::{
    AheadCertificate, CallerProof, ContinuationError, FrameDag, ResultCleanupProof, validate,
    validate_runtime_operations,
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
/// (`skipped`) that the resumed code accesses and whose root base is a slot
/// or a reloaded live value, bases first. Their checks passed before the suspension and read
/// only state the frame retains, so re-running them observes nothing.
pub fn resume_preparations(
    u: &LirUnit<Ready>,
    plan: &ContinuationPlan,
    state: u32,
    skipped: &[bool],
) -> Vec<(BlockId, usize)> {
    let (boundary_block, live, instruction) = if let Some(resume) = state.checked_sub(1).and_then(|i| plan.states.get(i as usize)) {
        (resume.boundary.block, resume.live.as_slice(), None)
    } else if let Some(cleanup) = plan.cleanups.iter().find(|s|s.state==state) {
        (cleanup.block, cleanup.live.as_slice(), Some(cleanup.instruction as usize))
    } else { return Vec::new(); };
    let mut at = BTreeMap::new();
    for (b, block) in u.blocks.iter().enumerate() {
        if !skipped.get(b).copied().unwrap_or(false) {
            continue;
        }
        for (i, inst) in block
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[])
            .iter()
            .enumerate()
        {
            if instruction.is_some_and(|before| b==boundary_block.index() && i>=before) {continue;}
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
    let carried = carried_from(u, boundary_block, instruction).0;
    // Every alternate has a place for each slot (frame-resident when live at
    // a boundary); a value base must be one the state reloads.
    let eligible = |a: AddressId| {
        carried.contains(&a)
            && match root(u, a) {
                AddressBase::Slot(_) => true,
                AddressBase::Value(v) => live.iter().any(|l| l.value == v),
                AddressBase::Address(_) => false,
            }
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

/// The dotted name of a `core@1` definition (`channel.send`), the key of the
/// runtime's reviewed operation descriptors; None outside `core@1`.
pub fn core_name(definition: &DefPath) -> Option<String> {
    let path = &definition.0;
    if !path.package.package_is("core", "1") {
        return None;
    }
    Some(
        path.modules
            .iter()
            .chain(std::iter::once(&path.name))
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("."),
    )
}

/// The reviewed ops a `core@1` operation or runtime entry performs.
fn core_ops(name: &str) -> Option<&'static [&'static str]> {
    use hirsel_rt_operations::descriptors as d;
    if let Some((_, ops)) = d::REGISTRY.iter().find(|(n, _)| *n == name) {
        return Some(ops);
    }
    let symbol = format!("hirsel_rt_v2_{}", name.replace('.', "_"));
    d::entry_descriptor(&symbol).map(|entry| entry.ops)
}

/// The resume point's hints for boundary operation `definition`: whether a
/// task that is not the Ordered head is logged for it (every op it performs
/// is class D) or may commit it early (every op is in §4.1 E or class D),
/// and whether its first op may leave its stack ([`leaves_at_boundary`]).
/// Physical only; they decide no observation.
pub fn resume_hint(definition: &DefPath) -> u32 {
    use crate::runtime_abi::{RESUME_EARLY, RESUME_LEAVES, RESUME_LOGGED};
    use hirsel_rt_operations::descriptors::{self as d, Class};
    let mut hint = 0;
    if leaves_at_boundary(definition) {
        hint |= RESUME_LEAVES;
    }
    let Some(ops) = core_name(definition).and_then(|name| core_ops(&name)) else {
        return hint;
    };
    if ops.is_empty() {
        return hint;
    }
    let logged = |op: &&str| d::op_descriptor(op).is_some_and(|o| o.class() == Class::D);
    if ops.iter().all(logged) {
        hint |= RESUME_LOGGED;
    }
    if ops.iter().all(|op| d::E.contains(op) || logged(op)) {
        hint |= RESUME_EARLY;
    }
    hint
}

/// Boundary operations whose first op may leave its stack in its Free
/// replay wait and return `STATUS_SUSPENDED`, for the boundary call to
/// reissue the whole operation (`RESUME_LEAVES`). The rule: the operation's
/// only effect begins at that op's runtime entry, so the reissue repeats
/// nothing. The runtime checks the rest itself: the op leaves only as the
/// first begun since a resume point that carries the hint.
///
/// - `channel.send`: its body builds `Some(message)` in a stack slot and calls
///   `$intrinsic.ChannelSend` ([`LEAVES_IN_LIBRARY`]).
/// - `sender.dup`: the runtime entry itself; it has no library body.
/// - `mutex.with`: its body builds a `None` slot and calls
///   `$intrinsic.MutexTake` ([`LEAVES_IN_LIBRARY`]). The body's put and kill
///   follow the take and the callback, so they never leave.
///
/// A channel or mutex close never leaves: it runs inside a release walk
/// (`finalize::run_close`), which has no boundary call to reissue.
const LEAVES_AT_BOUNDARY: &[&str] = &["channel.send", "sender.dup", "mutex.with"];

/// Whether boundary operation `definition`'s first op may leave its stack
/// ([`LEAVES_AT_BOUNDARY`]).
pub fn leaves_at_boundary(definition: &DefPath) -> bool {
    core_name(definition).is_some_and(|name| LEAVES_AT_BOUNDARY.contains(&name.as_str()))
}

/// Runtime entries that may return `STATUS_SUSPENDED` from inside their
/// operation's library body, which returns the status to its boundary call
/// unchanged; the boundary saves the frame and reissues the body on the wake.
/// Each is the first effect of a [`LEAVES_AT_BOUNDARY`] operation's body:
/// `$intrinsic.ChannelSend` of `channel.send`'s, `$intrinsic.MutexTake` of
/// `mutex.with`'s. `channel.receive` joins by the same rule once its wait can
/// leave.
const LEAVES_IN_LIBRARY: &[&str] = &["$intrinsic.ChannelSend", "$intrinsic.MutexTake"];

/// Whether a call of operation `definition` outside a boundary returns
/// `STATUS_SUSPENDED` to its caller ([`LEAVES_IN_LIBRARY`]).
pub fn leaves_in_library(definition: &DefPath) -> bool {
    core_name(definition).is_some_and(|name| LEAVES_IN_LIBRARY.contains(&name.as_str()))
}

/// The existing mutex.with take admission applies at its nested wrapper's
/// take site too. Put/kill/cause use a distinct entered-tail binder.
pub fn resume_site_hint(u: &LirUnit<Ready>, state: &ResumeState) -> u32 {
    if tail::known_wrapper(u)
        && core_name(&state.boundary.operation).as_deref() == Some("$intrinsic.MutexTake")
    {
        resume_hint(&u.header.definition)
    } else {
        resume_hint(&state.boundary.operation)
    }
}

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

/// A declared parameter, materialized once: state 0 stores its incoming value
/// (an address parameter's bytes) here and every state binds the parameter
/// from this record, so a re-entry never takes its arguments again. An
/// exclusive parameter's record is pointer-sized: it holds the caller place's
/// address, never a copy of its bytes ([`ExclusiveSlot`]). As Rayon's
/// `StackJob::new` does before the job is pushed (`join/mod.rs:132-138`), it is
/// in the frame before the first point a continuation can move.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FrameParameter {
    pub value: ValueId,
    pub source: u32,
    /// The ownership mode the parameter was received under.
    pub convention: Passing,
    pub passing: ParameterPassing,
    pub holding: Holding,
    /// The parameter's slot, whose storage this record is.
    pub slot: Option<SlotId>,
    pub layout: LayoutId,
    pub offset: u64,
}

/// A compiler-local region's record, in the frame for the whole body: a
/// region token is its record's address, so it stays valid across a save.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FrameRegion {
    pub region: RegionInstanceId,
    pub offset: u64,
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

/// Logical continuation custody. Physical resumption may retry only a call
/// that has not entered; a completed operation and an active cleanup have
/// different continuations even when they originated at the same CallId.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ResumeAction {
    RetryUnenteredCall(CallId),
    ResumeNested(CallId, NestedFrame),
    /// Continue the same entered runtime intrinsic, whose independently
    /// checked operation/result proof is this state's `runtime` descriptor.
    ResumeRuntime(CallId),
    AfterEnteredCall(CallId, EdgeId),
    ResumeCleanup {
        block: BlockId,
        instruction: u32,
        anchor: CleanupAnchorId,
    },
}

/// Call-edge evidence that a nested callee's exclusive parameter borrows one
/// whole slot of this frame directly: the slot is frame storage live at the
/// boundary, aligned for its non-owning scalar layout, and the call's one
/// active mutable loan names exactly that place. The frame's allocation moves
/// with its continuation and is never reallocated, so the address the callee
/// records stays valid across a save and a migration; the callee writes the
/// canonical place, so returning or failing needs no copy-out.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ExclusiveSlot {
    pub callee: ContentHash,
    pub call: CallId,
    /// The callee parameter (its ABI source) the slot is passed as.
    pub parameter: u32,
    pub address: AddressId,
    pub loan: LoanId,
    pub slot: SlotId,
    /// The slot's offset in this frame's storage.
    pub offset: u64,
    pub layout: LayoutId,
    pub size: u64,
    pub align: u64,
    /// Where the caller continues once the callee returns or fails: its
    /// mutations are already in the slot.
    pub normal: EdgeId,
    pub failure: FailureId,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ResumeState {
    pub boundary: ExistingBoundary,
    pub action: ResumeAction,
    /// A typed entered operation; absent for retry, nested and cleanup actions.
    pub runtime: Option<RuntimeOpPlan>,
    /// Entered intrinsic custody, independently derived from this Ready site.
    pub tail: Option<OwnedTail>,
    /// Set when the boundary is a call of a resumable body.
    pub callee: Option<NestedFrame>,
    /// Set when that callee takes an exclusive parameter.
    pub exclusive: Option<ExclusiveSlot>,
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

/// An entered release instruction. Its identity is an instruction coordinate,
/// never a fabricated CallId; the runtime owns its consumed operand until the
/// instruction's original success/failure suffix can continue.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CleanupResumeState {
    pub action: ResumeAction,
    pub block: BlockId,
    pub instruction: u32,
    pub value: ValueId,
    pub layout: LayoutId,
    pub anchor: CleanupAnchorId,
    pub failure: FailureId,
    pub state: u32,
    pub live: Vec<LiveOwnedSlot>,
    pub storage: Vec<LiveStorage>,
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
    /// Every declared parameter, in order, first after the frame header.
    pub parameters: Vec<FrameParameter>,
    /// Every compiler-local region's record, in region order.
    pub records: Vec<FrameRegion>,
    /// Storage live at any boundary. It lives in the frame at fixed offsets
    /// for the whole body, so addresses into it stay valid across a suspension.
    pub storage: Vec<LiveStorage>,
    pub states: Vec<ResumeState>,
    pub cleanups: Vec<CleanupResumeState>,
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
    table:&LayoutTable,
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
            let suspends = !cleanup::sites(u,table).is_empty() || u.tables.calls.iter().any(|call| {
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
    // A specialized reviewed scope exposes its nested callback and entered
    // intrinsics. Classification grants no custody: plan_unit and import
    // validation reconstruct its callable and scope-flow evidence first.
    if let Some(id) = body.filter(|id| scope::candidate(units[id])) {
        return if suspending.contains(&id) {
            CallEffect::Nested(id)
        } else {
            CallEffect::Local
        };
    }
    // A classified operation is the boundary itself, even when its native
    // disposition is a library body: that body is the operation's implementation.
    if core_name(&definition).as_deref() == Some("mutex.with")
        && let Some(id) = body
        && tail::known_wrapper(units[&id])
    {
        return CallEffect::Nested(id);
    }
    // Keep does not exclude an entered replay wait in this retained wrapper.
    if tail::known_wrapper(u) && core_name(&definition).as_deref()==Some("$intrinsic.MutexCause") {
        return CallEffect::Boundary(definition);
    }
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
    storage_before(u, at, usize::MAX)
}
fn storage_before(u: &LirUnit<Ready>, at: BlockId, before:usize) -> (BTreeSet<SlotId>, BTreeSet<LoanId>) {
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
        for (i, inst) in u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[]).iter().enumerate()
        {
            if b == at && i == before {
                result.0.extend(slots.iter().copied());
                result.1.extend(loans.iter().copied());
            }
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
        if b == at && before == usize::MAX {
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
) -> Vec<ValueId> {
    let mut values = live.successor_needs(u, block);
    for d in crate::liveness::terminator_defs(u, &u.terminators[block.index()]) {
        values.remove(d);
    }
    for v in crate::liveness::terminator_uses(u, &u.terminators[block.index()]) {
        values.insert(v);
    }
    for v in carried(u, block).1 {
        values.insert(v);
    }
    values.iter().collect()
}

/// Every block `b` can continue in: its terminator's edges and the cleanup
/// of each failing instruction.
pub(crate) fn successors_of(u: &LirUnit<Ready>, b: BlockId) -> Vec<BlockId> {
    let mut out = Vec::new();
    for inst in u.blocks[b.index()]
        .instructions
        .get(&u.instructions)
        .unwrap_or(&[])
    {
        if let Some(f) = inst.failure() {
            out.push(u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target);
        }
    }
    let term = &u.terminators[b.index()];
    for e in term.normal_edges(&u.pools) {
        out.push(u.tables.edges[e.index()].target);
    }
    for f in term.failure_edges(&u.tables) {
        out.push(u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target);
    }
    out
}

/// Where a state resumed at boundary `block` continues: its call's edges only,
/// since the block's instructions ran before the save.
pub fn resumed_successors(u: &LirUnit<Ready>, block: BlockId) -> Vec<BlockId> {
    let term = &u.terminators[block.index()];
    term.normal_edges(&u.pools)
        .into_iter()
        .map(|e| u.tables.edges[e.index()].target)
        .chain(
            term.failure_edges(&u.tables)
                .into_iter()
                .map(|f| u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target),
        )
        .collect()
}

/// Successors of a cleanup instruction's suffix; earlier failing checks
/// already completed and cannot become new edges after a physical save.
pub fn cleanup_successors(u:&LirUnit<Ready>,block:BlockId,before:usize)->Vec<BlockId> {
    let mut out=resumed_successors(u,block);
    for inst in u.blocks[block.index()].instructions.get(&u.instructions).unwrap_or(&[]).iter().skip(before) {
        if let Some(f)=inst.failure() {out.push(u.tables.edges[u.tables.failures[f.index()].cleanup.index()].target);}
    }
    out
}

/// The address an instruction accesses, which its preparation must precede.
fn accessed(inst: &Inst) -> Option<AddressId> {
    match *inst {
        Inst::Read { address, .. }
        | Inst::Copy { address, .. }
        | Inst::FlatCopy { address, .. }
        | Inst::FlatView { address, .. }
        | Inst::FlatLength { address, .. }
        | Inst::FlatNext { address, .. }
        | Inst::SeqReserve { address, .. }
        | Inst::ArenaPush { address, .. }
        | Inst::SeqSetLen { address, .. }
        | Inst::SeqAppend { address, .. }
        | Inst::FlatAppend { address, .. }
        | Inst::FlatField { address, .. }
        | Inst::FlatClose { address, .. }
        | Inst::Store { address, .. }
        | Inst::Replace { address, .. }
        | Inst::BeginLoan { address, .. }
        | Inst::Drop { address, .. }
        | Inst::FinalizeBegin { address, .. }
        | Inst::FinalizeEnd { address, .. } => Some(address),
        _ => None,
    }
}

/// The addresses the code resumed at boundary `block` (its call and what its
/// normal edge reaches) accesses but does not prepare, bases included, and
/// the values their preparations read: a resumed state retains those values
/// and prepares those addresses again.
pub(crate) fn carried(
    u: &LirUnit<Ready>,
    block: BlockId,
) -> (BTreeSet<AddressId>, BTreeSet<ValueId>) {
    carried_from(u,block,None)
}

pub(crate) fn carried_from(u:&LirUnit<Ready>,block:BlockId,before:Option<usize>) -> (BTreeSet<AddressId>, BTreeSet<ValueId>) {
    let mut used = Vec::new();
    let mut prepared = BTreeSet::new();
    let mut call_uses = |term: &Terminator, used: &mut Vec<AddressId>| {
        if let Terminator::Call { call, .. } = *term {
            for a in u.tables.calls[call.index()]
                .arguments
                .get(&u.pools.call_arguments)
                .unwrap_or(&[])
            {
                if let ArgumentValue::Address { address, .. } = a.value {
                    used.push(address);
                }
            }
        }
    };
    let mut seen = BTreeSet::new();
    let mut pending = if before.is_some() { vec![block] } else {
        call_uses(&u.terminators[block.index()], &mut used);
        resumed_successors(u, block)
    };
    while let Some(b) = pending.pop() {
        if !seen.insert(b) {
            continue;
        }
        for inst in u.blocks[b.index()]
            .instructions
            .get(&u.instructions)
            .unwrap_or(&[]).iter().skip(if b==block {before.unwrap_or(0)} else {0})
        {
            if let Inst::Address { address, .. } = *inst {
                prepared.insert(address);
                if let AddressBase::Address(base) = u.addresses[address.index()].base {
                    used.push(base);
                }
            }
            used.extend(accessed(inst));
        }
        call_uses(&u.terminators[b.index()], &mut used);
        pending.extend(if b==block { before.map(|i|cleanup_successors(u,b,i)).unwrap_or_else(||successors_of(u,b)) } else {successors_of(u,b)});
    }
    let mut addresses = BTreeSet::new();
    let mut values = BTreeSet::new();
    while let Some(a) = used.pop() {
        if prepared.contains(&a) || !addresses.insert(a) {
            continue;
        }
        let preparation = Inst::Address {
            address: a,
            failure: FailureId(0),
        };
        values.extend(crate::liveness::inst_uses(u, &preparation));
        if let AddressBase::Address(base) = u.addresses[a.index()].base {
            used.push(base);
        }
    }
    (addresses, values)
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
    /// The resumable callee's exclusive parameter, from the call's own FnAbi
    /// (never the callee's plan): its edge needs an [`ExclusiveSlot`] proof.
    pub(crate) exclusive: Option<ExclusiveParameter>,
}

/// A call's exclusive parameter: its ABI source and layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExclusiveParameter {
    pub(crate) source: u32,
    pub(crate) layout: LayoutId,
}

/// Whether a slot is homed outside the plan's storage: a parameter's slot is
/// its frame record.
fn parameter_slot(u: &LirUnit<Ready>, s: SlotId) -> bool {
    matches!(u.slots[s.index()].kind, SlotKind::Parameter(_))
}

/// Whether a `mut` parameter of `layout` can be borrowed in place by a
/// compact frame: a whole non-owning scalar (no pointer, scan, drop or
/// finalizer), so its record is only an address and no writeback, gather or
/// realignment exists. Its value must also be a plain `Scalar`: a handle
/// (a task set's, say) has a scalar layout but names runtime state.
fn exclusive_scalar(table: &LayoutTable, layout: LayoutId) -> bool {
    table.layout(layout).is_some_and(|l| {
        matches!(l.kind, LayoutKind::Scalar(k) if k != ScalarKind::Pointer)
            && matches!(l.size, SizeSpec::Fixed(_))
            && l.scan.is_empty()
            && l.drop.is_empty()
            && l.finalizer.is_none()
    })
}

/// The fixed area after the header: each declared parameter's record, then
/// each region's, and where storage starts. A parameter the frame cannot hold
/// by value refuses the compact form: a pointer/length extent is dynamic, and
/// an exclusive address names the caller's storage, so only one whole
/// non-owning scalar `mut` parameter, recorded as that address, is held.
fn fixed_area(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    target: &TargetSpec,
) -> Result<(Vec<FrameParameter>, Vec<FrameRegion>, u64), String> {
    let abi = table
        .fn_abis
        .get(u.header.abi.0 as usize)
        .ok_or("a body outside the ABI table")?;
    if abi.parameters.len() != u.parameters.len() {
        return Err("parameter count differs from the FnAbi".into());
    }
    let mut exclusive = 0;
    let mut extents = Vec::with_capacity(abi.parameters.len());
    for (p, &value) in abi.parameters.iter().zip(&u.parameters) {
        match p.passing {
            ParameterPassing::ByValue
            | ParameterPassing::ReadOnlyAddress
            | ParameterPassing::TransferAddress => {
                extents.push(fixed(table, p.layout).ok_or("dynamic parameter")?);
            }
            ParameterPassing::ExclusiveAddress => {
                exclusive += 1;
                let slotted = u
                    .slots
                    .iter()
                    .filter(|s| s.kind == SlotKind::Parameter(p.source))
                    .count()
                    == 1;
                let ty = &u.types[u.values[value.index()].ty.index()];
                if exclusive > 1
                    || p.convention != Passing::Mut
                    || !slotted
                    || !matches!(ty.repr, ValueRepr::Scalar(_))
                    || u.values[value.index()].holding != Holding::Plain
                    || !exclusive_scalar(table, p.layout)
                {
                    return Err("exclusive parameter".into());
                }
                let pointer = u64::from(target.pointer_bytes);
                extents.push((pointer, pointer));
            }
            ParameterPassing::PointerLength => return Err("pointer/length parameter".into()),
        }
    }
    let mut offsets = Vec::with_capacity(extents.len());
    let mut at = FRAME_HEADER_BYTES;
    for (size, align) in extents {
        at = at.next_multiple_of(align);
        offsets.push(at);
        at += size;
    }
    let end = at.next_multiple_of(8);
    let parameters = abi
        .parameters
        .iter()
        .zip(&u.parameters)
        .zip(offsets)
        .map(|((p, &value), offset)| FrameParameter {
            value,
            source: p.source,
            convention: p.convention,
            passing: p.passing,
            holding: u.values[value.index()].holding,
            slot: u
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(p.source))
                .map(|s| SlotId(s as u32)),
            layout: p.layout,
            offset,
        })
        .collect();
    let mut at = end;
    let mut records = Vec::new();
    for r in 0..u.tables.regions.len() {
        records.push(FrameRegion {
            region: RegionInstanceId(r as u32),
            offset: at,
        });
        at = at
            .checked_add(REGION_RECORD_BYTES)
            .ok_or("oversize live image")?;
    }
    Ok((parameters, records, at))
}

/// The [`ExclusiveSlot`] proof of a nested call edge, from the Ready body:
/// None unless a direct call passes, as the callee's exclusive parameter, a
/// whole local slot (no projection) with the parameter's layout, frame storage
/// live at the boundary and aligned there, under exactly one active loan of
/// that place, mutable, with no other argument or active loan reaching the slot
/// and no pretransfer gate.
#[allow(clippy::too_many_arguments)]
fn exclusive_edge(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    site: &Site,
    callee: ContentHash,
    record: &ExclusiveParameter,
    slots: &BTreeSet<SlotId>,
    loans: &BTreeSet<LoanId>,
    storage: &[LiveStorage],
) -> Option<ExclusiveSlot> {
    let call = &u.tables.calls[site.call.index()];
    let Terminator::Call {
        normal: Some(normal),
        failure,
        ..
    } = u.terminators[site.block.index()]
    else {
        return None;
    };
    if !matches!(call.callee, Callee::Direct(_)) || call.pretransfer_failure.is_some() {
        return None;
    }
    let arguments = call.arguments.get(&u.pools.call_arguments).unwrap_or(&[]);
    let mut matching = arguments.iter().filter(|a| a.parameter == record.source);
    let (Some(argument), None) = (matching.next(), matching.next()) else {
        return None;
    };
    let ArgumentValue::Address { address, .. } = argument.value else {
        return None;
    };
    let info = &u.addresses[address.index()];
    let AddressBase::Slot(slot) = info.base else {
        return None;
    };
    if info.projection.len != 0
        || !info.unchecked.is_empty()
        || !matches!(
            u.slots[slot.index()].kind,
            SlotKind::Local | SlotKind::Temporary
        )
        || slot_layout(u, slot).0 != record.layout
        || u.types[info.ty.index()].layout != record.layout
        || !exclusive_scalar(table, record.layout)
        || !matches!(slot_layout(u, slot).1, ValueRepr::Scalar(_))
        || !slots.contains(&slot)
    {
        return None;
    }
    let (size, align) = fixed(table, record.layout)?;
    let offset = storage.iter().find(|s| s.slot == slot)?.offset;
    if offset % align != 0 {
        return None;
    }
    // The slot's root, through any chain of address bases.
    let root = |mut a: AddressId| loop {
        match u.addresses[a.index()].base {
            AddressBase::Address(b) => a = b,
            AddressBase::Slot(s) => return Some(s),
            AddressBase::Value(_) => return None,
        }
    };
    let reaches = |a: AddressId| root(a) == Some(slot);
    // No other argument reaches the slot.
    if arguments.iter().any(|a| {
        a.parameter != record.source
            && matches!(a.value, ArgumentValue::Address { address, .. } if reaches(address))
    }) {
        return None;
    }
    // Exactly one active loan reaches the slot: the argument's, mutable.
    let mut reaching = loans
        .iter()
        .filter(|l| reaches(u.tables.loans[l.index()].address));
    let (Some(&loan), None) = (reaching.next(), reaching.next()) else {
        return None;
    };
    let info = &u.tables.loans[loan.index()];
    if info.address != address || !info.mutable {
        return None;
    }
    Some(ExclusiveSlot {
        callee,
        call: site.call,
        parameter: record.source,
        address,
        loan,
        slot,
        offset,
        layout: record.layout,
        size,
        align,
        normal,
        failure,
    })
}

type Fixed = (Vec<FrameParameter>, Vec<FrameRegion>);

/// Lay out every state over one fixed storage area from `base`.
fn states(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    sites: &[Site],
    base: u64,
) -> Result<(Vec<LiveStorage>, Vec<ResumeState>,Vec<CleanupResumeState>), String> {
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
        union.extend(slots.iter().copied().filter(|s| !parameter_slot(u, *s)));
        at_sites.push((slots, loans));
    }
    for (block,i) in cleanup::sites(u,table) {
        union.extend(storage_before(u,block,i).0.into_iter().filter(|s|!parameter_slot(u,*s)));
    }
    let slots: Vec<_> = union.into_iter().collect();
    let (offsets, base) = place(
        table,
        base,
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
        let values = live_across(u, &live, site.block);
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
        let exclusive = match (&site.exclusive, site.callee) {
            (None, _) => None,
            (Some(record), Some((body, _))) => Some(
                exclusive_edge(u, table, site, body, record, &slots, &loans, &storage)
                    .ok_or("exclusive argument outside this frame's slots")?,
            ),
            (Some(_), None) => return Err("exclusive argument without a nested frame".into()),
        };
        let tail = tail::derive(u, table, site.call, site.block, &slots, &loans, &storage)?;
        out.push(ResumeState {
            boundary: ExistingBoundary {
                block: site.block,
                call: site.call,
                operation: site.operation.clone(),
            },
            action: if tail.is_some() {
                ResumeAction::AfterEnteredCall(site.call, normal)
            } else {
                match callee {
                    Some(frame) => ResumeAction::ResumeNested(site.call, frame),
                    None => ResumeAction::RetryUnenteredCall(site.call),
                }
            },
            runtime: None,
            tail,
            callee,
            exclusive,
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
    let cleanups=cleanup::root_states(u,table,&storage,base, out.len() as u32+1)?;
    Ok((storage, out, cleanups))
}

/// The certificate binds everything the plan was derived from.
pub(crate) fn certify(
    u: &LirUnit<Ready>,
    inputs: &PlanInputs<'_>,
    effects: &[CallEffect],
    callee_digests: &BTreeMap<ContentHash, ContentHash>,
    representation: &Representation,
    (parameters, records): (&[FrameParameter], &[FrameRegion]),
    storage: &[LiveStorage],
    states: &[ResumeState],
    cleanups: &[CleanupResumeState],
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
    let tail =
        format!("{representation:?}{parameters:?}{records:?}{storage:?}{states:?}{cleanups:?}{captures:?}");
    bytes.extend_from_slice(&(tail.len() as u64).to_le_bytes());
    bytes.extend_from_slice(tail.as_bytes());
    ValidatedContinuation {
        identity: ContentHash::of_parts("hirsel.continuation.plan.v4", [bytes.as_slice()]),
    }
}

/// The boundaries of a closed body, in block order; Err refuses the compact form.
pub(crate) fn sites(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    effects: &[CallEffect],
    plans: &BTreeMap<ContentHash, ContinuationPlan>,
) -> Result<Vec<Site>, String> {
    let mut out = Vec::new();
    for (b, term) in u.terminators.iter().enumerate() {
        let Terminator::Call { call, .. } = term else {
            continue;
        };
        let (operation, callee, exclusive) = match &effects[call.index()] {
            CallEffect::Local => continue,
            CallEffect::Unknown(why) => return Err(why.clone()),
            CallEffect::Boundary(op) => (op.clone(), None, None),
            CallEffect::Nested(id) => match plans.get(id) {
                Some(p) if p.representation == Representation::Resumable => (
                    callee_symbol(u, &u.tables.calls[call.index()])
                        .unwrap()
                        .definition,
                    Some((*id, p.cost.frame_bytes)),
                    table
                        .fn_abis
                        .get(u.tables.calls[call.index()].abi.0 as usize)
                        .ok_or("a call outside the ABI table")?
                        .parameters
                        .iter()
                        .find(|p| p.passing == ParameterPassing::ExclusiveAddress)
                        .map(|p| ExclusiveParameter {
                            source: p.source,
                            layout: p.layout,
                        }),
                ),
                _ => return Err(format!("suspends inside stackful callee {id:?}")),
            },
        };
        out.push(Site {
            block: BlockId(b as u32),
            call: *call,
            operation,
            callee,
            exclusive,
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
    let built = sites(u, inputs.table, &effects, done).and_then(|sites| {
        if sites.is_empty() && cleanup::sites(u,inputs.table).is_empty() {
            return Ok((Fixed::default(), Vec::new(), Vec::new(),Vec::new()));
        }
        let (parameters, records, base) = fixed_area(u, inputs.table, inputs.target)?;
        let (storage, mut states, cleanups) = states(u, inputs.table, &sites, base)?;
        if scope::candidate(u) {
            let wrapper = scope::wrapper(u, units).ok_or("unproved scope callback or exit flow")?;
            for state in &mut states {
                wrapper.plan(u, inputs.table, state)?;
            }
        }
        Ok(((parameters, records), storage, states, cleanups))
    });
    let none = || (Fixed::default(), Vec::new(), Vec::new(),Vec::new());
    let (representation, (fixed, storage, states,cleanups)) = match built {
        Err(why) => (Representation::Stackful(why), none()),
        Ok((_, _, states,cleanups)) if states.is_empty() && cleanups.is_empty() => (Representation::Direct, none()),
        Ok((_, _, states,cleanups)) if states.iter().map(|s|s.bytes).chain(cleanups.iter().map(|s|s.bytes)).any(|bytes| bytes > MAX_FRAME_BYTES) => (
            Representation::Stackful("oversize live image".into()),none(),
        ),
        Ok(built) => (Representation::Resumable, built),
    };
    let cost = PlacementCost {
        instructions: (u.instructions.len() + u.terminators.len()) as u32,
        boundaries: (states.len()+cleanups.len()) as u32,
        frame_bytes: states.iter().map(|s| s.bytes).chain(cleanups.iter().map(|s|s.bytes)).max().unwrap_or(0),
        owned: states.iter().map(|s|&s.live).chain(cleanups.iter().map(|s|&s.live))
            .map(|live|live.iter().filter(|l|l.holding==Holding::Owned).count() as u32).max().unwrap_or(0),
    };
    let certificate = certify(
        u,
        inputs,
        &effects,
        digests,
        &representation,
        (&fixed.0, &fixed.1),
        &storage,
        &states,
        &cleanups,
        captures,
    );
    ContinuationPlan {
        body: u.header.instance,
        digest: u.digest(),
        representation,
        parameters: fixed.0,
        records: fixed.1,
        storage,
        states,
        cleanups,
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
        let suspends = suspending(&index, inputs.table, inputs.classify);
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
