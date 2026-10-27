//! The executed link set, harness inputs and configuration.
use crate::violation::{Checked, ViolationKind as V, err, fail};
use hirsel_codec::{DecodeError, Reader, Snapshot, Writer};
use hirsel_core_contract::OperationId;
use hirsel_lir::ir::{Callee, LirUnit, Symbol, SymbolKind};
use hirsel_lir::layout_types::{LayoutTable, ScalarKind, TargetSpec};
use hirsel_lir::stage::{Stage, StageKind};
use hirsel_lir::statics::StaticData;
use hirsel_syntax::{ContentHash, DefPath};

/// Every unit of one executable plus its layout table and static data. A
/// finalizer unit's header names the finalizer's definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Program<S> {
    pub target: TargetSpec,
    pub layouts: LayoutTable,
    pub units: Vec<LirUnit<S>>,
    pub statics: Vec<StaticData>,
    /// Independently reconstructed from validated TIR, outside candidate units.
    /// An empty map denotes a raw component fixture without source proof.
    pub invocation_bodies: Vec<hirsel_lir::invocations::ReferenceBody>,
    pub pretransfer_adapters: Vec<hirsel_lir::pretransfer::AdapterReference>,
    pub invocations:
        std::collections::BTreeMap<DefPath, Vec<hirsel_lir::invocations::ReferenceInvocation>>,
}

impl<S: Stage> Program<S> {
    /// Referential integrity of every unit against the table, and that the
    /// stage is one the executor runs: the post-RC stages Rc, Reuse and Ready.
    pub(crate) fn check(&self) -> Checked<()> {
        if !matches!(S::KIND, StageKind::Rc | StageKind::Reuse | StageKind::Ready) {
            return fail(
                V::Malformed,
                format!("the heap executor runs post-RC LIR, not {:?}", S::KIND),
            );
        }
        if self.layouts.target != self.target.digest() {
            return fail(V::Malformed, "layout table names another target");
        }
        for abi in &self.layouts.fn_abis {
            hirsel_lir::abi::check_receipt_contract(abi, &self.layouts)
                .map_err(|e| err(V::Malformed, e))?;
            if abi.receipts.as_ref().is_some_and(|contract| {
                !contract.captures.is_empty()
                    || contract.result.is_some()
                    || contract
                        .parameters
                        .iter()
                        .any(|p| p.mode == hirsel_lir::sealed_payload_source::ReceiptMode::MutSlot)
            }) {
                return fail(
                    V::Unsupported,
                    "H mutable, result or capture receipt custody is not implemented",
                );
            }
        }
        for data in &self.statics {
            data.validate(&self.layouts, u64::from(self.target.pointer_bytes))
                .map_err(|e| err(V::Malformed, e))?;
            for function in data.blocks.iter().flat_map(|block| &block.functions) {
                self.function(&function.symbol)?;
            }
        }
        for unit in &self.units {
            let abi = self
                .layouts
                .fn_abis
                .get(unit.header.abi.0 as usize)
                .ok_or_else(|| err(V::Malformed, "receipt unit ABI is missing"))?;
            if let Some(entry) = &unit.tables.receipt_entry {
                entry
                    .check(abi, unit, &self.layouts)
                    .map_err(|e| err(V::Malformed, e))?;
            } else if abi.receipts.is_some()
                || unit.slots.iter().any(|slot| {
                    matches!(
                        slot.kind,
                        hirsel_lir::ir::SlotKind::ReceiptParameter(_)
                            | hirsel_lir::ir::SlotKind::ReceiptCapture(_)
                            | hirsel_lir::ir::SlotKind::ReceiptResult
                    )
                })
            {
                return fail(V::Malformed, "receipt ABI lacks entry custody");
            }
            crate::pretransfer::audit(unit, &self.layouts, &self.pretransfer_adapters)?;
            if !self.invocations.is_empty()
                && (unit.header.origin == hirsel_lir::ir::UnitOrigin::Source
                    || self.invocation_bodies.iter().any(|r| {
                        r.definition == unit.header.definition && r.instance == unit.header.instance
                    }))
            {
                let reference = self
                    .invocations
                    .get(&unit.header.definition)
                    .ok_or_else(|| {
                        err(V::FrameImbalance, "source invocation reference is missing")
                    })?;
                let mut bodies = self.invocation_bodies.iter().filter(|r| {
                    r.definition == unit.header.definition && r.instance == unit.header.instance
                });
                let selected = bodies.next().ok_or_else(|| {
                    err(
                        V::FrameImbalance,
                        "independent source body selection is missing",
                    )
                })?;
                if bodies.next().is_some() {
                    return fail(V::FrameImbalance, "duplicate independent body selection");
                }
                crate::invocations::audit(
                    unit,
                    reference,
                    selected.body,
                    &self.invocation_bodies,
                    &self.layouts,
                )?;
            }
            if unit.header.origin == hirsel_lir::ir::UnitOrigin::Library {
                crate::invocations::audit_library(unit, &self.invocation_bodies, &self.layouts)?;
            }
            for plan in &unit.tables.structural {
                plan.check(&self.layouts)
                    .map_err(|e| err(V::Malformed, e))?;
            }
            unit.check_identity(&self.layouts).map_err(|e| {
                err(
                    V::Malformed,
                    format!("unit {:?}: {e:?}", unit.header.definition),
                )
            })?;
        }
        Ok(())
    }

    /// The unit a call or closure symbol names.
    pub(crate) fn function(&self, symbol: &Symbol) -> Checked<usize> {
        if symbol.kind != SymbolKind::Function {
            return fail(V::Malformed, format!("call of a {:?} symbol", symbol.kind));
        }
        let mut found = self.units.iter().enumerate().filter(|(_, u)| {
            u.header.definition == symbol.definition
                && symbol.instance.is_none_or(|i| i == u.header.instance)
        });
        match (found.next(), found.next()) {
            (Some((index, _)), None) => Ok(index),
            (None, _) => fail(V::Malformed, format!("no unit for {:?}", symbol.definition)),
            (Some(_), Some(_)) => fail(
                V::Malformed,
                format!("ambiguous symbol {:?}", symbol.definition),
            ),
        }
    }

    /// The unit implementing a resource layout's finalizer.
    pub(crate) fn finalizer(&self, definition: &DefPath) -> Checked<usize> {
        self.units
            .iter()
            .position(|u| &u.header.definition == definition)
            .ok_or_else(|| {
                err(
                    V::Malformed,
                    format!("no finalizer unit for {definition:?}"),
                )
            })
    }

    /// The unit to start from.
    /// The entry unit of a definition. A closure body shares its enclosing
    /// definition's path. Source anchors identify the TIR body, including
    /// contract bodies whose closure construction was eliminated. Units without
    /// source anchors retain the symbol-based convention used by IR fixtures.
    pub(crate) fn entry(&self, definition: &DefPath) -> Checked<usize> {
        let closures: Vec<(&DefPath, Option<ContentHash>)> = self
            .units
            .iter()
            .flat_map(|u| {
                // Closure bodies: a closure's own code, and each bound
                // call's instance (LIR schema revision 9).
                let codes = u
                    .tables
                    .closures
                    .iter()
                    .filter_map(|c| c.code.map(|c| c.symbol));
                let bound = u.tables.calls.iter().filter_map(|c| match c.callee {
                    Callee::Bound { code, .. } => Some(code),
                    _ => None,
                });
                codes
                    .chain(bound)
                    .filter_map(|s| u.tables.symbols.get(s.index()))
                    .map(|s| (&s.definition, s.instance))
            })
            .collect();
        let mut found = self.units.iter().enumerate().filter(|(_, u)| {
            u.header.definition == *definition
                && u.tables.source_anchors.iter().all(|anchor| {
                    // Cleanup-generated anchors complement the source body ID.
                    anchor.body == 0 || anchor.body == u32::MAX
                })
                && !closures.iter().any(|(d, i)| {
                    **d == u.header.definition && i.is_none_or(|i| i == u.header.instance)
                })
        });
        match (found.next(), found.next()) {
            (Some((index, _)), None) => Ok(index),
            (None, _) => fail(V::Malformed, format!("no unit for {definition:?}")),
            (Some(_), Some(_)) => fail(V::Malformed, format!("ambiguous entry {definition:?}")),
        }
    }
}

/// The canonical text of a program is the native snapshot format: the target,
/// the layout table, each unit and the static data.
impl<S: Stage> Snapshot for Program<S> {
    fn write(&self, w: &mut Writer) {
        w.open("heapcheck-program-invocations-v3");
        self.target.write(w);
        self.layouts.write(w);
        self.units.write(w);
        self.statics.write(w);
        self.invocation_bodies.write(w);
        self.pretransfer_adapters.write(w);
        w.open("invocations");
        for (definition, sites) in &self.invocations {
            w.open("definition");
            definition.write(w);
            sites.write(w);
            w.close();
        }
        w.close();
        w.close();
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.open("heapcheck-program-invocations-v3")?;
        let program = Self {
            target: TargetSpec::read(r)?,
            layouts: LayoutTable::read(r)?,
            units: Vec::read(r)?,
            statics: Vec::read(r)?,
            invocation_bodies: Vec::read(r)?,
            pretransfer_adapters: Vec::read(r)?,
            invocations: read_invocations(r)?,
        };
        r.close()?;
        Ok(program)
    }
}

fn read_invocations(
    r: &mut Reader<'_>,
) -> Result<
    std::collections::BTreeMap<DefPath, Vec<hirsel_lir::invocations::ReferenceInvocation>>,
    DecodeError,
> {
    r.open("invocations")?;
    let mut result = std::collections::BTreeMap::new();
    while !r.at_close() {
        r.open("definition")?;
        let definition = DefPath::read(r)?;
        let sites = Vec::read(r)?;
        r.close()?;
        if result.insert(definition, sites).is_some() {
            return Err(r.error("duplicate invocation reference definition"));
        }
    }
    r.close()?;
    Ok(result)
}

/// A harness root passed to the entry: only plain values. Heap inputs are
/// built by the program itself, so every block is created under the ledger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arg {
    Unit,
    Bits {
        scalar: ScalarKind,
        bits: u128,
    },
    /// A harness-owned context handle (`Config::context_handles`).
    Handle(u64),
}

/// A plain operand or result of a registry operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpValue {
    /// A runtime-built iterator of pointer-free semantic items. H owns its environment.
    Iterator(Vec<OpValue>),
    /// Model-owned plain cursor state, advanced only when the program pulls.
    LazyIterator(Box<OpValue>),
    /// Pointer-free fields in declaration order, including nested records.
    Aggregate(Vec<OpValue>),
    /// Pointer-free elements of an aggregate sequence or inline array.
    Sequence(Vec<OpValue>),
    Unit,
    Bits {
        scalar: ScalarKind,
        bits: u128,
    },
    /// Return a taken handle without closing or reopening its identity.
    TakenHandle {
        argument: u32,
    },
    /// A newly opened run-context handle.
    OpenHandle(u64),
    /// A handle the program holds, as an operand; as a result, a plain
    /// capability identity that is never closed (`Stdout`).
    Handle(u64),
    /// An operand: the bytes a String or Bytes slice shows.
    Bytes(Vec<u8>),
    /// An operand: the elements of a borrowed block of a scalar sequence (an
    /// `Array` or `List` of scalars), each as its scalar's bits.
    Elements {
        scalar: ScalarKind,
        bits: Vec<u128>,
    },
    /// A result: fresh Sequence[u8] storage allocated and audited by the executor.
    ByteSequence(Vec<u8>),
    /// A result range relative to an input slice. Borrow adds one owner credit;
    /// Take transfers its credit exactly once, without a copy.
    Slice {
        argument: u32,
        start: u64,
        length: u64,
    },
    /// A result: one payload field of an inline variant, at its declared ordinal.
    Variant {
        tag: u32,
        value: Box<OpValue>,
    },
    /// A result: a pointer-free inline value as the bytes of its layout,
    /// every byte initialized (a `Result` of plain parts).
    Plain(Vec<u8>),
    /// A borrowed Unicode cursor: the slice bytes and its relative position.
    /// The executor retains the original checked owner; the model never sees a pointer.
    ViewCursor {
        bytes: Vec<u8>,
        position: u64,
    },
    /// A Some(String) result sharing a range of an input cursor's view.
    SomeSlice {
        argument: u32,
        start: u64,
        length: u64,
    },
}

#[derive(Clone, Debug)]
pub enum OpOutcome {
    /// Completion with taken operands consumed by the external operation itself.
    /// Ordinals must name Take arguments; H removes their storage and closes handle IDs.
    Complete {
        result: Result<OpValue, String>,
        consumed: Vec<u32>,
    },
    Value(OpValue),
    /// A result plus updates to exclusive arguments, installed before returning.
    Updated {
        value: OpValue,
        arguments: Vec<(u32, OpValue)>,
    },
    /// A deterministic language failure with its rule.
    Fail(String),
    /// A timer wait (`clock.sleep_until`) on monotonic clock `clock` that is not yet due: its
    /// task suspends until the reading reaches `until`, then the operation is performed again
    /// (`call`), or completes `canceled` once its task is canceled first.
    Wait {
        clock: u64,
        until: u64,
    },
    /// An operation whose scripted source cannot answer yet (a stalled peer, a child at an
    /// `Expect`): its task suspends, and each poll performs it again until it completes, or it
    /// completes `canceled` once its task is canceled. It is no event source (CONC-8).
    Pending,
    /// An operation that races a deadline of monotonic clock `clock` at `at` (`proc.run`):
    /// it ends as `completed` unless the deadline fires first, then as `timed_out`.
    Deadline {
        clock: u64,
        at: u64,
        completed: Box<OpOutcome>,
        timed_out: Box<OpOutcome>,
    },
    /// The model has no implementation: the run is an `Unsupported` violation.
    Unsupported,
}

/// What an idle point's signal delivery did ([`Operations::deliver_signal`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalDelivery {
    /// No scripted arrival is left.
    None,
    /// The arrival is pending: the parked tasks are polled again.
    Pending,
    /// The arrival of this `SignalKind` ordinal takes the default action, ending the run.
    Default(u32),
}

/// Simulation state can be forked; host capabilities cannot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationDomain {
    Host,
    Simulation,
}

/// The harness's model of registry operations (Io and library leaves). It owns
/// no heap storage: operands are plain values.
pub trait Operations {
    /// Host models are never cloned. Simulations must opt in explicitly.
    fn domain(&self) -> OperationDomain {
        OperationDomain::Host
    }
    /// An exact independent copy of deterministic simulation state.
    fn fork_simulation(&self) -> Option<Box<dyn Operations>> {
        None
    }
    /// Resolve an inert Path ID in this run's semantic intern table.
    fn path_text(&self, _id: u64) -> Option<&str> {
        None
    }
    /// Intern `text` as `path.of` does, for a decoded Path.
    fn intern_path(&mut self, _text: &str) -> Option<u64> {
        None
    }

    /// The one-based line and column of a failure site, a byte range of
    /// `definition`'s source, for `task.isolate`'s `Failure` (FAIL-2).
    fn failure_position(&self, _definition: &DefPath, _site: (u32, u32)) -> Option<(u64, u64)> {
        None
    }

    /// Source-aware counterpart; legacy providers describe the declaration source.
    fn failure_source_position(
        &self,
        definition: &DefPath,
        site: hirsel_lir::ir::SiteRange,
    ) -> Option<(u64, u64)> {
        (site.source == 0)
            .then(|| self.failure_position(definition, (site.start, site.end)))
            .flatten()
    }

    fn iterator_next(
        &mut self,
        _state: &mut OpValue,
        _canceled: Option<u32>,
    ) -> Result<Option<OpValue>, String> {
        Err("unsupported lazy iterator".into())
    }
    fn call(&mut self, op: &OperationId, args: &[OpValue], output: &mut Vec<u8>) -> OpOutcome;
    /// The outcome of `op` in a task canceled with `cause` and not shielded (CONC-3), when
    /// `op` is a cancellation point: effect-free program errors win, otherwise it completes
    /// `Canceled` without the requested effect, as the runtime's blocking requests do (`hirsel-rt/src/host_waits.rs`), and `call` is not made. None
    /// performs `op` with `call`. Consumed-resource cleanup may append a report to `output`.
    fn canceled(
        &mut self,
        _op: &OperationId,
        _args: &[OpValue],
        _cause: u32,
        _output: &mut Vec<u8>,
    ) -> Option<OpOutcome> {
        None
    }
    /// Whether the last `call` was a capability operation, after which the running task
    /// yields to the next runnable one (CONC-1). Library leaves are not.
    fn yielded(&mut self) -> bool {
        false
    }
    /// The run's virtual monotonic reading, when `clock` is a monotonic clock this model
    /// granted. The model owns the reading; H owns what waits for it and when it advances.
    fn monotonic(&self, _clock: u64) -> Option<u64> {
        None
    }
    /// Advance the virtual monotonic reading to `ticks`; it never moves backward.
    fn advance_monotonic(&mut self, _ticks: u64) {}
    /// With no task runnable, before virtual time advances: deliver a signal script's next
    /// arrival (`signals.script@1`).
    fn deliver_signal(&mut self) -> SignalDelivery {
        SignalDelivery::None
    }
    /// Implicit File close/AtomicFile abort or network close, after H has checked its
    /// linear identity. `Some(rule)`: the close raised that language failure, after
    /// appending its report to `output`.
    fn close_handle(
        &mut self,
        _handle: u64,
        _output: &mut Vec<u8>,
    ) -> Result<Option<String>, String> {
        Ok(None)
    }
}

/// A program that calls no operation.
pub struct NoOperations;

impl Operations for NoOperations {
    fn domain(&self) -> OperationDomain {
        OperationDomain::Simulation
    }
    fn fork_simulation(&self) -> Option<Box<dyn Operations>> {
        Some(Box::new(Self))
    }

    fn call(&mut self, _: &OperationId, _: &[OpValue], _: &mut Vec<u8>) -> OpOutcome {
        OpOutcome::Unsupported
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Logical CPU grant. Physical placement is outside H's model.
    pub cpu_max_workers: Option<u32>,
    pub cpu_free: bool,
    /// Native start selections reconstructed from the validated Ready graph.
    /// Each row binds map instance, control call and actual mapper code.
    pub cpu_starts: Vec<(ContentHash, u32, ContentHash)>,
    /// Independently validated source/declaration inputs for direct graph execution.
    /// Candidate receipt tables cannot populate these inputs.
    pub receipt_sources: Vec<hirsel_lir::sealed_payload_source::ReceiptSource>,
    pub seal_sources: Vec<hirsel_lir::sealed_payload_source::SourceSeal>,
    pub seal_descriptors: Vec<hirsel_lir::sealed::SealDescriptor>,
    /// Finite counts saturate to sticky immortality above this (small values
    /// force saturation in tests).
    pub count_max: u64,
    /// Fail the failure-capable site with this execution index.
    pub inject: Option<u64>,
    /// Compare every block's count with its ideal holders after every step.
    pub check_every_step: bool,
    /// Reference mode: additionally recount all physical holders every step.
    /// Also enabled by HIRSEL_HEAPCHECK_FULL_RECOUNT=1 for whole-suite controls.
    pub full_recount_every_step: bool,
    /// A tooling limit, never a language outcome.
    pub step_limit: u64,
    /// Reference campaign: replay the entry for every injected site.
    pub reexecute_campaign: bool,
    /// Compare complete fork and replay campaigns (test/measurement control).
    pub compare_campaigns: bool,
    /// Harness-owned context handles, open for the whole run.
    pub context_handles: Vec<(u64, String)>,
    /// Fire the earliest pending deadline at the timer choice point with this execution index.
    pub fire: Option<u64>,
}

/// Default executor instruction budget: tooling under ratified spec issue #34,
/// never language semantics. The largest numeric clean or injected H run is
/// 1,441,572 steps; this is the next power of two with at least fourfold headroom.
pub const DEFAULT_INSTRUCTION_LIMIT: u64 = 8_388_608;

impl Default for Config {
    fn default() -> Self {
        Self {
            cpu_max_workers: None,
            cpu_free: false,
            cpu_starts: Vec::new(),
            receipt_sources: Vec::new(),
            seal_sources: Vec::new(),
            seal_descriptors: Vec::new(),
            count_max: u32::MAX as u64 - 1,
            inject: None,
            check_every_step: true,
            full_recount_every_step: std::env::var_os("HIRSEL_HEAPCHECK_FULL_RECOUNT")
                .is_some_and(|v| v == "1"),
            step_limit: DEFAULT_INSTRUCTION_LIMIT,
            reexecute_campaign: std::env::var_os("HIRSEL_HEAPCHECK_REEXECUTE_CAMPAIGN")
                .is_some_and(|v| v == "1"),
            compare_campaigns: std::env::var_os("HIRSEL_HEAPCHECK_COMPARE_CAMPAIGNS")
                .is_some_and(|v| v == "1"),
            context_handles: Vec::new(),
            fire: None,
        }
    }
}

/// The rule of an injected failure.
pub const INJECTED_RULE: &str = "HEAPCHECK-INJECTED";

#[cfg(test)]
mod receipt_layout_tests {
    use super::*;
    use hirsel_lir::layout_types::{CopyRecipe, DropStep, LayoutKind};
    use hirsel_lir::operation::{NativeType, Primitive};
    #[test]
    fn private_receipt_layout_can_be_linked_without_minting_a_holder() {
        let mut layouts = crate::build::Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let id = layouts.scalar(ScalarKind::Pointer);
        let mut program = Program::<hirsel_lir::stage::Rc> {
            target: layouts.target,
            layouts: layouts.table,
            units: vec![],
            statics: vec![],
            invocation_bodies: vec![],
            pretransfer_adapters: vec![],
            invocations: Default::default(),
        };
        program.check().unwrap();
        let record = &mut program.layouts.layouts[id.0 as usize];
        record.kind = LayoutKind::PayloadReceipt {
            root: NativeType::Primitive(Primitive::Bytes),
        };
        record.drop = vec![DropStep::ReleaseReceipt { offset: 0 }];
        record.copy = CopyRecipe::NotCopyable;
        program.check().unwrap();
    }
}
