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
use crate::stage::{InputIdentity, Stage, StageKind};
use hirsel_codec::{snapshot_enum, snapshot_struct};
use hirsel_core_contract::OperationId;
use hirsel_core_contract::Passing;
use hirsel_syntax::{ContentHash, DefPath};
use std::marker::PhantomData;

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
    /// A reference into uncounted storage (region, arena, frozen, static).
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
    /// The payload of a block reference value.
    Value(ValueId),
    Address(AddressId),
}

/// A field-path address. Its checks (bounds, tag) run at `Inst::Address`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AddressInfo {
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
/// (SEM-2: NaN propagates or is ignored, -0 < +0), and `TotalOrder` is IEEE
/// totalOrder after NaN canonicalization as an i32 -1, 0 or 1; none of the
/// float ones fails.
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
    TotalOrder,
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
}

/// How `Inst::IntArith` treats a mathematical result outside its kind
/// (SEM-1's named behaviours, LIR schema revision 6).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ArithMode {
    /// `int.wrapping_add`: the result modulo 2^width.
    Wrapping,
    /// `int.saturating_add`: the nearest bound of the kind.
    Saturating,
    /// `int.checked_add`: the wrapped result, and `flag` (a Bool) true
    /// exactly when it overflowed.
    Checked,
}

/// The deepest operand pair `Inst::Compare` visits before failing with MEM-3
/// Depth: the root pair is depth 0 and each field, element or boxed payload one
/// deeper. It is the interpreter's `structural::MAX_DEPTH` (64 native levels plus
/// 1024 scratch levels), provisional under spec #16; layouts erase distinct
/// wrappers, so numeric Depth parity stays unresolved until #16 rules.
pub const COMPARE_MAX_DEPTH: u64 = 64 + 1024;

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
    /// A `Shape` whose static data is not materialized.
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
    /// (`ArithMode`, LIR schema revision 6). The library names `Add` only,
    /// the one `op` admitted. `flag` is present exactly in `Checked` mode.
    /// Never fails.
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
    /// one credit. Deep (`unique x`): every counted heap block the value
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
    /// Value stage, library bodies only: open an arena that every return and
    /// propagation of the unit closes, and define its token. Placement lowers
    /// it to a region entry and that region's `ArenaToken`.
    ArenaOpen {
        result: ValueId,
    },
    /// The arena token of the region `mark` entered: its record's address.
    ArenaToken {
        result: ValueId,
        mark: ValueId,
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
/// disposal list that runs alongside the original cleanup.
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
    /// C proves every owned child belongs to this same stable arena token.
    /// No child-credit walk or payload drop is scheduled for this construction.
    pub arena_no_drop: bool,
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
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ClosureSite {
    pub code: Option<ClosureCode>,
    pub environment: LayoutId,
    pub captures: PoolRange<CaptureValue>,
    pub destination: DestinationClass,
    pub placement: Option<Placement>,
    pub anchor: SourceAnchorId,
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

/// A call: callee, ABI, arguments, result value and the callee's declared
/// effects (never body-derived public promises).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CallSite {
    pub callee: Callee,
    /// Concrete registry instantiation; read only for Operation callees (LIR 21).
    pub operation_arguments: Vec<crate::operation::OperationArgument>,
    pub abi: FnAbiId,
    pub arguments: PoolRange<CallArgument>,
    pub result: Option<ValueId>,
    pub destination: Option<DestinationClass>,
    pub declared: Effects,
    pub anchor: SourceAnchorId,
}

/// Typed literal bits; floats keep their bit patterns.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum ConstantValue {
    Unit,
    Bool(bool),
    Integer(u128),
    F32(u32),
    F64(u64),
    String(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ConstantData {
    pub ty: LirTypeId,
    pub value: ConstantValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SymbolKind {
    Function,
    Constant,
    Type,
    Finalizer,
    Shape,
}

/// A link-level symbol: definition plus instance digest for specialized code.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
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

/// Byte range of a source site within its definition's source revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SiteRange {
    pub start: u32,
    pub end: u32,
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
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
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
}

impl SideTables {
    /// The persistent live word's environment, if this body consumes captures.
    pub fn environment(&self) -> Option<LayoutId> {
        self.init_flags.iter().find_map(|flag| flag.environment)
    }
}

/// Variable-length operand pools.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Pools {
    pub values: Vec<ValueId>,
    pub addresses: Vec<AddressId>,
    pub projections: Vec<ProjectionStep>,
    pub switch_arms: Vec<SwitchArm>,
    pub call_arguments: Vec<CallArgument>,
    pub fields: Vec<FieldValue>,
    pub captures: Vec<CaptureValue>,
    pub mask_words: Vec<u64>,
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
    pub stage: StageKind,
    pub lir_version: u32,
    pub origin: UnitOrigin,
    pub input: InputIdentity,
    pub definition: DefPath,
    pub implementation_revision: ContentHash,
    /// Digest of the unit's `InstanceKey`.
    pub instance: ContentHash,
    pub target: ContentHash,
    /// Digest of the `LayoutTable` that `LayoutId`/`FnAbiId` index.
    pub layouts: ContentHash,
    pub abi: FnAbiId,
}

/// One function body at stage `S`. Fields are public so tests and adversarial
/// inputs can forge raw units; only validators produce `ValidatedUnit<S>`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
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

impl<S: Stage> LirUnit<S> {
    /// An empty unit whose header names stage `S`.
    pub fn new(mut header: UnitHeader) -> Self {
        header.stage = S::KIND;
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
    Values(PoolRange<ValueId>),
    Addresses(PoolRange<AddressId>),
    Projections(PoolRange<ProjectionStep>),
    SwitchArms(PoolRange<SwitchArm>),
    CallArguments(PoolRange<CallArgument>),
    Fields(PoolRange<FieldValue>),
    Captures(PoolRange<CaptureValue>),
    MaskWords(PoolRange<u64>),
    Instructions(PoolRange<Inst>),
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
            Self::IsUnique { result, value }
            | Self::Weaken { result, value }
            | Self::SeqLen { result, value }
            | Self::SeqCapacity { result, value }
            | Self::Recast { result, value } => {
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
            | Self::Forget { value }
            | Self::RegionPublish { value }
            | Self::Dispose { token: value }
            | Self::TokenNone { result: value }
            | Self::ArenaOpen { result: value } => f(R::Value(value)),
            Self::ArenaToken { result, mark } => {
                f(R::Value(result));
                f(R::Value(mark));
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
            Self::Propagate | Self::Unreachable | Self::InvariantFault { .. } => {}
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
    /// Check stage, versions, layout-table identity and that every index and
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
        if self.header.input.interface_version != crate::INTERFACE_VERSION {
            return Err(SchemaError::Stale("front-end interface version"));
        }
        if layouts.layout_version != crate::LAYOUT_VERSION
            || layouts.abi_version != crate::ABI_VERSION
        {
            return Err(SchemaError::Stale("layout table version"));
        }
        if self.header.layouts != layouts.digest() || self.header.target != layouts.target {
            return Err(SchemaError::Stale("layout table digest"));
        }
        if self.blocks.len() != self.terminators.len() {
            return Err(SchemaError::Malformed(
                "blocks and terminators differ in length",
            ));
        }
        let t = &self.tables;
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
            if let Some(result) = call.result {
                push(Ref::Value(result));
            }
            if let Some(destination) = &call.destination {
                destination.visit(&mut push);
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
        t.constants.iter().for_each(|c| push(Ref::Type(c.ty)));
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
        refs.into_iter().try_for_each(check)
    }
}

/// The binder of one generic actual: its declaring scope and ordinal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ActualBinder {
    Type(hirsel_types::TypeParameterRef),
    Size(hirsel_types::SizeParameterRef),
}

/// The declared callable convention of an instance (borrow/take/mut, once/many).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CallableConvention {
    pub parameters: Vec<Passing>,
    pub multiplicity: crate::abi_types::CallMultiplicity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstanceKeyError {
    /// A type actual whose kind does not match its binder, or vice versa.
    KindMismatch(u32),
    /// A body-scoped parameter or origin: the actual is not complete.
    Incomplete,
    MissingType,
    StructuralCycle,
    RecoveryType,
}

/// Complete identity of one native instance.
///
/// Canonicalization keeps the definition, the body's index within it, each
/// actual's binder and complete type structure, size actuals, the declared
/// callable convention and the environment layout. It erases every actual's
/// relation, brand, uniqueness and sendability at every depth. The
/// definition's implementation revision is not identity: it travels as
/// `UnitHeader::implementation_revision`, so a body edit leaves every
/// caller's call and symbol unchanged. Equality compares the full encoding;
/// hashing uses the digest.
#[derive(Clone, Debug)]
pub struct InstanceKey {
    definition: DefPath,
    body: u32,
    encoding: Vec<u8>,
    digest: ContentHash,
}

impl PartialEq for InstanceKey {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest && self.encoding == other.encoding
    }
}
impl Eq for InstanceKey {}
impl std::hash::Hash for InstanceKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.digest.hash(state);
    }
}

impl InstanceKey {
    /// Canonicalize an instance. `actuals` pairs each binder with its actual,
    /// whose TypeIds belong to `store`. `environment` is the digest of the
    /// closure environment layout for a concrete closure specialization.
    pub fn new(
        definition: &DefPath,
        body: u32,
        actuals: &[(ActualBinder, hirsel_types::GenericArgument)],
        store: &hirsel_types::TypeStore,
        convention: Option<&CallableConvention>,
        environment: Option<ContentHash>,
    ) -> Result<Self, InstanceKeyError> {
        use hirsel_types::{GenericArgument, Size};
        let mut e = KeyEncoder {
            bytes: Vec::new(),
            store,
            active: Vec::new(),
        };
        e.tag("hirsel.instance-key.v3");
        e.path(definition);
        e.number(u64::from(body));
        e.number(actuals.len() as u64);
        for (ordinal, (binder, actual)) in actuals.iter().enumerate() {
            match (binder, actual) {
                (ActualBinder::Type(binder), GenericArgument::Type(ty)) => {
                    e.tag("type");
                    e.binder(*binder)?;
                    e.ty(*ty)?;
                }
                (ActualBinder::Size(binder), GenericArgument::Size(size)) => {
                    e.tag("size");
                    e.binder(*binder)?;
                    match size {
                        Size::Constant(n) => e.number(*n),
                        Size::Parameter(parameter) => {
                            e.tag("size-parameter");
                            e.binder(*parameter)?;
                        }
                    }
                }
                _ => return Err(InstanceKeyError::KindMismatch(ordinal as u32)),
            }
        }
        e.boolean(convention.is_some());
        if let Some(convention) = convention {
            e.number(convention.parameters.len() as u64);
            for passing in &convention.parameters {
                e.tag(match passing {
                    Passing::Borrow => "borrow",
                    Passing::Take => "take",
                    Passing::Mut => "mut",
                });
            }
            e.tag(match convention.multiplicity {
                crate::abi_types::CallMultiplicity::Many => "many",
                crate::abi_types::CallMultiplicity::Once => "once",
            });
        }
        e.boolean(environment.is_some());
        if let Some(environment) = environment {
            e.atom(&environment.0);
        }
        let digest = ContentHash::of_parts("hirsel.instance-key.sha256.v3", [e.bytes.as_slice()]);
        Ok(Self {
            definition: definition.clone(),
            body,
            encoding: e.bytes,
            digest,
        })
    }
    pub fn definition(&self) -> &DefPath {
        &self.definition
    }
    pub fn body(&self) -> u32 {
        self.body
    }
    pub fn encoding(&self) -> &[u8] {
        &self.encoding
    }
    pub fn digest(&self) -> ContentHash {
        self.digest
    }
}

/// Declaration ordinal of a binder's parameter.
trait Ordinal: Copy {
    fn ordinal(self) -> u32;
}
impl Ordinal for hirsel_types::TypeParamId {
    fn ordinal(self) -> u32 {
        self.0
    }
}
impl Ordinal for hirsel_types::SizeParamId {
    fn ordinal(self) -> u32 {
        self.0
    }
}
impl Ordinal for hirsel_types::ParamId {
    fn ordinal(self) -> u32 {
        self.0
    }
}

/// Length-framed canonical bytes, as in `hirsel_types::encoding`, with the
/// four erased mode axes never written.
struct KeyEncoder<'a> {
    bytes: Vec<u8>,
    store: &'a hirsel_types::TypeStore,
    active: Vec<hirsel_types::TypeId>,
}

impl KeyEncoder<'_> {
    fn atom(&mut self, b: &[u8]) {
        self.bytes
            .extend_from_slice(&(b.len() as u64).to_le_bytes());
        self.bytes.extend_from_slice(b);
    }
    fn tag(&mut self, s: &str) {
        self.atom(s.as_bytes());
    }
    fn number(&mut self, n: u64) {
        self.atom(&n.to_le_bytes());
    }
    fn boolean(&mut self, b: bool) {
        self.atom(&[u8::from(b)]);
    }
    fn path(&mut self, p: &DefPath) {
        self.tag(&p.0.package.name);
        self.tag(&p.0.package.version);
        self.number(p.0.modules.len() as u64);
        for m in &p.0.modules {
            self.tag(m);
        }
        self.tag(&p.0.name);
    }
    fn name_site(&mut self, site: hirsel_types::NameSite) {
        use hirsel_types::NameSite;
        let (kind, index) = match site {
            NameSite::Expression(id) => ("expression", id.0),
            NameSite::Pattern(id) => ("pattern", id.0),
            NameSite::Type(id) => ("type", id.0),
            NameSite::Statement(id) => ("statement", id.0),
            NameSite::Item(id) => ("item", id.0),
        };
        self.tag(kind);
        self.number(u64::from(index));
    }
    /// A binder names its declaring scope (body site or relative depth) and ordinal.
    fn binder<P: Ordinal>(
        &mut self,
        reference: hirsel_types::BinderRef<P>,
    ) -> Result<(), InstanceKeyError> {
        match reference {
            hirsel_types::BinderRef::Bound { depth, parameter } => {
                self.tag("bound");
                self.number(u64::from(depth));
                self.number(u64::from(parameter.ordinal()));
            }
            hirsel_types::BinderRef::Body { scope, parameter } => {
                self.tag("body");
                self.name_site(scope);
                self.number(u64::from(parameter.ordinal()));
            }
        }
        Ok(())
    }
    /// A type parameter inside an actual is complete only if a binder within
    /// the actual (a generic function type) declares it.
    fn inner_parameter<P: Ordinal>(
        &mut self,
        reference: hirsel_types::BinderRef<P>,
    ) -> Result<(), InstanceKeyError> {
        match reference {
            hirsel_types::BinderRef::Bound { .. } => self.binder(reference),
            hirsel_types::BinderRef::Body { .. } => Err(InstanceKeyError::Incomplete),
        }
    }
    fn arguments(
        &mut self,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<(), InstanceKeyError> {
        use hirsel_types::{GenericArgument, Size};
        self.number(arguments.len() as u64);
        for argument in arguments {
            match argument {
                GenericArgument::Type(ty) => {
                    self.tag("type");
                    self.ty(*ty)?;
                }
                GenericArgument::Size(Size::Constant(n)) => {
                    self.tag("constant");
                    self.number(*n);
                }
                GenericArgument::Size(Size::Parameter(p)) => {
                    self.tag("size-parameter");
                    self.inner_parameter(*p)?;
                }
            }
        }
        Ok(())
    }
    fn authority(&mut self, a: &hirsel_types::AuthoritySummary) -> Result<(), InstanceKeyError> {
        use hirsel_types::{AuthoritySummary, AuthorityTerm};
        match a {
            AuthoritySummary::Unknown => self.tag("unknown"),
            AuthoritySummary::Known(terms) => {
                self.tag("known");
                let mut terms: Vec<_> = terms.iter().collect();
                terms.sort();
                terms.dedup();
                self.number(terms.len() as u64);
                for term in terms {
                    match term {
                        AuthorityTerm::Capability(p) => {
                            self.tag("capability");
                            self.path(p);
                        }
                        AuthorityTerm::Parameter(p) => {
                            self.tag("parameter");
                            self.inner_parameter(*p)?;
                        }
                        AuthorityTerm::TypeParameter(p) => {
                            self.tag("type-parameter");
                            self.inner_parameter(*p)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn type_parameters(
        &mut self,
        parameters: &[hirsel_types::TypeParameterRef],
    ) -> Result<(), InstanceKeyError> {
        let mut parameters = parameters.to_vec();
        parameters.sort();
        parameters.dedup();
        self.number(parameters.len() as u64);
        for p in parameters {
            self.inner_parameter(p)?;
        }
        Ok(())
    }
    fn ty(&mut self, id: hirsel_types::TypeId) -> Result<(), InstanceKeyError> {
        use hirsel_types::{
            AllocationTarget, DropAllocation, ErasedCaptures, GenericParameter, Multiplicity,
            TypeKind,
        };
        // Compare row identity without `TypeStore::get`, which asserts on foreign IDs.
        if self.store.id(id.index()) != Some(id) {
            return Err(InstanceKeyError::MissingType);
        }
        let store = self.store;
        let t = store.get(id).ok_or(InstanceKeyError::MissingType)?;
        if self.active.contains(&id) {
            return Err(InstanceKeyError::StructuralCycle);
        }
        self.active.push(id);
        match &t.kind {
            TypeKind::Primitive(p) => {
                use hirsel_types::Primitive as P;
                self.tag("primitive");
                self.tag(match p {
                    P::Bool => "bool",
                    P::U8 => "u8",
                    P::U16 => "u16",
                    P::U32 => "u32",
                    P::U64 => "u64",
                    P::U128 => "u128",
                    P::I8 => "i8",
                    P::I16 => "i16",
                    P::I32 => "i32",
                    P::I64 => "i64",
                    P::I128 => "i128",
                    P::F32 => "f32",
                    P::F64 => "f64",
                    P::String => "String",
                    P::Bytes => "Bytes",
                    P::Never => "never",
                });
            }
            TypeKind::Tuple(items) => {
                self.tag("tuple");
                self.number(items.len() as u64);
                for item in items.iter() {
                    self.ty(*item)?;
                }
            }
            TypeKind::Parameter(p) => {
                self.tag("parameter");
                self.inner_parameter(*p)?;
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                self.tag("named");
                self.path(definition);
                self.arguments(arguments)?;
            }
            TypeKind::Record(fields) => {
                self.tag("record");
                self.number(fields.len() as u64);
                for field in fields.iter() {
                    self.tag(&field.name);
                    self.boolean(field.growable);
                    self.ty(field.ty)?;
                }
            }
            TypeKind::Variant(variants) => {
                self.tag("variant");
                self.number(variants.len() as u64);
                for variant in variants.iter() {
                    self.tag(&variant.name);
                    self.number(variant.fields.len() as u64);
                    for (name, ty) in variant.fields.iter() {
                        self.boolean(name.is_some());
                        if let Some(name) = name {
                            self.tag(name);
                        }
                        self.ty(*ty)?;
                    }
                }
            }
            TypeKind::Function(f) => {
                self.tag("function");
                self.number(f.state_promises.len() as u64);
                for promise in &f.state_promises {
                    self.inner_parameter(promise.selector.parameter)?;
                    self.number(promise.selector.fields.len() as u64);
                    for field in &promise.selector.fields {
                        self.number(field.0 as u64);
                    }
                    self.number(u64::from(promise.advance_only));
                    self.number(u64::from(promise.preserve_captures));
                }
                self.number(f.generics.len() as u64);
                for g in f.generics.iter() {
                    match g {
                        GenericParameter::Type { bounds, .. } => {
                            self.tag("type");
                            use hirsel_types::Bound as Bd;
                            let mut bounds: Vec<_> = bounds
                                .iter()
                                .map(|b| match b {
                                    Bd::Plain => "plain",
                                    Bd::Flat => "flat",
                                    Bd::Data => "data",
                                    Bd::Key => "key",
                                    Bd::Sendable => "sendable",
                                    Bd::Inert => "inert",
                                    Bd::Copyable => "copyable",
                                    Bd::NoallocDrop => "noalloc_drop",
                                    Bd::Serializable => "serializable",
                                })
                                .collect();
                            bounds.sort();
                            bounds.dedup();
                            self.number(bounds.len() as u64);
                            for b in bounds {
                                self.tag(b);
                            }
                        }
                        GenericParameter::Size { .. } => self.tag("size"),
                    }
                }
                self.number(f.parameters.len() as u64);
                for p in f.parameters.iter() {
                    self.tag(match Passing::from(p.passing) {
                        Passing::Borrow => "borrow",
                        Passing::Take => "take",
                        Passing::Mut => "mut",
                    });
                    self.ty(p.ty)?;
                }
                self.ty(f.result)?;
                self.boolean(f.error.is_some());
                if let Some(error) = f.error {
                    self.ty(error)?;
                }
                self.boolean(f.allocation == hirsel_types::Allocation::NoAlloc);
                self.authority(&f.authority)?;
                match &f.uses {
                    hirsel_types::Uses::Unrestricted => self.tag("unrestricted"),
                    hirsel_types::Uses::Only(a) => {
                        self.tag("only");
                        self.authority(a)?;
                    }
                }
                self.number(f.contracts.len() as u64);
                for c in f.contracts.iter() {
                    self.number(u64::from(c.ordinal));
                    self.atom(&c.code_hash.0);
                }
                self.boolean(f.foreign.is_some());
            }
            TypeKind::Box(inner) => {
                self.tag("box");
                self.ty(*inner)?;
            }
            TypeKind::Builtin { kind, arguments } => {
                use hirsel_types::BuiltinType as B;
                self.tag("builtin");
                self.tag(match kind {
                    B::Array => "Array",
                    B::List => "List",
                    B::Map => "Map",
                    B::Set => "Set",
                    B::SortedMap => "SortedMap",
                    B::Inline => "Inline",
                    B::Simd => "Simd",
                    B::Packed => "Packed",
                    B::Shared => "Shared",
                    B::Slots => "Slots",
                    B::SlotId => "SlotId",
                    B::Region => "Region",
                    B::Iter => "Iter",
                    B::Task => "Task",
                    B::Sender => "Sender",
                    B::Receiver => "Receiver",
                    B::Mutex => "Mutex",
                    B::Atomic => "Atomic",
                    B::Ptr => "Ptr",
                    B::FnPtr => "FnPtr",
                    B::Shape => "Shape",
                });
                self.arguments(arguments)?;
            }
            TypeKind::Error => return Err(InstanceKeyError::RecoveryType),
        }
        // Kept: multiplicity, drop effect, allocation target, erased captures.
        // Erased: relation, brand, uniqueness, sendability.
        self.tag(match t.modes.multiplicity {
            Multiplicity::Many => "many",
            Multiplicity::Once => "once",
        });
        match &t.drop_allocation {
            DropAllocation::NoAlloc => self.tag("drop-noalloc"),
            DropAllocation::MayAllocate => self.tag("drop-may-allocate"),
            DropAllocation::DependsOn(ps) => {
                self.tag("drop-depends");
                self.type_parameters(ps)?;
            }
        }
        self.boolean(t.allocation_target == Some(AllocationTarget::Heap));
        match &t.erased_captures {
            None => self.tag("captures-none"),
            Some(ErasedCaptures::Known(a)) => {
                self.tag("captures-known");
                self.authority(a)?;
            }
            Some(ErasedCaptures::FromArguments(ps)) => {
                self.tag("captures-from");
                let mut ps = ps.to_vec();
                ps.sort();
                ps.dedup();
                self.number(ps.len() as u64);
                for p in ps {
                    self.inner_parameter(p.parameter)?;
                    self.number(p.fields.len() as u64);
                    for field in &p.fields {
                        self.number(field.0 as u64);
                    }
                }
            }
        }
        self.active.pop();
        Ok(())
    }
}

/// A parameter of the instance: source ordinal, instance ordinal, convention.
/// The remapping must be a bijection on ordinals.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ParameterView {
    pub source: u32,
    pub instance: u32,
    pub passing: Passing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CaptureView {
    pub ordinal: u32,
    pub usage: CaptureUse,
}

/// Owned substitution of one instance; TypeIds belong to `store` only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Substitution {
    pub store: hirsel_types::TypeStore,
    pub types: Vec<(hirsel_types::TypeParameterRef, hirsel_types::TypeId)>,
    pub sizes: Vec<(hirsel_types::SizeParameterRef, u64)>,
}

/// A shared generic TIR body viewed as one instance. The body is
/// never duplicated or edited; owner, capture, result and parameter
/// conventions are imported together.
#[derive(Clone, Debug)]
pub struct InstanceView {
    pub key: InstanceKey,
    pub definition: hirsel_tir::DefinitionRef,
    pub body: hirsel_tir::BodyId,
    pub owner: crate::abi_types::CallableShape,
    pub substitution: Substitution,
    pub parameters: Vec<ParameterView>,
    pub captures: Vec<CaptureView>,
    /// The result is written to the hidden caller destination.
    pub caller_destination: bool,
}

impl InstanceView {
    /// Referential integrity: the body exists, every substituted TypeId belongs
    /// to the view's own store, and parameter remapping is a bijection.
    pub fn check_identity(&self) -> Result<(), SchemaError> {
        let bodies = self.definition.bodies.len();
        if self.body.0 as usize >= bodies {
            return Err(SchemaError::ForeignId {
                kind: "TIR body",
                index: self.body.0,
                bound: bodies,
            });
        }
        if &self.definition.path != self.key.definition() || self.key.body() != self.body.0 {
            return Err(SchemaError::Stale("instance key names another body"));
        }
        let store = &self.substitution.store;
        for (_, ty) in &self.substitution.types {
            if store.id(ty.index()) != Some(*ty) {
                return Err(SchemaError::ForeignId {
                    kind: "TypeId",
                    index: ty.index() as u32,
                    bound: store.len(),
                });
            }
        }
        let n = self.parameters.len();
        let mut sources = vec![false; n];
        let mut targets = vec![false; n];
        for p in &self.parameters {
            let (s, t) = (p.source as usize, p.instance as usize);
            if s >= n || t >= n || std::mem::replace(&mut sources[s], true) {
                return Err(SchemaError::Malformed(
                    "parameter remapping is not a bijection",
                ));
            }
            if std::mem::replace(&mut targets[t], true) {
                return Err(SchemaError::Malformed(
                    "parameter remapping is not a bijection",
                ));
            }
        }
        Ok(())
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
    MaxNumber,
    TotalOrder
});
snapshot_enum!(ConvertMode {
    Widen,
    Truncate,
    TryNarrow,
    FloatToI32,
    IntToFloat,
    Reinterpret
});
snapshot_enum!(ArithMode {
    Wrapping,
    Saturating,
    Checked
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
    FlatAppend { address, construct, version },
    FlatField { address, field, op, version },
    FlatClose { address, construct, field, version },
    Hash { result, value, layout, failure },
    Recast { result, value },
    UniqueCopy { result, value, construct, deep, failure },
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
    Release { value, layout, live, anchor, failure },
    Reset { result, value, layout, live, anchor, failure },
    Reuse { result, token, construct, kept, failure },
    Dispose { token },
    TokenNone { result },
    RegionEnter { result, region },
    RegionReserve { result, mark, layout, extent, failure },
    RegionPublish { value },
    RegionExit { mark, region, failure },
    ArenaOpen { result },
    ArenaToken { result, mark },
    Defuse { address },
    FinalizeBegin { address, anchor },
    FinalizeEnd { address, anchor },
    Forget { value },
    FrameEnter { failure },
    FrameLeave,
    Unavailable { feature, anchor },
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
    Unreachable,
    InvariantFault { reason },
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
    code,
    environment,
    captures,
    destination,
    placement,
    anchor
});
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
snapshot_struct!(CallSite {
    callee,
    operation_arguments,
    abi,
    arguments,
    result,
    destination,
    declared,
    anchor
});
snapshot_enum!(ConstantValue {
    Unit,
    Bool(value),
    Integer(value),
    F32(bits),
    F64(bits),
    String(value),
    Bytes(value),
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
snapshot_struct!(SiteRange { start, end });
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
snapshot_struct!(RegionInstance { scope, anchor });
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
    init_flags
});
snapshot_struct!(Pools {
    values,
    addresses,
    projections,
    switch_arms,
    call_arguments,
    fields,
    captures,
    mask_words
});
snapshot_enum!(UnitOrigin { Source, Library });
snapshot_struct!(UnitHeader {
    stage,
    lir_version,
    origin,
    input,
    definition,
    implementation_revision,
    instance,
    target,
    layouts,
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
        r.open("LirUnit")?;
        let header = UnitHeader::read(r)?;
        if header.stage != S::KIND {
            return Err(r.error(format!(
                "stage mismatch: snapshot is {:?}, expected {:?}",
                header.stage,
                S::KIND
            )));
        }
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
        Ok(unit)
    }
}
