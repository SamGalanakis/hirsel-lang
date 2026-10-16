//! The integration tests of this crate, linked as one binary.
mod aggregates;
mod callback_writebacks;
mod callbacks;
mod constants;
mod execution;
mod generics;
mod library;
mod library_callbacks;
mod library_cursors;
mod library_launch;
mod library_net;
mod library_resources;
mod observation;
mod operation_mutation;
mod patterns;
mod scalars;
mod structural;
#[path = "../support/mod.rs"]
mod support;
