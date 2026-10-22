//! The integration tests of this crate, linked as one binary.

hirsel_guard::install!();

#[path = "../support/mod.rs"]
mod support;

include!("modules.rs");
