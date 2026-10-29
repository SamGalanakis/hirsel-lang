//! Mutex payload custody follows actual provider identities and committed
//! mutable callback writebacks. Borrowing a payload never creates a token.
use super::{CallState, Context, Owner, Resources, Storage, resources_in};
use crate::{OperationResult, Value, frame::Frame, observation::Anchor};
use hirsel_span::SourceMap;
use hirsel_types::UseKind;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Mutexes {
    payloads: BTreeMap<u64, u64>,
    runs: BTreeMap<u64, (u64, u64, u32, Context)>,
    completed: BTreeMap<u64, (u64, Context)>,
    pub(super) drops: BTreeMap<u64, u64>,
}
pub(super) fn owner(id: u64) -> Owner {
    Owner {
        task: None,
        storage: Storage::Object {
            name: "mutex".into(),
            id,
            item: 0,
        },
        path: vec![],
    }
}
impl<const OWNED: bool> Resources<OWNED> {
    pub(super) fn mutex_created(
        &mut self,
        call: CallState,
        arguments: &[Value],
        result: &OperationResult,
    ) {
        let OperationResult::Complete {
            value: Value::Mutex(root),
            consumed,
            arguments: updates,
            released,
        } = result
        else {
            self.unsupported("mutex creation did not commit an observed root");
            return;
        };
        let Some(value) = arguments.get(1) else {
            self.unsupported("mutex creation has no payload");
            return;
        };
        if consumed.as_slice() != [1]
            || !updates.is_empty()
            || !released.is_empty()
            || !Self::supported_tree(value)
        {
            self.unsupported("unsupported mutex creation transport");
            return;
        }
        let count = resources_in::<OWNED>(value);
        let destination = owner(root.id());
        if count != 0 {
            let [(1, UseKind::Take, from)] = call.arguments.as_slice() else {
                self.unsupported("mutex does not own its tracked payload");
                return;
            };
            let moved = self.tracker.move_tree(&call.context, from, &destination);
            if let Some(tokens) = self.checked(moved)
                && tokens.len() as u64 != count
            {
                self.unsupported("mutex creation lineage inventory differs");
                return;
            }
            self.move_borrows(from, &destination, false);
        } else if !call.arguments.is_empty() {
            self.unsupported("mutex creation has unattributed resources");
            return;
        }
        if self.mutexes.payloads.len() >= self.task_limit
            || self.mutexes.payloads.insert(root.id(), count).is_some()
        {
            self.unsupported("duplicate mutex identity or mutex metadata budget exhausted");
        }
    }
    pub(super) fn mutex_parameter(
        &mut self,
        callee: &Frame,
        frame: u64,
        parent: u64,
        id: u64,
        context: &Context,
    ) -> Option<u32> {
        let Some(local) = callee.body().parameters.first().map(|local| local.0) else {
            self.unsupported("mutex callback parameter unavailable");
            return None;
        };
        let Some(actual) = callee.slots.get(local as usize).and_then(Option::as_ref) else {
            self.unsupported("mutex callback payload unavailable");
            return None;
        };
        let count = resources_in::<OWNED>(actual);
        let expected = self.mutexes.payloads.get(&id).copied().unwrap_or(0);
        if count != expected {
            self.unsupported("mutex callback payload inventory differs from committed storage");
            return None;
        }
        self.mutexes
            .runs
            .insert(frame, (parent, id, local, context.clone()));
        Some(local)
    }
    pub(super) fn mutex_retiring(&mut self, callee: &Frame) {
        let Some(frame) = self.frames.last().map(|state| state.id) else {
            return;
        };
        let Some((parent, id, local, context)) = self.mutexes.runs.remove(&frame) else {
            return;
        };
        let Some(payload) = callee.slots.get(local as usize).and_then(Option::as_ref) else {
            self.unsupported("mutex callback consumed its mutable payload");
            return;
        };
        // The evaluator writes this exact local back after retiring the frame.
        self.mutexes
            .payloads
            .insert(id, resources_in::<OWNED>(payload));
        self.mutexes.completed.insert(parent, (id, context));
    }
    fn release_mutex_payload(&mut self, id: u64, values: &[Value], context: &Context) {
        let expected = self.mutexes.payloads.remove(&id).unwrap_or(0);
        let count = values.iter().map(resources_in::<OWNED>).sum::<u64>();
        if count != expected || values.len() > 1 {
            self.unsupported("mutex cleanup payload inventory differs");
            return;
        }
        let from = owner(id);
        if count != 0 {
            let to = self.scratch();
            let moved = self.tracker.move_tree(context, &from, &to);
            if let Some(tokens) = self.checked(moved) {
                if tokens.len() as u64 != count {
                    self.unsupported("mutex cleanup lineage inventory differs");
                    return;
                }
                self.releases.push_back(to);
            }
        }
        self.forget_borrows(&from);
    }
    pub(super) fn mutex_completed(&mut self, result: &OperationResult) {
        if self.error.is_some() {
            return;
        }
        let Some(parent) = self.frames.last().map(|state| state.id) else {
            return;
        };
        if matches!(result, OperationResult::Park { .. }) {
            return;
        }
        let Some((id, context)) = self.mutexes.completed.remove(&parent) else {
            return;
        };
        match result {
            OperationResult::Complete { .. } => (),
            OperationResult::FailureChain { released, .. } => {
                self.release_mutex_payload(id, released, &context);
            }
            _ => self.unsupported("unsupported mutex callback completion"),
        }
    }
    pub(super) fn mutex_released(
        &mut self,
        values: &[Value],
        anchor: &Anchor,
        sources: &SourceMap,
    ) {
        if self.error.is_some() {
            return;
        }
        let Some(frame) = self.frames.last().map(|state| state.id) else {
            return;
        };
        let Some(id) = self.mutexes.drops.remove(&frame) else {
            return;
        };
        let context = self.context(anchor.definition, anchor.body, anchor.site, sources);
        self.release_mutex_payload(id, values, &context);
    }
    pub(super) fn mutex_abandoned(
        &mut self,
        values: &[Value],
        anchor: &Anchor,
        sources: &SourceMap,
    ) {
        if self.error.is_some() {
            return;
        }
        let Some(frame) = self.frames.last().map(|state| state.id) else {
            return;
        };
        let Some((id, _)) = self.mutexes.completed.remove(&frame) else {
            if values.iter().any(|value| resources_in::<OWNED>(value) != 0) {
                self.unsupported("unattributed abandoned callback payload");
            }
            return;
        };
        let context = self.context(anchor.definition, anchor.body, anchor.site, sources);
        self.release_mutex_payload(id, values, &context);
    }
}
