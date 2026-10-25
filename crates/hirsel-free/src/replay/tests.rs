//! Wake delivery vectors. The registration vectors port Tokio `tests/sync_notify.rs:235-273`
//! (ce685ee6916549ed7e52dbfd5430858bbdc734d3): a notification before the wait registers
//! completes it at registration, and one after it registers wakes it once.
use super::*;
use crate::recorder::Position;
use std::sync::atomic::AtomicUsize;

struct Count(AtomicUsize);

impl std::task::Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn counter() -> (Arc<Count>, Waker) {
    let count = Arc::new(Count(AtomicUsize::new(0)));
    (count.clone(), Waker::from(count))
}

fn woken(count: &Count) -> usize {
    count.0.load(Ordering::SeqCst)
}

/// A waiter's identity as the engine forms it: row generation above the row index.
fn waiter(row: u32, generation: u32) -> u64 {
    u64::from(generation) << 32 | u64::from(row)
}

fn wake(wakes: Vec<Waker>) -> usize {
    let n = wakes.len();
    wakes.into_iter().for_each(Waker::wake);
    n
}

fn token(cell: &Arc<Version>, at: u64) -> DependencyToken {
    DependencyToken {
        cell: cell.clone(),
        at,
    }
}

fn shared(keys: &[&[bool]]) -> Shared {
    let epochs: Vec<KeyEpochs> = keys
        .iter()
        .map(|members| {
            let mut epochs = KeyEpochs::default();
            let mut epoch = 0;
            let mut last = None;
            for &shared in *members {
                if !(shared && last == Some(true)) {
                    epoch += 1;
                }
                epochs.member(epoch, shared).unwrap();
                last = Some(shared);
            }
            epochs
        })
        .collect();
    Shared {
        observe: true,
        end: None,
        cut: BTreeMap::new(),
        keys: (0..keys.len()).map(|key| format!("key/{key}")).collect(),
        streams: HashMap::new(),
        objects: RwLock::new(HashMap::new()),
        epochs: Mutex::new(Keys {
            waiters: epochs.iter().map(|_| Vec::new()).collect(),
            epochs,
        }),
        visits: AtomicU64::new(0),
    }
}

#[test]
fn a_version_published_before_registration_completes_the_wait_at_registration() {
    let cell = Arc::new(Version::new(1));
    let pending = token(&cell, 2);
    let mut wakes = Vec::new();
    let visits = AtomicU64::new(0);
    assert!(cell.advance(1, &visits, &mut wakes));
    assert!(wakes.is_empty());
    let (count, waker) = counter();
    assert!(pending.subscribe(Subscriber {
        waiter: waiter(1, 0),
        waker,
    }));
    assert_eq!(pending.waiting(), 0, "a ready wait stays unregistered");
    assert_eq!(woken(&count), 0);
}

#[test]
fn a_version_published_after_registration_wakes_the_wait_once() {
    let cell = Arc::new(Version::new(1));
    let pending = token(&cell, 2);
    let (count, waker) = counter();
    assert!(!pending.subscribe(Subscriber {
        waiter: waiter(1, 0),
        waker,
    }));
    assert_eq!(pending.waiting(), 1);
    let mut wakes = Vec::new();
    let visits = AtomicU64::new(0);
    assert!(cell.advance(1, &visits, &mut wakes));
    assert_eq!(
        pending.waiting(),
        0,
        "the publication took its registration"
    );
    assert_eq!(wake(wakes), 1);
    assert_eq!(woken(&count), 1);
    assert!(pending.subscribe(Subscriber {
        waiter: waiter(1, 0),
        waker: counter().1,
    }));
}

#[test]
fn registering_one_wait_twice_keeps_one_entry_and_wakes_once() {
    let cell = Arc::new(Version::new(1));
    let pending = token(&cell, 2);
    let (first, waker1) = counter();
    let (second, waker2) = counter();
    assert!(!pending.subscribe(Subscriber {
        waiter: waiter(3, 7),
        waker: waker1,
    }));
    // A spurious wake re-runs the wait loop, which registers again.
    assert!(!pending.subscribe(Subscriber {
        waiter: waiter(3, 7),
        waker: waker2,
    }));
    assert_eq!(pending.waiting(), 1);
    let mut wakes = Vec::new();
    assert!(cell.advance(1, &AtomicU64::new(0), &mut wakes));
    assert_eq!(wake(wakes), 1);
    assert_eq!((woken(&first), woken(&second)), (0, 1));
}

#[test]
fn a_reused_row_registers_as_a_distinct_waiter() {
    let cell = Arc::new(Version::new(1));
    let (old, stale) = counter();
    let (new, fresh) = counter();
    assert!(!token(&cell, 3).subscribe(Subscriber {
        waiter: waiter(4, 1),
        waker: stale,
    }));
    // The row's next task waits for an earlier version under its next generation; it neither
    // replaces nor inherits the retired waiter's registration.
    assert!(!token(&cell, 2).subscribe(Subscriber {
        waiter: waiter(4, 2),
        waker: fresh,
    }));
    assert_eq!(token(&cell, 2).waiting(), 2);
    let mut wakes = Vec::new();
    assert!(cell.advance(1, &AtomicU64::new(0), &mut wakes));
    assert_eq!(wake(wakes), 1);
    assert_eq!((woken(&old), woken(&new)), (0, 1));
}

#[test]
fn a_gapped_wait_wakes_only_when_its_last_predecessor_publishes() {
    let cell = Arc::new(Version::new(1));
    let pending = token(&cell, 4);
    let (count, waker) = counter();
    assert!(!pending.subscribe(Subscriber {
        waiter: waiter(2, 0),
        waker,
    }));
    let visits = AtomicU64::new(0);
    for from in 1..3 {
        let mut wakes = Vec::new();
        assert!(cell.advance(from, &visits, &mut wakes));
        assert_eq!(
            wake(wakes),
            0,
            "version {} is not the one it needs",
            from + 1
        );
    }
    let mut wakes = Vec::new();
    assert!(cell.advance(3, &visits, &mut wakes));
    assert_eq!(wake(wakes), 1);
    assert_eq!(woken(&count), 1);
    assert!(
        !cell.advance(3, &visits, &mut Vec::new()),
        "a stale advance fails"
    );
}

#[test]
fn an_exclusive_key_wait_wakes_once_every_shared_predecessor_ended() {
    // Epoch 1 holds two shared transitions; epoch 2 is exclusive.
    let shared = shared(&[&[true, true, false]]);
    let first = Position { key: 0, epoch: 1 };
    let exclusive = [Position { key: 0, epoch: 2 }];
    assert!(shared.subscribe_positions(
        &[first],
        Subscriber {
            waiter: waiter(1, 0),
            waker: counter().1,
        }
    ));
    let (count, waker) = counter();
    assert!(!shared.subscribe_positions(
        &exclusive,
        Subscriber {
            waiter: waiter(3, 0),
            waker,
        }
    ));
    assert_eq!(wake(shared.end_positions(&[first])), 0);
    assert!(!shared.positions_ready(&exclusive));
    assert_eq!(wake(shared.end_positions(&[first])), 1);
    assert_eq!(woken(&count), 1);
    assert!(shared.positions_ready(&exclusive));
}

#[test]
fn a_wait_on_several_keys_registers_on_the_first_unready_one() {
    let shared = shared(&[&[false, false], &[false, false]]);
    let wanted = [Position { key: 0, epoch: 2 }, Position { key: 1, epoch: 2 }];
    let (count, waker) = counter();
    assert!(!shared.subscribe_positions(
        &wanted,
        Subscriber {
            waiter: waiter(5, 0),
            waker,
        }
    ));
    assert_eq!(
        wake(shared.end_positions(&[Position { key: 1, epoch: 1 }])),
        0,
        "key 1 holds no registration yet"
    );
    assert_eq!(shared.visits(), 0);
    assert_eq!(
        wake(shared.end_positions(&[Position { key: 0, epoch: 1 }])),
        1
    );
    assert_eq!(woken(&count), 1);
    // The woken wait rechecks and finds both keys ready.
    assert!(shared.subscribe_positions(
        &wanted,
        Subscriber {
            waiter: waiter(5, 0),
            waker: counter().1,
        }
    ));
}

#[test]
fn publications_visit_only_their_own_waiter_group() {
    for groups in [1usize, 2, 4] {
        const WAITERS: usize = 3;
        // Each group waits on its own object and its own key.
        let cells: Vec<_> = (0..groups).map(|_| Arc::new(Version::new(1))).collect();
        let members: Vec<&[bool]> = (0..groups).map(|_| &[false, false][..]).collect();
        let shared = shared(&members);
        let mut counts = Vec::new();
        for (group, cell) in cells.iter().enumerate() {
            for w in 0..WAITERS {
                let (count, waker) = counter();
                let row = (group * WAITERS + w) as u32;
                assert!(!token(cell, 2).subscribe(Subscriber {
                    waiter: waiter(row, 0),
                    waker,
                }));
                let (key_count, key_waker) = counter();
                assert!(!shared.subscribe_positions(
                    &[Position {
                        key: group as u32,
                        epoch: 2
                    }],
                    Subscriber {
                        waiter: waiter(row, 0),
                        waker: key_waker,
                    }
                ));
                counts.push((group, count, key_count));
            }
        }
        for (group, cell) in cells.iter().enumerate() {
            let before = shared.visits();
            let mut wakes = Vec::new();
            assert!(cell.advance(1, &shared.visits, &mut wakes));
            wakes.extend(shared.end_positions(&[Position {
                key: group as u32,
                epoch: 1,
            }]));
            assert_eq!(wake(wakes), 2 * WAITERS);
            assert_eq!(
                shared.visits() - before,
                2 * WAITERS as u64,
                "{groups} groups: group {group}'s publications visit only its registrations"
            );
            for (owner, count, key_count) in &counts {
                let expected = usize::from(*owner <= group);
                assert_eq!((woken(count), woken(key_count)), (expected, expected));
            }
        }
    }
}

#[test]
fn finish_drops_the_registrations_of_waits_that_never_became_ready() {
    let shared = Arc::new(shared(&[&[false, false]]));
    shared.gate("package:1");
    let cell = shared.object(&ObjectId::Gate("package:1".into())).unwrap();
    let (count, waker) = counter();
    assert!(!token(&cell, 3).subscribe(Subscriber {
        waiter: waiter(1, 0),
        waker,
    }));
    assert!(!shared.subscribe_positions(
        &[Position { key: 0, epoch: 2 }],
        Subscriber {
            waiter: waiter(1, 0),
            waker: counter().1,
        }
    ));
    shared.finish(std::iter::empty()).unwrap();
    assert_eq!(token(&cell, 3).waiting(), 0);
    assert!(shared.epochs.lock().unwrap().waiters[0].is_empty());
    assert_eq!(woken(&count), 0);
}
