use super::*;
use crate::tasks;
use std::collections::BTreeSet;

fn atomic_body(ctx: *mut TaskCtx, ordered: bool) -> u8 {
    unsafe {
        let frame = hirsel_rt_v2_ownership_enter(ctx, ABI_VERSION, [7; 32].as_ptr());
        hirsel_rt_v2_ownership_event(ctx, frame, 0, 0, 0, 0, 42, 7);
        let mut word = 0;
        let initial = 7u64;
        assert_eq!(
            tasks::hirsel_rt_v2_atomic_create(
                ctx,
                std::ptr::null_mut(),
                &mut word,
                (&raw const initial).cast(),
                8,
                0,
                8
            ),
            crate::STATUS_OK
        );
        hirsel_rt_v2_ownership_event(ctx, frame, 1, 1, 0, 1, 43, 7);
        let changed = 8u64;
        assert_eq!(
            tasks::hirsel_rt_v2_atomic_store(ctx, &word, (&raw const changed).cast(), 8),
            crate::STATUS_OK
        );
        hirsel_rt_v2_ownership_event(ctx, frame, 2, 2, 1, 1, 44, 7);
        let mut result = 0u64;
        assert_eq!(
            tasks::hirsel_rt_v2_atomic_load(ctx, &word, (&raw mut result).cast(), 8),
            crate::STATUS_OK
        );
        assert_eq!(result, changed);
        hirsel_rt_v2_ownership_leave(ctx, frame);
        if ordered {
            crate::stop::check(ctx, tasks::ordered::engine::root_end(ctx, crate::STATUS_OK));
        }
    }
    crate::STATUS_OK
}

#[test]
fn ownership_operation_stamps_match_live_ordered_retirements_and_free_recording() {
    let directory = tempfile::tempdir().unwrap();
    for ordered in [true, false] {
        let destination = directory
            .path()
            .join(if ordered { "ordered.json" } else { "free.json" });
        let output = destination.clone();
        let mut run = crate::host::BodyRun::new(1, false);
        run.free = !ordered;
        unsafe { run.use_physical_dispatch() };
        let expected = if ordered {
            let (outcome, canonical) = run.run_traced(
                move |ctx| unsafe {
                    start(ctx, [49; 32], output, 100).unwrap();
                    tasks::ordered::engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                    tasks::ordered::events::enable(ctx);
                },
                |ctx| atomic_body(ctx, true),
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            canonical
                .unwrap()
                .bytes()
                .split(|byte| *byte == b'\n')
                .filter(|row| !row.is_empty())
                .map(|row| {
                    serde_json::from_slice::<serde_json::Value>(row).unwrap()["ordinal"]
                        .as_u64()
                        .unwrap()
                })
                .collect::<BTreeSet<_>>()
        } else {
            let recording =
                tasks::free::record::Recording::new(b"ownership operation stamps", false);
            let install = recording.clone();
            let outcome = run.run(
                move |ctx| unsafe {
                    start(ctx, [49; 32], output, 100).unwrap();
                    tasks::free::enable(ctx);
                    install.root(ctx);
                },
                |ctx| atomic_body(ctx, false),
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            // Free compresses event-free operation prefixes. Its completed
            // progress is the authority for every ordinal, including gaps.
            let cut = recording.recorder.flush().unwrap().unwrap();
            let completed = cut.progress[&Vec::new()].op;
            assert_eq!(completed, 3);
            (1..=completed).collect::<BTreeSet<_>>()
        };
        let log: Log = serde_json::from_slice(&std::fs::read(destination).unwrap()).unwrap();
        assert!(log.complete);
        let actual = log
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Operation { task: 0, ordinal } => Some(*ordinal),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(actual.len() >= 3);
        assert_eq!(actual, expected.into_iter().collect::<Vec<_>>());
    }
}
