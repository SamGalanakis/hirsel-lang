//! The integration tests of this crate, linked as one binary.

hirsel_guard::install!();

mod arena_bump;
mod compile;
mod diverging;
mod emit;
mod glue;
mod heap_fast_path;
mod native;
mod noalias;
mod once_capture;
mod ready;
mod remote_free;
mod root_ssa;
mod sequence_capacity;
#[path = "../support/mod.rs"]
mod support;
