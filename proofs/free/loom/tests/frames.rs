//! The production saved-frame hand-off (crates/hirsel-free/src/frame.rs over schedule.rs's
//! state word) in two transitions of the physical dispatcher (crates/hirsel-rt/src/pool/free.rs):
//! a compact frame leaving its task's stack and activating on another worker's lent stack, and
//! a stale wake whose dispatch is refused and returns the frame saved and unstarted. Loom's
//! `UnsafeCell` checks that no worker reads an image its leave has not published.

hirsel_guard::install!();
use hirsel_free_loom::frame::{self, Queued};

/// A frame image: the continuation a compact leave records, and the stack it runs on.
struct Image {
    compact: Option<u32>,
    stack: Option<u32>,
}

/// Activate a compact frame as `resume` does: the token's start acquires the leave's image, the
/// frame has no stack of its own, and this worker lends it one before it parks again.
fn activate(queued: Queued<Image>, lent: u32) {
    let mut running = queued.start();
    assert_eq!(
        running.value().compact,
        Some(7),
        "activated before the leave's image"
    );
    assert_eq!(
        running.value().stack,
        None,
        "a left frame still owns its old stack"
    );
    running.value_mut().stack = Some(lent);
    assert!(running.save(false).is_none());
}

/// A compact frame leaves worker 0's stack while a role-free wake races the leave: the frame
/// activates exactly once, on another worker, from the image the leave saved.
#[test]
fn a_compact_frame_activates_once_from_its_left_image() {
    hirsel_free_loom::sync::model(|| {
        let (wake, queued) = frame::prepared(Image {
            compact: None,
            stack: Some(0),
        });
        let mut running = queued.start();
        let waker =
            loom::thread::spawn(move || wake.notify().map(|queued| activate(queued, 1)).is_some());
        // The leave: the continuation is recorded, the stack goes back to worker 0's pool.
        running.value_mut().compact = Some(7);
        running.value_mut().stack = None;
        let published = running.save(false);
        let left_queued = published.is_some();
        if let Some(queued) = published {
            loom::thread::spawn(move || activate(queued, 2))
                .join()
                .unwrap();
        }
        let woke = waker.join().unwrap();
        assert!(left_queued != woke, "a left frame activated twice or never");
    });
}

/// A wake reaches a running frame that then parks: the wake is remembered and the save
/// requeues it, or it finds the frame saved and takes the token itself; either way exactly
/// one dispatch follows. That dispatch is refused (the row is parked on another wait) and
/// returns the image unstarted: with no newer wake, the frame is idle again, so the quiet
/// verdict the save deferred to this wake can be taken.
#[test]
fn a_remembered_wake_is_dispatched_and_its_refusal_leaves_the_frame_idle() {
    hirsel_free_loom::sync::model(|| {
        let (wake, queued) = frame::prepared(0u32);
        let running = queued.start();
        let observer = wake.clone();
        let refuse = move |queued: Queued<u32>| {
            // As pool/free.rs `resume` refusing a stale token: the production return path.
            match queued.start().return_unstarted() {
                frame::Returned::Requeued(_) => panic!("a refused dispatch requeued itself"),
                frame::Returned::Idle(quiet) => quiet.take(|| {
                    assert!(observer.idle(), "the quiet verdict saw a busy frame");
                }),
            }
        };
        let stale = {
            let refuse = refuse.clone();
            loom::thread::spawn(move || wake.notify().map(refuse).is_some())
        };
        // The task parks; its save callback defers the verdict when a wake is remembered.
        let requeued = running.save(false).map(refuse).is_some();
        let dispatched = stale.join().unwrap();
        assert!(
            requeued != dispatched,
            "a stale wake was lost or dispatched twice"
        );
    });
}

/// The compact leave's placement words (`frame::Placement`, pool/free.rs `saved` and `notify`):
/// a frame leaving executor A while a wake publishes it. Whichever side dispatches it, the
/// router reads A as the executor it left, so forced migration sends it elsewhere; the
/// activation then clears the word.
#[test]
fn every_dispatch_of_a_left_compact_frame_routes_from_its_executor() {
    use hirsel_free_loom::frame::Placement;
    use loom::sync::Arc;
    const A: usize = 8;
    hirsel_free_loom::sync::model(|| {
        let placement = Arc::new(Placement::new(A | 3));
        let (wake, queued) = frame::prepared(Image {
            compact: None,
            stack: Some(0),
        });
        let mut running = queued.start();
        let dispatch = {
            let placement = placement.clone();
            move |queued: Queued<Image>| {
                assert_eq!(placement.owner(), 0, "a left frame keeps a pinned owner");
                assert_eq!(placement.left(), A, "routed without the executor it left");
                let running = queued.start();
                placement.activated();
                drop(running);
            }
        };
        let waker = {
            let (placement, dispatch) = (placement.clone(), dispatch.clone());
            loom::thread::spawn(move || {
                // A role-free token's owner: any worker.
                placement.set_owner(0);
                wake.notify().map(dispatch).is_some()
            })
        };
        running.value_mut().compact = Some(7);
        running.value_mut().stack = None;
        let requeued = placement.leave(running, A, || {}).map(dispatch).is_some();
        assert!(requeued != waker.join().unwrap());
        assert_eq!(placement.left(), 0);
    });
}
