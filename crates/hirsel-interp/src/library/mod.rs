//! Deterministic core operation kernels; signatures are driver registry data.
pub mod collection;
pub mod float;
pub mod integer;
pub mod simulation;
pub mod string;
pub use simulation::{FileReadScript, Simulation, SimulationConfig};
mod codec;
mod dictionaries;
mod dispatch;
mod file;
mod filesystem;
mod net;
pub mod process;
mod shape;
mod signals;
pub use dispatch::{Library, error, i32_value, none, ok, path, some, u32_value, u64_value};
mod launch;
pub use launch::{
    LaunchError, LaunchedExecution, core_runtime_types, launch, launch_resources_with_runtime,
    launch_with_runtime,
};

mod callbacks;
mod cpu;
pub mod race;
mod root_trace;
mod sealed;
mod shared;
mod slots;
pub mod walk;

mod cursors;

/// Canonical evaluator role selected by the driver, not a source-name heuristic.
pub const FORGET_OPERATION: &str = "forget";
/// `arena.with` calls its callback with an opaque arena handle (MEM-10).
pub const ARENA_WITH_OPERATION: &str = "arena.with";
/// `arena.bounded` calls its callback with a bounded arena handle (MEM-6).
pub const ARENA_BOUNDED_OPERATION: &str = "arena.bounded";
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationDisposition {
    Primitive,
    Capability,
    Evaluator,
    Unavailable,
}
/// Reverse coverage comes from executable dispatch declarations themselves.
pub fn operation_dispatch()
-> std::collections::BTreeMap<hirsel_syntax::DefPath, OperationDisposition> {
    let mut result = std::collections::BTreeMap::new();
    let entries = dispatch::PRIMARY_OPERATIONS
        .iter()
        .map(|(name, kind)| (name.to_string(), *kind))
        .chain(
            dictionaries::INTEGER_ORDERS
                .iter()
                .map(|(name, _)| (name.to_string(), OperationDisposition::Primitive)),
        )
        .chain(
            cursors::operation_names()
                .into_iter()
                .map(|name| (name, OperationDisposition::Primitive)),
        )
        .chain([
            (FORGET_OPERATION.into(), OperationDisposition::Evaluator),
            (ARENA_WITH_OPERATION.into(), OperationDisposition::Evaluator),
            (
                ARENA_BOUNDED_OPERATION.into(),
                OperationDisposition::Evaluator,
            ),
        ]);
    for (name, kind) in entries {
        assert!(
            result
                .insert(hirsel_syntax::DefPath::core(&name), kind)
                .is_none(),
            "duplicate dispatch operation {name}"
        );
    }
    result
}

pub mod runners;
mod tasks;
mod timers;

mod channels;
mod telemetry;
mod watch;

mod free;
pub use free::{FreePrint, FreeWrite, outcome_name as free_outcome_name};
mod synchronization;

pub(crate) use dispatch::numeric_work;
