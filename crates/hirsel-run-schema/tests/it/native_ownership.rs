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
