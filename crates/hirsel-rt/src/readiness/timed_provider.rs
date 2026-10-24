use crate::io::{Adapter, AdapterError, Completion, Request};
use crate::io_real::RealIo;
use crate::process::{ChildStream, Command, Deadline, ExitStatus, ProcessFailure};
use hirsel_run_schema::{ChildRun, ChildStep, ProcessGrant, encode_child_runs};

fn provider(steps: &str, now: u64) -> RealIo {
    let grant: ProcessGrant = serde_json::from_str(&format!(r#"{{"provider":"process.script.timed@1","clock":"run.monotonic","rules":[{{"program":"/bin/cat"}}],"runs":[],"children":[{{"program":"/bin/cat","args":[],"steps":{steps}}}]}}"#)).unwrap();
    assert_eq!(grant.provider, "process.script.timed@1");
    assert_eq!(grant.timed.as_ref().unwrap().clock, "run.monotonic");
    let mut io = RealIo::fixture(
        Default::default(),
        Default::default(),
        vec![],
        std::io::sink(),
    )
    .unwrap();
    io.virtual_clock(now);
    io.set_process_grant(&grant);
    io
}

fn spawned(io: &mut RealIo) -> (u64, u64, u64) {
    let Completion::Handle(Ok(process)) = io.call(Request::Process { io: 1 }).unwrap() else {
        panic!("process grant")
    };
    let Completion::ClockGrant(Ok(clock)) = io.call(Request::MonotonicClock { io: 1 }).unwrap()
    else {
        panic!("clock grant")
    };
    let Completion::Spawned(Ok(child)) = io
        .call(Request::ProcessSpawn {
            process,
            command: Command {
                program: "/bin/cat".into(),
                args: vec![],
                directory: None,
                env: None,
                own_group: false,
            },
        })
        .unwrap()
    else {
        panic!("scripted child")
    };
    (process, child, clock)
}

fn ticks(io: &mut RealIo, clock: u64) -> u64 {
    let Completion::Monotonic(Ok(ticks)) = io.call(Request::Monotonic { clock }).unwrap() else {
        panic!("virtual ticks")
    };
    ticks
}

#[test]
fn runtime50_timed_provider_contract() {
    // Starting at 100 makes an At(105) absolute gate distinguishable from a
    // relative delay. Readiness probes leave time unchanged while work runs.
    let mut io = provider(r#"[{"at":105},{"print":[120]},{"exit":{"exited":7}}]"#, 100);
    let (process, child, clock) = spawned(&mut io);
    let read = Request::ChildRead {
        process,
        child,
        stream: ChildStream::Stdout,
        max: 10,
    };
    io.enter(17);
    assert_eq!(io.call(read.clone()), Err(AdapterError::Blocked));
    io.parked(17);
    io.ready().unwrap();
    assert_eq!(ticks(&mut io, clock), 100);
    assert!(io.wait().unwrap());
    io.ready().unwrap();
    io.process_batch(&[]).unwrap();
    assert_eq!(ticks(&mut io, clock), 105);
    io.enter(17);
    assert_eq!(io.call(read), Ok(Completion::ChildBytes(Ok(b"x".to_vec()))));
    io.finish(17);
    assert_eq!(
        io.call(Request::ChildWait {
            process,
            child,
            deadline: None
        }),
        Ok(Completion::ChildExit(Ok(ExitStatus::Exited(7))))
    );

    // At equality the timeout wins over the child gate. Cancellation wins
    // over both, including a cancellation issued before any clock advance.
    for cancel in [false, true] {
        let mut io = provider(r#"[{"at":105},{"print":[120]},{"exit":{"exited":7}}]"#, 100);
        let (process, child, clock) = spawned(&mut io);
        let wait = Request::ChildWait {
            process,
            child,
            deadline: Some(Deadline { clock, ticks: 105 }),
        };
        io.enter(23);
        assert_eq!(io.call(wait.clone()), Err(AdapterError::Blocked));
        io.parked(23);
        assert!(io.wait().unwrap());
        io.ready().unwrap();
        let canceled = [child];
        io.process_batch(if cancel { &canceled } else { &[] })
            .unwrap();
        let result = if cancel {
            crate::io_checks::cancel(&mut io, &wait, 2)
                .unwrap()
                .unwrap()
        } else {
            io.call(wait).unwrap()
        };
        assert_eq!(
            result,
            Completion::ChildExit(Err(if cancel {
                ProcessFailure::Canceled(2)
            } else {
                ProcessFailure::TimedOut
            }))
        );
        assert!(io.stdout().is_empty());
        io.finish(23);
        assert!(!io.process_sources());
    }
    let mut io = provider(r#"[{"at":105},{"exit":{"exited":7}}]"#, 100);
    let (process, child, clock) = spawned(&mut io);
    let wait = Request::ChildWait {
        process,
        child,
        deadline: Some(Deadline { clock, ticks: 105 }),
    };
    assert_eq!(
        crate::io_checks::cancel(&mut io, &wait, 2).unwrap(),
        Some(Completion::ChildExit(Err(ProcessFailure::Canceled(2))))
    );
    assert_eq!(ticks(&mut io, clock), 100);
    assert!(!io.process_sources());
    assert!(
        !io.tape()
            .iter()
            .any(|event| matches!(event.request, Request::ProcessTime { .. }))
    );

    // Exact legacy script bytes are a control independent of the timed wire.
    let untimed = ChildRun {
        program: "/bin/cat".into(),
        args: vec![],
        steps: vec![
            ChildStep::Print(b"x".to_vec()),
            ChildStep::Exit(ExitStatus::Exited(0)),
        ],
    };
    assert_eq!(
        encode_child_runs(&[untimed]),
        br#"[{"program":"/bin/cat","args":[],"steps":[{"print":[120]},{"exit":{"exited":0}}]}]"#
    );
}
