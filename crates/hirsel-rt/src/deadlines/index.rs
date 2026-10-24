use super::super::Deadlines;

#[test]
fn exact_domain_indexes_remove_ready_and_due_without_retaining_entries() {
    for idle in [256, 512, 1024] {
        let mut real = Deadlines::default();
        let mut virtual_time = Deadlines::default();
        for token in 0..idle {
            real.insert(token, 1000u64 + token);
            virtual_time.insert(token, 10_000u64 + token);
        }
        real.insert(idle, 50);
        // One ready token is removed directly; one expiry visits the due
        // prefix only. Neither step enumerates the idle registrations.
        assert_eq!(real.remove(idle - 1), Some(1000 + idle - 1));
        let mut visited = 1;
        while let Some((at, token)) = real.pop_due(50) {
            assert_eq!((at, token), (50, idle));
            visited += 1;
        }
        assert_eq!(visited, 2);
        assert_eq!(real.stored(), (idle as usize - 1, idle as usize - 1));
        assert_eq!(virtual_time.stored(), (idle as usize, idle as usize));
        assert_eq!(virtual_time.pop_due(50), None, "separate clock domain");
        for token in 0..=idle {
            real.remove(token);
            virtual_time.remove(token);
        }
        assert_eq!(real.stored(), (0, 0));
        assert_eq!(virtual_time.stored(), (0, 0));
        println!("idle={idle} visits={visited} stored_after_exit=0");
    }
}

#[test]
fn exact_index_rearm_exit_equality_and_stable_ties() {
    let mut timers = Deadlines::default();
    // Insertion order is deliberately reversed; semantic registration tokens
    // determine ties, rather than the tree's insertion or allocation order.
    for token in (0..4).rev() {
        timers.insert(token, 100u64);
    }
    assert_eq!(timers.pop_due(99), None);
    assert_eq!(timers.remove(1), Some(100));
    timers.insert(2, 200);
    assert_eq!(timers.stored(), (3, 3));
    assert_eq!(timers.pop_due(100), Some((100, 0)));
    assert_eq!(timers.pop_due(100), Some((100, 3)));
    assert_eq!(timers.pop_due(100), None);
    assert_eq!(timers.next(), Some(200));
    assert_eq!(timers.pop_due(200), Some((200, 2)));
    assert_eq!(timers.stored(), (0, 0));
}
