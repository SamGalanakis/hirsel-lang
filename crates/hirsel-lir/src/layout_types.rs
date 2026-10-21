//! Target descriptors and the single physical layout table.
//!
//! These are schemas only. Layout derivation and its independent verifier live
//! in `layout.rs` and `layout_validate.rs`; nothing here computes an offset.
use crate::abi_types::FnAbi;
use crate::snapshot;
use hirsel_codec::{snapshot_enum, snapshot_struct};
use hirsel_syntax::{ContentHash, DefPath};

/// Index into `LayoutTable::layouts`; meaningful only with that table's digest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct LayoutId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Endian {
    Little,
    Big,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ObjectFormat {
    Elf,
}

/// The target's C calling convention, used for C-facing wrappers only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CConvention {
    SysV64,
    Aapcs64,
}

/// Scalar value classes shared by layouts, LIR types and the runtime ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ScalarKind {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    /// An opaque target pointer (block reference, address or code symbol).
    Pointer,
}

/// ABI alignment in bytes of every scalar on one target.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ScalarAlignments {
    pub i8: u32,
    pub i16: u32,
    pub i32: u32,
    pub i64: u32,
    pub i128: u32,
    pub f32: u32,
    pub f64: u32,
    pub pointer: u32,
    /// Largest natural vector alignment of the baseline feature set.
    pub vector: u32,
}

/// The layout, ABI, runtime and IR versions a native artifact was built against.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ArtifactVersions {
    pub lir: u32,
    pub layout: u32,
    pub abi: u32,
    pub runtime_abi: u32,
    pub flat_format: u32,
}

impl ArtifactVersions {
    pub const fn current() -> Self {
        Self {
            lir: crate::LIR_VERSION,
            layout: crate::LAYOUT_VERSION,
            abi: crate::ABI_VERSION,
            runtime_abi: crate::RUNTIME_ABI_VERSION,
            flat_format: crate::FLAT_FORMAT_VERSION,
        }
    }
}

/// One executed target. Emission cross-checks LLVM's data layout
/// against these fields; it is not a second answer.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct TargetSpec {
    pub triple: String,
    pub cpu: String,
    /// LLVM feature spellings; empty is the triple's baseline.
    pub features: Vec<String>,
    pub endian: Endian,
    pub pointer_bytes: u32,
    pub alignments: ScalarAlignments,
    pub c_convention: CConvention,
    pub object_format: ObjectFormat,
    /// The pinned LLVM 21 data layout string expected for this triple.
    pub llvm_data_layout: String,
    pub versions: ArtifactVersions,
}

impl TargetSpec {
    pub fn x86_64_unknown_linux_gnu() -> Self {
        Self {
            triple: "x86_64-unknown-linux-gnu".into(),
            cpu: "x86-64".into(),
            features: Vec::new(),
            endian: Endian::Little,
            pointer_bytes: 8,
            alignments: ScalarAlignments {
                i8: 1,
                i16: 2,
                i32: 4,
                i64: 8,
                i128: 16,
                f32: 4,
                f64: 8,
                pointer: 8,
                vector: 16,
            },
            c_convention: CConvention::SysV64,
            object_format: ObjectFormat::Elf,
            llvm_data_layout:
                "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
                    .into(),
            versions: ArtifactVersions::current(),
        }
    }
    pub fn aarch64_unknown_linux_gnu() -> Self {
        Self {
            triple: "aarch64-unknown-linux-gnu".into(),
            cpu: "generic".into(),
            features: Vec::new(),
            endian: Endian::Little,
            pointer_bytes: 8,
            alignments: ScalarAlignments {
                i8: 1,
                i16: 2,
                i32: 4,
                i64: 8,
                i128: 16,
                f32: 4,
                f64: 8,
                pointer: 8,
                vector: 16,
            },
            c_convention: CConvention::Aapcs64,
            object_format: ObjectFormat::Elf,
            llvm_data_layout:
                "e-m:e-p270:32:32-p271:32:32-p272:64:64-i8:8:32-i16:16:32-i64:64-i128:128-n32:64-S128-Fn32"
                    .into(),
            versions: ArtifactVersions::current(),
        }
    }
    /// The two executed M1b targets.
    pub fn executed() -> [Self; 2] {
        [
            Self::x86_64_unknown_linux_gnu(),
            Self::aarch64_unknown_linux_gnu(),
        ]
    }
    /// Size in bytes of a scalar on this target.
    pub fn scalar_size(&self, scalar: ScalarKind) -> u64 {
        match scalar {
            ScalarKind::Bool | ScalarKind::U8 | ScalarKind::I8 => 1,
            ScalarKind::U16 | ScalarKind::I16 => 2,
            ScalarKind::U32 | ScalarKind::I32 | ScalarKind::F32 => 4,
            ScalarKind::U64 | ScalarKind::I64 | ScalarKind::F64 => 8,
            ScalarKind::U128 | ScalarKind::I128 => 16,
            ScalarKind::Pointer => u64::from(self.pointer_bytes),
        }
    }
    /// ABI alignment in bytes of a scalar on this target.
    pub fn scalar_align(&self, scalar: ScalarKind) -> u32 {
        let a = &self.alignments;
        match scalar {
            ScalarKind::Bool | ScalarKind::U8 | ScalarKind::I8 => a.i8,
            ScalarKind::U16 | ScalarKind::I16 => a.i16,
            ScalarKind::U32 | ScalarKind::I32 => a.i32,
            ScalarKind::U64 | ScalarKind::I64 => a.i64,
            ScalarKind::U128 | ScalarKind::I128 => a.i128,
            ScalarKind::F32 => a.f32,
            ScalarKind::F64 => a.f64,
            ScalarKind::Pointer => a.pointer,
        }
    }
    pub fn digest(&self) -> ContentHash {
        snapshot::digest("target-spec", self)
    }
}

/// Storage domains a layout may be placed in.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum StorageKind {
    Heap,
    LocalRegion,
    Arena,
    Frozen,
    Static,
}

/// Representation family of a layout.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum LayoutKind {
    /// No payload.
    Unit,
    Scalar(ScalarKind),
    /// Tuples, records and distinct types: inline declaration-order fields.
    Aggregate,
    /// Once environment with a trailing u64 live word; bits name movable captures.
    Environment {
        movable: Vec<u32>,
    },
    /// Ordinary variants: explicit u32 ordinal tag plus maximum aligned payload.
    Variant,
    /// `Packed[T]`: active ordinal plus only that variant's payload.
    Packed,
    /// `Inline[T, n]`.
    Inline {
        element: LayoutId,
        count: u64,
    },
    /// `Simd[T, n]`; n is positive.
    Simd {
        element: LayoutId,
        lanes: u32,
    },
    /// A counted block reference (Box, String, Bytes, List, Array).
    Block {
        payload: LayoutId,
    },
    /// A Box reference, with the same representation as Block.
    Box {
        payload: LayoutId,
    },
    /// A `Shared[T]` root reference (MEM-11), with the same representation as
    /// Block: one pointer to a heap root block whose `Frozen` payload holds the
    /// value. The block's header count is the root count.
    Shared {
        payload: LayoutId,
    },
    /// The payload of a `Shared` root block: the frozen value inline at offset
    /// 0. Its drop program thaws the value before releasing it.
    Frozen {
        value: LayoutId,
    },
    /// A slice: backing owner plus start/length, so it never loses its release root.
    Slice {
        element: LayoutId,
    },
    /// An opaque declared layout owned by `owner` (Map, Set, SortedMap, Slots).
    Opaque {
        owner: DefPath,
    },
    /// A logical handle ID into a run-context table; never an address.
    Handle,
    /// Inert interned Path ID: equality/hash by word, order by context text.
    Path,
    /// An arena token: one pointer word holding an open arena's runtime
    /// record address, or null for `heap`.
    Arena,
    /// Callable pair: code symbol plus environment address.
    Callable {
        environment: Option<LayoutId>,
    },
    /// Erased `Iter[T]` environment with next/drop thunks.
    Iterator {
        element: LayoutId,
    },
    /// Inline resource with its finalization state.
    Resource,
    /// The counted block header (runtime ABI v1).
    Header,
    /// A FlatFormatV0 byte extent: pointer-free, alignment 1, no padding.
    /// Fields carry static offsets; a field after variable ones lies further by
    /// each earlier variable field's u64 length prefix value. A variable field's
    /// record has size `8 + prefix` and one field, its content: at offset 8, or
    /// at 16 after a u64 element count for a list of variable elements.
    Flat,
    /// Fixed-record sequence stored as declaration-order columns (#47 assumption).
    SoaColumns {
        element: LayoutId,
    },
    /// The payload of a counted sequence block (List, Array, String and Bytes
    /// storage): a u64 element count at offset 0, then elements at a fixed stride.
    Sequence {
        element: LayoutId,
    },
    /// The payload of a counted sequence block of `Packed[T]` elements (MEM-13,
    /// LIR schema revision 19): a u64 element count at offset 0, a u64 count of
    /// used content bytes at offset 8, then the elements back to back from the
    /// first offset aligned to the element, each at its own tag-selected extent.
    /// Elements are reached only by walking from the first; there is no stride.
    Sequential {
        element: LayoutId,
    },
}

/// One step of a checked size expression, evaluated as a postfix program.
/// All arithmetic is checked; overflow is an invariant error, never a wrap.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum SizeTerm {
    Constant(u64),
    /// Read an unsigned length stored at `offset` in the value itself.
    ReadLength {
        offset: u64,
        scalar: ScalarKind,
    },
    Add,
    Multiply,
    AlignUp(u64),
    /// Duplicate the top of the stack.
    Dup,
    /// Pop an offset and push `program`'s result, its reads relative to that
    /// offset: the sequential cursor of flat and Packed extents.
    At(Vec<SizeTerm>),
    /// Read the u32 tag at `tag_offset` and push the matching variant's program
    /// result; a tag with no entry is an invariant error.
    ByTag {
        tag_offset: u64,
        variants: Vec<(u32, Vec<SizeTerm>)>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum SizeSpec {
    Fixed(u64),
    Dynamic(Vec<SizeTerm>),
}

impl SizeSpec {
    /// The bytes storage of this layout needs for any of its values: the
    /// fixed size, or for a size chosen by tag among constants (`Packed[T]`)
    /// the largest, as an ordinary variant reserves its largest payload.
    /// None for an extent whose size reads its own lengths (variable flat).
    pub fn storage(&self) -> Option<u64> {
        match self {
            SizeSpec::Fixed(size) => Some(*size),
            SizeSpec::Dynamic(program) => match program.as_slice() {
                [SizeTerm::ByTag { variants, .. }] => variants
                    .iter()
                    .map(|(_, p)| match p.as_slice() {
                        [SizeTerm::Constant(size)] => Some(*size),
                        _ => None,
                    })
                    .try_fold(0, |max, size| size.map(|s| max.max(s))),
                _ => None,
            },
        }
    }
}

/// One field at its physical offset. `unaligned` marks `@packed` storage, read
/// with unaligned loads or memcpy.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FieldLayout {
    pub ordinal: u32,
    pub offset: u64,
    pub layout: LayoutId,
    pub unaligned: bool,
}

/// The fields of one active variant.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct VariantLayout {
    pub ordinal: u32,
    pub fields: Vec<FieldLayout>,
}

/// How a variant stores its active ordinal at its tag offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TagForm {
    /// An explicit u32 ordinal.
    Ordinal,
    /// A persistent environment live bit, decoded as zero or one.
    LiveBit { bit: u32 },
    /// No tag word (layout version 2): the offset holds a pointer word inside
    /// variant `dataful`'s payload that is never null while it is active (a
    /// block reference, Ptr, FnPtr or slice owner, nested in inline aggregates
    /// since layout version 5), and a null word there means the nullary
    /// variant `nullary` is active.
    Niche { nullary: u32, dataful: u32 },
}

impl TagForm {
    /// Bytes read at the tag offset: the u32 ordinal, or the pointer word.
    pub fn width(self, pointer_bytes: u32) -> u64 {
        match self {
            Self::Ordinal => 4,
            Self::LiveBit { .. } => 8,
            Self::Niche { .. } => u64::from(pointer_bytes),
        }
    }
    /// The active ordinal of the `width` little-endian bytes at the tag offset.
    pub fn active(self, word: u64) -> u32 {
        match self {
            Self::Ordinal => word as u32,
            Self::LiveBit { bit } => ((word >> bit) & 1) as u32,
            Self::Niche { nullary, .. } if word == 0 => nullary,
            Self::Niche { dataful, .. } => dataful,
        }
    }
}

/// The active ordinal at `offset`, stored as `form`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct TagEncoding {
    pub offset: u64,
    pub form: TagForm,
    pub variants: Vec<VariantLayout>,
}

/// Which bytes of a value are initialized.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum InitializedBytes {
    /// Field bytes only; padding and inactive payload are unspecified.
    Fields,
    /// Padding and inactive payload bytes are zero (exported byte representations).
    ZeroPadding,
}

/// One step of an arbitrary-offset scan program (holders a value contains).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum HandleKind {
    Unknown,
    Atomic,
    Mutex,
    Sender,
    Receiver,
    Task,
    TaskSet,
    Scope,
    Capability,
}

impl HandleKind {
    /// Native holder-scan operand; kept independent of enum memory layout.
    pub const fn code(self) -> u64 {
        match self {
            Self::Unknown => 0,
            Self::Atomic => 1,
            Self::Mutex => 2,
            Self::Sender => 3,
            Self::Receiver => 4,
            Self::Task => 5,
            Self::TaskSet => 6,
            Self::Scope => 7,
            Self::Capability => 8,
        }
    }
}

/// One step of an arbitrary-offset scan program (holders a value contains).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum ScanStep {
    /// A counted block reference.
    Counted { offset: u64 },
    /// A `Shared` root reference (MEM-11): a counted block whose count is the exact root
    /// count, added to only by `hirsel_rt_v2_root_dup`'s contract (runtime ABI 25).
    Root { offset: u64 },
    /// A reference into uncounted storage: lifetime/domain metadata, no counter.
    Uncounted { offset: u64 },
    /// A slice's backing owner.
    SliceOwner { offset: u64 },
    /// An inline child scanned with its own layout.
    Inline { offset: u64, layout: LayoutId },
    /// Scan only the active variant's steps; the ordinal is read as `form`.
    ActiveVariant {
        tag_offset: u64,
        form: TagForm,
        variants: Vec<(u32, Vec<ScanStep>)>,
    },
    /// `count` elements of `element`, `stride` bytes apart.
    Sequence {
        count_offset: u64,
        offset: u64,
        stride: u64,
        element: LayoutId,
    },
    /// A callable's environment reference.
    Environment { offset: u64 },
    /// A run-context handle.
    Handle {
        offset: u64,
        kind: HandleKind,
        /// Identity of the instantiated handle type, preserved through flattening.
        ty: Option<LayoutId>,
        /// Payload/result obtainable by a keep op, including leftover finalizers.
        /// Sender and scalar Atomic handles cannot obtain another handle.
        obtains: Option<LayoutId>,
    },
    /// `count` inline elements (`Inline[T, n]`), `stride` bytes apart.
    Repeat {
        offset: u64,
        count: u64,
        stride: u64,
        element: LayoutId,
    },
    /// The fields of a structure-of-arrays sequence, column by column.
    Columns(SoaColumns),
}

/// One step of the ordered drop program; steps run in listed order.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum DropStep {
    /// Run the user finalizer (borrowed self, shielded) before the fields.
    Finalize,
    /// Release one counted holder.
    Release { offset: u64 },
    /// Release one `Shared` root (MEM-11) through `hirsel_rt_v2_root_release`, never the
    /// inline count edge: the root count is exact and its releases may be deferred.
    ReleaseRoot { offset: u64 },
    /// Drop an inline child with its own layout.
    Child { offset: u64, layout: LayoutId },
    /// Drop the active variant's steps; the ordinal is read as `form`.
    ActiveVariant {
        tag_offset: u64,
        form: TagForm,
        variants: Vec<(u32, Vec<DropStep>)>,
    },
    /// Drop elements from the highest index to the lowest.
    ElementsReverse {
        count_offset: u64,
        offset: u64,
        stride: u64,
        element: LayoutId,
    },
    /// Restore ordinary counts in the frozen value at `offset`, whose layout
    /// is `layout`, before it is released: the first step of a `Frozen`
    /// payload's program. Nested `Shared` roots stay as they are.
    Thaw { offset: u64, layout: LayoutId },
    /// Release a callable's environment.
    Environment { offset: u64 },
    /// Close a run-context handle.
    CloseHandle { offset: u64 },
    /// Drop `count` inline elements from the highest index to the lowest.
    RepeatReverse {
        offset: u64,
        count: u64,
        stride: u64,
        element: LayoutId,
    },
    /// Drop structure-of-arrays elements from the highest index to the lowest,
    /// each element's fields in reverse declaration order.
    ColumnsReverse(SoaColumns),
}

/// Column placement of a structure-of-arrays sequence payload (#47 assumption).
/// Column `c` starts at `data_offset + capacity * c.prefix`, where `capacity`
/// is the u64 element capacity stored at `capacity_offset`, always a multiple
/// of `capacity_quantum`; element `i` of the column is `i * c.stride` further.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SoaColumns {
    pub count_offset: u64,
    pub capacity_offset: u64,
    pub data_offset: u64,
    pub capacity_quantum: u64,
    pub columns: Vec<SoaColumn>,
}

/// One field column: the element field's ordinal, the summed strides of the
/// columns before it, its own stride and its layout.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SoaColumn {
    pub field: u32,
    pub prefix: u64,
    pub stride: u64,
    pub layout: LayoutId,
}

/// How a semantic Copy of this layout is performed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CopyRecipe {
    /// Plain: copy the bits.
    Bits,
    /// Flat: copy the logical byte extent into distinct owned storage.
    FlatBytes,
    /// Counted block reference: add one credit.
    Dup,
    /// Inline aggregate: copy each field with its own recipe.
    Fields,
    /// Noncopyable (resources, once callables, iterators).
    NotCopyable,
}

/// The user finalizer of a resource layout.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FinalizerRef {
    pub definition: DefPath,
    /// The TIR body of the finalizer within its definition.
    pub body: u32,
    /// A failure inside an `unsafe ffi` package's finalizer terminates (FAIL-3).
    pub unsafe_ffi_package: bool,
}

/// Inputs to ABI classification: every scalar leaf at its offset.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AbiInputs {
    pub scalars: Vec<(u64, ScalarKind)>,
    pub unaligned: bool,
}

/// Identity of a layout record: owner representation revision, instance, target
/// and layout version.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct LayoutKey {
    /// `RepresentationRecord.representation_revision` for nominal owners.
    pub owner_revision: Option<ContentHash>,
    /// Digest of the complete `InstanceKey`, for generic instances.
    pub instance: Option<ContentHash>,
    pub target: ContentHash,
    pub layout_version: u32,
}

/// The only physical truth for one representation.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct LayoutRecord {
    pub key: LayoutKey,
    pub kind: LayoutKind,
    pub size: SizeSpec,
    /// Alignment in bytes; a power of two.
    pub align: u32,
    pub fields: Vec<FieldLayout>,
    pub tag: Option<TagEncoding>,
    pub initialized: InitializedBytes,
    pub scan: Vec<ScanStep>,
    pub drop: Vec<DropStep>,
    pub copy: CopyRecipe,
    pub finalizer: Option<FinalizerRef>,
    pub storage: Vec<StorageKind>,
    pub abi: AbiInputs,
}

/// `LayoutId -> LayoutRecord` and `FnAbiId -> FnAbi` for one target and version.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct LayoutTable {
    pub target: ContentHash,
    pub layout_version: u32,
    pub abi_version: u32,
    pub layouts: Vec<LayoutRecord>,
    pub fn_abis: Vec<FnAbi>,
}

impl LayoutTable {
    pub fn new(target: &TargetSpec) -> Self {
        Self {
            target: target.digest(),
            layout_version: crate::LAYOUT_VERSION,
            abi_version: crate::ABI_VERSION,
            layouts: Vec::new(),
            fn_abis: Vec::new(),
        }
    }
    pub fn layout(&self, id: LayoutId) -> Option<&LayoutRecord> {
        self.layouts.get(id.0 as usize)
    }
    /// The layout of the field (of any variant) stored at `offset` in `layout`, at any
    /// inline depth: an aggregate's drop program lifts the steps of an inline field,
    /// such as an `Option[String]` inside a record, to offsets in the whole value.
    pub fn field_at(&self, layout: &LayoutRecord, offset: u64) -> Option<LayoutId> {
        let variants = layout.tag.iter().flat_map(|t| &t.variants);
        let fields = layout.fields.iter().chain(variants.flat_map(|v| &v.fields));
        let mut inside = None;
        for field in fields {
            if field.offset == offset {
                return Some(field.layout);
            }
            if field.offset < offset
                && let Some(child) = self.layout(field.layout)
                && let SizeSpec::Fixed(size) = child.size
                && offset - field.offset < size
            {
                inside = Some((child, offset - field.offset));
            }
        }
        let (child, within) = inside?;
        self.field_at(child, within)
    }
    pub fn digest(&self) -> ContentHash {
        snapshot::digest("layout-table", self)
    }
}

snapshot_enum!(Endian { Little, Big });
snapshot_enum!(ObjectFormat { Elf });
snapshot_enum!(CConvention { SysV64, Aapcs64 });
snapshot_enum!(ScalarKind {
    Bool,
    U8,
    U16,
    U32,
    U64,
    U128,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    Pointer
});
snapshot_struct!(ScalarAlignments {
    i8,
    i16,
    i32,
    i64,
    i128,
    f32,
    f64,
    pointer,
    vector
});
snapshot_struct!(ArtifactVersions {
    lir,
    layout,
    abi,
    runtime_abi,
    flat_format
});
snapshot_struct!(TargetSpec {
    triple,
    cpu,
    features,
    endian,
    pointer_bytes,
    alignments,
    c_convention,
    object_format,
    llvm_data_layout,
    versions
});
snapshot_enum!(StorageKind {
    Heap,
    LocalRegion,
    Arena,
    Frozen,
    Static
});
snapshot_enum!(LayoutKind {
    Unit,
    Scalar(scalar),
    Aggregate,
    Environment { movable },
    Variant,
    Packed,
    Inline { element, count },
    Simd { element, lanes },
    Block { payload },
    Box { payload },
    Shared { payload },
    Frozen { value },
    Slice { element },
    Opaque { owner },
    Handle,
    Path,
    Arena,
    Callable { environment },
    Iterator { element },
    Resource,
    Header,
    Flat,
    SoaColumns { element },
    Sequence { element },
    Sequential { element },
});
snapshot_enum!(SizeTerm {
    Constant(value),
    ReadLength { offset, scalar },
    Add,
    Multiply,
    AlignUp(align),
    Dup,
    At(program),
    ByTag {
        tag_offset,
        variants
    },
});
snapshot_enum!(SizeSpec {
    Fixed(size),
    Dynamic(terms)
});
snapshot_struct!(FieldLayout {
    ordinal,
    offset,
    layout,
    unaligned
});
snapshot_struct!(VariantLayout { ordinal, fields });
snapshot_enum!(TagForm {
    Ordinal,
    LiveBit { bit },
    Niche { nullary, dataful },
});
snapshot_struct!(TagEncoding {
    offset,
    form,
    variants
});
snapshot_enum!(InitializedBytes {
    Fields,
    ZeroPadding
});
snapshot_enum!(HandleKind {
    Unknown,
    Atomic,
    Mutex,
    Sender,
    Receiver,
    Task,
    TaskSet,
    Scope,
    Capability
});
snapshot_enum!(ScanStep {
    Counted { offset },
    Uncounted { offset },
    SliceOwner { offset },
    Inline { offset, layout },
    ActiveVariant {
        tag_offset,
        form,
        variants
    },
    Sequence {
        count_offset,
        offset,
        stride,
        element
    },
    Environment { offset },
    Handle { offset, kind, ty, obtains },
    Repeat {
        offset,
        count,
        stride,
        element
    },
    Columns(columns),
    Root { offset },
});
snapshot_enum!(DropStep {
    Finalize,
    Release { offset },
    Child { offset, layout },
    ActiveVariant {
        tag_offset,
        form,
        variants
    },
    ElementsReverse {
        count_offset,
        offset,
        stride,
        element
    },
    Thaw { offset, layout },
    Environment { offset },
    CloseHandle { offset },
    RepeatReverse {
        offset,
        count,
        stride,
        element
    },
    ColumnsReverse(columns),
    ReleaseRoot { offset },
});
snapshot_struct!(SoaColumns {
    count_offset,
    capacity_offset,
    data_offset,
    capacity_quantum,
    columns
});
snapshot_struct!(SoaColumn {
    field,
    prefix,
    stride,
    layout
});
snapshot_enum!(CopyRecipe {
    Bits,
    FlatBytes,
    Dup,
    Fields,
    NotCopyable
});
snapshot_struct!(FinalizerRef {
    definition,
    body,
    unsafe_ffi_package
});
snapshot_struct!(AbiInputs { scalars, unaligned });
snapshot_struct!(LayoutKey {
    owner_revision,
    instance,
    target,
    layout_version
});
snapshot_struct!(LayoutRecord {
    key,
    kind,
    size,
    align,
    fields,
    tag,
    initialized,
    scan,
    drop,
    copy,
    finalizer,
    storage,
    abi
});
snapshot_struct!(LayoutTable {
    target,
    layout_version,
    abi_version,
    layouts,
    fn_abis
});
impl hirsel_codec::Snapshot for LayoutId {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        w.uint(u128::from(self.0));
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        Ok(Self(<u32 as hirsel_codec::Snapshot>::read(r)?))
    }
}
