//! Independent lineage join controls; the original live RPC acceptance stays open.
#[path = "../support/rpc_gateway_30/lineage.rs"]
mod lineage;
use hirsel_run_schema::Site;
use hirsel_run_schema::ownership_trace::{
    Action, Event, Owner, Storage, TaskPath, Token, Trace, VERSION,
};
use lineage::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

const PROGRAM: [u8; 32] = [59; 32];
fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../support/rpc_gateway_30/admission_vectors.json"
    ))
    .unwrap()
}
fn vector(name: &str) -> Value {
    vectors()["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == name)
        .unwrap()
        .clone()
}
fn site(kind: Kind) -> Site {
    Site {
        definition: "fixture.Credit".into(),
        file: "fixture.hir".into(),
        line: match kind {
            Kind::Create => 1,
            Kind::Move => 2,
            Kind::Drop => 3,
        },
        column: 1,
    }
}
fn declarations() -> Vec<CreditSite> {
    [Role::Request, Role::Upstream]
        .into_iter()
        .flat_map(|role| {
            [Kind::Create, Kind::Move, Kind::Drop]
                .into_iter()
                .map(move |kind| CreditSite {
                    role,
                    value_type: "fixture.Credit".into(),
                    kind,
                    site: site(kind),
                    path: vec![],
                    from_path: (kind == Kind::Move).then(Vec::new),
                })
        })
        .collect()
}

/// Fixture projection uses the finite oracle's semantic times, never ordinal
/// times. Equal Credit payloads are deliberately absent from all evidence.
fn fixture(vector: &Value) -> (Trace, RpcObservations) {
    let mut trace = Trace {
        version: VERSION,
        program: PROGRAM,
        complete: true,
        event_count: 0,
        events: vec![],
    };
    let mut rpc = RpcObservations {
        program: PROGRAM,
        complete: true,
        event_count: 0,
        events: vec![],
    };
    let mut held = BTreeMap::new();
    let mut sequence = 0;
    for (index, transition) in vector["events"].as_array().unwrap().iter().enumerate() {
        let location = Location {
            task: TaskPath::default(),
            operation: index as u64,
            subordinal: 0,
        };
        let mut transition = transition.clone();
        let time_ms = transition
            .as_object_mut()
            .unwrap()
            .remove("t")
            .unwrap()
            .as_u64();
        transition
            .as_object_mut()
            .unwrap()
            .entry("c")
            .or_insert(json!(0));
        let credit_kind = match transition["op"].as_str().unwrap() {
            "reserve" => Some((Role::Request, Kind::Create)),
            "acquire" => Some((Role::Upstream, Kind::Create)),
            "release_request" => Some((Role::Request, Kind::Drop)),
            "release_slot" => Some((Role::Upstream, Kind::Drop)),
            _ => None,
        };
        let credit = credit_kind.map(|(role, kind)| {
            let key = (
                transition["c"].as_u64().unwrap_or(0),
                transition["r"].as_u64().unwrap_or(0),
                role,
            );
            let (token, owner) = if kind == Kind::Create {
                let token = Token {
                    task: TaskPath::default(),
                    sequence,
                };
                let owner = Owner {
                    task: Some(TaskPath::default()),
                    storage: Storage::Value {
                        frame: 0,
                        value: sequence as u32,
                    },
                    path: vec![],
                };
                sequence += 1;
                held.insert(key, (token.clone(), owner.clone()));
                (token, owner)
            } else {
                held.remove(&key).unwrap()
            };
            let action = if kind == Kind::Create {
                Action::Create {
                    token: token.clone(),
                    owner: owner.clone(),
                    value_type: "fixture.Credit".into(),
                }
            } else {
                Action::Drop {
                    token: token.clone(),
                    owner: owner.clone(),
                }
            };
            trace.events.push(Event {
                task: location.task.clone(),
                operation: location.operation,
                subordinal: location.subordinal,
                site: site(kind),
                action,
            });
            CreditJoin {
                role,
                token,
                owner,
                from: None,
            }
        });
        rpc.events.push(Observation {
            location,
            transition: Some(transition),
            time_ms,
            credit,
        });
    }
    trace.event_count = trace.events.len() as u64;
    rpc.event_count = rpc.events.len();
    // Rejection controls must start from a qualifying join, so an unrelated
    // missing mapping cannot make every corruption test pass vacuously.
    let baseline = consume(&trace, Some(&rpc), &declarations(), PROGRAM);
    assert_eq!(baseline["complete"], true, "fixture: {baseline}");
    (trace, rpc)
}
fn joined(vector: &Value, trace: &Trace, rpc: &RpcObservations) -> Value {
    json!({"lineage": consume(trace, Some(rpc), &declarations(), PROGRAM),
        "config": vector["config"], "report": vector["report"], "replay": vector["replay"]})
}
fn judge(view: &Value, output: &Value) -> Value {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("admission.py"),
        include_str!("../support/rpc_gateway_30/admission.py"),
    )
    .unwrap();
    let mut child = Command::new("python3").current_dir(dir.path()).args(["-c",
        "import json,sys; from admission import judge_recording; v,o=json.load(sys.stdin); print(json.dumps(judge_recording(v,o)))"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&(view, output)).unwrap().as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{result:?}");
    serde_json::from_slice(&result.stdout).unwrap()
}
fn assert_missing(trace: &Trace, rpc: &RpcObservations) {
    let result = consume(trace, Some(rpc), &declarations(), PROGRAM);
    assert_eq!(result["complete"], false, "{result}");
    assert!(!result["missing"].as_array().unwrap().is_empty());
}

#[test]
fn rpc_lineage_59_original_cancel_before_reply_and_release_before_retry() {
    for name in [
        "queued-cancellation",
        "idempotent-refusal-1-2-3",
        "original-16-controlled",
        "original-17-controlled",
    ] {
        let v = vector(name);
        let (trace, rpc) = fixture(&v);
        let view = joined(&v, &trace, &rpc);
        assert_eq!(view["lineage"]["complete"], true, "{name}: {view}");
        let verdict = judge(&view, &v["replay"]["expected"]);
        assert_eq!(verdict["semantic_verdict"], "pass", "{name}: {verdict}");
    }
}

#[test]
fn rpc_lineage_59_equal_credits_keep_distinct_tokens_and_request_identity() {
    let v = vector("original-16-controlled");
    let (trace, mut rpc) = fixture(&v);
    let creates: Vec<_> = trace
        .events
        .iter()
        .filter(|e| matches!(e.action, Action::Create { .. }))
        .collect();
    assert_ne!(creates[0].action.token(), creates[1].action.token());
    let entry = rpc
        .events
        .iter_mut()
        .find(|e| e.transition.as_ref().unwrap()["op"] == "release_request")
        .unwrap();
    entry.credit.as_mut().unwrap().token = creates[1].action.token().clone();
    assert_missing(&trace, &rpc);
}

#[test]
fn rpc_lineage_59_duplicate_missing_drop_and_truncation_refuse() {
    let (trace, rpc) = fixture(&vector("terminal-ok"));
    let mut duplicate = trace.clone();
    let mut drop = duplicate.events.last().unwrap().clone();
    drop.operation += 100;
    duplicate.events.push(drop);
    duplicate.event_count += 1;
    assert_missing(&duplicate, &rpc);
    let mut missing = trace.clone();
    missing.events.pop();
    missing.event_count -= 1;
    assert_missing(&missing, &rpc);
    for failure in 0..3 {
        let mut bad = trace.clone();
        match failure {
            0 => bad.complete = false,
            1 => bad.event_count += 1,
            _ => {
                bad.events.pop();
            }
        }
        assert_missing(&bad, &rpc);
    }
}

#[test]
fn rpc_lineage_59_program_task_site_path_and_role_must_match_exactly() {
    let (trace, rpc) = fixture(&vector("terminal-ok"));
    for failure in 0..8 {
        let mut trace = trace.clone();
        let mut rpc = rpc.clone();
        match failure {
            0 => trace.program = [60; 32],
            1 => rpc.program = [60; 32],
            2 => {
                trace.program = [60; 32];
                rpc.program = [60; 32];
            }
            3 => rpc.events[0].location.task = TaskPath(vec![1]),
            4 => trace.events[0].site.line += 1,
            5 => rpc.events[0]
                .credit
                .as_mut()
                .unwrap()
                .owner
                .path
                .push(hirsel_run_schema::ownership_trace::Projection::Field { field: 0 }),
            6 => rpc.events[0].credit.as_mut().unwrap().role = Role::Upstream,
            _ => rpc.events[0].transition.as_mut().unwrap()["r"] = json!(1),
        }
        assert_missing(&trace, &rpc);
    }
    // A valid nested field path qualifies only its exact declaration, not a
    // prefix or a neighbouring field holding an equal-valued Credit.
    let mut nested = trace.clone();
    let path = vec![
        hirsel_run_schema::ownership_trace::Projection::Field { field: 1 },
        hirsel_run_schema::ownership_trace::Projection::Field { field: 0 },
    ];
    for event in &mut nested.events {
        match &mut event.action {
            Action::Create { owner, .. } | Action::Drop { owner, .. } => owner.path = path.clone(),
            Action::Move { from, owner, .. } => {
                from.path = path.clone();
                owner.path = path.clone();
            }
        }
    }
    let mut rpc = rpc;
    for event in &mut rpc.events {
        if let Some(credit) = &mut event.credit {
            credit.owner.path = path.clone();
        }
    }
    let mut sites = declarations();
    for site in &mut sites {
        site.path = path.clone();
    }
    assert_eq!(
        consume(&nested, Some(&rpc), &sites, PROGRAM)["complete"],
        true
    );
    for site in &mut sites {
        site.path.pop();
    }
    assert_eq!(
        consume(&nested, Some(&rpc), &sites, PROGRAM)["complete"],
        false
    );
}

#[test]
fn rpc_lineage_59_missing_identity_timing_join_or_stream_refuses() {
    let v = vector("terminal-ok");
    let (trace, rpc) = fixture(&v);
    for failure in 0..7 {
        let mut rpc = rpc.clone();
        match failure {
            0 => rpc.events[0].transition = None,
            1 => rpc.events[1].time_ms = None,
            2 => rpc.events[0].credit = None,
            3 => rpc.complete = false,
            4 => rpc.event_count += 1,
            5 => rpc.events[0].location.subordinal += 1,
            _ => {
                rpc.events.pop();
                rpc.event_count -= 1;
            }
        }
        let view = joined(&v, &trace, &rpc);
        let verdict = judge(&view, &v["replay"]["expected"]);
        assert_ne!(verdict["semantic_verdict"], "pass", "{failure}: {verdict}");
    }
    assert_eq!(
        consume(&trace, None, &declarations(), PROGRAM)["complete"],
        false
    );
}

#[test]
fn rpc_lineage_59_independent_move_and_drop_are_witnessed() {
    let v = vector("terminal-ok");
    let (mut trace, mut rpc) = fixture(&v);
    // A separate subordinal observes an actual move between reserve and admit.
    let first = trace.events[0].clone();
    let from = first.action.owner().clone();
    let mut owner = from.clone();
    owner.storage = Storage::Slot { frame: 0, slot: 99 };
    owner.task = Some(TaskPath(vec![2]));
    let token = first.action.token().clone();
    let moved = Event {
        task: first.task.clone(),
        operation: first.operation,
        subordinal: 1,
        site: site(Kind::Move),
        action: Action::Move {
            token: token.clone(),
            from: from.clone(),
            owner: owner.clone(),
        },
    };
    trace.events.insert(1, moved.clone());
    trace.event_count += 1;
    let drop = trace
        .events
        .iter_mut()
        .find(|e| matches!(&e.action, Action::Drop { token: t, .. } if *t == token))
        .unwrap();
    drop.task = TaskPath(vec![2]);
    drop.operation = 0; // Per-task ordinals restart; they are not clocks.
    if let Action::Drop { owner: o, .. } = &mut drop.action {
        *o = owner.clone();
    }
    let drop = rpc
        .events
        .iter_mut()
        .find(|e| e.transition.as_ref().unwrap()["op"] == "release_request")
        .unwrap();
    drop.location.task = TaskPath(vec![2]);
    drop.location.operation = 0;
    drop.credit.as_mut().unwrap().owner = owner.clone();
    rpc.events.insert(
        1,
        Observation {
            location: Location {
                task: moved.task,
                operation: moved.operation,
                subordinal: 1,
            },
            transition: Some(json!({"op":"move_credit", "c":0,"r":0})),
            time_ms: None,
            credit: Some(CreditJoin {
                role: Role::Request,
                token,
                owner,
                from: Some(from),
            }),
        },
    );
    rpc.event_count += 1;
    let view = joined(&v, &trace, &rpc);
    assert_eq!(view["lineage"]["complete"], true, "{view}");
    assert_eq!(
        view["lineage"]["witnessed"].as_array().unwrap().len(),
        trace.events.len()
    );
    assert_eq!(
        judge(&view, &v["replay"]["expected"])["semantic_verdict"],
        "pass"
    );
    rpc.events[1].credit.as_mut().unwrap().from = None;
    assert_missing(&trace, &rpc);
}

#[test]
fn rpc_lineage_59_retains_all_seventeen_invariant_vectors() {
    let dir = tempfile::tempdir().unwrap();
    for (name, contents) in [
        (
            "admission.py",
            include_str!("../support/rpc_gateway_30/admission.py"),
        ),
        (
            "test_admission.py",
            include_str!("../support/rpc_gateway_30/test_admission.py"),
        ),
        (
            "admission_vectors.json",
            include_str!("../support/rpc_gateway_30/admission_vectors.json"),
        ),
    ] {
        std::fs::write(dir.path().join(name), contents).unwrap();
    }
    let result = Command::new("python3")
        .current_dir(dir.path())
        .args(["-m", "unittest", "-v", "test_admission"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&result.stderr));
}

#[test]
fn rpc_lineage_59_actual_two_equal_credit_source_without_rpc_mapping_is_unproven() {
    hirsel_engine::with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("main.hir");
        std::fs::write(&source, "resource type Credit { amount: u64 }\ndrop(x) {}\npub fn main() -> Result[(), ()] {\n let first = Credit(amount: 1u64)\n let second = Credit(amount: 1u64)\n let held = take first\n Ok(())\n}\n").unwrap();
        for checked in [false, true] {
            let run = hirsel_driver::ownership_trace::run_resources_on_host(
                &source,
                hirsel_driver::HostLaunch::Live {
                    config: hirsel_driver::LaunchConfig::Grants {
                        input: std::ffi::OsStr::new("{}"),
                        arguments: None,
                    },
                    record: None,
                    revocations: None,
                },
                Some(100_000),
                hirsel_engine::Verify::On,
                checked,
                100,
            )
            .unwrap();
            let trace = run.trace.unwrap();
            let analysis = trace.analyze(run.program, 100).unwrap();
            assert_eq!(
                (analysis.created, analysis.dropped, analysis.live.len()),
                (2, 2, 0)
            );
            // The source trace also exercises exact independently supplied
            // ownership joins. These fixture labels are not live RPC admission.
            let mut catalogue = Vec::new();
            let mut observations = Vec::new();
            for event in &trace.events {
                let action_kind = match &event.action {
                    Action::Create { .. } => Kind::Create,
                    Action::Move { .. } => Kind::Move,
                    Action::Drop { .. } => Kind::Drop,
                };
                let from = match &event.action {
                    Action::Move { from, .. } => Some(from.clone()),
                    _ => None,
                };
                let token = event.action.token();
                let value_type = trace
                    .events
                    .iter()
                    .find_map(|e| match &e.action {
                        Action::Create {
                            token: t,
                            value_type,
                            ..
                        } if t == token => Some(value_type.clone()),
                        _ => None,
                    })
                    .unwrap();
                let declaration = CreditSite {
                    role: Role::Request,
                    value_type,
                    kind: action_kind,
                    site: event.site.clone(),
                    path: event.action.owner().path.clone(),
                    from_path: from.as_ref().map(|o| o.path.clone()),
                };
                if !catalogue.iter().any(|d: &CreditSite| {
                    d.kind == declaration.kind
                        && d.site == declaration.site
                        && d.path == declaration.path
                        && d.from_path == declaration.from_path
                }) {
                    catalogue.push(declaration);
                }
                observations.push(Observation {
                    location: Location { task: event.task.clone(), operation: event.operation, subordinal: event.subordinal },
                    transition: Some(json!({"op": match action_kind { Kind::Create => "reserve", Kind::Move => "move_credit", Kind::Drop => "release_request" },
                        "c": 0, "r": token.sequence})), time_ms: Some(0),
                    credit: Some(CreditJoin { role: Role::Request, token: token.clone(), owner: event.action.owner().clone(), from }),
                });
            }
            let rpc = RpcObservations {
                program: run.program,
                complete: true,
                event_count: observations.len(),
                events: observations,
            };
            let joined = consume(&trace, Some(&rpc), &catalogue, run.program);
            assert_eq!(joined["complete"], true, "{joined}");
            assert_eq!(
                joined["witnessed"].as_array().unwrap().len(),
                trace.events.len()
            );
            let result = consume(&trace, None, &[], run.program);
            assert_eq!(result["complete"], false);
            assert_eq!(result["missing"][0], "RPC identity/timing stream missing");
            assert!(result["events"].as_array().unwrap().is_empty());
        }
    });
}
