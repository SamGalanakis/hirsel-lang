//! See README.md: invariants and allowed dependencies. Modules land with their owning tasks.

#[cfg(test)]
hirsel_guard::install!();

pub mod append_into;
pub mod arena_credit;
pub mod cleanup;
pub mod closure_calls;
pub mod elide;
pub mod flat_walk;
pub mod import;
pub mod insert;
pub mod instance;
pub mod legalize;
pub mod library;
pub mod liveness;
pub mod normalize;
pub mod placement;
pub mod ready;
pub mod reuse;
pub mod reuse_stores;
pub mod spare_push;
mod spawn;
pub mod specialize_drop;
pub mod statics;

mod pass_error;
mod paths;
pub use pass_error::PassError;
