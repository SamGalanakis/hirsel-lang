//! `io.signals` and `signal.wait` through the typed protocol: program checks, scripted
//! delivery at idle points, tape forms and replay without a signal.
use hirsel_rt::{io::*, io_real::RealIo, io_sim::SimIo};
use hirsel_run_schema::{SignalKind, SignalsGrant};

fn scripted(kinds: &[SignalKind], script: &[SignalKind]) -> RealIo {
    let mut real = RealIo::grant(std::path::Path::new(".")).unwrap();
    real.set_signal_grant(&SignalsGrant {
        provider: "signals.script@1".into(),
        kinds: kinds.to_vec(),
        script: Some(script.to_vec()),
    })
    .unwrap();
    real
}
fn wait(
    io: &mut impl Adapter,
    signals: u64,
    kinds: &[SignalKind],
) -> Result<Completion, AdapterError> {
    let kinds = kinds.iter().fold(0, |bits, k| bits | k.bit());
    io.call(Request::SignalWait { signals, kinds })
}

#[test]
fn scripted_arrivals_complete_waits_at_idle_points_and_replay_from_their_tape_forms() {
    use SignalKind::*;
    let mut real = scripted(&[Interrupt, Terminate], &[Terminate, Hangup]);
    assert_eq!(
        real.call(Request::Signals { io: 1 }).unwrap(),
        Completion::Handle(Ok(2))
    );
    // An empty set, then a kind outside the grant, before anything waits.
    assert_eq!(
        wait(&mut real, 2, &[]).unwrap(),
        Completion::Signal(Err(SignalError::InvalidInput))
    );
    assert_eq!(
        wait(&mut real, 2, &[Interrupt, Hangup]).unwrap(),
        Completion::Signal(Err(SignalError::NotGranted))
    );
    // Nothing is pending: the wait cannot complete until an idle point delivers.
    assert_eq!(wait(&mut real, 2, &[Terminate]), Err(AdapterError::Blocked));
    assert_eq!(real.wait(), Ok(true));
    assert_eq!(
        wait(&mut real, 2, &[Interrupt, Terminate]).unwrap(),
        Completion::Signal(Ok(Terminate))
    );
    // Hangup is not observed: its default action ends the run.
    assert_eq!(real.wait(), Err(AdapterError::Signaled(Hangup)));
    let tape: Vec<Event> = real
        .tape()
        .iter()
        .filter(|e| !matches!(e.completion, Completion::Blocked { .. }))
        .cloned()
        .collect();
    let text = hirsel_rt::launch::render_tape(&tape).unwrap();
    for form in [
        "signals 1",
        "signal_wait 2 0",
        "signal invalid_input",
        "signal not_granted",
        "signal terminate",
    ] {
        assert!(text.contains(form), "{form}: {text}");
    }
    assert_eq!(hirsel_rt::launch::parse_tape(&text).unwrap(), tape);
    let mut sim = SimIo::new(Tree::default(), tape.clone());
    sim.set_signal_kinds(Interrupt.bit() | Terminate.bit());
    for e in &tape {
        assert_eq!(sim.call(e.request.clone()).unwrap(), e.completion);
    }
    sim.finish().unwrap();
}

#[test]
fn replay_refuses_a_completion_that_contradicts_the_grant_or_the_waited_kinds() {
    use SignalKind::*;
    let project = Event {
        request: Request::Signals { io: 1 },
        completion: Completion::Handle(Ok(2)),
        fault: None,
    };
    for (kinds, completion) in [
        // A kind the wait did not name.
        (Interrupt.bit(), Completion::Signal(Ok(Terminate))),
        // A program error the request does not have.
        (
            Interrupt.bit(),
            Completion::Signal(Err(SignalError::InvalidInput)),
        ),
        (
            Interrupt.bit(),
            Completion::Signal(Err(SignalError::NotGranted)),
        ),
    ] {
        let tape = vec![
            project.clone(),
            Event {
                request: Request::SignalWait { signals: 2, kinds },
                completion,
                fault: None,
            },
        ];
        let mut sim = SimIo::new(Tree::default(), tape.clone());
        sim.set_signal_kinds(Interrupt.bit() | Terminate.bit());
        sim.call(tape[0].request.clone()).unwrap();
        assert_eq!(
            sim.call(tape[1].request.clone()),
            Err(AdapterError::InvalidCompletion),
            "{:?}",
            tape[1]
        );
    }
}

#[test]
fn a_run_without_the_grant_cannot_project_signals() {
    let mut real = RealIo::grant(std::path::Path::new(".")).unwrap();
    assert!(matches!(
        real.call(Request::Signals { io: 1 }).unwrap(),
        Completion::Handle(Err(_))
    ));
}
