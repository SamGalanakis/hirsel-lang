use crate::{queue, sync::model};

#[test]
fn retiring_one_producer_keeps_other_queues_and_remote_steals_intact() {
    model(|| {
        let mut queues = queue::locals::<u32, 4>(2);
        let (mut left, thief) = queues.next().unwrap();
        let (mut right, _) = queues.next().unwrap();
        assert!(left.push(7).is_empty());
        assert!(right.push(11).is_empty());
        let stealing = loom::thread::spawn(move || thief.steal_half());
        drop(left);
        assert_eq!(right.pop(), Some(11));
        assert!(right.push(13).is_empty());
        assert_eq!(stealing.join().unwrap(), [7]);
        assert_eq!(right.drain(), [13]);
    });
}

#[test]
fn dropping_a_queues_last_handle_releases_its_values_before_other_queues_end() {
    use crate::sync::{Arc, AtomicBool, Ordering};
    struct Owned {
        value: u32,
        released: Arc<AtomicBool>,
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            self.released.store(true, Ordering::Release);
        }
    }
    model(|| {
        let left_released = Arc::new(AtomicBool::new(false));
        let right_released = Arc::new(AtomicBool::new(false));
        let mut queues = queue::locals::<Owned, 4>(2);
        let (mut left, thief) = queues.next().unwrap();
        let (mut right, _) = queues.next().unwrap();
        assert!(
            left.push(Owned {
                value: 7,
                released: left_released.clone()
            })
            .is_empty()
        );
        assert!(
            right
                .push(Owned {
                    value: 11,
                    released: right_released.clone()
                })
                .is_empty()
        );
        drop(left);
        assert!(!left_released.load(Ordering::Acquire));
        drop(thief);
        assert!(left_released.load(Ordering::Acquire));
        assert!(!right_released.load(Ordering::Acquire));
        let value = right.pop().unwrap();
        assert_eq!(value.value, 11);
        drop(value);
        assert!(right_released.load(Ordering::Acquire));
    });
}
