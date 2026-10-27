//! A builder for hand-made LIR units and a fixed layout table, shared by the
//! validator tests. Every test builds a valid unit and its one-edit
//! mutation from the same function, so each rejection has an accepted twin.

use hirsel_core_contract::{
    Disposition, EngineDispositions, Manifest, OperationId, OperationRow, Passing, SemanticEffects,
};
use hirsel_lir::abi;
use hirsel_lir::abi_types::*;
use hirsel_lir::effect::Effects;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::operation::OperationDeclarations;
use hirsel_lir::runtime_abi::{HEADER_SIZE, HEADER_V2};
use hirsel_lir::stage::*;
use hirsel_lir::testing::UnitBuilder;
use hirsel_lir::validate::{self, Inputs, ViolationKind};
use hirsel_syntax::{ContentHash, DefPath, ItemPath, PackageId};

pub use hirsel_lir::validate::ViolationKind as K;

thread_local! {
    /// A test's session interner: every type row a test builds, owner shapes
    /// and operation signatures included, lives in it, as in a compile session.
    static SESSION: std::sync::Arc<hirsel_types::TypeInterner> =
        hirsel_types::TypeInterner::new(None);
}

/// This test thread's session interner.
pub fn session() -> std::sync::Arc<hirsel_types::TypeInterner> {
    SESSION.with(|s| s.clone())
}

/// A type builder over this test thread's session interner.
pub fn ty() -> hirsel_types::testing::Types {
    hirsel_types::testing::Types { store: session() }
}

/// An operation registry over this test thread's session interner.
pub fn registry() -> hirsel_layout::operation::OperationRegistry {
    hirsel_layout::operation::OperationRegistry {
        seal_sources: Default::default(),
        receipt_sources: Default::default(),
        pretransfer_adapters: Vec::new(),
        pretransfer_actuals: Vec::new(),
        invocation_bodies: Vec::new(),
        invocations: Default::default(),
        type_declarations: Default::default(),
        types: session(),
        signatures: Default::default(),
        owners: Default::default(),
    }
}

/// The outermost scope every fixture unit has.
pub const ROOT: ScopeId = ScopeId(0);

pub fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "fixture".into(),
            version: "0".into(),
        },
        modules: vec!["validate".into()],
        name: name.into(),
    })
}

pub fn hash(label: &str) -> ContentHash {
    ContentHash::of_bytes(label.as_bytes())
}

/// Layout ids of the fixture table.
pub mod l {
    use hirsel_lir::layout_types::LayoutId;
    pub const UNIT: LayoutId = LayoutId(0);
    pub const BOOL: LayoutId = LayoutId(1);
    pub const I64: LayoutId = LayoutId(2);
    pub const U64: LayoutId = LayoutId(3);
    pub const PTR: LayoutId = LayoutId(4);
    /// A 16-byte, 8-aligned record of two i64 fields.
    pub const PAY: LayoutId = LayoutId(5);
    /// A counted block of `PAY`.
    pub const BLK: LayoutId = LayoutId(6);
    /// An inline record of two counted `BLK` fields.
    pub const PAIR: LayoutId = LayoutId(7);
    /// A counted block of `PAIR`.
    pub const PAIRBLK: LayoutId = LayoutId(8);
    /// A resource with a user finalizer.
    pub const RES: LayoutId = LayoutId(9);
    /// A counted block of `RES`.
    pub const RESBLK: LayoutId = LayoutId(10);
    /// The counted block header: raw reuse storage.
    pub const HDR: LayoutId = LayoutId(11);
    /// A 64-byte record (another size class).
    pub const BIG: LayoutId = LayoutId(12);
    pub const BIGBLK: LayoutId = LayoutId(13);
    /// A 16-byte, 16-aligned record (same class, other alignment).
    pub const WIDE: LayoutId = LayoutId(14);
    pub const WIDEBLK: LayoutId = LayoutId(15);
    /// A variant: tag 0 empty, tag 1 one i64.
    pub const OPT: LayoutId = LayoutId(16);
    /// Header field scalars.
    pub const U32: LayoutId = LayoutId(17);
    pub const U8: LayoutId = LayoutId(18);
    /// A String slice: owner block, start and length over `U8` elements.
    pub const SLICE: LayoutId = LayoutId(19);
    /// A closure environment of one i64 capture.
    pub const ENV: LayoutId = LayoutId(20);
    /// The callable specialized to `ENV`.
    pub const CLOSURE: LayoutId = LayoutId(21);
    /// The erased callable, many and once.
    pub const ERASED: LayoutId = LayoutId(22);
    pub const ERASED_ONCE: LayoutId = LayoutId(23);
    /// `flat type Item { text: String, n: u64 }`: the length-prefixed text,
    /// then `n` at static offset 8 (its cursor further by the prefix value).
    pub const FLAT_TEXT: LayoutId = LayoutId(24);
    pub const FLAT_ITEM: LayoutId = LayoutId(25);
    /// A variable `Item` behind a counted block of its bytes.
    pub const FLAT_ITEM_BLK: LayoutId = LayoutId(26);
    /// An `Item` inside a flat value: behind its own length prefix.
    pub const FLAT_WRAP: LayoutId = LayoutId(27);
    /// A flat `List[Item]`: prefix, count, then prefixed elements.
    pub const FLAT_LIST: LayoutId = LayoutId(28);
    /// `flat type Items { items: List[Item] }` and its block.
    pub const FLAT_ITEMS: LayoutId = LayoutId(29);
    pub const FLAT_ITEMS_BLK: LayoutId = LayoutId(30);
    /// `flat type Outer { inner: Item, k: u64 }` and its block.
    pub const FLAT_OUTER: LayoutId = LayoutId(32);
    pub const FLAT_OUTER_BLK: LayoutId = LayoutId(33);
    /// `Leaf | Node(BLK, i64)` as a null-pointer niche (layout version 2):
    /// no tag word, the `BLK` word at offset 0 is null for `Leaf`.
    pub const NICHE: LayoutId = LayoutId(34);
    /// A counted block of `NICHE`.
    pub const NICHEBLK: LayoutId = LayoutId(35);
    /// A resource with a user finalizer, an i64 and a counted `BLK` field
    /// (DROP-2-reverse-fields' shape).
    pub const RESCNT: LayoutId = LayoutId(36);
    /// `None | Two(BLK, BLK)`: an ordinal tag and two counted fields in its
    /// one dataful variant.
    pub const TWOBOX: LayoutId = LayoutId(37);
    /// Scalars for the named scalar operations (LIR schema revision 6).
    pub const F64: LayoutId = LayoutId(38);
    pub const I32: LayoutId = LayoutId(39);
    /// A counted sequence payload of i64 (List and Array storage) and its
    /// block (LIR schema revision 7).
    pub const SEQ: LayoutId = LayoutId(40);
    pub const SEQBLK: LayoutId = LayoutId(41);
    /// The borrowed cursor's opaque source: a block over no payload.
    pub const SOURCE: LayoutId = LayoutId(42);
    /// `Iter[i64]`: an environment, its step thunk and a zero word.
    pub const ITER: LayoutId = LayoutId(43);
    /// An arena token: one pointer word.
    pub const ARENA: LayoutId = LayoutId(44);
    /// f32, the other width `Reinterpret` pairs (LIR revision 23).
    pub const F32: LayoutId = LayoutId(45);
    /// A frozen `BLK` and the `Shared` root over it (LIR revision 27).
    pub const FROZEN: LayoutId = LayoutId(46);
    pub const SHARED: LayoutId = LayoutId(47);
    /// A closure environment of one borrowed `BLK` capture, and the callable
    /// specialized to it.
    pub const ENV_BLK: LayoutId = LayoutId(48);
    pub const CLOSURE_BLK: LayoutId = LayoutId(49);
}

fn record(table: &LayoutTable, kind: LayoutKind, size: u64, align: u32) -> LayoutRecord {
    LayoutRecord {
        key: LayoutIdentity {
            owner_revision: None,
            instance: None,
            target: table.target,
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
        storage: vec![
            StorageKind::Heap,
            StorageKind::LocalRegion,
            StorageKind::Arena,
            StorageKind::Frozen,
            StorageKind::Static,
        ],
        abi: AbiInputs {
            scalars: Vec::new(),
            unaligned: false,
        },
    }
}

fn field(ordinal: u32, offset: u64, layout: LayoutId) -> FieldLayout {
    FieldLayout {
        ordinal,
        offset,
        layout,
        unaligned: false,
    }
}

pub fn table() -> LayoutTable {
    let mut t = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
    let block = |t: &LayoutTable, payload| {
        let mut r = record(t, LayoutKind::Block { payload }, 8, 8);
        r.copy = CopyRecipe::Dup;
        r.drop = vec![DropStep::Release { offset: 0 }];
        r.scan = vec![ScanStep::Counted { offset: 0 }];
        r
    };
    let mut layouts = vec![
        record(&t, LayoutKind::Unit, 0, 1),
        record(&t, LayoutKind::Scalar(ScalarKind::Bool), 1, 1),
        record(&t, LayoutKind::Scalar(ScalarKind::I64), 8, 8),
        record(&t, LayoutKind::Scalar(ScalarKind::U64), 8, 8),
        record(&t, LayoutKind::Scalar(ScalarKind::Pointer), 8, 8),
    ];
    let mut pay = record(&t, LayoutKind::Aggregate, 16, 8);
    pay.fields = vec![field(0, 0, l::I64), field(1, 8, l::I64)];
    layouts.push(pay);
    layouts.push(block(&t, l::PAY));
    let mut pair = record(&t, LayoutKind::Aggregate, 16, 8);
    pair.fields = vec![field(0, 0, l::BLK), field(1, 8, l::BLK)];
    pair.copy = CopyRecipe::Fields;
    pair.drop = vec![
        DropStep::Release { offset: 8 },
        DropStep::Release { offset: 0 },
    ];
    pair.scan = vec![
        ScanStep::Counted { offset: 0 },
        ScanStep::Counted { offset: 8 },
    ];
    layouts.push(pair);
    layouts.push(block(&t, l::PAIR));
    let mut res = record(&t, LayoutKind::Resource, 8, 8);
    res.fields = vec![field(0, 0, l::I64)];
    res.copy = CopyRecipe::NotCopyable;
    res.drop = vec![DropStep::Finalize];
    res.finalizer = Some(FinalizerRef {
        definition: path("Res"),
        body: 1,
        unsafe_ffi_package: false,
    });
    layouts.push(res);
    layouts.push(block(&t, l::RES));
    let mut header = record(&t, LayoutKind::Header, HEADER_SIZE, 8);
    header.fields = HEADER_V2
        .iter()
        .enumerate()
        .map(|(i, &(_, scalar, offset))| {
            let layout = match scalar {
                ScalarKind::U64 => l::U64,
                ScalarKind::Pointer => l::PTR,
                ScalarKind::U32 => l::U32,
                _ => l::U8,
            };
            field(i as u32, offset, layout)
        })
        .collect();
    header.copy = CopyRecipe::NotCopyable;
    header.storage = vec![StorageKind::Heap, StorageKind::Static];
    layouts.push(header);
    let mut big = record(&t, LayoutKind::Aggregate, 64, 8);
    big.fields = (0..8).map(|i| field(i, 8 * u64::from(i), l::I64)).collect();
    layouts.push(big);
    layouts.push(block(&t, l::BIG));
    let mut wide = record(&t, LayoutKind::Aggregate, 16, 16);
    wide.fields = vec![field(0, 0, l::I64), field(1, 8, l::I64)];
    layouts.push(wide);
    layouts.push(block(&t, l::WIDE));
    let mut opt = record(&t, LayoutKind::Variant, 16, 8);
    opt.tag = Some(TagEncoding {
        offset: 0,
        form: TagForm::Ordinal,
        variants: vec![
            VariantLayout {
                ordinal: 0,
                fields: Vec::new(),
            },
            VariantLayout {
                ordinal: 1,
                fields: vec![field(0, 8, l::I64)],
            },
        ],
    });
    layouts.push(opt);
    layouts.push(record(&t, LayoutKind::Scalar(ScalarKind::U32), 4, 4));
    layouts.push(record(&t, LayoutKind::Scalar(ScalarKind::U8), 1, 1));
    let mut slice = record(&t, LayoutKind::Slice { element: l::U8 }, 24, 8);
    slice.fields = vec![
        field(0, 0, l::PTR),
        field(1, 8, l::U64),
        field(2, 16, l::U64),
    ];
    slice.copy = CopyRecipe::Dup;
    slice.scan = vec![ScanStep::SliceOwner { offset: 0 }];
    slice.drop = vec![DropStep::Release { offset: 0 }];
    layouts.push(slice);
    let mut env = record(&t, LayoutKind::Aggregate, 8, 8);
    env.fields = vec![field(0, 0, l::I64)];
    layouts.push(env);
    let callable = |t: &LayoutTable, environment, copy| {
        let mut r = record(t, LayoutKind::Callable { environment }, 16, 8);
        r.fields = vec![field(0, 0, l::PTR), field(1, 8, l::PTR)];
        r.copy = copy;
        r.scan = vec![ScanStep::Environment { offset: 8 }];
        r.drop = vec![DropStep::Environment { offset: 8 }];
        r
    };
    layouts.push(callable(&t, Some(l::ENV), CopyRecipe::Dup));
    layouts.push(callable(&t, None, CopyRecipe::Dup));
    layouts.push(callable(&t, None, CopyRecipe::NotCopyable));
    let flat = |t: &LayoutTable, fields: Vec<FieldLayout>| {
        let mut r = record(t, LayoutKind::Flat, 0, 1);
        r.size = SizeSpec::Dynamic(vec![
            SizeTerm::Constant(8),
            SizeTerm::ReadLength {
                offset: 0,
                scalar: ScalarKind::U64,
            },
            SizeTerm::Add,
        ]);
        r.fields = fields;
        r.initialized = InitializedBytes::ZeroPadding;
        r.copy = CopyRecipe::FlatBytes;
        r
    };
    layouts.push(flat(&t, vec![field(0, 8, l::U8)]));
    let mut n = field(1, 8, l::U64);
    n.unaligned = true;
    layouts.push(flat(&t, vec![field(0, 0, l::FLAT_TEXT), n.clone()]));
    let flat_block = |t: &LayoutTable, payload| {
        let mut r = block(t, payload);
        r.copy = CopyRecipe::FlatBytes;
        r
    };
    layouts.push(flat_block(&t, l::FLAT_ITEM));
    layouts.push(flat(&t, vec![field(0, 8, l::FLAT_ITEM)]));
    layouts.push(flat(&t, vec![field(0, 16, l::FLAT_WRAP)]));
    layouts.push(flat(&t, vec![field(0, 0, l::FLAT_LIST)]));
    layouts.push(flat_block(&t, l::FLAT_ITEMS));
    let mut element = field(0, 8, l::U64);
    element.unaligned = true;
    layouts.push(flat(&t, vec![element]));
    layouts.push(flat(&t, vec![field(0, 0, l::FLAT_WRAP), n]));
    layouts.push(flat_block(&t, l::FLAT_OUTER));
    let mut niche = record(&t, LayoutKind::Variant, 16, 8);
    let form = TagForm::Niche {
        nullary: 0,
        dataful: 1,
    };
    niche.tag = Some(TagEncoding {
        offset: 0,
        form,
        variants: vec![
            VariantLayout {
                ordinal: 0,
                fields: Vec::new(),
            },
            VariantLayout {
                ordinal: 1,
                fields: vec![field(0, 0, l::BLK), field(1, 8, l::I64)],
            },
        ],
    });
    niche.scan = vec![ScanStep::ActiveVariant {
        tag_offset: 0,
        form,
        variants: vec![(1, vec![ScanStep::Counted { offset: 0 }])],
    }];
    niche.drop = vec![DropStep::ActiveVariant {
        tag_offset: 0,
        form,
        variants: vec![(1, vec![DropStep::Release { offset: 0 }])],
    }];
    niche.copy = CopyRecipe::Fields;
    layouts.push(niche);
    layouts.push(block(&t, l::NICHE));
    let mut counted = record(&t, LayoutKind::Resource, 16, 8);
    counted.fields = vec![field(0, 0, l::I64), field(1, 8, l::BLK)];
    counted.copy = CopyRecipe::NotCopyable;
    counted.drop = vec![DropStep::Finalize, DropStep::Release { offset: 8 }];
    counted.scan = vec![ScanStep::Counted { offset: 8 }];
    counted.finalizer = Some(FinalizerRef {
        definition: path("Counted"),
        body: 1,
        unsafe_ffi_package: false,
    });
    layouts.push(counted);
    let mut twobox = record(&t, LayoutKind::Variant, 24, 8);
    twobox.tag = Some(TagEncoding {
        offset: 0,
        form: TagForm::Ordinal,
        variants: vec![
            VariantLayout {
                ordinal: 0,
                fields: Vec::new(),
            },
            VariantLayout {
                ordinal: 1,
                fields: vec![field(0, 8, l::BLK), field(1, 16, l::BLK)],
            },
        ],
    });
    twobox.scan = vec![ScanStep::ActiveVariant {
        tag_offset: 0,
        form: TagForm::Ordinal,
        variants: vec![(
            1,
            vec![
                ScanStep::Counted { offset: 8 },
                ScanStep::Counted { offset: 16 },
            ],
        )],
    }];
    twobox.drop = vec![DropStep::ActiveVariant {
        tag_offset: 0,
        form: TagForm::Ordinal,
        variants: vec![(
            1,
            vec![
                DropStep::Release { offset: 16 },
                DropStep::Release { offset: 8 },
            ],
        )],
    }];
    twobox.copy = CopyRecipe::Fields;
    layouts.push(twobox);
    layouts.push(record(&t, LayoutKind::Scalar(ScalarKind::F64), 8, 8));
    layouts.push(record(&t, LayoutKind::Scalar(ScalarKind::I32), 4, 4));
    let mut seq = record(&t, LayoutKind::Sequence { element: l::I64 }, 8, 8);
    seq.fields = vec![field(0, 0, l::U64), field(1, 8, l::U64)];
    seq.size = SizeSpec::Dynamic(vec![
        SizeTerm::Constant(16),
        SizeTerm::ReadLength {
            offset: 0,
            scalar: ScalarKind::U64,
        },
        SizeTerm::Constant(8),
        SizeTerm::Multiply,
        SizeTerm::Add,
    ]);
    layouts.push(seq);
    layouts.push(block(&t, l::SEQ));
    layouts.push(block(&t, l::UNIT));
    let mut iter = record(&t, LayoutKind::Iterator { element: l::I64 }, 24, 8);
    iter.fields = vec![
        field(0, 0, l::PTR),
        field(1, 8, l::PTR),
        field(2, 16, l::PTR),
    ];
    iter.copy = CopyRecipe::NotCopyable;
    iter.scan = vec![ScanStep::Environment { offset: 0 }];
    iter.drop = vec![DropStep::Environment { offset: 0 }];
    layouts.push(iter);
    let mut arena = record(&t, LayoutKind::Arena, 8, 8);
    arena.fields = vec![field(0, 0, l::PTR)];
    layouts.push(arena);
    layouts.push(record(&t, LayoutKind::Scalar(ScalarKind::F32), 4, 4));
    let mut frozen = record(&t, LayoutKind::Frozen { value: l::BLK }, 8, 8);
    frozen.fields = vec![field(0, 0, l::BLK)];
    frozen.copy = CopyRecipe::NotCopyable;
    frozen.scan = vec![ScanStep::Inline {
        offset: 0,
        layout: l::BLK,
    }];
    frozen.drop = vec![
        DropStep::Thaw {
            offset: 0,
            layout: l::BLK,
        },
        DropStep::Child {
            offset: 0,
            layout: l::BLK,
        },
    ];
    layouts.push(frozen);
    let mut shared = block(&t, l::FROZEN);
    shared.kind = LayoutKind::Shared { payload: l::FROZEN };
    shared.scan = vec![ScanStep::Root { offset: 0 }];
    shared.drop = vec![DropStep::ReleaseRoot { offset: 0 }];
    layouts.push(shared);
    let mut env_blk = record(&t, LayoutKind::Aggregate, 8, 8);
    env_blk.fields = vec![field(0, 0, l::BLK)];
    // A view: scanned uncounted, never released.
    env_blk.scan = vec![ScanStep::Uncounted { offset: 0 }];
    layouts.push(env_blk);
    layouts.push(callable(&t, Some(l::ENV_BLK), CopyRecipe::Dup));
    t.layouts = layouts;
    t
}

/// The representation a value of each fixture layout has by default.
pub fn repr_of(layout: LayoutId) -> ValueRepr {
    match layout {
        l::UNIT => ValueRepr::Unit,
        l::BOOL => ValueRepr::Scalar(ScalarKind::Bool),
        l::I64 => ValueRepr::Scalar(ScalarKind::I64),
        l::U64 => ValueRepr::Scalar(ScalarKind::U64),
        l::F64 => ValueRepr::Scalar(ScalarKind::F64),
        l::F32 => ValueRepr::Scalar(ScalarKind::F32),
        l::U32 => ValueRepr::Scalar(ScalarKind::U32),
        l::I32 => ValueRepr::Scalar(ScalarKind::I32),
        l::PTR => ValueRepr::RegionMark,
        l::BLK
        | l::PAIRBLK
        | l::RESBLK
        | l::BIGBLK
        | l::WIDEBLK
        | l::FLAT_ITEM_BLK
        | l::FLAT_ITEMS_BLK
        | l::FLAT_OUTER_BLK
        | l::NICHEBLK
        | l::SEQBLK
        | l::SHARED
        | l::SOURCE => ValueRepr::Block,
        l::HDR => ValueRepr::Token,
        l::ARENA => ValueRepr::Arena,
        l::CLOSURE | l::CLOSURE_BLK | l::ERASED | l::ERASED_ONCE => ValueRepr::Callable,
        _ => ValueRepr::Aggregate,
    }
}

pub fn operation() -> OperationId {
    OperationId {
        definition: path("op"),
        declaration_digest: hash("op declaration"),
    }
}

pub fn manifest(native: Disposition) -> Manifest {
    let mut m = Manifest::skeleton([operation()]);
    m.replace(OperationRow {
        id: operation(),
        ownership: None,
        effects: SemanticEffects::CONSERVATIVE,
        dispositions: EngineDispositions {
            reference: Disposition::Body,
            native,
            heap_check: Disposition::Body,
        },
        vectors: Vec::new(),
        failure: None,
    })
    .expect("a manifest row");
    m
}

/// One step of a cleanup sequence.
#[derive(Clone, Copy, Debug)]
pub enum Undo {
    End(LoanId),
    Release(ValueId),
    /// Drop a whole slot.
    Drop(SlotId),
}

/// A unit under construction. Blocks collect instructions until `finish`
/// lays them out in one arena.
pub struct Lir<S> {
    pub builder: UnitBuilder<S>,
    pub table: LayoutTable,
    pub manifest: Manifest,
    pub operations: hirsel_layout::operation::OperationRegistry,
    types: Vec<((ValueRepr, LayoutId), LirTypeId)>,
    own: (Vec<(Passing, LayoutId)>, Option<LayoutId>),
    anchor: SourceAnchorId,
}

pub const ENTRY: BlockId = BlockId(0);

impl<S: Stage> Lir<S> {
    /// A unit with an entry block and the given own signature.
    pub fn new(params: &[(Passing, LayoutId)], result: Option<LayoutId>) -> Self {
        let table = table();
        let mut unit = LirUnit::<S>::new(UnitHeader {
            source_count: 1,
            key: hirsel_lir::objects::UnitKey(hirsel_syntax::ContentHash([0; 32])),
            track_caller: false,
            stage: S::KIND,
            lir_version: hirsel_lir::LIR_VERSION,
            origin: UnitOrigin::Source,
            definition: path("f"),
            implementation_revision: hash("f source"),
            instance: hash("f instance"),
            target: table.target,
            abi: FnAbiId(0),
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
                end: 1,
            },
        });
        unit.tables.operations.push(operation());
        let mut lir = Self {
            builder: UnitBuilder::new(unit),
            table,
            operations: registry(),
            manifest: manifest(Disposition::RuntimePrimitive("hirsel_rt_op".into())),
            types: Vec::new(),
            own: (params.to_vec(), result),
            anchor: SourceAnchorId(0),
        };
        let abi = lir.abi(params, result);
        lir.builder.unit.header.abi = abi;
        lir.builder.block(BlockKind::Body, &[]);
        lir
    }

    pub fn abi(&mut self, params: &[(Passing, LayoutId)], result: Option<LayoutId>) -> FnAbiId {
        self.abi_shaped(CallableShape::Definition, params, result)
    }

    /// Supply a declaration for the scalar Ready-operation fixtures. Rebase its
    /// derived scalar records onto this fixture's established layout numbering.
    pub fn declare_operation(
        &mut self,
        signature: hirsel_types::SignatureRef,
        owners: hirsel_layout::layout::Owners,
        abi: FnAbiId,
    ) {
        let operation = OperationId {
            definition: signature.definition.path().clone(),
            declaration_digest: signature.fingerprint,
        };
        self.operations = hirsel_layout::operation::OperationRegistry {
            seal_sources: Default::default(),
            receipt_sources: Default::default(),
            pretransfer_adapters: Vec::new(),
            pretransfer_actuals: Vec::new(),
            invocation_bodies: Vec::new(),
            invocations: Default::default(),
            type_declarations: Default::default(),
            types: session(),
            signatures: [(operation.clone(), signature)].into(),
            owners,
        };
        let (mut expected, table) = self
            .operations
            .derive(&operation, &[], &TargetSpec::x86_64_unknown_linux_gnu())
            .unwrap();
        let mut map = Vec::new();
        for record in &table.layouts {
            let i = if matches!(record.kind, LayoutKind::Unit | LayoutKind::Scalar(_)) {
                let i = self
                    .table
                    .layouts
                    .iter()
                    .position(|r| r.kind == record.kind)
                    .unwrap();
                self.table.layouts[i] = record.clone();
                i
            } else {
                assert_eq!(record.kind, LayoutKind::Aggregate);
                let i = self.table.layouts.len();
                self.table.layouts.push(record.clone());
                i
            };
            map.push(LayoutId(i as u32));
        }
        for (record, &mapped) in table.layouts.iter().zip(&map) {
            for (field, original) in self.table.layouts[mapped.0 as usize]
                .fields
                .iter_mut()
                .zip(&record.fields)
            {
                field.layout = map[original.layout.0 as usize];
            }
        }
        for p in &mut expected.parameters {
            p.layout = map[p.layout.0 as usize];
        }
        if let Some(r) = &mut expected.result {
            r.layout = map[r.layout.0 as usize];
        }
        self.table.fn_abis[abi.0 as usize] = expected;
        self.builder.unit.tables.operations = vec![operation.clone()];
        let mut row = self.manifest.rows().next().unwrap().clone();
        row.id = operation;
        self.manifest = Manifest::skeleton([row.id.clone()]);
        self.manifest.replace(row).unwrap();
    }

    pub fn abi_shaped(
        &mut self,
        shape: CallableShape,
        params: &[(Passing, LayoutId)],
        result: Option<LayoutId>,
    ) -> FnAbiId {
        let id = FnAbiId(self.table.fn_abis.len() as u32);
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let pointer = abi::slot(&target, ScalarKind::Pointer);
        let parameters = params
            .iter()
            .enumerate()
            .map(|(i, &(convention, layout))| {
                let kind = &self.table.layouts[layout.0 as usize].kind;
                let (passing, slots, extension) = match (convention, kind) {
                    (Passing::Mut, _) => (
                        ParameterPassing::ExclusiveAddress,
                        vec![pointer],
                        ScalarExtension::None,
                    ),
                    (_, LayoutKind::Scalar(k)) => (
                        ParameterPassing::ByValue,
                        vec![abi::slot(&target, *k)],
                        abi::extension_of(*k),
                    ),
                    (_, LayoutKind::Unit) => {
                        (ParameterPassing::ByValue, Vec::new(), ScalarExtension::None)
                    }
                    (_, LayoutKind::Block { .. } | LayoutKind::Box { .. }) => (
                        ParameterPassing::ByValue,
                        vec![pointer],
                        ScalarExtension::None,
                    ),
                    (Passing::Take, _) => (
                        ParameterPassing::TransferAddress,
                        vec![pointer],
                        ScalarExtension::None,
                    ),
                    _ => (
                        ParameterPassing::ReadOnlyAddress,
                        vec![pointer],
                        ScalarExtension::None,
                    ),
                };
                AbiParameter {
                    source: i as u32,
                    convention,
                    passing,
                    layout,
                    slots,
                    extension,
                }
            })
            .collect();
        self.table.fn_abis.push(FnAbi {
            receipts: None,
            nonretaining: Vec::new(),
            nonretaining_captures: Vec::new(),
            scoped_contract: hirsel_lir::abi::scoped_contract_digest(&[], &[]),
            scoped_entries: Vec::new(),
            key: FnAbiKey {
                receipt_contract: None,
                callable: hash(&format!("callable {}", id.0)),
                abi_version: hirsel_lir::ABI_VERSION,
            },
            shape,
            multiplicity: CallMultiplicity::Many,
            context: true,
            environment: shape == CallableShape::Closure,
            caller_site: shape == CallableShape::Closure,
            destination: false,
            result: result.map(|layout| AbiResult {
                borrowed_parameters: vec![],
                layout,
                slots: vec![pointer],
                register: hirsel_lir::abi::register_kind(
                    &self.table.layouts[layout.0 as usize].kind,
                ),
            }),
            result_arena: None,
            diverges: false,
            parameters,
            version: hirsel_lir::ABI_VERSION,
        });
        id
    }

    pub fn ty(&mut self, repr: ValueRepr, layout: LayoutId) -> LirTypeId {
        if let Some(&(_, id)) = self.types.iter().find(|(key, _)| *key == (repr, layout)) {
            return id;
        }
        self.builder.unit.types.push(LirType { repr, layout });
        let id = LirTypeId(self.builder.unit.types.len() as u32 - 1);
        self.types.push(((repr, layout), id));
        id
    }

    pub fn value_as(&mut self, repr: ValueRepr, layout: LayoutId, holding: Holding) -> ValueId {
        let ty = self.ty(repr, layout);
        self.builder.unit.values.push(ValueInfo { ty, holding });
        ValueId(self.builder.unit.values.len() as u32 - 1)
    }

    /// A value of the layout's default representation.
    pub fn value(&mut self, layout: LayoutId, holding: Holding) -> ValueId {
        self.value_as(repr_of(layout), layout, holding)
    }
    pub fn owned(&mut self, layout: LayoutId) -> ValueId {
        self.value(layout, Holding::Owned)
    }
    pub fn plain(&mut self, layout: LayoutId) -> ValueId {
        self.value(layout, Holding::Plain)
    }
    pub fn borrowed(&mut self, layout: LayoutId, loan: LoanId) -> ValueId {
        self.value(layout, Holding::Borrowed(loan))
    }

    /// Declare the value of the next own parameter.
    pub fn param(&mut self, value: ValueId) -> ValueId {
        self.builder.unit.parameters.push(value);
        value
    }

    /// A slot of `scope`. A capture slot also gets the version name of its
    /// environment content (`LirUnit::captures`).
    pub fn slot(&mut self, layout: LayoutId, kind: SlotKind, scope: ScopeId) -> SlotId {
        let ty = self.ty(repr_of(layout), layout);
        self.builder.unit.slots.push(SlotInfo { ty, kind, scope });
        if let SlotKind::Capture(i) = kind {
            let version = self.plain(layout);
            let captures = &mut self.builder.unit.captures;
            if captures.len() <= i as usize {
                captures.resize(i as usize + 1, version);
            }
            captures[i as usize] = version;
        }
        SlotId(self.builder.unit.slots.len() as u32 - 1)
    }

    pub fn address(
        &mut self,
        base: AddressBase,
        steps: &[ProjectionStep],
        layout: LayoutId,
    ) -> AddressId {
        let ty = self.ty(repr_of(layout), layout);
        let projection = PoolRange::push(
            &mut self.builder.unit.pools.projections,
            steps.iter().copied(),
        );
        self.builder.unit.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base,
            projection,
            ty,
        });
        AddressId(self.builder.unit.addresses.len() as u32 - 1)
    }

    pub fn failure_disposing(
        &mut self,
        target: BlockId,
        args: &[ValueId],
        dispose: &[ValueId],
    ) -> FailureId {
        let cleanup = self.builder.edge(target, args);
        let dispose = PoolRange::push(&mut self.builder.unit.pools.values, dispose.iter().copied());
        self.builder.unit.tables.failures.push(FailureEdge {
            cleanup,
            rule: None,
            anchor: self.anchor,
            dispose,
        });
        FailureId(self.builder.unit.tables.failures.len() as u32 - 1)
    }

    pub fn failure(&mut self, target: BlockId) -> FailureId {
        self.failure_disposing(target, &[], &[])
    }

    /// A failure edge to plain propagation.
    pub fn fails(&mut self) -> FailureId {
        let p = self.builder.propagate();
        self.failure(p)
    }

    /// A failure-cleanup block releasing `values` in order, then propagating.
    pub fn releasing(&mut self, values: &[ValueId]) -> BlockId {
        let steps: Vec<_> = values.iter().map(|&v| Undo::Release(v)).collect();
        self.cleanup(&steps)
    }

    /// A failure-cleanup block running `steps` in order, then propagating. A
    /// release or drop that fails continues with the remaining steps; a
    /// slot's address is prepared at the block's start and retries the block.
    pub fn cleanup(&mut self, steps: &[Undo]) -> BlockId {
        if steps.is_empty() {
            return self.builder.propagate();
        }
        let b = self.builder.block(BlockKind::FailureCleanup, &[]);
        self.run(b, steps, b);
        self.builder.term(b, Terminator::Propagate);
        b
    }

    /// Run `steps` at the end of `b`; a failing address preparation goes to
    /// `retry`, a failing release or drop to the cleanup of the rest.
    pub fn run(&mut self, b: BlockId, steps: &[Undo], retry: BlockId) {
        self.run_then(b, steps, retry, None);
    }

    /// A normal-exit cleanup block running `steps`, then returning `result`.
    /// Its address preparations and failures continue in failure cleanup.
    pub fn cleanup_exit(&mut self, steps: &[Undo], result: Option<ValueId>) -> BlockId {
        let b = self.builder.block(BlockKind::Cleanup, &[]);
        let mut with_result = steps.to_vec();
        with_result.extend(result.map(Undo::Release));
        // Only slot drops prepare an address that may retry the cleanup.
        let retry = if steps.iter().any(|s| matches!(s, Undo::Drop(_))) {
            self.cleanup(&with_result)
        } else {
            b
        };
        self.run_then(b, steps, retry, result);
        self.builder.ret(b, result);
        b
    }

    fn run_then(&mut self, b: BlockId, steps: &[Undo], retry: BlockId, result: Option<ValueId>) {
        let mut addresses = Vec::new();
        for step in steps {
            if let Undo::Drop(slot) = *step {
                let layout = self.builder.unit.types
                    [self.builder.unit.slots[slot.index()].ty.index()]
                .layout;
                let a = self.address(AddressBase::Slot(slot), &[], layout);
                let failure = self.failure(retry);
                self.builder.push(
                    b,
                    Inst::Address {
                        address: a,
                        failure,
                    },
                );
                addresses.push(a);
            }
        }
        let mut addresses = addresses.into_iter();
        for (i, step) in steps.iter().enumerate() {
            let mut rest = steps[i + 1..].to_vec();
            rest.extend(result.map(Undo::Release));
            match *step {
                Undo::End(loan) => self.end(b, loan),
                Undo::Release(v) => {
                    let target = self.cleanup(&rest);
                    let failure = self.failure(target);
                    self.release_with(b, v, failure);
                }
                Undo::Drop(_) => {
                    let target = self.cleanup(&rest);
                    let failure = self.failure(target);
                    let anchor = self.cleanup_anchor();
                    let address = addresses.next().unwrap();
                    self.builder.push(
                        b,
                        Inst::Drop {
                            address,
                            if_live: false,
                            anchor,
                            failure,
                        },
                    );
                }
            }
        }
    }

    /// Release `value` in `b`; a failure runs `undo`.
    pub fn release_then(&mut self, b: BlockId, value: ValueId, undo: &[Undo]) {
        let target = self.cleanup(undo);
        let failure = self.failure(target);
        self.release_with(b, value, failure);
    }

    pub fn cleanup_anchor(&mut self) -> CleanupAnchorId {
        self.builder
            .unit
            .tables
            .cleanup_anchors
            .push(CleanupAnchor {
                body: 0,
                cleanup: self.builder.unit.tables.cleanup_anchors.len() as u32,
                action: 0,
                site: SiteRange {
                    source: 0,
                    start: 0,
                    end: 1,
                },
            });
        CleanupAnchorId(self.builder.unit.tables.cleanup_anchors.len() as u32 - 1)
    }

    /// An anchor at the reserved ordinal of a drop TIR states as its own
    /// instruction, which may sit in any block.
    pub fn instruction_anchor(&mut self) -> CleanupAnchorId {
        let anchors = &mut self.builder.unit.tables.cleanup_anchors;
        anchors.push(CleanupAnchor {
            body: 0,
            cleanup: INSTRUCTION_DROP,
            action: anchors.len() as u32,
            site: SiteRange {
                source: 0,
                start: 0,
                end: 1,
            },
        });
        CleanupAnchorId(anchors.len() as u32 - 1)
    }

    pub fn payload(&self, value: ValueId) -> LayoutId {
        let ty = &self.builder.unit.types[self.builder.unit.values[value.index()].ty.index()];
        match self.table.layouts[ty.layout.0 as usize].kind {
            LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } => payload,
            _ => ty.layout,
        }
    }

    pub fn release_with(&mut self, b: BlockId, value: ValueId, failure: FailureId) {
        let layout = self.payload(value);
        let anchor = self.cleanup_anchor();
        self.builder.push(
            b,
            Inst::Release {
                value,
                layout,
                live: MaskRef::All,
                anchor,
                failure,
            },
        );
    }

    /// Release `value` in `b`; failure releases `then_failing` and propagates.
    pub fn release(&mut self, b: BlockId, value: ValueId, then_failing: &[ValueId]) {
        let rest = self.releasing(then_failing);
        let failure = self.failure(rest);
        self.release_with(b, value, failure);
    }

    /// Prepare an address in `b` (failure: release `live` and propagate).
    pub fn prepare(&mut self, b: BlockId, address: AddressId, live: &[ValueId]) {
        let target = self.releasing(live);
        let failure = self.failure(target);
        self.builder.push(b, Inst::Address { address, failure });
    }

    /// Prepare an address in `b`, failing into the cleanup `undo`.
    pub fn prepare_undo(&mut self, b: BlockId, address: AddressId, undo: &[Undo]) {
        let target = self.cleanup(undo);
        let failure = self.failure(target);
        self.builder.push(b, Inst::Address { address, failure });
    }

    /// A slot address, prepared in `b`.
    pub fn at(
        &mut self,
        b: BlockId,
        slot: SlotId,
        steps: &[ProjectionStep],
        undo: &[Undo],
    ) -> AddressId {
        let layout = self.place_layout(AddressBase::Slot(slot), steps);
        let a = self.address(AddressBase::Slot(slot), steps, layout);
        self.prepare_undo(b, a, undo);
        a
    }

    /// A payload address of a block value, prepared in `b`.
    pub fn through(
        &mut self,
        b: BlockId,
        value: ValueId,
        steps: &[ProjectionStep],
        undo: &[Undo],
    ) -> AddressId {
        let layout = self.place_layout(AddressBase::Value(value), steps);
        let a = self.address(AddressBase::Value(value), steps, layout);
        self.prepare_undo(b, a, undo);
        a
    }

    pub fn loan(&mut self, address: AddressId, mutable: bool) -> LoanId {
        self.builder
            .unit
            .tables
            .loans
            .push(LoanInfo { address, mutable });
        LoanId(self.builder.unit.tables.loans.len() as u32 - 1)
    }

    pub fn begin(&mut self, b: BlockId, loan: LoanId) {
        let info = self.builder.unit.tables.loans[loan.index()].clone();
        self.builder.push(
            b,
            Inst::BeginLoan {
                loan,
                address: info.address,
                mutable: info.mutable,
                if_inactive: false,
            },
        );
    }

    pub fn end(&mut self, b: BlockId, loan: LoanId) {
        self.builder.push(
            b,
            Inst::EndLoan {
                loan,
                if_active: false,
            },
        );
    }

    /// A read of `address`; the result is plain, owned or borrowed by mode.
    pub fn read(
        &mut self,
        b: BlockId,
        address: AddressId,
        reaching: ValueId,
        mode: ReadMode,
        loan: Option<LoanId>,
    ) -> ValueId {
        let layout =
            self.builder.unit.types[self.builder.unit.addresses[address.index()].ty.index()].layout;
        let result = match (mode, loan) {
            (ReadMode::Borrow, Some(loan)) => self.borrowed(layout, loan),
            (ReadMode::Take, _) => self.owned_or_plain(layout),
            _ => self.plain(layout),
        };
        self.builder.push(
            b,
            Inst::Read {
                result,
                address,
                reaching,
                mode,
            },
        );
        result
    }

    /// Store `value` into `address`; returns the new version.
    pub fn store(
        &mut self,
        b: BlockId,
        address: AddressId,
        value: ValueId,
        slot: SlotId,
    ) -> ValueId {
        let ty = self.builder.unit.slots[slot.index()].ty;
        self.builder.unit.values.push(ValueInfo {
            ty,
            holding: Holding::Plain,
        });
        let version = ValueId(self.builder.unit.values.len() as u32 - 1);
        self.builder.push(
            b,
            Inst::Store {
                address,
                value,
                version,
            },
        );
        version
    }

    /// A plain version value of `slot`'s type (a parameter or join).
    pub fn version_of(&mut self, slot: SlotId) -> ValueId {
        let ty = self.builder.unit.slots[slot.index()].ty;
        self.builder.unit.values.push(ValueInfo {
            ty,
            holding: Holding::Plain,
        });
        ValueId(self.builder.unit.values.len() as u32 - 1)
    }

    fn place_layout(&self, base: AddressBase, steps: &[ProjectionStep]) -> LayoutId {
        let mut layout = match base {
            AddressBase::Slot(s) => {
                self.builder.unit.types[self.builder.unit.slots[s.index()].ty.index()].layout
            }
            AddressBase::Value(v) => self.payload(v),
            AddressBase::Address(a) => {
                self.builder.unit.types[self.builder.unit.addresses[a.index()].ty.index()].layout
            }
        };
        for step in steps {
            if *step == ProjectionStep::Payload {
                let LayoutKind::Box { payload } = self.table.layouts[layout.0 as usize].kind else {
                    panic!("Payload outside Box")
                };
                layout = payload;
                continue;
            }
            if let LayoutKind::Block { payload }
            | LayoutKind::Box { payload }
            | LayoutKind::Shared { payload } = self.table.layouts[layout.0 as usize].kind
            {
                layout = payload;
            }
            let record = &self.table.layouts[layout.0 as usize];
            layout = match *step {
                ProjectionStep::Payload => unreachable!(),
                ProjectionStep::Field(f) => {
                    record
                        .fields
                        .iter()
                        .find(|x| x.ordinal == f)
                        .unwrap()
                        .layout
                }
                ProjectionStep::VariantField { variant, field } => {
                    record.tag.as_ref().unwrap().variants[variant as usize].fields[field as usize]
                        .layout
                }
                // A flat sequence's one field is the element.
                ProjectionStep::Index(_) | ProjectionStep::FlatAt { .. } => match record.kind {
                    LayoutKind::Flat => record.fields[0].layout,
                    LayoutKind::Sequence { element } | LayoutKind::Sequential { element } => {
                        element
                    }
                    _ => panic!("fixture has no other indexed layouts"),
                },
                ProjectionStep::Slot(_) => match record.kind {
                    LayoutKind::Sequence { element } => element,
                    _ => panic!("a slot step outside a sequence payload"),
                },
            };
        }
        layout
    }

    pub fn construct_site(
        &mut self,
        constructor: ConstructorKind,
        layout: LayoutId,
        fields: &[(u32, ValueId)],
        placement: Option<Placement>,
    ) -> ConstructId {
        let fields = PoolRange::push(
            &mut self.builder.unit.pools.fields,
            fields
                .iter()
                .map(|&(field, value)| FieldValue { field, value }),
        );
        self.builder.unit.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            tracked: false,
            constructor,
            layout,
            fields,
            destination: DestinationClass::Heap,
            placement,
            anchor: self.anchor,
        });
        ConstructId(self.builder.unit.tables.constructs.len() as u32 - 1)
    }

    /// The placement a construction needs at this stage: heap from Placed on.
    pub fn heap(&self) -> Option<Placement> {
        (S::KIND >= StageKind::Placed)
            .then_some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0))))
    }

    /// Construct a `PAY` block from two fresh i64 constants; returns it owned.
    pub fn new_block(&mut self, b: BlockId, undo: &[Undo]) -> ValueId {
        self.new_block_of(b, l::PAY, undo)
    }

    pub fn new_block_of(&mut self, b: BlockId, payload: LayoutId, undo: &[Undo]) -> ValueId {
        let ordinals: Vec<_> = self.table.layouts[payload.0 as usize]
            .fields
            .iter()
            .map(|f| f.ordinal)
            .collect();
        let fields: Vec<_> = ordinals.into_iter().map(|o| (o, self.int(b))).collect();
        let placement = self.heap();
        let symbol = self.symbol(SymbolKind::Type);
        let site =
            self.construct_site(ConstructorKind::Record(symbol), payload, &fields, placement);
        let block_layout = match payload {
            l::PAY => l::BLK,
            l::BIG => l::BIGBLK,
            l::WIDE => l::WIDEBLK,
            l::RES => l::RESBLK,
            l::PAIR => l::PAIRBLK,
            _ => panic!("no block layout"),
        };
        let result = self.owned(block_layout);
        let target = self.cleanup(undo);
        let failure = self.failure(target);
        self.builder.push(
            b,
            Inst::Construct {
                result,
                construct: site,
                failure,
            },
        );
        result
    }

    pub fn symbol(&mut self, kind: SymbolKind) -> SymbolId {
        self.builder.unit.tables.symbols.push(Symbol {
            kind,
            definition: path(&format!("s{}", self.builder.unit.tables.symbols.len())),
            instance: None,
        });
        SymbolId(self.builder.unit.tables.symbols.len() as u32 - 1)
    }

    /// A fresh i64 constant.
    pub fn int(&mut self, b: BlockId) -> ValueId {
        let ty = self.ty(repr_of(l::I64), l::I64);
        self.builder.unit.tables.constants.push(ConstantData {
            ty,
            value: ConstantValue::Integer(7),
        });
        let constant = ConstantId(self.builder.unit.tables.constants.len() as u32 - 1);
        let result = self.plain(l::I64);
        self.builder.push(b, Inst::Const { result, constant });
        result
    }

    pub fn boolean(&mut self, b: BlockId) -> ValueId {
        self.bool_const(b, true)
    }

    pub fn bool_const(&mut self, b: BlockId, value: bool) -> ValueId {
        let ty = self.ty(repr_of(l::BOOL), l::BOOL);
        self.builder.unit.tables.constants.push(ConstantData {
            ty,
            value: ConstantValue::Bool(value),
        });
        let constant = ConstantId(self.builder.unit.tables.constants.len() as u32 - 1);
        let result = self.plain(l::BOOL);
        self.builder.push(b, Inst::Const { result, constant });
        result
    }

    /// A drop flag of `slot` (LIR schema revision 8), to be a block parameter.
    pub fn flag(&mut self, slot: SlotId) -> ValueId {
        let value = self.plain(l::BOOL);
        self.builder.unit.tables.init_flags.push(InitFlag {
            path: PoolRange::EMPTY,
            value,
            slot,
            environment: None,
        });
        value
    }

    /// A branch on `condition`.
    pub fn branch_on(
        &mut self,
        b: BlockId,
        condition: ValueId,
        yes: (BlockId, &[ValueId]),
        no: (BlockId, &[ValueId]),
    ) {
        let yes = self.builder.edge(yes.0, yes.1);
        let no = self.builder.edge(no.0, no.1);
        self.builder
            .term(b, Terminator::Branch { condition, yes, no });
    }

    pub fn branch(&mut self, b: BlockId, yes: (BlockId, &[ValueId]), no: (BlockId, &[ValueId])) {
        let condition = self.boolean(b);
        let yes = self.builder.edge(yes.0, yes.1);
        let no = self.builder.edge(no.0, no.1);
        self.builder
            .term(b, Terminator::Branch { condition, yes, no });
    }

    /// A direct call; `failure` names the failure target. Returns the result.
    pub fn call(
        &mut self,
        b: BlockId,
        params: &[(Passing, LayoutId)],
        result: Option<LayoutId>,
        args: &[ArgumentValue],
        normal: (BlockId, &[ValueId]),
        failure: BlockId,
    ) -> Option<ValueId> {
        let abi = self.abi(params, result);
        let symbol = self.symbol(SymbolKind::Function);
        self.call_to(
            b,
            Callee::Direct(symbol),
            abi,
            params,
            result,
            args,
            normal,
            failure,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn call_to(
        &mut self,
        b: BlockId,
        callee: Callee,
        abi: FnAbiId,
        params: &[(Passing, LayoutId)],
        result: Option<LayoutId>,
        args: &[ArgumentValue],
        normal: (BlockId, &[ValueId]),
        failure: BlockId,
    ) -> Option<ValueId> {
        let arguments = PoolRange::push(
            &mut self.builder.unit.pools.call_arguments,
            params
                .iter()
                .zip(args)
                .enumerate()
                .map(|(i, (&(passing, _), &value))| CallArgument {
                    parameter: i as u32,
                    passing,
                    value,
                }),
        );
        let result = result.map(|layout| self.owned_or_plain(layout));
        self.builder.unit.tables.calls.push(CallSite {
            receipts: None,
            pretransfer_failure: None,
            forward_caller: true,
            operation_arguments: Vec::new(),
            callee,
            abi,
            arguments,
            result,
            destination: None,
            declared: Effects::ALL,
            anchor: self.anchor,
        });
        let call = CallId(self.builder.unit.tables.calls.len() as u32 - 1);
        let normal = self.builder.edge(normal.0, normal.1);
        let failure = self.failure(failure);
        self.builder.term(
            b,
            Terminator::Call {
                call,
                normal: Some(normal),
                failure,
            },
        );
        result
    }

    /// A diverging direct call through `abi`: no result and no normal edge.
    pub fn diverging_call(&mut self, b: BlockId, abi: FnAbiId, failure: BlockId) {
        let callee = Callee::Direct(self.symbol(SymbolKind::Function));
        self.builder.unit.tables.calls.push(CallSite {
            receipts: None,
            pretransfer_failure: None,
            forward_caller: true,
            operation_arguments: Vec::new(),
            callee,
            abi,
            arguments: PoolRange::EMPTY,
            result: None,
            destination: None,
            declared: Effects::ALL,
            anchor: self.anchor,
        });
        let call = CallId(self.builder.unit.tables.calls.len() as u32 - 1);
        let failure = self.failure(failure);
        self.builder.term(
            b,
            Terminator::Call {
                call,
                normal: None,
                failure,
            },
        );
    }

    /// Owned for a layout that releases something, plain otherwise.
    pub fn owned_or_plain(&mut self, layout: LayoutId) -> ValueId {
        let record = &self.table.layouts[layout.0 as usize];
        let owns = matches!(
            repr_of(layout),
            ValueRepr::Block | ValueRepr::Token | ValueRepr::RegionMark
        ) || !record.drop.is_empty()
            || record.finalizer.is_some();
        if owns {
            self.owned(layout)
        } else {
            self.plain(layout)
        }
    }

    pub fn finish(self) -> (LirUnit<S>, LayoutTable, Manifest) {
        let unit = self.builder.finish();
        let _ = &self.own;
        (unit, self.table, self.manifest)
    }

    /// Every violation of the finished unit.
    pub fn check(self) -> Vec<ViolationKind> {
        let operations = self.operations.clone();
        let (unit, table, manifest) = self.finish();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let inputs = Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &operations,
            target: &target,
            layouts: &table,
            manifest: &manifest,
        };
        let ordinary = validate::check(&unit, &inputs);
        let context = validate::ValidationContext::new(&inputs);
        // Every mutation and its accepted twin must agree when program checks
        // are first computed, then reused by a later stage/unit check.
        assert_eq!(context.check(&unit, &inputs), ordinary);
        assert_eq!(context.check(&unit, &inputs), ordinary);
        ordinary.into_iter().map(|v| v.kind).collect()
    }
}

/// The valid twin is accepted and the mutation reports `expected`.
#[track_caller]
pub fn pair(case: impl Fn(bool) -> Vec<ViolationKind>, expected: impl Fn(&ViolationKind) -> bool) {
    let valid = case(false);
    assert!(valid.is_empty(), "the valid twin is rejected: {valid:#?}");
    let mutant = case(true);
    if std::env::var_os("HIRSEL_SHOW_MUTANTS").is_some() {
        eprintln!("mutant: {mutant:?}");
    }
    assert!(
        mutant.iter().any(&expected),
        "the mutation is not rejected as expected: {mutant:#?}"
    );
}
