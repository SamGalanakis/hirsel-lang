//! The integration tests of this crate, linked as one binary.

hirsel_guard::install!();

#[path = "../fixtures/mod.rs"]
mod fixtures;

include!("modules.rs");
