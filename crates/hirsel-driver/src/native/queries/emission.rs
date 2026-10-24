//! Request-wide backend facts extracted on the actual Value miss and carried
//! in the C-bound discovery descriptor. Selection never opens a Value body.
use super::*;
use hirsel_lir::ir::{LirUnit, Terminator};
use hirsel_syntax::DefPath;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EmissionFacts {
    pub callbacks: Vec<DefPath>,
    pub tracking: bool,
    pub inline: Vec<ContentHash>,
}
impl EmissionFacts {
    pub(super) fn extract<S>(
        unit: &LirUnit<S>,
        inline: impl IntoIterator<Item = ContentHash>,
    ) -> Self {
        Self {
            callbacks: crate::native::stages::callbacks(unit)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            tracking: unit
                .terminators
                .iter()
                .any(|term| matches!(term, Terminator::Recover { .. })),
            inline: inline
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    }
    pub(super) fn canonical(&self) -> bool {
        !self.callbacks.windows(2).any(|pair| pair[0] >= pair[1])
            && !self.inline.windows(2).any(|pair| pair[0] >= pair[1])
    }
}
