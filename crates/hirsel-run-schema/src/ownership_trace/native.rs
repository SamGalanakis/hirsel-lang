//! Bounded native hook rows, decoded after issuers are quiescent. Source
//! metadata comes from the compiler's checked plan, independently of the log.
//! Task registrations bind reused scheduler slots to canonical task paths.
//! Nonlocal ownership transport still requires a checked compiler relation.
use super::{Context, Diagnostic, Owner, Storage, TaskPath, Trace, Tracker};
use crate::Site;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod cleanups;
mod queues;
mod results;
mod transfers;

pub const VERSION: u32 = 16;
const MUTABLE_PARAMETER: u64 = (u32::MAX - 1) as u64;

/// Copy-only callback rows fit in the preallocated ownership Buffer. No
/// language value, payload hash or native address serves as token identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Row {
    /// The producer observed an unbound transport. Source execution continues.
    ObservationLost,
    /// Runtime-owned canonical identity, published before this task can run.
    Task {
        task: u64,
        depth: u32,
    },
    /// One canonical child ordinal; rows can interleave across registrations.
    TaskPart {
        task: u64,
        index: u32,
        ordinal: u32,
    },
    /// Actual spawn commitment, bound to the compiler-authored source intent.
    Spawn {
        task: u64,
        frame: u64,
        marker: u32,
        child: u64,
    },
    /// Runtime completion after the compiled body and environment cleanup.
    TaskEnd {
        task: u64,
    },
    TaskResultHeld {
        task: u64,
        frame: u64,
        marker: u32,
        layout: u32,
    },
    TaskResultJoin {
        task: u64,
        frame: u64,
        marker: u32,
        child: u64,
    },
    TaskResultMoved {
        task: u64,
        frame: u64,
        marker: u32,
        child: u64,
        source_frame: u64,
        source_marker: u32,
        layout: u32,
    },
    TaskResultDiscarded {
        task: u64,
        frame: u64,
        marker: u32,
    },
    TaskResultUnavailable {
        task: u64,
        frame: u64,
        marker: u32,
        child: u64,
        status: u32,
    },
    /// Canonical operation ordinal assigned by this task's runtime policy.
    /// This is a diagnostic association, never a clock or a retirement proof.
    Operation {
        task: u64,
        ordinal: u64,
    },
    /// Runtime-owned channel identity and its exact Option payload descriptor.
    /// These rows provide no Resource identity or destruction authority.
    QueueCreated {
        task: u64,
        queue: u64,
        layout: u32,
    },
    QueueSent {
        task: u64,
        queue: u64,
        layout: u32,
        item: u64,
        status: u32,
        pending: bool,
    },
    QueuePublished {
        task: u64,
        queue: u64,
        layout: u32,
        item: u64,
    },
    QueueReceived {
        task: u64,
        queue: u64,
        layout: u32,
        item: u64,
        status: u32,
    },
    /// Receiver close detached [first, end). Payload cleanup has not finished.
    QueueDetached {
        task: u64,
        queue: u64,
        layout: u32,
        first: u64,
        end: u64,
    },
    /// Actual per-image cleanup after receiver detachment, outside queue locks.
    QueueCleanupBegin {
        task: u64,
        queue: u64,
        layout: u32,
        item: u64,
    },
    /// The completed step of the independently checked payload descriptor.
    QueueCleanupDrop {
        task: u64,
        queue: u64,
        item: u64,
        step: u32,
    },
    QueueCleanupEnd {
        task: u64,
        queue: u64,
        item: u64,
    },
    /// A checked caller binding consumed at the actual queue transition.
    QueueTransfer {
        task: u64,
        frame: u64,
        marker: u32,
        queue: u64,
        layout: u32,
        item: u64,
        direction: ChannelDirection,
        status: u32,
    },
    /// Cancellation checked before touching FIFO state. No payload moves.
    QueueCanceled {
        task: u64,
        frame: u64,
        marker: u32,
        queue: u64,
        layout: u32,
        status: u32,
    },
    Enter {
        task: u64,
        frame: u64,
        unit: [u8; 32],
    },
    Event {
        task: u64,
        frame: u64,
        marker: u32,
        action: u32,
        token: u64,
        owner: u64,
        site: u64,
        layout: u32,
    },
    /// Internal checked environment relation, never an ownership event.
    Alias {
        task: u64,
        frame: u64,
        marker: u32,
        view: u64,
        owner: u64,
        site: u64,
        layout: u32,
    },
    CleanupBegin {
        task: u64,
        frame: u64,
        marker: u32,
    },
    CleanupDrop {
        task: u64,
        frame: u64,
        marker: u32,
        field: u32,
        layout: u32,
    },
    CleanupEnd {
        task: u64,
        frame: u64,
        marker: u32,
    },
    Leave {
        task: u64,
        frame: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Log {
    pub version: u32,
    pub program: [u8; 32],
    pub rows: Vec<Row>,
    pub row_count: u64,
    pub complete: bool,
}

/// Immutable static expectations from the checked compiler plan. These are
/// supplied separately, never reconstructed from the native row's assertions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    pub action: u32,
    pub channel: Option<ChannelBinding>,
    pub task_result: Option<TaskResultBinding>,
    /// Empty callable custody, proved by the complete checked code graph.
    pub empty_environment: bool,
    /// The checked structural carrier has a Resource-free alternative. An
    /// absent subtree never authorizes overwriting retained destination credit.
    pub optional_carrier: bool,
    /// Independently checked exact call-site target, present only for Alias.
    pub callee: Option<[u8; 32]>,
    /// Declaration ordinals and unit-local layouts in actual descriptor drop order.
    pub cleanup: Option<Vec<(u32, u32)>>,
    /// Exact declaration path for every physical cleanup binding key.
    pub cleanup_paths: BTreeMap<u32, Vec<u32>>,
    /// Other checked child completions of this same original compound Drop.
    /// Every row still names its exact physical child and original source id.
    pub additional_drops: Vec<DropMember>,
    pub token: u64,
    pub owner: u64,
    pub token_path: Option<LocationPath>,
    pub owner_path: Option<LocationPath>,
    pub site: u64,
    pub layout: u32,
    pub source: Site,
    pub value_type: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelDirection {
    Send,
    Receive,
}

/// The compiler derives this from a checked intrinsic call and the linked
/// descriptor registry. A runtime row cannot supply its own binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelBinding {
    pub direction: ChannelDirection,
    pub descriptor: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskResultDirection {
    Hold,
    Join,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskResultBinding {
    pub direction: TaskResultDirection,
    pub descriptor: u32,
}
/// Static checked declaration ordinals for a projected slot location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocationPath {
    pub root: u64,
    pub fields: Vec<super::Projection>,
}
/// A physical child Drop bound independently to its checked declaration path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DropMember {
    pub location: u64,
    pub path: Option<LocationPath>,
    pub layout: u32,
    pub value_type: String,
}
/// Marker records live outside the tree nodes. Sparse per-unit maps otherwise
/// reserve eleven full metadata records even for a single ownership marker.
pub type Units = BTreeMap<[u8; 32], BTreeMap<u32, Box<Metadata>>>;
/// Closed direct-call targets, derived from checked units rather than log rows.
pub type LocalCalls = BTreeMap<[u8; 32], BTreeSet<[u8; 32]>>;

/// Independently derived declaration path and finalizer source of one completed
/// descriptor step. Runtime rows contain neither this path nor this source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupBinding {
    pub path: Vec<super::Projection>,
    pub source: Site,
}
/// Linked payload descriptor index, then its exact Resource completion steps.
pub type CleanupLayouts = BTreeMap<u32, BTreeMap<u32, Box<CleanupBinding>>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Binding,
    Incomplete,
    UnsupportedTask(u64),
    InvalidTask(u64),
    InvalidOperation { task: u64, ordinal: u64 },
    InvalidFrame(u64),
    InvalidQueue(u64),
    UnsupportedQueueCleanup(u64),
    UnknownUnit([u8; 32]),
    InvalidMarker(u32),
    InvalidLocation(u64),
    RetainedEnvironment(u32),
    Consistency(Diagnostic),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Consistency(error) => error.fmt(f),
            error => write!(f, "native ownership trace: {error:?}"),
        }
    }
}
impl std::error::Error for Error {}

fn projected_owner(
    task: &TaskPath,
    frame: u64,
    parent: Option<u64>,
    environment: Option<&Owner>,
    parameters: &BTreeMap<u32, Owner>,
    location: u64,
    path: Option<&LocationPath>,
) -> Result<Owner, Error> {
    if location >> 32 >= 8 && location >> 32 < MUTABLE_PARAMETER {
        if let Some(path) = path {
            let [super::Projection::VariantField { field, .. }] = path.fields.as_slice() else {
                return Err(Error::InvalidLocation(location));
            };
            if path.root != location & u64::from(u32::MAX)
                || u64::from(*field) != (location >> 32) - 8
            {
                return Err(Error::InvalidLocation(location));
            }
            let mut at = owner(task, frame, parent, environment, parameters, path.root)?;
            at.path.extend(path.fields.iter().cloned());
            return Ok(at);
        }
    }
    if location >> 32 != 7 {
        if path.is_some() {
            return Err(Error::InvalidLocation(location));
        }
        return owner(task, frame, parent, environment, parameters, location);
    }
    let path = path.ok_or(Error::InvalidLocation(location))?;
    if path.fields.is_empty() || !matches!(path.root >> 32, 1 | 6 | MUTABLE_PARAMETER) {
        return Err(Error::InvalidLocation(location));
    }
    let mut at = owner(task, frame, parent, environment, parameters, path.root)?;
    if path.fields.iter().any(|projection| {
        !matches!(
            projection,
            super::Projection::Field { .. } | super::Projection::VariantField { .. }
        )
    }) {
        return Err(Error::InvalidLocation(location));
    }
    at.path.extend(path.fields.iter().cloned());
    Ok(at)
}

fn owner(
    task: &TaskPath,
    frame: u64,
    parent: Option<u64>,
    environment: Option<&Owner>,
    parameters: &BTreeMap<u32, Owner>,
    location: u64,
) -> Result<Owner, Error> {
    if location >> 32 == MUTABLE_PARAMETER {
        return parameters
            .get(&(location as u32))
            .cloned()
            .ok_or(Error::InvalidLocation(location));
    }
    if location >> 32 == u64::from(u32::MAX) {
        return Ok(Owner {
            task: Some(task.clone()),
            storage: Storage::Object {
                name: "native.parallel".into(),
                id: frame,
                item: 0,
            },
            path: vec![super::Projection::Field {
                field: location as u32,
            }],
        });
    }
    if location >> 32 == 6 {
        let mut owner = environment
            .cloned()
            .ok_or(Error::InvalidLocation(location))?;
        owner.path.push(super::Projection::Field {
            field: location as u32,
        });
        return Ok(owner);
    }
    if location >> 32 >= 8 {
        let mut owner = owner(
            task,
            frame,
            parent,
            environment,
            parameters,
            location & u64::from(u32::MAX),
        )?;
        owner.path.push(super::Projection::Field {
            field: (location >> 32) as u32 - 8,
        });
        return Ok(owner);
    }
    let index = location as u32;
    let storage = match location >> 32 {
        0 => Storage::Value {
            frame,
            value: index,
        },
        1 => Storage::Slot { frame, slot: index },
        2 => Storage::Argument {
            call: frame,
            parameter: index,
        },
        3 => Storage::Argument {
            call: parent.ok_or(Error::InvalidLocation(location))?,
            parameter: index,
        },
        4 if index == 0 => Storage::Return {
            call: parent.ok_or(Error::InvalidLocation(location))?,
        },
        5 if index == 0 => Storage::Return { call: frame },
        _ => return Err(Error::InvalidLocation(location)),
    };
    Ok(Owner {
        task: Some(task.clone()),
        storage,
        path: vec![],
    })
}

/// Decode actual committed hook calls. Static correspondence, fresh dynamic
/// frames, marker operands and token/owner consistency are independent checks.
/// A successful result establishes this staged adapter's lineage, not compiler
/// transport coverage before Ready or the RPC admission contract.
pub fn decode_leaf(
    log: &Log,
    program: [u8; 32],
    units: &Units,
    limit: usize,
) -> Result<Trace, Error> {
    decode(log, program, units, None, None, limit)
}

/// Local lifetimes may nest only across a compiler-checked call without owned
/// input/output transport. The original leaf decoder retains its strict policy.
pub fn decode_local_calls(
    log: &Log,
    program: [u8; 32],
    units: &Units,
    calls: &LocalCalls,
    limit: usize,
) -> Result<Trace, Error> {
    if calls.iter().any(|(parent, targets)| {
        !units.contains_key(parent) || targets.iter().any(|target| !units.contains_key(target))
    }) {
        return Err(Error::Binding);
    }
    decode(log, program, units, Some(calls), None, limit)
}

/// Decode runtime-owned image cleanup with independently checked descriptor
/// programs, in addition to the existing source markers and call bindings.
pub fn decode_runtime(
    log: &Log,
    program: [u8; 32],
    units: &Units,
    calls: &LocalCalls,
    cleanups: &CleanupLayouts,
    limit: usize,
) -> Result<Trace, Error> {
    if calls.iter().any(|(parent, targets)| {
        !units.contains_key(parent) || targets.iter().any(|target| !units.contains_key(target))
    }) {
        return Err(Error::Binding);
    }
    decode(log, program, units, Some(calls), Some(cleanups), limit)
}

fn decode(
    log: &Log,
    program: [u8; 32],
    units: &Units,
    calls: Option<&LocalCalls>,
    cleanup_layouts: Option<&CleanupLayouts>,
    limit: usize,
) -> Result<Trace, Error> {
    if log.version != VERSION || log.program != program {
        return Err(Error::Binding);
    }
    if !log.complete || log.row_count != log.rows.len() as u64 || log.rows.len() > limit {
        return Err(Error::Incomplete);
    }
    for markers in units.values() {
        for (&id, metadata) in markers {
            if metadata.optional_carrier
                && (!matches!(metadata.action, 1 | 8 | 9 | 10)
                    || metadata.empty_environment
                    || metadata.cleanup.is_some())
            {
                return Err(Error::InvalidMarker(id));
            }
            if matches!(metadata.action, 9 | 10) != metadata.task_result.is_some()
                || metadata.task_result.is_some_and(|binding| {
                    let (action, parameter) = match binding.direction {
                        TaskResultDirection::Hold => (9, 0),
                        TaskResultDirection::Join => (10, 2),
                    };
                    metadata.action != action
                        || metadata.owner != ((2u64 << 32) | parameter)
                        || metadata.channel.is_some()
                        || metadata.callee.is_some()
                        || metadata.empty_environment
                        || metadata.cleanup.is_some()
                        || !metadata.cleanup_paths.is_empty()
                        || !metadata.additional_drops.is_empty()
                })
            {
                return Err(Error::InvalidMarker(id));
            }
            if (metadata.action == 8) != metadata.channel.is_some()
                || (metadata.channel.is_some()
                    && (metadata.owner != ((2u64 << 32) | 1)
                        || metadata.callee.is_some()
                        || metadata.empty_environment
                        || metadata.cleanup.is_some()
                        || !metadata.cleanup_paths.is_empty()
                        || !metadata.additional_drops.is_empty()))
            {
                return Err(Error::InvalidMarker(id));
            }
            if metadata.action == 7
                && (metadata.token != metadata.owner
                    || metadata.token_path != metadata.owner_path
                    || metadata.empty_environment
                    || metadata.callee.is_some()
                    || metadata.cleanup.is_some()
                    || !metadata.cleanup_paths.is_empty())
            {
                return Err(Error::InvalidMarker(id));
            }
            if metadata.additional_drops.is_empty() {
                continue;
            }
            if metadata.action != 2
                || metadata.token != metadata.owner
                || metadata.token_path != metadata.owner_path
                || metadata.callee.is_some()
                || metadata.cleanup.is_some()
            {
                return Err(Error::InvalidMarker(id));
            }
            let mut seen = BTreeSet::from([(metadata.token, metadata.layout)]);
            for member in &metadata.additional_drops {
                if !seen.insert((member.location, member.layout)) {
                    return Err(Error::InvalidMarker(id));
                }
            }
        }
    }
    struct Frame {
        id: u64,
        unit: [u8; 32],
        environment: Option<Owner>,
        pending: Option<(Owner, [u8; 32])>,
        cleanup: Option<(u32, Owner, Vec<(u32, u32, Owner)>)>,
        parameters: BTreeMap<u32, Owner>,
        mutable_call: BTreeMap<u32, (Owner, [u8; 32])>,
        channel: Option<transfers::Intent>,
    }
    struct TaskIdentity {
        depth: u32,
        path: TaskPath,
        operation: Option<u64>,
    }
    let mut tasks = BTreeMap::from([(
        0,
        TaskIdentity {
            depth: 0,
            path: TaskPath::default(),
            operation: None,
        },
    )]);
    let mut slots = BTreeMap::<u32, u64>::new();
    let mut paths = BTreeSet::from([TaskPath::default()]);
    let mut stacks: BTreeMap<u64, Vec<Frame>> = BTreeMap::new();
    let mut entered = BTreeSet::new();
    struct SpawnIntent {
        owner: Owner,
        callee: [u8; 32],
        source: Site,
        empty: bool,
    }
    struct SpawnEnvironment {
        owner: Owner,
        callee: [u8; 32],
        entered: bool,
    }
    let mut spawns = BTreeMap::<(u64, u64, u32), SpawnIntent>::new();
    let mut environments = BTreeMap::<u64, SpawnEnvironment>::new();
    let mut ended_tasks = BTreeSet::new();
    let mut tracker = Tracker::new(limit);
    let mut queues = queues::Queues::default();
    let mut cleanups = cleanups::Cleanups::default();
    let mut results = results::Results::default();
    for &row in &log.rows {
        match row {
            Row::ObservationLost => return Err(Error::Incomplete),
            Row::Task { task, depth } => {
                let slot = task as u32;
                let generation = task >> 32;
                if slot == 0
                    || generation == 0
                    || depth == 0
                    || depth as usize > limit
                    || tasks.contains_key(&task)
                {
                    return Err(Error::InvalidTask(task));
                }
                if let Some(&old_key) = slots.get(&slot) {
                    let old = &tasks[&old_key];
                    if generation <= old_key >> 32
                        || old.path.0.len() != old.depth as usize
                        || stacks
                            .get(&old_key)
                            .is_some_and(|frames| !frames.is_empty())
                        || tracker
                            .inventory()
                            .owners
                            .keys()
                            .any(|owner| owner.task.as_ref() == Some(&old.path))
                    {
                        return Err(Error::InvalidTask(task));
                    }
                }
                slots.insert(slot, task);
                tasks.insert(
                    task,
                    TaskIdentity {
                        depth,
                        path: TaskPath::default(),
                        operation: None,
                    },
                );
                continue;
            }
            Row::TaskPart {
                task,
                index,
                ordinal,
            } => {
                let identity = tasks.get_mut(&task).ok_or(Error::InvalidTask(task))?;
                if task == 0
                    || slots.get(&(task as u32)) != Some(&task)
                    || index != identity.path.0.len() as u32
                    || index >= identity.depth
                {
                    return Err(Error::InvalidTask(task));
                }
                identity.path.0.push(ordinal);
                if identity.path.0.len() == identity.depth as usize {
                    let parent = TaskPath(identity.path.0[..identity.path.0.len() - 1].to_vec());
                    if !paths.contains(&parent) || !paths.insert(identity.path.clone()) {
                        return Err(Error::InvalidTask(task));
                    }
                }
                continue;
            }
            _ => (),
        }
        if let Row::Operation { task, ordinal } = row {
            let identity = tasks.get_mut(&task).ok_or(Error::UnsupportedTask(task))?;
            if (task != 0 && slots.get(&(task as u32)) != Some(&task))
                || identity.path.0.len() != identity.depth as usize
            {
                return Err(Error::InvalidTask(task));
            }
            // Ordered starts at zero; Free's tape starts at one. Subsequent
            // entries must be contiguous within the same canonical task.
            if identity
                .operation
                .map_or(ordinal > 1, |prior| prior.checked_add(1) != Some(ordinal))
            {
                return Err(Error::InvalidOperation { task, ordinal });
            }
            identity.operation = Some(ordinal);
            continue;
        }
        let task = match row {
            Row::Enter { task, .. }
            | Row::Event { task, .. }
            | Row::Alias { task, .. }
            | Row::CleanupBegin { task, .. }
            | Row::CleanupDrop { task, .. }
            | Row::CleanupEnd { task, .. }
            | Row::Leave { task, .. }
            | Row::Spawn { task, .. }
            | Row::TaskEnd { task }
            | Row::TaskResultHeld { task, .. }
            | Row::TaskResultJoin { task, .. }
            | Row::TaskResultMoved { task, .. }
            | Row::TaskResultDiscarded { task, .. }
            | Row::TaskResultUnavailable { task, .. }
            | Row::QueueCreated { task, .. }
            | Row::QueueSent { task, .. }
            | Row::QueuePublished { task, .. }
            | Row::QueueReceived { task, .. }
            | Row::QueueDetached { task, .. }
            | Row::QueueCleanupBegin { task, .. }
            | Row::QueueCleanupDrop { task, .. }
            | Row::QueueCleanupEnd { task, .. }
            | Row::QueueTransfer { task, .. }
            | Row::QueueCanceled { task, .. } => task,
            Row::ObservationLost
            | Row::Task { .. }
            | Row::TaskPart { .. }
            | Row::Operation { .. } => unreachable!(),
        };
        let identity = tasks.get(&task).ok_or(Error::UnsupportedTask(task))?;
        if task != 0 && slots.get(&(task as u32)) != Some(&task) {
            return Err(Error::InvalidTask(task));
        }
        if identity.path.0.len() != identity.depth as usize {
            return Err(Error::InvalidTask(task));
        }
        if ended_tasks.contains(&task) {
            return Err(Error::InvalidTask(task));
        }
        if let Row::TaskResultJoin { child, .. }
        | Row::TaskResultMoved { child, .. }
        | Row::TaskResultUnavailable { child, .. } = row
            && (child == 0 || child == task || !tasks.contains_key(&child))
        {
            return Err(Error::InvalidTask(child));
        }
        if let Row::TaskResultMoved { child, .. } = row
            && (ended_tasks.contains(&child) || slots.get(&(child as u32)) != Some(&child))
        {
            return Err(Error::InvalidTask(child));
        }
        if let Row::TaskResultMoved {
            task,
            frame,
            child,
            source_frame,
            ..
        } = row
        {
            for (issuer, expected_frame) in [(task, frame), (child, source_frame)] {
                if stacks.get(&issuer).is_none_or(|frames| {
                    !frames
                        .iter()
                        .rev()
                        .any(|active| active.id == expected_frame)
                }) {
                    return Err(Error::InvalidFrame(expected_frame));
                }
            }
        }
        if results.observe(
            row,
            stacks
                .get(&task)
                .and_then(|frames| frames.last())
                .map(|frame| frame.id),
            &identity.path,
            identity.operation.unwrap_or(0),
            &mut tracker,
        )? {
            continue;
        }
        if let Row::QueueDetached {
            queue,
            layout,
            first,
            end,
            ..
        } = row
            && tracker.inventory().owners.keys().any(|owner| {
                matches!(&owner.storage, Storage::Object { name, id, item }
                    if name == "native.channel" && *id == queue && (first..end).contains(item))
            })
            && !cleanup_layouts
                .and_then(|layouts| layouts.get(&layout))
                .is_some_and(|bindings| !bindings.is_empty())
        {
            // Raw detachment is not proof that a Resource finalizer completed.
            return Err(Error::UnsupportedQueueCleanup(queue));
        }
        if queues.observe(row)? {
            continue;
        }
        if cleanups.observe(
            row,
            &identity.path,
            identity.operation.unwrap_or(0),
            cleanup_layouts,
            &mut queues,
            &mut tracker,
        )? {
            continue;
        }
        if let Row::Spawn {
            frame,
            marker,
            child,
            ..
        } = row
        {
            let intent = spawns
                .remove(&(task, frame, marker))
                .ok_or(Error::InvalidMarker(marker))?;
            let destination = tasks.get(&child).ok_or(Error::InvalidTask(child))?;
            if child == 0
                || destination.path.0.len() != destination.depth as usize
                || destination.path.0.len() != identity.path.0.len() + 1
                || !destination.path.0.starts_with(&identity.path.0)
                || environments.contains_key(&child)
            {
                return Err(Error::InvalidTask(child));
            }
            let owner = Owner {
                task: Some(destination.path.clone()),
                storage: Storage::Object {
                    name: "native.task_environment".into(),
                    id: 0,
                    item: 0,
                },
                path: vec![],
            };
            let context = Context {
                task: destination.path.clone(),
                operation: 0,
                site: intent.source,
            };
            if intent.empty {
                if tracker.inventory().owners.keys().any(|at| {
                    [&intent.owner, &owner].iter().any(|root| {
                        at.task == root.task
                            && at.storage == root.storage
                            && at.path.starts_with(&root.path)
                    })
                }) {
                    return Err(Error::RetainedEnvironment(marker));
                }
            } else {
                let moved = tracker
                    .move_tree(&context, &intent.owner, &owner)
                    .map_err(Error::Consistency)?;
                if moved.is_empty() {
                    return Err(Error::RetainedEnvironment(marker));
                }
            }
            environments.insert(
                child,
                SpawnEnvironment {
                    owner,
                    callee: intent.callee,
                    entered: false,
                },
            );
            continue;
        }
        if let Row::TaskEnd { .. } = row {
            if task == 0 || stacks.get(&task).is_some_and(|frames| !frames.is_empty()) {
                return Err(Error::InvalidTask(task));
            }
            if let Some(environment) = environments.remove(&task)
                && (!environment.entered
                    || tracker.inventory().owners.keys().any(|owner| {
                        owner.task == environment.owner.task
                            && owner.storage == environment.owner.storage
                            && owner.path.starts_with(&environment.owner.path)
                    }))
            {
                return Err(Error::RetainedEnvironment(u32::MAX));
            }
            ended_tasks.insert(task);
            continue;
        }
        let frames = stacks.entry(task).or_default();
        let task = &identity.path;
        let operation = identity.operation.unwrap_or(0);
        match row {
            Row::QueueCanceled {
                frame,
                marker,
                queue,
                layout,
                status,
                ..
            } => {
                let current = frames.last_mut().ok_or(Error::InvalidFrame(frame))?;
                if current.id != frame {
                    return Err(Error::InvalidFrame(frame));
                }
                let intent = current.channel.take().ok_or(Error::InvalidMarker(marker))?;
                if intent.marker != marker
                    || intent.binding.direction != ChannelDirection::Receive
                    || intent.binding.descriptor != layout
                    || status < 2
                {
                    return Err(Error::InvalidMarker(marker));
                }
                queues.known(queue, layout)?;
                intent.commit(&mut tracker, queue, 0, status, operation)?;
            }
            Row::QueueTransfer {
                task: issuer,
                frame,
                marker,
                queue,
                layout,
                item,
                direction,
                status,
            } => {
                let current = frames.last_mut().ok_or(Error::InvalidFrame(frame))?;
                if current.id != frame {
                    return Err(Error::InvalidFrame(frame));
                }
                let intent = current.channel.take().ok_or(Error::InvalidMarker(marker))?;
                if intent.marker != marker
                    || intent.binding.direction != direction
                    || intent.binding.descriptor != layout
                {
                    return Err(Error::InvalidMarker(marker));
                }
                queues.bind(
                    queue,
                    queues::Receipt {
                        task: issuer,
                        layout,
                        item,
                        direction,
                        status,
                    },
                )?;
                intent.commit(&mut tracker, queue, item, status, operation)?;
            }
            Row::ObservationLost
            | Row::Task { .. }
            | Row::TaskPart { .. }
            | Row::Operation { .. }
            | Row::Spawn { .. }
            | Row::TaskEnd { .. }
            | Row::TaskResultHeld { .. }
            | Row::TaskResultJoin { .. }
            | Row::TaskResultMoved { .. }
            | Row::TaskResultDiscarded { .. }
            | Row::TaskResultUnavailable { .. }
            | Row::QueueCreated { .. }
            | Row::QueueSent { .. }
            | Row::QueuePublished { .. }
            | Row::QueueReceived { .. }
            | Row::QueueDetached { .. }
            | Row::QueueCleanupBegin { .. }
            | Row::QueueCleanupDrop { .. }
            | Row::QueueCleanupEnd { .. } => unreachable!(),
            Row::Enter { frame, unit, .. } => {
                if frame == 0 || !entered.insert(frame) {
                    return Err(Error::InvalidFrame(frame));
                }
                let metadata = units.get(&unit).ok_or(Error::UnknownUnit(unit))?;
                // Descriptor finalizers can enter an emitted body even when
                // Ready has no call instruction. Empty checked units carry no
                // tracked input/output or ownership events across this frame.
                let allowed = frames.last().is_some_and(|parent| {
                    calls
                        .and_then(|calls| calls.get(&parent.unit))
                        .is_some_and(|targets| targets.contains(&unit))
                });
                if !frames.is_empty() && !metadata.is_empty() && !allowed {
                    return Err(Error::InvalidFrame(frame));
                }
                let runtime_environment = if frames.is_empty() {
                    if let Some(binding) = environments.get_mut(&match row {
                        Row::Enter { task, .. } => task,
                        _ => unreachable!(),
                    }) {
                        if binding.entered || binding.callee != unit {
                            return Err(Error::InvalidFrame(frame));
                        }
                        binding.entered = true;
                        Some(binding.owner.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };
                let environment = match frames.last_mut() {
                    Some(parent)
                        if parent
                            .pending
                            .as_ref()
                            .is_some_and(|(_, expected)| *expected == unit) =>
                    {
                        if !allowed {
                            return Err(Error::InvalidFrame(frame));
                        }
                        parent.pending.take().map(|(owner, _)| owner)
                    }
                    Some(parent)
                        if parent.pending.is_some() && (allowed || !metadata.is_empty()) =>
                    {
                        return Err(Error::InvalidFrame(frame));
                    }
                    _ => runtime_environment,
                };
                let parameters = if let Some(parent) = frames.last_mut() {
                    if parent
                        .mutable_call
                        .values()
                        .any(|(_, target)| *target != unit)
                        || (!parent.mutable_call.is_empty() && !allowed)
                    {
                        return Err(Error::InvalidFrame(frame));
                    }
                    std::mem::take(&mut parent.mutable_call)
                        .into_iter()
                        .map(|(parameter, (owner, _))| (parameter, owner))
                        .collect()
                } else {
                    BTreeMap::new()
                };
                frames.push(Frame {
                    id: frame,
                    unit,
                    environment,
                    pending: None,
                    cleanup: None,
                    parameters,
                    mutable_call: BTreeMap::new(),
                    channel: None,
                });
            }
            Row::Leave { frame, .. } => {
                if frames.pop().is_none_or(|current| {
                    current.id != frame
                        || current.pending.is_some()
                        || current.cleanup.is_some()
                        || !current.mutable_call.is_empty()
                        || current.channel.is_some()
                }) {
                    return Err(Error::InvalidFrame(frame));
                }
            }
            Row::CleanupBegin { frame, marker, .. } => {
                let current = frames.last_mut().ok_or(Error::InvalidFrame(frame))?;
                if current.id != frame || current.cleanup.is_some() {
                    return Err(Error::InvalidFrame(frame));
                }
                let metadata = units[&current.unit]
                    .get(&marker)
                    .ok_or(Error::InvalidMarker(marker))?;
                if metadata.action != 4 || metadata.token != metadata.owner {
                    return Err(Error::InvalidMarker(marker));
                }
                let fields = metadata
                    .cleanup
                    .as_ref()
                    .ok_or(Error::InvalidMarker(marker))?;
                let parent = None; // cleanup holders are local SSA/slots or checked captures
                let root = projected_owner(
                    task,
                    frame,
                    parent,
                    current.environment.as_ref(),
                    &current.parameters,
                    metadata.token,
                    metadata.token_path.as_ref(),
                )?;
                if metadata.cleanup_paths.len() != fields.len() {
                    return Err(Error::InvalidMarker(marker));
                }
                let mut live = Vec::new();
                let mut seen = BTreeSet::new();
                for &(field, layout) in fields {
                    let path = metadata
                        .cleanup_paths
                        .get(&field)
                        .ok_or(Error::InvalidMarker(marker))?;
                    if path.is_empty() || !seen.insert(path.clone()) {
                        return Err(Error::InvalidMarker(marker));
                    }
                    let mut at = root.clone();
                    at.path
                        .extend(path.iter().map(|&field| super::Projection::Field { field }));
                    if tracker.inventory().owners.contains_key(&at) {
                        live.push((field, layout, at));
                    }
                }
                current.cleanup = Some((marker, root, live));
            }
            Row::CleanupDrop {
                frame,
                marker,
                field,
                layout,
                ..
            } => {
                let current = frames.last_mut().ok_or(Error::InvalidFrame(frame))?;
                if current.id != frame {
                    return Err(Error::InvalidFrame(frame));
                }
                let Some((expected, _, fields)) = &mut current.cleanup else {
                    return Err(Error::InvalidMarker(marker));
                };
                if *expected != marker
                    || fields
                        .first()
                        .is_none_or(|(key, kind, _)| (*key, *kind) != (field, layout))
                {
                    return Err(Error::InvalidMarker(marker));
                }
                let (_, _, at) = fields.remove(0);
                let source = &units[&current.unit][&marker].source;
                let context = Context {
                    task: task.clone(),
                    operation,
                    site: source.clone(),
                };
                tracker
                    .drop_value(&context, &at)
                    .map_err(Error::Consistency)?;
            }
            Row::CleanupEnd { frame, marker, .. } => {
                let current = frames.last_mut().ok_or(Error::InvalidFrame(frame))?;
                if current.id != frame {
                    return Err(Error::InvalidFrame(frame));
                }
                let Some((expected, root, fields)) = current.cleanup.take() else {
                    return Err(Error::InvalidMarker(marker));
                };
                if expected != marker
                    || !fields.is_empty()
                    || tracker.inventory().owners.keys().any(|at| {
                        at.task == root.task
                            && at.storage == root.storage
                            && at.path.starts_with(&root.path)
                    })
                {
                    return Err(Error::RetainedEnvironment(marker));
                }
            }
            Row::Alias {
                frame,
                marker,
                view,
                owner: target,
                site,
                layout,
                ..
            } => {
                let parent = frames.iter().rev().nth(1).map(|frame| frame.id);
                let current = frames.last_mut().ok_or(Error::InvalidFrame(frame))?;
                if current.id != frame || current.pending.is_some() || calls.is_none() {
                    return Err(Error::InvalidFrame(frame));
                }
                let expected = units[&current.unit]
                    .get(&marker)
                    .ok_or(Error::InvalidMarker(marker))?;
                if (
                    expected.action,
                    expected.token,
                    expected.owner,
                    expected.site,
                    expected.layout,
                ) != (3, view, target, site, layout)
                    || view >> 32 != 0
                    || !matches!(target >> 32, 1 | 6)
                {
                    return Err(Error::InvalidMarker(marker));
                }
                if expected.empty_environment && expected.callee.is_none() {
                    let at = projected_owner(
                        task,
                        frame,
                        parent,
                        current.environment.as_ref(),
                        &current.parameters,
                        target,
                        expected.owner_path.as_ref(),
                    )?;
                    if tracker.inventory().owners.keys().any(|owner| {
                        owner.task == at.task
                            && owner.storage == at.storage
                            && owner.path.starts_with(&at.path)
                    }) {
                        return Err(Error::RetainedEnvironment(marker));
                    }
                    continue;
                }
                let expected_callee = expected.callee.ok_or(Error::InvalidMarker(marker))?;
                if !calls
                    .and_then(|calls| calls.get(&current.unit))
                    .is_some_and(|targets| targets.contains(&expected_callee))
                {
                    return Err(Error::InvalidMarker(marker));
                }
                current.pending = Some((
                    projected_owner(
                        task,
                        frame,
                        parent,
                        current.environment.as_ref(),
                        &current.parameters,
                        target,
                        expected.owner_path.as_ref(),
                    )?,
                    expected_callee,
                ));
            }
            Row::Event {
                frame,
                marker,
                action,
                token,
                owner: recipient,
                site,
                layout,
                ..
            } => {
                let parent = frames.iter().rev().nth(1).map(|frame| frame.id);
                let Some(current) = frames.last_mut() else {
                    return Err(Error::InvalidFrame(frame));
                };
                if current.id != frame {
                    return Err(Error::InvalidFrame(frame));
                }
                let expected = units[&current.unit]
                    .get(&marker)
                    .ok_or(Error::InvalidMarker(marker))?;
                let (token_path, owner_path, value_type) =
                    if (action, token, recipient, site, layout)
                        == (
                            expected.action,
                            expected.token,
                            expected.owner,
                            expected.site,
                            expected.layout,
                        )
                    {
                        (
                            expected.token_path.as_ref(),
                            expected.owner_path.as_ref(),
                            expected.value_type.as_str(),
                        )
                    } else {
                        if expected.action != 2
                            || expected.token != expected.owner
                            || action != 2
                            || token != recipient
                            || site != expected.site
                        {
                            return Err(Error::InvalidMarker(marker));
                        }
                        let mut members = expected
                            .additional_drops
                            .iter()
                            .filter(|member| member.location == token && member.layout == layout);
                        let member = members.next().ok_or(Error::InvalidMarker(marker))?;
                        if members.next().is_some() {
                            return Err(Error::InvalidMarker(marker));
                        }
                        (
                            member.path.as_ref(),
                            member.path.as_ref(),
                            member.value_type.as_str(),
                        )
                    };
                if action == 8 {
                    if calls.is_none() || current.channel.is_some() {
                        return Err(Error::InvalidMarker(marker));
                    }
                    current.channel = Some(transfers::Intent {
                        marker,
                        binding: expected.channel.ok_or(Error::InvalidMarker(marker))?,
                        owner: projected_owner(
                            task,
                            frame,
                            parent,
                            current.environment.as_ref(),
                            &current.parameters,
                            token,
                            token_path,
                        )?,
                        context: Context {
                            task: task.clone(),
                            operation,
                            site: expected.source.clone(),
                        },
                        optional: expected.optional_carrier,
                    });
                    continue;
                }
                if matches!(action, 9 | 10) {
                    let binding = expected.task_result.ok_or(Error::InvalidMarker(marker))?;
                    if calls.is_none()
                        || match binding.direction {
                            TaskResultDirection::Hold => action != 9,
                            TaskResultDirection::Join => action != 10,
                        }
                    {
                        return Err(Error::InvalidMarker(marker));
                    }
                    results.intent(
                        match row {
                            Row::Event { task, .. } => task,
                            _ => unreachable!(),
                        },
                        frame,
                        marker,
                        results::Intent {
                            binding,
                            owner: projected_owner(
                                task,
                                frame,
                                parent,
                                current.environment.as_ref(),
                                &current.parameters,
                                token,
                                token_path,
                            )?,
                            source: expected.source.clone(),
                            optional: expected.optional_carrier,
                        },
                    )?;
                    continue;
                }
                if action == 6 {
                    if calls.is_none() || recipient >> 32 != 2 || expected.empty_environment {
                        return Err(Error::InvalidMarker(marker));
                    }
                    let callee = expected.callee.ok_or(Error::InvalidMarker(marker))?;
                    if !calls
                        .and_then(|calls| calls.get(&current.unit))
                        .is_some_and(|targets| targets.contains(&callee))
                    {
                        return Err(Error::InvalidMarker(marker));
                    }
                    let actual = projected_owner(
                        task,
                        frame,
                        parent,
                        current.environment.as_ref(),
                        &current.parameters,
                        token,
                        token_path,
                    )?;
                    if current
                        .mutable_call
                        .insert(recipient as u32, (actual, callee))
                        .is_some()
                    {
                        return Err(Error::InvalidMarker(marker));
                    }
                    continue;
                }
                let local = |location: u64| {
                    matches!(location >> 32, 0 | 1) || location >> 32 == u64::from(u32::MAX)
                };
                if calls.is_none() && (!local(token) || !local(recipient)) {
                    return Err(Error::InvalidLocation(token.max(recipient)));
                }
                if action == 5 {
                    if expected.owner >> 32 != 2 || expected.owner as u32 != 1 {
                        return Err(Error::InvalidMarker(marker));
                    }
                    let callee = expected.callee.ok_or(Error::InvalidMarker(marker))?;
                    if !calls
                        .and_then(|calls| calls.get(&current.unit))
                        .is_some_and(|targets| targets.contains(&callee))
                    {
                        return Err(Error::InvalidMarker(marker));
                    }
                    let from = projected_owner(
                        task,
                        frame,
                        parent,
                        current.environment.as_ref(),
                        &current.parameters,
                        token,
                        token_path,
                    )?;
                    let to = projected_owner(
                        task,
                        frame,
                        parent,
                        current.environment.as_ref(),
                        &current.parameters,
                        recipient,
                        owner_path,
                    )?;
                    let context = Context {
                        task: task.clone(),
                        operation,
                        site: expected.source.clone(),
                    };
                    if expected.empty_environment {
                        if tracker.inventory().owners.keys().any(|at| {
                            [&from, &to].iter().any(|root| {
                                at.task == root.task
                                    && at.storage == root.storage
                                    && at.path.starts_with(&root.path)
                            })
                        }) {
                            return Err(Error::RetainedEnvironment(marker));
                        }
                    } else {
                        let moved = tracker
                            .move_tree(&context, &from, &to)
                            .map_err(Error::Consistency)?;
                        if moved.is_empty() {
                            return Err(Error::InvalidMarker(marker));
                        }
                    }
                    if spawns
                        .insert(
                            (
                                match row {
                                    Row::Event { task, .. } => task,
                                    _ => unreachable!(),
                                },
                                frame,
                                marker,
                            ),
                            SpawnIntent {
                                owner: to,
                                callee,
                                source: expected.source.clone(),
                                empty: expected.empty_environment,
                            },
                        )
                        .is_some()
                    {
                        return Err(Error::InvalidMarker(marker));
                    }
                    continue;
                }
                // A checked resource-free runtime callback starts without a
                // tracked parent environment. Its callable shell may still
                // move out of a physical capture; that move carries no token.
                let absent_capture = |location: u64, path: Option<&LocationPath>| {
                    current.environment.is_none()
                        && (location >> 32 == 6
                            || (location >> 32 == 7
                                && path.is_some_and(|path| path.root >> 32 == 6)))
                };
                if action == 1
                    && expected.empty_environment
                    && (absent_capture(token, token_path) || absent_capture(recipient, owner_path))
                {
                    for (location, path) in [(token, token_path), (recipient, owner_path)] {
                        if absent_capture(location, path) {
                            continue;
                        }
                        let root = projected_owner(
                            task,
                            frame,
                            parent,
                            current.environment.as_ref(),
                            &current.parameters,
                            location,
                            path,
                        )?;
                        if tracker.inventory().owners.keys().any(|owner| {
                            owner.task == root.task
                                && owner.storage == root.storage
                                && owner.path.starts_with(&root.path)
                        }) {
                            return Err(Error::RetainedEnvironment(marker));
                        }
                    }
                    continue;
                }
                let from = projected_owner(
                    task,
                    frame,
                    parent,
                    current.environment.as_ref(),
                    &current.parameters,
                    token,
                    token_path,
                )?;
                let to = projected_owner(
                    task,
                    frame,
                    parent,
                    current.environment.as_ref(),
                    &current.parameters,
                    recipient,
                    owner_path,
                )?;
                let context = Context {
                    task: task.clone(),
                    operation,
                    site: expected.source.clone(),
                };
                match action {
                    0 if from == to => {
                        tracker
                            .create(&context, to, value_type.to_owned())
                            .map_err(Error::Consistency)?;
                    }
                    1 if expected.empty_environment => {
                        if tracker.inventory().owners.keys().any(|at| {
                            [from.clone(), to.clone()].iter().any(|root| {
                                at.task == root.task
                                    && at.storage == root.storage
                                    && at.path.starts_with(&root.path)
                            })
                        }) {
                            return Err(Error::RetainedEnvironment(marker));
                        }
                    }
                    1 => {
                        let retained = |root: &Owner| {
                            tracker.inventory().owners.keys().any(|at| {
                                at.task == root.task
                                    && at.storage == root.storage
                                    && at.path.starts_with(&root.path)
                            })
                        };
                        if expected.optional_carrier && !retained(&from) {
                            if retained(&to) {
                                return Err(Error::RetainedEnvironment(marker));
                            }
                            continue;
                        }
                        tracker
                            .move_tree(&context, &from, &to)
                            .map_err(Error::Consistency)?;
                    }
                    // Empty storage has no Resource identity. Its
                    // checked physical retirement is complete only when every
                    // actual captured resource already left or was dropped.
                    // Never infer descendant resource drops from the shell.
                    4 | 7 if from == to => {
                        if tracker.inventory().owners.keys().any(|at| {
                            at.task == from.task
                                && at.storage == from.storage
                                && at.path.starts_with(&from.path)
                        }) {
                            return Err(Error::RetainedEnvironment(marker));
                        }
                    }
                    2 if from == to => {
                        tracker
                            .drop_value(&context, &from)
                            .map_err(Error::Consistency)?;
                    }
                    _ => return Err(Error::InvalidMarker(marker)),
                }
            }
        }
    }
    if !spawns.is_empty()
        || !environments.is_empty()
        || stacks.values().any(|frames| !frames.is_empty())
        || entered.is_empty()
        || tasks
            .values()
            .any(|task| task.path.0.len() != task.depth as usize)
    {
        return Err(Error::Incomplete);
    }
    cleanups.finish()?;
    results.finish()?;
    queues.finish_cleanups(cleanup_layouts)?;
    tracker.finish(program, true).map_err(Error::Consistency)
}
