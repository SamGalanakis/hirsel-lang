//! Pending changes for program-level aggregation. A query may settle a dirty
//! memo before the driver visits it, so graph dirtiness alone is not a durable
//! notification. Acknowledgments name observed generations, like Salsa's
//! revision stamps, and cannot consume a newer S1 publication.
use super::*;

/// Definitions whose retained program products need to be refreshed or retired.
#[derive(Clone, Debug, Default)]
pub struct FrontChanges {
    pub changed: BTreeSet<DefId>,
    pub removed: BTreeSet<DefId>,
    pub affected: BTreeSet<DefId>,
    observations: BTreeMap<DefId, u64>,
}
impl FrontChanges {
    /// Generation identity for each pending notification. An aggregate may
    /// retire stale products once without acknowledging work still pending.
    pub(crate) fn generations(&self) -> impl Iterator<Item = (DefId, u64)> + '_ {
        self.observations
            .iter()
            .map(|(&definition, &generation)| (definition, generation))
    }
}
#[derive(Default)]
pub(super) struct FrontTracker {
    active: bool,
    generation: u64,
    pending: BTreeMap<DefId, Pending>,
}
#[derive(Default)]
struct Pending {
    generation: u64,
    present: Option<bool>,
    affected: bool,
}
impl FrontTracker {
    pub(super) fn is_active(&self) -> bool {
        self.active
    }
    fn activate(&mut self) -> bool {
        let was_active = self.active;
        self.active = true;
        !was_active
    }
    pub(super) fn changed(&mut self, definition: DefId, present: bool) {
        if !self.active {
            return;
        }
        self.generation += 1;
        let pending = self.pending.entry(definition).or_default();
        pending.generation = self.generation;
        pending.present = Some(present);
    }
    fn affected(&mut self, definition: DefId) {
        self.generation += 1;
        let pending = self.pending.entry(definition).or_default();
        pending.generation = self.generation;
        pending.affected = true;
    }
    fn snapshot(&self) -> FrontChanges {
        let mut changes = FrontChanges::default();
        for (&definition, pending) in &self.pending {
            changes.observations.insert(definition, pending.generation);
            match pending.present {
                Some(true) => {
                    changes.changed.insert(definition);
                }
                Some(false) => {
                    changes.removed.insert(definition);
                }
                None => (),
            }
            if pending.affected {
                changes.affected.insert(definition);
            }
        }
        changes
    }
    fn acknowledge(&mut self, observed: &FrontChanges, definitions: &[DefId]) {
        for definition in definitions {
            if let Some(pending) = self.pending.get(definition)
                && observed.observations.get(definition) == Some(&pending.generation)
            {
                self.pending.remove(definition);
            }
        }
    }
}
impl QueryEngine {
    /// Changes not yet incorporated into the caller's compact program products.
    /// The first call starts tracking and returns all indexed definitions as
    /// changed. Signature settling may add affected subscribers; no body memo is read.
    pub fn frontier_changes(&mut self) -> FrontChanges {
        self.note_environment();
        self.settle_signatures();
        if self.front_changes.activate() {
            // A new aggregate has no prior products. Tracking starts on demand;
            // scalar-only sessions retain no unused notification history.
            for path in self
                .item_paths()
                .into_iter()
                .collect::<Vec<_>>()
            {
                self.front_changes.changed(DefId::intern(&path), true);
            }
        }
        let changes = self.front_changes.snapshot();
        self.work.frontier_observations += changes.observations.len() as u64;
        changes
    }
    /// One publication's notification. Compact completion consumes only its
    /// owner's generation, leaving every unrelated pending owner untouched.
    pub(crate) fn frontier_change(&mut self, definition: DefId) -> FrontChanges {
        self.note_environment();
        self.settle_signatures();
        let mut changes = FrontChanges::default();
        if let Some(pending) = self.front_changes.pending.get(&definition) {
            self.work.frontier_observations += 1;
            changes.observations.insert(definition, pending.generation);
            match pending.present {
                Some(true) => {
                    changes.changed.insert(definition);
                }
                Some(false) => {
                    changes.removed.insert(definition);
                }
                None => (),
            }
            if pending.affected {
                changes.affected.insert(definition);
            }
        }
        changes
    }
    /// Acknowledge only products actually refreshed from `observed`. Changes
    /// arriving during S1 remain pending for the following frontier.
    pub fn acknowledge_frontier(&mut self, observed: &FrontChanges, definitions: &[DefId]) {
        self.front_changes.acknowledge(observed, definitions);
    }
    pub(super) fn apply_changes(&mut self, changes: &crate::memo::ChangeSet) {
        if !changes.is_empty() {
            // STORE-6 can demand private schema signatures beyond the checked
            // call graph. Keep its own result conservative until those reads
            // are an independently recorded query product.
            self.source_growth.invalidate_table();
        }
        let signatures: BTreeSet<_> = changes
            .reads
            .iter()
            .filter_map(|(read, _)| match read {
                ReadKey::Signature(definition) | ReadKey::FailedSignature(definition) => {
                    Some(*definition)
                }
                _ => None,
            })
            .collect();
        let expanded;
        let changes = if signatures.is_empty() {
            changes
        } else {
            // Body-only changes deliberately do not mark broad V inputs.
            // Header/candidate/signature changes can affect every overlay's
            // verdict, even when the base universe remains error-free.
            let mut next = changes.clone();
            for definition in signatures {
                next.read(
                    ReadKey::ValidationView(definition),
                    self.durability_of(&ReadKey::Signature(definition)),
                );
            }
            for read in self.graph.validation_context_reads() {
                next.read(read, crate::memo::Durability::Low);
            }
            expanded = next;
            &expanded
        };
        let mut affected = BTreeSet::new();
        for (read, _) in &changes.reads {
            for key in self.graph.subscribers(read).filter(|key| {
                matches!(
                    key.kind,
                    QueryKind::Check | QueryKind::CheckedSummary | QueryKind::Summary
                )
            }) {
                self.source_growth.invalidate(key.def);
            }
            affected.extend(self.graph.subscribers(read).map(|key| key.def));
        }
        for (definition, _) in &changes.texts {
            self.source_growth.invalidate(*definition);
            affected.extend(self.graph.memos_of(*definition).map(|key| key.def));
        }
        self.graph.apply(changes);
        if self.front_changes.active {
            for definition in affected {
                self.front_changes.affected(definition);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner(name: &str) -> DefId {
        DefId::intern(&DefPath(hirsel_syntax::ItemPath {
            package: hirsel_syntax::PackageId {
                name: "test".into(),
                version: "0".into(),
            },
            modules: vec![],
            name: name.into(),
        }))
    }
    #[test]
    fn validation_context_subscribers_are_marked_by_signatures_but_not_body_edits() {
        use crate::memo::{ChangeSet, Durability, PackageKey, ReadKind};
        let mut engine = QueryEngine::new(default_environment(), PhaseAdapters::default());
        let (source, dependent, ordinary) =
            (owner("source"), owner("dependent"), owner("ordinary"));
        engine.front_changes.activate();
        let context = ReadKey::ValidationUniverse(PackageKey::of(&dependent.path().0.package));
        let key = MemoKey::new(QueryKind::Lower, dependent);
        let other = MemoKey::new(QueryKind::Lower, ordinary);
        let digest = ContentHash::of_bytes(b"old universe");
        engine.graph.adopt(
            key,
            Arc::new(Reads::from_vec(vec![(context, digest)]).unwrap()),
            Durability::Low,
        );
        engine.graph.adopt(
            other,
            Arc::new(Reads::from_vec(vec![(ReadKey::Signature(ordinary), digest)]).unwrap()),
            Durability::Low,
        );
        let mut body = ChangeSet::default();
        body.read(ReadKey::ExecutedBody(source), Durability::Low);
        engine.apply_changes(&body);
        assert!(!engine.graph.is_dirty(&key));
        assert!(engine.front_changes.snapshot().affected.is_empty());
        let mut signature = ChangeSet::default();
        signature.read(ReadKey::Signature(source), Durability::Low);
        engine.apply_changes(&signature);
        assert!(engine.graph.is_dirty(&key));
        assert!(!engine.graph.is_dirty(&other));
        assert_eq!(
            engine.front_changes.snapshot().affected,
            BTreeSet::from([dependent])
        );
        assert_eq!(
            engine.graph.stats().invalidated[ReadKind::ValidationUniverse as usize],
            1
        );
        engine.graph.remove(&key);
        assert!(engine.graph.validation_context_reads().next().is_none());
        engine.graph.adopt(
            key,
            Arc::new(Reads::from_vec(vec![(context, digest)]).unwrap()),
            Durability::Low,
        );
        engine
            .graph
            .adopt(key, Arc::new(Reads::empty()), Durability::Low);
        assert!(
            engine.graph.validation_context_reads().next().is_none(),
            "replacing a memo retires its contextual key"
        );
    }

    #[test]
    fn owner_snapshot_preserves_other_owners_and_newer_generations() {
        let (a, b) = (owner("a"), owner("b"));
        let mut engine = QueryEngine::new(default_environment(), PhaseAdapters::default());
        engine.front_changes.activate();
        engine.front_changes.changed(a, true);
        engine.front_changes.changed(b, true);
        let first = engine.frontier_change(a);
        assert_eq!(first.changed, BTreeSet::from([a]));
        assert_eq!(first.observations.len(), 1);
        engine.front_changes.changed(a, false);
        engine.acknowledge_frontier(&first, &[a, b]);
        let pending = engine.front_changes.snapshot();
        assert_eq!(pending.changed, BTreeSet::from([b]));
        assert_eq!(pending.removed, BTreeSet::from([a]));
        let second = engine.frontier_change(a);
        engine.acknowledge_frontier(&second, &[a]);
        assert_eq!(engine.front_changes.snapshot().changed, BTreeSet::from([b]));
        assert_eq!(engine.work.frontier_observations, 2);
    }

    #[test]
    fn acknowledgment_cannot_consume_a_newer_publication_or_other_file() {
        let (a, b) = (owner("a"), owner("b"));
        let mut tracker = FrontTracker::default();
        tracker.activate();
        tracker.changed(a, true);
        tracker.changed(b, true);
        let first = tracker.snapshot();
        tracker.changed(a, false);
        tracker.acknowledge(&first, &[a, b]);
        let second = tracker.snapshot();
        assert!(second.changed.is_empty());
        assert_eq!(second.removed, BTreeSet::from([a]));
        tracker.changed(a, true);
        tracker.acknowledge(&second, &[a]);
        let third = tracker.snapshot();
        assert_eq!(third.changed, BTreeSet::from([a]));
        assert!(third.removed.is_empty());
        tracker.acknowledge(&third, &[]);
        assert_eq!(tracker.snapshot().changed, third.changed);
        tracker.acknowledge(&third, &[a]);
        assert!(tracker.snapshot().observations.is_empty());
    }
}
