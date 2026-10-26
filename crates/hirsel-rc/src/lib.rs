//! See README.md: invariants and allowed dependencies. Modules land with their owning tasks.

#[cfg(test)]
hirsel_guard::install!();

pub mod arena_credit;
pub mod cleanup;
pub mod closure_calls;
pub mod elide;
pub mod flat_walk;
pub mod import;
pub mod insert;
pub mod instance;
pub mod invocations;
pub mod legalize;
pub mod library;
pub mod liveness;
pub mod normalize;
mod observation;
pub mod observed;
pub mod placement;
pub mod ready;
#[cfg(test)]
mod receipt_transport_tests;
pub mod reuse;
pub mod reuse_stores;
mod scoped_consumer_9;
pub mod seal_demand;
pub mod sealed_source;
pub mod spare_push;
mod spawn;
pub mod specialize_drop;
pub mod statics;

mod pass_error;
mod paths;
pub use pass_error::PassError;
