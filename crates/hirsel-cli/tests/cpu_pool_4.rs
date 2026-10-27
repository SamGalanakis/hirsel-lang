hirsel_guard::install!();
include!("support/cpu_pool/canonical.rs");
#[test]
fn free_cpu_canonical_receipts_agree_across_emitters_and_replay() {
    cpu_free_canonical_receipts(4);
}
include!("support/cpu_pool/physical.rs");
#[test]
fn fresh_cpu_pool_has_physical_width_4() {
    run_width(4);
}
#[cfg(feature = "native-tsan")]
#[test]
fn native_tsan_free_llvm_width_4() {
    run_width_policy_backends(4, "tasks.free@1", &["llvm"]);
}
#[cfg(feature = "native-tsan")]
#[test]
fn native_tsan_free_cranelift_width_4() {
    run_width_policy_backends(4, "tasks.free@1", &["cranelift"]);
}
#[cfg(feature = "native-tsan")]
#[test]
fn native_tsan_ordered_llvm_width_4() {
    run_width_policy_backends(4, "tasks.cooperative@1", &["llvm"]);
}
#[cfg(feature = "native-tsan")]
#[test]
fn native_tsan_ordered_cranelift_width_4() {
    run_width_policy_backends(4, "tasks.cooperative@1", &["cranelift"]);
}
include!("support/cpu_pool/replay.rs");
#[test]
fn wide_graph_batches_record_and_replay_after_worker_teardown() {
    replay_width(4);
}

#[test]
fn ordered_scalar_jobs_match_serial_reference_and_replay() {
    ordered_replay_width(4);
}

include!("support/cpu_pool/jitter.rs");
#[test]
fn cpu_integer_float_bits_and_fresh_lifecycle_survive_jitter() {
    cpu_values_with_jitter(4);
}

include!("support/cpu_pool/generated.rs");
#[test]
fn free_generated_typed_batches_replay_with_one_permit() {
    generated_free_schedules(4);
}

include!("support/cpu_pool/watchdog.rs");
include!("support/cpu_pool/stops.rs");
#[test]
fn free_diverging_mappers_stop_for_fuel_or_external_stop_and_replay() {
    free_native_stops(4);
}

#[test]
fn free_typed_preparation_prefixes_release_each_row_root() {
    typed_preparation_prefix_census(4);
}

include!("support/cpu_pool/cleanup_cuts.rs");
include!("support/cpu_pool/build_coverage.rs");
#[test]
fn native_wide_success_every_byte_cleanup_cut() {
    wide_cleanup_cuts(4, "success");
}
#[test]
fn native_wide_failure_after_result_transfer_every_byte_cleanup_cut() {
    wide_cleanup_cuts(4, "failure");
}
#[test]
fn native_wide_cancel_after_result_transfer_every_byte_cleanup_cut() {
    wide_cleanup_cuts(4, "cancel");
}

include!("support/cpu_pool/allocation_audit.rs");
#[test]
fn native_typed_prefix_allocations_are_freed_before_heap_teardown() {
    typed_prefix_allocation_audit(4);
}
