//! crates/hirsel-free's section gate, cutter and claim word, compiled from their own source
//! against loom (`sync` below), so the models check the code the runtime runs.

#[cfg(test)]
hirsel_guard::install!();

pub mod sync;

#[path = "../../../../crates/hirsel-free/src/atomic.rs"]
pub mod atomic;

#[path = "../../../../crates/hirsel-free/src/mutex.rs"]
pub mod mutex;

#[path = "../../../../crates/hirsel-free/src/cancel.rs"]
pub mod cancel;

#[path = "../../../../crates/hirsel-free/src/claim.rs"]
pub mod claim;
#[path = "../../../../crates/hirsel-free/src/executors.rs"]
pub mod executors;
#[path = "../../../../crates/hirsel-free/src/phase.rs"]
pub mod phase;
#[path = "../../../../crates/hirsel-free/src/section.rs"]
pub mod section;
#[path = "../../../../crates/hirsel-free/src/replay/wakes.rs"]
pub mod wakes;

#[path = "../../../../crates/hirsel-free/src/schedule.rs"]
pub mod schedule;

#[path = "../../../../crates/hirsel-free/src/frame.rs"]
pub mod frame;

#[path = "../../../../crates/hirsel-free/src/queue.rs"]
pub mod queue;

// The runtime channel module uses the same primitive facade as the kernel.
// Normal library builds expose the model fixture; excluding it from the
// library unit harness avoids compiling unrelated runtime descriptor tests.
extern crate self as hirsel_free;
#[cfg(not(test))]
pub mod channel_models;
#[cfg(not(test))]
#[path = "../../../../crates/hirsel-rt/src/channels.rs"]
#[allow(dead_code)]
mod channels;
#[cfg(not(test))]
#[path = "../../../../crates/hirsel-free/src/record.rs"]
pub mod record;
#[cfg(not(test))]
#[path = "../../../../crates/hirsel-rt/src/slots.rs"]
#[allow(dead_code)]
mod slots;
// Channel wake tokens are runtime pool handles; a model's token wakes its parked receiver.
#[cfg(not(test))]
pub mod pool {
    pub mod free {
        pub struct Token(pub loom::sync::Arc<loom::sync::Notify>);
    }
}
#[cfg(not(test))]
fn fault(message: &str) -> ! {
    panic!("{message}");
}
// Channel storage reads only this immutable metadata. This fixture neither
// models nor substitutes the runtime ABI or descriptor/finalizer execution.
#[cfg(not(test))]
mod release {
    pub const LAYOUT_DYNAMIC: u32 = 2;
    pub struct Layout {
        pub size: u64,
        pub align: u32,
        pub flags: u32,
        pub finalizer: Option<()>,
        pub step_count: u64,
    }
}

#[path = "../../../../crates/hirsel-rt/src/parking.rs"]
#[allow(dead_code)]
pub mod parking;
pub mod parking_sync;
#[cfg(test)]
mod queue_arena_tests;

#[path = "../../../../crates/hirsel-rt/src/tasks/role/owner.rs"]
#[allow(dead_code)]
mod role_owner;

#[path = "../../../../crates/hirsel-free/src/batch.rs"]
pub mod batch;
