//! Inactive entered-begin protocol. The future checked caller retains this
//! record in its frame; it contains no pointer into TaskCtx or a native stack.
#![allow(dead_code)]

use crate::TaskCtx;
use crate::stop::Stopped;
use crate::tasks::entered_yield::{Poll, Yield};

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
