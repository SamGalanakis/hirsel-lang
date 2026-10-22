//! Pipeline tests of the front phases, runtime and code store (check, modes, lir,
//! llvm, lower, rt, code_store), linked
//! as one binary. A module named `<crate>_<file>` holds what was
//! `crates/hirsel-<crate>/tests/it/<file>.rs`, and `<crate>_<module>` the
//! driver- or interpreter-using unit tests of `src/<module>`; hirsel-rc's suites
//! link separately (tests/rc).

hirsel_guard::install!();

// hirsel-llvm's emission fixtures, shared with its own tests; llvm_scalar_library uses a part.
#[allow(dead_code)]
#[path = "../../../hirsel-llvm/tests/support/mod.rs"]
mod llvm_support;
// hirsel-rt's integration-test helpers, shared with its own tests; the rt_ modules use a part.
#[allow(dead_code)]
#[path = "../../../hirsel-rt/tests/support/mod.rs"]
mod rt_support;

include!("modules.rs");
