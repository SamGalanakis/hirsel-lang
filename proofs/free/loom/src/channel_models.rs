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
    crate::sync::model(|| {
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
    crate::sync::model(|| {
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
    crate::sync::model(|| {
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

/// An empty receive (tasks/free/objects.rs `channel_receive`) against a send: a receive that
/// finds nothing ready keeps its wake token under the cell lock, so register and recheck are
/// one step, while the sender takes the oldest token under the same lock and wakes it after
/// the unlock. The receiver always gets the message: by its recheck, or by the token's wake.
pub fn a_role_free_receive_that_finds_nothing_rechecks_or_is_woken() {
    use crate::pool::free::Token;
    crate::sync::model(|| {
        let (channels, _, _, index) = channel();
        let cell = channels.payload(index);
        let sender = loom::thread::spawn({
            let cell = cell.clone();
            move || {
                let mut image = [1u64, 55];
                let token = {
                    let mut state = cell.lock();
                    cell.prepare_send(&mut state);
                    let sent = unsafe { cell.send(&mut state, image.as_mut_ptr().cast(), false) };
                    assert_eq!(sent.0, DONE);
                    state.next_token()
                };
                if let Some(Token(wake)) = token {
                    wake.notify();
                }
            }
        });
        let wake = Arc::new(loom::sync::Notify::new());
        let mut out = [0u64; 2];
        loop {
            {
                let mut state = cell.lock();
                // A send took the token; any other wake left it.
                state.drop_token(0);
                if cell.ready(&state) {
                    assert_eq!(
                        unsafe { cell.receive(&mut state, out.as_mut_ptr().cast()) },
                        Ok(true)
                    );
                    break;
                }
                state.keep_token(0, Token(wake.clone()));
            }
            wake.wait();
        }
        sender.join().unwrap();
        assert_eq!(out, [1, 55]);
    });
}

/// A last sender's role-free close against a receiver parking with its wake token (dfbc7719,
/// tasks/free/objects.rs close): the receiver keeps its token under the cell lock when nothing
/// is ready, and the close takes every token under the same lock and wakes them after it. The
/// receiver always ends on Closed: by its own check, or by the close's wake.
pub fn a_last_sender_close_wakes_a_receiver_parking_with_its_token() {
    use crate::pool::free::Token;
    crate::sync::model(|| {
        let (channels, sender, _, index) = channel();
        let cell = channels.payload(index);
        let _ = sender;
        let closer = loom::thread::spawn({
            let cell = cell.clone();
            move || {
                let tokens = {
                    let mut state = cell.lock();
                    let closed = cell.close_locked(&mut state, true);
                    assert!(closed.unsent && !closed.last);
                    state.take_tokens()
                };
                for (_, Token(wake)) in tokens {
                    wake.notify();
                }
            }
        });
        let wake = Arc::new(loom::sync::Notify::new());
        let outcome = loop {
            let mut out = [0u64; 2];
            {
                let mut state = cell.lock();
                if cell.ready(&state) {
                    break unsafe { cell.receive(&mut state, out.as_mut_ptr().cast()) };
                }
                state.keep_token(0, Token(wake.clone()));
            }
            wake.wait();
            // Each attempt registers anew if it parks again.
            cell.lock().drop_token(0);
        };
        closer.join().unwrap();
        assert_eq!(outcome, Err(CLOSED));
    });
}
