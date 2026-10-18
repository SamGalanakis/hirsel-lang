//! Explicit cleanup work, drained by the existing frame scheduler.
use crate::{Value, observation::Anchor};
use hirsel_tir::CleanupAction;
#[derive(Debug)]
pub(crate) enum Work {
    Action(CleanupAction, Anchor),
    Chain(hirsel_tir::CleanupNodeId, Anchor),
    Release(Value, Anchor, Option<crate::frame::CallerSite>),
    Resume(crate::OperationResult, Callback),
}
pub(crate) enum Ending {
    Return(Value),
    Failed,
}

#[derive(Debug)]
pub(crate) struct OperationState {
    pub caller: Option<crate::frame::CallerSite>,
    pub arguments: Vec<Value>,
    pub owned: Vec<Value>,
    /// Incoming owners remain live until a completion explicitly transfers them.
    pub taken: Vec<usize>,
    pub finishing: Option<usize>,
    pub mut_places: Vec<(usize, hirsel_tir::PlaceId)>,
    /// None for a diverging call: completing it normally is an invariant fault.
    pub returns: Option<hirsel_tir::CallReturn>,
    pub on_failure: hirsel_tir::CleanupId,
    pub anchor: Anchor,
    pub already_failed: bool,
    /// A native named callback resumes its suspended operation after release.
    pub callback: Option<Box<Callback>>,
    pub callback_mut_arguments: Vec<usize>,
    /// Checked execution: the result's bound and each mut argument's restored bound.
    pub bounds: Option<crate::checked::OperationCheck>,
}
#[derive(Debug)]
pub(crate) struct Callback {
    pub failure_start: usize,
    pub token: u64,
    pub state: OperationState,
    pub mut_parameters: Vec<(usize, hirsel_tir::LocalId)>,
    pub writebacks: Vec<crate::CallbackWriteback>,
}
impl Drop for OperationState {
    fn drop(&mut self) {
        for value in self.arguments.drain(..).chain(self.owned.drain(..)) {
            value.discard();
        }
        while let Some(mut callback) = self.callback.take() {
            self.callback = callback.state.callback.take();
            for value in callback
                .state
                .arguments
                .drain(..)
                .chain(callback.state.owned.drain(..))
            {
                value.discard();
            }
        }
    }
}
pub(crate) fn discard_result(result: crate::OperationResult) {
    match result {
        crate::OperationResult::Complete {
            value,
            arguments,
            released,
            ..
        } => {
            value.discard();
            for v in released {
                v.discard();
            }
            for (_, v) in arguments {
                v.discard();
            }
        }
        crate::OperationResult::Call {
            callee,
            arguments,
            owned,
            updates,
            ..
        } => {
            callee.discard();
            for (_, v) in updates {
                v.discard();
            }
            for v in arguments.into_iter().chain(owned) {
                v.discard();
            }
        }
        crate::OperationResult::Failed(_)
        | crate::OperationResult::UndefinedBehavior(_)
        | crate::OperationResult::Park { .. }
        | crate::OperationResult::FailureChain { .. } => (),
    }
}
