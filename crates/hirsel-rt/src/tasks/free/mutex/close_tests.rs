use super::*;
use crate::tasks::free::record;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::session::Session;
use std::sync::Arc;

unsafe extern "C" fn destroy(ctx: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    let [tag, raw] = unsafe { value.cast::<[u64; 2]>().read() };
    assert_eq!(tag, 1);
    let recording = unsafe { Box::from_raw(raw as *mut Arc<record::Recording>) };
    let cut = recording.recorder.flush().unwrap().unwrap();
    assert_eq!(
        cut.progress[&Vec::new()],
        Progress::new(2, Phase::Done),
        "close is published before user cleanup"
    );
    assert_eq!(unsafe { crate::finalize::hirsel_rt_v2_shielded(ctx) }, 1);
    for _ in 0..130 {
        let _role = unsafe { role::Guard::new(ctx) };
        unsafe { try_yield(&mut *ctx).unwrap() };
    }
    let mut atomic = 0;
    let input = 77u64;
    let mut output = 0u64;
    unsafe {
        hirsel_rt_v2_atomic_create(
            ctx,
            std::ptr::null_mut(),
            &mut atomic,
            (&raw const input).cast(),
            8,
            0,
            8,
        );
        hirsel_rt_v2_atomic_load(ctx, &atomic, (&raw mut output).cast(), 8);
    }
    assert_eq!(output, 77);
    STATUS_OK
}

#[test]
fn a_recorded_last_mutex_close_detaches_before_its_yielding_payload_finalizer() {
    for width in [2, 8] {
        for durable in [false, true] {
            let recording = record::Recording::new(b"native mutex close", durable);
            let install = recording.clone();
            let own = recording.clone();
            let plain = super::tests::layout();
            let image = crate::Layout {
                finalizer: Some(destroy),
                // Hand-written runtime test thunks carry no emitted scope marker.
                flags: crate::release::LAYOUT_IMPLEMENTATION_FINALIZER,
                ..plain
            };
            let step = crate::Step {
                kind: crate::STEP_CLOSE_HANDLE,
                field: 0,
                variant: 0,
                span: 0,
                offset: 0,
                stride: 0,
                count: 0,
                count_offset: 0,
                layout: std::ptr::null(),
            };
            let wrapper = crate::Layout {
                size: 8,
                flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
                steps: &step,
                step_count: 1,
                field_count: 1,
                ..plain
            };
            let layouts = [&raw const image, &raw const wrapper];
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
                    let mut payload = [1u64, Box::into_raw(Box::new(own.clone())) as u64];
                    let none = [0u64; 2];
                    let mut handle = 0;
                    hirsel_rt_v2_mutex_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut handle,
                        payload.as_mut_ptr().cast(),
                        none.as_ptr().cast(),
                        2,
                        3,
                    );
                    assert_eq!(payload, none);
                    assert_eq!(
                        crate::roots::hirsel_rt_v2_root_drop(
                            ctx,
                            handle as *mut crate::Header,
                            0,
                            0
                        ),
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
            assert_eq!(cut.progress[&Vec::new()], Progress::new(4, Phase::Done));
            let trace = recording.trace();
            let marked = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink();
            for end in marked.marks {
                let bytes = &marked.bytes[..end];
                let tape = hirsel_free::tape::parse(bytes).unwrap();
                let mut replay = Session::replay(bytes).unwrap();
                if let Some(progress) = tape.cut.get(&Vec::new()) {
                    assert!(replay.start(&[]));
                    replay.begin(&[]);
                    replay.create(&[], (crate::free::MUTEX, 0)).unwrap();
                    replay.publish(&[], Phase::Done).unwrap();
                    replay.begin(&[]);
                    replay
                        .touch(&[], 0, (crate::free::MUTEX, 0), "close mutex")
                        .unwrap();
                    replay.publish(&[], Phase::Done).unwrap();
                    if progress.op == 4 {
                        replay.begin(&[]);
                        replay.publish(&[], Phase::Done).unwrap();
                        replay.begin(&[]);
                        assert_eq!(replay.load(&[], 0).unwrap(), 77);
                        replay.publish(&[], Phase::Done).unwrap();
                    }
                } else {
                    assert!(!replay.start(&[]));
                }
                assert_eq!(
                    replay.finish().unwrap(),
                    Projection::new(
                        if end == marked.bytes.len() {
                            "returned"
                        } else {
                            "truncated"
                        },
                        &tape.cut,
                        &trace
                    )
                );
            }
        }
    }
}
