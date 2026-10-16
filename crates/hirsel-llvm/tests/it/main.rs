//! The integration tests of this crate, linked as one binary.
mod arena_bump;
mod compile;
mod diverging;
mod emit;
mod glue;
mod heap_fast_path;
mod native;
mod once_capture;
mod ready;
mod root_ssa;
mod scalar_library;
mod sequence_capacity;
#[path = "../support/mod.rs"]
mod support;
