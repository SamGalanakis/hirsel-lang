use hirsel_run_schema::Site;
use hirsel_run_schema::ownership_trace::{Action, native::*};
use std::collections::BTreeMap;

fn fixture() -> (Log, Units) {
    let mut markers = BTreeMap::new();
    let mut rows = vec![Row::Enter {
        task: 0,
        frame: 1,
        unit: [7; 32],
    }];
    let slot = 1u64 << 32;
    for (marker, action, token, owner) in [
        (0, 0, 0, 0),
        (1, 1, 0, slot),
        (2, 2, slot, slot),
        (3, 0, 1, 1),
        (4, 1, 1, slot),
        (5, 2, slot, slot),
    ] {
        let metadata = Metadata {
            action,
            token,
            owner,
            site: 42,
            layout: 7,
            source: Site {
                definition: "fixture::Token".into(),
                file: "leaf.hir".into(),
                line: u64::from(marker) + 1,
                column: 1,
            },
            value_type: "Token".into(),
        };
        rows.push(Row::Event {
            task: 0,
            frame: 1,
            marker,
            action,
            token,
            owner,
            site: 42,
            layout: 7,
        });
        markers.insert(marker, metadata);
    }
    rows.push(Row::Leave { task: 0, frame: 1 });
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

#[test]
fn native_ownership_reused_storage_preserves_distinct_equal_value_lifetimes() {
    let (log, units) = fixture();
    let trace = decode_leaf(&log, [49; 32], &units, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    let creates: Vec<_> = trace
        .events
        .iter()
        .filter_map(|e| match &e.action {
            Action::Create { token, .. } => Some(token),
            _ => None,
        })
        .collect();
    assert_ne!(creates[0], creates[1]);
}

#[test]
fn native_ownership_marker_operands_and_source_must_match_independent_metadata() {
    let (log, units) = fixture();
    for field in 0..5 {
        let mut changed = log.clone();
        let Row::Event {
            action,
            token,
            owner,
            site,
            layout,
            ..
        } = &mut changed.rows[2]
        else {
            unreachable!()
        };
        match field {
            0 => *action = 0,
            1 => *token = 99,
            2 => *owner = 99,
            3 => *site = 99,
            _ => *layout = 99,
        }
        assert_eq!(
            decode_leaf(&changed, [49; 32], &units, 100).unwrap_err(),
            Error::InvalidMarker(1)
        );
    }
}

#[test]
fn native_ownership_double_drop_reports_the_exact_actual_create_move_drop_chain() {
    let (mut log, units) = fixture();
    log.rows.insert(4, log.rows[3]);
    log.row_count += 1;
    let Error::Consistency(error) = decode_leaf(&log, [49; 32], &units, 100).unwrap_err() else {
        panic!("expected a lineage error")
    };
    assert_eq!(error.history.len(), 3);
    assert!(matches!(error.history[0].action, Action::Create { .. }));
    assert!(matches!(error.history[1].action, Action::Move { .. }));
    assert!(matches!(error.history[2].action, Action::Drop { .. }));
    assert!(error.to_string().contains("fixture::Token:2:1"));
}

#[test]
fn native_ownership_missing_drop_is_a_live_token_and_cannot_be_hidden_by_retirement() {
    let (mut log, units) = fixture();
    log.rows.remove(6);
    log.row_count -= 1;
    let trace = decode_leaf(&log, [49; 32], &units, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 1, 1)
    );
    assert_eq!(analysis.live[0].events.len(), 2);
    assert!(matches!(
        analysis.live[0].events[1].action,
        Action::Move { .. }
    ));
}

#[test]
fn native_ownership_incomplete_suffix_wrong_count_and_buffer_limit_fail_closed() {
    let (log, units) = fixture();
    let mut changed = log.clone();
    changed.complete = false;
    assert_eq!(
        decode_leaf(&changed, [49; 32], &units, 100).unwrap_err(),
        Error::Incomplete
    );
    changed = log.clone();
    changed.row_count += 1;
    assert_eq!(
        decode_leaf(&changed, [49; 32], &units, 100).unwrap_err(),
        Error::Incomplete
    );
    changed = log.clone();
    changed.rows.pop();
    changed.row_count -= 1;
    assert_eq!(
        decode_leaf(&changed, [49; 32], &units, 100).unwrap_err(),
        Error::Incomplete
    );
    assert_eq!(
        decode_leaf(&log, [49; 32], &units, 7).unwrap_err(),
        Error::Incomplete
    );
}

#[test]
fn native_ownership_unknown_units_and_reused_frames_cannot_borrow_valid_metadata() {
    let (log, units) = fixture();
    let mut changed = log.clone();
    changed.rows[0] = Row::Enter {
        task: 0,
        frame: 1,
        unit: [8; 32],
    };
    assert_eq!(
        decode_leaf(&changed, [49; 32], &units, 100).unwrap_err(),
        Error::UnknownUnit([8; 32])
    );
    changed = log.clone();
    changed.rows.push(log.rows[0]);
    changed.row_count += 1;
    assert_eq!(
        decode_leaf(&changed, [49; 32], &units, 100).unwrap_err(),
        Error::InvalidFrame(1)
    );
    changed = log.clone();
    changed.rows.push(log.rows[1]);
    changed.row_count += 1;
    assert_eq!(
        decode_leaf(&changed, [49; 32], &units, 100).unwrap_err(),
        Error::InvalidFrame(1)
    );
}

#[test]
fn native_ownership_unsupported_tasks_and_other_programs_never_pass() {
    let (mut log, units) = fixture();
    assert_eq!(
        decode_leaf(&log, [50; 32], &units, 100).unwrap_err(),
        Error::Binding
    );
    log.rows[0] = Row::Enter {
        task: 4,
        frame: 1,
        unit: [7; 32],
    };
    assert_eq!(
        decode_leaf(&log, [49; 32], &units, 100).unwrap_err(),
        Error::UnsupportedTask(4)
    );
}

#[test]
fn native_ownership_wire_roundtrip_preserves_binding_and_rejects_unknown_claims() {
    let (log, units) = fixture();
    let encoded = serde_json::to_vec(&log).unwrap();
    let decoded: Log = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(log, decoded);
    decode_leaf(&decoded, [49; 32], &units, 100).unwrap();
    let mut claim: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    claim["semantic_valid"] = true.into();
    assert!(serde_json::from_value::<Log>(claim).is_err());
}

#[test]
fn native_ownership_empty_nested_units_preserve_parent_lifetimes_and_stack_checks() {
    let (original, mut units) = fixture();
    let expected = decode_leaf(&original, [49; 32], &units, 100).unwrap();
    units.insert([8; 32], BTreeMap::new());
    let mut nested = original.clone();
    nested.rows.splice(
        2..2,
        [
            Row::Enter {
                task: 0,
                frame: 2,
                unit: [8; 32],
            },
            Row::Leave { task: 0, frame: 2 },
        ],
    );
    nested.row_count += 2;
    assert_eq!(
        decode_leaf(&nested, [49; 32], &units, 100).unwrap(),
        expected
    );
    let mut tracked = units.clone();
    tracked.insert([8; 32], units[&[7; 32]].clone());
    assert_eq!(
        decode_leaf(&nested, [49; 32], &tracked, 100).unwrap_err(),
        Error::InvalidFrame(2)
    );
    let mut unknown = units.clone();
    unknown.remove(&[8; 32]);
    assert_eq!(
        decode_leaf(&nested, [49; 32], &unknown, 100).unwrap_err(),
        Error::UnknownUnit([8; 32])
    );
    let mut parent_event = nested.clone();
    parent_event.rows.insert(3, original.rows[2]);
    parent_event.row_count += 1;
    assert_eq!(
        decode_leaf(&parent_event, [49; 32], &units, 100).unwrap_err(),
        Error::InvalidFrame(1)
    );
    let mut wrong_leave = nested.clone();
    wrong_leave.rows[3] = Row::Leave { task: 0, frame: 1 };
    assert_eq!(
        decode_leaf(&wrong_leave, [49; 32], &units, 100).unwrap_err(),
        Error::InvalidFrame(1)
    );
    let mut reused = nested.clone();
    reused.rows.splice(
        4..4,
        [
            Row::Enter {
                task: 0,
                frame: 2,
                unit: [8; 32],
            },
            Row::Leave { task: 0, frame: 2 },
        ],
    );
    reused.row_count += 2;
    assert_eq!(
        decode_leaf(&reused, [49; 32], &units, 100).unwrap_err(),
        Error::InvalidFrame(2)
    );
}

fn nested_local_calls() -> (Log, Units, LocalCalls) {
    let (mut log, mut units) = fixture();
    let mut child = log.rows.clone();
    for row in &mut child {
        match row {
            Row::Enter { frame, unit, .. } => {
                *frame = 2;
                *unit = [8; 32];
            }
            Row::Event { frame, .. } | Row::Leave { frame, .. } => *frame = 2,
        }
    }
    log.rows.splice(2..2, child);
    log.row_count = log.rows.len() as u64;
    units.insert([8; 32], units[&[7; 32]].clone());
    let calls = BTreeMap::from([([7; 32], std::collections::BTreeSet::from([[8; 32]]))]);
    (log, units, calls)
}

#[test]
fn native_ownership_local_calls_keep_equal_storage_in_distinct_dynamic_frames() {
    let (log, units, calls) = nested_local_calls();
    let trace = decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (4, 4, 0)
    );
    let creates = trace
        .events
        .iter()
        .filter_map(|event| match &event.action {
            Action::Create { token, .. } => Some(token.clone()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(creates.len(), 4);
    assert_eq!(
        decode_leaf(&log, [49; 32], &units, 100).unwrap_err(),
        Error::InvalidFrame(2)
    );
}

#[test]
fn native_ownership_local_calls_refuse_wrong_parent_missing_target_and_parent_events() {
    let (log, units, calls) = nested_local_calls();
    assert_eq!(
        decode_local_calls(&log, [49; 32], &units, &BTreeMap::new(), 100).unwrap_err(),
        Error::InvalidFrame(2)
    );
    let wrong = BTreeMap::from([([8; 32], std::collections::BTreeSet::from([[7; 32]]))]);
    assert_eq!(
        decode_local_calls(&log, [49; 32], &units, &wrong, 100).unwrap_err(),
        Error::InvalidFrame(2)
    );
    let unknown = BTreeMap::from([([7; 32], std::collections::BTreeSet::from([[9; 32]]))]);
    assert_eq!(
        decode_local_calls(&log, [49; 32], &units, &unknown, 100).unwrap_err(),
        Error::Binding
    );
    let mut changed = log.clone();
    changed.rows.insert(3, log.rows[1]);
    changed.row_count += 1;
    assert_eq!(
        decode_local_calls(&changed, [49; 32], &units, &calls, 100).unwrap_err(),
        Error::InvalidFrame(1)
    );
}

fn owned_call_rows() -> (Log, Units, LocalCalls) {
    let parent = [7; 32];
    let child = [8; 32];
    let mut rows = vec![Row::Enter {
        task: 0,
        frame: 1,
        unit: parent,
    }];
    let mut units = Units::from([(parent, BTreeMap::new()), (child, BTreeMap::new())]);
    let mut event = |unit, frame, marker, action, token, owner| {
        let metadata = Metadata {
            action,
            token,
            owner,
            site: 42,
            layout: 7,
            source: Site {
                definition: "identity".into(),
                file: "call.hir".into(),
                line: 1,
                column: 1,
            },
            value_type: "Token".into(),
        };
        units.get_mut(&unit).unwrap().insert(marker, metadata);
        Row::Event {
            task: 0,
            frame,
            marker,
            action,
            token,
            owner,
            site: 42,
            layout: 7,
        }
    };
    // One logical token leaves parent SSA, enters the owned callee parameter,
    // returns in the same custody and is finally dropped by the caller.
    rows.push(event(parent, 1, 0, 0, 0, 0));
    rows.push(event(parent, 1, 1, 1, 0, 2u64 << 32));
    rows.push(Row::Enter {
        task: 0,
        frame: 2,
        unit: child,
    });
    rows.push(event(child, 2, 0, 1, 3u64 << 32, 0));
    rows.push(event(child, 2, 1, 1, 0, 4u64 << 32));
    rows.push(Row::Leave { task: 0, frame: 2 });
    rows.push(event(parent, 1, 2, 1, 5u64 << 32, 1));
    rows.push(event(parent, 1, 3, 2, 1, 1));
    rows.push(Row::Leave { task: 0, frame: 1 });
    (
        Log {
            version: VERSION,
            program: [49; 32],
            row_count: rows.len() as u64,
            rows,
            complete: true,
        },
        units,
        LocalCalls::from([(parent, [child].into())]),
    )
}
#[test]
fn native_ownership_direct_call_transports_one_token_through_parameter_and_return() {
    let (log, units, calls) = owned_call_rows();
    let trace = decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap();
    let result = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (result.created, result.dropped, result.live.len()),
        (1, 1, 0)
    );
    assert_eq!(trace.events.len(), 6);
    assert!(trace.events.iter().all(|event| match &event.action {
        Action::Create { token, .. } | Action::Move { token, .. } | Action::Drop { token, .. } =>
            token.sequence == 0,
    }));
}
#[test]
fn native_ownership_direct_call_refuses_missing_arguments_returns_and_root_transport() {
    let (log, units, calls) = owned_call_rows();
    for omitted in [2, 4, 5, 7] {
        let mut broken = log.clone();
        broken.rows.remove(omitted);
        broken.row_count -= 1;
        assert!(
            decode_local_calls(&broken, [49; 32], &units, &calls, 100).is_err(),
            "missing row {omitted}"
        );
    }
    assert!(decode_leaf(&log, [49; 32], &units, 100).is_err());
    let mut wrong = log.clone();
    if let Row::Event { token, .. } = &mut wrong.rows[4] {
        *token = 3u64 << 32 | 1;
    }
    assert!(decode_local_calls(&wrong, [49; 32], &units, &calls, 100).is_err());
}
