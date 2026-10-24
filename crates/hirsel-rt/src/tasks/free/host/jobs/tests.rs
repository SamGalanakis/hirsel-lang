use super::*;
use std::sync::mpsc;
use std::time::Duration;

fn unit() -> Output {
    (Ok(Completion::Unit(Ok(()))), None)
}

fn wait_for(mut done: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "job never returned"
        );
        std::thread::yield_now();
    }
}

#[test]
fn abandoned_job_retains_credit_until_return_then_grants_the_oldest_waiter() {
    let jobs = Jobs::new(1);
    let mut first = jobs.reserve(1);
    assert!(first.granted());
    let (release, held) = mpsc::channel::<()>();
    let slot = jobs.submit(first.take().unwrap(), 1, move || {
        held.recv().unwrap();
        unit()
    });
    let mut second = jobs.reserve(2);
    let mut third = jobs.reserve(3);
    assert!(!second.granted() && !third.granted());
    // The owner is gone (stop teardown); the call has not returned.
    assert!(slot.abandon());
    drop(slot);
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(jobs.held(), 1, "abandonment returned physical capacity");
    assert!(!second.granted());
    release.send(()).unwrap();
    wait_for(|| second.granted());
    assert!(!third.granted(), "FIFO grant");
    assert_eq!(jobs.drain(), [1, 2]);
    let stats = jobs.stats();
    assert_eq!(stats.quarantined.load(Ordering::Relaxed), 1);
    assert_eq!(stats.delivered.load(Ordering::Relaxed), 0);
    wait_for(|| stats.reclaimed.load(Ordering::Relaxed) == 1);
    // A granted but untaken reservation passes its credit on when dropped.
    drop(second);
    wait_for(|| third.granted());
    drop(third);
    assert_eq!(jobs.held(), 0);
}

#[test]
fn delivery_and_quarantine_have_exactly_one_winner() {
    let jobs = Jobs::new(4);
    for round in 0..200u64 {
        let reservation = jobs.reserve(round);
        let slot = jobs.submit(reservation.take().unwrap(), round, unit);
        let racer = slot.clone();
        let owner = std::thread::spawn(move || {
            wait_for(|| racer.returned() || racer.word.load(Ordering::Acquire) > RETURNED);
            racer.deliver().is_some()
        });
        let abandoned = round % 2 == 0 && slot.abandon();
        let delivered = owner.join().unwrap();
        assert!(!(delivered && abandoned), "round {round}");
        drop(slot);
    }
    let stats = jobs.stats();
    wait_for(|| stats.returned.load(Ordering::Relaxed) == 200);
    let delivered = stats.delivered.load(Ordering::Relaxed);
    let quarantined = stats.quarantined.load(Ordering::Relaxed);
    assert_eq!(delivered + quarantined, 200);
    wait_for(|| stats.reclaimed.load(Ordering::Relaxed) == quarantined);
    wait_for(|| jobs.held() == 0);
}

#[test]
fn withdrawn_queued_reservation_never_issues_and_keeps_its_place_free() {
    let jobs = Jobs::new(1);
    let first = jobs.reserve(1);
    let queued = jobs.reserve(2);
    let mut later = jobs.reserve(3);
    drop(queued);
    drop(first);
    assert!(later.granted());
    assert_eq!(jobs.drain(), [3]);
    assert_eq!(jobs.stats().submitted.load(Ordering::Relaxed), 0);
}
