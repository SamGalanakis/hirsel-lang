//! Pipeline tests of hirsel-rc, linked as one binary: each `rc_<file>` module holds
//! what was `crates/hirsel-rc/tests/it/<file>.rs`. They compile source through the
//! driver; the front-phase suites link separately (tests/front).

hirsel_guard::install!();

#[path = "support.rs"]
mod rc_support;

include!("modules.rs");
