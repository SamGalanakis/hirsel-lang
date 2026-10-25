//! One original recording identity and all replay adapters per bounded default body.
#[test]
fn no_subscriber_llvm_1() {
    super::pubsub_relay_30::native_cell("no-subscriber-llvm-1");
}
#[test]
fn one_receiver_llvm_1() {
    super::pubsub_relay_30::native_cell("one-receiver-llvm-1");
}
#[test]
fn order_two_llvm_1() {
    super::pubsub_relay_30::native_cell("order-two-llvm-1");
}
#[test]
fn duplicate_llvm_1() {
    super::pubsub_relay_30::native_cell("duplicate-llvm-1");
}
#[test]
fn unsubscribe_llvm_1() {
    super::pubsub_relay_30::native_cell("unsubscribe-llvm-1");
}
#[test]
fn late_subscribe_llvm_1() {
    super::pubsub_relay_30::native_cell("late-subscribe-llvm-1");
}
#[test]
fn two_receivers_llvm_1() {
    super::pubsub_relay_30::native_cell("two-receivers-llvm-1");
}
#[test]
fn two_receivers_cranelift_1() {
    super::pubsub_relay_30::native_cell("two-receivers-cranelift-1");
}
#[test]
fn malformed_llvm_1() {
    super::pubsub_relay_30::native_cell("malformed-llvm-1");
}
#[test]
fn malformed_cranelift_1() {
    super::pubsub_relay_30::native_cell("malformed-cranelift-1");
}
#[test]
fn truncated_header_llvm_1() {
    super::pubsub_relay_30::native_cell("truncated-header-llvm-1");
}
#[test]
fn truncated_body_llvm_1() {
    super::pubsub_relay_30::native_cell("truncated-body-llvm-1");
}
#[test]
fn oversize_llvm_1() {
    super::pubsub_relay_30::native_cell("oversize-llvm-1");
}
#[test]
fn pending_read_cancel_llvm_1() {
    super::pubsub_relay_30::native_cell("pending-read-cancel-llvm-1");
}
#[test]
fn pending_read_cancel_llvm_4() {
    super::pubsub_relay_30::native_cell("pending-read-cancel-llvm-4");
}
#[test]
fn pending_read_cancel_cranelift_1() {
    super::pubsub_relay_30::native_cell("pending-read-cancel-cranelift-1");
}
#[test]
fn frame_cap_llvm_1() {
    super::pubsub_relay_30::native_cell("frame-cap-llvm-1");
}
#[test]
fn admission_cap_llvm_1() {
    super::pubsub_relay_30::native_cell("admission-cap-llvm-1");
}
#[test]
fn jobs_zero_llvm_1() {
    super::pubsub_relay_30::native_cell("jobs-zero-llvm-1");
}
#[test]
fn jobs_over_llvm_1() {
    super::pubsub_relay_30::native_cell("jobs-over-llvm-1");
}
#[test]
fn duration_over_llvm_1() {
    super::pubsub_relay_30::native_cell("duration-over-llvm-1");
}
#[test]
fn denied_llvm_1() {
    super::pubsub_relay_30::native_cell("denied-llvm-1");
}
#[test]
fn host_success_llvm_1() {
    super::pubsub_relay_30::native_cell("host-success-llvm-1");
}
#[test]
fn host_success_llvm_4() {
    super::pubsub_relay_30::native_cell("host-success-llvm-4");
}
#[test]
fn host_success_cranelift_4() {
    super::pubsub_relay_30::native_cell("host-success-cranelift-4");
}
#[test]
fn host_domain_llvm_1() {
    super::pubsub_relay_30::native_cell("host-domain-llvm-1");
}
#[test]
fn host_domain_llvm_4() {
    super::pubsub_relay_30::native_cell("host-domain-llvm-4");
}
#[test]
fn host_domain_cranelift_4() {
    super::pubsub_relay_30::native_cell("host-domain-cranelift-4");
}
#[test]
fn host_cancel_llvm_1() {
    super::pubsub_relay_30::native_cell("host-cancel-llvm-1");
}
#[test]
fn host_cancel_llvm_4() {
    super::pubsub_relay_30::native_cell("host-cancel-llvm-4");
}
#[test]
fn host_cancel_cranelift_4() {
    super::pubsub_relay_30::native_cell("host-cancel-cranelift-4");
}
