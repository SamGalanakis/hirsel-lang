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

include!("support/cpu_pool/host_contraction.rs");
#[test]
#[ignore = "requires a delegated CPU controller in the current user's systemd manager"]
fn native_host_quota_contraction_drains_and_replays() {
    host_quota_contraction();
}

include!("support/cpu_pool/interpreter.rs");
#[test]
fn cpu_interpreter_matches_native_phases_and_strict_replay() {
    cpu_interpreter_native_phases(4);
}

#[test]
fn cpu_interpreter_consumes_ordered_native_receipts() {
    cpu_interpreter_ordered_receipts(4);
}

include!("support/cpu_pool/four_engine.rs");
#[test]
fn cpu_interpreter_scalar_fixture_agrees_across_four_engines() {
    cpu_fixture_four_engine_agreement(4);
}

#[test]
fn cpu_interpreter_canceled_admission_agrees_across_four_engines() {
    cpu_fixture_canceled_admission(4);
}

#[test]
fn cpu_job_withdrawal_and_transfer_retire_once_per_generation() {
    cpu_job_withdrawal(4);
}

#[test]
fn cpu_interpreter_ordered_cancellation_at_every_task_phase() {
    cpu_interpreter_cancel_cuts(1);
}

include!("support/cpu_pool/interpreter_receipts.rs");
#[test]
fn cpu_interpreter_cut_withdrawal_and_cancellation_receipts() {
    cpu_interpreter_receipt_cuts(4);
}
include!("support/cpu_pool/interpreter_fuel.rs");
#[test]
fn cpu_interpreter_cut_exact_source_fuel_stops() {
    cpu_interpreter_fuel_cuts(4);
}

#[test]
#[ignore = "requires exclusive CPUs; tools/cpu-overlap reports host-blocked without counting a pass"]
fn ordered_allocating_mappers_overlap_and_replay() {
    run_width_policy_backends_mode(4, "tasks.cooperative@1", &["llvm", "cranelift"], true);
}
#[test]
fn ordered_allocating_jobs_match_serial_reference_and_replay() {
    ordered_replay_width_mode(4, true);
}

#[test]
fn ordered_allocating_success_and_failure_release_every_owner_before_teardown() {
    ordered_allocating_cleanup_audit(4);
}

#[test]
fn ordered_allocating_fixture_agrees_across_four_engines_width_4() {
    cpu_fixture_allocating_four_engine_agreement(4);
}

#[test]
fn ordered_empty_list_mapper_agrees_across_four_engines_and_replay() {
    cpu_fixture_empty_list_four_engine_agreement(4);
}

#[test]
fn ordered_copied_list_mapper_agrees_across_four_engines_and_replay() {
    cpu_fixture_copied_list_four_engine_agreement(4);
}

#[test]
fn ordered_deferred_list_growth_and_copy_agree_across_four_engines_and_replay() {
    cpu_fixture_allocating_four_engine_mapper(
        4,
        "fn(take job: u64) -> unique List[u64] uses {} { let mut out = [job]; list.reserve(mut out, 8u64); for i in 0u64..255u64 { list.push(mut out, i) }; list.push(mut out, job + 7u64); list.reserve(mut out, 512u64); let copied = copy out; [copied[0], copied[list.len(copied) - 1u64]] }",
    );
}

#[test]
fn ordered_allocating_job_arena_reuses_buffers_and_returns_an_escaping_owner() {
    cpu_fixture_allocating_four_engine_mapper(
        4,
        "fn(take job: u64) -> unique List[u64] uses {} { let mut checksum = 0u64; for i in 0u64..130u64 { let mut chunk: List[u64] = []; list.reserve(mut chunk, 128u64); list.push(mut chunk, job); list.push(mut chunk, i); let copied = copy chunk; checksum = int.wrapping_add(checksum, copied[1]) }; let mut out: List[u64] = []; list.reserve(mut out, 128u64); list.push(mut out, job); list.push(mut out, job + checksum - 8378u64); take out }",
    );
}

#[test]
fn ordered_allocating_job_arena_preserves_serial_receipts_and_exact_trace() {
    ordered_arena_receipts_match_serial_and_exact_witness();
}

#[test]
fn ordered_allocating_pressure_agrees_across_four_engines_and_cross_replay() {
    cpu_fixture_allocating_pressure_four_engine_agreement(4);
}

#[test]
fn ordered_allocating_job_local_aliases_detach_independently_and_escape() {
    cpu_fixture_allocating_four_engine_mapper(
        4,
        "fn(take job: u64) -> unique List[u64] uses {} { let mut source: List[u64] = []; list.reserve(mut source, 128u64); list.push(mut source, job); list.push(mut source, job + 7u64); for i in 0u64..130u64 { let alias = copy source; let mut left = copy alias; let mut right = copy alias; left[0] = job + i; right[1] = job + 7u64 + i; failure.check(source[0] == job && alias[1] == job + 7u64 && left[0] == job + i && right[1] == job + 7u64 + i, Bounds()) }; take source }",
    );
}
