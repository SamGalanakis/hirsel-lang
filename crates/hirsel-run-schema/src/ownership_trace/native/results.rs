//! Actual child result copies join two independently checked formal intents.
//! Holding and discarding move no credit and never infer a Resource drop.
use super::{
    Context, Error, Owner, Row, TaskPath, TaskResultBinding, TaskResultDirection, Tracker,
};
use crate::Site;
use std::collections::BTreeMap;

pub(super) struct Intent {
    pub binding: TaskResultBinding,
    pub owner: Owner,
    pub source: Site,
    pub optional: bool,
}
struct State {
    intent: Intent,
    held: bool,
    child: Option<u64>,
}
#[derive(Default)]
pub(super) struct Results(BTreeMap<(u64, u64, u32), State>);

fn occupied(tracker: &Tracker, root: &Owner) -> bool {
    tracker.inventory().owners.keys().any(|owner| {
        owner.task == root.task
            && owner.storage == root.storage
            && owner.path.starts_with(&root.path)
    })
}

impl Results {
    pub(super) fn intent(
        &mut self,
        task: u64,
        frame: u64,
        marker: u32,
        intent: Intent,
    ) -> Result<(), Error> {
        if self
            .0
            .insert(
                (task, frame, marker),
                State {
                    intent,
                    held: false,
                    child: None,
                },
            )
            .is_some()
        {
            return Err(Error::InvalidMarker(marker));
        }
        Ok(())
    }
    pub(super) fn observe(
        &mut self,
        row: Row,
        current: Option<u64>,
        task_path: &TaskPath,
        operation: u64,
        tracker: &mut Tracker,
    ) -> Result<bool, Error> {
        let (task, frame, marker) = match row {
            Row::TaskResultHeld {
                task,
                frame,
                marker,
                ..
            }
            | Row::TaskResultJoin {
                task,
                frame,
                marker,
                ..
            }
            | Row::TaskResultMoved {
                task,
                frame,
                marker,
                ..
            }
            | Row::TaskResultDiscarded {
                task,
                frame,
                marker,
                ..
            }
            | Row::TaskResultUnavailable {
                task,
                frame,
                marker,
                ..
            } => (task, frame, marker),
            _ => return Ok(false),
        };
        // A delivery can run from a saved-frame poll. The checked intent owns
        // that frame until its entered call finishes; no parked context is read.
        if !matches!(row, Row::TaskResultMoved { .. }) && current != Some(frame) {
            return Err(Error::InvalidFrame(frame));
        }
        let key = (task, frame, marker);
        match row {
            Row::TaskResultHeld { layout, .. } => {
                let state = self.0.get_mut(&key).ok_or(Error::InvalidMarker(marker))?;
                if state.intent.binding
                    != (TaskResultBinding {
                        direction: TaskResultDirection::Hold,
                        descriptor: layout,
                    })
                    || state.held
                    || state.child.is_some()
                {
                    return Err(Error::InvalidMarker(marker));
                }
                state.held = true;
            }
            Row::TaskResultJoin { child, .. } => {
                let state = self.0.get_mut(&key).ok_or(Error::InvalidMarker(marker))?;
                if state.intent.binding.direction != TaskResultDirection::Join
                    || state.held
                    || state.child.replace(child).is_some()
                {
                    return Err(Error::InvalidMarker(marker));
                }
            }
            Row::TaskResultMoved {
                child,
                source_frame,
                source_marker,
                layout,
                ..
            } => {
                let recipient = self.0.remove(&key).ok_or(Error::InvalidMarker(marker))?;
                let source = self
                    .0
                    .remove(&(child, source_frame, source_marker))
                    .ok_or(Error::InvalidMarker(source_marker))?;
                if recipient.intent.binding
                    != (TaskResultBinding {
                        direction: TaskResultDirection::Join,
                        descriptor: layout,
                    })
                    || source.intent.binding
                        != (TaskResultBinding {
                            direction: TaskResultDirection::Hold,
                            descriptor: layout,
                        })
                    || recipient.child != Some(child)
                    || !source.held
                    || source.child.is_some()
                    || recipient.held
                    || occupied(tracker, &recipient.intent.owner)
                {
                    return Err(Error::InvalidMarker(marker));
                }
                if source.intent.optional && !occupied(tracker, &source.intent.owner) {
                    return Ok(true);
                }
                let moved = tracker
                    .move_tree(
                        &Context {
                            task: task_path.clone(),
                            operation,
                            site: recipient.intent.source,
                        },
                        &source.intent.owner,
                        &recipient.intent.owner,
                    )
                    .map_err(Error::Consistency)?;
                if moved.is_empty() {
                    return Err(Error::InvalidMarker(marker));
                }
            }
            Row::TaskResultDiscarded { .. } => {
                let source = self.0.remove(&key).ok_or(Error::InvalidMarker(marker))?;
                if source.intent.binding.direction != TaskResultDirection::Hold
                    || !source.held
                    || source.child.is_some()
                {
                    return Err(Error::InvalidMarker(marker));
                }
            }
            Row::TaskResultUnavailable { child, status, .. } => {
                let recipient = self.0.remove(&key).ok_or(Error::InvalidMarker(marker))?;
                if status != 1
                    || recipient.intent.binding.direction != TaskResultDirection::Join
                    || recipient.child != Some(child)
                    || recipient.held
                    || occupied(tracker, &recipient.intent.owner)
                {
                    return Err(Error::InvalidMarker(marker));
                }
            }
            _ => unreachable!(),
        }
        Ok(true)
    }
    pub(super) fn finish(&self) -> Result<(), Error> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error::Incomplete)
        }
    }
}
