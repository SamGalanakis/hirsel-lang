//! Native data schemas: LIR stages, layouts, ABI, runtime declarations, effects
//! and deterministic snapshots.
//!
//! A shared data crate. It owns schemas, effect classification and
//! pure validators; it never transforms programs or reads `OwnershipFacts`.

#[cfg(test)]
hirsel_guard::install!();

pub mod abi;
pub mod abi_types;
pub mod ahead;
pub mod codec;
pub mod construction;
pub mod continuation;
pub mod debug;
pub mod descriptor_registry;
pub mod discharge;
pub mod effect;
pub mod emission;
pub mod fragment;
pub mod fuel;
pub mod glue;
pub mod handle_scan;
pub mod invocations;
pub mod ir;
pub mod layout;
pub mod layout_index;
pub mod layout_keys;
pub mod layout_types;
pub mod layout_validate;
pub mod object;
pub mod objects;
pub mod operation;
pub mod ownership_trace;
pub mod persistent;
pub mod products;
pub mod profile;
pub mod receipt_closure;
pub mod receipt_entry;
pub mod receipt_transport;
pub mod rt_program;
pub mod runtime_abi;
pub mod sealed;
pub mod sealed_payload;
pub mod sealed_payload_source;
pub mod snapshot;
pub mod stage;
pub mod statics;
pub mod structural;
pub mod structural_access;
pub mod validate;

pub use abi_types::{FnAbi, FnAbiId};
pub use effect::{Effects, Opcode};
pub use ir::{LirUnit, SchemaError};
pub use layout_index::LayoutIndex;
pub use layout_keys::LayoutKeys;
pub use layout_types::{LayoutId, LayoutKey, LayoutRecord, LayoutTable, TargetSpec};
pub use stage::{
    InputIdentity, NativeReceipt, ObjectArtifact, Placed, RawReceipt, Rc, Ready, Reuse, Stage,
    StageKind, ValidatedUnit, Value,
};

/// The front-end interface version these schemas consume
/// (`QueryEnvironment.interface_version`). A change invalidates every artifact.
pub const INTERFACE_VERSION: u32 = 11;
pub const LIR_VERSION: u32 = 61;
pub const LAYOUT_VERSION: u32 = 10;
/// Hirsel cross-unit ABI v5: adds independently checked nested scoped entry identity.
pub const ABI_VERSION: u32 = 5;
/// `hirsel_rt_v2`.
pub const RUNTIME_ABI_VERSION: u32 = runtime_abi::ABI_VERSION;
/// Private FlatFormatV0, separate from the canonical SER-2 codec.
pub const FLAT_FORMAT_VERSION: u32 = 0;
pub const VALIDATOR_VERSION: u32 = 33;

/// Shared hand-built LIR assembly, available only with the `testing` feature.
#[cfg(feature = "testing")]
pub mod testing;

pub mod liveness;
pub mod logical_storage;

pub mod pretransfer;
