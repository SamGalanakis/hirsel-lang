//! Ownership trace data and lineage analysis shared by the engines.
//!
//! Tokens are fresh logical identities, never addresses or payload hashes.
//! This module neither executes effects nor changes a program's ownership. An
//! engine must supply events from its actual committed transitions; consistency
//! of submitted events does not establish emitter coverage or RPC semantics.

use crate::Site;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

mod tracker;
pub use tracker::{Context, Trace, Tracker};
mod buffer;
pub use buffer::{Buffer, BufferFull};
pub mod native;

pub const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskPath(pub Vec<u32>);
impl fmt::Display for TaskPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "root")?;
        for part in &self.0 {
            write!(f, ".{part}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Token {
    pub task: TaskPath,
    pub sequence: u64,
}
impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.task, self.sequence)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Storage {
    Value {
        frame: u64,
        value: u32,
    },
    Slot {
        frame: u64,
        slot: u32,
    },
    Capture {
        environment: u64,
        capture: u32,
    },
    Argument {
        call: u64,
        parameter: u32,
    },
    Return {
        call: u64,
    },
    /// Logical object identity and queue/item ordinal, never a native pointer.
    Object {
        name: String,
        id: u64,
        item: u64,
    },
    /// Explicit forget retains a live token for diagnostics; it is not Drop.
    Forgotten {
        frame: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Projection {
    Field { field: u32 },
    VariantField { variant: u32, field: u32 },
    Index { index: u64 },
    Payload,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    /// None for object custody between tasks, not an unknown task identity.
    pub task: Option<TaskPath>,
    pub storage: Storage,
    pub path: Vec<Projection>,
}
impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.task {
            Some(task) => write!(f, "{task}/")?,
            None => write!(f, "runtime/")?,
        }
        write!(f, "{:?}", self.storage)?;
        for projection in &self.path {
            write!(f, "/{projection:?}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Create {
        token: Token,
        owner: Owner,
        value_type: String,
    },
    Move {
        token: Token,
        from: Owner,
        owner: Owner,
    },
    Drop {
        token: Token,
        owner: Owner,
    },
}
impl Action {
    pub fn token(&self) -> &Token {
        match self {
            Self::Create { token, .. } | Self::Move { token, .. } | Self::Drop { token, .. } => {
                token
            }
        }
    }
    pub fn owner(&self) -> &Owner {
        match self {
            Self::Create { owner, .. } | Self::Move { owner, .. } | Self::Drop { owner, .. } => {
                owner
            }
        }
    }
}

/// Pure observations between runtime operations use ordered subordinals. Local
/// operation numbers are scoped to the full task path, never globally sorted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub task: TaskPath,
    pub operation: u64,
    pub subordinal: u32,
    pub site: Site,
    #[serde(flatten)]
    pub action: Action,
}
impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}.{} {}:{}:{} ",
            self.task,
            self.operation,
            self.subordinal,
            self.site.definition,
            self.site.line,
            self.site.column
        )?;
        match &self.action {
            Action::Create {
                token,
                owner,
                value_type,
            } => write!(f, "create {token} ({value_type}) -> {owner}"),
            Action::Move { token, from, owner } => write!(f, "move {token} {from} -> {owner}"),
            Action::Drop { token, owner } => write!(f, "drop {token} from {owner}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Lineage {
    pub token: Token,
    pub value_type: String,
    pub owner: Option<Owner>,
    /// Includes creation, every committed move and the drop, in actual order.
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Problem {
    Version,
    Program,
    MissingOwner(Owner),
    EventBudget,
    Order,
    IdentityReuse,
    UnknownToken,
    WrongOwner,
    WrongTask,
    OccupiedOwner,
    DoubleDrop,
    UseAfterDrop,
    Incomplete,
    EventCount,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub problem: Problem,
    pub event: Option<Box<Event>>,
    /// Exact prior move chain, including a previous drop when applicable.
    pub history: Vec<Event>,
}
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "ownership trace: {:?}", self.problem)?;
        for event in &self.history {
            writeln!(f, "  {event}")?;
        }
        if let Some(event) = &self.event {
            write!(f, "  rejected: {event}")?;
        }
        Ok(())
    }
}
impl std::error::Error for Diagnostic {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Analysis {
    pub events: u64,
    pub created: u64,
    pub dropped: u64,
    pub live: Vec<Lineage>,
}

/// Bounded diagnostic analysis. A rejected event poisons this analysis: ignoring
/// an error and continuing cannot turn a corrupt stream into a qualified trace.
/// Engines need not instantiate this while tracing is disabled.
pub struct Inventory {
    limit: usize,
    events: u64,
    last: BTreeMap<TaskPath, (u64, u32)>,
    lineages: BTreeMap<Token, Lineage>,
    owners: BTreeMap<Owner, Token>,
    failure: Option<Diagnostic>,
}
impl Inventory {
    pub fn new(event_limit: usize) -> Self {
        Self {
            limit: event_limit,
            events: 0,
            last: BTreeMap::new(),
            lineages: BTreeMap::new(),
            owners: BTreeMap::new(),
            failure: None,
        }
    }

    /// Whether this exact logical location currently owns a tracked token.
    pub fn contains_owner(&self, owner: &Owner) -> bool {
        self.owners.contains_key(owner)
    }

    pub fn lineage(&self, token: &Token) -> Option<&Lineage> {
        self.lineages.get(token)
    }

    pub fn apply(&mut self, event: &Event) -> Result<(), Diagnostic> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if let Err(problem) = self.validate(event) {
            let error = Diagnostic {
                problem,
                event: Some(Box::new(event.clone())),
                history: self
                    .lineages
                    .get(event.action.token())
                    .map_or_else(Vec::new, |l| l.events.clone()),
            };
            self.failure = Some(error.clone());
            return Err(error);
        }
        let token = event.action.token();
        match &event.action {
            Action::Create {
                owner, value_type, ..
            } => {
                self.lineages.insert(
                    token.clone(),
                    Lineage {
                        token: token.clone(),
                        value_type: value_type.clone(),
                        owner: Some(owner.clone()),
                        events: vec![event.clone()],
                    },
                );
                self.owners.insert(owner.clone(), token.clone());
            }
            Action::Move { from, owner, .. } => {
                self.owners.remove(from);
                self.owners.insert(owner.clone(), token.clone());
                let lineage = self.lineages.get_mut(token).expect("validated token");
                lineage.owner = Some(owner.clone());
                lineage.events.push(event.clone());
            }
            Action::Drop { owner, .. } => {
                self.owners.remove(owner);
                let lineage = self.lineages.get_mut(token).expect("validated token");
                lineage.owner = None;
                lineage.events.push(event.clone());
            }
        }
        self.last
            .insert(event.task.clone(), (event.operation, event.subordinal));
        self.events += 1;
        Ok(())
    }

    fn validate(&self, event: &Event) -> Result<(), Problem> {
        if self.events >= self.limit as u64 {
            return Err(Problem::EventBudget);
        }
        if self
            .last
            .get(&event.task)
            .is_some_and(|&last| (event.operation, event.subordinal) <= last)
        {
            return Err(Problem::Order);
        }
        let token = event.action.token();
        let owner = event.action.owner();
        let valid_task = |owner: &Owner| {
            owner.task.is_some()
                || matches!(
                    owner.storage,
                    Storage::Capture { .. } | Storage::Object { .. }
                )
        };
        if !valid_task(owner)
            || matches!(&event.action, Action::Move { from, .. } if !valid_task(from))
        {
            return Err(Problem::WrongTask);
        }
        if self.owners.get(owner).is_some_and(|held| held != token) {
            return Err(Problem::OccupiedOwner);
        }
        match &event.action {
            Action::Create { .. } => {
                if self.lineages.contains_key(token) {
                    return Err(Problem::IdentityReuse);
                }
                if token.task != event.task
                    || owner.task.as_ref().is_some_and(|task| task != &event.task)
                {
                    return Err(Problem::WrongTask);
                }
            }
            Action::Move { from, .. } => {
                let lineage = self.lineages.get(token).ok_or(Problem::UnknownToken)?;
                if lineage.owner.is_none() {
                    return Err(Problem::UseAfterDrop);
                }
                if lineage.owner.as_ref() != Some(from) {
                    return Err(Problem::WrongOwner);
                }
                if (from.task.is_some() || owner.task.is_some())
                    && from.task.as_ref() != Some(&event.task)
                    && owner.task.as_ref() != Some(&event.task)
                {
                    return Err(Problem::WrongTask);
                }
            }
            Action::Drop { .. } => {
                let lineage = self.lineages.get(token).ok_or(Problem::UnknownToken)?;
                if lineage.owner.is_none() {
                    return Err(Problem::DoubleDrop);
                }
                if lineage.owner.as_ref() != Some(owner) {
                    return Err(Problem::WrongOwner);
                }
                if owner.task.as_ref().is_some_and(|task| task != &event.task) {
                    return Err(Problem::WrongTask);
                }
            }
        }
        Ok(())
    }

    /// Live tokens at the actual retirement of one frame. Values transferred to
    /// environments, objects, another frame or another task are not frame leaks.
    pub fn held_by_frame(&self, task: &TaskPath, frame: u64) -> Vec<&Lineage> {
        self.lineages
            .values()
            .filter(|lineage| {
                lineage.owner.as_ref().is_some_and(|owner| {
                    owner.task.as_ref() == Some(task)
                        && matches!(owner.storage,
                Storage::Value { frame: f, .. } | Storage::Slot { frame: f, .. } |
                Storage::Forgotten { frame: f } if f == frame)
                })
            })
            .collect()
    }

    /// A complete stream may diagnose leaked owners. Completeness, event-count
    /// matching and absence of corrupt transitions are separate from leak status.
    pub fn finish(self, complete: bool, event_count: u64) -> Result<Analysis, Diagnostic> {
        let problem = if let Some(error) = self.failure {
            return Err(error);
        } else if !complete {
            Some(Problem::Incomplete)
        } else if event_count != self.events {
            Some(Problem::EventCount)
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(Diagnostic {
                problem,
                event: None,
                history: vec![],
            });
        }
        let created = self.lineages.len() as u64;
        let live: Vec<_> = self
            .lineages
            .into_values()
            .filter(|lineage| lineage.owner.is_some())
            .collect();
        Ok(Analysis {
            events: self.events,
            created,
            dropped: created - live.len() as u64,
            live,
        })
    }
}
