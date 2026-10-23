use super::*;
use crate::launch::{RecordedRun, Recording};
use crate::tape_wire::Wire;
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::record::{Encoder, Record};
use hirsel_run_config::RunConfig;
use hirsel_run_schema::{
    CallForm, CallOutcome, CallRecord, CalleeOutcome, Cause, LaunchKind, RunnerCompletion,
    RunnerRequest,
};

fn header(record: RunConfig, instance: Option<[u8; 16]>) -> Recording {
    let run = record.validated().unwrap();
    Recording {
        arguments: vec![],
        run_identity: run.identity(),
        hash_seed: run.config().hash_seed(),
        random_seed: run.config().random_seed(),
        test_seed: run.config().test_seed(),
        binding: Some(RecordedRun {
            canonical: run.canonical().to_vec(),
            placement: None,
            admitted_at: 0,
            revocation: None,
            instance,
        }),
        tree: Default::default(),
        events: vec![],
    }
}
fn fixture() -> (Recording, Recording, RunnerRequest, RunnerCompletion) {
    let instance = [17; 16];
    let key = hirsel_run_schema::call_key(&instance, 0);
    let request = RunnerRequest {
        runner: "worker".into(),
        form: CallForm::Call,
        path: "fixture@1/child".into(),
        grants: vec![],
        definitions: vec![],
        key,
        digest: [29; 32],
        argument: vec![8],
    };
    let mut record = RunConfig::empty([1; 32], "fixture@1/main".into(), LaunchKind::Main);
    record.io = Some(serde_json::from_value(serde_json::json!({"tasks":"tasks.free@1","dirs":[],
        "runners":[{"name":"worker","provider":"local.threads@1","grants":{"dirs":[]},"definitions":null}]})).unwrap());
    let caller = header(record, Some(instance));
    let mut record = RunConfig::empty([2; 32], request.path.clone(), LaunchKind::Call);
    record.io = Some(Default::default());
    record.call = Some(CallRecord::new(
        CallForm::Call,
        &Cause {
            instance,
            ordinal: 0,
            digest: request.digest,
        },
        &request.argument,
    ));
    let mut callee = header(record, Some(hirsel_run_schema::callee_instance(&key)));
    callee.events.push(crate::io::Event {
        request: Request::CallReturn(CalleeOutcome::Ok(vec![9])),
        completion: Completion::Ready,
        fault: None,
    });
    let completion = RunnerCompletion {
        outcome: CallOutcome::Ok(vec![9]),
        callee: Some(hirsel_run_schema::callee_instance(&key)),
    };
    (caller, callee, request, completion)
}
fn event(request: Request, completion: Completion) -> crate::io::Event {
    crate::io::Event {
        request,
        completion,
        fault: None,
    }
}
fn records(op: u64, request: Request, completion: Option<Completion>) -> Vec<Record> {
    let mut payload = vec![];
    request.encode(&mut payload);
    let mut observed = vec![];
    (payload, Vec::<super::super::requests::Authority>::new()).encode(&mut observed);
    let mut out = vec![
        Record::Prepared { op, id: op },
        Record::Observation {
            op,
            bytes: observed,
        },
    ];
    if let Some(completion) = completion {
        let envelope = Envelope {
            refusal: None,
            result: Ok(completion),
            created: vec![],
            output_count: None,
        };
        let mut bytes = vec![];
        envelope.encode(&mut bytes);
        out.push(Record::Completion { op, bytes });
    }
    out
}
fn tape(header: &Recording, logs: Vec<(Vec<u32>, Vec<Record>, Progress)>) -> Vec<u8> {
    let mut binding = b"F1B3".to_vec();
    crate::ABI_VERSION.encode(&mut binding);
    header.encode(&mut binding);
    false.encode(&mut binding);
    let mut out = hirsel_free::tape::header(&binding);
    let tasks: Vec<_> = logs
        .into_iter()
        .map(|(path, records, progress)| {
            let mut encoder = Encoder::default();
            let mut bytes = vec![];
            for record in records {
                encoder.encode(&record, &mut bytes);
            }
            hirsel_free::tape::TaskChunk {
                path,
                bytes,
                progress,
            }
        })
        .collect();
    out.extend(hirsel_free::tape::flush(&[], &tasks));
    out
}
fn checked(caller: Recording, callee: Recording) -> Result<crate::tape_links::Links, Vec<String>> {
    crate::tape_links::check(&[("parent".into(), caller), ("child".into(), callee)])
}

#[test]
fn linked_free_inspection_pairs_spawned_poll_and_rejects_forged_links() {
    let (caller, callee, request, completion) = fixture();
    let bytes = tape(
        &caller,
        vec![
            (
                vec![],
                records(
                    1,
                    Request::RunnerCall(Box::new(request.clone())),
                    Some(Completion::Ready),
                ),
                Progress::new(1, Phase::Done),
            ),
            (
                vec![0],
                records(
                    1,
                    Request::RunnerPoll {
                        key: request.key,
                        cancel: None,
                    },
                    Some(Completion::Runner(completion.clone())),
                ),
                Progress::new(1, Phase::Done),
            ),
        ],
    );
    let parsed = linked_recording(&bytes).unwrap();
    assert_eq!(
        checked(parsed.clone(), callee.clone())
            .unwrap()
            .checked
            .len(),
        1
    );
    for point in 0..5 {
        let mut bad = parsed.clone();
        match point {
            0 => {
                if let Request::RunnerCall(r) = &mut bad.events[0].request {
                    r.digest[0] ^= 1;
                }
            }
            1 => {
                if let Request::RunnerCall(r) = &mut bad.events[0].request {
                    r.argument.push(0);
                }
            }
            2 => {
                if let Request::RunnerCall(r) = &mut bad.events[0].request {
                    r.path.push('x');
                }
            }
            3 => {
                if let Completion::Runner(c) = &mut bad.events[1].completion {
                    c.callee = Some([0; 16]);
                }
            }
            _ => {
                if let Completion::Runner(c) = &mut bad.events[1].completion {
                    c.outcome = CallOutcome::Ok(vec![10]);
                }
            }
        }
        assert!(checked(bad, callee.clone()).is_err(), "forged link {point}");
    }
    let mut bad = parsed.clone();
    bad.events.remove(0);
    assert!(checked(bad, callee.clone()).is_err());
    let mut bad = parsed;
    let mut conflicting = completion;
    conflicting.outcome = CallOutcome::Ok(vec![10]);
    bad.events.push(event(
        Request::RunnerPoll {
            key: request.key,
            cancel: None,
        },
        Completion::Runner(conflicting),
    ));
    assert!(checked(bad, callee.clone()).is_err());
    let missing = crate::tape_links::check(&[("child".into(), callee)]).unwrap();
    assert_eq!(missing.unchecked.len(), 1);
}

#[test]
fn linked_free_inspection_preserves_partial_boundaries_and_refuses_bad_envelopes() {
    let (caller, _, request, completion) = fixture();
    let entered = records(1, Request::RunnerCall(Box::new(request.clone())), None);
    let bytes = tape(
        &caller,
        vec![(vec![], entered.clone(), Progress::new(1, Phase::Entered))],
    );
    let parsed = linked_recording(&bytes).unwrap();
    assert_eq!(parsed.events.len(), 1);
    assert_eq!(parsed.events[0].completion, Completion::Ready);
    let returned = records(1, Request::CallReturn(CalleeOutcome::Ok(vec![9])), None);
    assert!(
        linked_recording(&tape(
            &caller,
            vec![(vec![], returned, Progress::new(1, Phase::Entered))]
        ))
        .unwrap()
        .events
        .is_empty()
    );
    let done = records(
        1,
        Request::RunnerCall(Box::new(request)),
        Some(Completion::Runner(completion)),
    );
    let mut duplicate = done.clone();
    duplicate.push(done[2].clone());
    let mut unobserved = done.clone();
    unobserved.remove(1);
    let mut malformed = done.clone();
    if let Record::Completion { bytes, .. } = &mut malformed[2] {
        bytes.push(0);
    }
    for records in [duplicate, unobserved, malformed] {
        assert!(
            linked_recording(&tape(
                &caller,
                vec![(vec![], records, Progress::new(1, Phase::Done))]
            ))
            .is_err()
        );
    }
    assert!(
        linked_recording(&tape(
            &caller,
            vec![(vec![], done.clone(), Progress::new(1, Phase::Entered))]
        ))
        .is_err()
    );
    let valid = tape(&caller, vec![(vec![], done, Progress::new(1, Phase::Done))]);
    // A torn final flush retains the header's empty cut, never its host outcome.
    assert!(
        linked_recording(&valid[..valid.len() - 5])
            .unwrap()
            .events
            .is_empty()
    );
}
