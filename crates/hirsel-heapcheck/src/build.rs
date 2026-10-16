//! Builders for hand-made layout tables and LIR units: the executor's fixtures,
//! and a convenient way for tests to state small programs. Offsets here follow
//! the obvious declaration-order recipe; they are fixture data, not the layout
//! derivation.
use hirsel_core_contract::{OperationId, Passing};
use hirsel_lir::abi_types::{CallMultiplicity, CallableShape, FnAbi, FnAbiId, FnAbiKey};
use hirsel_lir::effect::Effects;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::stage::{InputIdentity, Stage, StageKind};
use hirsel_syntax::{ContentHash, DefPath, ItemPath, PackageId};

/// A fixture definition path `fixture::heap::<name>`.
pub fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "fixture".into(),
            version: "0".into(),
        },
        modules: vec!["heap".into()],
        name: name.into(),
    })
}

/// A layout table under construction, with derived scan and drop programs.
pub struct Layouts {
    pub target: TargetSpec,
    pub table: LayoutTable,
}

impl Layouts {
    pub fn new(target: TargetSpec) -> Self {
        let mut table = LayoutTable::new(&target);
        table.fn_abis.push(FnAbi {
            key: FnAbiKey {
                callable: ContentHash::of_bytes(b"heapcheck fixture abi"),
                abi_version: hirsel_lir::ABI_VERSION,
            },
            shape: CallableShape::Definition,
            multiplicity: CallMultiplicity::Many,
            context: true,
            environment: false,
            destination: false,
            result: None,
            result_arena: None,
            diverges: false,
            parameters: Vec::new(),
            version: hirsel_lir::ABI_VERSION,
        });
        Self { target, table }
    }

    fn word(&self) -> u64 {
        u64::from(self.target.pointer_bytes)
    }

    fn push(&mut self, kind: LayoutKind, size: u64, align: u32) -> LayoutId {
        let id = LayoutId(self.table.layouts.len() as u32);
        self.table.layouts.push(LayoutRecord {
            key: LayoutKey {
                owner_revision: None,
                instance: None,
                target: self.table.target,
                layout_version: hirsel_lir::LAYOUT_VERSION,
            },
            kind,
            size: SizeSpec::Fixed(size),
            align,
            fields: Vec::new(),
            tag: None,
            initialized: InitializedBytes::Fields,
            scan: Vec::new(),
            drop: Vec::new(),
            copy: CopyRecipe::Bits,
            finalizer: None,
            storage: vec![StorageKind::Heap, StorageKind::LocalRegion],
            abi: AbiInputs {
                scalars: Vec::new(),
                unaligned: false,
            },
        });
        id
    }

    pub fn get(&self, id: LayoutId) -> &LayoutRecord {
        &self.table.layouts[id.0 as usize]
    }

    /// For deliberately broken tables.
    pub fn get_mut(&mut self, id: LayoutId) -> &mut LayoutRecord {
        &mut self.table.layouts[id.0 as usize]
    }

    fn fixed(&self, id: LayoutId) -> u64 {
        match self.get(id).size {
            SizeSpec::Fixed(n) => n,
            SizeSpec::Dynamic(_) => panic!("fixture layouts are fixed"),
        }
    }

    pub fn unit(&mut self) -> LayoutId {
        self.push(LayoutKind::Unit, 0, 1)
    }

    pub fn scalar(&mut self, kind: ScalarKind) -> LayoutId {
        let size = self.target.scalar_size(kind);
        let align = self.target.scalar_align(kind);
        let id = self.push(LayoutKind::Scalar(kind), size, align);
        self.get_mut(id).abi.scalars.push((0, kind));
        id
    }

    /// A counted block reference: one holder, released at death of its holder.
    pub fn block(&mut self, payload: LayoutId) -> LayoutId {
        let (word, align) = (self.word(), self.target.alignments.pointer);
        let id = self.push(LayoutKind::Block { payload }, word, align);
        let r = self.get_mut(id);
        r.scan = vec![ScanStep::Counted { offset: 0 }];
        r.drop = vec![DropStep::Release { offset: 0 }];
        r.copy = CopyRecipe::Dup;
        id
    }

    /// An arena token: one pointer word naming an open arena (or the heap).
    pub fn arena(&mut self) -> LayoutId {
        let (word, align) = (self.word(), self.target.alignments.pointer);
        let id = self.push(LayoutKind::Arena, word, align);
        self.get_mut(id).abi.scalars.push((0, ScalarKind::Pointer));
        id
    }

    /// A reference into uncounted (region or static) storage.
    pub fn uncounted(&mut self, payload: LayoutId) -> LayoutId {
        let (word, align) = (self.word(), self.target.alignments.pointer);
        let id = self.push(LayoutKind::Block { payload }, word, align);
        self.get_mut(id).scan = vec![ScanStep::Uncounted { offset: 0 }];
        id
    }

    fn lay_out(&self, fields: &[LayoutId], start: u64) -> (Vec<FieldLayout>, u64, u32) {
        let mut offset = start;
        let mut align = 1u32;
        let mut out = Vec::new();
        for (ordinal, &layout) in fields.iter().enumerate() {
            let a = self.get(layout).align.max(1);
            align = align.max(a);
            offset = offset.div_ceil(u64::from(a)) * u64::from(a);
            out.push(FieldLayout {
                ordinal: ordinal as u32,
                offset,
                layout,
                unaligned: false,
            });
            offset += self.fixed(layout);
        }
        (out, offset, align)
    }

    /// Scan, drop and copy programs of a field list: children in reverse
    /// declaration order.
    fn programs(&self, fields: &[FieldLayout]) -> (Vec<ScanStep>, Vec<DropStep>, CopyRecipe) {
        let mut scan = Vec::new();
        let mut drop = Vec::new();
        let mut copy = CopyRecipe::Bits;
        for fl in fields {
            let r = self.get(fl.layout);
            match r.kind {
                LayoutKind::Block { .. } | LayoutKind::Box { .. } => {
                    for step in &r.scan {
                        scan.push(match step {
                            ScanStep::Counted { .. } => ScanStep::Counted { offset: fl.offset },
                            _ => ScanStep::Uncounted { offset: fl.offset },
                        });
                    }
                    if !r.drop.is_empty() {
                        drop.push(DropStep::Release { offset: fl.offset });
                    }
                }
                LayoutKind::Callable { .. } => {
                    scan.push(ScanStep::Environment {
                        offset: fl.offset + self.word(),
                    });
                    drop.push(DropStep::Environment {
                        offset: fl.offset + self.word(),
                    });
                }
                _ if !r.drop.is_empty() || !r.scan.is_empty() => {
                    scan.push(ScanStep::Inline {
                        offset: fl.offset,
                        layout: fl.layout,
                    });
                    if !r.drop.is_empty() {
                        drop.push(DropStep::Child {
                            offset: fl.offset,
                            layout: fl.layout,
                        });
                    }
                }
                _ => {}
            }
            copy = match (copy, r.copy) {
                (CopyRecipe::NotCopyable, _) | (_, CopyRecipe::NotCopyable) => {
                    CopyRecipe::NotCopyable
                }
                (CopyRecipe::Bits, CopyRecipe::Bits) => CopyRecipe::Bits,
                _ => CopyRecipe::Fields,
            };
        }
        drop.reverse();
        (scan, drop, copy)
    }

    /// An inline record of `fields` in declaration order.
    pub fn record(&mut self, fields: &[LayoutId]) -> LayoutId {
        let (fields, end, align) = self.lay_out(fields, 0);
        let size = end.div_ceil(u64::from(align)) * u64::from(align);
        let (scan, drop, copy) = self.programs(&fields);
        let id = self.push(LayoutKind::Aggregate, size, align);
        let r = self.get_mut(id);
        r.fields = fields;
        r.scan = scan;
        r.drop = drop;
        r.copy = copy;
        id
    }

    /// A resource: user finalizer first, then its fields high to low.
    pub fn resource(&mut self, fields: &[LayoutId], finalizer: Option<DefPath>) -> LayoutId {
        let (fields, end, align) = self.lay_out(fields, 0);
        let size = end.div_ceil(u64::from(align)) * u64::from(align);
        let (scan, mut drop, _) = self.programs(&fields);
        if finalizer.is_some() {
            drop.insert(0, DropStep::Finalize);
        }
        let id = self.push(LayoutKind::Resource, size, align);
        let r = self.get_mut(id);
        r.fields = fields;
        r.scan = scan;
        r.drop = drop;
        r.copy = CopyRecipe::NotCopyable;
        r.finalizer = finalizer.map(|definition| FinalizerRef {
            definition,
            body: 0,
            unsafe_ffi_package: false,
        });
        id
    }

    /// An ordinary variant: a u32 tag, then each variant's fields.
    pub fn variant(&mut self, variants: &[&[LayoutId]]) -> LayoutId {
        let mut size = 4u64;
        let mut align = 4u32;
        let mut encoded = Vec::new();
        let mut scan_arms = Vec::new();
        let mut drop_arms = Vec::new();
        let mut copy = CopyRecipe::Bits;
        for (ordinal, fields) in variants.iter().enumerate() {
            let (fields, end, a) = self.lay_out(fields, 4);
            align = align.max(a);
            size = size.max(end);
            let (scan, drop, c) = self.programs(&fields);
            copy = match (copy, c) {
                (CopyRecipe::NotCopyable, _) | (_, CopyRecipe::NotCopyable) => {
                    CopyRecipe::NotCopyable
                }
                (CopyRecipe::Bits, CopyRecipe::Bits) => CopyRecipe::Bits,
                _ => CopyRecipe::Fields,
            };
            if !scan.is_empty() {
                scan_arms.push((ordinal as u32, scan));
            }
            if !drop.is_empty() {
                drop_arms.push((ordinal as u32, drop));
            }
            encoded.push(VariantLayout {
                ordinal: ordinal as u32,
                fields,
            });
        }
        let size = size.div_ceil(u64::from(align)) * u64::from(align);
        let id = self.push(LayoutKind::Variant, size, align);
        let r = self.get_mut(id);
        r.tag = Some(TagEncoding {
            offset: 0,
            form: TagForm::Ordinal,
            variants: encoded,
        });
        if !scan_arms.is_empty() {
            r.scan = vec![ScanStep::ActiveVariant {
                tag_offset: 0,
                form: TagForm::Ordinal,
                variants: scan_arms,
            }];
        }
        if !drop_arms.is_empty() {
            r.drop = vec![DropStep::ActiveVariant {
                tag_offset: 0,
                form: TagForm::Ordinal,
                variants: drop_arms,
            }];
        }
        r.copy = copy;
        id
    }

    /// A null-pointer niche (layout version 2): fieldless variant `nullary`
    /// and variant `1 - nullary` with `fields`, whose first block reference
    /// field's word is the tag, null for `nullary`.
    pub fn niche(&mut self, nullary: u32, fields: &[LayoutId]) -> LayoutId {
        let dataful = 1 - nullary;
        let (fields, end, align) = self.lay_out(fields, 0);
        let word = fields
            .iter()
            .find(|f| {
                matches!(
                    self.get(f.layout).kind,
                    LayoutKind::Block { .. } | LayoutKind::Box { .. }
                )
            })
            .expect("a niche needs a block reference field")
            .offset;
        let (scan, drop, copy) = self.programs(&fields);
        let size = end.div_ceil(u64::from(align)) * u64::from(align);
        let form = TagForm::Niche { nullary, dataful };
        let id = self.push(LayoutKind::Variant, size, align);
        let r = self.get_mut(id);
        r.tag = Some(TagEncoding {
            offset: word,
            form,
            variants: (0..2)
                .map(|ordinal| VariantLayout {
                    ordinal,
                    fields: if ordinal == dataful {
                        fields.clone()
                    } else {
                        Vec::new()
                    },
                })
                .collect(),
        });
        r.scan = vec![ScanStep::ActiveVariant {
            tag_offset: word,
            form,
            variants: vec![(dataful, scan)],
        }];
        r.drop = vec![DropStep::ActiveVariant {
            tag_offset: word,
            form,
            variants: vec![(dataful, drop)],
        }];
        r.copy = copy;
        id
    }

    /// A callable pair: code word, then environment reference.
    pub fn callable(&mut self, environment: Option<LayoutId>) -> LayoutId {
        let (word, align) = (self.word(), self.target.alignments.pointer);
        let id = self.push(LayoutKind::Callable { environment }, 2 * word, align);
        let r = self.get_mut(id);
        r.scan = vec![ScanStep::Environment { offset: word }];
        r.drop = vec![DropStep::Environment { offset: word }];
        r.copy = CopyRecipe::Dup;
        id
    }

    pub fn finish(self) -> (TargetSpec, LayoutTable) {
        (self.target, self.table)
    }
}

struct Draft {
    kind: BlockKind,
    parameters: Vec<ValueId>,
    instructions: Vec<Inst>,
    terminator: Option<Terminator>,
}

/// One unit under construction. Blocks are laid out contiguously at `finish`.
pub struct UnitBuilder<S> {
    pub unit: LirUnit<S>,
    drafts: Vec<Draft>,
}

/// The default source anchor every builder unit has.
pub const ANCHOR: SourceAnchorId = SourceAnchorId(0);
/// The default cleanup anchor.
pub const CLEANUP: CleanupAnchorId = CleanupAnchorId(0);

impl<S: Stage> UnitBuilder<S> {
    pub fn new(definition: DefPath) -> Self {
        let revision = ContentHash::of_parts("heapcheck fixture", [definition.0.name.as_bytes()]);
        let mut unit = LirUnit::<S>::new(UnitHeader {
            stage: StageKind::Value,
            lir_version: hirsel_lir::LIR_VERSION,
            origin: UnitOrigin::Source,
            input: InputIdentity {
                interface_version: hirsel_lir::INTERFACE_VERSION,
                digest: ContentHash::of_bytes(b"heapcheck fixture input"),
            },
            definition,
            implementation_revision: revision,
            instance: revision,
            target: ContentHash::of_bytes(b"unset"),
            layouts: ContentHash::of_bytes(b"unset"),
            abi: FnAbiId(0),
        });
        unit.tables.task_domains = 1;
        unit.scopes.push(ScopeInfo { parent: None });
        unit.tables.source_anchors.push(SourceAnchor {
            body: 0,
            block: 0,
            statement: 0,
            site: SiteRange { start: 0, end: 0 },
        });
        unit.tables.cleanup_anchors.push(CleanupAnchor {
            body: 0,
            cleanup: 0,
            action: 0,
            site: SiteRange { start: 0, end: 0 },
        });
        Self {
            unit,
            drafts: Vec::new(),
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
            base,
            projection,
            ty,
        });
        AddressId(self.unit.addresses.len() as u32 - 1)
    }

    pub fn block(&mut self, kind: BlockKind, parameters: &[ValueId]) -> BlockId {
        self.drafts.push(Draft {
            kind,
            parameters: parameters.to_vec(),
            instructions: Vec::new(),
            terminator: None,
        });
        BlockId(self.drafts.len() as u32 - 1)
    }

    pub fn push(&mut self, block: BlockId, inst: Inst) {
        self.drafts[block.index()].instructions.push(inst);
    }

    pub fn end(&mut self, block: BlockId, terminator: Terminator) {
        self.drafts[block.index()].terminator = Some(terminator);
    }

    pub fn edge(&mut self, target: BlockId, arguments: &[ValueId]) -> EdgeId {
        let arguments = PoolRange::push(&mut self.unit.pools.values, arguments.iter().copied());
        self.unit.tables.edges.push(Edge { target, arguments });
        EdgeId(self.unit.tables.edges.len() as u32 - 1)
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
            anchor: ANCHOR,
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
            constructor,
            layout,
            fields,
            destination: DestinationClass::Heap,
            placement: Some(placement),
            anchor: ANCHOR,
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
            code: Some(ClosureCode {
                symbol: code,
                abi: FnAbiId(0),
            }),
            environment,
            captures,
            destination: DestinationClass::Heap,
            placement: Some(placement),
            anchor: ANCHOR,
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
            anchor: ANCHOR,
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
            anchor: ANCHOR,
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

    /// Lay out the blocks and bind the unit to `table`.
    pub fn finish(mut self, table: &LayoutTable) -> LirUnit<S> {
        let unit = &mut self.unit;
        for draft in self.drafts {
            let parameters = PoolRange::push(&mut unit.pools.values, draft.parameters);
            let instructions = PoolRange::push(&mut unit.instructions, draft.instructions);
            unit.blocks.push(Block {
                kind: draft.kind,
                parameters,
                instructions,
                anchor: ANCHOR,
            });
            unit.terminators
                .push(draft.terminator.expect("every fixture block is terminated"));
        }
        unit.header.target = table.target;
        unit.header.layouts = table.digest();
        self.unit
    }
}
