//! Native data schemas: LIR stages, layouts, ABI, runtime declarations, effects
//! and deterministic snapshots.
//!
//! A shared data crate. It owns schemas, effect classification and
//! pure validators; it never transforms programs or reads `OwnershipFacts`.
pub mod abi;
pub mod abi_types;
pub mod descriptor_registry;
pub mod effect;
pub mod ir;
pub mod layout;
pub mod layout_types;
pub mod layout_validate;
pub mod operation;
pub mod persistent;
pub mod rt_program;
pub mod runtime_abi;
pub mod snapshot;
pub mod stage;
pub mod statics;
pub mod validate;

pub use abi_types::{FnAbi, FnAbiId};
pub use effect::{Effects, Opcode};
pub use ir::{LirUnit, SchemaError};
pub use layout_types::{LayoutId, LayoutRecord, LayoutTable, TargetSpec};
pub use stage::{
    InputIdentity, NativeReceipt, ObjectArtifact, Placed, RawReceipt, Rc, Ready, Reuse, Stage,
    StageKind, ValidatedUnit, Value,
};

/// The front-end interface version these schemas consume
/// (`QueryEnvironment.interface_version`). A change invalidates every artifact.
pub const INTERFACE_VERSION: u32 = 11;
pub const LIR_VERSION: u32 = 25;
pub const LAYOUT_VERSION: u32 = 3;
/// Hirsel cross-unit ABI v1.
pub const ABI_VERSION: u32 = 1;
/// `hirsel_rt_v2`.
pub const RUNTIME_ABI_VERSION: u32 = runtime_abi::ABI_VERSION;
/// Private FlatFormatV0, separate from the canonical SER-2 codec.
pub const FLAT_FORMAT_VERSION: u32 = 0;
pub const VALIDATOR_VERSION: u32 = 8;

/// Shared hand-built LIR assembly, available only with the `testing` feature.
#[cfg(feature = "testing")]
pub mod testing;
