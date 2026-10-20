//! The integration tests of this crate, linked as one binary.

hirsel_guard::install!(support::Counting: support::Counting);

mod abi;
mod alloc;
mod arena_lists;
mod clocks;
mod copy;
mod count;
mod finalizers;
mod handles;
mod io;
mod io_properties;
mod measurement;
mod model;
mod mounts;
mod net;
mod net_script;
mod process;
mod recorder_overlap;
mod recording;
mod regions;
mod release;
mod roots;
#[path = "../support/mod.rs"]
mod support;
mod tape_property;
mod tasks;
mod test_reporting;
mod text;
mod tls;
mod tls_server;
mod views;
mod waits;
