//! The integration tests of this crate, linked as one binary.

hirsel_guard::install!();

mod grants;
mod managed_cores;
mod net;
mod object_stores;
mod placement;
mod process;
mod program;
mod record;
mod runners;
mod signals;
