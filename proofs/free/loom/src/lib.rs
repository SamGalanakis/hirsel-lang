//! crates/hirsel-free's section gate, cutter and claim word, compiled from their own source
//! against loom (`sync` below), so the models check the code the runtime runs.

pub mod sync;

#[path = "../../../../crates/hirsel-free/src/claim.rs"]
pub mod claim;
#[path = "../../../../crates/hirsel-free/src/phase.rs"]
pub mod phase;
#[path = "../../../../crates/hirsel-free/src/section.rs"]
pub mod section;
