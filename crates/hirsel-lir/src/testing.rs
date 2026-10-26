//! Raw LIR assembly shared by hand-built validator, backend and heap-checker fixtures.
//! Suite-specific layouts, identities, cleanup recipes and validation stay with the tests.
use crate::abi_types::FnAbiId;
use crate::effect::Effects;
use crate::ir::*;
use crate::layout_types::{LayoutId, LayoutTable};
use crate::stage::Stage;
use hirsel_core_contract::{OperationId, Passing};
use hirsel_syntax::DefPath;

/// Buffers each block's instructions and terminator until the unit is finished.
/// The caller initializes the header and side tables and declares the entry block.
/// Convenience helpers use source anchor, scope and ABI zero; the caller supplies
/// those entries. Constructions and calls default to the task heap.
pub struct UnitBuilder<S> {
    pub unit: LirUnit<S>,
    insts: Vec<Vec<Inst>>,
    terms: Vec<Option<Terminator>>,
    propagate: Option<BlockId>,
}

impl<S: Stage> UnitBuilder<S> {
    pub fn new(unit: LirUnit<S>) -> Self {
        Self {
            unit,
            insts: Vec::new(),
            terms: Vec::new(),
            propagate: None,
        }
    }
    pub fn ty(&mut self, repr: ValueRepr, layout: LayoutId) -> LirTypeId {
        let ty = LirType { repr, layout };
        if let Some(i) = self.unit.types.iter().position(|t| *t == ty) {
            return LirTypeId(i as u32);
        }
        self.unit.types.push(ty);
        LirTypeId(self.unit.types.len() as u32 - 1)
    }

    pub fn value(&mut self, repr: ValueRepr, layout: LayoutId, holding: Holding) -> ValueId {
        let ty = self.ty(repr, layout);
        self.unit.values.push(ValueInfo { ty, holding });
        ValueId(self.unit.values.len() as u32 - 1)
    }

    pub fn owned(&mut self, repr: ValueRepr, layout: LayoutId) -> ValueId {
        self.value(repr, layout, Holding::Owned)
    }

    pub fn plain(&mut self, repr: ValueRepr, layout: LayoutId) -> ValueId {
        self.value(repr, layout, Holding::Plain)
    }

    /// Declare `value` as the next parameter.
    pub fn parameter(&mut self, value: ValueId) {
        self.unit.parameters.push(value);
    }

    /// A slot in the unit's one scope. A capture slot also gets the version
    /// name of its environment content (`LirUnit::captures`).
    pub fn slot(&mut self, repr: ValueRepr, layout: LayoutId, kind: SlotKind) -> SlotId {
        let ty = self.ty(repr, layout);
        self.unit.slots.push(SlotInfo {
            ty,
            kind,
            scope: ScopeId(0),
        });
        if let SlotKind::Capture(i) = kind {
            let version = self.plain(repr, layout);
            let captures = &mut self.unit.captures;
            if captures.len() <= i as usize {
                captures.resize(i as usize + 1, version);
            }
            captures[i as usize] = version;
        }
        SlotId(self.unit.slots.len() as u32 - 1)
    }

    pub fn address(
        &mut self,
        base: AddressBase,
        steps: &[ProjectionStep],
        repr: ValueRepr,
        layout: LayoutId,
    ) -> AddressId {
        let ty = self.ty(repr, layout);
        let projection = PoolRange::push(&mut self.unit.pools.projections, steps.iter().copied());
        self.unit.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base,
            projection,
            ty,
        });
        AddressId(self.unit.addresses.len() as u32 - 1)
    }

    pub fn failure(
        &mut self,
        cleanup: EdgeId,
        rule: Option<&str>,
        dispose: &[ValueId],
    ) -> FailureId {
        let dispose = PoolRange::push(&mut self.unit.pools.values, dispose.iter().copied());
        self.unit.tables.failures.push(FailureEdge {
            cleanup,
            rule: rule.map(str::to_string),
            anchor: SourceAnchorId(0),
            dispose,
        });
        FailureId(self.unit.tables.failures.len() as u32 - 1)
    }

    pub fn symbol(&mut self, kind: SymbolKind, definition: DefPath) -> SymbolId {
        let symbol = Symbol {
            kind,
            definition,
            instance: None,
        };
        if let Some(i) = self.unit.tables.symbols.iter().position(|s| *s == symbol) {
            return SymbolId(i as u32);
        }
        self.unit.tables.symbols.push(symbol);
        SymbolId(self.unit.tables.symbols.len() as u32 - 1)
    }

    pub fn construct(
        &mut self,
        constructor: ConstructorKind,
        layout: LayoutId,
        fields: &[(u32, ValueId)],
        placement: Placement,
    ) -> ConstructId {
        let fields = PoolRange::push(
            &mut self.unit.pools.fields,
            fields
                .iter()
                .map(|&(field, value)| FieldValue { field, value }),
        );
        self.unit.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            tracked: false,
            constructor,
            layout,
            fields,
            destination: DestinationClass::Heap,
            placement: Some(placement),
            anchor: SourceAnchorId(0),
        });
        ConstructId(self.unit.tables.constructs.len() as u32 - 1)
    }

    pub fn closure(
        &mut self,
        code: DefPath,
        environment: LayoutId,
        captures: &[(ValueId, CaptureUse)],
        placement: Placement,
    ) -> ClosureId {
        let code = self.symbol(SymbolKind::Function, code);
        let captures = PoolRange::push(
            &mut self.unit.pools.captures,
            captures
                .iter()
                .map(|&(value, usage)| CaptureValue { value, usage }),
        );
        self.unit.tables.closures.push(ClosureSite {
            receipts: None,
            scoped: false,
            code: Some(ClosureCode {
                symbol: code,
                abi: FnAbiId(0),
            }),
            environment,
            captures,
            destination: DestinationClass::Heap,
            placement: Some(placement),
            anchor: SourceAnchorId(0),
        });
        ClosureId(self.unit.tables.closures.len() as u32 - 1)
    }

    pub fn call(
        &mut self,
        callee: Callee,
        arguments: &[(Passing, ArgumentValue)],
        result: Option<ValueId>,
    ) -> CallId {
        let arguments = PoolRange::push(
            &mut self.unit.pools.call_arguments,
            arguments
                .iter()
                .enumerate()
                .map(|(i, &(passing, value))| CallArgument {
                    parameter: i as u32,
                    passing,
                    value,
                }),
        );
        self.unit.tables.calls.push(CallSite {
            receipts: None,
            pretransfer_failure: None,
            forward_caller: true,
            operation_arguments: Vec::new(),
            callee,
            abi: FnAbiId(0),
            arguments,
            result,
            destination: Some(DestinationClass::Heap),
            declared: Effects::READS
                | Effects::WRITES
                | Effects::OWNS
                | Effects::MAY_FAIL
                | Effects::ALLOCATES
                | Effects::OBSERVABLE
                | Effects::BARRIER,
            anchor: SourceAnchorId(0),
        });
        CallId(self.unit.tables.calls.len() as u32 - 1)
    }

    pub fn direct(&mut self, definition: DefPath) -> Callee {
        Callee::Direct(self.symbol(SymbolKind::Function, definition))
    }

    pub fn operation(&mut self, id: OperationId) -> Callee {
        self.unit.tables.operations.push(id);
        Callee::Operation(OperationRef(self.unit.tables.operations.len() as u32 - 1))
    }

    pub fn loan(&mut self, address: AddressId, mutable: bool) -> LoanId {
        self.unit.tables.loans.push(LoanInfo { address, mutable });
        LoanId(self.unit.tables.loans.len() as u32 - 1)
    }

    /// A static mask over `fields` fields with `live` set.
    pub fn mask(&mut self, fields: u32, live: &[u32]) -> MaskId {
        let mut words = vec![0u64; (fields as usize).div_ceil(64)];
        for &f in live {
            words[f as usize / 64] |= 1 << (f % 64);
        }
        let words = PoolRange::push(&mut self.unit.pools.mask_words, words);
        self.unit.tables.masks.push(FieldMask { fields, words });
        MaskId(self.unit.tables.masks.len() as u32 - 1)
    }

    pub fn region(&mut self) -> RegionInstanceId {
        self.unit.tables.regions.push(RegionInstance {
            scope: ScopeId(0),
            anchor: SourceAnchorId(0),
            seal_demand: false,
        });
        RegionInstanceId(self.unit.tables.regions.len() as u32 - 1)
    }

    pub fn constant(
        &mut self,
        repr: ValueRepr,
        layout: LayoutId,
        value: ConstantValue,
    ) -> ConstantId {
        let ty = self.ty(repr, layout);
        self.unit.tables.constants.push(ConstantData { ty, value });
        ConstantId(self.unit.tables.constants.len() as u32 - 1)
    }

    pub fn block(&mut self, kind: BlockKind, params: &[ValueId]) -> BlockId {
        let parameters = PoolRange::push(&mut self.unit.pools.values, params.iter().copied());
        self.unit.blocks.push(Block {
            kind,
            parameters,
            instructions: PoolRange::EMPTY,
            anchor: SourceAnchorId(0),
        });
        self.insts.push(Vec::new());
        self.terms.push(None);
        BlockId(self.unit.blocks.len() as u32 - 1)
    }

    pub fn push(&mut self, b: BlockId, inst: Inst) {
        self.insts[b.index()].push(inst);
    }

    pub fn term(&mut self, b: BlockId, term: Terminator) {
        self.terms[b.index()] = Some(term);
    }

    pub fn edge(&mut self, target: BlockId, args: &[ValueId]) -> EdgeId {
        let arguments = PoolRange::push(&mut self.unit.pools.values, args.iter().copied());
        self.unit.tables.edges.push(Edge { target, arguments });
        EdgeId(self.unit.tables.edges.len() as u32 - 1)
    }

    pub fn jump(&mut self, from: BlockId, to: BlockId, args: &[ValueId]) {
        let e = self.edge(to, args);
        self.term(from, Terminator::Jump(e));
    }

    pub fn propagate(&mut self) -> BlockId {
        if let Some(b) = self.propagate {
            return b;
        }
        let b = self.block(BlockKind::FailureCleanup, &[]);
        self.term(b, Terminator::Propagate);
        self.propagate = Some(b);
        b
    }

    pub fn ret(&mut self, b: BlockId, value: Option<ValueId>) {
        self.term(b, Terminator::Return(value));
    }

    /// Bind the finished unit to its fixture layout table.
    pub fn finish_with_layouts(mut self, table: &LayoutTable) -> LirUnit<S> {
        self.unit.header.target = table.target;
        self.finish()
    }

    pub fn finish(mut self) -> LirUnit<S> {
        for (b, insts) in self.insts.iter().enumerate() {
            let range = PoolRange::push(&mut self.unit.instructions, insts.iter().copied());
            self.unit.blocks[b].instructions = range;
        }
        self.unit.terminators = self
            .terms
            .iter()
            .enumerate()
            .map(|(b, t)| {
                t.clone()
                    .unwrap_or_else(|| panic!("block {b} has no terminator"))
            })
            .collect();
        self.unit
    }
}
