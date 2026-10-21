//! Memo records, the reverse subscriber index and logical verification
//! (design §1b). Eager work per revision touches only the changed inputs and
//! their subscribers, which get a dirty mark; every other memo is valid at
//! the current revision and gets its `verified_at` stamp lazily, when read.
use super::{Durability, ReadKey, ReadKind, Reads, Revision};
use crate::QueryKind;
use hirsel_intern::IdHasher;
use hirsel_intern::timings::{self, Count};
use hirsel_syntax::{ContentHash, DefId};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// One memo's identity: its query kind, its definition and, for memos with
/// arguments (staged values, generated instances), a session ordinal of the
/// argument key (0 for none).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MemoKey {
    pub kind: QueryKind,
    pub def: DefId,
    pub instance: u32,
}
impl MemoKey {
    pub fn new(kind: QueryKind, def: DefId) -> Self {
        Self {
            kind,
            def,
            instance: 0,
        }
    }
}

/// A memo's value with its record (design §3). The value is shared, and the
/// reads are stored once.
pub struct Memo<T> {
    pub key: MemoKey,
    /// The definition's own content: the text hash the result was computed from.
    pub text: ContentHash,
    pub value: Result<Arc<T>, hirsel_types::QueryError>,
    pub reads: Arc<Reads>,
    pub durability: Durability,
    pub changed_at: Revision,
    verified_at: AtomicU64,
}
impl<T> Memo<T> {
    /// Project a retained value without changing its graph identity or logical
    /// verification stamps. Eviction keeps the original read subscriptions.
    pub(crate) fn map_value<U>(&self, map: impl FnOnce(&T) -> U) -> Memo<U> {
        Memo {
            key: self.key,
            text: self.text,
            value: self
                .value
                .as_ref()
                .map(|value| Arc::new(map(value)))
                .map_err(Clone::clone),
            reads: self.reads.clone(),
            durability: self.durability,
            changed_at: self.changed_at,
            verified_at: AtomicU64::new(self.verified_at()),
        }
    }
    pub fn verified_at(&self) -> u64 {
        self.verified_at.load(Relaxed)
    }
}
impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            text: self.text,
            value: self.value.clone(),
            reads: self.reads.clone(),
            durability: self.durability,
            changed_at: self.changed_at,
            verified_at: AtomicU64::new(self.verified_at()),
        }
    }
}

/// The inputs whose observed value changed in one revision, with each change's
/// durability, and the definitions whose own text changed.
#[derive(Clone, Debug, Default)]
pub struct ChangeSet {
    pub reads: Vec<(ReadKey, Durability)>,
    pub texts: Vec<(DefId, Durability)>,
}
impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.reads.is_empty() && self.texts.is_empty()
    }
    pub fn read(&mut self, key: ReadKey, durability: Durability) {
        self.reads.push((key, durability));
    }
    pub fn text(&mut self, def: DefId, durability: Durability) {
        self.texts.push((def, durability));
    }
}

/// Why a memo was invalidated, for the slow-path counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Invalidation {
    Read(ReadKind),
    Text,
}

/// Counters the engine keeps per session (`SessionStats`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoStats {
    /// Memos read or written by the engine (marked, verified, inserted, removed).
    pub touched: u64,
    /// Dirty marks by the kind of the read that changed, in `ReadKind::ALL` order.
    pub invalidated: [u64; ReadKind::ALL.len()],
    /// Dirty marks from a definition's own text.
    pub invalidated_text: u64,
    /// Memos recorded as recomputed, by query kind.
    pub recomputed: std::collections::BTreeMap<QueryKind, u64>,
}

struct Record {
    reads: Arc<Reads>,
    durability: Durability,
    changed_at: Revision,
}

/// The session's memo bookkeeping: per-memo records, the reverse index
/// `ReadKey → subscribers`, the per-definition index for text changes, and the
/// dirty set. Values live in the engine's typed tables.
pub struct MemoGraph {
    revision: Revision,
    /// The last revision an input of at least each durability changed, indexed
    /// by `Durability as usize`.
    last_changed: [Revision; 3],
    records: HashMap<MemoKey, Record>,
    subscribers: HashMap<ReadKey, HashSet<MemoKey>>,
    /// Broad V inputs, indexed separately so a changed signature does not
    /// scan every unrelated signature/body read merely to mark their users.
    validation_contexts: HashSet<ReadKey>,
    by_def: HashMap<DefId, HashSet<MemoKey>, IdHasher>,
    dirty: HashMap<MemoKey, Invalidation>,
    stats: MemoStats,
}

impl Default for MemoGraph {
    fn default() -> Self {
        Self {
            revision: Revision::FIRST,
            last_changed: [Revision::FIRST; 3],
            records: HashMap::new(),
            subscribers: HashMap::new(),
            validation_contexts: HashSet::new(),
            by_def: HashMap::default(),
            dirty: HashMap::new(),
            stats: MemoStats::default(),
        }
    }
}

/// What reading a memo found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    /// No dirty mark: valid at the current revision, now stamped.
    Valid,
    /// Marked by a changed input; deep-verify the reads, then `confirm` or recompute.
    Dirty(Invalidation),
    /// No record.
    Missing,
}

impl MemoGraph {
    pub fn revision(&self) -> Revision {
        self.revision
    }
    pub fn stats(&self) -> &MemoStats {
        &self.stats
    }
    /// Count completed worker work independently of memo publication. Failed
    /// computations can be counted without retaining a value or subscriptions.
    pub(crate) fn record_recompute(&mut self, kind: QueryKind) {
        *self.stats.recomputed.entry(kind).or_default() += 1;
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
    /// The last revision any input of durability `d` or lower changed.
    pub fn last_changed(&self, d: Durability) -> Revision {
        self.last_changed[d as usize]
    }
    /// The memos that read `key`, in unspecified order.
    pub fn subscribers(&self, key: &ReadKey) -> impl Iterator<Item = &MemoKey> {
        self.subscribers.get(key).into_iter().flatten()
    }
    /// Contextual V read keys that currently have subscribers.
    pub fn validation_context_reads(&self) -> impl Iterator<Item = ReadKey> + '_ {
        self.validation_contexts.iter().copied()
    }
    /// The memos keyed on `def`.
    pub fn memos_of(&self, def: DefId) -> impl Iterator<Item = &MemoKey> {
        self.by_def.get(&def).into_iter().flatten()
    }
    /// Every read some memo subscribes to, in unspecified order.
    pub fn read_keys(&self) -> Vec<ReadKey> {
        self.subscribers.keys().copied().collect()
    }
    /// The definitions with a dirty memo, sorted by DefPath.
    pub fn dirty_definitions(&self) -> Vec<DefId> {
        let mut defs: Vec<DefId> = self.dirty.keys().map(|k| k.def).collect();
        defs.sort();
        defs.dedup();
        defs
    }
    pub fn is_dirty(&self, key: &MemoKey) -> bool {
        self.dirty.contains_key(key)
    }

    /// Starts the next revision: marks exactly the subscribers of each changed
    /// read and the memos keyed on each changed text. Nothing else is touched.
    /// A change at durability D advances `last_changed` for D and every level
    /// below it.
    pub fn apply(&mut self, changes: &ChangeSet) -> Revision {
        if changes.is_empty() {
            return self.revision;
        }
        self.revision = self.revision.next();
        let highest = changes
            .reads
            .iter()
            .map(|(_, d)| *d)
            .chain(changes.texts.iter().map(|(_, d)| *d))
            .max()
            .unwrap_or(Durability::Low);
        for d in Durability::ALL {
            if d <= highest {
                self.last_changed[d as usize] = self.revision;
            }
        }
        // Several changed inputs often share subscribers (header, candidate,
        // representation and citation changes in particular). Union the mark
        // requests before touching records, preserving the first read's reason.
        let mut readers = BTreeMap::new();
        for (read, _) in &changes.reads {
            for key in self.subscribers(read) {
                readers.entry(*key).or_insert(read.kind());
            }
        }
        for (key, kind) in readers {
            self.mark(key, Invalidation::Read(kind));
        }
        let texts: BTreeSet<_> = changes
            .texts
            .iter()
            .flat_map(|(def, _)| self.memos_of(*def).copied())
            .collect();
        for key in texts {
            self.mark(key, Invalidation::Text);
        }
        self.revision
    }

    /// Marks the subscribers of one changed read. S0 calls this as it finds
    /// changes, so a recomputed signature whose fingerprint changed marks its
    /// readers in the same revision.
    pub fn mark_subscribers(&mut self, read: &ReadKey) {
        let keys: Vec<MemoKey> = self.subscribers(read).copied().collect();
        for key in keys {
            self.mark(key, Invalidation::Read(read.kind()));
        }
    }

    fn mark(&mut self, key: MemoKey, why: Invalidation) {
        self.stats.touched += 1;
        // A text mark outranks a read mark: a read-marked memo is deep-verified
        // and may be kept, but one whose own items changed is recomputed. A
        // change set marks its reads first, so the text mark must replace one.
        match (self.dirty.get(&key), why) {
            (Some(Invalidation::Text), _)
            | (Some(Invalidation::Read(_)), Invalidation::Read(_)) => {
                return;
            }
            _ => {}
        }
        self.dirty.insert(key, why);
        match why {
            Invalidation::Read(kind) => {
                self.stats.invalidated[kind as usize] += 1;
                timings::count(invalidation_count(kind));
            }
            Invalidation::Text => self.stats.invalidated_text += 1,
        }
    }

    /// Reads a memo's record: a memo with no dirty mark is valid now and its
    /// `verified_at` is stamped.
    pub fn check<T>(&mut self, memo: &Memo<T>) -> Verdict {
        self.stats.touched += 1;
        if !self.records.contains_key(&memo.key) {
            return Verdict::Missing;
        }
        if let Some(why) = self.dirty.get(&memo.key) {
            return Verdict::Dirty(*why);
        }
        memo.verified_at.store(self.revision.get(), Relaxed);
        Verdict::Valid
    }

    /// A dirty memo's reads all still observe their recorded values: clear the
    /// mark and stamp it.
    pub fn confirm<T>(&mut self, memo: &Memo<T>) {
        self.dirty.remove(&memo.key);
        memo.verified_at.store(self.revision.get(), Relaxed);
    }

    /// Records a freshly computed memo, replacing any previous record and its
    /// subscriptions. `unchanged` backdates: the output's observed value equals
    /// the previous one, so `changed_at` is kept and readers stay valid. The
    /// caller never backdates a member of a signature cycle.
    pub fn insert<T>(
        &mut self,
        key: MemoKey,
        text: ContentHash,
        value: Result<Arc<T>, hirsel_types::QueryError>,
        reads: Arc<Reads>,
        durability: Durability,
        unchanged: bool,
    ) -> Memo<T> {
        self.stats.touched += 1;
        self.record_recompute(key.kind);
        let changed_at = match self.records.get(&key) {
            Some(previous) if unchanged => previous.changed_at,
            _ => self.revision,
        };
        self.unsubscribe(&key);
        for (read, _) in reads.iter() {
            self.subscribers.entry(*read).or_default().insert(key);
            if matches!(
                read,
                ReadKey::ValidationUniverse(_) | ReadKey::UniverseErrors | ReadKey::FirstHeap
            ) {
                self.validation_contexts.insert(*read);
            }
        }
        self.by_def.entry(key.def).or_default().insert(key);
        self.records.insert(
            key,
            Record {
                reads: reads.clone(),
                durability,
                changed_at,
            },
        );
        self.dirty.remove(&key);
        Memo {
            key,
            text,
            value,
            reads,
            durability,
            changed_at,
            verified_at: AtomicU64::new(self.revision.get()),
        }
    }

    /// Adopts a memo whose record came from elsewhere (the result store):
    /// subscribes it without counting a recompute.
    pub fn adopt(&mut self, key: MemoKey, reads: Arc<Reads>, durability: Durability) {
        self.stats.touched += 1;
        self.unsubscribe(&key);
        for (read, _) in reads.iter() {
            self.subscribers.entry(*read).or_default().insert(key);
            if matches!(
                read,
                ReadKey::ValidationUniverse(_) | ReadKey::UniverseErrors | ReadKey::FirstHeap
            ) {
                self.validation_contexts.insert(*read);
            }
        }
        self.by_def.entry(key.def).or_default().insert(key);
        self.records.insert(
            key,
            Record {
                reads,
                durability,
                changed_at: self.revision,
            },
        );
        self.dirty.remove(&key);
    }

    /// Adopts a stored value and its read record, restoring subscribers without
    /// charging a recomputation to the current session.
    pub fn adopt_value<T>(
        &mut self,
        key: MemoKey,
        text: ContentHash,
        value: Result<Arc<T>, hirsel_types::QueryError>,
        reads: Arc<Reads>,
        durability: Durability,
    ) -> Memo<T> {
        self.adopt(key, reads.clone(), durability);
        Memo {
            key,
            text,
            value,
            reads,
            durability,
            changed_at: self.revision,
            verified_at: AtomicU64::new(self.revision.get()),
        }
    }

    /// Forgets a memo and its subscriptions (a removed definition, a retired
    /// registration, an evicted hold).
    pub fn remove(&mut self, key: &MemoKey) {
        self.stats.touched += 1;
        self.unsubscribe(key);
        self.dirty.remove(key);
        if let Some(keys) = self.by_def.get_mut(&key.def) {
            keys.remove(key);
            if keys.is_empty() {
                self.by_def.remove(&key.def);
            }
        }
    }

    fn unsubscribe(&mut self, key: &MemoKey) {
        if let Some(previous) = self.records.remove(key) {
            for (read, _) in previous.reads.iter() {
                if let Some(set) = self.subscribers.get_mut(read) {
                    set.remove(key);
                    if set.is_empty() {
                        self.subscribers.remove(read);
                        self.validation_contexts.remove(read);
                    }
                }
            }
        }
    }

    /// The bytes the bookkeeping holds, for `--mem-usage`.
    pub fn heap_bytes(&self) -> usize {
        let entry = std::mem::size_of::<MemoKey>() + std::mem::size_of::<Record>();
        let read = std::mem::size_of::<(ReadKey, ContentHash)>();
        self.validation_contexts.capacity() * std::mem::size_of::<ReadKey>()
            + self.records.len() * entry
            + self
                .records
                .values()
                .map(|r| r.reads.len() * read)
                .sum::<usize>()
            + self
                .subscribers
                .values()
                .map(|s| std::mem::size_of::<ReadKey>() + s.len() * std::mem::size_of::<MemoKey>())
                .sum::<usize>()
    }
}

/// The `--timings` count for a dirty mark caused by a read of `kind`.
pub fn invalidation_count(kind: ReadKind) -> Count {
    match kind {
        ReadKind::Signature => Count::InvalidatedSignature,
        ReadKind::FailedSignature => Count::InvalidatedFailedSignature,
        ReadKind::Lookup => Count::InvalidatedLookup,
        ReadKind::NameSet => Count::InvalidatedNameSet,
        ReadKind::Representation => Count::InvalidatedRepresentation,
        ReadKind::ExecutedBody => Count::InvalidatedExecutedBody,
        ReadKind::ExecutedSignature => Count::InvalidatedExecutedSignature,
        ReadKind::CitedText => Count::InvalidatedCitedText,
        ReadKind::DeclarationCandidates => Count::InvalidatedDeclarationCandidates,
        ReadKind::ValidationView => Count::InvalidatedValidationView,
        ReadKind::UniverseErrors => Count::InvalidatedUniverseErrors,
        ReadKind::FirstHeap => Count::InvalidatedFirstHeap,
        ReadKind::Environment => Count::InvalidatedEnvironment,
        ReadKind::ValidationUniverse => Count::InvalidatedValidationUniverse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::{DefPath, ItemPath, PackageId};
    fn def(name: &str) -> DefId {
        DefId::intern(&DefPath(ItemPath {
            package: PackageId {
                name: "g".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: name.into(),
        }))
    }
    fn h(s: &str) -> ContentHash {
        ContentHash::of_bytes(s.as_bytes())
    }
    fn reads(list: &[(ReadKey, &str)]) -> Arc<Reads> {
        Arc::new(Reads::from_vec(list.iter().map(|(k, v)| (*k, h(v))).collect()).unwrap())
    }

    #[test]
    fn worker_recomputations_do_not_require_retained_values_or_touch_subscriptions() {
        let mut graph = MemoGraph::default();
        let key = MemoKey::new(QueryKind::Lower, def("root"));
        // A failed, nonretained completion still performed real Lower work.
        graph.record_recompute(QueryKind::Lower);
        assert_eq!(graph.stats().recomputed[&QueryKind::Lower], 1);
        assert_eq!(graph.stats().touched, 0);
        assert!(graph.is_empty());
        // A later successful completion can publish facts separately, once.
        graph.record_recompute(QueryKind::Lower);
        let memo = graph.adopt_value(
            key,
            h("body"),
            Ok(Arc::new(())),
            reads(&[(ReadKey::Signature(def("callee")), "signature")]),
            Durability::Low,
        );
        assert_eq!(graph.stats().recomputed[&QueryKind::Lower], 2);
        assert_eq!(graph.stats().touched, 1);
        assert_eq!(graph.check(&memo), Verdict::Valid);
        assert_eq!(graph.stats().recomputed[&QueryKind::Lower], 2);
    }

    #[test]
    fn projecting_a_value_preserves_stamps_and_subscriptions_without_touching_graph() {
        let mut graph = MemoGraph::default();
        let a = def("a");
        let b = def("b");
        let memo = graph.insert(
            MemoKey::new(QueryKind::Lower, a),
            h("body"),
            Ok(Arc::new(vec![1, 2, 3])),
            reads(&[(ReadKey::Signature(b), "signature")]),
            Durability::Low,
            false,
        );
        let before = graph.stats().clone();
        let receipt = memo.map_value(Vec::len);
        assert_eq!(receipt.value.as_ref().unwrap().as_ref(), &3);
        assert_eq!(receipt.verified_at(), memo.verified_at());
        assert_eq!(receipt.changed_at, memo.changed_at);
        assert!(Arc::ptr_eq(&receipt.reads, &memo.reads));
        assert_eq!(graph.stats(), &before);
        let mut changed = ChangeSet::default();
        changed.read(ReadKey::Signature(b), Durability::Low);
        graph.apply(&changed);
        assert_eq!(
            graph.check(&receipt),
            Verdict::Dirty(Invalidation::Read(ReadKind::Signature))
        );
    }

    /// A memo both reading a changed candidate list and owning a replaced item
    /// is recomputed: the text mark replaces the read mark the change set made
    /// first, so deep verification cannot keep spans at the old item's anchor.
    #[test]
    fn a_text_mark_outranks_a_read_mark_in_one_change_set() {
        let mut graph = MemoGraph::default();
        let a = def("a");
        let key = MemoKey::new(QueryKind::Lower, a);
        let memo = graph.insert::<()>(
            key,
            h("a body"),
            Ok(Arc::new(())),
            reads(&[(ReadKey::DeclarationCandidates(a), "one")]),
            Durability::Low,
            false,
        );
        let mut moved = ChangeSet::default();
        moved.read(ReadKey::DeclarationCandidates(a), Durability::Low);
        moved.text(a, Durability::Low);
        graph.apply(&moved);
        assert_eq!(graph.check(&memo), Verdict::Dirty(Invalidation::Text));
        let mut again = ChangeSet::default();
        again.read(ReadKey::DeclarationCandidates(a), Durability::Low);
        graph.apply(&again);
        assert_eq!(graph.check(&memo), Verdict::Dirty(Invalidation::Text));
    }

    /// A chain of 1000 callers of `callee`, plus 1000 unrelated memos. A body
    /// edit of the callee marks only memos keyed on it; a signature edit marks
    /// only its subscribers; neither touches the unrelated memos.
    #[test]
    fn invalidation_touches_only_changed_inputs_and_subscribers() {
        let mut graph = MemoGraph::default();
        let callee = def("callee");
        let mut memos = vec![];
        let check = |d| MemoKey::new(QueryKind::Check, d);
        memos.push(graph.insert::<()>(
            check(callee),
            h("callee body"),
            Ok(Arc::new(())),
            reads(&[]),
            Durability::Low,
            false,
        ));
        for i in 0..1000 {
            let caller = def(&format!("caller{i}"));
            memos.push(graph.insert::<()>(
                check(caller),
                h("caller"),
                Ok(Arc::new(())),
                reads(&[(ReadKey::Signature(callee), "sig")]),
                Durability::Low,
                false,
            ));
            let other = def(&format!("other{i}"));
            memos.push(graph.insert::<()>(
                check(other),
                h("other"),
                Ok(Arc::new(())),
                reads(&[(ReadKey::Signature(other), "own")]),
                Durability::Low,
                false,
            ));
        }
        let base = graph.stats().touched;
        let mut body = ChangeSet::default();
        body.text(callee, Durability::Low);
        graph.apply(&body);
        assert_eq!(graph.stats().touched - base, 1, "one memo keyed on callee");
        assert!(graph.is_dirty(&check(callee)));
        assert_eq!(graph.check(&memos[0]), Verdict::Dirty(Invalidation::Text));
        assert_eq!(graph.check(&memos[1]), Verdict::Valid);
        assert_eq!(memos[1].verified_at(), graph.revision().get());

        let base = graph.stats().touched;
        let mut signature = ChangeSet::default();
        signature.read(ReadKey::Signature(callee), Durability::Low);
        graph.apply(&signature);
        assert_eq!(
            graph.stats().touched - base,
            1000,
            "exactly the subscribers"
        );
        assert_eq!(
            graph.stats().invalidated[ReadKind::Signature as usize],
            1000
        );
        assert!((1..2001).step_by(2).all(|i| graph.is_dirty(&memos[i].key)));
        assert!((2..2001).step_by(2).all(|i| !graph.is_dirty(&memos[i].key)));
    }

    #[test]
    fn overlapping_change_inputs_share_one_read_mark_and_text_still_wins() {
        let mut graph = MemoGraph::default();
        let (a, b) = (def("a"), def("b"));
        let key = MemoKey::new(QueryKind::Check, a);
        let memo = graph.insert::<()>(
            key,
            h("a"),
            Ok(Arc::new(())),
            reads(&[
                (ReadKey::Signature(b), "signature"),
                (ReadKey::Representation(b), "shape"),
            ]),
            Durability::Low,
            false,
        );
        let base = graph.stats().touched;
        let mut change = ChangeSet::default();
        change.read(ReadKey::Signature(b), Durability::Low);
        change.read(ReadKey::Representation(b), Durability::Low);
        change.read(ReadKey::Signature(b), Durability::Low);
        graph.apply(&change);
        assert_eq!(graph.stats().touched - base, 1);
        assert_eq!(graph.stats().invalidated[ReadKind::Signature as usize], 1);
        assert_eq!(
            graph.check(&memo),
            Verdict::Dirty(Invalidation::Read(ReadKind::Signature))
        );
        change.text(a, Durability::Low);
        change.text(a, Durability::Low);
        let base = graph.stats().touched;
        graph.apply(&change);
        assert_eq!(
            graph.stats().touched - base,
            2,
            "one read mark then one text override"
        );
        assert_eq!(graph.check(&memo), Verdict::Dirty(Invalidation::Text));
        assert_eq!(graph.stats().invalidated_text, 1);
    }

    #[test]
    fn backdating_keeps_changed_at_and_resubscribes() {
        let mut graph = MemoGraph::default();
        let (a, b) = (def("a"), def("b"));
        let key = MemoKey::new(QueryKind::Signature, a);
        let first = graph.insert::<()>(
            key,
            h("a"),
            Ok(Arc::new(())),
            reads(&[(ReadKey::Signature(b), "1")]),
            Durability::Low,
            false,
        );
        let mut change = ChangeSet::default();
        change.text(a, Durability::Low);
        graph.apply(&change);
        let again = graph.insert::<()>(
            key,
            h("a2"),
            Ok(Arc::new(())),
            reads(&[(ReadKey::Signature(def("c")), "1")]),
            Durability::Low,
            true,
        );
        assert_eq!(again.changed_at, first.changed_at, "backdated");
        assert_eq!(graph.subscribers(&ReadKey::Signature(b)).count(), 0);
        assert_eq!(graph.subscribers(&ReadKey::Signature(def("c"))).count(), 1);
        graph.apply(&change);
        let fresh = graph.insert::<()>(
            key,
            h("a3"),
            Ok(Arc::new(())),
            reads(&[]),
            Durability::Low,
            false,
        );
        assert_eq!(fresh.changed_at, graph.revision());
        graph.remove(&key);
        assert!(graph.is_empty());
        assert_eq!(graph.memos_of(a).count(), 0);
        assert_eq!(graph.check(&fresh), Verdict::Missing);
    }

    #[test]
    fn durability_levels_advance_from_the_change_down() {
        let mut graph = MemoGraph::default();
        let mut change = ChangeSet::default();
        change.read(ReadKey::Environment, Durability::Medium);
        let r = graph.apply(&change);
        assert_eq!(graph.last_changed(Durability::Low), r);
        assert_eq!(graph.last_changed(Durability::Medium), r);
        assert_eq!(graph.last_changed(Durability::High), Revision::FIRST);
        assert_eq!(
            graph.apply(&ChangeSet::default()),
            r,
            "no change, no revision"
        );
    }
}
