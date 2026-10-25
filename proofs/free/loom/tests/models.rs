//! Loom models of the free-mode section gate, cutter and claim word (design §5 Op sections and
//! the cut, Claim versus withdraw). Loom runs every interleaving, and its `UnsafeCell` checks
//! that no log, dirty list or scan position is touched concurrently: that exclusion is what
//! makes a cut readable while workers run. Each model also asserts its protocol property.

use hirsel_free_loom::claim::{Arbiter, Claim, Requests};
use hirsel_free_loom::phase::{Phase, Progress};
use hirsel_free_loom::section::{Gate, Scanned, Stopped, TaskLog};
use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::collections::BTreeSet;

/// The one-byte records the models write: (kind, value).
fn records(scanned: &[Scanned]) -> BTreeSet<(Vec<u32>, u8, u8)> {
    let mut out = BTreeSet::new();
    for s in scanned {
        for pair in s.bytes.chunks(2) {
            out.insert((s.path.clone(), pair[0], pair[1]));
        }
    }
    out
}

const TAKE: u8 = 1;
const SEND: u8 = 2;
const RECEIVE: u8 = 3;
const SPAWN: u8 = 4;
const END: u8 = 5;

/// Sections per worker in the closure model; loom's state space grows fast with it.
const OPS: u64 = 1;

/// Two workers take versions of one shared object in sections while a flusher cuts and then a
/// stop cuts for good: every cut's versions are exactly 0..n, a crash at any section boundary
/// therefore leaves a causally closed prefix, and no section opens after the stop.
#[test]
fn every_cut_is_causally_closed() {
    loom::model(|| {
        let gate = Arc::new(Gate::new(2));
        let version = Arc::new(AtomicU64::new(0));
        let workers: Vec<_> = (0..2u32)
            .map(|slot| {
                let (gate, version) = (gate.clone(), version.clone());
                loom::thread::spawn(move || {
                    let task = TaskLog::new(vec![slot]);
                    // SAFETY: this thread owns the new log; its sections write within this room.
                    unsafe { task.reserve(64) };
                    for op in 1..=OPS {
                        let Ok(section) = gate.enter(slot as usize) else {
                            return;
                        };
                        let v = version.fetch_add(1, Ordering::Relaxed) as u8;
                        section.append(&task, &[TAKE, v]);
                        section.publish(&task, Progress::new(op, Phase::Done));
                    }
                })
            })
            .collect();
        let mut seen = BTreeSet::new();
        let mut lengths = std::collections::BTreeMap::new();
        for stop in [false, true] {
            let mut cut = if stop {
                gate.stop_cut().unwrap()
            } else {
                gate.flush_cut().unwrap()
            };
            let scanned = cut.scan();
            for s in &scanned {
                // Progress and records are published together: two bytes per op so far.
                let length = lengths.entry(s.path.clone()).or_insert(0);
                *length += s.bytes.len();
                assert_eq!(*length as u64, 2 * s.progress.op);
            }
            seen.extend(records(&scanned).into_iter().map(|(_, _, v)| v));
            let expected: BTreeSet<u8> = (0..seen.len() as u8).collect();
            assert_eq!(seen, expected, "a cut with a hole");
            drop(cut);
        }
        assert_eq!(gate.enter(0).err(), Some(Stopped));
        for worker in workers {
            worker.join().unwrap();
        }
    });
}

/// A receive whose section found an older send, against a newer send and a cut: whenever the
/// cut holds a receive of message n, it holds the send of message n.
#[test]
fn old_receive_new_send() {
    loom::model(|| {
        let gate = Arc::new(Gate::new(2));
        let sent = Arc::new(AtomicU64::new(0));
        let sender = {
            let (gate, sent) = (gate.clone(), sent.clone());
            loom::thread::spawn(move || {
                let task = TaskLog::new(vec![0]);
                // SAFETY: this thread owns the new log; its sections write within this room.
                unsafe { task.reserve(64) };
                for op in 1..=OPS {
                    let Ok(section) = gate.enter(0) else { return };
                    let n = sent.fetch_add(1, Ordering::Relaxed) as u8;
                    section.append(&task, &[SEND, n]);
                    section.publish(&task, Progress::new(op, Phase::Done));
                }
            })
        };
        let receiver = {
            let (gate, sent) = (gate.clone(), sent.clone());
            loom::thread::spawn(move || {
                let task = TaskLog::new(vec![1]);
                // SAFETY: this thread owns the new log; its sections write within this room.
                unsafe { task.reserve(64) };
                // Entered, then one retry of the done phase under the same ordinal.
                let Ok(section) = gate.enter(1) else { return };
                section.publish(&task, Progress::new(1, Phase::Entered));
                drop(section);
                {
                    let Ok(section) = gate.enter(1) else { return };
                    if sent.load(Ordering::Relaxed) > 0 {
                        section.append(&task, &[RECEIVE, 0]);
                        section.publish(&task, Progress::new(1, Phase::Done));
                        return;
                    }
                }
            })
        };
        let mut cut = gate.flush_cut().unwrap();
        let scanned = records(&cut.scan());
        drop(cut);
        for (_, kind, n) in &scanned {
            if *kind == RECEIVE {
                assert!(
                    scanned.contains(&(vec![0], SEND, *n)),
                    "a receive without its send"
                );
            }
        }
        sender.join().unwrap();
        receiver.join().unwrap();
    });
}

/// A spawn registers its child in the parent's section and the child ends in its own, while a
/// cutter scans twice: a cut that shows the child's end shows its registration and the
/// parent's spawn, and the terminal progress list holds every registered task.
#[test]
fn registration_and_end_during_a_cut() {
    loom::model(|| {
        let gate = Arc::new(Gate::new(2));
        let parent = {
            let gate = gate.clone();
            loom::thread::spawn(move || {
                let parent = TaskLog::new(vec![]);
                // SAFETY: this thread owns the new log; its sections write within this room.
                unsafe { parent.reserve(64) };
                let child = TaskLog::new(vec![0]);
                // SAFETY: this thread owns the new log; its sections write within this room.
                unsafe { child.reserve(64) };
                {
                    let Ok(section) = gate.enter(0) else { return };
                    section.register(&child);
                    section.append(&parent, &[SPAWN, 0]);
                    section.publish(&parent, Progress::new(1, Phase::Done));
                }
                // The child runs on the other worker, migrated through a happens-before edge.
                let gate = gate.clone();
                loom::thread::spawn(move || {
                    let Ok(section) = gate.enter(1) else { return };
                    section.append(&child, &[END, 0]);
                    section.publish(&child, Progress::new(1, Phase::Done));
                })
                .join()
                .unwrap();
            })
        };
        let mut seen = BTreeSet::new();
        let mut registered = BTreeSet::new();
        for stop in [false, true] {
            let mut cut = if stop {
                gate.stop_cut().unwrap()
            } else {
                gate.flush_cut().unwrap()
            };
            let scanned = cut.scan();
            registered.extend(scanned.iter().map(|s| s.path.clone()));
            seen.extend(records(&scanned));
            if seen.contains(&(vec![0], END, 0)) {
                assert!(
                    seen.contains(&(vec![], SPAWN, 0)),
                    "an end without its spawn"
                );
                assert!(registered.contains(&vec![0]));
            }
            if stop {
                let listed: BTreeSet<Vec<u32>> =
                    cut.progress().into_iter().map(|(path, _)| path).collect();
                assert_eq!(listed, registered);
            }
            drop(cut);
        }
        parent.join().unwrap();
    });
}

/// The buffers a request record owns: counts its frees.
struct Buffers(Arc<AtomicUsize>);
impl Drop for Buffers {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// The claim matrix: one host request (entered, claimed, the host call, ended, done) against a
/// stop at every point, with and without recording. Exactly one of claim and withdraw wins; a
/// withdrawn request never calls the host; a claimed one at the stop is in doubt; a completion
/// after the terminal cut is quarantined, never recorded; and the buffers are freed once,
/// by whichever holder leaves last.
fn claim_matrix(recording: bool) {
    loom::model(move || {
        let arbiter = Arc::new(Arbiter::new(recording, 2));
        let requests = Arc::new(Requests::default());
        let issued = Arc::new(AtomicBool::new(false));
        let frees = Arc::new(AtomicUsize::new(0));
        let prepared = Arc::new(AtomicBool::new(false));
        let task = TaskLog::new(vec![0]);
        // SAFETY: this thread owns the new log; its sections write within this room.
        unsafe { task.reserve(64) };
        let issuer = {
            let (arbiter, requests, issued, frees, task, prepared) = (
                arbiter.clone(),
                requests.clone(),
                issued.clone(),
                frees.clone(),
                task.clone(),
                prepared.clone(),
            );
            loom::thread::spawn(move || -> Option<bool> {
                // Entered: Prepared, in a section.
                let section = arbiter.section(0).ok()?;
                let request = requests.prepare(1, Buffers(frees));
                prepared.store(true, Ordering::Relaxed);
                if let Some(section) = &section {
                    section.publish(&task, Progress::new(1, Phase::Entered));
                }
                drop(section);
                // Claimed: the CAS in its section.
                let section = arbiter.section(0).ok()?;
                if !request.claim() {
                    return Some(false);
                }
                if let Some(section) = &section {
                    section.publish(&task, Progress::new(1, Phase::Claimed));
                }
                drop(section);
                // The host call, outside sections; the stop never waits for it.
                issued.store(true, Ordering::Relaxed);
                // Done: the completion, or quarantine after the terminal cut.
                match arbiter.section(0) {
                    Ok(section) => {
                        request.finish();
                        if let Some(section) = &section {
                            section.append(&task, &[END, 1]);
                            section.publish(&task, Progress::new(1, Phase::Done));
                        }
                        drop(section);
                        requests.retire(1);
                        Some(true)
                    }
                    Err(Stopped) => None,
                }
            })
        };
        let terminal = arbiter.stop(0, &[&*requests]).expect("the only stop wins");
        let recorded = terminal.cut.map(|mut cut| records(&cut.scan()));
        let sweep = terminal.sweep;
        let outcome = issuer.join().unwrap();
        let issued = issued.load(Ordering::Relaxed);
        let withdrawn = sweep.withdrawn == vec![1];
        assert!(!(withdrawn && issued), "a withdrawn request issued");
        assert!(sweep.withdrawn.len() + sweep.in_doubt.len() <= 1);
        if outcome == Some(false) {
            assert!(withdrawn, "a failed claim means the sweep withdrew it");
        }
        if !sweep.in_doubt.is_empty() {
            assert!(!withdrawn);
        }
        if let Some(recorded) = recorded {
            if recorded.contains(&(vec![0], END, 1)) {
                assert!(
                    issued && outcome == Some(true),
                    "a recorded completion without its call"
                );
                assert!(sweep.in_doubt.is_empty());
            } else if outcome == Some(true) {
                unreachable!("a delivered completion missing from the terminal cut");
            }
        }
        drop(requests);
        let prepared = prepared.load(Ordering::Relaxed);
        assert_eq!(
            frees.load(Ordering::Relaxed),
            usize::from(prepared),
            "the buffers are freed once"
        );
    });
}

#[test]
fn claim_matrix_recording() {
    claim_matrix(true);
}

#[test]
fn claim_matrix_without_recording() {
    claim_matrix(false);
}

/// The home bridge: a posted request is claimed by the home thread by CAS, and closing the
/// bridge withdraws every unclaimed one; home never acts on a withdrawn request.
#[test]
fn bridge_claim_and_close() {
    loom::model(|| {
        let bridge = Arc::new(Requests::<()>::default());
        let acted = Arc::new(AtomicBool::new(false));
        let posted = bridge.prepare(7, ());
        let home = {
            let acted = acted.clone();
            loom::thread::spawn(move || {
                if posted.claim() {
                    acted.store(true, Ordering::Relaxed);
                    posted.finish();
                }
                posted.state()
            })
        };
        let sweep = bridge.sweep();
        let state = home.join().unwrap();
        let acted = acted.load(Ordering::Relaxed);
        assert_eq!(acted, sweep.withdrawn.is_empty());
        assert_eq!(state, if acted { Claim::Done } else { Claim::Withdrawn });
        // A request posted after the close is born withdrawn.
        assert!(!bridge.prepare(8, ()).claim());
    });
}
