hirsel_guard::install!();
use hirsel_free_loom::batch::{Binding, Directory, Key};
use hirsel_free_loom::sync::{Arc, AtomicU32, Ordering, model};

fn binding(generation: u32, lease: Arc<AtomicU32>) -> Binding<Arc<AtomicU32>, u32> {
    Binding {
        key: Key {
            slot: 7,
            generation,
        },
        path: vec![1, generation].into_boxed_slice(),
        lease,
        image: 0,
    }
}

#[test]
fn a_batch_wake_cannot_read_an_unsaved_image_or_publish_two_owners() {
    model(|| {
        let lease = Arc::new(AtomicU32::new(0));
        let (directory, mut saved) = Directory::admit(vec![binding(1, lease)]).unwrap();
        let notice = directory
            .notice(
                0,
                Key {
                    slot: 7,
                    generation: 1,
                },
            )
            .unwrap();
        let mut active = saved.pop().unwrap().start();
        let wake = loom::thread::spawn(move || {
            notice
                .notify()
                .map(|saved| {
                    let active = saved.start();
                    assert_eq!(*active.image(), 42);
                })
                .is_some()
        });
        *active.image_mut() = 42;
        let published = active
            .save(false)
            .map(|saved| {
                let active = saved.start();
                assert_eq!(*active.image(), 42);
            })
            .is_some();
        assert_ne!(published, wake.join().unwrap());
    });
}

#[test]
fn retained_notifications_keep_the_old_generation_after_directory_replacement() {
    model(|| {
        let lease = Arc::new(AtomicU32::new(0));
        let (old, mut saved) = Directory::admit(vec![binding(1, lease.clone())]).unwrap();
        let key = Key {
            slot: 7,
            generation: 1,
        };
        let notice = old.notice(0, key).unwrap();
        let active = saved.pop().unwrap().start();
        drop(old);
        let (new, mut next) = Directory::admit(vec![binding(2, lease)]).unwrap();
        assert!(new.notice(0, key).is_none());
        let late = loom::thread::spawn(move || notice.notify().is_some());
        drop(active);
        assert!(!late.join().unwrap());
        let mut active = next.pop().unwrap().start();
        assert_eq!(active.key(), key.successor().unwrap());
        *active.image_mut() = 99;
        assert_eq!(active.path(), &[1, 2]);
    });
}

#[test]
fn claim_storage_order_does_not_assign_retirement_order() {
    model(|| {
        let lease = Arc::new(AtomicU32::new(0));
        let (_, mut saved) =
            Directory::admit(vec![binding(3, lease.clone()), binding(1, lease)]).unwrap();
        let second = saved.pop().unwrap().start();
        assert_eq!(second.ordinal(), 1);
        assert_eq!(second.path(), &[1, 1]);
        let first = saved.pop().unwrap().start();
        assert_eq!(first.ordinal(), 0);
        assert_eq!(first.path(), &[1, 3]);
    });
}

#[test]
fn logical_retirement_keeps_the_active_borrow_and_generation_lease_until_physical_exit() {
    use hirsel_free_loom::{batch::Identity, frame};
    struct Lease(Arc<AtomicU32>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }
    struct Image {
        claim: Arc<Identity<Lease>>,
        value: u32,
    }
    model(|| {
        let freed = Arc::new(AtomicU32::new(0));
        let identity = Arc::new(Identity::bind(
            Key {
                slot: 7,
                generation: 3,
            },
            1,
            vec![4, 2].into_boxed_slice(),
            Lease(freed.clone()),
        ));
        let (wake, queued) = frame::prepared(Image {
            claim: identity.clone(),
            value: 0,
        });
        let mut active = queued.start();
        let terminal = loom::thread::spawn(move || identity.retire());
        active.value_mut().value = 42;
        terminal.join().unwrap();
        assert!(!active.value().claim.active());
        assert_eq!(active.value().claim.path(), &[4, 2]);
        assert_eq!(active.value().value, 42);
        assert_eq!(
            freed.load(Ordering::Acquire),
            0,
            "a terminal cut cannot free an active borrowed frame"
        );
        drop(active);
        assert!(wake.notify().is_none());
        assert_eq!(freed.load(Ordering::Acquire), 1);
    });
}

#[test]
fn startup_exposes_one_aggregate_only_after_every_success_or_failure() {
    use hirsel_free_loom::batch::Startup;
    model(|| {
        let startup = Arc::new(Startup::new(2));
        let first = startup.clone();
        let a = loom::thread::spawn(move || first.acknowledge(true));
        let second = startup.clone();
        let b = loom::thread::spawn(move || second.acknowledge(false));
        if let Some(outcome) = startup.outcome() {
            assert!(!outcome);
        }
        assert_ne!(
            a.join().unwrap(),
            b.join().unwrap(),
            "only the last acknowledgement wakes admission"
        );
        assert_eq!(startup.outcome(), Some(false));
    });
}

#[test]
fn saved_cpu_envelopes_keep_exact_jobs_stops_and_verdict_before_cleanup() {
    use hirsel_free_loom::batch::{Commit, publication::Publication};
    use hirsel_free_loom::cpu_budget::{self, stretch::*};
    use hirsel_run_schema::BudgetKind;
    model(|| {
        let budgets = Budgets {
            fuel: true,
            memory: true,
        };
        let mut state = Settled {
            level: 0,
            limit: 100,
            fuel: 5,
        };
        let mut jobs = Jobs::new(4, 2, state);
        let mut first = Stretch::new(budgets);
        first.fuel();
        first.alloc(60).unwrap();
        jobs.pause(0, first);
        assert!(cpu_budget::retire(&mut jobs, 0, &mut state).1.is_ok());
        cpu_budget::finish(&mut jobs, 0, Stretch::new(budgets), &mut state).unwrap();
        // The real holding rest can insert non-CPU charges between summaries.
        let mut rest = Stretch::new(budgets);
        rest.free(20);
        rest.fuel();
        rest.alloc(5).unwrap();
        state.settle(&rest).unwrap();
        let slot = Arc::new(Publication::default());
        let cleaned = Arc::new(AtomicU32::new(0));
        let child = loom::thread::spawn({
            let slot = slot.clone();
            let cleaned = cleaned.clone();
            move || {
                let mut charges = Stretch::new(budgets);
                charges.fuel_points(2);
                charges.alloc(60).unwrap();
                slot.publish((charges, vec![42u64, 99]));
                let accepted = loop {
                    if let Some(verdict) = slot.verdict() {
                        break verdict;
                    }
                    loom::thread::yield_now();
                };
                assert!(!accepted, "memory stop refuses the saved continuation");
                cleaned.store(1, Ordering::Release);
                slot.drained();
            }
        });
        let (envelope, verdict) = loop {
            if let Some(publication) = Publication::take(&slot) {
                break publication;
            }
            loom::thread::yield_now();
        };
        assert_eq!(cleaned.load(Ordering::Acquire), 0);
        assert_eq!(envelope.1, [42, 99]);
        // Storage ordinals do not grant authority; the real head identity does.
        assert!(Commit::claim(&mut jobs, 19, Some(7), true).is_none());
        assert!(Commit::claim(&mut jobs, 7, Some(7), false).is_none());
        let outcome = {
            let mut head = Commit::claim(&mut jobs, 7, Some(7), true).unwrap();
            head.state().pause(1, envelope.0);
            cpu_budget::retire(head.state(), 1, &mut state).1
        };
        assert_eq!(outcome.unwrap_err().kind, BudgetKind::Memory);
        assert_eq!(
            cleaned.load(Ordering::Acquire),
            0,
            "no cleanup before the fallible verdict"
        );
        drop(verdict);
        child.join().unwrap();
        assert!(slot.is_drained());
        assert_eq!(cleaned.load(Ordering::Acquire), 1);
    });
}

#[test]
fn withdrawal_keeps_the_frozen_charge_and_borrow_until_the_real_head_restores_it() {
    use hirsel_free_loom::batch::publication::Publication;
    use hirsel_free_loom::cpu_budget::{self, stretch::*};
    model(|| {
        let budgets = Budgets {
            fuel: true,
            memory: true,
        };
        let mut state = Settled {
            level: 0,
            limit: 100,
            fuel: 10,
        };
        let initial = state;
        let mut jobs = Jobs::new(4, 2, state);
        let slot = Arc::new(Publication::default());
        let child = loom::thread::spawn({
            let slot = slot.clone();
            move || {
                let mut prefix = Stretch::new(budgets);
                prefix.fuel();
                prefix.alloc(60).unwrap();
                slot.publish(prefix);
                loop {
                    if let Some(accepted) = slot.verdict() {
                        assert!(accepted);
                        break;
                    }
                    loom::thread::yield_now();
                }
                slot.drained();
            }
        });
        let (prefix, verdict) = loop {
            if let Some(value) = Publication::take(&slot) {
                break value;
            }
            loom::thread::yield_now();
        };
        jobs.pause(1, prefix.clone());
        let held = jobs.withdraw_younger(0);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].1, prefix);
        assert_eq!(state, initial);
        assert_eq!(
            slot.verdict(),
            None,
            "temporary withdrawal is not a continuation verdict"
        );
        assert!(
            Publication::take(&slot).is_none(),
            "head and withdrawal cannot own the envelope twice"
        );
        for (ordinal, charges, complete) in held {
            assert!(!complete);
            jobs.pause(ordinal, charges);
        }
        cpu_budget::finish(&mut jobs, 0, Stretch::new(budgets), &mut state).unwrap();
        let (accepted, result) = cpu_budget::retire(&mut jobs, 1, &mut state);
        assert_eq!(accepted, prefix);
        assert!(result.is_ok());
        verdict.accept();
        child.join().unwrap();
        assert!(slot.is_drained());
        assert_eq!(
            state,
            Settled {
                level: 60,
                limit: 100,
                fuel: 9
            }
        );
    });
}

#[test]
fn a_failed_receipt_refuses_before_any_saved_user_continuation() {
    use hirsel_free_loom::batch::publication::Publication;
    model(|| {
        let slot = Arc::new(Publication::default());
        let effects = Arc::new(AtomicU32::new(0));
        let child = loom::thread::spawn({
            let slot = slot.clone();
            let effects = effects.clone();
            move || {
                slot.publish(vec![42u64]);
                loop {
                    if let Some(accepted) = slot.verdict() {
                        if accepted {
                            effects.fetch_add(1, Ordering::Relaxed);
                        }
                        break;
                    }
                    loom::thread::yield_now();
                }
                slot.drained();
            }
        });
        let (value, pending) = loop {
            if let Some(value) = Publication::take(&slot) {
                break value;
            }
            loom::thread::yield_now();
        };
        assert_eq!(value, [42]);
        assert_eq!(effects.load(Ordering::Acquire), 0);
        // The same owned permit drops on record_summary's error return.
        drop(pending);
        child.join().unwrap();
        assert!(slot.is_drained());
        assert_eq!(effects.load(Ordering::Acquire), 0);
    });
}
