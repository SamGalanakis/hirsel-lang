//! The production queue/state word under loom's memory model. Every token
//! must have exactly one owner; wakes racing the saved-stack publication
//! cannot enqueue a running fiber or disappear at park.

hirsel_guard::install!();
use hirsel_free_loom::{queue, schedule::TaskState};
use loom::sync::Arc;
use loom::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn a_queue_token_cannot_expose_a_frame_before_the_idle_save() {
    use hirsel_free_loom::frame;
    hirsel_free_loom::sync::model(|| {
        let (wake, queued) = frame::prepared(0usize);
        let mut running = queued.start();
        let thread = loom::thread::spawn(move || {
            wake.notify().is_some_and(|queued| {
                let running = queued.start();
                assert_eq!(*running.value(), 42);
                true
            })
        });
        // A real executor writes the saved stack pointer here on its idle
        // continuation. A racing notification must never read the old image.
        *running.value_mut() = 42;
        let queued = running.save(false);
        let here = queued.is_some_and(|queued| {
            let running = queued.start();
            assert_eq!(*running.value(), 42);
            true
        });
        let remote = thread.join().unwrap();
        assert_eq!(usize::from(here) + usize::from(remote), 1);
    });
}

#[test]
fn discarded_queue_and_running_owners_drop_images_once_and_ignore_late_wakes() {
    use hirsel_free_loom::frame;
    struct Image(Arc<AtomicUsize>);
    impl Drop for Image {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    for started in [false, true] {
        hirsel_free_loom::sync::model(move || {
            let freed = Arc::new(AtomicUsize::new(0));
            let (wake, queued) = frame::prepared(Image(freed.clone()));
            let late = wake.clone();
            let thread = loom::thread::spawn(move || assert!(wake.notify().is_none()));
            if started {
                drop(queued.start());
            } else {
                drop(queued);
            }
            thread.join().unwrap();
            assert!(late.notify().is_none());
            assert_eq!(freed.load(Ordering::Relaxed), 1);
        });
    }
}

#[test]
fn a_wake_racing_park_publishes_exactly_once_after_stack_save() {
    hirsel_free_loom::sync::model(|| {
        let state = Arc::new(TaskState::default());
        assert!(state.notify());
        assert!(state.start());
        let publications = Arc::new(AtomicUsize::new(0));
        let thread = loom::thread::spawn({
            let (state, publications) = (state.clone(), publications.clone());
            move || {
                if state.notify() {
                    publications.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        if state.suspend(false) {
            publications.fetch_add(1, Ordering::Relaxed);
        }
        thread.join().unwrap();
        assert_eq!(publications.load(Ordering::Relaxed), 1);
        assert!(state.start());
        assert!(!state.start());
        state.complete();
        assert!(!state.notify());
    });
}

#[test]
fn two_thieves_and_owner_each_get_disjoint_tokens() {
    hirsel_free_loom::sync::model(|| {
        let (mut owner, thief) = queue::local::<_, 4>();
        for id in 0..3 {
            assert!(owner.push(id).is_empty());
        }
        let a = loom::thread::spawn({
            let thief = thief.clone();
            move || thief.steal_half()
        });
        let b = loom::thread::spawn(move || thief.steal_half());
        let mut taken = owner.drain();
        taken.extend(a.join().unwrap());
        taken.extend(b.join().unwrap());
        taken.sort_unstable();
        assert_eq!(taken, [0, 1, 2]);
    });
}

#[test]
fn overflow_racing_steal_and_reuse_loses_no_token() {
    hirsel_free_loom::sync::model(|| {
        let (mut owner, thief) = queue::local::<_, 4>();
        for id in 0..4 {
            assert!(owner.push(id).is_empty());
        }
        let thread = loom::thread::spawn(move || thief.steal_half());
        let mut taken = owner.push(4);
        taken.extend(owner.push(5));
        taken.extend(owner.drain());
        taken.extend(thread.join().unwrap());
        taken.sort_unstable();
        assert_eq!(taken, [0, 1, 2, 3, 4, 5]);
    });
}

#[test]
fn a_wake_racing_completion_never_resurrects_a_task() {
    hirsel_free_loom::sync::model(|| {
        let state = Arc::new(TaskState::default());
        assert!(state.notify());
        assert!(state.start());
        let thread = loom::thread::spawn({
            let state = state.clone();
            move || assert!(!state.notify())
        });
        state.complete();
        thread.join().unwrap();
        assert!(!state.notify());
        assert!(!state.start());
    });
}

#[test]
fn first_publication_racing_a_thief_publishes_initialized_ring_storage() {
    hirsel_free_loom::sync::model(|| {
        let (mut owner, thief) = queue::local::<u32, 4>();
        let thread = loom::thread::spawn(move || thief.steal_half());
        assert!(owner.push(7).is_empty());
        let mut taken = owner.drain();
        taken.extend(thread.join().unwrap());
        assert_eq!(taken, [7]);
    });
}
