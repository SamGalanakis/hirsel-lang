//! crates/hirsel-free's section gate, cutter and claim word, compiled from their own source
//! against loom (`sync` below), so the models check the code the runtime runs.

pub mod sync;

#[path = "../../../../crates/hirsel-free/src/cancel.rs"]
pub mod cancel;

#[path = "../../../../crates/hirsel-free/src/claim.rs"]
pub mod claim;
#[path = "../../../../crates/hirsel-free/src/phase.rs"]
pub mod phase;
#[path = "../../../../crates/hirsel-free/src/section.rs"]
pub mod section;

#[path = "../../../../crates/hirsel-free/src/schedule.rs"]
pub mod schedule;

#[path = "../../../../crates/hirsel-free/src/frame.rs"]
pub mod frame;

#[path = "../../../../crates/hirsel-free/src/queue.rs"]
pub mod queue;
