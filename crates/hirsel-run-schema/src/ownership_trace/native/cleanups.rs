//! A detached queue is not destruction. Only a checked descriptor completion
//! inside its actual image cleanup may retire an existing Resource credit.
use super::{
    CleanupLayouts, Context, Error, Owner, Row, Storage, TaskPath, Tracker, queues::Queues,
};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Cleanups(BTreeMap<TaskPath, Vec<Scope>>);
struct Scope {
    queue: u64,
    item: u64,
    layout: u32,
    last: Option<u32>,
}
impl Cleanups {
    pub(super) fn observe(
        &mut self,
        row: Row,
        task: &TaskPath,
        operation: u64,
        layouts: Option<&CleanupLayouts>,
        queues: &mut Queues,
        tracker: &mut Tracker,
    ) -> Result<bool, Error> {
        let (queue, item) = match row {
            Row::QueueCleanupBegin { queue, item, .. }
            | Row::QueueCleanupDrop { queue, item, .. }
            | Row::QueueCleanupEnd { queue, item, .. } => (queue, item),
            _ => return Ok(false),
        };
        let layouts = layouts.ok_or(Error::UnsupportedQueueCleanup(queue))?;
        let scopes = self.0.entry(task.clone()).or_default();
        match row {
            Row::QueueCleanupBegin { layout, .. } => {
                queues.cleanup_begin(queue, layout, item)?;
                scopes.push(Scope {
                    queue,
                    item,
                    layout,
                    last: None,
                });
            }
            Row::QueueCleanupDrop { step, .. } => {
                let scope = scopes.last_mut().ok_or(Error::InvalidQueue(queue))?;
                if (scope.queue, scope.item) != (queue, item)
                    || scope.last.is_some_and(|previous| previous >= step)
                {
                    return Err(Error::InvalidQueue(queue));
                }
                let binding = layouts
                    .get(&scope.layout)
                    .and_then(|bindings| bindings.get(&step))
                    .ok_or(Error::InvalidQueue(queue))?;
                let owner = Owner {
                    task: None,
                    storage: Storage::Object {
                        name: "native.channel".into(),
                        id: queue,
                        item,
                    },
                    path: binding.path.clone(),
                };
                tracker
                    .drop_value(
                        &Context {
                            task: task.clone(),
                            operation,
                            site: binding.source.clone(),
                        },
                        &owner,
                    )
                    .map_err(Error::Consistency)?;
                scope.last = Some(step);
            }
            Row::QueueCleanupEnd { .. } => {
                let scope = scopes.last().ok_or(Error::InvalidQueue(queue))?;
                if (scope.queue, scope.item) != (queue, item) {
                    return Err(Error::InvalidQueue(queue));
                }
                if tracker.inventory().owners.keys().any(|owner| {
                    matches!(&owner.storage, Storage::Object { name, id, item: at }
                        if name == "native.channel" && *id == queue && *at == item)
                }) {
                    return Err(Error::UnsupportedQueueCleanup(queue));
                }
                queues.cleanup_end(queue, item)?;
                scopes.pop();
            }
            _ => unreachable!(),
        }
        Ok(true)
    }
    pub(super) fn finish(&self) -> Result<(), Error> {
        if let Some(scope) = self.0.values().find_map(|scopes| scopes.last()) {
            return Err(Error::UnsupportedQueueCleanup(scope.queue));
        }
        Ok(())
    }
}
