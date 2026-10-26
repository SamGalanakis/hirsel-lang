//! An entered Yield owns its successor independently of the issuer's stack.
//! Polls finish synchronously before Pending, as in the entered host protocol.
use super::{engine, host};
use crate::TaskCtx;
use crate::ordered::{Next, Progress};
use crate::stop::Stopped;

#[derive(Debug, Default)]
pub(crate) enum Yield {
    #[default]
    Before,
    Waiting,
    Complete,
}

impl Yield {
    /// True is complete; false needs the same head acquisition after save.
    /// The caller holds the task role and owns this record through Pending.
    pub(crate) fn drive(&mut self, ctx: *mut TaskCtx) -> Stopped<bool> {
        match self {
            Self::Before => {
                if !engine::head_step(ctx)? {
                    return Ok(false);
                }
                host::begin(ctx, false)?;
                let (progress, private) = host::finish_start(ctx, Next::Yield)?;
                // The logical Yield has committed. Neither poll nor head retry
                // can reissue it, including when the poll stops the run.
                *self = Self::Waiting;
                if private {
                    engine::continue_after_read(ctx, progress)?;
                    *self = Self::Complete;
                    return Ok(true);
                }
                if progress == Progress::Poll {
                    engine::poll(ctx)?;
                }
            }
            Self::Waiting => {}
            Self::Complete => panic!("entered Yield delivered twice"),
        }
        if !engine::head_step(ctx)? {
            return Ok(false);
        }
        *self = Self::Complete;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    unsafe extern "C" fn child(ctx: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
        unsafe { yield_now(&mut *ctx) };
        let mut result = [1u64, 7];
        unsafe {
            hirsel_rt_v2_task_finish(
                ctx,
                result.as_mut_ptr().cast(),
                [0u64; 2].as_ptr().cast(),
                16,
                u32::MAX,
            )
        }
    }

    #[test]
    fn pending_begin_keeps_one_scope_and_one_logical_yield() {
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 16,
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
        let layouts = [&layout as *const crate::Layout];
        let mut run = crate::host::BodyRun::new(1, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let pending = Arc::new(AtomicUsize::new(0));
        let observed = pending.clone();
        let outcome = run.run(
            |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
            },
            move |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    crate::STATUS_OK
                );
                let mut task = 0;
                let environment =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
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
                        0
                    ),
                    crate::STATUS_OK
                );
                let task_id = engine::current(ctx);
                let before =
                    engine::with_driver(ctx, |d| d.submitted.get(&task_id).copied().unwrap_or(0));
                let mut begin = crate::isolate::entered::Begin::start(ctx);
                let mark = begin.mark();
                loop {
                    match begin.drive(ctx, false).unwrap() {
                        crate::tasks::entered_yield::Poll::Complete => break,
                        crate::tasks::entered_yield::Poll::Pending => {
                            observed.fetch_add(1, Ordering::Relaxed);
                            engine::suspend(ctx, task_id, true);
                        }
                    }
                }
                assert_eq!(begin.mark(), mark);
                assert_eq!(
                    engine::with_driver(ctx, |d| d.submitted[&task_id]),
                    before + 1
                );
                assert_eq!(
                    crate::isolate::hirsel_rt_v2_isolate_end(ctx, &0),
                    crate::STATUS_OK
                );
                let mut value = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut task,
                        value.as_mut_ptr().cast(),
                        16
                    ),
                    crate::STATUS_OK
                );
                assert_eq!(value, [1, 7]);
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    crate::STATUS_OK
                );
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert!(
            pending.load(Ordering::Relaxed) > 0,
            "entered yield never waited for head"
        );
    }
}
