//! Effect flags and the LIR opcode vocabulary.
//!
//! A call site carries its callee's declared effects as a product of independent
//! flags, never body-derived public promises. Every instruction and terminator row
//! names its opcode, which each stage's vocabulary admits or rejects.
use crate::ir::{Inst, Terminator};

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
