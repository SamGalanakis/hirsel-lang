//! Bounded native hook rows, decoded after issuers are quiescent. Source
//! metadata comes from the compiler's checked plan, independently of the log.
//! This adapter currently refuses nonroot tasks and nonlocal storage.
use super::{Context, Diagnostic, Owner, Storage, TaskPath, Trace, Tracker};
use crate::Site;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: u32 = 1;

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
    pub token: u64,
    pub owner: u64,
    pub site: u64,
    pub layout: u32,
    pub source: Site,
    pub value_type: String,
}
pub type Units = BTreeMap<[u8; 32], BTreeMap<u32, Metadata>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Binding,
    Incomplete,
    UnsupportedTask(u32),
    InvalidFrame(u64),
    UnknownUnit([u8; 32]),
    InvalidMarker(u32),
    InvalidLocation(u64),
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

fn owner(frame: u64, location: u64) -> Result<Owner, Error> {
    let index = location as u32;
    let storage = match location >> 32 {
        0 => Storage::Value {
            frame,
            value: index,
        },
        1 => Storage::Slot { frame, slot: index },
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
    if log.version != VERSION || log.program != program {
        return Err(Error::Binding);
    }
    if !log.complete || log.row_count != log.rows.len() as u64 || log.rows.len() > limit {
        return Err(Error::Incomplete);
    }
    let mut frames = Vec::new();
    let mut entered = BTreeSet::new();
    let mut tracker = Tracker::new(limit);
    for &row in &log.rows {
        let task = match row {
            Row::Enter { task, .. } | Row::Event { task, .. } | Row::Leave { task, .. } => task,
        };
        if task != 0 {
            return Err(Error::UnsupportedTask(task));
        }
        match row {
            Row::Enter { frame, unit, .. } => {
                if frame == 0 || !frames.is_empty() || !entered.insert(frame) {
                    return Err(Error::InvalidFrame(frame));
                }
                if !units.contains_key(&unit) {
                    return Err(Error::UnknownUnit(unit));
                }
                frames.push((frame, unit));
            }
            Row::Leave { frame, .. } => {
                if frames.pop().is_none_or(|(current, _)| current != frame) {
                    return Err(Error::InvalidFrame(frame));
                }
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
                let Some(&(current, unit)) = frames.last() else {
                    return Err(Error::InvalidFrame(frame));
                };
                if current != frame {
                    return Err(Error::InvalidFrame(frame));
                }
                let expected = units[&unit]
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
                let from = owner(frame, token)?;
                let to = owner(frame, recipient)?;
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
