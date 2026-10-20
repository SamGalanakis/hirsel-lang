//! Change watchers over simulated trees: `watcher.next` and `test.dir_event` (CAP-2, CONC-3/8).
//! A simulated tree has no change source of its own: a watcher's queue holds only the events
//! `test.dir_event` delivers into a `test.dir` tree. An event equal to the last one queued
//! is coalesced into it, and past [`QUEUE`] pending events the queue ends with one
//! `Overflow()`, dropping later events until it is read. `watcher.next` with an empty queue
//! parks until an event arrives or its task is canceled.
use super::dispatch::{arg, encode_arguments, encode_value, path, path_text, variant};
use super::simulation::Grant;
use super::{Library, error, ok};
use crate::{BlockedWait, Error, OperationCall, OperationContext, OperationResult, Value, invalid};
use std::collections::VecDeque;

pub(super) const QUEUE: usize = 1024;
const OVERFLOW: u32 = 4;

/// A change by `WatchEvent` ordinal (Created, Modified, Removed, Renamed, Overflow) with
/// its tree paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Change {
    kind: u32,
    paths: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Queue {
    events: VecDeque<Change>,
    overflowed: bool,
}
impl Queue {
    fn push(&mut self, change: Change) {
        if self.overflowed || self.events.back() == Some(&change) {
            return;
        }
        if change.kind == OVERFLOW || self.events.len() >= QUEUE {
            self.overflowed = true;
            self.events.push_back(Change {
                kind: OVERFLOW,
                paths: vec![],
            });
        } else {
            self.events.push_back(change);
        }
    }
    fn pop(&mut self) -> Option<Change> {
        let change = self.events.pop_front()?;
        if change.kind == OVERFLOW {
            self.overflowed = false;
        }
        Some(change)
    }
}

/// Whether a watch of `root` sees `path`: the root, an entry in it, or with `recursive`
/// anything below it.
fn covers(root: &str, recursive: bool, path: &str) -> bool {
    if path == root {
        return true;
    }
    let rest = if root.is_empty() {
        Some(path)
    } else {
        path.strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
    };
    rest.is_some_and(|rest| recursive || !rest.contains('/'))
}

/// `change` as a watch of `root` sees it: a rename into the watch creates there, one out
/// of it removes.
fn seen(root: &str, recursive: bool, change: &Change) -> Option<Change> {
    let covered: Vec<bool> = change
        .paths
        .iter()
        .map(|p| covers(root, recursive, p))
        .collect();
    match (change.kind, covered.as_slice()) {
        (OVERFLOW, _) => Some(change.clone()),
        (3, [true, true]) => Some(change.clone()),
        (3, [true, false]) => Some(Change {
            kind: 2,
            paths: vec![change.paths[0].clone()],
        }),
        (3, [false, true]) => Some(Change {
            kind: 0,
            paths: vec![change.paths[1].clone()],
        }),
        (_, [true]) => Some(change.clone()),
        _ => None,
    }
}

/// A WatchEvent value with `change`'s paths relative to Dir root `root`.
fn event_value(root: &str, change: Change) -> Value {
    let local = |p: String| -> String {
        if root.is_empty() {
            p
        } else if p == root {
            String::new()
        } else {
            p.strip_prefix(&format!("{root}/"))
                .map_or(p.clone(), str::to_owned)
        }
    };
    variant(
        change.kind,
        change.paths.into_iter().map(|p| path(&local(p))).collect(),
    )
}

impl Library {
    pub(super) fn watch_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let a = &*call.arguments;
        if name == "test.dir_event" {
            return self.dir_event(a, context);
        }
        let (_, _, dir) = self.simulation.directory(arg(a, 0)?)?;
        let watcher = self.simulation.id(arg(a, 1)?, "Watcher")?;
        if !matches!(self.simulation.grants.get(&watcher), Some(Grant::Watcher { dir: owner, .. }) if *owner == dir)
        {
            return Err(invalid("watcher of another directory"));
        }
        let input = encode_arguments(&a[..2]);
        if let Some(value) = self.watch_ready(watcher, dir)? {
            self.tasks.yield_now = true;
            self.simulation
                .record("Dir", "watcher.next", input, encode_value(&value), context)?;
            return Ok(OperationResult::returned(value));
        }
        // Like a stdin read, an empty watcher parks at once.
        let token = self.next_callback;
        self.next_callback += 1;
        self.watch_waits.insert(token, (watcher, dir, input));
        Ok(OperationResult::Park {
            token,
            wait: BlockedWait {
                spawn_path: self.tasks.path(),
                anchor: context.anchor().unwrap().clone(),
                kind: "watcher.next".into(),
                targets: vec![watcher],
            },
        })
    }
    /// The next event or the task's cancellation, or `None` while the queue is empty.
    fn watch_ready(&mut self, watcher: u64, dir: u64) -> Result<Option<Value>, Error> {
        if let Some(cause) = self.tasks.canceled() {
            return Ok(Some(error(variant(1, vec![variant(cause, vec![])]))));
        }
        let root = match self.simulation.grants.get(&dir) {
            Some(Grant::Dir { root, .. }) => root.clone(),
            _ => return Err(invalid("dead watcher directory")),
        };
        let Some(Grant::Watcher { queue, .. }) = self.simulation.grants.get_mut(&watcher) else {
            return Err(invalid("dead watcher"));
        };
        Ok(queue.pop().map(|change| ok(event_value(&root, change))))
    }
    pub(super) fn poll_watch(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let (watcher, dir, _) = self.watch_waits[&token];
        let Some(value) = self.watch_ready(watcher, dir)? else {
            return Ok(None);
        };
        let (_, _, input) = self.watch_waits.remove(&token).unwrap();
        self.simulation
            .record("Dir", "watcher.next", input, encode_value(&value), context)?;
        Ok(Some(OperationResult::returned(value)))
    }
    /// Deliver an event to the watchers of a `test.dir` tree that see it; one whose path
    /// leaves the Dir reaches none, nor does one on another tree.
    fn dir_event(
        &mut self,
        a: &[Value],
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.tasks.yield_now = true;
        self.simulation.id(arg(a, 0)?, "Test")?;
        let (tree, root, _) = self.simulation.directory(arg(a, 1)?)?;
        let Value::Variant {
            variant: kind,
            fields,
        } = arg(a, 2)?
        else {
            return Err(invalid("invalid WatchEvent"));
        };
        let mut paths = vec![];
        for field in fields {
            let p = path_text(field.as_ref().ok_or_else(|| invalid("dead event path"))?)?;
            paths.push(hirsel_run_schema::confined_relative(&root, p));
        }
        if tree != 0
            && let Some(paths) = paths.into_iter().collect::<Option<Vec<_>>>()
        {
            let change = Change { kind: *kind, paths };
            for grant in self.simulation.grants.values_mut() {
                if let Grant::Watcher {
                    tree: watched,
                    root,
                    recursive,
                    queue,
                    ..
                } = grant
                    && *watched == tree
                    && let Some(seen) = seen(root, *recursive, &change)
                {
                    queue.push(seen);
                }
            }
        }
        self.simulation.record(
            "Test",
            "test.dir_event",
            encode_arguments(a),
            encode_value(&Value::Unit),
            context,
        )?;
        Ok(OperationResult::returned(Value::Unit))
    }
}
