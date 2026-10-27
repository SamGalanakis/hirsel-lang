//! The machine: frames and drains on an explicit activity stack, so neither
//! deep call chains nor deep release graphs use the Rust stack.
//!
//! Every SSA value carries its holder credits. An owning use consumes one; a
//! value that still holds a credit when its frame exits is a leaked holder (an
//! omitted decrement). After every step, each counted block's actual count is
//! compared with its independent physical-holder tally. Full recounts cross-check
//! the tally geometrically, on failures and at terminal audit: live value credits,
//! counted cells, harness roots and forget roots are its only inputs.
use crate::ledger::{
    AllocId, AllocState, Cell, Count, Event, Exemption, Extent, Ledger, Owner, Ptr, Resource,
    ResourceId, ResourceState, Token, TokenId,
};
use crate::observe::{OBSERVATION_SCHEMA_VERSION, Observation, Observed, ObservedFailure};
use crate::program::{
    Arg, Config, INJECTED_RULE, OpOutcome, OpValue, Operations, Program, SignalDelivery,
};
use crate::scalar;
use crate::snapshot::Snapshot;
use crate::violation::{Checked, Site, ViolationKind as V, err, fail};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{
    CopyRecipe, DropStep, LayoutId, LayoutKind, LayoutRecord, ScalarKind, SizeSpec, TagForm,
};
use hirsel_lir::runtime_abi::{self, FREE_POISON, UNINIT_POISON};
use hirsel_lir::stage::Stage;
use std::collections::BTreeMap;

mod codec;
mod compare;
mod flat;
mod primitive_work;
mod projection;
mod receipts;
mod sealed_drop;
mod sealed_graph;
mod sealed_read;
mod sealed_seal;
mod structural;
mod structural_values;
pub use flat::{FlatStorage, run_with_flat_storage_observer};
mod soa;
use hirsel_lir::layout::SoaPart;
use soa::ColumnRow;
mod fork;
pub(crate) use fork::campaign as fork_campaign;
mod channels;
mod clock;
mod holders;
mod isolate;
mod sync;
mod tasks;
mod vector;
mod versions;
mod views;
pub use vector::{rc_trace, rc_trace_with_config};

/// A run-time value. Pointers are checked handles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Val {
    Unit,
    Bits {
        scalar: ScalarKind,
        bits: u128,
    },
    Ptr(Ptr),
    /// A constructor-tail hole: the open block whose payload it fills. It is
    /// no holder of the block (LIR schema revision 59).
    Hole(Ptr),
    /// An inline aggregate, variant or resource: its bytes and cells.
    Image(Extent),
    /// A callable: its code unit (none for a generic nested function's
    /// value, LIR schema revision 9) and environment.
    Callable {
        code: Option<usize>,
        env: Option<Ptr>,
    },
    Handle(u64),
    /// An allocator token: None is the counted heap, Some names a ledger region.
    Arena(Option<usize>),
    Token(Option<TokenId>),
    Mark(usize),
    Mask(Vec<bool>),
    /// A storage version marker (`Store`/`Replace` `version`), never an operand.
    Version,
}

/// Why a value without credit may be used: a loan of this frame, or a borrow
/// the caller guarantees for the frame's lifetime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Borrow {
    Loan(LoanId),
    Caller,
    /// A borrowed parameter, also the provenance carried across a return.
    Parameter(u32),
    /// A cell of a borrowed inline argument. Retain its source offset so a
    /// returned view can recover the caller's original leaf provenance.
    ParameterCell(u32, u64),
}

#[derive(Clone, Debug)]
pub(crate) struct Held {
    pub val: Val,
    pub credits: u32,
    pub borrow: Option<Borrow>,
}

/// A value leaving its binding: whether it carried a credit.
struct Moved {
    val: Val,
    credited: bool,
    borrow: Option<Borrow>,
}

#[derive(Clone, Copy, Debug)]
enum SlotStore {
    Dead,
    Own(Ptr),
    /// A capture slot: field i of the closure environment.
    Alias(Ptr),
}

/// The normal edge of a returning call. A diverging call (LIR schema revision
/// 19) has none: its callee returning normally stops the frame.
fn returning(normal: Option<EdgeId>) -> Checked<EdgeId> {
    normal.ok_or_else(|| err(V::Malformed, "a diverging call returned normally"))
}

#[derive(Clone, Debug)]
enum After {
    Next,
    Operation { normal: Option<EdgeId> },
    Reset { result: ValueId, token: TokenId },
    RegionExit { region: usize },
}

#[derive(Clone, Debug)]
enum Resume {
    Running,
    Call {
        call: CallId,
        /// None for a diverging call: a normal return stops the frame.
        normal: Option<EdgeId>,
        failure: FailureId,
        /// Slots a `mut` argument names, with the write-back version the
        /// call defines for each on both of its edges.
        written: Vec<(SlotId, ValueId)>,
    },
    Drain {
        after: After,
        failure: Option<FailureId>,
    },
    /// A decode's checks (SER-1): the decoded `result`, held in the temporary `held` of
    /// its layout while each `pending` check (its unit and the occurrence it borrows,
    /// next last) runs.
    Checks {
        result: ValueId,
        held: Ptr,
        pending: Vec<(usize, Extent)>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    Entry,
    Callee,
    Finalizer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CallerSite {
    unit: usize,
    range: SiteRange,
}

/// Physical formal slots stage these credits while the language caller keeps
/// custody. Admission commits them once; rejection returns the identical values.
#[derive(Clone)]
struct PendingCustody {
    actuals: Vec<(ValueId, u32, Held)>,
    admitted: bool,
    handed: bool,
}

#[derive(Clone)]
pub(crate) struct Frame {
    pending_custody: Option<PendingCustody>,
    unit: usize,
    caller: Option<CallerSite>,
    operation: Option<CallerSite>,
    block: BlockId,
    statement: u32,
    values: Snapshot<Option<Held>>,
    slots: Snapshot<SlotStore>,
    column_aliases: Snapshot<Option<ColumnRow>>,
    versions: Snapshot<Option<ValueId>>,
    /// Where each prepared address's preparation located its place.
    prepared: Snapshot<Option<Located>>,
    /// Read-only inline SSA snapshots, one frame spill per static address.
    spills: Snapshot<Option<Ptr>>,
    loans: Snapshot<Option<Ptr>>,
    /// Actual Take reads of Arena parameter storage, independent of candidate grant uses.
    seal_grants: Vec<(ValueId, usize)>,
    sealed_loans: Snapshot<
        Option<(
            crate::ledger::sealed::SealedId,
            crate::ledger::sealed::SealedLoan,
        )>,
    >,
    failing: bool,
    admitted: u32,
    invocation_sites: Vec<(Option<CallerSite>, Option<CallerSite>)>,
    /// Actual ledger places whose cleanup activations have entered. This is
    /// independent of C's address/retirement map and survives shared suffixes.
    invocation_resources: Vec<Option<(Ptr, LayoutId)>>,
    /// H-owned runtime callback token, separate from candidate FrameEnter events.
    implicit_invocation: Option<bool>,
    env: Option<Ptr>,
    destination: Option<DestinationClass>,
    regions: Vec<usize>,
    resume: Resume,
    role: Role,
    /// Inline drop bodies entered and not yet ended (`FinalizeBegin`..`FinalizeEnd`).
    finalizing: u32,
    /// Set by a control transfer, so the instruction pointer does not advance.
    jumped: bool,
}

impl Frame {
    /// The frame runs destruction, which no cancellation check observes (CONC-3), as the
    /// runtime's finalizer shield suppresses them.
    fn shields(&self) -> bool {
        self.role == Role::Finalizer || self.finalizing > 0
    }
}

#[derive(Clone, Debug)]
enum Work {
    /// Drop the content at `at`. For a block-reference layout the mask gates the
    /// released block's payload fields; otherwise its own fields.
    DropAt {
        at: Ptr,
        layout: LayoutId,
        mask: Option<Vec<bool>>,
    },
    Step {
        base: Ptr,
        step: DropStep,
        logical_base: Ptr,
        layout: LayoutId,
        dead: Vec<Vec<ProjectionStep<u64>>>,
        child_mask: Option<Vec<bool>>,
    },
    SealedStorageDeath(AllocId),
    SealedOwnerDone {
        owner: crate::ledger::sealed::SealedId,
        control: Ptr,
    },
    Finalized(ResourceId),
    RetireCleanupInvocation(u32),
    /// The location's steps are done: its storage ends.
    Done {
        at: Ptr,
        size: u64,
        layout: LayoutId,
    },
    /// A dying block's payload is drained: free it, or keep it as raw storage.
    Death {
        alloc: AllocId,
        raw: Option<TokenId>,
    },
}

#[derive(Clone)]
pub(crate) struct Drain {
    caller: Option<CallerSite>,
    operation: Option<CallerSite>,
    work: Vec<Work>,
    failed: bool,
}

#[derive(Clone)]
enum Activity {
    Frame(Box<Frame>),
    Drain(Drain),
}

enum Exit {
    RejectTransfer(Vec<(ValueId, Held)>),
    Return(Option<(Held, LirTypeId)>),
    Fail,
}

enum Control {
    Continue,
    Push(Activity),
    Exit(Exit),
}

/// The entry's result, before the terminal audit.
#[derive(Clone)]
enum Finished {
    Returned(Option<(Held, LayoutId, ValueRepr)>),
    Failed,
}

pub(crate) struct Machine<'p, S> {
    invocation_events: BTreeMap<u32, Vec<bool>>,
    receipts: BTreeMap<AllocId, receipts::SnapshotValue>,
    seal_inventory: BTreeMap<usize, BTreeMap<AllocId, sealed_graph::Entry>>,
    seal_allocations: BTreeMap<AllocId, (usize, SourceAnchor)>,
    seal_sources_checked: bool,
    program: &'p Program<S>,
    config: &'p Config,
    ops: &'p mut dyn Operations,
    pub ledger: Ledger,
    stack: Vec<Activity>,
    entry: usize,
    pub output: Vec<u8>,
    failures: Vec<ObservedFailure>,
    unanchored_failures: std::collections::BTreeSet<usize>,
    pub sites: u64,
    /// Timer choice points executed: the early-firing domain ([`clock`]).
    pub choices: u64,
    pub(crate) steps: u64,
    /// The kind (`SignalKind` ordinal) whose default action ended the run.
    pub signaled: Option<u32>,
    runtime_iterators: Snapshot<std::sync::Arc<[OpValue]>>,
    lazy_iterators: Snapshot<OpValue>,
    version_slots: std::sync::Arc<Vec<Vec<Option<SlotId>>>>,
    map_storage: std::sync::Arc<hirsel_lir::logical_storage::MapStorage>,
    statics: BTreeMap<(usize, u32), Ptr>,
    constants: BTreeMap<(usize, u32), Ptr>,
    roots: BTreeMap<u64, Held>,
    forgotten: Snapshot<Held>,
    finished: Option<Finished>,
    /// A failure ended the run where it stood (FAIL-3: a serializable check's failure):
    /// nothing is cleaned up, so the terminal audit does not run.
    ended: bool,
    terminal: Option<Observation>,
    word: u64,
    /// Nesting of loan re-resolution: a loan whose address reads a value
    /// borrowed under itself would otherwise never terminate.
    resolving: u32,
    holders: holders::Holders,
    physical: holders::Physical,
    full_recount_due: bool,
    arena_no_drop: Snapshot<()>,
    /// Constructor-tail blocks whose payload is still open (LIR schema
    /// revision 59): their release frees the storage alone, unread.
    open_holes: Snapshot<()>,
    /// Each flat list builder with an open element, and the field its bytes
    /// are built in (LIR schema revision 24).
    open_elements: BTreeMap<AllocId, u32>,
    /// The structured-task scheduler, from the first scope on (CONC-1).
    tasks: Option<Box<tasks::Tasks>>,
}

/// What a resolved address names.
struct Resolved {
    columns: Option<ColumnRow>,
    ptr: Ptr,
    layout: LayoutId,
    /// Counted blocks traversed, outermost first; all must be unique to write.
    path: Vec<AllocId>,
    root_slot: Option<SlotId>,
    /// Where the elements of the variable-element flat sequence the last
    /// step indexed begin (`FlatNext` and `FlatAt` offsets count from it).
    flat_content: Option<Ptr>,
    /// End boundary independently read by H for a terminal FlatAt.
    flat_end: Option<u64>,
}

impl Resolved {
    fn located(&self) -> Located {
        if let Some(row) = self.columns {
            return Located::Columns(
                row.base.alloc,
                row.base.generation,
                row.base.offset,
                row.capacity,
                row.index,
            );
        }
        if self.path.is_empty() {
            Located::Frame(self.ptr.offset)
        } else {
            Located::Heap(self.ptr.alloc, self.ptr.generation, self.ptr.offset)
        }
    }
}

/// The location an address's preparation computed, which native code keeps
/// and reuses at every later use (`hirsel-llvm` `prepare`). A place inside a
/// counted block is that block's handle and offset; one in frame storage is
/// its offset, since a native slot's storage never moves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Located {
    Columns(AllocId, u32, u64, u64, u64),
    Heap(AllocId, u32, u64),
    Frame(u64),
    /// An unchecked address whose root did not resolve when it was prepared.
    Unresolved,
}

/// A language failure detected while resolving an address (bounds or tag).
struct LangFailure;

fn tracked(repr: ValueRepr) -> bool {
    matches!(
        repr,
        ValueRepr::Block
            | ValueRepr::PayloadReceipt
            | ValueRepr::Aggregate
            | ValueRepr::Callable
            | ValueRepr::Handle
            | ValueRepr::Token
            | ValueRepr::Hole
    )
}

/// A count a write may pass: one holder, or an unshared tracked arena block
/// (`IsWritable`, LIR schema revision 32).
fn writable(count: Option<Count>) -> bool {
    matches!(
        count,
        Some(Count::Finite(1) | Count::Arena { shared: false })
    )
}

fn mask_bit(mask: &Option<Vec<bool>>, i: usize) -> bool {
    mask.as_ref()
        .is_none_or(|m| m.get(i).copied().unwrap_or(false))
}

impl<'p, S: Stage> Machine<'p, S> {
    pub fn new(program: &'p Program<S>, config: &'p Config, ops: &'p mut dyn Operations) -> Self {
        let mut ledger = Ledger::default();
        for (handle, what) in &config.context_handles {
            ledger
                .handles
                .insert(*handle, (crate::ledger::HandleState::Open, true));
            ledger.exemptions.push(Exemption::Context {
                handle: *handle,
                what: what.clone(),
            });
        }
        Self {
            program,
            config,
            ops,
            ledger,
            stack: Vec::new(),
            invocation_events: BTreeMap::new(),
            receipts: BTreeMap::new(),
            seal_inventory: BTreeMap::new(),
            seal_allocations: BTreeMap::new(),
            seal_sources_checked: false,
            entry: 0,
            output: Vec::new(),
            failures: Vec::new(),
            unanchored_failures: std::collections::BTreeSet::new(),
            sites: 0,
            steps: 0,
            map_storage: std::sync::Arc::new(hirsel_lir::logical_storage::MapStorage::new(
                &program.layouts,
            )),
            runtime_iterators: Default::default(),
            lazy_iterators: Default::default(),
            version_slots: std::sync::Arc::new(program.units.iter().map(versions::slots).collect()),
            statics: BTreeMap::new(),
            constants: BTreeMap::new(),
            roots: BTreeMap::new(),
            forgotten: Default::default(),
            finished: None,
            ended: false,
            terminal: None,
            word: u64::from(program.target.pointer_bytes),
            resolving: 0,
            holders: Default::default(),
            physical: Default::default(),
            arena_no_drop: Default::default(),
            open_holes: Default::default(),
            open_elements: BTreeMap::new(),
            tasks: None,
            choices: 0,
            signaled: None,
            full_recount_due: false,
        }
    }

    // ---- Program access -------------------------------------------------

    fn unit(&self, index: usize) -> &'p LirUnit<S> {
        &self.program.units[index]
    }
    fn record(&self, id: LayoutId) -> Checked<&'p LayoutRecord> {
        self.program
            .layouts
            .layout(id)
            .ok_or_else(|| err(V::Malformed, format!("unknown layout {id:?}")))
    }
    /// The bytes a value of `id` occupies in storage: a Packed value's
    /// largest variant (`SizeSpec::storage`).
    fn size(&self, id: LayoutId) -> Checked<u64> {
        match self.record(id)?.size.storage() {
            Some(n) => Ok(n),
            None => fail(V::Unsupported, format!("dynamic size of {id:?}")),
        }
    }
    fn lir_type(&self, unit: usize, id: LirTypeId) -> Checked<&'p LirType> {
        self.unit(unit)
            .types
            .get(id.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown type {id:?}")))
    }
    fn value_type(&self, unit: usize, v: ValueId) -> Checked<&'p LirType> {
        let info = self.value_info(unit, v)?;
        self.lir_type(unit, info.ty)
    }
    fn value_info(&self, unit: usize, v: ValueId) -> Checked<&'p ValueInfo> {
        self.unit(unit)
            .values
            .get(v.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown value {v:?}")))
    }
    fn pool<'a, T>(&self, range: PoolRange<T>, pool: &'a [T]) -> Checked<&'a [T]> {
        range
            .get(pool)
            .ok_or_else(|| err(V::Malformed, "pool range out of bounds"))
    }
    fn site(&self, f: &Frame) -> Site {
        Site {
            definition: self.unit(f.unit).header.definition.clone(),
            block: f.block.0,
            statement: f.statement,
        }
    }
    /// Count one execution of a failure-capable site; true if it is injected.
    fn injection_site(&mut self) -> bool {
        let index = self.sites;
        self.sites += 1;
        let injected = self.config.inject == Some(index);
        if injected {
            self.ledger.trace.push(Event::Injection { site: index });
        }
        self.full_recount_due |= injected;
        injected
    }

    // ---- Frames and values ---------------------------------------------

    fn frame(&self, unit: usize, role: Role, env: Option<Ptr>) -> Frame {
        let u = self.unit(unit);
        Frame {
            pending_custody: None,
            unit,
            caller: None,
            operation: None,
            block: u.entry,
            statement: 0,
            values: (0..u.values.len()).map(|_| None).collect(),
            slots: u.slots.iter().map(|_| SlotStore::Dead).collect(),
            column_aliases: (0..u.slots.len()).map(|_| None).collect(),
            versions: (0..u.slots.len()).map(|_| None).collect(),
            prepared: (0..u.addresses.len()).map(|_| None).collect(),
            spills: (0..u.addresses.len()).map(|_| None).collect(),
            loans: (0..u.tables.loans.len()).map(|_| None).collect(),
            seal_grants: Vec::new(),
            sealed_loans: (0..u.tables.loans.len()).map(|_| None).collect(),
            failing: false,
            admitted: 0,
            invocation_sites: Vec::new(),
            invocation_resources: Vec::new(),
            implicit_invocation: None,
            env,
            destination: None,
            regions: Vec::new(),
            resume: Resume::Running,
            role,
            finalizing: 0,
            jumped: false,
        }
    }

    /// Bind capture slots to their environment fields.
    fn bind_captures(&mut self, f: &mut Frame) -> Checked<()> {
        let u = self.unit(f.unit);
        for (index, slot) in u.slots.iter().enumerate() {
            if let SlotKind::Capture(i) = slot.kind {
                let version = *u
                    .captures
                    .get(i as usize)
                    .ok_or_else(|| err(V::Malformed, format!("no version for capture {i}")))?;
                let Some(env) = f.env else {
                    // A zero-sized environment is no block (`closure`): each of
                    // its captures is zero-sized, with empty frame storage.
                    let layout = self.lir_type(f.unit, slot.ty)?.layout;
                    if self.size(layout)? != 0 {
                        return fail(V::Malformed, "capture slot in a call without environment");
                    }
                    let align = self.record(layout)?.align;
                    let ptr = self
                        .ledger
                        .alloc(Owner::Frame, Some(layout), 0, 0, align, None);
                    f.slots[index] = SlotStore::Own(ptr);
                    f.versions[index] = Some(version);
                    self.define_version(f, version)?;
                    continue;
                };
                let layout = self
                    .ledger
                    .check(env)?
                    .layout
                    .ok_or_else(|| err(V::Malformed, "environment without layout"))?;
                let field = self
                    .record(layout)?
                    .fields
                    .iter()
                    .find(|fl| fl.ordinal == i)
                    .ok_or_else(|| err(V::Malformed, format!("no capture {i}")))?;
                // The slot is the field: a borrowed capture's slot has the
                // captured view's layout, as the field does.
                if self.lir_type(f.unit, slot.ty)?.layout != field.layout {
                    return fail(
                        V::TypeConfusion,
                        format!("capture slot {i} is not its environment field's layout"),
                    );
                }
                f.slots[index] = SlotStore::Alias(self.ledger.project(
                    env,
                    field.offset,
                    ProjectionStep::Field(field.ordinal),
                ));
                f.versions[index] = Some(version);
                self.define_version(f, version)?;
            }
        }
        Ok(())
    }

    /// Bind parameter slots, as the validator models them: live and
    /// initialized at entry, holding the parameter's value under the
    /// parameter's own name as their version. A parameter passed by address
    /// aliases the caller's place; one passed by value moves into frame
    /// storage, so the parameter name is from then on a version only.
    fn bind_parameter_slots(
        &mut self,
        f: &mut Frame,
        by_address: &[Option<(Ptr, Option<ColumnRow>)>],
    ) -> Checked<()> {
        let u = self.unit(f.unit);
        let Some(abi) = self.program.layouts.fn_abis.get(u.header.abi.0 as usize) else {
            return Ok(());
        };
        for (i, param) in abi.parameters.iter().enumerate() {
            let Some(index) = u
                .slots
                .iter()
                .position(|s| s.kind == SlotKind::Parameter(param.source))
            else {
                continue;
            };
            let Some(&p) = u.parameters.get(i) else {
                return fail(V::Malformed, format!("no parameter value {i}"));
            };
            f.versions[index] = Some(p);
            if let Some(Some((ptr, columns))) = by_address.get(i) {
                f.column_aliases[index] = *columns;
                f.slots[index] = SlotStore::Alias(*ptr);
                // The caller's place is the slot now: the parameter value
                // names its first version, which an edge may join.
                if let Some(held) = f.values[p.index()].as_mut() {
                    self.holders.value(&held.val, -i64::from(held.credits));
                    *held = Held {
                        val: Val::Version,
                        credits: 0,
                        borrow: None,
                    };
                }
                continue;
            }
            let layout = self.lir_type(f.unit, u.slots[index].ty)?.layout;
            let size = self.size(layout)?;
            let align = self.record(layout)?.align;
            let ptr = self
                .ledger
                .alloc(Owner::Frame, Some(layout), size, size, align, None);
            f.slots[index] = SlotStore::Own(ptr);
            let held = std::mem::replace(
                f.values[p.index()]
                    .as_mut()
                    .ok_or_else(|| err(V::Malformed, format!("unbound parameter {p:?}")))?,
                Held {
                    val: Val::Version,
                    credits: 0,
                    borrow: None,
                },
            );
            self.holders.value(&held.val, -i64::from(held.credits));
            let moved = Moved {
                val: held.val,
                credited: held.credits > 0,
                borrow: held.borrow,
            };
            // A borrowed parameter is a view: the caller keeps the resources
            // it shows (a drop body called explicitly borrows its resource).
            let view = param.convention == hirsel_core_contract::Passing::Borrow;
            self.write_value_as(ptr, layout, moved, view)?;
        }
        Ok(())
    }

    fn get<'f>(&self, f: &'f Frame, v: ValueId) -> Checked<&'f Held> {
        f.values
            .get(v.index())
            .and_then(Option::as_ref)
            .ok_or_else(|| err(V::Malformed, format!("use of undefined value {v:?}")))
    }

    /// Whether a value still owns something that must be released.
    fn carries(&self, val: &Val) -> bool {
        match val {
            Val::Ptr(p) => self.counted_target(*p),
            Val::Image(e) => {
                e.cells
                    .values()
                    .any(|c| matches!(c, Cell::Data { counted: true, .. }))
                    || e.resources.values().any(|r| {
                        let r = &self.ledger.resources[r.0 as usize];
                        r.state != ResourceState::Dead && !r.forgotten
                    })
            }
            Val::Callable { env: Some(p), .. } => self.counted_target(*p),
            Val::Token(Some(_)) => true,
            Val::Handle(h) => self
                .ledger
                .handles
                .get(h)
                .is_some_and(|(s, _)| *s == crate::ledger::HandleState::Open),
            _ => false,
        }
    }

    /// A finite-count heap block, a `Shared` root, or a block of a frozen graph,
    /// whose copies a credit counts: one that a credit actually keeps alive.
    fn counted_target(&self, p: Ptr) -> bool {
        self.ledger
            .allocations
            .get(p.alloc.0 as usize)
            .is_some_and(|a| {
                a.owner == Owner::Heap
                    && matches!(
                        a.count,
                        Some(Count::Finite(_) | Count::Root(_) | Count::Frozen { .. })
                    )
            })
    }

    /// A block a write through an address must find unique: a counted one,
    /// including a block of a frozen graph, which no write may reach (MEM-11),
    /// or a tracked arena block (MEM-6).
    fn guarded_target(&self, p: Ptr) -> bool {
        self.counted_target(p)
            || self
                .ledger
                .allocations
                .get(p.alloc.0 as usize)
                .is_some_and(|a| matches!(a.count, Some(Count::Arena { .. })))
    }

    fn define(&mut self, f: &mut Frame, v: ValueId, held: Held) -> Checked<()> {
        let slot = f
            .values
            .get_mut(v.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown value {v:?}")))?;
        if let Some(old) = slot.take() {
            self.holders.value(&old.val, -i64::from(old.credits));
            if old.credits > 0 && self.carries(&old.val) {
                return fail(
                    V::LeakedCredit,
                    format!("{v:?} redefined while it still held a credit"),
                );
            }
        }
        self.holders.value(&held.val, i64::from(held.credits));
        f.values[v.index()] = Some(held);
        Ok(())
    }

    /// Bind a storage version marker.
    fn define_version(&mut self, f: &mut Frame, version: ValueId) -> Checked<()> {
        self.define(
            f,
            version,
            Held {
                val: Val::Version,
                credits: 0,
                borrow: None,
            },
        )
    }

    /// Bind a fresh result: owned tracked values start with one credit.
    /// Define a scalar operation's flag, present exactly when the operation reports one.
    fn define_flag(
        &mut self,
        f: &mut Frame,
        flag: Option<ValueId>,
        value: Option<(ScalarKind, u128)>,
    ) -> Checked<()> {
        match (flag, value) {
            (Some(v), Some((scalar, bits))) => self.define_fresh(f, v, Val::Bits { scalar, bits }),
            (None, None) => Ok(()),
            _ => fail(V::Malformed, "a scalar flag the operation does not report"),
        }
    }

    fn define_fresh(&mut self, f: &mut Frame, v: ValueId, val: Val) -> Checked<()> {
        let info = self.value_info(f.unit, v)?;
        let ty = self.lir_type(f.unit, info.ty)?;
        let (credits, borrow) = match info.holding {
            Holding::Owned if tracked(ty.repr) => (1, None),
            Holding::Borrowed(l) => (0, Some(Borrow::Loan(l))),
            _ => (0, None),
        };
        self.define(
            f,
            v,
            Held {
                val,
                credits,
                borrow,
            },
        )
    }

    /// Bind a moved value, keeping its credit or borrow.
    fn bind(&mut self, f: &mut Frame, v: ValueId, m: Moved) -> Checked<()> {
        let info = self.value_info(f.unit, v)?;
        let ty = self.lir_type(f.unit, info.ty)?;
        if info.holding == Holding::Owned
            && tracked(ty.repr)
            && !m.credited
            && self.carries_ref(&m.val)
        {
            return fail(
                V::CreditUnderflow,
                format!("owned {v:?} received a value without a holder credit"),
            );
        }
        let borrow = m.borrow.or(match info.holding {
            Holding::Borrowed(l) => Some(Borrow::Loan(l)),
            _ => None,
        });
        self.define(
            f,
            v,
            Held {
                val: m.val,
                credits: u32::from(m.credited && tracked(ty.repr)),
                borrow,
            },
        )
    }

    /// Whether a value references counted storage (credited or not).
    fn carries_ref(&self, val: &Val) -> bool {
        match val {
            Val::Ptr(p) | Val::Callable { env: Some(p), .. } => self.counted_target(*p),
            Val::Image(e) => e.cells.values().any(|c| match c {
                Cell::Data { ptr, .. } => self.counted_target(*ptr),
                Cell::Code(_) | Cell::Arena(_) => false,
            }),
            Val::Token(Some(_)) => true,
            _ => false,
        }
    }

    /// Every handle a value contains is live at its generation.
    fn check_val(&self, val: &Val) -> Checked<()> {
        match val {
            Val::Handle(h)
                if self
                    .ledger
                    .handles
                    .get(h)
                    .is_some_and(|(state, _)| *state == crate::ledger::HandleState::Closed) =>
            {
                fail(V::UseAfterFree, format!("closed handle {h} used again"))
            }
            Val::Ptr(p) | Val::Callable { env: Some(p), .. } => self.ledger.check(*p).map(|_| ()),
            Val::Image(e) => {
                for cell in e.cells.values() {
                    if let Cell::Data { ptr, .. } = cell {
                        self.ledger.check(*ptr)?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn check_borrow(&mut self, f: &Frame, borrow: Borrow) -> Checked<()> {
        let Borrow::Loan(loan) = borrow else {
            return Ok(());
        };
        let Some(backing) = f.loans.get(loan.index()).copied().flatten() else {
            return fail(
                V::BorrowExpired,
                format!("borrow of {loan:?} used while the loan is not active"),
            );
        };
        let info = self
            .unit(f.unit)
            .tables
            .loans
            .get(loan.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown loan {loan:?}")))?;
        if self.resolving > 64 {
            return fail(
                V::Malformed,
                format!("{loan:?}'s address depends on itself"),
            );
        }
        self.resolving += 1;
        let resolved = self.resolve(f, info.address);
        self.resolving -= 1;
        let now = match resolved {
            Ok(Ok(r)) => Some(r.ptr),
            Ok(Err(LangFailure)) => None,
            Err(e) if e.kind == V::Malformed => return Err(e),
            Err(_) => None,
        };
        if now != Some(backing) {
            return fail(
                V::ViewDetached,
                format!(
                    "view under {loan:?} used after its backing owner changed (was {:?}, now {:?})",
                    backing, now
                ),
            );
        }
        Ok(())
    }

    /// Check returned provenance against the ABI, independently of C. Only
    /// loans of declared Borrow parameters end after every returned cell is checked.
    fn returned_borrow(
        &mut self,
        f: &mut Frame,
        borrow: Borrow,
        allowed: &[u32],
    ) -> Checked<Borrow> {
        let parameter = match borrow {
            Borrow::Loan(loan) => {
                self.check_borrow(f, borrow)?;
                let address = self.unit(f.unit).tables.loans[loan.index()].address;
                let root = self.resolve_use(f, address)?.root_slot;
                let Some(slot) = root else {
                    return fail(V::BorrowedReturn, "returned loan has no parameter root");
                };
                let SlotKind::Parameter(parameter) = self.unit(f.unit).slots[slot.index()].kind
                else {
                    return fail(V::BorrowedReturn, "returned loan names local storage");
                };
                if !allowed.contains(&parameter) {
                    return fail(
                        V::BorrowedReturn,
                        "returned loan names an undeclared parameter",
                    );
                }
                parameter
            }
            Borrow::Parameter(parameter) | Borrow::ParameterCell(parameter, _) => parameter,
            Borrow::Caller => {
                return fail(
                    V::BorrowedReturn,
                    "returned view has no parameter provenance",
                );
            }
        };
        let abi = &self.program.layouts.fn_abis[self.unit(f.unit).header.abi.0 as usize];
        if !allowed.contains(&parameter)
            || abi.parameters[parameter as usize].convention
                != hirsel_core_contract::Passing::Borrow
        {
            return fail(
                V::BorrowedReturn,
                "returned view is not from a declared Borrow parameter",
            );
        }
        Ok(match borrow {
            Borrow::ParameterCell(_, _) => borrow,
            _ => Borrow::Parameter(parameter),
        })
    }

    /// A read-only use: the value is still held (not consumed) and valid.
    fn read_use(&mut self, f: &Frame, v: ValueId) -> Checked<Val> {
        let held = self.get(f, v)?.clone();
        if matches!(held.val, Val::Version) {
            return fail(
                V::Malformed,
                format!("{v:?} is a storage version, not a value"),
            );
        }
        if let Some(b) = held.borrow {
            self.check_borrow(f, b)?;
        } else if held.credits == 0 && self.carries_ref(&held.val) {
            let info = self.value_info(f.unit, v)?;
            if info.holding == Holding::Owned {
                if matches!(held.val, Val::Token(_)) {
                    return fail(
                        V::TokenReused,
                        format!("token {v:?} used after it was consumed"),
                    );
                }
                return fail(
                    V::CreditUnderflow,
                    format!("{v:?} used after its last credit was consumed"),
                );
            }
        }
        if let Val::Image(image) = &held.val {
            for cell in image.cells.values() {
                if let Cell::Data {
                    borrow: Some(borrow),
                    ..
                } = cell
                {
                    self.check_borrow(f, *borrow)?;
                }
            }
        }
        self.check_val(&held.val)?;
        Ok(held.val)
    }

    /// An owning use: consume one credit if the value has one.
    fn take_value(&mut self, f: &mut Frame, v: ValueId) -> Checked<Moved> {
        let val = self.read_use(f, v)?;
        let held = f.values[v.index()].as_mut().expect("checked defined");
        if held.credits > 0 {
            held.credits -= 1;
            self.holders.value(&val, -1);
            return Ok(Moved {
                val,
                credited: true,
                borrow: None,
            });
        }
        let borrow = held.borrow;
        if matches!(val, Val::Token(_)) {
            return fail(V::TokenReused, format!("token {v:?} consumed twice"));
        }
        Ok(Moved {
            val,
            credited: false,
            borrow,
        })
    }

    /// An owning use that must transfer a credit when the value holds counted storage.
    /// Consume a hole: its one credit, never a second use.
    fn take_hole(&mut self, f: &mut Frame, v: ValueId, what: &str) -> Checked<Ptr> {
        match self.take_value(f, v)? {
            Moved {
                val: Val::Hole(target),
                credited: true,
                ..
            } => Ok(target),
            Moved {
                val: Val::Hole(_), ..
            } => fail(
                V::CreditUnderflow,
                format!("hole {v:?} consumed twice at its {what}"),
            ),
            _ => fail(
                V::TypeConfusion,
                format!("{what} of {v:?}, which is no hole"),
            ),
        }
    }

    fn take_owned(&mut self, f: &mut Frame, v: ValueId, what: &str) -> Checked<Moved> {
        let m = self.take_value(f, v)?;
        if !m.credited && self.carries_ref(&m.val) {
            return fail(
                V::CreditUnderflow,
                format!("{what} of {v:?}, which holds no credit"),
            );
        }
        Ok(m)
    }

    // ---- Addresses -----------------------------------------------------

    fn checked_steps(&self, unit: usize, address: AddressId) -> Checked<bool> {
        let u = self.unit(unit);
        let info = u
            .addresses
            .get(address.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown address {address:?}")))?;
        let steps = self.pool(info.projection, &u.pools.projections)?;
        let own = steps.iter().enumerate().any(|(i, s)| {
            !info.unchecked.contains(&(i as u32))
                && !matches!(s, ProjectionStep::Field(_) | ProjectionStep::Slot(_))
        });
        Ok(own
            || match info.base {
                AddressBase::Address(a) => self.checked_steps(unit, a)?,
                _ => false,
            })
    }

    /// Mirror the backends' frame spill without giving borrowed cells credits.
    fn prepare_spill(&mut self, f: &mut Frame, mut address: AddressId) -> Checked<()> {
        loop {
            match self.unit(f.unit).addresses[address.index()].base {
                AddressBase::Address(base) => address = base,
                AddressBase::Slot(_) => return Ok(()),
                AddressBase::Value(value) => {
                    let ty = self.value_type(f.unit, value)?;
                    let layout = ty.layout;
                    if !matches!(
                        ty.repr,
                        ValueRepr::Aggregate
                            | ValueRepr::Callable
                            | ValueRepr::Scalar(_)
                            | ValueRepr::Handle
                    ) {
                        return Ok(());
                    }
                    if !matches!(
                        self.record(layout)?.copy,
                        CopyRecipe::Bits | CopyRecipe::Dup | CopyRecipe::Fields
                    ) {
                        return fail(V::TypeConfusion, "a spill of a non-copyable value");
                    }
                    let val = self.read_use(f, value)?;
                    let borrow = f.values[value.index()]
                        .as_ref()
                        .and_then(|held| held.borrow);
                    let size = self.size(layout)?;
                    let ptr = if let Some(ptr) = f.spills[address.index()] {
                        self.ledger
                            .get_mut(ptr.alloc)
                            .extent
                            .take(0, size, UNINIT_POISON, None);
                        ptr
                    } else {
                        let align = self.record(layout)?.align;
                        let ptr =
                            self.ledger
                                .alloc(Owner::Frame, Some(layout), size, size, align, None);
                        f.spills[address.index()] = Some(ptr);
                        ptr
                    };
                    self.write_value(
                        ptr,
                        layout,
                        Moved {
                            val,
                            credited: false,
                            borrow,
                        },
                    )?;
                    return Ok(());
                }
            }
        }
    }

    /// Resolve an address now, running its bounds and tag checks.
    fn resolve(&mut self, f: &Frame, address: AddressId) -> Checked<Result<Resolved, LangFailure>> {
        let u = self.unit(f.unit);
        let info = u
            .addresses
            .get(address.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown address {address:?}")))?;
        let mut path = Vec::new();
        let mut root_slot = None;
        let mut flat_content = None;
        let mut flat_end = None;
        let mut columns = None;
        let (mut ptr, mut layout) = match info.base {
            AddressBase::Slot(s) => {
                columns = f.column_aliases[s.index()];
                let slot = u
                    .slots
                    .get(s.index())
                    .ok_or_else(|| err(V::Malformed, format!("unknown slot {s:?}")))?;
                let layout = self.lir_type(f.unit, slot.ty)?.layout;
                root_slot = Some(s);
                match f.slots[s.index()] {
                    SlotStore::Own(p) | SlotStore::Alias(p) => (p, layout),
                    SlotStore::Dead => {
                        return fail(V::UninitializedRead, format!("{s:?} is not live"));
                    }
                }
            }
            AddressBase::Value(v) => {
                let ty = self.value_type(f.unit, v)?;
                let val = self.read_use(f, v)?;
                let (p, layout) = match self.record(ty.layout)?.kind {
                    LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                        let Val::Ptr(p) = val else {
                            return fail(
                                V::TypeConfusion,
                                format!("address base {v:?} is not a pointer"),
                            );
                        };
                        (p, payload)
                    }
                    _ if matches!(
                        ty.repr,
                        ValueRepr::Aggregate
                            | ValueRepr::Callable
                            | ValueRepr::Scalar(_)
                            | ValueRepr::Handle
                    ) =>
                    {
                        (
                            f.spills[address.index()].ok_or_else(|| {
                                err(V::Malformed, "inline value address not prepared")
                            })?,
                            ty.layout,
                        )
                    }
                    _ => {
                        let Val::Ptr(p) = val else {
                            return fail(
                                V::TypeConfusion,
                                "a pointer address base is not a pointer",
                            );
                        };
                        (p, ty.layout)
                    }
                };
                if self.guarded_target(p) {
                    path.push(p.alloc);
                }
                (p, layout)
            }
            AddressBase::Address(a) => match self.resolve(f, a)? {
                Ok(r) => {
                    path = r.path;
                    root_slot = r.root_slot;
                    flat_content = r.flat_content;
                    flat_end = r.flat_end;
                    columns = r.columns;
                    (r.ptr, r.layout)
                }
                Err(e) => return Ok(Err(e)),
            },
        };
        for (ordinal, step) in self
            .pool(info.projection, &u.pools.projections)?
            .iter()
            .enumerate()
        {
            let unchecked = info.unchecked.contains(&(ordinal as u32));
            // The cached end belongs to the element, not any projected field.
            flat_end = None;
            // Payload is one explicit Box dereference, even when the payload is another Box.
            if *step == ProjectionStep::Payload {
                let LayoutKind::Box { payload } = self.record(layout)?.kind else {
                    return fail(V::TypeConfusion, "Payload requires a Box address");
                };
                let target = self.read_ptr(ptr)?;
                self.ledger.check(target)?;
                if self.guarded_target(target) {
                    path.push(target.alloc);
                }
                ptr = target;
                layout = payload;
                continue;
            }
            // Projecting through a block reference dereferences it first; through a
            // `Shared` root, into its `Frozen` payload, whose root count then guards
            // every write (MEM-11).
            if let LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } = self.record(layout)?.kind
            {
                let target = self.read_ptr(ptr)?;
                self.ledger.check(target)?;
                if self.guarded_target(target) {
                    path.push(target.alloc);
                }
                ptr = target;
                layout = payload;
            }
            if let (Some(row), ProjectionStep::Field(ordinal)) = (columns, *step) {
                let shape = self.column_shape(layout)?;
                let Some(span) = shape.field(ordinal) else {
                    return fail(V::Malformed, "column field absent");
                };
                let SoaPart::Field { layout: field, .. } = span.part else {
                    unreachable!("a field span")
                };
                let row_at = self
                    .ledger
                    .project(row.base, 0, ProjectionStep::Index(row.index));
                ptr = self.ledger.project(
                    row_at,
                    shape.data_offset + row.capacity * span.prefix + row.index * span.stride,
                    ProjectionStep::Field(ordinal),
                );
                layout = field;
                columns = None;
                continue;
            }
            // A variant row's payload field lies in its payload column, read only under
            // the arm its tag column names.
            if let (Some(row), ProjectionStep::VariantField { variant, field }) = (columns, *step) {
                let row = Resolved {
                    ptr,
                    layout,
                    columns: Some(row),
                    path: path.clone(),
                    root_slot,
                    flat_content: None,
                    flat_end: None,
                };
                match self.column_variant_field(&row, variant, field, unchecked)? {
                    Ok((at, field)) => {
                        ptr = at;
                        layout = field;
                    }
                    Err(failure) => return Ok(Err(failure)),
                }
                columns = None;
                continue;
            }
            let record = self.record(layout)?;
            let logical = (*step).try_map_index(|value| match self.read_use(f, value)? {
                Val::Bits { scalar, bits } if scalar::integer(scalar).is_some() => {
                    u64::try_from(bits)
                        .map_err(|_| err(V::Malformed, "projection index exceeds u64"))
                }
                _ => fail(V::TypeConfusion, "projection index is not an integer"),
            })?;
            match *step {
                ProjectionStep::Payload => unreachable!("handled before implicit dereference"),
                ProjectionStep::Field(i) => {
                    let field = record
                        .fields
                        .iter()
                        .find(|fl| fl.ordinal == i)
                        .ok_or_else(|| err(V::Malformed, format!("no field {i} in {layout:?}")))?;
                    // A flat field lies at its cursor: further than its static
                    // offset by each earlier variable field's u64 prefix value.
                    let mut cursor = 0;
                    if record.kind == LayoutKind::Flat {
                        let mut earlier: Vec<_> = record
                            .fields
                            .iter()
                            .filter(|fl| fl.offset < field.offset)
                            .collect();
                        earlier.sort_by_key(|fl| fl.offset);
                        for fl in earlier {
                            if matches!(self.record(fl.layout)?.size, SizeSpec::Dynamic(_)) {
                                let prefix = self.read_scalar(ptr.at(fl.offset + cursor), 8)?;
                                cursor += u64::try_from(prefix).map_err(|_| {
                                    err(V::Malformed, "a flat length prefix beyond u64")
                                })?;
                            }
                        }
                    }
                    ptr = self.ledger.project(ptr, field.offset + cursor, logical);
                    layout = field.layout;
                }
                ProjectionStep::VariantField { variant, field } => {
                    let tag = record.tag.as_ref().ok_or_else(|| {
                        err(V::Malformed, format!("variant projection of {layout:?}"))
                    })?;
                    let active = self.active_variant(ptr.at(tag.offset), tag.form)?;
                    if active != variant {
                        if unchecked {
                            return fail(V::TypeConfusion, "invalid proved variant projection");
                        }
                        return Ok(Err(LangFailure));
                    }
                    let fields = &tag
                        .variants
                        .iter()
                        .find(|v| v.ordinal == variant)
                        .ok_or_else(|| err(V::Malformed, "unknown variant"))?
                        .fields;
                    let fl = fields
                        .iter()
                        .find(|fl| fl.ordinal == field)
                        .ok_or_else(|| err(V::Malformed, format!("no variant field {field}")))?;
                    ptr = self.ledger.project(ptr, fl.offset, logical);
                    layout = fl.layout;
                }
                ProjectionStep::Index(v) | ProjectionStep::Slot(v)
                    if matches!(record.kind, LayoutKind::SoaColumns { .. }) =>
                {
                    let LayoutKind::SoaColumns { element } = record.kind else {
                        unreachable!()
                    };
                    let Val::Bits { bits, .. } = self.read_use(f, v)? else {
                        return fail(V::TypeConfusion, "column index is not an integer");
                    };
                    let capacity = self.columns_capacity(ptr, element)?;
                    let bound = if matches!(step, ProjectionStep::Slot(_)) {
                        capacity
                    } else {
                        self.read_scalar(ptr, 8)? as u64
                    };
                    if bits >= u128::from(bound) {
                        if matches!(step, ProjectionStep::Slot(_)) || unchecked {
                            return fail(V::OutOfExtent, "column index exceeds bound");
                        }
                        return Ok(Err(LangFailure));
                    }
                    columns = Some(ColumnRow {
                        base: ptr,
                        capacity,
                        index: bits as u64,
                    });
                    ptr = self.ledger.project(ptr, 0, logical);
                    layout = element;
                }
                ProjectionStep::Slot(v) => {
                    // An element slot below the capacity, initialised or not:
                    // the read or write that follows meets the bytes' own
                    // initialisation state.
                    let LayoutKind::Sequence { element } = record.kind else {
                        return fail(
                            V::TypeConfusion,
                            format!("an element slot of {:?}", record.kind),
                        );
                    };
                    let Val::Bits { scalar, bits } = self.read_use(f, v)? else {
                        return fail(V::TypeConfusion, "slot index is not an integer");
                    };
                    if scalar::integer(scalar).is_none() {
                        return fail(V::TypeConfusion, "slot index is not an integer");
                    }
                    let (start, stride) = self.sequence_shape(element)?;
                    let slots = self.sequence_capacity(ptr, element)?;
                    if bits >= u128::from(slots) {
                        return fail(
                            V::OutOfExtent,
                            format!("element slot {bits} of a sequence holding {slots}"),
                        );
                    }
                    ptr = self
                        .ledger
                        .project(ptr, start + bits as u64 * stride, logical);
                    layout = element;
                }
                ProjectionStep::FlatAt { index, offset } => {
                    let read = |m: &mut Self, v: ValueId| -> Checked<u128> {
                        match m.read_use(f, v)? {
                            Val::Bits { scalar, bits } if scalar::integer(scalar).is_some() => {
                                Ok(bits)
                            }
                            _ => fail(V::TypeConfusion, "a flat walk operand is not an integer"),
                        }
                    };
                    let (index, offset) = (read(self, index)?, read(self, offset)?);
                    // A sequential Packed sequence (MEM-13) counts offsets from its
                    // first element; a flat one from past its prefix and count.
                    let walked = if let LayoutKind::Sequential { element } = record.kind {
                        let content = ptr.at(self.sequential_start(element)?);
                        self.sequential_at(element, ptr, index, offset)?
                            .map(|(at, end)| (content, at, element, end))
                    } else {
                        if self.flat_prefixed(layout)?.map(|(at, _)| at) != Some(16) {
                            return fail(
                                V::TypeConfusion,
                                "a flat walk of a sequence of fixed elements",
                            );
                        }
                        let content = ptr.at(16);
                        self.flat_at(layout, ptr, index, offset)?
                            .map(|(at, element, end)| (content, at, element, end))
                    };
                    match walked {
                        Some((content, at, element, end)) => {
                            flat_end = Some(content.offset + end);
                            flat_content = Some(content);
                            ptr = self.ledger.project(ptr, at.offset - ptr.offset, logical);
                            layout = element;
                        }
                        None if unchecked => {
                            return fail(V::OutOfExtent, "invalid proved flat index projection");
                        }
                        None => return Ok(Err(LangFailure)),
                    }
                }
                ProjectionStep::Index(v)
                    if matches!(record.kind, LayoutKind::Sequential { .. }) =>
                {
                    // Element i of a sequential Packed sequence (MEM-13): walked to past
                    // each earlier element's active extent.
                    let LayoutKind::Sequential { element } = record.kind else {
                        unreachable!("matched above")
                    };
                    let Val::Bits { scalar, bits } = self.read_use(f, v)? else {
                        return fail(V::TypeConfusion, "index is not an integer");
                    };
                    if scalar::integer(scalar).is_none() {
                        return fail(V::TypeConfusion, "index is not an integer");
                    }
                    let count = u64::try_from(self.read_scalar(ptr, 8)?)
                        .map_err(|_| err(V::Malformed, "a sequence count beyond u64"))?;
                    if bits >= u128::from(count) {
                        if unchecked {
                            return fail(V::OutOfExtent, "invalid proved index projection");
                        }
                        return Ok(Err(LangFailure));
                    }
                    let mut at = self.sequential_start(element)?;
                    flat_content = Some(ptr.at(at));
                    for _ in 0..bits as u64 {
                        self.ledger.counters.flat_prefix_reads += 1;
                        at += self.bounded_size(ptr.at(at), element)?;
                    }
                    ptr = self.ledger.project(ptr, at, logical);
                    layout = element;
                }
                ProjectionStep::Index(v) if record.kind == LayoutKind::Flat => {
                    // Element i of a flat sequence lies at its cursor.
                    let Val::Bits { scalar, bits } = self.read_use(f, v)? else {
                        return fail(V::TypeConfusion, "index is not an integer");
                    };
                    if scalar::integer(scalar).is_none() {
                        return fail(V::TypeConfusion, "index is not an integer");
                    }
                    match self.flat_element(layout, ptr, bits)? {
                        Some((at, element)) => {
                            if self.flat_prefixed(layout)?.map(|(at, _)| at) == Some(16) {
                                flat_content = Some(ptr.at(16));
                            }
                            ptr = self.ledger.project(ptr, at.offset - ptr.offset, logical);
                            layout = element;
                        }
                        None if unchecked => {
                            return fail(V::OutOfExtent, "invalid proved flat index projection");
                        }
                        None => return Ok(Err(LangFailure)),
                    }
                }
                ProjectionStep::Index(v) => {
                    // Simd lanes are inline elements too (legalized lane code). A sequence
                    // payload holds its u64 count, then its elements from their alignment.
                    let (element, count, start) = match record.kind {
                        LayoutKind::Inline { element, count } => (element, count, 0),
                        LayoutKind::Simd { element, lanes } => (element, u64::from(lanes), 0),
                        LayoutKind::Slice { element } => {
                            let [owner, start, length] = record.fields.as_slice() else {
                                return fail(V::Malformed, "slice index fields");
                            };
                            let target = self.read_ptr(ptr.at(owner.offset))?;
                            let origin = u64::try_from(self.read_scalar(ptr.at(start.offset), 8)?)
                                .map_err(|_| err(V::Malformed, "slice origin beyond u64"))?;
                            let count = u64::try_from(self.read_scalar(ptr.at(length.offset), 8)?)
                                .map_err(|_| err(V::Malformed, "slice length beyond u64"))?;
                            self.ledger.check(target)?;
                            if self.guarded_target(target) {
                                path.push(target.alloc);
                            }
                            ptr = target;
                            let start = origin
                                .checked_add(8)
                                .ok_or_else(|| err(V::OutOfExtent, "slice origin overflow"))?;
                            (element, count, start)
                        }
                        LayoutKind::Sequence { element } => {
                            let count = u64::try_from(self.read_scalar(ptr, 8)?)
                                .map_err(|_| err(V::Malformed, "a sequence count beyond u64"))?;
                            let start =
                                hirsel_lir::layout::sequence_elements_offset(self.record(element)?)
                                    .map_err(|_| err(V::Malformed, "a sequence element offset"))?;
                            (element, count, start)
                        }
                        _ => {
                            return fail(
                                V::Unsupported,
                                format!("indexing {:?} layouts", record.kind),
                            );
                        }
                    };
                    let Val::Bits { scalar, bits } = self.read_use(f, v)? else {
                        return fail(V::TypeConfusion, "index is not an integer");
                    };
                    if scalar::integer(scalar).is_none() {
                        return fail(V::TypeConfusion, "index is not an integer");
                    }
                    if bits >= u128::from(count) {
                        if unchecked {
                            return fail(V::OutOfExtent, "invalid proved index projection");
                        }
                        return Ok(Err(LangFailure));
                    }
                    ptr = self.ledger.project(
                        ptr,
                        start + bits as u64 * self.size(element)?,
                        logical,
                    );
                    layout = element;
                }
            }
        }
        Ok(Ok(Resolved {
            columns,
            ptr,
            layout,
            path,
            root_slot,
            flat_content,
            flat_end,
        }))
    }

    /// Where a sequential sequence payload's first element starts: past its count
    /// and used-bytes words, aligned to the element (MEM-13).
    /// The sequential block of `r` with room for `need` bytes from its start:
    /// `block` itself, or full storage moved bitwise into a block of twice its
    /// size or more, the place updated and the old block freed shallowly as
    /// the elements hold no reference (MEM-13).
    fn sequential_room(&mut self, r: &Resolved, block: Ptr, keep: u64, need: u64) -> Checked<Ptr> {
        let held = self.ledger.get(block.alloc).extent.len();
        Ok(if need > held {
            let old = self.ledger.get(block.alloc);
            let (payload, align) = (old.layout, old.align);
            let Some(payload) = payload else {
                return fail(V::UseAfterFree, "an append to raw storage");
            };
            let moved = self
                .ledger
                .get_mut(block.alloc)
                .extent
                .take(0, keep, FREE_POISON, None);
            // An unshared tracked arena block grows in its arena, uncharged
            // (MEM-6); its abandoned storage holds nothing.
            let grown = match self.tracked_region(block)? {
                Some(region) => {
                    let grown = self.arena_alloc(region, payload, need.max(2 * held))?;
                    self.ledger.get_mut(grown.alloc).count = Some(Count::Arena { shared: false });
                    grown
                }
                None => {
                    let grown = self.heap_alloc(payload, need.max(2 * held), align)?;
                    self.ledger.get_mut(block.alloc).count = None;
                    self.ledger.free(block.alloc)?;
                    grown
                }
            };
            self.ledger.get_mut(grown.alloc).extent.place(0, moved);
            if let Some(field) = self.open_elements.remove(&block.alloc) {
                self.open_elements.insert(grown.alloc, field);
            }
            let word = self.word;
            self.ledger.get_mut(r.ptr.alloc).extent.put_cell(
                r.ptr.offset,
                word,
                Cell::Data {
                    ptr: grown,
                    counted: true,
                    borrow: None,
                },
            );
            grown
        } else {
            block
        })
    }

    /// Whether the place at `address` holds a sequential block.
    fn holds_sequential(&mut self, f: &Frame, address: AddressId) -> Checked<bool> {
        let r = self.resolve_use(f, address)?;
        Ok(match &self.record(r.layout)?.kind {
            LayoutKind::Block { payload } => {
                matches!(self.record(*payload)?.kind, LayoutKind::Sequential { .. })
            }
            _ => false,
        })
    }

    /// A builder with no open element, which only an open element's own
    /// writes may write (LIR schema revision 24).
    fn closed_builder(&self, block: Ptr) -> Checked<()> {
        if self.open_elements.contains_key(&block.alloc) {
            return fail(V::Malformed, "a write into a builder with an open element");
        }
        Ok(())
    }

    /// The element of the builder at `address` open at `field`: its place,
    /// block and content record, the field, where its length word lies from
    /// the payload's start and the bytes in it so far.
    fn open_element(
        &mut self,
        f: &Frame,
        address: AddressId,
        field: u32,
    ) -> Checked<(Resolved, Ptr, LayoutId, u32, u64, u64)> {
        let (r, block, element) = self.sequence_write(f, address, true)?;
        let Some((8, content)) = self.flat_prefixed(element)? else {
            return fail(V::TypeConfusion, "an open element of an unprefixed builder");
        };
        match self.open_elements.get(&block.alloc) {
            Some(&open) if open == field => {}
            Some(_) => return fail(V::Malformed, "a write of another open field"),
            None => {
                return fail(
                    V::Malformed,
                    "a write of a builder's open element when none is open",
                );
            }
        }
        let used = u64::try_from(self.read_scalar(block.at(8), 8)?)
            .map_err(|_| err(V::Malformed, "a used byte count beyond u64"))?;
        let word = self.sequential_start(element)? + used + 8 + self.open_field(content, field)?;
        let a = self.ledger.check(block)?;
        if !a.extent.all_init(block.offset + word, 8) {
            return fail(V::UninitializedRead, "an open element's length word");
        }
        let len = u64::try_from(self.read_scalar(block.at(word), 8)?)
            .map_err(|_| err(V::Malformed, "an open field length beyond u64"))?;
        Ok((r, block, content, field, word, len))
    }

    fn sequential_start(&self, element: LayoutId) -> Checked<u64> {
        hirsel_lir::layout::sequential_elements_offset(self.record(element)?)
            .map_err(|_| err(V::Malformed, "a sequential element offset"))
    }

    /// Where a sequence payload's elements start and their stride.
    fn sequence_shape(&self, element: LayoutId) -> Checked<(u64, u64)> {
        let start = hirsel_lir::layout::sequence_elements_offset(self.record(element)?)
            .map_err(|_| err(V::Malformed, "a sequence element offset"))?;
        Ok((start, self.size(element)?))
    }

    /// A whole byte sequence payload of `element` holding `bytes` as constructions lay it out
    /// (runtime ABI 5): its u64 count, its u64 capacity (the count), then the bytes from their
    /// alignment; and the start of a slice over all of it.
    fn byte_sequence(&self, element: LayoutId, bytes: &[u8]) -> Checked<(Vec<u8>, u64)> {
        let record = self.record(element)?;
        let (Ok(first), Ok(whole)) = (
            hirsel_lir::layout::sequence_elements_offset(record),
            hirsel_lir::layout::sequence_slice_start(record),
        ) else {
            return fail(V::Malformed, "a byte sequence element offset");
        };
        let count = (bytes.len() as u64).to_le_bytes();
        let capacity = hirsel_lir::layout::SEQUENCE_CAPACITY_OFFSET as usize;
        let mut stored = vec![0u8; first as usize];
        stored[..8].copy_from_slice(&count);
        stored[capacity..capacity + 8].copy_from_slice(&count);
        stored.extend_from_slice(bytes);
        Ok((stored, whole))
    }

    /// How many elements the storage of the sequence payload at `at` holds: every real
    /// slot, which its capacity word never exceeds.
    fn sequence_capacity(&self, at: Ptr, element: LayoutId) -> Checked<u64> {
        let (start, stride) = self.sequence_shape(element)?;
        if stride == 0 {
            return Ok(u64::MAX);
        }
        let a = self.ledger.check(at)?;
        Ok(a.capacity.max(a.size).saturating_sub(at.offset + start) / stride)
    }

    /// `SeqCapacity` of the sequence payload at `at`: its capacity word (runtime ABI 5), one
    /// load, checked against the ledger. A word beyond the storage's real slots would let a
    /// push write past it.
    fn sequence_capacity_word(&self, at: Ptr, element: LayoutId) -> Checked<u64> {
        let (_, stride) = self.sequence_shape(element)?;
        if stride == 0 {
            return Ok(u64::MAX);
        }
        let word = self.read_scalar(at.at(hirsel_lir::layout::SEQUENCE_CAPACITY_OFFSET), 8)?;
        let word =
            u64::try_from(word).map_err(|_| err(V::Malformed, "a sequence capacity beyond u64"))?;
        let slots = self.sequence_capacity(at, element)?;
        if word > slots {
            return fail(
                V::OutOfExtent,
                format!("a sequence capacity word of {word} over {slots} slots"),
            );
        }
        Ok(word)
    }

    /// Make the capacity word of the sequence payload at `at` exact: every real slot, as
    /// `hirsel_rt_v2_seq_grow` leaves it on each path.
    fn exact_capacity(&mut self, at: Ptr, element: LayoutId) -> Checked<()> {
        let slots = self.sequence_capacity(at, element)?;
        self.ledger.get_mut(at.alloc).extent.write_bytes(
            at.offset + hirsel_lir::layout::SEQUENCE_CAPACITY_OFFSET,
            &slots.to_le_bytes(),
        );
        Ok(())
    }

    /// The sequence block a `SeqReserve`, `SeqSetLen` or (`sequential`)
    /// `SeqAppend` writes: the block the place holds, which must be a unique
    /// heap block (a shared or static one is another holder's to observe),
    /// and its element layout.
    fn sequence_write(
        &mut self,
        f: &Frame,
        address: AddressId,
        sequential: bool,
    ) -> Checked<(Resolved, Ptr, LayoutId)> {
        let (r, block, element) = self.sequence_place(f, address, sequential)?;
        self.check_private(block)?;
        Ok((r, block, element))
    }

    /// The place a sequence write replaces or writes through, writable, the
    /// sequence block it holds, and its element layout.
    fn sequence_place(
        &mut self,
        f: &Frame,
        address: AddressId,
        sequential: bool,
    ) -> Checked<(Resolved, Ptr, LayoutId)> {
        let r = self.resolve_use(f, address)?;
        self.check_writable(&r)?;
        let element = match &self.record(r.layout)?.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                match (&self.record(*payload)?.kind, sequential) {
                    (
                        LayoutKind::Sequence { element } | LayoutKind::SoaColumns { element },
                        false,
                    )
                    | (LayoutKind::Sequential { element }, true) => *element,
                    (kind, _) => {
                        return fail(V::TypeConfusion, format!("a sequence write of {kind:?}"));
                    }
                }
            }
            kind => return fail(V::TypeConfusion, format!("a sequence write of {kind:?}")),
        };
        let block = self.read_ptr(r.ptr)?;
        Ok((r, block, element))
    }

    /// A block written in place must be a unique heap block or an unshared
    /// tracked arena block (MEM-6).
    fn check_private(&self, block: Ptr) -> Checked<()> {
        let a = self.ledger.check(block)?;
        if a.count == Some(Count::Arena { shared: false }) && !a.published {
            return Ok(());
        }
        if a.count != Some(Count::Finite(1)) {
            return fail(
                V::WriteShared,
                format!(
                    "sequence write of {:?} whose count is {:?}",
                    block.alloc, a.count
                ),
            );
        }
        if a.published || a.owner != Owner::Heap {
            return fail(
                V::WritePublished,
                format!(
                    "sequence write of {:?}, which is not a private heap block",
                    block.alloc
                ),
            );
        }
        Ok(())
    }

    /// Resolve an address at a use: its checks must already have run, and its
    /// place must still be where its preparation located it.
    fn resolve_use(&mut self, f: &Frame, address: AddressId) -> Checked<Resolved> {
        let located = f.prepared[address.index()];
        if located.is_none() && self.checked_steps(f.unit, address)? {
            return fail(
                V::Malformed,
                format!("{address:?} used before its bounds and tag checks"),
            );
        }
        match self.resolve(f, address)? {
            Ok(r) => match located {
                Some(at) if at != Located::Unresolved && at != r.located() => fail(
                    V::StaleAddress,
                    format!(
                        "{address:?} was located at {at:?} but its place is now {:?}",
                        r.located()
                    ),
                ),
                _ => Ok(r),
            },
            Err(LangFailure) => fail(
                V::Malformed,
                format!("{address:?} failed its checks after preparation"),
            ),
        }
    }

    /// Logical resource occurrences inside a layout, including an empty root.
    /// Moves list every arm and transport only extant occurrences; completeness
    /// uses checked storage to require exactly the active arm's occurrences.
    fn resources_of(&self, layout: LayoutId) -> Checked<Vec<Vec<ProjectionStep<u64>>>> {
        self.resource_walk(layout, None)
    }

    fn resource_walk(
        &self,
        layout: LayoutId,
        storage: Option<Ptr>,
    ) -> Checked<Vec<Vec<ProjectionStep<u64>>>> {
        let mut out = Vec::new();
        let mut pending = vec![(Vec::new(), storage, layout)];
        while let Some((base, at, id)) = pending.pop() {
            let record = self.record(id)?;
            if record.kind == LayoutKind::Resource {
                out.push(base.clone());
            }
            if let Some(tag) = &record.tag {
                let active = at
                    .map(|at| self.active_variant(at.at(tag.offset), tag.form))
                    .transpose()?;
                for variant in &tag.variants {
                    if active.is_some_and(|active| active != variant.ordinal) {
                        continue;
                    }
                    for field in &variant.fields {
                        let mut path = base.clone();
                        path.push(ProjectionStep::VariantField {
                            variant: variant.ordinal,
                            field: field.ordinal,
                        });
                        pending.push((path, at.map(|at| at.at(field.offset)), field.layout));
                    }
                }
                continue;
            }
            match &record.kind {
                LayoutKind::Resource | LayoutKind::Aggregate | LayoutKind::Environment { .. } => {
                    for field in &record.fields {
                        if hirsel_lir::layout::field_is_borrowed(
                            &self.program.layouts,
                            record,
                            field,
                        ) {
                            continue;
                        }
                        let mut path = base.clone();
                        path.push(ProjectionStep::Field(field.ordinal));
                        pending.push((path, at.map(|at| at.at(field.offset)), field.layout));
                    }
                }
                LayoutKind::Inline { element, count } => {
                    let stride = self.size(*element)?;
                    for index in 0..*count {
                        let mut path = base.clone();
                        path.push(ProjectionStep::Index(index));
                        pending.push((path, at.map(|at| at.at(index * stride)), *element));
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    fn address_layout(&self, unit: usize, address: AddressId) -> Checked<LayoutId> {
        let info = self
            .unit(unit)
            .addresses
            .get(address.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown address {address:?}")))?;
        Ok(self.lir_type(unit, info.ty)?.layout)
    }

    /// Writing through `r` must not be observable by another holder.
    fn check_writable(&self, r: &Resolved) -> Checked<()> {
        for alloc in &r.path {
            let a = self.ledger.get(*alloc);
            if !writable(a.count) {
                return fail(
                    V::WriteShared,
                    format!("write through {alloc:?} whose count is {:?}", a.count),
                );
            }
        }
        let target = self.ledger.check(r.ptr)?;
        if target.published || target.owner == Owner::Static {
            return fail(
                V::WritePublished,
                format!("write into published {:?}", r.ptr.alloc),
            );
        }
        Ok(())
    }

    /// Copy semantic operation operands out of checked storage. This transports no
    /// pointers or credits into the model; the executor still owns every allocation.
    fn sequence_operand(&self, p: Ptr, layout: LayoutId) -> Checked<OpValue> {
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) =
            self.record(layout)?.kind
        else {
            return fail(V::Unsupported, "operation pointer is not a block");
        };
        let LayoutKind::Sequence { element } = self.record(payload)?.kind else {
            return fail(V::Unsupported, "operation block is not a sequence");
        };
        let count = self.read_scalar(p, 8)? as u64;
        let record = self.record(element)?;
        let start = hirsel_lir::layout::sequence_elements_offset(record)
            .map_err(|_| err(V::Malformed, "sequence element offset"))?;
        let stride = self.size(element)?;
        let extent = &self.ledger.check(p)?.extent;
        let bytes = count
            .checked_mul(stride)
            .and_then(|n| start.checked_add(n))
            .ok_or_else(|| err(V::OutOfExtent, "operation sequence extent overflow"))?;
        extent.check_range(p.offset, bytes)?;
        if let LayoutKind::Scalar(scalar) = record.kind {
            return Ok(OpValue::Elements {
                scalar,
                bits: (0..count)
                    .map(|i| self.read_scalar(p.at(start + i * stride), stride))
                    .collect::<Checked<_>>()?,
            });
        }
        Ok(OpValue::Sequence(
            (0..count)
                .map(|i| self.operand_at(extent, p.offset + start + i * stride, element))
                .collect::<Checked<_>>()?,
        ))
    }

    fn operand_at(&self, image: &Extent, offset: u64, layout: LayoutId) -> Checked<OpValue> {
        let record = self.record(layout)?;
        let size = self.size(layout)?;
        image.check_range(offset, size)?;
        if !self.initialized(image, offset, layout)?.0 {
            return fail(
                V::UninitializedRead,
                "operation operand contains taken data",
            );
        }
        let fields = |fields: &[hirsel_lir::layout_types::FieldLayout]| -> Checked<Vec<OpValue>> {
            let mut fields: Vec<_> = fields.iter().collect();
            fields.sort_by_key(|f| f.ordinal);
            fields
                .into_iter()
                .map(|f| self.operand_at(image, offset + f.offset, f.layout))
                .collect()
        };
        if let Some(tag) = &record.tag {
            let ordinal = image.active(offset + tag.offset, tag.form, self.word)?;
            let variant = tag
                .variants
                .iter()
                .find(|v| v.ordinal == ordinal)
                .ok_or_else(|| err(V::Malformed, "operation operand tag"))?;
            let mut payload = fields(&variant.fields)?;
            return Ok(OpValue::Variant {
                tag: ordinal,
                value: Box::new(match payload.len() {
                    0 => OpValue::Unit,
                    1 => payload.remove(0),
                    _ => OpValue::Aggregate(payload),
                }),
            });
        }
        Ok(match record.kind {
            LayoutKind::Unit => OpValue::Unit,
            LayoutKind::Scalar(scalar) => {
                if size > 16 {
                    return fail(V::Unsupported, "operation scalar width");
                }
                let mut bits = [0; 16];
                bits[..size as usize].copy_from_slice(image.read_bytes(offset, size));
                OpValue::Bits {
                    scalar,
                    bits: u128::from_le_bytes(bits),
                }
            }
            LayoutKind::Path | LayoutKind::Handle | LayoutKind::HostCapability { .. } => {
                let mut bits = [0; 8];
                bits.copy_from_slice(image.read_bytes(offset, 8));
                OpValue::Handle(u64::from_le_bytes(bits))
            }
            LayoutKind::Block { .. } | LayoutKind::Box { .. } => {
                let Some(Cell::Data { ptr, .. }) = image.cells.get(&offset) else {
                    return fail(V::ForgedPointer, "operation sequence pointer");
                };
                self.sequence_operand(*ptr, layout)?
            }
            LayoutKind::Slice { .. } => OpValue::Bytes(self.slice_bytes(
                &Val::Image(image.copy(offset, size, Some((&[], &[])))),
                layout,
            )?),
            LayoutKind::Aggregate | LayoutKind::Opaque { .. } => {
                OpValue::Aggregate(fields(&record.fields)?)
            }
            LayoutKind::Inline { element, count } => {
                let stride = self.size(element)?;
                OpValue::Sequence(
                    (0..count)
                        .map(|i| self.operand_at(image, offset + i * stride, element))
                        .collect::<Checked<_>>()?,
                )
            }
            _ => {
                return fail(
                    V::Unsupported,
                    format!("operation operand layout {:?}", record.kind),
                );
            }
        })
    }

    // ---- Bytes and cells -------------------------------------------------

    fn read_scalar(&self, at: Ptr, size: u64) -> Checked<u128> {
        let a = self.ledger.check(at)?;
        a.extent.check_range(at.offset, size)?;
        if !a.extent.all_init(at.offset, size) {
            return fail(
                V::UninitializedRead,
                format!(
                    "{size} bytes at {:?}+{} are not initialized",
                    at.alloc, at.offset
                ),
            );
        }
        let mut bytes = [0u8; 16];
        bytes[..size as usize].copy_from_slice(a.extent.read_bytes(at.offset, size));
        Ok(u128::from_le_bytes(bytes))
    }

    /// The active ordinal of the variant whose tag, read as `form`, is at `at`.
    pub(crate) fn active_variant(&self, at: Ptr, form: TagForm) -> Checked<u32> {
        self.ledger
            .check(at)?
            .extent
            .active(at.offset, form, self.word)
    }

    /// Move the `size` bytes of a part out of its place. Every data cell's
    /// word the move takes stays vacated, as the native move leaves its bits:
    /// a null-pointer niche there, the part's own or one nested in a String or
    /// an aggregate, still reads as set.
    fn take_part(&mut self, at: Ptr, size: u64, owned: &[Vec<ProjectionStep<u64>>]) -> Extent {
        let path = self.ledger.path(at);
        let extent = &mut self.ledger.get_mut(at.alloc).extent;
        let held: Vec<u64> = extent
            .cells
            .range(at.offset..at.offset + size)
            .filter(|(_, cell)| matches!(cell, Cell::Data { .. }))
            .map(|(&offset, _)| offset)
            .collect();
        let taken = extent.take(at.offset, size, UNINIT_POISON, Some((&path, owned)));
        extent.vacated.extend(held);
        if taken.init.iter().all(|i| *i) {
            extent.ghosts.insert(at.offset, taken.bytes.to_vec());
        }
        taken
    }

    fn read_ptr(&self, at: Ptr) -> Checked<Ptr> {
        match self.read_cell(at)? {
            Cell::Data { ptr, .. } => Ok(ptr),
            Cell::Code(_) | Cell::Arena(_) => {
                fail(V::TypeConfusion, "code or arena word read as a data handle")
            }
        }
    }

    fn read_cell(&self, at: Ptr) -> Checked<Cell> {
        let a = self.ledger.check(at)?;
        a.extent.check_range(at.offset, self.word)?;
        if !a.extent.all_init(at.offset, self.word) {
            return fail(
                V::UninitializedRead,
                format!("handle at {:?}+{} is not initialized", at.alloc, at.offset),
            );
        }
        a.extent.cells.get(&at.offset).cloned().ok_or_else(|| {
            err(
                V::ForgedPointer,
                format!(
                    "bytes at {:?}+{} are not a checked handle",
                    at.alloc, at.offset
                ),
            )
        })
    }

    /// Initialization follows logical fields, never an ordinary record's
    /// padding or a variant's inactive payload. The second result detects
    /// live field bytes when a sequence count would abandon a slot.
    fn initialized(&self, extent: &Extent, offset: u64, layout: LayoutId) -> Checked<(bool, bool)> {
        use hirsel_lir::layout_types::InitializedBytes;
        let mut work = vec![(offset, layout)];
        let (mut all, mut any) = (true, false);
        while let Some((offset, layout)) = work.pop() {
            let record = self.record(layout)?;
            let size = match record.size {
                SizeSpec::Fixed(n) => n,
                SizeSpec::Dynamic(_) => extent.len().saturating_sub(offset),
            };
            extent.check_range(offset, size)?;
            if record.initialized == InitializedBytes::ZeroPadding {
                all &= extent.all_init(offset, size);
                any |= extent.any_init(offset, size);
                continue;
            }
            if let Some(tag) = &record.tag {
                let width = if tag.form == TagForm::Ordinal {
                    4
                } else {
                    self.word
                };
                let tag_at = offset + tag.offset;
                all &= extent.all_init(tag_at, width);
                any |= extent.any_init(tag_at, width);
                if !extent.all_init(tag_at, width) {
                    continue;
                }
                let active = extent.active(tag_at, tag.form, self.word)?;
                let active = tag
                    .variants
                    .iter()
                    .find(|v| v.ordinal == active)
                    .ok_or_else(|| err(V::Malformed, "unknown variant tag"))?;
                for field in &active.fields {
                    work.push((offset + field.offset, field.layout));
                }
                continue;
            }
            match &record.kind {
                LayoutKind::Aggregate
                | LayoutKind::Resource
                | LayoutKind::Opaque { .. }
                | LayoutKind::Slice { .. } => {
                    for field in &record.fields {
                        work.push((offset + field.offset, field.layout));
                    }
                }
                LayoutKind::Variant => return fail(V::Malformed, "a variant has no tag"),
                LayoutKind::Inline { element, count } => {
                    let stride = self.size(*element)?;
                    for i in 0..*count {
                        work.push((offset + i * stride, *element));
                    }
                }
                LayoutKind::Simd { element, lanes } => {
                    let stride = self.size(*element)?;
                    for i in 0..u64::from(*lanes) {
                        work.push((offset + i * stride, *element));
                    }
                }
                _ => {
                    all &= extent.all_init(offset, size);
                    any |= extent.any_init(offset, size);
                }
            }
        }
        Ok((all, any))
    }

    /// The value an extent holds at offset zero, by representation.
    fn val_from(
        &self,
        e: Extent,
        repr: ValueRepr,
        layout: LayoutId,
    ) -> Checked<(Val, Option<Borrow>, bool)> {
        let size = e.len();
        let uninit = || {
            err(
                V::UninitializedRead,
                format!("read of uninitialized {layout:?}"),
            )
        };
        match repr {
            ValueRepr::PayloadReceipt => {
                if size != self.word || !e.all_init(0, self.word) {
                    return Err(uninit());
                }
                let Some(Cell::Data {
                    ptr,
                    counted,
                    borrow,
                }) = e.cells.get(&0)
                else {
                    return fail(V::ForgedPointer, "receipt bytes are not a private control");
                };
                self.check_receipt_cell(*ptr, layout)?;
                Ok((Val::Ptr(*ptr), *borrow, *counted))
            }
            ValueRepr::Unit => Ok((Val::Unit, None, false)),
            ValueRepr::Scalar(k) => {
                if !e.all_init(0, size) {
                    return Err(uninit());
                }
                let mut bytes = [0u8; 16];
                bytes[..size as usize].copy_from_slice(e.read_bytes(0, size));
                let bits = scalar::canonical(k, u128::from_le_bytes(bytes));
                Ok((Val::Bits { scalar: k, bits }, None, false))
            }
            ValueRepr::Uncounted
                if !matches!(
                    self.record(layout)?.kind,
                    LayoutKind::Block { .. } | LayoutKind::Box { .. }
                ) =>
            {
                if size > 0 && !self.initialized(&e, 0, layout)?.1 {
                    return Err(uninit());
                }
                Ok((Val::Image(e), None, false))
            }
            ValueRepr::Block | ValueRepr::Uncounted => {
                if !e.all_init(0, self.word) {
                    return Err(uninit());
                }
                match e.cells.get(&0) {
                    Some(Cell::Data {
                        ptr,
                        counted,
                        borrow,
                    }) => Ok((Val::Ptr(*ptr), *borrow, *counted)),
                    Some(Cell::Code(_) | Cell::Arena(_)) => {
                        fail(V::TypeConfusion, "code or arena word read as a block")
                    }
                    None => fail(
                        V::ForgedPointer,
                        "bytes read as a block are not a checked handle",
                    ),
                }
            }
            ValueRepr::Aggregate
                if matches!(self.record(layout)?.kind, LayoutKind::Iterator { .. }) =>
            {
                let Some(Cell::Code(code)) = e.cells.get(&self.word).cloned() else {
                    return Err(uninit());
                };
                match e.cells.get(&0) {
                    Some(Cell::Data {
                        ptr,
                        counted,
                        borrow,
                    }) => Ok((
                        Val::Callable {
                            code,
                            env: Some(*ptr),
                        },
                        *borrow,
                        *counted,
                    )),
                    _ => Ok((Val::Callable { code, env: None }, None, false)),
                }
            }
            ValueRepr::Aggregate => {
                if size > 0 && !self.initialized(&e, 0, layout)?.1 {
                    return Err(uninit());
                }
                Ok((Val::Image(e), None, true))
            }
            ValueRepr::Callable => {
                let Some(Cell::Code(code)) = e.cells.get(&0).cloned() else {
                    return Err(uninit());
                };
                match e.cells.get(&self.word) {
                    Some(Cell::Data {
                        ptr,
                        counted,
                        borrow,
                    }) => Ok((
                        Val::Callable {
                            code,
                            env: Some(*ptr),
                        },
                        *borrow,
                        *counted,
                    )),
                    _ => Ok((Val::Callable { code, env: None }, None, false)),
                }
            }
            // A hole is an uncounted checked cell.
            ValueRepr::Hole => {
                if !e.all_init(0, self.word) {
                    return Err(uninit());
                }
                match e.cells.get(&0) {
                    Some(Cell::Data {
                        ptr,
                        counted: false,
                        borrow: None,
                    }) => Ok((Val::Hole(*ptr), None, true)),
                    _ => fail(
                        V::ForgedPointer,
                        "bytes read as a hole are not a checked hole",
                    ),
                }
            }
            // A token is a checked cell; bits alone are a forged arena.
            ValueRepr::Arena => {
                if !e.all_init(0, self.word) {
                    return Err(uninit());
                }
                match e.cells.get(&0) {
                    Some(Cell::Arena(region)) => Ok((Val::Arena(*region), None, false)),
                    _ => fail(
                        V::ForgedPointer,
                        "bytes read as an arena token are not a checked token",
                    ),
                }
            }
            ValueRepr::Handle
                if matches!(self.record(layout)?.kind, LayoutKind::SealedRegion { .. }) =>
            {
                if size != self.word || !e.all_init(0, self.word) {
                    return Err(uninit());
                }
                let Some(Cell::Data {
                    ptr,
                    counted,
                    borrow,
                }) = e.cells.get(&0)
                else {
                    return fail(
                        V::ForgedPointer,
                        "sealed owner bytes are not a checked control pointer",
                    );
                };
                self.ledger.sealed_control(*ptr)?;
                if self.ledger.check(*ptr)?.layout != Some(layout) {
                    return fail(V::TypeConfusion, "sealed control has another owner layout");
                }
                Ok((Val::Ptr(*ptr), *borrow, *counted))
            }
            ValueRepr::Handle => {
                if size != 8 || !e.all_init(0, 8) {
                    return Err(uninit());
                }
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(e.read_bytes(0, 8));
                Ok((Val::Handle(u64::from_le_bytes(bytes)), None, true))
            }
            ValueRepr::Token | ValueRepr::RegionMark | ValueRepr::InitMask { .. } => {
                fail(V::TypeConfusion, format!("{repr:?} is not storable"))
            }
        }
    }

    /// Initialized bytes or a logical resource occurrence make storage live.
    /// An empty resource has no bytes, but still carries its own obligation.
    fn has_content(&self, at: Ptr, size: u64) -> Checked<bool> {
        let extent = &self.ledger.check(at)?.extent;
        let prefix = self.ledger.path(at);
        Ok(extent.any_init(at.offset, size)
            || extent
                .resources
                .keys()
                .any(|path| path.starts_with(&prefix)))
    }

    /// Write a value into uninitialized storage. A credit moves into a counted
    /// cell; persistent storage never receives an uncredited counted reference.
    fn write_value(&mut self, at: Ptr, layout: LayoutId, m: Moved) -> Checked<()> {
        self.write_value_as(at, layout, m, false)
    }

    /// `write_value`; `view` stores a borrowed view into a closure
    /// environment's borrowed-capture field, whose holders the environment's
    /// layout scans as uncounted: it keeps no credit and stays a borrow even in
    /// persistent storage (LIR schema revision 3).
    fn write_value_as(
        &mut self,
        at: Ptr,
        layout: LayoutId,
        mut m: Moved,
        view: bool,
    ) -> Checked<()> {
        let mut size = self.size(layout)?;
        // A dynamic Packed value writes only its active extent: as a sequential
        // element its storage ends there (MEM-13).
        if let Val::Image(image) = &mut m.val
            && hirsel_lir::layout::packed_extents(self.record(layout)?).is_some()
            && image.len() == size
        {
            let tag = image.active(0, TagForm::Ordinal, self.word)?;
            size = self.walked_extent(layout, tag)?;
            *image = image.copy(0, size, None);
        }
        let a = self.ledger.check(at)?;
        a.extent.check_range(at.offset, size)?;
        if self.has_content(at, size)? {
            return fail(
                V::StoreOverInitialized,
                format!(
                    "store over initialized bytes at {:?}+{}",
                    at.alloc, at.offset
                ),
            );
        }
        let persistent = matches!(a.owner, Owner::Heap | Owner::Region(_) | Owner::Static);
        let record = self.record(layout)?;
        let cell_for = |this: &Self,
                        ptr: Ptr,
                        credited: bool,
                        borrow: Option<Borrow>,
                        view: bool|
         -> Checked<Cell> {
            let counted = credited && this.counted_target(ptr);
            if persistent && !view && !counted && this.counted_target(ptr) {
                return fail(
                    V::BorrowedStore,
                    format!(
                        "reference to {:?} stored into {:?} without a holder credit",
                        ptr.alloc, at.alloc
                    ),
                );
            }
            Ok(Cell::Data {
                ptr,
                counted,
                borrow: if persistent && !view { None } else { borrow },
            })
        };
        if matches!(record.kind, LayoutKind::SealedRegion { .. }) {
            let Val::Ptr(control) = &m.val else {
                return fail(
                    V::ForgedPointer,
                    "sealed owner store requires a checked control pointer",
                );
            };
            self.ledger.sealed_control(*control)?;
            if self.ledger.check(*control)?.layout != Some(layout) {
                return fail(
                    V::TypeConfusion,
                    "sealed owner store has another control layout",
                );
            }
        }
        if matches!(record.kind, LayoutKind::PayloadReceipt { .. }) {
            let Val::Ptr(control) = &m.val else {
                return fail(V::ForgedPointer, "receipt store has no private control");
            };
            self.check_receipt_cell(*control, layout)?;
        }
        match m.val {
            Val::Unit => {}
            Val::Bits { scalar, bits } => {
                let ok = match record.kind {
                    LayoutKind::Scalar(k) => k == scalar,
                    LayoutKind::Path => scalar == ScalarKind::U64,
                    _ => false,
                };
                if !ok {
                    return fail(
                        V::TypeConfusion,
                        format!("{scalar:?} stored as {:?}", record.kind),
                    );
                }
                let bytes = bits.to_le_bytes();
                self.ledger
                    .get_mut(at.alloc)
                    .extent
                    .write_bytes(at.offset, &bytes[..size as usize]);
            }
            Val::Ptr(ptr) => {
                if !matches!(
                    record.kind,
                    LayoutKind::Block { .. }
                        | LayoutKind::Box { .. }
                        | LayoutKind::Shared { .. }
                        | LayoutKind::Slice { .. }
                        | LayoutKind::Opaque { .. }
                        | LayoutKind::Iterator { .. }
                        | LayoutKind::SealedRegion { .. }
                        | LayoutKind::PayloadReceipt { .. }
                ) {
                    return fail(
                        V::TypeConfusion,
                        format!("pointer stored as {:?}", record.kind),
                    );
                }
                let cell = cell_for(self, ptr, m.credited, m.borrow, view)?;
                let word = self.word;
                self.ledger
                    .get_mut(at.alloc)
                    .extent
                    .put_cell(at.offset, word, cell);
            }
            Val::Hole(ptr) => {
                if persistent || !matches!(record.kind, LayoutKind::Box { .. }) {
                    return fail(V::TypeConfusion, "a hole stored outside a frame slot");
                }
                let cell = Cell::Data {
                    ptr,
                    counted: false,
                    borrow: None,
                };
                let word = self.word;
                self.ledger
                    .get_mut(at.alloc)
                    .extent
                    .put_cell(at.offset, word, cell);
            }
            Val::Image(mut image) => {
                if image.len() != size {
                    return fail(
                        V::TypeConfusion,
                        format!(
                            "{}-byte image stored as {size}-byte {layout:?}",
                            image.len()
                        ),
                    );
                }
                // An ordinary aggregate can already contain borrowed leaves:
                // storing that aggregate does not turn its views into owners.
                let views: std::collections::HashSet<_> = if persistent && !view {
                    self.scan_cell_offsets(&image, layout, false)?
                        .into_iter()
                        .collect()
                } else {
                    Default::default()
                };
                let cells: Vec<(u64, Cell)> =
                    std::mem::take(&mut image.cells).into_iter().collect();
                for (offset, cell) in cells {
                    let cell = match cell {
                        Cell::Data {
                            ptr,
                            counted,
                            borrow,
                        } => cell_for(
                            self,
                            ptr,
                            counted && m.credited,
                            borrow.or(m.borrow),
                            view || views.contains(&offset),
                        )?,
                        code => code,
                    };
                    image.cells.insert(offset, cell);
                }
                if view {
                    // The creator keeps the resources a view shows.
                    image.resources.clear();
                } else if !m.credited && !image.resources.is_empty() {
                    return fail(
                        V::TypeConfusion,
                        "a borrowed resource stored as an owned value",
                    );
                }
                let path = self.ledger.path(at);
                self.ledger
                    .get_mut(at.alloc)
                    .extent
                    .place_at(at.offset, &path, image);
            }
            Val::Callable { code, env } => {
                if !matches!(
                    record.kind,
                    LayoutKind::Callable { .. } | LayoutKind::Iterator { .. }
                ) {
                    return fail(
                        V::TypeConfusion,
                        format!("callable stored as {:?}", record.kind),
                    );
                }
                let word = self.word;
                let iterator = matches!(record.kind, LayoutKind::Iterator { .. });
                let code_offset = if iterator { word } else { 0 };
                let env_offset = if iterator { 0 } else { word };
                if iterator {
                    self.ledger
                        .get_mut(at.alloc)
                        .extent
                        .write_bytes(at.offset + 2 * word, &0u64.to_le_bytes());
                }
                self.ledger.get_mut(at.alloc).extent.put_cell(
                    at.offset + code_offset,
                    word,
                    Cell::Code(code),
                );
                if let Some(env) = env {
                    let cell = cell_for(self, env, m.credited, m.borrow, view)?;
                    self.ledger.get_mut(at.alloc).extent.put_cell(
                        at.offset + env_offset,
                        word,
                        cell,
                    );
                } else {
                    self.ledger
                        .get_mut(at.alloc)
                        .extent
                        .write_bytes(at.offset + env_offset, &0u64.to_le_bytes()[..word as usize]);
                }
            }
            Val::Handle(h) => {
                self.ledger
                    .get_mut(at.alloc)
                    .extent
                    .write_bytes(at.offset, &h.to_le_bytes());
            }
            Val::Arena(region) => {
                if self.record(layout)?.kind != LayoutKind::Arena {
                    return fail(V::TypeConfusion, "an arena token stored as another layout");
                }
                let word = self.word;
                self.ledger
                    .get_mut(at.alloc)
                    .extent
                    .put_cell(at.offset, word, Cell::Arena(region));
            }
            Val::Token(_) => return fail(V::TokenStored, "a reuse token stored into memory"),
            Val::Mark(_) | Val::Mask(_) | Val::Version => {
                return fail(V::TypeConfusion, "a marker value stored into memory");
            }
        }
        Ok(())
    }

    /// Materialize a value in temporary storage, for a release or a construction.
    fn temporary(&mut self, layout: LayoutId) -> Checked<Ptr> {
        let size = self.size(layout)?;
        let align = self.record(layout)?.align;
        Ok(self
            .ledger
            .alloc(Owner::Temporary, Some(layout), size, size, align, None))
    }

    // ---- Counts --------------------------------------------------------

    fn dup(&mut self, ptr: Ptr) -> Checked<()> {
        self.ledger.check(ptr)?;
        let max = self.config.count_max;
        let a = self.ledger.get_mut(ptr.alloc);
        match a.count {
            Some(Count::Finite(n) | Count::Root(n)) if n >= max => {
                a.count = Some(Count::Immortal);
                self.ledger
                    .exemptions
                    .push(Exemption::Overflow { alloc: ptr.alloc });
                self.ledger.trace.push(Event::Saturate { alloc: ptr.alloc });
            }
            Some(Count::Finite(n)) => {
                a.count = Some(Count::Finite(n + 1));
                self.ledger.trace.push(Event::Dup {
                    alloc: ptr.alloc,
                    count: Some(n + 1),
                });
            }
            Some(Count::Root(n)) => {
                a.count = Some(Count::Root(n + 1));
                self.ledger.trace.push(Event::Dup {
                    alloc: ptr.alloc,
                    count: Some(n + 1),
                });
            }
            Some(Count::Immortal) => self.ledger.trace.push(Event::Dup {
                alloc: ptr.alloc,
                count: None,
            }),
            // A second holder of a tracked arena block shares it for good.
            Some(Count::Arena { .. }) => {
                a.count = Some(Count::Arena { shared: true });
                self.ledger.trace.push(Event::Dup {
                    alloc: ptr.alloc,
                    count: None,
                });
            }
            // A copy read out of a frozen value: one more uncredited holder.
            Some(Count::Frozen { copies }) => {
                a.count = Some(Count::Frozen { copies: copies + 1 });
                self.ledger.trace.push(Event::Dup {
                    alloc: ptr.alloc,
                    count: None,
                });
            }
            None => {
                return fail(
                    V::TypeConfusion,
                    format!("dup of uncounted {:?}", ptr.alloc),
                );
            }
        }
        self.ledger.counters.dups += 1;
        Ok(())
    }

    /// Consume one credit of `ptr`'s block; true when that was its last
    /// holder and the block is now dying (count zero, payload still to drain).
    pub(crate) fn count_down(&mut self, ptr: Ptr) -> Checked<bool> {
        self.ledger.check(ptr)?;
        self.ledger.counters.releases += 1;
        let a = self.ledger.get_mut(ptr.alloc);
        match a.count {
            Some(Count::Finite(0) | Count::Root(0)) | None => fail(
                V::TypeConfusion,
                format!("release of {:?}, which has no count", ptr.alloc),
            ),
            Some(Count::Frozen { copies: 0 }) => fail(
                V::WriteShared,
                format!("release of {:?}, a block of a frozen graph", ptr.alloc),
            ),
            Some(Count::Frozen { copies }) => {
                a.count = Some(Count::Frozen { copies: copies - 1 });
                self.ledger.trace.push(Event::Release {
                    alloc: ptr.alloc,
                    count: None,
                });
                Ok(false)
            }
            Some(Count::Finite(n)) => {
                a.count = Some(Count::Finite(n - 1));
                if n == 1 {
                    a.state = AllocState::Dying;
                }
                self.ledger.trace.push(Event::Release {
                    alloc: ptr.alloc,
                    count: Some(n - 1),
                });
                Ok(n == 1)
            }
            // The last root's drop program thaws the graph first (`DropStep::Thaw`).
            Some(Count::Root(n)) => {
                a.count = Some(Count::Root(n - 1));
                if n == 1 {
                    a.state = AllocState::Dying;
                }
                self.ledger.trace.push(Event::Release {
                    alloc: ptr.alloc,
                    count: Some(n - 1),
                });
                Ok(n == 1)
            }
            Some(Count::Immortal | Count::Arena { .. }) => {
                self.ledger.trace.push(Event::Release {
                    alloc: ptr.alloc,
                    count: None,
                });
                Ok(false)
            }
        }
    }

    /// Consume one credit of `ptr`'s block; at zero its payload drains on `work`.
    fn decrement(
        &mut self,
        ptr: Ptr,
        mask: Option<Vec<bool>>,
        work: &mut Vec<Work>,
    ) -> Checked<()> {
        if self.count_down(ptr)? {
            if self.open_holes.get(ptr.alloc.0 as usize).is_some() {
                self.open_holes.remove(ptr.alloc.0 as usize);
                work.push(Work::Death {
                    alloc: ptr.alloc,
                    raw: None,
                });
                return Ok(());
            }
            let layout = self
                .ledger
                .get(ptr.alloc)
                .layout
                .expect("live block has a layout");
            work.push(Work::Death {
                alloc: ptr.alloc,
                raw: None,
            });
            work.push(Work::DropAt {
                at: Ptr { offset: 0, ..ptr },
                layout,
                mask,
            });
        }
        Ok(())
    }

    /// Ideal holders, recounted from every holder position, against each
    /// block's actual count.
    fn recount_holders(&self, validate_cells: bool) -> Checked<BTreeMap<AllocId, u64>> {
        fork::scanned(self.ledger.allocations.len());
        let mut holders = BTreeMap::new();
        let mut add = |ptr: Ptr, n: u64| *holders.entry(ptr.alloc).or_default() += n;
        for (index, a) in self.ledger.allocations.indexed() {
            if a.state == AllocState::Dead {
                continue;
            }
            for (offset, cell) in &a.extent.cells {
                if let Cell::Data {
                    ptr, counted: true, ..
                } = cell
                {
                    let target = self.ledger.get(ptr.alloc);
                    if validate_cells
                        && (matches!(target.state, AllocState::Dead | AllocState::Raw(_))
                            || target.generation != ptr.generation)
                    {
                        return fail(
                            V::UseAfterFree,
                            format!(
                                "AllocId({index})+{offset} holds a credit of freed {:?}",
                                ptr.alloc
                            ),
                        );
                    }
                    add(*ptr, 1);
                }
            }
        }
        let mut held = |h: &Held| match &h.val {
            Val::Ptr(p) | Val::Callable { env: Some(p), .. } => add(*p, u64::from(h.credits)),
            Val::Image(e) => {
                for cell in e.cells.values() {
                    if let Cell::Data {
                        ptr, counted: true, ..
                    } = cell
                    {
                        add(*ptr, u64::from(h.credits));
                    }
                }
            }
            _ => {}
        };
        for activity in self.all_activities() {
            if let Activity::Frame(f) = activity {
                f.values.iter().flatten().for_each(&mut held);
            }
        }
        self.roots
            .values()
            .chain(&self.forgotten)
            .for_each(&mut held);
        if let Some(Finished::Returned(Some((value, _, _)))) = &self.finished {
            held(value);
        }
        Ok(holders)
    }

    fn recount_counts(&self) -> Checked<()> {
        fork::scanned(self.ledger.allocations.len());
        let holders = self.recount_holders(true)?;
        for (index, a) in self.ledger.allocations.indexed() {
            if a.owner != Owner::Heap || matches!(a.state, AllocState::Dead | AllocState::Raw(_)) {
                continue;
            }
            let ideal = holders.get(&AllocId(index as u32)).copied().unwrap_or(0);
            if let Some(n) = a.count.and_then(Count::holders)
                && n != ideal
            {
                return fail(
                    V::CountMismatch,
                    format!("AllocId({index}) has count {n} but {ideal} holders"),
                );
            }
        }
        Ok(())
    }

    // ---- Failures --------------------------------------------------------

    /// Inert source identities; a library forwards the actual source invocation.
    fn caller_sites(
        &self,
        f: &Frame,
        range: SiteRange,
        forward: bool,
    ) -> (Option<CallerSite>, Option<CallerSite>) {
        let header = &self.unit(f.unit).header;
        let operation = if header.origin == UnitOrigin::Library {
            f.operation
        } else {
            Some(CallerSite {
                unit: f.unit,
                range,
            })
        };
        let caller = if forward && (header.track_caller || header.origin == UnitOrigin::Library) {
            f.caller
        } else {
            operation
        };
        (caller, operation)
    }

    fn active_call(&self, f: &Frame) -> Option<&'p CallSite> {
        let u = self.unit(f.unit);
        let block = u.blocks.get(f.block.index())?;
        if f.statement < block.instructions.len {
            return None;
        }
        let Terminator::Call { call, .. } = u.terminators.get(f.block.index())? else {
            return None;
        };
        u.tables.calls.get(call.index())
    }

    fn source_range(&self, f: &Frame, anchor: SourceAnchorId) -> SiteRange {
        self.unit(f.unit)
            .tables
            .source_anchors
            .get(anchor.index())
            .map_or(SiteRange::UNMAPPED, |a| a.site)
    }

    fn cleanup_sites(
        &self,
        f: &Frame,
        failure: Option<FailureId>,
    ) -> (Option<CallerSite>, Option<CallerSite>) {
        let u = self.unit(f.unit);
        let inst = u
            .blocks
            .get(f.block.index())
            .and_then(|b| b.instructions.get(&u.instructions))
            .and_then(|instructions| instructions.get(f.statement as usize));
        if let Some(
            Inst::Drop { anchor, .. } | Inst::Release { anchor, .. } | Inst::Reset { anchor, .. },
        ) = inst
        {
            let range = u
                .tables
                .cleanup_anchors
                .get(anchor.index())
                .map_or(SiteRange::UNMAPPED, |a| a.site);
            return self.caller_sites(f, range, true);
        }
        if let Some(call) = self.active_call(f) {
            return self.caller_sites(f, self.source_range(f, call.anchor), call.forward_caller);
        }
        let range = failure
            .and_then(|id| u.tables.failures.get(id.index()))
            .map_or(SiteRange::UNMAPPED, |edge| {
                self.source_range(f, edge.anchor)
            });
        self.caller_sites(f, range, true)
    }

    fn observe_site(&self, site: CallerSite) -> (hirsel_syntax::DefPath, SiteRange) {
        (self.unit(site.unit).header.definition.clone(), site.range)
    }

    fn record_sites(
        &mut self,
        rule: &str,
        caller: Option<CallerSite>,
        operation: Option<CallerSite>,
        fallback: usize,
        unanchored: bool,
    ) {
        self.full_recount_due = true;
        let primary = caller.or(operation);
        let (definition, site) = primary
            .map(|site| self.observe_site(site))
            .unwrap_or_else(|| {
                (
                    self.unit(fallback).header.definition.clone(),
                    SiteRange::UNMAPPED,
                )
            });
        let operation = operation.map(|site| self.observe_site(site)).filter(
            |(operation_definition, operation_range)| {
                operation_definition != &definition || operation_range != &site
            },
        );
        if primary.is_none() && unanchored {
            self.unanchored_failures.insert(self.failures.len());
        }
        self.failures.push(ObservedFailure {
            operation_source: operation.as_ref().map_or(u32::MAX, |(_, s)| s.source),
            operation: operation.map(|(d, s)| (d, (s.start, s.end))),
            source: site.source,
            rule: rule.to_string(),
            definition,
            site: (site.start, site.end),
        });
        self.ledger.trace.push(Event::Failure {
            rule: rule.to_string(),
        });
    }

    fn record_failure(&mut self, f: &Frame, rule: &str, anchor: SourceAnchorId) {
        let forward = self.active_call(f).is_none_or(|call| call.forward_caller);
        let (caller, operation) = self.caller_sites(f, self.source_range(f, anchor), forward);
        self.record_sites(rule, caller, operation, f.unit, true);
    }

    /// A cleanup retains its pair independently of any user finalizer it calls.
    fn record_close_failure(&mut self, rule: &str, drain: &Drain) {
        self.record_sites(rule, drain.caller, drain.operation, self.entry, false);
    }

    /// Take a failure edge: record its failure, dispose its tokens, enter cleanup.
    fn take_failure(&mut self, f: &mut Frame, failure: FailureId, injected: bool) -> Checked<()> {
        let u = self.unit(f.unit);
        let edge = u
            .tables
            .failures
            .get(failure.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown failure {failure:?}")))?;
        if !injected && edge.rule.as_deref() == Some("MEM-9") {
            return fail(
                V::UndefinedBehavior,
                "int.unchecked_add: mathematical sum must fit the destination integer type",
            );
        }
        if injected {
            self.record_failure(f, INJECTED_RULE, edge.anchor);
        } else if let Some(rule) = &edge.rule {
            self.record_failure(f, rule, edge.anchor);
        } else if self.failures.is_empty() {
            return fail(
                V::Malformed,
                "a propagating failure edge with no failure active",
            );
        }
        self.unwind(f, failure)
    }

    /// MEM-6: a bounded arena's charge exceeded its budget. Record its
    /// `OutOfSpace` at the edge's site and take the edge.
    fn exhaust(&mut self, f: &mut Frame, failure: FailureId, region: usize) -> Checked<Control> {
        let u = self.unit(f.unit);
        let anchor = u
            .tables
            .failures
            .get(failure.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown failure {failure:?}")))?
            .anchor;
        let index = self.failures.len();
        self.record_failure(f, "MEM-6", anchor);
        if let Some(budget) = &mut self.ledger.regions[region].budget {
            budget.exhausted = Some(index);
        }
        self.unwind(f, failure)?;
        Ok(Control::Continue)
    }

    /// Dispose a failure edge's tokens and enter its cleanup.
    fn unwind(&mut self, f: &mut Frame, failure: FailureId) -> Checked<()> {
        let u = self.unit(f.unit);
        let edge = u
            .tables
            .failures
            .get(failure.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown failure {failure:?}")))?;
        for v in self.pool(edge.dispose, &u.pools.values)? {
            let m = self.take_value(f, *v)?;
            let Val::Token(token) = m.val else {
                return fail(
                    V::TypeConfusion,
                    format!("{v:?} in a disposal list is not a token"),
                );
            };
            self.dispose(token)?;
        }
        f.failing = true;
        self.jump(f, edge.cleanup)
    }

    fn dispose(&mut self, token: Option<TokenId>) -> Checked<()> {
        let Some(t) = token else {
            return Ok(());
        };
        let tk = &mut self.ledger.tokens[t.0 as usize];
        if tk.consumed {
            return fail(
                V::TokenReused,
                format!("{t:?} disposed after it was consumed"),
            );
        }
        tk.consumed = true;
        let (alloc, generation) = (tk.alloc, tk.generation);
        let a = self.ledger.get(alloc);
        if a.state != AllocState::Raw(t) || a.generation != generation {
            return fail(
                V::StaleReuseGeneration,
                format!("{t:?} names {alloc:?} generation {generation}, which it no longer owns"),
            );
        }
        self.ledger.free(alloc)?;
        self.ledger.counters.disposes += 1;
        self.ledger.trace.push(Event::Dispose { alloc });
        Ok(())
    }

    // ---- Control ---------------------------------------------------------

    fn jump(&mut self, f: &mut Frame, edge: EdgeId) -> Checked<()> {
        let u = self.unit(f.unit);
        let e = u
            .tables
            .edges
            .get(edge.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown edge {edge:?}")))?;
        let block = u
            .blocks
            .get(e.target.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown block {:?}", e.target)))?;
        let arguments = self.pool(e.arguments, &u.pools.values)?;
        let parameters = self.pool(block.parameters, &u.pools.values)?;
        if arguments.len() != parameters.len() {
            return fail(
                V::Malformed,
                format!("edge to {:?} with wrong arity", e.target),
            );
        }
        // Reconstruct field flags from H's byte/resource ledger, independently
        // of C's abstract ownership state. Inactive variants have no payload.
        for flag in &u.tables.init_flags {
            if flag.environment.is_some() || flag.path.len == 0 {
                continue;
            }
            let Some(ordinal) = parameters.iter().position(|p| *p == flag.value) else {
                continue;
            };
            let Some(expected) = self.flag_part(f, flag)? else {
                continue;
            };
            if self.flag_bits(f, arguments[ordinal])? != u128::from(expected) {
                return fail(
                    V::Malformed,
                    "field drop flag disagrees with initialized storage",
                );
            }
        }
        // A version argument renames the slot content it names (a version
        // join); every other argument moves into its parameter.
        let mut moved = Vec::with_capacity(arguments.len());
        for a in arguments {
            if f.values.get(a.index()).is_some_and(Option::is_none) {
                let slot = self.version_slots[f.unit]
                    .get(a.index())
                    .copied()
                    .flatten()
                    .ok_or_else(|| err(V::Malformed, "undefined non-version edge argument"))?;
                if let SlotStore::Own(ptr) | SlotStore::Alias(ptr) = f.slots[slot.index()] {
                    let layout = self.lir_type(f.unit, u.slots[slot.index()].ty)?.layout;
                    if self
                        .ledger
                        .check(ptr)?
                        .extent
                        .any_init(ptr.offset, self.size(layout)?)
                    {
                        return fail(V::Malformed, "undefined version of initialized storage");
                    }
                }
                moved.push(None);
            } else {
                moved.push(match self.get(f, *a)?.val {
                    Val::Version => None,
                    _ if self.get(f, *a)?.credits == 0
                        && self.value_info(f.unit, *a)?.holding != Holding::Owned =>
                    {
                        // Non-owning edge transport is not a read. Keep stale
                        // provenance for the next actual use to check, as C does.
                        let held = self.get(f, *a)?;
                        Some(Moved {
                            val: held.val.clone(),
                            credited: false,
                            borrow: held.borrow,
                        })
                    }
                    _ => Some(self.take_value(f, *a)?),
                });
            }
        }
        let renamed: Vec<(usize, ValueId)> = arguments
            .iter()
            .zip(parameters)
            .zip(&moved)
            .filter(|(_, m)| m.is_none())
            .flat_map(|((a, p), _)| {
                f.versions
                    .iter()
                    .enumerate()
                    .filter(move |(_, v)| **v == Some(*a))
                    .map(move |(s, _)| (s, *p))
            })
            .collect();
        for (p, m) in parameters.iter().zip(moved) {
            match m {
                Some(m) => self.bind(f, *p, m)?,
                None => self.define_version(f, *p)?,
            }
        }
        for (s, p) in renamed {
            f.versions[s] = Some(p);
        }
        f.block = e.target;
        f.statement = 0;
        f.jumped = true;
        Ok(())
    }

    fn flag_bits(&self, f: &Frame, value: ValueId) -> Checked<u128> {
        match self.get(f, value)?.val {
            Val::Bits { bits, .. } => Ok(bits),
            _ => fail(V::Malformed, "drop flag operand is not scalar"),
        }
    }

    fn flag_part(&self, f: &Frame, flag: &hirsel_lir::ir::InitFlag) -> Checked<Option<bool>> {
        let unit = self.unit(f.unit);
        let mut layout = self
            .lir_type(f.unit, unit.slots[flag.slot.index()].ty)?
            .layout;
        let mut ptr = match f.slots[flag.slot.index()] {
            SlotStore::Dead => return Ok(Some(false)),
            SlotStore::Own(ptr) | SlotStore::Alias(ptr) => ptr,
        };
        for step in self.pool(flag.path, &unit.pools.projections)? {
            let record = self.record(layout)?;
            let field = match *step {
                ProjectionStep::Field(field) => record.fields.iter().find(|f| f.ordinal == field),
                ProjectionStep::VariantField { variant, field } => {
                    let tag = record
                        .tag
                        .as_ref()
                        .ok_or_else(|| err(V::Malformed, "flag variant has no tag"))?;
                    let extent = &self.ledger.check(ptr)?.extent;
                    // A whole take leaves only ghosts; there is no live tag.
                    if !extent.any_init(ptr.offset, self.size(layout)?) {
                        return Ok(Some(false));
                    }
                    // An inactive variant's field owns nothing, but C counts a
                    // variant no test proved as present, as a failure edge
                    // before the test sees it.
                    if self.active_variant(ptr.at(tag.offset), tag.form)? != variant {
                        return Ok(None);
                    }
                    tag.variants
                        .iter()
                        .find(|v| v.ordinal == variant)
                        .and_then(|v| v.fields.iter().find(|f| f.ordinal == field))
                }
                ProjectionStep::Index(index) => {
                    let LayoutKind::Inline { element, count } = record.kind else {
                        return fail(V::Malformed, "flag index is not inline");
                    };
                    let constant = unit
                        .instructions
                        .iter()
                        .find_map(|i| match *i {
                            Inst::Const { result, constant } if result == index => Some(constant),
                            _ => None,
                        })
                        .ok_or_else(|| err(V::Malformed, "flag index is not constant"))?;
                    let ConstantValue::Integer(index) =
                        unit.tables.constants[constant.index()].value
                    else {
                        return fail(V::Malformed, "flag index is not integer");
                    };
                    let index = u64::try_from(index)
                        .map_err(|_| err(V::Malformed, "flag index exceeds u64"))?;
                    if index >= count {
                        return fail(V::Malformed, "flag index is out of bounds");
                    }
                    ptr = self.ledger.project(
                        ptr,
                        index * self.size(element)?,
                        ProjectionStep::Index(index),
                    );
                    layout = element;
                    continue;
                }
                _ => return fail(V::Malformed, "flag path crosses dynamic storage"),
            }
            .ok_or_else(|| err(V::Malformed, "flag names no field"))?;
            let logical = (*step).try_map_index(|_: ValueId| -> Checked<u64> {
                fail(V::Malformed, "dynamic flag path")
            })?;
            ptr = self.ledger.project(ptr, field.offset, logical);
            layout = field.layout;
        }
        let extent = &self.ledger.check(ptr)?.extent;
        let path = self.ledger.path(ptr);
        let record = self.record(layout)?;
        if record.drop.is_empty() && record.finalizer.is_none() {
            // A plain part keeps initialized-byte semantics.
            return Ok(Some(
                self.initialized(extent, ptr.offset, layout)?.1
                    || extent.resources.contains_key(&path),
            ));
        }
        // Any other part holds content while an owner remains in its active
        // storage. An initialized part with none left, such as an inner
        // variant whose active case owns nothing, may carry either value: C
        // cannot always know that case, and its drop releases nothing.
        let initialized = extent.any_init(ptr.offset, self.size(layout)?);
        Ok(if self.owners(extent, ptr.offset, layout, path)? {
            Some(true)
        } else if initialized {
            None
        } else {
            Some(false)
        })
    }

    /// Whether an owner remains in the active storage of a value of `layout`
    /// at `offset` and `path`: a data handle not yet taken, or a resource not
    /// yet finalized. Inactive variants, plain parts and the fields a pending
    /// finalizer already completed own nothing.
    fn owners(
        &self,
        extent: &Extent,
        offset: u64,
        layout: LayoutId,
        path: Vec<ProjectionStep<u64>>,
    ) -> Checked<bool> {
        let mut work = vec![(offset, layout, path)];
        while let Some((offset, layout, path)) = work.pop() {
            let record = self.record(layout)?;
            if record.drop.is_empty() && record.finalizer.is_none() {
                continue;
            }
            if let Some(r) = extent.resources.get(&path) {
                match self.ledger.resources[r.0 as usize].state {
                    ResourceState::Live | ResourceState::Finalizing => return Ok(true),
                    ResourceState::FieldsPending => {}
                    ResourceState::Dead => continue,
                }
            }
            let size = self.size(layout)?;
            if let Some(tag) = &record.tag {
                let width = if tag.form == TagForm::Ordinal {
                    4
                } else {
                    self.word
                };
                let tag_at = offset + tag.offset;
                if !extent.all_init(tag_at, width) {
                    continue;
                }
                let active = extent.active(tag_at, tag.form, self.word)?;
                let active = tag
                    .variants
                    .iter()
                    .find(|v| v.ordinal == active)
                    .ok_or_else(|| err(V::Malformed, "unknown variant tag"))?;
                for field in &active.fields {
                    let mut at = path.clone();
                    at.push(ProjectionStep::VariantField {
                        variant: active.ordinal,
                        field: field.ordinal,
                    });
                    work.push((offset + field.offset, field.layout, at));
                }
                continue;
            }
            match &record.kind {
                // A run-context handle, such as a file's, owns its word until
                // a move takes it.
                LayoutKind::Handle | LayoutKind::HostCapability { .. } => {
                    if extent.any_init(offset, size) {
                        return Ok(true);
                    }
                }
                LayoutKind::Aggregate | LayoutKind::Resource | LayoutKind::Opaque { .. } => {
                    for field in &record.fields {
                        let mut at = path.clone();
                        at.push(ProjectionStep::Field(field.ordinal));
                        work.push((offset + field.offset, field.layout, at));
                    }
                }
                LayoutKind::Inline { element, count } => {
                    let stride = self.size(*element)?;
                    for i in 0..*count {
                        let mut at = path.clone();
                        at.push(ProjectionStep::Index(i));
                        work.push((offset + i * stride, *element, at));
                    }
                }
                _ => {
                    if extent
                        .cells
                        .range(offset..offset + size)
                        .any(|(_, c)| matches!(c, Cell::Data { .. }))
                    {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn mask(&self, f: &Frame, live: MaskRef) -> Checked<Option<Vec<bool>>> {
        Ok(match live {
            MaskRef::All => None,
            MaskRef::Static(m) => {
                let u = self.unit(f.unit);
                let mask = u
                    .tables
                    .masks
                    .get(m.index())
                    .ok_or_else(|| err(V::Malformed, format!("unknown mask {m:?}")))?;
                let words = self.pool(mask.words, &u.pools.mask_words)?;
                Some(
                    (0..mask.fields as usize)
                        .map(|i| words.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1))
                        .collect(),
                )
            }
            MaskRef::Dynamic(v) => match self.get(f, v)?.val.clone() {
                Val::Mask(bits) => Some(bits),
                _ => return fail(V::TypeConfusion, format!("{v:?} is not a mask")),
            },
        })
    }

    // ---- Execution -------------------------------------------------------

    pub fn start(&mut self, entry: usize, args: &[Arg]) -> Checked<()> {
        self.check_seal_sources()?;
        self.seal_sources_checked = true;
        if self.config.count_max == 0 || self.config.count_max >= u32::MAX as u64 {
            return fail(
                V::Malformed,
                "finite count bound does not fit the ABI v2 u32 counter",
            );
        }
        if let Some(binding) = &self.unit(entry).tables.receipt_entry {
            let unit = self.unit(entry);
            let abi = &self.program.layouts.fn_abis[unit.header.abi.0 as usize];
            binding
                .check(abi, unit, &self.program.layouts)
                .map_err(|e| err(V::Malformed, e))?;
            if abi
                .receipts
                .as_ref()
                .is_none_or(|contract| !contract.is_empty())
            {
                return fail(
                    V::Malformed,
                    "harness entry has no incoming receipt custody",
                );
            }
            let source = self
                .config
                .receipt_sources
                .iter()
                .find(|source| {
                    source.original.definition == unit.header.definition
                        && source.instance == unit.header.instance
                })
                .ok_or_else(|| {
                    err(
                        V::Malformed,
                        "receipt entry lacks independent source declaration",
                    )
                })?;
            source.check_structure().map_err(|e| err(V::Malformed, e))?;
            if abi.receipts.as_ref() != Some(&source.contract)
                || source.implementation_revision != unit.header.implementation_revision
            {
                return fail(
                    V::Malformed,
                    "receipt entry differs from independent source declaration",
                );
            }
        }
        self.entry = entry;
        let mut f = self.frame(entry, Role::Entry, None);
        let parameters = &self.unit(entry).parameters;
        if parameters.len() != args.len() {
            return fail(
                V::Malformed,
                "entry arity differs from the harness arguments",
            );
        }
        for (p, a) in parameters.iter().zip(args) {
            let val = match *a {
                Arg::Unit => Val::Unit,
                Arg::Bits { scalar, bits } => Val::Bits { scalar, bits },
                Arg::Handle(h) => {
                    if !self.config.context_handles.iter().any(|(c, _)| *c == h) {
                        return fail(V::Malformed, "an entry handle the context does not own");
                    }
                    Val::Handle(h)
                }
            };
            self.define(
                &mut f,
                *p,
                Held {
                    val,
                    credits: 0,
                    borrow: None,
                },
            )?;
        }
        self.bind_parameter_slots(&mut f, &[])?;
        self.stack.push(Activity::Frame(Box::new(f)));
        Ok(())
    }

    pub fn run(&mut self) -> Checked<()> {
        while self.step()? {}
        Ok(())
    }

    fn step(&mut self) -> Checked<bool> {
        let Some(activity) = self.stack.pop() else {
            // A child's stack empties once its frame returned and its closure was released.
            if self.in_child() {
                self.task_complete()?;
                return Ok(true);
            }
            return Ok(false);
        };
        self.steps += 1;
        if self.steps > self.config.step_limit {
            return fail(V::Unsupported, "step limit reached (a tooling limit)");
        }
        match activity {
            Activity::Frame(mut f) => {
                let control = match self.step_frame(&mut f) {
                    Ok(c) => c,
                    Err(mut v) => {
                        v.site.get_or_insert_with(|| self.site(&f));
                        return Err(v);
                    }
                };
                if self.ended {
                    return Ok(false);
                }
                match control {
                    Control::Continue => self.stack.push(Activity::Frame(f)),
                    Control::Push(next) => {
                        self.stack.push(Activity::Frame(f));
                        self.stack.push(next);
                    }
                    Control::Exit(exit) => {
                        let site = self.site(&f);
                        self.exit_frame(*f, exit).map_err(|mut v| {
                            v.site.get_or_insert(site);
                            v
                        })?;
                    }
                }
            }
            Activity::Drain(mut d) => match d.work.pop() {
                None => self.drain_done(d)?,
                Some(work) => {
                    let pushed = self.step_drain(&mut d, work)?;
                    self.stack.push(Activity::Drain(d));
                    if let Some(next) = pushed {
                        self.stack.push(next);
                    }
                }
            },
        }
        self.finish_switch()?;
        if self.config.check_every_step {
            self.check_counts()?;
        }
        Ok(true)
    }

    fn step_frame(&mut self, f: &mut Frame) -> Checked<Control> {
        let u = self.unit(f.unit);
        let block = u
            .blocks
            .get(f.block.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown block {:?}", f.block)))?;
        let insts = self.pool(block.instructions, &u.instructions)?;
        if let Some(inst) = insts.get(f.statement as usize) {
            if !S::KIND.admits(inst.opcode()) {
                return fail(
                    V::Malformed,
                    format!("{:?} is not in the {:?} vocabulary", inst.opcode(), S::KIND),
                );
            }
            f.jumped = false;
            let control = self.exec(f, inst)?;
            if matches!(control, Control::Continue)
                && !f.jumped
                && matches!(f.resume, Resume::Running)
            {
                f.statement += 1;
            }
            return Ok(control);
        }
        let term = u
            .terminators
            .get(f.block.index())
            .ok_or_else(|| err(V::Malformed, "block without terminator"))?;
        if !S::KIND.admits(term.opcode()) {
            return fail(V::Malformed, format!("{:?} not admitted", term.opcode()));
        }
        self.terminate(f, term)
    }

    /// Jump to a failure edge without advancing past the instruction.
    fn fail_at(&mut self, f: &mut Frame, failure: FailureId, injected: bool) -> Checked<Control> {
        self.take_failure(f, failure, injected)?;
        Ok(Control::Continue)
    }

    fn exec(&mut self, f: &mut Frame, inst: &Inst) -> Checked<Control> {
        if f.pending_custody.as_ref().is_some_and(|p| !p.admitted)
            && !matches!(
                inst,
                Inst::Address { .. }
                    | Inst::BeginLoan { mutable: false, .. }
                    | Inst::Read {
                        mode: ReadMode::Borrow,
                        ..
                    }
                    | Inst::Hash { .. }
                    | Inst::EndLoan { .. }
            )
        {
            return fail(
                V::FrameImbalance,
                "adapter transfers, charges or mutates before key admission",
            );
        }
        let unit = f.unit;
        let u = self.unit(unit);
        match *inst {
            Inst::SealArena {
                result,
                grant,
                root,
                shape,
                origin,
                failure,
            } => {
                if let Some(region) = self.seal_root(f, result, grant, root, (shape, origin))? {
                    return self.exhaust(f, failure, region);
                }
            }
            Inst::ArenaInventory {
                arena,
                allocation,
                value,
                shape,
                previous,
            } => {
                self.record_seal_inventory(f, arena, allocation, value, shape, previous)?;
            }
            Inst::PayloadReceipt {
                result,
                value,
                shape,
                binding,
                inputs,
            } => {
                self.payload_receipt(f, result, value, shape, binding, inputs)?;
            }
            Inst::BorrowSealedRoot {
                result,
                owner,
                loan,
                shape,
            } => {
                self.read_sealed_root(f, result, owner, loan, shape)?;
            }
            Inst::Const { result, constant } => {
                let data = u
                    .tables
                    .constants
                    .get(constant.index())
                    .ok_or_else(|| err(V::Malformed, "unknown constant"))?;
                let ty = self.lir_type(unit, data.ty)?;
                let val = match (&data.value, ty.repr) {
                    (ConstantValue::Heap, ValueRepr::Arena) => Val::Arena(None),
                    (ConstantValue::Unit, _) => Val::Unit,
                    (ConstantValue::Bool(b), ValueRepr::Scalar(ScalarKind::Bool)) => Val::Bits {
                        scalar: ScalarKind::Bool,
                        bits: u128::from(*b),
                    },
                    (ConstantValue::Integer(n), ValueRepr::Scalar(k))
                        if scalar::integer(k).is_some() =>
                    {
                        Val::Bits {
                            scalar: k,
                            bits: scalar::canonical(k, *n),
                        }
                    }
                    (ConstantValue::F32(b), ValueRepr::Scalar(ScalarKind::F32)) => Val::Bits {
                        scalar: ScalarKind::F32,
                        bits: u128::from(*b),
                    },
                    (ConstantValue::F64(b), ValueRepr::Scalar(ScalarKind::F64)) => Val::Bits {
                        scalar: ScalarKind::F64,
                        bits: u128::from(*b),
                    },
                    // This executor's descriptor registry is the layout table itself: the
                    // index of a layout's descriptor is its id, which a channel's open reads.
                    (ConstantValue::Descriptor(layout), ValueRepr::Scalar(ScalarKind::U32)) => {
                        self.record(*layout)?;
                        Val::Bits {
                            scalar: ScalarKind::U32,
                            bits: u128::from(layout.0),
                        }
                    }
                    (ConstantValue::String(s), ValueRepr::Block | ValueRepr::Uncounted) => {
                        Val::Ptr(self.static_bytes(
                            unit,
                            constant.0,
                            ty.layout,
                            s.as_bytes(),
                            "string constant",
                        )?)
                    }
                    (ConstantValue::Bytes(b), ValueRepr::Block | ValueRepr::Uncounted) => Val::Ptr(
                        self.static_bytes(unit, constant.0, ty.layout, b, "bytes constant")?,
                    ),
                    // A literal as a slice over static storage laid out as a
                    // byte sequence payload: its count, then its bytes.
                    (ConstantValue::String(_) | ConstantValue::Bytes(_), ValueRepr::Aggregate)
                        if matches!(self.record(ty.layout)?.kind, LayoutKind::Slice { .. }) =>
                    {
                        let bytes: &[u8] = match &data.value {
                            ConstantValue::String(s) => s.as_bytes(),
                            ConstantValue::Bytes(b) => b,
                            _ => unreachable!("matched above"),
                        };
                        let length = bytes.len() as u64;
                        let LayoutKind::Slice { element } = self.record(ty.layout)?.kind else {
                            unreachable!()
                        };
                        let (payload, whole) = self.byte_sequence(element, bytes)?;
                        let owner = match self.constants.get(&(unit, constant.0)) {
                            Some(p) => *p,
                            None => {
                                let bytes_layout = self
                                    .program
                                    .layouts
                                    .layouts
                                    .iter()
                                    .position(|record| {
                                        record.kind == LayoutKind::Sequence { element }
                                    })
                                    .map(|index| LayoutId(index as u32))
                                    .ok_or_else(|| {
                                        err(V::Malformed, "slice has no byte payload")
                                    })?;
                                let p = self.static_alloc(
                                    bytes_layout,
                                    &payload,
                                    "slice constant".to_string(),
                                )?;
                                self.constants.insert((unit, constant.0), p);
                                p
                            }
                        };
                        Val::Image(self.slice_image(ty.layout, owner, whole, length)?)
                    }
                    (value, repr) => {
                        return fail(V::TypeConfusion, format!("constant {value:?} as {repr:?}"));
                    }
                };
                self.define_fresh(f, result, val)?;
            }
            Inst::FunctionRef { result, symbol } => {
                let ty = self.lir_type(unit, u.values[result.index()].ty)?;
                if ty.repr == ValueRepr::Scalar(ScalarKind::Pointer) {
                    return fail(V::Unsupported, "a C callback: H does not run C");
                }
                let code = Some(self.program.function(self.symbol(unit, symbol)?)?);
                self.define_fresh(f, result, Val::Callable { code, env: None })?;
            }
            Inst::ReadStatic { result, symbol } => {
                let sym = self.symbol(unit, symbol)?;
                let index = self
                    .program
                    .statics
                    .iter()
                    .position(|s| &s.symbol == sym)
                    .ok_or_else(|| err(V::Malformed, "no static data for symbol"))?;
                let root = self.static_blocks(index)?;
                let held = self.program.statics[index].blocks[0].layout;
                let ty = self.value_type(unit, result)?;
                // A reference to the constant's block names it; any other
                // result reads the constant's value out of it.
                let val = match &self.record(ty.layout)?.kind {
                    LayoutKind::Block { payload } | LayoutKind::Box { payload }
                        if *payload == held =>
                    {
                        Val::Ptr(root)
                    }
                    _ if ty.repr == ValueRepr::Uncounted
                        && matches!(
                            self.record(ty.layout)?.kind,
                            LayoutKind::Block { .. } | LayoutKind::Box { .. }
                        ) =>
                    {
                        Val::Ptr(root)
                    }
                    _ if ty.layout == held => {
                        let size = self.size(held)?;
                        let e = self.ledger.check(root)?.extent.copy(0, size, None);
                        self.val_from(e, ty.repr, ty.layout)?.0
                    }
                    _ => return fail(V::TypeConfusion, "a static read at another layout"),
                };
                self.define_fresh(f, result, val)?;
            }
            Inst::Unary {
                result,
                op,
                operand,
                failure,
            } => {
                let Val::Bits { scalar, bits } = self.read_use(f, operand)? else {
                    return fail(V::TypeConfusion, "unary operand is not a scalar");
                };
                if scalar::unary_may_fail(op, scalar) && self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                match scalar::unary(op, scalar, bits).map_err(|m| err(V::TypeConfusion, m))? {
                    Some(bits) => self.define_fresh(f, result, Val::Bits { scalar, bits })?,
                    None => return self.fail_at(f, failure, false),
                }
            }
            Inst::Binary {
                result,
                op,
                left,
                right,
                failure,
            } => {
                let (
                    Val::Bits {
                        scalar: lk,
                        bits: l,
                    },
                    Val::Bits {
                        scalar: rk,
                        bits: r,
                    },
                ) = (self.read_use(f, left)?, self.read_use(f, right)?)
                else {
                    return fail(V::TypeConfusion, "binary operand is not a scalar");
                };
                if scalar::binary_may_fail(op, lk) && self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                match scalar::binary(op, lk, l, rk, r).map_err(|m| err(V::TypeConfusion, m))? {
                    Some((scalar, bits)) => {
                        self.define_fresh(f, result, Val::Bits { scalar, bits })?
                    }
                    None => return self.fail_at(f, failure, false),
                }
            }
            Inst::Convert {
                result,
                flag,
                operand,
                mode,
            } => {
                let Val::Bits { scalar, bits } = self.read_use(f, operand)? else {
                    return fail(V::TypeConfusion, "conversion operand is not a scalar");
                };
                let ValueRepr::Scalar(to) = self.value_type(f.unit, result)?.repr else {
                    return fail(V::TypeConfusion, "conversion result is not a scalar");
                };
                let (bits, status) = scalar::convert(mode, scalar, bits, to)
                    .map_err(|m| err(V::TypeConfusion, m))?;
                self.define_fresh(f, result, Val::Bits { scalar: to, bits })?;
                self.define_flag(f, flag, status)?;
            }
            Inst::IntArith {
                result,
                flag,
                op,
                mode,
                left,
                right,
            } => {
                let (
                    Val::Bits {
                        scalar: lk,
                        bits: l,
                    },
                    Val::Bits {
                        scalar: rk,
                        bits: r,
                    },
                ) = (self.read_use(f, left)?, self.read_use(f, right)?)
                else {
                    return fail(V::TypeConfusion, "integer operand is not a scalar");
                };
                if lk != rk {
                    return fail(V::TypeConfusion, "integer operands of two kinds");
                }
                let (bits, overflow) =
                    scalar::int_arith(op, mode, lk, l, r).map_err(|m| err(V::TypeConfusion, m))?;
                self.define_fresh(f, result, Val::Bits { scalar: lk, bits })?;
                self.define_flag(f, flag, overflow)?;
            }
            Inst::MulAdd {
                result,
                left,
                right,
                addend,
            } => {
                let mut operands = Vec::new();
                for v in [left, right, addend] {
                    let Val::Bits { scalar, bits } = self.read_use(f, v)? else {
                        return fail(V::TypeConfusion, "mul_add operand is not a scalar");
                    };
                    operands.push((scalar, bits));
                }
                let kind = operands[0].0;
                if operands.iter().any(|(k, _)| *k != kind) {
                    return fail(V::TypeConfusion, "mul_add operands of two kinds");
                }
                let bits = scalar::mul_add(kind, operands[0].1, operands[1].1, operands[2].1)
                    .map_err(|m| err(V::TypeConfusion, m))?;
                self.define_fresh(f, result, Val::Bits { scalar: kind, bits })?;
            }
            Inst::ScalarUnary {
                result,
                op,
                operand,
            } => {
                let Val::Bits { scalar: kind, bits } = self.read_use(f, operand)? else {
                    return fail(V::TypeConfusion, "scalar unary operand is not a scalar");
                };
                let bits =
                    scalar::scalar_unary(op, kind, bits).map_err(|m| err(V::TypeConfusion, m))?;
                self.define_fresh(f, result, Val::Bits { scalar: kind, bits })?;
            }
            Inst::Compare {
                result,
                op,
                left,
                right,
                layout,
                failure,
            } => {
                let (l, r) = (self.read_use(f, left)?, self.read_use(f, right)?);
                if self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                let plan = u
                    .tables
                    .structural
                    .iter()
                    .find(|p| p.root_layout() == layout)
                    .ok_or_else(|| err(V::Malformed, "comparison has no logical access plan"))?;
                if !self.structural_preflight(plan, &l)? || !self.structural_preflight(plan, &r)? {
                    return self.fail_at(f, failure, false);
                }
                let order = if plan.needs_value_program(&self.program.layouts) {
                    Ok(self.projection_order(plan, &l, &r)?)
                } else {
                    self.structural_order(&l, &r, layout)?
                };
                let Ok(order) = order else {
                    return self.fail_at(f, failure, false);
                };
                use std::cmp::Ordering as O;
                let holds = match op {
                    CompareOp::Eq => order == Some(O::Equal),
                    CompareOp::Ne => order != Some(O::Equal),
                    CompareOp::Lt => order == Some(O::Less),
                    CompareOp::Le => matches!(order, Some(O::Less | O::Equal)),
                    CompareOp::Gt => order == Some(O::Greater),
                    CompareOp::Ge => matches!(order, Some(O::Greater | O::Equal)),
                };
                self.define_fresh(
                    f,
                    result,
                    Val::Bits {
                        scalar: ScalarKind::Bool,
                        bits: u128::from(holds),
                    },
                )?;
            }
            // The Lean model's uniqueness test (`cnt = .fin 1`, the guard of
            // T05 write, T06 take and T12 reset in proofs/rc/Rc/Machine.lean): an
            // immortal count never passes it (Sticky.lean, immortal_never_unique).
            // A `Shared` root passes it as the sole root (`shared.thaw`).
            Inst::IsUnique { result, value } => {
                let Val::Ptr(p) = self.read_use(f, value)? else {
                    return fail(V::TypeConfusion, "is_unique of a non-block");
                };
                let unique = matches!(
                    self.ledger.check(p)?.count,
                    Some(Count::Finite(1) | Count::Root(1))
                );
                self.define_fresh(
                    f,
                    result,
                    Val::Bits {
                        scalar: ScalarKind::Bool,
                        bits: u128::from(unique),
                    },
                )?;
            }
            // The runtime's write test (LIR schema revision 32): one holder, or
            // an unshared tracked arena block, which a write keeps in its arena.
            Inst::IsWritable { result, value } => {
                let Val::Ptr(p) = self.read_use(f, value)? else {
                    return fail(V::TypeConfusion, "is_writable of a non-block");
                };
                let count = self.ledger.check(p)?.count;
                let writable = writable(count) || count == Some(Count::Root(1));
                self.define_fresh(
                    f,
                    result,
                    Val::Bits {
                        scalar: ScalarKind::Bool,
                        bits: u128::from(writable),
                    },
                )?;
            }
            // A semantic copy shares the tracked arena blocks its value holds
            // at inline depth (MEM-6).
            Inst::Share { value } => {
                let held: Vec<Ptr> = match self.read_use(f, value)? {
                    Val::Ptr(p) => vec![p],
                    Val::Image(image) => image
                        .cells
                        .values()
                        .filter_map(|c| self.holder_target(c))
                        .collect(),
                    _ => Vec::new(),
                };
                for p in held {
                    let a = self.ledger.get_mut(p.alloc);
                    if matches!(a.count, Some(Count::Arena { .. })) {
                        a.count = Some(Count::Arena { shared: true });
                    }
                }
            }
            Inst::ArenaPush {
                address,
                count,
                version,
                failure,
            } => {
                let count = match count {
                    None => 1,
                    Some(count) => match self.read_use(f, count)? {
                        Val::Bits {
                            scalar: ScalarKind::U64,
                            bits,
                        } => bits as u64,
                        _ => return fail(V::TypeConfusion, "an arena push count is no u64"),
                    },
                };
                let sequential = self.holds_sequential(f, address)?;
                let (r, block, element) = self.sequence_place(f, address, sequential)?;
                if let Some(region) = self.arena_push(&r, block, element, count)? {
                    return self.exhaust(f, failure, region);
                }
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                self.define_version(f, version)?;
            }
            Inst::Weaken { result, value } => {
                let m = self.take_value(f, value)?;
                if !matches!(m.val, Val::Callable { .. }) {
                    return fail(V::TypeConfusion, "weakening a non-callable");
                }
                self.bind(f, result, m)?;
            }
            Inst::Check { condition, failure } => {
                let Val::Bits {
                    scalar: ScalarKind::Bool,
                    bits,
                } = self.read_use(f, condition)?
                else {
                    return fail(V::TypeConfusion, "check condition is not a Bool");
                };
                if self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                if bits == 0 {
                    return self.fail_at(f, failure, false);
                }
            }
            Inst::StorageLive { slot } => {
                self.check_receipt_slot_lifetime(f, slot, true)?;
                let info = u
                    .slots
                    .get(slot.index())
                    .ok_or_else(|| err(V::Malformed, format!("unknown slot {slot:?}")))?;
                if matches!(info.kind, SlotKind::Capture(_)) {
                    return fail(V::Malformed, "StorageLive of a capture slot");
                }
                // Re-entering live storage (a loop body's slot) ends the old
                // storage first, as the validator allows when it holds nothing.
                match f.slots[slot.index()] {
                    SlotStore::Dead => {}
                    SlotStore::Own(ptr) => self.end_slot(ptr, slot)?,
                    SlotStore::Alias(_) => {
                        return fail(V::Malformed, format!("{slot:?} is an alias"));
                    }
                }
                let layout = self.lir_type(unit, info.ty)?.layout;
                let size = self.size(layout)?;
                let align = self.record(layout)?.align;
                let ptr = self
                    .ledger
                    .alloc(Owner::Frame, Some(layout), size, size, align, None);
                f.slots[slot.index()] = SlotStore::Own(ptr);
                f.versions[slot.index()] = None;
            }
            Inst::StorageDead { slot, if_live } => match f.slots[slot.index()] {
                SlotStore::Dead if if_live => {}
                SlotStore::Dead => return fail(V::Malformed, format!("{slot:?} is not live")),
                SlotStore::Alias(_) => return fail(V::Malformed, "StorageDead of a capture slot"),
                SlotStore::Own(ptr) => {
                    self.check_receipt_slot_lifetime(f, slot, false)?;
                    self.end_slot(ptr, slot)?;
                    f.slots[slot.index()] = SlotStore::Dead;
                }
            },
            Inst::EndScope { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. } => {}
            Inst::Address { address, failure } => {
                self.prepare_spill(f, address)?;
                let checked = self.checked_steps(unit, address)?;
                if checked && self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                if !checked {
                    // Nothing to check: the place is located if its root
                    // resolves now, and each use resolves it again.
                    let at = match self.resolve(f, address) {
                        Ok(Ok(r)) => r.located(),
                        _ => Located::Unresolved,
                    };
                    f.prepared[address.index()] = Some(at);
                    return Ok(Control::Continue);
                }
                match self.resolve(f, address)? {
                    Ok(r) => f.prepared[address.index()] = Some(r.located()),
                    Err(LangFailure) => return self.fail_at(f, failure, false),
                }
            }
            Inst::Read {
                result,
                address,
                reaching,
                mode,
            } => {
                let r = self.resolve_use(f, address)?;
                if let Some(s) = r.root_slot
                    && let Some(current) = f.versions[s.index()]
                    && current != reaching
                {
                    return fail(
                        V::Malformed,
                        format!("read names {reaching:?} but {s:?} holds {current:?}"),
                    );
                }
                let ty = self.value_type(unit, result)?;
                let image = self.read_resolved(&r, ty.layout, false)?;
                if ty.repr == ValueRepr::Scalar(ScalarKind::U8)
                    && u.addresses[address.index()]
                        .projection
                        .get(&u.pools.projections)
                        .unwrap_or(&[])
                        .iter()
                        .any(|p| matches!(p, ProjectionStep::Index(_)))
                {
                    self.ledger.counters.indexed_byte_reads += 1;
                }
                let through_slot = u.addresses[address.index()]
                    .projection
                    .get(&u.pools.projections)
                    .unwrap_or(&[])
                    .iter()
                    .any(|p| matches!(p, ProjectionStep::Slot(_)));
                if through_slot && !self.initialized(&image, 0, ty.layout)?.0 {
                    return fail(V::UninitializedRead, "element slot has an unwritten field");
                }
                match mode {
                    ReadMode::Take => {
                        self.check_writable(&r)?;
                        let e = self.read_resolved(&r, ty.layout, true)?;
                        let e = self.widened(e, ty.layout)?;
                        let (val, borrow, credited) = self.val_from(e, ty.repr, ty.layout)?;
                        f.seal_grants.retain(|(value, _)| *value != result);
                        if let Val::Arena(Some(region)) = &val
                            && borrow.is_none()
                            && let Some(slot) = r.root_slot
                            && let SlotKind::Parameter(parameter) = u.slots[slot.index()].kind
                            && self.program.layouts.fn_abis[u.header.abi.0 as usize]
                                .parameters
                                .iter()
                                .any(|p| {
                                    p.source == parameter
                                        && p.convention == hirsel_core_contract::Passing::Take
                                })
                        {
                            f.seal_grants.push((result, *region));
                        }
                        self.bind(
                            f,
                            result,
                            Moved {
                                val,
                                credited,
                                borrow,
                            },
                        )?;
                    }
                    ReadMode::Bits | ReadMode::Borrow => {
                        let e = image;
                        let e = self.widened(e, ty.layout)?;
                        let (val, cell_borrow, _) = self.val_from(e, ty.repr, ty.layout)?;
                        let info = self.value_info(unit, result)?;
                        let borrow = match (mode, info.holding) {
                            (_, Holding::Owned) if self.carries_ref(&val) => {
                                return fail(
                                    V::CreditUnderflow,
                                    format!("{mode:?} read into owned {result:?} creates a credit"),
                                );
                            }
                            (ReadMode::Borrow, Holding::Borrowed(l)) => {
                                let mut base = address;
                                let inherited = loop {
                                    match u.addresses[base.index()].base {
                                        AddressBase::Address(parent) => base = parent,
                                        AddressBase::Value(value)
                                            if self.value_info(unit, value)?.holding
                                                == info.holding =>
                                        {
                                            break f.values[value.index()]
                                                .as_ref()
                                                .and_then(|held| held.borrow);
                                        }
                                        _ => break None,
                                    }
                                };
                                let borrow = inherited.unwrap_or(Borrow::Loan(l));
                                self.check_borrow(f, borrow)?;
                                Some(borrow)
                            }
                            _ => cell_borrow,
                        };
                        self.define(
                            f,
                            result,
                            Held {
                                val,
                                credits: 0,
                                borrow,
                            },
                        )?;
                    }
                }
            }
            Inst::CopyValue { result, value } => {
                let val = self.read_use(f, value)?;
                let ty = self.value_type(unit, value)?;
                let (val, credited) = self.copy_value(val, ty.layout)?;
                self.bind(
                    f,
                    result,
                    Moved {
                        val,
                        credited,
                        borrow: None,
                    },
                )?;
            }
            Inst::Copy {
                result,
                address,
                reaching: _,
                failure,
            } => {
                if self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                let r = self.resolve_use(f, address)?;
                let ty = self.value_type(unit, result)?;
                let e = self.read_resolved(&r, ty.layout, false)?;
                let e = self.widened(e, ty.layout)?;
                let (val, _, _) = self.val_from(e, ty.repr, ty.layout)?;
                let (val, credited) = self.copy_value(val, ty.layout)?;
                self.bind(
                    f,
                    result,
                    Moved {
                        val,
                        credited,
                        borrow: None,
                    },
                )?;
            }
            Inst::FlatCopy {
                result,
                address,
                layout,
                failure,
            } => {
                if self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                let r = self.resolve_use(f, address)?;
                let ty = self.value_type(unit, result)?;
                if ty.layout != layout {
                    let length = self.read_scalar(r.ptr, 8)? as u64;
                    let at = r.ptr.at(8);
                    let allocation = self.ledger.check(at)?;
                    allocation.extent.check_range(at.offset, length)?;
                    if !allocation.extent.all_init(at.offset, length) {
                        return fail(V::UninitializedRead, "flat copy of an unwritten field");
                    }
                    let bytes = allocation.extent.copy(at.offset, length, None);
                    if !bytes.cells.is_empty() || !bytes.resources.is_empty() {
                        return fail(V::TypeConfusion, "a flat extent holds a handle");
                    }
                    let val = match self.record(ty.layout)?.kind {
                        LayoutKind::Block { payload } => {
                            let owner =
                                self.heap_alloc(payload, length, self.record(payload)?.align)?;
                            self.ledger.get_mut(owner.alloc).extent.place(0, bytes);
                            Val::Ptr(owner)
                        }
                        LayoutKind::Slice { element } => {
                            let payload = self
                                .program
                                .layouts
                                .layouts
                                .iter()
                                .position(|r| r.kind == LayoutKind::Sequence { element })
                                .map(|i| LayoutId(i as u32))
                                .ok_or_else(|| err(V::Malformed, "flat copy slice storage"))?;
                            // A whole fresh byte sequence: its count, its capacity (the
                            // count), then the bytes (runtime ABI 5).
                            let (header, whole) = self.byte_sequence(element, &[])?;
                            let first = header.len() as u64;
                            let owner = self.heap_alloc(
                                payload,
                                length + first,
                                self.record(payload)?.align,
                            )?;
                            let extent = &mut self.ledger.get_mut(owner.alloc).extent;
                            extent.write_bytes(0, &header);
                            extent.write_bytes(0, &length.to_le_bytes());
                            extent.write_bytes(
                                hirsel_lir::layout::SEQUENCE_CAPACITY_OFFSET,
                                &length.to_le_bytes(),
                            );
                            extent.place(first, bytes);
                            let mut image = self.slice_image(ty.layout, owner, whole, length)?;
                            for cell in image.cells.values_mut() {
                                if let Cell::Data { counted, .. } = cell {
                                    *counted = true;
                                }
                            }
                            Val::Image(image)
                        }
                        _ => return fail(V::TypeConfusion, "flat copy source layout"),
                    };
                    self.define_fresh(f, result, val)?;
                    return Ok(Control::Continue);
                }
                // A variable flat value is a block of its extent: copy the
                // payload the place's block holds into a new heap block.
                let (at, extent) = match (ty.repr, &self.record(layout)?.kind) {
                    (ValueRepr::Aggregate, _) => (r.ptr, layout),
                    (
                        ValueRepr::Block,
                        LayoutKind::Block { payload } | LayoutKind::Box { payload },
                    ) => (self.read_ptr(r.ptr)?, *payload),
                    (repr, _) => {
                        return fail(V::TypeConfusion, format!("flat copy into {repr:?}"));
                    }
                };
                let size = self.extent_of(extent, at)?;
                let owned = self.resources_of(extent)?;
                let e = self.ledger.check(at)?.extent.copy(
                    at.offset,
                    size,
                    Some((&self.ledger.path(at), &owned)),
                );
                if !self.initialized(&e, 0, extent)?.0 {
                    return fail(V::UninitializedRead, "flat copy of an unwritten field");
                }
                if !e.cells.is_empty() || !e.resources.is_empty() {
                    return fail(V::TypeConfusion, "a flat extent holds a handle");
                }
                let val = match ty.repr {
                    ValueRepr::Block => {
                        let align = self.record(extent)?.align;
                        let p = self.heap_alloc(extent, size, align)?;
                        self.ledger.get_mut(p.alloc).extent.place(0, e);
                        Val::Ptr(p)
                    }
                    _ => Val::Image(e),
                };
                self.define_fresh(f, result, val)?;
            }
            Inst::SliceView {
                result,
                value,
                start,
                end,
                failure,
            } => {
                let Val::Image(image) = self.read_use(f, value)? else {
                    return fail(V::TypeConfusion, "slice view source");
                };
                let (Val::Bits { bits: from, .. }, Val::Bits { bits: to, .. }) =
                    (self.read_use(f, start)?, self.read_use(f, end)?)
                else {
                    return fail(V::TypeConfusion, "slice view bounds");
                };
                let layout = self.value_type(unit, value)?.layout;
                // Independently inspect the source's bytes and range, then
                // preserve its original owner and checked origin.
                let length = self.slice_bytes(&Val::Image(image.clone()), layout)?.len() as u128;
                if self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                if from > to || to > length {
                    return self.fail_at(f, failure, false);
                }
                let fields = &self.record(layout)?.fields;
                let owner = match image.cells.get(&fields[0].offset) {
                    Some(Cell::Data { ptr, .. }) => *ptr,
                    _ => return fail(V::ForgedPointer, "slice view owner"),
                };
                let origin =
                    u64::from_le_bytes(image.read_bytes(fields[1].offset, 8).try_into().unwrap());
                let start = origin
                    .checked_add(from as u64)
                    .ok_or_else(|| err(V::OutOfExtent, "slice view origin overflow"))?;
                let val = Val::Image(self.slice_image(layout, owner, start, (to - from) as u64)?);
                self.define_fresh(f, result, val)?;
            }
            Inst::FlatView { result, address } => {
                let r = self.resolve_use(f, address)?;
                if let Holding::Borrowed(l) = self.value_info(unit, result)?.holding
                    && f.loans[l.index()].is_none()
                {
                    return fail(V::BorrowExpired, format!("flat view under inactive {l:?}"));
                }
                let layout = self.value_type(unit, result)?.layout;
                let val = self.flat_view(&r, layout)?;
                self.define_fresh(f, result, val)?;
            }
            Inst::FlatLength { result, address } => {
                let r = self.resolve_use(f, address)?;
                let count = self.flat_count(r.layout, r.ptr)?;
                self.define_fresh(
                    f,
                    result,
                    Val::Bits {
                        scalar: ScalarKind::U64,
                        bits: u128::from(count),
                    },
                )?;
            }
            Inst::FlatNext { result, address } => {
                let r = self.resolve_use(f, address)?;
                let Some(content) = r.flat_content else {
                    return fail(
                        V::TypeConfusion,
                        "the end of a place that is no variable flat element",
                    );
                };
                let end = match r.flat_end {
                    Some(end) => end,
                    None => {
                        self.ledger.counters.flat_prefix_reads += 1;
                        r.ptr.offset + self.extent_of(r.layout, r.ptr)?
                    }
                };
                self.define_fresh(
                    f,
                    result,
                    Val::Bits {
                        scalar: ScalarKind::U64,
                        bits: u128::from(end - content.offset),
                    },
                )?;
            }
            Inst::SeqLen { result, value } => {
                let layout = self.value_type(unit, value)?.layout;
                let count = match (self.read_use(f, value)?, &self.record(layout)?.kind) {
                    (Val::Ptr(p), LayoutKind::Block { payload } | LayoutKind::Box { payload })
                        if matches!(
                            self.record(*payload)?.kind,
                            LayoutKind::Sequence { .. }
                                | LayoutKind::Sequential { .. }
                                | LayoutKind::SoaColumns { .. }
                        ) =>
                    {
                        self.read_scalar(p, 8)?
                    }
                    (Val::Image(image), LayoutKind::Slice { .. }) => {
                        let length = self
                            .record(layout)?
                            .fields
                            .iter()
                            .find(|fl| fl.ordinal == 2)
                            .ok_or_else(|| err(V::Malformed, "a slice layout without its words"))?
                            .offset;
                        image.check_range(length, 8)?;
                        if !image.all_init(length, 8) {
                            return fail(V::UninitializedRead, "a slice length is not initialized");
                        }
                        let mut bytes = [0u8; 8];
                        bytes.copy_from_slice(image.read_bytes(length, 8));
                        u128::from(u64::from_le_bytes(bytes))
                    }
                    (val, kind) => {
                        return fail(
                            V::TypeConfusion,
                            format!("the sequence length of {val:?} as {kind:?}"),
                        );
                    }
                };
                self.define_fresh(
                    f,
                    result,
                    Val::Bits {
                        scalar: ScalarKind::U64,
                        bits: count,
                    },
                )?;
            }
            Inst::SeqCapacity { result, value } => {
                let layout = self.value_type(unit, value)?.layout;
                let capacity = match (self.read_use(f, value)?, &self.record(layout)?.kind) {
                    (Val::Ptr(p), LayoutKind::Block { payload } | LayoutKind::Box { payload }) => {
                        match &self.record(*payload)?.kind {
                            LayoutKind::Sequence { element } => {
                                self.sequence_capacity_word(p, *element)?
                            }
                            LayoutKind::SoaColumns { element } => {
                                self.columns_capacity(p, *element)?
                            }
                            kind => {
                                return fail(
                                    V::TypeConfusion,
                                    format!("the capacity of a block of {kind:?}"),
                                );
                            }
                        }
                    }
                    (val, kind) => {
                        return fail(
                            V::TypeConfusion,
                            format!("the capacity of {val:?} as {kind:?}"),
                        );
                    }
                };
                self.define_fresh(
                    f,
                    result,
                    Val::Bits {
                        scalar: ScalarKind::U64,
                        bits: u128::from(capacity),
                    },
                )?;
            }
            Inst::SeqReserve {
                address,
                capacity,
                version,
            } if self.holds_columns(f, address)?.is_some() => {
                let element = self.holds_columns(f, address)?.unwrap();
                self.reserve_columns(f, address, capacity, version, element)?;
            }
            Inst::SeqReserve {
                address,
                capacity,
                version,
            } if self.holds_sequential(f, address)? => {
                // A flat builder's reservation: room for `capacity` of its least
                // element in all, its unique block kept (LIR schema revision 22).
                let (r, mut block, element) = self.sequence_place(f, address, true)?;
                self.closed_builder(block)?;
                let least = match self.flat_prefixed(element)? {
                    Some((prefix, content)) => prefix.saturating_add(
                        hirsel_lir::layout::least_size(&self.record(content)?.size),
                    ),
                    None => hirsel_lir::layout::least_size(&self.record(element)?.size),
                };
                let Val::Bits { bits, .. } = self.read_use(f, capacity)? else {
                    return fail(V::TypeConfusion, "a capacity that is not an integer");
                };
                let used = u64::try_from(self.read_scalar(block.at(8), 8)?)
                    .map_err(|_| err(V::Malformed, "a used byte count beyond u64"))?;
                let start = self.sequential_start(element)?;
                let need = u64::try_from(bits)
                    .ok()
                    .map(|n| n.saturating_mul(least).saturating_add(start))
                    .ok_or_else(|| err(V::Malformed, "a reservation beyond u64"))?;
                // Empty Packed lists use the same static empty storage as other
                // lists. Reserve their final backing directly, without first
                // detaching an empty header and immediately growing it again.
                let count = self.read_scalar(block, 8)?;
                if count == 0 && self.check_private(block).is_err() {
                    let Cell::Data { counted, .. } = self.read_cell(r.ptr)? else {
                        return fail(V::TypeConfusion, "an empty reservation without a holder");
                    };
                    let old = block;
                    block = self.detach_block(old, need)?;
                    let word = self.word;
                    self.ledger.get_mut(r.ptr.alloc).extent.put_cell(
                        r.ptr.offset,
                        word,
                        Cell::Data {
                            ptr: block,
                            counted: true,
                            borrow: None,
                        },
                    );
                    if counted && self.count_down(old)? {
                        self.ledger.free(old.alloc)?;
                    }
                } else {
                    self.check_private(block)?;
                }
                self.sequential_room(&r, block, start + used, need)?;
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                self.define_version(f, version)?;
            }
            Inst::SeqReserve {
                address,
                capacity,
                version,
            } => {
                let (r, block, element) = self.sequence_place(f, address, false)?;
                let Val::Bits { bits, .. } = self.read_use(f, capacity)? else {
                    return fail(V::TypeConfusion, "a capacity that is not an integer");
                };
                let (start, stride) = self.sequence_shape(element)?;
                let a = self.ledger.check(block)?;
                let private =
                    a.count == Some(Count::Finite(1)) && !a.published && a.owner == Owner::Heap;
                if matches!(a.count, Some(Count::Arena { .. })) {
                    // A tracked arena block's reservation is a hint (MEM-6):
                    // only a push grows it, charged.
                } else if !private {
                    // Shared, published or static: detached once, at the
                    // requested capacity, and the place's credit of the
                    // original given back.
                    let count = u64::from_le_bytes(
                        a.extent
                            .read_bytes(block.offset, 8)
                            .try_into()
                            .expect("a count"),
                    );
                    let size = u64::try_from(bits)
                        .ok()
                        .map(|n| n.max(count))
                        .and_then(|n| n.checked_mul(stride))
                        .and_then(|n| n.checked_add(start))
                        .ok_or_else(|| err(V::Malformed, "a sequence capacity beyond u64"))?;
                    let Cell::Data { counted, .. } = self.read_cell(r.ptr)? else {
                        return fail(V::TypeConfusion, "a sequence place holding a code word");
                    };
                    let grown = self.detach_block(block, size)?;
                    self.exact_capacity(grown, element)?;
                    let word = self.word;
                    self.ledger.get_mut(r.ptr.alloc).extent.put_cell(
                        r.ptr.offset,
                        word,
                        Cell::Data {
                            ptr: grown,
                            counted: true,
                            borrow: None,
                        },
                    );
                    if counted && self.count_down(block)? {
                        return fail(V::Malformed, "a shared sequence block died at a reserve");
                    }
                    if !counted && self.counted_target(block) {
                        return fail(
                            V::CreditUnderflow,
                            format!("reserve replaced {:?} held without credit", block.alloc),
                        );
                    }
                } else if stride != 0 && u128::from(self.sequence_capacity(block, element)?) < bits
                {
                    let size = u64::try_from(bits)
                        .ok()
                        .and_then(|n| n.checked_mul(stride))
                        .and_then(|n| n.checked_add(start))
                        .ok_or_else(|| err(V::Malformed, "a sequence capacity beyond u64"))?;
                    let old = self.ledger.get(block.alloc);
                    let (payload, align, held) = (old.layout, old.align, old.extent.len());
                    let Some(payload) = payload else {
                        return fail(V::UseAfterFree, "a reserve of raw storage");
                    };
                    // The elements move bitwise; the old storage is freed
                    // shallowly, never scanned.
                    let moved =
                        self.ledger
                            .get_mut(block.alloc)
                            .extent
                            .take(0, held, FREE_POISON, None);
                    let grown = self.heap_alloc(payload, size, align)?;
                    self.ledger.get_mut(grown.alloc).extent.place(0, moved);
                    self.exact_capacity(grown, element)?;
                    self.ledger.get_mut(block.alloc).count = None;
                    self.ledger.free(block.alloc)?;
                    let word = self.word;
                    self.ledger.get_mut(r.ptr.alloc).extent.put_cell(
                        r.ptr.offset,
                        word,
                        Cell::Data {
                            ptr: grown,
                            counted: true,
                            borrow: None,
                        },
                    );
                } else {
                    // Left alone, its capacity word made exact.
                    self.exact_capacity(block, element)?;
                }
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                self.define_version(f, version)?;
            }
            // MEM-13: the element's active extent past the used bytes; full storage
            // moves bitwise into a block of twice its size or more (LIR schema
            // revision 19), freed shallowly as the elements hold no reference.
            Inst::SeqAppend {
                address,
                value,
                version,
            } => {
                let (r, block, element) = self.sequence_write(f, address, true)?;
                self.closed_builder(block)?;
                let m = self.take_value(f, value)?;
                let Val::Image(image) = &m.val else {
                    return fail(V::TypeConfusion, "an appended element is no image");
                };
                let tag = image.active(0, TagForm::Ordinal, self.word)?;
                let bytes = self.walked_extent(element, tag)?;
                let count = u64::try_from(self.read_scalar(block, 8)?)
                    .map_err(|_| err(V::Malformed, "a sequence count beyond u64"))?;
                let used = u64::try_from(self.read_scalar(block.at(8), 8)?)
                    .map_err(|_| err(V::Malformed, "a used byte count beyond u64"))?;
                let keep = self.sequential_start(element)? + used;
                let need = keep + bytes;
                let block = self.sequential_room(&r, block, keep, need)?;
                let at = self
                    .ledger
                    .project(block, keep, ProjectionStep::Index(count));
                self.write_value(at, element, m)?;
                let extent = &mut self.ledger.get_mut(block.alloc).extent;
                extent.write_bytes(block.offset, &(count + 1).to_le_bytes());
                extent.write_bytes(block.offset + 8, &(used + bytes).to_le_bytes());
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                self.define_version(f, version)?;
            }
            // An element built in its flat encoding past the used bytes (LIR
            // schema revision 22): its length prefix, then exactly the bytes a
            // flat construction of the site writes. Storage grows as for
            // `SeqAppend`.
            Inst::FlatAppend {
                address,
                construct,
                version,
            } => {
                let (r, block, element) = self.sequence_write(f, address, true)?;
                let Some((8, content)) = self.flat_prefixed(element)? else {
                    return fail(V::TypeConfusion, "a flat append of an unprefixed element");
                };
                let site = self.construct_site(unit, construct)?;
                if site.layout != content {
                    return fail(V::TypeConfusion, "a flat append of another record");
                }
                self.closed_builder(block)?;
                let fields = self.pool(site.fields, &self.unit(f.unit).pools.fields)?;
                let mut bytes = self.flat_content(f, site, fields, None)?;
                let mut encoded = (bytes.len() as u64).to_le_bytes().to_vec();
                encoded.append(&mut bytes);
                let count = u64::try_from(self.read_scalar(block, 8)?)
                    .map_err(|_| err(V::Malformed, "a sequence count beyond u64"))?;
                let used = u64::try_from(self.read_scalar(block.at(8), 8)?)
                    .map_err(|_| err(V::Malformed, "a used byte count beyond u64"))?;
                let keep = self.sequential_start(element)? + used;
                let need = keep + encoded.len() as u64;
                let block = self.sequential_room(&r, block, keep, need)?;
                let extent = &mut self.ledger.get_mut(block.alloc).extent;
                extent.write_bytes(block.offset + keep, &encoded);
                extent.write_bytes(block.offset, &(count + 1).to_le_bytes());
                extent.write_bytes(
                    block.offset + 8,
                    &(used + encoded.len() as u64).to_le_bytes(),
                );
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                self.define_version(f, version)?;
            }
            // An open element's writes past the used bytes (LIR schema
            // revision 24): the count and the used bytes are kept, and growth
            // keeps the open element's bytes.
            Inst::FlatField {
                address,
                field,
                op: FlatFieldOp::Open,
                version,
            } => {
                let (r, block, element) = self.sequence_write(f, address, true)?;
                let Some((8, content)) = self.flat_prefixed(element)? else {
                    return fail(V::TypeConfusion, "an open element of an unprefixed builder");
                };
                if self.open_elements.contains_key(&block.alloc) {
                    return fail(V::Malformed, "an element opened in a builder with one open");
                }
                let used = u64::try_from(self.read_scalar(block.at(8), 8)?)
                    .map_err(|_| err(V::Malformed, "a used byte count beyond u64"))?;
                let keep = self.sequential_start(element)? + used;
                let word = keep + 8 + self.open_field(content, field)?;
                let block = self.sequential_room(&r, block, keep, word + 8)?;
                self.ledger
                    .get_mut(block.alloc)
                    .extent
                    .write_bytes(block.offset + word, &0u64.to_le_bytes());
                self.open_elements.insert(block.alloc, field);
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                self.define_version(f, version)?;
            }
            Inst::FlatField {
                address,
                field,
                op: FlatFieldOp::Reserve { capacity },
                version,
            } => {
                let (r, block, _, _, word, len) = self.open_element(f, address, field)?;
                let Val::Bits { bits, .. } = self.read_use(f, capacity)? else {
                    return fail(V::TypeConfusion, "a capacity that is not an integer");
                };
                let keep = word + 8 + len;
                let need = u64::try_from(bits).unwrap_or(u64::MAX).saturating_add(keep);
                self.sequential_room(&r, block, keep, need)?;
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                self.define_version(f, version)?;
            }
            Inst::FlatField {
                address,
                field,
                op: FlatFieldOp::Push { value, spare },
                version,
            } => {
                let (r, block, _, _, word, len) = self.open_element(f, address, field)?;
                let Val::Bits { bits, .. } = self.read_use(f, value)? else {
                    return fail(V::TypeConfusion, "a pushed byte that is not an integer");
                };
                let keep = word + 8 + len;
                // A spare push writes where its proof says room is (FACT-3).
                if spare && keep + 1 > self.ledger.get(block.alloc).extent.len() {
                    return fail(V::OutOfExtent, "a spare push past the builder's capacity");
                }
                let block = self.sequential_room(&r, block, keep, keep + 1)?;
                let extent = &mut self.ledger.get_mut(block.alloc).extent;
                extent.write_bytes(block.offset + keep, &[bits as u8]);
                extent.write_bytes(block.offset + word, &(len + 1).to_le_bytes());
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                self.define_version(f, version)?;
            }
            // The open element completed: the construction's other fields
            // written around the bytes in place, as `FlatAppend` writes an
            // element (LIR schema revision 24).
            Inst::FlatClose {
                address,
                construct,
                field,
                version,
            } => {
                let (r, block, content, _, word, len) = self.open_element(f, address, field)?;
                let site = self.construct_site(unit, construct)?;
                if site.layout != content {
                    return fail(V::TypeConfusion, "a flat close of another record");
                }
                let a = self.ledger.check(block)?;
                if !a.extent.all_init(block.offset + word, 8 + len) {
                    return fail(V::UninitializedRead, "an open field's bytes");
                }
                let in_place = a.extent.read_bytes(block.offset + word, 8 + len).to_vec();
                let fields = self.pool(site.fields, &self.unit(f.unit).pools.fields)?;
                let mut bytes = self.flat_content(f, site, fields, Some((field, in_place)))?;
                let mut encoded = (bytes.len() as u64).to_le_bytes().to_vec();
                encoded.append(&mut bytes);
                let count = u64::try_from(self.read_scalar(block, 8)?)
                    .map_err(|_| err(V::Malformed, "a sequence count beyond u64"))?;
                let used = u64::try_from(self.read_scalar(block.at(8), 8)?)
                    .map_err(|_| err(V::Malformed, "a used byte count beyond u64"))?;
                let element = word - 8 - self.open_field(content, field)?;
                let need = element + encoded.len() as u64;
                let block = self.sequential_room(&r, block, word + 8 + len, need)?;
                self.open_elements.remove(&block.alloc);
                let extent = &mut self.ledger.get_mut(block.alloc).extent;
                extent.write_bytes(block.offset + element, &encoded);
                extent.write_bytes(block.offset, &(count + 1).to_le_bytes());
                extent.write_bytes(
                    block.offset + 8,
                    &(used + encoded.len() as u64).to_le_bytes(),
                );
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                self.define_version(f, version)?;
            }
            Inst::SeqSetLen {
                address,
                len,
                version,
            } if self.holds_columns(f, address)?.is_some() => {
                let element = self.holds_columns(f, address)?.unwrap();
                self.set_columns_len(f, address, len, version, element)?;
            }
            Inst::SeqSetLen {
                address,
                len,
                version,
            } => {
                let (r, block, element) = self.sequence_write(f, address, false)?;
                let Val::Bits { bits, .. } = self.read_use(f, len)? else {
                    return fail(V::TypeConfusion, "a length that is not an integer");
                };
                let (start, stride) = self.sequence_shape(element)?;
                let slots = self.sequence_capacity(block, element)?;
                if bits > u128::from(slots) {
                    return fail(
                        V::OutOfExtent,
                        format!("length {bits} of a sequence holding {slots}"),
                    );
                }
                let len = bits as u64;
                if stride != 0 {
                    let extent = &self.ledger.get(block.alloc).extent;
                    for i in 0..slots {
                        let at = block.offset + start + i * stride;
                        let (initialized, live) = self.initialized(extent, at, element)?;
                        if i < len && !initialized {
                            return fail(
                                V::UninitializedRead,
                                format!("length {len} covers uninitialized element slot {i}"),
                            );
                        }
                        if i >= len && live {
                            return fail(
                                V::LeakedSlot,
                                format!("length {len} abandons initialized element slot {i}"),
                            );
                        }
                    }
                }
                self.ledger
                    .get_mut(block.alloc)
                    .extent
                    .write_bytes(block.offset, &len.to_le_bytes());
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                self.define_version(f, version)?;
            }
            Inst::Hash {
                result,
                value,
                layout,
                failure,
            } => {
                let val = self.read_use(f, value)?;
                if self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                let plan = u
                    .tables
                    .structural
                    .iter()
                    .find(|p| p.root_layout() == layout)
                    .ok_or_else(|| err(V::Malformed, "hash has no logical access plan"))?;
                if !self.structural_preflight(plan, &val)? {
                    return self.fail_at(f, failure, false);
                }
                let hash = if plan.needs_value_program(&self.program.layouts) {
                    Ok(self.projection_hash(plan, &val)?)
                } else {
                    self.structural_hash(&val, layout)?
                };
                let Ok(hash) = hash else {
                    return self.fail_at(f, failure, false);
                };
                self.define_fresh(
                    f,
                    result,
                    Val::Bits {
                        scalar: ScalarKind::U64,
                        bits: u128::from(hash),
                    },
                )?;
                if let Some(pending) = &mut f.pending_custody {
                    if pending.admitted {
                        return fail(V::FrameImbalance, "adapter admits pending custody twice");
                    }
                    pending.admitted = true;
                }
            }
            Inst::Encode {
                result,
                value,
                codec,
            }
            | Inst::Decode {
                result,
                value,
                codec,
            } => {
                let plan = u
                    .tables
                    .codecs
                    .get(codec.index())
                    .ok_or_else(|| err(V::Malformed, "unknown codec"))?;
                let val = self.read_use(f, value)?;
                let (produced, checks) = if matches!(inst, Inst::Encode { .. }) {
                    (self.codec_encode(plan, &val)?, vec![])
                } else {
                    self.codec_decode(plan, &val)?
                };
                if checks.is_empty() {
                    self.define_fresh(f, result, produced)?;
                } else {
                    let mut pending = checks
                        .into_iter()
                        .map(|(check, occurrence)| {
                            let check = plan.checks.get(check as usize).ok_or_else(|| {
                                err(V::Malformed, "a codec check outside its plan")
                            })?;
                            Ok((
                                self.program.function(self.symbol(f.unit, check.symbol)?)?,
                                occurrence,
                            ))
                        })
                        .collect::<Checked<Vec<_>>>()?;
                    pending.reverse();
                    let layout = self.value_type(f.unit, result)?.layout;
                    let held = self.temporary(layout)?;
                    self.write_value(
                        held,
                        layout,
                        Moved {
                            val: produced,
                            credited: true,
                            borrow: None,
                        },
                    )?;
                    let (unit, occurrence) = pending.pop().expect("a check");
                    let Some(frame) = self.check_frame(f, unit, occurrence)? else {
                        return Ok(Control::Continue);
                    };
                    f.resume = Resume::Checks {
                        result,
                        held,
                        pending,
                    };
                    return Ok(Control::Push(Activity::Frame(Box::new(frame))));
                }
            }
            Inst::Recast { result, value } => {
                let held = self.get(f, value)?.clone();
                let Val::Ptr(p) = held.val else {
                    return fail(V::TypeConfusion, "a recast of a value that is no block");
                };
                // A recast back from the opaque source must name the layout
                // the block was built at.
                let to = self.value_type(unit, result)?.layout;
                if let LayoutKind::Block { payload } | LayoutKind::Box { payload } =
                    self.record(to)?.kind
                    && self.record(payload)?.kind != LayoutKind::Unit
                    && self.ledger.check(p)?.layout != Some(payload)
                {
                    return fail(
                        V::TypeConfusion,
                        format!(
                            "a recast of {:?}, built at {:?}, to {payload:?}",
                            p.alloc,
                            self.ledger.get(p.alloc).layout
                        ),
                    );
                }
                if let Some(borrow) = held.borrow {
                    self.check_borrow(f, borrow)?;
                }
                if self.value_info(unit, result)?.holding == Holding::Owned {
                    let moved = self.take_owned(f, value, "sequence recast")?;
                    self.bind(f, result, moved)?;
                } else {
                    self.define(
                        f,
                        result,
                        Held {
                            val: Val::Ptr(p),
                            credits: 0,
                            borrow: held.borrow,
                        },
                    )?;
                }
            }
            Inst::UniqueCopy {
                result,
                value,
                construct,
                deep,
                failure,
            } => {
                if self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                let site = self.construct_site(unit, construct)?;
                if !deep
                    && !matches!(
                        site.placement,
                        Some(Placement::Fixed(StorageOwner::Heap(_)))
                    )
                {
                    return fail(V::Unsupported, "shallow detach into a non-heap destination");
                }
                let val = self.read_use(f, value)?;
                if deep && let Some(region) = self.bounded_region(f, site.placement)? {
                    let layout = self.value_type(unit, value)?.layout;
                    let bytes = self.copy_charge(&val, layout)?;
                    if !self.charge(region, bytes) {
                        return self.exhaust(f, failure, region);
                    }
                }
                if !deep
                    && let Val::Ptr(p) = val
                    && let Some(region) = self.tracked_region(p)?
                {
                    // A tracked arena block is copied within its arena,
                    // charged its count (MEM-6); a Map's or Set's block
                    // charges nothing itself.
                    let layout = self.ledger.get(p.alloc).layout.expect("live");
                    let len = if hirsel_lir::layout::keyed(&self.program.layouts, layout) {
                        0
                    } else {
                        self.logical_count(p, layout)?
                    };
                    let bytes = self.block_charge(layout, len)?;
                    if !self.charge(region, bytes) {
                        return self.exhaust(f, failure, region);
                    }
                }
                let val = if deep {
                    let layout = self.value_type(unit, value)?.layout;
                    self.deep_copy(f, site.placement, site.tracked, layout, val)?
                } else {
                    self.detach(val)?
                };
                self.define_fresh(f, result, val)?;
            }
            Inst::Freeze { result, value } => {
                let (payload, frozen) =
                    self.frozen_layouts(self.value_type(unit, result)?.layout)?;
                let m = self.take_owned(f, value, "freeze")?;
                let (size, align) = (self.size(payload)?, self.record(payload)?.align);
                let root = self.heap_alloc(payload, size, align)?;
                self.ledger.get_mut(root.alloc).count = Some(Count::Root(1));
                self.write_value(root, frozen, m)?;
                self.refreeze(root.alloc, Count::Finite(1), Count::Frozen { copies: 0 })?;
                self.define_fresh(f, result, Val::Ptr(root))?;
            }
            Inst::Thaw { result, value } => {
                let (_, frozen) = self.frozen_layouts(self.value_type(unit, value)?.layout)?;
                let m = self.take_owned(f, value, "thaw")?;
                let Val::Ptr(root) = m.val else {
                    return fail(V::TypeConfusion, "a thaw of a value that is no root");
                };
                if self.ledger.check(root)?.count != Some(Count::Root(1)) {
                    return fail(
                        V::WriteShared,
                        format!("thaw of {:?}, which is not the last root", root.alloc),
                    );
                }
                self.refreeze(root.alloc, Count::Frozen { copies: 0 }, Count::Finite(1))?;
                let ty = self.value_type(unit, result)?;
                let (size, owned) = (self.size(frozen)?, self.resources_of(frozen)?);
                let e = self.take_part(root, size, &owned);
                let e = self.widened(e, frozen)?;
                let (val, borrow, credited) = self.val_from(e, ty.repr, frozen)?;
                self.bind(
                    f,
                    result,
                    Moved {
                        val,
                        credited,
                        borrow,
                    },
                )?;
                let a = self.ledger.get_mut(root.alloc);
                a.count = Some(Count::Root(0));
                a.state = AllocState::Dying;
                self.ledger.free(root.alloc)?;
            }
            Inst::Store {
                address,
                value,
                version,
            } => {
                let r = self.resolve_use(f, address)?;
                self.check_writable(&r)?;
                let layout = self.address_layout(unit, address)?;
                let m = self.take_value(f, value)?;
                let allocation = self.ledger.check(r.ptr)?;
                if let Some(environment) = allocation.layout
                    && let LayoutKind::Environment { movable } = &self.record(environment)?.kind
                    && self
                        .record(environment)?
                        .fields
                        .last()
                        .is_some_and(|word| word.offset == r.ptr.offset && word.layout == layout)
                {
                    let old = self.read_scalar(r.ptr, 8)? as u64;
                    let Val::Bits {
                        scalar: ScalarKind::U64,
                        bits,
                    } = m.val
                    else {
                        return fail(V::TypeConfusion, "live word store");
                    };
                    let new = bits as u64;
                    if old & new != new
                        || (old ^ new).count_ones() != 1
                        || (old ^ new).trailing_zeros() as usize >= movable.len()
                    {
                        return fail(V::Malformed, "live word store is not one clear");
                    }
                    self.ledger.get_mut(r.ptr.alloc).extent.take(
                        r.ptr.offset,
                        8,
                        FREE_POISON,
                        Some((&[], &[])),
                    );
                }
                self.write_resolved(&r, layout, m)?;
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                if version != value {
                    self.define(
                        f,
                        version,
                        Held {
                            val: Val::Version,
                            credits: 0,
                            borrow: None,
                        },
                    )?;
                }
            }
            Inst::Replace {
                address,
                value,
                displaced,
                version,
                displaced_version,
                if_live,
                failure: _,
            } => {
                // Not a fault-injection site: native code exchanges the two
                // contents with nothing between the stores that can fail
                // (hirsel-llvm emit.rs), so no native run takes this edge.
                let r = self.resolve_use(f, address)?;
                self.check_writable(&r)?;
                let layout = self.address_layout(unit, address)?;
                let size = self.size(layout)?;
                let live = if r.columns.is_some() {
                    let image = self.read_resolved(&r, layout, false)?;
                    self.initialized(&image, 0, layout)?.1
                } else {
                    self.ledger
                        .check(r.ptr)?
                        .extent
                        .check_range(r.ptr.offset, size)?;
                    self.has_content(r.ptr, size)?
                };
                if live {
                    let SlotStore::Own(d) = f.slots[displaced.index()] else {
                        return fail(V::Malformed, format!("displaced {displaced:?} is not live"));
                    };
                    if self.has_content(d, size)? {
                        return fail(
                            V::StoreOverInitialized,
                            "displaced slot already holds a value",
                        );
                    }
                    let old = self.read_resolved(&r, layout, true)?;
                    self.ledger.get_mut(d.alloc).extent.place(0, old);
                } else if !if_live {
                    return fail(V::UninitializedRead, "replace of uninitialized storage");
                }
                let m = self.take_value(f, value)?;
                let allocation = self.ledger.check(r.ptr)?;
                if let Some(environment) = allocation.layout
                    && let LayoutKind::Environment { movable } = &self.record(environment)?.kind
                    && self
                        .record(environment)?
                        .fields
                        .last()
                        .is_some_and(|word| word.offset == r.ptr.offset && word.layout == layout)
                {
                    let old = self.read_scalar(r.ptr, 8)? as u64;
                    let Val::Bits {
                        scalar: ScalarKind::U64,
                        bits,
                    } = m.val
                    else {
                        return fail(V::TypeConfusion, "live word store");
                    };
                    let new = bits as u64;
                    if old & new != new
                        || (old ^ new).count_ones() != 1
                        || (old ^ new).trailing_zeros() as usize >= movable.len()
                    {
                        return fail(V::Malformed, "live word store is not one clear");
                    }
                    self.ledger.get_mut(r.ptr.alloc).extent.take(
                        r.ptr.offset,
                        8,
                        FREE_POISON,
                        Some((&[], &[])),
                    );
                }
                self.write_resolved(&r, layout, m)?;
                if let Some(s) = r.root_slot {
                    f.versions[s.index()] = Some(version);
                }
                if version != value {
                    self.define(
                        f,
                        version,
                        Held {
                            val: Val::Version,
                            credits: 0,
                            borrow: None,
                        },
                    )?;
                }
                // The displaced content's own reaching version.
                f.versions[displaced.index()] = Some(displaced_version);
                self.define(
                    f,
                    displaced_version,
                    Held {
                        val: Val::Version,
                        credits: 0,
                        borrow: None,
                    },
                )?;
            }
            Inst::UpdateMask {
                result,
                mask,
                field,
                initialized,
            } => {
                let current = self.read_use(f, mask)?;
                if let Val::Bits {
                    scalar: ScalarKind::U64,
                    bits,
                } = current
                {
                    if field >= 64 {
                        return fail(V::Malformed, "live bit outside its word");
                    }
                    let bits = if initialized {
                        bits | (1u128 << field)
                    } else {
                        bits & !(1u128 << field)
                    };
                    self.define_fresh(
                        f,
                        result,
                        Val::Bits {
                            scalar: ScalarKind::U64,
                            bits,
                        },
                    )?;
                    return Ok(Control::Continue);
                }
                let Val::Mask(mut bits) = current else {
                    return fail(V::TypeConfusion, format!("{mask:?} is not a mask"));
                };
                let slot = bits
                    .get_mut(field as usize)
                    .ok_or_else(|| err(V::Malformed, "mask field out of range"))?;
                *slot = initialized;
                self.define_fresh(f, result, Val::Mask(bits))?;
            }
            Inst::Construct {
                result,
                construct,
                failure,
            } => {
                let site = self.construct_site(unit, construct)?;
                let layout = self.value_type(unit, result)?.layout;
                if let Some(region) = self.bounded_region(f, site.placement)? {
                    let bytes = self.construct_charge(f, site, layout)?;
                    if !self.charge(region, bytes) {
                        return self.exhaust(f, failure, region);
                    }
                }
                let val = self.construct(f, site, None, layout)?;
                if !self.config.seal_sources.is_empty()
                    && let Val::Ptr(ptr) = &val
                {
                    let anchor = self.unit(unit).tables.source_anchors[site.anchor.index()].clone();
                    self.seal_allocations.insert(ptr.alloc, (unit, anchor));
                }
                self.define_fresh(f, result, val)?;
            }
            // A constructor-tail box opens at its stand-in layout, whose
            // payload has nothing to release; its fill installs the box's
            // payload and layout. An abandonment releases the root by each
            // block's own layout, as the runtime's header walk does, so an
            // open block frees its shell alone (LIR schema revision 59).
            Inst::HoleOpen {
                result,
                hole,
                construct,
                open,
                ..
            } => {
                let site = self.construct_site(unit, construct)?;
                let LayoutKind::Box { payload } = self.record(open)?.kind else {
                    return fail(V::Malformed, "a hole's stand-in is not a box");
                };
                let target = self.place(f, site.placement, payload)?;
                self.open_holes.set(target.alloc.0 as usize, ());
                self.define_fresh(f, result, Val::Ptr(target))?;
                self.define_fresh(f, hole, Val::Hole(target))?;
            }
            Inst::HoleFill { hole, value } => {
                let target = self.take_hole(f, hole, "fill")?;
                let LayoutKind::Box { payload } =
                    self.record(self.value_type(unit, hole)?.layout)?.kind
                else {
                    return fail(V::Malformed, "a hole of no box");
                };
                let m = self.take_owned(f, value, "hole fill")?;
                self.ledger.check(target)?;
                if self.ledger.get(target.alloc).layout == Some(payload) {
                    return fail(V::StoreOverInitialized, "a fill of a filled hole");
                }
                self.ledger.get_mut(target.alloc).layout = Some(payload);
                self.open_holes.remove(target.alloc.0 as usize);
                self.write_value(target, payload, m)?;
            }
            Inst::HoleAbandon { hole, root } => {
                self.take_hole(f, hole, "abandonment")?;
                if let Some(root) = root {
                    let Val::Ptr(block) = self.take_owned(f, root, "hole abandonment")?.val else {
                        return fail(V::TypeConfusion, "an abandoned root is no block");
                    };
                    let mut work = Vec::new();
                    self.decrement(block, None, &mut work)?;
                    return Ok(self.drain(f, work, After::Next, None));
                }
            }
            Inst::Closure {
                result,
                closure,
                failure,
            } => {
                let site = u
                    .tables
                    .closures
                    .get(closure.index())
                    .ok_or_else(|| err(V::Malformed, format!("unknown closure {closure:?}")))?;
                if let Some(region) = self.bounded_region(f, site.placement)? {
                    let bytes = self.environment_charge(site)?;
                    if !self.charge(region, bytes) {
                        return self.exhaust(f, failure, region);
                    }
                }
                let val = self.closure(f, closure)?;
                self.define_fresh(f, result, val)?;
            }
            Inst::BeginLoan {
                loan,
                address,
                mutable: _,
                if_inactive,
            } => {
                let slot = f
                    .loans
                    .get(loan.index())
                    .ok_or_else(|| err(V::Malformed, format!("unknown loan {loan:?}")))?;
                if slot.is_some() {
                    if !if_inactive {
                        return fail(V::Malformed, format!("{loan:?} begun while active"));
                    }
                } else {
                    let r = self.resolve_use(f, address)?;
                    f.loans[loan.index()] = Some(r.ptr);
                }
            }
            Inst::EndLoan { loan, if_active } => {
                let slot = f
                    .loans
                    .get_mut(loan.index())
                    .ok_or_else(|| err(V::Malformed, format!("unknown loan {loan:?}")))?;
                if slot.take().is_none() && !if_active {
                    return fail(V::Malformed, format!("{loan:?} ended while inactive"));
                }
                if let Some((owner, pinned)) = f.sealed_loans[loan.index()].take() {
                    self.ledger.end_sealed_loan(owner, pinned)?;
                }
            }
            Inst::Drop {
                address,
                if_live,
                anchor: _,
                failure,
            } => {
                let r = self.resolve_use(f, address)?;
                let layout = self.address_layout(unit, address)?;
                let size = self.size(layout)?;
                let a = self.ledger.check(r.ptr)?;
                a.extent.check_range(r.ptr.offset, size)?;
                if if_live
                    && !a.extent.any_init(r.ptr.offset, size)
                    && !a
                        .extent
                        .resources
                        .keys()
                        .any(|path| path.starts_with(&self.ledger.path(r.ptr)))
                {
                    return Ok(Control::Continue);
                }
                self.check_writable(&r)?;
                // A part moved out is not dropped again: the validator tracks
                // moved parts, and H derives what is left from its own cells.
                let record = self.record(layout)?;
                let fields = if let Some(tag) = &record.tag {
                    let active = self
                        .active_variant(r.ptr.at(tag.offset), tag.form)
                        .map_err(|v| {
                            if v.kind == V::UninitializedRead {
                                err(V::UninitializedScan, v.message)
                            } else {
                                v
                            }
                        })?;
                    &tag.variants
                        .iter()
                        .find(|v| v.ordinal == active)
                        .ok_or_else(|| err(V::Malformed, "unknown variant tag"))?
                        .fields
                } else {
                    &record.fields
                };
                let mask = if matches!(
                    record.kind,
                    LayoutKind::Block { .. } | LayoutKind::Box { .. }
                ) || fields.is_empty()
                {
                    None
                } else {
                    let a = self.ledger.check(r.ptr)?;
                    let mut bits = Vec::new();
                    for fl in fields {
                        let n = self.size(fl.layout)?;
                        let i = fl.ordinal as usize;
                        if bits.len() <= i {
                            bits.resize(i + 1, true);
                        }
                        let step = match &record.tag {
                            Some(tag) => ProjectionStep::VariantField {
                                variant: self.active_variant(r.ptr.at(tag.offset), tag.form)?,
                                field: fl.ordinal,
                            },
                            None => ProjectionStep::Field(fl.ordinal),
                        };
                        let part = self.ledger.project(r.ptr, fl.offset, step);
                        let walk = self.resources_of(fl.layout)?;
                        let resource_present = a
                            .extent
                            .resources
                            .keys()
                            .any(|path| path.starts_with(&self.ledger.path(part)));
                        bits[i] = a.extent.any_init(part.offset, n)
                            || resource_present
                            || (n == 0 && walk.is_empty());
                    }
                    bits.iter().any(|b| !b).then_some(bits)
                };
                return Ok(self.drain(
                    f,
                    vec![Work::DropAt {
                        at: r.ptr,
                        layout,
                        mask,
                    }],
                    After::Next,
                    Some(failure),
                ));
            }
            Inst::Dup { value } => {
                let val = self.read_use(f, value)?;
                match val {
                    Val::Ptr(p) | Val::Callable { env: Some(p), .. } => self.dup(p)?,
                    other => return fail(V::TypeConfusion, format!("dup of {other:?}")),
                }
                let held = f.values[value.index()].as_mut().expect("defined");
                held.credits += 1;
                self.holders.value(&held.val, 1);
            }
            Inst::Release {
                value,
                layout,
                live,
                anchor: _,
                failure,
            } => {
                // The instruction names the released block's payload layout,
                // as C checks; the value itself is released through its own
                // block-reference layout, whose drop program releases it.
                let own = self.value_type(unit, value)?.layout;
                let payload = match self.record(own)?.kind {
                    LayoutKind::Block { payload }
                    | LayoutKind::Box { payload }
                    | LayoutKind::Shared { payload } => Some(payload),
                    _ => None,
                };
                if layout != own && payload != Some(layout) {
                    return fail(
                        V::Malformed,
                        format!("release of {value:?} names {layout:?}, not its payload"),
                    );
                }
                let layout = own;
                let m = self.take_owned(f, value, "release")?;
                let mask = self.mask(f, live)?;
                let temp = self.temporary(layout)?;
                self.write_value(temp, layout, m)?;
                return Ok(self.drain(
                    f,
                    vec![
                        Work::Death {
                            alloc: temp.alloc,
                            raw: None,
                        },
                        Work::DropAt {
                            at: temp,
                            layout,
                            mask,
                        },
                    ],
                    After::Next,
                    Some(failure),
                ));
            }
            Inst::Reset {
                result,
                value,
                layout: _,
                live,
                anchor: _,
                failure,
            } => {
                let m = self.take_value(f, value)?;
                // A closure donates its environment block (LIR schema revision 5).
                let p = match m.val {
                    Val::Ptr(p) | Val::Callable { env: Some(p), .. } => p,
                    _ => return fail(V::IneligibleDonor, "reset of a non-block value"),
                };
                if !m.credited || !self.counted_target(p) {
                    return fail(
                        V::IneligibleDonor,
                        format!("reset of {:?} without an owned heap credit", p.alloc),
                    );
                }
                if matches!(self.ledger.check(p)?.count, Some(Count::Root(_))) {
                    return fail(
                        V::IneligibleDonor,
                        format!("reset of {:?}, a Shared root", p.alloc),
                    );
                }
                let mask = self.mask(f, live)?;
                let a = self.ledger.check(p)?.clone();
                self.ledger.counters.resets += 1;
                if a.count == Some(Count::Finite(1)) {
                    let token = TokenId(self.ledger.tokens.len() as u32);
                    self.ledger.tokens.push(Token {
                        alloc: p.alloc,
                        generation: a.generation,
                        consumed: false,
                        native: a.extent.native().into(),
                    });
                    let layout = a.layout.expect("live block has a layout");
                    let a = self.ledger.get_mut(p.alloc);
                    a.count = Some(Count::Finite(0));
                    a.state = AllocState::Dying;
                    return Ok(self.drain(
                        f,
                        vec![
                            Work::Death {
                                alloc: p.alloc,
                                raw: Some(token),
                            },
                            Work::DropAt {
                                at: Ptr { offset: 0, ..p },
                                layout,
                                mask,
                            },
                        ],
                        After::Reset { result, token },
                        Some(failure),
                    ));
                }
                let mut work = Vec::new();
                self.decrement(p, None, &mut work)?;
                debug_assert!(work.is_empty());
                self.ledger.trace.push(Event::Reset {
                    alloc: p.alloc,
                    token: None,
                });
                self.define_fresh(f, result, Val::Token(None))?;
            }
            Inst::Reuse {
                result,
                token,
                construct,
                kept,
                failure: _,
            } => {
                let m = self.take_value(f, token)?;
                let Val::Token(t) = m.val else {
                    return fail(V::TypeConfusion, format!("{token:?} is not a token"));
                };
                let site = self.construct_site(unit, construct)?;
                let layout = self.value_type(unit, result)?.layout;
                let (reuse, native) = match t {
                    None => (None, std::sync::Arc::<[Option<u8>]>::default()),
                    Some(t) => {
                        let native = self.ledger.tokens[t.0 as usize].native.clone();
                        (Some(self.install(t, site, layout)?), native)
                    }
                };
                let val = self.construct(f, site, reuse, layout)?;
                if let (Some(at), Some(kept)) = (reuse, kept) {
                    let kept = self.mask(f, MaskRef::Static(kept))?.unwrap_or_default();
                    self.check_kept(site, at, &kept, &native)?;
                }
                self.define_fresh(f, result, val)?;
            }
            Inst::Dispose { token } => {
                let m = self.take_value(f, token)?;
                let Val::Token(t) = m.val else {
                    return fail(V::TypeConfusion, format!("{token:?} is not a token"));
                };
                self.dispose(t)?;
            }
            Inst::TokenNone { result } => self.define_fresh(f, result, Val::Token(None))?,
            Inst::RegionEnter { result, region } => {
                if u.tables.regions.get(region.index()).is_none() {
                    return fail(V::Malformed, format!("unknown region {region:?}"));
                }
                let rt = self.ledger.regions.len();
                self.ledger.regions.push(crate::ledger::Region {
                    storage: crate::ledger::sealed::StorageCustody::Scope,
                    instance: region,
                    live: true,
                    allocations: Default::default(),
                    budget: None,
                });
                f.regions.push(rt);
                self.ledger.trace.push(Event::RegionEnter { region: rt });
                self.define_fresh(f, result, Val::Mark(rt))?;
            }
            Inst::RegionReserve {
                result,
                mark,
                layout,
                extent,
                failure: _,
            } => {
                let rt = self.region_of(f, mark)?;
                let size = match self.record(layout)?.size {
                    SizeSpec::Fixed(n) => n,
                    SizeSpec::Dynamic(_) => match self.read_use(f, extent)? {
                        Val::Bits { bits, .. } => bits as u64,
                        _ => return fail(V::TypeConfusion, "region extent is not an integer"),
                    },
                };
                let p = self.region_alloc(rt, layout, size)?;
                self.define_fresh(f, result, Val::Ptr(p))?;
            }
            Inst::ArenaToken {
                result,
                mark,
                budget,
            } => {
                let rt = self.region_of(f, mark)?;
                if let Some(budget) = budget {
                    let Val::Bits {
                        scalar: ScalarKind::U64,
                        bits,
                    } = self.read_use(f, budget)?
                    else {
                        return fail(V::TypeConfusion, "an arena budget is not a u64");
                    };
                    self.ledger.regions[rt].budget = Some(crate::ledger::Budget {
                        remaining: bits as u64,
                        start: self.failures.len(),
                        exhausted: None,
                    });
                }
                self.define_fresh(f, result, Val::Arena(Some(rt)))?;
            }
            Inst::ArenaOpen { .. } => {
                return fail(V::Malformed, "an arena opening after placement");
            }
            Inst::RegionPublish { value } => {
                let Val::Ptr(p) = self.read_use(f, value)? else {
                    return fail(V::TypeConfusion, "publish of a non-pointer");
                };
                let a = self.ledger.get_mut(p.alloc);
                if !matches!(a.owner, Owner::Region(_)) {
                    return fail(V::TypeConfusion, "publish of non-region storage");
                }
                a.published = true;
            }
            Inst::RegionExit {
                mark,
                region,
                failure,
            } => {
                let rt = self.region_of(f, mark)?;
                if self.ledger.regions[rt].instance != region {
                    return fail(V::Malformed, "region exit names another region");
                }
                if f.regions.last() != Some(&rt) {
                    return fail(V::Malformed, "a region exits before one entered inside it");
                }
                if self.ledger.regions[rt].storage != crate::ledger::sealed::StorageCustody::Scope {
                    self.exit_region(f, rt)?;
                    return Ok(Control::Continue);
                }
                let mut work = Vec::new();
                for alloc in self.ledger.regions[rt].allocations.clone() {
                    let a = self.ledger.get(alloc);
                    if let Some(layout) = a.layout
                        && self.arena_no_drop.get(alloc.0 as usize).is_none()
                        && a.extent.any_init(0, a.extent.len())
                    {
                        work.push(Work::DropAt {
                            at: Ptr {
                                alloc,
                                generation: a.generation,
                                offset: 0,
                                path: 0,
                            },
                            layout,
                            mask: None,
                        });
                    }
                }
                return Ok(self.drain(f, work, After::RegionExit { region: rt }, Some(failure)));
            }
            Inst::Defuse { address } => {
                let r = self.resolve_use(f, address)?;
                self.check_writable(&r)?;
                if self.record(r.layout)?.finalizer.is_none() {
                    return fail(V::Malformed, "defuse of a layout without a finalizer");
                }
                let extent = &self.ledger.check(r.ptr)?.extent;
                let prefix = self.ledger.path(r.ptr);
                let complete = self
                    .resource_walk(r.layout, Some(r.ptr))?
                    .iter()
                    .all(|relative| {
                        let mut path = prefix.clone();
                        path.extend(relative);
                        extent.resources.contains_key(&path)
                    });
                if !complete || !self.initialized(extent, r.ptr.offset, r.layout)?.0 {
                    return fail(V::UninitializedRead, "defuse of a partially moved resource");
                }
                let rid = self.resource_at(r.ptr)?;
                let res = &mut self.ledger.resources[rid.0 as usize];
                if res.state != ResourceState::Live {
                    return fail(
                        V::DuplicateFinalizer,
                        format!("defuse of {rid:?} while {:?}", res.state),
                    );
                }
                res.state = ResourceState::FieldsPending;
            }
            Inst::FinalizeBegin { address, anchor: _ } => {
                let r = self.resolve_use(f, address)?;
                let rid = self.resource_at(r.ptr)?;
                let res = &mut self.ledger.resources[rid.0 as usize];
                if res.state != ResourceState::Live {
                    return fail(
                        V::DuplicateFinalizer,
                        format!("finalizer of {rid:?} entered while it is {:?}", res.state),
                    );
                }
                res.state = ResourceState::Finalizing;
                f.finalizing += 1;
                self.ledger.counters.finalizers += 1;
                self.ledger.trace.push(Event::Finalize { resource: rid });
            }
            Inst::FinalizeEnd { address, anchor: _ } => {
                let r = self.resolve_use(f, address)?;
                let rid = self.resource_at(r.ptr)?;
                let res = &mut self.ledger.resources[rid.0 as usize];
                if res.state != ResourceState::Finalizing {
                    return fail(
                        V::Malformed,
                        format!("finalizer end of {rid:?} while {:?}", res.state),
                    );
                }
                res.state = ResourceState::FieldsPending;
                f.finalizing = f.finalizing.saturating_sub(1);
            }
            Inst::Forget { value } => {
                let m = self.take_owned(f, value, "forget")?;
                self.forget(m.val)?;
            }
            Inst::FrameEnter { failure } => {
                if self.injection_site() {
                    return self.fail_at(f, failure, true);
                }
                if self.admit_invocation(f).is_none() {
                    return self.fail_at(f, failure, false);
                }
                let anchor = self.unit(f.unit).tables.failures[failure.index()].anchor;
                let unit = self.unit(f.unit);
                let cleanup = (unit.tables.source_anchors[anchor.index()].block == u32::MAX)
                    .then(|| {
                        unit.blocks[f.block.index()]
                            .instructions
                            .get(&unit.instructions)
                            .unwrap_or(&[])
                            .iter()
                            .rev()
                            .find_map(|inst| match inst {
                                Inst::FinalizeBegin { address, .. } => Some(*address),
                                _ => None,
                            })
                    })
                    .flatten();
                let resource = cleanup
                    .map(|address| self.resolve_use(f, address).map(|at| (at.ptr, at.layout)))
                    .transpose()?;
                f.invocation_resources.push(resource);
                let sites = self.caller_sites(f, self.source_range(f, anchor), true);
                f.invocation_sites.push(sites);
                f.admitted += 1;
            }
            Inst::FrameLeave => {
                if f.admitted == 0 {
                    return fail(V::FrameImbalance, "frame leave without admission");
                }
                if let Some(Some((ptr, layout))) = f.invocation_resources.pop() {
                    let extent = &self.ledger.check(ptr)?.extent;
                    if self.owners(extent, ptr.offset, layout, self.ledger.path(ptr))? {
                        return fail(
                            V::FrameImbalance,
                            "cleanup invocation retires before its owned descendants",
                        );
                    }
                }
                f.admitted -= 1;
                if f.invocation_sites.pop().is_none() {
                    return fail(V::FrameImbalance, "invocation site retired without entry");
                }
                let task = self.tasks.as_ref().map_or(0, |tasks| tasks.current);
                if self
                    .invocation_events
                    .get_mut(&task)
                    .and_then(Vec::pop)
                    .is_none()
                {
                    return fail(
                        V::FrameImbalance,
                        "task invocation retirement without entry",
                    );
                }
            }
            Inst::Unavailable { feature, .. } => {
                return fail(V::Unsupported, format!("M2 operation {feature:?}"));
            }
        }
        Ok(Control::Continue)
    }

    fn drain(
        &mut self,
        f: &mut Frame,
        work: Vec<Work>,
        after: After,
        failure: Option<FailureId>,
    ) -> Control {
        let (caller, operation) = self.cleanup_sites(f, failure);
        f.resume = Resume::Drain { after, failure };
        Control::Push(Activity::Drain(Drain {
            caller,
            operation,
            work,
            failed: false,
        }))
    }

    fn symbol(&self, unit: usize, symbol: SymbolId) -> Checked<&'p Symbol> {
        self.unit(unit)
            .tables
            .symbols
            .get(symbol.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown symbol {symbol:?}")))
    }

    fn construct_site(&self, unit: usize, construct: ConstructId) -> Checked<&'p ConstructSite> {
        self.unit(unit)
            .tables
            .constructs
            .get(construct.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown construct {construct:?}")))
    }

    fn region_of(&mut self, f: &Frame, mark: ValueId) -> Checked<usize> {
        let Val::Mark(rt) = self.read_use(f, mark)? else {
            return fail(V::TypeConfusion, format!("{mark:?} is not a region mark"));
        };
        if !self.ledger.regions[rt].live || !f.regions.contains(&rt) {
            return fail(
                V::Malformed,
                "region mark of a region this frame does not hold",
            );
        }
        Ok(rt)
    }

    fn resource_at(&self, at: Ptr) -> Checked<ResourceId> {
        self.ledger
            .check(at)?
            .extent
            .resources
            .get(&self.ledger.path(at))
            .copied()
            .ok_or_else(|| {
                err(
                    V::Malformed,
                    format!("no resource at {:?}+{}", at.alloc, at.offset),
                )
            })
    }

    fn end_slot(&mut self, ptr: Ptr, slot: SlotId) -> Checked<()> {
        let a = self.ledger.check(ptr)?;
        if a.extent
            .cells
            .values()
            .any(|c| matches!(c, Cell::Data { counted: true, .. }))
        {
            return fail(
                V::LeakedSlot,
                format!("{slot:?} ended while it still held a holder"),
            );
        }
        for r in a.extent.resources.values() {
            let res = &self.ledger.resources[r.0 as usize];
            if res.state == ResourceState::Live && res.finalizer && !res.forgotten {
                return fail(
                    V::LeakedSlot,
                    format!("{slot:?} ended while it still held live {r:?}"),
                );
            }
        }
        self.ledger.free(ptr.alloc)
    }

    fn heap_alloc(&mut self, layout: LayoutId, size: u64, align: u32) -> Checked<Ptr> {
        let align = align.max(8);
        let capacity = match runtime_abi::size_class(size, u64::from(align)) {
            runtime_abi::SizeClass::Small { capacity, .. }
            | runtime_abi::SizeClass::Large { capacity } => capacity,
        };
        Ok(self.ledger.alloc(
            Owner::Heap,
            Some(layout),
            size,
            capacity,
            align,
            Some(Count::Finite(1)),
        ))
    }

    /// The immortal blocks of static `index`, allocated once with each
    /// reference cell naming its target block; block 0's handle.
    fn static_blocks(&mut self, index: usize) -> Checked<Ptr> {
        if let Some(p) = self.statics.get(&(usize::MAX, index as u32)) {
            return Ok(*p);
        }
        let data = self.program.statics[index].clone();
        let what = format!("static {:?}", data.symbol.definition);
        let mut blocks = Vec::with_capacity(data.blocks.len());
        for block in &data.blocks {
            blocks.push(self.static_alloc(block.layout, &block.bytes, what.clone())?);
        }
        for (block, at) in data.blocks.iter().zip(&blocks) {
            for r in &block.references {
                let Some(target) = blocks.get(r.target as usize) else {
                    return fail(V::Malformed, "a static reference to no block");
                };
                let cell = Cell::Data {
                    ptr: *target,
                    counted: false,
                    borrow: None,
                };
                let word = self.word;
                self.ledger
                    .get_mut(at.alloc)
                    .extent
                    .put_cell(r.offset, word, cell);
            }
        }
        for (block, at) in data.blocks.iter().zip(&blocks) {
            for function in &block.functions {
                let code = self.program.function(&function.symbol)?;
                self.ledger.get_mut(at.alloc).extent.put_cell(
                    function.offset,
                    self.word,
                    Cell::Code(Some(code)),
                );
            }
        }
        for (block, &at) in data.blocks.iter().zip(&blocks) {
            if let LayoutKind::SoaColumns { element } = self.record(block.layout)?.kind {
                self.clear_column_spare(at, element)?;
            }
        }
        let Some(root) = blocks.first().copied() else {
            return fail(V::Malformed, "static data without a block");
        };
        self.statics.insert((usize::MAX, index as u32), root);
        Ok(root)
    }

    fn static_alloc(&mut self, layout: LayoutId, bytes: &[u8], what: String) -> Checked<Ptr> {
        let align = self.record(layout)?.align;
        let size = bytes.len() as u64;
        let p = self.ledger.alloc(
            Owner::Static,
            Some(layout),
            size,
            size,
            align,
            Some(Count::Immortal),
        );
        self.ledger.get_mut(p.alloc).extent.write_bytes(0, bytes);
        self.ledger.get_mut(p.alloc).published = true;
        self.ledger.exemptions.push(Exemption::Static {
            alloc: p.alloc,
            what,
        });
        Ok(p)
    }

    fn static_bytes(
        &mut self,
        unit: usize,
        constant: u32,
        layout: LayoutId,
        bytes: &[u8],
        what: &str,
    ) -> Checked<Ptr> {
        if let Some(p) = self.constants.get(&(unit, constant)) {
            return Ok(*p);
        }
        let payload = match self.record(layout)?.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
            _ => layout,
        };
        // A String or Bytes block is a byte sequence payload: its u64 count and capacity, then
        // its bytes from their alignment, as native literals are laid out.
        let stored = match self.record(payload)?.kind {
            LayoutKind::Sequence { element } => self.byte_sequence(element, bytes)?.0,
            _ => bytes.to_vec(),
        };
        let p = self.static_alloc(payload, &stored, what.to_string())?;
        self.constants.insert((unit, constant), p);
        Ok(p)
    }

    /// Storage for a construction: heap, a live region, or the caller's destination.
    fn place(
        &mut self,
        f: &Frame,
        placement: Option<Placement>,
        payload: LayoutId,
    ) -> Checked<Ptr> {
        let size = self.size(payload)?;
        self.place_sized(f, placement, payload, size)
    }

    /// `place` for a payload of `size` bytes (a dynamic flat extent).
    fn place_sized(
        &mut self,
        f: &Frame,
        placement: Option<Placement>,
        payload: LayoutId,
        size: u64,
    ) -> Checked<Ptr> {
        let align = self.record(payload)?.align;
        let heap = match placement {
            None => {
                return fail(
                    V::Malformed,
                    "construction without placement in a post-RC stage",
                );
            }
            Some(Placement::Fixed(StorageOwner::Heap(_))) => true,
            Some(Placement::Fixed(StorageOwner::LocalRegion(r))) => {
                let rt = f
                    .regions
                    .iter()
                    .rev()
                    .copied()
                    .find(|rt| self.ledger.regions[*rt].instance == r)
                    .ok_or_else(|| err(V::Malformed, format!("{r:?} is not live in this frame")))?;
                return self.region_alloc(rt, payload, size);
            }
            Some(Placement::Fixed(StorageOwner::Static)) => {
                return fail(V::Unsupported, "run-time construction into static storage");
            }
            Some(Placement::Fixed(StorageOwner::Arena(arena))) => {
                if let Some(region) = self.arena_of(f, arena)? {
                    return self.arena_alloc(region, payload, size);
                }
                true
            }
            Some(Placement::Fixed(StorageOwner::Frozen(_))) => {
                return fail(V::Unsupported, "M2 Shared placement");
            }
            Some(Placement::CallerDestination) => match &f.destination {
                Some(DestinationClass::Heap | DestinationClass::Inferred) => true,
                Some(other) => {
                    return fail(V::Unsupported, format!("caller destination {other:?}"));
                }
                None => {
                    return fail(
                        V::Malformed,
                        "caller destination in a call that passed none",
                    );
                }
            },
        };
        debug_assert!(heap, "every other placement returned above");
        self.heap_alloc(payload, size, align)
    }

    /// The arena an arena placement names: the token in its instance's slot,
    /// which must be a checked token of an open arena.
    fn arena_of(&self, f: &Frame, arena: ArenaInstanceId) -> Checked<Option<usize>> {
        let u = self.unit(f.unit);
        let instance = u
            .tables
            .arenas
            .get(arena.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown arena {arena:?}")))?;
        let slot = instance.slot;
        let ptr = match f.slots.get(slot.index()) {
            Some(SlotStore::Own(p) | SlotStore::Alias(p)) => *p,
            _ => return fail(V::Malformed, "an arena token slot without storage"),
        };
        let ty = u
            .slots
            .get(slot.index())
            .and_then(|s| u.types.get(s.ty.index()))
            .ok_or_else(|| err(V::Malformed, "an arena token slot outside the unit"))?;
        let size = self.size(ty.layout)?;
        let e = self.ledger.check(ptr)?.extent.copy(ptr.offset, size, None);
        let (Val::Arena(region), _, _) = self.val_from(e, ty.repr, ty.layout)? else {
            return fail(
                V::TypeConfusion,
                "an arena placement whose slot holds no token",
            );
        };
        if let Some(region) = region {
            let scope =
                self.ledger.regions.get(region).ok_or_else(|| {
                    err(V::ForgedPointer, "arena placement names an unknown scope")
                })?;
            if !scope.live {
                return fail(V::UseAfterFree, "an arena placement in a closed arena");
            }
            if scope.storage != crate::ledger::sealed::StorageCustody::Scope {
                return fail(
                    V::CreditUnderflow,
                    "allocation through a consumed arena grant",
                );
            }
        }
        Ok(region)
    }

    /// An arena block: immortal, owned by its arena's region, which frees it at
    /// close after dropping the payloads it holds (MEM-4).
    /// The region of the bounded arena `placement` names, if it names one.
    fn bounded_region(&self, f: &Frame, placement: Option<Placement>) -> Checked<Option<usize>> {
        let Some(Placement::Fixed(StorageOwner::Arena(arena))) = placement else {
            return Ok(None);
        };
        Ok(self
            .arena_of(f, arena)?
            .filter(|&region| self.ledger.regions[region].budget.is_some()))
    }

    /// Take `bytes` from a bounded arena's budget; false when they exceed it.
    fn charge(&mut self, region: usize, bytes: u64) -> bool {
        let Some(budget) = &mut self.ledger.regions[region].budget else {
            return true;
        };
        match budget.remaining.checked_sub(bytes) {
            Some(remaining) => {
                budget.remaining = remaining;
                true
            }
            None => false,
        }
    }

    fn logical_count(&self, p: Ptr, layout: LayoutId) -> Checked<u64> {
        let count = self.read_scalar(Ptr { offset: 0, ..p }, 8)? as u64;
        let Some(entries) = self.map_storage.entries(layout) else {
            return Ok(count);
        };
        let mut live = 0u64;
        for i in 0..count {
            let at =
                compare::At::Heap(p.at(entries.first + i * entries.stride + entries.tag.offset));
            live += u64::from(self.tag_at(at, entries.tag.form)? == 1);
        }
        Ok(live)
    }

    /// The logical charge of a block of payload `payload` holding `count`
    /// elements (MEM-6, the runtime README's table).
    fn block_charge(&self, payload: LayoutId, count: u64) -> Checked<u64> {
        if let Some(entries) = self.map_storage.entries(payload) {
            return Ok(entries.logical.saturating_mul(count));
        }
        hirsel_lir::layout::block_charge(&self.program.layouts, payload)
            .map(|charge| charge.bytes(count))
            .ok_or_else(|| err(V::Unsupported, "a bounded-arena charge of a flat payload"))
    }

    /// A construction's charge: its block's, before any operand is consumed.
    fn construct_charge(&self, f: &Frame, site: &ConstructSite, result: LayoutId) -> Checked<u64> {
        if !matches!(
            self.record(result)?.kind,
            LayoutKind::Block { .. } | LayoutKind::Box { .. }
        ) {
            return Ok(0);
        }
        let u = self.unit(f.unit);
        let fields = self.pool(site.fields, &u.pools.fields)?;
        self.block_charge(site.layout, fields.len() as u64)
    }

    /// A closure environment's charge: its captures'.
    fn environment_charge(&self, site: &ClosureSite) -> Checked<u64> {
        if self.size(site.environment)? == 0 {
            return Ok(0);
        }
        let record = self.record(site.environment)?;
        let mut total = 0u64;
        for i in 0..site.captures.len {
            let field = record
                .fields
                .iter()
                .find(|fl| fl.ordinal == i)
                .ok_or_else(|| err(V::Malformed, "a capture outside its environment"))?;
            let size = hirsel_lir::layout::logical_size(&self.program.layouts, field.layout)
                .ok_or_else(|| err(V::Unsupported, "a bounded-arena charge of a flat capture"))?;
            total = total.saturating_add(size);
        }
        Ok(total)
    }

    fn copy_children(
        &self,
        image: &Extent,
        layout: LayoutId,
        root: bool,
    ) -> Checked<(Vec<Ptr>, u64)> {
        let slices = self.scan_cell_offsets_kind(image, layout, true, true)?;
        let mut bytes = 0u64;
        for offset in &slices {
            bytes = bytes.saturating_add(u64::from_le_bytes(
                image.read_bytes(offset + 16, 8).try_into().unwrap(),
            ));
        }
        let keyed = hirsel_lir::layout::keyed(&self.program.layouts, layout);
        let children = image
            .cells
            .iter()
            .filter(|(o, _)| (!keyed || **o == 0) && !slices.contains(o))
            .filter_map(|(_, c)| {
                if root {
                    self.block_target(c)
                } else {
                    self.holder_target(c)
                }
            })
            .collect();
        Ok((children, bytes))
    }

    /// A deep copy's charge: every block of the copied tree, once per
    /// reference to it, independently of sharing.
    fn copy_charge(&self, val: &Val, layout: LayoutId) -> Checked<u64> {
        let (roots, viewed): (Vec<Ptr>, u64) = match val {
            Val::Callable {
                env: Some(root), ..
            }
            | Val::Ptr(root) => (vec![*root], 0),
            Val::Image(image) => self.copy_children(image, layout, true)?,
            _ => (Vec::new(), 0),
        };
        let mut charges: BTreeMap<AllocId, u64> = BTreeMap::new();
        let mut work: Vec<(Ptr, bool)> = roots.iter().map(|&p| (p, false)).collect();
        while let Some((p, expanded)) = work.pop() {
            if charges.contains_key(&p.alloc) {
                continue;
            }
            let a = self.ledger.check(p)?;
            let (children, views) =
                self.copy_children(&a.extent, a.layout.expect("live"), false)?;
            if !expanded {
                work.push((p, true));
                work.extend(
                    children
                        .into_iter()
                        .filter(|c| !charges.contains_key(&c.alloc))
                        .map(|c| (c, false)),
                );
                continue;
            }
            let layout = a.layout.expect("live");
            let count = match self.record(layout)?.kind {
                LayoutKind::Sequence { .. }
                | LayoutKind::Sequential { .. }
                | LayoutKind::SoaColumns { .. } => self.logical_count(p, layout)?,
                _ => 1,
            };
            let mut total = self.block_charge(layout, count)?.saturating_add(views);
            for c in children {
                total = total.saturating_add(charges[&c.alloc]);
            }
            charges.insert(p.alloc, total);
        }
        Ok(roots
            .iter()
            .fold(viewed, |total, r| total.saturating_add(charges[&r.alloc])))
    }

    /// Whether no block below one of `layout` is tracked: a SortedMap's,
    /// Slots' or environment's (`LAYOUT_UNTRACKED_BELOW`, MEM-6).
    fn untracked_below(&self, layout: LayoutId) -> Checked<bool> {
        Ok(!hirsel_lir::layout::keyed(&self.program.layouts, layout)
            && matches!(
                self.record(layout)?.kind,
                LayoutKind::Opaque { .. } | LayoutKind::Environment { .. }
            ))
    }

    /// The arena of the tracked arena block `p` names, if it is one (MEM-6).
    fn tracked_region(&self, p: Ptr) -> Checked<Option<usize>> {
        let a = self.ledger.check(p)?;
        match (a.count, a.owner) {
            (Some(Count::Arena { .. }), Owner::Region(region)) => Ok(Some(region)),
            (Some(Count::Arena { .. }), owner) => fail(
                V::Malformed,
                format!("a tracked arena block {:?} owned by {owner:?}", p.alloc),
            ),
            _ => Ok(None),
        }
    }

    /// `hirsel_rt_v2_arena_push` (runtime ABI 11), or `hirsel_rt_v2_arena_reserve`
    /// of `count` elements (ABI 13), on the List block `block` the place `r`
    /// holds: a tracked arena block is charged `count` elements and, when
    /// shared, its count of them, then a shared block is copied and one
    /// without room for `count` more relocated within its arena to
    /// `max(4, 2 * len, len + count)` elements. Returns the arena when the
    /// charge exceeds its budget, nothing changed.
    fn arena_push(
        &mut self,
        r: &Resolved,
        block: Ptr,
        element: LayoutId,
        count: u64,
    ) -> Checked<Option<usize>> {
        let Some(region) = self.tracked_region(block)? else {
            return Ok(None);
        };
        let a = self.ledger.get(block.alloc);
        let (shared, layout) = (
            a.count == Some(Count::Arena { shared: true }),
            a.layout.expect("live"),
        );
        let len = u64::try_from(self.read_scalar(block, 8)?)
            .map_err(|_| err(V::Malformed, "a sequence count beyond u64"))?;
        let each = self.block_charge(layout, 1)?;
        let copied = if shared {
            each.saturating_mul(self.logical_count(block, layout)?)
        } else {
            0
        };
        if !self.charge(region, each.saturating_mul(count).saturating_add(copied)) {
            return Ok(Some(region));
        }
        // An `@soa` List's columns move with its capacity and a Packed List's
        // elements have no stride: a shared one is copied at its capacity, and
        // its growth regrows it in its arena.
        if matches!(
            self.record(layout)?.kind,
            LayoutKind::SoaColumns { .. } | LayoutKind::Sequential { .. }
        ) {
            if shared {
                let copy = self.detach_block(block, 0)?;
                let Cell::Data { counted, .. } = self.read_cell(r.ptr)? else {
                    return fail(V::TypeConfusion, "a sequence place holding a code word");
                };
                let word = self.word;
                self.ledger.get_mut(r.ptr.alloc).extent.put_cell(
                    r.ptr.offset,
                    word,
                    Cell::Data {
                        ptr: copy,
                        counted,
                        borrow: None,
                    },
                );
            }
            return Ok(None);
        }
        let wanted = len
            .checked_add(count)
            .ok_or_else(|| err(V::Malformed, "a sequence capacity beyond u64"))?;
        if shared || wanted > self.sequence_capacity_word(block, element)? {
            let slots = len
                .checked_mul(2)
                .ok_or_else(|| err(V::Malformed, "a sequence capacity beyond u64"))?
                .max(wanted)
                .max(4);
            self.tracked_move(r, block, element, region, slots, shared)?;
        }
        Ok(None)
    }

    /// Move the tracked arena List `block` at the place `r` into fresh storage
    /// of `region` holding `slots` elements, unshared. A shared block is
    /// copied, each child gaining a holder; an unshared one moves, its old
    /// storage abandoned empty, so the arena's exit drops nothing there.
    fn tracked_move(
        &mut self,
        r: &Resolved,
        block: Ptr,
        element: LayoutId,
        region: usize,
        slots: u64,
        shared: bool,
    ) -> Checked<()> {
        let (start, stride) = self.sequence_shape(element)?;
        let len = u64::try_from(self.read_scalar(block, 8)?)
            .map_err(|_| err(V::Malformed, "a sequence count beyond u64"))?;
        let used = start + len * stride;
        let size = slots
            .checked_mul(stride)
            .and_then(|n| n.checked_add(start))
            .ok_or_else(|| err(V::Malformed, "a sequence capacity beyond u64"))?;
        let a = self.ledger.check(block)?;
        if !a.extent.resources.is_empty() {
            return fail(V::TypeConfusion, "an arena push to a List of resources");
        }
        let (layout, held) = (a.layout.expect("live"), a.extent.len());
        let image = if shared {
            a.extent.copy(block.offset, used, None)
        } else {
            let whole = self
                .ledger
                .get_mut(block.alloc)
                .extent
                .take(0, held, FREE_POISON, None);
            whole.copy(block.offset, used, None)
        };
        let children: Vec<Ptr> = image
            .cells
            .values()
            .filter_map(|c| self.holder_target(c))
            .collect();
        let fresh = self.arena_alloc(region, layout, size)?;
        self.ledger.get_mut(fresh.alloc).count = Some(Count::Arena { shared: false });
        self.ledger.get_mut(fresh.alloc).extent.place(0, image);
        self.exact_capacity(fresh, element)?;
        if shared {
            for child in children {
                self.dup(child)?;
            }
        }
        let Cell::Data { counted, .. } = self.read_cell(r.ptr)? else {
            return fail(V::TypeConfusion, "a sequence place holding a code word");
        };
        let word = self.word;
        self.ledger.get_mut(r.ptr.alloc).extent.put_cell(
            r.ptr.offset,
            word,
            Cell::Data {
                ptr: fresh,
                counted,
                borrow: None,
            },
        );
        Ok(())
    }

    fn arena_alloc(&mut self, region: usize, layout: LayoutId, size: u64) -> Checked<Ptr> {
        let record = self.record(layout)?;
        let (flat, align) = (record.kind == LayoutKind::Flat, record.align);
        let capacity = if flat {
            hirsel_lir::layout::align_up(size, 16)
                .map_err(|_| err(V::OutOfExtent, "arena flat capacity overflow"))?
        } else {
            size
        };
        let block = self.ledger.alloc(
            Owner::Region(region),
            Some(layout),
            size,
            capacity,
            align,
            Some(Count::Immortal),
        );
        self.ledger.regions[region].allocations.push(block.alloc);
        Ok(block)
    }

    /// A flat region allocation has an immortal owner, but keeps the region's
    /// lifetime. Other region allocations are genuinely headerless.
    fn region_alloc(&mut self, region: usize, layout: LayoutId, size: u64) -> Checked<Ptr> {
        let record = self.record(layout)?;
        let (flat, align) = (record.kind == LayoutKind::Flat, record.align);
        let capacity = if flat {
            hirsel_lir::layout::align_up(size, 16)
                .map_err(|_| err(V::OutOfExtent, "regional flat capacity overflow"))?
        } else {
            size
        };
        let owner = self.ledger.alloc(
            Owner::Region(region),
            Some(layout),
            size,
            capacity,
            align,
            flat.then_some(Count::Immortal),
        );
        self.ledger.regions[region].allocations.push(owner.alloc);
        Ok(owner)
    }

    /// Build a construction's value, into `reuse` when a token supplied
    /// storage. A block result allocates its payload, whose fields the site
    /// writes (the site names the payload layout); any other result is an
    /// inline image of the site's layout.
    fn construct(
        &mut self,
        f: &mut Frame,
        site: &ConstructSite,
        reuse: Option<Ptr>,
        result: LayoutId,
    ) -> Checked<Val> {
        let record = self.record(result)?;
        let u = self.unit(f.unit);
        let fields = self.pool(site.fields, &u.pools.fields)?;
        if self.record(site.layout)?.kind == LayoutKind::Flat
            && site.constructor != ConstructorKind::Box
        {
            if reuse.is_some() {
                return fail(V::Unsupported, "reuse into a flat construction");
            }
            let block = matches!(
                record.kind,
                LayoutKind::Block { .. } | LayoutKind::Box { .. }
            );
            return self.construct_flat(f, site, fields, block);
        }
        match record.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                let target = match (reuse, &self.record(payload)?.kind) {
                    (Some(p), _) => p,
                    // A counted sequence: its u64 count, then its elements
                    // from their alignment.
                    (None, LayoutKind::SoaColumns { element }) => {
                        let shape = self.column_shape(*element)?;
                        let (data, quantum, width) =
                            (shape.data_offset, shape.quantum, shape.width);
                        let cap = hirsel_lir::layout::align_up(fields.len() as u64, quantum)
                            .map_err(|_| err(V::OutOfExtent, "column construction overflow"))?;
                        self.place_sized(f, site.placement, payload, data + cap * width)?
                    }
                    (None, LayoutKind::Sequence { element }) => {
                        let (start, stride) = self.sequence_shape(*element)?;
                        let size = start + fields.len() as u64 * stride;
                        self.place_sized(f, site.placement, payload, size)?
                    }
                    // A sequential sequence: its header, then each element at its
                    // active extent (MEM-13).
                    (None, LayoutKind::Sequential { element }) => {
                        let mut size = self.sequential_start(*element)?;
                        for fv in fields {
                            let Val::Image(image) = &self.get(f, fv.value)?.val else {
                                return fail(V::TypeConfusion, "a Packed element is no image");
                            };
                            let tag = image.active(0, TagForm::Ordinal, self.word)?;
                            size += self.walked_extent(*element, tag)?;
                        }
                        self.place_sized(f, site.placement, payload, size)?
                    }
                    (None, _) => self.place(f, site.placement, payload)?,
                };
                self.write_fields(f, target, payload, site.constructor, fields)?;
                if site.arena_no_drop {
                    self.arena_no_drop.set(target.alloc.0 as usize, ());
                }
                // An arena block is immutable once built (MEM-4), but a tracked
                // List or Array block stays writable while unshared (MEM-6).
                if matches!(
                    site.placement,
                    Some(Placement::Fixed(StorageOwner::Arena(_)))
                ) {
                    let bounded = matches!(self.ledger.get(target.alloc).owner,
                        Owner::Region(region) if self.ledger.regions[region].budget.is_some());
                    let a = self.ledger.get_mut(target.alloc);
                    if site.tracked
                        && reuse.is_none()
                        && bounded
                        && a.count == Some(Count::Immortal)
                    {
                        a.count = Some(Count::Arena { shared: false });
                    } else {
                        a.published = true;
                    }
                }
                Ok(Val::Ptr(target))
            }
            _ => {
                if reuse.is_some() {
                    return fail(V::IncompatibleReuse, "reuse into an inline construction");
                }
                let temp = self.temporary(site.layout)?;
                self.write_fields(f, temp, site.layout, site.constructor, fields)?;
                let size = self.size(site.layout)?;
                let image = self
                    .ledger
                    .get_mut(temp.alloc)
                    .extent
                    .take(0, size, FREE_POISON, None);
                self.ledger.free(temp.alloc)?;
                Ok(Val::Image(image))
            }
        }
    }

    fn write_fields(
        &mut self,
        f: &mut Frame,
        target: Ptr,
        layout: LayoutId,
        constructor: ConstructorKind,
        fields: &[FieldValue],
    ) -> Checked<()> {
        let record = self.record(layout)?;
        let layouts = match constructor {
            ConstructorKind::Variant { variant, .. } => {
                let tag = record.tag.as_ref().ok_or_else(|| {
                    err(V::Malformed, "variant construction of an untagged layout")
                })?;
                // A niche's dataful variant is tagged by its block reference field;
                // its fieldless variant by a null word.
                let bytes = match tag.form {
                    TagForm::Ordinal => variant.to_le_bytes().to_vec(),
                    TagForm::Niche { nullary, .. } if variant == nullary => {
                        vec![0; self.word as usize]
                    }
                    TagForm::Niche { .. } => Vec::new(),
                    TagForm::LiveBit { .. } => {
                        return fail(V::Malformed, "a live word is not a variant tag");
                    }
                };
                self.ledger
                    .get_mut(target.alloc)
                    .extent
                    .write_bytes(target.offset + tag.offset, &bytes);
                &tag.variants
                    .iter()
                    .find(|v| v.ordinal == variant)
                    .ok_or_else(|| err(V::Malformed, "unknown variant"))?
                    .fields
            }
            ConstructorKind::Box => {
                // `Box(v)`: the one operand is the whole payload.
                let [fv] = fields else {
                    return fail(V::Malformed, "a box construction takes exactly one operand");
                };
                let m = self.take_value(f, fv.value)?;
                return self.write_value(target, layout, m);
            }
            // A fixed-count sequence (Inline, or Simd lanes in ascending order):
            // element i at i times the element stride.
            ConstructorKind::Sequence
                if matches!(
                    record.kind,
                    LayoutKind::Inline { .. } | LayoutKind::Simd { .. }
                ) =>
            {
                let (LayoutKind::Inline { element, .. } | LayoutKind::Simd { element, .. }) =
                    record.kind
                else {
                    unreachable!()
                };
                let stride = self.size(element)?;
                for fv in fields {
                    let m = self.take_value(f, fv.value)?;
                    let at = self.ledger.project(
                        target,
                        u64::from(fv.field) * stride,
                        ProjectionStep::Index(u64::from(fv.field)),
                    );
                    self.write_value(at, element, m)?;
                }
                return Ok(());
            }
            ConstructorKind::Sequence if matches!(record.kind, LayoutKind::SoaColumns { .. }) => {
                let LayoutKind::SoaColumns { element } = record.kind else {
                    unreachable!()
                };
                let quantum = self.column_shape(element)?.quantum;
                let count = fields.len() as u64;
                let cap = hirsel_lir::layout::align_up(count, quantum)
                    .map_err(|_| err(V::OutOfExtent, "column construction overflow"))?;
                let extent = &mut self.ledger.get_mut(target.alloc).extent;
                extent.write_bytes(target.offset, &count.to_le_bytes());
                extent.write_bytes(target.offset + 8, &cap.to_le_bytes());
                for fv in fields {
                    let moved = self.take_value(f, fv.value)?;
                    let row = self.column_resolved(target, element, u64::from(fv.field))?;
                    self.write_resolved(&row, element, moved)?;
                }
                return Ok(());
            }
            ConstructorKind::Sequence if matches!(record.kind, LayoutKind::Sequence { .. }) => {
                let LayoutKind::Sequence { element } = record.kind else {
                    unreachable!()
                };
                let (start, stride) = self.sequence_shape(element)?;
                // The count, then the capacity: the count, never more than the slots.
                let count = (fields.len() as u64).to_le_bytes();
                let extent = &mut self.ledger.get_mut(target.alloc).extent;
                extent.write_bytes(target.offset, &count);
                extent.write_bytes(
                    target.offset + hirsel_lir::layout::SEQUENCE_CAPACITY_OFFSET,
                    &count,
                );
                for fv in fields {
                    let m = self.take_value(f, fv.value)?;
                    let at = self.ledger.project(
                        target,
                        start + u64::from(fv.field) * stride,
                        ProjectionStep::Index(u64::from(fv.field)),
                    );
                    self.write_value(at, element, m)?;
                }
                return Ok(());
            }
            ConstructorKind::Sequence if matches!(record.kind, LayoutKind::Sequential { .. }) => {
                let LayoutKind::Sequential { element } = record.kind else {
                    unreachable!()
                };
                let start = self.sequential_start(element)?;
                let mut ordered = fields.to_vec();
                ordered.sort_by_key(|fv| fv.field);
                let mut used = 0;
                for fv in ordered {
                    let m = self.take_value(f, fv.value)?;
                    let at = self.ledger.project(
                        target,
                        start + used,
                        ProjectionStep::Index(u64::from(fv.field)),
                    );
                    self.write_value(at, element, m)?;
                    used += self.bounded_size(at, element)?;
                }
                let extent = &mut self.ledger.get_mut(target.alloc).extent;
                extent.write_bytes(target.offset, &(fields.len() as u64).to_le_bytes());
                extent.write_bytes(target.offset + 8, &used.to_le_bytes());
                return Ok(());
            }
            ConstructorKind::Sequence => {
                return fail(
                    V::Unsupported,
                    format!("{constructor:?} construction of {:?}", record.kind),
                );
            }
            _ => &record.fields,
        };
        for fv in fields {
            let fl = layouts
                .iter()
                .find(|fl| fl.ordinal == fv.field)
                .ok_or_else(|| err(V::Malformed, format!("no field {} in {layout:?}", fv.field)))?;
            let borrowed = hirsel_lir::layout::field_is_borrowed(&self.program.layouts, record, fl);
            let m = if borrowed {
                Moved {
                    val: self.read_use(f, fv.value)?,
                    credited: false,
                    borrow: self.get(f, fv.value)?.borrow,
                }
            } else {
                self.take_value(f, fv.value)?
            };
            if let Val::Image(image) = &m.val {
                if !self.initialized(image, 0, fl.layout)?.0 {
                    return fail(V::UninitializedRead, "construction from an unwritten field");
                }
            }
            let step = match constructor {
                ConstructorKind::Variant { variant, .. } => ProjectionStep::VariantField {
                    variant,
                    field: fl.ordinal,
                },
                _ => ProjectionStep::Field(fl.ordinal),
            };
            let at = self.ledger.project(target, fl.offset, step);
            // A typed whole-byte Slice construct transfers a block handle into
            // its owner word. Store that handle with its actual owning layout;
            // the Slice scan then accounts for the same physical holder cell.
            let mut field_layout = fl.layout;
            if fv.field == 0
                && matches!(record.kind, LayoutKind::Slice { element }
                    if self.record(element)?.kind == LayoutKind::Scalar(ScalarKind::U8))
            {
                let unit = self.unit(f.unit);
                let actual = unit.types[unit.values[fv.value.index()].ty.index()].layout;
                if let LayoutKind::Block { payload } = self.record(actual)?.kind
                    && matches!(self.record(payload)?.kind, LayoutKind::Sequence { element }
                        if self.record(element)?.kind == LayoutKind::Scalar(ScalarKind::U8))
                {
                    field_layout = actual;
                }
            }
            self.write_value_as(at, field_layout, m, borrowed)?;
        }
        if matches!(record.kind, LayoutKind::Resource) {
            let rid = ResourceId(self.ledger.resources.len() as u32);
            self.ledger.resources.push(Resource {
                layout,
                finalizer: record.finalizer.is_some(),
                state: ResourceState::Live,
                forgotten: false,
            });
            let path = self.ledger.path(target);
            self.ledger
                .get_mut(target.alloc)
                .extent
                .resources
                .insert(path, rid);
        }
        Ok(())
    }

    fn closure(&mut self, f: &mut Frame, closure: ClosureId) -> Checked<Val> {
        let u = self.unit(f.unit);
        let site = u
            .tables
            .closures
            .get(closure.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown closure {closure:?}")))?;
        if site.receipts.is_some() {
            return fail(
                V::Unsupported,
                "receipt closure transport requires independent H cells",
            );
        }
        let code = match site.code {
            Some(c) => Some(self.program.function(self.symbol(f.unit, c.symbol)?)?),
            None => None,
        };
        if self.size(site.environment)? == 0 {
            return Ok(Val::Callable { code, env: None });
        }
        let env = self.place(f, site.placement, site.environment)?;
        let record = self.record(site.environment)?;
        for (i, capture) in self
            .pool(site.captures, &u.pools.captures)?
            .iter()
            .enumerate()
        {
            let fl = record
                .fields
                .iter()
                .find(|fl| fl.ordinal == i as u32)
                .ok_or_else(|| err(V::Malformed, format!("no environment field {i}")))?;
            let m = match capture.usage {
                CaptureUse::Take => self.take_value(f, capture.value)?,
                CaptureUse::Copy => {
                    let val = self.read_use(f, capture.value)?;
                    let (val, credited) = self.copy_value(val, fl.layout)?;
                    Moved {
                        val,
                        credited,
                        borrow: None,
                    }
                }
                CaptureUse::Borrow | CaptureUse::Mut => Moved {
                    val: self.read_use(f, capture.value)?,
                    credited: false,
                    borrow: Some(Borrow::Caller),
                },
            };
            let view = hirsel_lir::layout::field_is_borrowed(&self.program.layouts, record, fl);
            let at = self
                .ledger
                .project(env, fl.offset, ProjectionStep::Field(fl.ordinal));
            self.write_value_as(at, fl.layout, m, view)?;
        }
        if let LayoutKind::Environment { movable } = &record.kind {
            let field = record.fields.last().expect("live word");
            let bits = u64::MAX >> (64 - movable.len());
            let at = self
                .ledger
                .project(env, field.offset, ProjectionStep::Field(field.ordinal));
            self.write_value(
                at,
                field.layout,
                Moved {
                    val: Val::Bits {
                        scalar: ScalarKind::U64,
                        bits: u128::from(bits),
                    },
                    credited: false,
                    borrow: None,
                },
            )?;
        }
        Ok(Val::Callable {
            code,
            env: Some(env),
        })
    }

    /// Owning cells of an inline copy come from its layout, not the target's
    /// current counter. Immortal and regional targets still copy an owner word;
    /// uncounted scan cells remain views under their original permission.
    fn copy_owner_offsets(&self, image: &Extent, layout: LayoutId) -> Checked<Vec<u64>> {
        self.scan_cell_offsets(image, layout, true)
    }

    /// Physical cells selected by the active scan, either owners or views.
    fn scan_cell_offsets(
        &self,
        image: &Extent,
        layout: LayoutId,
        owning: bool,
    ) -> Checked<Vec<u64>> {
        self.scan_cell_offsets_kind(image, layout, owning, false)
    }

    fn scan_cell_offsets_kind(
        &self,
        image: &Extent,
        layout: LayoutId,
        owning: bool,
        slices: bool,
    ) -> Checked<Vec<u64>> {
        use hirsel_lir::layout_types::ScanStep;
        let mut work: Vec<_> = self
            .record(layout)?
            .scan
            .iter()
            .cloned()
            .map(|s| (0, s))
            .collect();
        let mut offsets = Vec::new();
        while let Some((base, step)) = work.pop() {
            match step {
                ScanStep::Counted { offset }
                | ScanStep::Root { offset }
                | ScanStep::Environment { offset } => {
                    if owning && !slices {
                        offsets.push(base + offset);
                    }
                }
                ScanStep::SliceOwner { offset } => {
                    if owning {
                        offsets.push(base + offset);
                    }
                }
                ScanStep::Uncounted { offset } => {
                    if !owning && !slices {
                        offsets.push(base + offset);
                    }
                }
                ScanStep::Handle { .. } => {}
                ScanStep::Inline { offset, layout } => {
                    work.extend(
                        self.record(layout)?
                            .scan
                            .iter()
                            .cloned()
                            .map(|s| (base + offset, s)),
                    );
                }
                ScanStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let active = image.active(base + tag_offset, form, self.word)?;
                    // A variant without holders has no scan arm. Its tag
                    // remains checked by the ordinary value read.
                    if let Some((_, steps)) = variants.iter().find(|(v, _)| *v == active) {
                        work.extend(steps.iter().cloned().map(|s| (base, s)));
                    }
                }
                ScanStep::Repeat {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    self.copy_scan_elements(
                        image,
                        &mut work,
                        base + offset,
                        count,
                        stride,
                        element,
                    )?;
                }
                ScanStep::Sequence {
                    count_offset,
                    offset,
                    stride,
                    element,
                } => {
                    let at = base + count_offset;
                    image.check_range(at, 8)?;
                    if !image.all_init(at, 8) {
                        return fail(V::UninitializedRead, "copy scan has an unwritten count");
                    }
                    let count = u64::from_le_bytes(image.read_bytes(at, 8).try_into().unwrap());
                    self.copy_scan_elements(
                        image,
                        &mut work,
                        base + offset,
                        count,
                        stride,
                        element,
                    )?;
                }
                ScanStep::Columns(columns) => {
                    let word = |offset| -> Checked<u64> {
                        let at = base + offset;
                        image.check_range(at, 8)?;
                        if !image.all_init(at, 8) {
                            return fail(
                                V::UninitializedRead,
                                "column scan has an unwritten header",
                            );
                        }
                        Ok(u64::from_le_bytes(
                            image.read_bytes(at, 8).try_into().unwrap(),
                        ))
                    };
                    let count = word(columns.count_offset)?;
                    let capacity = word(columns.capacity_offset)?;
                    if count > capacity || capacity % columns.capacity_quantum != 0 {
                        return fail(V::OutOfExtent, "invalid column scan capacity");
                    }
                    if let Some(tag) = columns.tag {
                        // Each row's payload through the arm its tag names, and no other.
                        let width = soa_tag_width(tag.scalar);
                        let at = |prefix: u64, i: u64, stride: u64| {
                            capacity
                                .checked_mul(prefix)
                                .and_then(|n| n.checked_add(columns.data_offset + base))
                                .and_then(|n| i.checked_mul(stride).and_then(|j| n.checked_add(j)))
                                .ok_or_else(|| err(V::OutOfExtent, "column scan offset overflow"))
                        };
                        for i in 0..count {
                            let tag_at = at(tag.prefix, i, width)?;
                            image.check_range(tag_at, width)?;
                            if !image.all_init(tag_at, width) {
                                return fail(
                                    V::UninitializedRead,
                                    "column scan has an unwritten tag",
                                );
                            }
                            let mut ordinal = [0u8; 8];
                            ordinal[..width as usize]
                                .copy_from_slice(&image.read_bytes(tag_at, width));
                            let ordinal = u64::from_le_bytes(ordinal);
                            for column in &columns.columns {
                                let Some(arm) = column.arm else {
                                    return fail(V::Malformed, "a tagged column without an arm");
                                };
                                if u64::from(arm) == ordinal {
                                    let start = at(column.prefix, i, column.stride)?;
                                    self.copy_scan_elements(
                                        image,
                                        &mut work,
                                        start,
                                        1,
                                        column.stride,
                                        column.layout,
                                    )?;
                                }
                            }
                        }
                        continue;
                    }
                    if columns.columns.iter().any(|c| c.arm.is_some()) {
                        return fail(V::Malformed, "an arm column without a tag column");
                    }
                    for column in columns.columns {
                        let start = capacity
                            .checked_mul(column.prefix)
                            .and_then(|n| n.checked_add(columns.data_offset))
                            .and_then(|n| n.checked_add(base))
                            .ok_or_else(|| err(V::OutOfExtent, "column scan offset overflow"))?;
                        self.copy_scan_elements(
                            image,
                            &mut work,
                            start,
                            count,
                            column.stride,
                            column.layout,
                        )?;
                    }
                }
            }
        }
        Ok(offsets)
    }

    /// The bytes a value of `layout` at `at` occupies: a dynamic Packed value its
    /// active variant's extent, which may end a sequential sequence (MEM-13).
    fn bounded_size(&self, at: Ptr, layout: LayoutId) -> Checked<u64> {
        if hirsel_lir::layout::packed_extents(self.record(layout)?).is_none() {
            return self.size(layout);
        }
        let tag = self.active_variant(at, TagForm::Ordinal)?;
        self.walked_extent(layout, tag)
    }

    /// A value image read through `bounded_size`, widened to the storage of its
    /// layout with its inactive bytes uninitialized.
    fn widened(&self, image: Extent, layout: LayoutId) -> Checked<Extent> {
        let size = self.size(layout)?;
        if image.len() >= size {
            return Ok(image);
        }
        let mut full = Extent::new(size);
        full.place(0, image);
        Ok(full)
    }

    /// The extent of a walked Packed element (MEM-13) whose tag is `tag`.
    fn walked_extent(&self, element: LayoutId, tag: u32) -> Checked<u64> {
        hirsel_lir::layout::packed_extents(self.record(element)?)
            .and_then(|extents| extents.into_iter().find(|(o, _)| *o == tag))
            .map(|(_, bytes)| bytes)
            .ok_or_else(|| err(V::TypeConfusion, "a walked element's tag has no extent"))
    }

    fn copy_scan_elements(
        &self,
        image: &Extent,
        work: &mut Vec<(u64, hirsel_lir::layout_types::ScanStep)>,
        start: u64,
        count: u64,
        stride: u64,
        element: LayoutId,
    ) -> Checked<()> {
        let steps = &self.record(element)?.scan;
        if count == 0 || steps.is_empty() {
            return Ok(());
        }
        let element_size = self.size(element)?;
        let span = (count - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(element_size))
            .ok_or_else(|| err(V::OutOfExtent, "copy scan extent overflow"))?;
        image.check_range(start, span)?;
        for index in 0..count {
            work.extend(steps.iter().cloned().map(|s| (start + index * stride, s)));
        }
        Ok(())
    }

    /// A semantic Copy by the layout's recipe; returns whether it made a credit.
    fn copy_value(&mut self, val: Val, layout: LayoutId) -> Checked<(Val, bool)> {
        use hirsel_lir::layout_types::CopyRecipe as C;
        match self.record(layout)?.copy {
            C::Bits => Ok((val, false)),
            C::Dup if matches!(val, Val::Callable { env: None, .. }) => {
                // FunctionRef has a static empty environment: copying its code
                // and absent environment adds no heap holder.
                Ok((val, false))
            }
            C::Dup if !matches!(val, Val::Image(_)) => {
                let (Val::Ptr(p) | Val::Callable { env: Some(p), .. }) = &val else {
                    return fail(V::TypeConfusion, "dup copy of a non-block value");
                };
                self.dup(*p)?;
                Ok((val, true))
            }
            // A slice is its owner word plus bounds: its one counted cell is
            // duplicated as a field copy duplicates each of its cells.
            C::Dup | C::Fields => {
                let Val::Image(mut e) = val else {
                    return fail(V::TypeConfusion, "field copy of a non-aggregate value");
                };
                if !e.resources.is_empty() {
                    return fail(V::TypeConfusion, "copy of a resource");
                }
                let owners = self.copy_owner_offsets(&e, layout)?;
                // An owning scan cell needs a counted allocation even when
                // its current count is immortal. A regional view cannot turn
                // into an owning copy: native copying would touch a header
                // that its allocation does not have.
                for offset in &owners {
                    if let Some(Cell::Data { ptr, .. }) = e.cells.get(offset)
                        && self.ledger.check(*ptr)?.count.is_none()
                    {
                        return fail(V::TypeConfusion, "copy of an uncounted owner");
                    }
                }
                let targets: Vec<Ptr> = owners
                    .iter()
                    .filter_map(|offset| match e.cells.get(offset) {
                        Some(Cell::Data { ptr, .. }) if self.counted_target(*ptr) => Some(*ptr),
                        _ => None,
                    })
                    .collect();
                for p in targets {
                    self.dup(p)?;
                }
                for offset in owners {
                    if let Some(Cell::Data {
                        ptr,
                        counted,
                        borrow,
                    }) = e.cells.get_mut(&offset)
                    {
                        *counted = self.counted_target(*ptr);
                        *borrow = None;
                    }
                }
                Ok((Val::Image(e), true))
            }
            C::FlatBytes => match val {
                Val::Image(e) if e.cells.is_empty() => Ok((Val::Image(e), true)),
                // A variable flat value: its payload extent into a new block.
                Val::Ptr(p) => {
                    let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) =
                        self.record(layout)?.kind
                    else {
                        return fail(V::TypeConfusion, "flat copy of a pointer to inline bytes");
                    };
                    let size = self.extent_of(payload, p)?;
                    let e = self
                        .ledger
                        .check(p)?
                        .extent
                        .copy(p.offset, size, Some((&[], &[])));
                    if !e.cells.is_empty() || !self.initialized(&e, 0, payload)?.0 {
                        return fail(V::TypeConfusion, "a flat extent holds a handle");
                    }
                    let align = self.record(payload)?.align;
                    let copy = self.heap_alloc(payload, size, align)?;
                    self.ledger.get_mut(copy.alloc).extent.place(0, e);
                    Ok((Val::Ptr(copy), true))
                }
                _ => fail(V::Unsupported, "flat copy of a block through semantic Copy"),
            },
            C::NotCopyable => fail(V::TypeConfusion, format!("copy of noncopyable {layout:?}")),
        }
    }

    /// A copy-on-write detach (OWN-4): a heap copy of the block `val` names
    /// (a static one included), each holder cell given a credit and its
    /// target one more holder, so the copy shares every child with the
    /// original.
    fn detach(&mut self, val: Val) -> Checked<Val> {
        let Val::Ptr(root) = val else {
            return fail(V::Unsupported, "a detach of a non-block value");
        };
        Ok(Val::Ptr(self.detach_block(root, 0)?))
    }

    /// The detach of `root`'s block into a fresh heap block of at least
    /// `size` bytes (a reserve's), its children sharing one more holder. A
    /// tracked arena block is copied within its arena, unshared (MEM-6); the
    /// caller charged it.
    fn detach_block(&mut self, root: Ptr, size: u64) -> Checked<Ptr> {
        let region = self.tracked_region(root)?;
        let a = self.ledger.check(root)?;
        if !a.extent.resources.is_empty() {
            return fail(V::TypeConfusion, "a detach of a resource");
        }
        // Sequence writes can initialize spare capacity after allocation.
        // Preserve its bytes and initialization, not just the original request.
        let (layout, held, align) = (a.layout.expect("live"), a.extent.len(), a.align);
        let mut image = a.extent.copy(0, held, None);
        let children: Vec<(u64, Ptr)> = image
            .cells
            .iter()
            .filter_map(|(o, c)| self.holder_target(c).map(|p| (*o, p)))
            .collect();
        for &(offset, ptr) in &children {
            image.cells.insert(
                offset,
                Cell::Data {
                    ptr,
                    counted: true,
                    borrow: None,
                },
            );
        }
        let copy = match region {
            Some(region) => {
                let copy = self.arena_alloc(region, layout, held.max(size))?;
                self.ledger.get_mut(copy.alloc).count = Some(Count::Arena { shared: false });
                copy
            }
            None => self.heap_alloc(layout, held.max(size), align)?,
        };
        self.ledger.get_mut(copy.alloc).extent.place(0, image);
        for (_, child) in children {
            self.dup(child)?;
        }
        Ok(Ptr {
            offset: root.offset,
            ..copy
        })
    }

    /// The headered heap, static or arena block a holder cell names: a deep copy copies it.
    /// A static block's cells hold its children without a credit, so any
    /// non-borrowed data cell is a holder.
    fn holder_target(&self, cell: &Cell) -> Option<Ptr> {
        match cell {
            Cell::Data { borrow: None, .. } => self.block_target(cell),
            _ => None,
        }
    }

    /// The payload layout of `Shared` root layout `root` and the frozen value's layout in it.
    fn frozen_layouts(&self, root: LayoutId) -> Checked<(LayoutId, LayoutId)> {
        if let LayoutKind::Shared { payload } = self.record(root)?.kind
            && let LayoutKind::Frozen { value } = self.record(payload)?.kind
        {
            return Ok((payload, value));
        }
        fail(V::TypeConfusion, format!("{root:?} is not a Shared root"))
    }

    /// Set every counted heap block of the graph frozen in root block `root`
    /// from `from` to `to`, once each, stopping at static and region blocks and
    /// at nested roots, as the runtime's freeze and thaw walks do (MEM-11). The
    /// graph is a tree, frozen from a unique value: a block found at another
    /// count is either shared or written through a root.
    fn refreeze(&mut self, root: AllocId, from: Count, to: Count) -> Checked<()> {
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            let targets: Vec<Ptr> = (self.ledger.get(id).extent.cells.values())
                .filter_map(|c| self.holder_target(c))
                .collect();
            for p in targets {
                let a = self.ledger.check(p)?;
                if a.owner != Owner::Heap || matches!(a.count, Some(Count::Root(_))) {
                    continue;
                }
                if let (Count::Frozen { .. }, Some(Count::Frozen { copies })) = (from, a.count)
                    && copies > 0
                {
                    return fail(
                        V::WriteShared,
                        format!(
                            "{copies} copies of {:?} in the frozen graph of {root:?} outlive its last root",
                            p.alloc
                        ),
                    );
                }
                if a.count != Some(from) {
                    return fail(
                        V::WriteShared,
                        format!(
                            "{:?} in the frozen graph of {root:?} has count {:?}, not {from:?}",
                            p.alloc, a.count
                        ),
                    );
                }
                self.ledger.get_mut(p.alloc).count = Some(to);
                pending.push(p.alloc);
            }
        }
        Ok(())
    }

    /// The headered block a data cell names, borrowed or not: the
    /// cells of a value read through a loan are its holders' views.
    fn block_target(&self, cell: &Cell) -> Option<Ptr> {
        let Cell::Data { ptr, .. } = cell else {
            return None;
        };
        self.ledger
            .allocations
            .get(ptr.alloc.0 as usize)
            .filter(|a| {
                matches!(a.owner, Owner::Heap | Owner::Static | Owner::Region(_))
                    && a.count.is_some()
            })
            .map(|_| *ptr)
    }

    /// A partial String/Bytes view owns a fresh byte sequence containing exactly its
    /// interval. Each holder owns its copy, even for equal intervals.
    fn copy_view_intervals(
        &mut self,
        f: &Frame,
        destination: (Option<Placement>, bool),
        layout: LayoutId,
        image: &mut Extent,
    ) -> Checked<std::collections::BTreeSet<u64>> {
        let mut copied = std::collections::BTreeSet::new();
        for offset in self.scan_cell_offsets_kind(image, layout, true, true)? {
            let Some(old) = image.cells.get(&offset).and_then(|c| self.block_target(c)) else {
                return fail(V::ForgedPointer, "slice copy has no owner");
            };
            let start = u64::from_le_bytes(image.read_bytes(offset + 8, 8).try_into().unwrap());
            let length = u64::from_le_bytes(image.read_bytes(offset + 16, 8).try_into().unwrap());
            let a = self.ledger.check(old)?;
            let payload = a.layout.expect("live");
            if matches!(self.record(payload)?.kind, LayoutKind::Sequence { element }
                if self.record(element)?.kind == LayoutKind::Scalar(ScalarKind::U8))
                && start == 8
                && self.logical_count(old, payload)? == length
            {
                continue;
            }
            let copy = {
                let at = old
                    .offset
                    .checked_add(8)
                    .and_then(|n| n.checked_add(start))
                    .ok_or_else(|| err(V::OutOfExtent, "slice copy interval overflow"))?;
                a.extent.check_range(at, length)?;
                if !a.extent.all_init(at, length) {
                    return fail(V::UninitializedRead, "slice copy of unwritten bytes");
                }
                let bytes = a.extent.read_bytes(at, length).to_vec();
                let (payload, element) = self
                    .program
                    .layouts
                    .layouts
                    .iter()
                    .enumerate()
                    .find_map(|(i, r)| match r.kind {
                        LayoutKind::Sequence { element }
                            if self.record(element).ok()?.kind
                                == LayoutKind::Scalar(ScalarKind::U8) =>
                        {
                            Some((LayoutId(i as u32), element))
                        }
                        _ => None,
                    })
                    .ok_or_else(|| err(V::Malformed, "slice copy has no byte sequence layout"))?;
                let (stored, _) = self.byte_sequence(element, &bytes)?;
                let copy = self.place_sized(f, destination.0, payload, stored.len() as u64)?;
                if destination.1 {
                    self.ledger.get_mut(copy.alloc).count = Some(Count::Arena { shared: false });
                }
                self.ledger
                    .get_mut(copy.alloc)
                    .extent
                    .write_bytes(0, &stored);
                copy
            };
            image.put_cell(
                offset,
                self.word,
                Cell::Data {
                    ptr: copy,
                    counted: true,
                    borrow: None,
                },
            );
            image.write_bytes(offset + 8, &8u64.to_le_bytes());
            copied.insert(offset);
        }
        Ok(copied)
    }

    /// Copy one ordinary block per incoming holder, stopping at Shared roots.
    /// The pending images keep this independent ledger walk iterative (MEM-3).
    fn copy_tree_block(
        &mut self,
        f: &Frame,
        destination: (Option<Placement>, bool, Option<bool>),
        p: Ptr,
        pending: &mut Vec<(Ptr, bool)>,
    ) -> Checked<Ptr> {
        let (placement, below, tracked) = destination;
        let a = self.ledger.check(p)?;
        if matches!(a.count, Some(Count::Root(_))) {
            self.dup(p)?;
            return Ok(p);
        }
        if !a.extent.resources.is_empty() {
            return fail(V::TypeConfusion, "unique copy of a resource");
        }
        let (layout, size) = (a.layout.expect("live"), a.extent.len());
        let image = a.extent.copy(0, size, None);
        let sequence = matches!(
            self.record(layout)?.kind,
            LayoutKind::Sequence { .. }
                | LayoutKind::SoaColumns { .. }
                | LayoutKind::Sequential { .. }
        ) || hirsel_lir::layout::keyed(&self.program.layouts, layout);
        let copy = self.place_sized(f, placement, layout, size)?;
        self.ledger.get_mut(copy.alloc).extent.place(0, image);
        if tracked.unwrap_or(below && sequence) {
            let a = self.ledger.get_mut(copy.alloc);
            if a.count != Some(Count::Immortal) || !matches!(a.owner, Owner::Region(_)) {
                return fail(V::Malformed, "a tracked copy outside an arena");
            }
            a.count = Some(Count::Arena { shared: false });
        }
        pending.push((copy, below && !self.untracked_below(layout)?));
        Ok(Ptr {
            offset: p.offset,
            ..copy
        })
    }

    fn copy_tree_image(
        &mut self,
        f: &Frame,
        destination: (Option<Placement>, bool),
        layout: LayoutId,
        image: &mut Extent,
        pending: &mut Vec<(Ptr, bool)>,
    ) -> Checked<()> {
        let (placement, below) = destination;
        let intervals = self.copy_view_intervals(f, destination, layout, image)?;
        let keyed = hirsel_lir::layout::keyed(&self.program.layouts, layout);
        for offset in self.copy_owner_offsets(image, layout)? {
            if intervals.contains(&offset) {
                continue;
            }
            let Some(old) = image.cells.get(&offset).and_then(|c| self.block_target(c)) else {
                return fail(V::ForgedPointer, "unique copy has no owner");
            };
            let tracks = below && (!keyed || offset == 0);
            let new = self.copy_tree_block(f, (placement, tracks, None), old, pending)?;
            image.set_cell(
                offset,
                Cell::Data {
                    ptr: new,
                    counted: true,
                    borrow: None,
                },
            );
        }
        Ok(())
    }

    /// A unique result exclusively owns each ordinary counted block (MODE-2).
    /// Copy source diamonds per holder; preserve ordinary copy, shallow detach,
    /// nested roots, destination tracking and the independent precharge walk.
    fn deep_copy(
        &mut self,
        f: &Frame,
        placement: Option<Placement>,
        tracked: bool,
        layout: LayoutId,
        val: Val,
    ) -> Checked<Val> {
        let arena = match placement {
            Some(Placement::Fixed(StorageOwner::Arena(arena))) => self
                .arena_of(f, arena)?
                .is_some_and(|region| self.ledger.regions[region].budget.is_some()),
            _ => false,
        };
        let mut pending = Vec::new();
        let result = match val {
            Val::Ptr(root) => Val::Ptr(self.copy_tree_block(
                f,
                (placement, arena, Some(tracked && arena)),
                root,
                &mut pending,
            )?),
            Val::Callable {
                code,
                env: Some(root),
            } => Val::Callable {
                code,
                env: Some(self.copy_tree_block(
                    f,
                    (placement, arena, Some(tracked && arena)),
                    root,
                    &mut pending,
                )?),
            },
            Val::Image(mut image) => {
                let below = arena && !self.untracked_below(layout)?;
                self.copy_tree_image(f, (placement, below), layout, &mut image, &mut pending)?;
                Val::Image(image)
            }
            Val::Unit
            | Val::Bits { .. }
            | Val::Handle(_)
            | Val::Arena(_)
            | Val::Callable { env: None, .. } => return Ok(val),
            _ => return fail(V::Unsupported, "unique copy of a non-block value"),
        };
        while let Some((copy, below)) = pending.pop() {
            let a = self.ledger.get(copy.alloc);
            let (layout, mut image) = (
                a.layout.expect("live"),
                a.extent.copy(0, a.extent.len(), None),
            );
            self.copy_tree_image(f, (placement, below), layout, &mut image, &mut pending)?;
            self.ledger.get_mut(copy.alloc).extent.place(0, image);
        }
        Ok(result)
    }

    /// Each kept field of a reuse, as the construction wrote it at `at`, must
    /// be exactly the bits native memory already held there when the donor
    /// was reset: skipping its store is then unobservable.
    fn check_kept(
        &self,
        site: &ConstructSite,
        at: Ptr,
        kept: &[bool],
        native: &[Option<u8>],
    ) -> Checked<()> {
        let record = self.record(site.layout)?;
        let fields = match site.constructor {
            ConstructorKind::Variant { variant, .. } => record
                .tag
                .iter()
                .flat_map(|t| &t.variants)
                .find(|v| v.ordinal == variant)
                .map(|v| v.fields.clone())
                .unwrap_or_default(),
            ConstructorKind::Box => {
                return fail(V::SkippedStore, "a kept field of a box construction");
            }
            _ => record.fields.clone(),
        };
        for (ordinal, _) in kept.iter().enumerate().filter(|(_, k)| **k) {
            let Some(field) = fields.iter().find(|x| x.ordinal as usize == ordinal) else {
                return fail(
                    V::SkippedStore,
                    format!("kept field {ordinal} is not a field"),
                );
            };
            let size = self.size(field.layout)?;
            let a = self.ledger.check(at)?;
            let written = a.extent.read_bytes(at.offset + field.offset, size);
            let held = native.get(field.offset as usize..(field.offset + size) as usize);
            let same =
                held.is_some_and(|held| held.iter().zip(written).all(|(n, w)| *n == Some(*w)));
            if !same {
                return fail(
                    V::SkippedStore,
                    format!("kept field {ordinal} does not already hold its operand"),
                );
            }
        }
        Ok(())
    }

    /// Install a construction into the raw storage a token owns.
    fn install(&mut self, t: TokenId, site: &ConstructSite, result: LayoutId) -> Checked<Ptr> {
        let (LayoutKind::Block { payload } | LayoutKind::Box { payload }) =
            self.record(result)?.kind
        else {
            return fail(V::IncompatibleReuse, "reuse into an inline construction");
        };
        let heap = matches!(
            site.placement,
            Some(Placement::Fixed(StorageOwner::Heap(_)))
        );
        self.install_raw(t, payload, heap)
    }

    /// Consume token `t` and install a fresh `payload` value in the raw storage
    /// it owns: the exact capacity class, alignment and heap domain, at the
    /// next generation. Nothing changes when a check fails.
    pub(crate) fn install_raw(
        &mut self,
        t: TokenId,
        payload: LayoutId,
        heap: bool,
    ) -> Checked<Ptr> {
        let tk = self.ledger.tokens[t.0 as usize].clone();
        if tk.consumed {
            return fail(
                V::TokenReused,
                format!("{t:?} reused after it was consumed"),
            );
        }
        let a = self.ledger.get(tk.alloc);
        if a.state != AllocState::Raw(t) || a.generation != tk.generation {
            return fail(
                V::StaleReuseGeneration,
                format!(
                    "{t:?} names {:?} generation {}, which it no longer owns",
                    tk.alloc, tk.generation
                ),
            );
        }
        let size = self.size(payload)?;
        let align = self.record(payload)?.align.max(8);
        let capacity = match runtime_abi::size_class(size, u64::from(align)) {
            runtime_abi::SizeClass::Small { capacity, .. }
            | runtime_abi::SizeClass::Large { capacity } => capacity,
        };
        if capacity != a.capacity || align != a.align || !heap {
            return fail(
                V::IncompatibleReuse,
                format!(
                    "token of capacity {} align {} installed as capacity {capacity} align {align}",
                    a.capacity, a.align
                ),
            );
        }
        self.ledger.tokens[t.0 as usize].consumed = true;
        let a = self.ledger.get_mut(tk.alloc);
        a.generation += 1;
        a.state = AllocState::Live;
        a.count = Some(Count::Finite(1));
        a.layout = Some(payload);
        a.size = size;
        let edits = a.extent.holder_edits.take();
        a.extent = Extent::allocation(a.capacity);
        a.extent.holder_edits = edits;
        let generation = a.generation;
        self.ledger.counters.reuses += 1;
        self.ledger.trace.push(Event::Reuse {
            alloc: tk.alloc,
            generation,
        });
        Ok(Ptr {
            alloc: tk.alloc,
            generation,
            offset: 0,
            path: 0,
        })
    }

    /// Keep a value and everything it owns alive under a named forget record.
    fn forget(&mut self, val: Val) -> Checked<()> {
        let mut allocations = Vec::new();
        let mut resources: Vec<ResourceId> = Vec::new();
        let mut pending: Vec<Ptr> = Vec::new();
        match &val {
            Val::Ptr(p) | Val::Callable { env: Some(p), .. } => pending.push(*p),
            Val::Image(e) => {
                resources.extend(e.resources.values());
                for c in e.cells.values() {
                    if let Cell::Data {
                        ptr, counted: true, ..
                    } = c
                    {
                        pending.push(*ptr);
                    }
                }
            }
            _ => {}
        }
        while let Some(p) = pending.pop() {
            if allocations.contains(&p.alloc) || !self.counted_target(p) {
                continue;
            }
            allocations.push(p.alloc);
            let a = self.ledger.check(p)?;
            resources.extend(a.extent.resources.values());
            for c in a.extent.cells.values() {
                if let Cell::Data {
                    ptr, counted: true, ..
                } = c
                {
                    pending.push(*ptr);
                }
            }
        }
        for r in &resources {
            self.ledger.resources[r.0 as usize].forgotten = true;
        }
        self.ledger.exemptions.push(Exemption::Forget {
            anchor: None,
            allocations: allocations.into(),
            resources: resources.into(),
        });
        self.ledger.trace.push(Event::Forget);
        self.holders.value(&val, 1);
        self.forgotten.push(Held {
            val,
            credits: 1,
            borrow: None,
        });
        Ok(())
    }

    fn terminate(&mut self, f: &mut Frame, term: &Terminator) -> Checked<Control> {
        let u = self.unit(f.unit);
        match *term {
            Terminator::Jump(e) => self.jump(f, e)?,
            Terminator::Branch { condition, yes, no } => {
                let Val::Bits {
                    scalar: ScalarKind::Bool,
                    bits,
                } = self.read_use(f, condition)?
                else {
                    return fail(V::TypeConfusion, "branch condition is not a Bool");
                };
                self.jump(f, if bits == 1 { yes } else { no })?;
            }
            Terminator::Switch {
                value,
                arms,
                otherwise,
            } => {
                // A switch on a variant value selects by its active ordinal.
                let bits = match self.read_use(f, value)? {
                    Val::Bits { bits, .. } => bits,
                    Val::Image(image) => {
                        let layout = self.value_type(f.unit, value)?.layout;
                        let Some(tag) = &self.record(layout)?.tag else {
                            return fail(V::TypeConfusion, "switch on an untagged aggregate");
                        };
                        u128::from(image.active(tag.offset, tag.form, self.word)?)
                    }
                    _ => return fail(V::TypeConfusion, "switch on a non-scalar"),
                };
                let edge = self
                    .pool(arms, &u.pools.switch_arms)?
                    .iter()
                    .find(|a| u128::from(a.value) == bits)
                    .map_or(otherwise, |a| a.edge);
                self.jump(f, edge)?;
            }
            Terminator::Call {
                call,
                normal,
                failure,
            } => return self.call(f, call, normal, failure),
            Terminator::Return(value) => {
                if f.failing {
                    return fail(V::FailureSwallowed, "return after a failure edge was taken");
                }
                let returned = match value {
                    None => None,
                    Some(v) => {
                        let ty = self.value_info(f.unit, v)?.ty;
                        let mut m = self.take_value(f, v)?;
                        let abi = &self.program.layouts.fn_abis[u.header.abi.0 as usize];
                        let borrowed_return = abi
                            .result
                            .as_ref()
                            .is_some_and(|r| !r.borrowed_parameters.is_empty());
                        if !m.credited
                            && !borrowed_return
                            && (m.borrow.is_some() || self.carries_ref(&m.val))
                        {
                            return fail(
                                V::BorrowedReturn,
                                format!("{v:?} returned without a holder credit"),
                            );
                        }
                        if borrowed_return {
                            let allowed = abi.result.as_ref().unwrap().borrowed_parameters.clone();
                            if let Some(borrow) = m.borrow {
                                m.borrow = Some(self.returned_borrow(f, borrow, &allowed)?);
                            }
                            if let Val::Image(image) = &mut m.val {
                                for cell in image.cells.values_mut() {
                                    if let Cell::Data {
                                        borrow: Some(borrow),
                                        ..
                                    } = cell
                                    {
                                        *borrow = self.returned_borrow(f, *borrow, &allowed)?;
                                    }
                                }
                            }
                            let mut returned_parameters = std::collections::BTreeSet::new();
                            if let Some(Borrow::Parameter(p) | Borrow::ParameterCell(p, _)) =
                                m.borrow
                            {
                                returned_parameters.insert(p);
                            }
                            if let Val::Image(image) = &m.val {
                                for cell in image.cells.values() {
                                    if let Cell::Data {
                                        borrow:
                                            Some(Borrow::Parameter(p) | Borrow::ParameterCell(p, _)),
                                        ..
                                    } = cell
                                    {
                                        returned_parameters.insert(*p);
                                    }
                                }
                            }
                            for i in 0..f.loans.len() {
                                if f.loans[i].is_none() {
                                    continue;
                                }
                                let loan = LoanId(i as u32);
                                let address = self.unit(f.unit).tables.loans[i].address;
                                if let Some(slot) = self.resolve_use(f, address)?.root_slot {
                                    if let SlotKind::Parameter(p) =
                                        self.unit(f.unit).slots[slot.index()].kind
                                    {
                                        if returned_parameters.contains(&p) && allowed.contains(&p)
                                        {
                                            self.check_borrow(f, Borrow::Loan(loan))?;
                                            f.loans[i] = None;
                                        }
                                    }
                                }
                            }
                        }
                        let repr = self.lir_type(f.unit, ty)?.repr;
                        Some((
                            Held {
                                val: m.val,
                                credits: u32::from(m.credited && tracked(repr)),
                                borrow: m.borrow,
                            },
                            ty,
                        ))
                    }
                };
                return Ok(Control::Exit(Exit::Return(returned)));
            }
            Terminator::Raise(failure) => {
                let has_rule = u
                    .tables
                    .failures
                    .get(failure.index())
                    .is_some_and(|e| e.rule.is_some());
                if !has_rule {
                    return fail(V::Malformed, "raise along an edge without a rule");
                }
                self.take_failure(f, failure, false)?;
            }
            Terminator::RejectTransfer => {
                let pending = f.pending_custody.take().ok_or_else(|| {
                    err(V::FrameImbalance, "ordinary body forges private status 3")
                })?;
                if pending.admitted || pending.handed || !f.failing {
                    return fail(V::FrameImbalance, "adapter rejects after custody handoff");
                }
                let mut restored = Vec::new();
                for (value, parameter, original) in pending.actuals {
                    let slot = u
                        .slots
                        .iter()
                        .position(|s| s.kind == SlotKind::Parameter(parameter))
                        .ok_or_else(|| err(V::Malformed, "pending formal has no slot"))?;
                    let formal = u.parameters[parameter as usize];
                    if f.versions[slot] != Some(formal) {
                        return fail(V::FrameImbalance, "rejected formal was changed");
                    }
                    let SlotStore::Own(ptr) = f.slots[slot] else {
                        return fail(V::FrameImbalance, "pending take formal is not staged");
                    };
                    let ty = self.lir_type(f.unit, u.slots[slot].ty)?;
                    let size = self.size(ty.layout)?;
                    let owned = self.resources_of(ty.layout)?;
                    let extent = self.take_part(ptr, size, &owned);
                    let (val, _, credited) = self.val_from(extent, ty.repr, ty.layout)?;
                    if val != original.val || credited != (original.credits != 0) {
                        return fail(
                            V::FrameImbalance,
                            "rejection changed an actual or its holder credit",
                        );
                    }
                    restored.push((value, original));
                }
                return Ok(Control::Exit(Exit::RejectTransfer(restored)));
            }
            Terminator::Propagate => {
                if !f.failing {
                    return fail(
                        V::PropagateWithoutFailure,
                        "propagate without an active failure",
                    );
                }
                return Ok(Control::Exit(Exit::Fail));
            }
            // MEM-6: the active failure is exactly this arena's own `OutOfSpace`,
            // the first record since it opened, with no later one.
            Terminator::Recover {
                arena,
                caught,
                failed,
            } => {
                if !f.failing {
                    return fail(V::Malformed, "recover without an active failure");
                }
                let Val::Arena(Some(region)) = self.read_use(f, arena)? else {
                    return fail(V::TypeConfusion, "recover of no arena");
                };
                let Some(budget) = self.ledger.regions[region].budget else {
                    return fail(V::Malformed, "recover of an unbounded arena");
                };
                if budget.exhausted == Some(budget.start) && self.failures.len() == budget.start + 1
                {
                    self.failures.truncate(budget.start);
                    self.unanchored_failures.remove(&budget.start);
                    f.failing = false;
                    self.jump(f, caught)?;
                } else {
                    self.jump(f, failed)?;
                }
            }
            Terminator::Isolate { start, caught } => {
                self.isolate_catch(f, start)?;
                self.jump(f, caught)?;
            }
            Terminator::InvariantFault { ref reason } => {
                return fail(V::Malformed, format!("invariant fault: {reason}"));
            }
            Terminator::Unreachable => return fail(V::UnreachableReached, "unreachable reached"),
        }
        Ok(Control::Continue)
    }

    fn call(
        &mut self,
        f: &mut Frame,
        call: CallId,
        normal: Option<EdgeId>,
        failure: FailureId,
    ) -> Checked<Control> {
        let u = self.unit(f.unit);
        let site = u
            .tables
            .calls
            .get(call.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown call {call:?}")))?;
        let arguments = self.pool(site.arguments, &u.pools.call_arguments)?;
        if self
            .program
            .layouts
            .fn_abis
            .get(site.abi.0 as usize)
            .map(|a| a.shape)
            == Some(hirsel_lir::abi_types::CallableShape::Foreign)
        {
            return fail(
                V::Unsupported,
                "a call of an extern \"c\" function: H does not run C",
            );
        }
        let (callee, env) = match site.callee {
            Callee::Direct(symbol) => (self.program.function(self.symbol(f.unit, symbol)?)?, None),
            Callee::Indirect(v) => match self.read_use(f, v)? {
                Val::Callable {
                    code: Some(code),
                    env,
                } => (code, env),
                Val::Callable { code: None, .. } => {
                    return fail(
                        V::Malformed,
                        format!("indirect call of {v:?}, a closure without code"),
                    );
                }
                _ => {
                    return fail(
                        V::TypeConfusion,
                        format!("indirect call of non-callable {v:?}"),
                    );
                }
            },
            // A bound call (LIR schema revision 9) runs the named instance
            // with its closure's environment.
            Callee::Bound { code, closure } => match self.read_use(f, closure)? {
                Val::Callable { env, .. } => {
                    (self.program.function(self.symbol(f.unit, code)?)?, env)
                }
                _ => {
                    return fail(
                        V::TypeConfusion,
                        format!("bound call of non-callable {closure:?}"),
                    );
                }
            },
            Callee::Operation(op) => return self.operation(f, site, op, normal, failure),
        };
        if callee == usize::MAX - 1 {
            if !arguments.is_empty() {
                return fail(V::Malformed, "runtime iterator step arguments");
            }
            let environment =
                env.ok_or_else(|| err(V::Malformed, "runtime iterator without environment"))?;
            self.ledger.check(environment)?;
            let position = self.scan_scalar(environment, 8)? as usize;
            let (item, next) = if let Some(mut state) = self
                .lazy_iterators
                .get(environment.alloc.0 as usize)
                .cloned()
            {
                let item = if position != 0 {
                    None
                } else {
                    let cause = self
                        .tasks
                        .as_deref()
                        .and_then(|s| s.canceled(self.shielded(f)));
                    self.ops
                        .iterator_next(&mut state, cause)
                        .map_err(|e| err(V::Malformed, e))?
                };
                let ended = item.is_none() || matches!(item, Some(OpValue::Variant { tag: 1, .. }));
                self.lazy_iterators.set(environment.alloc.0 as usize, state);
                (item, usize::from(ended))
            } else {
                let items = self
                    .runtime_iterators
                    .get(environment.alloc.0 as usize)
                    .ok_or_else(|| err(V::ForgedPointer, "unknown runtime iterator environment"))?;
                let item = items.get(position).cloned();
                let next = match &item {
                    Some(OpValue::Variant { tag: 1, .. }) => items.len(),
                    Some(_) => position + 1,
                    None => position,
                };
                (item, next)
            };
            self.ledger
                .get_mut(environment.alloc)
                .extent
                .write_bytes(environment.offset, &(next as u64).to_le_bytes());
            let value = match item {
                Some(item) => OpValue::Variant {
                    tag: 1,
                    value: Box::new(item),
                },
                None => OpValue::Variant {
                    tag: 0,
                    value: Box::new(OpValue::Unit),
                },
            };
            let result = site
                .result
                .ok_or_else(|| err(V::Malformed, "runtime iterator step without output"))?;
            let layout = self.value_type(f.unit, result)?.layout;
            let image = self.operation_image(
                f,
                layout,
                value,
                &[],
                &mut std::collections::BTreeSet::new(),
            )?;
            self.define_fresh(f, result, Val::Image(image))?;
            self.jump(f, returning(normal)?)?;
            return Ok(Control::Continue);
        }
        let cu = self.unit(callee);
        let mut new = self.frame(callee, Role::Callee, env);
        if self.program.pretransfer_adapters.iter().any(|r| {
            r.adapter.definition == cu.header.definition
                && r.adapter.instance == Some(cu.header.instance)
        }) {
            if site.pretransfer_failure.is_none() {
                return fail(
                    V::FrameImbalance,
                    "keyed adapter entered without caller rejection custody",
                );
            }
            new.pending_custody = Some(PendingCustody {
                actuals: Vec::new(),
                admitted: false,
                handed: false,
            });
        }
        if let Some(pending) = &mut f.pending_custody {
            if !pending.admitted || pending.handed {
                return fail(V::FrameImbalance, "adapter handoff before gate or twice");
            }
            pending.handed = true;
        }
        let (caller, operation) =
            self.caller_sites(f, self.source_range(f, site.anchor), site.forward_caller);
        if cu.header.track_caller || cu.header.origin == UnitOrigin::Library {
            new.caller = caller;
        }
        new.operation = operation;
        new.destination = site.destination.clone();
        self.bind_receipt_call(f, &mut new, site)?;
        let mut bound = vec![false; cu.parameters.len()];
        let mut by_address = vec![None; cu.parameters.len()];
        let mut written = Vec::new();
        for arg in arguments {
            let p = *cu
                .parameters
                .get(arg.parameter as usize)
                .ok_or_else(|| err(V::Malformed, format!("no parameter {}", arg.parameter)))?;
            if std::mem::replace(&mut bound[arg.parameter as usize], true) {
                return fail(
                    V::Malformed,
                    format!("parameter {} passed twice", arg.parameter),
                );
            }
            let held = match (arg.passing, arg.value) {
                (hirsel_core_contract::Passing::Take, ArgumentValue::Value(v)) => {
                    if let Some(pending) = &mut new.pending_custody {
                        pending
                            .actuals
                            .push((v, arg.parameter, self.get(f, v)?.clone()));
                    }
                    let m = self.take_owned(f, v, "take argument")?;
                    let repr = self.value_type(callee, p)?.repr;
                    Held {
                        val: m.val,
                        credits: u32::from(m.credited && tracked(repr)),
                        borrow: None,
                    }
                }
                (hirsel_core_contract::Passing::Borrow, ArgumentValue::Value(v)) => {
                    // Validate every leaf in the caller before rebinding its
                    // provenance. Loan IDs are frame-local: an aggregate's
                    // cells need the same parameter provenance as its holder.
                    // The suspended caller keeps the argument live until return.
                    let mut val = self.read_use(f, v)?;
                    let borrow = Borrow::Parameter(arg.parameter);
                    if let Val::Image(image) = &mut val {
                        for (&offset, cell) in &mut image.cells {
                            if let Cell::Data { borrow: source, .. } = cell {
                                *source = Some(Borrow::ParameterCell(arg.parameter, offset));
                            }
                        }
                    }
                    Held {
                        val,
                        credits: 0,
                        borrow: Some(borrow),
                    }
                }
                (
                    passing,
                    ArgumentValue::Address {
                        address: a,
                        version,
                    },
                ) => {
                    let r = self.resolve_use(f, a)?;
                    if passing == hirsel_core_contract::Passing::Mut
                        && let Some(s) = r.root_slot
                    {
                        written.push((s, version));
                    }
                    if passing != hirsel_core_contract::Passing::Take {
                        by_address[arg.parameter as usize] = Some((r.ptr, r.columns));
                    }
                    match passing {
                        hirsel_core_contract::Passing::Borrow => {}
                        _ => self.check_writable(&r)?,
                    }
                    if passing == hirsel_core_contract::Passing::Take {
                        let ty = self.value_type(callee, p)?;
                        let size = self.size(ty.layout)?;
                        let owned = self.resources_of(ty.layout)?;
                        let e = self.take_part(r.ptr, size, &owned);
                        let (val, _, credited) = self.val_from(e, ty.repr, ty.layout)?;
                        Held {
                            val,
                            credits: u32::from(credited && tracked(ty.repr)),
                            borrow: None,
                        }
                    } else {
                        Held {
                            val: Val::Ptr(r.ptr),
                            credits: 0,
                            borrow: Some(Borrow::Parameter(arg.parameter)),
                        }
                    }
                }
                (hirsel_core_contract::Passing::Mut, ArgumentValue::Value(_)) => {
                    return fail(V::Malformed, "mut argument passed by value");
                }
            };
            self.define(&mut new, p, held)?;
        }
        if bound.iter().any(|b| !b) {
            return fail(V::Malformed, "call leaves a parameter unbound");
        }
        self.bind_parameter_slots(&mut new, &by_address)?;
        self.bind_captures(&mut new)?;
        f.resume = Resume::Call {
            call,
            normal,
            failure,
            written,
        };
        Ok(Control::Push(Activity::Frame(Box::new(new))))
    }

    fn operation(
        &mut self,
        f: &mut Frame,
        site: &CallSite,
        op: OperationRef,
        normal: Option<EdgeId>,
        failure: FailureId,
    ) -> Checked<Control> {
        let u = self.unit(f.unit);
        let id = u
            .tables
            .operations
            .get(op.index())
            .ok_or_else(|| err(V::Malformed, format!("unknown operation {op:?}")))?
            .clone();
        if views::is_view_operation(&id) {
            return self.view_operation(f, site, op, returning(normal)?, failure);
        }
        if let Some(name) = tasks::operation(&id) {
            return self.task_operation(f, site, name, normal, failure);
        }
        let arguments = self.pool(site.arguments, &u.pools.call_arguments)?.to_vec();
        let mut args = Vec::new();
        for arg in &arguments {
            let (layout, value) = match arg.value {
                ArgumentValue::Value(v) => {
                    (self.value_type(f.unit, v)?.layout, self.read_use(f, v)?)
                }
                ArgumentValue::Address { address, .. } => {
                    let resolved = self.resolve_use(f, address)?;
                    if arg.passing != hirsel_core_contract::Passing::Borrow {
                        self.check_writable(&resolved)?;
                        let mut exclusive = 0;
                        for (index, loan) in f.loans.iter().enumerate() {
                            if let Some(pointer) = loan
                                && pointer.alloc == resolved.ptr.alloc
                                && pointer.offset == resolved.ptr.offset
                            {
                                let info = &self.unit(f.unit).tables.loans[index];
                                if arg.passing == hirsel_core_contract::Passing::Take
                                    || !info.mutable
                                {
                                    return fail(
                                        V::WriteShared,
                                        "operation handle aliases a live loan",
                                    );
                                }
                                exclusive += 1;
                            }
                        }
                        if exclusive > 1 {
                            return fail(
                                V::WriteShared,
                                "operation handle aliases another exclusive loan",
                            );
                        }
                    }
                    let layout = resolved.layout;
                    let size = self.size(layout)?;
                    let allocation = self.ledger.check(resolved.ptr)?;
                    allocation.extent.check_range(resolved.ptr.offset, size)?;
                    if !allocation.extent.all_init(resolved.ptr.offset, size) {
                        return fail(
                            V::UninitializedRead,
                            "operation address contains a taken value",
                        );
                    }
                    let image = allocation
                        .extent
                        .copy(resolved.ptr.offset, size, Some((&[], &[])));
                    let repr = self
                        .lir_type(f.unit, self.unit(f.unit).addresses[address.index()].ty)?
                        .repr;
                    let (value, _, _) = self.val_from(image, repr, layout)?;
                    self.check_val(&value)?;
                    (layout, value)
                }
            };
            if self.record(layout)?.kind == LayoutKind::Path {
                let Val::Bits {
                    scalar: ScalarKind::U64,
                    bits,
                } = value
                else {
                    return fail(V::TypeConfusion, "Path is not a word");
                };
                args.push(OpValue::Handle(bits as u64));
                continue;
            }
            args.push(match value {
                Val::Unit => OpValue::Unit,
                Val::Bits { scalar, bits } => OpValue::Bits { scalar, bits },
                Val::Handle(h) => OpValue::Handle(h),
                image @ Val::Image(_)
                    if matches!(self.record(layout)?.kind, LayoutKind::Slice { .. }) =>
                {
                    OpValue::Bytes(self.slice_bytes(&image, layout)?)
                }
                Val::Ptr(p) => self.sequence_operand(p, layout)?,
                Val::Image(image) if self.record(layout)?.scan.is_empty() => {
                    let fields = &self.record(layout)?.fields;
                    let [field] = fields.as_slice() else {
                        args.push(self.operand_at(&image, 0, layout)?);
                        continue;
                    };
                    let LayoutKind::Scalar(scalar) = self.record(field.layout)?.kind else {
                        args.push(self.operand_at(&image, 0, layout)?);
                        continue;
                    };
                    let width = self.size(field.layout)?;
                    if width > 16 || !image.all_init(field.offset, width) {
                        return fail(V::UninitializedRead, "operation scalar wrapper");
                    }
                    let mut bits = [0u8; 16];
                    bits[..width as usize].copy_from_slice(image.read_bytes(field.offset, width));
                    OpValue::Bits {
                        scalar,
                        bits: u128::from_le_bytes(bits),
                    }
                }
                Val::Image(image) => self.operand_at(&image, 0, layout)?,
                other => return fail(V::Unsupported, format!("operation operand {other:?}")),
            });
        }
        let outcome = match self.resumed()? {
            Some(outcome) => outcome,
            None => {
                // H derives pure primitive work from checked operands, independently
                // of the provider. Exhaustion precedes the call and any result/effect.
                let cost = primitive_work::cost(&id, &args)?;
                if cost > self.config.step_limit.saturating_sub(self.steps) {
                    self.steps = self.config.step_limit.saturating_add(1);
                    return fail(V::Unsupported, "step limit reached (a tooling limit)");
                }
                self.steps += cost;
                self.timer_choice()?;
                let shielded = self.shielded(f);
                let cause = self.tasks.as_deref().and_then(|s| s.canceled(shielded));
                let outcome = match cause
                    .and_then(|cause| self.ops.canceled(&id, &args, cause, &mut self.output))
                {
                    Some(outcome) => outcome,
                    None => self.ops.call(&id, &args, &mut self.output),
                };
                let outcome = match outcome {
                    OpOutcome::Wait { clock, until } => {
                        // The wait yields when it parks; its call runs again once it is over.
                        self.ops.yielded();
                        self.suspend(Some((clock, until)), id, args)?;
                        return Ok(Control::Continue);
                    }
                    OpOutcome::Pending => {
                        // A stalled scripted source: as a timer wait, with no deadline.
                        self.ops.yielded();
                        self.suspend(None, id, args)?;
                        return Ok(Control::Continue);
                    }
                    OpOutcome::Deadline {
                        clock,
                        at,
                        completed,
                        timed_out,
                    } => {
                        if self.deadline_choice(clock, at)? {
                            *timed_out
                        } else {
                            *completed
                        }
                    }
                    outcome => outcome,
                };
                // Every capability operation is a scheduling point (CONC-1).
                if self.ops.yielded() {
                    self.task_yield()?;
                }
                outcome
            }
        };
        let outcome = match outcome {
            OpOutcome::Updated {
                value,
                arguments: updates,
            } => {
                let mut seen = std::collections::BTreeSet::new();
                for (ordinal, value) in updates {
                    if !seen.insert(ordinal) {
                        return fail(V::Malformed, "duplicate operation update");
                    }
                    let argument = arguments
                        .iter()
                        .find(|a| a.parameter == ordinal)
                        .ok_or_else(|| err(V::Malformed, "update of absent argument"))?;
                    if argument.passing != hirsel_core_contract::Passing::Mut {
                        return fail(V::Malformed, "update of nonexclusive argument");
                    }
                    let ArgumentValue::Address { address, .. } = argument.value else {
                        return fail(V::Malformed, "update without an address");
                    };
                    if id.definition.is_core("bytes.replace_range") {
                        self.replacement_update(f, address, value)?;
                    } else {
                        self.operation_update(f, address, value)?;
                    }
                }
                OpOutcome::Value(value)
            }
            other => other,
        };
        for arg in &arguments {
            if arg.passing == hirsel_core_contract::Passing::Mut
                && let ArgumentValue::Address { address, version } = arg.value
                && let Some(slot) = self.resolve_use(f, address)?.root_slot
            {
                f.versions[slot.index()] = Some(version);
                self.define_version(f, version)?;
            }
        }
        if normal.is_none()
            && !matches!(outcome, OpOutcome::Fail(_))
            && !matches!(outcome, OpOutcome::Complete { result: Err(_), .. })
        {
            return fail(V::Malformed, "a diverging operation completed");
        }
        let (outcome, consumed) = match outcome {
            OpOutcome::Complete { result, consumed } => (
                match result {
                    Ok(v) => OpOutcome::Value(v),
                    Err(rule) => OpOutcome::Fail(rule),
                },
                consumed,
            ),
            other => (other, Vec::new()),
        };
        let mut transferred = std::collections::BTreeSet::new();
        for ordinal in consumed {
            let argument = arguments
                .iter()
                .find(|a| a.parameter == ordinal)
                .ok_or_else(|| err(V::Malformed, "consumed absent argument"))?;
            if argument.passing != hirsel_core_contract::Passing::Take
                || !transferred.insert(ordinal)
            {
                return fail(V::Malformed, "invalid operation consumption");
            }
            let (moved, _) = self.operation_take(f, argument)?;
            if let Val::Handle(h) = moved.val {
                match self.ledger.handles.get_mut(&h) {
                    Some((state @ crate::ledger::HandleState::Open, _)) => {
                        *state = crate::ledger::HandleState::Closed
                    }
                    _ => return fail(V::DoubleClose, "operation consumed a closed handle"),
                }
            } else if self.carries(&moved.val) {
                return fail(
                    V::Malformed,
                    "operation consumed a counted value without transfer",
                );
            }
        }
        match outcome {
            OpOutcome::Value(value) => {
                let val = match value {
                    OpValue::Unit => Val::Unit,
                    OpValue::Bits { scalar, bits } => Val::Bits { scalar, bits },
                    OpValue::OpenHandle(h) => {
                        if self.ledger.handles.contains_key(&h) {
                            return fail(V::Malformed, "operation reopened a handle");
                        }
                        self.ledger
                            .handles
                            .insert(h, (crate::ledger::HandleState::Open, false));
                        Val::Handle(h)
                    }
                    OpValue::Plain(bytes) => {
                        let Some(result) = site.result else {
                            return fail(V::Malformed, "an operation result with no place");
                        };
                        let layout = self.value_type(f.unit, result)?.layout;
                        if self.size(layout)? != bytes.len() as u64 {
                            return fail(
                                V::Malformed,
                                "an operation result of another size than its layout",
                            );
                        }
                        let mut image = Extent::new(bytes.len() as u64);
                        image.write_bytes(0, &bytes);
                        Val::Image(image)
                    }
                    value @ (OpValue::LazyIterator(_)
                    | OpValue::Iterator(_)
                    | OpValue::Aggregate(_)
                    | OpValue::Sequence(_)
                    | OpValue::TakenHandle { .. }
                    | OpValue::ByteSequence(_)
                    | OpValue::Slice { .. }
                    | OpValue::Variant { .. }) => {
                        let result = site
                            .result
                            .ok_or_else(|| err(V::Malformed, "byte result without output"))?;
                        let layout = self.value_type(f.unit, result)?.layout;
                        let image =
                            self.operation_image(f, layout, value, &arguments, &mut transferred)?;
                        let repr = self.value_type(f.unit, result)?.repr;
                        self.val_from(image, repr, layout)?.0
                    }
                    // A plain capability identity: nothing to close, so the
                    // ledger tracks no handle for it.
                    OpValue::Handle(h) => {
                        if site.result.is_some_and(|result| {
                            self.record(self.value_type(f.unit, result).unwrap().layout)
                                .unwrap()
                                .kind
                                == LayoutKind::Path
                        }) {
                            Val::Bits {
                                scalar: ScalarKind::U64,
                                bits: u128::from(h),
                            }
                        } else {
                            Val::Handle(h)
                        }
                    }
                    OpValue::ViewCursor { .. }
                    | OpValue::SomeSlice { .. }
                    | OpValue::Bytes(_)
                    | OpValue::Elements { .. } => {
                        return fail(V::Malformed, "an operand value returned as a result");
                    }
                };
                if let Some(result) = site.result {
                    self.define_fresh(f, result, val)?;
                }
                let work = self.operation_cleanup(f, &arguments, &transferred)?;
                let normal = returning(normal)?;
                if work.is_empty() {
                    self.jump(f, normal)?;
                } else {
                    return Ok(self.drain(
                        f,
                        work,
                        After::Operation {
                            normal: Some(normal),
                        },
                        Some(failure),
                    ));
                }
            }
            OpOutcome::Fail(rule) => {
                self.record_failure(f, &rule, site.anchor);
                let work = self.operation_cleanup(f, &arguments, &transferred)?;
                if work.is_empty() {
                    self.take_failure(f, failure, false)?;
                } else {
                    return Ok(self.drain(
                        f,
                        work,
                        After::Operation { normal: None },
                        Some(failure),
                    ));
                }
            }
            OpOutcome::Complete { .. } => unreachable!(),
            OpOutcome::Wait { .. } | OpOutcome::Pending | OpOutcome::Deadline { .. } => {
                return fail(V::Malformed, "a nested suspended outcome");
            }
            OpOutcome::Updated { .. } => {
                return fail(V::Malformed, "unexpected exclusive operation update");
            }
            OpOutcome::Unsupported => {
                return fail(V::Unsupported, format!("operation {:?}", id.definition));
            }
        }
        Ok(Control::Continue)
    }

    fn operation_take(
        &mut self,
        f: &mut Frame,
        argument: &CallArgument,
    ) -> Checked<(Moved, LayoutId)> {
        match argument.value {
            ArgumentValue::Value(value) => {
                let layout = self.value_type(f.unit, value)?.layout;
                Ok((self.take_owned(f, value, "operation argument")?, layout))
            }
            ArgumentValue::Address { address, .. } => {
                let resolved = self.resolve_use(f, address)?;
                self.check_writable(&resolved)?;
                let layout = resolved.layout;
                let size = self.size(layout)?;
                let owned = self.resources_of(layout)?;
                let image = self.take_part(resolved.ptr, size, &owned);
                let repr = self
                    .lir_type(f.unit, self.unit(f.unit).addresses[address.index()].ty)?
                    .repr;
                let (val, borrow, credited) = self.val_from(image, repr, layout)?;
                Ok((
                    Moved {
                        val,
                        credited,
                        borrow,
                    },
                    layout,
                ))
            }
        }
    }
    fn operation_cleanup(
        &mut self,
        f: &mut Frame,
        arguments: &[CallArgument],
        transferred: &std::collections::BTreeSet<u32>,
    ) -> Checked<Vec<Work>> {
        let mut work = Vec::new();
        // The work stack pops latest first: append in source argument order.
        for argument in arguments {
            if argument.passing == hirsel_core_contract::Passing::Take
                && !transferred.contains(&argument.parameter)
            {
                let (moved, layout) = self.operation_take(f, argument)?;
                let temporary = self.temporary(layout)?;
                self.write_value(temporary, layout, moved)?;
                work.push(Work::Death {
                    alloc: temporary.alloc,
                    raw: None,
                });
                work.push(Work::DropAt {
                    at: temporary,
                    layout,
                    mask: None,
                });
            }
        }
        Ok(work)
    }
    /// Install pointer-free scalar buffer updates while retaining H's own storage
    /// and credit accounting. The external model never supplies a heap pointer.
    /// The wrapper has already established writable, initialized storage and
    /// accounted any arena COW. Replacement cannot publish a different owner,
    /// length or capacity as generic append/update marshalling does.
    fn replacement_update(&mut self, f: &Frame, address: AddressId, value: OpValue) -> Checked<()> {
        let OpValue::Elements { scalar, bits } = value else {
            return fail(V::Malformed, "replacement update is not a byte buffer");
        };
        let r = self.resolve_use(f, address)?;
        self.check_writable(&r)?;
        let LayoutKind::Block { payload } = self.record(r.layout)?.kind else {
            return fail(V::TypeConfusion, "replacement owner layout");
        };
        let LayoutKind::Sequence { element } = self.record(payload)?.kind else {
            return fail(V::TypeConfusion, "replacement sequence layout");
        };
        if scalar != hirsel_lir::layout_types::ScalarKind::U8
            || self.record(element)?.kind != LayoutKind::Scalar(scalar)
        {
            return fail(V::TypeConfusion, "replacement element layout");
        }
        let owner = self.read_ptr(r.ptr)?;
        self.check_private(owner)?;
        let len = self.read_scalar(owner, 8)? as u64;
        if bits.len() as u64 != len {
            return fail(V::OutOfExtent, "replacement changed initialized length");
        }
        let (start, stride) = self.sequence_shape(element)?;
        // Require all original bytes to be initialized before updating them.
        let extent = &self.ledger.check(owner)?.extent;
        extent.check_range(owner.offset + start, len * stride)?;
        if !extent.all_init(owner.offset + start, len * stride) {
            return fail(V::UninitializedRead, "replacement of uninitialized bytes");
        }
        let extent = &mut self.ledger.get_mut(owner.alloc).extent;
        for (i, byte) in bits.into_iter().enumerate() {
            extent.write_bytes(owner.offset + start + i as u64 * stride, &[byte as u8]);
        }
        Ok(())
    }

    fn operation_update(&mut self, f: &Frame, address: AddressId, value: OpValue) -> Checked<()> {
        let OpValue::Elements { scalar, bits } = value else {
            return fail(V::Malformed, "operation update is not a scalar buffer");
        };
        let r = self.resolve_use(f, address)?;
        self.check_writable(&r)?;
        let (element, old, count) = match self.record(r.layout)?.kind {
            LayoutKind::Block { payload } => {
                let LayoutKind::Sequence { element } = self.record(payload)?.kind else {
                    return fail(V::TypeConfusion, "update block payload");
                };
                (
                    element,
                    Some((self.read_ptr(r.ptr)?, payload)),
                    bits.len() as u64,
                )
            }
            LayoutKind::Inline { element, count } if count == bits.len() as u64 => {
                (element, None, count)
            }
            _ => return fail(V::TypeConfusion, "operation update layout"),
        };
        if self.record(element)?.kind != LayoutKind::Scalar(scalar) {
            return fail(V::TypeConfusion, "operation update scalar kind");
        }
        let (start, stride) = if old.is_some() {
            self.sequence_shape(element)?
        } else {
            (0, self.size(element)?)
        };
        let size = count
            .checked_mul(stride)
            .and_then(|n| start.checked_add(n))
            .ok_or_else(|| err(V::OutOfExtent, "operation update extent"))?;
        if let Some((old, _)) = old
            && self.tracked_region(old)?.is_some()
        {
            // An Io read into a tracked arena List appends in the room its
            // `ArenaPush` prepared and charged (MEM-6, LIR schema revision 34).
            self.check_private(old)?;
            if count > self.sequence_capacity_word(old, element)? {
                return fail(
                    V::OutOfExtent,
                    "an Io read past the room prepared in its arena List",
                );
            }
            let extent = &mut self.ledger.get_mut(old.alloc).extent;
            extent.write_bytes(old.offset, &count.to_le_bytes());
            for (i, bits) in bits.into_iter().enumerate() {
                extent.write_bytes(
                    old.offset + start + i as u64 * stride,
                    &bits.to_le_bytes()[..stride as usize],
                );
            }
            return Ok(());
        }
        let mut image = Extent::new(size);
        if old.is_some() {
            image.write_bytes(0, &count.to_le_bytes());
        }
        for (i, bits) in bits.into_iter().enumerate() {
            image.write_bytes(
                start + i as u64 * stride,
                &bits.to_le_bytes()[..stride as usize],
            );
        }
        if let Some((old, payload)) = old {
            let Cell::Data { counted, .. } = self.read_cell(r.ptr)? else {
                return fail(V::ForgedPointer, "update sequence owner");
            };
            if !counted && self.counted_target(old) {
                return fail(V::CreditUnderflow, "update replaced an uncredited owner");
            }
            let owner = self.heap_alloc(payload, size, self.record(payload)?.align)?;
            self.ledger.get_mut(owner.alloc).extent.place(0, image);
            // A semantic buffer update constructs a complete sequence header,
            // including the capacity read by a later scalar push.
            self.exact_capacity(owner, element)?;
            if counted && self.count_down(old)? {
                self.ledger.free(old.alloc)?;
            }
            self.ledger.get_mut(r.ptr.alloc).extent.put_cell(
                r.ptr.offset,
                self.word,
                Cell::Data {
                    ptr: owner,
                    counted: true,
                    borrow: None,
                },
            );
        } else {
            self.ledger
                .get_mut(r.ptr.alloc)
                .extent
                .place(r.ptr.offset, image);
        }
        Ok(())
    }

    /// Materialize semantic results using only checked layouts, storage and credits.
    fn operation_image(
        &mut self,
        f: &mut Frame,
        layout: LayoutId,
        value: OpValue,
        arguments: &[CallArgument],
        transferred: &mut std::collections::BTreeSet<u32>,
    ) -> Checked<Extent> {
        match value {
            OpValue::TakenHandle { argument } => {
                let argument = arguments
                    .iter()
                    .find(|a| a.parameter == argument)
                    .ok_or_else(|| err(V::Malformed, "transfer of absent handle"))?;
                if argument.passing != hirsel_core_contract::Passing::Take
                    || !transferred.insert(argument.parameter)
                {
                    return fail(V::Malformed, "invalid handle transfer");
                }
                let (moved, source_layout) = self.operation_take(f, argument)?;
                let Val::Handle(handle) = moved.val else {
                    return fail(V::TypeConfusion, "transfer of non-handle");
                };
                if source_layout != layout {
                    return fail(V::TypeConfusion, "transferred handle layout");
                }
                self.operation_image(f, layout, OpValue::Handle(handle), arguments, transferred)
            }
            OpValue::Aggregate(values) => {
                let record = self.record(layout)?.clone();
                if !matches!(
                    record.kind,
                    LayoutKind::Aggregate | LayoutKind::Opaque { .. }
                ) || record.tag.is_some()
                {
                    return fail(V::TypeConfusion, "aggregate operation result layout");
                }
                let mut fields = record.fields;
                fields.sort_by_key(|f| f.ordinal);
                if fields.len() != values.len() {
                    return fail(V::TypeConfusion, "aggregate operation result arity");
                }
                let mut image = Extent::new(self.size(layout)?);
                for (field, value) in fields.iter().zip(values) {
                    let part =
                        self.operation_image(f, field.layout, value, arguments, transferred)?;
                    image.place_at(field.offset, &[ProjectionStep::Field(field.ordinal)], part);
                }
                Ok(image)
            }
            OpValue::Sequence(values) => {
                let record = self.record(layout)?.clone();
                let (element, block) = match record.kind {
                    LayoutKind::Block { payload } => {
                        let LayoutKind::Sequence { element } = self.record(payload)?.kind else {
                            return fail(V::TypeConfusion, "sequence result payload");
                        };
                        (element, Some(payload))
                    }
                    LayoutKind::Inline { element, count } if count == values.len() as u64 => {
                        (element, None)
                    }
                    _ => return fail(V::TypeConfusion, "sequence operation result layout"),
                };
                let (start, stride) = if block.is_some() {
                    self.sequence_shape(element)?
                } else {
                    (0, self.size(element)?)
                };
                let count = values.len() as u64;
                let size = count
                    .checked_mul(stride)
                    .and_then(|n| start.checked_add(n))
                    .ok_or_else(|| err(V::OutOfExtent, "operation sequence result extent"))?;
                let mut image = Extent::new(size);
                if block.is_some() {
                    image.write_bytes(0, &count.to_le_bytes());
                    image.write_bytes(
                        hirsel_lir::layout::SEQUENCE_CAPACITY_OFFSET,
                        &count.to_le_bytes(),
                    );
                }
                for (index, value) in values.into_iter().enumerate() {
                    let part = self.operation_image(f, element, value, arguments, transferred)?;
                    image.place_at(
                        start + index as u64 * stride,
                        &[ProjectionStep::Index(index as u64)],
                        part,
                    );
                }
                if let Some(payload) = block {
                    let owner = self.heap_alloc(payload, size, self.record(payload)?.align)?;
                    self.ledger.get_mut(owner.alloc).extent.place(0, image);
                    let mut image = Extent::new(self.word);
                    image.put_cell(
                        0,
                        self.word,
                        Cell::Data {
                            ptr: owner,
                            counted: true,
                            borrow: None,
                        },
                    );
                    Ok(image)
                } else {
                    Ok(image)
                }
            }
            value @ (OpValue::Iterator(_) | OpValue::LazyIterator(_)) => {
                let (items, lazy) = match value {
                    OpValue::Iterator(items) => (items, None),
                    OpValue::LazyIterator(state) => (vec![], Some(*state)),
                    _ => unreachable!(),
                };
                if !matches!(self.record(layout)?.kind, LayoutKind::Iterator { .. }) {
                    return fail(V::TypeConfusion, "runtime iterator result layout");
                }
                for item in &items {
                    match item {
                        OpValue::Variant { tag: 0, value }
                            if matches!(**value, OpValue::Handle(_)) => {}
                        OpValue::Variant { tag: 1, value } if matches!(**value,OpValue::Plain(ref bytes) if bytes.len()==8) =>
                            {}
                        _ => {
                            return fail(
                                V::Malformed,
                                "runtime walk item is not a Path or FileError",
                            );
                        }
                    }
                }
                let payload = self
                    .program
                    .layouts
                    .layouts
                    .iter()
                    .position(|r| r.kind == LayoutKind::Scalar(ScalarKind::U64))
                    .ok_or_else(|| err(V::Malformed, "runtime iterator needs u64 layout"))?;
                let environment = self.heap_alloc(LayoutId(payload as u32), 8, 8)?;
                self.ledger
                    .get_mut(environment.alloc)
                    .extent
                    .write_bytes(0, &0u64.to_le_bytes());
                if let Some(state) = lazy {
                    self.lazy_iterators.set(environment.alloc.0 as usize, state);
                } else {
                    self.runtime_iterators
                        .set(environment.alloc.0 as usize, items.into());
                }
                let mut image = Extent::new(self.size(layout)?);
                image.write_bytes(0, &vec![0; self.size(layout)? as usize]);
                image.put_cell(
                    0,
                    self.word,
                    Cell::Data {
                        ptr: environment,
                        counted: true,
                        borrow: None,
                    },
                );
                image.put_cell(self.word, self.word, Cell::Code(Some(usize::MAX - 1)));
                Ok(image)
            }

            OpValue::Handle(handle) | OpValue::OpenHandle(handle) => {
                if self.record(layout)?.kind == LayoutKind::Path {
                    let mut image = Extent::new(8);
                    image.write_bytes(0, &handle.to_le_bytes());
                    return Ok(image);
                }
                if !matches!(
                    self.record(layout)?.kind,
                    LayoutKind::Handle | LayoutKind::HostCapability { .. }
                ) {
                    return fail(V::TypeConfusion, "handle result at another layout");
                }
                if matches!(value, OpValue::OpenHandle(_)) {
                    if self.ledger.handles.contains_key(&handle) {
                        return fail(V::Malformed, "operation reopened a handle");
                    }
                    self.ledger
                        .handles
                        .insert(handle, (crate::ledger::HandleState::Open, false));
                }
                let mut image = Extent::new(self.size(layout)?);
                image.write_bytes(0, &handle.to_le_bytes());
                Ok(image)
            }
            OpValue::Bits { scalar, bits } => {
                let kind = &self.record(layout)?.kind;
                if *kind != LayoutKind::Scalar(scalar) {
                    return fail(
                        V::TypeConfusion,
                        format!("operation {scalar:?} result at a {kind:?} layout"),
                    );
                }
                let size = self.size(layout)?;
                let mut image = Extent::new(size);
                image.write_bytes(0, &bits.to_le_bytes()[..size as usize]);
                Ok(image)
            }
            OpValue::Unit => {
                if self.size(layout)? != 0 {
                    return fail(V::TypeConfusion, "unit operation payload");
                }
                Ok(Extent::new(0))
            }

            OpValue::ByteSequence(bytes) => {
                let LayoutKind::Slice { element } = self.record(layout)?.kind else {
                    return fail(
                        V::TypeConfusion,
                        "byte sequence returned at a non-slice layout",
                    );
                };
                if self.record(element)?.kind != LayoutKind::Scalar(ScalarKind::U8) {
                    return fail(V::TypeConfusion, "byte sequence with a non-byte element");
                }
                let payload = self
                    .program
                    .layouts
                    .layouts
                    .iter()
                    .enumerate()
                    .find(|(_, r)| r.kind == LayoutKind::Sequence { element })
                    .map(|(i, _)| LayoutId(i as u32))
                    .ok_or_else(|| {
                        err(V::Malformed, "slice has no byte sequence payload layout")
                    })?;
                let length = bytes.len() as u64;
                let (stored, whole) = self.byte_sequence(element, &bytes)?;
                let owner = self.heap_alloc(payload, stored.len() as u64, 8)?;
                self.ledger
                    .get_mut(owner.alloc)
                    .extent
                    .write_bytes(0, &stored);
                let mut image = self.slice_image(layout, owner, whole, length)?;
                for cell in image.cells.values_mut() {
                    if let Cell::Data { counted, .. } = cell {
                        *counted = true;
                    }
                }
                Ok(image)
            }
            OpValue::Slice {
                argument,
                start,
                length,
            } => {
                let arg = arguments
                    .iter()
                    .find(|a| a.parameter == argument)
                    .ok_or_else(|| err(V::Malformed, "slice result names an absent argument"))?;
                let ArgumentValue::Value(v) = arg.value else {
                    return fail(V::Malformed, "slice result names an address argument");
                };
                let source_layout = self.value_type(f.unit, v)?.layout;
                if !matches!(self.record(layout)?.kind, LayoutKind::Slice { .. })
                    || !matches!(self.record(source_layout)?.kind, LayoutKind::Slice { .. })
                {
                    return fail(V::TypeConfusion, "slice result or source is not a slice");
                }
                let LayoutKind::Slice { element } = self.record(layout)?.kind else {
                    unreachable!();
                };
                if self.record(element)?.kind != LayoutKind::Scalar(ScalarKind::U8)
                    || self.record(source_layout)?.kind != (LayoutKind::Slice { element })
                {
                    return fail(
                        V::TypeConfusion,
                        "byte slice result changes its input element layout",
                    );
                }
                let source = self.read_use(f, v)?;
                let bytes = self.slice_bytes(&source, source_layout)?;
                if start
                    .checked_add(length)
                    .is_none_or(|end| end > bytes.len() as u64)
                {
                    return fail(V::OutOfExtent, "slice result exceeds its input range");
                }
                let Val::Image(source) = source else {
                    unreachable!("slice_bytes checked");
                };
                let fields = &self.record(source_layout)?.fields;
                let Some(Cell::Data { ptr: owner, .. }) = source.cells.get(&fields[0].offset)
                else {
                    return fail(V::ForgedPointer, "slice result source owner");
                };
                let owner = *owner;
                let origin =
                    u64::from_le_bytes(source.read_bytes(fields[1].offset, 8).try_into().unwrap());
                let origin = origin
                    .checked_add(start)
                    .ok_or_else(|| err(V::OutOfExtent, "slice origin overflow"))?;
                let mut image = self.slice_image(layout, owner, origin, length)?;
                match arg.passing {
                    hirsel_core_contract::Passing::Borrow => {
                        let (Val::Image(image), _) = self.copy_value(Val::Image(image), layout)?
                        else {
                            unreachable!()
                        };
                        Ok(image)
                    }
                    hirsel_core_contract::Passing::Take => {
                        if !transferred.insert(argument) {
                            return fail(V::CreditUnderflow, "taken slice returned twice");
                        }
                        let moved = self.take_owned(f, v, "operation slice result")?;
                        for cell in image.cells.values_mut() {
                            if let Cell::Data {
                                counted, borrow, ..
                            } = cell
                            {
                                *counted = moved.credited && self.counted_target(owner);
                                *borrow = None;
                            }
                        }
                        Ok(image)
                    }
                    _ => fail(V::Malformed, "slice result names an exclusive argument"),
                }
            }
            OpValue::Variant {
                tag: ordinal,
                value,
            } => {
                let record = self.record(layout)?.clone();
                let tag = record
                    .tag
                    .ok_or_else(|| err(V::TypeConfusion, "variant result at an untagged layout"))?;
                let variant = tag
                    .variants
                    .iter()
                    .find(|v| v.ordinal == ordinal)
                    .ok_or_else(|| err(V::TypeConfusion, "variant result has an invalid tag"))?;
                if variant.fields.len() > 1 {
                    return fail(V::Malformed, "operation variant payload arity");
                }
                let part = match variant.fields.first() {
                    Some(field) => Some((
                        field.offset,
                        self.operation_image(f, field.layout, *value, arguments, transferred)?,
                    )),
                    None => {
                        if !matches!(*value, OpValue::Unit) {
                            return fail(V::Malformed, "dataful value for nullary variant");
                        }
                        None
                    }
                };
                let mut image = Extent::new(self.size(layout)?);
                image.write_bytes(0, &vec![0; self.size(layout)? as usize]);
                if tag.form == TagForm::Ordinal {
                    image.write_bytes(tag.offset, &ordinal.to_le_bytes());
                }
                if let Some((offset, part)) = part {
                    image.place_at(
                        offset,
                        &[ProjectionStep::VariantField {
                            variant: ordinal,
                            field: variant.fields[0].ordinal,
                        }],
                        part,
                    );
                }
                Ok(image)
            }
            OpValue::Plain(bytes) => {
                if bytes.len() as u64 != self.size(layout)? || !self.record(layout)?.scan.is_empty()
                {
                    return fail(V::TypeConfusion, "plain variant payload size or pointers");
                }
                let mut image = Extent::new(bytes.len() as u64);
                image.write_bytes(0, &bytes);
                Ok(image)
            }
            _ => fail(V::Malformed, "unsupported inline operation payload"),
        }
    }

    // ---- Frame exit ------------------------------------------------------

    /// H computes task-local admission from its own event ledger. No native
    /// runtime counter or producer-asserted invocation total is consulted.
    fn admit_invocation(&mut self, f: &Frame) -> Option<bool> {
        let task = self.tasks.as_ref().map_or(0, |tasks| tasks.current);
        let root = task == 0
            && !self
                .unit(self.entry)
                .header
                .definition
                .0
                .modules
                .iter()
                .any(|module| module.as_str() == "$native");
        let events = self.invocation_events.entry(task).or_default();
        let cleanup = f.shields() || events.last() == Some(&true);
        let used = events.iter().filter(|&&event| event == cleanup).count()
            + usize::from(!cleanup && root);
        let limit = if cleanup {
            usize::from(hirsel_core_contract::depth::CLEANUP_LIMIT)
        } else {
            usize::from(hirsel_core_contract::depth::FOREGROUND_LIMIT)
        };
        if used >= limit {
            return None;
        }
        events.push(cleanup);
        Some(cleanup)
    }

    /// Checked serialization cannot unwind into ordinary language cleanup.
    fn fatal_check(&mut self) {
        self.stack.clear();
        self.tasks = None;
        self.finished = Some(Finished::Failed);
        self.ended = true;
    }

    /// A frame calling a decode's check (SER-1) on a copy of the occurrence it borrows.
    fn check_frame(
        &mut self,
        caller: &Frame,
        unit: usize,
        occurrence: Extent,
    ) -> Checked<Option<Frame>> {
        let sites = caller
            .invocation_sites
            .last()
            .copied()
            .unwrap_or((caller.caller, caller.operation));
        let Some(cleanup) = self.admit_invocation(caller) else {
            self.record_sites("LANG-10", sites.0, sites.1, caller.unit, true);
            self.fatal_check();
            return Ok(None);
        };
        let [p] = self.unit(unit).parameters.as_slice() else {
            return fail(
                V::Malformed,
                "a codec check takes exactly its borrowed value",
            );
        };
        let p = *p;
        let mut frame = self.frame(unit, Role::Callee, None);
        frame.implicit_invocation = Some(cleanup);
        frame.caller = sites.0;
        frame.operation = sites.1;
        self.define(
            &mut frame,
            p,
            Held {
                val: Val::Image(occurrence),
                credits: 0,
                borrow: Some(Borrow::Caller),
            },
        )?;
        self.bind_parameter_slots(&mut frame, &[None])?;
        Ok(Some(frame))
    }

    /// A decode's check returned to `caller`, or failed.
    fn check_returned(&mut self, mut caller: Box<Frame>, exit: Exit) -> Checked<()> {
        let Resume::Checks {
            result,
            held,
            mut pending,
        } = std::mem::replace(&mut caller.resume, Resume::Running)
        else {
            unreachable!("checked by the caller")
        };
        let verdict = match exit {
            Exit::RejectTransfer(_) => {
                return fail(V::FrameImbalance, "codec check returned private status 3");
            }
            // FAIL-3: the codec cannot unwind, so the check's failure ends the run.
            Exit::Fail => {
                self.fatal_check();
                return Ok(());
            }
            Exit::Return(Some((
                Held {
                    val:
                        Val::Bits {
                            scalar: ScalarKind::Bool,
                            bits,
                        },
                    ..
                },
                _,
            ))) => bits != 0,
            Exit::Return(_) => return fail(V::Malformed, "a codec check that returns no bool"),
        };
        let ty = self.value_type(caller.unit, result)?;
        if verdict && let Some((unit, occurrence)) = pending.pop() {
            let Some(frame) = self.check_frame(&caller, unit, occurrence)? else {
                return Ok(());
            };
            caller.resume = Resume::Checks {
                result,
                held,
                pending,
            };
            self.stack.push(Activity::Frame(caller));
            self.stack.push(Activity::Frame(Box::new(frame)));
        } else if verdict {
            let size = self.size(ty.layout)?;
            let owned = self.resources_of(ty.layout)?;
            let e = self.take_part(held, size, &owned);
            let (val, _, _) = self.val_from(e, ty.repr, ty.layout)?;
            self.ledger.free(held.alloc)?;
            self.define_fresh(&mut caller, result, val)?;
            caller.statement += 1;
            self.stack.push(Activity::Frame(caller));
        } else {
            // The first rejection releases the decoded value: Error(Invalid).
            let unit = self.unit(caller.unit);
            let Some(Inst::Decode { codec, .. }) = self
                .pool(
                    unit.blocks[caller.block.index()].instructions,
                    &unit.instructions,
                )?
                .get(caller.statement as usize)
            else {
                return fail(V::Malformed, "codec checks resumed off their decode");
            };
            let plan = &unit.tables.codecs[codec.index()];
            let (invalid, _, _) = self.codec_invalid(plan)?;
            self.define_fresh(&mut caller, result, invalid)?;
            caller.resume = Resume::Drain {
                after: After::Next,
                failure: None,
            };
            self.stack.push(Activity::Frame(caller));
            self.stack.push(Activity::Drain(Drain {
                caller: None,
                operation: None,
                work: vec![
                    Work::Death {
                        alloc: held.alloc,
                        raw: None,
                    },
                    Work::DropAt {
                        at: held,
                        layout: ty.layout,
                        mask: None,
                    },
                ],
                failed: false,
            }));
        }
        Ok(())
    }

    fn exit_frame(&mut self, f: Frame, exit: Exit) -> Checked<()> {
        if let Some(pending) = &f.pending_custody {
            if !pending.admitted || matches!(exit, Exit::Return(_)) && !pending.handed {
                return fail(
                    V::FrameImbalance,
                    "adapter exits without committing pending custody",
                );
            }
        }
        if !matches!(f.resume, Resume::Running) {
            return fail(V::Malformed, "frame exits while waiting");
        }
        for (index, held) in f.values.iter().enumerate() {
            if let Some(held) = held
                && held.credits > 0
                && self.carries(&held.val)
            {
                let kind = if matches!(held.val, Val::Token(_)) {
                    V::LeakedToken
                } else {
                    V::LeakedCredit
                };
                return fail(
                    kind,
                    format!(
                        "ValueId({index}) still holds {} credit(s) at frame exit",
                        held.credits
                    ),
                );
            }
        }
        for held in f.values.iter().flatten() {
            self.holders.value(&held.val, -i64::from(held.credits));
        }
        if let Some(l) = f.loans.iter().position(Option::is_some) {
            return fail(
                V::LoanOutlivesFrame,
                format!("LoanId({l}) is active at frame exit"),
            );
        }
        if !f.regions.is_empty() {
            return fail(V::LiveRegion, "a region is live at frame exit");
        }
        if f.admitted != 0 || !f.invocation_sites.is_empty() {
            return fail(V::FrameImbalance, "frame admission not left at exit");
        }
        for (index, slot) in f.slots.iter().enumerate() {
            if let SlotStore::Own(p) = slot {
                self.end_slot(*p, SlotId(index as u32))?;
            }
        }
        for ptr in f.spills.iter().flatten() {
            self.ledger.free(ptr.alloc)?;
        }
        if let Exit::Return(Some((held, _))) = &exit {
            self.check_val(&held.val)?;
        }
        if let Some(cleanup) = f.implicit_invocation {
            let task = self.tasks.as_ref().map_or(0, |tasks| tasks.current);
            if self.invocation_events.get_mut(&task).and_then(Vec::pop) != Some(cleanup) {
                return fail(
                    V::FrameImbalance,
                    "implicit check retires without matching entry",
                );
            }
        }
        match self.stack.pop() {
            Some(Activity::Frame(caller)) if matches!(caller.resume, Resume::Checks { .. }) => {
                self.check_returned(caller, exit)?;
            }
            Some(Activity::Frame(mut caller)) => {
                let Resume::Call {
                    call,
                    normal,
                    failure,
                    written,
                } = std::mem::replace(&mut caller.resume, Resume::Running)
                else {
                    return fail(
                        V::Malformed,
                        "callee returned to a frame that is not calling",
                    );
                };
                if let Exit::RejectTransfer(restored) = exit {
                    let rejected = self.unit(caller.unit).tables.calls[call.index()]
                        .pretransfer_failure
                        .ok_or_else(|| {
                            err(
                                V::FrameImbalance,
                                "caller has no pending-custody rejection edge",
                            )
                        })?;
                    for (value, held) in restored {
                        self.define(&mut caller, value, held)?;
                    }
                    self.take_failure(&mut caller, rejected, false)?;
                    self.stack.push(Activity::Frame(caller));
                    return Ok(());
                }
                for (slot, version) in written {
                    caller.versions[slot.index()] = Some(version);
                    self.define_version(&mut caller, version)?;
                }
                let site_result = self.unit(caller.unit).tables.calls[call.index()].result;
                let result = match exit {
                    Exit::RejectTransfer(_) => unreachable!("handled before writebacks"),
                    Exit::Fail => {
                        let unit = self.unit(caller.unit);
                        if unit.header.origin != UnitOrigin::Library {
                            let call = &unit.tables.calls[call.index()];
                            let (inherited, operation) = self.caller_sites(
                                &caller,
                                self.source_range(&caller, call.anchor),
                                call.forward_caller,
                            );
                            let primary = inherited.or(operation);
                            if let Some(primary) = primary {
                                let (definition, range) = self.observe_site(primary);
                                let operation = operation
                                    .filter(|site| *site != primary)
                                    .map(|site| self.observe_site(site));
                                for i in std::mem::take(&mut self.unanchored_failures) {
                                    self.failures[i].definition = definition.clone();
                                    self.failures[i].site = (range.start, range.end);
                                    self.failures[i].source = range.source;
                                    self.failures[i].operation_source =
                                        operation.as_ref().map_or(u32::MAX, |(_, s)| s.source);
                                    self.failures[i].operation =
                                        operation.clone().map(|(d, s)| (d, (s.start, s.end)));
                                }
                            }
                        }
                        self.take_failure(&mut caller, failure, false)?;
                        None
                    }
                    Exit::Return(returned) => {
                        match (site_result, returned) {
                            (Some(r), Some((mut held, _))) => {
                                let call_site =
                                    self.unit(caller.unit).tables.calls[call.index()].clone();
                                let bind = |borrow: Borrow| -> Checked<Borrow> {
                                    let (parameter, offset) = match borrow {
                                        Borrow::Parameter(parameter) => (parameter, None),
                                        Borrow::ParameterCell(parameter, offset) => {
                                            (parameter, Some(offset))
                                        }
                                        _ => return Ok(borrow),
                                    };
                                    let argument = call_site
                                        .arguments
                                        .get(&self.unit(caller.unit).pools.call_arguments)
                                        .unwrap_or(&[])
                                        .iter()
                                        .find(|a| a.parameter == parameter)
                                        .ok_or_else(|| {
                                            err(V::BorrowedReturn, "returned view has no argument")
                                        })?;
                                    match argument.value {
                                        ArgumentValue::Value(v) => {
                                            let source = self.get(&caller, v)?;
                                            let leaf = if let Some(offset) = offset {
                                                let Val::Image(image) = &source.val else {
                                                    return fail(
                                                        V::BorrowedReturn,
                                                        "returned cell has no argument image",
                                                    );
                                                };
                                                let Some(Cell::Data { borrow, .. }) =
                                                    image.cells.get(&offset)
                                                else {
                                                    return fail(
                                                        V::BorrowedReturn,
                                                        "returned cell has no argument provenance",
                                                    );
                                                };
                                                *borrow
                                            } else {
                                                None
                                            };
                                            Ok(leaf.or(source.borrow).unwrap_or(Borrow::Caller))
                                        }
                                        ArgumentValue::Address { .. } => fail(
                                            V::BorrowedReturn,
                                            "returned view requires a borrowed argument value",
                                        ),
                                    }
                                };
                                if let Some(borrow) = held.borrow {
                                    held.borrow = Some(bind(borrow)?);
                                }
                                if let Val::Image(image) = &mut held.val {
                                    for cell in image.cells.values_mut() {
                                        if let Cell::Data {
                                            borrow: Some(borrow),
                                            ..
                                        } = cell
                                        {
                                            *borrow = bind(*borrow)?;
                                        }
                                    }
                                }
                                self.define(&mut caller, r, held)?;
                            }
                            (None, None) => {}
                            (None, Some((held, _))) if !self.carries(&held.val) => {}
                            _ => {
                                return fail(
                                    V::Malformed,
                                    "call result arity differs from the return",
                                );
                            }
                        }
                        Some(returning(normal)?)
                    }
                };
                if let Some(normal) = result {
                    self.jump(&mut caller, normal)?;
                }
                self.stack.push(Activity::Frame(caller));
            }
            Some(Activity::Drain(mut d)) => {
                if f.role != Role::Finalizer {
                    return fail(V::Malformed, "a drain resumed by a non-finalizer frame");
                }
                match exit {
                    Exit::RejectTransfer(_) => {
                        return fail(V::FrameImbalance, "finalizer returned private status 3");
                    }
                    Exit::Fail => d.failed = true,
                    Exit::Return(None) => {}
                    Exit::Return(Some(_)) => {
                        return fail(V::Malformed, "a finalizer returned a value");
                    }
                }
                self.stack.push(Activity::Drain(d));
            }
            None if self.in_child() => {
                if matches!(exit, Exit::RejectTransfer(_)) {
                    return fail(V::FrameImbalance, "task entry returned private status 3");
                }
                self.task_exited(matches!(exit, Exit::Fail))?;
            }
            None => {
                self.finished = Some(match exit {
                    Exit::RejectTransfer(_) => {
                        return fail(V::FrameImbalance, "entry returned private status 3");
                    }
                    Exit::Fail => Finished::Failed,
                    Exit::Return(None) => Finished::Returned(None),
                    Exit::Return(Some((held, ty))) => {
                        let ty = self.lir_type(f.unit, ty)?;
                        // The entry transfers its result to the generic-call harness.
                        self.holders.value(&held.val, i64::from(held.credits));
                        Finished::Returned(Some((held, ty.layout, ty.repr)))
                    }
                });
            }
        }
        Ok(())
    }

    fn drain_done(&mut self, d: Drain) -> Checked<()> {
        let mut f = match self.stack.pop() {
            Some(Activity::Frame(f)) => f,
            // A harness drain: the terminal release of roots, or of a child's closure.
            None => return self.task_drained(d.failed),
            Some(Activity::Drain(_)) => {
                return fail(V::Malformed, "a drain finished above a drain");
            }
        };
        let Resume::Drain { after, failure } = std::mem::replace(&mut f.resume, Resume::Running)
        else {
            return fail(
                V::Malformed,
                "drain finished under a frame that is not waiting",
            );
        };
        if let After::Operation { normal } = after {
            if d.failed || normal.is_none() {
                self.take_failure(
                    &mut f,
                    failure.ok_or_else(|| {
                        err(V::Malformed, "operation cleanup without failure edge")
                    })?,
                    false,
                )?;
            } else {
                self.jump(&mut f, normal.unwrap())?;
            }
            self.stack.push(Activity::Frame(f));
            return Ok(());
        }
        let site = self.site(&f);
        let result: Checked<()> = (|| {
            match after {
                After::Operation { .. } => unreachable!(),
                After::Next => {}
                After::Reset { result, token } => {
                    if d.failed {
                        // A failing finalizer disposes storage; no token escapes.
                        self.ledger.tokens[token.0 as usize].consumed = true;
                        let alloc = self.ledger.tokens[token.0 as usize].alloc;
                        self.ledger.free(alloc)?;
                    } else {
                        self.ledger.trace.push(Event::Reset {
                            alloc: self.ledger.tokens[token.0 as usize].alloc,
                            token: Some(token),
                        });
                        self.define_fresh(&mut f, result, Val::Token(Some(token)))?;
                    }
                }
                After::RegionExit { region } => self.exit_region(&mut f, region)?,
            }
            if d.failed {
                let failure =
                    failure.ok_or_else(|| err(V::Malformed, "failing drain without edge"))?;
                self.take_failure(&mut f, failure, false)?;
            } else {
                f.statement += 1;
            }
            Ok(())
        })();
        result.map_err(|mut v| {
            v.site.get_or_insert(site);
            v
        })?;
        self.stack.push(Activity::Frame(f));
        Ok(())
    }

    fn exit_region(&mut self, f: &mut Frame, rt: usize) -> Checked<()> {
        if self.ledger.regions[rt].storage == crate::ledger::sealed::StorageCustody::Scope {
            let inside = self.ledger.regions[rt].allocations.clone();
            if let Some((source, offset, ptr)) =
                self.physical.references.escaping(&self.ledger, &inside)
            {
                return fail(
                    V::RegionEscape,
                    format!(
                        "{source:?}+{offset} references {:?} of a region at its exit",
                        ptr.alloc
                    ),
                );
            }
            for alloc in inside {
                self.ledger.free(alloc)?;
            }
        } else {
            self.ledger.check_sealed_ownership()?;
        }
        self.ledger.regions[rt].live = false;
        f.regions.retain(|r| *r != rt);
        self.ledger.trace.push(Event::RegionExit { region: rt });
        Ok(())
    }

    /// Attach logical occurrences to the physical drop program. Flattened
    /// recipes retain the reverse declaration order of their layout fields;
    /// identical offsets/layouts are consumed in that order, never conflated.
    fn drop_work(
        &self,
        base: Ptr,
        layout: LayoutId,
        steps: &[DropStep],
        variant: Option<u32>,
        bias: u64,
        dead: &[Vec<ProjectionStep<u64>>],
        child_mask: &Option<Vec<bool>>,
    ) -> Checked<Vec<Work>> {
        let record = self.record(layout)?;
        let fields = match variant {
            Some(variant) => {
                &record
                    .tag
                    .as_ref()
                    .and_then(|t| t.variants.iter().find(|v| v.ordinal == variant))
                    .ok_or_else(|| err(V::Malformed, "drop variant has no layout arm"))?
                    .fields
            }
            None => &record.fields,
        };
        let mut pending = Vec::new();
        for field in fields {
            if !hirsel_lir::layout::field_is_borrowed(&self.program.layouts, record, field) {
                let step = match variant {
                    Some(variant) => ProjectionStep::VariantField {
                        variant,
                        field: field.ordinal,
                    },
                    None => ProjectionStep::Field(field.ordinal),
                };
                pending.push((bias + field.offset, field.layout, vec![step]));
            }
        }
        let mut targets = Vec::new();
        while let Some((offset, id, path)) = pending.pop() {
            let child = self.record(id)?;
            targets.push((offset, id, path.clone()));
            if child.finalizer.is_none()
                && !matches!(
                    child.kind,
                    LayoutKind::Inline { .. } | LayoutKind::Variant | LayoutKind::Packed
                )
            {
                for field in &child.fields {
                    if !hirsel_lir::layout::field_is_borrowed(&self.program.layouts, child, field) {
                        let mut path = path.clone();
                        path.push(ProjectionStep::Field(field.ordinal));
                        pending.push((offset + field.offset, field.layout, path));
                    }
                }
            }
        }
        let mut work = Vec::new();
        for step in steps {
            let target = targets.iter().position(|(offset, id, _)| match step {
                DropStep::Child { offset: at, layout } => offset == at && id == layout,
                DropStep::Release { offset: at }
                | DropStep::ReleaseRoot { offset: at }
                | DropStep::ReleaseSealed { offset: at }
                | DropStep::ReleaseReceipt { offset: at }
                | DropStep::Environment { offset: at }
                | DropStep::CloseHandle { offset: at } => self.record(*id).ok().is_some_and(|r| {
                    r.drop.iter().any(|candidate| match (step, candidate) {
                        (DropStep::Release { .. }, DropStep::Release { offset: relative })
                        | (
                            DropStep::ReleaseRoot { .. },
                            DropStep::ReleaseRoot { offset: relative },
                        )
                        | (
                            DropStep::ReleaseSealed { .. },
                            DropStep::ReleaseSealed { offset: relative },
                        )
                        | (
                            DropStep::Environment { .. },
                            DropStep::Environment { offset: relative },
                        )
                        | (
                            DropStep::CloseHandle { .. },
                            DropStep::CloseHandle { offset: relative },
                        ) => offset + relative == *at,
                        _ => false,
                    })
                }),
                DropStep::RepeatReverse { offset: at, .. } => {
                    offset == at
                        && matches!(
                            self.record(*id).map(|r| &r.kind),
                            Ok(LayoutKind::Inline { .. })
                        )
                }
                DropStep::ActiveVariant {
                    tag_offset, form, ..
                } => self.record(*id).ok().is_some_and(|r| match form {
                    TagForm::LiveBit { .. } => {
                        matches!(r.kind, LayoutKind::Environment { .. })
                            && r.fields
                                .last()
                                .is_some_and(|f| offset + f.offset == *tag_offset)
                    }
                    _ => r
                        .tag
                        .as_ref()
                        .is_some_and(|t| offset + t.offset == *tag_offset && t.form == *form),
                }),
                _ => false,
            });
            let (logical_base, owner) = match target {
                Some(index) => {
                    let (_, id, path) = targets.remove(index);
                    let mut ptr = base;
                    for step in path {
                        ptr = self.ledger.project(ptr, 0, step);
                    }
                    (ptr, id)
                }
                None => (base, layout),
            };
            work.push(Work::Step {
                base,
                logical_base,
                layout: owner,
                step: step.clone(),
                dead: dead.to_vec(),
                child_mask: child_mask.clone(),
            });
        }
        Ok(work)
    }

    fn step_drain(&mut self, d: &mut Drain, work: Work) -> Checked<Option<Activity>> {
        match work {
            Work::DropAt { at, layout, mask } => {
                let record = self.record(layout)?;
                // A dynamic flat payload's extent is read from its bytes.
                let size = self.extent_of(layout, at)?;
                let a = self.ledger.check(at)?;
                a.extent.check_range(at.offset, size)?;
                // Under a live mask every field may have moved out (LIR
                // schema revision 5); the fields it keeps are read below.
                if size > 0 && mask.is_none() && !a.extent.any_init(at.offset, size) {
                    return fail(
                        V::UninitializedScan,
                        format!(
                            "drop of uninitialized {layout:?} at {:?}+{}",
                            at.alloc, at.offset
                        ),
                    );
                }
                if let LayoutKind::Environment { movable } = &record.kind {
                    let word = record.fields.last().expect("live word");
                    let bits = self.scan_scalar(at.at(word.offset), 8)?;
                    for (bit, &ordinal) in movable.iter().enumerate() {
                        let field = &record.fields[ordinal as usize];
                        let (all, any) =
                            self.initialized(&a.extent, at.offset + field.offset, field.layout)?;
                        let live = bits & (1u128 << bit) != 0;
                        if (live && !all) || (!live && any) {
                            return fail(
                                V::UninitializedScan,
                                "capture live bit disagrees with its cells",
                            );
                        }
                    }
                }
                d.work.push(Work::Done { at, size, layout });
                let (dead, child_mask) = if matches!(
                    record.kind,
                    LayoutKind::Block { .. } | LayoutKind::Box { .. }
                ) {
                    (Vec::new(), mask)
                } else {
                    // A variant's mask names the active variant's fields.
                    let fields = match (&record.tag, &mask) {
                        (Some(tag), Some(_)) => {
                            let active = self.active_variant(at.at(tag.offset), tag.form)?;
                            tag.variants
                                .iter()
                                .find(|v| v.ordinal == active)
                                .map_or(&[][..], |v| &v.fields[..])
                        }
                        _ => &record.fields[..],
                    };
                    let variant = match (&record.tag, &mask) {
                        (Some(tag), Some(_)) => {
                            Some(self.active_variant(at.at(tag.offset), tag.form)?)
                        }
                        _ => None,
                    };
                    let mut dead = Vec::new();
                    for fl in fields {
                        if !mask_bit(&mask, fl.ordinal as usize) {
                            let step = match variant {
                                Some(variant) => ProjectionStep::VariantField {
                                    variant,
                                    field: fl.ordinal,
                                },
                                None => ProjectionStep::Field(fl.ordinal),
                            };
                            dead.push(self.ledger.path(self.ledger.project(at, 0, step)));
                        }
                    }
                    (dead, None)
                };
                d.work.extend(
                    self.drop_work(at, layout, &record.drop, None, 0, &dead, &child_mask)?
                        .into_iter()
                        .rev(),
                );
            }
            Work::Step {
                base,
                logical_base,
                layout: owner_layout,
                step,
                dead,
                child_mask,
            } => {
                let logical_path = self.ledger.path(logical_base);
                let gated = |_: u64| dead.iter().any(|path| logical_path.starts_with(path));
                match step {
                    DropStep::ReleaseSealed { offset } => {
                        if !gated(offset) {
                            self.release_sealed(base.at(offset), owner_layout, &mut d.work)?;
                        }
                    }
                    DropStep::ReleaseReceipt { offset } => {
                        if !gated(offset) {
                            self.release_receipt(base.at(offset), owner_layout)?;
                        }
                    }
                    DropStep::Finalize => {
                        let rid = self.resource_at(base)?;
                        let res = &mut self.ledger.resources[rid.0 as usize];
                        if res.state != ResourceState::Live {
                            return fail(
                                V::DuplicateFinalizer,
                                format!("finalizer of {rid:?} entered while it is {:?}", res.state),
                            );
                        }
                        let layout = res.layout;
                        let language = self.record(layout)?.finalizer.as_ref().is_some_and(|f| {
                            !f.definition
                                .0
                                .modules
                                .iter()
                                .any(|m| m.as_str() == "$native")
                        });
                        if language {
                            let task = self.tasks.as_ref().map_or(0, |tasks| tasks.current);
                            let events = self.invocation_events.entry(task).or_default();
                            if events.iter().filter(|&&cleanup| cleanup).count() == 64 {
                                self.ledger.resources[rid.0 as usize].state =
                                    ResourceState::FieldsPending;
                                self.record_close_failure("LANG-10", d);
                                d.failed = true;
                                return Ok(None);
                            }
                            events.push(true);
                            // The matching Done follows all fields of this resource.
                            let end = d.work.iter().rposition(|work|
                                matches!(work, Work::Done { at, layout: own, .. } if *at == base && *own == layout))
                                .ok_or_else(|| err(V::Malformed, "resource cleanup has no field scope end"))?;
                            d.work.insert(end, Work::RetireCleanupInvocation(task));
                        }
                        self.ledger.resources[rid.0 as usize].state = ResourceState::Finalizing;
                        self.ledger.counters.finalizers += 1;
                        self.ledger.trace.push(Event::Finalize { resource: rid });
                        d.work.push(Work::Finalized(rid));
                        let finalizer =
                            self.record(layout)?.finalizer.as_ref().ok_or_else(|| {
                                err(V::Malformed, "finalize step of a layout without finalizer")
                            })?;
                        if self.injection_site() {
                            d.failed = true;
                            let definition = finalizer.definition.clone();
                            self.failures.push(ObservedFailure {
                                source: u32::MAX,
                                operation_source: u32::MAX,
                                operation: None,
                                rule: INJECTED_RULE.to_string(),
                                definition,
                                site: (0, 0),
                            });
                            self.ledger.trace.push(Event::Failure {
                                rule: INJECTED_RULE.to_string(),
                            });
                            return Ok(None);
                        }
                        let unit = self.program.finalizer(&finalizer.definition)?;
                        let finalizer_unit = self.unit(unit);
                        let abi =
                            &self.program.layouts.fn_abis[finalizer_unit.header.abi.0 as usize];
                        let expected = if finalizer_unit.header.origin == UnitOrigin::Library {
                            hirsel_core_contract::Passing::Mut
                        } else {
                            hirsel_core_contract::Passing::Borrow
                        };
                        if abi.parameters.len() != 1 || abi.parameters[0].convention != expected {
                            return fail(
                                V::Malformed,
                                "a library finalizer takes mut self; a source finalizer borrows self",
                            );
                        }
                        let mut frame = self.frame(unit, Role::Finalizer, None);
                        if finalizer_unit.header.track_caller
                            || finalizer_unit.header.origin == UnitOrigin::Library
                        {
                            frame.caller = d.caller;
                        }
                        frame.operation = d.operation;
                        let parameters = &self.unit(unit).parameters;
                        let [p] = parameters.as_slice() else {
                            return fail(
                                V::Malformed,
                                "a finalizer takes exactly its borrowed self",
                            );
                        };
                        self.define(
                            &mut frame,
                            *p,
                            Held {
                                val: Val::Ptr(base),
                                credits: 0,
                                borrow: Some(Borrow::Caller),
                            },
                        )?;
                        // The borrowed self is passed by address: a parameter slot, as for
                        // any address-passed parameter, aliases the finalized value.
                        self.bind_parameter_slots(&mut frame, &[Some((base, None))])?;
                        return Ok(Some(Activity::Frame(Box::new(frame))));
                    }
                    // A root's count is exact in the ledger, so its release is the cell's.
                    DropStep::Release { offset } | DropStep::ReleaseRoot { offset } => {
                        if !gated(offset) {
                            self.release_cell(base.at(offset), child_mask, &mut d.work)?;
                        }
                    }
                    // A function reference or an empty closure stores no environment (the
                    // native static empty one, which a release leaves alone).
                    DropStep::Environment { offset } => {
                        if !gated(offset) && !self.environment_absent(base.at(offset))? {
                            self.release_cell(base.at(offset), child_mask, &mut d.work)?;
                        }
                    }
                    // The dying root's frozen graph has ordinary unique counts again
                    // before the following steps release it.
                    DropStep::Thaw { offset, layout: _ } => {
                        let a = self.ledger.check(base)?;
                        if offset != 0
                            || base.offset != 0
                            || a.count != Some(Count::Root(0))
                            || a.state != AllocState::Dying
                        {
                            return fail(V::Malformed, "a thaw step outside a dying Shared root");
                        }
                        self.refreeze(base.alloc, Count::Frozen { copies: 0 }, Count::Finite(1))?;
                    }
                    DropStep::Child { offset, layout } => {
                        if !gated(offset) {
                            d.work.push(Work::DropAt {
                                at: logical_base.at(offset),
                                layout,
                                mask: None,
                            });
                        }
                    }
                    // An inline variant child a live mask leaves dead moved out whole:
                    // its tag is not read, as the runtime skips the tag's span.
                    DropStep::ActiveVariant { tag_offset, .. } if gated(tag_offset) => {}
                    DropStep::ActiveVariant {
                        tag_offset,
                        form,
                        variants,
                    } => {
                        let tag = self
                            .active_variant(base.at(tag_offset), form)
                            .map_err(|v| {
                                if v.kind == V::UninitializedRead {
                                    err(V::UninitializedScan, v.message)
                                } else {
                                    v
                                }
                            })?;
                        if let Some((_, steps)) = variants.iter().find(|(v, _)| *v == tag) {
                            let owner = self.record(owner_layout)?;
                            let (variant, local_tag) = match form {
                                TagForm::LiveBit { .. } => (
                                    None,
                                    owner
                                        .fields
                                        .last()
                                        .ok_or_else(|| {
                                            err(V::Malformed, "capture guard has no live word")
                                        })?
                                        .offset,
                                ),
                                _ => (
                                    Some(tag),
                                    owner
                                        .tag
                                        .as_ref()
                                        .ok_or_else(|| {
                                            err(V::Malformed, "variant guard has no tag")
                                        })?
                                        .offset,
                                ),
                            };
                            let bias = tag_offset - local_tag;
                            d.work.extend(
                                self.drop_work(
                                    logical_base,
                                    owner_layout,
                                    steps,
                                    variant,
                                    bias,
                                    &dead,
                                    &None,
                                )?
                                .into_iter()
                                .rev(),
                            );
                        }
                    }
                    DropStep::ElementsReverse {
                        count_offset,
                        offset,
                        stride,
                        element,
                    } => {
                        let count = self.scan_scalar(base.at(count_offset), 8)? as u64;
                        for i in 0..count {
                            d.work.push(Work::DropAt {
                                at: self.ledger.project(
                                    logical_base,
                                    offset + i * stride,
                                    ProjectionStep::Index(i),
                                ),
                                layout: element,
                                mask: None,
                            });
                        }
                    }
                    // A fixed inline repeat is one field: pushed low to high, so the
                    // highest index drops first.
                    DropStep::RepeatReverse {
                        offset,
                        count,
                        stride,
                        element,
                    } => {
                        if !gated(offset) {
                            for i in 0..count {
                                d.work.push(Work::DropAt {
                                    at: self.ledger.project(
                                        logical_base,
                                        offset + i * stride,
                                        ProjectionStep::Index(i),
                                    ),
                                    layout: element,
                                    mask: None,
                                });
                            }
                        }
                    }
                    DropStep::ColumnsReverse(columns) => {
                        if !gated(columns.data_offset) {
                            let count = self.scan_scalar(base.at(columns.count_offset), 8)? as u64;
                            let capacity =
                                self.scan_scalar(base.at(columns.capacity_offset), 8)? as u64;
                            if count > capacity || capacity % columns.capacity_quantum != 0 {
                                return fail(V::OutOfExtent, "invalid column drop capacity");
                            }
                            if columns.tag.is_none()
                                && columns.columns.iter().any(|c| c.arm.is_some())
                            {
                                return fail(V::Malformed, "an arm column without a tag column");
                            }
                            // Pushed low to high: rows drop high to low, and each
                            // row's fields in reverse declaration order; a variant row
                            // only its active arm's payload.
                            for i in 0..count {
                                let active = match columns.tag {
                                    Some(tag) => {
                                        let width = soa_tag_width(tag.scalar);
                                        let at = capacity
                                            .checked_mul(tag.prefix)
                                            .and_then(|n| n.checked_add(columns.data_offset))
                                            .and_then(|n| {
                                                i.checked_mul(width).and_then(|j| n.checked_add(j))
                                            })
                                            .ok_or_else(|| {
                                                err(V::OutOfExtent, "column drop offset overflow")
                                            })?;
                                        Some(self.scan_scalar(base.at(at), width)? as u64)
                                    }
                                    None => None,
                                };
                                for column in &columns.columns {
                                    if let Some(active) = active
                                        && column.arm.map(u64::from) != Some(active)
                                    {
                                        continue;
                                    }
                                    let offset = capacity
                                        .checked_mul(column.prefix)
                                        .and_then(|n| n.checked_add(columns.data_offset))
                                        .and_then(|n| {
                                            i.checked_mul(column.stride)
                                                .and_then(|j| n.checked_add(j))
                                        })
                                        .ok_or_else(|| {
                                            err(V::OutOfExtent, "column drop offset overflow")
                                        })?;
                                    let row = self.ledger.project(
                                        logical_base,
                                        offset,
                                        ProjectionStep::Index(i),
                                    );
                                    let at = self.ledger.project(
                                        row,
                                        0,
                                        ProjectionStep::Field(column.field),
                                    );
                                    d.work.push(Work::DropAt {
                                        at,
                                        layout: column.layout,
                                        mask: None,
                                    });
                                }
                            }
                        }
                    }
                    DropStep::CloseHandle { offset } => {
                        if !gated(offset) {
                            let h = self.scan_scalar(base.at(offset), 8)? as u64;
                            match self.ledger.handles.get_mut(&h) {
                                Some((state @ crate::ledger::HandleState::Open, _)) => {
                                    *state = crate::ledger::HandleState::Closed
                                }
                                Some(_) => {
                                    return fail(
                                        V::DoubleClose,
                                        format!("handle {h} closed twice"),
                                    );
                                }
                                None => {
                                    return fail(V::ForgedPointer, format!("unknown handle {h}"));
                                }
                            }
                            if h & tasks::TASK_SET != 0 {
                                // A task set closes in the task scheduler (CONC-2).
                                self.close_task_set(h)?;
                            } else if h & channels::ENDPOINT != 0 {
                                // A channel endpoint closes in the task scheduler (CONC-6).
                                self.close_endpoint(h, &mut d.work)?;
                            } else if h & sync::MUTEX != 0 {
                                self.close_mutex(h, &mut d.work)?;
                            } else if let Some(rule) = self
                                .ops
                                .close_handle(h, &mut self.output)
                                .map_err(|message| err(V::Malformed, message))?
                            {
                                self.record_close_failure(&rule, d);
                                d.failed = true;
                            }
                            self.ledger.get_mut(base.alloc).extent.take(
                                base.offset + offset,
                                8,
                                FREE_POISON,
                                Some((&[], &[])),
                            );
                        }
                    }
                }
            }
            Work::SealedStorageDeath(alloc) => {
                if self.ledger.get(alloc).state != AllocState::Live {
                    return fail(V::UseAfterFree, "sealed storage was drained more than once");
                }
                self.ledger.free(alloc)?;
            }
            Work::SealedOwnerDone { owner, control } => {
                self.ledger.free(control.alloc)?;
                self.ledger.finish_sealed_drop(owner)?;
            }
            Work::RetireCleanupInvocation(task) => {
                if self.invocation_events.get_mut(&task).and_then(Vec::pop) != Some(true) {
                    return fail(
                        V::FrameImbalance,
                        "cleanup invocation retirement without entry",
                    );
                }
            }
            Work::Finalized(rid) => {
                self.ledger.resources[rid.0 as usize].state = ResourceState::FieldsPending;
            }
            Work::Done { at, size, layout } => {
                self.ledger.check(at)?;
                let owned = self.resources_of(layout)?;
                let path = self.ledger.path(at);
                let rest = self.ledger.get_mut(at.alloc).extent.take(
                    at.offset,
                    size,
                    FREE_POISON,
                    Some((&path, &owned)),
                );
                if let Some((offset, _)) = rest
                    .cells
                    .iter()
                    .find(|(_, c)| matches!(c, Cell::Data { counted: true, .. }))
                {
                    return fail(
                        V::LeakedCredit,
                        format!(
                            "drop of {:?}+{} left a holder at offset {offset}",
                            at.alloc, at.offset
                        ),
                    );
                }
                self.ledger.end_resources(&rest)?;
            }
            Work::Death { alloc, raw } => {
                self.lazy_iterators.remove(alloc.0 as usize);
                self.runtime_iterators.remove(alloc.0 as usize);
                match raw {
                    None => self.ledger.free(alloc)?,
                    Some(token) => {
                        let size = self.ledger.get(alloc).extent.len();
                        let rest =
                            self.ledger
                                .get_mut(alloc)
                                .extent
                                .take(0, size, FREE_POISON, None);
                        if !rest.cells.is_empty() {
                            return fail(
                                V::LeakedCredit,
                                format!("reset of {alloc:?} left a holder"),
                            );
                        }
                        let a = self.ledger.get_mut(alloc);
                        a.state = AllocState::Raw(token);
                        a.layout = None;
                        a.count = None;
                    }
                }
            }
        }
        Ok(None)
    }

    /// A scalar a drop program reads: uninitialized bytes are a scan violation.
    fn scan_scalar(&self, at: Ptr, size: u64) -> Checked<u128> {
        self.read_scalar(at, size).map_err(|v| {
            if v.kind == V::UninitializedRead {
                err(V::UninitializedScan, v.message)
            } else {
                v
            }
        })
    }

    /// A callable or iterator without captures stores an initialized null
    /// environment word beside its checked code word.
    fn environment_absent(&self, at: Ptr) -> Checked<bool> {
        let word = self.word;
        let a = self.ledger.check(at)?;
        a.extent.check_range(at.offset, word)?;
        let code = at
            .offset
            .checked_sub(word)
            .and_then(|o| a.extent.cells.get(&o));
        let next = a.extent.cells.get(&(at.offset + word));
        Ok(
            (matches!(code, Some(Cell::Code(_))) || matches!(next, Some(Cell::Code(_))))
                && a.extent.all_init(at.offset, word)
                && !a.extent.cells.contains_key(&at.offset)
                && a.extent.read_bytes(at.offset, word).iter().all(|b| *b == 0),
        )
    }

    fn release_cell(
        &mut self,
        at: Ptr,
        mask: Option<Vec<bool>>,
        work: &mut Vec<Work>,
    ) -> Checked<()> {
        let cell = self.read_cell(at).map_err(|v| {
            if v.kind == V::UninitializedRead {
                err(V::UninitializedScan, format!("drop program: {}", v.message))
            } else {
                v
            }
        })?;
        let Cell::Data { ptr, counted, .. } = cell else {
            return fail(V::TypeConfusion, "drop program released a code word");
        };
        let word = self.word;
        self.ledger
            .get_mut(at.alloc)
            .extent
            .take(at.offset, word, FREE_POISON, Some((&[], &[])));
        if !counted {
            if self.counted_target(ptr) {
                return fail(
                    V::CreditUnderflow,
                    format!(
                        "drop program released {:?} through a reference without credit",
                        ptr.alloc
                    ),
                );
            }
            // A reference into immortal or uncounted storage carries nothing.
            return Ok(());
        }
        self.decrement(ptr, mask, work)
    }

    // ---- Terminal audit ----------------------------------------------------

    /// Observe the result, drop harness roots, drain cleanup, then require
    /// that nothing unexplained remains.
    fn prepare_finish(&mut self) -> Checked<()> {
        let finished = self
            .finished
            .take()
            .ok_or_else(|| err(V::Malformed, "the entry did not finish"))?;
        if let Finished::Returned(Some((held, _, _))) = &finished {
            // Move the harness credit into the temporary drained below.
            self.holders.value(&held.val, -i64::from(held.credits));
        }
        let returned = match &finished {
            Finished::Returned(Some((held, layout, repr))) => {
                Some(self.observe_val(&held.val, *layout, *repr)?)
            }
            Finished::Returned(None) => Some(Observed::Unit),
            Finished::Failed => None,
        };
        let observation = Observation {
            schema_version: OBSERVATION_SCHEMA_VERSION,
            returned,
            output: self.output.clone(),
            failures: self.failures.clone(),
            blocked: false,
            signaled: None,
        };
        self.terminal = Some(observation);
        if self.ended {
            return Ok(());
        }
        if let Finished::Returned(Some((held, layout, _))) = finished
            && held.credits > 0
            && self.carries(&held.val)
        {
            let temp = self.temporary(layout)?;
            self.write_value(
                temp,
                layout,
                Moved {
                    val: held.val,
                    credited: true,
                    borrow: None,
                },
            )?;
            self.stack.push(Activity::Drain(Drain {
                caller: None,
                operation: None,
                work: vec![
                    Work::Death {
                        alloc: temp.alloc,
                        raw: None,
                    },
                    Work::DropAt {
                        at: temp,
                        layout,
                        mask: None,
                    },
                ],
                failed: false,
            }));
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Checked<(Observation, crate::History<Exemption>)> {
        if self.terminal.is_none() {
            self.prepare_finish()?;
        }
        self.run()?;
        let mut observation = self.terminal.take().expect("prepared terminal observation");
        // Returned-value finalizers may fail during the harness drain.
        observation.failures = self.failures.clone();
        observation.output = self.output.clone();
        // A run that ended where it stood is discarded whole, as its runtime context is.
        if !self.ended {
            self.audit()?;
        }
        Ok((observation, self.ledger.exemptions.clone()))
    }

    fn audit(&mut self) -> Checked<()> {
        self.audit_receipts()?;
        self.ledger.audit_sealed_owners()?;
        self.holders.sync(&mut self.ledger);
        self.cross_check_holders()?;
        let l = &self.ledger;
        if let Some((i, _)) = self.physical.pending_tokens.indexed().next() {
            return fail(
                V::LeakedToken,
                format!("TokenId({i}) was never reused or disposed"),
            );
        }
        for (r, _) in self.physical.live_regions.indexed() {
            let region = &l.regions[r];
            if region.live {
                return fail(
                    V::LiveRegion,
                    format!("region {r} is live at the terminal audit"),
                );
            }
        }
        for (i, _) in self.physical.open_resources.indexed() {
            let r = &l.resources[i];
            if r.state != ResourceState::Dead && !r.forgotten {
                return fail(
                    V::OpenResource,
                    format!("ResourceId({i}) is {:?} at the terminal audit", r.state),
                );
            }
        }
        if let Some((h, _)) = self.physical.open_handles.indexed().next() {
            return fail(
                V::OpenHandle,
                format!("handle {h} is open at the terminal audit"),
            );
        }
        for (i, _) in self.physical.terminal_allocations.indexed() {
            fork::scanned(1);
            let a = &l.allocations[i];
            let id = AllocId(i as u32);
            match a.state {
                AllocState::Dead => {}
                AllocState::Raw(t) => {
                    return fail(V::LeakedToken, format!("{id:?} is raw storage of {t:?}"));
                }
                AllocState::Live | AllocState::Dying => {
                    return fail(
                        V::UnexplainedAllocation,
                        format!(
                            "{id:?} ({}) is live at the terminal audit with count {:?}",
                            crate::ledger::owner_name(a.owner),
                            a.count
                        ),
                    );
                }
            }
        }
        self.physical.counts(&self.ledger)
    }

    fn observe_val(&self, val: &Val, layout: LayoutId, repr: ValueRepr) -> Checked<Observed> {
        match val {
            Val::Unit => Ok(Observed::Unit),
            Val::Bits {
                scalar: ScalarKind::Bool,
                bits,
            } => Ok(Observed::Bool(*bits == 1)),
            Val::Bits {
                scalar: ScalarKind::F32,
                bits,
            } => Ok(Observed::F32(*bits as u32)),
            Val::Bits {
                scalar: ScalarKind::F64,
                bits,
            } => Ok(Observed::F64(*bits as u64)),
            Val::Bits { scalar, bits } => Ok(Observed::Integer {
                scalar: *scalar,
                bits: *bits,
            }),
            Val::Callable { .. } => Ok(Observed::NonComparableFunction),
            Val::Handle(h) => Ok(Observed::Handle(*h)),
            Val::Ptr(p) if matches!(repr, ValueRepr::Block | ValueRepr::Uncounted) => {
                let payload = match self.record(layout)?.kind {
                    LayoutKind::Block { payload } | LayoutKind::Box { payload } => payload,
                    _ => layout,
                };
                let a = self.ledger.check(*p)?;
                Ok(Observed::Block(Box::new(
                    self.observe_at(&a.extent, p.offset, payload, 0)?,
                )))
            }
            Val::Image(e) => self.observe_at(e, 0, layout, 0),
            _ => Ok(Observed::Opaque),
        }
    }

    fn observe_at(
        &self,
        e: &Extent,
        offset: u64,
        layout: LayoutId,
        depth: u32,
    ) -> Checked<Observed> {
        if depth > 10_000 {
            return fail(V::Unsupported, "observation depth limit");
        }
        let record = self.record(layout)?;
        // Variable payloads have no fixed storage size. Their existing checked
        // allocation bounds the opaque observation; cleanup still audits it.
        let size = record
            .size
            .storage()
            .unwrap_or_else(|| e.len().saturating_sub(offset));
        e.check_range(offset, size)?;
        let fields =
            |list: &[hirsel_lir::layout_types::FieldLayout]| -> Checked<Vec<Option<Observed>>> {
                let mut sorted: Vec<_> = list.iter().collect();
                sorted.sort_by_key(|f| f.ordinal);
                sorted
                    .into_iter()
                    .map(|fl| {
                        let s = self.size(fl.layout)?;
                        if s > 0 && !e.any_init(offset + fl.offset, s) {
                            Ok(None)
                        } else {
                            self.observe_at(e, offset + fl.offset, fl.layout, depth + 1)
                                .map(Some)
                        }
                    })
                    .collect()
            };
        Ok(match &record.kind {
            LayoutKind::Unit => Observed::Unit,
            LayoutKind::Scalar(k) => {
                if !e.all_init(offset, size) {
                    return fail(V::UninitializedRead, "observed scalar is not initialized");
                }
                let mut bytes = [0u8; 16];
                bytes[..size as usize].copy_from_slice(e.read_bytes(offset, size));
                let bits = u128::from_le_bytes(bytes);
                match k {
                    ScalarKind::Bool => Observed::Bool(bits == 1),
                    ScalarKind::F32 => Observed::F32(bits as u32),
                    ScalarKind::F64 => Observed::F64(bits as u64),
                    ScalarKind::Pointer => Observed::Opaque,
                    k => Observed::Integer { scalar: *k, bits },
                }
            }
            LayoutKind::Aggregate | LayoutKind::Resource => {
                Observed::Aggregate(fields(&record.fields)?)
            }
            LayoutKind::Variant | LayoutKind::Packed => {
                let tag_layout = record
                    .tag
                    .as_ref()
                    .ok_or_else(|| err(V::Malformed, "variant layout without tag"))?;
                let tag = e.active(offset + tag_layout.offset, tag_layout.form, self.word)?;
                let variant = tag_layout
                    .variants
                    .iter()
                    .find(|v| v.ordinal == tag)
                    .ok_or_else(|| err(V::Malformed, "observed unknown variant"))?;
                Observed::Variant {
                    tag,
                    fields: fields(&variant.fields)?,
                }
            }
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                match e.cells.get(&offset) {
                    Some(Cell::Data { ptr, .. }) => {
                        let a = self.ledger.check(*ptr)?;
                        Observed::Block(Box::new(self.observe_at(
                            &a.extent,
                            ptr.offset,
                            *payload,
                            depth + 1,
                        )?))
                    }
                    _ => return fail(V::ForgedPointer, "observed block reference is not a handle"),
                }
            }
            LayoutKind::Inline { element, count } => {
                let stride = self.size(*element)?;
                Observed::Aggregate(
                    (0..*count)
                        .map(|i| {
                            self.observe_at(e, offset + i * stride, *element, depth + 1)
                                .map(Some)
                        })
                        .collect::<Checked<_>>()?,
                )
            }
            LayoutKind::Callable { .. } | LayoutKind::Iterator { .. } => {
                Observed::NonComparableFunction
            }
            LayoutKind::Handle | LayoutKind::HostCapability { .. } => {
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(e.read_bytes(offset, 8));
                Observed::Handle(u64::from_le_bytes(bytes))
            }
            _ => Observed::Opaque,
        })
    }
}

/// The bytes of an `@soa` tag column's ordinal.
fn soa_tag_width(scalar: ScalarKind) -> u64 {
    match scalar {
        ScalarKind::U8 => 1,
        ScalarKind::U16 => 2,
        _ => 4,
    }
}

#[cfg(test)]
mod sealed_tests;

#[cfg(test)]
mod sealed_read_tests;

#[cfg(test)]
mod sealed_drop_tests;

#[cfg(test)]
mod sealed_seal_tests;

#[cfg(test)]
mod sealed_graph_tests;
