//! The integration tests of this crate, linked as one binary.

hirsel_guard::install!();

mod aggregates;
mod callback_writebacks;
mod callbacks;
mod constants;
mod execution;
mod generics;
mod interp_list_cost_5;
mod library;
mod library_callbacks;
mod library_cursors;
mod library_launch;
mod library_net;
mod observation;
mod operation_mutation;
mod patterns;
mod scalars;
mod structural;
#[path = "../support/mod.rs"]
mod support;

mod interp_immediate_cost_6;
