//! Schema fixtures: deterministic round trips, identity checks, the instruction
//! row bound, opcode coverage, instance-key erasure and the operation
//! manifest skeleton.
use hirsel_codec::{self as snapshot, Snapshot};
use hirsel_core_contract::{
    CallOwnership, CleanupEffect, Disposition, EngineDispositions, M2Feature, Manifest,
    OperationId, OperationRow, ParameterOwnership, Passing, SemanticEffects, TestVectorId,
    Unavailable,
};
use hirsel_layout::instance::{
    ActualBinder, CallableConvention, CaptureView, InstanceKey, InstanceKeyError, InstanceView,
    ParameterView, Substitution,
};
use hirsel_lir::abi_types::*;
use hirsel_lir::codec::{CodecNode, CodecNodeId, CodecPlan};
use hirsel_lir::effect::{Effects, Opcode};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::runtime_abi::{self, RuntimeEntry, SizeClass};
use hirsel_lir::snapshot as lir_snapshot;
use hirsel_lir::stage::*;
use hirsel_syntax::{ContentHash, DefPath, ItemPath, PackageId};

fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "fixture".into(),
            version: "0".into(),
        },
        modules: vec!["m".into()],
        name: name.into(),
    })
}

fn hash(label: &str) -> ContentHash {
    ContentHash::of_bytes(label.as_bytes())
}

fn layout(table: &LayoutTable, kind: LayoutKind, size: SizeSpec, align: u32) -> LayoutRecord {
    LayoutRecord {
        key: LayoutIdentity {
            owner_revision: None,
            instance: None,
            target: table.target,
            layout_version: LAYOUT_VERSION_FOR_FIXTURES,
        },
        kind,
        size,
        align,
        fields: Vec::new(),
        tag: None,
        initialized: InitializedBytes::Fields,
        scan: Vec::new(),
        drop: Vec::new(),
        copy: CopyRecipe::Bits,
        finalizer: None,
        storage: vec![StorageKind::Heap],
        abi: AbiInputs {
            scalars: Vec::new(),
            unaligned: false,
        },
    }
}
const LAYOUT_VERSION_FOR_FIXTURES: u32 = hirsel_lir::LAYOUT_VERSION;

/// One record per layout family, plus one FnAbi per callable shape.
fn fixture_table(target: &TargetSpec) -> LayoutTable {
    let mut t = LayoutTable::new(target);
    let id = LayoutId;
    let fixed = SizeSpec::Fixed;
    let mut records = vec![
        layout(&t, LayoutKind::Unit, fixed(0), 1),
        layout(&t, LayoutKind::Scalar(ScalarKind::I64), fixed(8), 8),
    ];
    let mut aggregate = layout(&t, LayoutKind::Aggregate, fixed(16), 8);
    aggregate.fields = vec![
        FieldLayout {
            ordinal: 0,
            offset: 0,
            layout: id(1),
            unaligned: false,
        },
        FieldLayout {
            ordinal: 1,
            offset: 9,
            layout: id(1),
            unaligned: true,
        },
    ];
    aggregate.initialized = InitializedBytes::ZeroPadding;
    aggregate.copy = CopyRecipe::Fields;
    aggregate.abi = AbiInputs {
        scalars: vec![(0, ScalarKind::I64), (9, ScalarKind::I64)],
        unaligned: true,
    };
    records.push(aggregate);
    let mut variant = layout(&t, LayoutKind::Variant, fixed(16), 8);
    variant.tag = Some(TagEncoding {
        offset: 0,
        form: TagForm::Ordinal,
        variants: vec![
            VariantLayout {
                ordinal: 0,
                fields: Vec::new(),
            },
            VariantLayout {
                ordinal: 1,
                fields: vec![FieldLayout {
                    ordinal: 0,
                    offset: 8,
                    layout: id(7),
                    unaligned: false,
                }],
            },
        ],
    });
    variant.scan = vec![ScanStep::ActiveVariant {
        tag_offset: 0,
        form: TagForm::Ordinal,
        variants: vec![(1, vec![ScanStep::Counted { offset: 8 }])],
    }];
    variant.drop = vec![DropStep::ActiveVariant {
        tag_offset: 0,
        form: TagForm::Ordinal,
        variants: vec![(1, vec![DropStep::Release { offset: 8 }])],
    }];
    records.push(variant);
    records.push(layout(
        &t,
        LayoutKind::Packed,
        SizeSpec::Dynamic(vec![
            SizeTerm::Constant(4),
            SizeTerm::ReadLength {
                offset: 0,
                scalar: ScalarKind::U32,
            },
            SizeTerm::Constant(2),
            SizeTerm::Multiply,
            SizeTerm::Add,
            SizeTerm::AlignUp(8),
        ]),
        8,
    ));
    records.push(layout(
        &t,
        LayoutKind::Inline {
            element: id(1),
            count: 4,
        },
        fixed(32),
        8,
    ));
    records.push(layout(
        &t,
        LayoutKind::Simd {
            element: id(1),
            lanes: 2,
        },
        fixed(16),
        16,
    ));
    let mut block = layout(&t, LayoutKind::Block { payload: id(2) }, fixed(8), 8);
    block.scan = vec![ScanStep::Counted { offset: 0 }];
    block.drop = vec![DropStep::Release { offset: 0 }];
    block.copy = CopyRecipe::Dup;
    records.push(block);
    let mut slice = layout(&t, LayoutKind::Slice { element: id(1) }, fixed(24), 8);
    slice.scan = vec![ScanStep::SliceOwner { offset: 0 }];
    records.push(slice);
    let mut opaque = layout(&t, LayoutKind::Opaque { owner: path("Map") }, fixed(48), 8);
    opaque.scan = vec![ScanStep::Sequence {
        count_offset: 0,
        offset: 8,
        stride: 16,
        element: id(2),
    }];
    opaque.drop = vec![DropStep::ElementsReverse {
        count_offset: 0,
        offset: 8,
        stride: 16,
        element: id(2),
    }];
    records.push(opaque);
    let mut handle = layout(&t, LayoutKind::Handle, fixed(8), 8);
    handle.scan = vec![ScanStep::Handle {
        offset: 0,
        kind: hirsel_lir::layout_types::HandleKind::Unknown,
        ty: None,
        obtains: None,
    }];
    handle.drop = vec![DropStep::CloseHandle { offset: 0 }];
    records.push(handle);
    let mut callable = layout(
        &t,
        LayoutKind::Callable {
            environment: Some(id(2)),
        },
        fixed(16),
        8,
    );
    callable.scan = vec![ScanStep::Environment { offset: 8 }];
    callable.drop = vec![DropStep::Environment { offset: 8 }];
    records.push(callable);
    let mut iterator = layout(&t, LayoutKind::Iterator { element: id(1) }, fixed(24), 8);
    iterator.copy = CopyRecipe::NotCopyable;
    records.push(iterator);
    let mut resource = layout(&t, LayoutKind::Resource, fixed(16), 8);
    resource.finalizer = Some(FinalizerRef {
        definition: path("File.drop"),
        body: 0,
        unsafe_ffi_package: false,
    });
    resource.drop = vec![
        DropStep::Finalize,
        DropStep::Child {
            offset: 8,
            layout: id(10),
        },
    ];
    resource.copy = CopyRecipe::NotCopyable;
    resource.storage = vec![StorageKind::Heap, StorageKind::LocalRegion];
    records.push(resource);
    records.push(layout(&t, LayoutKind::Header, fixed(48), 8));
    let mut flat = layout(&t, LayoutKind::Flat, fixed(8), 8);
    flat.copy = CopyRecipe::FlatBytes;
    flat.scan = vec![ScanStep::Uncounted { offset: 0 }];
    flat.storage = vec![
        StorageKind::Heap,
        StorageKind::LocalRegion,
        StorageKind::Arena,
        StorageKind::Frozen,
        StorageKind::Static,
    ];
    records.push(flat);
    let mut soa = layout(&t, LayoutKind::SoaColumns { element: id(2) }, fixed(24), 8);
    soa.scan = vec![ScanStep::Inline {
        offset: 0,
        layout: id(2),
    }];
    records.push(soa);
    t.layouts = records;
    let slot = |ty, size, align| PhysicalSlot { ty, size, align };
    let parameters = vec![
        AbiParameter {
            source: 0,
            convention: Passing::Borrow,
            passing: ParameterPassing::ByValue,
            layout: id(1),
            slots: vec![slot(PhysicalType::I8, 1, 1)],
            extension: ScalarExtension::Sign,
        },
        AbiParameter {
            source: 1,
            convention: Passing::Borrow,
            passing: ParameterPassing::ReadOnlyAddress,
            layout: id(2),
            slots: vec![slot(PhysicalType::Pointer, 8, 8)],
            extension: ScalarExtension::None,
        },
        AbiParameter {
            source: 2,
            convention: Passing::Mut,
            passing: ParameterPassing::ExclusiveAddress,
            layout: id(2),
            slots: vec![slot(PhysicalType::Pointer, 8, 8)],
            extension: ScalarExtension::None,
        },
        AbiParameter {
            source: 3,
            convention: Passing::Take,
            passing: ParameterPassing::TransferAddress,
            layout: id(2),
            slots: vec![slot(PhysicalType::Pointer, 8, 8)],
            extension: ScalarExtension::None,
        },
        AbiParameter {
            source: 4,
            convention: Passing::Take,
            passing: ParameterPassing::PointerLength,
            layout: id(15),
            slots: vec![
                slot(PhysicalType::Pointer, 8, 8),
                slot(PhysicalType::I64, 8, 8),
            ],
            extension: ScalarExtension::Zero,
        },
        AbiParameter {
            source: 5,
            convention: Passing::Borrow,
            passing: ParameterPassing::ByValue,
            layout: id(1),
            slots: vec![
                slot(PhysicalType::I16, 2, 2),
                slot(PhysicalType::I32, 4, 4),
                slot(PhysicalType::I128, 16, 16),
                slot(PhysicalType::F32, 4, 4),
                slot(PhysicalType::F64, 8, 8),
            ],
            extension: ScalarExtension::None,
        },
    ];
    for (shape, multiplicity) in [
        (CallableShape::Definition, CallMultiplicity::Many),
        (CallableShape::Closure, CallMultiplicity::Once),
        (CallableShape::IterNext, CallMultiplicity::Many),
        (CallableShape::IterDrop, CallMultiplicity::Once),
        (CallableShape::Foreign, CallMultiplicity::Many),
    ] {
        t.fn_abis.push(FnAbi {
            receipts: None,
            nonretaining: Vec::new(),
            nonretaining_captures: Vec::new(),
            scoped_contract: hirsel_lir::abi::scoped_contract_digest(&[], &[]),
            scoped_entries: Vec::new(),
            key: FnAbiKey {
                receipt_contract: None,
                callable: hash(&format!("{shape:?}")),
                abi_version: hirsel_lir::ABI_VERSION,
            },
            shape,
            multiplicity,
            context: shape != CallableShape::Foreign,
            environment: matches!(
                shape,
                CallableShape::Closure | CallableShape::IterNext | CallableShape::IterDrop
            ),
            caller_site: matches!(
                shape,
                CallableShape::Closure | CallableShape::IterNext | CallableShape::IterDrop
            ),
            destination: shape == CallableShape::Definition,
            result: (shape != CallableShape::IterDrop).then(|| AbiResult {
                borrowed_parameters: vec![],
                layout: id(2),
                register: None,
                slots: vec![slot(PhysicalType::Pointer, 8, 8)],
            }),
            result_arena: None,
            diverges: false,
            parameters: if shape == CallableShape::Definition {
                parameters.clone()
            } else {
                Vec::new()
            },
            version: hirsel_lir::ABI_VERSION,
        });
    }
    t
}

/// A referentially valid unit using every instruction and terminator variant and
/// every enum family the unit schema contains.
fn fixture_unit<S: Stage>(table: &LayoutTable) -> LirUnit<S> {
    let mut u = LirUnit::<S>::new(UnitHeader {
        source_count: 1,
        key: hirsel_lir::objects::UnitKey(hirsel_syntax::ContentHash([0; 32])),
        track_caller: false,
        stage: StageKind::Value,
        lir_version: hirsel_lir::LIR_VERSION,
        origin: UnitOrigin::Source,
        definition: path("f"),
        implementation_revision: hash("f source"),
        instance: hash("f instance"),
        target: table.target,
        abi: FnAbiId(0),
    });
    let reprs = [
        (ValueRepr::Unit, 0),
        (ValueRepr::Scalar(ScalarKind::I64), 1),
        (ValueRepr::Block, 7),
        (ValueRepr::Uncounted, 15),
        (ValueRepr::Aggregate, 2),
        (ValueRepr::Callable, 11),
        (ValueRepr::Handle, 10),
        (ValueRepr::Token, 14),
        (ValueRepr::RegionMark, 14),
        (ValueRepr::InitMask { fields: 2 }, 2),
        (ValueRepr::Scalar(ScalarKind::Bool), 1),
    ];
    u.types = reprs
        .iter()
        .map(|&(repr, layout)| LirType {
            repr,
            layout: LayoutId(layout),
        })
        .collect();
    let holdings = [Holding::Plain, Holding::Owned, Holding::Borrowed(LoanId(0))];
    u.values = (0..66)
        .map(|i| ValueInfo {
            ty: LirTypeId(i % reprs.len() as u32),
            holding: holdings[i as usize % holdings.len()],
        })
        .collect();
    let v = ValueId;
    u.parameters = vec![v(0), v(1)];
    u.captures = vec![v(4)];
    u.scopes = vec![
        ScopeInfo { parent: None },
        ScopeInfo {
            parent: Some(ScopeId(0)),
        },
    ];
    u.slots = [
        SlotKind::Parameter(0),
        SlotKind::Local,
        SlotKind::Temporary,
        SlotKind::Capture(0),
        SlotKind::ReturnSlot,
        SlotKind::LoopCarried(0),
        SlotKind::Displaced,
    ]
    .into_iter()
    .map(|kind| SlotInfo {
        ty: LirTypeId(4),
        kind,
        scope: ScopeId(0),
    })
    .collect();
    let p = &mut u.pools;
    let projection = PoolRange::push(
        &mut p.projections,
        [
            ProjectionStep::Field(0),
            ProjectionStep::Index(v(1)),
            ProjectionStep::VariantField {
                variant: 1,
                field: 0,
            },
            ProjectionStep::Slot(v(1)),
            ProjectionStep::FlatAt {
                index: v(1),
                offset: v(10),
            },
        ],
    );
    u.addresses = vec![
        AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Slot(SlotId(1)),
            projection,
            ty: LirTypeId(4),
        },
        AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Value(v(2)),
            projection: PoolRange::EMPTY,
            ty: LirTypeId(4),
        },
        AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: AddressBase::Address(AddressId(0)),
            projection: PoolRange::EMPTY,
            ty: LirTypeId(1),
        },
    ];
    let fields_of = PoolRange::push(&mut p.addresses, [AddressId(1), AddressId(2)]);
    let block_args = PoolRange::push(&mut p.values, [v(1)]);
    let dispose = PoolRange::push(&mut p.values, [v(7)]);
    let construct_fields = PoolRange::push(
        &mut p.fields,
        [
            FieldValue {
                field: 0,
                value: v(1),
            },
            FieldValue {
                field: 1,
                value: v(12),
            },
        ],
    );
    let captures = PoolRange::push(
        &mut p.captures,
        [
            CaptureUse::Borrow,
            CaptureUse::Take,
            CaptureUse::Copy,
            CaptureUse::Mut,
        ]
        .map(|usage| CaptureValue { value: v(4), usage }),
    );
    let arguments = PoolRange::push(
        &mut p.call_arguments,
        [
            CallArgument {
                parameter: 0,
                passing: Passing::Borrow,
                value: ArgumentValue::Value(v(1)),
            },
            CallArgument {
                parameter: 2,
                passing: Passing::Mut,
                value: ArgumentValue::Address {
                    address: AddressId(0),
                    version: v(4),
                },
            },
            CallArgument {
                parameter: 3,
                passing: Passing::Take,
                value: ArgumentValue::Value(v(2)),
            },
        ],
    );
    let words = PoolRange::push(&mut p.mask_words, [0b01]);
    let t = &mut u.tables;
    t.task_domains = 1;
    t.source_anchors = vec![SourceAnchor {
        body: 0,
        block: 0,
        statement: 3,
        site: SiteRange {
            source: 0,
            start: 10,
            end: 24,
        },
    }];
    t.cleanup_anchors = vec![CleanupAnchor {
        body: 0,
        cleanup: 1,
        action: 0,
        site: SiteRange {
            source: 0,
            start: 30,
            end: 31,
        },
    }];
    t.loans = vec![LoanInfo {
        address: AddressId(0),
        mutable: false,
    }];
    t.masks = vec![FieldMask { fields: 2, words }];
    t.regions = vec![RegionInstance {
        scope: ScopeId(1),
        anchor: SourceAnchorId(0),
        seal_demand: false,
    }];
    t.arenas = vec![ExternalInstance { slot: SlotId(1) }];
    t.shared = vec![ExternalInstance { slot: SlotId(2) }];
    t.operations = vec![OperationId {
        definition: path("print"),
        declaration_digest: hash("print decl"),
    }];
    t.symbols = [
        SymbolKind::Function,
        SymbolKind::Constant,
        SymbolKind::Type,
        SymbolKind::Finalizer,
        SymbolKind::Shape,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, kind)| Symbol {
        kind,
        definition: path(&format!("s{i}")),
        instance: (i % 2 == 0).then(|| hash("instance")),
    })
    .collect();
    t.constants = [
        ConstantValue::Heap,
        ConstantValue::Unit,
        ConstantValue::Bool(true),
        ConstantValue::Integer(u128::MAX),
        ConstantValue::F32(0x7fc0_0000),
        ConstantValue::F64(0x8000_0000_0000_0000),
        ConstantValue::String("é \"quoted\" \\ \u{1F600}".into()),
        ConstantValue::Bytes(vec![0, 255, 16]),
    ]
    .into_iter()
    .map(|value| ConstantData {
        ty: LirTypeId(1),
        value,
    })
    .collect();
    let destinations = [
        DestinationClass::Heap,
        DestinationClass::Arena(SlotId(1)),
        DestinationClass::Caller(vec![0, 2]),
        DestinationClass::Region,
        DestinationClass::Inferred,
    ];
    let placements = [
        None,
        Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)))),
        Some(Placement::Fixed(StorageOwner::LocalRegion(
            RegionInstanceId(0),
        ))),
        Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0)))),
        Some(Placement::Fixed(StorageOwner::Frozen(SharedInstanceId(0)))),
        Some(Placement::Fixed(StorageOwner::Static)),
        Some(Placement::CallerDestination),
    ];
    t.constructs = [
        ConstructorKind::Tuple,
        ConstructorKind::Record(SymbolId(2)),
        ConstructorKind::Variant {
            definition: SymbolId(2),
            variant: 1,
        },
        ConstructorKind::Sequence,
        ConstructorKind::Box,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, constructor)| ConstructSite {
        arena_no_drop: false,
        tracked: false,
        constructor,
        layout: LayoutId(2),
        fields: construct_fields,
        destination: destinations[i % destinations.len()].clone(),
        placement: placements[i % placements.len()],
        anchor: SourceAnchorId(0),
    })
    .collect();
    t.closures = vec![ClosureSite {
        receipts: None,
        scoped: false,
        code: Some(ClosureCode {
            symbol: SymbolId(0),
            abi: FnAbiId(1),
        }),
        environment: LayoutId(2),
        captures,
        destination: DestinationClass::Inferred,
        placement: Some(Placement::Fixed(StorageOwner::LocalRegion(
            RegionInstanceId(0),
        ))),
        anchor: SourceAnchorId(0),
    }];
    t.calls = [
        Callee::Direct(SymbolId(0)),
        Callee::Indirect(v(5)),
        Callee::Operation(OperationRef(0)),
    ]
    .into_iter()
    .map(|callee| CallSite {
        receipts: None,
        pretransfer_failure: None,
        forward_caller: true,
        operation_arguments: Vec::new(),
        callee,
        abi: FnAbiId(0),
        arguments,
        result: Some(v(30)),
        destination: Some(DestinationClass::Caller(vec![0])),
        declared: Effects::READS | Effects::MAY_FAIL,
        anchor: SourceAnchorId(0),
    })
    .collect();
    let edge = |target: u32, arguments| Edge {
        target: BlockId(target),
        arguments,
    };
    t.edges = vec![
        edge(1, block_args),
        edge(2, PoolRange::EMPTY),
        edge(3, PoolRange::EMPTY),
        edge(4, PoolRange::EMPTY),
        edge(5, PoolRange::EMPTY),
        edge(6, PoolRange::EMPTY),
        edge(7, PoolRange::EMPTY),
    ];
    t.failures = vec![
        FailureEdge {
            cleanup: EdgeId(6),
            rule: Some("MEM-14".into()),
            anchor: SourceAnchorId(0),
            dispose,
        },
        FailureEdge {
            cleanup: EdgeId(6),
            rule: None,
            anchor: SourceAnchorId(0),
            dispose: PoolRange::EMPTY,
        },
    ];
    t.codecs = vec![CodecPlan {
        descriptor: vec![4],
        nodes: vec![
            CodecNode::Scalar(LayoutId(0)),
            CodecNode::Text {
                layout: LayoutId(0),
                storage: LayoutId(0),
                utf8: false,
            },
            CodecNode::Variant {
                layout: LayoutId(0),
                arms: vec![vec![CodecNodeId(0)], Vec::new()],
            },
            CodecNode::FlatText {
                layout: LayoutId(0),
                utf8: true,
            },
            CodecNode::Framed {
                layout: LayoutId(0),
                value: CodecNodeId(3),
            },
        ],
        root: CodecNodeId(0),
        bytes: CodecNodeId(1),
        result: Some(CodecNodeId(2)),
        checks: vec![],
    }];
    let arms = PoolRange::push(
        &mut u.pools.switch_arms,
        [
            SwitchArm {
                value: 0,
                edge: EdgeId(2),
            },
            SwitchArm {
                value: 1,
                edge: EdgeId(3),
            },
        ],
    );
    let f = FailureId(0);
    let a = AddressId;
    let anchor = CleanupAnchorId(0);
    let mut insts = vec![
        Inst::Const {
            result: v(10),
            constant: ConstantId(2),
        },
        Inst::FunctionRef {
            result: v(5),
            symbol: SymbolId(0),
        },
        Inst::ReadStatic {
            result: v(11),
            symbol: SymbolId(1),
        },
        Inst::Check {
            condition: v(10),
            failure: f,
        },
        Inst::StorageLive { slot: SlotId(1) },
        Inst::StorageDead {
            slot: SlotId(2),
            if_live: true,
        },
        Inst::EndScope { scope: ScopeId(1) },
        Inst::Address {
            address: a(0),
            failure: f,
        },
        Inst::Read {
            result: v(12),
            address: a(0),
            reaching: v(4),
            mode: ReadMode::Borrow,
        },
        Inst::Read {
            result: v(13),
            address: a(1),
            reaching: v(4),
            mode: ReadMode::Take,
        },
        Inst::Read {
            result: v(14),
            address: a(2),
            reaching: v(4),
            mode: ReadMode::Bits,
        },
        Inst::Copy {
            result: v(15),
            address: a(0),
            reaching: v(4),
            failure: f,
        },
        Inst::CopyValue {
            result: v(15),
            value: v(4),
        },
        Inst::FlatCopy {
            result: v(16),
            address: a(0),
            layout: LayoutId(15),
            failure: f,
        },
        Inst::UniqueCopy {
            result: v(17),
            value: v(2),
            construct: ConstructId(0),
            deep: true,
            failure: f,
        },
        Inst::Store {
            address: a(0),
            value: v(4),
            version: v(18),
        },
        Inst::Replace {
            address: a(0),
            value: v(4),
            displaced: SlotId(6),
            version: v(19),
            displaced_version: v(50),
            if_live: false,
            failure: f,
        },
        Inst::UpdateMask {
            result: v(20),
            mask: v(9),
            field: 1,
            initialized: false,
        },
        Inst::Construct {
            result: v(21),
            construct: ConstructId(1),
            failure: f,
        },
        Inst::Closure {
            result: v(22),
            closure: ClosureId(0),
            failure: f,
        },
        Inst::BeginLoan {
            loan: LoanId(0),
            address: a(0),
            mutable: true,
            if_inactive: false,
        },
        Inst::EndLoan {
            loan: LoanId(0),
            if_active: true,
        },
        Inst::BindOrigin {
            source: a(0),
            place: a(1),
            kind: OriginKind::Alias,
        },
        Inst::BindOrigin {
            source: a(0),
            place: a(1),
            kind: OriginKind::Fresh,
        },
        Inst::BindFields {
            source: a(0),
            fields: fields_of,
        },
        Inst::EndOrigin { place: a(1) },
        Inst::Drop {
            address: a(0),
            if_live: true,
            anchor,
            failure: f,
        },
        Inst::Dup { value: v(2) },
        Inst::Release {
            value: v(2),
            layout: LayoutId(7),
            live: MaskRef::All,
            anchor,
            failure: f,
        },
        Inst::Release {
            value: v(2),
            layout: LayoutId(7),
            live: MaskRef::Dynamic(v(9)),
            anchor,
            failure: f,
        },
        Inst::Reset {
            result: v(7),
            value: v(2),
            layout: LayoutId(7),
            live: MaskRef::Static(MaskId(0)),
            anchor,
            failure: f,
        },
        Inst::Reuse {
            result: v(23),
            token: v(7),
            construct: ConstructId(2),
            kept: None,
            failure: f,
        },
        Inst::Dispose { token: v(7) },
        Inst::TokenNone { result: v(24) },
        Inst::HoleOpen {
            result: v(3),
            hole: v(4),
            construct: ConstructId(2),
            open: LayoutId(2),
            failure: f,
        },
        Inst::HoleFill {
            hole: v(4),
            value: v(1),
        },
        Inst::HoleAbandon {
            hole: v(4),
            root: Some(v(3)),
        },
        Inst::RegionEnter {
            result: v(8),
            region: RegionInstanceId(0),
        },
        Inst::RegionReserve {
            result: v(25),
            mark: v(8),
            layout: LayoutId(2),
            extent: v(1),
            failure: f,
        },
        Inst::RegionPublish { value: v(25) },
        Inst::RegionExit {
            mark: v(8),
            region: RegionInstanceId(0),
            failure: f,
        },
        Inst::ArenaOpen {
            result: v(26),
            budget: Some(v(1)),
        },
        Inst::ArenaToken {
            result: v(27),
            mark: v(8),
            budget: None,
        },
        Inst::Defuse { address: a(0) },
        Inst::FinalizeBegin {
            address: a(0),
            anchor,
        },
        Inst::FinalizeEnd {
            address: a(0),
            anchor,
        },
        Inst::Forget { value: v(2) },
        Inst::FrameEnter { failure: f },
        Inst::FrameLeave,
    ];
    for (i, op) in [UnaryOp::Neg, UnaryOp::Not, UnaryOp::BitNot]
        .into_iter()
        .enumerate()
    {
        insts.push(Inst::Unary {
            result: v(40 + i as u32),
            op,
            operand: v(1),
            failure: f,
        });
    }
    for op in [
        BinaryOp::Add,
        BinaryOp::Sub,
        BinaryOp::Mul,
        BinaryOp::Div,
        BinaryOp::Rem,
        BinaryOp::Shl,
        BinaryOp::Shr,
        BinaryOp::BitAnd,
        BinaryOp::BitOr,
        BinaryOp::BitXor,
        BinaryOp::Eq,
        BinaryOp::Ne,
        BinaryOp::Lt,
        BinaryOp::Le,
        BinaryOp::Gt,
        BinaryOp::Ge,
        BinaryOp::DivFloor,
        BinaryOp::Mod,
        BinaryOp::DivExact,
        BinaryOp::Min,
        BinaryOp::Max,
        BinaryOp::MinNumber,
        BinaryOp::MaxNumber,
    ] {
        insts.push(Inst::Binary {
            result: v(44),
            op,
            left: v(1),
            right: v(10),
            failure: f,
        });
    }
    for op in [
        CompareOp::Eq,
        CompareOp::Ne,
        CompareOp::Lt,
        CompareOp::Le,
        CompareOp::Gt,
        CompareOp::Ge,
    ] {
        insts.push(Inst::Compare {
            result: v(45),
            op,
            left: v(2),
            right: v(13),
            layout: LayoutId(7),
            failure: f,
        });
    }
    for (mode, flag) in [
        (ConvertMode::Widen, None),
        (ConvertMode::Truncate, None),
        (ConvertMode::TryNarrow, Some(v(51))),
        (ConvertMode::FloatToI32, Some(v(51))),
        (ConvertMode::IntToFloat, None),
        (ConvertMode::Reinterpret, None),
    ] {
        insts.push(Inst::Convert {
            result: v(50),
            flag,
            operand: v(1),
            mode,
        });
    }
    for (mode, flag) in [
        (ArithMode::Wrapping, None),
        (ArithMode::Saturating, None),
        (ArithMode::Checked, Some(v(53))),
    ] {
        insts.push(Inst::IntArith {
            result: v(52),
            flag,
            op: BinaryOp::Add,
            mode,
            left: v(1),
            right: v(10),
        });
    }
    insts.push(Inst::MulAdd {
        result: v(54),
        left: v(1),
        right: v(10),
        addend: v(1),
    });
    insts.push(Inst::ScalarUnary {
        result: v(54),
        op: ScalarUnaryOp::ReverseBits,
        operand: v(1),
    });
    insts.push(Inst::IsUnique {
        result: v(46),
        value: v(2),
    });
    insts.push(Inst::Weaken {
        result: v(47),
        value: v(5),
    });
    insts.push(Inst::FlatView {
        result: v(48),
        address: a(0),
    });
    insts.push(Inst::SliceView {
        result: v(48),
        value: v(2),
        start: v(1),
        end: v(3),
        failure: f,
    });
    insts.push(Inst::FlatLength {
        result: v(49),
        address: a(0),
    });
    // LIR schema revision 7.
    insts.push(Inst::FlatNext {
        result: v(55),
        address: a(0),
    });
    insts.push(Inst::SeqLen {
        result: v(56),
        value: v(2),
    });
    insts.push(Inst::SeqCapacity {
        result: v(57),
        value: v(2),
    });
    insts.push(Inst::SeqReserve {
        address: a(0),
        capacity: v(1),
        version: v(58),
    });
    insts.push(Inst::SeqSetLen {
        address: a(0),
        len: v(1),
        version: v(59),
    });
    insts.push(Inst::SeqAppend {
        address: a(0),
        value: v(1),
        version: v(59),
    });
    // LIR schema revision 22.
    insts.push(Inst::FlatAppend {
        address: a(0),
        construct: ConstructId(0),
        version: v(59),
    });
    // LIR schema revision 24.
    for op in [
        FlatFieldOp::Open,
        FlatFieldOp::Reserve { capacity: v(1) },
        FlatFieldOp::Push {
            value: v(1),
            spare: true,
        },
    ] {
        insts.push(Inst::FlatField {
            address: a(0),
            field: 2,
            op,
            version: v(59),
        });
    }
    insts.push(Inst::FlatClose {
        address: a(0),
        construct: ConstructId(0),
        field: 2,
        version: v(59),
    });
    insts.push(Inst::Hash {
        result: v(60),
        value: v(2),
        layout: LayoutId(0),
        failure: FailureId(0),
    });
    insts.push(Inst::Recast {
        result: v(61),
        value: v(2),
    });
    insts.push(Inst::Freeze {
        result: v(62),
        value: v(2),
    });
    insts.push(Inst::Thaw {
        result: v(63),
        value: v(62),
    });
    insts.push(Inst::Encode {
        result: v(64),
        value: v(2),
        codec: CodecId(0),
    });
    insts.push(Inst::Decode {
        result: v(65),
        value: v(64),
        codec: CodecId(0),
    });
    // LIR schema revision 32.
    insts.push(Inst::IsWritable {
        result: v(46),
        value: v(2),
    });
    insts.push(Inst::Share { value: v(2) });
    insts.push(Inst::ArenaPush {
        address: a(0),
        count: None,
        version: v(59),
        failure: f,
    });
    // LIR schema revision 34.
    insts.push(Inst::ArenaPush {
        address: a(0),
        count: Some(v(1)),
        version: v(59),
        failure: f,
    });
    for feature in [
        UnavailableFeature::Shared,
        UnavailableFeature::Arena,
        UnavailableFeature::Task,
        UnavailableFeature::Channel,
        UnavailableFeature::Mutex,
        UnavailableFeature::Atomic,
        UnavailableFeature::ReleaseRegion,
        UnavailableFeature::CloseArena,
        UnavailableFeature::JoinTasks,
        UnavailableFeature::Shape,
    ] {
        insts.push(Inst::Unavailable {
            feature,
            anchor: SourceAnchorId(0),
        });
    }
    let body_len = insts.len() as u32;
    u.instructions = insts;
    let kinds = [
        BlockKind::Body,
        BlockKind::Body,
        BlockKind::Body,
        BlockKind::Body,
        BlockKind::Body,
        BlockKind::Body,
        BlockKind::Cleanup,
        BlockKind::FailureCleanup,
    ];
    u.blocks = kinds
        .iter()
        .enumerate()
        .map(|(i, &kind)| Block {
            kind,
            parameters: if i == 1 { block_args } else { PoolRange::EMPTY },
            instructions: if i == 0 {
                PoolRange::new(0, body_len)
            } else {
                PoolRange::EMPTY
            },
            anchor: SourceAnchorId(0),
        })
        .collect();
    u.terminators = vec![
        Terminator::Jump(EdgeId(0)),
        Terminator::Branch {
            condition: v(10),
            yes: EdgeId(1),
            no: EdgeId(2),
        },
        Terminator::Switch {
            value: v(1),
            arms,
            otherwise: EdgeId(4),
        },
        Terminator::Call {
            call: CallId(0),
            normal: Some(EdgeId(4)),
            failure: FailureId(1),
        },
        Terminator::Return(Some(v(30))),
        Terminator::Return(None),
        Terminator::Raise(FailureId(0)),
        Terminator::Propagate,
    ];
    u.blocks.push(Block {
        kind: BlockKind::FailureCleanup,
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::EMPTY,
        anchor: SourceAnchorId(0),
    });
    u.terminators.push(Terminator::Unreachable);
    u.blocks.push(Block {
        kind: BlockKind::Body,
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::EMPTY,
        anchor: SourceAnchorId(0),
    });
    u.terminators.push(Terminator::InvariantFault {
        reason: "container extent exceeds u32".into(),
    });
    u.blocks.push(Block {
        kind: BlockKind::FailureCleanup,
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::EMPTY,
        anchor: SourceAnchorId(0),
    });
    u.terminators.push(Terminator::Recover {
        arena: v(26),
        caught: EdgeId(0),
        failed: EdgeId(1),
    });
    u.blocks.push(Block {
        kind: BlockKind::FailureCleanup,
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::EMPTY,
        anchor: SourceAnchorId(0),
    });
    u.terminators.push(Terminator::Isolate {
        start: v(1),
        caught: EdgeId(0),
    });
    u
}

fn assert_round_trip<T: Snapshot + PartialEq + std::fmt::Debug>(value: &T) -> String {
    let text = snapshot::to_text(value);
    let back: T = snapshot::from_text(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
    assert_eq!(&back, value);
    assert_eq!(
        snapshot::to_text(&back),
        text,
        "re-encoding is not deterministic"
    );
    assert!(!text.contains("TypeId"), "a dump contains a TypeId");
    assert!(!text.contains("0x"), "a dump contains an address");
    text
}

fn unit_round_trip<S: Stage>(table: &LayoutTable) {
    let unit = fixture_unit::<S>(table);
    unit.check_identity(table)
        .expect("fixture is referentially valid");
    let text = assert_round_trip(&unit);
    assert_eq!(unit.digest(), fixture_unit::<S>(table).digest());
    assert!(text.starts_with("(LirUnit\n  (UnitHeader"));
}

#[test]
fn raw_artifacts_round_trip_deterministically_on_both_targets() {
    let mut digests = Vec::new();
    for target in TargetSpec::executed() {
        assert_round_trip(&target);
        let table = fixture_table(&target);
        assert_round_trip(&table);
        unit_round_trip::<Value>(&table);
        unit_round_trip::<Placed>(&table);
        unit_round_trip::<Rc>(&table);
        unit_round_trip::<Reuse>(&table);
        unit_round_trip::<Ready>(&table);
        digests.push((target.digest(), table.digest()));
    }
    assert_ne!(digests[0].0, digests[1].0, "targets share a digest");
    assert_ne!(digests[0].1, digests[1].1, "layout tables share a digest");
    for target in TargetSpec::executed() {
        assert_eq!(target.endian, Endian::Little);
        assert_eq!(target.pointer_bytes, 8);
        assert_eq!(target.scalar_size(ScalarKind::I128), 16);
        assert_eq!(target.scalar_align(ScalarKind::I128), 16);
        assert_eq!(target.object_format, ObjectFormat::Elf);
        assert_eq!(target.versions, ArtifactVersions::current());
    }
    let [x86, arm] = TargetSpec::executed();
    assert_eq!(x86.c_convention, CConvention::SysV64);
    assert_eq!(arm.c_convention, CConvention::Aapcs64);
    assert_eq!(arm.triple, "aarch64-unknown-linux-gnu");
}

#[test]
fn receipts_artifacts_and_manifests_round_trip_and_bind_the_unit_key() {
    let raw = RawReceipt {
        key: hirsel_lir::objects::UnitKey(hash("unit key")),
        stage: StageKind::Ready,
        lir_version: hirsel_lir::LIR_VERSION,
        unit: hash("unit"),
        validator_version: hirsel_lir::VALIDATOR_VERSION,
    };
    assert_round_trip(&raw);
    for stage in StageKind::ALL {
        assert_round_trip(&stage);
    }
    let mut stale = raw.clone();
    stale.key = hirsel_lir::objects::UnitKey(hash("other key"));
    assert_ne!(raw.digest(), stale.digest());

    let artifact = ObjectArtifact {
        ahead: None,
        target: hash("target"),
        format: ObjectFormat::Elf,
        bytes: vec![0x7f, b'E', b'L', b'F'],
        exports: vec![ObjectSymbol {
            name: "hirsel_f".into(),
            kind: ObjectSymbolKind::Function,
            definition: Some(path("f")),
            abi: Some(hash("abi")),
        }],
        imports: vec![
            ObjectSymbol {
                name: RuntimeEntry::Alloc.declaration().symbol.into(),
                kind: ObjectSymbolKind::Runtime,
                definition: None,
                abi: None,
            },
            ObjectSymbol {
                name: "hirsel_table".into(),
                kind: ObjectSymbolKind::Data,
                definition: None,
                abi: None,
            },
        ],
        relocations: vec![Relocation {
            offset: 16,
            symbol: 0,
            kind: 4,
            addend: -4,
        }],
        abi: hash("abi"),
        reproducibility: Reproducibility {
            toolchain: "llvm 23.1.4".into(),
            options: hash("O2"),
            lir_version: hirsel_lir::LIR_VERSION,
            runtime_abi_version: hirsel_lir::RUNTIME_ABI_VERSION,
        },
        debug: vec![hirsel_lir::debug::DebugFunction::from_source_rows(
            "hirsel_f".into(),
            32,
            vec![
                (path("f"), 0),
                (path("f"), 1),
                (DefPath::core("$intrinsic.DebugFixture"), 0),
            ],
            4,
            [(0, 8, 0, 4), (8, 12, 0, 4), (16, 20, 1, 9), (20, 20, 0, 1)],
        )],
    };
    assert!(artifact.debug[0].well_formed());
    assert_eq!(
        artifact.debug[0].owner_kinds,
        vec![
            hirsel_lir::debug::DebugOwnerKind::Source,
            hirsel_lir::debug::DebugOwnerKind::Source,
            hirsel_lir::debug::DebugOwnerKind::SourceLessCoreIntrinsic,
        ]
    );
    let mut forged = artifact.debug[0].clone();
    forged.owner_kinds[0] = hirsel_lir::debug::DebugOwnerKind::SourceLessCoreIntrinsic;
    assert!(
        !forged.well_formed(),
        "a real source cannot be marked source-less"
    );
    let mut truncated = artifact.debug[0].clone();
    truncated.owner_kinds.pop();
    assert!(!truncated.well_formed());
    assert_eq!(
        artifact.debug[0].ranges.len(),
        2,
        "rows of one site merge, a gap stays and an empty row is dropped"
    );
    assert_round_trip(&artifact);
    let mut other = artifact.clone();
    other.reproducibility.lir_version += 1;
    assert_ne!(artifact.digest(), other.digest());

    let id = |name: &str| OperationId {
        definition: path(name),
        declaration_digest: hash(name),
    };
    let mut manifest = Manifest::skeleton([id("a"), id("b")]);
    manifest
        .replace(OperationRow {
            id: id("b"),
            ownership: Some(CallOwnership {
                parameters: vec![
                    ParameterOwnership {
                        ordinal: 0,
                        passing: Passing::Take,
                        cleanup: CleanupEffect::DependsOn(vec![0]),
                    },
                    ParameterOwnership {
                        ordinal: 1,
                        passing: Passing::Mut,
                        cleanup: CleanupEffect::NoAlloc,
                    },
                    ParameterOwnership {
                        ordinal: 2,
                        passing: Passing::Borrow,
                        cleanup: CleanupEffect::MayAllocate,
                    },
                ],
                result_cleanup: CleanupEffect::NoAlloc,
            }),
            effects: SemanticEffects {
                allocates: false,
                ..SemanticEffects::CONSERVATIVE
            },
            dispositions: EngineDispositions {
                reference: Disposition::Body,
                native: Disposition::RuntimePrimitive("hirsel_rt_v2_io_call".into()),
                heap_check: Disposition::Instruction("i64.add.checked".into()),
            },
            vectors: vec![TestVectorId("b/basic".into())],
            failure: None,
        })
        .unwrap();
    let text = assert_round_trip(&manifest);
    assert!(text.contains("Unclassified"));
    for disposition in [
        Disposition::DeclarationOnly,
        Disposition::Unavailable(Unavailable::M2(M2Feature::Shared)),
        Disposition::Unavailable(Unavailable::OpenSpec(46)),
        Disposition::Unavailable(Unavailable::NotImplemented),
    ] {
        assert_round_trip(&disposition);
    }
    for feature in [
        M2Feature::Tasks,
        M2Feature::Channels,
        M2Feature::Mutexes,
        M2Feature::Atomics,
        M2Feature::Shared,
        M2Feature::Arenas,
        M2Feature::Regions,
        M2Feature::Slots,
    ] {
        assert_round_trip(&Disposition::Unavailable(Unavailable::M2(feature)));
    }
    // A snapshot with a duplicated row cannot decode into a manifest.
    let duplicated = text.replacen(
        "(Manifest",
        &format!(
            "(Manifest {}",
            snapshot::to_text(manifest.row(&id("a")).unwrap())
        ),
        1,
    );
    assert!(snapshot::from_text::<Manifest>(&duplicated).is_err());
}

#[test]
fn a_snapshot_decodes_only_as_its_own_stage() {
    let table = fixture_table(&TargetSpec::x86_64_unknown_linux_gnu());
    let text = snapshot::to_text(&fixture_unit::<Value>(&table));
    assert!(snapshot::from_text::<LirUnit<Value>>(&text).is_ok());
    let error = snapshot::from_text::<LirUnit<Rc>>(&text).unwrap_err();
    assert!(error.message.contains("stage mismatch"), "{error:?}");

    // A forged header naming another stage fails the identity check.
    let mut forged = fixture_unit::<Ready>(&table);
    forged.header.stage = StageKind::Rc;
    assert_eq!(
        forged.check_identity(&table),
        Err(SchemaError::StageMismatch {
            expected: StageKind::Ready,
            found: StageKind::Rc
        })
    );
    // Retagging produces the new stage's raw data, still unvalidated.
    let retagged: LirUnit<Ready> = fixture_unit::<Rc>(&table).into_stage();
    assert_eq!(retagged.header.stage, StageKind::Ready);
    assert_eq!(retagged.check_identity(&table), Ok(()));
}

#[test]
fn descriptor_constants_rebind_and_participate_in_import_evidence() {
    let table = fixture_table(&TargetSpec::x86_64_unknown_linux_gnu());
    let mut unit = fixture_unit::<Value>(&table);
    // Use a layout absent from all other operands so matching read and write
    // walks alone cannot hide the omission of this constant's dependency.
    let descriptor = LayoutId(table.layouts.len() as u32);
    unit.tables.constants.push(ConstantData {
        ty: LirTypeId(0),
        value: ConstantValue::Descriptor(descriptor),
    });
    assert!(
        hirsel_lir::objects::references(&unit)
            .0
            .contains(&descriptor)
    );
    let (erased, mut layouts, abis) = hirsel_lir::objects::erased(&unit);
    let index = layouts
        .iter()
        .position(|id| *id == descriptor)
        .expect("descriptor constants must be named by layout content");
    assert_eq!(
        erased.tables.constants.last().unwrap().value,
        ConstantValue::Descriptor(LayoutId(index as u32))
    );
    let replacement = LayoutId(descriptor.0 + 7);
    layouts[index] = replacement;
    unit.tables.constants.last_mut().unwrap().value = ConstantValue::Descriptor(replacement);
    assert_eq!(
        hirsel_lir::objects::rebound(erased.clone(), &layouts, &abis).unwrap(),
        unit
    );
    layouts.truncate(index);
    assert!(hirsel_lir::objects::rebound(erased, &layouts, &abis).is_err());
}

#[test]
fn foreign_and_stale_identities_are_rejected() {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let table = fixture_table(&target);
    let unit = fixture_unit::<Rc>(&table);
    let (_, mut layouts, mut abis) = hirsel_lir::objects::erased(&unit);
    layouts.sort();
    abis.sort();
    assert_eq!(hirsel_lir::objects::references(&unit), (layouts, abis));

    assert_eq!(unit.check_identity(&table), Ok(()));

    let mut foreign_source = unit.clone();
    foreign_source.tables.source_anchors[0].site.source = unit.header.source_count;
    assert!(matches!(
        foreign_source.check_identity(&table),
        Err(SchemaError::ForeignId {
            kind: "source selector",
            ..
        })
    ));
    let mut reversed_source = unit.clone();
    reversed_source.tables.cleanup_anchors[0].site = SiteRange {
        source: 0,
        start: 9,
        end: 8,
    };
    assert_eq!(
        reversed_source.check_identity(&table),
        Err(SchemaError::Malformed("reversed source range"))
    );
    let mut gap = unit.clone();
    gap.tables.source_anchors[0].site = SiteRange::UNMAPPED;
    assert_eq!(gap.check_identity(&table), Ok(()));
    let revision = hash("revision");
    let declaration = SiteRange {
        source: 0,
        start: 10,
        end: 24,
    };
    let generated = SiteRange {
        source: 1,
        ..declaration
    };
    assert_ne!(
        hirsel_lir::emission::site_code(&path("f"), &revision, declaration),
        hirsel_lir::emission::site_code(&path("f"), &revision, generated),
        "equal offsets from different sources cannot share failure identity"
    );

    let mut foreign_value = unit.clone();
    foreign_value.instructions[0] = Inst::Const {
        result: ValueId(foreign_value.values.len() as u32),
        constant: ConstantId(0),
    };
    assert!(matches!(
        foreign_value.check_identity(&table),
        Err(SchemaError::ForeignId { kind: "value", .. })
    ));

    let mut foreign_layout = unit.clone();
    foreign_layout.types[0].layout = LayoutId(table.layouts.len() as u32);
    assert!(matches!(
        foreign_layout.check_identity(&table),
        Err(SchemaError::ForeignId { kind: "layout", .. })
    ));

    let mut foreign_edge = unit.clone();
    foreign_edge.terminators[0] = Terminator::Jump(EdgeId(99));
    assert!(matches!(
        foreign_edge.check_identity(&table),
        Err(SchemaError::ForeignId { kind: "edge", .. })
    ));

    let mut foreign_range = unit.clone();
    foreign_range.blocks[1].parameters = PoolRange::new(0, 1000);
    assert!(matches!(
        foreign_range.check_identity(&table),
        Err(SchemaError::ForeignRange { kind: "values", .. })
    ));

    let mut foreign_pool = unit.clone();
    foreign_pool.pools.values.push(ValueId(10_000));
    assert!(matches!(
        foreign_pool.check_identity(&table),
        Err(SchemaError::ForeignId { kind: "value", .. })
    ));

    let mut stale_version = unit.clone();
    stale_version.header.lir_version += 1;
    assert_eq!(
        stale_version.check_identity(&table),
        Err(SchemaError::Stale("LIR version"))
    );

    // The same unit checked against the other target's table is stale.
    let other = fixture_table(&TargetSpec::aarch64_unknown_linux_gnu());
    assert_eq!(
        unit.check_identity(&other),
        Err(SchemaError::Stale("layout table target"))
    );
    let mut edited = table.clone();
    edited.layouts[1].align = 16;
    // Same-target row content is checked by validation and receipt evidence.
    assert_eq!(unit.check_identity(&edited), Ok(()));

    let mut torn = unit.clone();
    torn.terminators.pop();
    assert!(matches!(
        torn.check_identity(&table),
        Err(SchemaError::Malformed(_))
    ));
}

#[test]
fn instruction_rows_are_at_most_32_bytes() {
    let row = std::mem::size_of::<Inst>();
    let table = fixture_table(&TargetSpec::x86_64_unknown_linux_gnu());
    let unit = fixture_unit::<Ready>(&table);
    println!(
        "LIR instruction row: {row} bytes/instruction; fixture: {} instructions, {} retained arena bytes",
        unit.instructions.len(),
        unit.retained_bytes()
    );
    assert!(row <= ir::MAX_INSTRUCTION_BYTES, "{row} bytes");
    assert_eq!(ir::MAX_INSTRUCTION_BYTES, 32);
}
use hirsel_lir::ir;

#[test]
fn every_opcode_is_covered_and_admitted_by_a_stage() {
    let table = fixture_table(&TargetSpec::x86_64_unknown_linux_gnu());
    let unit = fixture_unit::<Value>(&table);
    let mut seen: Vec<Opcode> = unit
        .instructions
        .iter()
        .map(Inst::opcode)
        .chain(unit.terminators.iter().map(Terminator::opcode))
        .collect();
    // The typed sealed vocabulary is not executable Ready input yet. Its
    // standalone rows still have complete snapshot and opcode coverage.
    for instruction in [
        Inst::SealArena {
            result: ValueId(0),
            grant: ValueId(1),
            root: ValueId(2),
            shape: SealDescriptorId(0),
            failure: FailureId(0),
            origin: SourceAnchorId(0),
        },
        Inst::BorrowSealedRoot {
            result: ValueId(0),
            owner: ValueId(1),
            loan: LoanId(0),
            shape: SealDescriptorId(0),
        },
        Inst::ArenaInventory {
            arena: ValueId(0),
            allocation: SourceAnchorId(0),
            value: ValueId(1),
            shape: SealDescriptorId(0),
            previous: None,
        },
        Inst::PayloadReceipt {
            result: ValueId(0),
            value: ValueId(1),
            shape: SealDescriptorId(0),
            binding: PayloadBindingId(0),
            inputs: PoolRange::EMPTY,
        },
    ] {
        assert_round_trip(&instruction);
        let opcode = instruction.opcode();
        assert!(!StageKind::Ready.admits(opcode));
        seen.push(opcode);
    }
    assert_round_trip(&Terminator::RejectTransfer);
    seen.push(Terminator::RejectTransfer.opcode());
    seen.sort();
    seen.dedup();
    let mut all = Opcode::ALL.to_vec();
    all.sort();
    assert_eq!(seen, all, "the fixture covers every opcode");
    for &op in Opcode::ALL {
        assert!(
            StageKind::ALL.iter().any(|stage| stage.admits(op)),
            "{op:?} is admitted by no stage"
        );
    }
    for flags in [
        Effects::NONE,
        Effects::ALL,
        Effects::OWNS | Effects::BARRIER,
    ] {
        assert_round_trip(&flags);
    }
}

#[test]
fn stage_vocabularies_are_successively_restricted() {
    use StageKind::*;
    assert!(Value.admits(Opcode::Copy) && Value.admits(Opcode::Drop));
    assert!(!Value.admits(Opcode::RegionEnter) && Placed.admits(Opcode::RegionEnter));
    assert!(!Placed.admits(Opcode::Dup) && Rc.admits(Opcode::Dup) && Rc.admits(Opcode::Release));
    assert!(!Rc.admits(Opcode::Reset) && Reuse.admits(Opcode::Reset));
    assert!(Ready.admits(Opcode::Reuse) && Ready.admits(Opcode::Dispose));
    for op in [Opcode::Drop, Opcode::Unavailable] {
        assert!(!Ready.admits(op), "{op:?}");
    }
    // A recipe-resolved Copy is a Ready primitive; the Ready validator
    // rejects the recipes that lower away.
    assert!(Ready.admits(Opcode::Copy));
}

mod instance_keys {
    use super::*;
    use hirsel_types::{
        Brand, GenericArgument, Modes, Origin, OriginRoot, ParamId, ParameterRef, Primitive, Reach,
        Relation, Sendability, Size, SizeParamId, SizeParameterRef, Type, TypeInterner, TypeKind,
        TypeParamId, TypeParameterRef, Uniqueness,
    };

    fn element(store: &TypeInterner, modes: Modes, brand: Brand) -> hirsel_types::TypeId {
        let int = store.intern(Type {
            kind: TypeKind::Primitive(Primitive::I64),
            modes: modes.clone(),
            ..Type::unit()
        });
        store.intern(Type {
            kind: TypeKind::Builtin {
                kind: hirsel_types::BuiltinType::List,
                arguments: vec![GenericArgument::Type(int)].into(),
            },
            modes,
            brand,
            ..Type::unit()
        })
    }

    fn key(
        store: &TypeInterner,
        actual: hirsel_types::TypeId,
        size: u64,
        binder: TypeParameterRef,
    ) -> InstanceKey {
        InstanceKey::new(
            &path("map"),
            0,
            &[
                (ActualBinder::Type(binder), GenericArgument::Type(actual)),
                (
                    ActualBinder::Size(SizeParameterRef::bound(SizeParamId(1))),
                    GenericArgument::Size(Size::Constant(size)),
                ),
            ],
            store,
            Some(&CallableConvention {
                parameters: vec![Passing::Borrow, Passing::Take],
                multiplicity: CallMultiplicity::Many,
            }),
            None,
        )
        .expect("complete actuals")
    }

    #[test]
    fn keys_erase_relation_brand_uniqueness_and_sendability_but_not_sizes_or_binders() {
        let store = crate::support::session();
        let binder = TypeParameterRef::bound(TypeParamId(0));
        let plain = element(&store, Modes::global(), Brand::default());
        let place = Origin {
            root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
            fields: vec![].into(),
        };
        let related = element(
            &store,
            Modes {
                relation: Relation::new([Reach::Place(place.clone())]),
                ..Modes::global()
            },
            Brand::default(),
        );
        let branded = element(
            &store,
            Modes::global(),
            Brand {
                alternatives: vec![place].into(),
            },
        );
        let unique = element(
            &store,
            Modes {
                uniqueness: Uniqueness::Unique,
                ..Modes::global()
            },
            Brand::default(),
        );
        let sendable = element(
            &store,
            Modes {
                sendability: Sendability::Sendable,
                ..Modes::global()
            },
            Brand::default(),
        );
        let mut requirement = store.get(plain).unwrap().clone();
        requirement.sendability_requirement = hirsel_types::SendabilityRequirement::Sendable;
        let requirement = store.intern(requirement);
        let base = key(&store, plain, 4, binder);
        for (label, actual) in [
            ("crossing requirement", requirement),
            ("relation", related),
            ("brand", branded),
            ("uniqueness", unique),
            ("sendability", sendable),
        ] {
            let other = key(&store, actual, 4, binder);
            assert_eq!(base, other, "an actual's {label} changed the key");
            assert_eq!(base.digest(), other.digest());
        }
        // Keys from an independent interner with fresh TypeIds are equal too.
        let fresh = TypeInterner::new(None);
        let again = element(&fresh, Modes::global(), Brand::default());
        assert_eq!(base, key(&fresh, again, 4, binder));

        assert_ne!(base, key(&store, plain, 5, binder), "size actual");
        assert_ne!(
            base,
            key(&store, plain, 4, TypeParameterRef::bound(TypeParamId(1))),
            "binder ordinal"
        );
        assert_ne!(
            base,
            key(
                &store,
                plain,
                4,
                TypeParameterRef::Bound {
                    depth: 1,
                    parameter: TypeParamId(0)
                }
            ),
            "binder depth"
        );
        assert_ne!(
            base,
            key(
                &store,
                plain,
                4,
                TypeParameterRef::body(
                    hirsel_syntax::LexicalOwner::first(&path("f"))
                        .site(hirsel_types::NameSite::Item),
                    TypeParamId(0)
                )
            ),
            "binder scope"
        );
        let int = store.intern(Type {
            kind: TypeKind::Primitive(Primitive::I64),
            ..Type::unit()
        });
        assert_ne!(base, key(&store, int, 4, binder), "type structure");
    }

    #[test]
    fn keys_reject_incomplete_mismatched_and_foreign_actuals() {
        let store = crate::support::session();
        let open = store.intern(Type {
            kind: TypeKind::Parameter(TypeParameterRef::body(
                hirsel_syntax::LexicalOwner::first(&path("f")).site(hirsel_types::NameSite::Item),
                TypeParamId(0),
            )),
            ..Type::unit()
        });
        let binder = ActualBinder::Type(TypeParameterRef::bound(TypeParamId(0)));
        let make = |store: &TypeInterner, binder, actual| {
            InstanceKey::new(&path("f"), 0, &[(binder, actual)], store, None, None)
        };
        assert_eq!(
            make(&store, binder, GenericArgument::Type(open)).unwrap_err(),
            InstanceKeyError::Incomplete
        );
        assert_eq!(
            make(&store, binder, GenericArgument::Size(Size::Constant(1))).unwrap_err(),
            InstanceKeyError::KindMismatch(0)
        );
        // A TypeId with no row in the session interner.
        assert_eq!(
            make(
                &store,
                binder,
                GenericArgument::Type(hirsel_types::TypeId::DANGLING)
            )
            .unwrap_err(),
            InstanceKeyError::MissingType
        );
    }

    #[test]
    fn instance_views_reject_foreign_type_ids_and_non_bijective_remaps() {
        let store = crate::support::session();
        let unit_ty = store.intern(Type::unit());
        let definition = std::sync::Arc::new(hirsel_tir::Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: vec![unit_ty].into(),
            path: hirsel_syntax::DefId::intern(&path("f")),
            signature_hash: hash("f signature"),
            bodies: vec![super::tir_coverage_body()],
            entry: hirsel_tir::BodyId(0),
            contracts: Vec::new(),
        });
        let key = InstanceKey::new(&path("f"), 0, &[], &store, None, None).unwrap();
        let view = InstanceView::from_definition(
            key,
            definition.clone(),
            hirsel_tir::BodyId(0),
            CallableShape::Definition,
            Substitution {
                types: vec![(TypeParameterRef::bound(TypeParamId(0)), unit_ty)],
                sizes: vec![(SizeParameterRef::bound(SizeParamId(0)), 8)],
            },
            vec![
                ParameterView {
                    source: 0,
                    instance: 1,
                    passing: Passing::Borrow,
                },
                ParameterView {
                    source: 1,
                    instance: 0,
                    passing: Passing::Take,
                },
            ],
            vec![CaptureView {
                scoped: false,
                ordinal: 0,
                usage: CaptureUse::Borrow,
            }],
            false,
        );
        assert_eq!(view.check_identity(&store), Ok(()));
        let mut detached = InstanceView::from_declaration(
            view.key.clone(),
            view.declaration().clone(),
            view.body,
            view.owner,
            view.substitution.clone(),
            view.parameters.clone(),
            view.captures.clone(),
            view.caller_destination,
        );
        assert!(detached.definition().is_err());
        assert!(matches!(
            detached.check_identity(&store),
            Err(SchemaError::Stale(_))
        ));
        let original = view.definition().unwrap().clone();
        let mut wrong = original.as_ref().clone();
        wrong.signature_hash = ContentHash::default();
        assert!(
            detached
                .attach_definition(std::sync::Arc::new(wrong))
                .is_err()
        );
        assert!(detached.definition().is_err());
        detached.attach_definition(original).unwrap();
        assert_eq!(detached.check_identity(&store), Ok(()));
        detached.release_definition();
        assert!(detached.definition().is_err());

        let mut foreign = view.clone();
        foreign.substitution.types[0].1 = hirsel_types::TypeId::DANGLING;
        assert!(matches!(
            foreign.check_identity(&store),
            Err(SchemaError::ForeignId { kind: "TypeId", .. })
        ));
        let mut not_bijective = view.clone();
        not_bijective.parameters[1].instance = 1;
        assert!(matches!(
            not_bijective.check_identity(&store),
            Err(SchemaError::Malformed(_))
        ));
        let mut wrong_body = view.clone();
        wrong_body.body = hirsel_tir::BodyId(4);
        assert!(matches!(
            wrong_body.check_identity(&store),
            Err(SchemaError::ForeignId { .. })
        ));
    }
}

fn tir_coverage_body() -> hirsel_tir::Body {
    hirsel_tir::Body {
        arena_scope_grant: None,
        scoped_parameters: Vec::new(),
        parameter_scope: hirsel_syntax::LexicalOwner::first(&path("f"))
            .site(hirsel_types::NameSite::Item),
        locals: Vec::new(),
        places: Vec::new(),
        scopes: Vec::new(),
        blocks: Vec::new(),
        rvalues: Vec::new(),
        cleanups: Vec::new(),
        cleanup_nodes: Vec::new(),
        entry: hirsel_tir::BlockId(0),
        parameters: Vec::new(),
        result: crate::support::session().intern(hirsel_types::Type::unit()),
        borrow_cursors: vec![],
        tail_sites: Vec::new(),
        matches: Vec::new(),
    }
}

#[test]
fn runtime_declarations_match_the_landed_c_header() {
    let header = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../hirsel-rt/include/hirsel_rt_v2.h"
    ))
    .expect("hirsel-rt publishes include/hirsel_rt_v2.h");
    let normalized = header
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(|line| line.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(");
    let declared: std::collections::BTreeSet<_> = normalized
        .split(';')
        .filter(|item| !item.contains("typedef"))
        .filter_map(|item| {
            let open = item
                .find("(hirsel_rt_v2_ctx")
                .or_else(|| item.find("(hirsel_rt_v2_ahead_physical"))
                .or_else(|| item.find("(void)"))
                .or_else(|| item.find("(void *"))
                .or_else(|| item.find("(const hirsel_rt_v2_config"))
                .or_else(|| item.find("(hirsel_rt_v2_header"))
                .or_else(|| item.find("(uint64_t "))
                .or_else(|| item.find("(const void *previous"))?;
            let name = item[..open].rsplit([' ', '*']).next()?;
            name.starts_with("hirsel_rt_v2_").then(|| name.to_string())
        })
        .collect();
    let mut symbols = std::collections::BTreeSet::new();
    for entry in RuntimeEntry::ALL {
        let declaration = entry.declaration();
        assert_eq!(declaration.entry, entry);
        let prototype = declaration.c_prototype();
        assert!(
            normalized.contains(&prototype),
            "{prototype} is not declared in hirsel_rt_v2.h"
        );
        assert!(
            symbols.insert(declaration.symbol.to_string()),
            "duplicate symbol"
        );
        assert_round_trip(&entry);
    }
    assert_eq!(
        symbols, declared,
        "hirsel_rt_v2.h declares entries this schema lacks"
    );
    let define = |name: &str, value: &str| {
        assert!(
            header.contains(&format!("#define HIRSEL_RT_V2_{name} {value}\n")),
            "HIRSEL_RT_V2_{name} is not {value}"
        )
    };
    define("ABI_VERSION", &runtime_abi::ABI_VERSION.to_string());
    define(
        "FOREIGN_SCOPE_BYTES",
        &runtime_abi::FOREIGN_SCOPE_BYTES.to_string(),
    );
    define(
        "FOREIGN_SCOPE_ALIGN",
        &runtime_abi::FOREIGN_SCOPE_ALIGN.to_string(),
    );
    for (name, value) in [
        ("FOREIGN_THREADS_HOME", runtime_abi::FOREIGN_THREADS_HOME),
        (
            "FOREIGN_THREADS_SERIAL",
            runtime_abi::FOREIGN_THREADS_SERIAL,
        ),
        ("FOREIGN_THREADS_ANY", runtime_abi::FOREIGN_THREADS_ANY),
        ("FOREIGN_THREADS_MASK", runtime_abi::FOREIGN_THREADS_MASK),
        ("FOREIGN_BLOCKING", runtime_abi::FOREIGN_BLOCKING),
        (
            "FOREIGN_PACKAGE_SHIFT",
            u64::from(runtime_abi::FOREIGN_PACKAGE_SHIFT),
        ),
    ] {
        define(name, &value.to_string());
    }
    define("STATUS_OK", &runtime_abi::STATUS_OK.to_string());
    define("STATUS_FAILURE", &runtime_abi::STATUS_FAILURE.to_string());
    define("LAUNCH_OK", &runtime_abi::LAUNCH_OK.to_string());
    define(
        "LAUNCH_ABI_MISMATCH",
        &runtime_abi::LAUNCH_ABI_MISMATCH.to_string(),
    );
    define(
        "LAUNCH_INVALID_CONFIG",
        &runtime_abi::LAUNCH_INVALID_CONFIG.to_string(),
    );
    define(
        "CONFIG_DIAGNOSTIC",
        &runtime_abi::CONFIG_DIAGNOSTIC.to_string(),
    );
    define(
        "SPAWN_IMMEDIATE_AWAIT",
        &runtime_abi::SPAWN_IMMEDIATE_AWAIT.to_string(),
    );
    define("SPAWN_OP_FREE", &runtime_abi::SPAWN_OP_FREE.to_string());
    define("HEADER_SIZE", &runtime_abi::HEADER_SIZE.to_string());
    define("COUNT_IMMORTAL", "UINT32_MAX");
    define(
        "COUNT_ARENA_UNIQUE",
        &format!("{}u", runtime_abi::COUNT_ARENA_UNIQUE),
    );
    define(
        "COUNT_ARENA_SHARED",
        &format!("{}u", runtime_abi::COUNT_ARENA_SHARED),
    );
    define(
        "TRACKED_REGION_BACK",
        &runtime_abi::TRACKED_REGION_BACK.to_string(),
    );
    define(
        "TRACKED_PREFIX_BYTES",
        &runtime_abi::TRACKED_PREFIX_BYTES.to_string(),
    );
    define("ALLOC_TRACKED", &runtime_abi::ALLOC_TRACKED.to_string());
    // The count word contract of compiled drop glue: one offset, width and pair of values.
    define("COUNT_OFFSET", &runtime_abi::COUNT_OFFSET.to_string());
    define("COUNT_UNIQUE", &runtime_abi::COUNT_UNIQUE.to_string());
    define("LAYOUT_OFFSET", &runtime_abi::LAYOUT_OFFSET.to_string());
    for (field, kind, offset) in [
        (
            runtime_abi::HeaderField::Count,
            ScalarKind::U32,
            runtime_abi::COUNT_OFFSET,
        ),
        (
            runtime_abi::HeaderField::Descriptor,
            ScalarKind::U32,
            runtime_abi::LAYOUT_OFFSET,
        ),
    ] {
        assert!(
            runtime_abi::HEADER_V2.contains(&(field, kind, offset)),
            "{field:?}"
        );
    }
    assert!(header.contains("offsetof(hirsel_rt_v2_header, count) == HIRSEL_RT_V2_COUNT_OFFSET"));
    assert!(header.contains("sizeof(((hirsel_rt_v2_header *)0)->count) == 4"));
    assert!(
        header.contains("offsetof(hirsel_rt_v2_header, descriptor) == HIRSEL_RT_V2_LAYOUT_OFFSET")
    );
    define("CLASS_COUNT", &runtime_abi::SIZE_CLASSES.len().to_string());
    define("SMALL_MAX", &runtime_abi::SMALL_MAX.to_string());
    define("SMALL_ALIGN_MAX", &runtime_abi::SMALL_ALIGN_MAX.to_string());
    define("SLAB_SIZE", &runtime_abi::SLAB_SIZE.to_string());
    define(
        "SLAB_POOL_OFFSET",
        &runtime_abi::SLAB_POOL_OFFSET.to_string(),
    );
    define(
        "LARGE_PREFIX_POINTER_BACK",
        &runtime_abi::LARGE_PREFIX_POINTER_BACK.to_string(),
    );
    define(
        "LARGE_CAPACITY_OFFSET",
        &runtime_abi::LARGE_CAPACITY_OFFSET.to_string(),
    );
    define(
        "IMMORTAL_CAPACITY_BACK",
        &runtime_abi::IMMORTAL_CAPACITY_BACK.to_string(),
    );
    define("LARGE_CLASS", &runtime_abi::LARGE_CLASS.to_string());
    define("SEGMENT_SIZE", &runtime_abi::SEGMENT_SIZE.to_string());
    define(
        "REGION_CURSOR_OFFSET",
        &runtime_abi::REGION_CURSOR_OFFSET.to_string(),
    );
    define(
        "REGION_END_OFFSET",
        &runtime_abi::REGION_END_OFFSET.to_string(),
    );
    define(
        "REGION_POOL_IDLE_CAP",
        &runtime_abi::REGION_POOL_IDLE_CAP.to_string(),
    );
    for (name, value) in [
        ("CTX_DUP_MAX_OFFSET", runtime_abi::CTX_DUP_MAX_OFFSET),
        (
            "CTX_FREE_INLINE_OFFSET",
            runtime_abi::CTX_FREE_INLINE_OFFSET,
        ),
        ("CTX_IDLE_SLABS_OFFSET", runtime_abi::CTX_IDLE_SLABS_OFFSET),
        ("CTX_PAGES_OFFSET", runtime_abi::CTX_PAGES_OFFSET),
        ("CTX_FUEL_OFFSET", runtime_abi::CTX_FUEL_OFFSET),
        ("CTX_HEAP_ID_OFFSET", runtime_abi::CTX_HEAP_ID_OFFSET),
        (
            "CTX_FOREGROUND_COUNT_OFFSET",
            runtime_abi::CTX_FOREGROUND_COUNT_OFFSET,
        ),
        (
            "CTX_CLEANUP_COUNT_OFFSET",
            runtime_abi::CTX_CLEANUP_COUNT_OFFSET,
        ),
        (
            "CTX_CLEANUP_MODE_OFFSET",
            runtime_abi::CTX_CLEANUP_MODE_OFFSET,
        ),
        ("PAGE_OWNER_OFFSET", runtime_abi::PAGE_OWNER_OFFSET),
        (
            "PAGE_XTHREAD_FREE_OFFSET",
            runtime_abi::PAGE_XTHREAD_FREE_OFFSET,
        ),
        ("POOL_COUNT", runtime_abi::POOL_COUNT),
        ("PAGE_FREE_OFFSET", runtime_abi::PAGE_FREE_OFFSET),
        ("PAGE_LIVE_OFFSET", runtime_abi::PAGE_LIVE_OFFSET),
        (
            "PAGE_LOCAL_FREE_OFFSET",
            runtime_abi::PAGE_LOCAL_FREE_OFFSET,
        ),
        ("KIND_SMALL", u64::from(runtime_abi::KIND_SMALL)),
    ] {
        define(name, &value.to_string());
    }
    define(
        "LAYOUT_MAGIC",
        &format!("{:#x}u", runtime_abi::LAYOUT_MAGIC),
    );
    define(
        "LAYOUT_VERSION",
        &runtime_abi::LAYOUT_DESCRIPTOR_VERSION.to_string(),
    );
    define(
        "LAYOUT_UNSAFE_FFI",
        &runtime_abi::LAYOUT_UNSAFE_FFI.to_string(),
    );
    define("LAYOUT_DYNAMIC", &runtime_abi::LAYOUT_DYNAMIC.to_string());
    define(
        "LAYOUT_NO_EXTENT",
        &runtime_abi::LAYOUT_NO_EXTENT.to_string(),
    );
    for (name, value) in [
        ("RELEASE", runtime_abi::STEP_RELEASE),
        ("FINALIZE", runtime_abi::STEP_FINALIZE),
        ("CLOSE_HANDLE", runtime_abi::STEP_CLOSE_HANDLE),
        ("TAG", runtime_abi::STEP_TAG),
        ("LOOP", runtime_abi::STEP_LOOP),
        ("COLUMN", runtime_abi::STEP_COLUMN),
        ("NONNULL", runtime_abi::STEP_NONNULL),
        ("LIVE_BIT", runtime_abi::STEP_LIVE_BIT),
        ("RELEASE_LAST", runtime_abi::STEP_RELEASE_LAST),
        ("RELEASE_RECEIPT", runtime_abi::STEP_RELEASE_RECEIPT),
        ("RELEASE_SEALED", runtime_abi::STEP_RELEASE_SEALED),
    ] {
        define(&format!("STEP_{name}"), &value.to_string());
    }
    define(
        "STEP_CURSOR_BITS",
        &runtime_abi::STEP_CURSOR_BITS.to_string(),
    );
    define("WHOLE_VALUE", "UINT32_MAX");
    define("NO_OFFSET", "UINT64_MAX");
    for thunk in [runtime_abi::DROP_THUNK, runtime_abi::CLOSE_THUNK] {
        let typedef = thunk.c_typedef();
        assert!(
            normalized.contains(&typedef),
            "{typedef} is not in hirsel_rt_v2.h"
        );
    }
    define("FREE_POISON", &format!("{:#x}", runtime_abi::FREE_POISON));
    define(
        "UNINIT_POISON",
        &format!("{:#x}", runtime_abi::UNINIT_POISON),
    );
    for (tag, name) in [
        (runtime_abi::DomainTag::Heap, "HEAP"),
        (runtime_abi::DomainTag::Static, "STATIC"),
        (runtime_abi::DomainTag::Region, "REGION"),
    ] {
        define(&format!("DOMAIN_{name}"), &(tag as u8).to_string());
    }
    use runtime_abi::BlockState as B;
    for (state, name) in [
        (B::Free, "FREE"),
        (B::Live, "LIVE"),
        (B::FinalizePending, "FINALIZE_PENDING"),
        (B::Finalizing, "FINALIZING"),
        (B::FieldsPending, "FIELDS_PENDING"),
        (B::Raw, "RAW"),
    ] {
        define(&format!("STATE_{name}"), &(state as u8).to_string());
    }
    let classes: Vec<_> = runtime_abi::SIZE_CLASSES
        .iter()
        .map(u64::to_string)
        .collect();
    let flat: String = header
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\\')
        .collect();
    assert!(
        flat.contains(&format!("CLASS_SIZES{{{}}}", classes.join(","))),
        "class table differs from HIRSEL_RT_V2_CLASS_SIZES"
    );
    assert_eq!(runtime_abi::HEADER_V2.len(), 2);
    assert_eq!(
        runtime_abi::HEADER_V2[1],
        (runtime_abi::HeaderField::Descriptor, ScalarKind::U32, 4)
    );
    let x86 = TargetSpec::x86_64_unknown_linux_gnu();
    let (last, kind, offset) = runtime_abi::HEADER_V2[runtime_abi::HEADER_V2.len() - 1];
    assert_eq!(
        offset + x86.scalar_size(kind),
        runtime_abi::HEADER_SIZE,
        "{last:?}"
    );
    for pair in runtime_abi::HEADER_V2.windows(2) {
        let (_, kind, offset) = pair[0];
        assert!(offset + x86.scalar_size(kind) <= pair[1].2, "{pair:?}");
        assert_eq!(
            pair[1].2 % u64::from(x86.scalar_align(pair[1].1)),
            0,
            "{pair:?}"
        );
    }
    assert_eq!(runtime_abi::COUNT_IMMORTAL, u32::MAX as u64);
    assert_eq!(
        runtime_abi::DomainTag::of(StorageKind::LocalRegion),
        None,
        "uncounted region storage has no header"
    );
    assert_eq!(
        runtime_abi::DomainTag::of(StorageKind::Heap),
        Some(runtime_abi::DomainTag::Heap)
    );
}

#[test]
fn size_classes_follow_design_section_4() {
    let classes = runtime_abi::SIZE_CLASSES;
    assert!(classes.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(
        &classes[..16],
        &(1..=16).map(|i| i * 16).collect::<Vec<_>>()[..]
    );
    for (i, &capacity) in classes.iter().enumerate().skip(16) {
        let octave = 256u64 << ((i - 16) / 4);
        assert_eq!(capacity, octave + octave / 4 * ((i as u64 - 16) % 4 + 1));
    }
    assert_eq!(*classes.last().unwrap(), 4096);
    for (i, &capacity) in classes.iter().enumerate() {
        assert_eq!(
            runtime_abi::size_class(capacity, 16),
            SizeClass::Small {
                index: i as u8,
                capacity
            }
        );
        let next = runtime_abi::size_class(capacity + 1, 8);
        match classes.get(i + 1) {
            Some(&bigger) => assert_eq!(
                next,
                SizeClass::Small {
                    index: i as u8 + 1,
                    capacity: bigger
                }
            ),
            None => assert_eq!(next, SizeClass::Large { capacity: 4112 }),
        }
    }
    assert_eq!(
        runtime_abi::size_class(0, 1),
        SizeClass::Small {
            index: 0,
            capacity: 16
        }
    );
    assert_eq!(
        runtime_abi::size_class(64, 32),
        SizeClass::Large { capacity: 64 }
    );
}

#[test]
fn runtime_failure_rules_round_trip_and_change_manifest_identity() {
    use hirsel_core_contract::{Disposition, Manifest, OperationId, OperationRow, RuleCode};
    let id = OperationId {
        definition: path("text"),
        declaration_digest: hash("text declaration"),
    };
    let mut manifest = Manifest::skeleton([id.clone()]);
    let mut row = OperationRow::unclassified(id.clone());
    row.dispositions.native = Disposition::RuntimePrimitive("hirsel_rt_v2_text".into());
    manifest.replace(row.clone()).unwrap();
    let no_failure = lir_snapshot::digest("manifest", &manifest);
    row.failure = Some(RuleCode("SEM-3".into()));
    manifest.replace(row.clone()).unwrap();
    let text = assert_round_trip(&manifest);
    assert!(text.contains("SEM-3"));
    let sem3 = lir_snapshot::digest("manifest", &manifest);
    assert_ne!(no_failure, sem3);
    row.failure = Some(RuleCode("CAP-2".into()));
    manifest.replace(row).unwrap();
    assert_ne!(sem3, lir_snapshot::digest("manifest", &manifest));
}
