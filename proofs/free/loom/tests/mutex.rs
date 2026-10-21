//! FIFO permit state from production under overlapping acquisitions/returns.
use hirsel_free_loom::mutex::{Acquired, Permit};
use loom::sync::{Arc, Mutex};

#[test]
fn a_new_acquirer_cannot_overtake_a_reserved_waiter() {
    loom::model(|| {
        let mut permit = Permit::new();
        let mut old = false;
        assert_eq!(permit.acquire(1, &mut old), Acquired::Pending);
        let permit = Arc::new(Mutex::new(permit));
        let newer = loom::thread::spawn({
            let permit = permit.clone();
            move || {
                let mut queued = false;
                assert_eq!(
                    permit.lock().unwrap().acquire(2, &mut queued),
                    Acquired::Pending
                );
            }
        });
        assert_eq!(permit.lock().unwrap().release(), Some(1));
        newer.join().unwrap();
        let mut permit = permit.lock().unwrap();
        assert_eq!(permit.acquire(1, &mut old), Acquired::Payload);
        assert_eq!(permit.release(), Some(2));
    });
}

#[test]
fn two_acquirers_transfer_the_available_payload_once() {
    loom::model(|| {
        let mut initial = Permit::new();
        initial.release();
        let permit = Arc::new(Mutex::new(initial));
        let first = loom::thread::spawn({
            let permit = permit.clone();
            move || {
                let mut queued = false;
                permit.lock().unwrap().acquire(1, &mut queued)
            }
        });
        let mut queued = false;
        let second = permit.lock().unwrap().acquire(2, &mut queued);
        assert_eq!(
            usize::from(first.join().unwrap() == Acquired::Payload)
                + usize::from(second == Acquired::Payload),
            1
        );
    });
}
