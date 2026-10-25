hirsel_guard::install!();

#[test]
fn concurrent_senders_keep_fifo_images_in_version_order() {
    hirsel_free_loom::channel_models::concurrent_senders_keep_fifo_images_in_version_order();
}

#[test]
fn send_racing_receiver_close_preserves_exactly_one_image_owner() {
    hirsel_free_loom::channel_models::send_racing_receiver_close_preserves_exactly_one_image_owner(
    );
}

#[test]
fn last_sender_close_cannot_overtake_its_queued_message() {
    hirsel_free_loom::channel_models::last_sender_close_cannot_overtake_its_queued_message();
}

#[test]
fn a_role_free_receive_that_finds_nothing_rechecks_or_is_woken() {
    hirsel_free_loom::channel_models::a_role_free_receive_that_finds_nothing_rechecks_or_is_woken();
}

#[test]
fn a_last_sender_close_wakes_a_receiver_parking_with_its_token() {
    hirsel_free_loom::channel_models::a_last_sender_close_wakes_a_receiver_parking_with_its_token();
}
