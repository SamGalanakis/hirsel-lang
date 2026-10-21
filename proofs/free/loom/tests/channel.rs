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
