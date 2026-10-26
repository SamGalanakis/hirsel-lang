//! Bounded native hook rows, decoded after issuers are quiescent. Source
//! metadata comes from the compiler's checked plan, independently of the log.
//! This adapter currently refuses nonroot tasks and nonlocal storage.
use super::{Context, Diagnostic, Owner, Storage, TaskPath, Trace, Tracker};
use crate::Site;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: u32 = 3;

/// Copy-only callback rows fit in the preallocated ownership Buffer. No
/// language value, payload hash or native address serves as token identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Row {
    Enter {
        task: u32,
        frame: u64,
        unit: [u8; 32],
    },
    Event {
        task: u32,
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
        task: u32,
        frame: u64,
        marker: u32,
        view: u64,
        owner: u64,
        site: u64,
        layout: u32,
    },
    CleanupBegin {
        task: u32,
        frame: u64,
        marker: u32,
    },
    CleanupDrop {
        task: u32,
        frame: u64,
        marker: u32,
        field: u32,
        layout: u32,
    },
    CleanupEnd {
        task: u32,
        frame: u64,
        marker: u32,
    },
    Leave {
        task: u32,
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
    /// Independently checked exact call-site target, present only for Alias.
    pub callee: Option<[u8; 32]>,
    /// Declaration ordinals and unit-local layouts in actual descriptor drop order.
    pub cleanup: Option<Vec<(u32, u32)>>,
    pub token: u64,
    pub owner: u64,
    pub site: u64,
    pub layout: u32,
    pub source: Site,
    pub value_type: String,
}
pub type Units = BTreeMap<[u8; 32], BTreeMap<u32, Metadata>>;
/// Closed direct-call targets, derived from checked units rather than log rows.
pub type LocalCalls = BTreeMap<[u8; 32], BTreeSet<[u8; 32]>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Binding,
    Incomplete,
    UnsupportedTask(u32),
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

fn owner(
    frame: u64,
    parent: Option<u64>,
    environment: Option<&Owner>,
    location: u64,
) -> Result<Owner, Error> {
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
        let mut owner = owner(frame, parent, environment, location & u64::from(u32::MAX))?;
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
        task: Some(TaskPath::default()),
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
    struct Frame {
        id: u64,
        unit: [u8; 32],
        environment: Option<Owner>,
        pending: Option<(Owner, [u8; 32])>,
        cleanup: Option<(u32, Owner, Vec<(u32, u32)>)>,
    }
    let mut frames: Vec<Frame> = Vec::new();
    let mut entered = BTreeSet::new();
    let mut tracker = Tracker::new(limit);
    for &row in &log.rows {
        let task = match row {
            Row::Enter { task, .. }
            | Row::Event { task, .. }
            | Row::Alias { task, .. }
            | Row::CleanupBegin { task, .. }
            | Row::CleanupDrop { task, .. }
            | Row::CleanupEnd { task, .. }
            | Row::Leave { task, .. } => task,
        };
        if task != 0 {
            return Err(Error::UnsupportedTask(task));
        }
        match row {
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
                let root = owner(frame, parent, current.environment.as_ref(), metadata.token)?;
                let live = fields
                    .iter()
                    .copied()
                    .filter(|&(field, _)| {
                        let mut at = root.clone();
                        at.path.push(super::Projection::Field { field });
                        tracker.inventory().owners.contains_key(&at)
                    })
                    .collect();
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
                let Some((expected, root, fields)) = &mut current.cleanup else {
                    return Err(Error::InvalidMarker(marker));
                };
                if *expected != marker || fields.first() != Some(&(field, layout)) {
                    return Err(Error::InvalidMarker(marker));
                }
                fields.remove(0);
                let mut at = root.clone();
                at.path.push(super::Projection::Field { field });
                let source = &units[&current.unit][&marker].source;
                let context = Context {
                    task: TaskPath::default(),
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
                    owner(frame, parent, current.environment.as_ref(), target)?,
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
                if (action, token, recipient, site, layout)
                    != (
                        expected.action,
                        expected.token,
                        expected.owner,
                        expected.site,
                        expected.layout,
                    )
                {
                    return Err(Error::InvalidMarker(marker));
                }
                let parent = frames.iter().rev().nth(1).map(|frame| frame.id);
                if calls.is_none() && (token >> 32 > 1 || recipient >> 32 > 1) {
                    return Err(Error::InvalidLocation(token.max(recipient)));
                }
                let from = owner(frame, parent, current.environment.as_ref(), token)?;
                let to = owner(frame, parent, current.environment.as_ref(), recipient)?;
                let context = Context {
                    task: TaskPath::default(),
                    operation: 0,
                    site: expected.source.clone(),
                };
                match action {
                    0 if from == to => {
                        tracker
                            .create(&context, to, expected.value_type.clone())
                            .map_err(Error::Consistency)?;
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
    if !frames.is_empty() || entered.is_empty() {
        return Err(Error::Incomplete);
    }
    tracker.finish(program, true).map_err(Error::Consistency)
}
