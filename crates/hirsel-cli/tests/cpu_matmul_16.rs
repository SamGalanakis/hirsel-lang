hirsel_guard::install!();
include!("support/cpu_pool/matmul.rs");
#[test]
fn matmul_t16_matches_upstream_on_both_native_backends() {
    quick_matmul(16);
}

#[test]
fn every_cell_matches_serial_for_small_matrices() {
    every_cell(16);
}
