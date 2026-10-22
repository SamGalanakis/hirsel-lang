//! The integration tests of this crate, linked as one binary, apart from the std
//! suites (tests/it-stdlib) and scaling probes (tests/it-scaling).

hirsel_guard::install!();

#[path = "../support/image_fixture.rs"]
mod image_fixture;
#[path = "../support/source_std.rs"]
mod source_std;

include!("modules.rs");
