//! The builder a library body is written with: `Cx` (what a body may read
//! and require) and `Body` (a Value-stage unit under construction, over the
//! shared `Editor`).
//!
//! A body is written against a unique block and states only semantic
//! operations: `Copy`, `Read Take`, `Store`, `Drop`, `Compare`, `Hash` and
//! the sequence instructions. It never writes `Dup`, `Release`, `IsUnique`
//! or a reuse token: P3 to P6 insert and check those exactly as for a unit
//! imported from TIR. Consuming cursor
//! open bodies have one checked exception: they record the source
//! uniqueness before transferring its credit into their cursor. `shared.thaw`
//! tests its owned `Shared` root, whose count is the root count.
use super::Request;
use crate::PassError;
use crate::insert::Editor;
use crate::instance::{InstanceId, Instances, Program};
use hirsel_core_contract::Passing;
use hirsel_layout::layout::{EnvironmentShape, LayoutBuilder, Owners};
use hirsel_lir::abi_types::{CallableShape, FnAbi, FnAbiId};
use hirsel_lir::ir::{
    AddressBase, AddressId, AddressInfo, ArgumentValue, ArithMode, BinaryOp, BlockId, BlockKind,
    CallArgument, CallId, CallSite, Callee, CleanupAnchor, CleanupAnchorId, ConstantData,
    ConstantId, ConstantValue, DestinationClass, EdgeId, FailureEdge, FailureId, Holding, Inst,
    LirType, LirTypeId, LirUnit, LoanId, LoanInfo, PoolRange, ProjectionStep, ReadMode, ScopeId,
    ScopeInfo, SiteRange, SlotId, SlotInfo, SlotKind, SourceAnchor, SourceAnchorId, Symbol,
    SymbolId, SymbolKind, Terminator, UnitHeader, UnitOrigin, ValueId, ValueRepr,
};
use hirsel_lir::layout::LayoutError;
use hirsel_lir::layout_types::{LayoutId, LayoutKind, LayoutRecord, LayoutTable, ScalarKind};
use hirsel_lir::operation::OperationArgument;
use hirsel_lir::stage::{StageKind, Value};
use hirsel_syntax::DefPath;
use hirsel_types::{GenericArgument, Primitive, Size, Type, TypeId, TypeInterner, TypeKind};

type R<T> = Result<T, PassError>;

/// The layout derivation a body and the instance collector read: the one
/// `LayoutBuilder` of the program being lowered.
pub trait Layouts {
    fn layout(&mut self, types: &TypeInterner, ty: TypeId) -> Result<LayoutId, LayoutError>;
    fn record(&self, id: LayoutId) -> &LayoutRecord;
    fn payload_receipt(&mut self, root: &hirsel_lir::operation::NativeType) -> LayoutId;
    fn fn_abi(
        &mut self,
        types: &TypeInterner,
        callable: TypeId,
        shape: CallableShape,
    ) -> Result<FnAbiId, LayoutError>;
    fn with_caller_site(&mut self, base: FnAbiId) -> Result<FnAbiId, LayoutError>;
    fn with_scoped_contract(
        &mut self,
        base: FnAbiId,
        parameters: Vec<hirsel_lir::abi_types::ParameterPath>,
        captures: Vec<hirsel_lir::abi_types::ParameterPath>,
    ) -> Result<FnAbiId, LayoutError>;
    fn with_scoped_entry(
        &mut self,
        base: FnAbiId,
        parameter: hirsel_lir::abi_types::ParameterPath,
        nested: FnAbiId,
    ) -> Result<FnAbiId, LayoutError>;
    fn table(&self) -> &LayoutTable;
    /// Reconstruct a sealed root against these exact physical layouts and target.
    fn seal_descriptor(
        &self,
        declarations: &hirsel_layout::operation::OperationRegistry,
        root: &hirsel_lir::operation::NativeType,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, LayoutError>;
    fn owners(&self) -> &Owners;
    /// The cursor's opaque source (`OwnerRepresentation::Cursor`).
    fn cursor_source(&mut self) -> LayoutId;
    fn environment(
        &mut self,
        types: &TypeInterner,
        shape: &EnvironmentShape<'_>,
    ) -> Result<LayoutId, LayoutError>;
    /// The sequence a flat list builder appends `element`'s encoding to.
    fn flat_builder(&mut self, element: LayoutId) -> Result<LayoutId, LayoutError>;
    /// The stand-in of the box layout `boxed` while its payload is a
    /// constructor-tail hole (LANG-10).
    fn open_box(&mut self, boxed: LayoutId) -> Result<LayoutId, LayoutError>;
}

impl Layouts for LayoutBuilder<'_> {
    fn layout(&mut self, types: &TypeInterner, ty: TypeId) -> Result<LayoutId, LayoutError> {
        LayoutBuilder::layout(self, types, ty)
    }
    fn payload_receipt(&mut self, root: &hirsel_lir::operation::NativeType) -> LayoutId {
        LayoutBuilder::payload_receipt(self, root)
    }
    fn record(&self, id: LayoutId) -> &LayoutRecord {
        LayoutBuilder::record(self, id)
    }
    fn fn_abi(
        &mut self,
        types: &TypeInterner,
        callable: TypeId,
        shape: CallableShape,
    ) -> Result<FnAbiId, LayoutError> {
        LayoutBuilder::fn_abi(self, types, callable, shape)
    }
    fn with_caller_site(&mut self, base: FnAbiId) -> Result<FnAbiId, LayoutError> {
        LayoutBuilder::with_caller_site(self, base)
    }
    fn with_scoped_contract(
        &mut self,
        base: FnAbiId,
        parameters: Vec<hirsel_lir::abi_types::ParameterPath>,
        captures: Vec<hirsel_lir::abi_types::ParameterPath>,
    ) -> Result<FnAbiId, LayoutError> {
        LayoutBuilder::with_scoped_contract(self, base, parameters, captures)
    }
    fn with_scoped_entry(
        &mut self,
        base: FnAbiId,
        parameter: hirsel_lir::abi_types::ParameterPath,
        nested: FnAbiId,
    ) -> Result<FnAbiId, LayoutError> {
        LayoutBuilder::with_scoped_entry(self, base, parameter, nested)
    }
    fn table(&self) -> &LayoutTable {
        LayoutBuilder::table(self)
    }
    fn owners(&self) -> &Owners {
        LayoutBuilder::owners(self)
    }
    fn seal_descriptor(
        &self,
        declarations: &hirsel_layout::operation::OperationRegistry,
        root: &hirsel_lir::operation::NativeType,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, LayoutError> {
        hirsel_layout::sealed::derive(declarations, root, self.table(), self.target())
    }
    fn cursor_source(&mut self) -> LayoutId {
        LayoutBuilder::cursor_source(self)
    }
    fn environment(
        &mut self,
        types: &TypeInterner,
        shape: &EnvironmentShape<'_>,
    ) -> Result<LayoutId, LayoutError> {
        LayoutBuilder::environment(self, types, shape)
    }
    fn flat_builder(&mut self, element: LayoutId) -> Result<LayoutId, LayoutError> {
        LayoutBuilder::flat_builder(self, element)
    }
    fn open_box(&mut self, boxed: LayoutId) -> Result<LayoutId, LayoutError> {
        LayoutBuilder::open_box(self, boxed)
    }
}

/// The value representation of a layout kind.
pub fn repr_of(kind: &LayoutKind) -> ValueRepr {
    match kind {
        LayoutKind::Unit => ValueRepr::Unit,
        LayoutKind::Scalar(k) => ValueRepr::Scalar(*k),
        LayoutKind::Block { .. } | LayoutKind::Box { .. } | LayoutKind::Shared { .. } => {
            ValueRepr::Block
        }
        LayoutKind::Callable { .. } => ValueRepr::Callable,
        LayoutKind::Handle
        | LayoutKind::HostCapability { .. }
        | LayoutKind::SealedRegion { .. } => ValueRepr::Handle,
        LayoutKind::Path => ValueRepr::Scalar(ScalarKind::U64),
        LayoutKind::Arena => ValueRepr::Arena,
        _ => ValueRepr::Aggregate,
    }
}

/// A direct callee: the symbol of an instance and its `FnAbi`.
#[derive(Clone, Debug)]
pub struct Direct {
    pub symbol: Symbol,
    pub abi: FnAbiId,
}

/// What a library body builds from: its own instance (path, complete type
/// and size actuals, callable type and `FnAbi`), the program's layout
/// derivation, and the instances it requires.
pub struct Cx<'a> {
    pub program: &'a Program,
    pub layouts: &'a mut dyn Layouts,
    instances: &'a mut Instances,
    pub id: InstanceId,
    pub path: DefPath,
    /// The session interner: the actuals and the callable type are its rows.
    pub types: &'a TypeInterner,
    /// Complete generic actuals in ordinal order.
    pub arguments: Vec<GenericArgument>,
    pub callable: TypeId,
    pub abi: FnAbiId,
}

impl<'a> Cx<'a> {
    pub fn new(
        program: &'a Program,
        layouts: &'a mut dyn Layouts,
        instances: &'a mut Instances,
        id: InstanceId,
    ) -> Self {
        let actuals = &instances.actuals[id.index()];
        Self {
            program,
            path: instances.view(id).key.definition().clone(),
            types: &program.types,
            arguments: actuals.arguments.clone(),
            callable: instances.callables[id.index()],
            abi: instances.abis[id.index()],
            layouts,
            instances,
            id,
        }
    }

    pub(super) fn unsupported<T>(&self, what: impl std::fmt::Display) -> R<T> {
        Err(PassError::Unsupported(format!("{:?}: {what}", self.path)))
    }

    /// The body's own `FnAbi`: `operation_abi(path, actuals)` of its call.
    pub fn fn_abi(&self) -> FnAbi {
        self.layouts.table().fn_abis[self.abi.0 as usize].clone()
    }

    /// Type actual `ordinal`.
    pub fn type_actual(&self, ordinal: usize) -> R<TypeId> {
        match self.arguments.get(ordinal) {
            Some(GenericArgument::Type(ty)) => Ok(*ty),
            _ => Err(PassError::Malformed(format!(
                "{:?}: generic actual {ordinal} is not a type",
                self.path
            ))),
        }
    }

    /// Size actual `ordinal`, folded to a constant.
    pub fn size_actual(&self, ordinal: usize) -> R<u64> {
        match self.arguments.get(ordinal) {
            Some(GenericArgument::Size(Size::Constant(n))) => Ok(*n),
            _ => Err(PassError::Malformed(format!(
                "{:?}: generic actual {ordinal} is not a constant size",
                self.path
            ))),
        }
    }

    /// The layout of a type.
    pub fn layout(&mut self, ty: TypeId) -> R<LayoutId> {
        self.layouts
            .layout(self.types, ty)
            .map_err(|e| PassError::Layout(self.path.clone(), e))
    }

    /// The layout of a primitive type.
    pub fn primitive(&mut self, primitive: Primitive) -> R<LayoutId> {
        let ty = self
            .types
            .intern(Type::with_kind(TypeKind::Primitive(primitive)));
        self.layout(ty)
    }

    pub fn record(&self, layout: LayoutId) -> &LayoutRecord {
        self.layouts.record(layout)
    }

    /// The payload of a block reference layout.
    pub fn payload(&self, layout: LayoutId) -> Option<LayoutId> {
        match self.record(layout).kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => Some(payload),
            _ => None,
        }
    }

    /// The element layout of a counted sequence block (List, Array, String
    /// or Bytes storage).
    pub fn element(&self, layout: LayoutId) -> R<LayoutId> {
        match self.payload(layout).map(|p| &self.record(p).kind) {
            Some(LayoutKind::Sequence { element } | LayoutKind::SoaColumns { element }) => {
                Ok(*element)
            }
            // MEM-13: elements lie back to back at their own extents, so only a
            // walk from the first reaches one; pop and take-iteration would be O(n).
            Some(LayoutKind::Sequential { .. }) => self.unsupported(
                "a sequential Packed list is walk-only (MEM-13): it has no element slots",
            ),
            _ => self.unsupported(format!("{layout:?} is not a counted sequence block")),
        }
    }

    /// The environment layout of this closure-shaped library instance.
    pub fn environment(&self) -> Option<LayoutId> {
        self.instances.environments[self.id.index()]
    }

    /// Require another library instance and name it as a direct callee.
    /// A body's requirements are exactly the requests its builder makes.
    pub fn require(&mut self, request: Request) -> R<Direct> {
        let path = request.path.clone();
        let instance = super::instantiate(self.layouts, self.instances, request)?;
        Ok(Direct {
            symbol: Symbol {
                kind: SymbolKind::Function,
                definition: path,
                instance: Some(self.instances.view(instance).key.digest()),
            },
            abi: self.instances.abis[instance.index()],
        })
    }
}

/// One declared parameter of the body under construction: its entry value
/// (the first version of its slot), its slot and that slot's whole address,
/// prepared at entry.
#[derive(Clone, Copy, Debug)]
pub struct Parameter {
    pub value: ValueId,
    pub slot: SlotId,
    pub address: AddressId,
    pub passing: Passing,
    pub layout: LayoutId,
}

/// A Value-stage library unit under construction. Instructions go to the
/// current block; every instruction carries the unit's one source anchor,
/// so an observation never names a library-internal site.
pub struct Body<'c, 'a> {
    pub cx: &'c mut Cx<'a>,
    pub ed: Editor<Value>,
    pub parameters: Vec<Parameter>,
    /// The unit's one source anchor.
    pub anchor: SourceAnchorId,
    /// The scope of the parameters and of every slot a body adds.
    pub scope: ScopeId,
    current: BlockId,
    /// The reaching version of each slot in the current block.
    versions: Vec<Option<ValueId>>,
    /// Owned slots a failure here must drop (a `take` parameter not yet
    /// moved out), with their whole addresses, in the order they filled.
    held: Vec<(SlotId, AddressId)>,
    /// What is held at entry: the owned `take` parameters.
    entry_held: Vec<(SlotId, AddressId)>,
    /// The failure cleanup of each `held` list already built.
    cleanups: std::collections::HashMap<(Vec<SlotId>, Option<BlockId>), BlockId>,
    drop_anchor: Option<CleanupAnchorId>,
    propagation: Option<BlockId>,
}

impl<'c, 'a> Body<'c, 'a> {
    /// An empty body with its declared parameters in slots.
    pub fn new(cx: &'c mut Cx<'a>) -> R<Self> {
        let abi = cx.fn_abi();
        let table = cx.layouts.table();
        let key = cx.instances.view(cx.id).key.digest();
        let mut unit = LirUnit::<Value>::new(UnitHeader {
            source_count: 1,
            key: hirsel_lir::objects::UnitKey(hirsel_syntax::ContentHash([0; 32])),
            track_caller: false,
            stage: StageKind::Value,
            lir_version: hirsel_lir::LIR_VERSION,
            origin: UnitOrigin::Library,
            definition: cx.path.clone(),
            implementation_revision: super::revision(&cx.path),
            instance: key,
            target: table.target,
            abi: cx.abi,
        });
        unit.tables.task_domains = 1;
        unit.scopes.push(ScopeInfo { parent: None });
        unit.tables.source_anchors.push(SourceAnchor {
            body: 0,
            block: 0,
            statement: 0,
            site: SiteRange {
                source: 0,
                start: 0,
                end: 0,
            },
        });
        let anchor = SourceAnchorId(0);
        let mut ed = Editor::new(unit);
        // The entry prepares the parameters' places and jumps to the body,
        // as an imported unit's does.
        let entry = ed.block(BlockKind::Body, &[], anchor);
        ed.unit.entry = entry;
        let mut body = Self {
            cx,
            ed,
            parameters: Vec::new(),
            anchor,
            scope: ScopeId(0),
            current: entry,
            versions: Vec::new(),
            held: Vec::new(),
            entry_held: Vec::new(),
            cleanups: std::collections::HashMap::new(),
            drop_anchor: None,
            propagation: None,
        };
        for p in &abi.parameters {
            let ty = body.ty(p.layout);
            let value = body.ed.value(ty, Holding::Plain);
            let slot = body.slot(p.layout, SlotKind::Parameter(p.source));
            body.versions[slot.index()] = Some(value);
            body.ed.unit.parameters.push(value);
            let address = body.unprepared(AddressBase::Slot(slot), &[], p.layout);
            body.parameters.push(Parameter {
                value,
                slot,
                address,
                passing: p.convention,
                layout: p.layout,
            });
            if p.convention == Passing::Take && body.owns(p.layout) {
                body.held.push((slot, address));
            }
        }
        // Every parameter is in its slot from entry, so each preparation's
        // failure drops every owned one.
        body.entry_held = body.held.clone();
        for p in body.parameters.clone() {
            let failure = body.failure(None);
            body.push(Inst::Address {
                address: p.address,
                failure,
            });
        }
        let start = body.block(BlockKind::Body, &[]);
        body.jump(start, &[]);
        body.at(start);
        Ok(body)
    }

    // --- types, values and storage ---

    pub fn record(&self, layout: LayoutId) -> &LayoutRecord {
        self.cx.record(layout)
    }

    /// The interned LIR type of a layout.
    pub fn ty(&mut self, layout: LayoutId) -> LirTypeId {
        let ty = LirType {
            repr: repr_of(&self.record(layout).kind),
            layout,
        };
        if let Some(i) = self.ed.unit.types.iter().position(|t| *t == ty) {
            return LirTypeId(i as u32);
        }
        self.ed.unit.types.push(ty);
        LirTypeId(self.ed.unit.types.len() as u32 - 1)
    }

    pub fn layout_of(&self, value: ValueId) -> LayoutId {
        let unit = &self.ed.unit;
        unit.types[unit.values[value.index()].ty.index()].layout
    }

    /// Whether a value of this layout carries a holder credit.
    pub fn owns(&self, layout: LayoutId) -> bool {
        let record = self.record(layout);
        match repr_of(&record.kind) {
            ValueRepr::Block => true,
            ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Handle => {
                !(record.drop.is_empty() && record.finalizer.is_none())
            }
            _ => false,
        }
    }

    pub fn value(&mut self, layout: LayoutId, holding: Holding) -> ValueId {
        let ty = self.ty(layout);
        self.ed.value(ty, holding)
    }

    /// A freshly produced value: owned if its layout carries a credit.
    pub fn fresh(&mut self, layout: LayoutId) -> ValueId {
        let holding = if self.owns(layout) {
            Holding::Owned
        } else {
            Holding::Plain
        };
        self.value(layout, holding)
    }

    /// A new slot in the body's scope. A local needs `StorageLive` before
    /// its first store and `StorageDead` once it is empty.
    pub fn slot(&mut self, layout: LayoutId, kind: SlotKind) -> SlotId {
        let ty = self.ty(layout);
        self.ed.unit.slots.push(SlotInfo {
            ty,
            kind,
            scope: self.scope,
        });
        self.versions.push(None);
        SlotId(self.ed.unit.slots.len() as u32 - 1)
    }

    /// The version of `slot` that reaches the current point.
    pub fn reaching(&self, slot: SlotId) -> R<ValueId> {
        self.versions[slot.index()].ok_or_else(|| {
            PassError::Malformed(format!("{:?}: {slot:?} has no version", self.cx.path))
        })
    }

    /// State the version of `slot` at the current point: a version-join
    /// parameter of the block just entered.
    pub fn set_reaching(&mut self, slot: SlotId, version: ValueId) {
        self.versions[slot.index()] = Some(version);
    }

    /// A value naming a new version of `slot`: the `version` of a write, or
    /// a version-join block parameter.
    pub fn version(&mut self, slot: SlotId) -> ValueId {
        let ty = self.ed.unit.slots[slot.index()].ty;
        self.ed.value(ty, Holding::Plain)
    }

    /// A capture aliases its environment field; the environment owner retains
    /// cleanup responsibility while this body mutates it in place.
    pub fn capture(&mut self, ordinal: u32, layout: LayoutId) -> R<Parameter> {
        let slot = self.slot(layout, SlotKind::Capture(ordinal));
        let value = self.version(slot);
        self.set_reaching(slot, value);
        self.ed.unit.captures.push(value);
        let address = self.place(AddressBase::Slot(slot), &[], layout, None)?;
        Ok(Parameter {
            value,
            slot,
            address,
            passing: Passing::Mut,
            layout,
        })
    }

    // --- blocks ---

    pub fn block(&mut self, kind: BlockKind, parameters: &[ValueId]) -> BlockId {
        self.ed.block(kind, parameters, self.anchor)
    }

    /// Continue in `block`.
    pub fn at(&mut self, block: BlockId) {
        self.current = block;
    }

    pub fn current(&self) -> BlockId {
        self.current
    }

    pub fn push(&mut self, inst: Inst) {
        self.ed.code[self.current.index()].push(inst);
    }

    pub fn edge(&mut self, target: BlockId, arguments: &[ValueId]) -> EdgeId {
        self.ed.edge(target, arguments)
    }

    pub fn terminate(&mut self, terminator: Terminator) {
        self.ed.unit.terminators[self.current.index()] = terminator;
    }

    pub fn jump(&mut self, target: BlockId, arguments: &[ValueId]) {
        let edge = self.edge(target, arguments);
        self.terminate(Terminator::Jump(edge));
    }

    pub fn branch(&mut self, condition: ValueId, yes: BlockId, no: BlockId) {
        let yes = self.edge(yes, &[]);
        let no = self.edge(no, &[]);
        self.terminate(Terminator::Branch { condition, yes, no });
    }

    pub fn ret(&mut self, value: Option<ValueId>) {
        self.terminate(Terminator::Return(value));
    }

    /// Schedule an owned local for failure cleanup in initialization order.
    pub fn hold(&mut self, slot: SlotId, address: AddressId) {
        if self.owns(self.address_layout(address)) {
            self.held.push((slot, address));
        }
    }

    /// Stop scheduling cleanup for storage the body has emptied through
    /// all its inline positions. C independently checks the content is gone.
    pub fn emptied(&mut self, slot: SlotId) {
        self.held.retain(|&(held, _)| held != slot);
    }

    // --- failures ---

    /// A failure edge into `cleanup` (a `FailureCleanup` block the body
    /// wrote, releasing what it still owns there). `rule` is the language
    /// rule the failure starts, None when it propagates or cannot happen.
    pub fn failure_to(
        &mut self,
        cleanup: BlockId,
        arguments: &[ValueId],
        rule: Option<&str>,
    ) -> FailureId {
        let cleanup = self.edge(cleanup, arguments);
        self.ed.unit.tables.failures.push(FailureEdge {
            cleanup,
            rule: rule.map(str::to_owned),
            anchor: self.anchor,
            dispose: PoolRange::EMPTY,
        });
        FailureId(self.ed.unit.tables.failures.len() as u32 - 1)
    }

    /// A failure edge into the cleanup that drops every slot the body
    /// still holds (`held`), latest first, then propagates.
    pub fn failure(&mut self, rule: Option<&str>) -> FailureId {
        let block = self.cleanup(self.held.clone(), self.propagation);
        self.failure_to(block, &[], rule)
    }

    /// Drain remaining owners before entering a library boundary's catch.
    /// The tail is part of the key: cleanup outside the boundary still propagates.
    pub fn caught_failure(&mut self, tail: BlockId) -> FailureId {
        let block = self.cleanup(self.held.clone(), Some(tail));
        self.failure_to(block, &[], None)
    }

    fn cleanup(&mut self, held: Vec<(SlotId, AddressId)>, tail: Option<BlockId>) -> BlockId {
        let key: Vec<SlotId> = held.iter().map(|(s, _)| *s).collect();
        let key = (key, tail);
        if let Some(&b) = self.cleanups.get(&key) {
            return b;
        }
        if held.is_empty()
            && let Some(tail) = tail
        {
            return tail;
        }
        let b = self.block(BlockKind::FailureCleanup, &[]);
        match held.split_last() {
            None => self.ed.unit.terminators[b.index()] = Terminator::Propagate,
            Some((&(_, address), rest)) => {
                let next = self.cleanup(rest.to_vec(), tail);
                let anchor = self.drop_anchor();
                let failure = self.failure_to(next, &[], None);
                self.ed.code[b.index()].push(Inst::Drop {
                    address,
                    if_live: false,
                    anchor,
                    failure,
                });
                let edge = self.edge(next, &[]);
                self.ed.unit.terminators[b.index()] = Terminator::Jump(edge);
            }
        }
        self.cleanups.insert(key, b);
        b
    }

    fn drop_anchor(&mut self) -> CleanupAnchorId {
        if let Some(a) = self.drop_anchor {
            return a;
        }
        self.ed.unit.tables.cleanup_anchors.push(CleanupAnchor {
            body: 0,
            cleanup: 0,
            action: 0,
            site: SiteRange {
                source: 0,
                start: 0,
                end: 0,
            },
        });
        let a = CleanupAnchorId(self.ed.unit.tables.cleanup_anchors.len() as u32 - 1);
        self.drop_anchor = Some(a);
        a
    }

    /// Ownership state at a branch. Each successor builds its own transfers.
    /// The failure of a preparation at entry, which releases what the body
    /// holds there (its owned `take` parameters) whatever it holds now.
    pub fn entry_failure(&mut self) -> FailureId {
        let held = std::mem::replace(&mut self.held, self.entry_held.clone());
        let propagation = self.propagation.take();
        let failure = self.failure(None);
        self.propagation = propagation;
        self.held = held;
        failure
    }

    /// Select the boundary bookkeeping retired after failure cleanup.
    pub fn propagation(&mut self, tail: Option<BlockId>) -> Option<BlockId> {
        std::mem::replace(&mut self.propagation, tail)
    }
    pub fn held_state(&self) -> Vec<(SlotId, AddressId)> {
        self.held.clone()
    }
    pub fn restore_held(&mut self, held: &[(SlotId, AddressId)]) {
        self.held = held.to_vec();
    }
    /// A semantic drop completes this obligation even when its finalizer fails.
    pub fn forget_held(&mut self, address: AddressId) {
        self.held.retain(|&(_, a)| a != address);
    }

    // --- constants and scalars ---

    pub fn constant(&mut self, layout: LayoutId, value: ConstantValue) -> ValueId {
        let ty = self.ty(layout);
        let data = ConstantData { ty, value };
        let constants = &mut self.ed.unit.tables.constants;
        let constant = match constants.iter().position(|c| *c == data) {
            Some(i) => ConstantId(i as u32),
            None => {
                constants.push(data);
                ConstantId(constants.len() as u32 - 1)
            }
        };
        let result = self.ed.value(ty, Holding::Plain);
        self.push(Inst::Const { result, constant });
        result
    }

    pub fn u64_layout(&mut self) -> R<LayoutId> {
        self.cx.primitive(Primitive::U64)
    }

    pub fn bool_layout(&mut self) -> R<LayoutId> {
        self.cx.primitive(Primitive::Bool)
    }

    pub fn u64(&mut self, n: u64) -> R<ValueId> {
        let layout = self.u64_layout()?;
        Ok(self.constant(layout, ConstantValue::Integer(u128::from(n))))
    }

    /// A scalar comparison: a Plain bool. It cannot fail.
    pub fn compare(&mut self, op: BinaryOp, left: ValueId, right: ValueId) -> R<ValueId> {
        let layout = self.bool_layout()?;
        let result = self.value(layout, Holding::Plain);
        let failure = self.failure(None);
        self.push(Inst::Binary {
            result,
            op,
            left,
            right,
            failure,
        });
        Ok(result)
    }

    /// `left + right` of one integer kind, wrapping or saturating: positions
    /// and capacities a body computes never reach either bound.
    pub fn add(&mut self, mode: ArithMode, left: ValueId, right: ValueId) -> ValueId {
        let layout = self.layout_of(left);
        let result = self.value(layout, Holding::Plain);
        self.push(Inst::IntArith {
            result,
            flag: None,
            op: BinaryOp::Add,
            mode,
            left,
            right,
        });
        result
    }

    // --- places ---

    /// An address with no preparation yet.
    fn unprepared(
        &mut self,
        base: AddressBase,
        steps: &[ProjectionStep],
        layout: LayoutId,
    ) -> AddressId {
        let ty = self.ty(layout);
        let projection =
            PoolRange::push(&mut self.ed.unit.pools.projections, steps.iter().copied());
        self.ed.unit.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base,
            projection,
            ty,
        });
        AddressId(self.ed.unit.addresses.len() as u32 - 1)
    }

    /// An address and its preparation in the current block. `rule` is the
    /// rule a checked step fails with (`MEM-14` for an `Index` step); a
    /// field or `Slot` path checks nothing.
    pub fn place(
        &mut self,
        base: AddressBase,
        steps: &[ProjectionStep],
        layout: LayoutId,
        rule: Option<&str>,
    ) -> R<AddressId> {
        let address = self.unprepared(base, steps, layout);
        let failure = self.failure(rule);
        self.push(Inst::Address { address, failure });
        Ok(address)
    }

    /// The root slot of an address chain, if it has one.
    pub fn root(&self, address: AddressId) -> Option<SlotId> {
        let mut at = address;
        loop {
            match self.ed.unit.addresses[at.index()].base {
                AddressBase::Slot(slot) => return Some(slot),
                AddressBase::Address(base) => at = base,
                AddressBase::Value(_) => return None,
            }
        }
    }

    pub(super) fn address_layout(&self, address: AddressId) -> LayoutId {
        let unit = &self.ed.unit;
        unit.types[unit.addresses[address.index()].ty.index()].layout
    }

    fn reaching_of(&self, address: AddressId) -> R<ValueId> {
        match self.root(address) {
            Some(slot) => self.reaching(slot),
            None => {
                let mut at = address;
                loop {
                    match self.ed.unit.addresses[at.index()].base {
                        AddressBase::Value(v) => return Ok(v),
                        AddressBase::Address(base) => at = base,
                        AddressBase::Slot(_) => unreachable!("rooted in a value"),
                    }
                }
            }
        }
    }

    /// Plain bits of a place.
    pub fn read_bits(&mut self, address: AddressId) -> R<ValueId> {
        let layout = self.address_layout(address);
        let result = self.value(layout, Holding::Plain);
        let reaching = self.reaching_of(address)?;
        self.push(Inst::Read {
            result,
            address,
            reaching,
            mode: ReadMode::Bits,
        });
        Ok(result)
    }

    /// Move the content of a place out; the place becomes empty, so a
    /// later failure no longer drops it.
    pub fn take(&mut self, address: AddressId) -> R<ValueId> {
        self.held.retain(|&(_, a)| a != address);
        let layout = self.address_layout(address);
        let result = self.fresh(layout);
        let reaching = self.reaching_of(address)?;
        self.push(Inst::Read {
            result,
            address,
            reaching,
            mode: ReadMode::Take,
        });
        Ok(result)
    }

    /// Begin a loan on a place and read it as a borrowed view. End the loan
    /// with `end_loan` once no view of it is used again.
    pub fn borrow(&mut self, address: AddressId, mutable: bool) -> R<(ValueId, LoanId)> {
        let loans = &mut self.ed.unit.tables.loans;
        loans.push(LoanInfo { address, mutable });
        let loan = LoanId(loans.len() as u32 - 1);
        self.push(Inst::BeginLoan {
            loan,
            address,
            mutable,
            if_inactive: false,
        });
        let layout = self.address_layout(address);
        let holding = if self.owns(layout) {
            Holding::Borrowed(loan)
        } else {
            Holding::Plain
        };
        let result = self.value(layout, holding);
        let reaching = self.reaching_of(address)?;
        self.push(Inst::Read {
            result,
            address,
            reaching,
            mode: ReadMode::Borrow,
        });
        Ok((result, loan))
    }

    pub fn end_loan(&mut self, loan: LoanId) {
        self.push(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }

    /// A semantic copy of a place. Its failure is allocation exhaustion,
    /// which carries no language rule.
    pub fn copy(&mut self, address: AddressId, failure: FailureId) -> R<ValueId> {
        let layout = self.address_layout(address);
        let result = self.fresh(layout);
        let reaching = self.reaching_of(address)?;
        self.push(Inst::Copy {
            result,
            address,
            reaching,
            failure,
        });
        Ok(result)
    }

    /// Initialise an empty place; the root slot has a new version.
    pub fn store(&mut self, address: AddressId, value: ValueId) -> R<ValueId> {
        let version = self.write(address)?;
        self.push(Inst::Store {
            address,
            value,
            version,
        });
        Ok(version)
    }

    fn write(&mut self, address: AddressId) -> R<ValueId> {
        let Some(slot) = self.root(address) else {
            return Err(PassError::Malformed(format!(
                "{:?}: a write through a value",
                self.cx.path
            )));
        };
        let version = self.version(slot);
        self.versions[slot.index()] = Some(version);
        Ok(version)
    }

    // --- sequences ---

    /// The element count of a sequence block or slice value.
    pub fn seq_len(&mut self, value: ValueId) -> R<ValueId> {
        let layout = self.u64_layout()?;
        let result = self.value(layout, Holding::Plain);
        self.push(Inst::SeqLen { result, value });
        Ok(result)
    }

    /// How many elements a sequence block holds without growing.
    pub fn seq_capacity(&mut self, value: ValueId) -> R<ValueId> {
        let layout = self.u64_layout()?;
        let result = self.value(layout, Holding::Plain);
        self.push(Inst::SeqCapacity { result, value });
        Ok(result)
    }

    /// Make the sequence block at `address` hold at least `capacity`
    /// elements. A write: P4 makes the block unique first.
    pub fn seq_reserve(&mut self, address: AddressId, capacity: ValueId) -> R<ValueId> {
        let version = self.write(address)?;
        self.push(Inst::SeqReserve {
            address,
            capacity,
            version,
        });
        Ok(version)
    }

    /// Prepare one push to the List block at `address` (MEM-6, LIR schema
    /// revision 32): a tracked arena List is charged and, when shared or
    /// full, copied or relocated within its arena. A write that P4 leaves to
    /// it, so `address` should be its own.
    pub fn arena_push(&mut self, address: AddressId) -> R<ValueId> {
        let version = self.write(address)?;
        let failure = self.failure(None);
        self.push(Inst::ArenaPush {
            address,
            count: None,
            version,
            failure,
        });
        Ok(version)
    }

    /// Append the owned Packed `value` to the sequential sequence block at
    /// `address` (MEM-13). A write: P4 makes the block unique first.
    pub fn seq_append(&mut self, address: AddressId, value: ValueId) -> R<ValueId> {
        let version = self.write(address)?;
        self.push(Inst::SeqAppend {
            address,
            value,
            version,
        });
        Ok(version)
    }

    /// Set the count of the sequence block at `address`. The body has
    /// initialised every slot below `len` and emptied every slot from it.
    pub fn seq_set_len(&mut self, address: AddressId, len: ValueId) -> R<ValueId> {
        let version = self.write(address)?;
        self.push(Inst::SeqSetLen {
            address,
            len,
            version,
        });
        Ok(version)
    }

    /// Element slot `index` of the sequence block at `sequence`, with no
    /// count check.
    pub fn slot_place(&mut self, sequence: AddressId, index: ValueId) -> R<AddressId> {
        let element = self.cx.element(self.address_layout(sequence))?;
        self.place(
            AddressBase::Address(sequence),
            &[ProjectionStep::Slot(index)],
            element,
            None,
        )
    }

    // --- calls ---

    /// Call `callee` at `abi` and continue after it. The invocation loans end
    /// on both outcomes, in reverse, before a failure enters the body's
    /// held-owner cleanup; mutable arguments publish their write-back version
    /// on failure too.
    /// Admission keeps evaluated owned actuals here, before the callee
    /// consumes them. Its failure ends prepared loans, drains all owners,
    /// and then reaches the caller's original cleanup continuation.
    pub(super) fn admission_failure(
        &mut self,
        callee: &Callee,
        arguments: &[CallArgument],
        loans: &[LoanId],
        tail: FailureId,
    ) -> FailureId {
        let mut owners: Vec<ValueId> = arguments
            .iter()
            .filter_map(|argument| match argument.value {
                ArgumentValue::Value(value)
                    if argument.passing == Passing::Take
                        && self.ed.unit.values[value.index()].holding == Holding::Owned =>
                {
                    Some(value)
                }
                _ => None,
            })
            .collect();
        let closure = match callee {
            Callee::Indirect(value) => Some(*value),
            Callee::Bound { closure, .. } => Some(*closure),
            _ => None,
        };
        if let Some(value) = closure
            && self.ed.unit.values[value.index()].holding == Holding::Owned
        {
            owners.insert(0, value);
        }
        let at = self.current();
        let mut next = self.ed.unit.tables.edges
            [self.ed.unit.tables.failures[tail.index()].cleanup.index()]
        .target;
        // Construct forwards so execution drains reverse evaluation order.
        for value in owners {
            let block = self.block(BlockKind::FailureCleanup, &[]);
            self.at(block);
            let failure = self.failure_to(next, &[], None);
            let (_, address) = super::iter::local(self, value)
                .expect("evaluated invocation owner has a closed local layout");
            self.forget_held(address);
            let anchor = self.drop_anchor();
            self.push(Inst::Drop {
                address,
                if_live: false,
                anchor,
                failure,
            });
            self.jump(next, &[]);
            next = block;
        }
        let rejected = self.block(BlockKind::FailureCleanup, &[]);
        self.at(rejected);
        for &loan in loans.iter().rev() {
            self.end_loan(loan);
        }
        self.jump(next, &[]);
        self.at(at);
        self.failure_to(rejected, &[], Some("LANG-10"))
    }

    pub fn call(
        &mut self,
        callee: Callee,
        operation_arguments: Vec<OperationArgument>,
        abi: FnAbiId,
        arguments: Vec<CallArgument>,
        loans: &[LoanId],
    ) -> Option<ValueId> {
        let invocation = matches!(callee, Callee::Indirect(_) | Callee::Bound { .. });
        if invocation {
            let tail = self.failure(None);
            let admission = self.admission_failure(&callee, &arguments, loans, tail);
            self.push(Inst::FrameEnter { failure: admission });
        }
        let pretransfer_failure = if invocation {
            let tail = self.failure(None);
            let rejected = self.admission_failure(&callee, &arguments, loans, tail);
            self.ed.unit.tables.failures[rejected.index()].rule = None;
            let edge = self.ed.unit.tables.failures[rejected.index()].cleanup;
            let cleanup = self.ed.unit.tables.edges[edge.index()].target;
            let at = self.current();
            let retire = self.block(BlockKind::FailureCleanup, &[]);
            self.at(retire);
            self.push(Inst::FrameLeave);
            self.jump(cleanup, &[]);
            self.at(at);
            self.ed.unit.tables.edges[edge.index()].target = retire;
            Some(rejected)
        } else {
            None
        };
        let shape = self.cx.layouts.table().fn_abis[abi.0 as usize].clone();
        let result = shape.result.map(|r| self.fresh(r.layout));
        let failed = self.block(BlockKind::FailureCleanup, &[]);
        let at = self.current();
        self.at(failed);
        if invocation {
            self.push(Inst::FrameLeave);
        }
        for &loan in loans.iter().rev() {
            self.end_loan(loan);
        }
        let failure = self.failure(None);
        let tail = self.ed.unit.tables.failures[failure.index()].cleanup;
        self.terminate(Terminator::Jump(tail));
        self.at(at);
        let failure = self.failure_to(failed, &[], None);
        let after = self.block(BlockKind::Body, &[]);
        let normal = self.edge(after, &[]);
        let arguments = PoolRange::push(&mut self.ed.unit.pools.call_arguments, arguments);
        let id = CallId(self.ed.unit.tables.calls.len() as u32);
        self.ed.unit.tables.calls.push(CallSite {
            receipts: None,
            pretransfer_failure,
            forward_caller: !matches!(callee, Callee::Indirect(_) | Callee::Bound { .. }),
            operation_arguments,
            callee,
            abi,
            arguments,
            result,
            destination: shape.destination.then_some(DestinationClass::Inferred),
            declared: crate::import::CALL_EFFECTS,
            anchor: self.anchor,
        });
        self.terminate(Terminator::Call {
            call: id,
            normal: Some(normal),
            failure,
        });
        self.at(after);
        if invocation {
            self.push(Inst::FrameLeave);
        }
        for &loan in loans.iter().rev() {
            self.end_loan(loan);
        }
        result
    }

    pub fn structural_access(&mut self, source: TypeId, layout: LayoutId) -> R<()> {
        if self
            .ed
            .unit
            .tables
            .structural
            .iter()
            .any(|p| p.root_layout() == layout)
        {
            return Ok(());
        }
        let plan = hirsel_layout::structural_access::at_layout_with_owners(
            self.cx.types,
            source,
            &mut |d| Ok(self.cx.program.signatures.get(d.path()).cloned()),
            self.cx.layouts.owners(),
            self.cx.layouts.table(),
            layout,
        )
        .map_err(|e| PassError::Layout(self.cx.path.clone(), e))?;
        self.ed.unit.tables.structural.push(plan);
        Ok(())
    }

    // --- symbols ---

    pub fn symbol(&mut self, symbol: Symbol) -> SymbolId {
        let symbols = &mut self.ed.unit.tables.symbols;
        match symbols.iter().position(|s| *s == symbol) {
            Some(i) => SymbolId(i as u32),
            None => {
                symbols.push(symbol);
                SymbolId(symbols.len() as u32 - 1)
            }
        }
    }

    /// Lay the blocks out. The unit still has to pass the Value validator.
    pub fn finish(self) -> LirUnit<Value> {
        self.ed.finish()
    }
}

/// The scalar kind of a layout, if it is one.
pub fn scalar(record: &LayoutRecord) -> Option<ScalarKind> {
    match record.kind {
        LayoutKind::Scalar(kind) => Some(kind),
        _ => None,
    }
}
