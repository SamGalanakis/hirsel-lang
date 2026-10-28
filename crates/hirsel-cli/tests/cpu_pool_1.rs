hirsel_guard::install!();
include!("support/cpu_pool/physical.rs");
#[test]
fn fresh_cpu_pool_has_physical_width_1() {
    run_width(1);
}

include!("support/cpu_pool/jitter.rs");
#[test]
fn cpu_integer_float_bits_and_fresh_lifecycle_survive_jitter() {
    cpu_values_with_jitter(1);
}

include!("support/cpu_pool/generated.rs");
#[test]
fn free_generated_typed_batches_replay_with_one_permit() {
    generated_free_schedules(1);
}

include!("support/cpu_pool/watchdog.rs");
include!("support/cpu_pool/cleanup_cuts.rs");
#[test]
fn native_job_result_cleanup_terminal_every_byte_cut_replays_previous_prefix() {
    native_result_cleanup_cuts();
}

#[test]
fn free_typed_preparation_prefixes_release_each_row_root() {
    typed_preparation_prefix_census(1);
}

include!("support/cpu_pool/canonical.rs");
#[test]
fn free_cpu_canonical_receipts_agree_across_emitters_and_replay() {
    cpu_free_canonical_receipts(1);
}

include!("support/cpu_pool/allocation_audit.rs");
#[test]
fn native_typed_prefix_allocations_are_freed_before_heap_teardown() {
    typed_prefix_allocation_audit(1);
}

include!("support/cpu_pool/interpreter.rs");
#[test]
fn cpu_interpreter_matches_native_phases_and_strict_replay() {
    cpu_interpreter_native_phases(1);
}

#[test]
fn cpu_interpreter_consumes_ordered_native_receipts() {
    cpu_interpreter_ordered_receipts(1);
}

#[test]
fn cpu_job_withdrawal_and_transfer_retire_once_per_generation() {
    cpu_job_withdrawal(1);
}

include!("support/cpu_pool/interpreter_receipts.rs");
#[test]
fn cpu_interpreter_cut_withdrawal_and_cancellation_receipts() {
    cpu_interpreter_receipt_cuts(1);
}
include!("support/cpu_pool/interpreter_fuel.rs");
#[test]
fn cpu_interpreter_cut_exact_source_fuel_stops() {
    cpu_interpreter_fuel_cuts(1);
}

include!("support/cpu_pool/four_engine.rs");
include!("support/cpu_pool/replay.rs");

#[test]
fn ordered_allocating_jobs_match_serial_reference_and_replay() {
    ordered_replay_width_mode(1, true);
}

#[test]
fn ordered_allocating_fixture_agrees_across_four_engines_width_1() {
    cpu_fixture_allocating_four_engine_agreement(1);
}

#[test]
fn ordered_empty_list_mapper_agrees_across_four_engines_and_replay() {
    cpu_fixture_empty_list_four_engine_agreement(1);
}

#[test]
fn ordered_copied_list_mapper_agrees_across_four_engines_and_replay() {
    cpu_fixture_copied_list_four_engine_agreement(1);
}
