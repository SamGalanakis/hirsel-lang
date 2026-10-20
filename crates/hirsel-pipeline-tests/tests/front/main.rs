//! Pipeline tests of the front phases, runtime and code store (check, modes, lir,
//! llvm, lower, rt, code_store), linked
//! as one binary. A module named `<crate>_<file>` holds what was
//! `crates/hirsel-<crate>/tests/it/<file>.rs`, and `<crate>_<module>` the
//! driver- or interpreter-using unit tests of `src/<module>`; hirsel-rc's suites
//! link separately (tests/rc).

hirsel_guard::install!();

mod analysis_canaries;
mod check_audit_tests;
mod check_lower_cursor_tests;
mod check_lower_match_tests;
mod check_lower_runtime_tests;
mod check_lower_tests;
mod check_serializable;
mod check_signature_tests;
mod check_support;
mod code_store_programs;
mod lir_schema;
mod llvm_scalar_library;
// hirsel-llvm's emission fixtures, shared with its own tests; llvm_scalar_library uses a part.
#[allow(dead_code)]
#[path = "../../../hirsel-llvm/tests/support/mod.rs"]
mod llvm_support;
mod lower_tests;
mod modes_authority_corpus;
mod modes_authority_tests;
mod modes_explanations;
mod modes_flow_scaling;
mod modes_flow_solver;
mod modes_loop_projections;
mod modes_ownership_tests;
mod modes_repair_probes;
mod modes_state_promises;
mod rt_io;
mod rt_io_native;
mod rt_io_properties;
mod rt_measurement_native;
mod rt_mounts;
mod rt_net;
mod rt_net_native;
mod rt_network_replay;
mod rt_operations;
mod rt_process_native;
mod rt_replay;
// hirsel-rt's integration-test helpers, shared with its own tests; the rt_ modules use a part.
#[allow(dead_code)]
#[path = "../../../hirsel-rt/tests/support/mod.rs"]
mod rt_support;
mod rt_text_native;
mod rt_tls;
mod rt_tls_native;
mod stdlib_http_tls;
