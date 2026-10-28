use hirsel_run_schema::Site;
use hirsel_run_schema::ownership_trace::{Action, Storage, native::*};
use std::collections::{BTreeMap, BTreeSet};

const FIRST: u64 = 1 << 32;
const SECOND: u64 = (1 << 32) | 1;
const ARGUMENT: u64 = (2 << 32) | 1;

fn event(marker: u32, action: u32, token: u64, owner: u64) -> Row {
    Row::Event {
        task: 0,
        frame: 1,
        marker,
        action,
        token,
        owner,
        site: 42,
        layout: 7,
    }
}
fn transfer(marker: u32, direction: ChannelDirection, status: u32) -> Row {
    Row::QueueTransfer {
        task: 0,
        frame: 1,
        marker,
        queue: 1,
        layout: 99,
        item: 0,
        direction,
        status,
    }
}
fn fixture() -> (Log, Units) {
    let markers = [
        (0, 0, FIRST, FIRST),
        (1, 8, FIRST, ARGUMENT),
        (2, 8, SECOND, ARGUMENT),
        (3, 2, SECOND, SECOND),
    ]
    .into_iter()
    .map(|(id, action, token, owner)| {
        (
            id,
            Box::new(Metadata {
                action,
                channel: (action == 8).then_some(ChannelBinding {
                    direction: if id == 1 {
                        ChannelDirection::Send
                    } else {
                        ChannelDirection::Receive
                    },
                    descriptor: 99,
                }),
                optional_carrier: false,
                empty_environment: false,
                callee: None,
                cleanup: None,
                cleanup_paths: BTreeMap::new(),
                additional_drops: Vec::new(),
                token_path: None,
                owner_path: None,
                token,
                owner,
                site: 42,
                layout: 7,
                source: Site {
                    definition: "fixture::channel".into(),
                    file: "channel.hir".into(),
                    line: id as u64 + 1,
                    column: 1,
                },
                value_type: "Token".into(),
            }),
        )
    })
    .collect();
    let rows = vec![
        Row::Enter {
            task: 0,
            frame: 1,
            unit: [7; 32],
        },
        Row::QueueCreated {
            task: 0,
            queue: 1,
            layout: 99,
        },
        event(0, 0, FIRST, FIRST),
        event(1, 8, FIRST, ARGUMENT),
        Row::QueueSent {
            task: 0,
            queue: 1,
            layout: 99,
            item: 0,
            status: 0,
            pending: false,
        },
        transfer(1, ChannelDirection::Send, 0),
        event(2, 8, SECOND, ARGUMENT),
        Row::QueueReceived {
            task: 0,
            queue: 1,
            layout: 99,
            item: 0,
            status: 0,
        },
        transfer(2, ChannelDirection::Receive, 0),
        event(3, 2, SECOND, SECOND),
        Row::Leave { task: 0, frame: 1 },
    ];
    (
        Log {
            version: VERSION,
            program: [49; 32],
            row_count: rows.len() as u64,
            rows,
            complete: true,
        },
        BTreeMap::from([([7; 32], markers)]),
    )
}

fn decode(
    log: &mut Log,
    units: &Units,
) -> Result<hirsel_run_schema::ownership_trace::Trace, Error> {
    log.row_count = log.rows.len() as u64;
    decode_local_calls(
        log,
        log.program,
        units,
        &BTreeMap::from([([7; 32], BTreeSet::new())]),
        100,
    )
}

#[test]
fn queue_transfer_preserves_one_token_through_checked_caller_and_fifo_custody() {
    let (mut log, units) = fixture();
    let trace = decode(&mut log, &units).unwrap();
    let analysis = trace.analyze(log.program, 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    assert_eq!(trace.events.len(), 4);
    let Action::Move { from, owner, .. } = &trace.events[1].action else {
        panic!("missing send move")
    };
    assert_eq!(from.storage, Storage::Slot { frame: 1, slot: 0 });
    assert_eq!(owner.task, None);
    assert_eq!(
        owner.storage,
        Storage::Object {
            name: "native.channel".into(),
            id: 1,
            item: 0
        }
    );
    let Action::Move {
        from: received,
        owner: destination,
        ..
    } = &trace.events[2].action
    else {
        panic!("missing receive move")
    };
    assert_eq!(received, owner);
    assert_eq!(destination.storage, Storage::Slot { frame: 1, slot: 1 });
}

#[test]
fn queue_transfer_refuses_missing_or_duplicated_intents_receipts_and_commits() {
    let (log, units) = fixture();
    for index in [2, 3, 4, 5, 6, 7, 8] {
        let mut changed = log.clone();
        changed.rows.remove(index);
        assert!(
            decode(&mut changed, &units).is_err(),
            "missing row {index} passed"
        );
    }
    for index in [3, 4, 5, 6, 7, 8] {
        let mut changed = log.clone();
        changed.rows.insert(index, changed.rows[index]);
        assert!(
            decode(&mut changed, &units).is_err(),
            "duplicate row {index} passed"
        );
    }
}

#[test]
fn queue_transfer_refuses_forged_descriptor_direction_position_marker_and_frame() {
    let (log, units) = fixture();
    for field in 0..7 {
        let mut changed = log.clone();
        let Row::QueueTransfer {
            frame,
            marker,
            queue,
            layout,
            item,
            direction,
            status,
            ..
        } = &mut changed.rows[5]
        else {
            unreachable!()
        };
        match field {
            0 => *frame = 2,
            1 => *marker = 2,
            2 => *queue = 2,
            3 => *layout = 100,
            4 => *item = 1,
            5 => *direction = ChannelDirection::Receive,
            _ => *status = 1,
        }
        assert!(
            decode(&mut changed, &units).is_err(),
            "forged field {field} passed"
        );
    }
    let mut forged = units.clone();
    forged
        .get_mut(&[7; 32])
        .unwrap()
        .get_mut(&1)
        .unwrap()
        .channel = None;
    assert!(decode(&mut log.clone(), &forged).is_err());
}

#[test]
fn queue_transfer_pending_publication_and_canceled_receive_do_not_reassign_credit() {
    let (mut log, units) = fixture();
    let Row::QueueSent { pending, .. } = &mut log.rows[4] else {
        unreachable!()
    };
    *pending = true;
    assert!(decode(&mut log.clone(), &units).is_err());
    log.rows.insert(
        6,
        Row::QueuePublished {
            task: 0,
            queue: 1,
            layout: 99,
            item: 0,
        },
    );
    let expected = decode(&mut log, &units).unwrap();
    log.rows.splice(
        7..7,
        [
            event(2, 8, SECOND, ARGUMENT),
            Row::QueueReceived {
                task: 0,
                queue: 1,
                layout: 99,
                item: 0,
                status: 2,
            },
            transfer(2, ChannelDirection::Receive, 2),
        ],
    );
    assert_eq!(decode(&mut log, &units).unwrap(), expected);
}

#[test]
fn queue_transfer_closed_send_keeps_the_actual_caller_owner() {
    let (mut log, mut units) = fixture();
    log.rows.truncate(6);
    log.rows.insert(
        2,
        Row::QueueDetached {
            task: 0,
            queue: 1,
            layout: 99,
            first: 0,
            end: 0,
        },
    );
    let Row::QueueSent { status, .. } = &mut log.rows[5] else {
        unreachable!()
    };
    *status = 1;
    log.rows[6] = transfer(1, ChannelDirection::Send, 1);
    log.rows
        .extend([event(3, 2, FIRST, FIRST), Row::Leave { task: 0, frame: 1 }]);
    let drop = units.get_mut(&[7; 32]).unwrap().get_mut(&3).unwrap();
    drop.token = FIRST;
    drop.owner = FIRST;
    let trace = decode(&mut log, &units).unwrap();
    assert_eq!(trace.events.len(), 2);
    let analysis = trace.analyze(log.program, 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
}

#[test]
fn queue_transfer_cannot_overwrite_a_live_receive_destination() {
    let (mut log, mut units) = fixture();
    let mut second = units[&[7; 32]][&0].clone();
    second.token = SECOND;
    second.owner = SECOND;
    units.get_mut(&[7; 32]).unwrap().insert(4, second);
    log.rows.insert(6, event(4, 0, SECOND, SECOND));
    assert_eq!(
        decode(&mut log, &units).unwrap_err(),
        Error::InvalidMarker(2)
    );
}

#[test]
fn queue_cancellation_binds_the_known_receiver_without_consuming_a_fifo_receipt() {
    let (mut log, units) = fixture();
    let expected = decode(&mut log, &units).unwrap();
    log.rows.splice(
        6..6,
        [
            event(2, 8, SECOND, ARGUMENT),
            Row::QueueCanceled {
                task: 0,
                frame: 1,
                marker: 2,
                queue: 1,
                layout: 99,
                status: 2,
            },
        ],
    );
    assert_eq!(decode(&mut log, &units).unwrap(), expected);
    for field in 0..5 {
        let mut changed = log.clone();
        let Row::QueueCanceled {
            marker,
            queue,
            layout,
            frame,
            status,
            ..
        } = &mut changed.rows[7]
        else {
            unreachable!()
        };
        match field {
            0 => *marker = 1,
            1 => *queue = 2,
            2 => *layout = 100,
            3 => *frame = 2,
            _ => *status = 0,
        }
        assert!(decode(&mut changed, &units).is_err());
    }
    let mut sender = fixture().0;
    sender.rows[4] = Row::QueueCanceled {
        task: 0,
        frame: 1,
        marker: 1,
        queue: 1,
        layout: 99,
        status: 2,
    };
    assert_eq!(
        decode(&mut sender, &units).unwrap_err(),
        Error::InvalidMarker(1)
    );
}

#[test]
fn queue_detachment_cannot_invent_completion_of_queued_resource_finalizers() {
    let (mut log, units) = fixture();
    log.rows.insert(
        6,
        Row::QueueDetached {
            task: 0,
            queue: 1,
            layout: 99,
            first: 0,
            end: 1,
        },
    );
    assert_eq!(
        decode(&mut log, &units).unwrap_err(),
        Error::UnsupportedQueueCleanup(1)
    );
}

fn queued_cleanup() -> (Log, Units, CleanupLayouts, LocalCalls) {
    let (mut log, units) = fixture();
    log.rows.truncate(6);
    log.rows.extend([
        Row::QueueDetached {
            task: 0,
            queue: 1,
            layout: 99,
            first: 0,
            end: 1,
        },
        Row::QueueCleanupBegin {
            task: 0,
            queue: 1,
            layout: 99,
            item: 0,
        },
        Row::QueueCleanupDrop {
            task: 0,
            queue: 1,
            item: 0,
            step: 17,
        },
        Row::QueueCleanupEnd {
            task: 0,
            queue: 1,
            item: 0,
        },
        Row::Leave { task: 0, frame: 1 },
    ]);
    let layouts = BTreeMap::from([(
        99,
        BTreeMap::from([(
            17,
            Box::new(CleanupBinding {
                path: vec![],
                source: Site {
                    definition: "fixture::Token.drop".into(),
                    file: "channel.hir".into(),
                    line: 70,
                    column: 1,
                },
            }),
        )]),
    )]);
    let calls = BTreeMap::from([([7; 32], BTreeSet::new())]);
    (log, units, layouts, calls)
}

fn cleanup_decode(
    log: &mut Log,
    units: &Units,
    layouts: &CleanupLayouts,
    calls: &LocalCalls,
) -> Result<hirsel_run_schema::ownership_trace::Trace, Error> {
    log.row_count = log.rows.len() as u64;
    decode_runtime(log, log.program, units, calls, layouts, 100)
}

#[test]
fn queued_resource_drops_only_at_its_checked_actual_descriptor_completion() {
    let (mut log, units, layouts, calls) = queued_cleanup();
    let trace = cleanup_decode(&mut log, &units, &layouts, &calls).unwrap();
    let analysis = trace.analyze(log.program, 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    assert_eq!(trace.events.len(), 3);
    let last = trace.events.last().unwrap();
    let Action::Drop { owner, .. } = &last.action else {
        panic!("missing completed drop")
    };
    assert_eq!(last.site, layouts[&99][&17].source);
    assert_eq!(
        owner.storage,
        Storage::Object {
            name: "native.channel".into(),
            id: 1,
            item: 0
        }
    );
}

#[test]
fn queued_cleanup_rejects_missing_duplicate_and_forged_scope_completions() {
    let (log, units, layouts, calls) = queued_cleanup();
    for index in 6..10 {
        let mut changed = log.clone();
        changed.rows.remove(index);
        assert!(
            cleanup_decode(&mut changed, &units, &layouts, &calls).is_err(),
            "missing {index}"
        );
        let mut changed = log.clone();
        changed.rows.insert(index, changed.rows[index]);
        assert!(
            cleanup_decode(&mut changed, &units, &layouts, &calls).is_err(),
            "duplicate {index}"
        );
    }
    let mut changed = log.clone();
    changed.rows.drain(7..10);
    assert!(
        cleanup_decode(&mut changed, &units, &layouts, &calls).is_err(),
        "a deleted balanced scope passed"
    );
    let mut changed = log.clone();
    let Row::QueueCleanupDrop { step, .. } = &mut changed.rows[8] else {
        unreachable!()
    };
    *step = 18;
    assert!(cleanup_decode(&mut changed, &units, &layouts, &calls).is_err());
    let mut changed = log.clone();
    let Row::QueueCleanupBegin { layout, .. } = &mut changed.rows[7] else {
        unreachable!()
    };
    *layout = 100;
    assert!(cleanup_decode(&mut changed, &units, &layouts, &calls).is_err());
    let mut changed = log.clone();
    assert!(
        cleanup_decode(&mut changed, &units, &CleanupLayouts::new(), &calls).is_err(),
        "raw detachment acquired drop authority"
    );
}

#[test]
fn queued_equal_resource_fields_keep_distinct_credit_and_descriptor_drop_order() {
    use hirsel_run_schema::ownership_trace::Projection;
    let (mut log, mut units, mut layouts, calls) = queued_cleanup();
    let markers = units.get_mut(&[7; 32]).unwrap();
    let first = markers.get_mut(&0).unwrap();
    let field = |ordinal| {
        Some(LocationPath {
            root: FIRST,
            fields: vec![Projection::Field { field: ordinal }],
        })
    };
    first.token_path = field(0);
    first.owner_path = field(0);
    let mut second = first.clone();
    second.token_path = field(1);
    second.owner_path = field(1);
    markers.insert(4, second);
    log.rows.insert(3, event(4, 0, FIRST, FIRST));
    layouts.get_mut(&99).unwrap().get_mut(&17).unwrap().path = vec![Projection::Field { field: 1 }];
    let mut earlier = layouts[&99][&17].clone();
    earlier.path = vec![Projection::Field { field: 0 }];
    layouts.get_mut(&99).unwrap().insert(19, earlier);
    log.rows.insert(
        10,
        Row::QueueCleanupDrop {
            task: 0,
            queue: 1,
            item: 0,
            step: 19,
        },
    );
    let trace = cleanup_decode(&mut log, &units, &layouts, &calls).unwrap();
    let created = trace
        .events
        .iter()
        .filter_map(|event| match &event.action {
            Action::Create { token, .. } => Some(token.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let dropped = trace
        .events
        .iter()
        .filter_map(|event| match &event.action {
            Action::Drop { token, .. } => Some(token.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(dropped, [created[1].clone(), created[0].clone()]);
    let analysis = trace.analyze(log.program, 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    let mut reversed = log.clone();
    reversed.rows.swap(9, 10);
    assert!(
        cleanup_decode(&mut reversed, &units, &layouts, &calls).is_err(),
        "reordered valid steps passed"
    );
}

#[test]
fn untracked_image_cleanup_accepts_no_resource_destruction_authority() {
    let (mut log, _, _, calls) = queued_cleanup();
    log.rows = vec![
        Row::Enter {
            task: 0,
            frame: 1,
            unit: [7; 32],
        },
        Row::QueueCreated {
            task: 0,
            queue: 1,
            layout: 99,
        },
        Row::QueueSent {
            task: 0,
            queue: 1,
            layout: 99,
            item: 0,
            status: 0,
            pending: false,
        },
        Row::QueueDetached {
            task: 0,
            queue: 1,
            layout: 99,
            first: 0,
            end: 1,
        },
        Row::QueueCleanupBegin {
            task: 0,
            queue: 1,
            layout: 99,
            item: 0,
        },
        Row::QueueCleanupEnd {
            task: 0,
            queue: 1,
            item: 0,
        },
        Row::Leave { task: 0, frame: 1 },
    ];
    let units = BTreeMap::from([([7; 32], BTreeMap::new())]);
    let layouts = CleanupLayouts::new();
    let trace = cleanup_decode(&mut log, &units, &layouts, &calls).unwrap();
    assert!(trace.events.is_empty());
    log.rows.insert(
        5,
        Row::QueueCleanupDrop {
            task: 0,
            queue: 1,
            item: 0,
            step: 17,
        },
    );
    assert!(cleanup_decode(&mut log, &units, &layouts, &calls).is_err());
}
