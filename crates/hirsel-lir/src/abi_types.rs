//! The Hirsel cross-unit ABI v5 schema (Target and ABI).
//!
//! `FnAbi` is computed once from the specialized declared callable type and its
//! layouts (`abi.rs`); both emitters and the heap executor consume it.
//! A relation, brand or uniqueness mode never changes an `FnAbi`; only the
//! declared parameter convention does.
use crate::layout_types::{LayoutId, ScalarKind};
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

/// A typed non-unit result. Runtime primitives use the output pointer; Hirsel
/// entries return a plain scalar beside status when `register` is present.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AbiResult {
    /// Borrowed holders in the result are tied to these Borrow parameters.
    /// Empty for owned results, even when their storage has an `in` relation.
    pub borrowed_parameters: Vec<u32>,
    pub layout: LayoutId,
    pub slots: Vec<PhysicalSlot>,
    pub register: Option<ScalarKind>,
}

/// Field positions of the native scalar result pair. Value first keeps arithmetic
/// results in the platform's ordinary result register (RAX or X0). Runtime
/// primitives and aggregate results retain their output pointer and bare status.
pub const REGISTER_VALUE_INDEX: usize = 0;
pub const REGISTER_STATUS_INDEX: usize = 1;

/// Identity of an FnAbi: the declared specialized callable shape and ABI version.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FnAbiKey {
    pub callable: ContentHash,
    pub abi_version: u32,
}

/// Invocation-bounded nonretention of a physical parameter's selected content.
/// The empty field path selects the whole value. This grants no ownership or authority.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ParameterPath {
    pub parameter: u32,
    pub fields: Vec<u32>,
}
snapshot_struct!(ParameterPath { parameter, fields });

/// Independently checked entry signature of callable content selected in a parameter.
/// This is a claim, never code-target or captured-data identity.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ScopedEntry {
    pub parameter: ParameterPath,
    pub identity: ContentHash,
}
snapshot_struct!(ScopedEntry {
    parameter,
    identity
});

/// Physical order: context, [environment], [caller and operation sites],
/// [caller destination], [result output pointer], then declared parameters.
/// Both native backends return a typed scalar as (value, u8 status) in
/// registers; all other results use the output pointer and return bare status
/// (0 success, 1 language failure). A runtime primitive always uses its output
/// pointer. Callable values are the two scalars (code, environment); direct,
/// bound and erased entries share the same typed result convention.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct FnAbi {
    pub key: FnAbiKey,
    pub shape: CallableShape,
    pub multiplicity: CallMultiplicity,
    /// Hidden `TaskCtx*`; always present on Hirsel entries.
    pub context: bool,
    pub environment: bool,
    /// Two hidden inert u64 codes: inherited attribution and actual source invocation.
    pub caller_site: bool,
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
    pub nonretaining: Vec<ParameterPath>,
    /// Capture ordinals use the environment field order, never source local ids.
    pub nonretaining_captures: Vec<ParameterPath>,
    /// Binds the complete selector lists; independent validation recomputes it.
    pub scoped_contract: ContentHash,
    pub scoped_entries: Vec<ScopedEntry>,
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
    slots,
    register
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
    caller_site,
    destination,
    result,
    diverges,
    result_arena,
    parameters,
    version,
    nonretaining,
    nonretaining_captures,
    scoped_contract,
    scoped_entries
});
impl hirsel_codec::Snapshot for FnAbiId {
    fn write(&self, w: &mut hirsel_codec::Writer) {
        w.uint(u128::from(self.0));
    }
    fn read(r: &mut hirsel_codec::Reader<'_>) -> Result<Self, hirsel_codec::DecodeError> {
        Ok(Self(<u32 as hirsel_codec::Snapshot>::read(r)?))
    }
}
