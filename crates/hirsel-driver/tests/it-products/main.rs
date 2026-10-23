//! Ordinary installed query products, in a separate fixture-dependent harness.
hirsel_guard::install!();
#[path = "../support/image_fixture.rs"]
mod image_fixture;
include!("modules.rs");
