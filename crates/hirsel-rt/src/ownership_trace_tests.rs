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

#[test]
fn task_end_records_actual_free_children_including_lent_fibers() {
    use crate::tasks::{
        hirsel_rt_v2_task_finish, hirsel_rt_v2_task_join, hirsel_rt_v2_task_scope_enter,
        hirsel_rt_v2_task_scope_exit, hirsel_rt_v2_task_spawn,
    };
    const NONE: [u64; 2] = [0; 2];
    unsafe extern "C" fn child(ctx: *mut TaskCtx, _: *mut crate::Header, _: u64, _: u64) -> u8 {
        unsafe {
            let frame = hirsel_rt_v2_ownership_enter(ctx, ABI_VERSION, [7; 32].as_ptr());
            let mut result = [1u64, 42];
            assert_eq!(
                hirsel_rt_v2_task_finish(
                    ctx,
                    result.as_mut_ptr().cast(),
                    NONE.as_ptr().cast(),
                    16,
                    u32::MAX
                ),
                crate::STATUS_OK
            );
            assert_eq!(result, NONE);
            hirsel_rt_v2_ownership_leave(ctx, frame);
        }
        crate::STATUS_OK
    }
    let descriptor = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 0,
        align: 8,
        finalizer: None,
        steps: std::ptr::null(),
        step_count: 0,
        field_count: 0,
        flags: 0,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: 0,
    };
    let layouts = [&descriptor as *const crate::Layout];
    for flags in [0u64, 1, 2] {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("trace.json");
        let output = destination.clone();
        let mut run = crate::host::BodyRun::new(2, true);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                start(ctx, [49; 32], output, 1000).unwrap();
                crate::tasks::free::enable(ctx);
            },
            move |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    0
                );
                for _ in 0..8 {
                    let environment =
                        crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 0, 8, crate::DOMAIN_HEAP, 0);
                    let mut task = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut task,
                            &scope,
                            child,
                            environment,
                            flags
                        ),
                        0
                    );
                    let mut result = NONE;
                    assert_eq!(
                        hirsel_rt_v2_task_join(
                            ctx,
                            0,
                            0,
                            &scope,
                            &mut task,
                            result.as_mut_ptr().cast(),
                            16
                        ),
                        0
                    );
                    assert_eq!(result, [1, 42]);
                }
                assert_eq!(hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0), 0);
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        let log: Log = serde_json::from_slice(&std::fs::read(destination).unwrap()).unwrap();
        assert!(log.complete);
        let children = log
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Task { task, .. } if *task != 0 => Some(*task),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let ended = log
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::TaskEnd { task } => Some(*task),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(children.len(), 8);
        assert_eq!(ended.len(), 8, "flags={flags}");
        assert_eq!(
            ended.into_iter().collect::<BTreeSet<_>>(),
            children,
            "flags={flags}"
        );
    }
}
