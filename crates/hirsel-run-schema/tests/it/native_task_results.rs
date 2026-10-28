use hirsel_run_schema::{
    Site,
    ownership_trace::{Action, native::*},
};
use std::collections::{BTreeMap, BTreeSet};
const SLOT: u64 = 1 << 32;
const CHILD: u64 = (1 << 32) | 1;
fn metadata(action: u32, token: u64, owner: u64) -> Box<Metadata> {
    Box::new(Metadata {
        action,
        channel: None,
        task_result: match action {
            9 => Some(TaskResultBinding {
                direction: TaskResultDirection::Hold,
                descriptor: 99,
            }),
            10 => Some(TaskResultBinding {
                direction: TaskResultDirection::Join,
                descriptor: 99,
            }),
            _ => None,
        },
        optional_carrier: false,
        empty_environment: false,
        callee: None,
        cleanup: None,
        cleanup_paths: BTreeMap::new(),
        additional_drops: vec![],
        token_path: None,
        owner_path: None,
        token,
        owner,
        site: 42,
        layout: 7,
        value_type: "Token".into(),
        source: Site {
            definition: "fixture::Token".into(),
            file: "result.hir".into(),
            line: 1,
            column: 1,
        },
    })
}
fn event(task: u64, frame: u64, marker: u32, action: u32, token: u64, owner: u64) -> Row {
    Row::Event {
        task,
        frame,
        marker,
        action,
        token,
        owner,
        site: 42,
        layout: 7,
    }
}
fn log(rows: Vec<Row>) -> Log {
    Log {
        version: VERSION,
        program: [49; 32],
        row_count: rows.len() as u64,
        rows,
        complete: true,
    }
}
fn fixture(count: u32) -> (Log, Units) {
    let mut parent = BTreeMap::new();
    let mut rows = vec![Row::Enter {
        task: 0,
        frame: 1,
        unit: [7; 32],
    }];
    for index in 0..count {
        let child = (u64::from(index) + 1) << 32 | 1;
        let frame = u64::from(index) + 2;
        let slot = SLOT | u64::from(index);
        parent.insert(index * 2, metadata(10, slot, (2 << 32) | 2));
        parent.insert(index * 2 + 1, metadata(2, slot, slot));
        rows.extend([
            Row::Task {
                task: child,
                depth: 1,
            },
            Row::TaskPart {
                task: child,
                index: 0,
                ordinal: index,
            },
            Row::Enter {
                task: child,
                frame,
                unit: [8; 32],
            },
            event(child, frame, 0, 0, SLOT, SLOT),
            event(child, frame, 1, 9, SLOT, 2 << 32),
            Row::TaskResultHeld {
                task: child,
                frame,
                marker: 1,
                layout: 99,
            },
            event(0, 1, index * 2, 10, slot, (2 << 32) | 2),
            Row::TaskResultJoin {
                task: 0,
                frame: 1,
                marker: index * 2,
                child,
            },
            Row::TaskResultMoved {
                task: 0,
                frame: 1,
                marker: index * 2,
                child,
                source_frame: frame,
                source_marker: 1,
                layout: 99,
            },
            Row::Leave { task: child, frame },
            Row::TaskEnd { task: child },
            event(0, 1, index * 2 + 1, 2, slot, slot),
        ]);
    }
    rows.push(Row::Leave { task: 0, frame: 1 });
    (
        log(rows),
        BTreeMap::from([
            ([7; 32], parent),
            (
                [8; 32],
                BTreeMap::from([
                    (0, metadata(0, SLOT, SLOT)),
                    (1, metadata(9, SLOT, 2 << 32)),
                ]),
            ),
        ]),
    )
}
fn decode(log: &Log, units: &Units) -> Result<hirsel_run_schema::ownership_trace::Trace, Error> {
    decode_local_calls(log, [49; 32], units, &BTreeMap::new(), 100)
}
#[test]
fn actual_result_transfers_keep_equal_tokens_distinct_across_reused_task_slots() {
    let (log, units) = fixture(2);
    let trace = decode(&log, &units).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    let moved = trace
        .events
        .iter()
        .filter_map(|event| match &event.action {
            Action::Move { token, from, owner } => {
                assert_eq!(from.task.as_ref(), Some(&token.task));
                assert_eq!(owner.task.as_ref(), Some(&Default::default()));
                Some(token.clone())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(moved.len(), 2);
    assert!(moved.iter().all(|token| token.sequence == 0));
}
#[test]
fn result_transfer_rejects_missing_held_receipts_wrong_child_layout_frame_and_duplicates() {
    for corruption in 0..6 {
        let (mut log, units) = fixture(1);
        match corruption {
            0 => {
                log.rows.remove(6);
            }
            1 => {
                if let Row::TaskResultMoved { child, .. } = &mut log.rows[9] {
                    *child = CHILD + 1;
                }
            }
            2 => {
                if let Row::TaskResultMoved { layout, .. } = &mut log.rows[9] {
                    *layout = 100;
                }
            }
            3 => {
                if let Row::TaskResultMoved { source_frame, .. } = &mut log.rows[9] {
                    *source_frame = 3;
                }
            }
            4 => {
                log.rows.insert(10, log.rows[9]);
            }
            5 => {
                let moved = log.rows.remove(9);
                log.rows.insert(12, moved);
            }
            _ => unreachable!(),
        }
        log.row_count = log.rows.len() as u64;
        assert!(decode(&log, &units).is_err(), "corruption {corruption}");
    }
}
#[test]
fn result_discard_preserves_child_credit_for_its_actual_cleanup() {
    let (mut log, mut units) = fixture(1);
    log.rows.splice(
        7..10,
        [
            Row::TaskResultDiscarded {
                task: CHILD,
                frame: 2,
                marker: 1,
            },
            event(CHILD, 2, 2, 2, SLOT, SLOT),
        ],
    );
    log.rows
        .retain(|row| !matches!(row, Row::Event { task: 0, .. }));
    log.row_count = log.rows.len() as u64;
    units
        .get_mut(&[8; 32])
        .unwrap()
        .insert(2, metadata(2, SLOT, SLOT));
    let trace = decode(&log, &units).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    assert!(
        !trace
            .events
            .iter()
            .any(|event| matches!(event.action, Action::Move { .. }))
    );
}
#[test]
fn failed_child_result_requires_actual_failed_join_and_keeps_the_destination_empty() {
    let (_, units) = fixture(1);
    let mut log = log(vec![
        Row::Enter {
            task: 0,
            frame: 1,
            unit: [7; 32],
        },
        Row::Task {
            task: CHILD,
            depth: 1,
        },
        Row::TaskPart {
            task: CHILD,
            index: 0,
            ordinal: 0,
        },
        Row::Enter {
            task: CHILD,
            frame: 2,
            unit: [8; 32],
        },
        Row::Leave {
            task: CHILD,
            frame: 2,
        },
        Row::TaskEnd { task: CHILD },
        event(0, 1, 0, 10, SLOT, (2 << 32) | 2),
        Row::TaskResultJoin {
            task: 0,
            frame: 1,
            marker: 0,
            child: CHILD,
        },
        Row::TaskResultUnavailable {
            task: 0,
            frame: 1,
            marker: 0,
            child: CHILD,
            status: 1,
        },
        Row::Leave { task: 0, frame: 1 },
    ]);
    assert!(decode(&log, &units).unwrap().events.is_empty());
    if let Row::TaskResultUnavailable { status, .. } = &mut log.rows[8] {
        *status = 0;
    }
    assert!(decode(&log, &units).is_err());
    log.rows.remove(8);
    log.row_count = log.rows.len() as u64;
    assert!(decode(&log, &units).is_err());
}
#[test]
fn task_result_metadata_cannot_swap_roles_or_authorize_an_unbound_formal() {
    for corruption in 0..3 {
        let (log, mut units) = fixture(1);
        let held = units.get_mut(&[8; 32]).unwrap().get_mut(&1).unwrap();
        match corruption {
            0 => held.task_result = None,
            1 => held.task_result.as_mut().unwrap().direction = TaskResultDirection::Join,
            2 => held.owner |= 1,
            _ => unreachable!(),
        }
        assert!(decode(&log, &units).is_err(), "corruption {corruption}");
    }
}
