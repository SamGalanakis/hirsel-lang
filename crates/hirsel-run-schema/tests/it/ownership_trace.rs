use hirsel_run_schema::Site;
use hirsel_run_schema::ownership_trace::*;

fn task(n: u32) -> TaskPath {
    TaskPath(vec![n])
}
fn token(n: u64) -> Token {
    Token {
        task: task(0),
        sequence: n,
    }
}
fn owner(n: u32) -> Owner {
    Owner {
        task: Some(task(0)),
        storage: Storage::Value { frame: 1, value: n },
        path: vec![],
    }
}
fn event(sequence: u64, action: Action) -> Event {
    Event {
        task: task(0),
        operation: sequence,
        subordinal: 0,
        site: Site {
            definition: "fixture.resource".into(),
            file: "fixture.hir".into(),
            line: sequence + 1,
            column: 1,
        },
        action,
    }
}
fn create(sequence: u64, id: u64, at: Owner) -> Event {
    event(
        sequence,
        Action::Create {
            token: token(id),
            owner: at,
            value_type: "fixture.Token".into(),
        },
    )
}
fn moved(sequence: u64, id: u64, from: Owner, to: Owner) -> Event {
    event(
        sequence,
        Action::Move {
            token: token(id),
            from,
            owner: to,
        },
    )
}
fn dropped(sequence: u64, id: u64, at: Owner) -> Event {
    event(
        sequence,
        Action::Drop {
            token: token(id),
            owner: at,
        },
    )
}
fn chain() -> Vec<Event> {
    vec![
        create(0, 0, owner(0)),
        moved(1, 0, owner(0), owner(1)),
        moved(2, 0, owner(1), owner(2)),
        dropped(3, 0, owner(2)),
    ]
}
fn inventory(events: &[Event]) -> Inventory {
    let mut inventory = Inventory::new(100);
    for event in events {
        inventory.apply(event).unwrap();
    }
    inventory
}

#[test]
fn ownership_trace_equal_pointer_free_values_have_distinct_lineages() {
    let events = [create(0, 0, owner(0)), create(1, 1, owner(1))];
    let inventory = inventory(&events);
    assert_ne!(
        inventory.lineage(&token(0)).unwrap().token,
        inventory.lineage(&token(1)).unwrap().token
    );
    let analysis = inventory.finish(true, 2).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 0, 2)
    );
}

#[test]
fn ownership_trace_leak_at_scope_exit_retains_its_creation_and_move() {
    let forgotten = Owner {
        task: Some(task(0)),
        storage: Storage::Forgotten { frame: 1 },
        path: vec![],
    };
    let events = [create(0, 0, owner(0)), moved(1, 0, owner(0), forgotten)];
    let inventory = inventory(&events);
    let leaks = inventory.held_by_frame(&task(0), 1);
    assert_eq!(leaks.len(), 1);
    assert_eq!(leaks[0].events, events);
    let analysis = inventory.finish(true, 2).unwrap();
    assert_eq!(analysis.live.len(), 1);
    assert_eq!(analysis.dropped, 0, "forget cannot manufacture a drop");
}

#[test]
fn ownership_trace_double_drop_reports_the_exact_move_chain() {
    let events = chain();
    let mut inventory = inventory(&events);
    let extra = dropped(4, 0, owner(2));
    let error = inventory.apply(&extra).unwrap_err();
    assert_eq!(error.problem, Problem::DoubleDrop);
    assert_eq!(error.history, events);
    let text = error.to_string();
    assert!(text.contains("create root.0#0"));
    assert_eq!(text.matches("move root.0#0").count(), 2);
    assert_eq!(text.matches("drop root.0#0").count(), 2);
    assert!(
        inventory.finish(true, 4).is_err(),
        "ignored corruption cannot become a pass"
    );
}

#[test]
fn ownership_trace_drop_by_the_previous_owner_is_use_after_move() {
    let events = chain();
    let mut inventory = inventory(&events[..3]);
    let error = inventory.apply(&dropped(3, 0, owner(0))).unwrap_err();
    assert_eq!(error.problem, Problem::WrongOwner);
    assert_eq!(error.history, events[..3]);
}

#[test]
fn ownership_trace_reused_storage_does_not_reuse_a_token() {
    let events = [
        create(0, 0, owner(0)),
        dropped(1, 0, owner(0)),
        create(2, 1, owner(0)),
        dropped(3, 1, owner(0)),
    ];
    let observed = inventory(&events);
    assert_eq!(observed.lineage(&token(0)).unwrap().events, events[..2]);
    assert_eq!(observed.lineage(&token(1)).unwrap().events, events[2..]);
    let analysis = observed.finish(true, 4).unwrap();
    assert_eq!((analysis.created, analysis.dropped), (2, 2));
    let mut reused = inventory(&events[..2]);
    assert_eq!(
        reused.apply(&create(2, 0, owner(0))).unwrap_err().problem,
        Problem::IdentityReuse
    );
}

#[test]
fn ownership_trace_field_capture_channel_and_child_task_keep_one_identity() {
    let field = Owner {
        path: vec![Projection::Field { field: 2 }],
        ..owner(1)
    };
    let capture = Owner {
        task: None,
        storage: Storage::Capture {
            environment: 7,
            capture: 0,
        },
        path: vec![],
    };
    let child = Owner {
        task: Some(task(1)),
        storage: Storage::Slot { frame: 1, slot: 0 },
        path: vec![],
    };
    let queue = Owner {
        task: None,
        storage: Storage::Object {
            name: "channel".into(),
            id: 3,
            item: 0,
        },
        path: vec![],
    };
    let writer = Owner {
        task: Some(task(2)),
        storage: Storage::Value { frame: 1, value: 0 },
        path: vec![],
    };
    let mut events = vec![
        create(10, 0, owner(0)),
        moved(11, 0, owner(0), field.clone()),
        moved(12, 0, field, capture.clone()),
    ];
    let mut entry = moved(0, 0, capture, child.clone());
    entry.task = task(1);
    events.push(entry);
    let mut send = moved(1, 0, child, queue.clone());
    send.task = task(1);
    events.push(send);
    let mut receive = moved(0, 0, queue, writer.clone());
    receive.task = task(2);
    events.push(receive);
    let mut drop = dropped(1, 0, writer);
    drop.task = task(2);
    events.push(drop);
    let inventory = inventory(&events);
    assert!(inventory.held_by_frame(&task(0), 1).is_empty());
    assert_eq!(inventory.lineage(&token(0)).unwrap().events, events);
    let analysis = inventory.finish(true, 7).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
}

#[test]
fn ownership_trace_cancellation_before_child_entry_drops_the_captured_token() {
    let environment = Owner {
        task: None,
        storage: Storage::Capture {
            environment: 8,
            capture: 1,
        },
        path: vec![],
    };
    let events = [
        create(0, 0, owner(0)),
        moved(1, 0, owner(0), environment.clone()),
        dropped(2, 0, environment),
    ];
    let analysis = inventory(&events).finish(true, 3).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
}

#[test]
fn ownership_trace_unknown_token_wrong_task_and_occupied_owner_are_rejected() {
    let mut unknown = Inventory::new(10);
    assert_eq!(
        unknown.apply(&dropped(0, 0, owner(0))).unwrap_err().problem,
        Problem::UnknownToken
    );
    let first = create(0, 0, owner(0));
    let mut occupied = inventory(&[first.clone()]);
    assert_eq!(
        occupied.apply(&create(1, 1, owner(0))).unwrap_err().problem,
        Problem::OccupiedOwner
    );
    let mut wrong_task = inventory(&[first]);
    let mut drop = dropped(1, 0, owner(0));
    drop.task = task(1);
    assert_eq!(
        wrong_task.apply(&drop).unwrap_err().problem,
        Problem::WrongTask
    );
}

#[test]
fn ownership_trace_per_task_operation_and_subordinal_order_is_strict() {
    let first = create(0, 0, owner(0));
    let mut inventory = inventory(&[first]);
    let mut next = moved(0, 0, owner(0), owner(1));
    next.subordinal = 1;
    inventory.apply(&next).unwrap();
    let mut repeated = dropped(0, 0, owner(1));
    repeated.subordinal = 1;
    assert_eq!(
        inventory.apply(&repeated).unwrap_err().problem,
        Problem::Order
    );
}

#[test]
fn ownership_trace_missing_suffix_wrong_count_and_overflow_never_pass() {
    let events = chain();
    assert_eq!(
        inventory(&events).finish(false, 4).unwrap_err().problem,
        Problem::Incomplete
    );
    assert_eq!(
        inventory(&events).finish(true, 3).unwrap_err().problem,
        Problem::EventCount
    );
    let mut small = Inventory::new(1);
    small.apply(&events[0]).unwrap();
    assert_eq!(
        small.apply(&events[1]).unwrap_err().problem,
        Problem::EventBudget
    );
    assert_eq!(
        small.finish(true, 1).unwrap_err().problem,
        Problem::EventBudget
    );
}

#[test]
fn ownership_trace_json_roundtrip_preserves_readable_logical_identity() {
    let events = chain();
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(encoded.contains("\"kind\":\"move\""));
    let decoded: Vec<Event> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, events);
    assert_eq!(inventory(&decoded).finish(true, 4).unwrap().live.len(), 0);
}

fn context(operation: u64) -> Context {
    Context {
        task: task(0),
        operation,
        site: Site {
            definition: "fixture".into(),
            file: "fixture.hir".into(),
            line: operation + 1,
            column: 1,
        },
    }
}

#[test]
fn ownership_tracker_assigns_fresh_ids_through_reused_locations() {
    let mut tracker = Tracker::new(20);
    let first = tracker
        .create(&context(0), owner(0), "fixture.Token".into())
        .unwrap();
    tracker.drop_value(&context(1), &owner(0)).unwrap();
    let second = tracker
        .create(&context(2), owner(0), "fixture.Token".into())
        .unwrap();
    assert_ne!(first, second);
    tracker.drop_value(&context(3), &owner(0)).unwrap();
    let trace = tracker.finish([7; 32], true).unwrap();
    assert_eq!(trace.analyze([7; 32], 20).unwrap().dropped, 2);
}

#[test]
fn ownership_tracker_moves_nested_fields_without_recreating_their_identities() {
    let mut tracker = Tracker::new(20);
    let root = tracker
        .create(&context(0), owner(0), "fixture.Container".into())
        .unwrap();
    let field = Owner {
        path: vec![
            Projection::Field { field: 1 },
            Projection::Index { index: 3 },
        ],
        ..owner(0)
    };
    let child = tracker
        .create(&context(1), field, "fixture.Token".into())
        .unwrap();
    let moved = tracker
        .move_tree(&context(2), &owner(0), &owner(1))
        .unwrap();
    assert!(moved.contains(&root) && moved.contains(&child));
    tracker.drop_value(&context(3), &owner(1)).unwrap();
    let held = tracker.inventory().lineage(&child).unwrap();
    assert!(
        held.owner.is_some(),
        "dropping the container cannot assert its child was finalized"
    );
    assert_eq!(
        held.owner.as_ref().unwrap().path,
        vec![
            Projection::Field { field: 1 },
            Projection::Index { index: 3 }
        ]
    );
    let child_at = held.owner.clone().unwrap();
    tracker.drop_value(&context(4), &child_at).unwrap();
    let trace = tracker.finish([7; 32], true).unwrap();
    assert_eq!(trace.analyze([7; 32], 20).unwrap().live.len(), 0);
}

#[test]
fn ownership_tracker_copies_fresh_instances_and_keeps_the_source_live() {
    let mut tracker = Tracker::new(20);
    let source = tracker
        .create(&context(0), owner(0), "Bytes".into())
        .unwrap();
    let copy = tracker
        .copy_tree(&context(1), &owner(0), &owner(1))
        .unwrap();
    assert_eq!(copy.len(), 1);
    assert_ne!(copy[0], source);
    let trace = tracker.finish([7; 32], true).unwrap();
    assert_eq!(trace.analyze([7; 32], 20).unwrap().live.len(), 2);
}

#[test]
fn ownership_tracker_missing_moved_owner_retains_the_actual_chain() {
    let mut tracker = Tracker::new(20);
    tracker
        .create(&context(0), owner(0), "fixture.Token".into())
        .unwrap();
    tracker
        .move_tree(&context(1), &owner(0), &owner(1))
        .unwrap();
    tracker
        .move_tree(&context(2), &owner(1), &owner(2))
        .unwrap();
    let error = tracker.drop_value(&context(3), &owner(0)).unwrap_err();
    assert_eq!(error.problem, Problem::MissingOwner(owner(0)));
    assert_eq!(error.history.len(), 3);
    assert!(tracker.finish([7; 32], true).is_err());
}

#[test]
fn ownership_tracker_sidecar_binding_completeness_and_count_are_independent() {
    let mut tracker = Tracker::new(20);
    tracker
        .create(&context(0), owner(0), "fixture.Token".into())
        .unwrap();
    tracker.drop_value(&context(1), &owner(0)).unwrap();
    let trace = tracker.finish([7; 32], true).unwrap();
    let encoded = serde_json::to_string(&trace).unwrap();
    let mut decoded: Trace = serde_json::from_str(&encoded).unwrap();
    decoded.analyze([7; 32], 20).unwrap();
    assert_eq!(
        decoded.analyze([8; 32], 20).unwrap_err().problem,
        Problem::Program
    );
    decoded.version += 1;
    assert_eq!(
        decoded.analyze([7; 32], 20).unwrap_err().problem,
        Problem::Version
    );
    decoded = trace.clone();
    decoded.complete = false;
    assert_eq!(
        decoded.analyze([7; 32], 20).unwrap_err().problem,
        Problem::Incomplete
    );
    decoded = trace;
    decoded.event_count += 1;
    assert_eq!(
        decoded.analyze([7; 32], 20).unwrap_err().problem,
        Problem::EventCount
    );
}

#[test]
fn ownership_trace_an_absent_frame_task_cannot_masquerade_as_object_custody() {
    let mut absent = owner(0);
    absent.task = None;
    let mut trace = Inventory::new(20);
    assert_eq!(
        trace.apply(&create(0, 0, absent)).unwrap_err().problem,
        Problem::WrongTask
    );
}

#[test]
fn ownership_trace_moving_a_retired_token_is_use_after_drop() {
    let events = chain();
    let mut trace = inventory(&events);
    let error = trace.apply(&moved(4, 0, owner(2), owner(3))).unwrap_err();
    assert_eq!(error.problem, Problem::UseAfterDrop);
    assert_eq!(error.history, events);
}
