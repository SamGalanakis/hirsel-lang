//! Executor-owned recording slots on real dispatcher execs: a home owner's nested callback
//! and two runs whose home is one thread (Tokio `rt_handle` nested entry and
//! `rt_worker_index` worker versus external identity, with Hirsel's nesting policy).

use super::*;
use crate::pool::TestExec;
use hirsel_free::concurrent::HOME;
use hirsel_free::phase::{Phase, Progress};

/// A recording with its root registered at admission, as [`Recording::root`] does.
fn admitted(binding: &[u8]) -> (Arc<Recording>, hirsel_free::concurrent::Task) {
    let recording = Recording::new(binding, false);
    let mut home = recording.recorder.executor(HOME).unwrap();
    let root = recording.recorder.root(&mut home).unwrap();
    recording.recorder.retire_executor(home);
    (recording, root)
}

fn step(recording: &Recording, slot: &mut ExecutorSlot, root: &mut hirsel_free::concurrent::Task) {
    let next = Progress::new(root.progress().op + 1, Phase::Done);
    root.reserve(PHASE);
    recording
        .recorder
        .phase(slot, root, next, |w| w.load(1))
        .unwrap();
}

#[test]
fn a_nested_home_callback_registers_a_distinct_active_slot() {
    let (recording, mut root) = admitted(b"nested home");
    let home = TestExec::new();
    home.enter(|registration| unsafe {
        registration.register_with(&recording, HOME, false);
        assert_eq!(registration.index(), Some(HOME));
        let mut outer = checkout(&recording);
        assert!(outer.own);
        assert_eq!(outer.slot.index(), HOME);
        // A callback re-entering this executor while its slot is checked out: it cannot
        // reuse the active word, so it runs on a distinct transient one.
        let mut nested = checkout(&recording);
        assert!(!nested.own);
        assert!(nested.slot.index() >= TRANSIENT);
        step(&recording, &mut nested.slot, &mut root);
        let nested_index = nested.slot.index();
        checkin(&recording, nested);
        assert_eq!(recording.recorder.registered(), vec![HOME]);
        assert!(recording.recorder.executor(nested_index).is_some());
        step(&recording, &mut outer.slot, &mut root);
        checkin(&recording, outer);
        // The executor keeps its registration across phases until it leaves.
        assert_eq!(registration.index(), Some(HOME));
        let again = checkout(&recording);
        assert!(again.own);
        checkin(&recording, again);
        registration.retire_from(&recording);
    });
    assert!(recording.recorder.registered().is_empty());
    assert_eq!(
        recording
            .transients
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    let cut = recording.recorder.flush().unwrap().unwrap();
    assert_eq!(cut.progress[&Vec::new()], Progress::new(2, Phase::Done));
}

#[test]
fn two_runs_sharing_a_home_thread_keep_their_own_registrations() {
    let (a, mut root_a) = admitted(b"run a");
    let (b, mut root_b) = admitted(b"run b");
    let (home_a, home_b) = (TestExec::new(), TestExec::new());
    unsafe {
        home_a.enter(|r| r.register_with(&a, HOME, false));
        home_b.enter(|r| r.register_with(&b, HOME, false));
    }
    // A multi-run home owner alternates between its runs' homes: each phase uses that run's
    // own registration, with no retirement or re-registration between them.
    for _ in 0..4 {
        home_a.enter(|_| {
            let mut slot = checkout(&a);
            assert!(slot.own);
            step(&a, &mut slot.slot, &mut root_a);
            checkin(&a, slot);
        });
        home_b.enter(|_| {
            let mut slot = checkout(&b);
            assert!(slot.own);
            step(&b, &mut slot.slot, &mut root_b);
            checkin(&b, slot);
        });
    }
    // Run b's home nested inside run a's home (a nested launch on this thread): run a's phase
    // there is an external caller of b's executor and takes a transient slot of its own run.
    home_a.enter(|_| {
        home_b.enter(|_| {
            let mut slot = checkout(&a);
            assert!(!slot.own);
            step(&a, &mut slot.slot, &mut root_a);
            checkin(&a, slot);
        })
    });
    assert_eq!(a.recorder.registered(), vec![HOME]);
    assert_eq!(b.recorder.registered(), vec![HOME]);
    unsafe {
        home_a.enter(|r| r.retire_from(&a));
        home_b.enter(|r| r.retire_from(&b));
    }
    assert!(a.recorder.registered().is_empty() && b.recorder.registered().is_empty());
    let transients = |r: &Recording| r.transients.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!((transients(&a), transients(&b)), (1, 0));
    assert_eq!(
        a.recorder.flush().unwrap().unwrap().progress[&Vec::new()],
        Progress::new(5, Phase::Done)
    );
    assert_eq!(
        b.recorder.flush().unwrap().unwrap().progress[&Vec::new()],
        Progress::new(4, Phase::Done)
    );
}
