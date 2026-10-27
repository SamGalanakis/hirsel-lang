//! Inactive entered-begin protocol. The future checked caller retains this
//! record in its frame; it contains no pointer into TaskCtx or a native stack.
#![allow(dead_code)]

use crate::TaskCtx;
use crate::stop::Stopped;
use crate::tasks::entered_yield::{Poll, Yield};

pub(crate) const KIND: u32 = 24;

pub(super) struct Pending {
    site: crate::continuation::binding::BoundSite,
    result: usize,
    begin: Begin,
}

/// Continue a generation-qualified begin from task-owned storage. The mark's
/// retained output cell is initialized once, before any scheduling decision.
/// # Safety
/// The disarmed BoundSite owns its complete checked callchain and frame.
pub(crate) unsafe fn begin(
    ctx: *mut TaskCtx,
    out: *mut u64,
    site: crate::continuation::binding::BoundSite,
) -> Stopped<u8> {
    let result = out as usize;
    assert!(
        result.is_multiple_of(8)
            && result >= site.frame
            && result.checked_add(8).is_some_and(|end| {
                site.frame
                    .checked_add(site.bytes as usize)
                    .is_some_and(|limit| end <= limit)
            }),
        "an isolate begin result outside its retained frame"
    );
    // No borrow into the invocation Vec crosses a drive or its synchronous
    // poll. Only bounded bookkeeping is temporarily local; all language owners
    // and the initialized mark remain in their original retained storage.
    let pending =
        unsafe { (*ctx).isolations.scopes.last_mut() }.and_then(|scope| scope.entered.take());
    let mut pending = match pending {
        Some(pending) => {
            assert_eq!(pending.site, site, "a different isolate begin resumed");
            assert_eq!(pending.result, result, "isolate begin result moved");
            pending
        }
        None => {
            let begin = unsafe { Begin::start(ctx) };
            unsafe { out.write(begin.mark()) };
            Pending {
                site,
                result,
                begin,
            }
        }
    };
    loop {
        let leave = unsafe { crate::continuation::binding::can_leave(ctx, site) };
        match unsafe { pending.begin.drive(ctx, leave) }? {
            Poll::Complete => {
                unsafe { crate::continuation::binding::complete_bound(ctx, site) };
                return Ok(crate::STATUS_OK);
            }
            Poll::Pending if leave => {
                unsafe { (*ctx).isolations.active().entered = Some(pending) };
                return Ok(crate::STATUS_SUSPENDED);
            }
            Poll::Pending => crate::tasks::ordered::yielding::wait(ctx),
        }
    }
}

pub(crate) struct Begin {
    generation: u64,
    parent: Option<u64>,
    mark: usize,
    yielding: Yield,
}

impl Begin {
    /// # Safety
    /// The task owns the active wrapper and its transferred closure. The
    /// caller retains this record and all language owners across Pending.
    pub(crate) unsafe fn start(ctx: *mut TaskCtx) -> Self {
        let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
        let mark = unsafe { (*ctx).failure.count() };
        unsafe { (*ctx).isolations.begin(mark) };
        #[cfg(feature = "diagnostic")]
        crate::compact_trace::isolate_begin(unsafe { (*ctx).task_id });
        let scope = unsafe { (*ctx).isolations.active() };
        Self {
            generation: scope.generation,
            parent: scope.parent,
            mark,
            yielding: Yield::default(),
        }
    }

    pub(crate) fn mark(&self) -> u64 {
        self.mark as u64
    }

    fn matches(&self, scopes: &super::Invocations) -> bool {
        scopes.scopes.last().is_some_and(|scope| {
            scope.generation == self.generation
                && scope.parent == self.parent
                && scope.mark == self.mark
                && scope.caught.is_none()
        })
    }

    /// Continue the original yield, never Begin. The mark is already owned
    /// here before the first possible wait and is not reconstructed on resume.
    /// # Safety
    /// As for start. `leave` additionally requires a checked BoundSite and
    /// full retained callchain, as tasks::entered_yield::Yield::drive requires.
    pub(crate) unsafe fn drive(&mut self, ctx: *mut TaskCtx, leave: bool) -> Stopped<Poll> {
        let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
        if !self.matches(unsafe { &(*ctx).isolations }) {
            crate::fault("entered isolate begin has a stale invocation");
        }
        unsafe { self.yielding.drive(ctx, leave) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Observed {
        started: usize,
        completed: usize,
        entries: Vec<(u32, usize)>,
    }

    unsafe extern "C" fn child(
        ctx: *mut TaskCtx,
        environment: *mut crate::Header,
        caller: u64,
        operation: u64,
    ) -> u8 {
        unsafe {
            crate::continuation::hirsel_rt_v2_resume_run(
                ctx,
                environment,
                caller,
                operation,
                dispatch,
                32,
            )
        }
    }

    // A stand-in for the checked emitter's retained call result and status
    // dispatch. Every Pending unwinds to the real shared physical dispatcher.
    unsafe extern "C" fn dispatch(
        ctx: *mut TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
        frame: *mut u8,
    ) -> u8 {
        let observed = unsafe {
            &*(crate::alloc::payload(environment).cast::<usize>().read() as *const Mutex<Observed>)
        };
        let state = frame.cast::<u32>();
        let mark = unsafe { frame.add(8).cast::<u64>() };
        let result = unsafe { frame.add(16).cast::<[u64; 2]>() };
        observed
            .lock()
            .unwrap()
            .entries
            .push((unsafe { state.read() }, ctx as usize));
        if unsafe { state.read() } == 0 {
            observed.lock().unwrap().started += 1;
            unsafe {
                mark.write(u64::MAX);
                state.write(1)
            };
        }
        if unsafe { state.read() } == 1 {
            unsafe {
                crate::continuation::binding::hirsel_rt_v2_resume_tail(
                    ctx, frame, 1, KIND, 32, 0, 0,
                )
            };
            let status = unsafe { crate::isolate::hirsel_rt_v2_isolate_begin(ctx, mark, &0) };
            if status == crate::STATUS_SUSPENDED {
                return status;
            }
            assert_eq!(status, crate::STATUS_OK);
            assert_eq!(unsafe { mark.read() }, 0);
            let scopes = unsafe { &(*ctx).isolations.scopes };
            assert_eq!(scopes.len(), 1, "the entered begin restarted");
            assert_eq!(scopes[0].generation, 1, "the original generation changed");
            assert!(scopes[0].entered.is_none());
            unsafe { crate::isolate::hirsel_rt_v2_isolate_end(ctx, &0) };
            assert!(unsafe { (*ctx).isolations.scopes.is_empty() });
            observed.lock().unwrap().completed += 1;
            unsafe {
                result.write([1, 7]);
                state.write(2)
            };
        }
        unsafe {
            crate::tasks::hirsel_rt_v2_task_finish(
                ctx,
                result.cast(),
                [0u64; 2].as_ptr().cast(),
                16,
                u32::MAX,
            )
        }
    }

    #[test]
    fn entered_begin_keeps_generation_and_retained_mark_after_physical_relocation() {
        for width in [1, 4] {
            let environment = crate::Layout {
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
            let layouts = [&environment as *const crate::Layout];
            let observed = Arc::new(Mutex::new(Observed::default()));
            let pointer = Arc::as_ptr(&observed) as usize;
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    crate::tasks::ordered::engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    assert_eq!(
                        crate::tasks::hirsel_rt_v2_task_scope_enter(
                            ctx,
                            std::ptr::null_mut(),
                            &mut scope,
                            &0
                        ),
                        crate::STATUS_OK
                    );
                    let mut children = [0u64; 4];
                    for task in &mut children {
                        let environment = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            8,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(environment)
                            .cast::<usize>()
                            .write(pointer);
                        assert_eq!(
                            crate::tasks::hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                task,
                                &scope,
                                child,
                                environment,
                                0
                            ),
                            crate::STATUS_OK
                        );
                    }
                    for task in &mut children {
                        let mut result = [0u64; 2];
                        assert_eq!(
                            crate::tasks::hirsel_rt_v2_task_join(
                                ctx,
                                0,
                                0,
                                &scope,
                                task,
                                result.as_mut_ptr().cast(),
                                16
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!(result, [1, 7]);
                    }
                    assert_eq!(
                        crate::tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        crate::STATUS_OK
                    );
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            let observed = observed.lock().unwrap();
            assert_eq!((observed.started, observed.completed), (4, 4));
            let starts: std::collections::BTreeSet<_> = observed
                .entries
                .iter()
                .filter(|(state, _)| *state == 0)
                .map(|(_, ctx)| *ctx)
                .collect();
            assert!(
                observed
                    .entries
                    .iter()
                    .any(|(state, ctx)| *state == 1 && !starts.contains(ctx)),
                "no saved begin moved off its original native stack"
            );
        }
    }

    #[test]
    fn equal_marks_do_not_authorize_a_child_or_a_reused_scope() {
        let mut scopes = crate::isolate::Invocations::default();
        scopes.begin(0);
        let scope = scopes.active();
        let ticket = Begin {
            generation: scope.generation,
            parent: scope.parent,
            mark: scope.mark,
            yielding: Yield::default(),
        };
        assert!(ticket.matches(&scopes));
        scopes.begin(0);
        assert!(!ticket.matches(&scopes));
        scopes.end();
        assert!(ticket.matches(&scopes));
        scopes.active().caught = Some(Vec::new());
        assert!(!ticket.matches(&scopes));
        scopes.active().caught = None;
        scopes.end();
        scopes.begin(0);
        assert!(!ticket.matches(&scopes));
    }
}
