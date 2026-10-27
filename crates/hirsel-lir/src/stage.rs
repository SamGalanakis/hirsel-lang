//! Stages, receipts and owned object artifacts.
//!
//! A stage is an invariant on one LIR family, checked by its validator; it is
//! not a set of optional flags. Raw artifacts (`LirUnit<S>`, `RawReceipt`,
//! `ObjectArtifact`) are plain data that tests can forge. The validated
//! wrappers `ValidatedUnit<S>` and `NativeReceipt` have constructors private to
//! this crate, called only by the stage validators (`validate/`) after
//! every check passed; no other crate can manufacture them, and no snapshot
//! decodes into them.
use crate::effect::Opcode;
use crate::ir::LirUnit;
use crate::layout_types::ObjectFormat;
use hirsel_codec::{snapshot_enum, snapshot_struct};
use hirsel_syntax::{ContentHash, DefPath};

/// The frozen front-end input a native artifact was built from: the interface
/// version (`QueryEnvironment.interface_version`) and the driver's digest of the
/// `ValidatedNativeInput` snapshot. The driver computes the digest; this crate
/// does not depend on the driver.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct InputIdentity {
    pub interface_version: u32,
    pub digest: ContentHash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum StageKind {
    /// P2: SSA import.
    Value,
    /// P3: placement and ownership planning.
    Placed,
    /// P4: Perceus.
    Rc,
    /// P5: drop specialization and reuse.
    Reuse,
    /// P6: legalized, ready for emission.
    Ready,
}

impl StageKind {
    pub const ALL: [StageKind; 5] = [
        StageKind::Value,
        StageKind::Placed,
        StageKind::Rc,
        StageKind::Reuse,
        StageKind::Ready,
    ];

    /// The opcode vocabulary of each stage. Value LIR has semantic Copy/Drop and
    /// no counts and may open arenas; Placed adds compiler-local regions and
    /// lowers arena openings to them; Rc adds Dup/Release/IsUnique/IsWritable; Reuse
    /// adds tokens; Ready removes semantic Drop and unavailable placeholders.
    /// `Copy` stays: at Ready it is the primitive copy of a place whose layout
    /// recipe is Dup or Fields (a load plus one increment per counted leaf; it
    /// never allocates or fails), and the Ready validator rejects a Bits or
    /// FlatBytes copy, which lowers to `Read Bits` or `FlatCopy`. Loan and
    /// origin events stay until the Ready validator discharges them.
    /// Vocabulary is necessary, not sufficient: each stage validator checks
    /// the rest of its invariant.
    pub const fn admits(self, op: Opcode) -> bool {
        let rank = self as u8;
        match op {
            Opcode::PayloadReceipt => true,
            Opcode::Drop | Opcode::Unavailable => rank < StageKind::Ready as u8,
            Opcode::RegionEnter
            | Opcode::RegionReserve
            | Opcode::RegionPublish
            | Opcode::RegionExit
            | Opcode::ArenaToken => rank >= StageKind::Placed as u8,
            // Placement lowers an arena opening to its region's entry and token.
            Opcode::ArenaOpen => rank == StageKind::Value as u8,
            Opcode::Dup | Opcode::Release | Opcode::IsUnique | Opcode::IsWritable => {
                rank >= StageKind::Rc as u8
            }
            Opcode::FinalizeBegin | Opcode::FinalizeEnd => rank == StageKind::Ready as u8,
            Opcode::Reset | Opcode::Reuse | Opcode::Dispose | Opcode::TokenNone => {
                rank >= StageKind::Reuse as u8
            }
            // Source/grant/loan checks remain mandatory. Heap seals also
            // require a live companion describing the exact taken snapshot.
            Opcode::SealArena | Opcode::BorrowSealedRoot => true,
            Opcode::ArenaInventory => rank < StageKind::Ready as u8,
            Opcode::CopyValue => rank <= StageKind::Placed as u8,
            Opcode::HoleOpen | Opcode::HoleFill | Opcode::HoleAbandon => true,
            Opcode::Copy
            | Opcode::Const
            | Opcode::FunctionRef
            | Opcode::ReadStatic
            | Opcode::Unary
            | Opcode::Binary
            | Opcode::Compare
            | Opcode::Convert
            | Opcode::IntArith
            | Opcode::MulAdd
            | Opcode::ScalarUnary
            | Opcode::Check
            | Opcode::StorageLive
            | Opcode::StorageDead
            | Opcode::EndScope
            | Opcode::Address
            | Opcode::Read
            | Opcode::FlatCopy
            | Opcode::FlatView
            | Opcode::SliceView
            | Opcode::FlatLength
            | Opcode::FlatNext
            | Opcode::SeqLen
            | Opcode::SeqCapacity
            | Opcode::SeqReserve
            | Opcode::SeqSetLen
            | Opcode::SeqAppend
            | Opcode::ArenaPush
            | Opcode::Share
            | Opcode::FlatAppend
            | Opcode::FlatField
            | Opcode::FlatClose
            | Opcode::Hash
            | Opcode::Encode
            | Opcode::Decode
            | Opcode::Recast
            | Opcode::UniqueCopy
            | Opcode::Freeze
            | Opcode::Thaw
            | Opcode::Store
            | Opcode::Replace
            | Opcode::UpdateMask
            | Opcode::Construct
            | Opcode::Closure
            | Opcode::Weaken
            | Opcode::BeginLoan
            | Opcode::EndLoan
            | Opcode::BindOrigin
            | Opcode::BindFields
            | Opcode::EndOrigin
            | Opcode::Defuse
            | Opcode::Forget
            | Opcode::FrameEnter
            | Opcode::FrameLeave
            | Opcode::Jump
            | Opcode::Branch
            | Opcode::Switch
            | Opcode::Call
            | Opcode::Return
            | Opcode::Raise
            | Opcode::RejectTransfer
            | Opcode::Propagate
            | Opcode::Recover
            | Opcode::Isolate
            | Opcode::Unreachable
            | Opcode::InvariantFault => true,
        }
    }
}

mod sealed {
    pub trait Sealed {}
}

/// A stage marker type. Sealed: the five stages are the whole family.
pub trait Stage: sealed::Sealed + Copy + Eq + std::fmt::Debug + std::hash::Hash + 'static {
    const KIND: StageKind;
}

macro_rules! stages { ($($name:ident),*) => {$(
    #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
    pub enum $name {}
    impl sealed::Sealed for $name {}
    impl Stage for $name {
        const KIND: StageKind = StageKind::$name;
    }
)*}; }
stages!(Value, Placed, Rc, Reuse, Ready);

/// The digests a stage receipt binds. Plain data; decodable; never a proof.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RawReceipt {
    pub key: crate::objects::UnitKey,
    pub stage: StageKind,
    pub lir_version: u32,
    pub unit: ContentHash,
    pub validator_version: u32,
}

impl RawReceipt {
    /// Content digest, including the unit content key.
    pub fn digest(&self) -> ContentHash {
        crate::snapshot::digest("receipt", self)
    }
}

/// Proof that a stage validator accepted exactly these digests in this
/// process. Constructed only by `issue`, which only validators call; a
/// deserialized `RawReceipt` never substitutes for revalidation.
///
/// ```compile_fail
/// use hirsel_lir::{NativeReceipt, RawReceipt};
/// fn forge(raw: RawReceipt) -> NativeReceipt {
///     NativeReceipt { raw }
/// }
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct NativeReceipt {
    raw: RawReceipt,
    checked_inputs: ContentHash,
}

impl NativeReceipt {
    /// For the stage validators only, after every check passed.
    pub(crate) fn issue(raw: RawReceipt, checked_inputs: ContentHash) -> Self {
        Self {
            raw,
            checked_inputs,
        }
    }
    pub(crate) fn checked_inputs(&self) -> ContentHash {
        self.checked_inputs
    }
    pub fn raw(&self) -> &RawReceipt {
        &self.raw
    }
    pub fn digest(&self) -> ContentHash {
        self.raw.digest()
    }
}

/// A unit accepted by its stage validator, with the receipt binding it.
///
/// ```compile_fail
/// use hirsel_lir::{LirUnit, NativeReceipt, Ready, ValidatedUnit};
/// fn forge(unit: LirUnit<Ready>, receipt: NativeReceipt) -> ValidatedUnit<Ready> {
///     ValidatedUnit { unit, receipt }
/// }
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedUnit<S> {
    unit: LirUnit<S>,
    receipt: NativeReceipt,
}

impl<S: Stage> ValidatedUnit<S> {
    /// For the stage validators only, after every check passed.
    pub(crate) fn seal(unit: LirUnit<S>, receipt: NativeReceipt) -> Self {
        Self { unit, receipt }
    }
    pub fn unit(&self) -> &LirUnit<S> {
        &self.unit
    }
    pub fn receipt(&self) -> &NativeReceipt {
        &self.receipt
    }
    /// Give up the proof to transform the unit; the result must be revalidated.
    pub fn into_raw(self) -> LirUnit<S> {
        self.unit
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ObjectSymbolKind {
    Function,
    Data,
    /// A `hirsel_rt_v2` runtime entry.
    Runtime,
    /// An `extern "c"` function, which the root's `unsafe ffi` link inputs
    /// define (PROG-4).
    Foreign,
}

/// A symbol an object exports or imports, with the ABI digest it was built for.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ObjectSymbol {
    pub name: String,
    pub kind: ObjectSymbolKind,
    pub definition: Option<DefPath>,
    pub abi: Option<ContentHash>,
}

/// A relocation the linker needs, as emitted by the object writer.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Relocation {
    pub offset: u64,
    /// Index into the artifact's imports.
    pub symbol: u32,
    /// Target-specific relocation type.
    pub kind: u32,
    pub addend: i64,
}

/// What reproduces the object bit-for-bit.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Reproducibility {
    /// The pinned backend, e.g. "llvm 23.1.4".
    pub toolchain: String,
    pub options: ContentHash,
    pub lir_version: u32,
    pub runtime_abi_version: u32,
}

/// Owned object bytes and metadata. No LIR, LLVM context, provider,
/// arena pointer or borrowed buffer, and nothing that names the program the object was
/// built in (`crate::glue`).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ObjectArtifact {
    pub ahead: Option<crate::ahead::MachineEvidence>,
    pub target: ContentHash,
    pub format: ObjectFormat,
    pub bytes: Vec<u8>,
    pub exports: Vec<ObjectSymbol>,
    pub imports: Vec<ObjectSymbol>,
    pub relocations: Vec<Relocation>,
    pub abi: ContentHash,
    pub reproducibility: Reproducibility,
    /// Each function's debug intervals, when the build records them (`debug::Profile`).
    pub debug: Vec<crate::debug::DebugFunction>,
}

impl ObjectArtifact {
    /// Content digest.
    pub fn digest(&self) -> ContentHash {
        crate::snapshot::digest("object-artifact", self)
    }
}

snapshot_struct!(InputIdentity {
    interface_version,
    digest
});
snapshot_enum!(StageKind {
    Value,
    Placed,
    Rc,
    Reuse,
    Ready
});
snapshot_struct!(RawReceipt {
    key,
    stage,
    lir_version,
    unit,
    validator_version
});
snapshot_enum!(ObjectSymbolKind {
    Function,
    Data,
    Runtime,
    Foreign
});
snapshot_struct!(ObjectSymbol {
    name,
    kind,
    definition,
    abi
});
snapshot_struct!(Relocation {
    offset,
    symbol,
    kind,
    addend
});
snapshot_struct!(Reproducibility {
    toolchain,
    options,
    lir_version,
    runtime_abi_version
});
snapshot_struct!(ObjectArtifact {
    ahead,
    target,
    format,
    bytes,
    exports,
    imports,
    relocations,
    abi,
    reproducibility,
    debug
});
