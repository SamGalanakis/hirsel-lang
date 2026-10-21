//! Interleavings of the actual runtime channel storage source. The recorder
//! gate is covered separately; these check the object lock's payload/version
//! order and the endpoint close that a receive observes.
use crate::channels::{CLOSED, Channels, DONE, Element};
use crate::release::Layout;
use loom::sync::Arc;

fn channel() -> (Channels, u64, u64, u32) {
    let layout = Layout {
        size: 16,
        align: 8,
        flags: 0,
        finalizer: None,
        step_count: 0,
    };
    let none = [0u64; 2];
    let mut channels = Channels::default();
    let element = unsafe { Element::new(0, &layout, none.as_ptr().cast()) };
    let (sender, receiver) = channels.open(element, &layout);
    let index = channels.channel(sender, true);
    (channels, sender, receiver, index)
}

pub fn concurrent_senders_keep_fifo_images_in_version_order() {
    loom::model(|| {
        let (channels, _, _, index) = channel();
        let cell = channels.payload(index);
        let send = |cell: Arc<crate::channels::ChannelCell>, value: u64| {
            let mut image = [1u64, value];
            let mut state = cell.lock();
            cell.prepare_send(&mut state);
            assert_eq!(
                unsafe { cell.send(&mut state, image.as_mut_ptr().cast(), false) }.0,
                DONE
            );
            let version = cell.next_event();
            assert_eq!(image, [0, 0]);
            (version, value)
        };
        let first = loom::thread::spawn({
            let cell = cell.clone();
            move || send(cell, 11)
        });
        let second = send(cell.clone(), 22);
        let mut sent = [first.join().unwrap(), second];
        sent.sort_unstable();
        assert_eq!([sent[0].0, sent[1].0], [2, 3]);
        for (_, value) in sent {
            let mut image = [0u64; 2];
            assert_eq!(
                unsafe { cell.receive(&mut cell.lock(), image.as_mut_ptr().cast()) },
                Ok(true)
            );
            assert_eq!(image, [1, value]);
        }
    });
}

pub fn send_racing_receiver_close_preserves_exactly_one_image_owner() {
    loom::model(|| {
        let (mut channels, _, receiver, index) = channel();
        let cell = channels.payload(index);
        let send = loom::thread::spawn({
            let cell = cell.clone();
            move || {
                let mut image = [1u64, 77];
                let mut state = cell.lock();
                cell.prepare_send(&mut state);
                let status = unsafe { cell.send(&mut state, image.as_mut_ptr().cast(), false) }.0;
                (status, cell.next_event(), image)
            }
        });
        assert!(channels.close(receiver).is_none());
        let (status, version, image) = send.join().unwrap();
        if status == DONE {
            assert_eq!(version, 2);
            assert_eq!(image, [0, 0]);
        } else {
            assert_eq!(status, CLOSED);
            assert_eq!(version, 3);
            assert_eq!(image, [1, 77]);
        }
        let mut out = [0u64; 2];
        assert_eq!(
            unsafe { cell.receive(&mut cell.lock(), out.as_mut_ptr().cast()) },
            Ok(false)
        );
        assert_eq!(out, [0, 0]);
    });
}

pub fn last_sender_close_cannot_overtake_its_queued_message() {
    loom::model(|| {
        let (mut channels, sender, _, index) = channel();
        let cell = channels.payload(index);
        let producer = loom::thread::spawn({
            let cell = cell.clone();
            move || {
                let mut image = [1u64, 88];
                {
                    let mut state = cell.lock();
                    cell.prepare_send(&mut state);
                    assert_eq!(
                        unsafe { cell.send(&mut state, image.as_mut_ptr().cast(), false) }.0,
                        DONE
                    );
                    assert_eq!(cell.next_event(), 2);
                }
                assert!(channels.close(sender).is_none());
                assert_eq!(image, [0, 0]);
            }
        });
        let mut out = [0u64; 2];
        let received = unsafe { cell.receive(&mut cell.lock(), out.as_mut_ptr().cast()) }.unwrap();
        producer.join().unwrap();
        if !received {
            assert_eq!(
                unsafe { cell.receive(&mut cell.lock(), out.as_mut_ptr().cast()) },
                Ok(true)
            );
        }
        assert_eq!(out, [1, 88]);
        let mut empty = [0u64; 2];
        assert_eq!(
            unsafe { cell.receive(&mut cell.lock(), empty.as_mut_ptr().cast()) },
            Err(CLOSED)
        );
        assert_eq!(empty, [0, 0]);
    });
}
