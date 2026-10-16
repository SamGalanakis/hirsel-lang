use hirsel_rt::{io::*, io_real::RealIo, io_sim::SimIo};

fn event(request: Request, completion: Completion) -> Event {
    Event {
        request,
        completion,
        fault: None,
    }
}

#[test]
fn independent_clock_grants_deny_the_other_projection_and_replay() {
    let tape = vec![
        event(
            Request::MonotonicClock { io: 1 },
            Completion::ClockGrant(Ok(2)),
        ),
        event(
            Request::WallClock { io: 1 },
            Completion::ClockGrant(Err(())),
        ),
        event(
            Request::Monotonic { clock: 2 },
            Completion::Monotonic(Ok(0)),
        ),
        event(
            Request::Monotonic { clock: 2 },
            Completion::Monotonic(Ok(u64::MAX)),
        ),
        event(
            Request::Monotonic { clock: 2 },
            Completion::Monotonic(Err(ClockError::OutOfRange)),
        ),
    ];
    let encoded = hirsel_rt::launch::render_tape(&tape).unwrap();
    assert_eq!(hirsel_rt::launch::parse_tape(&encoded).unwrap(), tape);
    let mut replay = SimIo::new(Tree::default(), tape.clone());
    replay.clock_grants(true, false);
    for e in tape {
        assert_eq!(replay.call(e.request).unwrap(), e.completion);
    }
    replay.finish().unwrap();
}

#[test]
fn regressing_monotonic_input_is_refused_before_state_or_tape_position_changes() {
    let tape = vec![
        event(
            Request::MonotonicClock { io: 1 },
            Completion::ClockGrant(Ok(2)),
        ),
        event(
            Request::Monotonic { clock: 2 },
            Completion::Monotonic(Ok(10)),
        ),
        event(
            Request::Monotonic { clock: 2 },
            Completion::Monotonic(Ok(9)),
        ),
    ];
    let mut replay = SimIo::new(Tree::default(), tape.clone());
    replay.clock_grants(true, false);
    for e in &tape[..2] {
        replay.call(e.request.clone()).unwrap();
    }
    let handles = replay.handles().clone();
    assert_eq!(
        replay.call(tape[2].request.clone()),
        Err(AdapterError::InvalidCompletion)
    );
    assert_eq!(replay.position(), 2);
    assert_eq!(replay.handles(), &handles);
    let mut valid = tape;
    valid[2].completion = Completion::Monotonic(Ok(10));
    let mut replay = SimIo::new(Tree::default(), valid.clone());
    replay.clock_grants(true, false);
    for e in valid {
        replay.call(e.request).unwrap();
    }
    replay.finish().unwrap();
}

#[test]
fn wall_time_is_signed_can_reverse_and_records_errors_without_cancellation() {
    let tape = vec![
        event(Request::WallClock { io: 1 }, Completion::ClockGrant(Ok(2))),
        event(
            Request::MonotonicClock { io: 1 },
            Completion::ClockGrant(Err(())),
        ),
        event(Request::Wall { clock: 2 }, Completion::Wall(Ok(i64::MAX))),
        event(Request::Wall { clock: 2 }, Completion::Wall(Ok(i64::MIN))),
        event(
            Request::Wall { clock: 2 },
            Completion::Wall(Err(ClockError::Unavailable)),
        ),
    ];
    assert_eq!(
        hirsel_rt::launch::parse_tape(&hirsel_rt::launch::render_tape(&tape).unwrap()).unwrap(),
        tape
    );
    let mut replay = SimIo::new(Tree::default(), tape.clone());
    replay.clock_grants(false, true);
    for e in tape {
        assert_eq!(replay.call(e.request).unwrap(), e.completion);
    }
    replay.finish().unwrap();
}

#[test]
fn real_clock_inputs_replay_without_reading_the_host_clock() {
    let mut real = RealIo::grant(std::path::Path::new(".")).unwrap();
    real.clock_grants(true, true);
    assert_eq!(
        real.call(Request::MonotonicClock { io: 1 }).unwrap(),
        Completion::ClockGrant(Ok(2))
    );
    assert_eq!(
        real.call(Request::WallClock { io: 1 }).unwrap(),
        Completion::ClockGrant(Ok(3))
    );
    for request in [
        Request::Monotonic { clock: 2 },
        Request::Monotonic { clock: 2 },
        Request::Wall { clock: 3 },
    ] {
        real.call(request).unwrap();
    }
    real.inject(Fault {
        error: IoError::Fault(1),
        partial: 0,
    });
    assert_eq!(
        real.call(Request::Monotonic { clock: 2 }).unwrap(),
        Completion::Monotonic(Err(ClockError::OutOfRange))
    );
    let mut replay = SimIo::new(Tree::default(), real.tape().to_vec());
    replay.clock_grants(true, true);
    for e in real.tape() {
        assert_eq!(replay.call(e.request.clone()).unwrap(), e.completion);
    }
    replay.finish().unwrap();
}

use crate::support;

#[test]
fn native_clock_abi_mints_only_the_sample_word_and_ticks_access_allocates_nothing() {
    let mut replay = SimIo::new(
        Tree::default(),
        vec![
            event(
                Request::MonotonicClock { io: 1 },
                Completion::ClockGrant(Ok(2)),
            ),
            event(
                Request::WallClock { io: 1 },
                Completion::ClockGrant(Err(())),
            ),
            event(
                Request::Monotonic { clock: 2 },
                Completion::Monotonic(Ok(u64::MAX)),
            ),
        ],
    );
    replay.clock_grants(true, false);
    let ctx = support::Ctx::diagnostic();
    let mut binding = Box::new(Binding::new(replay));
    let mut result = ClockResult {
        tag: 99,
        padding: 0,
        value: 0,
    };
    // SAFETY: the stable binding, readable grant words and writable results live
    // throughout each exclusive native call and are unbound before destruction.
    unsafe {
        binding.bind(&mut *ctx.0);
        assert_eq!(
            hirsel_rt_v2_io_monotonic_clock(ctx.0, std::ptr::null_mut(), &mut result, &1),
            0
        );
        assert_eq!(
            result,
            ClockResult {
                tag: 0,
                padding: 0,
                value: 2
            }
        );
        let clock = result.value;
        assert_eq!(
            hirsel_rt_v2_io_wall_clock(ctx.0, std::ptr::null_mut(), &mut result, &1),
            0
        );
        assert_eq!(
            result,
            ClockResult {
                tag: 1,
                padding: 0,
                value: 0
            }
        );
        assert_eq!(
            hirsel_rt_v2_clock_monotonic(ctx.0, std::ptr::null_mut(), &mut result, &clock),
            0
        );
        assert_eq!(
            result,
            ClockResult {
                tag: 0,
                padding: 0,
                value: u64::MAX
            }
        );
        let allocated = support::host_calls();
        let mut ticks = 0;
        assert_eq!(
            hirsel_rt_v2_instant_ticks(ctx.0, &mut ticks, &result.value),
            0
        );
        assert_eq!(ticks, u64::MAX);
        assert_eq!(support::host_calls(), allocated);
        (*ctx.0).bind_io_data(std::ptr::null_mut());
    }
}
