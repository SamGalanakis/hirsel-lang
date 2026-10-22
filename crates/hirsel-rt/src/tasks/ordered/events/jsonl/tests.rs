use super::*;

pub(super) fn event(outcome: CanonicalOutcome) -> CanonicalEvent {
    CanonicalEvent {
        task: vec![2, 1],
        ordinal: u64::MAX,
        subevent: u32::MAX,
        op: "test.\"escape\nλ",
        transition: Transition::Call {
            members: vec![vec![1], vec![0, 2]],
            then_yield: true,
        },
        item: CanonicalItem::Object(ObjectId {
            creator: vec![1, 2],
            ordinal: u64::MAX,
        }),
        version: u64::MAX,
        outcome,
        retirement: u64::MAX,
        before: Stretch::new(crate::budget::stretch::Budgets {
            fuel: true,
            memory: true,
        }),
        credit: u64::MAX,
    }
}

#[test]
fn canonical_jsonl_preserves_exact_integers_escaping_stretches_and_initialized_bytes() {
    let mut row = event(CanonicalOutcome::Received {
        status: 0,
        payload: Some(vec![0, 255, 7]),
    });
    row.before.fuel_points(2);
    row.before.alloc(24).unwrap();
    row.before.free(8);
    row.before.credit(16).unwrap();
    row.before.alloc(32).unwrap();
    let mut log = Log::default();
    log.canonical.push(row.clone());
    let encoded = log.jsonl().unwrap();
    assert_eq!(encoded.iter().filter(|&&byte| byte == b'\n').count(), 1);
    assert!(encoded.ends_with(b"\n"));
    let parsed: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(parsed["ordinal"].as_u64(), Some(u64::MAX));
    assert_eq!(parsed["subevent"].as_u64(), Some(u32::MAX as u64));
    assert_eq!(parsed["op"], "test.\"escape\nλ");
    assert_eq!(parsed["outcome"]["payload"], "00ff07");
    assert_eq!(
        parsed["before"],
        json!([
            {"credit":0,"fuel":2,"net":16,"envelope":[[2,24]]},
            {"credit":16,"fuel":0,"net":32,"envelope":[[0,32]]},
        ])
    );
    assert_eq!(
        parsed["transition"],
        json!({"kind":"call","members":[[1],[0,2]],"then_yield":true})
    );
    // Native evidence and the test-only exact charge journal are excluded.
    log.boundaries.push(Boundary {
        task: vec![],
        ordinal: 0,
        op: None,
        transition: Transition::Keep,
        retirement: 999,
        before: row.before,
        credit: 0,
        native_outcome: Some((99, vec![123; 48])),
    });
    log.canonical[0].before.exact.clear();
    assert_eq!(log.jsonl().unwrap(), encoded);
    assert!(Log::default().jsonl().unwrap().is_empty());
}

#[test]
fn canonical_jsonl_keeps_absence_distinct_from_empty_and_preserves_failure_sites() {
    let site = hirsel_run_schema::Site {
        definition: "d\"λ".into(),
        file: "file\n.hir".into(),
        line: u64::MAX,
        column: 7,
    };
    let failure = hirsel_run_schema::Failure {
        rule: "FAIL-1".into(),
        site: site.clone(),
        operation: Some(site),
    };
    let mut log = Log::default();
    for payload in [None, Some(vec![])] {
        log.canonical
            .push(event(CanonicalOutcome::Received { status: 0, payload }));
    }
    log.canonical.push(event(CanonicalOutcome::TaskFailed {
        failures: vec![(failure.clone(), vec![1, 2])],
    }));
    let encoded = log.jsonl().unwrap();
    let rows: Vec<Value> = std::str::from_utf8(&encoded)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 3);
    assert!(rows[0]["outcome"]["payload"].is_null());
    assert_eq!(rows[1]["outcome"]["payload"], "");
    assert_eq!(
        rows[2]["outcome"]["failures"][0]["site"]["line"].as_u64(),
        Some(u64::MAX)
    );
    assert_eq!(
        rows[2]["outcome"]["failures"][0]["operation"]["file"],
        failure.site.file
    );
    assert_eq!(rows[2]["outcome"]["failures"][0]["task"], json!([1, 2]));
}

#[test]
fn canonical_jsonl_rejects_incomplete_evidence_before_returning_any_bytes() {
    for pending in 0..3 {
        let mut log = Log::default();
        log.canonical.push(event(CanonicalOutcome::Unit));
        match pending {
            0 => log.unsupported.push(Unsupported {
                task: vec![],
                ordinal: 0,
                op: Some("unknown"),
            }),
            1 => {
                log.host_entries.insert(
                    (vec![], 0),
                    Boundary {
                        task: vec![],
                        ordinal: 0,
                        op: Some("host.request"),
                        transition: Transition::Keep,
                        retirement: 0,
                        before: log.canonical[0].before.clone(),
                        credit: 0,
                        native_outcome: None,
                    },
                );
            }
            2 => {
                log.host_certified.insert((vec![], 0));
            }
            _ => unreachable!(),
        }
        assert_eq!(log.jsonl(), Err("incomplete canonical event evidence"));
    }
}

#[test]
fn canonical_jsonl_terminal_and_split_metadata_keep_logical_identity_and_codec() {
    let parent = HostHandleId {
        operation: ObjectId {
            creator: vec![3, 1],
            ordinal: 7,
        },
        field: 0,
    };
    let half = HostHandleId {
        operation: ObjectId {
            creator: vec![3, 1],
            ordinal: 8,
        },
        field: 1,
    };
    let mut log = Log::default();
    let mut created = event(CanonicalOutcome::HostCreated(host_keys::Creation {
        handle: half.clone(),
        resource: host_keys::Resource::Created(half.clone()),
        kind: host_keys::ResourceKind::Half {
            stream: host_keys::Resource::Created(parent),
        },
    }));
    created.item = CanonicalItem::HostHandle(half);
    log.canonical.push(created);
    let mut root = event(CanonicalOutcome::RootReleased { last: true });
    root.item = CanonicalItem::SharedRoot(crate::ordered::root_ids::SharedRootId {
        creator: vec![2],
        freeze: 9,
    });
    log.canonical.push(root);
    for ended in [
        hirsel_run_schema::RunOutcome::BudgetExceeded(hirsel_run_schema::BudgetKind::Memory),
        hirsel_run_schema::RunOutcome::Refused("λ\n\"refusal".into()),
    ] {
        let mut stopped = event(CanonicalOutcome::Stopped(ended.clone()));
        stopped.item = CanonicalItem::Terminal;
        log.canonical.push(stopped);
        let serialized = log.jsonl().unwrap();
        let parsed: Value = serde_json::from_str(
            std::str::from_utf8(&serialized)
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(parsed["outcome"]["codec"], "hirsel.tape.fixint@1");
        let text = parsed["outcome"]["payload"].as_str().unwrap();
        let bytes: Vec<_> = text
            .as_bytes()
            .chunks_exact(2)
            .map(|digits| u8::from_str_radix(std::str::from_utf8(digits).unwrap(), 16).unwrap())
            .collect();
        assert_eq!(
            crate::tape_wire::decode::<hirsel_run_schema::RunOutcome>(&bytes).unwrap(),
            ended
        );
    }
    let serialized = log.jsonl().unwrap();
    let rows: Vec<Value> = std::str::from_utf8(&serialized)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["item"]["id"]["field"], 1);
    assert_eq!(
        rows[0]["outcome"]["creation"]["resource_kind"]["stream"]["handle"]["operation"]["ordinal"],
        7
    );
    assert_eq!(
        rows[1]["item"],
        json!({"kind":"shared_root","creator":[2],"freeze":9})
    );
}
