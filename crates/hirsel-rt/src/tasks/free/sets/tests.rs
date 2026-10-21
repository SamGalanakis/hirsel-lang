use super::*;
use crate::tasks::free::record;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::session::Session;
use std::sync::Arc;

unsafe extern "C" fn result(ctx: *mut TaskCtx, environment: *mut Header, _: u64, _: u64) -> u8 {
    let id = unsafe { environment.add(1).cast::<u64>().read() };
    let mut image = [1u64, id];
    let none = [0u64; 2];
    let status = unsafe {
        hirsel_rt_v2_task_finish(ctx, image.as_mut_ptr().cast(), none.as_ptr().cast(), 16)
    };
    assert_eq!(image, none);
    status
}

#[test]
fn late_set_creation_and_reverse_insertion_preserve_scope_completion_ranks() {
    for durable in [false, true] {
        let recording = record::Recording::new(b"native late task set", durable);
        let install = recording.clone();
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 8,
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
        let layouts = [&raw const layout];
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            |ctx| unsafe {
                let mut scope = 0;
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                let mut children = [0u64; 2];
                for (id, child) in children.iter_mut().enumerate() {
                    let environment = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    environment.add(1).cast::<u64>().write(id as u64);
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        child,
                        &scope,
                        result,
                        environment,
                        0,
                    );
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    loop {
                        let held = {
                            let _role = role::Guard::new(ctx);
                            let s = scheduler(ctx);
                            s.task(*child as u32).state == State::Holding
                                && readiness::saved(s, *child as u32)
                        };
                        if held {
                            break;
                        }
                        assert!(
                            std::time::Instant::now() < until,
                            "the child must complete before set creation"
                        );
                        std::thread::yield_now();
                    }
                }
                let mut set = 0;
                hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope);
                hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut children[1]);
                hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut children[0]);
                let mut index = IndexOption {
                    tag: 99,
                    padding: 99,
                    index: 99,
                };
                hirsel_rt_v2_task_await_any(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut index,
                    &scope,
                    &set,
                );
                assert_eq!((index.tag, index.index), (1, 1));
                let mut has = 99;
                hirsel_rt_v2_task_set_has(ctx, &mut has, &scope, &set, 1);
                assert_eq!(has, 1);
                let mut task = 0;
                hirsel_rt_v2_task_set_take(
                    ctx,
                    std::ptr::null_mut(),
                    &mut task,
                    &scope,
                    &mut set,
                    1,
                );
                let mut image = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut task,
                        image.as_mut_ptr().cast(),
                        16
                    ),
                    STATUS_OK
                );
                assert_eq!(image, [1, 0]);
                hirsel_rt_v2_task_set_has(ctx, &mut has, &scope, &set, 1);
                assert_eq!(has, 0);
                hirsel_rt_v2_task_await_any(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut index,
                    &scope,
                    &set,
                );
                assert_eq!((index.tag, index.index), (1, 0));
                hirsel_rt_v2_task_set_take(
                    ctx,
                    std::ptr::null_mut(),
                    &mut task,
                    &scope,
                    &mut set,
                    0,
                );
                image = [0, 0];
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut task,
                        image.as_mut_ptr().cast(),
                        16
                    ),
                    STATUS_OK
                );
                assert_eq!(image, [1, 1]);
                hirsel_rt_v2_task_await_any(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut index,
                    &scope,
                    &set,
                );
                assert_eq!((index.tag, index.index), (0, 0));
                {
                    let _role = role::Guard::new(ctx);
                    assert_eq!(close_endpoint(ctx, set, 0, 0), STATUS_OK);
                }
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
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
        assert_eq!(cut.progress[&vec![]], Progress::new(17, Phase::Done));
        let trace = recording.trace();
        let marked = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink();
        let mut replay = Session::replay(&marked.bytes).unwrap();
        assert!(replay.start(&[]));
        for path in [&[0][..], &[1][..]] {
            assert!(replay.start(path));
            replay.begin(path);
            replay.publish(path, Phase::Entered).unwrap();
            replay.publish(path, Phase::Done).unwrap();
        }
        for op in 1..=17 {
            replay.begin(&[]);
            if let Some(index) = match op {
                7 => Some(1),
                12 => Some(0),
                _ => None,
            } {
                assert_eq!(replay.await_any(&[], None, |_| true).unwrap(), Some(index));
            }
            if [10, 14, 17].contains(&op) {
                replay.publish(&[], Phase::Entered).unwrap();
            }
            replay.publish(&[], Phase::Done).unwrap();
        }
        assert_eq!(
            replay.finish().unwrap(),
            Projection::new("returned", &cut.progress, &trace)
        );
    }
}
