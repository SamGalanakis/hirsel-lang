use super::super::{CANCEL_DEADLINE, CANCEL_PARENT, CANCEL_REQUESTED, Scheduler, Scope, Task};
use super::*;
use std::sync::atomic::AtomicU64;

/// A scheduler with a root task, one open root scope and `children` linked
/// children spawned into it, allocated through the production slot path.
fn tree(children: usize) -> (Scheduler, u32, Vec<u32>) {
    let mut s = Scheduler::default();
    let (root, generation) = s.take_task_slot();
    assert_eq!((root, generation), (0, 1));
    s.tasks[0] = Some(Task::new(
        generation,
        std::ptr::null_mut(),
        None,
        vec![],
        None,
    ));
    let (scope, generation) = s.take_scope_slot();
    s.scopes[scope as usize] = Some(Scope {
        generation,
        owner: root,
        children: Vec::new(),
        canceled: Default::default(),
        deadline: None,
        completions: AtomicU64::new(0),
    });
    s.task(root).scopes.push(scope);
    let rows = (0..children)
        .map(|i| spawn(&mut s, scope, i as u32))
        .collect();
    (s, scope, rows)
}

fn spawn(s: &mut Scheduler, scope: u32, ordinal: u32) -> u32 {
    let (index, generation) = s.take_task_slot();
    let row = Task::new(
        generation,
        std::ptr::null_mut(),
        Some(scope),
        vec![ordinal],
        None,
    );
    assert!(s.tasks[index as usize].replace(row).is_none());
    s.scope(scope).children.push(index);
    s.link_child(0, index);
    index
}

fn retire(s: &mut Scheduler, scope: u32, child: u32) -> Task {
    s.scope(scope).children.retain(|&c| c != child);
    s.retire_task(child)
}

/// Spawn until a slot other than every fresh one is handed out, or `tries`
/// spawns; returns the reused slots in order.
fn reused(s: &mut Scheduler, scope: u32, tries: u32) -> Vec<u32> {
    let mut reused = Vec::new();
    for i in 0..tries {
        let before = s.tasks.len();
        let index = spawn(s, scope, 100 + i);
        if (index as usize) < before {
            reused.push(index);
        }
    }
    reused
}

#[derive(Clone, Copy, Debug)]
enum Owner {
    /// Unsaved: the worker's Running image still owns the frame.
    Active,
    /// Saved, but a queued publication still holds the frame.
    Saved,
    /// A notification token retains the task's control block.
    Notified,
    /// A delayed host request retains the task's control block.
    Request,
}

#[test]
fn runtime50_task_slot_reuse_waits_for_save_and_leases() {
    for owner in [Owner::Active, Owner::Saved, Owner::Notified, Owner::Request] {
        let (mut s, scope, children) = tree(3);
        let victim = children[1];
        let old = s.task_key(victim).unwrap();
        // Ownership acquired while the row was live.
        let mut frame = None;
        let mut held_frame = None;
        let mut lease = None;
        match owner {
            Owner::Active | Owner::Saved => {
                let f = crate::pool::free::Frame::unpublished(victim, false);
                if matches!(owner, Owner::Saved) {
                    f.mark_saved();
                }
                held_frame = Some(f.clone());
                frame = Some(Arc::downgrade(&f));
                s.task(victim).dispatch = Some(f);
            }
            Owner::Notified | Owner::Request => lease = Some(s.lease(victim)),
        }
        let control = s.lease(victim).downgrade();
        assert_eq!(
            lease.as_ref().map(TaskLease::key),
            lease.is_some().then_some(old)
        );

        s.reserve_retirements(1);
        let row = retire(&mut s, scope, victim);
        // The row is logically dead: no stale lookup, from key or lease.
        assert_eq!(s.live_task(old), None);
        assert!(s.tasks[victim as usize].is_none());
        if let Some(lease) = &lease {
            assert!(!lease.live());
        }
        drop(row);
        // One destruction only, after the last owner: not yet.
        assert!(control.upgrade().is_some(), "{owner:?}");

        // Force reuse pressure: many spawns never receive the retained slot.
        let early = reused(&mut s, scope, 16);
        assert!(!early.contains(&victim), "{owner:?} reused {early:?}");
        assert_eq!(s.task_slots.retained(), vec![victim], "{owner:?}");

        // The owner leaves; Active additionally needs its save acknowledgement.
        match owner {
            Owner::Active => {
                drop(held_frame.take());
                // Its last outside reference left, but without a save
                // acknowledgement the slot still is not reclaimable.
                s.task_slots.reclaim_all();
                assert_eq!(s.task_slots.retained(), vec![victim]);
                assert!(frame.as_ref().unwrap().upgrade().is_some());
                frame.as_ref().unwrap().upgrade().unwrap().mark_saved();
            }
            Owner::Saved => drop(held_frame.take()),
            Owner::Notified | Owner::Request => drop(lease.take()),
        }
        s.task_slots.reclaim_all();
        assert!(s.task_slots.retained().is_empty(), "{owner:?}");
        assert!(control.upgrade().is_none(), "{owner:?}");
        if let Some(frame) = &frame {
            assert!(frame.upgrade().is_none(), "{owner:?}");
        }

        let index = spawn(&mut s, scope, 999);
        assert_eq!(index, victim, "{owner:?}");
        let new = s.task_key(index).unwrap();
        assert_eq!(new.slot, old.slot);
        assert!(new.generation > old.generation);
        assert_eq!(s.live_task(old), None, "{owner:?}");
        assert_eq!(s.live_task(new), Some(victim));
    }
}

#[test]
fn an_unleased_saved_row_is_reused_immediately_with_its_next_generation() {
    let (mut s, scope, children) = tree(1);
    let child = children[0];
    let old = s.task_key(child).unwrap();
    drop(retire(&mut s, scope, child));
    assert_eq!(spawn(&mut s, scope, 1), child);
    assert_eq!(s.task_key(child).unwrap().generation, old.generation + 1);
}

#[test]
fn an_exhausted_slot_generation_is_never_reused() {
    let mut slots = Slots::<3>::default();
    assert_eq!(slots.take(0), (0, 1));
    slots.release(0, 1);
    assert_eq!(slots.take(1), (0, 2));
    slots.release(0, 2);
    assert_eq!(slots.take(1), (0, 3));
    slots.release(0, 3);
    // Exhausted: a fresh slot, never generation 1 of slot 0 again.
    assert_eq!(slots.take(1), (1, 1));
}

#[test]
fn scope_and_set_generations_are_per_slot_and_validated_by_key() {
    let (mut s, scope, _) = tree(0);
    let key = s.scope_key(scope).unwrap();
    assert_eq!(s.live_scope(key), Some(scope));
    s.task(0).scopes.pop();
    drop(s.take_scope(scope));
    assert_eq!(s.live_scope(key), None);
    let (again, generation) = s.take_scope_slot();
    assert_eq!((again, u64::from(generation)), (scope, key.generation + 1));
    let (set, generation) = s.take_set_slot();
    assert_eq!(generation, 1);
    s.sets[set as usize] = Some(super::super::TaskSet {
        generation,
        scope: again,
        members: Vec::new(),
    });
    s.close_set(set);
    assert_eq!(s.take_set_slot(), (set, 2));
}

#[test]
fn cancellation_notices_visit_only_affected_subscriptions_with_a_post_register_check() {
    let (mut s, scope, children) = tree(3);
    let (a, b, c) = (children[0], children[1], children[2]);
    // b's descendant d, linked under b.
    let (d, generation) = s.take_task_slot();
    s.tasks[d as usize] = Some(Task::new(
        generation,
        std::ptr::null_mut(),
        Some(scope),
        vec![1, 0],
        None,
    ));
    s.link_child(b, d);
    let ra = s.subscribe_cancel(a, 10);
    let rb = s.subscribe_cancel(b, 20);
    let rd = s.subscribe_cancel(d, 40);
    let rc = s.subscribe_cancel(c, 30);
    assert_eq!(ra.canceled, None);
    s.unsubscribe_cancel(rc.key, rc.registration);

    s.cancel(b, CANCEL_REQUESTED);
    let mut out = Vec::new();
    s.drain_cancel_notices(&mut out);
    assert_eq!(
        out,
        vec![
            CancelNotice {
                task: rb.key,
                token: 20,
                registration: rb.registration
            },
            CancelNotice {
                task: rd.key,
                token: 40,
                registration: rd.registration
            },
        ]
    );
    // Services partition notices by token bits without consuming others'.
    s.cancel(a, CANCEL_REQUESTED);
    let mut concurrent = Vec::new();
    s.drain_cancel_notices_matching(1 << 63, 1 << 63, &mut concurrent);
    assert!(concurrent.is_empty());
    s.drain_cancel_notices_matching(1 << 63, 0, &mut concurrent);
    assert_eq!(concurrent.len(), 1);
    assert_eq!(concurrent[0].token, 10);
    // A registration after the cancellation sees it at once.
    let late = s.subscribe_cancel(d, 41);
    assert_eq!(late.canceled, Some(CANCEL_PARENT));
    out.clear();
    s.drain_cancel_notices(&mut out);
    assert_eq!(
        out,
        vec![CancelNotice {
            task: late.key,
            token: 41,
            registration: late.registration
        }]
    );
    out.clear();

    // A scope cancellation notifies only the scope's owner.
    let r0 = s.subscribe_cancel(0, 1);
    s.cancel_scope(scope, CANCEL_DEADLINE);
    s.drain_cancel_notices(&mut out);
    assert_eq!(out.iter().map(|n| n.token).collect::<Vec<_>>(), vec![1]);
    assert_eq!(out[0].registration, r0.registration);

    // Retirement takes a row's subscriptions with it; a stale key is inert.
    s.reserve_retirements(1);
    drop(retire(&mut s, scope, c));
    s.unsubscribe_cancel(rc.key, rc.registration);
    let e = spawn(&mut s, scope, 7);
    assert_eq!(e, c);
    assert!(s.task(e).cancel_subs.is_empty());
}
