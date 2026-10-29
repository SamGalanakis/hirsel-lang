//! Rule 3 of design §4.3. This is a may-touch analysis of held handles, never an
//! analysis of the code a task happened to execute in a profiling run.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub type TaskId = u64;
pub type TypeId = u32;

/// Stable object identity: its creator's spawn path and operation ordinal.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ObjectId {
    pub creator: Vec<u32>,
    pub ordinal: u64,
}

/// The concrete items of an operation. Only Object items can belong to E.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Item {
    Object(ObjectId),
    Cancel(TaskId),
    Rank(TaskId),
    Task(TaskId),
    Credit,
    Host,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Footprint {
    pub reads: BTreeSet<Item>,
    pub writes: BTreeSet<Item>,
}

impl Footprint {
    pub fn conflicts(&self, other: &Self) -> bool {
        !self.writes.is_disjoint(&other.writes)
            || !self.writes.is_disjoint(&other.reads)
            || !self.reads.is_disjoint(&other.writes)
    }

    pub fn objects_only(&self) -> bool {
        self.reads
            .iter()
            .chain(&self.writes)
            .all(|item| matches!(item, Item::Object(_)))
    }
}

/// Handles held at spawn, created, received, or obtained by a keep operation.
/// Kept conservatively until the turn retires. `obtains` includes the handle
/// types in Receiver leftovers and TaskSetTake results, including finalizers.
#[derive(Clone, Debug)]
pub struct Held {
    pub objects: BTreeSet<ObjectId>,
    pub obtains: BTreeSet<TypeId>,
    pub holding: BTreeSet<TaskId>,
    /// The caller whose turn continues when this holding child's rest ends.
    pub caller: Option<TaskId>,
    /// Missing capture/closure metadata is a conflict, not an empty footprint.
    pub unknown: bool,
}

impl Default for Held {
    fn default() -> Self {
        Self {
            objects: BTreeSet::new(),
            obtains: BTreeSet::new(),
            holding: BTreeSet::new(),
            caller: None,
            unknown: true,
        }
    }
}

impl Held {
    /// Capture scan certified that this task holds no object handles and
    /// cannot obtain any through a keep operation.
    pub fn empty() -> Self {
        Self {
            unknown: false,
            ..Self::default()
        }
    }
}

/// Types form a graph, since a message may hold another Receiver, Mutex,
/// Task, or TaskSet. `register` also covers objects created during run-ahead.
#[derive(Clone, Debug, Default)]
pub struct Carries {
    types: Arc<BTreeMap<TypeId, BTreeSet<TypeId>>>,
    objects: BTreeMap<TypeId, BTreeSet<ObjectId>>,
}

impl Carries {
    pub(super) fn install_types(&mut self, types: BTreeMap<TypeId, BTreeSet<TypeId>>) {
        if self.types.is_empty() {
            self.types = Arc::new(types);
        } else {
            Arc::make_mut(&mut self.types).extend(types);
        }
    }

    pub fn define(&mut self, ty: TypeId, carries: BTreeSet<TypeId>) {
        Arc::make_mut(&mut self.types).insert(ty, carries);
    }

    pub fn register(&mut self, ty: TypeId, object: ObjectId) {
        self.objects.entry(ty).or_default().insert(object);
    }

    /// Returns false when metadata cannot certify disjointness. Missing types
    /// are deliberately different from types with an explicitly empty closure.
    pub fn disjoint(
        &self,
        task: TaskId,
        held: &BTreeMap<TaskId, Held>,
        operation: &Footprint,
    ) -> bool {
        let mut tasks = vec![task];
        let mut seen_tasks = BTreeSet::new();
        let mut types = Vec::new();
        while let Some(task) = tasks.pop() {
            if !seen_tasks.insert(task) {
                continue;
            }
            let Some(handles) = held.get(&task) else {
                return false;
            };
            if handles.unknown {
                return false;
            }
            if handles.objects.iter().any(|object| {
                operation.reads.contains(&Item::Object(object.clone()))
                    || operation.writes.contains(&Item::Object(object.clone()))
            }) {
                return false;
            }
            types.extend(handles.obtains.iter().copied());
            tasks.extend(handles.holding.iter().copied());
            tasks.extend(handles.caller);
        }
        let mut seen_types = BTreeSet::new();
        while let Some(ty) = types.pop() {
            if !seen_types.insert(ty) {
                continue;
            }
            let Some(carries) = self.types.get(&ty) else {
                return false;
            };
            types.extend(carries);
            if self.objects.get(&ty).is_some_and(|objects| {
                objects.iter().any(|object| {
                    operation.reads.contains(&Item::Object(object.clone()))
                        || operation.writes.contains(&Item::Object(object.clone()))
                })
            }) {
                return false;
            }
        }
        // Parked receiver polls are filtered reads. Their channels are omitted
        // here because the backend must keep early sends pending until retire.
        true
    }
}
