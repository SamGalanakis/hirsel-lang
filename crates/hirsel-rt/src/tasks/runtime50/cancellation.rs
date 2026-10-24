use super::super::*;
const CONCURRENT: u64 = 1 << 63;

#[test]
fn runtime50_cancel_visits_only_affected_registrations() {
    for idle in [256, 512, 1024] {
        let mut s = Scheduler::new(Schedule::default(), std::ptr::null_mut());
        s.logical = Logical::Free(free::Driver::default());
        let (scope, generation) = s.take_scope_slot();
        s.scopes[scope as usize] = Some(Scope {
            generation,
            owner: 0,
            children: Vec::new(),
            canceled: Default::default(),
            deadline: None,
            completions: AtomicU64::new(0),
        });
        s.task(0).scopes.push(scope);
        let mut children = Vec::new();
        for ordinal in 0..idle {
            let (slot, generation) = s.take_task_slot();
            s.tasks[slot as usize] = Some(Task::new(
                generation,
                std::ptr::null_mut(),
                Some(scope),
                vec![ordinal],
                None,
            ));
            s.scope(scope).children.push(slot);
            s.link_child(0, slot);
            s.current = slot;
            s.park(Wait::Host {
                token: CONCURRENT | u64::from(ordinal),
            });
            children.push(slot);
        }
        s.current = 0;
        let child = children[0];
        s.cancel(child, CANCEL_REQUESTED);
        assert_eq!(
            s.cancel_notices.len(),
            1,
            "unrelated subscriptions are untouched"
        );
        assert_eq!(
            s.host_cancellations(CONCURRENT, CONCURRENT),
            vec![CONCURRENT]
        );
        assert!(s.host_cancellations(CONCURRENT, CONCURRENT).is_empty());
        // A notice queued before unpark cannot revive a retry with the same
        // request token. Its new registration's post-register recheck wins.
        s.cancel(child, CANCEL_REQUESTED);
        let stale = s.cancel_notices[0];
        s.unpark(child);
        assert!(s.task(child).cancel_subs.is_empty());
        s.current = child;
        s.park(Wait::Host { token: CONCURRENT });
        let current = s.task(child).host_cancel.unwrap();
        assert_ne!(current.registration, stale.registration);
        assert_eq!(
            s.host_cancellations(CONCURRENT, CONCURRENT),
            vec![CONCURRENT]
        );
        s.unpark(child);
        // Delayed cancellation after actual row reuse must not reach a new
        // task that happens to be waiting on the same numerical token.
        s.scope(scope).children.retain(|&id| id != child);
        drop(s.retire_task(child));
        let (reused, generation) = s.take_task_slot();
        assert_eq!(reused, child);
        s.tasks[reused as usize] = Some(Task::new(
            generation,
            std::ptr::null_mut(),
            Some(scope),
            vec![idle],
            None,
        ));
        s.scope(scope).children.push(reused);
        s.link_child(0, reused);
        s.current = reused;
        s.park(Wait::Host { token: CONCURRENT });
        s.cancel_notices.push(stale);
        assert!(s.host_cancellations(CONCURRENT, CONCURRENT).is_empty());
        assert_ne!(s.task_key(reused), Some(stale.task));
        for child in children {
            if s.task(child).wait.is_some() {
                s.unpark(child);
            }
        }
        assert!(
            s.tasks
                .iter()
                .flatten()
                .all(|row| row.cancel_subs.is_empty() && row.host_cancel.is_none())
        );
    }
}
