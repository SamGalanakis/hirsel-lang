//! Bounded native hook rows, decoded after issuers are quiescent. Source
//! metadata comes from the compiler's checked plan, independently of the log.
//! Task registrations bind reused scheduler slots to canonical task paths.
//! Nonlocal ownership transport still requires a checked compiler relation.
use super::{Context, Diagnostic, Owner, Storage, TaskPath, Trace, Tracker};
use crate::Site;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: u32 = 7;

/// Copy-only callback rows fit in the preallocated ownership Buffer. No
/// language value, payload hash or native address serves as token identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Row {
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
    /// Empty callable custody, proved by the complete checked code graph.
    pub empty_environment: bool,
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
/// Static checked declaration ordinals for a projected slot location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocationPath {
    pub root: u64,
    pub fields: Vec<u32>,
}
/// A physical child Drop bound independently to its checked declaration path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DropMember {
    pub location: u64,
    pub path: Option<LocationPath>,
    pub layout: u32,
    pub value_type: String,
}
pub type Units = BTreeMap<[u8; 32], BTreeMap<u32, Metadata>>;
/// Closed direct-call targets, derived from checked units rather than log rows.
pub type LocalCalls = BTreeMap<[u8; 32], BTreeSet<[u8; 32]>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Binding,
    Incomplete,
    UnsupportedTask(u64),
    InvalidTask(u64),
    InvalidFrame(u64),
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
    location: u64,
    path: Option<&LocationPath>,
) -> Result<Owner, Error> {
    if location >> 32 != 7 {
        if path.is_some() {
            return Err(Error::InvalidLocation(location));
        }
        return owner(task, frame, parent, environment, location);
    }
    let path = path.ok_or(Error::InvalidLocation(location))?;
    if path.fields.is_empty() || !matches!(path.root >> 32, 1 | 6) {
        return Err(Error::InvalidLocation(location));
    }
    let mut at = owner(task, frame, parent, environment, path.root)?;
    at.path.extend(
        path.fields
            .iter()
            .map(|&field| super::Projection::Field { field }),
    );
    Ok(at)
}

fn owner(
    task: &TaskPath,
    frame: u64,
    parent: Option<u64>,
    environment: Option<&Owner>,
    location: u64,
) -> Result<Owner, Error> {
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
    decode(log, program, units, None, limit)
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
    decode(log, program, units, Some(calls), limit)
}

fn decode(
    log: &Log,
    program: [u8; 32],
    units: &Units,
    calls: Option<&LocalCalls>,
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
    }
    struct TaskIdentity {
        depth: u32,
        path: TaskPath,
    }
    let mut tasks = BTreeMap::from([(
        0,
        TaskIdentity {
            depth: 0,
            path: TaskPath::default(),
        },
    )]);
    let mut slots = BTreeMap::<u32, u64>::new();
    let mut paths = BTreeSet::from([TaskPath::default()]);
    let mut stacks: BTreeMap<u64, Vec<Frame>> = BTreeMap::new();
    let mut entered = BTreeSet::new();
    let mut tracker = Tracker::new(limit);
    for &row in &log.rows {
        match row {
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
        let task = match row {
            Row::Enter { task, .. }
            | Row::Event { task, .. }
            | Row::Alias { task, .. }
            | Row::CleanupBegin { task, .. }
            | Row::CleanupDrop { task, .. }
            | Row::CleanupEnd { task, .. }
            | Row::Leave { task, .. } => task,
            Row::Task { .. } | Row::TaskPart { .. } => unreachable!(),
        };
        let identity = tasks.get(&task).ok_or(Error::UnsupportedTask(task))?;
        if task != 0 && slots.get(&(task as u32)) != Some(&task) {
            return Err(Error::InvalidTask(task));
        }
        if identity.path.0.len() != identity.depth as usize {
            return Err(Error::InvalidTask(task));
        }
        let frames = stacks.entry(task).or_default();
        let task = &identity.path;
        match row {
            Row::Task { .. } | Row::TaskPart { .. } => unreachable!(),
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
                    _ => None,
                };
                frames.push(Frame {
                    id: frame,
                    unit,
                    environment,
                    pending: None,
                    cleanup: None,
                });
            }
            Row::Leave { frame, .. } => {
                if frames.pop().is_none_or(|current| {
                    current.id != frame || current.pending.is_some() || current.cleanup.is_some()
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
                    operation: 0,
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
                let Some(current) = frames.last() else {
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
                let parent = frames.iter().rev().nth(1).map(|frame| frame.id);
                let local = |location: u64| {
                    matches!(location >> 32, 0 | 1) || location >> 32 == u64::from(u32::MAX)
                };
                if calls.is_none() && (!local(token) || !local(recipient)) {
                    return Err(Error::InvalidLocation(token.max(recipient)));
                }
                let from = projected_owner(
                    task,
                    frame,
                    parent,
                    current.environment.as_ref(),
                    token,
                    token_path,
                )?;
                let to = projected_owner(
                    task,
                    frame,
                    parent,
                    current.environment.as_ref(),
                    recipient,
                    owner_path,
                )?;
                let context = Context {
                    task: task.clone(),
                    operation: 0,
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
                        tracker
                            .move_tree(&context, &from, &to)
                            .map_err(Error::Consistency)?;
                    }
                    // A callable shell has no Resource identity. Its
                    // checked physical retirement is complete only when every
                    // actual captured resource already left or was dropped.
                    // Never infer descendant resource drops from the shell.
                    4 if from == to => {
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
    if stacks.values().any(|frames| !frames.is_empty())
        || entered.is_empty()
        || tasks
            .values()
            .any(|task| task.path.0.len() != task.depth as usize)
    {
        return Err(Error::Incomplete);
    }
    tracker.finish(program, true).map_err(Error::Consistency)
}
