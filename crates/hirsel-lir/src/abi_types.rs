//! The Hirsel cross-unit ABI v1 schema (Target and ABI).
//!
//! `FnAbi` is computed once from the specialized declared callable type and its
//! layouts (`abi.rs`); both emitters and the heap executor consume it.
//! A relation, brand or uniqueness mode never changes an `FnAbi`; only the
//! declared parameter convention does.
use crate::layout_types::LayoutId;
use hirsel_codec::{snapshot_enum, snapshot_struct};
use hirsel_core_contract::Passing;
use hirsel_syntax::ContentHash;

/// Index into `LayoutTable::fn_abis`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct FnAbiId(pub u32);

/// A machine-level value class passed in one physical slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PhysicalType {
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    Pointer,
}

/// One physical slot with its size and alignment in bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct PhysicalSlot {
    pub ty: PhysicalType,
    pub size: u64,
    pub align: u32,
}

/// Scalar extension applied when a narrow integer is passed in a wider register.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ScalarExtension {
    None,
    Sign,
    Zero,
}

/// How one declared parameter is passed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ParameterPassing {
    /// Small scalars and block references, borrowed or taken. Borrowed scalar
    /// passing by value grants no ownership.
    ByValue,
    /// Borrowed inline aggregates: a read-only address.
    ReadOnlyAddress,
    /// `mut`: an exclusive address.
    ExclusiveAddress,
    /// Taken inline aggregates: an address whose contents transfer at entry.
    TransferAddress,
    /// Dynamic flat extents: pointer/length pair outside their bytes.
    PointerLength,
}

/// Declared multiplicity of a callable (MODE-3); part of its convention.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CallMultiplicity {
    Many,
    Once,
}

/// Which kind of entry the ABI describes. Closure and iterator thunks receive
/// an environment pointer immediately after the context; definitions do not.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CallableShape {
    Definition,
    Closure,
    IterNext,
    IterDrop,
    /// An `extern "c"` function (PROG-4): the platform C convention, with no
    /// run context, environment, destination or status. Emission passes the
    /// declared parameters in C form (a byte view as pointer and length, a
    /// capability not at all) and returns a scalar result directly.
    Foreign,
}

/// One declared parameter in ordinal order.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AbiParameter {
    /// Declaration ordinal of the source parameter.
    pub source: u32,
    pub convention: Passing,
    pub passing: ParameterPassing,
    pub layout: LayoutId,
    pub slots: Vec<PhysicalSlot>,
    pub extension: ScalarExtension,
}

/// The output pointer of a non-unit result.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AbiResult {
    /// Borrowed holders in the result are tied to these Borrow parameters.
    /// Empty for owned results, even when their storage has an `in` relation.
    pub borrowed_parameters: Vec<u32>,
    pub layout: LayoutId,
    pub slots: Vec<PhysicalSlot>,
}

/// Identity of an FnAbi: the declared specialized callable shape and ABI version.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FnAbiKey {
    pub callable: ContentHash,
    pub abi_version: u32,
}

/// Physical order: context, [environment], [caller destination], [result
/// output pointer], then declared parameters in ordinal order; returns a u8
/// status (0 success, 1 language failure). The LLVM lowering returns a plain
/// scalar result of at most 64 bits beside the status in registers instead of
/// through the output pointer (`hirsel-llvm` README, Register results).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FnAbi {
    pub key: FnAbiKey,
    pub shape: CallableShape,
    pub multiplicity: CallMultiplicity,
    /// Hidden `RunCtx*`; always present on Hirsel entries.
    pub context: bool,
    pub environment: bool,
    /// Hidden caller allocation destination.
    pub destination: bool,
    pub result: Option<AbiResult>,
    /// The declared result is Never (LIR schema revision 19): no result, and a
    /// successful status return is impossible. Never is not erased to unit.
    pub diverges: bool,
    /// Every returned block is placed in the arena token passed to this
    /// immutable parameter. Independently proved from all returns by C.
    pub result_arena: Option<u32>,
    pub parameters: Vec<AbiParameter>,
    pub version: u32,
}

snapshot_enum!(PhysicalType {
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    Pointer
});
snapshot_struct!(PhysicalSlot { ty, size, align });
snapshot_enum!(ScalarExtension { None, Sign, Zero });
snapshot_enum!(ParameterPassing {
    ByValue,
    ReadOnlyAddress,
    ExclusiveAddress,
    TransferAddress,
    PointerLength
});
snapshot_enum!(CallMultiplicity { Many, Once });
snapshot_enum!(CallableShape {
    Definition,
    Closure,
    IterNext,
    IterDrop,
    Foreign
});
snapshot_struct!(AbiParameter {
    source,
    convention,
    passing,
    layout,
    slots,
    extension
});
snapshot_struct!(AbiResult {
    borrowed_parameters,
    layout,
    slots
});
snapshot_struct!(FnAbiKey {
    callable,
    abi_version
});
snapshot_struct!(FnAbi {
    key,
    shape,
    multiplicity,
    context,
    environment,
    destination,
    result,
    diverges,
    result_arena,
    parameters,
    version
});
impl hirsel_codec::Snapshot for FnAbiId {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        w.uint(u128::from(self.0));
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        Ok(Self(<u32 as hirsel_codec::Snapshot>::read(r)?))
    }
}
