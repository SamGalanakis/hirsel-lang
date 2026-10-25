//! The production executor registry (crates/hirsel-free/src/executors.rs) against the section
//! gate. An executor that retires after its section word is acknowledged inactive hands the
//! word to a registration waiting for it: the waiter is woken (never lost), and its sections
//! on the same word are ordered after the retired owner's, which loom's `UnsafeCell` checks on
//! the word's dirty list. The run's stop wakes a waiter, which then starts unregistered.

hirsel_guard::install!();
use hirsel_free_loom::executors::Executors;
use hirsel_free_loom::phase::{Phase, Progress};
use hirsel_free_loom::section::{Gate, TaskLog};
use loom::sync::Arc;

/// One executor's life on the run: register (waiting for a held word), one phase in a
/// section, then retirement after the acknowledgement. `Some(word)` when it registered.
fn serve(executors: &Executors, gate: &Gate, path: u32) -> Option<usize> {
    let slot = executors.register(1, || gate.stopped())?;
    let task = TaskLog::new(&[path]);
    // SAFETY: this thread owns the new log; its section writes within this room.
    unsafe { task.reserve(64) };
    if let Ok(section) = gate.enter(slot.index()) {
        section.append(&task, &[path as u8]);
        section.publish(&task, Progress::new(1, Phase::Done));
    }
    assert!(gate.inactive(slot.index()), "retiring with a section open");
    let word = slot.index();
    drop(slot);
    Some(word)
}

/// Two executors prefer the one non-home word: whichever registers second waits for the
/// first's retirement, then runs on the same word, while a flush cuts. Neither is left
/// waiting, both phases reach the terminal cut, and home's word is never taken.
#[test]
fn a_retirement_hands_its_word_to_a_waiting_registration() {
    hirsel_free_loom::sync::model(|| {
        let executors = Executors::new(2);
        let gate = Arc::new(Gate::new(2));
        let starts: Vec<_> = (0..2u32)
            .map(|path| {
                let (executors, gate) = (executors.clone(), gate.clone());
                loom::thread::spawn(move || serve(&executors, &gate, path))
            })
            .collect();
        drop(gate.flush_cut().unwrap());
        let words: Vec<_> = starts.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(words, [Some(1), Some(1)]);
        assert!(executors.registered().is_empty());
        let mut cut = gate.stop_cut().unwrap();
        let mut paths: Vec<_> = cut.scan().into_iter().map(|s| s.path).collect();
        paths.sort();
        assert_eq!(paths, [vec![0], vec![1]]);
    });
}

/// A registration waiting behind an executor that has not retired when the run stops: the
/// stop's wake releases it unregistered, and it never takes home's word.
#[test]
fn the_stop_releases_a_waiting_registration() {
    hirsel_free_loom::sync::model(|| {
        let executors = Executors::new(2);
        let gate = Arc::new(Gate::new(2));
        let held = executors.executor(1).unwrap();
        let waiter = {
            let (executors, gate) = (executors.clone(), gate.clone());
            loom::thread::spawn(move || executors.register(1, || gate.stopped()).is_some())
        };
        drop(gate.stop_cut().unwrap());
        executors.wake();
        assert!(!waiter.join().unwrap(), "registered past the stop");
        assert_eq!(executors.registered(), [1]);
        drop(held);
    });
}

/// Tokio `blocking_shutdown` (`runtime/tests/loom_blocking.rs:7-25`): two blocking executors
/// start against one free word while the run stops. Each either registered and later retires,
/// or the stop released it unregistered; afterwards no word is held and nobody waits.
#[test]
fn executors_starting_against_the_stop_all_leave() {
    hirsel_free_loom::sync::model(|| {
        let executors = Executors::new(2);
        let gate = Arc::new(Gate::new(2));
        let starts: Vec<_> = (0..2u32)
            .map(|path| {
                let (executors, gate) = (executors.clone(), gate.clone());
                loom::thread::spawn(move || serve(&executors, &gate, path))
            })
            .collect();
        drop(gate.stop_cut().unwrap());
        executors.wake();
        for start in starts {
            assert!(matches!(start.join().unwrap(), None | Some(1)));
        }
        assert!(executors.registered().is_empty());
    });
}
