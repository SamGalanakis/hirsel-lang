use super::super::tests::Clock;
use super::super::*;

fn service(clock: Arc<Clock>) -> Service {
    Service {
        adapter: clock,
        live: Default::default(),
        driver: None,
        rows: Default::default(),
        pending: AtomicUsize::new(0),
        opaque: Default::default(),
        timers: Default::default(),
    }
}

fn scope(generation: u32, token: Option<u64>) -> Scope {
    Scope {
        generation,
        owner: 0,
        children: Vec::new(),
        canceled: Default::default(),
        deadline: Some(ScopeDeadline {
            token,
            expired: false,
        }),
        completions: Default::default(),
    }
}

#[test]
fn runtime50_timer_index_visits_only_affected() {
    let sizes = std::env::var("HIRSEL_RUNTIME50_IDLE")
        .map(|s| vec![s.parse::<u32>().unwrap()])
        .unwrap_or_else(|_| vec![256, 512, 1024]);
    for idle in sizes {
        let clock = Arc::new(Clock::default());
        let io = service(clock.clone());
        let mut s = Scheduler::new(Schedule::default(), std::ptr::null_mut());
        let mut tokens = Vec::new();
        for slot in 0..idle {
            let token = io
                .arm(
                    ScopeKey {
                        slot: slot,
                        generation: 2,
                    },
                    11,
                    100,
                )
                .unwrap()
                .unwrap();
            tokens.push(token);
            s.scopes.push(Some(scope(2, Some(token))));
        }
        clock.due.store(true, Ordering::Release);
        io.timers
            .poll(&*clock, &crate::io::Ready::Tokens(vec![tokens[0]]))
            .unwrap();
        assert!(io.timers.expire(&mut s));
        assert!(s.scope(0).deadline.as_ref().unwrap().expired);
        {
            let table = io.timers.0.lock().unwrap();
            assert_eq!(
                table.visits.load(Ordering::Relaxed),
                2,
                "one ready lookup plus the completed prefix"
            );
            assert_eq!(table.rows.len(), idle as usize - 1);
            assert_eq!(
                table.deadlines.stored(),
                (idle as usize - 1, idle as usize - 1)
            );
            println!(
                "idle={idle} service_visits={} index_entries={}",
                table.visits.load(Ordering::Relaxed),
                table.rows.len()
            );
        }
        assert_eq!(*clock.finishes.lock().unwrap(), vec![tokens[0]]);
        for token in tokens {
            io.disarm(token);
        }
        let table = io.timers.0.lock().unwrap();
        assert!(table.rows.is_empty());
        assert!(table.due.is_empty());
        assert_eq!(table.deadlines.stored(), (0, 0));
    }
}

#[test]
fn runtime50_timer_exit_equality_and_reuse() {
    // Production readiness uses inclusive equality and distinct clock indexes.
    let driver = crate::readiness::Readiness::virtual_clock(99);
    driver.enter(7);
    assert_eq!(driver.timer(100), Ok(false));
    driver.parked(7);
    assert!(driver.ready().unwrap().is_empty());
    assert!(driver.wait().unwrap());
    assert_eq!(driver.virtual_time(), Some(100));
    assert_eq!(driver.ready().unwrap(), vec![7]);
    driver.enter(8);
    assert_eq!(driver.timer(100), Ok(true), "equality is already due");
    driver.finish(7);

    let clock = Arc::new(Clock::default());
    let io = service(clock.clone());
    let mut s = Scheduler::new(Schedule::default(), std::ptr::null_mut());
    let a = io
        .arm(
            ScopeKey {
                slot: 0,
                generation: 2,
            },
            11,
            100,
        )
        .unwrap()
        .unwrap();
    let exited = io
        .arm(
            ScopeKey {
                slot: 1,
                generation: 3,
            },
            11,
            100,
        )
        .unwrap()
        .unwrap();
    let stale = io
        .arm(
            ScopeKey {
                slot: 2,
                generation: 4,
            },
            11,
            100,
        )
        .unwrap()
        .unwrap();
    let b = io
        .arm(
            ScopeKey {
                slot: 3,
                generation: 5,
            },
            11,
            100,
        )
        .unwrap()
        .unwrap();
    s.scopes.extend([
        Some(scope(2, None)),
        Some(scope(3, Some(exited))),
        Some(scope(6, None)),
        Some(scope(5, Some(b))),
    ]);
    io.disarm(exited);
    clock.due.store(true, Ordering::Release);
    io.timers
        .poll(
            &*clock,
            &crate::io::Ready::Tokens(vec![a, exited, stale, b]),
        )
        .unwrap();
    // Equal deadlines finish in the original token/arming order; exited
    // registrations are ignored, and the reused scope cannot receive expiry.
    assert_eq!(*clock.finishes.lock().unwrap(), vec![exited, a, stale, b]);
    assert!(io.timers.expire(&mut s));
    assert!(
        !s.scope(0).deadline.as_ref().unwrap().expired,
        "ready before Done is latched"
    );
    assert!(
        !s.scope(2).deadline.as_ref().unwrap().expired,
        "stale generation ignored"
    );
    assert!(s.scope(3).deadline.as_ref().unwrap().expired);
    s.scope(0).deadline.as_mut().unwrap().token = Some(a);
    io.timers
        .poll(&*clock, &crate::io::Ready::Tokens(Vec::new()))
        .unwrap();
    assert!(io.timers.expire(&mut s));
    assert!(s.scope(0).deadline.as_ref().unwrap().expired);
    assert!(!io.timers.expire(&mut s));
    assert!(!io.pending());
    assert_eq!(io.timers.0.lock().unwrap().deadlines.stored(), (0, 0));
    io.disarm(a);
    io.disarm(b);
    assert_eq!(
        clock.finishes.lock().unwrap().len(),
        4,
        "finish exactly once"
    );
}
