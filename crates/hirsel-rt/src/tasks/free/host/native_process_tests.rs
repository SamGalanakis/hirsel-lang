//! Native process issue derives inherited domains only from persisted creation metadata.
use super::*;
use crate::io::GrantResult;
use hirsel_run_schema::{Command, ExitStatus, ProcessFailure, ProcessOutcome, ProcessRule};

unsafe fn issue(ctx: *mut TaskCtx, request: Request) -> Completion {
    let _role = unsafe { role::Guard::new(ctx) };
    let (result, parked) = unsafe { crate::stop::check(ctx, native::issue(ctx, request)) };
    let completion = result.unwrap();
    unsafe { native::finish_result(ctx, Ok(completion), true, parked) }.unwrap()
}
fn command(program: &str) -> Command {
    Command {
        program: program.into(),
        args: vec![
            "-c".into(),
            "printf 'process output'; printf 'process error' >&2".into(),
        ],
        directory: None,
        env: Some(vec![]),
        own_group: false,
    }
}
unsafe fn program(ctx: *mut TaskCtx, granted: bool) {
    let mut first = GrantResult::default();
    let mut alias = GrantResult::default();
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_process(ctx, std::ptr::null_mut(), &mut first, &1) },
        STATUS_OK
    );
    assert_eq!(
        unsafe { crate::io::hirsel_rt_v2_io_process(ctx, std::ptr::null_mut(), &mut alias, &1) },
        STATUS_OK
    );
    assert_eq!(
        (first.tag, alias.tag),
        (u32::from(!granted), u32::from(!granted))
    );
    if !granted {
        return;
    }
    assert_ne!(first.value, alias.value);
    let narrowed = unsafe {
        issue(
            ctx,
            Request::ProcessNarrow {
                process: first.value,
                rules: vec![ProcessRule {
                    program: "/bin/sh".into(),
                    env: vec![],
                }],
            },
        )
    };
    let Completion::Spawned(Ok(narrowed)) = narrowed else {
        panic!("narrowed process")
    };
    for process in [narrowed, alias.value] {
        assert_eq!(
            unsafe {
                issue(
                    ctx,
                    Request::ProcessRun {
                        process,
                        command: command("/bin/sh"),
                        stdin: vec![],
                        max_output: 100,
                        deadline: None,
                    },
                )
            },
            Completion::Process(ProcessOutcome::Output {
                status: ExitStatus::Exited(0),
                stdout: b"process output".to_vec(),
                stderr: b"process error".to_vec()
            })
        );
    }
    assert_eq!(
        unsafe {
            issue(
                ctx,
                Request::ProcessRun {
                    process: first.value,
                    command: command("/bin/false"),
                    stdin: vec![],
                    max_output: 100,
                    deadline: None,
                },
            )
        },
        Completion::Process(ProcessOutcome::Error(ProcessFailure::NotGranted))
    );
}

#[test]
fn native_process_issue_replays_inherited_sink_domains_without_an_adapter() {
    for width in [1, 8] {
        for durable in [false, true] {
            for granted in [false, true] {
                let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
                real.discard_history();
                let (output, _) = crate::host::SendSink::capture();
                let (error, _) = crate::host::SendSink::capture();
                real.set_send_output(output);
                real.set_send_stderr(error);
                real.set_process_grant(&hirsel_run_schema::ProcessGrant {
                    timed: None,
                    provider: "process.host@1".into(),
                    rules: vec![ProcessRule {
                        program: "/bin/sh".into(),
                        env: vec![],
                    }],
                    runs: None,
                    children: None,
                });
                if !granted {
                    real.set_projections(Default::default());
                }
                let adapter = Arc::new(
                    real.into_concurrent()
                        .unwrap_or_else(|_| panic!("Send process")),
                );
                let mut admitted = Some(adapter.clone());
                let recording =
                    super::super::record::Recording::new(b"native process domains", durable);
                let install = recording.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        {
                            let _role = role::Guard::new(ctx);
                            let _segment = role::Segment::new(ctx);
                            concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                            native::enable(ctx);
                        }
                        program(ctx, granted);
                        STATUS_OK
                    },
                );
                assert_eq!(
                    outcome.ended,
                    Ok(STATUS_OK),
                    "width{width} durable{durable} granted{granted}"
                );
                assert_eq!(outcome.pending, 0);
                assert_eq!(Arc::strong_count(&adapter), 1);
                drop(adapter);
                let cut = recording
                    .recorder
                    .stop("returned", None)
                    .unwrap()
                    .unwrap()
                    .0;
                let projection = hirsel_free::projection::Projection::new(
                    "returned",
                    &cut.progress,
                    &recording.trace(),
                );
                let bytes = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink()
                    .bytes;
                let tape = hirsel_free::tape::parse(&bytes).unwrap();
                let records = hirsel_free::record::decode(&tape.logs[&Vec::new()]).unwrap();
                let creations: Vec<_> = records
                    .iter()
                    .filter_map(|record| {
                        if let hirsel_free::record::Record::Completion { bytes, .. } = record {
                            let envelope: Envelope = crate::tape_wire::decode(bytes).unwrap();
                            envelope.created.into_iter().next()
                        } else {
                            None
                        }
                    })
                    .collect();
                assert_eq!(creations.len(), if granted { 3 } else { 0 });
                if granted {
                    let keys = creations[0].process_keys.as_ref().unwrap();
                    assert!(keys.len() >= 4, "provider, namespace and unprojected sinks");
                    assert!(
                        creations
                            .iter()
                            .all(|created| created.process_keys.as_ref() == Some(keys))
                    );
                }
                for width in [1, 8] {
                    let replay = super::super::replay::Replay::new(&bytes).unwrap();
                    let install = replay.clone();
                    let mut run = crate::host::BodyRun::new(width, false);
                    run.free = true;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        move |ctx| unsafe {
                            install.root(ctx);
                            native::enable(ctx);
                        },
                        move |ctx| unsafe {
                            program(ctx, granted);
                            let _role = role::Guard::new(ctx);
                            super::super::replay::start(ctx);
                            unreachable!("process replay beyond cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(outcome.pending, 0);
                    assert_eq!(replay.finish().unwrap(), projection);
                }
            }
        }
    }
}

#[test]
fn native_process_domains_reject_missing_unsorted_or_changed_creation_metadata() {
    fn envelope(keys: Option<Vec<u32>>) -> Envelope {
        Envelope {
            refusal: None,
            result: Ok(Completion::Handle(Ok(0))),
            output_count: None,
            created: vec![Created {
                path: vec![1],
                ordinal: 7,
                resource: 17,
                kind: crate::io_keys::Kind::Other,
                process_keys: keys,
                net_ports: None,
            }],
        }
    }
    let mut handles = handles::Handles::default();
    for keys in [vec![], vec![0], vec![17], vec![17, 0], vec![0, 17, 17]] {
        assert!(handles.substitute(envelope(Some(keys))).is_err());
    }
    let result = handles
        .substitute(envelope(Some(vec![0, 1, 17, 21])))
        .unwrap();
    let Ok(Completion::Handle(Ok(word))) = result else {
        unreachable!()
    };
    let request = Request::ProcessRun {
        process: word,
        command: command("/bin/sh"),
        stdin: vec![],
        max_output: 100,
        deadline: None,
    };
    assert_eq!(
        native::replay_keys(&handles, &request).unwrap(),
        KeySet::new(vec![
            (crate::io_keys::Key(0), Mode::Exclusive),
            (crate::io_keys::Key(1), Mode::Exclusive),
            (crate::io_keys::Key(17), Mode::Exclusive),
            (crate::io_keys::Key(21), Mode::Exclusive)
        ])
    );
    assert!(handles.substitute(envelope(Some(vec![0, 17, 22]))).is_err());
    assert!(handles.substitute(envelope(None)).is_err());
    assert!(
        native::request_keys(&request, |_| Ok((
            crate::io_keys::Key(17),
            crate::io_keys::Kind::Other
        )))
        .is_err()
    );
    let mut physical = envelope(Some(vec![0, 17, 21]));
    physical.result = Ok(Completion::Handle(Ok(0xfedc_ba98_7654_3210)));
    assert!(handles.substitute(physical).is_err());
    let mut encoded = Vec::new();
    envelope(Some(vec![0, 17, 21])).encode(&mut encoded);
    let decoded: Envelope = crate::tape_wire::decode(&encoded).unwrap();
    assert_eq!(decoded.created[0].process_keys, Some(vec![0, 17, 21]));
    encoded[..4].copy_from_slice(b"F1H2");
    assert!(crate::tape_wire::decode::<Envelope>(&encoded).is_err());
}
