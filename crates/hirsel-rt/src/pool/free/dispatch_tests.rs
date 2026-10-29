use super::*;

pub(super) struct Topology(pub(super) Execs);
// SAFETY: these tests share only ready queues, topology membership and atomics.
// The fixture has no physical stacks/workers; home_idle remains None throughout.
unsafe impl Sync for Topology {}
impl std::ops::Deref for Topology {
    type Target = Execs;
    fn deref(&self) -> &Execs {
        &self.0
    }
}
pub(super) fn topology(width: u16) -> Topology {
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
        queued_stop_witness: None,
        searcher: AtomicUsize::new(0),
        after_search: Mutex::new(None),
        before_save: Default::default(),
        before_idle: Default::default(),
        after_compact: Default::default(),
    };
    let cell = std::sync::OnceLock::new();
    assert!(cell.set(dispatch).is_ok());
    Topology(Execs {
        run: RunPtr(std::ptr::null_mut()),
        demand: false,
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
        scoped: Mutex::new(None),
        scoped_cleanup: Default::default(),
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
        task: TaskKey::new(task, 1),
        sp: std::ptr::null_mut(),
    });
    Saved {
        queued,
        frame: Arc::new(Frame {
            notification,
            placement: Placement::new(owner_word(Owner::Any)),
            saved: AtomicBool::new(true),
            stop_before_start: false,
        }),
        lease: Lease::new(&execs.dispatch.get().unwrap().count),
        chained: false,
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
    assert_eq!(
        next(&execs, &exec)
            .unwrap()
            .queued
            .start()
            .value()
            .task
            .slot,
        7
    );
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
        let (begin, ready) = std::sync::mpsc::channel();
        let mut begin = Some(begin);
        let publisher = scope.spawn({
            let execs = &execs;
            move || {
                ready.recv().unwrap();
                publish(execs, segment(execs, 8), false, false);
            }
        });
        // Publish after the last predicate check, potentially before parking.
        while exec.ready_epoch.load(Ordering::Acquire) == epoch {
            drop(inbox);
            if let Some(begin) = begin.take() {
                begin.send(()).unwrap();
            }
            assert!(
                exec.bell.wait_timeout(Duration::from_secs(5)),
                "publication lost at the park boundary"
            );
            inbox = exec.inbox.lock().unwrap();
        }
        drop(inbox);
        publisher.join().unwrap();
    });
    assert_eq!(
        next(&execs, &exec)
            .unwrap()
            .queued
            .start()
            .value()
            .task
            .slot,
        8
    );
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
                        .push(saved.queued.start().value().task.slot);
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
    assert_eq!(
        next(&execs, &exec)
            .unwrap()
            .queued
            .start()
            .value()
            .task
            .slot,
        11
    );
    exec.busy.store(false, Ordering::SeqCst);
    publish(&execs, segment(&execs, 12), false, true);
    assert_eq!(
        epochs(&execs),
        1,
        "home cannot run a wider compute continuation"
    );
    assert_eq!(
        next(&execs, &exec)
            .unwrap()
            .queued
            .start()
            .value()
            .task
            .slot,
        12
    );
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
            reader
                .join()
                .unwrap()
                .unwrap()
                .queued
                .start()
                .value()
                .task
                .slot,
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
    assert_eq!(
        next(&execs, &peer)
            .unwrap()
            .queued
            .start()
            .value()
            .task
            .slot,
        13
    );
}

fn exec(execs: &Execs, index: usize) -> Arc<Exec> {
    execs.compute.lock().unwrap().active[index].clone()
}

/// The calling test thread acts as `exec`'s own OS thread until it drops.
struct OwnThread(*const Exec);
impl OwnThread {
    fn new(exec: &Arc<Exec>) -> Self {
        Self(EXEC.with(|e| e.replace(Arc::as_ptr(exec))))
    }
}
impl Drop for OwnThread {
    fn drop(&mut self) {
        EXEC.with(|e| e.set(self.0));
    }
}

/// A claimed dispatch: starting its token is the only access to the image.
struct Started {
    task: u32,
    chained: bool,
    running: Running<Image>,
    frame: Arc<Frame>,
    lease: Lease,
}

fn start(saved: Saved) -> Started {
    let Saved {
        queued,
        frame,
        lease,
        chained,
    } = saved;
    let running = queued.start();
    Started {
        task: running.value().task.slot,
        chained,
        running,
        frame,
        lease,
    }
}

impl Started {
    /// The idle continuation saves, then publishes: a yield to the FIFO, a
    /// notified continuation to its worker's LIFO slot.
    fn republish(self, execs: &Execs, yielded: bool) {
        let queued = self.running.save(true).expect("a yield is always ready");
        publish(
            execs,
            Saved {
                queued,
                frame: self.frame,
                lease: self.lease,
                chained: false,
            },
            yielded,
            true,
        );
    }
}

#[test]
fn runtime50_global_injection_has_bounded_service() {
    use hirsel_free::work::GLOBAL_INTERVAL;
    let bound = GLOBAL_INTERVAL as usize;
    // Every phase of the worker's tick counter, including a remote completion
    // arriving just after a forced check and at the first opportunity.
    for arrival in 0..2 * bound {
        let execs = topology(1);
        let worker = exec(&execs, 0);
        let _thread = OwnThread::new(&worker);
        // Four local tasks that yield forever: the local FIFO never empties.
        for task in 0..4 {
            start(segment(&execs, task)).republish(&execs, true);
        }
        let mut dispatches = 0;
        let served = loop {
            if dispatches == arrival {
                // A remote completion from a host or timer thread: no LIFO slot.
                publish(&execs, segment(&execs, 100), false, false);
            }
            let started = start(next(&execs, &worker).expect("local work is never empty"));
            dispatches += 1;
            assert!(!started.chained, "neither the FIFO nor injection chains");
            if started.task == 100 {
                break dispatches;
            }
            assert!(started.task < 4);
            started.republish(&execs, true);
            assert!(
                dispatches < arrival + bound,
                "injected work starved behind a busy local queue"
            );
        };
        let delay = served - arrival;
        assert!(
            (1..=bound).contains(&delay),
            "arrival {arrival}: injected work waited {delay} dispatches"
        );
        // Local work was served meanwhile; the forced check is not a drain.
        assert_eq!(execs.dispatch.get().unwrap().ready.injected(), 0);
    }
    // A LIFO handoff continues its worker's chain budget; three polls bound
    // the slot, and the FIFO then starts a fresh chain. The first save
    // precedes this worker's first search, so it is injected, not chained.
    let execs = topology(1);
    let worker = exec(&execs, 0);
    let _thread = OwnThread::new(&worker);
    start(segment(&execs, 7)).republish(&execs, false);
    let chained: Vec<bool> = (0..8)
        .map(|_| {
            let started = start(next(&execs, &worker).unwrap());
            assert_eq!(started.task, 7);
            let chained = started.chained;
            started.republish(&execs, false);
            chained
        })
        .collect();
    assert_eq!(
        chained,
        [false, true, true, true, false, true, true, true],
        "LIFO polls are bounded by {}",
        hirsel_free::schedule::LIFO_POLLS
    );
}

#[test]
fn runtime50_forced_service_never_hides_a_saved_continuation() {
    // The forced check runs injected work that need not return to this
    // search. At every LIFO phase, the worker's own saved continuation must
    // already be visible to another worker.
    for shift in 0..=usize::from(hirsel_free::schedule::LIFO_POLLS) {
        let execs = topology(2);
        let (worker, thief) = (exec(&execs, 0), exec(&execs, 1));
        let _thread = OwnThread::new(&worker);
        assert!(next(&execs, &worker).is_none());
        start(segment(&execs, 7)).republish(&execs, false);
        publish(&execs, segment(&execs, 100), false, false);
        let mut dispatches = 0;
        loop {
            let started = start(next(&execs, &worker).unwrap());
            if started.task == 100 {
                break;
            }
            assert_eq!(started.task, 7);
            started.republish(&execs, dispatches < shift);
            dispatches += 1;
        }
        let stolen = start(next(&execs, &thief).expect("a hidden continuation"));
        assert_eq!(stolen.task, 7, "shift {shift}");
    }
}

#[test]
fn runtime50_width_aware_refill_takes_a_fair_share() {
    let execs = topology(4);
    let ready = &execs.dispatch.get().unwrap().ready;
    assert_eq!(ready.width(), 4);
    for task in 0..40 {
        publish(&execs, segment(&execs, task), false, false);
    }
    // 40 / 4 + 1: one runs directly and ten move to the first worker's FIFO,
    // leaving the rest for other workers rather than half the capacity.
    let first = start(next(&execs, &exec(&execs, 0)).unwrap());
    assert_eq!(first.task, 0);
    assert_eq!(ready.injected(), 29);
    let second = start(next(&execs, &exec(&execs, 1)).unwrap());
    assert_eq!(second.task, 11, "the second worker refills its own share");
    assert_eq!(ready.injected(), 29 - (29 / 4 + 1));
    assert_eq!(start(next(&execs, &exec(&execs, 0)).unwrap()).task, 1);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Wake,
    DuplicateWake,
    Save,
    Flush,
    Retire,
}

fn permutations(events: &mut Vec<Event>, at: usize, out: &mut Vec<Vec<Event>>) {
    if at == events.len() {
        out.push(events.clone());
        return;
    }
    for i in at..events.len() {
        events.swap(at, i);
        permutations(events, at + 1, out);
        events.swap(at, i);
    }
}

#[test]
fn runtime50_notify_save_shrink_batch() {
    let mut orders = Vec::new();
    permutations(
        &mut vec![
            Event::Wake,
            Event::DuplicateWake,
            Event::Save,
            Event::Flush,
            Event::Retire,
        ],
        0,
        &mut orders,
    );
    assert_eq!(orders.len(), 120);
    for order in orders {
        let execs = topology(2);
        let dispatch = execs.dispatch.get().unwrap();
        let (home, thief) = (exec(&execs, 0), exec(&execs, 1));
        let _thread = OwnThread::new(&home);
        // Bind a local to the running worker, then park the thief after its
        // empty search: any shared publication must reach a sleeper.
        assert!(next(&execs, &home).is_none());
        assert!(next(&execs, &thief).is_none());
        let epochs_before = epochs(&execs);
        // Task 1 runs on `home`; task 2 is already admitted into the batch.
        let mut running = Some(start(segment(&execs, 1)));
        let frame = running.as_ref().unwrap().frame.clone();
        let mut batch = ReadyBatch {
            run: std::ptr::null_mut(),
            execs: &*execs as *const Execs,
            entries: [const { std::mem::MaybeUninit::uninit() }; BATCH],
            len: 0,
        };
        batch.push(segment(&execs, 2));
        let mut retired = false;
        let mut publications = 0;
        for event in &order {
            match event {
                Event::Wake | Event::DuplicateWake => {
                    // Notification never reads the running image.
                    if let Some(queued) = frame.notification.notify() {
                        publications += 1;
                        batch.push(Saved {
                            queued,
                            frame: frame.clone(),
                            lease: Lease::new(&dispatch.count),
                            chained: false,
                        });
                    }
                }
                Event::Save => {
                    let Started {
                        running: image,
                        lease,
                        ..
                    } = running.take().unwrap();
                    // Wake-before-save latched on RUNNING: only the save
                    // acknowledgement may publish the started image.
                    if let Some(queued) = image.save(false) {
                        publications += 1;
                        publish(
                            &execs,
                            Saved {
                                queued,
                                frame: frame.clone(),
                                lease,
                                chained: false,
                            },
                            false,
                            true,
                        );
                    }
                }
                Event::Flush => batch.flush(),
                Event::Retire => {
                    retire(&execs, &home);
                    retired = true;
                }
            }
        }
        drop(batch);
        assert_eq!(
            publications, 1,
            "{order:?}: two wakes and a save made one publication"
        );
        assert!(
            frame.notification.notify().is_none(),
            "{order:?}: a queued wake coalesces"
        );
        if dispatch.ready.injected() != 0 {
            assert!(
                epochs(&execs) > epochs_before,
                "{order:?}: shared publication woke no sleeping thief"
            );
        }
        let mut claims = Vec::new();
        loop {
            let claimed =
                next(&execs, &thief).or_else(|| if retired { None } else { next(&execs, &home) });
            let Some(saved) = claimed else { break };
            // Exactly one resumable owner: starting the token is the only
            // access to the saved image, and each succeeds once.
            claims.push(start(saved).task);
        }
        claims.sort_unstable();
        assert_eq!(claims, [1, 2], "{order:?}: each continuation has one owner");
        assert_eq!(dispatch.queued.load(Ordering::SeqCst), 0, "{order:?}");
        drop(frame);
        assert_eq!(dispatch.count.live.load(Ordering::Acquire), 0, "{order:?}");
    }
}

#[test]
fn scoped_workers_take_only_addressed_frames_and_never_ordinary_work() {
    let execs = topology(1);
    let scoped = Exec::new(Kind::Scoped(0), std::ptr::null_mut(), false);
    publish(&execs, segment(&execs, 19), false, false);
    assert!(next(&execs, &scoped).is_none());
    let owner = Owner::Compute(Arc::as_ptr(&scoped).expose_provenance());
    let addressed = segment(&execs, 23);
    addressed.frame.placement.set_owner(owner_word(owner));
    publish(&execs, addressed, false, false);
    let Post::Saved(saved) = scoped.take().0 else {
        panic!("scoped addressed frame")
    };
    assert_eq!(saved.queued.start().value().task.slot, 23);
    let ordinary = execs.compute.lock().unwrap().active[0].clone();
    assert_eq!(
        next(&execs, &ordinary)
            .unwrap()
            .queued
            .start()
            .value()
            .task
            .slot,
        19
    );
}

#[test]
fn a_late_token_wake_routes_a_saved_compact_frame_off_its_last_worker() {
    for owner in [Some(owner_word(Owner::Any)), None] {
        let mut execs = topology(2);
        execs.0.migrate = true;
        let first = exec(&execs, 0);
        let peer = exec(&execs, 1);
        let saved = segment(&execs, 19);
        let frame = saved.frame.clone();
        assert!(
            frame
                .placement
                .leave(saved.queued.start(), Arc::as_ptr(&first).addr(), || {})
                .is_none()
        );
        let token = Token { frame, owner };
        notify_frame(&execs, &token);
        assert!(
            next(&execs, &first).is_none(),
            "late wake returned to the old worker"
        );
        assert!(
            first
                .inbox
                .lock()
                .unwrap()
                .posts
                .iter()
                .all(|(post, _)| !matches!(post, Post::Saved(_))),
            "late wake was addressed to the old worker"
        );
        let mut inbox = peer.inbox.lock().unwrap();
        let at = inbox
            .posts
            .iter()
            .position(|(post, _)| matches!(post, Post::Saved(_)))
            .expect("late wake did not reach its migration peer");
        let (Post::Saved(queued), _) = inbox.posts.remove(at).unwrap() else {
            unreachable!()
        };
        assert_eq!(queued.queued.start().value().task.slot, 19);
        assert!(
            inbox
                .posts
                .iter()
                .all(|(post, _)| !matches!(post, Post::Saved(_))),
            "late wake duplicated the activation"
        );
    }
}

fn scoped_publication_at_park(after_lock: bool) {
    let execs = topology(1);
    let scoped = Exec::new(Kind::Scoped(0), std::ptr::null_mut(), false);
    let owner = Owner::Compute(Arc::as_ptr(&scoped).expose_provenance());
    let addressed = segment(&execs, 23);
    addressed.frame.placement.set_owner(owner_word(owner));
    // A scoped executor must leave ordinary ready work for its ordinary owner.
    publish(&execs, segment(&execs, 19), false, false);
    assert!(next(&execs, &scoped).is_none());
    if after_lock {
        let mut inbox = scoped.inbox.lock().unwrap();
        assert!(inbox.posts.is_empty());
        std::thread::scope(|scope| {
            let (started, ready) = std::sync::mpsc::channel();
            let execs = &execs;
            let publisher = scope.spawn(move || {
                started.send(()).unwrap();
                publish(execs, addressed, false, false);
            });
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            // Release the predicate lock before parking. A post in that
            // interval must retain a notification.
            while inbox.posts.is_empty() {
                drop(inbox);
                assert!(
                    scoped.bell.wait_timeout(Duration::from_secs(5)),
                    "addressed CPU post lost at park"
                );
                inbox = scoped.inbox.lock().unwrap();
            }
            drop(inbox);
            publisher.join().unwrap();
        });
    } else {
        // A post and its notification survive until the receiver waits.
        publish(&execs, addressed, false, false);
    }
    let Post::Saved(saved) = scoped.take().0 else {
        panic!("addressed CPU continuation")
    };
    assert_eq!(start(saved).task, 23);
    assert!(scoped.inbox.lock().unwrap().posts.is_empty());
    assert!(next(&execs, &scoped).is_none());
    let ordinary = execs.compute.lock().unwrap().active[0].clone();
    assert_eq!(start(next(&execs, &ordinary).unwrap()).task, 19);
    assert_eq!(
        execs.dispatch.get().unwrap().queued.load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        execs
            .dispatch
            .get()
            .unwrap()
            .count
            .live
            .load(Ordering::Acquire),
        0
    );
}

#[test]
fn scoped_addressed_post_before_park_preserves_the_only_claim() {
    scoped_publication_at_park(false);
}

#[test]
fn scoped_addressed_post_during_park_cannot_lose_its_wakeup() {
    scoped_publication_at_park(true);
}
