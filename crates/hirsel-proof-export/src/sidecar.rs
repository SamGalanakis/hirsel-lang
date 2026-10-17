//! PS-12 sidecar evidence, assessed against one export (PS-7, PS-13, PS-15).
//!
//! A record is keyed by (program revision, obligation id, checksum) and names
//! the checker that produced its verdict with the checker's version and its
//! deterministic budget. The trust hook is the simplest one PS-15 allows: an
//! explicitly trusted external verdict. The build names the checkers it
//! trusts; a `proved` verdict of one of them that is current for this export
//! is accepted. Nothing is replayed, so trust is exactly that list, and every
//! other record, and every obligation without one, keeps its check.
use crate::{DefinitionSection, Export, Obligation};
use hirsel_syntax::ContentHash;
use std::collections::BTreeMap;
use std::fmt;

/// The sidecar format this consumer reads.
pub const SIDECAR_FORMAT: &str = "hirsel.proof.sidecar.v1";

/// Deterministic resource counts (PS-15); wall time is never a budget.
pub const BUDGET_UNITS: [&str; 5] = ["steps", "heartbeats", "conflicts", "rlimit", "fuel"];

/// A checker identity, written `tool@version`.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Checker {
    pub tool: String,
    pub version: String,
}
impl Checker {
    /// `tool@version`, both nonempty.
    pub fn parse(text: &str) -> Option<Self> {
        let (tool, version) = text.rsplit_once('@')?;
        (!tool.is_empty() && !version.is_empty()).then(|| Self {
            tool: tool.into(),
            version: version.into(),
        })
    }
}
impl fmt::Display for Checker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.tool, self.version)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Budget {
    pub unit: String,
    pub count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    Proved,
    Refuted,
    Unknown,
    Timeout,
}
impl Verdict {
    pub const ALL: [Self; 4] = [Self::Proved, Self::Refuted, Self::Unknown, Self::Timeout];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proved => "proved",
            Self::Refuted => "refuted",
            Self::Unknown => "unknown",
            Self::Timeout => "timeout",
        }
    }
}

/// One sidecar record. `obligation` is the written PS-1 id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    pub revision: ContentHash,
    pub obligation: String,
    pub checksum: ContentHash,
    pub checker: Checker,
    pub budget: Budget,
    pub verdict: Verdict,
}

/// Why an obligation keeps its check (PS-13).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Keep {
    /// No record names it.
    Missing,
    /// Records name it, none for this revision and checksum.
    Stale(String),
    /// The current record's verdict is not `proved`.
    Rejected(String),
    /// The current record cannot be replayed by a checker this build trusts.
    Unreplayable(String),
}
impl fmt::Display for Keep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "missing"),
            Self::Stale(why) => write!(f, "stale: {why}"),
            Self::Rejected(why) => write!(f, "rejected: {why}"),
            Self::Unreplayable(why) => write!(f, "unreplayable: {why}"),
        }
    }
}

/// One obligation of the export and what its evidence licenses.
pub struct Assessed<'e> {
    pub section: &'e DefinitionSection,
    pub obligation: &'e Obligation,
    /// Accepted, with the checker whose verdict it trusts, or kept.
    pub status: Result<&'e Checker, Keep>,
    /// Whether any record names this obligation.
    pub recorded: bool,
}

pub struct Assessment<'e> {
    /// Every obligation of the export, in export order.
    pub obligations: Vec<Assessed<'e>>,
    /// Records naming no obligation of this export, with the written id.
    pub orphans: Vec<&'e Record>,
}

/// Two records with one key are a malformed sidecar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DuplicateKey(pub String);

/// PS-7/PS-12/PS-15: a record is current when its revision is the export's
/// program revision and its checksum is the obligation's; a current record
/// is accepted when a trusted checker proved it within a deterministic
/// budget.
pub fn assess<'e>(
    export: &'e Export,
    records: &'e [Record],
    trusted: &[Checker],
) -> Result<Assessment<'e>, DuplicateKey> {
    let mut by_id: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
    for record in records {
        let named = by_id.entry(&record.obligation).or_default();
        if named
            .iter()
            .any(|r| r.revision == record.revision && r.checksum == record.checksum)
        {
            return Err(DuplicateKey(record.obligation.clone()));
        }
        named.push(record);
    }
    let revision = export.header.program_revision;
    let mut obligations = vec![];
    for section in &export.definitions {
        for obligation in &section.obligations {
            let named = by_id
                .remove(obligation.id.to_string().as_str())
                .unwrap_or_default();
            let current = named
                .iter()
                .find(|r| Some(r.revision) == revision && r.checksum == obligation.checksum);
            let status = match (current, named.first()) {
                (Some(record), _) => accept(record, trusted),
                (None, None) => Err(Keep::Missing),
                (None, Some(_)) if revision.is_none() => Err(Keep::Stale(
                    "this program declares no launch entry, so no revision".into(),
                )),
                (None, Some(_)) => Err(Keep::Stale(
                    if named.iter().any(|r| Some(r.revision) == revision) {
                        "the checksum differs from this export's"
                    } else {
                        "recorded for another program revision"
                    }
                    .into(),
                )),
            };
            obligations.push(Assessed {
                section,
                obligation,
                status,
                recorded: !named.is_empty(),
            });
        }
    }
    Ok(Assessment {
        obligations,
        orphans: by_id.into_values().flatten().collect(),
    })
}

fn accept<'r>(record: &'r Record, trusted: &[Checker]) -> Result<&'r Checker, Keep> {
    if !trusted.contains(&record.checker) {
        return Err(Keep::Unreplayable(format!(
            "this build trusts no checker {}",
            record.checker
        )));
    }
    if !BUDGET_UNITS.contains(&record.budget.unit.as_str()) || record.budget.count == 0 {
        return Err(Keep::Unreplayable(format!(
            "budget {} {} is not a positive deterministic count",
            record.budget.count, record.budget.unit
        )));
    }
    match record.verdict {
        Verdict::Proved => Ok(&record.checker),
        other => Err(Keep::Rejected(format!("the verdict is {}", other.as_str()))),
    }
}
