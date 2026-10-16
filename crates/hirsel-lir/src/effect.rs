//! Exhaustive effect classification.
//!
//! One total match per family returns a product of independent flags. There is
//! no catch-all arm and no pure fallback: a new TIR or LIR variant fails to
//! compile until it is classified. Calls use declared effects plus conservative
//! failure and barrier flags, never body-derived public promises.
use crate::ir::{CallSite, Callee, Inst, ReadMode, SideTables, Terminator, UnavailableFeature};
use hirsel_tir as tir;

/// A product of effect flags.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct Effects(u16);

impl Effects {
    pub const NONE: Self = Self(0);
    pub const READS: Self = Self(1 << 0);
    pub const WRITES: Self = Self(1 << 1);
    pub const OWNS: Self = Self(1 << 2);
    pub const MAY_FAIL: Self = Self(1 << 3);
    pub const ALLOCATES: Self = Self(1 << 4);
    pub const OBSERVABLE: Self = Self(1 << 5);
    pub const BARRIER: Self = Self(1 << 6);
    pub const MUST_EMIT: Self = Self(1 << 7);
    /// Control transfer: a terminator's successor choice.
    pub const CONTROL: Self = Self(1 << 8);
    pub const ALL: Self = Self((1 << 9) - 1);

    pub const NAMES: [(Self, &'static str); 9] = [
        (Self::READS, "reads"),
        (Self::WRITES, "writes"),
        (Self::OWNS, "owns"),
        (Self::MAY_FAIL, "may_fail"),
        (Self::ALLOCATES, "allocates"),
        (Self::OBSERVABLE, "observable"),
        (Self::BARRIER, "barrier"),
        (Self::MUST_EMIT, "must_emit"),
        (Self::CONTROL, "control"),
    ];

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn is_pure(self) -> bool {
        self.0 == 0
    }
    pub const fn bits(self) -> u16 {
        self.0
    }
}

impl std::ops::BitOr for Effects {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl hirsel_codec::Snapshot for Effects {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        w.open("Effects");
        for (flag, name) in Self::NAMES {
            if self.contains(flag) {
                w.text(name);
            }
        }
        w.close();
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        r.open("Effects")?;
        let mut effects = Self::NONE;
        let mut last = None;
        while !r.at_close() {
            let name = r.text()?;
            let index = Self::NAMES
                .iter()
                .position(|(_, n)| *n == name)
                .ok_or_else(|| r.error(format!("unknown effect {name}")))?;
            if last.is_some_and(|l| l >= index) {
                return Err(r.error("effects out of canonical order"));
            }
            last = Some(index);
            effects = effects | Self::NAMES[index].0;
        }
        r.close()?;
        Ok(effects)
    }
}

macro_rules! opcodes {
    ($($name:ident),* $(,)?) => {
        /// One code per LIR instruction and terminator variant.
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
        pub enum Opcode { $($name),* }
        impl Opcode {
            pub const ALL: &'static [Opcode] = &[$(Opcode::$name),*];
        }
    };
}
opcodes!(
    Const,
    FunctionRef,
    ReadStatic,
    Unary,
    Binary,
    Compare,
    Convert,
    IntArith,
    MulAdd,
    Check,
    StorageLive,
    StorageDead,
    EndScope,
    Address,
    Read,
    Copy,
    FlatCopy,
    FlatView,
    FlatLength,
    FlatNext,
    SeqLen,
    SeqCapacity,
    SeqReserve,
    SeqSetLen,
    SeqAppend,
    FlatAppend,
    FlatField,
    FlatClose,
    Hash,
    Recast,
    UniqueCopy,
    Store,
    Replace,
    UpdateMask,
    Construct,
    Closure,
    Weaken,
    BeginLoan,
    EndLoan,
    BindOrigin,
    BindFields,
    EndOrigin,
    Drop,
    Dup,
    IsUnique,
    Release,
    Reset,
    Reuse,
    Dispose,
    TokenNone,
    RegionEnter,
    RegionReserve,
    RegionPublish,
    RegionExit,
    ArenaOpen,
    ArenaToken,
    FinalizeBegin,
    Defuse,
    FinalizeEnd,
    Forget,
    FrameEnter,
    FrameLeave,
    Unavailable,
    Jump,
    Branch,
    Switch,
    Call,
    Return,
    Raise,
    Propagate,
    Unreachable,
    InvariantFault,
);

impl Opcode {
    pub const fn is_terminator(self) -> bool {
        matches!(
            self,
            Self::Jump
                | Self::Branch
                | Self::Switch
                | Self::Call
                | Self::Return
                | Self::Raise
                | Self::Propagate
                | Self::Unreachable
                | Self::InvariantFault
        )
    }
}

impl Inst {
    pub const fn opcode(&self) -> Opcode {
        match self {
            Self::Const { .. } => Opcode::Const,
            Self::FunctionRef { .. } => Opcode::FunctionRef,
            Self::ReadStatic { .. } => Opcode::ReadStatic,
            Self::Unary { .. } => Opcode::Unary,
            Self::Binary { .. } => Opcode::Binary,
            Self::Compare { .. } => Opcode::Compare,
            Self::Convert { .. } => Opcode::Convert,
            Self::IntArith { .. } => Opcode::IntArith,
            Self::MulAdd { .. } => Opcode::MulAdd,
            Self::Check { .. } => Opcode::Check,
            Self::StorageLive { .. } => Opcode::StorageLive,
            Self::StorageDead { .. } => Opcode::StorageDead,
            Self::EndScope { .. } => Opcode::EndScope,
            Self::Address { .. } => Opcode::Address,
            Self::Read { .. } => Opcode::Read,
            Self::Copy { .. } => Opcode::Copy,
            Self::FlatCopy { .. } => Opcode::FlatCopy,
            Self::FlatView { .. } => Opcode::FlatView,
            Self::FlatLength { .. } => Opcode::FlatLength,
            Self::FlatNext { .. } => Opcode::FlatNext,
            Self::SeqLen { .. } => Opcode::SeqLen,
            Self::SeqCapacity { .. } => Opcode::SeqCapacity,
            Self::SeqReserve { .. } => Opcode::SeqReserve,
            Self::SeqSetLen { .. } => Opcode::SeqSetLen,
            Self::SeqAppend { .. } => Opcode::SeqAppend,
            Self::FlatAppend { .. } => Opcode::FlatAppend,
            Self::FlatField { .. } => Opcode::FlatField,
            Self::FlatClose { .. } => Opcode::FlatClose,
            Self::Hash { .. } => Opcode::Hash,
            Self::Recast { .. } => Opcode::Recast,
            Self::UniqueCopy { .. } => Opcode::UniqueCopy,
            Self::Store { .. } => Opcode::Store,
            Self::Replace { .. } => Opcode::Replace,
            Self::UpdateMask { .. } => Opcode::UpdateMask,
            Self::Construct { .. } => Opcode::Construct,
            Self::Closure { .. } => Opcode::Closure,
            Self::Weaken { .. } => Opcode::Weaken,
            Self::BeginLoan { .. } => Opcode::BeginLoan,
            Self::EndLoan { .. } => Opcode::EndLoan,
            Self::BindOrigin { .. } => Opcode::BindOrigin,
            Self::BindFields { .. } => Opcode::BindFields,
            Self::EndOrigin { .. } => Opcode::EndOrigin,
            Self::Drop { .. } => Opcode::Drop,
            Self::Dup { .. } => Opcode::Dup,
            Self::IsUnique { .. } => Opcode::IsUnique,
            Self::Release { .. } => Opcode::Release,
            Self::Reset { .. } => Opcode::Reset,
            Self::Reuse { .. } => Opcode::Reuse,
            Self::Dispose { .. } => Opcode::Dispose,
            Self::TokenNone { .. } => Opcode::TokenNone,
            Self::RegionEnter { .. } => Opcode::RegionEnter,
            Self::RegionReserve { .. } => Opcode::RegionReserve,
            Self::RegionPublish { .. } => Opcode::RegionPublish,
            Self::RegionExit { .. } => Opcode::RegionExit,
            Self::ArenaOpen { .. } => Opcode::ArenaOpen,
            Self::ArenaToken { .. } => Opcode::ArenaToken,
            Self::FinalizeBegin { .. } => Opcode::FinalizeBegin,
            Self::Defuse { .. } => Opcode::Defuse,
            Self::FinalizeEnd { .. } => Opcode::FinalizeEnd,
            Self::Forget { .. } => Opcode::Forget,
            Self::FrameEnter { .. } => Opcode::FrameEnter,
            Self::FrameLeave => Opcode::FrameLeave,
            Self::Unavailable { .. } => Opcode::Unavailable,
        }
    }
}

impl Terminator {
    pub const fn opcode(&self) -> Opcode {
        match self {
            Self::Jump(_) => Opcode::Jump,
            Self::Branch { .. } => Opcode::Branch,
            Self::Switch { .. } => Opcode::Switch,
            Self::Call { .. } => Opcode::Call,
            Self::Return(_) => Opcode::Return,
            Self::Raise(_) => Opcode::Raise,
            Self::Propagate => Opcode::Propagate,
            Self::Unreachable => Opcode::Unreachable,
            Self::InvariantFault { .. } => Opcode::InvariantFault,
        }
    }
}

const R: Effects = Effects::READS;
const W: Effects = Effects::WRITES;
const O: Effects = Effects::OWNS;
const F: Effects = Effects::MAY_FAIL;
const A: Effects = Effects::ALLOCATES;
const V: Effects = Effects::OBSERVABLE;
const B: Effects = Effects::BARRIER;
const E: Effects = Effects::MUST_EMIT;
const C: Effects = Effects::CONTROL;

/// The effects every instance of an opcode may have.
pub const fn opcode_effects(op: Opcode) -> Effects {
    let flags = match op {
        // Pure values: removable only with exact typed semantics and dominance.
        Opcode::Const | Opcode::FunctionRef | Opcode::UpdateMask => Effects::NONE,
        // Named scalar operations that report range in a flag instead of failing.
        Opcode::Convert | Opcode::IntArith | Opcode::MulAdd => Effects::NONE,
        Opcode::ReadStatic => R,
        // Checked scalar operations may fail; float forms do not, but the row is typed per value.
        Opcode::Unary | Opcode::Binary => F,
        // Structural comparison reads both operands and may fail with Depth.
        Opcode::Compare => R.union(F),
        Opcode::Check => F.union(E),
        Opcode::StorageLive | Opcode::StorageDead => W,
        Opcode::EndScope => B,
        Opcode::Address => R.union(F),
        Opcode::Read => R,
        // A view or the count of a flat variable part: reads, never fails.
        Opcode::FlatView | Opcode::FlatLength | Opcode::FlatNext => R,
        // Reads of a sequence block's count or usable size, a keyed hash of
        // a value and a block reference at another payload layout (LIR
        // schema revision 7): each reads and never fails.
        Opcode::SeqLen | Opcode::SeqCapacity | Opcode::Recast => R,
        Opcode::Hash => R.union(F),
        // An in-place reallocation that keeps the count and the elements.
        Opcode::SeqReserve => R.union(W).union(O).union(A).union(E),
        // The count alone changes; which slots are owned changes with it.
        Opcode::SeqSetLen => W.union(O).union(E),
        // A write past the used bytes that consumes its value and may reallocate.
        Opcode::SeqAppend => R.union(W).union(O).union(A).union(E),
        // Reads the encoded fields, consumes the fixed ones, writes past the
        // used bytes and may reallocate.
        Opcode::FlatAppend => R.union(W).union(O).union(A).union(E),
        // Reads and writes the open element past the used bytes and may
        // reallocate.
        Opcode::FlatField => R.union(W).union(A).union(E),
        // Completes the open element as `FlatAppend` writes one.
        Opcode::FlatClose => R.union(W).union(O).union(A).union(E),
        Opcode::Copy | Opcode::FlatCopy | Opcode::UniqueCopy => R.union(O).union(A),
        Opcode::Store => W.union(O),
        // Replace commit is a barrier; preparation may allocate and fail.
        Opcode::Replace => W.union(O).union(F).union(A).union(B),
        // Ownership plus optional allocation; resource construction is an observable anchor.
        Opcode::Construct | Opcode::Closure => O.union(A).union(F).union(V),
        // A pure change of copy recipe: the credit moves, nothing is written.
        Opcode::Weaken => O,
        // Static loan and origin events: no bytes, but lifetime barriers.
        Opcode::BeginLoan
        | Opcode::EndLoan
        | Opcode::BindOrigin
        | Opcode::BindFields
        | Opcode::EndOrigin => B,
        // Semantic drop and release may run finalizers, which may allocate and fail.
        Opcode::Drop => W.union(O).union(F).union(A).union(V).union(E),
        Opcode::Dup => O,
        // A read of the block's count; it neither fails nor transfers.
        Opcode::IsUnique => R,
        Opcode::Release => O.union(F).union(A).union(V),
        Opcode::Reset => O.union(F).union(A).union(V).union(E),
        Opcode::Reuse => W.union(O).union(F).union(A).union(E),
        Opcode::Dispose | Opcode::TokenNone => O.union(E),
        Opcode::RegionEnter => O.union(A).union(B).union(E),
        Opcode::RegionReserve => O.union(A).union(F),
        Opcode::RegionPublish => W.union(B),
        Opcode::RegionExit => O.union(F).union(A).union(V).union(B).union(E),
        // Opening an arena is a region entry; its close is the region exit.
        Opcode::ArenaOpen => O.union(A).union(B).union(E),
        // The address of an entered region's record: a pure value.
        Opcode::ArenaToken => Effects::NONE,
        Opcode::FinalizeBegin | Opcode::FinalizeEnd => V.union(B).union(E),
        Opcode::Defuse => O.union(B).union(E),
        Opcode::Forget => O.union(V).union(E),
        // Logical frame admission (#16).
        Opcode::FrameEnter => F.union(E),
        Opcode::FrameLeave => E,
        // M2 placeholders: conservative, and rejected by the Ready support check.
        Opcode::Unavailable => R
            .union(W)
            .union(O)
            .union(F)
            .union(A)
            .union(V)
            .union(B)
            .union(E),
        Opcode::Jump | Opcode::Branch | Opcode::Switch | Opcode::Unreachable => C,
        Opcode::InvariantFault => C.union(B).union(Effects::MUST_EMIT),
        Opcode::Call => C.union(F).union(B).union(E),
        Opcode::Return => C.union(O),
        Opcode::Raise => C.union(F).union(V).union(E),
        Opcode::Propagate => C.union(F),
    };
    flags
}

/// Effects of one instruction row, refined by its operands.
pub const fn inst_effects(inst: &Inst) -> Effects {
    let base = opcode_effects(inst.opcode());
    match inst {
        Inst::Recast { .. } => base.union(O),
        Inst::Read {
            mode: ReadMode::Take,
            ..
        } => base.union(W).union(O),
        _ => base,
    }
}

/// Effects of a terminator; a call adds its callee's declared effects.
pub fn terminator_effects(term: &Terminator, tables: &SideTables) -> Effects {
    let base = opcode_effects(term.opcode());
    match term {
        Terminator::Call { call, .. } => match tables.calls.get(call.index()) {
            Some(site) => base | call_effects(site),
            None => Effects::ALL,
        },
        _ => base,
    }
}

/// Declared callee effects. An indirect or unknown call is conservative; a
/// bound call names its code, as a direct one does.
pub fn call_effects(site: &CallSite) -> Effects {
    match site.callee {
        Callee::Indirect(_) => Effects::ALL,
        Callee::Direct(_) | Callee::Operation(_) | Callee::Bound { .. } => site.declared,
    }
}

/// How one TIR item lowers: the LIR opcodes it maps to and their effects, or a
/// typed unavailable result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TirClass {
    Lowered {
        ops: &'static [Opcode],
        effects: Effects,
    },
    Unavailable(UnavailableFeature),
}

const fn lowered(ops: &'static [Opcode], extra: Effects) -> TirClass {
    let mut effects = extra;
    let mut i = 0;
    while i < ops.len() {
        effects = effects.union(opcode_effects(ops[i]));
        i += 1;
    }
    TirClass::Lowered { ops, effects }
}

fn operand_class(usage: hirsel_types::UseKind) -> TirClass {
    use hirsel_types::UseKind;
    match usage {
        // A variable part of a flat value is viewed at its source type.
        UseKind::Borrow => lowered(
            &[Opcode::Address, Opcode::Read, Opcode::FlatView],
            Effects::NONE,
        ),
        // A many-call callable taken where a once-call one is expected weakens.
        UseKind::Take => lowered(&[Opcode::Address, Opcode::Read, Opcode::Weaken], W.union(O)),
        UseKind::Copy => lowered(&[Opcode::Address, Opcode::Copy], Effects::NONE),
        UseKind::Mut => lowered(&[Opcode::Address], W),
    }
}

/// Every TIR rvalue.
pub fn classify_rvalue(rvalue: &tir::Rvalue) -> TirClass {
    use hirsel_syntax::{BinaryOp as SB, UnaryOp as SU};
    match rvalue {
        tir::Rvalue::Constant(_) => lowered(&[Opcode::Const], Effects::NONE),
        tir::Rvalue::ReadConstant { .. } => lowered(&[Opcode::ReadStatic], Effects::NONE),
        tir::Rvalue::FunctionReference { .. } => lowered(&[Opcode::FunctionRef], Effects::NONE),
        // A generic size read folds to a u64 constant in each instance.
        tir::Rvalue::Size(_) => lowered(&[Opcode::Const], Effects::NONE),
        tir::Rvalue::Use(operand) => operand_class(operand.usage),
        tir::Rvalue::Unary { op, value } => match op {
            SU::Neg | SU::Not | SU::BitNot => lowered(
                &[Opcode::Address, Opcode::Read, Opcode::Unary],
                Effects::NONE,
            ),
            // Lowering turns ownership requests into explicit operand uses; a
            // surviving one is classified by the use it denotes.
            SU::Take | SU::Copy | SU::Mut => operand_class(value.usage),
            SU::Unique => lowered(
                &[Opcode::Address, Opcode::Read, Opcode::UniqueCopy],
                Effects::NONE,
            ),
            SU::Box => lowered(
                &[Opcode::Address, Opcode::Read, Opcode::Construct],
                Effects::NONE,
            ),
        },
        tir::Rvalue::Binary { op, .. } => match op {
            SB::Add
            | SB::Sub
            | SB::Mul
            | SB::Div
            | SB::Rem
            | SB::Shl
            | SB::Shr
            | SB::BitAnd
            | SB::BitOr
            | SB::BitXor
            | SB::Eq
            | SB::Ne
            | SB::Lt
            | SB::Le
            | SB::Gt
            | SB::Ge => lowered(
                &[
                    Opcode::Address,
                    Opcode::Read,
                    Opcode::Binary,
                    Opcode::Compare,
                ],
                Effects::NONE,
            ),
            // Short circuits are CFG edges, never eager operations.
            SB::And | SB::Or => lowered(&[Opcode::Address, Opcode::Read, Opcode::Branch], R),
            SB::Range | SB::RangeInclusive => lowered(
                &[Opcode::Address, Opcode::Read, Opcode::Construct],
                Effects::NONE,
            ),
        },
        tir::Rvalue::Construct { .. } => lowered(&[Opcode::Construct], R),
        tir::Rvalue::Closure { .. } => lowered(&[Opcode::Closure], R),
        // Materialized static data when supported; the importer reports
        // `UnavailableFeature::Shape` otherwise, never an empty placeholder.
        tir::Rvalue::Shape(_) => lowered(&[Opcode::ReadStatic], Effects::NONE),
    }
}

/// Every TIR instruction kind. `Assign` adds its rvalue's class.
pub fn classify_instruction(kind: &tir::InstructionKind, body: &tir::Body) -> TirClass {
    use tir::InstructionKind as K;
    match kind {
        K::BindOrigin { .. } => lowered(&[Opcode::BindOrigin], Effects::NONE),
        K::BindFields { .. } => lowered(&[Opcode::BindFields], Effects::NONE),
        K::Defuse { .. } => lowered(&[Opcode::Defuse], Effects::NONE),
        K::EndOrigin(_) => lowered(&[Opcode::EndOrigin], Effects::NONE),
        K::StorageLive(_) => lowered(&[Opcode::StorageLive], Effects::NONE),
        K::StorageDead { .. } => lowered(&[Opcode::StorageDead], Effects::NONE),
        K::EndScope(_) => lowered(&[Opcode::EndScope], Effects::NONE),
        K::Assign { value, .. } => match body.rvalues.get(value.0 as usize) {
            Some(rvalue) => match classify_rvalue(rvalue) {
                TirClass::Lowered { effects, .. } => {
                    lowered(&[Opcode::Address, Opcode::Store], effects)
                }
                unavailable @ TirClass::Unavailable(_) => unavailable,
            },
            // A dangling rvalue is malformed TIR; classify it as everything.
            None => lowered(&[Opcode::Address, Opcode::Store], Effects::ALL),
        },
        K::Replace { .. } => lowered(&[Opcode::Address, Opcode::Replace], Effects::NONE),
        K::BeginLoan { .. } => lowered(&[Opcode::Address, Opcode::BeginLoan], Effects::NONE),
        K::EndLoan { .. } => lowered(&[Opcode::EndLoan], Effects::NONE),
        K::Drop { .. } => lowered(&[Opcode::Address, Opcode::Drop], Effects::NONE),
        // The cleanup chain expands into explicit cleanup blocks of drops and loan ends.
        K::RunCleanup(_) => lowered(&[Opcode::Jump, Opcode::Drop, Opcode::EndLoan], B),
        K::Check { .. } => lowered(
            &[Opcode::Address, Opcode::Read, Opcode::Check],
            Effects::NONE,
        ),
        K::Snapshot { .. } => lowered(
            &[Opcode::Address, Opcode::Read, Opcode::Store],
            Effects::NONE,
        ),
    }
}

/// Every TIR terminator.
pub fn classify_terminator(term: &tir::Terminator) -> TirClass {
    use tir::Terminator as T;
    match term {
        T::Goto(_) => lowered(&[Opcode::Jump], Effects::NONE),
        // An ordered switch with guard continuations.
        T::Match { .. } => lowered(
            &[
                Opcode::Address,
                Opcode::Read,
                Opcode::Switch,
                Opcode::Branch,
            ],
            Effects::NONE,
        ),
        T::Branch { .. } => lowered(
            &[Opcode::Address, Opcode::Read, Opcode::Branch],
            Effects::NONE,
        ),
        T::Switch { .. } => lowered(
            &[Opcode::Address, Opcode::Read, Opcode::Switch],
            Effects::NONE,
        ),
        // Unknown at this level: union of explicit transfer and conservative
        // effects. The length of a flat list part is read in place, and a
        // scalar intrinsic becomes its instruction (LIR schema revision 6).
        T::Call { .. } => lowered(
            &[
                Opcode::Call,
                Opcode::FlatLength,
                Opcode::Convert,
                Opcode::IntArith,
                Opcode::MulAdd,
            ],
            Effects::ALL,
        ),
        T::Return { .. } => lowered(
            &[Opcode::Address, Opcode::Read, Opcode::Return],
            Effects::NONE,
        ),
        T::Fail { .. } => lowered(&[Opcode::Raise], Effects::NONE),
        T::Unreachable => lowered(&[Opcode::Unreachable], Effects::NONE),
    }
}

/// Every TIR cleanup action. Region, arena and task cleanup are M2.
pub fn classify_cleanup(action: &tir::CleanupAction) -> TirClass {
    use tir::CleanupAction as A;
    match action {
        A::Drop { .. } => lowered(&[Opcode::Address, Opcode::Drop], Effects::NONE),
        A::EndLoan { .. } => lowered(&[Opcode::EndLoan], Effects::NONE),
        A::ReleaseRegion(_) => TirClass::Unavailable(UnavailableFeature::ReleaseRegion),
        A::CloseArena(_) => TirClass::Unavailable(UnavailableFeature::CloseArena),
        A::JoinTasks { .. } => TirClass::Unavailable(UnavailableFeature::JoinTasks),
    }
}
