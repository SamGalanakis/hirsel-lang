//! The LIR family: function-local arenas, pooled side tables, explicit failure
//! edges and ownership operations.
//!
//! Mutable locations are explicit `SlotId`/`AddressId`; the immutable value a
//! location holds gets a fresh `ValueId` after each write, and reads name that
//! reaching value. Block parameters merge values and initialization masks;
//! addresses never stand in for owned values. Every index is local to its unit
//! (or, for `LayoutId`/`FnAbiId`, to the layout table whose digest the unit
//! header names). Variable-length operands live in typed pools, so an
//! instruction row stays at most 32 bytes on 64-bit hosts.
use crate::abi_types::FnAbiId;
use crate::effect::Effects;
use crate::layout_types::{LayoutId, ScalarKind};
pub use crate::receipt_closure::{ReceiptClosure, ReceiptClosureCapture};
pub use crate::receipt_entry::{ReceiptEntry, ReceiptEntrySlot};
pub use crate::receipt_transport::{ReceiptArgument, ReceiptCall};
use crate::stage::{Stage, StageKind};
use hirsel_codec::{snapshot_enum, snapshot_struct};
use hirsel_core_contract::OperationId;
use hirsel_core_contract::Passing;
use hirsel_syntax::{ContentHash, DefPath};
use std::marker::PhantomData;

/// A compact optional SSA operand. The reserved maximum index keeps instruction
/// rows within 32 bytes; every present index still passes ordinary reference checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct OptionalValueId(u32);
impl OptionalValueId {
    pub const NONE: Self = Self(u32::MAX);
    pub const fn get(self) -> Option<ValueId> {
        if self.0 == u32::MAX {
            None
        } else {
            Some(ValueId(self.0))
        }
    }
}
impl From<Option<ValueId>> for OptionalValueId {
    fn from(value: Option<ValueId>) -> Self {
        Self(value.map_or(u32::MAX, |value| {
            assert_ne!(value.0, u32::MAX, "SSA value index exhausted");
            value.0
        }))
    }
}
impl hirsel_codec::Snapshot for OptionalValueId {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        w.uint(u128::from(self.0));
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        Ok(Self(<u32 as hirsel_codec::Snapshot>::read(r)?))
    }
}

/// Native persistent environment and SSA initialization masks share one u64 word.
pub const LIVE_MASK_WORD_BITS: u32 = u64::BITS;

/// A native implementation limit, reported as unavailable/unsupported by consumers.
pub fn live_mask_word_limit(subject: &str, bits: usize) -> String {
    format!(
        "{subject} requires {bits} bits; native live-mask word implementation limit is {LIVE_MASK_WORD_BITS} bits"
    )
}

macro_rules! ids { ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
    $(#[$doc])*
    #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
    pub struct $name(pub u32);
    impl $name {
        pub const fn index(self) -> usize {
            self.0 as usize
        }
    }
    impl hirsel_codec::Snapshot for $name {
        fn write(&self, w: &mut hirsel_codec::Writer) {
            w.uint(u128::from(self.0));
        }
        fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
            Ok(Self(<u32 as hirsel_codec::Snapshot>::read(r)?))
        }
    }
)*}; }
ids!(
    /// An immutable SSA value.
    ValueId,
    /// A mutable storage slot (TIR local storage, loop-carried or displaced slots).
    SlotId,
    /// A computed address: base plus projection path.
    AddressId,
    BlockId,
    LirTypeId,
    CallId,
    ConstructId,
    ClosureId,
    ConstantId,
    SymbolId,
    /// A registry operation named in `SideTables::operations`.
    OperationRef,
    LoanId,
    FailureId,
    EdgeId,
    MaskId,
    SourceAnchorId,
    CleanupAnchorId,
    RegionInstanceId,
    ArenaInstanceId,
    SharedInstanceId,
    /// A task heap domain; M1b is single-task, so only the executing task exists.
    TaskDomainId,
    /// A lexical scope (TIR `ScopeId`); `LirUnit::scopes` records its parent.
    ScopeId,
    /// A SER-2 codec plan in `SideTables::codecs` (LIR schema revision 29).
    CodecId,
    /// A typed sealed-storage descriptor in SideTables::seal_descriptors.
    SealDescriptorId,
    /// Flattened original binding ordinal in SideTables::seal_payloads.
    PayloadBindingId,
);

/// A typed range in one of the unit's pools.
pub struct PoolRange<T> {
    pub start: u32,
    pub len: u32,
    _element: PhantomData<fn() -> T>,
}

impl<T> PoolRange<T> {
    pub const EMPTY: Self = Self::new(0, 0);
    pub const fn new(start: u32, len: u32) -> Self {
        Self {
            start,
            len,
            _element: PhantomData,
        }
    }
    /// The range's elements, or None if it reaches outside `pool` (a foreign range).
    pub fn get<'a>(&self, pool: &'a [T]) -> Option<&'a [T]> {
        let start = self.start as usize;
        pool.get(start..start.checked_add(self.len as usize)?)
    }
    /// Append `items` to `pool` and return their range.
    pub fn push(pool: &mut Vec<T>, items: impl IntoIterator<Item = T>) -> Self {
        let start = pool.len() as u32;
        pool.extend(items);
        Self::new(start, pool.len() as u32 - start)
    }
}
impl<T> Clone for PoolRange<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for PoolRange<T> {}
impl<T> PartialEq for PoolRange<T> {
    fn eq(&self, other: &Self) -> bool {
        (self.start, self.len) == (other.start, other.len)
    }
}
impl<T> Eq for PoolRange<T> {}
impl<T> std::hash::Hash for PoolRange<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.start, self.len).hash(state);
    }
}
impl<T> std::fmt::Debug for PoolRange<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..+{}", self.start, self.len)
    }
}
impl<T> hirsel_codec::Snapshot for PoolRange<T> {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        w.open("range");
        w.uint(u128::from(self.start));
        w.uint(u128::from(self.len));
        w.close();
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        r.open("range")?;
        let range = Self::new(u32::read(r)?, u32::read(r)?);
        r.close()?;
        Ok(range)
    }
}

/// How a value representation behaves for ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ValueRepr {
    Unit,
    Scalar(ScalarKind),
    /// A counted heap block reference: one holder credit.
    Block,
    /// An uncounted block reference, or inline aggregate of regional references.
    Uncounted,
    /// An inline aggregate value.
    Aggregate,
    /// Code symbol plus environment address.
    Callable,
    /// A run-context handle ID.
    Handle,
    /// A raw reuse token or explicit None (`TokenOption`).
    Token,
    /// A compiler-local region mark.
    RegionMark,
    /// An arena token: an open arena's record address, or null for `heap`.
    Arena,
    /// A dynamic initialized-field mask over `fields` fields.
    InitMask {
        fields: u32,
    },
    /// Private immutable metadata handle, never a language Shared root.
    PayloadReceipt,
    /// A constructor-tail destination (LANG-10, LIR schema revision 59): the
    /// open payload of the unique box block of its layout that a `HoleOpen`
    /// allocated. Linear: one `HoleFill` or `HoleAbandon` consumes it.
    Hole,
}

/// A LIR value type: representation plus its physical layout.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct LirType {
    pub repr: ValueRepr,
    pub layout: LayoutId,
}

/// Whether a value carries an owning credit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Holding {
    /// Bits with no ownership (Plain).
    Plain,
    /// Owns one holder credit.
    Owned,
    /// A scoped nonowning handle, valid while its loan is active.
    Borrowed(LoanId),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ValueInfo {
    pub ty: LirTypeId,
    pub holding: Holding,
}

/// The source storage class of a slot (TIR `Storage`, plus native slots).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SlotKind {
    Parameter(u32),
    Local,
    Temporary,
    Capture(u32),
    ReturnSlot,
    /// An internal owned loop slot for a normalized self-tail argument.
    LoopCarried(u32),
    /// The fresh slot receiving displaced content of a Replace.
    Displaced,
    ReceiptParameter(u32),
    ReceiptCapture(u32),
    ReceiptResult,
}

impl SlotKind {
    /// Whether the slot releases as a statement temporary in cleanup order
    /// (before its scope's names): TIR `Temporary` storage, which includes a
    /// Replace's displaced owner.
    pub fn is_temporary(self) -> bool {
        matches!(self, SlotKind::Temporary | SlotKind::Displaced)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SlotInfo {
    pub ty: LirTypeId,
    pub kind: SlotKind,
    /// The TIR scope that owns the slot's lifetime.
    pub scope: ScopeId,
}

/// One lexical scope: the scope it is nested in (None for a body's outermost
/// scopes). Cleanup order reads nesting from this relation, never from ordinals.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ScopeInfo {
    pub parent: Option<ScopeId>,
}

/// One projection step; indices are already-evaluated values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ProjectionStep<I = ValueId> {
    /// Dereference exactly one Box; no byte offset (LIR revision 16).
    Payload,
    Field(u32),
    Index(I),
    VariantField {
        variant: u32,
        field: u32,
    },
    /// Element slot `i` of the sequence block at this place, `i < capacity`,
    /// with no count check: the slot may be uninitialised (a `Store` then
    /// initialises it) or hold an element past the count (a `Read Take` then
    /// empties it). Library units only (LIR schema revision 7).
    Slot(I),
    /// Element `index` of a variable-element flat sequence whose encoding
    /// starts `offset` bytes into the sequence's content. Checked like
    /// `Index`: `index < count` or MEM-14. `offset` must be the value
    /// `FlatNext` produced for element `index - 1` of the same reaching
    /// version, or zero for element 0 (LIR schema revision 7).
    FlatAt {
        index: I,
        offset: I,
    },
}

impl<I> ProjectionStep<I> {
    /// Resolve index operands without changing the projection vocabulary.
    pub fn try_map_index<J, E>(
        self,
        mut eval: impl FnMut(I) -> Result<J, E>,
    ) -> Result<ProjectionStep<J>, E> {
        Ok(match self {
            Self::Payload => ProjectionStep::Payload,
            Self::Field(field) => ProjectionStep::Field(field),
            Self::VariantField { variant, field } => {
                ProjectionStep::VariantField { variant, field }
            }
            Self::Index(index) => ProjectionStep::Index(eval(index)?),
            Self::Slot(index) => ProjectionStep::Slot(eval(index)?),
            Self::FlatAt { index, offset } => ProjectionStep::FlatAt {
                index: eval(index)?,
                offset: eval(offset)?,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum AddressBase {
    Slot(SlotId),
    /// The payload of a block reference value, or a read-only frame spill of
    /// a fixed-size copyable inline value. Preparing the latter snapshots its bits without
    /// transferring ownership; reads retain the source value as their backing.
    Value(ValueId),
    Address(AddressId),
}

/// A field-path address. Its checks (bounds, tag) run at `Inst::Address`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AddressInfo {
    /// PS-2 canonical TIR PlaceId, present on source projection preparations.
    pub source_place: Option<u32>,
    /// Projection ordinals whose bounds/tag guards accepted PS-13 evidence
    /// removes. The path and all storage/ownership checks stay unchanged.
    pub unchecked: Vec<u32>,
    pub base: AddressBase,
    pub projection: PoolRange<ProjectionStep>,
    pub ty: LirTypeId,
}

/// Scalar unary operations; ownership requests never reach LIR as operators.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}

/// Scalar binary operations on typed values. Short-circuit `and`/`or` lower to
/// branches. Integer forms are checked; float forms are exact IEEE operations.
///
/// The named library operations (LIR schema revision 6) follow the operators
/// on the same instruction: `DivFloor`, `Mod` and `DivExact` are integer
/// operations failing SEM-1 on a zero divisor and on signed `MIN / -1`, and
/// `DivExact` also on a nonzero remainder (docs/spec/core-library.md "Numeric and string
/// details"). `Min`, `Max`, `MinNumber` and `MaxNumber` are IEEE 754-2019
/// `minimum`, `maximum`, `minimumNumber` and `maximumNumber` on floats
/// (SEM-2: NaN propagates or is ignored, -0 < +0); none of the float ones
/// fails. (Revision 41 removed `TotalOrder`: totalOrder is std source.)
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    DivFloor,
    Mod,
    DivExact,
    Min,
    Max,
    MinNumber,
    MaxNumber,
}

/// What `Inst::Convert` computes (LIR schema revision 6; SEM-1 conversions,
/// docs/spec/core-library.md "Numeric and string details", and `float.to_i32`). An integer mode's
/// result is the operand modulo 2^width of the result kind, read with the
/// result's signedness (truncation, or extension by the operand's
/// signedness), so it is the operand itself whenever it is representable.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ConvertMode {
    /// `int.widen`: the operand kind's complete range fits the result kind.
    Widen,
    /// `int.truncate`: the result kind is no wider than the operand's.
    Truncate,
    /// `int.try_narrow` (and, followed by a `Check`, `int.narrow`): `flag`
    /// is a Bool, true exactly when the operand is representable.
    TryNarrow,
    /// `float.to_i32` of an f32 or f64: `flag` is an I32 status (the
    /// result's own kind, so no other layout is needed), 0 when the operand
    /// truncated toward zero fits i32 (the result), 1 for NaN or an infinity
    /// (`NotFinite`) and 2 otherwise (`OutOfRange`). The result is the
    /// truncation saturated to i32's bounds, NaN giving 0.
    FloatToI32,
    /// `float.from_int`: any sized integer into f32 or f64, rounded to
    /// nearest, ties to even. No flag and no error case.
    IntToFloat,
    /// `f32.bits`/`f64.bits` and `f32.from_bits`/`f64.from_bits`: the same
    /// bits read as the other kind of one pair, f32 with u32 or f64 with u64,
    /// in either direction (docs/spec/core-library.md "IEEE bit primitives"). A NaN, as operand or
    /// as result, reads as its float kind's one observable NaN, the quiet NaN
    /// with a zero sign and payload (SEM-2). No flag and no error case.
    Reinterpret,
    /// `f64.from_f32` and `f32.from_f64`: an f32 into an f64 exactly, or an
    /// f64 into an f32 rounded to nearest, ties to even (SEM-2). A NaN reads
    /// as the result kind's one observable NaN. No flag and no error case.
    FloatResize,
}

/// How `Inst::IntArith` treats a mathematical result outside its kind
/// (SEM-1's named behaviours, LIR schema revision 6).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ArithMode {
    /// Integer Add/Sub/Mul modulo 2^width; Neg is Sub from zero.
    /// Div/Rem require a nonzero divisor, signed Div excludes MIN / -1,
    /// and shifts require a count in 0..width. These forms are exact when
    /// their SEM-1 obligation holds; signed MIN % -1 remains zero.
    Wrapping,
    /// `int.saturating_add`: the nearest bound of the kind.
    Saturating,
    /// `int.checked_add`: the wrapped result, and `flag` (a Bool) true
    /// exactly when it overflowed.
    Checked,
}

/// What `Inst::ScalarUnary` computes (LIR schema revision 41,
/// docs/spec/core-library.md "Numeric and string details"). Every op is
/// exactly defined, so each engine gives the same bits.
///
/// The float ops take and give one float kind: `Sqrt` is IEEE 754
/// squareRoot, correctly rounded; `Floor`, `Ceil`, `Trunc` and `Round` are
/// roundToIntegralTowardNegative, TowardPositive, TowardZero and TiesToAway;
/// `Abs` clears the sign. Signed zeros and infinities pass through the
/// rounding ops, and a NaN result reads as the kind's one observable NaN
/// (SEM-2).
///
/// The integer ops take and give one sized integer kind and read its two's
/// complement bits: `LeadingZeros`, `TrailingZeros` and `CountOnes` count
/// bits (the width for zero), `SwapBytes` reverses the byte order and
/// `ReverseBits` the bit order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ScalarUnaryOp {
    Sqrt,
    Floor,
    Ceil,
    Trunc,
    Round,
    Abs,
    LeadingZeros,
    TrailingZeros,
    CountOnes,
    SwapBytes,
    ReverseBits,
}

impl ScalarUnaryOp {
    /// Whether the op is one of the float ops; the rest are integer ops.
    pub const fn is_float(self) -> bool {
        matches!(
            self,
            Self::Sqrt | Self::Floor | Self::Ceil | Self::Trunc | Self::Round | Self::Abs
        )
    }
}

/// Complete logical operand depth admitted by MEM-3, with the root at zero.
/// This is independent of physical layout and native/scratch implementation choices.
pub const COMPARE_MAX_DEPTH: u64 = hirsel_core_contract::depth::STRUCTURAL_LIMIT;

/// Structural comparison operators (`Inst::Compare`): equality and the total
/// structural order over one non-scalar layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// How a read uses the storage it names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ReadMode {
    /// A scoped nonowning handle.
    Borrow,
    /// Transfer the holder credit; the storage becomes uninitialized.
    Take,
    /// Plain bits.
    Bits,
}

/// Origin bookkeeping kinds (TIR `OriginBindingKind`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum OriginKind {
    Alias,
    Fresh,
}

/// A release or reset liveness mask.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum MaskRef {
    /// Every field initialized.
    All,
    Static(MaskId),
    Dynamic(ValueId),
}

/// M2-only operation families: classified conservatively, never emitted as no-ops,
/// rejected by the M1b Ready support check.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum UnavailableFeature {
    Shared,
    Arena,
    Task,
    Channel,
    Mutex,
    Atomic,
    /// TIR `ReleaseRegion`: user regions are M2; compiler-local regions are LIR-only.
    ReleaseRegion,
    CloseArena,
    JoinTasks,
    /// A `TypeShape` whose static data is not materialized.
    Shape,
}

/// One instruction row. Every variant is at most seven 32-bit words so the row is
/// at most 32 bytes including its tag; variable operands live in pools.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Inst {
    // Pure values.
    Const {
        result: ValueId,
        constant: ConstantId,
    },
    /// A definition as a callable value, or, at a pointer scalar result, the
    /// address of its C callback thunk (PROG-4).
    FunctionRef {
        result: ValueId,
        symbol: SymbolId,
    },
    /// Program-lifetime static data: a constant or materialized shape.
    ReadStatic {
        result: ValueId,
        symbol: SymbolId,
    },
    Unary {
        result: ValueId,
        op: UnaryOp,
        operand: ValueId,
        failure: FailureId,
    },
    Binary {
        result: ValueId,
        op: BinaryOp,
        left: ValueId,
        right: ValueId,
        failure: FailureId,
    },
    /// Structural `op` over two read-only operands of the non-scalar `layout`
    /// (scalars stay on `Binary`). The order is the interpreter's
    /// (`hirsel_interp::structural::compare`): fields in declaration order,
    /// variant tag ordinals before payloads, sequences lexicographically with
    /// the shorter prefix first, String and Bytes bytewise, floats by IEEE
    /// (an unordered pair makes every operator but `Ne` false). It fails with
    /// MEM-3 Depth past `COMPARE_MAX_DEPTH` nesting (spec #16).
    Compare {
        result: ValueId,
        op: CompareOp,
        left: ValueId,
        right: ValueId,
        layout: LayoutId,
        failure: FailureId,
    },
    /// A scalar conversion (`ConvertMode`, LIR schema revision 6). `flag`
    /// is present exactly for `TryNarrow` and `FloatToI32`. Never fails.
    Convert {
        result: ValueId,
        flag: Option<ValueId>,
        operand: ValueId,
        mode: ConvertMode,
    },
    /// `op` of two integers of one kind with a named overflow behaviour
    /// (`ArithMode`, LIR schema revision 6). The library names `Add`, `Sub`
    /// and `Mul` (revision 41), the `op`s admitted; `Mul` has no `Saturating`
    /// form. `flag` is present exactly in `Checked` mode. Never fails.
    IntArith {
        result: ValueId,
        flag: Option<ValueId>,
        op: BinaryOp,
        mode: ArithMode,
        left: ValueId,
        right: ValueId,
    },
    /// `float.mul_add`: `left * right + addend` of one float kind, rounded
    /// once (SEM-2, LIR schema revision 6). Never fails.
    MulAdd {
        result: ValueId,
        left: ValueId,
        right: ValueId,
        addend: ValueId,
    },
    /// `op` (`ScalarUnaryOp`, LIR schema revision 41) of one scalar of the
    /// result's kind. Never fails.
    ScalarUnary {
        result: ValueId,
        op: ScalarUnaryOp,
        operand: ValueId,
    },
    /// A source check; the failure edge carries its rule.
    Check {
        condition: ValueId,
        failure: FailureId,
    },
    // Storage and addresses.
    StorageLive {
        slot: SlotId,
    },
    StorageDead {
        slot: SlotId,
        if_live: bool,
    },
    EndScope {
        scope: ScopeId,
    },
    /// Prepare an address: bounds and tag checks precede any use.
    Address {
        address: AddressId,
        failure: FailureId,
    },
    Read {
        result: ValueId,
        address: AddressId,
        reaching: ValueId,
        mode: ReadMode,
    },
    /// Shallow copy of a borrowed SSA block handle. RC turns this into a
    /// Dup and a credit transfer; the reference itself has no address.
    CopyValue {
        result: ValueId,
        value: ValueId,
    },
    /// Semantic Copy before RC; Ready replaces it by bits, flat copy or Dup.
    Copy {
        result: ValueId,
        address: AddressId,
        reaching: ValueId,
        failure: FailureId,
    },
    /// Copy the flat encoding at `address`, typed by `layout`, into distinct
    /// owned storage. The result has that layout, or its ordinary source
    /// layout: a variable flat record block without the encoding's prefix,
    /// or a String/Bytes slice over a fresh counted byte sequence.
    FlatCopy {
        result: ValueId,
        address: AddressId,
        layout: LayoutId,
        failure: FailureId,
    },
    /// A borrowed view, at its source type, of the String or Bytes encoding
    /// at `address` inside a flat value held in a block: a `Slice` whose
    /// owner is that block, whose start is the encoding's byte offset in the
    /// block's payload and whose length is the encoding's u64 prefix, so its
    /// bytes lie at `payload + 8 + start`. Read-only; it cannot fail.
    FlatView {
        result: ValueId,
        address: AddressId,
    },
    /// A checked byte subinterval under the source's existing loan. Checks
    /// start <= end <= value.length before publishing any view; no credit
    /// is acquired. The result has the same Slice layout and borrowed loan.
    SliceView {
        result: ValueId,
        value: ValueId,
        start: ValueId,
        end: ValueId,
        failure: FailureId,
    },
    /// The element count of the flat sequence encoding (List, Array, String,
    /// Bytes) at `address`: a Plain u64. A list of variable elements stores
    /// its count after the byte-length prefix; any other sequence divides its
    /// byte length by the element size. Read-only; it cannot fail.
    FlatLength {
        result: ValueId,
        address: AddressId,
    },
    /// The content offset just past the flat element at `address`: a Plain
    /// u64. Read-only; it cannot fail (LIR schema revision 7).
    FlatNext {
        result: ValueId,
        address: AddressId,
    },
    /// The element count of a counted sequence block (List, Array, String or
    /// Bytes storage), or the length of a Slice: a Plain u64. Read-only; it
    /// cannot fail (LIR schema revision 7).
    SeqLen {
        result: ValueId,
        value: ValueId,
    },
    /// How many elements the block's storage holds without growing: a Plain
    /// u64, at least its count. Zero-sized elements report u64::MAX.
    /// Read-only; it cannot fail (LIR schema revision 7).
    SeqCapacity {
        result: ValueId,
        value: ValueId,
    },
    /// Make the sequence block at `address` hold at least `capacity`
    /// elements, keeping its count and elements (moved bitwise; the old
    /// storage is freed shallowly, never scanned). A block that is not
    /// unique (shared, or static as an empty list literal's) is detached
    /// instead, once, at the requested capacity: its count and elements are
    /// copied, each counted element gains a holder, and the place's credit
    /// of the original is given back. Either way the place then holds a
    /// unique block, so a place whose only write is a reserve needs no
    /// detach first.
    /// A write to the place: `version` is the root's new reaching value.
    /// Allocates; it cannot fail (an impossible size is an invariant fault,
    /// host exhaustion is environmental). Library units only (LIR schema
    /// revision 7).
    SeqReserve {
        address: AddressId,
        capacity: ValueId,
        version: ValueId,
    },
    /// Prepare one push to the sequence block at `address` (MEM-6, LIR
    /// schema revision 32). A tracked arena List is charged its element's
    /// logical size against its bounded arena; then, when shared, it is
    /// copied within its arena as a new unshared List, charged its count;
    /// then, when full, it relocates within its arena to `max(4, 2 * count)`
    /// elements, uncharged. Afterwards it is unshared with room for one more
    /// element. A heap, static or untracked block is left alone. A write:
    /// `version` is the root's new reaching value. Fails with the arena's
    /// `OutOfSpace`. Library units only.
    ///
    /// With `count` (a u64, LIR schema revision 34) it prepares that many
    /// appended elements at once, before an Io read appends up to `count`
    /// bytes: the List is charged `count` elements, copied when shared,
    /// then relocated to `max(4, 2 * len, len + count)` elements when they
    /// do not fit, so the read itself never charges or relocates. Any unit.
    ArenaPush {
        address: AddressId,
        count: Option<ValueId>,
        version: ValueId,
        failure: FailureId,
    },
    /// Set the count of the sequence block at `address` to `len`,
    /// `len <= capacity`. It moves no element and runs no drop: the unit
    /// must already have initialised every slot below `len` and emptied
    /// every slot at or above it. The block must be unique. A write.
    /// Library units only (LIR schema revision 7).
    SeqSetLen {
        address: AddressId,
        len: ValueId,
        version: ValueId,
    },
    /// Append the owned Packed `value` at the byte end of the sequential
    /// sequence block at `address` (MEM-13): its active extent is written past
    /// the used bytes, then the count and the used bytes grow. Storage grows
    /// as needed, moving the bytes (the elements hold no reference). The block
    /// must be unique. A write: `version` is the root's new reaching value.
    /// Allocates; it cannot fail (an impossible size is an invariant fault,
    /// host exhaustion is environmental). Library units only (LIR schema
    /// revision 19).
    SeqAppend {
        address: AddressId,
        value: ValueId,
        version: ValueId,
    },
    /// Append one element at the byte end of the sequential sequence block at
    /// `address`, built in its flat encoding (LIR schema revision 22): the
    /// block's element layout is a variable flat record's prefixed encoding
    /// (its u64 content length, then the content at offset 8), and
    /// `construct` names a flat construction of exactly that content. Its
    /// fixed fields are consumed and its encoded fields only read, as for a
    /// flat `Construct`, but no value is built: the prefix and content are
    /// written past the used bytes, then the count and the used bytes grow.
    /// The site's destination and placement name no storage. Storage grows
    /// as `SeqAppend`'s does. The block must be unique. A write: `version`
    /// is the root's new reaching value. Allocates; it cannot fail (an
    /// impossible size is an invariant fault, host exhaustion is
    /// environmental).
    FlatAppend {
        address: AddressId,
        construct: ConstructId,
        version: ValueId,
    },
    /// A write of the open element of the flat list builder at `address`
    /// (LIR schema revision 24): one element under construction past the
    /// used bytes whose byte field `field` is built in place, so a byte list
    /// frozen into it is never a separate block. `field` is a String or
    /// Bytes field of the builder's content record after only fixed fields,
    /// so it lies at a fixed offset. The element's prefix, the
    /// fields before that field and the field's length word lie where the
    /// element will, and the field's bytes follow the word; the count and
    /// the used bytes exclude them until `FlatClose` completes the element.
    /// Storage grows keeping them. The block must be unique. A write:
    /// `version` is the root's new reaching value. Allocates; it cannot fail.
    FlatField {
        address: AddressId,
        field: u32,
        op: FlatFieldOp,
        version: ValueId,
    },
    /// Complete the open element of the builder at `address` (LIR schema
    /// revision 24): `construct` is a flat construction of the builder's
    /// content record with a value for every field but `field`, the open
    /// field, whose bytes are already in place. Its fixed fields are
    /// consumed and its encoded fields only read, as for `FlatAppend`; the
    /// content's length prefix and the other fields are written around the
    /// open field, then the count and the used bytes grow by the element.
    /// The site's destination and placement name no storage. Storage grows
    /// keeping the open element. The block must be unique. A write.
    /// Allocates; it cannot fail.
    FlatClose {
        address: AddressId,
        construct: ConstructId,
        field: u32,
        version: ValueId,
    },
    /// A hash of `value` at the non-scalar or scalar `layout`, keyed by the
    /// run context's seed: a Plain u64. Equal values (by `Compare Eq`) hash
    /// equal. A value nested past `COMPARE_MAX_DEPTH` (counted as `Compare`
    /// counts: the root at depth 0, each field, element or Box payload one
    /// deeper) is the MEM-3 Depth failure, so every keyed operation fails
    /// at its call whatever the container holds (spec #90). Its bits are
    /// unobservable (SEM-5: the seed affects placement only) and need not
    /// agree across engines (LIR schema revision 7).
    Hash {
        result: ValueId,
        value: ValueId,
        layout: LayoutId,
        failure: FailureId,
    },
    /// `codec.encode` (LIR schema revision 29): the SER-2 envelope of the
    /// borrowed `value`, at its plan's root layout, as a fresh owned `Bytes`
    /// at its plan's bytes layout. The value is read whole and never written;
    /// recursion is through data, not the native stack. It cannot fail.
    Encode {
        result: ValueId,
        value: ValueId,
        codec: CodecId,
    },
    /// `codec.decode` (LIR schema revision 29): the borrowed `Bytes` `value`
    /// validated completely against the plan's descriptor and schema, then
    /// either `Ok` of a fresh owned value of the plan's root layout or `Error`
    /// of the first `DecodeError` in reading order, at the plan's result
    /// layout. Nothing partially decoded exists; it cannot fail.
    Decode {
        result: ValueId,
        value: ValueId,
        codec: CodecId,
    },
    /// The same borrowed block reference at another payload layout. Exactly
    /// one of the operand's and the result's layouts is the cursor's opaque
    /// source (`LayoutKind::Block` over `Unit`), preserving its loan; or a move
    /// of an Owned block to another block layout with the exact same payload.
    /// It cannot fail. The latter transfers the credit without copying bytes.
    /// Library units only (LIR schema revision 7).
    Recast {
        result: ValueId,
        value: ValueId,
    },
    /// A copy of a counted block into the construct's destination, holding
    /// one credit. Deep (`unique x`, `copy_to`): every headered block the value
    /// reaches is copied once, sharing inside the graph preserved. Shallow
    /// (a copy-on-write detach, OWN-4): only the block itself, each counted
    /// child shared with one more holder.
    UniqueCopy {
        result: ValueId,
        value: ValueId,
        construct: ConstructId,
        deep: bool,
        failure: FailureId,
    },
    /// `shared.freeze` (MEM-11, LIR schema revision 27, library units only):
    /// move the owned `value` into a fresh heap root block of the `Shared`
    /// layout of `result`, whose `Frozen` payload is the value's layout, with
    /// a root count of one, then make every counted block the value reaches
    /// immortal, stopping at nested `Shared` roots and static blocks. It
    /// cannot fail.
    Freeze {
        result: ValueId,
        value: ValueId,
    },
    /// `shared.thaw` of the last root (library units only): consume the owned
    /// `Shared` `value`, whose root count must be one, restore ordinary unique
    /// counts in its frozen value, move that value out as `result` and free
    /// the root block. It cannot fail.
    Thaw {
        result: ValueId,
        value: ValueId,
    },
    /// Initialize dead storage; `version` is the root's new reaching value.
    Store {
        address: AddressId,
        value: ValueId,
        version: ValueId,
    },
    /// Atomic exchange into live storage; old content moves to `displaced`.
    /// `version` is the root's new reaching value and `displaced_version`
    /// the displaced slot's, typed at that slot's type (LIR schema revision
    /// 5).
    Replace {
        address: AddressId,
        value: ValueId,
        displaced: SlotId,
        version: ValueId,
        displaced_version: ValueId,
        if_live: bool,
        failure: FailureId,
    },
    /// Set or clear one field of a dynamic initialization mask.
    UpdateMask {
        result: ValueId,
        mask: ValueId,
        field: u32,
        initialized: bool,
    },
    // Construction.
    Construct {
        result: ValueId,
        construct: ConstructId,
        failure: FailureId,
    },
    Closure {
        result: ValueId,
        closure: ClosureId,
        failure: FailureId,
    },
    /// Consume a many-call callable as the once-call callable of the same
    /// environment: the same bits, a copy recipe that no longer copies.
    Weaken {
        result: ValueId,
        value: ValueId,
    },
    // Loans and origin bookkeeping: no machine bytes, lifetime barriers.
    BeginLoan {
        loan: LoanId,
        address: AddressId,
        mutable: bool,
        if_inactive: bool,
    },
    EndLoan {
        loan: LoanId,
        if_active: bool,
    },
    BindOrigin {
        source: AddressId,
        place: AddressId,
        kind: OriginKind,
    },
    BindFields {
        source: AddressId,
        fields: PoolRange<AddressId>,
    },
    EndOrigin {
        place: AddressId,
    },
    // Ownership.
    /// Semantic Drop at its TIR cleanup anchor; Ready lowers it.
    Drop {
        address: AddressId,
        if_live: bool,
        anchor: CleanupAnchorId,
        failure: FailureId,
    },
    /// Add one finite credit or saturate; never allocates.
    Dup {
        value: ValueId,
    },
    /// Whether a borrowed counted block's count is exactly one (an immortal
    /// count never is): a Plain bool. Read-only; it cannot fail.
    IsUnique {
        result: ValueId,
        value: ValueId,
    },
    /// Whether a write may change the borrowed block in place: its count is
    /// exactly one, or it is an unshared arena List or Array (MEM-6,
    /// `runtime_abi::COUNT_ARENA_UNIQUE`): a Plain bool. Copy-on-write tests
    /// it; unlike `IsUnique` it never licenses freeing or reusing the block.
    /// Read-only; it cannot fail (LIR schema revision 32).
    IsWritable {
        result: ValueId,
        value: ValueId,
    },
    /// A semantic copy of `value` (MEM-6, LIR schema revision 32): every
    /// unshared arena List or Array block it holds at inline depth (the block
    /// itself for a block reference, else each holder slot of its layout's
    /// drop program) becomes shared, so its next write copies it. Reads
    /// `value`, which keeps its credits; never allocates or fails.
    Share {
        value: ValueId,
    },
    /// Consume one credit; may run finalizers at `anchor`.
    Release {
        value: ValueId,
        layout: LayoutId,
        live: MaskRef,
        anchor: CleanupAnchorId,
        failure: FailureId,
    },
    /// Consume the donor; yields a token if unique, else releases and yields None.
    Reset {
        result: ValueId,
        value: ValueId,
        layout: LayoutId,
        live: MaskRef,
        anchor: CleanupAnchorId,
        failure: FailureId,
    },
    /// Consume exactly one token (None means allocate) and install a construction.
    /// `kept` (LIR schema revision 5) names the construction's fields whose
    /// store is skipped when the token holds storage: the token's raw storage
    /// already holds exactly that operand at that physical slot, because the
    /// operand was taken from the same field of the donor that the token's
    /// `Reset` consumed, with the same layout and active variant, and the
    /// reset's live mask excluded it. With a None token every field is stored.
    Reuse {
        result: ValueId,
        token: ValueId,
        construct: ConstructId,
        kept: Option<MaskId>,
        failure: FailureId,
    },
    /// Shallow-free raw storage; never scans stale payloads or finalizes.
    Dispose {
        token: ValueId,
    },
    /// An explicit None token for a join edge.
    TokenNone {
        result: ValueId,
    },
    /// Constructor-tail destination passing (LANG-10, LIR schema revision
    /// 59). Allocate the unique block of `construct`, a `Box` construction
    /// with no field, whose payload stays open: `result` is the ordinary owned
    /// block reference at the construction's layout and `hole` the linear
    /// destination of its payload. Until its hole is filled the block carries
    /// `open`, the box's stand-in layout over plain bytes of the payload's
    /// size and alignment, so releasing it frees only its shell. Nothing may
    /// read, borrow, compare, hash, publish, share, send, capture or return the
    /// block, or a value built from it, while any hole is live.
    HoleOpen {
        result: ValueId,
        hole: ValueId,
        construct: ConstructId,
        open: LayoutId,
        failure: FailureId,
    },
    /// Consume `hole` and the owned `value` of its payload layout: the value
    /// moves into the open payload and the block takes its box layout. It
    /// cannot fail.
    HoleFill {
        hole: ValueId,
        value: ValueId,
    },
    /// On a failure path, consume `hole` and, when present, release `root`,
    /// the instance's outermost box, by the layouts its blocks' headers
    /// carry: a block whose payload is still open frees its shell alone and is
    /// never read, so compiled drop glue, which walks static layouts, never
    /// reaches an open payload. Without `root` the hole's block is already
    /// consumed. Nothing `root` holds is destructible: it cannot fail.
    HoleAbandon {
        hole: ValueId,
        root: Option<ValueId>,
    },
    // Compiler-local regions.
    RegionEnter {
        result: ValueId,
        region: RegionInstanceId,
    },
    RegionReserve {
        result: ValueId,
        mark: ValueId,
        layout: LayoutId,
        extent: ValueId,
        failure: FailureId,
    },
    /// Published storage never moves or mutates afterwards.
    RegionPublish {
        value: ValueId,
    },
    RegionExit {
        mark: ValueId,
        region: RegionInstanceId,
        failure: FailureId,
    },
    // Arenas (MEM-4).
    /// Value stage, library bodies only, in the entry block: open an arena
    /// that every return and propagation of the unit closes, and define its
    /// token. Placement lowers it to a region entry at the unit's start and
    /// that region's `ArenaToken` in its place. A `budget` (a u64) bounds the
    /// logical bytes charged to the arena from its token on (MEM-6, LIR schema
    /// revision 26); None is unbounded.
    ArenaOpen {
        result: ValueId,
        budget: Option<ValueId>,
    },
    /// The arena token of the region `mark` entered: its record's address,
    /// with its opening's budget.
    ArenaToken {
        result: ValueId,
        mark: ValueId,
        budget: Option<ValueId>,
    },
    // Finalization and cleanup continuations.
    /// Whole destructuring suppresses this resource's finalizer (#97),
    /// leaving ordinary field cleanup pending.
    Defuse {
        address: AddressId,
    },
    FinalizeBegin {
        address: AddressId,
        anchor: CleanupAnchorId,
    },
    FinalizeEnd {
        address: AddressId,
        anchor: CleanupAnchorId,
    },
    /// Explicit suppression of a value's destruction and its descendants'.
    Forget {
        value: ValueId,
    },
    // Logical frame admission, pending #16: no numeric charge is defined yet.
    FrameEnter {
        failure: FailureId,
    },
    FrameLeave,
    /// An M2 placeholder; never emitted.
    Unavailable {
        feature: UnavailableFeature,
        anchor: SourceAnchorId,
    },
    /// Atomic entered-body seal transition: success consumes grant and root,
    /// publishes one owner and disarms storage cleanup; failure publishes none.
    SealArena {
        result: ValueId,
        grant: ValueId,
        root: ValueId,
        shape: SealDescriptorId,
        failure: FailureId,
        origin: SourceAnchorId,
    },
    /// Read-only root view for an existing exclusive invocation loan.
    BorrowSealedRoot {
        result: ValueId,
        owner: ValueId,
        loan: LoanId,
        shape: SealDescriptorId,
    },
    /// Retained semantic allocation event. COW has a fresh source allocation
    /// identity; growth replaces previous storage without adding an allocation.
    ArenaInventory {
        arena: ValueId,
        allocation: SourceAnchorId,
        value: ValueId,
        shape: SealDescriptorId,
        previous: Option<ValueId>,
        /// Paired live original root metadata, required for child-bearing payloads.
        receipt: OptionalValueId,
    },
    /// Snapshot the original source semantics before physical rewriting.
    /// Inputs follow the selected original binding exactly. This retains only
    /// private metadata holders and acquires no source storage credits.
    PayloadReceipt {
        result: ValueId,
        value: ValueId,
        shape: SealDescriptorId,
        binding: PayloadBindingId,
        inputs: PoolRange<ReceiptInput>,
        /// Explicit original Arena destination; heap and inline snapshots use NONE.
        arena: OptionalValueId,
    },
}

/// A control edge with its block arguments.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Edge {
    pub target: BlockId,
    pub arguments: PoolRange<ValueId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SwitchArm {
    pub value: u32,
    pub edge: EdgeId,
}

/// A failure successor: its cleanup entry, the rule it starts (None when it
/// propagates a callee's failure), its source anchor and the native token
/// disposal list that runs alongside the original cleanup. A MEM-9 precondition
/// edge instead ends the run with an UndefinedBehavior diagnostic before cleanup.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FailureEdge {
    pub cleanup: EdgeId,
    pub rule: Option<String>,
    pub anchor: SourceAnchorId,
    pub dispose: PoolRange<ValueId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Terminator {
    Jump(EdgeId),
    Branch {
        condition: ValueId,
        yes: EdgeId,
        no: EdgeId,
    },
    /// Ordered switch; TIR `Match` imports as switches with guard continuations.
    Switch {
        value: ValueId,
        arms: PoolRange<SwitchArm>,
        otherwise: EdgeId,
    },
    /// Take arguments transfer at entry; the result exists only on `normal`.
    /// `normal` is None exactly for a diverging call (LIR schema revision 19):
    /// its ABI `diverges`, it has no result, and only `failure` is reachable.
    Call {
        call: CallId,
        normal: Option<EdgeId>,
        failure: FailureId,
    },
    /// Return the extracted owned result; cleanup already ran.
    Return(Option<ValueId>),
    /// Begin fail-stop failure (TIR `Fail`) along the failure edge.
    Raise(FailureId),
    /// Return failure status after failure cleanup finished.
    Propagate,
    /// Private keyed-adapter rejection; actuals remain in pending caller custody.
    RejectTransfer,
    /// In failure cleanup: when the active failure is exactly the `OutOfSpace`
    /// of the bounded arena `arena`, which this unit opened, with no failure
    /// recorded after it, forget it and resume ordinary code on `caught`;
    /// otherwise continue failure cleanup on `failed` (MEM-6, LIR schema
    /// revision 26).
    Recover {
        arena: ValueId,
        caught: EdgeId,
        failed: EdgeId,
    },
    /// In failure cleanup, `task.isolate`'s library body only: catch every
    /// failure recorded since `start` (the record count the body read before
    /// its callback ran), which the runtime keeps for the caught code to read,
    /// and resume ordinary code on `caught` (FAIL-2, LIR schema revision 31).
    Isolate {
        start: ValueId,
        caught: EdgeId,
    },
    /// Requires proof, not an optimizer guess.
    Unreachable,
    /// Implementation limit or broken invariant. No language failure or cleanup.
    InvariantFault {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BlockKind {
    Body,
    /// Normal-exit cleanup expanded from a TIR cleanup chain.
    Cleanup,
    /// Failure-path cleanup.
    FailureCleanup,
}

/// A block: parameters, a contiguous instruction range, and a terminator in the
/// parallel `LirUnit::terminators` arena.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Block {
    pub kind: BlockKind,
    pub parameters: PoolRange<ValueId>,
    pub instructions: PoolRange<Inst>,
    pub anchor: SourceAnchorId,
}

/// Allocation obligation class read from TIR `Destination`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum DestinationClass {
    Heap,
    /// The arena held by this slot.
    Arena(SlotId),
    /// The hidden caller destination of these declared parameter ordinals.
    Caller(Vec<u32>),
    Region,
    /// P3's own choice.
    Inferred,
}

/// An allocation-instance fact. Not a source brand or a relation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum StorageOwner {
    Heap(TaskDomainId),
    LocalRegion(RegionInstanceId),
    Arena(ArenaInstanceId),
    Frozen(SharedInstanceId),
    Static,
}

/// Where P3 placed an allocation: a fixed owner, or the dynamic hidden caller
/// destination whose domain the checked block reference carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Placement {
    Fixed(StorageOwner),
    CallerDestination,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ConstructorKind {
    Tuple,
    Record(SymbolId),
    Variant { definition: SymbolId, variant: u32 },
    Sequence,
    Box,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FieldValue {
    /// Declaration ordinal of the field.
    pub field: u32,
    pub value: ValueId,
}

/// What a `FlatField` write does to its builder's open element (LIR schema
/// revision 24).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FlatFieldOp {
    /// Begin the open element: room is made for the element's prefix, the
    /// fields before the open field and its length word, which is set to
    /// zero. The builder must have no open element.
    Open,
    /// Room for `capacity` (a u64) more bytes of the open field, the size
    /// saturating.
    Reserve { capacity: ValueId },
    /// Append the u8 `value` to the open field, growing storage
    /// geometrically when it is full. `spare` states that the room is
    /// already there (FACT-3): no capacity is read and nothing grows.
    Push { value: ValueId, spare: bool },
}

/// A construction: layout, fields in source evaluation order, destination
/// obligation and (from Placed on) its storage placement.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ConstructSite {
    /// C proves every nontrivial child belongs to the same stable arena token
    /// or local region.
    /// No child-credit walk or payload drop is scheduled for this construction.
    /// Never set on a tracked one, which stays writable (MEM-6).
    pub arena_no_drop: bool,
    /// The construction builds a List or Array (MEM-6, LIR schema revision
    /// 32), or a Map's or Set's block or entries List (revision 33): placed in
    /// an arena, its block is tracked and starts unshared; a deep copy tracks
    /// the List, Array, Map and Set blocks it copies there, a Map's or Set's
    /// entries included and its index excluded.
    pub tracked: bool,
    pub constructor: ConstructorKind,
    pub layout: LayoutId,
    pub fields: PoolRange<FieldValue>,
    pub destination: DestinationClass,
    pub placement: Option<Placement>,
    pub anchor: SourceAnchorId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CaptureUse {
    Borrow,
    Take,
    Copy,
    Mut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CaptureValue {
    pub value: ValueId,
    pub usage: CaptureUse,
}

/// A closure: code symbol, environment layout and captures in evaluation order.
/// Its `Inst::Closure` result is a `Callable`, or (LIR schema revision 7) an
/// `Iterator`: then `code` is the step thunk, the captures are the state, and
/// the three words are the environment block, the thunk and a zero third word.
/// A generic nested function's value (LIR schema revision 9) has no code of
/// its own: spec #42 lets it be only a direct callee, so it is called only
/// through `Callee::Bound`, which names each instantiation's code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosureSite {
    pub scoped: bool,
    pub code: Option<ClosureCode>,
    pub environment: LayoutId,
    pub captures: PoolRange<CaptureValue>,
    pub destination: DestinationClass,
    pub placement: Option<Placement>,
    pub anchor: SourceAnchorId,
    /// Optional private capture companions, in a separate metadata namespace.
    pub receipts: Option<ReceiptClosure>,
}

impl std::hash::Hash for ClosureSite {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.scoped.hash(state);
        self.code.hash(state);
        self.environment.hash(state);
        self.captures.hash(state);
        self.destination.hash(state);
        self.placement.hash(state);
        self.anchor.hash(state);
        if let Some(receipts) = &self.receipts {
            "hirsel.receipt-closure.v1".hash(state);
            receipts.hash(state);
        }
    }
}

/// A closure's own code: its body's instance and that instance's `FnAbi`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ClosureCode {
    pub symbol: SymbolId,
    pub abi: FnAbiId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Callee {
    Direct(SymbolId),
    /// A callable value, or (LIR schema revision 7) an `Iterator`, whose call
    /// uses its step thunk's `FnAbi`: `fn() -> Option[T]` with the hidden
    /// environment.
    Indirect(ValueId),
    /// A registry operation with a manifest disposition.
    Operation(OperationRef),
    /// (LIR schema revision 9) Call `code`, a closure body's instance, with
    /// the environment of callable value `closure`: a generic nested
    /// function called at one instantiation. The environment is statically
    /// known, so the call is classified like a direct one.
    Bound {
        code: SymbolId,
        closure: ValueId,
    },
}

impl Callee {
    /// The callable value a call reads: an indirect callee or a bound
    /// call's closure.
    pub fn value(&self) -> Option<ValueId> {
        match *self {
            Callee::Indirect(v) | Callee::Bound { closure: v, .. } => Some(v),
            Callee::Direct(_) | Callee::Operation(_) => None,
        }
    }

    /// `value`, for rewriting.
    pub fn value_mut(&mut self) -> Option<&mut ValueId> {
        match self {
            Callee::Indirect(v) | Callee::Bound { closure: v, .. } => Some(v),
            Callee::Direct(_) | Callee::Operation(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ArgumentValue {
    Value(ValueId),
    /// A `mut` argument: the exclusive address and the root slot's new reaching
    /// version once the callee has written it back, defined at the call and
    /// reaching both its normal and its failure edge.
    Address {
        address: AddressId,
        version: ValueId,
    },
}

/// A call input in source order, mapped to its declared parameter ordinal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CallArgument {
    pub parameter: u32,
    pub passing: Passing,
    pub value: ArgumentValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ReceiptInput {
    /// Static source correspondence only; this does not reread a consumed value.
    pub original: ValueId,
    /// Absent only for independently reconstructible plain/literal leaves.
    pub receipt: Option<ValueId>,
}

/// A call: callee, ABI, arguments, result value and the callee's declared
/// effects (never body-derived public promises).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallSite {
    /// Status 3 from a validated materialized keyed-primitive adapter. On this
    /// edge evaluated Take actuals remain in pending caller custody.
    pub pretransfer_failure: Option<FailureId>,
    pub forward_caller: bool,
    pub callee: Callee,
    /// Concrete registry instantiation; read only for Operation callees (LIR 21).
    pub operation_arguments: Vec<crate::operation::OperationArgument>,
    pub abi: FnAbiId,
    pub arguments: PoolRange<CallArgument>,
    pub result: Option<ValueId>,
    pub destination: Option<DestinationClass>,
    pub declared: Effects,
    pub anchor: SourceAnchorId,
    /// Receipt carriers appended after ordinary parameters. Absence preserves
    /// the existing call encoding, identity and physical convention.
    pub receipts: Option<ReceiptCall>,
}

// Preserve the pre-receipt Hash stream for ordinary calls. A present contract
// has an explicit namespace, including a contract with no dynamic carriers.
impl std::hash::Hash for CallSite {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.pretransfer_failure.hash(state);
        self.forward_caller.hash(state);
        self.callee.hash(state);
        self.operation_arguments.hash(state);
        self.abi.hash(state);
        self.arguments.hash(state);
        self.result.hash(state);
        self.destination.hash(state);
        self.declared.hash(state);
        self.anchor.hash(state);
        if let Some(receipts) = &self.receipts {
            "hirsel.receipt-call.v1".hash(state);
            receipts.hash(state);
        }
    }
}

/// Typed literal bits; floats keep their bit patterns.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum ConstantValue {
    /// The distinguished counted-heap Arena token.
    Heap,
    Unit,
    Bool(bool),
    Integer(u128),
    F32(u32),
    F64(u64),
    String(String),
    Bytes(Vec<u8>),
    /// A `u32`: the run's descriptor-registry index of `LayoutId`'s runtime layout descriptor
    /// (`hirsel_rt_v2_layout`), the index a construction's allocation receives. A runtime
    /// entry that holds values of a type it was not compiled for (a channel's queued
    /// messages, CONC-6) reads their size and drop program through it.
    Descriptor(LayoutId),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ConstantData {
    pub ty: LirTypeId,
    pub value: ConstantValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum SymbolKind {
    Function,
    Constant,
    Type,
    Finalizer,
    Shape,
}

/// A link-level symbol: definition plus instance digest for specialized code.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub definition: DefPath,
    pub instance: Option<ContentHash>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct LoanInfo {
    pub address: AddressId,
    pub mutable: bool,
}

/// Byte range relative to one existing source anchor of the original TIR
/// definition. `source` is its canonical first-encounter selector, never a
/// process Anchor id. `u32::MAX` is an unmapped site.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct SiteRange {
    pub source: u32,
    pub start: u32,
    pub end: u32,
}

impl SiteRange {
    pub const UNMAPPED: Self = Self {
        source: u32::MAX,
        start: 0,
        end: 0,
    };
}

/// `(BodyId, block, statement)` in the TIR body, plus a stable site. The
/// statement equal to the block's instruction count names its terminator.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SourceAnchor {
    pub body: u32,
    pub block: u32,
    pub statement: u32,
    pub site: SiteRange,
}

/// `(BodyId, CleanupId, action ordinal)` correspondence anchor.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CleanupAnchor {
    pub body: u32,
    pub cleanup: u32,
    pub action: u32,
    pub site: SiteRange,
}

/// Cleanup-anchor ordinals reserved for drops TIR states outside a cleanup chain.
pub const INSTRUCTION_DROP: u32 = u32::MAX;
/// The release of a self-tail loop's internal owned slot (LANG-10).
pub const LOOP_RELEASE: u32 = u32::MAX - 1;
/// The disposal of an owned value pending when an operation failed.
pub const SCRATCH_DISPOSAL: u32 = u32::MAX - 2;
/// A release derived from the state where TIR names no cleanup (an operation
/// TIR cannot make fail: a prologue preparation or a match test).
pub const STATE_RELEASE: u32 = u32::MAX - 3;

impl CleanupAnchor {
    /// Whether this anchor names an action of a TIR cleanup chain, which runs
    /// in a `Cleanup` or `FailureCleanup` block, rather than a reserved
    /// ordinal (an instruction's own drop, a loop release, a disposal or a
    /// state release).
    pub fn names_tir_cleanup(&self) -> bool {
        self.cleanup < STATE_RELEASE
    }
}

/// A static initialized-field mask.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FieldMask {
    pub fields: u32,
    pub words: PoolRange<u64>,
}

/// One compiler-local region instance.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RegionInstance {
    /// The TIR scope whose exit ends the region.
    pub scope: ScopeId,
    pub anchor: SourceAnchorId,
    /// Untrusted physical hint: reserve the demanded guard extension. Neither
    /// value grants source permission; false at a sealing scope fails closed
    /// at the runtime's ordinary-guard refusal.
    pub seal_demand: bool,
}

impl RegionInstance {
    pub fn record_bytes(&self) -> u64 {
        if self.seal_demand {
            crate::runtime_abi::RECEIPT_REGION_RECORD_BYTES
        } else {
            crate::runtime_abi::REGION_RECORD_BYTES
        }
    }
}

/// An M2 arena or Shared instance: the slot holding its handle.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ExternalInstance {
    pub slot: SlotId,
}

/// A drop flag (LIR schema revision 8): the bool block parameter `value` is
/// true exactly when `slot` holds content (live and initialized) on entry to
/// its block. Cleanup lowering adds one where a drop meets conditionally
/// initialized content and branches on it; the validator checks each
/// argument against the slot's state on its edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct InitFlag {
    pub value: ValueId,
    pub slot: SlotId,
    /// An inline field path; empty retains the whole-slot revision-8 meaning.
    /// True names live content at this part, accounting for transferred and
    /// completed owners. Plain parts retain initialized-byte semantics.
    pub path: PoolRange<ProjectionStep>,
    /// A persistent environment live word. `value` is its entry capture version;
    /// without an environment this is the revision-8 bool join flag.
    pub environment: Option<LayoutId>,
}

/// Fixed-size side tables referenced by instructions and terminators.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SideTables {
    pub calls: Vec<CallSite>,
    pub constructs: Vec<ConstructSite>,
    pub closures: Vec<ClosureSite>,
    pub constants: Vec<ConstantData>,
    pub symbols: Vec<Symbol>,
    pub operations: Vec<OperationId>,
    pub loans: Vec<LoanInfo>,
    pub failures: Vec<FailureEdge>,
    pub edges: Vec<Edge>,
    pub masks: Vec<FieldMask>,
    pub regions: Vec<RegionInstance>,
    pub arenas: Vec<ExternalInstance>,
    pub shared: Vec<ExternalInstance>,
    pub task_domains: u32,
    pub source_anchors: Vec<SourceAnchor>,
    pub cleanup_anchors: Vec<CleanupAnchor>,
    /// Drop flags (LIR schema revision 8).
    pub init_flags: Vec<InitFlag>,
    /// SER-2 codec plans (LIR schema revision 29).
    pub codecs: Vec<crate::codec::CodecPlan>,
    /// MEM-3 logical types and complete physical access, independently validated.
    pub structural: Vec<crate::structural_access::AccessPlan>,
    /// None preserves the ordinary side-table identity.
    pub receipt_entry: Option<ReceiptEntry>,
    pub seal_descriptors: Vec<crate::sealed::SealDescriptor>,
    pub seal_payloads: Vec<crate::sealed_payload::PayloadTable>,
}

impl std::hash::Hash for SideTables {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let Self {
            calls,
            constructs,
            closures,
            constants,
            symbols,
            operations,
            loans,
            failures,
            edges,
            masks,
            regions,
            arenas,
            shared,
            task_domains,
            source_anchors,
            cleanup_anchors,
            init_flags,
            codecs,
            structural,
            receipt_entry,
            seal_descriptors,
            seal_payloads,
        } = self;
        calls.hash(state);
        constructs.hash(state);
        closures.hash(state);
        constants.hash(state);
        symbols.hash(state);
        operations.hash(state);
        loans.hash(state);
        failures.hash(state);
        edges.hash(state);
        masks.hash(state);
        regions.hash(state);
        arenas.hash(state);
        shared.hash(state);
        task_domains.hash(state);
        source_anchors.hash(state);
        cleanup_anchors.hash(state);
        init_flags.hash(state);
        codecs.hash(state);
        structural.hash(state);
        if let Some(entry) = receipt_entry {
            "hirsel.receipt-entry.v1".hash(state);
            entry.hash(state);
        }
        if !seal_descriptors.is_empty() {
            seal_descriptors.hash(state);
        }
        if !seal_payloads.is_empty() {
            seal_payloads.hash(state);
        }
    }
}

impl SideTables {
    /// Resolve a unit-local original binding ordinal across descriptor tables.
    /// The descriptor is retained so an ordinal cannot be reused at another root.
    pub fn payload_binding(
        &self,
        id: PayloadBindingId,
    ) -> Option<(SealDescriptorId, &crate::sealed_payload::PayloadBinding)> {
        let mut ordinal = id.index();
        for table in &self.seal_payloads {
            if ordinal < table.bindings.len() {
                return Some((table.descriptor, &table.bindings[ordinal]));
            }
            ordinal -= table.bindings.len();
        }
        None
    }

    /// The persistent live word's environment, if this body consumes captures.
    pub fn environment(&self) -> Option<LayoutId> {
        self.init_flags.iter().find_map(|flag| flag.environment)
    }
}

/// Variable-length operand pools.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Pools {
    pub values: Vec<ValueId>,
    pub addresses: Vec<AddressId>,
    pub projections: Vec<ProjectionStep>,
    pub switch_arms: Vec<SwitchArm>,
    pub call_arguments: Vec<CallArgument>,
    pub fields: Vec<FieldValue>,
    pub captures: Vec<CaptureValue>,
    pub mask_words: Vec<u64>,
    pub receipt_inputs: Vec<ReceiptInput>,
}

impl std::hash::Hash for Pools {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let Self {
            values,
            addresses,
            projections,
            switch_arms,
            call_arguments,
            fields,
            captures,
            mask_words,
            receipt_inputs,
        } = self;
        values.hash(state);
        addresses.hash(state);
        projections.hash(state);
        switch_arms.hash(state);
        call_arguments.hash(state);
        fields.hash(state);
        captures.hash(state);
        mask_words.hash(state);
        if !receipt_inputs.is_empty() {
            "hirsel.payload-receipt-inputs.v1".hash(state);
            receipt_inputs.hash(state);
        }
    }
}

/// Where a unit's body came from (LIR schema revision 7).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum UnitOrigin {
    /// Imported from a TIR body.
    Source,
    /// Built by `hirsel_rc::library` for a registry operation or a library
    /// finalizer. Only such a unit may contain a `Slot` step, `SeqReserve`,
    /// `SeqSetLen`, `SeqAppend` or `Recast`.
    Library,
}

/// Identity and versions of one unit.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct UnitHeader {
    /// Number of existing source anchors in the original definition's canonical walk.
    pub source_count: u32,
    pub track_caller: bool,
    pub stage: StageKind,
    pub lir_version: u32,
    pub origin: UnitOrigin,
    /// The planned content key; independently checked input evidence lives in the receipt.
    pub key: crate::objects::UnitKey,
    pub definition: DefPath,
    pub implementation_revision: ContentHash,
    /// Digest of the unit's `InstanceKey`.
    pub instance: ContentHash,
    pub target: ContentHash,
    pub abi: FnAbiId,
}

/// One function body at stage `S`. Fields are public so tests and adversarial
/// inputs can forge raw units; only validators produce `ValidatedUnit<S>`.
#[derive(Debug, Eq, PartialEq, Hash)]
pub struct LirUnit<S> {
    pub header: UnitHeader,
    pub types: Vec<LirType>,
    pub values: Vec<ValueInfo>,
    pub slots: Vec<SlotInfo>,
    pub addresses: Vec<AddressInfo>,
    pub blocks: Vec<Block>,
    /// Parallel to `blocks`.
    pub terminators: Vec<Terminator>,
    pub instructions: Vec<Inst>,
    pub entry: BlockId,
    /// Values of the declared parameters, in ordinal order.
    pub parameters: Vec<ValueId>,
    /// Version names of the environment content of the `SlotKind::Capture`
    /// slots, in capture ordinal order, defined at entry.
    pub captures: Vec<ValueId>,
    /// The scope nesting forest, indexed by `ScopeId`.
    pub scopes: Vec<ScopeInfo>,
    pub tables: SideTables,
    pub pools: Pools,
    pub stage: PhantomData<S>,
}

impl<S: Clone> Clone for LirUnit<S> {
    fn clone(&self) -> Self {
        crate::construction::record(&self.header, crate::construction::ValueWork::Clone);
        Self {
            header: self.header.clone(),
            types: self.types.clone(),
            values: self.values.clone(),
            slots: self.slots.clone(),
            addresses: self.addresses.clone(),
            blocks: self.blocks.clone(),
            terminators: self.terminators.clone(),
            instructions: self.instructions.clone(),
            entry: self.entry,
            parameters: self.parameters.clone(),
            captures: self.captures.clone(),
            scopes: self.scopes.clone(),
            tables: self.tables.clone(),
            pools: self.pools.clone(),
            stage: self.stage,
        }
    }
}

impl<S: Stage> LirUnit<S> {
    /// An empty unit whose header names stage `S`.
    pub fn new(mut header: UnitHeader) -> Self {
        header.stage = S::KIND;
        crate::construction::record(&header, crate::construction::ValueWork::Create);
        Self {
            header,
            types: Vec::new(),
            values: Vec::new(),
            slots: Vec::new(),
            addresses: Vec::new(),
            blocks: Vec::new(),
            terminators: Vec::new(),
            instructions: Vec::new(),
            entry: BlockId(0),
            parameters: Vec::new(),
            captures: Vec::new(),
            scopes: Vec::new(),
            tables: SideTables::default(),
            pools: Pools::default(),
            stage: PhantomData,
        }
    }
    /// Retag the raw data as another stage. The result is unvalidated: its
    /// stage validator must accept it before it can be used as that stage.
    pub fn into_stage<T: Stage>(self) -> LirUnit<T> {
        let mut header = self.header;
        header.stage = T::KIND;
        crate::construction::record(&header, crate::construction::ValueWork::Retag);
        LirUnit {
            header,
            types: self.types,
            values: self.values,
            slots: self.slots,
            addresses: self.addresses,
            blocks: self.blocks,
            terminators: self.terminators,
            instructions: self.instructions,
            entry: self.entry,
            parameters: self.parameters,
            captures: self.captures,
            scopes: self.scopes,
            tables: self.tables,
            pools: self.pools,
            stage: PhantomData,
        }
    }
    pub fn digest(&self) -> ContentHash {
        crate::snapshot::digest("lir-unit", self)
    }
    /// Total bytes retained by the unit's arenas, side tables and pools, for the
    /// design's measurement report (rows alone are `size_of::<Inst>()` each).
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of_val;
        let t = &self.tables;
        let p = &self.pools;
        size_of_val(self.types.as_slice())
            + size_of_val(self.values.as_slice())
            + size_of_val(self.slots.as_slice())
            + size_of_val(self.addresses.as_slice())
            + size_of_val(self.blocks.as_slice())
            + size_of_val(self.terminators.as_slice())
            + size_of_val(self.instructions.as_slice())
            + size_of_val(self.captures.as_slice())
            + size_of_val(self.scopes.as_slice())
            + size_of_val(t.calls.as_slice())
            + size_of_val(t.constructs.as_slice())
            + size_of_val(t.closures.as_slice())
            + size_of_val(t.constants.as_slice())
            + size_of_val(t.failures.as_slice())
            + size_of_val(t.edges.as_slice())
            + size_of_val(p.values.as_slice())
            + size_of_val(p.addresses.as_slice())
            + size_of_val(p.projections.as_slice())
            + size_of_val(p.switch_arms.as_slice())
            + size_of_val(p.call_arguments.as_slice())
            + size_of_val(p.fields.as_slice())
            + size_of_val(p.captures.as_slice())
            + size_of_val(p.mask_words.as_slice())
    }
}

/// The 32-byte instruction row bound.
pub const MAX_INSTRUCTION_BYTES: usize = 32;
const _: () = assert!(std::mem::size_of::<Inst>() <= MAX_INSTRUCTION_BYTES);

/// Every index an instruction, terminator or side-table row names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ref {
    Value(ValueId),
    Slot(SlotId),
    Address(AddressId),
    Block(BlockId),
    Type(LirTypeId),
    Layout(LayoutId),
    Abi(FnAbiId),
    Call(CallId),
    Construct(ConstructId),
    Closure(ClosureId),
    Constant(ConstantId),
    Symbol(SymbolId),
    Operation(OperationRef),
    Loan(LoanId),
    Failure(FailureId),
    Edge(EdgeId),
    Mask(MaskId),
    SourceAnchor(SourceAnchorId),
    CleanupAnchor(CleanupAnchorId),
    Region(RegionInstanceId),
    Arena(ArenaInstanceId),
    Shared(SharedInstanceId),
    TaskDomain(TaskDomainId),
    Scope(ScopeId),
    Codec(CodecId),
    SealDescriptor(SealDescriptorId),
    PayloadBinding(PayloadBindingId),
    Values(PoolRange<ValueId>),
    Addresses(PoolRange<AddressId>),
    Projections(PoolRange<ProjectionStep>),
    SwitchArms(PoolRange<SwitchArm>),
    CallArguments(PoolRange<CallArgument>),
    Fields(PoolRange<FieldValue>),
    Captures(PoolRange<CaptureValue>),
    MaskWords(PoolRange<u64>),
    Instructions(PoolRange<Inst>),
    ReceiptInputs(PoolRange<ReceiptInput>),
}

impl MaskRef {
    fn visit(&self, f: &mut impl FnMut(Ref)) {
        match *self {
            Self::All => {}
            Self::Static(mask) => f(Ref::Mask(mask)),
            Self::Dynamic(value) => f(Ref::Value(value)),
        }
    }
}

impl Inst {
    /// Values defined by this instruction, in field order.
    pub fn defs(&self) -> Vec<ValueId> {
        match *self {
            Inst::Const { result, .. }
            | Inst::FunctionRef { result, .. }
            | Inst::ReadStatic { result, .. }
            | Inst::Unary { result, .. }
            | Inst::Binary { result, .. }
            | Inst::Compare { result, .. }
            | Inst::IsUnique { result, .. }
            | Inst::IsWritable { result, .. }
            | Inst::Weaken { result, .. }
            | Inst::Read { result, .. }
            | Inst::Copy { result, .. }
            | Inst::FlatCopy { result, .. }
            | Inst::FlatView { result, .. }
            | Inst::SliceView { result, .. }
            | Inst::FlatLength { result, .. }
            | Inst::FlatNext { result, .. }
            | Inst::SeqLen { result, .. }
            | Inst::SeqCapacity { result, .. }
            | Inst::Hash { result, .. }
            | Inst::Encode { result, .. }
            | Inst::Decode { result, .. }
            | Inst::CopyValue { result, .. }
            | Inst::Recast { result, .. }
            | Inst::Freeze { result, .. }
            | Inst::Thaw { result, .. }
            | Inst::UniqueCopy { result, .. }
            | Inst::UpdateMask { result, .. }
            | Inst::Construct { result, .. }
            | Inst::Closure { result, .. }
            | Inst::Reset { result, .. }
            | Inst::Reuse { result, .. }
            | Inst::TokenNone { result }
            | Inst::RegionEnter { result, .. }
            | Inst::RegionReserve { result, .. }
            | Inst::SealArena { result, .. }
            | Inst::BorrowSealedRoot { result, .. }
            | Inst::PayloadReceipt { result, .. }
            | Inst::ArenaOpen { result, .. }
            | Inst::ArenaToken { result, .. } => vec![result],
            Inst::HoleOpen { result, hole, .. } => vec![result, hole],
            Inst::Convert { result, flag, .. } | Inst::IntArith { result, flag, .. } => {
                std::iter::once(result).chain(flag).collect()
            }
            Inst::MulAdd { result, .. } | Inst::ScalarUnary { result, .. } => vec![result],
            Inst::Store { version, .. }
            | Inst::SeqReserve { version, .. }
            | Inst::ArenaPush { version, .. }
            | Inst::SeqSetLen { version, .. }
            | Inst::SeqAppend { version, .. }
            | Inst::FlatAppend { version, .. }
            | Inst::FlatField { version, .. }
            | Inst::FlatClose { version, .. } => vec![version],
            Inst::Replace {
                version,
                displaced_version,
                ..
            } => vec![version, displaced_version],
            Inst::Check { .. }
            | Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::EndScope { .. }
            | Inst::Address { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::Drop { .. }
            | Inst::Dup { .. }
            | Inst::Share { .. }
            | Inst::Release { .. }
            | Inst::Dispose { .. }
            | Inst::HoleFill { .. }
            | Inst::HoleAbandon { .. }
            | Inst::RegionPublish { .. }
            | Inst::ArenaInventory { .. }
            | Inst::RegionExit { .. }
            | Inst::Defuse { .. }
            | Inst::FinalizeBegin { .. }
            | Inst::FinalizeEnd { .. }
            | Inst::Forget { .. }
            | Inst::FrameEnter { .. }
            | Inst::FrameLeave
            | Inst::Unavailable { .. } => Vec::new(),
        }
    }

    /// Explicit failure continuation, if this instruction can fail.
    pub fn failure(&self) -> Option<FailureId> {
        match *self {
            Inst::Unary { failure, .. }
            | Inst::Binary { failure, .. }
            | Inst::Compare { failure, .. }
            | Inst::Hash { failure, .. }
            | Inst::Check { failure, .. }
            | Inst::Address { failure, .. }
            | Inst::Copy { failure, .. }
            | Inst::FlatCopy { failure, .. }
            | Inst::SliceView { failure, .. }
            | Inst::UniqueCopy { failure, .. }
            | Inst::ArenaPush { failure, .. }
            | Inst::Replace { failure, .. }
            | Inst::Construct { failure, .. }
            | Inst::Closure { failure, .. }
            | Inst::Drop { failure, .. }
            | Inst::Release { failure, .. }
            | Inst::Reset { failure, .. }
            | Inst::Reuse { failure, .. }
            | Inst::HoleOpen { failure, .. }
            | Inst::RegionReserve { failure, .. }
            | Inst::RegionExit { failure, .. }
            | Inst::SealArena { failure, .. }
            | Inst::FrameEnter { failure } => Some(failure),
            Inst::Const { .. }
            | Inst::FunctionRef { .. }
            | Inst::ReadStatic { .. }
            | Inst::Convert { .. }
            | Inst::IntArith { .. }
            | Inst::MulAdd { .. }
            | Inst::ScalarUnary { .. }
            | Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::EndScope { .. }
            | Inst::Read { .. }
            | Inst::FlatView { .. }
            | Inst::FlatLength { .. }
            | Inst::FlatNext { .. }
            | Inst::SeqLen { .. }
            | Inst::SeqCapacity { .. }
            | Inst::SeqReserve { .. }
            | Inst::SeqSetLen { .. }
            | Inst::SeqAppend { .. }
            | Inst::FlatAppend { .. }
            | Inst::FlatField { .. }
            | Inst::FlatClose { .. }
            | Inst::Encode { .. }
            | Inst::Decode { .. }
            | Inst::CopyValue { .. }
            | Inst::Recast { .. }
            | Inst::Freeze { .. }
            | Inst::Thaw { .. }
            | Inst::Store { .. }
            | Inst::UpdateMask { .. }
            | Inst::Weaken { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::Dup { .. }
            | Inst::Share { .. }
            | Inst::IsUnique { .. }
            | Inst::IsWritable { .. }
            | Inst::Dispose { .. }
            | Inst::TokenNone { .. }
            | Inst::HoleFill { .. }
            | Inst::HoleAbandon { .. }
            | Inst::RegionEnter { .. }
            | Inst::RegionPublish { .. }
            | Inst::ArenaOpen { .. }
            | Inst::ArenaToken { .. }
            | Inst::ArenaInventory { .. }
            | Inst::PayloadReceipt { .. }
            | Inst::BorrowSealedRoot { .. }
            | Inst::Defuse { .. }
            | Inst::FinalizeBegin { .. }
            | Inst::FinalizeEnd { .. }
            | Inst::Forget { .. }
            | Inst::FrameLeave
            | Inst::Unavailable { .. } => None,
        }
    }

    /// Mutable explicit failure continuation.
    pub fn failure_mut(&mut self) -> Option<&mut FailureId> {
        match self {
            Inst::Unary { failure, .. }
            | Inst::Binary { failure, .. }
            | Inst::Compare { failure, .. }
            | Inst::Hash { failure, .. }
            | Inst::Check { failure, .. }
            | Inst::Address { failure, .. }
            | Inst::Copy { failure, .. }
            | Inst::FlatCopy { failure, .. }
            | Inst::SliceView { failure, .. }
            | Inst::UniqueCopy { failure, .. }
            | Inst::ArenaPush { failure, .. }
            | Inst::Replace { failure, .. }
            | Inst::Construct { failure, .. }
            | Inst::Closure { failure, .. }
            | Inst::Drop { failure, .. }
            | Inst::Release { failure, .. }
            | Inst::Reset { failure, .. }
            | Inst::Reuse { failure, .. }
            | Inst::HoleOpen { failure, .. }
            | Inst::RegionReserve { failure, .. }
            | Inst::RegionExit { failure, .. }
            | Inst::SealArena { failure, .. }
            | Inst::FrameEnter { failure } => Some(failure),
            Inst::Const { .. }
            | Inst::FunctionRef { .. }
            | Inst::ReadStatic { .. }
            | Inst::Convert { .. }
            | Inst::IntArith { .. }
            | Inst::MulAdd { .. }
            | Inst::ScalarUnary { .. }
            | Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::EndScope { .. }
            | Inst::Read { .. }
            | Inst::FlatView { .. }
            | Inst::FlatLength { .. }
            | Inst::FlatNext { .. }
            | Inst::SeqLen { .. }
            | Inst::SeqCapacity { .. }
            | Inst::SeqReserve { .. }
            | Inst::SeqSetLen { .. }
            | Inst::SeqAppend { .. }
            | Inst::FlatAppend { .. }
            | Inst::FlatField { .. }
            | Inst::FlatClose { .. }
            | Inst::Encode { .. }
            | Inst::Decode { .. }
            | Inst::CopyValue { .. }
            | Inst::Recast { .. }
            | Inst::Freeze { .. }
            | Inst::Thaw { .. }
            | Inst::Store { .. }
            | Inst::UpdateMask { .. }
            | Inst::Weaken { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::BindFields { .. }
            | Inst::EndOrigin { .. }
            | Inst::Dup { .. }
            | Inst::Share { .. }
            | Inst::IsUnique { .. }
            | Inst::IsWritable { .. }
            | Inst::Dispose { .. }
            | Inst::TokenNone { .. }
            | Inst::HoleFill { .. }
            | Inst::HoleAbandon { .. }
            | Inst::RegionEnter { .. }
            | Inst::RegionPublish { .. }
            | Inst::ArenaOpen { .. }
            | Inst::ArenaToken { .. }
            | Inst::ArenaInventory { .. }
            | Inst::PayloadReceipt { .. }
            | Inst::BorrowSealedRoot { .. }
            | Inst::Defuse { .. }
            | Inst::FinalizeBegin { .. }
            | Inst::FinalizeEnd { .. }
            | Inst::Forget { .. }
            | Inst::FrameLeave
            | Inst::Unavailable { .. } => None,
        }
    }

    /// Visit every index the row names, in field order. Exhaustive: a new
    /// variant cannot compile without stating its references.
    pub fn visit(&self, f: &mut impl FnMut(Ref)) {
        use Ref as R;
        match *self {
            Self::Const { result, constant } => {
                f(R::Value(result));
                f(R::Constant(constant));
            }
            Self::FunctionRef { result, symbol } | Self::ReadStatic { result, symbol } => {
                f(R::Value(result));
                f(R::Symbol(symbol));
            }
            Self::Unary {
                result,
                op: _,
                operand,
                failure,
            } => {
                f(R::Value(result));
                f(R::Value(operand));
                f(R::Failure(failure));
            }
            Self::Binary {
                result,
                op: _,
                left,
                right,
                failure,
            } => {
                f(R::Value(result));
                f(R::Value(left));
                f(R::Value(right));
                f(R::Failure(failure));
            }
            Self::Compare {
                result,
                op: _,
                left,
                right,
                layout,
                failure,
            } => {
                f(R::Value(result));
                f(R::Value(left));
                f(R::Value(right));
                f(R::Layout(layout));
                f(R::Failure(failure));
            }
            Self::Convert {
                result,
                flag,
                operand,
                mode: _,
            } => {
                f(R::Value(result));
                if let Some(flag) = flag {
                    f(R::Value(flag));
                }
                f(R::Value(operand));
            }
            Self::IntArith {
                result,
                flag,
                op: _,
                mode: _,
                left,
                right,
            } => {
                f(R::Value(result));
                if let Some(flag) = flag {
                    f(R::Value(flag));
                }
                f(R::Value(left));
                f(R::Value(right));
            }
            Self::MulAdd {
                result,
                left,
                right,
                addend,
            } => {
                f(R::Value(result));
                f(R::Value(left));
                f(R::Value(right));
                f(R::Value(addend));
            }
            Self::ScalarUnary {
                result,
                op: _,
                operand,
            } => {
                f(R::Value(result));
                f(R::Value(operand));
            }
            Self::IsUnique { result, value }
            | Self::IsWritable { result, value }
            | Self::Weaken { result, value }
            | Self::SeqLen { result, value }
            | Self::SeqCapacity { result, value }
            | Self::CopyValue { result, value }
            | Self::Recast { result, value }
            | Self::Freeze { result, value }
            | Self::Thaw { result, value } => {
                f(R::Value(result));
                f(R::Value(value));
            }
            Self::Check { condition, failure } => {
                f(R::Value(condition));
                f(R::Failure(failure));
            }
            Self::StorageLive { slot } | Self::StorageDead { slot, if_live: _ } => f(R::Slot(slot)),
            Self::EndScope { scope } => f(R::Scope(scope)),
            Self::FrameLeave => {}
            Self::Address { address, failure } => {
                f(R::Address(address));
                f(R::Failure(failure));
            }
            Self::Read {
                result,
                address,
                reaching,
                mode: _,
            } => {
                f(R::Value(result));
                f(R::Address(address));
                f(R::Value(reaching));
            }
            Self::Copy {
                result,
                address,
                reaching,
                failure,
            } => {
                f(R::Value(result));
                f(R::Address(address));
                f(R::Value(reaching));
                f(R::Failure(failure));
            }
            Self::FlatCopy {
                result,
                address,
                layout,
                failure,
            } => {
                f(R::Value(result));
                f(R::Address(address));
                f(R::Layout(layout));
                f(R::Failure(failure));
            }
            Self::Hash {
                result,
                value,
                layout,
                failure,
            } => {
                f(R::Value(result));
                f(R::Value(value));
                f(R::Layout(layout));
                f(R::Failure(failure));
            }
            Self::Encode {
                result,
                value,
                codec,
            }
            | Self::Decode {
                result,
                value,
                codec,
            } => {
                f(R::Value(result));
                f(R::Value(value));
                f(R::Codec(codec));
            }
            Self::SeqReserve {
                address,
                capacity: value,
                version,
            }
            | Self::SeqSetLen {
                address,
                len: value,
                version,
            }
            | Self::SeqAppend {
                address,
                value,
                version,
            } => {
                f(R::Address(address));
                f(R::Value(value));
                f(R::Value(version));
            }
            Self::ArenaPush {
                address,
                count,
                version,
                failure,
            } => {
                f(R::Address(address));
                if let Some(count) = count {
                    f(R::Value(count));
                }
                f(R::Value(version));
                f(R::Failure(failure));
            }
            Self::FlatAppend {
                address,
                construct,
                version,
            } => {
                f(R::Address(address));
                f(R::Construct(construct));
                f(R::Value(version));
            }
            Self::FlatField {
                address,
                field: _,
                op,
                version,
            } => {
                f(R::Address(address));
                match op {
                    FlatFieldOp::Open => {}
                    FlatFieldOp::Reserve { capacity: value } | FlatFieldOp::Push { value, .. } => {
                        f(R::Value(value))
                    }
                }
                f(R::Value(version));
            }
            Self::FlatClose {
                address,
                construct,
                field: _,
                version,
            } => {
                f(R::Address(address));
                f(R::Construct(construct));
                f(R::Value(version));
            }
            Self::SliceView {
                result,
                value,
                start,
                end,
                failure,
            } => {
                for v in [result, value, start, end] {
                    f(R::Value(v));
                }
                f(R::Failure(failure));
            }
            Self::FlatView { result, address }
            | Self::FlatLength { result, address }
            | Self::FlatNext { result, address } => {
                f(R::Value(result));
                f(R::Address(address));
            }
            Self::UniqueCopy {
                result,
                value,
                construct,
                deep: _,
                failure,
            } => {
                f(R::Value(result));
                f(R::Value(value));
                f(R::Construct(construct));
                f(R::Failure(failure));
            }
            Self::Store {
                address,
                value,
                version,
            } => {
                f(R::Address(address));
                f(R::Value(value));
                f(R::Value(version));
            }
            Self::Replace {
                address,
                value,
                displaced,
                version,
                displaced_version,
                if_live: _,
                failure,
            } => {
                f(R::Address(address));
                f(R::Value(value));
                f(R::Slot(displaced));
                f(R::Value(version));
                f(R::Value(displaced_version));
                f(R::Failure(failure));
            }
            Self::UpdateMask {
                result,
                mask,
                field: _,
                initialized: _,
            } => {
                f(R::Value(result));
                f(R::Value(mask));
            }
            Self::Construct {
                result,
                construct,
                failure,
            } => {
                f(R::Value(result));
                f(R::Construct(construct));
                f(R::Failure(failure));
            }
            Self::Closure {
                result,
                closure,
                failure,
            } => {
                f(R::Value(result));
                f(R::Closure(closure));
                f(R::Failure(failure));
            }
            Self::BeginLoan {
                loan,
                address,
                mutable: _,
                if_inactive: _,
            } => {
                f(R::Loan(loan));
                f(R::Address(address));
            }
            Self::EndLoan { loan, if_active: _ } => f(R::Loan(loan)),
            Self::BindOrigin {
                source,
                place,
                kind: _,
            } => {
                f(R::Address(source));
                f(R::Address(place));
            }
            Self::BindFields { source, fields } => {
                f(R::Address(source));
                f(R::Addresses(fields));
            }
            Self::EndOrigin { place } => f(R::Address(place)),
            Self::Drop {
                address,
                if_live: _,
                anchor,
                failure,
            } => {
                f(R::Address(address));
                f(R::CleanupAnchor(anchor));
                f(R::Failure(failure));
            }
            Self::Dup { value }
            | Self::Share { value }
            | Self::Forget { value }
            | Self::RegionPublish { value }
            | Self::Dispose { token: value }
            | Self::TokenNone { result: value } => f(R::Value(value)),
            Self::HoleFill { hole, value } => {
                f(R::Value(hole));
                f(R::Value(value));
            }
            Self::HoleAbandon { hole, root } => {
                f(R::Value(hole));
                if let Some(root) = root {
                    f(R::Value(root));
                }
            }
            Self::HoleOpen {
                result,
                hole,
                construct,
                open,
                failure,
            } => {
                f(R::Value(result));
                f(R::Value(hole));
                f(R::Construct(construct));
                f(R::Layout(open));
                f(R::Failure(failure));
            }
            Self::SealArena {
                result,
                grant,
                root,
                shape,
                failure,
                origin,
            } => {
                f(R::Value(result));
                f(R::Value(grant));
                f(R::Value(root));
                f(R::SealDescriptor(shape));
                f(R::Failure(failure));
                f(R::SourceAnchor(origin));
            }
            Self::BorrowSealedRoot {
                result,
                owner,
                loan,
                shape,
            } => {
                f(R::Value(result));
                f(R::Value(owner));
                f(R::Loan(loan));
                f(R::SealDescriptor(shape));
            }
            Self::ArenaInventory {
                receipt,
                arena,
                allocation,
                value,
                shape,
                previous,
            } => {
                if let Some(receipt) = receipt.get() {
                    f(R::Value(receipt));
                }
                f(R::Value(arena));
                f(R::SourceAnchor(allocation));
                f(R::Value(value));
                f(R::SealDescriptor(shape));
                if let Some(previous) = previous {
                    f(R::Value(previous));
                }
            }
            Self::PayloadReceipt {
                arena,
                result,
                value,
                shape,
                binding,
                inputs,
            } => {
                f(R::Value(result));
                f(R::Value(value));
                f(R::SealDescriptor(shape));
                f(R::PayloadBinding(binding));
                f(R::ReceiptInputs(inputs));
                if let Some(arena) = arena.get() {
                    f(R::Value(arena));
                }
            }
            Self::ArenaOpen { result, budget } => {
                f(R::Value(result));
                if let Some(budget) = budget {
                    f(R::Value(budget));
                }
            }
            Self::ArenaToken {
                result,
                mark,
                budget,
            } => {
                f(R::Value(result));
                f(R::Value(mark));
                if let Some(budget) = budget {
                    f(R::Value(budget));
                }
            }
            Self::Release {
                value,
                layout,
                live,
                anchor,
                failure,
            } => {
                f(R::Value(value));
                f(R::Layout(layout));
                live.visit(f);
                f(R::CleanupAnchor(anchor));
                f(R::Failure(failure));
            }
            Self::Reset {
                result,
                value,
                layout,
                live,
                anchor,
                failure,
            } => {
                f(R::Value(result));
                f(R::Value(value));
                f(R::Layout(layout));
                live.visit(f);
                f(R::CleanupAnchor(anchor));
                f(R::Failure(failure));
            }
            Self::Reuse {
                result,
                token,
                construct,
                kept,
                failure,
            } => {
                f(R::Value(result));
                f(R::Value(token));
                f(R::Construct(construct));
                if let Some(kept) = kept {
                    f(R::Mask(kept));
                }
                f(R::Failure(failure));
            }
            Self::RegionEnter { result, region } => {
                f(R::Value(result));
                f(R::Region(region));
            }
            Self::RegionReserve {
                result,
                mark,
                layout,
                extent,
                failure,
            } => {
                f(R::Value(result));
                f(R::Value(mark));
                f(R::Layout(layout));
                f(R::Value(extent));
                f(R::Failure(failure));
            }
            Self::RegionExit {
                mark,
                region,
                failure,
            } => {
                f(R::Value(mark));
                f(R::Region(region));
                f(R::Failure(failure));
            }
            Self::Defuse { address } => f(R::Address(address)),
            Self::FinalizeBegin { address, anchor } | Self::FinalizeEnd { address, anchor } => {
                f(R::Address(address));
                f(R::CleanupAnchor(anchor));
            }
            Self::FrameEnter { failure } => f(R::Failure(failure)),
            Self::Unavailable { feature: _, anchor } => f(R::SourceAnchor(anchor)),
        }
    }
}

impl Terminator {
    /// Edges taken when the terminator succeeds, in declared order.
    pub fn normal_edges(&self, pools: &Pools) -> Vec<EdgeId> {
        match *self {
            Terminator::Jump(e) => vec![e],
            Terminator::Branch { yes, no, .. } => vec![yes, no],
            Terminator::Switch {
                arms, otherwise, ..
            } => arms
                .get(&pools.switch_arms)
                .unwrap_or(&[])
                .iter()
                .map(|a| a.edge)
                .chain([otherwise])
                .collect(),
            Terminator::Call { normal, .. } => normal.into_iter().collect(),
            Terminator::Recover { caught, failed, .. } => vec![caught, failed],
            Terminator::Isolate { caught, .. } => vec![caught],
            Terminator::Return(_)
            | Terminator::Raise(_)
            | Terminator::Propagate
            | Terminator::RejectTransfer
            | Terminator::Unreachable
            | Terminator::InvariantFault { .. } => Vec::new(),
        }
    }

    /// Explicit failure continuation, if any.
    pub fn failure(&self) -> Option<FailureId> {
        match *self {
            Terminator::Call { failure, .. } | Terminator::Raise(failure) => Some(failure),
            Terminator::Jump(_)
            | Terminator::Branch { .. }
            | Terminator::Switch { .. }
            | Terminator::Return(_)
            | Terminator::Propagate
            | Terminator::RejectTransfer
            | Terminator::Recover { .. }
            | Terminator::Isolate { .. }
            | Terminator::Unreachable
            | Terminator::InvariantFault { .. } => None,
        }
    }

    /// Entered-callee failure and, where allowed, pending-transfer rejection.
    pub fn failure_edges(&self, tables: &SideTables) -> Vec<FailureId> {
        let mut failures: Vec<_> = self.failure().into_iter().collect();
        if let Self::Call { call, .. } = self {
            if let Some(failure) = tables
                .calls
                .get(call.index())
                .and_then(|call| call.pretransfer_failure)
            {
                failures.push(failure);
            }
        }
        failures
    }

    pub fn visit(&self, f: &mut impl FnMut(Ref)) {
        match *self {
            Self::Jump(edge) => f(Ref::Edge(edge)),
            Self::Branch { condition, yes, no } => {
                f(Ref::Value(condition));
                f(Ref::Edge(yes));
                f(Ref::Edge(no));
            }
            Self::Switch {
                value,
                arms,
                otherwise,
            } => {
                f(Ref::Value(value));
                f(Ref::SwitchArms(arms));
                f(Ref::Edge(otherwise));
            }
            Self::Call {
                call,
                normal,
                failure,
            } => {
                f(Ref::Call(call));
                if let Some(normal) = normal {
                    f(Ref::Edge(normal));
                }
                f(Ref::Failure(failure));
            }
            Self::Return(value) => {
                if let Some(value) = value {
                    f(Ref::Value(value));
                }
            }
            Self::Raise(failure) => f(Ref::Failure(failure)),
            Self::Recover {
                arena,
                caught,
                failed,
            } => {
                f(Ref::Value(arena));
                f(Ref::Edge(caught));
                f(Ref::Edge(failed));
            }
            Self::Isolate { start, caught } => {
                f(Ref::Value(start));
                f(Ref::Edge(caught));
            }
            Self::Propagate
            | Self::RejectTransfer
            | Self::Unreachable
            | Self::InvariantFault { .. } => {}
        }
    }
}

impl DestinationClass {
    fn visit(&self, f: &mut impl FnMut(Ref)) {
        if let Self::Arena(slot) = self {
            f(Ref::Slot(*slot));
        }
    }
}

impl Placement {
    fn visit(&self, f: &mut impl FnMut(Ref)) {
        match *self {
            Self::CallerDestination | Self::Fixed(StorageOwner::Static) => {}
            Self::Fixed(StorageOwner::Heap(domain)) => f(Ref::TaskDomain(domain)),
            Self::Fixed(StorageOwner::LocalRegion(region)) => f(Ref::Region(region)),
            Self::Fixed(StorageOwner::Arena(arena)) => f(Ref::Arena(arena)),
            Self::Fixed(StorageOwner::Frozen(shared)) => f(Ref::Shared(shared)),
        }
    }
}

/// A schema-level rejection: wrong stage, stale versions, foreign indices or an
/// undecodable snapshot. Semantic validity is the stage validators' job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaError {
    StageMismatch {
        expected: StageKind,
        found: StageKind,
    },
    /// A version or digest that names another input, table or format.
    Stale(&'static str),
    /// An index outside the arena it names (from another unit or table).
    ForeignId {
        kind: &'static str,
        index: u32,
        bound: usize,
    },
    /// A pool range outside its pool.
    ForeignRange {
        kind: &'static str,
        start: u32,
        len: u32,
        bound: usize,
    },
    /// The parallel block/terminator arenas disagree, or a remap is not a bijection.
    Malformed(&'static str),
    Decode(hirsel_codec::DecodeError),
}

impl<S: Stage> LirUnit<S> {
    /// Check stage, versions, target identity and that every index and
    /// pool range lies inside the arena it names. This is referential integrity
    /// only; it never substitutes for a stage validator.
    pub fn check_identity(&self, layouts: &crate::LayoutTable) -> Result<(), SchemaError> {
        if self.header.stage != S::KIND {
            return Err(SchemaError::StageMismatch {
                expected: S::KIND,
                found: self.header.stage,
            });
        }
        if self.header.lir_version != crate::LIR_VERSION {
            return Err(SchemaError::Stale("LIR version"));
        }
        if layouts.layout_version != crate::LAYOUT_VERSION
            || layouts.abi_version != crate::ABI_VERSION
        {
            return Err(SchemaError::Stale("layout table version"));
        }
        if self.header.target != layouts.target {
            return Err(SchemaError::Stale("layout table target"));
        }
        if self.blocks.len() != self.terminators.len() {
            return Err(SchemaError::Malformed(
                "blocks and terminators differ in length",
            ));
        }
        let t = &self.tables;
        for site in t
            .source_anchors
            .iter()
            .map(|a| a.site)
            .chain(t.cleanup_anchors.iter().map(|a| a.site))
        {
            if site.start > site.end {
                return Err(SchemaError::Malformed("reversed source range"));
            }
            if site.source != u32::MAX && site.source >= self.header.source_count {
                return Err(SchemaError::ForeignId {
                    kind: "source selector",
                    index: site.source,
                    bound: self.header.source_count as usize,
                });
            }
        }
        let p = &self.pools;
        let id = |kind, index: u32, bound: usize| {
            if (index as usize) < bound {
                Ok(())
            } else {
                Err(SchemaError::ForeignId { kind, index, bound })
            }
        };
        let range = |kind, start: u32, len: u32, bound: usize| {
            if (start as usize)
                .checked_add(len as usize)
                .is_some_and(|end| end <= bound)
            {
                Ok(())
            } else {
                Err(SchemaError::ForeignRange {
                    kind,
                    start,
                    len,
                    bound,
                })
            }
        };
        let check = |r: Ref| -> Result<(), SchemaError> {
            match r {
                Ref::Value(v) => id("value", v.0, self.values.len()),
                Ref::Slot(v) => id("slot", v.0, self.slots.len()),
                Ref::Address(v) => id("address", v.0, self.addresses.len()),
                Ref::Block(v) => id("block", v.0, self.blocks.len()),
                Ref::Type(v) => id("type", v.0, self.types.len()),
                Ref::Layout(v) => id("layout", v.0, layouts.layouts.len()),
                Ref::Abi(v) => id("fn abi", v.0, layouts.fn_abis.len()),
                Ref::Call(v) => id("call", v.0, t.calls.len()),
                Ref::Construct(v) => id("construct", v.0, t.constructs.len()),
                Ref::Closure(v) => id("closure", v.0, t.closures.len()),
                Ref::Constant(v) => id("constant", v.0, t.constants.len()),
                Ref::Symbol(v) => id("symbol", v.0, t.symbols.len()),
                Ref::Operation(v) => id("operation", v.0, t.operations.len()),
                Ref::Loan(v) => id("loan", v.0, t.loans.len()),
                Ref::Failure(v) => id("failure", v.0, t.failures.len()),
                Ref::Edge(v) => id("edge", v.0, t.edges.len()),
                Ref::Mask(v) => id("mask", v.0, t.masks.len()),
                Ref::SourceAnchor(v) => id("source anchor", v.0, t.source_anchors.len()),
                Ref::CleanupAnchor(v) => id("cleanup anchor", v.0, t.cleanup_anchors.len()),
                Ref::Region(v) => id("region", v.0, t.regions.len()),
                Ref::Arena(v) => id("arena", v.0, t.arenas.len()),
                Ref::Shared(v) => id("shared", v.0, t.shared.len()),
                Ref::TaskDomain(v) => id("task domain", v.0, t.task_domains as usize),
                Ref::Scope(v) => id("scope", v.0, self.scopes.len()),
                Ref::Codec(v) => id("codec", v.0, t.codecs.len()),
                Ref::SealDescriptor(v) => id("seal descriptor", v.0, t.seal_descriptors.len()),
                Ref::PayloadBinding(v) => id(
                    "payload binding",
                    v.0,
                    t.seal_payloads
                        .iter()
                        .map(|table| table.bindings.len())
                        .sum(),
                ),
                Ref::Values(r) => range("values", r.start, r.len, p.values.len()),
                Ref::Addresses(r) => range("addresses", r.start, r.len, p.addresses.len()),
                Ref::Projections(r) => range("projections", r.start, r.len, p.projections.len()),
                Ref::SwitchArms(r) => range("switch arms", r.start, r.len, p.switch_arms.len()),
                Ref::CallArguments(r) => {
                    range("call arguments", r.start, r.len, p.call_arguments.len())
                }
                Ref::Fields(r) => range("fields", r.start, r.len, p.fields.len()),
                Ref::Captures(r) => range("captures", r.start, r.len, p.captures.len()),
                Ref::MaskWords(r) => range("mask words", r.start, r.len, p.mask_words.len()),
                Ref::Instructions(r) => {
                    range("instructions", r.start, r.len, self.instructions.len())
                }
                Ref::ReceiptInputs(r) => {
                    range("receipt inputs", r.start, r.len, p.receipt_inputs.len())
                }
            }
        };
        let mut refs = Vec::new();
        let mut push = |r: Ref| refs.push(r);
        push(Ref::Block(self.entry));
        push(Ref::Abi(self.header.abi));
        self.parameters.iter().for_each(|&v| push(Ref::Value(v)));
        self.captures.iter().for_each(|&v| push(Ref::Value(v)));
        self.scopes
            .iter()
            .filter_map(|s| s.parent)
            .for_each(|p| push(Ref::Scope(p)));
        self.types
            .iter()
            .for_each(|ty| push(Ref::Layout(ty.layout)));
        for value in &self.values {
            push(Ref::Type(value.ty));
            if let Holding::Borrowed(loan) = value.holding {
                push(Ref::Loan(loan));
            }
        }
        for slot in &self.slots {
            push(Ref::Type(slot.ty));
            push(Ref::Scope(slot.scope));
        }
        for address in &self.addresses {
            match address.base {
                AddressBase::Slot(slot) => push(Ref::Slot(slot)),
                AddressBase::Value(value) => push(Ref::Value(value)),
                AddressBase::Address(base) => push(Ref::Address(base)),
            }
            push(Ref::Projections(address.projection));
            push(Ref::Type(address.ty));
        }
        for block in &self.blocks {
            push(Ref::Values(block.parameters));
            push(Ref::Instructions(block.instructions));
            push(Ref::SourceAnchor(block.anchor));
        }
        self.instructions
            .iter()
            .for_each(|inst| inst.visit(&mut push));
        self.terminators
            .iter()
            .for_each(|term| term.visit(&mut push));
        if let Some(entry) = &t.receipt_entry {
            entry.visit(&mut push);
        }
        for payloads in &t.seal_payloads {
            push(Ref::SealDescriptor(payloads.descriptor));
            for binding in &payloads.bindings {
                binding
                    .values
                    .iter()
                    .for_each(|&value| push(Ref::Value(value)));
            }
        }
        for descriptor in &t.seal_descriptors {
            descriptor
                .layouts()
                .for_each(|layout| push(Ref::Layout(layout)));
        }
        for call in &t.calls {
            match call.callee {
                Callee::Direct(symbol) => push(Ref::Symbol(symbol)),
                Callee::Indirect(value) => push(Ref::Value(value)),
                Callee::Operation(operation) => push(Ref::Operation(operation)),
                Callee::Bound { code, closure } => {
                    push(Ref::Symbol(code));
                    push(Ref::Value(closure));
                }
            }
            push(Ref::Abi(call.abi));
            push(Ref::CallArguments(call.arguments));
            if let Some(receipts) = &call.receipts {
                receipts.visit(&mut push);
            }
            if let Some(result) = call.result {
                push(Ref::Value(result));
            }
            if let Some(destination) = &call.destination {
                destination.visit(&mut push);
            }
            if let Some(failure) = call.pretransfer_failure {
                push(Ref::Failure(failure));
            }
            push(Ref::SourceAnchor(call.anchor));
        }
        for site in &t.constructs {
            match site.constructor {
                ConstructorKind::Record(symbol)
                | ConstructorKind::Variant {
                    definition: symbol,
                    variant: _,
                } => push(Ref::Symbol(symbol)),
                ConstructorKind::Tuple | ConstructorKind::Sequence | ConstructorKind::Box => {}
            }
            push(Ref::Layout(site.layout));
            push(Ref::Fields(site.fields));
            site.destination.visit(&mut push);
            if let Some(placement) = &site.placement {
                placement.visit(&mut push);
            }
            push(Ref::SourceAnchor(site.anchor));
        }
        for site in &t.closures {
            if let Some(receipts) = &site.receipts {
                receipts.visit(&mut push);
            }
            if let Some(code) = site.code {
                push(Ref::Symbol(code.symbol));
                push(Ref::Abi(code.abi));
            }
            push(Ref::Layout(site.environment));
            push(Ref::Captures(site.captures));
            site.destination.visit(&mut push);
            if let Some(placement) = &site.placement {
                placement.visit(&mut push);
            }
            push(Ref::SourceAnchor(site.anchor));
        }
        for constant in &t.constants {
            push(Ref::Type(constant.ty));
            if let ConstantValue::Descriptor(layout) = constant.value {
                push(Ref::Layout(layout));
            }
        }
        t.loans.iter().for_each(|l| push(Ref::Address(l.address)));
        for failure in &t.failures {
            push(Ref::Edge(failure.cleanup));
            push(Ref::SourceAnchor(failure.anchor));
            push(Ref::Values(failure.dispose));
        }
        for edge in &t.edges {
            push(Ref::Block(edge.target));
            push(Ref::Values(edge.arguments));
        }
        t.masks.iter().for_each(|m| push(Ref::MaskWords(m.words)));
        for region in &t.regions {
            push(Ref::Scope(region.scope));
            push(Ref::SourceAnchor(region.anchor));
        }
        for instance in t.arenas.iter().chain(&t.shared) {
            push(Ref::Slot(instance.slot));
        }
        for flag in &t.init_flags {
            push(Ref::Value(flag.value));
            push(Ref::Slot(flag.slot));
            push(Ref::Projections(flag.path));
            if let Some(layout) = flag.environment {
                push(Ref::Layout(layout));
            }
        }
        for plan in &t.structural {
            for layout in plan.layouts() {
                push(Ref::Layout(layout));
            }
        }
        for plan in &t.codecs {
            for node in &plan.nodes {
                push(Ref::Layout(node.layout()));
                if let crate::codec::CodecNode::Text { storage, .. } = node {
                    push(Ref::Layout(*storage));
                }
            }
        }
        p.values.iter().for_each(|&v| push(Ref::Value(v)));
        p.addresses.iter().for_each(|&a| push(Ref::Address(a)));
        for step in &p.projections {
            match *step {
                ProjectionStep::Payload
                | ProjectionStep::Field(_)
                | ProjectionStep::VariantField { .. } => {}
                ProjectionStep::Index(value) | ProjectionStep::Slot(value) => {
                    push(Ref::Value(value));
                }
                ProjectionStep::FlatAt { index, offset } => {
                    push(Ref::Value(index));
                    push(Ref::Value(offset));
                }
            }
        }
        p.switch_arms
            .iter()
            .for_each(|arm| push(Ref::Edge(arm.edge)));
        for argument in &p.call_arguments {
            match argument.value {
                ArgumentValue::Value(value) => push(Ref::Value(value)),
                ArgumentValue::Address { address, version } => {
                    push(Ref::Address(address));
                    push(Ref::Value(version));
                }
            }
        }
        p.fields.iter().for_each(|f| push(Ref::Value(f.value)));
        p.captures.iter().for_each(|c| push(Ref::Value(c.value)));
        for input in &p.receipt_inputs {
            push(Ref::Value(input.original));
            input.receipt.iter().for_each(|&r| push(Ref::Value(r)));
        }
        refs.into_iter().try_for_each(check)
    }
}

snapshot_enum!(ValueRepr {
    Unit,
    Scalar(scalar),
    Block,
    Uncounted,
    Aggregate,
    Callable,
    Handle,
    Token,
    RegionMark,
    Arena,
    InitMask { fields },
    PayloadReceipt,
    Hole,
});
snapshot_struct!(LirType { repr, layout });
snapshot_enum!(Holding {
    Plain,
    Owned,
    Borrowed(loan)
});
snapshot_struct!(ValueInfo { ty, holding });
snapshot_enum!(SlotKind {
    Parameter(ordinal),
    Local,
    Temporary,
    Capture(ordinal),
    ReturnSlot,
    LoopCarried(ordinal),
    Displaced,
    ReceiptParameter(ordinal),
    ReceiptCapture(ordinal),
    ReceiptResult,
});
snapshot_struct!(SlotInfo { ty, kind, scope });
snapshot_struct!(ScopeInfo { parent });
snapshot_enum!(ProjectionStep {
    Payload,
    Field(field),
    Index(value),
    VariantField { variant, field },
    Slot(value),
    FlatAt { index, offset }
});
snapshot_enum!(AddressBase {
    Slot(slot),
    Value(value),
    Address(address)
});
snapshot_struct!(AddressInfo {
    source_place,
    unchecked,
    base,
    projection,
    ty
});
snapshot_enum!(UnaryOp { Neg, Not, BitNot });
snapshot_enum!(BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    DivFloor,
    Mod,
    DivExact,
    Min,
    Max,
    MinNumber,
    MaxNumber
});
snapshot_enum!(ConvertMode {
    Widen,
    Truncate,
    TryNarrow,
    FloatToI32,
    IntToFloat,
    Reinterpret,
    FloatResize
});
snapshot_enum!(ArithMode {
    Wrapping,
    Saturating,
    Checked
});
snapshot_enum!(ScalarUnaryOp {
    Sqrt,
    Floor,
    Ceil,
    Trunc,
    Round,
    Abs,
    LeadingZeros,
    TrailingZeros,
    CountOnes,
    SwapBytes,
    ReverseBits
});
snapshot_enum!(CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge
});
snapshot_enum!(ReadMode { Borrow, Take, Bits });
snapshot_enum!(OriginKind { Alias, Fresh });
snapshot_enum!(MaskRef {
    All,
    Static(mask),
    Dynamic(value)
});
snapshot_enum!(UnavailableFeature {
    Shared,
    Arena,
    Task,
    Channel,
    Mutex,
    Atomic,
    ReleaseRegion,
    CloseArena,
    JoinTasks,
    Shape,
});
snapshot_enum!(FlatFieldOp {
    Open,
    Reserve { capacity },
    Push { value, spare }
});
snapshot_enum!(Inst {
    Const { result, constant },
    FunctionRef { result, symbol },
    ReadStatic { result, symbol },
    Unary { result, op, operand, failure },
    Binary { result, op, left, right, failure },
    Compare { result, op, left, right, layout, failure },
    Convert { result, flag, operand, mode },
    IntArith { result, flag, op, mode, left, right },
    MulAdd { result, left, right, addend },
    ScalarUnary { result, op, operand },
    Check { condition, failure },
    StorageLive { slot },
    StorageDead { slot, if_live },
    EndScope { scope },
    Address { address, failure },
    Read { result, address, reaching, mode },
    Copy { result, address, reaching, failure },
    FlatCopy { result, address, layout, failure },
    FlatView { result, address },
    FlatLength { result, address },
    FlatNext { result, address },
    SeqLen { result, value },
    SeqCapacity { result, value },
    SeqReserve { address, capacity, version },
    SeqSetLen { address, len, version },
    SeqAppend { address, value, version },
    ArenaPush { address, count, version, failure },
    FlatAppend { address, construct, version },
    FlatField { address, field, op, version },
    FlatClose { address, construct, field, version },
    Hash { result, value, layout, failure },
    Encode { result, value, codec },
    Decode { result, value, codec },
    Recast { result, value },
    CopyValue { result, value },
    UniqueCopy { result, value, construct, deep, failure },
    Freeze { result, value },
    Thaw { result, value },
    Store { address, value, version },
    Replace { address, value, displaced, version, displaced_version, if_live, failure },
    UpdateMask { result, mask, field, initialized },
    Construct { result, construct, failure },
    Closure { result, closure, failure },
    Weaken { result, value },
    BeginLoan { loan, address, mutable, if_inactive },
    EndLoan { loan, if_active },
    BindOrigin { source, place, kind },
    BindFields { source, fields },
    EndOrigin { place },
    Drop { address, if_live, anchor, failure },
    Dup { value },
    IsUnique { result, value },
    IsWritable { result, value },
    Share { value },
    Release { value, layout, live, anchor, failure },
    Reset { result, value, layout, live, anchor, failure },
    Reuse { result, token, construct, kept, failure },
    Dispose { token },
    TokenNone { result },
    HoleOpen { result, hole, construct, open, failure },
    HoleFill { hole, value },
    HoleAbandon { hole, root },
    RegionEnter { result, region },
    RegionReserve { result, mark, layout, extent, failure },
    RegionPublish { value },
    RegionExit { mark, region, failure },
    ArenaOpen { result, budget },
    ArenaToken {
        result,
        mark,
        budget
    },
    Defuse { address },
    FinalizeBegin { address, anchor },
    FinalizeEnd { address, anchor },
    Forget { value },
    FrameEnter { failure },
    FrameLeave,
    Unavailable { feature, anchor },
    SliceView { result, value, start, end, failure },
    SealArena { result, grant, root, shape, failure, origin },
    BorrowSealedRoot { result, owner, loan, shape },
    ArenaInventory { arena, allocation, value, shape, previous, receipt },
    PayloadReceipt { result, value, shape, binding, inputs, arena },
});
snapshot_struct!(Edge { target, arguments });
snapshot_struct!(SwitchArm { value, edge });
snapshot_struct!(FailureEdge {
    cleanup,
    rule,
    anchor,
    dispose
});
snapshot_enum!(Terminator {
    Jump(edge),
    Branch { condition, yes, no },
    Switch { value, arms, otherwise },
    Call { call, normal, failure },
    Return(value),
    Raise(failure),
    Propagate,
    Recover {
        arena,
        caught,
        failed
    },
    Isolate { start, caught },
    Unreachable,
    InvariantFault { reason },
    RejectTransfer,
});
snapshot_enum!(BlockKind {
    Body,
    Cleanup,
    FailureCleanup
});
snapshot_struct!(Block {
    kind,
    parameters,
    instructions,
    anchor
});
snapshot_enum!(DestinationClass {
    Heap,
    Arena(slot),
    Caller(parameters),
    Region,
    Inferred,
});
snapshot_enum!(StorageOwner {
    Heap(domain),
    LocalRegion(region),
    Arena(arena),
    Frozen(shared),
    Static,
});
snapshot_enum!(Placement {
    Fixed(owner),
    CallerDestination
});
snapshot_enum!(ConstructorKind {
    Tuple,
    Record(definition),
    Variant { definition, variant },
    Sequence,
    Box,
});
snapshot_struct!(FieldValue { field, value });
snapshot_struct!(ConstructSite {
    arena_no_drop,
    tracked,
    constructor,
    layout,
    fields,
    destination,
    placement,
    anchor
});
snapshot_enum!(CaptureUse {
    Borrow,
    Take,
    Copy,
    Mut
});
snapshot_struct!(CaptureValue { value, usage });
snapshot_struct!(ClosureCode { symbol, abi });
snapshot_struct!(ClosureSite {
    scoped,
    code,
    environment,
    captures,
    destination,
    placement,
    anchor
} trailing_optional receipts);
snapshot_enum!(Callee {
    Direct(symbol),
    Indirect(value),
    Operation(operation),
    Bound { code, closure },
});
snapshot_enum!(ArgumentValue {
    Value(value),
    Address { address, version }
});
snapshot_struct!(CallArgument {
    parameter,
    passing,
    value
});
snapshot_struct!(ReceiptInput { original, receipt });
snapshot_struct!(CallSite {
    pretransfer_failure,
    forward_caller,
    callee,
    operation_arguments,
    abi,
    arguments,
    result,
    destination,
    declared,
    anchor
} trailing_optional receipts);
snapshot_enum!(ConstantValue {
    Heap,
    Unit,
    Bool(value),
    Integer(value),
    F32(bits),
    F64(bits),
    String(value),
    Bytes(value),
    Descriptor(layout),
});
snapshot_struct!(ConstantData { ty, value });
snapshot_enum!(SymbolKind {
    Function,
    Constant,
    Type,
    Finalizer,
    Shape
});
snapshot_struct!(Symbol {
    kind,
    definition,
    instance
});
snapshot_struct!(LoanInfo { address, mutable });
snapshot_struct!(SiteRange { source, start, end });
snapshot_struct!(SourceAnchor {
    body,
    block,
    statement,
    site
});
snapshot_struct!(CleanupAnchor {
    body,
    cleanup,
    action,
    site
});
snapshot_struct!(FieldMask { fields, words });
// Omit false so ordinary region rows keep their exact previous encoding.
impl hirsel_codec::Snapshot for RegionInstance {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        use hirsel_codec::Snapshot;
        w.open("RegionInstance");
        self.scope.write(w);
        self.anchor.write(w);
        if self.seal_demand {
            true.write(w);
        }
        w.close();
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        use hirsel_codec::Snapshot;
        r.open("RegionInstance")?;
        let scope = ScopeId::read(r)?;
        let anchor = SourceAnchorId::read(r)?;
        let seal_demand = if r.at_close() { false } else { bool::read(r)? };
        r.close()?;
        Ok(Self {
            scope,
            anchor,
            seal_demand,
        })
    }
}
snapshot_struct!(ExternalInstance { slot });
snapshot_struct!(InitFlag {
    value,
    slot,
    path,
    environment
});
snapshot_struct!(SideTables {
    calls,
    constructs,
    closures,
    constants,
    symbols,
    operations,
    loans,
    failures,
    edges,
    masks,
    regions,
    arenas,
    shared,
    task_domains,
    source_anchors,
    cleanup_anchors,
    init_flags,
    codecs,
    structural
} trailing_optional receipt_entry trailing_empty seal_descriptors, seal_payloads);
snapshot_struct!(Pools {
    values,
    addresses,
    projections,
    switch_arms,
    call_arguments,
    fields,
    captures,
    mask_words
} trailing_empty receipt_inputs);
snapshot_enum!(UnitOrigin { Source, Library });
snapshot_struct!(UnitHeader {
    source_count,
    track_caller,
    stage,
    lir_version,
    origin,
    key,
    definition,
    implementation_revision,
    instance,
    target,
    abi
});

/// A unit decodes only as the stage its header names.
impl<S: Stage> hirsel_codec::Snapshot for LirUnit<S> {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        w.open("LirUnit");
        self.header.write(w);
        self.types.write(w);
        self.values.write(w);
        self.slots.write(w);
        self.addresses.write(w);
        self.blocks.write(w);
        self.terminators.write(w);
        self.instructions.write(w);
        self.entry.write(w);
        self.parameters.write(w);
        self.captures.write(w);
        self.scopes.write(w);
        self.tables.write(w);
        self.pools.write(w);
        w.close();
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        use hirsel_codec::Snapshot as Sn;
        let start = r.offset();
        r.open("LirUnit")?;
        let header = UnitHeader::read(r)?;
        if header.stage != S::KIND {
            return Err(r.error(format!(
                "stage mismatch: snapshot is {:?}, expected {:?}",
                header.stage,
                S::KIND
            )));
        }
        crate::construction::record(&header, crate::construction::ValueWork::Decode);
        let unit = LirUnit {
            header,
            types: Sn::read(r)?,
            values: Sn::read(r)?,
            slots: Sn::read(r)?,
            addresses: Sn::read(r)?,
            blocks: Sn::read(r)?,
            terminators: Sn::read(r)?,
            instructions: Sn::read(r)?,
            entry: Sn::read(r)?,
            parameters: Sn::read(r)?,
            captures: Sn::read(r)?,
            scopes: Sn::read(r)?,
            tables: Sn::read(r)?,
            pools: Sn::read(r)?,
            stage: PhantomData,
        };
        r.close()?;
        crate::construction::record(
            &unit.header,
            crate::construction::ValueWork::DecodedBytes((r.offset() - start) as u64),
        );
        Ok(unit)
    }
}
