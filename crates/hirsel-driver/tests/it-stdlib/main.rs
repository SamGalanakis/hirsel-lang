//! The standard-library suites of this crate: ordinary std source compiled,
//! checked and executed through the driver, linked as their own binary so a
//! compiler edit relinks three smaller harnesses in parallel.

hirsel_guard::install!();

#[path = "../support/source_std.rs"]
mod source_std;

include!("modules.rs");
