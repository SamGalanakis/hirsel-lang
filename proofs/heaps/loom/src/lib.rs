//! crates/hirsel-rt's cross-worker return lists and owner word, compiled from their own source
//! against loom (`remote_sync` below), so the models check the code the runtime runs.

#[cfg(test)]
hirsel_guard::install!();

/// The runtime's `remote_sync` names over loom.
pub mod remote_sync {
    pub use loom::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
}

#[path = "../../../../crates/hirsel-rt/src/remote.rs"]
pub mod remote;
