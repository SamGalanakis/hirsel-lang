use super::super::*;
use std::time::Duration;

#[test]
fn exact_deadline_storage_tracks_rearm_cancel_and_domain_exit() {
    for virtual_time in [false, true] {
        let driver = if virtual_time {
            Readiness::virtual_clock(10)
        } else {
            Readiness::default()
        };
        let now = Instant::now();
        for token in 0..1024 {
            driver.enter(token);
            if virtual_time {
                driver.pending().virtual_deadline = Some(1000 + token);
            } else {
                driver.pending().deadline = Some(now + Duration::from_secs(1000 + token));
            }
            driver.parked(token);
        }
        assert!(driver.ready().unwrap().is_empty());
        {
            let state = driver.0.lock().unwrap();
            let expected = (1024, 1024);
            assert_eq!(
                state.timers.stored(),
                if virtual_time { (0, 0) } else { expected }
            );
            assert_eq!(
                state.virtual_timers.stored(),
                if virtual_time { expected } else { (0, 0) }
            );
        }
        // Rescheduling one token replaces its old entry, including a move
        // from a timer to a non-timer source; no stale entry is retained.
        driver.enter(0);
        {
            let mut p = driver.pending();
            p.deadline = None;
            p.virtual_deadline = None;
            p.delivery = true;
        }
        assert!(driver.ready().unwrap().is_empty());
        assert_eq!(driver.ready_deadline(), None);
        driver.finish(0);
        if !virtual_time {
            assert!(driver.ready_deadline().is_some());
        }
        for token in 1..1024 {
            driver.finish(token);
        }
        let state = driver.0.lock().unwrap();
        assert_eq!(state.timers.stored(), (0, 0));
        assert_eq!(state.virtual_timers.stored(), (0, 0));
        assert_eq!(state.non_timer, 0);
    }
}

#[test]
fn scripted_readiness_indexes_registration_races_and_only_live_subscribers() {
    let driver = Readiness::default();
    for token in 0..1024 {
        driver.enter(token);
        driver.scripted();
        driver.parked(token);
    }
    assert!(driver.ready().unwrap().is_empty());
    assert!(driver.0.lock().unwrap().scripted_ready.is_empty());
    assert!(!driver.wait().unwrap());
    driver.progressed();
    assert_eq!(driver.ready().unwrap().len(), 1024);
    for token in 0..1024 {
        driver.finish(token);
    }
    assert!(driver.0.lock().unwrap().scripted.is_empty());
    assert!(driver.0.lock().unwrap().scripted_ready.is_empty());
    // Capture the request epoch before a progress/poll racing registration.
    let since = driver.0.lock().unwrap().progress;
    driver.progressed();
    assert!(driver.ready().unwrap().is_empty());
    driver.enter(2048);
    driver.pending().scripted = Some(ScriptedWait::Progress(since));
    assert!(driver.ready().unwrap().is_empty(), "owner has not parked");
    driver.parked(2048);
    assert_eq!(driver.ready().unwrap(), vec![2048]);
    driver.scripted();
    assert!(
        driver.ready().unwrap().is_empty(),
        "rearm acknowledges progress"
    );
    driver.finish(2048);
    assert!(driver.0.lock().unwrap().scripted_ready.is_empty());
}
