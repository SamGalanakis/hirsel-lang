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
        for unit in &self.units {
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
    /// definition's path, so units some closure site names as its code are
    /// not entry candidates.
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
        w.open("heapcheck-program");
        self.target.write(w);
        self.layouts.write(w);
        self.units.write(w);
        self.statics.write(w);
        w.close();
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.open("heapcheck-program")?;
        let program = Self {
            target: TargetSpec::read(r)?,
            layouts: LayoutTable::read(r)?,
            units: Vec::read(r)?,
            statics: Vec::read(r)?,
        };
        r.close()?;
        Ok(program)
    }
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
    /// The model has no implementation: the run is an `Unsupported` violation.
    Unsupported,
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

    /// The one-based line and column of a failure site, a byte range of
    /// `definition`'s source, for `task.isolate`'s `Failure` (FAIL-2).
    fn failure_position(&self, _definition: &DefPath, _site: (u32, u32)) -> Option<(u64, u64)> {
        None
    }

    fn call(&mut self, op: &OperationId, args: &[OpValue], output: &mut Vec<u8>) -> OpOutcome;
    /// The outcome of `op` in a task canceled with `cause` and not shielded (CONC-3), when
    /// `op` is a cancellation point: it completes `Canceled` with no effect, as the runtime's
    /// blocking requests do (`hirsel-rt/src/host_waits.rs`), and `call` is not made. None
    /// performs `op` with `call`.
    fn canceled(&mut self, _op: &OperationId, _args: &[OpValue], _cause: u32) -> Option<OpOutcome> {
        None
    }
    /// Whether the last `call` was a capability operation, after which the running task
    /// yields to the next runnable one (CONC-1). Library leaves are not.
    fn yielded(&mut self) -> bool {
        false
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
}

/// Default executor instruction budget: tooling under ratified spec issue #34,
/// never language semantics. The largest numeric clean or injected H run is
/// 1,441,572 steps; this is the next power of two with at least fourfold headroom.
pub const DEFAULT_INSTRUCTION_LIMIT: u64 = 8_388_608;

impl Default for Config {
    fn default() -> Self {
        Self {
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
        }
    }
}

/// The rule of an injected failure.
pub const INJECTED_RULE: &str = "HEAPCHECK-INJECTED";
