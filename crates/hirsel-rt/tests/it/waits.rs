//! Random draws and timer waits through the typed protocol: providers, tape forms,
//! replay validation and the cooperative Readiness timer.
use hirsel_rt::{io::*, io_real::RealIo, io_sim::SimIo, readiness::Readiness};
use std::time::{Duration, Instant};

fn granted() -> RealIo {
    let mut real = RealIo::grant(std::path::Path::new(".")).unwrap();
    real.clock_grants(true, false);
    real
}
fn replay(tape: &[Event]) -> SimIo {
    let mut replay = SimIo::new(Tree::default(), tape.to_vec());
    replay.clock_grants(true, false);
    replay
}
fn draw(io: &mut impl Adapter, random: u64, count: u64) -> Result<Vec<u8>, RandomError> {
    match io.call(Request::RandomBytes { random, count }).unwrap() {
        Completion::Random(value) => value,
        other => panic!("{other:?}"),
    }
}
fn ticks(io: &mut impl Adapter, clock: u64) -> u64 {
    match io.call(Request::Monotonic { clock }).unwrap() {
        Completion::Monotonic(Ok(ticks)) => ticks,
        other => panic!("{other:?}"),
    }
}

#[test]
fn splitmix_draws_are_the_seeded_words_and_replay_with_their_tape_forms() {
    let mut real = granted();
    real.set_random(hirsel_run_config::ProviderContract::RandomSplitMix64, 42);
    assert_eq!(
        real.call(Request::Random { io: 1 }).unwrap(),
        Completion::Handle(Ok(2))
    );
    // SplitMix64(42)'s first two outputs, little-endian; the second is cut to five bytes.
    let first = 0xbdd732262feb6e95u64.to_le_bytes();
    let second = 0x28efe333b266f103u64.to_le_bytes();
    assert_eq!(
        draw(&mut real, 2, 13).unwrap(),
        [&first[..], &second[..5]].concat()
    );
    // The cut word is consumed: the next draw starts at the third output.
    let third = draw(&mut real, 2, 8).unwrap();
    assert_ne!(third, second);
    let tape = real.tape().to_vec();
    let text = hirsel_rt::launch::render_tape(&tape).unwrap();
    assert!(text.contains("random_bytes 2 13"), "{text}");
    assert_eq!(hirsel_rt::launch::parse_tape(&text).unwrap(), tape);
    let mut sim = replay(&tape);
    for e in &tape {
        assert_eq!(sim.call(e.request.clone()).unwrap(), e.completion);
    }
    sim.finish().unwrap();
}

#[test]
fn system_draws_fill_the_count_from_the_host_and_replay_only_the_recorded_bytes() {
    let mut real = granted();
    real.set_random(hirsel_run_config::ProviderContract::RandomSystem, 0);
    real.call(Request::Random { io: 1 }).unwrap();
    let a = draw(&mut real, 2, 64).unwrap();
    let b = draw(&mut real, 2, 64).unwrap();
    assert_eq!((a.len(), b.len()), (64, 64));
    // 2^-512: two equal 64-byte host draws mean the provider is not reading entropy.
    assert_ne!(a, b);
    assert_eq!(draw(&mut real, 2, 1 << 20).unwrap().len(), 1 << 20);
    real.inject(Fault {
        error: IoError::Fault(0),
        partial: 0,
    });
    assert_eq!(draw(&mut real, 2, 4), Err(RandomError::Unavailable));
    let tape = real.tape().to_vec();
    let mut sim = replay(&tape);
    for e in &tape {
        assert_eq!(sim.call(e.request.clone()).unwrap(), e.completion);
    }
    // A recorded draw of the wrong length, or one for a zero count, contradicts its request.
    for (count, bytes) in [(4, vec![1, 2, 3]), (0, vec![])] {
        let bad = vec![
            tape[0].clone(),
            Event {
                request: Request::RandomBytes { random: 2, count },
                completion: Completion::Random(Ok(bytes)),
                fault: None,
            },
        ];
        let mut sim = replay(&bad);
        sim.call(bad[0].request.clone()).unwrap();
        assert_eq!(
            sim.call(bad[1].request.clone()),
            Err(AdapterError::InvalidCompletion)
        );
        assert_eq!(sim.position(), 1);
    }
}

#[test]
fn a_blocking_wait_reaches_its_deadline_and_replay_never_waits() {
    let mut real = granted();
    real.call(Request::MonotonicClock { io: 1 }).unwrap();
    let start = ticks(&mut real, 2);
    // An elapsed deadline returns at once.
    assert_eq!(
        real.call(Request::SleepUntil {
            clock: 2,
            deadline: start
        })
        .unwrap(),
        Completion::Wait(Ok(()))
    );
    let deadline = start + 20_000_000;
    let begun = Instant::now();
    assert_eq!(
        real.call(Request::SleepUntil { clock: 2, deadline })
            .unwrap(),
        Completion::Wait(Ok(()))
    );
    assert!(begun.elapsed() >= Duration::from_millis(19));
    assert!(ticks(&mut real, 2) >= deadline);
    let tape = real.tape().to_vec();
    let text = hirsel_rt::launch::render_tape(&tape).unwrap();
    assert!(
        text.contains(&format!("sleep_until 2 {deadline}")),
        "{text}"
    );
    assert!(text.contains("wait ok"), "{text}");
    assert_eq!(hirsel_rt::launch::parse_tape(&text).unwrap(), tape);
    let begun = Instant::now();
    let mut sim = replay(&tape);
    for e in &tape {
        assert_eq!(sim.call(e.request.clone()).unwrap(), e.completion);
    }
    sim.finish().unwrap();
    assert!(begun.elapsed() < Duration::from_millis(19));
    // A completed wait proves the clock reached its deadline: an earlier recorded
    // sample afterwards is refused before the replay position moves.
    let mut bad = tape.clone();
    let last = bad.len() - 1;
    bad[last].completion = Completion::Monotonic(Ok(deadline - 1));
    let mut sim = replay(&bad);
    for e in &bad[..last] {
        sim.call(e.request.clone()).unwrap();
    }
    assert_eq!(
        sim.call(bad[last].request.clone()),
        Err(AdapterError::InvalidCompletion)
    );
    assert_eq!(sim.position(), last);
}

#[test]
fn a_cooperative_wait_parks_on_a_readiness_timer_until_due() {
    let mut real = granted();
    real.set_cooperative(Readiness::default());
    real.call(Request::MonotonicClock { io: 1 }).unwrap();
    let start = ticks(&mut real, 2);
    let deadline = start + 20_000_000;
    real.enter(7);
    let parked = real.tape().len();
    assert_eq!(
        real.call(Request::SleepUntil { clock: 2, deadline }),
        Err(AdapterError::Blocked)
    );
    // The park is recorded; the wait has not completed.
    assert_eq!(
        real.tape()[parked].completion,
        Completion::Blocked { committed: false }
    );
    assert_eq!(real.tape().len(), parked + 1);
    assert!(!real.ready(7).unwrap());
    // With no runnable task the scheduler sleeps until the earliest timer.
    let begun = Instant::now();
    assert!(real.wait().unwrap());
    assert!(begun.elapsed() >= Duration::from_millis(15));
    while !real.ready(7).unwrap() {
        assert!(real.wait().unwrap());
    }
    // The ready poll is recorded, then the reissued wait completes.
    assert!(matches!(
        real.tape().last().unwrap().request,
        Request::Ready { .. }
    ));
    assert_eq!(
        real.call(Request::SleepUntil { clock: 2, deadline }),
        Ok(Completion::Wait(Ok(())))
    );
    real.finish(7);
    // The completed timer left no interest behind.
    assert!(!real.wait().unwrap());
    // Cancellation finishes a parked token, which removes its timer.
    real.enter(8);
    assert_eq!(
        real.call(Request::SleepUntil {
            clock: 2,
            deadline: deadline + 3_600_000_000_000
        }),
        Err(AdapterError::Blocked)
    );
    real.finish(8);
    assert!(!real.wait().unwrap());
}

#[test]
fn a_draw_or_wait_on_a_foreign_handle_is_refused_before_any_provider_effect() {
    let mut real = granted();
    real.set_random(hirsel_run_config::ProviderContract::RandomSplitMix64, 1);
    real.call(Request::MonotonicClock { io: 1 }).unwrap();
    assert_eq!(
        real.call(Request::RandomBytes {
            random: 2,
            count: 8
        }),
        Err(AdapterError::InvalidHandle(2))
    );
    real.call(Request::Random { io: 1 }).unwrap();
    assert_eq!(
        real.call(Request::SleepUntil {
            clock: 3,
            deadline: 0
        }),
        Err(AdapterError::InvalidHandle(3))
    );
    // The refused draw did not advance the seeded state.
    let fresh = {
        let mut other = granted();
        other.set_random(hirsel_run_config::ProviderContract::RandomSplitMix64, 1);
        other.call(Request::Random { io: 1 }).unwrap();
        draw(&mut other, 2, 8).unwrap()
    };
    assert_eq!(draw(&mut real, 3, 8).unwrap(), fresh);
}
