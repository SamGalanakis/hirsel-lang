//! H checks a clocked program (`hirsel-heapcheck/src/exec/clock.rs`): its clean run under
//! virtual time observes what the interpreter did, and every run that fires a deadline early
//! at one of its timer choice points passes the terminal audit.
use hirsel_driver::native::{
    HeapWorld, Observation, Options, Sites, TargetSpec, build, heap_campaign_in,
};
use hirsel_engine::*;

/// The timer choice points of the clean run, after checking `engine`'s program with a
/// monotonic clock that starts at `monotonic` against the interpreter's `expected`.
pub fn check(engine: &mut QueryEngine, expected: &Observation, monotonic: u64) -> u64 {
    let entry = entry_path(&default_context());
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let program = build(
        &validated,
        &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
    )
    .unwrap();
    let world = HeapWorld {
        monotonic: Some(monotonic),
    };
    let run = heap_campaign_in(&program, &entry, &Sites::new(validated.snapshot()), &world)
        .unwrap_or_else(|e| panic!("{}", e.message));
    assert_eq!(run.observation, *expected);
    run.choices
}
