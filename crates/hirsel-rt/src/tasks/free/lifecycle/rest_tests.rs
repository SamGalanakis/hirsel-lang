use super::*;
use crate::tasks::free::record;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::session::Session;
use std::sync::Arc;

unsafe extern "C" fn result(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let [root, fail] = unsafe { crate::alloc::payload(env).cast::<[u64; 2]>().read() };
    let mut image = [1u64, 42];
    let none = [0u64; 2];
    unsafe { hirsel_rt_v2_task_finish(ctx, image.as_mut_ptr().cast(), none.as_ptr().cast(), 16) };
    assert_eq!(image, none);
    unsafe { crate::roots::hirsel_rt_v2_root_release(ctx, root as *mut Header) };
    assert!(
        unsafe { (*ctx).roots.has_pending() },
        "a returned rest keeps its root release for its own completion entry"
    );
    if fail != 0 {
        unsafe {
            (*ctx).failure.record(crate::failure::FailureRecord {
                kind: 1,
                rule: 1,
                site: 0,
                operation_site: 0,
            })
        };
        STATUS_FAILURE
    } else {
        STATUS_OK
    }
}

#[test]
fn returned_native_rests_publish_root_cleanup_and_failure_before_join_done() {
    for width in [2, 8] {
        for durable in [false, true] {
            for fail in [false, true] {
                let recording = record::Recording::new(b"native returned rest", durable);
                let install = recording.clone();
                let scalar = crate::Layout {
                    magic: crate::LAYOUT_MAGIC,
                    version: crate::LAYOUT_VERSION,
                    size: 8,
                    align: 8,
                    finalizer: None,
                    steps: std::ptr::null(),
                    step_count: 0,
                    field_count: 1,
                    flags: crate::LAYOUT_FROZEN,
                    logical: 0,
                    logical_count: std::ptr::null(),
                    scan: std::ptr::null(),
                    scan_words: 0,
                };
                let environment = crate::Layout {
                    size: 16,
                    flags: 0,
                    ..scalar
                };
                let layouts = [&raw const scalar, &raw const environment];
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 2;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                        let root = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(root).cast::<u64>().write(7);
                        crate::roots::hirsel_rt_v2_root_dup(ctx, root);
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            3,
                            16,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env)
                            .cast::<[u64; 2]>()
                            .write([root as u64, u64::from(fail)]);
                        let mut child = 0;
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut child,
                            &scope,
                            result,
                            env,
                            0,
                        );
                        let mut image = [0u64; 2];
                        assert_eq!(
                            hirsel_rt_v2_task_join(
                                ctx,
                                0,
                                0,
                                &scope,
                                &mut child,
                                image.as_mut_ptr().cast(),
                                16
                            ),
                            u8::from(fail)
                        );
                        assert_eq!(image, [1, 42]);
                        let chain = (*ctx).failure.take_chain(&[]);
                        assert_eq!(chain.len(), usize::from(fail));
                        if fail {
                            assert_eq!(chain[0].1, [0]);
                        }
                        assert_eq!(crate::roots::count(&*ctx, root), 1);
                        let mut last = 99;
                        crate::shared::hirsel_rt_v2_thaw_turn(ctx, &mut last, root);
                        assert_eq!(last, 1);
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                        crate::roots::hirsel_rt_v2_root_release(ctx, root);
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let cut = recording
                    .recorder
                    .stop("returned", None)
                    .unwrap()
                    .unwrap()
                    .0;
                assert_eq!(cut.progress[&vec![0]], Progress::new(2, Phase::Done));
                assert_eq!(cut.progress[&Vec::new()], Progress::new(5, Phase::Done));
                let trace = recording.trace();
                let marked = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink();
                let mut replay = Session::replay(&marked.bytes).unwrap();
                assert!(replay.start(&[]));
                assert!(replay.start(&[0]));
                for _ in 0..2 {
                    replay.begin(&[0]);
                    replay.publish(&[0], Phase::Entered).unwrap();
                    replay.publish(&[0], Phase::Done).unwrap();
                }
                for op in 1..=5 {
                    replay.begin(&[]);
                    if [3, 5].contains(&op) {
                        replay.publish(&[], Phase::Entered).unwrap();
                    }
                    if op == 4 {
                        assert!(replay.thaw(&[], true).unwrap());
                    }
                    replay.publish(&[], Phase::Done).unwrap();
                }
                assert_eq!(
                    replay.finish().unwrap(),
                    Projection::new("returned", &cut.progress, &trace)
                );
            }
        }
    }
}
