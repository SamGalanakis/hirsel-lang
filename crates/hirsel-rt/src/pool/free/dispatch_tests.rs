use super::*;

struct Topology(Execs);
// SAFETY: these tests share only ready queues, topology membership and atomics.
// The fixture has no physical stacks/workers; home_idle remains None throughout.
unsafe impl Sync for Topology {}
impl std::ops::Deref for Topology {
    type Target = Execs;
    fn deref(&self) -> &Execs {
        &self.0
    }
}
fn topology(width: u16) -> Topology {
    let home = Exec::new(Kind::Home, std::ptr::null_mut(), false);
    let (ready, locals) = hirsel_free::work::workers(u32::from(width), None);
    let dispatch = Dispatch {
        ready,
        locals: Mutex::new(locals),
        count: Arc::new(Count {
            live: AtomicU32::new(0),
            home: Arc::downgrade(&home),
        }),
        queued: AtomicUsize::new(0),
        #[cfg(feature = "free-qualification")]
        queued_stop_witness: None,
        searcher: AtomicUsize::new(0),
        after_search: Mutex::new(None),
    };
    let cell = std::sync::OnceLock::new();
    assert!(cell.set(dispatch).is_ok());
    Topology(Execs {
        home,
        home_idle: Mutex::new(None),
        compute: Mutex::new(ComputeWorkers {
            active: (0..width)
                .map(|i| Exec::new(Kind::Compute(i), std::ptr::null_mut(), false))
                .collect(),
            retiring: Vec::new(),
            threads: Vec::new(),
        }),
        pool: Mutex::new(Vec::new()),
        permits: Arc::new(Permits::new(u32::from(width))),
        holders: Arc::new(Holders {
            count: AtomicU32::new(1),
            detached: AtomicBool::new(false),
            reclaimed: AtomicBool::new(false),
            workers: Mutex::new(Vec::new()),
        }),
        cursor: AtomicUsize::new(0),
        migrate: false,
        home_at_one: false,
        calls: Default::default(),
        next_call: AtomicU64::new(0),
        activity: Mutex::new(()),
        dispatch: cell,
    })
}

fn segment(execs: &Execs, task: u32) -> Saved {
    // These owned queue images are never resumed or dereferenced.
    let (notification, queued) = frame::prepared(Image {
        ctx: std::ptr::null_mut(),
        task,
        sp: std::ptr::null_mut(),
    });
    Saved {
        queued,
        frame: Arc::new(Frame {
            notification,
            owner: AtomicUsize::new(owner_word(Owner::Any)),
            saved: AtomicBool::new(true),
            #[cfg(feature = "free-qualification")]
            stop_before_start: false,
        }),
        lease: Lease::new(&execs.dispatch.get().unwrap().count),
    }
}

fn epochs(execs: &Execs) -> u64 {
    execs
        .compute
        .lock()
        .unwrap()
        .active
        .iter()
        .map(|e| e.ready_epoch.load(Ordering::Acquire))
        .sum()
}

#[test]
fn publication_before_the_park_lock_is_retained_by_the_epoch() {
    let execs = topology(1);
    let exec = execs.compute.lock().unwrap().active[0].clone();
    let epoch = exec.ready_epoch.load(Ordering::Acquire);
    assert!(next(&execs, &exec).is_none());
    publish(&execs, segment(&execs, 7), false, false);
    // This is serve's final predicate check after its earlier empty search.
    let inbox = exec.inbox.lock().unwrap();
    assert_ne!(exec.ready_epoch.load(Ordering::Acquire), epoch);
    drop(inbox);
    assert_eq!(next(&execs, &exec).unwrap().queued.start().value().task, 7);
    assert_eq!(
        execs.dispatch.get().unwrap().queued.load(Ordering::SeqCst),
        0
    );
}

#[test]
fn publication_while_subscribing_to_the_bell_wakes_the_parked_worker() {
    let execs = topology(1);
    let exec = execs.compute.lock().unwrap().active[0].clone();
    let epoch = exec.ready_epoch.load(Ordering::Acquire);
    assert!(next(&execs, &exec).is_none());
    let mut inbox = exec.inbox.lock().unwrap();
    std::thread::scope(|scope| {
        let publisher = scope.spawn(|| publish(&execs, segment(&execs, 8), false, false));
        // The publisher needs this predicate lock before changing the epoch.
        while exec.ready_epoch.load(Ordering::Acquire) == epoch {
            let (guard, timeout) = exec
                .bell
                .wait_timeout(inbox, Duration::from_secs(5))
                .unwrap();
            inbox = guard;
            assert!(
                !timeout.timed_out(),
                "publication lost at the park boundary"
            );
        }
        drop(inbox);
        publisher.join().unwrap();
    });
    assert_eq!(next(&execs, &exec).unwrap().queued.start().value().task, 8);
}

#[test]
fn addressed_work_hands_an_outstanding_search_to_another_worker() {
    let execs = topology(4);
    for task in 0..4 {
        publish(&execs, segment(&execs, task), false, false);
    }
    assert_eq!(
        epochs(&execs),
        1,
        "publications share one outstanding search"
    );
    let dispatch = execs.dispatch.get().unwrap();
    let searcher = dispatch.searcher.load(Ordering::SeqCst);
    let exec = execs
        .compute
        .lock()
        .unwrap()
        .active
        .iter()
        .find(|e| Arc::as_ptr(e) as usize == searcher)
        .unwrap()
        .clone();
    // As serve does before a pinned Saved post which can run pure code.
    exec.post(Post::Wake, None);
    exec.busy.store(true, Ordering::SeqCst);
    cancel_search(&execs, &exec);
    assert_ne!(dispatch.searcher.load(Ordering::SeqCst), 0);
    assert_ne!(dispatch.searcher.load(Ordering::SeqCst), searcher);
    assert_eq!(epochs(&execs), 2);
}

#[test]
fn concurrent_claims_consume_each_ready_reservation_once() {
    let execs = topology(4);
    for task in 0..256 {
        publish(&execs, segment(&execs, task), false, false);
    }
    let workers = execs.compute.lock().unwrap().active.clone();
    let tasks = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for exec in &workers {
            let execs = &execs;
            let tasks = &tasks;
            scope.spawn(move || {
                while let Some(saved) = next(execs, exec) {
                    tasks
                        .lock()
                        .unwrap()
                        .push(saved.queued.start().value().task);
                    exec.busy.store(false, Ordering::SeqCst);
                }
                cancel_search(execs, exec);
            });
        }
    });
    let mut tasks = tasks.into_inner().unwrap();
    tasks.sort_unstable();
    assert_eq!(tasks, (0..256).collect::<Vec<_>>());
    assert_eq!(
        execs.dispatch.get().unwrap().queued.load(Ordering::SeqCst),
        0
    );
}

#[test]
fn a_local_idle_continuation_needs_no_sleeper_but_home_publication_does() {
    let execs = topology(2);
    let exec = execs.compute.lock().unwrap().active[0].clone();
    assert!(next(&execs, &exec).is_none());
    EXEC.with(|e| {
        assert!(e.replace(Arc::as_ptr(&exec)).is_null());
        publish(&execs, segment(&execs, 11), false, true);
        e.set(std::ptr::null());
    });
    assert_eq!(epochs(&execs), 0);
    assert_eq!(next(&execs, &exec).unwrap().queued.start().value().task, 11);
    exec.busy.store(false, Ordering::SeqCst);
    publish(&execs, segment(&execs, 12), false, true);
    assert_eq!(
        epochs(&execs),
        1,
        "home cannot run a wider compute continuation"
    );
    assert_eq!(next(&execs, &exec).unwrap().queued.start().value().task, 12);
}

#[test]
fn the_last_searcher_rechecks_a_publication_which_suppressed_its_wake() {
    let execs = topology(1);
    let exec = execs.compute.lock().unwrap().active[0].clone();
    let dispatch = execs.dispatch.get().unwrap();
    dispatch
        .searcher
        .store(Arc::as_ptr(&exec) as usize, Ordering::SeqCst);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let first = barrier.clone();
    *dispatch.after_search.lock().unwrap() = Some(Box::new(move || {
        first.wait();
        first.wait();
    }));
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| next(&execs, &exec));
        barrier.wait();
        publish(&execs, segment(&execs, 99), false, false);
        assert_eq!(
            epochs(&execs),
            0,
            "the existing searcher suppressed notification"
        );
        barrier.wait();
        assert_eq!(
            reader.join().unwrap().unwrap().queued.start().value().task,
            99
        );
    });
    assert_eq!(dispatch.queued.load(Ordering::SeqCst), 0);
    assert_eq!(dispatch.searcher.load(Ordering::SeqCst), 0);
}

#[test]
fn forced_migration_publishes_to_other_workers_even_for_one_continuation() {
    let mut execs = topology(2);
    execs.0.migrate = true;
    let exec = execs.compute.lock().unwrap().active[0].clone();
    assert!(next(&execs, &exec).is_none());
    EXEC.with(|e| {
        assert!(e.replace(Arc::as_ptr(&exec)).is_null());
        publish(&execs, segment(&execs, 13), true, true);
        e.set(std::ptr::null());
    });
    assert_eq!(
        epochs(&execs),
        1,
        "the forced-migration peer must be notified"
    );
    let peer = execs.compute.lock().unwrap().active[1].clone();
    assert_eq!(next(&execs, &peer).unwrap().queued.start().value().task, 13);
}
