use super::*;

fn site(frame: usize, state: u32, kind: u32) -> BoundSite {
    BoundSite {
        task: TaskKey {
            slot: 2,
            generation: 7,
        },
        invocation: 0,
        frame,
        state,
        kind,
        bytes: 64,
        anchor: 31,
        operation_site: 41,
    }
}

#[test]
fn pending_rebind_retains_identity_and_loop_reentry_gets_a_fresh_key() {
    let mut bindings = Bindings::default();
    let input = site(0x1000, 1, 16);
    bindings.bind(input);
    let original = bindings.take(16).unwrap();
    for _ in 0..4 {
        assert_eq!(
            bindings.take(16),
            None,
            "opaque callers inherit no permission"
        );
        bindings.bind(input);
        assert_eq!(bindings.take(16), Some(original));
    }
    bindings.complete(original);
    bindings.bind(input);
    let next = bindings.take(16).unwrap();
    assert_ne!(original.invocation, next.invocation);
    bindings.complete(next);
    assert!(bindings.stack.is_empty());
}

#[test]
fn child_bindings_retain_disarmed_parent_and_never_share_invocation_identity() {
    let mut bindings = Bindings::default();
    bindings.bind(site(0x1000, 1, 16));
    let parent = bindings.take(16).unwrap();
    bindings.bind(site(0x1020, 2, 0));
    let child = bindings.take(0).unwrap();
    assert_ne!(parent.invocation, child.invocation);
    assert_eq!(bindings.take(16), None);
    bindings.complete(child);
    assert_eq!(bindings.stack.last().unwrap().site, parent);
    bindings.complete(parent);
}

#[test]
#[should_panic(expected = "stale bound tail completion")]
fn bad_parent_completion_cannot_retire_a_pending_child() {
    let mut bindings = Bindings::default();
    bindings.bind(site(0x1000, 1, 16));
    let parent = bindings.take(16).unwrap();
    bindings.bind(site(0x1020, 2, 0));
    bindings.take(0);
    bindings.complete(parent);
}

#[test]
#[should_panic(expected = "different entered-tail invocation")]
fn bad_rebind_cannot_target_a_reused_task_slot() {
    let mut bindings = Bindings::default();
    let mut input = site(0x1000, 1, 16);
    bindings.bind(input);
    bindings.take(16);
    input.task.generation += 1;
    bindings.bind(input);
}

#[test]
#[should_panic(expected = "bound tail reached a different intrinsic")]
fn bad_armed_binding_cannot_enter_another_operation_kind() {
    let mut bindings = Bindings::default();
    bindings.bind(site(0x1000, 1, 16));
    bindings.take(0);
}

#[test]
#[should_panic(expected = "entered tail rebound before its intrinsic")]
fn bad_duplicate_bind_cannot_begin_the_same_site_twice() {
    let mut bindings = Bindings::default();
    let input = site(0x1000, 1, 16);
    bindings.bind(input);
    bindings.bind(input);
}

#[test]
fn abandoning_nested_bindings_reclaims_only_administrative_keys() {
    let mut bindings = Bindings::default();
    for i in 0..16 {
        bindings.bind(site(0x1000 + i * 64, 1, 16));
        bindings.take(16);
    }
    // No waker, task lease, native stack pointer or language-drop thunk is
    // stored in this record. Teardown cannot execute another language action.
    assert_eq!(bindings.stack.len(), 16);
    drop(bindings);
}
