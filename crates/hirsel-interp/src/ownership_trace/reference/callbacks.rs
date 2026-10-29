//! Result custody of the single, identity-preserving library callbacks. These
//! operations pass their callback result unchanged; other protocols fail closed.
use super::{CallState, Context, FrameState, Owner, Resources, Storage, resources_in};
use crate::{OperationResult, Value, frame::Frame, resources::Ending};
use hirsel_syntax::DefPath;
use hirsel_tir::BodyId;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Callbacks {
    calls: BTreeMap<u64, Invocation>,
    runs: BTreeMap<u64, u64>,
    outputs: BTreeMap<u64, Output>,
}
struct Invocation {
    call: CallState,
    definition: DefPath,
    body: Option<BodyId>,
    captures: Vec<(u32, Owner, u64)>,
    mutex: Option<u64>,
}
struct Output {
    owner: Owner,
    destination: Option<Owner>,
    context: Context,
    count: u64,
}
impl<const OWNED: bool> Resources<OWNED> {
    pub(super) fn callback_called(
        &mut self,
        call: CallState,
        arguments: &[Value],
        result: &OperationResult,
    ) {
        if !matches!(
            result,
            OperationResult::Call { .. } | OperationResult::Park { .. }
        ) {
            if !call.arguments.is_empty() {
                self.unsupported("resource callback did not commit callback entry");
            }
            return;
        }
        let parameter = if call.definition.is_core("mutex.with") {
            2
        } else {
            1
        };
        if call
            .arguments
            .iter()
            .any(|(index, _, _)| *index != parameter)
        {
            self.unsupported("resource callback input needs an observed transport");
            return;
        }
        let Some(callback) = arguments.get(parameter as usize) else {
            self.unsupported("callback argument unavailable");
            return;
        };
        let mutex = if call.definition.is_core("mutex.with") {
            match arguments.get(1) {
                Some(Value::Mutex(root)) => Some(root.id()),
                _ => {
                    self.unsupported("mutex callback has no actual mutex identity");
                    return;
                }
            }
        } else {
            None
        };
        let owner = call
            .arguments
            .iter()
            .map(|(index, _, owner)| (index, owner))
            .chain(call.views.iter().map(|(index, _, owner)| (index, owner)))
            .find_map(|(index, owner)| (*index == parameter).then_some(owner));
        let mut incoming = vec![];
        let (definition, body) = match callback {
            Value::Closure {
                definition,
                body,
                captures,
                environment,
                invoked,
            } => {
                if captures.len() != environment.owned_captures.len()
                    || invoked.as_ref().is_some_and(|state| state.get().is_some())
                {
                    self.unsupported("callback environment is inconsistent or already invoked");
                    return;
                }
                for (index, (value, owned)) in
                    captures.iter().zip(&environment.owned_captures).enumerate()
                {
                    let Some(value) = value else { continue };
                    let count = resources_in::<OWNED>(value);
                    if !owned || count == 0 {
                        continue;
                    }
                    if invoked.is_none() || !Self::supported_tree(value) {
                        self.unsupported("reusable owned resource callback environment");
                        return;
                    }
                    let Some(mut from) = owner.cloned() else {
                        self.unsupported("owned callback capture has no source owner");
                        return;
                    };
                    from.path.push(super::Step::Field {
                        field: index as u32,
                    });
                    incoming.push((index as u32, from, count));
                }
                (definition.path().clone(), Some(*body))
            }
            Value::FunctionReference(reference) => (reference.definition.clone(), None),
            _ => {
                self.unsupported("unsupported library callback target");
                return;
            }
        };
        if let OperationResult::Call { callee, .. } = result {
            let agrees = match callee {
                Value::Closure {
                    definition: actual,
                    body: actual_body,
                    ..
                } => actual.path() == &definition && body == Some(*actual_body),
                Value::FunctionReference(actual) => {
                    actual.definition == definition && body.is_none()
                }
                _ => false,
            };
            if !agrees {
                self.unsupported("library substituted a different callback target");
                return;
            }
        }
        let Some(frame) = self.frames.last().map(|state| state.id) else {
            self.unsupported("library callback has no observed source frame");
            return;
        };
        self.callbacks.calls.insert(
            frame,
            Invocation {
                call,
                definition,
                body,
                captures: incoming,
                mutex,
            },
        );
    }
    pub(super) fn callback_frame(&mut self, callee: &Frame) {
        if self.error.is_some() {
            return;
        }
        let Some(parent) = self.frames.last().map(|state| state.id) else {
            self.unsupported("callback entered without its parent frame");
            return;
        };
        let Some(invocation) = self.callbacks.calls.remove(&parent) else {
            // Resource-free callbacks from other protocols keep the existing
            // conservative sync admission; they cannot return tracked values.
            if callee
                .slots
                .iter()
                .flatten()
                .any(|value| resources_in::<OWNED>(value) != 0)
            {
                self.unsupported("unobserved resource callback input");
            }
            return;
        };
        if callee.definition.path.path() != &invocation.definition
            || invocation.body.is_some_and(|body| body != callee.body_id)
            || callee.bounded.is_some()
        {
            self.unsupported("committed callback target or transport changed");
            return;
        }
        let call = invocation.call;
        let mut aliases = BTreeMap::new();
        let callback_parameter = if call.definition.is_core("mutex.with") {
            2
        } else {
            1
        };
        if let Some((_, _, environment)) = call
            .views
            .iter()
            .chain(call.arguments.iter())
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
                if invocation
                    .captures
                    .iter()
                    .any(|(index, _, _)| *index == capture as u32)
                {
                    continue;
                }
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
        let id = self.next_frame;
        self.next_frame += 1;
        if let Some(mutex) = invocation.mutex {
            let Some(local) = self.mutex_parameter(callee, id, parent, mutex, &call.context) else {
                return;
            };
            aliases.insert(local, super::mutexes::owner(mutex));
        }
        let mut owned = BTreeMap::new();
        for (capture, from, expected) in invocation.captures {
            let Some(local) = Self::capture_local(callee, capture) else {
                self.unsupported("owned callback capture slot unavailable");
                return;
            };
            let actual = callee
                .slots
                .get(local as usize)
                .and_then(Option::as_ref)
                .map_or(0, resources_in::<OWNED>);
            if actual != expected || aliases.contains_key(&local) {
                self.unsupported("owned callback capture inventory changed");
                return;
            }
            let to = self.frame_slot(id, local);
            let result = self.tracker.move_tree(&call.context, &from, &to);
            if let Some(tokens) = self.checked(result)
                && tokens.len() as u64 != expected
            {
                self.unsupported("owned callback capture lineage differs");
                return;
            }
            self.move_borrows(&from, &to, false);
            owned.insert(local, expected);
        }
        if callee.slots.iter().enumerate().any(|(local, value)| {
            value
                .as_ref()
                .is_some_and(|value| resources_in::<OWNED>(value) != 0)
                && !aliases.contains_key(&(local as u32))
                && !owned.contains_key(&(local as u32))
        }) {
            self.unsupported("resource callback parameters or captures need an observed transport");
            return;
        }
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
                output.count = resources_in::<OWNED>(value);
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
        if resources_in::<OWNED>(value) != output.count {
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
