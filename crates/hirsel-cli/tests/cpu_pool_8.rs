hirsel_guard::install!();
include!("support/cpu_pool/physical.rs");
#[test]
fn fresh_cpu_pool_has_physical_width_8() {
    run_width(8);
}
