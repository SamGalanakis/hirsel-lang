//! Result custody of the single, identity-preserving library callbacks. These
//! operations pass their callback result unchanged; other protocols fail closed.
use super::{CallState, Context, FrameState, Owner, Resources, Storage, resources_in};
use crate::{OperationResult, Value, frame::Frame, resources::Ending};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Callbacks {
    calls: BTreeMap<u64, CallState>,
    runs: BTreeMap<u64, u64>,
    outputs: BTreeMap<u64, Output>,
}
struct Output {
    owner: Owner,
    destination: Option<Owner>,
    context: Context,
    count: u64,
}
impl Resources {
    pub(super) fn callback_called(&mut self, call: CallState, result: &OperationResult) {
        if !(call.definition.is_core("task.scope")
            || call.definition.is_core("mutex.with")
            || call.definition.is_core("shared.with"))
            || !matches!(
                result,
                OperationResult::Call { .. } | OperationResult::Park { .. }
            )
        {
            return;
        }
        let Some(frame) = self.frames.last().map(|state| state.id) else {
            self.unsupported("library callback has no observed source frame");
            return;
        };
        self.callbacks.calls.insert(frame, call);
    }
    pub(super) fn callback_frame(&mut self, callee: &Frame) {
        if self.error.is_some() {
            return;
        }
        let Some(parent) = self.frames.last().map(|state| state.id) else {
            self.unsupported("callback entered without its parent frame");
            return;
        };
        let Some(call) = self.callbacks.calls.remove(&parent) else {
            // Resource-free callbacks from other protocols keep the existing
            // conservative sync admission; they cannot return tracked values.
            if callee
                .slots
                .iter()
                .flatten()
                .any(|value| resources_in(value) != 0)
            {
                self.unsupported("unobserved resource callback input");
            }
            return;
        };
        let mut aliases = BTreeMap::new();
        let callback_parameter = if call.definition.is_core("mutex.with") {
            2
        } else {
            1
        };
        if let Some((_, _, environment)) = call
            .views
            .iter()
            .find(|(parameter, _, _)| *parameter == callback_parameter)
        {
            for (capture, (local, _)) in callee
                .body()
                .locals
                .iter()
                .enumerate()
                .filter(|(_, local)| local.storage == hirsel_tir::Storage::Capture)
                .enumerate()
            {
                let mut from = environment.clone();
                from.path.push(super::Step::Field {
                    field: capture as u32,
                });
                if let Some(target) = self.borrow_target(&from) {
                    aliases.insert(local as u32, target);
                } else if self.has_borrows(&from) {
                    aliases.insert(local as u32, from);
                }
            }
        }
        if !call.arguments.is_empty()
            || callee.slots.iter().enumerate().any(|(local, value)| {
                value.as_ref().is_some_and(|value| resources_in(value) != 0)
                    && !aliases.contains_key(&(local as u32))
            })
        {
            self.unsupported("resource callback parameters or captures need an observed transport");
            return;
        }
        let id = self.next_frame;
        self.next_frame += 1;
        let owner = self.scratch();
        self.callbacks.runs.insert(id, parent);
        self.callbacks.outputs.insert(
            parent,
            Output {
                owner: owner.clone(),
                destination: call.destination,
                context: call.context,
                count: 0,
            },
        );
        self.frames.push(FrameState {
            id,
            finalizer: false,
            aliases,
            return_to: Some(owner),
            returned: None,
        });
    }
    pub(super) fn callback_retiring(&mut self, ending: &Ending) {
        let Some(frame) = self.frames.last().map(|state| state.id) else {
            return;
        };
        let Some(parent) = self.callbacks.runs.remove(&frame) else {
            return;
        };
        match ending {
            Ending::Return(value) => {
                let Some(output) = self.callbacks.outputs.get_mut(&parent) else {
                    self.unsupported("callback result lost its operation custody");
                    return;
                };
                output.count = resources_in(value);
            }
            Ending::Failed => {
                self.callbacks.outputs.remove(&parent);
            }
        }
    }
    pub(super) fn callback_returned(&mut self, value: &Value) -> bool {
        let Some(frame) = self.frames.last().map(|state| state.id) else {
            return false;
        };
        self.callbacks.calls.remove(&frame);
        let Some(output) = self.callbacks.outputs.remove(&frame) else {
            return false;
        };
        if resources_in(value) != output.count {
            self.unsupported("library callback changed the returned resource inventory");
            return true;
        }
        if output.count == 0 {
            if self.has_borrows(&output.owner) {
                let Some(destination) = output.destination else {
                    self.unsupported("callback borrowed environment has no receiving place");
                    return true;
                };
                self.move_borrows(&output.owner, &destination, false);
            }
            return false;
        }
        let Some(destination) = output.destination else {
            self.unsupported("resource callback has no receiving place");
            return true;
        };
        if let Storage::Slot {
            frame: target,
            slot,
        } = destination.storage
            && target == frame
            && destination.path.is_empty()
        {
            self.frames.last_mut().unwrap().aliases.remove(&slot);
        }
        let moved = self
            .tracker
            .move_tree(&output.context, &output.owner, &destination);
        if let Some(tokens) = self.checked(moved)
            && tokens.len() as u64 != output.count
        {
            self.unsupported("callback returned lineage inventory differs");
        }
        true
    }
}
