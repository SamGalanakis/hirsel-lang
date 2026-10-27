//! Explicit cleanup work, drained by the existing frame scheduler.
use crate::{Value, observation::Anchor};
use hirsel_tir::CleanupAction;
#[derive(Debug)]
pub(crate) enum Work {
    Retire(crate::depth::Guard),
    Action(CleanupAction, Anchor),
    Chain(hirsel_tir::CleanupNodeId, Anchor),
    Release(Value, Anchor, Option<crate::frame::CallerSite>),
    /// A dropped slot no longer owns its shadow. Keep its holders through the
    /// preceding release, including nested releases and suspended finalizers.
    RetireShadow(crate::checked::Shadow),
    Resume(crate::OperationResult, Box<Callback>),
}
pub(crate) enum Ending {
    Return(Value),
    Failed,
}

#[derive(Debug)]
pub(crate) struct OperationState {
    pub activation: Option<crate::depth::Guard>,
    pub caller: Option<crate::frame::CallerSite>,
    pub root_loan: Option<crate::sealed_storage::RootLoan>,
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
        if let Some((value, _)) = self
            .bounds
            .as_mut()
            .and_then(|bounds| bounds.task_arguments.take())
        {
            value.discard();
        }
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
impl OperationState {
    /// The logical bytes this suspended operation holds (`budgets.limits@2`).
    pub(crate) fn logical_bytes(&self) -> u64 {
        let mut total = self
            .arguments
            .iter()
            .chain(&self.owned)
            .map(Value::logical_bytes)
            .fold(0u64, u64::saturating_add);
        if let Some(callback) = &self.callback {
            total = total.saturating_add(callback.state.logical_bytes());
        }
        total
    }
}
/// The logical bytes an operation result not yet published holds.
pub(crate) fn result_bytes(result: &crate::OperationResult) -> u64 {
    use crate::OperationResult::*;
    let sum = |values: &mut dyn Iterator<Item = &Value>| {
        values
            .map(Value::logical_bytes)
            .fold(0u64, u64::saturating_add)
    };
    match result {
        ArenaCharge { result, .. } | BorrowRegion { result, .. } => result_bytes(result),
        SealArena { .. } => 0,
        Complete {
            value,
            arguments,
            released,
            ..
        } => sum(&mut std::iter::once(value)
            .chain(released)
            .chain(arguments.iter().map(|(_, v)| v))),
        Call {
            callee,
            arguments,
            owned,
            updates,
            ..
        } => sum(&mut std::iter::once(callee)
            .chain(arguments)
            .chain(owned)
            .chain(updates.iter().map(|(_, v)| v))),
        FailureChain { released, .. } => sum(&mut released.iter()),
        Failed(_) | UndefinedBehavior(_) | Park { .. } => 0,
    }
}
pub(crate) fn discard_result(result: crate::OperationResult) {
    match result {
        crate::OperationResult::ArenaCharge { result, .. }
        | crate::OperationResult::BorrowRegion { result, .. } => discard_result(*result),
        crate::OperationResult::SealArena { .. } => (),
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
