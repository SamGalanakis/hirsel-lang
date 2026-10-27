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
            empty_environment: false,
            action,
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
            Row::Event { frame, .. }
            | Row::Alias { frame, .. }
            | Row::Leave { frame, .. }
            | Row::CleanupBegin { frame, .. }
            | Row::CleanupDrop { frame, .. }
            | Row::CleanupEnd { frame, .. } => *frame = 2,
            Row::Task { .. }
            | Row::TaskPart { .. }
            | Row::Operation { .. }
            | Row::Spawn { .. }
            | Row::TaskEnd { .. } => unreachable!(),
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
            empty_environment: false,
            action,
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

#[test]
fn native_alias_binds_capture_custody_without_an_ownership_event() {
    let caller = [7; 32];
    let callee = [8; 32];
    let field = 8u64 << 32 | 1; // capture 0 of callable SSA value 1
    let slot = 1u64 << 32;
    let capture = 6u64 << 32;
    let mut units = Units::new();
    let mut rows = vec![Row::Enter {
        task: 0,
        frame: 1,
        unit: caller,
    }];
    let mut emit = |unit, frame, marker, action, token, owner, site| {
        units.entry(unit).or_default().insert(
            marker,
            Metadata {
                empty_environment: false,
                action,
                callee: None,
                cleanup: None,
                cleanup_paths: BTreeMap::new(),
                additional_drops: Vec::new(),
                token_path: None,
                owner_path: None,
                token,
                owner,
                site,
                layout: 7,
                source: Site {
                    definition: "fixture::Token".into(),
                    file: "capture.hir".into(),
                    line: site,
                    column: 1,
                },
                value_type: "Token".into(),
            },
        );
        Row::Event {
            task: 0,
            frame,
            marker,
            action,
            token,
            owner,
            site,
            layout: 7,
        }
    };
    rows.push(emit(caller, 1, 0, 0, 0, 0, 40));
    rows.push(emit(caller, 1, 1, 1, 0, field, 41));
    rows.push(emit(caller, 1, 2, 1, 1, slot, 42));
    let alias = Row::Alias {
        task: 0,
        frame: 1,
        marker: 3,
        view: 2,
        owner: slot,
        site: 43,
        layout: 7,
    };
    rows.push(alias);
    rows.push(Row::Enter {
        task: 0,
        frame: 2,
        unit: callee,
    });
    rows.push(emit(callee, 2, 0, 1, capture, 0, 44));
    rows.push(emit(callee, 2, 1, 1, 0, 4u64 << 32, 45));
    rows.push(Row::Leave { task: 0, frame: 2 });
    rows.push(emit(caller, 1, 4, 1, 5u64 << 32, 3, 46));
    rows.push(emit(caller, 1, 5, 2, 3, 3, 47));
    rows.push(Row::Leave { task: 0, frame: 1 });
    let mut alias_metadata = units[&caller][&2].clone();
    alias_metadata.action = 3;
    alias_metadata.callee = Some(callee);
    alias_metadata.token = 2;
    alias_metadata.owner = slot;
    alias_metadata.site = 43;
    units.get_mut(&caller).unwrap().insert(3, alias_metadata);
    let log = Log {
        version: VERSION,
        program: [49; 32],
        row_count: rows.len() as u64,
        rows,
        complete: true,
    };
    let calls = LocalCalls::from([(caller, [callee].into()), (callee, Default::default())]);
    let trace = decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    assert_eq!(trace.events.len(), 7); // no Alias ownership event
    let token = trace.events[0].action.token();
    assert!(
        trace
            .events
            .iter()
            .all(|event| event.action.token() == token)
    );
    assert!(decode_leaf(&log, [49; 32], &units, 100).is_err());
    let mut missing = log.clone();
    missing.rows.remove(4);
    missing.row_count -= 1;
    assert!(matches!(
        decode_local_calls(&missing, [49; 32], &units, &calls, 100),
        Err(Error::InvalidLocation(_))
    ));
    let mut forged = log.clone();
    let Row::Alias { owner, .. } = &mut forged.rows[4] else {
        unreachable!()
    };
    *owner += 1;
    assert_eq!(
        decode_local_calls(&forged, [49; 32], &units, &calls, 100).unwrap_err(),
        Error::InvalidMarker(3)
    );
    let mut swapped = log.clone();
    let Row::Enter { unit, .. } = &mut swapped.rows[5] else {
        unreachable!()
    };
    *unit = [9; 32];
    let mut swapped_units = units.clone();
    swapped_units.insert([9; 32], units[&callee].clone());
    let mut swapped_calls = calls.clone();
    swapped_calls.get_mut(&caller).unwrap().insert([9; 32]);
    assert_eq!(
        decode_local_calls(&swapped, [49; 32], &swapped_units, &swapped_calls, 100).unwrap_err(),
        Error::InvalidFrame(2)
    );
    let mut false_metadata = units.clone();
    false_metadata
        .get_mut(&caller)
        .unwrap()
        .get_mut(&3)
        .unwrap()
        .owner += 1;
    assert!(matches!(
        decode_local_calls(&forged, [49; 32], &false_metadata, &calls, 100),
        Err(Error::Consistency(_))
    ));
}

#[test]
fn native_empty_carrier_retirement_never_invents_resource_drops() {
    let (mut log, mut units) = fixture();
    let owner = 1u64 << 32;
    let mut marker = units[&[7; 32]][&2].clone();
    marker.action = 4;
    marker.token = owner;
    marker.owner = owner;
    units.get_mut(&[7; 32]).unwrap().insert(20, marker.clone());
    let retirement = Row::Event {
        task: 0,
        frame: 1,
        marker: 20,
        action: 4,
        token: owner,
        owner,
        site: marker.site,
        layout: marker.layout,
    };
    log.rows.insert(4, retirement); // immediately after actual Resource Drop
    log.row_count += 1;
    let trace = decode_leaf(&log, [49; 32], &units, 100).unwrap();
    assert_eq!(trace.events.len(), 6);
    assert_eq!(trace.analyze([49; 32], 100).unwrap().dropped, 2);
    log.rows.remove(4);
    log.rows.insert(3, retirement); // still-owned resource before actual Drop
    assert_eq!(
        decode_leaf(&log, [49; 32], &units, 100).unwrap_err(),
        Error::RetainedEnvironment(20)
    );
}

#[test]
fn native_capture_cleanup_requires_actual_typed_retirements_before_scope_end() {
    let (mut log, mut units) = fixture();
    let calls = LocalCalls::from([([7; 32], Default::default())]);
    // Original token moves into capture 0 of callable SSA 1, then to slot 0.
    let field = 8u64 << 32 | 1;
    let slot = 1u64 << 32;
    let rows = units.get_mut(&[7; 32]).unwrap();
    rows.get_mut(&1).unwrap().owner = field;
    let mut move_carrier = rows[&1].clone();
    move_carrier.token = 1;
    move_carrier.owner = slot;
    rows.insert(30, move_carrier);
    let mut drop_carrier = rows[&2].clone();
    drop_carrier.action = 4;
    drop_carrier.cleanup = Some(vec![(0, 7)]);
    drop_carrier.cleanup_paths = BTreeMap::from([(0, vec![0])]);
    rows.insert(31, drop_carrier);
    if let Row::Event { owner, .. } = &mut log.rows[2] {
        *owner = field;
    }
    log.rows.splice(
        3..4,
        [
            Row::Event {
                task: 0,
                frame: 1,
                marker: 30,
                action: 1,
                token: 1,
                owner: slot,
                site: 42,
                layout: 7,
            },
            Row::CleanupBegin {
                task: 0,
                frame: 1,
                marker: 31,
            },
            Row::CleanupDrop {
                task: 0,
                frame: 1,
                marker: 31,
                field: 0,
                layout: 7,
            },
            Row::CleanupEnd {
                task: 0,
                frame: 1,
                marker: 31,
            },
        ],
    );
    log.row_count = log.rows.len() as u64;
    let trace = decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    let mut missing = log.clone();
    missing.rows.remove(5);
    missing.row_count -= 1;
    assert_eq!(
        decode_local_calls(&missing, [49; 32], &units, &calls, 100).unwrap_err(),
        Error::RetainedEnvironment(31)
    );
    for (field, layout) in [(1, 7), (0, 8)] {
        let mut forged = log.clone();
        forged.rows[5] = Row::CleanupDrop {
            task: 0,
            frame: 1,
            marker: 31,
            field,
            layout,
        };
        assert_eq!(
            decode_local_calls(&forged, [49; 32], &units, &calls, 100).unwrap_err(),
            Error::InvalidMarker(31)
        );
    }
    let mut duplicate = log.clone();
    duplicate.rows.insert(6, duplicate.rows[5]);
    duplicate.row_count += 1;
    assert_eq!(
        decode_local_calls(&duplicate, [49; 32], &units, &calls, 100).unwrap_err(),
        Error::InvalidMarker(31)
    );
    let mut early = log.clone();
    early.rows.swap(5, 6);
    assert_eq!(
        decode_local_calls(&early, [49; 32], &units, &calls, 100).unwrap_err(),
        Error::RetainedEnvironment(31)
    );
    let mut unfinished = log.clone();
    unfinished.rows.remove(6);
    unfinished.row_count -= 1;
    assert!(matches!(
        decode_local_calls(&unfinished, [49; 32], &units, &calls, 100),
        Err(Error::InvalidFrame(1))
    ));
}

#[test]
fn native_capture_cleanup_rejects_swapped_retirement_of_equal_resource_captures() {
    let mut units = Units::new();
    let unit = [7; 32];
    let slot = 1u64 << 32;
    let mut rows = vec![Row::Enter {
        task: 0,
        frame: 1,
        unit,
    }];
    for (marker, action, token, owner) in [
        (0, 0, 0, 0),
        (1, 1, 0, 8u64 << 32 | 1),
        (2, 0, 2, 2),
        (3, 1, 2, 9u64 << 32 | 1),
        (4, 1, 1, slot),
    ] {
        units.entry(unit).or_default().insert(
            marker,
            Metadata {
                empty_environment: false,
                action,
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
                    definition: "fixture::Token".into(),
                    file: "captures.hir".into(),
                    line: 1,
                    column: 1,
                },
                value_type: "Token".into(),
            },
        );
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
    }
    let mut retirement = units[&unit][&4].clone();
    retirement.action = 4;
    retirement.token = slot;
    retirement.cleanup = Some(vec![(1, 7), (0, 7)]);
    retirement.cleanup_paths = BTreeMap::from([(1, vec![1]), (0, vec![0])]);
    units.get_mut(&unit).unwrap().insert(5, retirement);
    rows.extend([
        Row::CleanupBegin {
            task: 0,
            frame: 1,
            marker: 5,
        },
        Row::CleanupDrop {
            task: 0,
            frame: 1,
            marker: 5,
            field: 1,
            layout: 7,
        },
        Row::CleanupDrop {
            task: 0,
            frame: 1,
            marker: 5,
            field: 0,
            layout: 7,
        },
        Row::CleanupEnd {
            task: 0,
            frame: 1,
            marker: 5,
        },
        Row::Leave { task: 0, frame: 1 },
    ]);
    let mut log = Log {
        version: VERSION,
        program: [49; 32],
        row_count: rows.len() as u64,
        rows,
        complete: true,
    };
    let calls = LocalCalls::from([(unit, Default::default())]);
    let trace = decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    log.rows.swap(7, 8);
    assert_eq!(
        decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap_err(),
        Error::InvalidMarker(5)
    );
}

#[test]
fn native_projected_fields_require_independent_complete_static_paths() {
    let (mut log, mut units) = fixture();
    let slot = 1u64 << 32;
    let projected = (7u64 << 32) | 11;
    let path = LocationPath {
        root: slot,
        fields: vec![0, 1],
    };
    for row in &mut log.rows {
        if let Row::Event { token, owner, .. } = row {
            if *token == slot {
                *token = projected;
            }
            if *owner == slot {
                *owner = projected;
            }
        }
    }
    for marker in units.get_mut(&[7; 32]).unwrap().values_mut() {
        if marker.token == slot {
            marker.token = projected;
            marker.token_path = Some(path.clone());
        }
        if marker.owner == slot {
            marker.owner = projected;
            marker.owner_path = Some(path.clone());
        }
    }
    let calls = BTreeMap::from([([7; 32], Default::default())]);
    let trace = decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    assert!(trace.events.iter().any(|event| event.action.owner().path
        == vec![
            hirsel_run_schema::ownership_trace::Projection::Field { field: 0 },
            hirsel_run_schema::ownership_trace::Projection::Field { field: 1 },
        ]));
    for changed in 0..4 {
        let mut forged = units.clone();
        let drop = forged.get_mut(&[7; 32]).unwrap().get_mut(&2).unwrap();
        match changed {
            0 => drop.token_path = None,
            1 => drop.token_path.as_mut().unwrap().fields.clear(),
            2 => drop.token_path.as_mut().unwrap().root = projected,
            _ => drop.token_path.as_mut().unwrap().fields[1] = 0,
        }
        assert!(decode_local_calls(&log, [49; 32], &forged, &calls, 100).is_err());
    }
    let mut stale = log;
    stale.version = VERSION - 1;
    assert_eq!(
        decode_local_calls(&stale, [49; 32], &units, &calls, 100).unwrap_err(),
        Error::Binding
    );
}

#[test]
fn native_compound_drop_members_keep_original_marker_and_exact_field_lifetimes() {
    let (mut log, mut units) = fixture();
    let first = (7u64 << 32) | 11;
    let second = (7u64 << 32) | 12;
    let paths = [
        LocationPath {
            root: 1u64 << 32,
            fields: vec![0],
        },
        LocationPath {
            root: 1u64 << 32,
            fields: vec![1],
        },
    ];
    let records = units.get_mut(&[7; 32]).unwrap();
    for (id, location, path) in [
        (1, first, &paths[0]),
        (2, first, &paths[0]),
        (4, second, &paths[1]),
        (5, second, &paths[1]),
    ] {
        let metadata = records.get_mut(&id).unwrap();
        metadata.owner = location;
        metadata.owner_path = Some(path.clone());
        if metadata.action == 2 {
            metadata.token = location;
            metadata.token_path = Some(path.clone());
        }
    }
    let child = records.remove(&5).unwrap();
    records
        .get_mut(&2)
        .unwrap()
        .additional_drops
        .push(DropMember {
            location: child.token,
            path: child.token_path,
            layout: child.layout,
            value_type: child.value_type,
        });
    // Both equal-valued resources are live in distinct fields before cleanup.
    log.rows = [0, 1, 2, 4, 5, 3, 6, 7]
        .into_iter()
        .map(|index| log.rows[index])
        .collect();
    for row in &mut log.rows {
        if let Row::Event {
            marker,
            token,
            owner,
            ..
        } = row
        {
            match *marker {
                1 => *owner = first,
                2 => {
                    *token = first;
                    *owner = first;
                }
                4 => *owner = second,
                5 => {
                    *marker = 2;
                    *token = second;
                    *owner = second;
                }
                _ => (),
            }
        }
    }
    let calls = BTreeMap::from([([7; 32], Default::default())]);
    let trace = decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    let drops = trace
        .events
        .iter()
        .filter_map(|event| match &event.action {
            Action::Drop { token, owner } => Some((token.clone(), owner.path.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_ne!(drops[0].0, drops[1].0);
    assert_eq!(
        drops[0].1,
        vec![hirsel_run_schema::ownership_trace::Projection::Field { field: 0 }]
    );
    assert_eq!(
        drops[1].1,
        vec![hirsel_run_schema::ownership_trace::Projection::Field { field: 1 }]
    );
    let mut missing = log.clone();
    missing.rows.remove(6);
    missing.row_count -= 1;
    let trace = decode_local_calls(&missing, [49; 32], &units, &calls, 100).unwrap();
    assert_eq!(trace.analyze([49; 32], 100).unwrap().live.len(), 1);
    let mut forged = log.clone();
    if let Row::Event { token, owner, .. } = &mut forged.rows[6] {
        *token = second + 1;
        *owner = second + 1;
    }
    assert!(decode_local_calls(&forged, [49; 32], &units, &calls, 100).is_err());
    let mut duplicate = log.clone();
    duplicate.rows.insert(6, duplicate.rows[5]);
    duplicate.row_count += 1;
    assert!(decode_local_calls(&duplicate, [49; 32], &units, &calls, 100).is_err());
    let mut ambiguous = units.clone();
    let metadata = ambiguous.get_mut(&[7; 32]).unwrap().get_mut(&2).unwrap();
    metadata
        .additional_drops
        .push(metadata.additional_drops[0].clone());
    assert!(decode_local_calls(&log, [49; 32], &ambiguous, &calls, 100).is_err());
}

#[test]
fn native_nested_capture_retirements_require_complete_exact_declaration_paths() {
    let (mut log, mut units) = fixture();
    let unit = [7; 32];
    let root = 1u64 << 32;
    let first = (7u64 << 32) | 11;
    let second = (7u64 << 32) | 12;
    for (id, location, path) in [(1, first, vec![0, 0]), (4, second, vec![0, 1])] {
        let marker = units.get_mut(&unit).unwrap().get_mut(&id).unwrap();
        marker.owner = location;
        marker.owner_path = Some(LocationPath { root, fields: path });
    }
    let mut cleanup = units[&unit][&2].clone();
    cleanup.action = 4;
    cleanup.token = root;
    cleanup.owner = root;
    cleanup.cleanup = Some(vec![(2, 7), (3, 7)]);
    cleanup.cleanup_paths = BTreeMap::from([(2, vec![0, 1]), (3, vec![0, 0])]);
    units.get_mut(&unit).unwrap().insert(20, cleanup);
    let mut rows = [0, 1, 2, 4, 5]
        .into_iter()
        .map(|index| log.rows[index])
        .collect::<Vec<_>>();
    for row in &mut rows {
        if let Row::Event { marker, owner, .. } = row {
            if *marker == 1 {
                *owner = first;
            }
            if *marker == 4 {
                *owner = second;
            }
        }
    }
    rows.extend([
        Row::CleanupBegin {
            task: 0,
            frame: 1,
            marker: 20,
        },
        Row::CleanupDrop {
            task: 0,
            frame: 1,
            marker: 20,
            field: 2,
            layout: 7,
        },
        Row::CleanupDrop {
            task: 0,
            frame: 1,
            marker: 20,
            field: 3,
            layout: 7,
        },
        Row::CleanupEnd {
            task: 0,
            frame: 1,
            marker: 20,
        },
        Row::Leave { task: 0, frame: 1 },
    ]);
    log.rows = rows;
    log.row_count = log.rows.len() as u64;
    let calls = LocalCalls::from([(unit, Default::default())]);
    let trace = decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (2, 2, 0)
    );
    assert_eq!(
        trace
            .events
            .iter()
            .filter_map(|e| match &e.action {
                Action::Drop { token, .. } => Some(token.sequence),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [1, 0]
    );
    let mut swapped = log.clone();
    swapped.rows.swap(6, 7);
    assert!(decode_local_calls(&swapped, [49; 32], &units, &calls, 100).is_err());
    let mut missing = log.clone();
    missing.rows.remove(6);
    missing.row_count -= 1;
    assert!(decode_local_calls(&missing, [49; 32], &units, &calls, 100).is_err());
    let mut duplicate = log.clone();
    duplicate.rows.insert(7, duplicate.rows[6]);
    duplicate.row_count += 1;
    assert!(decode_local_calls(&duplicate, [49; 32], &units, &calls, 100).is_err());
    for kind in 0..4 {
        let mut forged = units.clone();
        let metadata = forged.get_mut(&unit).unwrap().get_mut(&20).unwrap();
        match kind {
            0 => {
                metadata.cleanup_paths.remove(&2);
            }
            1 => metadata.cleanup_paths.get_mut(&2).unwrap().clear(),
            2 => {
                metadata.cleanup_paths.insert(2, vec![0, 0]);
            }
            _ => {
                metadata.cleanup_paths.insert(2, vec![0, 2]);
            }
        }
        assert!(decode_local_calls(&log, [49; 32], &forged, &calls, 100).is_err());
    }
    log.version = VERSION - 1;
    assert_eq!(
        decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap_err(),
        Error::Binding
    );
}

#[test]
fn native_ownership_empty_callable_moves_need_static_evidence_and_empty_storage() {
    let (mut empty, mut units) = fixture();
    empty.rows = vec![empty.rows[0], empty.rows[2], *empty.rows.last().unwrap()];
    empty.row_count = empty.rows.len() as u64;
    units.get_mut(&[7; 32]).unwrap().retain(|id, _| *id == 1);
    units
        .get_mut(&[7; 32])
        .unwrap()
        .get_mut(&1)
        .unwrap()
        .empty_environment = true;
    let trace = decode_leaf(&empty, [49; 32], &units, 100).unwrap();
    assert!(trace.events.is_empty());
    assert!(trace.analyze([49; 32], 100).unwrap().live.is_empty());
    let mut missing = units.clone();
    missing
        .get_mut(&[7; 32])
        .unwrap()
        .get_mut(&1)
        .unwrap()
        .empty_environment = false;
    assert!(matches!(
        decode_leaf(&empty, [49; 32], &missing, 100),
        Err(Error::Consistency(_))
    ));
    let (log, mut actual) = fixture();
    actual
        .get_mut(&[7; 32])
        .unwrap()
        .get_mut(&1)
        .unwrap()
        .empty_environment = true;
    assert_eq!(
        decode_leaf(&log, [49; 32], &actual, 100).unwrap_err(),
        Error::RetainedEnvironment(1)
    );
    let mut destination = empty.clone();
    let mut marker = actual[&[7; 32]][&0].clone();
    marker.token = 1u64 << 32;
    marker.owner = marker.token;
    actual.get_mut(&[7; 32]).unwrap().insert(0, marker.clone());
    destination.rows.insert(
        1,
        Row::Event {
            task: 0,
            frame: 1,
            marker: 0,
            action: 0,
            token: marker.token,
            owner: marker.owner,
            site: marker.site,
            layout: marker.layout,
        },
    );
    destination.row_count = destination.rows.len() as u64;
    assert_eq!(
        decode_leaf(&destination, [49; 32], &actual, 100).unwrap_err(),
        Error::RetainedEnvironment(1)
    );
}

#[test]
fn native_parallel_cycles_and_chains_preserve_equal_resource_identities() {
    use hirsel_run_schema::ownership_trace::Storage;
    for targets in [[1, 0], [1, 2]] {
        let (_, original) = fixture();
        let template = original[&[7; 32]][&0].clone();
        let temporary = |index| (u64::from(u32::MAX) << 32) | index;
        let operations = [
            (0, 0, 0),
            (0, 1, 1),
            (1, 0, temporary(0)),
            (1, 1, temporary(1)),
            (1, temporary(0), targets[0]),
            (1, temporary(1), targets[1]),
            (2, targets[1], targets[1]),
            (2, targets[0], targets[0]),
        ];
        let mut metadata = BTreeMap::new();
        let mut rows = vec![Row::Enter {
            task: 0,
            frame: 1,
            unit: [7; 32],
        }];
        for (marker, (action, token, owner)) in operations.into_iter().enumerate() {
            let marker = marker as u32;
            metadata.insert(
                marker,
                Metadata {
                    action,
                    token,
                    owner,
                    ..template.clone()
                },
            );
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
        }
        rows.push(Row::Leave { task: 0, frame: 1 });
        let units = Units::from([([7; 32], metadata)]);
        let log = Log {
            version: VERSION,
            program: [49; 32],
            row_count: rows.len() as u64,
            rows,
            complete: true,
        };
        let trace = decode_leaf(&log, [49; 32], &units, 100).unwrap();
        let analysis = trace.analyze([49; 32], 100).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (2, 2, 0)
        );
        let drops: Vec<_> = trace
            .events
            .iter()
            .filter_map(|event| match &event.action {
                Action::Drop { token, owner } => Some((token.sequence, owner.storage.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            drops,
            vec![
                (
                    1,
                    Storage::Value {
                        frame: 1,
                        value: targets[1] as u32
                    }
                ),
                (
                    0,
                    Storage::Value {
                        frame: 1,
                        value: targets[0] as u32
                    }
                ),
            ]
        );
        let mut incomplete = log.clone();
        incomplete.rows.remove(3);
        incomplete.row_count -= 1;
        assert!(decode_leaf(&incomplete, [49; 32], &units, 100).is_err());
        let mut reordered = log.clone();
        reordered.rows.swap(4, 5);
        assert!(decode_leaf(&reordered, [49; 32], &units, 100).is_err());
        let mut forged = log.clone();
        let Row::Event { token, .. } = &mut forged.rows[5] else {
            unreachable!()
        };
        *token = temporary(99);
        assert!(decode_leaf(&forged, [49; 32], &units, 100).is_err());
        let mut old = log;
        old.version = 5;
        assert_eq!(
            decode_leaf(&old, [49; 32], &units, 100).unwrap_err(),
            Error::Binding
        );
    }
}

fn task_rows(task: u32, generation: u32, ordinal: u32, frame: u64) -> Vec<Row> {
    let task = (u64::from(generation) << 32) | u64::from(task);
    let (log, _) = fixture();
    let mut rows = vec![
        Row::Task { task, depth: 1 },
        Row::TaskPart {
            task,
            index: 0,
            ordinal,
        },
    ];
    for row in log.rows {
        rows.push(match row {
            Row::Enter { unit, .. } => Row::Enter { task, frame, unit },
            Row::Event {
                marker,
                action,
                token,
                owner,
                site,
                layout,
                ..
            } => Row::Event {
                task,
                frame,
                marker,
                action,
                token,
                owner,
                site,
                layout,
            },
            Row::Leave { .. } => Row::Leave { task, frame },
            _ => unreachable!(),
        });
    }
    rows
}
#[test]
fn native_task_paths_keep_interleaved_equal_values_and_reused_slots_distinct() {
    use hirsel_run_schema::ownership_trace::TaskPath;
    let (mut log, units) = fixture();
    let first = task_rows(1, 1, 0, 2);
    let second = task_rows(2, 1, 1, 3);
    log.rows = first
        .into_iter()
        .zip(second)
        .flat_map(|(a, b)| [a, b])
        .collect();
    log.rows.extend(task_rows(1, 2, 2, 4));
    log.row_count = log.rows.len() as u64;
    let trace = decode_leaf(&log, [49; 32], &units, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (6, 6, 0)
    );
    let tasks = trace
        .events
        .iter()
        .map(|e| e.task.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        tasks,
        [TaskPath(vec![0]), TaskPath(vec![1]), TaskPath(vec![2])].into()
    );
    let tokens = trace
        .events
        .iter()
        .filter_map(|e| {
            if let Action::Create { token, .. } = &e.action {
                Some(token)
            } else {
                None
            }
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(tokens.len(), 6);
}
#[test]
fn native_task_identity_refuses_incomplete_reused_live_duplicate_and_forged_paths() {
    let (mut log, units) = fixture();
    for mutation in 0..6 {
        let mut rows = task_rows(1, 1, 0, 2);
        match mutation {
            0 => {
                rows.remove(1);
            }
            1 => {
                rows.extend(task_rows(1, 1, 1, 3));
            }
            2 => {
                rows.insert(
                    3,
                    Row::Task {
                        task: (2 << 32) | 1,
                        depth: 1,
                    },
                );
            }
            3 => {
                rows.extend(task_rows(2, 1, 0, 3));
            }
            4 => {
                rows[0] = Row::Task {
                    task: (1 << 32) | 1,
                    depth: 2,
                };
                rows.insert(
                    2,
                    Row::TaskPart {
                        task: (1 << 32) | 1,
                        index: 1,
                        ordinal: 0,
                    },
                );
                rows[1] = Row::TaskPart {
                    task: (1 << 32) | 1,
                    index: 0,
                    ordinal: 99,
                };
            }
            5 => {
                rows[1] = Row::TaskPart {
                    task: (2 << 32) | 1,
                    index: 0,
                    ordinal: 0,
                };
            }
            _ => unreachable!(),
        }
        log.rows = rows;
        log.row_count = log.rows.len() as u64;
        assert!(
            matches!(
                decode_leaf(&log, [49; 32], &units, 100),
                Err(Error::InvalidTask(_))
            ),
            "mutation {mutation}"
        );
    }
}

#[test]
fn native_task_missing_rebind_and_stale_generation_never_relabel_owners() {
    let (mut log, units) = fixture();
    let first = task_rows(1, 1, 0, 2);
    let second = task_rows(1, 2, 1, 3);
    log.rows = first.clone();
    log.rows.extend_from_slice(&second[2..]);
    log.row_count = log.rows.len() as u64;
    assert_eq!(
        decode_leaf(&log, [49; 32], &units, 100).unwrap_err(),
        Error::UnsupportedTask((2 << 32) | 1)
    );
    log.rows = first.clone();
    log.rows.extend_from_slice(&second[..2]);
    log.rows.push(first[3]);
    log.rows.extend_from_slice(&second[2..]);
    log.row_count = log.rows.len() as u64;
    assert_eq!(
        decode_leaf(&log, [49; 32], &units, 100).unwrap_err(),
        Error::InvalidTask((1 << 32) | 1)
    );
    log.rows = first
        .into_iter()
        .filter(|row| !matches!(row, Row::Event { marker: 5, .. }))
        .collect();
    log.rows.extend(second);
    log.row_count = log.rows.len() as u64;
    assert_eq!(
        decode_leaf(&log, [49; 32], &units, 100).unwrap_err(),
        Error::InvalidTask((2 << 32) | 1)
    );
}

#[test]
fn native_ownership_empty_runtime_capture_cannot_hide_a_live_destination() {
    let (mut log, mut units) = fixture();
    let mut metadata = units[&[7; 32]][&1].clone();
    metadata.empty_environment = true;
    metadata.token = 6u64 << 32;
    metadata.owner = 0;
    units.get_mut(&[7; 32]).unwrap().insert(6, metadata);
    let event = Row::Event {
        task: 0,
        frame: 1,
        marker: 6,
        action: 1,
        token: 6u64 << 32,
        owner: 0,
        site: 42,
        layout: 7,
    };
    log.rows.insert(1, event);
    log.row_count += 1;
    let calls = LocalCalls::new();
    assert!(decode_local_calls(&log, [49; 32], &units, &calls, 100).is_ok());
    log.rows.swap(1, 2);
    assert_eq!(
        decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap_err(),
        Error::RetainedEnvironment(6)
    );
    units
        .get_mut(&[7; 32])
        .unwrap()
        .get_mut(&6)
        .unwrap()
        .empty_environment = false;
    assert!(matches!(
        decode_local_calls(&log, [49; 32], &units, &calls, 100),
        Err(Error::InvalidLocation(_))
    ));
}

#[test]
fn native_ownership_operation_stamps_preserve_task_local_ordinals_and_refuse_gaps() {
    let (mut log, units) = fixture();
    log.rows.insert(
        4,
        Row::Operation {
            task: 0,
            ordinal: 1,
        },
    );
    log.rows.insert(
        1,
        Row::Operation {
            task: 0,
            ordinal: 0,
        },
    );
    log.row_count = log.rows.len() as u64;
    let trace = decode_leaf(&log, [49; 32], &units, 100).unwrap();
    assert!(trace.events[..3].iter().all(|event| event.operation == 0));
    assert!(trace.events[3..].iter().all(|event| event.operation == 1));
    assert_eq!(trace.events[3].subordinal, 0);
    for ordinal in [0, 2, u64::MAX] {
        let mut forged = log.clone();
        forged.rows[5] = Row::Operation { task: 0, ordinal };
        assert_eq!(
            decode_leaf(&forged, [49; 32], &units, 100).unwrap_err(),
            Error::InvalidOperation { task: 0, ordinal }
        );
    }
    let mut unknown = log.clone();
    unknown.rows[1] = Row::Operation {
        task: 99,
        ordinal: 0,
    };
    assert_eq!(
        decode_leaf(&unknown, [49; 32], &units, 100).unwrap_err(),
        Error::UnsupportedTask(99)
    );
    let mut missing = log.clone();
    missing.rows.insert(
        5,
        Row::Operation {
            task: 0,
            ordinal: 2,
        },
    );
    missing.row_count += 1;
    assert!(matches!(
        decode_leaf(&missing, [49; 32], &units, 100),
        Err(Error::InvalidOperation { .. })
    ));
}

#[test]
fn native_ownership_spawn_binds_actual_child_to_checked_capture_custody() {
    let (_, template) = fixture();
    let base = template[&[7; 32]][&0].clone();
    let field = 8u64 << 32 | 1;
    let parameter = 2u64 << 32 | 1;
    let capture = 6u64 << 32;
    let child = (1u64 << 32) | 1;
    let metadata = |action, token, owner, callee| Metadata {
        action,
        token,
        owner,
        callee,
        ..base.clone()
    };
    let units = BTreeMap::from([
        (
            [7; 32],
            BTreeMap::from([
                (0, metadata(0, 0, 0, None)),
                (1, metadata(1, 0, field, None)),
                (2, metadata(5, 1, parameter, Some([8; 32]))),
            ]),
        ),
        (
            [8; 32],
            BTreeMap::from([
                (0, metadata(1, capture, 0, None)),
                (1, metadata(2, 0, 0, None)),
            ]),
        ),
    ]);
    let event = |task, frame, marker, action, token, owner| Row::Event {
        task,
        frame,
        marker,
        action,
        token,
        owner,
        site: 42,
        layout: 7,
    };
    let rows = vec![
        Row::Enter {
            task: 0,
            frame: 1,
            unit: [7; 32],
        },
        event(0, 1, 0, 0, 0, 0),
        event(0, 1, 1, 1, 0, field),
        event(0, 1, 2, 5, 1, parameter),
        Row::Task {
            task: child,
            depth: 1,
        },
        Row::TaskPart {
            task: child,
            index: 0,
            ordinal: 0,
        },
        Row::Spawn {
            task: 0,
            frame: 1,
            marker: 2,
            child,
        },
        Row::Leave { task: 0, frame: 1 },
        Row::Enter {
            task: child,
            frame: 2,
            unit: [8; 32],
        },
        event(child, 2, 0, 1, capture, 0),
        event(child, 2, 1, 2, 0, 0),
        Row::Leave {
            task: child,
            frame: 2,
        },
        Row::TaskEnd { task: child },
    ];
    let log = Log {
        version: VERSION,
        program: [49; 32],
        row_count: rows.len() as u64,
        rows,
        complete: true,
    };
    let calls = BTreeMap::from([([7; 32], [[8; 32]].into()), ([8; 32], Default::default())]);
    let trace = decode_local_calls(&log, [49; 32], &units, &calls, 100).unwrap();
    let analysis = trace.analyze([49; 32], 100).unwrap();
    assert_eq!(
        (analysis.created, analysis.dropped, analysis.live.len()),
        (1, 1, 0)
    );
    assert!(
        trace
            .events
            .iter()
            .any(|event| !event.task.0.is_empty() && matches!(event.action, Action::Drop { .. }))
    );
    for removed in [6, 12] {
        let mut missing = log.clone();
        missing.rows.remove(removed);
        missing.row_count -= 1;
        assert!(decode_local_calls(&missing, [49; 32], &units, &calls, 100).is_err());
    }
    let mut wrong = log.clone();
    let Row::Spawn { marker, .. } = &mut wrong.rows[6] else {
        unreachable!()
    };
    *marker = 1;
    assert!(decode_local_calls(&wrong, [49; 32], &units, &calls, 100).is_err());
    let mut wrong = log.clone();
    let Row::Enter { unit, .. } = &mut wrong.rows[8] else {
        unreachable!()
    };
    *unit = [7; 32];
    assert!(decode_local_calls(&wrong, [49; 32], &units, &calls, 100).is_err());
}
