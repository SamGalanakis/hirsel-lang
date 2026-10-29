//! Request-wide backend facts extracted on the actual Value miss and carried
//! in the C-bound discovery descriptor. Selection never opens a Value body.
use super::*;
use hirsel_lir::{
    ir::{LirUnit, Terminator},
    validate::NativeEnvironment,
};
use hirsel_syntax::DefPath;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
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
/// This mirrors native/stages.rs build_units' Value-based callback/recovery
/// selection and the actual specialization inline set. Ordinary stored rows
/// cannot authorize source/native acceptance; each input is re-admitted by C.
pub(super) fn select(
    instances: &super::demand::DemandInstances,
    environment: &NativeEnvironment<'_>,
) -> Result<EmissionFacts, String> {
    let mut callbacks = BTreeSet::new();
    let mut inline = BTreeSet::new();
    let mut owners = BTreeSet::new();
    let mut tracking = false;
    for (owner, _) in instances.completed_inputs()? {
        let (current, _) = instances.admitted(&owner, environment)?;
        if !current.facts.emission.canonical() {
            return Err("noncanonical emission facts".into());
        }
        owners.insert(owner.instance.digest());
        callbacks.extend(current.facts.emission.callbacks.iter().cloned());
        tracking |= current.facts.emission.tracking;
        inline.extend(&current.facts.emission.inline);
        if current.facts.source.ready_source.origin() != &owner.instance {
            if !owner
                .instance
                .is_same_or_derived_from(current.facts.source.ready_source.origin())
            {
                return Err("derived emission owner has another source origin".into());
            }
            inline.insert(owner.instance.digest());
        }
    }
    if !inline.is_subset(&owners) {
        return Err("emission inline instance has no admitted output".into());
    }
    Ok(EmissionFacts {
        callbacks: callbacks.into_iter().collect(),
        tracking,
        inline: inline.into_iter().collect(),
    })
}
