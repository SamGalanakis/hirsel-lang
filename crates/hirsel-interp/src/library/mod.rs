//! Deterministic core operation kernels; signatures are driver registry data.
pub mod collection;
pub mod float;
pub mod integer;
pub mod simulation;
pub mod string;
pub use simulation::{Simulation, SimulationConfig};
mod dictionaries;
mod dispatch;
mod net;
pub use dispatch::{Library, error, i32_value, none, ok, path, some, u32_value, u64_value};
mod launch;
pub use launch::{LaunchError, LaunchedExecution, core_runtime_types, launch, launch_with_runtime};

mod callbacks;

mod cursors;

/// Canonical evaluator role selected by the driver, not a source-name heuristic.
pub const FORGET_OPERATION: &str = "forget";
/// `arena.with` calls its callback with an opaque arena handle (MEM-10).
pub const ARENA_WITH_OPERATION: &str = "arena.with";
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

mod tasks;

mod channels;

mod synchronization;
