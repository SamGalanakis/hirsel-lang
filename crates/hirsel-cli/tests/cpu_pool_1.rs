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
