hirsel_guard::install!();
include!("support/cpu_pool/matmul.rs");
#[test]
fn matmul_t8_matches_upstream_on_both_native_backends() {
    quick_matmul(8);
}
