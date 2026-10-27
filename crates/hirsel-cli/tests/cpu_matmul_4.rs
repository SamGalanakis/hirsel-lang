hirsel_guard::install!();
include!("support/cpu_pool/matmul.rs");
#[test]
fn matmul_t4_matches_upstream_on_both_native_backends() {
    quick_matmul(4);
}

#[test]
fn every_cell_matches_serial_for_small_matrices() {
    every_cell(4);
}

#[test]
fn every_cell_matches_serial_at_width_one_including_empty_input() {
    every_cell(1);
}
