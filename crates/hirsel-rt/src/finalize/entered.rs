//! Inactive driver for an independently certified finalizer dispatcher.
//! Admission and shielding precede the first call and survive Pending, like
//! Tokio ce685ee6 runtime/task/harness.rs:502-559. Hirsel retains the exact
//! failure chain and retires admission only at the existing EndFinalize step.
#![allow(dead_code)]

use crate::TaskCtx;
use crate::alloc::Header;
use crate::release::{Finalizer, LAYOUT_IMPLEMENTATION_FINALIZER, LAYOUT_UNSAFE_FFI};
use crate::stop::Stopped;
use crate::tasks::registry::TaskKey;
use crate::{STATUS_FAILURE, STATUS_OK, STATUS_SUSPENDED};

/// The checked native alternate receives its original borrowed self and a
/// retained nested frame. This ABI is private and has no exported callsite.
pub(crate) type Dispatch = unsafe extern "C" fn(*mut TaskCtx, *mut u8, u64, u64, *mut u8) -> u8;

pub(crate) enum Poll {
    Pending,
    Complete(u8),
}

enum Stage {
    New,
    Active {
        work: *mut Header,
        shield: u32,
        admissions: usize,
    },
    Complete,
}

/// The caller owns this record under the same task's Active/Saved custody as
/// the cursor. Every address names retained storage, never a native stack or
/// TaskCtx. There is deliberately no Drop that runs language code.
pub(crate) struct Driver {
    task: TaskKey,
    token: Finalizer,
    dispatch: Dispatch,
    frame: usize,
    stage: Stage,
}

impl Driver {
    /// # Safety
    /// The independent checker matched dispatch to token.layout's actual
    /// finalizer body/ABI, including all cleanup and nested continuation
    /// coverage. The frame is a disjoint retained extent of that checked
    /// callchain; its original zero PC is initialized exactly once here.
    /// token.value and the frame outlive completion or task abandonment.
    pub(crate) unsafe fn new(
        task: TaskKey,
        token: Finalizer,
        dispatch: Dispatch,
        frame: *mut u8,
        bytes: u32,
    ) -> Self {
        assert!(!token.layout.is_null() && !token.value.is_null());
        assert!(unsafe { (*token.layout).finalizer.is_some() });
        assert!(!frame.is_null() && (frame as usize).is_multiple_of(16));
        assert!((8..=4096).contains(&bytes));
        unsafe { frame.cast::<u32>().write(0) };
        Self {
            task,
            token,
            dispatch,
            frame: frame as usize,
            stage: Stage::New,
        }
    }

    /// # Safety
    /// This task exclusively owns the checked original frame and cursor.
    /// Pending unwinds through every checked caller before physical release;
    /// returning Pending here alone grants no permission to abandon a stack.
    pub(crate) unsafe fn poll(&mut self, ctx: *mut TaskCtx) -> Stopped<Poll> {
        let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
        crate::stop::observe(ctx)?;
        assert_eq!(
            crate::tasks::running_key(ctx),
            self.task,
            "stale finalizer task"
        );
        if matches!(self.stage, Stage::New) {
            let flags = unsafe { (*self.token.layout).flags };
            if flags & LAYOUT_IMPLEMENTATION_FINALIZER == 0 {
                let Some(activation) = (unsafe { &mut (*ctx).activations }).admit_finalizer()
                else {
                    let rejections = unsafe { &mut (*ctx).cleanup_rejections };
                    *rejections = rejections
                        .checked_add(1)
                        .expect("finalizer rejection overflow");
                    crate::failure::record_runtime(
                        ctx,
                        "LANG-10",
                        self.token.anchor,
                        self.token.operation_site,
                    );
                    if flags & LAYOUT_UNSAFE_FFI != 0 {
                        super::terminate_unsafe_ffi();
                    }
                    self.stage = Stage::Complete;
                    return Ok(Poll::Complete(STATUS_FAILURE));
                };
                unsafe { (*ctx).cleanup_invocations.push(activation) };
            }
            let shield = unsafe { (*ctx).shield };
            unsafe {
                (*ctx).shield = shield.checked_add(1).expect("finalizer shield overflow");
                crate::tasks::shield_changed(ctx);
            }
            self.stage = Stage::Active {
                work: unsafe { (*ctx).work },
                shield,
                admissions: unsafe { (*ctx).cleanup_invocations.len() },
            };
        }
        let Stage::Active {
            work,
            shield,
            admissions,
        } = self.stage
        else {
            panic!("completed finalizer polled");
        };
        // The borrowed role is local to this one call. Its guard and Segment
        // are gone before Pending reaches the physical dispatcher.
        let status = {
            let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
            unsafe {
                (self.dispatch)(
                    ctx,
                    self.token.value,
                    self.token.anchor,
                    self.token.operation_site,
                    self.frame as *mut u8,
                )
            }
        };
        match status {
            STATUS_SUSPENDED => {
                assert_ne!(
                    unsafe { (self.frame as *const u32).read() },
                    0,
                    "finalizer Pending without an entered PC"
                );
                assert!(
                    unsafe { (*ctx).shield } > shield,
                    "pending finalizer lost shield"
                );
                Ok(Poll::Pending)
            }
            STATUS_OK | STATUS_FAILURE => {
                assert_eq!(
                    unsafe { (*ctx).work },
                    work,
                    "finalizer returned pending release work"
                );
                assert_eq!(
                    unsafe { (*ctx).shield },
                    shield + 1,
                    "unbalanced nested finalizer shield"
                );
                assert_eq!(
                    unsafe { (*ctx).cleanup_invocations.len() },
                    admissions,
                    "unretired nested finalizer admission"
                );
                unsafe {
                    (*ctx).shield = shield;
                    crate::tasks::shield_changed(ctx);
                }
                if status == STATUS_FAILURE {
                    if unsafe { (*ctx).heap().diagnostic && !(*ctx).failure.active() } {
                        crate::fault("a finalizer failed without recording a failure");
                    }
                    if unsafe { (*self.token.layout).flags } & LAYOUT_UNSAFE_FFI != 0 {
                        super::terminate_unsafe_ffi();
                    }
                }
                self.stage = Stage::Complete;
                Ok(Poll::Complete(status))
            }
            _ => crate::fault("entered finalizer returned an invalid status"),
        }
    }
}

#[cfg(test)]
mod tests;
