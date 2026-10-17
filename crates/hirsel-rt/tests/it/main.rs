//! The integration tests of this crate, linked as one binary.
mod abi;
mod alloc;
mod clocks;
mod copy;
mod count;
mod finalizers;
mod handles;
mod io;
mod measurement;
mod model;
mod mounts;
mod net;
mod net_script;
mod network_replay;
mod process;
mod recorder_overlap;
mod recording;
mod regions;
mod release;
#[path = "../support/mod.rs"]
mod support;
mod test_reporting;
mod text;
mod tls;
mod views;
