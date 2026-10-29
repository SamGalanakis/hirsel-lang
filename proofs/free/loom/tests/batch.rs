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
