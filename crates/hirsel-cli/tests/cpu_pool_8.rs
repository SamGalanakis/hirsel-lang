hirsel_guard::install!();
include!("support/cpu_pool/canonical.rs");
#[test]
fn free_cpu_canonical_receipts_agree_across_emitters_and_replay() {
    cpu_free_canonical_receipts(8);
}
include!("support/cpu_pool/physical.rs");
#[test]
fn fresh_cpu_pool_has_physical_width_8() {
    run_width(8);
}
#[cfg(feature = "native-tsan")]
#[test]
fn native_tsan_free_llvm_width_8() {
    run_width_policy_backends(8, "tasks.free@1", &["llvm"]);
}
#[cfg(feature = "native-tsan")]
#[test]
fn native_tsan_free_cranelift_width_8() {
    run_width_policy_backends(8, "tasks.free@1", &["cranelift"]);
}
#[cfg(feature = "native-tsan")]
#[test]
fn native_tsan_ordered_llvm_width_8() {
    run_width_policy_backends(8, "tasks.cooperative@1", &["llvm"]);
}
#[cfg(feature = "native-tsan")]
#[test]
fn native_tsan_ordered_cranelift_width_8() {
    run_width_policy_backends(8, "tasks.cooperative@1", &["cranelift"]);
}
include!("support/cpu_pool/replay.rs");
#[test]
fn wide_graph_batches_record_and_replay_after_worker_teardown() {
    replay_width(8);
}

#[test]
fn ordered_scalar_jobs_match_serial_reference_and_replay() {
    ordered_replay_width(8);
}

include!("support/cpu_pool/remaining.rs");
#[test]
fn ordered_qualification_failure_order_and_cleanup() {
    ordered_remaining("failure", ORDERED_FAILURE, 0);
}
#[test]
fn ordered_qualification_cancel_before_claim() {
    ordered_remaining("cancel-before", ORDERED_CANCEL, 0);
}
#[test]
fn ordered_qualification_cancel_after_claim() {
    ordered_remaining("cancel-after", ORDERED_CANCEL, 0);
}
#[test]
fn ordered_qualification_fuel_stop() {
    ordered_remaining(
        "fuel",
        ORDERED_FUEL,
        hirsel_run_schema::exit::BUDGET_EXCEEDED,
    );
}

#[test]
fn ordered_qualification_reserves_ordinary_control_capacity_before_preparation() {
    ordered_remaining("no-controller", ORDERED_NO_CONTROLLER, 0);
}

include!("support/cpu_pool/jitter.rs");
#[test]
fn cpu_integer_float_bits_and_fresh_lifecycle_survive_jitter() {
    cpu_values_with_jitter(8);
}

include!("support/cpu_pool/generated.rs");
#[test]
fn free_generated_typed_batches_replay_with_one_permit() {
    generated_free_schedules(8);
}

#[test]
fn free_typed_preparation_prefixes_release_each_row_root() {
    typed_preparation_prefix_census(8);
}

include!("support/cpu_pool/watchdog.rs");

include!("support/cpu_pool/allocation_audit.rs");
#[test]
fn native_typed_prefix_allocations_are_freed_before_heap_teardown() {
    typed_prefix_allocation_audit(8);
}

include!("support/cpu_pool/cleanup_cuts.rs");
#[test]
fn native_wide_success_every_byte_cleanup_cut() {
    wide_cleanup_cuts(8, "success");
}
#[test]
fn native_wide_failure_every_byte_cleanup_cut() {
    wide_cleanup_cuts(8, "failure");
}
#[test]
fn native_wide_cancel_every_byte_cleanup_cut() {
    wide_cleanup_cuts(8, "cancel");
}
