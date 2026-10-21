//! The production queue/state word under loom's memory model. Every token
//! must have exactly one owner; wakes racing the saved-stack publication
//! cannot enqueue a running fiber or disappear at park.
use hirsel_free_loom::{queue, schedule::TaskState};
use loom::sync::Arc;
use loom::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn a_wake_racing_park_publishes_exactly_once_after_stack_save() {
    loom::model(|| {
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
    loom::model(|| {
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
    loom::model(|| {
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
    loom::model(|| {
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
