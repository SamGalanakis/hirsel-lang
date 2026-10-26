//! Body-derived evidence at same-module direct calls (MODE-2). These facts
//! never participate in a callable type, signature fingerprint or variance.
use crate::ParamId;

/// A result header row, without traversing arbitrary nominal representations.
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash,
)]
pub enum UniqueResultRow {
    Root,
    Tuple(u32),
    Payload { variant: u32, field: u32 },
}

/// Inferred, independently re-proved evidence. Absence grants no guarantee.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct UniqueSummary {
    pub preserves_unique: Vec<ParamId>,
    pub results: Vec<UniqueResultRow>,
}
impl UniqueSummary {
    pub fn is_empty(&self) -> bool {
        self.preserves_unique.is_empty() && self.results.is_empty()
    }
}
