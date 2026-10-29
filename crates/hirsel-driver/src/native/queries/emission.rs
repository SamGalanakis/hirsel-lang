//! Request-wide backend facts extracted on the actual Value miss and carried
//! in the C-bound discovery descriptor. Selection never opens a Value body.
use super::*;
use hirsel_lir::{
    ir::{LirUnit, Terminator},
    validate::NativeEnvironment,
};
use hirsel_syntax::DefPath;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EmissionFacts {
    pub callbacks: Vec<DefPath>,
    pub tracking: bool,
    pub inline: InlineInstances,
}
/// One complete inline selection shared by a callable scope. A small escaped
/// view keeps the original registry grant; portable bytes contain only hashes.
#[derive(Clone, Default)]
pub struct InlineInstances(Option<Arc<InlineOwner>>);
struct InlineOwner {
    values: Box<[ContentHash]>,
    _grant: Option<Arc<hirsel_engine::runtime::MemoryReservation>>,
}
impl InlineInstances {
    pub(super) fn new(
        values: impl IntoIterator<Item = ContentHash>,
        grant: Option<Arc<hirsel_engine::runtime::MemoryReservation>>,
    ) -> Self {
        let values = values.into_iter().collect::<Box<[_]>>();
        if values.is_empty() {
            Self::default()
        } else {
            Self(Some(Arc::new(InlineOwner {
                values,
                _grant: grant,
            })))
        }
    }
    pub(super) fn with_grant(
        self,
        grant: Option<Arc<hirsel_engine::runtime::MemoryReservation>>,
    ) -> Self {
        match &self.0 {
            Some(owner) if owner._grant.is_none() && grant.is_some() => {
                Self::new(owner.values.iter().copied(), grant)
            }
            _ => self,
        }
    }
    pub(super) fn identity(&self) -> Option<*const ()> {
        self.0.as_ref().map(|owner| Arc::as_ptr(owner).cast())
    }
    pub(super) fn retained_bytes(&self) -> u64 {
        self.0.as_ref().map_or(0, |owner| {
            (size_of::<InlineOwner>() + 2 * size_of::<usize>() + size_of_val(&*owner.values)) as u64
        })
    }
}
impl std::ops::Deref for InlineInstances {
    type Target = [ContentHash];
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().map_or(&[], |owner| &owner.values)
    }
}
impl AsRef<[ContentHash]> for InlineInstances {
    fn as_ref(&self) -> &[ContentHash] {
        self
    }
}
impl std::fmt::Debug for InlineInstances {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_ref().fmt(f)
    }
}
impl PartialEq for InlineInstances {
    fn eq(&self, other: &Self) -> bool {
        self.identity() == other.identity() || **self == **other
    }
}
impl Eq for InlineInstances {}
impl Serialize for InlineInstances {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_ref().serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for InlineInstances {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Box::<[ContentHash]>::deserialize(deserializer).map(|values| Self::new(values, None))
    }
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
            inline: InlineInstances::new(inline.into_iter().collect::<BTreeSet<_>>(), None),
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
        inline.extend(current.facts.emission.inline.iter().copied());
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
        inline: InlineInstances::new(inline, None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_selection_preserves_portable_membership_and_retains_its_complete_grant() {
        let budget = hirsel_engine::runtime::ProcessBudget::new(1 << 20);
        let grant = Arc::new(budget.reserve(1 << 20).unwrap());
        let values = [ContentHash::of_bytes(b"one"), ContentHash::of_bytes(b"two")];
        let selection = InlineInstances::new(values, Some(grant.clone()));
        let bytes = serde_json::to_vec(&selection).unwrap();
        assert_eq!(bytes, serde_json::to_vec(&values).unwrap());
        let restored: InlineInstances = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored, selection);
        assert_ne!(restored.identity(), selection.identity());
        let escaped = selection.clone();
        assert_eq!(escaped.identity(), selection.identity());
        assert!(selection.retained_bytes() >= size_of_val(&values) as u64);
        assert_ne!(selection, InlineInstances::new([values[0]], None));
        drop(grant);
        drop(selection);
        assert_eq!(budget.reserved(), 1 << 20);
        drop(escaped);
        assert_eq!(budget.reserved(), 0);
        assert_eq!(restored.as_ref(), values);
        assert_eq!(InlineInstances::default().retained_bytes(), 0);
    }
}
