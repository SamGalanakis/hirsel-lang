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
            Metadata {
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
            },
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
