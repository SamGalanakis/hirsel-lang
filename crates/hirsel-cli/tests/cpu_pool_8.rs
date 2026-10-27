hirsel_guard::install!();
include!("support/cpu_pool/physical.rs");
#[test]
fn fresh_cpu_pool_has_physical_width_8() {
    run_width(8);
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
