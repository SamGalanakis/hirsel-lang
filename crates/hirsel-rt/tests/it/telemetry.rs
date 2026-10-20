use hirsel_rt::telemetry::{Capture, kernel};
use hirsel_rt::{io::*, io_real::RealIo, io_sim::SimIo};
use hirsel_run_schema::ProviderContract;

/// A root parent: zero IDs, sampled, an empty tracestate tail.
fn root() -> Vec<u8> {
    let mut parent = vec![0; 24];
    parent.extend_from_slice(&[1, 7, 7]);
    parent
}

fn begin(parent: Vec<u8>) -> Request {
    Request::TelemetryBegin {
        telemetry: 2,
        parent,
        spec: b"spec".to_vec(),
    }
}

/// Telemetry touches no directory: the grant's root is never read.
fn live(capture: Capture) -> RealIo {
    let mut real = RealIo::grant(std::path::Path::new(".")).unwrap();
    real.set_telemetry(capture);
    assert_eq!(
        real.call(Request::Telemetry { io: 1 }),
        Ok(Completion::Handle(Ok(2)))
    );
    real
}

/// Root, child, an event, a finish, a log, a measurement and an abandoning close.
fn run(real: &mut RealIo) -> Vec<Completion> {
    let mut completions = vec![real.call(begin(root())).unwrap()];
    let Completion::Span { trace, span, .. } = completions[0].clone() else {
        panic!("{completions:?}")
    };
    let mut child = trace.to_vec();
    child.extend_from_slice(&span);
    child.extend_from_slice(&[1, 7, 7]);
    for request in [
        begin(child),
        Request::TelemetryEvent {
            telemetry: 2,
            span: 4,
            payload: b"event".to_vec(),
        },
        Request::TelemetryFinish {
            telemetry: 2,
            span: 4,
            status: b"ok".to_vec(),
        },
        Request::TelemetryLog {
            telemetry: 2,
            payload: b"{}".to_vec(),
        },
        Request::TelemetryMeasure {
            telemetry: 2,
            payload: vec![],
        },
        Request::Close { handle: 3 },
    ] {
        completions.push(real.call(request).unwrap());
    }
    completions
}

#[test]
fn null_identities_follow_the_seed_and_replay_from_the_tape() {
    let mut real = live(Capture::default());
    let completions = run(&mut real);
    assert_eq!(
        completions[0],
        Completion::Span {
            handle: 3,
            trace: *b"\xaf\xcd\x1d\x7b\x39\xa8\x20\xe2\xf4\x65\xb9\xa1\x6a\x9e\x78\x6e",
            span: *b"\x4f\x45\x09\x80\x18\x5d\xc4\x06",
        }
    );
    let Completion::Span { trace, span, .. } = completions[1] else {
        panic!()
    };
    assert_eq!(
        trace[..4],
        [0xaf, 0xcd, 0x1d, 0x7b],
        "a child keeps its trace"
    );
    assert_eq!(span, *b"\xec\x81\x4c\x72\xa8\xb8\x8b\xf8");
    // A closed span has no context; a live one carries its parent's flags and tail.
    assert_eq!(real.telemetry_context(2, 3), None);
    let Ok(Completion::Span { handle, .. }) = real.call(begin(root())) else {
        panic!()
    };
    assert_eq!(real.telemetry_context(2, handle).unwrap()[24..], [1, 7, 7]);
    // The finished child and the closed root are gone.
    assert_eq!(
        real.call(Request::TelemetryEvent {
            telemetry: 2,
            span: 4,
            payload: vec![],
        }),
        Err(AdapterError::InvalidHandle(4))
    );
    let tape = real.tape()[..8].to_vec();
    let encoded = hirsel_rt::launch::render_tape(&tape).unwrap();
    assert!(encoded.contains("telemetry_begin 2 "), "{encoded}");
    assert_eq!(hirsel_rt::launch::parse_tape(&encoded).unwrap(), tape);
    let mut replay = SimIo::new(Tree::default(), tape.clone());
    for event in &tape {
        assert_eq!(
            replay.call(event.request.clone()).as_ref(),
            Ok(&event.completion)
        );
    }
    replay.finish().unwrap();
}

#[test]
fn replay_refuses_changed_payloads_and_forged_identities() {
    let mut real = live(Capture::default());
    run(&mut real);
    let tape = real.tape().to_vec();
    // A changed observation payload.
    let mut replay = SimIo::new(Tree::default(), tape.clone());
    for event in &tape[..3] {
        replay.call(event.request.clone()).unwrap();
    }
    assert_eq!(
        replay.call(Request::TelemetryEvent {
            telemetry: 2,
            span: 4,
            payload: b"Event".to_vec(),
        }),
        Err(AdapterError::ReplayMismatch(3))
    );
    // A recorded child under another trace, a zero span and a reused span are forged.
    for forge in [
        |c: &mut Completion| {
            if let Completion::Span { trace, .. } = c {
                trace[0] ^= 1
            }
        },
        |c: &mut Completion| {
            if let Completion::Span { span, .. } = c {
                *span = [0; 8]
            }
        },
        |c: &mut Completion| {
            if let Completion::Span { span, .. } = c {
                *span = *b"\x4f\x45\x09\x80\x18\x5d\xc4\x06"
            }
        },
    ] {
        let mut forged = tape.clone();
        forge(&mut forged[2].completion);
        let mut replay = SimIo::new(Tree::default(), forged.clone());
        replay.call(forged[0].request.clone()).unwrap();
        replay.call(forged[1].request.clone()).unwrap();
        assert!(replay.call(forged[2].request.clone()).is_err());
    }
}

#[test]
fn exporter_congestion_drops_only_the_exporters_copy() {
    let mut open = live(Capture::of(ProviderContract::TelemetryNull, 0, None));
    let mut full = live(Capture::of(ProviderContract::TelemetryNull, 0, Some(0)));
    let mut narrow = live(Capture::of(ProviderContract::TelemetryNull, 0, Some(3)));
    let completions = run(&mut open);
    assert_eq!(run(&mut full), completions);
    assert_eq!(run(&mut narrow), completions);
    assert_eq!(full.tape(), open.tape());
    assert_eq!(narrow.tape(), open.tape());
    assert_eq!(
        (
            open.telemetry().exporter.queued,
            open.telemetry().exporter.dropped
        ),
        (7, 0)
    );
    assert_eq!(
        (
            full.telemetry().exporter.queued,
            full.telemetry().exporter.dropped
        ),
        (0, 7)
    );
    assert_eq!(
        (
            narrow.telemetry().exporter.queued,
            narrow.telemetry().exporter.dropped
        ),
        (3, 4)
    );
}

#[test]
fn host_identities_are_nonzero_and_distinct() {
    let mut real = live(Capture::of(ProviderContract::TelemetryCapture, 0, None));
    let mut spans = std::collections::BTreeSet::new();
    let mut traces = std::collections::BTreeSet::new();
    for _ in 0..64 {
        let Ok(Completion::Span { trace, span, .. }) = real.call(begin(root())) else {
            panic!()
        };
        assert!(trace != [0; 16] && span != [0; 8]);
        assert!(traces.insert(trace) && spans.insert(span));
    }
}

#[test]
fn invalid_parents_are_refused_by_the_shared_kernel() {
    assert_eq!(kernel::parent(&root()), Ok(None));
    let mut mixed = root();
    mixed[0] = 1;
    assert_eq!(kernel::parent(&mixed), Err(kernel::Refusal::InvalidParent));
    assert_eq!(
        kernel::parent(&[0; 24]),
        Err(kernel::Refusal::InvalidParent)
    );
    assert_eq!(
        kernel::bounded(&[&vec![0; kernel::PAYLOAD_LIMIT + 1]]),
        Err(kernel::Refusal::TooLarge)
    );
}
