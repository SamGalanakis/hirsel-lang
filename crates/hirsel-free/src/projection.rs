//! The normalised projection a free-mode tape is checked against (design §4.2).
//!
//! The live run and a replay of its tape each emit [`Event`]s, stamped with the phase whose
//! section produced them. A projection keeps the events a cut admits and normalises away what
//! the tape deliberately leaves free. It compares the run outcome, the cut, external outputs
//! per logical sink (in the sink key's epoch order), per-object sequences of (path, ordinal,
//! sub-event, outcome) in version order for every object except atomics, a channel's receives
//! in its receiver's op order, per-key host epochs (each epoch's members as a set), and each
//! task's observed cancellation, deadline, `await_any`, thaw and atomic-load outcomes. Atomic
//! store order, atomic versions, fuel and the meter are not compared.

use crate::phase::Progress;
use crate::record::ObjectId;
use std::collections::BTreeMap;

pub(crate) fn cpu_request(width: u32, action: u32, count: u64) -> [u8; 16] {
    let mut request = [0u8; 16];
    request[..4].copy_from_slice(&width.to_le_bytes());
    request[4..8].copy_from_slice(&action.to_le_bytes());
    request[8..].copy_from_slice(&count.to_le_bytes());
    request
}

/// An observed outcome a tape substitutes.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Observed {
    /// A CPU receipt binds the request as well as its substituted outcome.
    CpuControl {
        width: u32,
        action: u32,
        count: u64,
        outcome: u32,
    },
    /// The cancellation a check observed.
    Cancel(Option<u32>),
    /// Whether a `scope_until` deadline expired.
    Expired(bool),
    AwaitAny(u64),
    Thaw(bool),
    Load(u128),
}

/// What a projection compares.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// A sub-event that versions its object (creation is version 1).
    Versioned {
        object: ObjectId,
        version: u64,
        sub: u32,
        outcome: String,
    },
    /// A receive: ordered by its receiver's ops, not by the channel's version.
    Received {
        object: ObjectId,
        outcome: String,
    },
    Observed(Observed),
    /// A host-key transition at `epoch`; `index` is the request's claim (0) or attempt.
    Key {
        key: String,
        epoch: u64,
        index: u32,
    },
    /// Bytes a host call wrote to a logical sink, in the sink key's exclusive epoch `epoch`.
    Output {
        sink: String,
        epoch: u64,
        bytes: Vec<u8>,
    },
}

/// An event with the task and phase that produced it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stamped {
    pub task: Vec<u32>,
    pub progress: Progress,
    pub event: Event,
}

/// One touch of an object.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Touch {
    pub task: Vec<u32>,
    pub op: u64,
    pub sub: u32,
    pub outcome: String,
}

/// An object's history.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct History {
    pub versions: BTreeMap<u64, Touch>,
    pub receives: Vec<Touch>,
}

/// A key epoch's members, each as (task, op, claim or attempt index).
pub type Members = Vec<(Vec<u32>, u64, u32)>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Projection {
    pub outcome: String,
    pub cut: BTreeMap<Vec<u32>, Progress>,
    pub objects: BTreeMap<ObjectId, History>,
    /// Per key, per epoch, its members.
    pub keys: BTreeMap<String, BTreeMap<u64, Members>>,
    pub outputs: BTreeMap<String, Vec<u8>>,
    pub observed: BTreeMap<(Vec<u32>, u64), Vec<Observed>>,
}

impl Projection {
    /// The projection of the `events` that `cut` admits.
    pub fn new<'e>(
        outcome: &str,
        cut: &BTreeMap<Vec<u32>, Progress>,
        events: impl IntoIterator<Item = &'e Stamped>,
    ) -> Projection {
        let mut projection = Projection {
            outcome: outcome.into(),
            cut: cut.clone(),
            ..Projection::default()
        };
        let mut outputs: BTreeMap<String, BTreeMap<u64, Vec<u8>>> = BTreeMap::new();
        for stamped in events {
            if !cut
                .get(&stamped.task)
                .is_some_and(|at| at.admits(stamped.progress))
            {
                continue;
            }
            let touch = |sub, outcome: &String| Touch {
                task: stamped.task.clone(),
                op: stamped.progress.op,
                sub,
                outcome: outcome.clone(),
            };
            match &stamped.event {
                Event::Versioned {
                    object,
                    version,
                    sub,
                    outcome,
                } => {
                    let history = projection.objects.entry(object.clone()).or_default();
                    let previous = history.versions.insert(*version, touch(*sub, outcome));
                    assert!(
                        previous.is_none(),
                        "two sub-events made version {version} of {object}"
                    );
                }
                Event::Received { object, outcome } => projection
                    .objects
                    .entry(object.clone())
                    .or_default()
                    .receives
                    .push(touch(0, outcome)),
                Event::Observed(observed) => projection
                    .observed
                    .entry((stamped.task.clone(), stamped.progress.op))
                    .or_default()
                    .push(observed.clone()),
                Event::Key { key, epoch, index } => projection
                    .keys
                    .entry(key.clone())
                    .or_default()
                    .entry(*epoch)
                    .or_default()
                    .push((stamped.task.clone(), stamped.progress.op, *index)),
                Event::Output { sink, epoch, bytes } => {
                    let previous = outputs
                        .entry(sink.clone())
                        .or_default()
                        .insert(*epoch, bytes.clone());
                    assert!(
                        previous.is_none(),
                        "two outputs in exclusive epoch {epoch} of {sink}"
                    );
                }
            }
        }
        for history in projection.objects.values_mut() {
            history.receives.sort();
        }
        for epochs in projection.keys.values_mut() {
            for members in epochs.values_mut() {
                members.sort();
            }
        }
        projection.outputs = outputs
            .into_iter()
            .map(|(sink, writes)| (sink, writes.into_values().flatten().collect()))
            .collect();
        projection
    }
}
