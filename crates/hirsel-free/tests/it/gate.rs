//! The section gate and the claim word under real threads: every flush cut is causally
//! closed, a stop's cut is final, and exactly one of claim and withdraw wins.

use hirsel_free::claim::{Arbiter, Claim, Requests};
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::record::{Reader, put_varint};
use hirsel_free::section::{Gate, TaskLog};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[test]
fn every_cut_is_causally_closed() {
    const WORKERS: usize = 4;
    let gate = Arc::new(Gate::new(WORKERS));
    // A shared object: each section takes its next version and records it.
    let version = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..WORKERS)
        .map(|slot| {
            let (gate, version, done) = (gate.clone(), version.clone(), done.clone());
            std::thread::spawn(move || {
                let task = TaskLog::new(vec![slot as u32]);
                let Ok(section) = gate.enter(slot) else {
                    return;
                };
                section.register(&task);
                drop(section);
                let mut op = 0;
                while !done.load(Ordering::Relaxed) {
                    // SAFETY: this thread owns the task's writing side; no section is open.
                    unsafe { task.reserve(10) };
                    let Ok(section) = gate.enter(slot) else {
                        return;
                    };
                    op += 1;
                    let v = version.fetch_add(1, Ordering::Relaxed);
                    let mut bytes = Vec::new();
                    put_varint(&mut bytes, u128::from(v));
                    section.append(&task, &bytes);
                    section.publish(&task, Progress::new(op, Phase::Done));
                }
            })
        })
        .collect();
    let mut seen = Vec::new();
    let mut progress = std::collections::HashMap::new();
    // Flush while the workers run (at least 300 cuts and 2000 versions), then stop.
    let mut round = 0;
    loop {
        let stop = round >= 300 && version.load(Ordering::Relaxed) >= 2000;
        let mut cut = if stop {
            gate.stop_cut().unwrap()
        } else {
            gate.flush_cut().unwrap()
        };
        for scanned in cut.scan() {
            let mut reader = Reader::new(&scanned.bytes);
            let mut n = 0;
            while !reader.done() {
                seen.push(reader.u64().unwrap());
                n += 1;
            }
            let before = progress
                .insert(scanned.path.clone(), scanned.progress)
                .map_or(0, |p: Progress| p.op);
            assert_eq!(
                scanned.progress.op,
                before + n,
                "progress and records are published together"
            );
        }
        // Closed: the versions recorded so far are exactly 0..n, with no hole a later section
        // could still fill.
        seen.sort_unstable();
        assert!(
            seen.iter().enumerate().all(|(i, &v)| v == i as u64),
            "a hole in round {round}"
        );
        drop(cut);
        if stop {
            break;
        }
        round += 1;
    }
    assert!(gate.stopped());
    done.store(true, Ordering::Relaxed);
    for worker in workers {
        worker.join().unwrap();
    }
    assert!(gate.enter(0).is_err(), "no section opens after the stop");
    assert!(!seen.is_empty());
}

#[test]
fn claim_and_withdraw_race_to_exactly_one_winner() {
    for _ in 0..2000 {
        let requests = Arc::new(Requests::<Vec<u8>>::default());
        let request = requests.prepare(1, vec![0; 64]);
        let issuer = {
            let request = request.clone();
            std::thread::spawn(move || request.claim())
        };
        let arbiter = Arbiter::new(false, 1);
        let terminal = arbiter.stop(0, &[&*requests]).unwrap();
        let claimed = issuer.join().unwrap();
        assert_eq!(claimed, terminal.sweep.withdrawn.is_empty());
        assert_eq!(claimed, terminal.sweep.in_doubt == vec![1]);
        assert_eq!(
            request.state(),
            if claimed {
                Claim::ClaimedForIssue
            } else {
                Claim::Withdrawn
            }
        );
        // After the sweep a new request is born withdrawn and can never issue.
        assert!(!requests.prepare(2, vec![]).claim());
        assert!(
            arbiter.section(0).is_err(),
            "a completion after the stop is quarantined"
        );
    }
}
