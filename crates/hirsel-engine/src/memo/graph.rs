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
    output: Option<ContentHash>,
}

/// A broad input invalidates a shared reverse-index snapshot. Confirmations
/// name only demanded records; adding/removing subscriptions uses Arc::make_mut
/// so a later mutation cannot change an already published invalidation group.
#[derive(Default)]
struct ContextInvalidation {
    pending: Option<Arc<HashSet<MemoKey>>>,
    confirmed: HashSet<MemoKey>,
}
impl ContextInvalidation {
    fn contains(&self, key: &MemoKey) -> bool {
        self.pending.as_ref().is_some_and(|keys| keys.contains(key))
            && !self.confirmed.contains(key)
    }
    fn confirm(&mut self, key: MemoKey) {
        if self
            .pending
            .as_ref()
            .is_some_and(|keys| keys.contains(&key))
        {
            self.confirmed.insert(key);
        }
    }
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
    subscribers: HashMap<ReadKey, Arc<HashSet<MemoKey>>>,
    /// Broad V inputs invalidate shared reverse-index groups without
    /// eagerly accessing each subscriber's memo record.
    validation_contexts: HashMap<ReadKey, ContextInvalidation>,
    by_def: HashMap<DefId, HashSet<MemoKey>, IdHasher>,
    dirty: HashMap<MemoKey, Invalidation>,
    stats: MemoStats,
    touched_memos: Option<HashSet<MemoKey>>,
}

impl Default for MemoGraph {
    fn default() -> Self {
        Self {
            revision: Revision::FIRST,
            last_changed: [Revision::FIRST; 3],
            records: HashMap::new(),
            subscribers: HashMap::new(),
            validation_contexts: HashMap::new(),
            by_def: HashMap::default(),
            dirty: HashMap::new(),
            stats: MemoStats::default(),
            touched_memos: None,
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
    /// A source-only handoff starts a fresh engine's physical-work counters.
    /// Logical stamps, dirty marks and reverse subscriptions remain intact.
    pub(crate) fn reset_stats(&mut self) {
        self.stats = MemoStats::default();
        self.touched_memos = None;
    }
    /// Start a request's distinct-memo inventory without changing raw counters.
    pub fn begin_touch_tracking(&mut self) {
        self.touched_memos.get_or_insert_with(HashSet::new);
    }
    /// End the request and return every memo whose record was read or written.
    pub fn take_touched_memos(&mut self) -> Vec<MemoKey> {
        let mut touched: Vec<_> = self.touched_memos.take().into_iter().flatten().collect();
        touched.sort();
        touched
    }
    fn touch(&mut self, key: MemoKey) {
        self.stats.touched += 1;
        if let Some(touched) = &mut self.touched_memos {
            touched.insert(key);
        }
    }

    /// Read-only memo/read inventory: never verifies, stamps, or touches records.
    pub fn snapshot(&self) -> Vec<(MemoKey, Arc<Reads>)> {
        self.records
            .iter()
            .map(|(key, record)| (*key, record.reads.clone()))
            .collect()
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
        self.subscribers
            .get(key)
            .into_iter()
            .flat_map(|keys| keys.iter())
    }
    /// Contextual V read keys that currently have subscribers.
    pub fn validation_context_reads(&self) -> impl Iterator<Item = ReadKey> + '_ {
        self.validation_contexts.keys().copied()
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
        for context in self.validation_contexts.values() {
            defs.extend(
                context
                    .pending
                    .iter()
                    .flat_map(|keys| keys.iter())
                    .filter(|key| !context.confirmed.contains(key))
                    .map(|key| key.def),
            );
        }
        defs.sort();
        defs.dedup();
        defs
    }
    pub fn is_dirty(&self, key: &MemoKey) -> bool {
        self.invalidation(key).is_some()
    }

    /// Pending broad-context notifications, inspected through indices only.
    /// A direct input/text change or an absent memo cannot use this shortcut.
    /// The caller must compare every returned input with its completed facts;
    /// this neither verifies nor confirms the memo itself.
    pub(crate) fn pending_context_reads(&self, key: &MemoKey) -> Option<Vec<ReadKey>> {
        if self.dirty.contains_key(key) || !self.by_def.get(&key.def)?.contains(key) {
            return None;
        }
        let reads: Vec<_> = self
            .validation_contexts
            .iter()
            .filter_map(|(read, context)| context.contains(key).then_some(*read))
            .collect();
        Some(reads)
    }

    /// Index-only inspection: no per-memo Record or value is read here.
    fn invalidation(&self, key: &MemoKey) -> Option<Invalidation> {
        self.dirty.get(key).copied().or_else(|| {
            self.validation_contexts
                .iter()
                .filter_map(|(read, context)| context.contains(key).then_some(read.kind()))
                .min()
                .map(Invalidation::Read)
        })
    }

    /// As in Salsa's last-changed/verified-at split (ref-salsa
    /// src/function/maybe_changed_after.rs:358-389), a changed input does not
    /// verify its subscribers. Here shared reverse-index groups also avoid
    /// reading their individual memo records during broadcast bookkeeping.
    fn invalidate_context(&mut self, read: ReadKey) {
        let Some(subscribers) = self.subscribers.get(&read).cloned() else {
            return;
        };
        // Count logical newly dirty members using indices only. Actual memo
        // accesses remain counted by touch at demand, adoption and confirmation.
        let newly_dirty = subscribers
            .iter()
            .filter(|key| self.invalidation(key).is_none())
            .count() as u64;
        let context = self
            .validation_contexts
            .get_mut(&read)
            .expect("indexed context");
        context.pending = Some(subscribers);
        context.confirmed.clear();
        self.stats.invalidated[read.kind() as usize] += newly_dirty;
        for _ in 0..newly_dirty {
            timings::count(invalidation_count(read.kind()));
        }
    }

    fn confirm_contexts(&mut self, key: MemoKey) {
        for context in self.validation_contexts.values_mut() {
            context.confirm(key);
        }
    }

    /// The canonical output of a settled query, independent of its action.
    pub(crate) fn output(&self, key: &MemoKey) -> Option<ContentHash> {
        self.records.get(key)?.output
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
        // requests before touching records. Direct reads keep their precise
        // reason; broad V contexts publish shared invalidation groups.
        let mut readers = BTreeMap::new();
        let mut contexts = BTreeSet::new();
        for (read, _) in &changes.reads {
            if matches!(
                read,
                ReadKey::ValidationUniverse(_) | ReadKey::UniverseErrors | ReadKey::FirstHeap
            ) {
                contexts.insert(*read);
                continue;
            }
            for key in self.subscribers(read) {
                readers.entry(*key).or_insert(read.kind());
            }
        }
        for (key, kind) in readers {
            self.mark(key, Invalidation::Read(kind));
        }
        for read in contexts {
            self.invalidate_context(read);
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
        self.touch(key);
        // A text mark outranks a read mark: a read-marked memo is deep-verified
        // and may be kept, but one whose own items changed is recomputed. A
        // change set marks its reads first, so the text mark must replace one.
        match (self.invalidation(&key), why) {
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
        self.touch(memo.key);
        if !self.records.contains_key(&memo.key) {
            return Verdict::Missing;
        }
        if let Some(why) = self.invalidation(&memo.key) {
            return Verdict::Dirty(why);
        }
        memo.verified_at.store(self.revision.get(), Relaxed);
        Verdict::Valid
    }

    /// A dirty memo's reads all still observe their recorded values: clear the
    /// mark and stamp it.
    pub fn confirm<T>(&mut self, memo: &Memo<T>) {
        if let Some(touched) = &mut self.touched_memos {
            touched.insert(memo.key);
        }
        self.dirty.remove(&memo.key);
        self.confirm_contexts(memo.key);
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
        self.touch(key);
        self.record_recompute(key.kind);
        let changed_at = match self.records.get(&key) {
            Some(previous) if unchanged => previous.changed_at,
            _ => self.revision,
        };
        self.unsubscribe(&key);
        for (read, _) in reads.iter() {
            Arc::make_mut(self.subscribers.entry(*read).or_default()).insert(key);
            if matches!(
                read,
                ReadKey::ValidationUniverse(_) | ReadKey::UniverseErrors | ReadKey::FirstHeap
            ) {
                self.validation_contexts.entry(*read).or_default();
            }
        }
        self.by_def.entry(key.def).or_default().insert(key);
        self.records.insert(
            key,
            Record {
                reads: reads.clone(),
                durability,
                changed_at,
                output: None,
            },
        );
        self.dirty.remove(&key);
        self.confirm_contexts(key);
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

    /// Equal settled outputs preserve their change revision even when their
    /// source or dependency witnesses changed. None marks a provisional or
    /// unencodable product and can never establish equality.
    pub(crate) fn insert_output<T>(
        &mut self,
        key: MemoKey,
        text: ContentHash,
        value: Result<Arc<T>, hirsel_types::QueryError>,
        reads: Arc<Reads>,
        durability: Durability,
        output: Option<ContentHash>,
    ) -> Memo<T> {
        let output = output.filter(|_| value.is_ok());
        let unchanged = output.is_some() && output == self.output(&key);
        let memo = self.insert(key, text, value, reads, durability, unchanged);
        self.records.get_mut(&key).expect("inserted record").output = output;
        memo
    }

    pub(crate) fn adopt_output<T>(
        &mut self,
        key: MemoKey,
        text: ContentHash,
        value: Result<Arc<T>, hirsel_types::QueryError>,
        reads: Arc<Reads>,
        durability: Durability,
        output: Option<ContentHash>,
    ) -> Memo<T> {
        let output = output.filter(|_| value.is_ok());
        let previous = self.records.get(&key).and_then(|record| {
            (output.is_some() && output == record.output).then_some(record.changed_at)
        });
        let mut memo = self.adopt_value(key, text, value, reads, durability);
        let record = self.records.get_mut(&key).expect("adopted record");
        record.output = output;
        if let Some(revision) = previous {
            record.changed_at = revision;
            memo.changed_at = revision;
        }
        memo
    }

    /// Adopts a memo whose record came from elsewhere (the result store):
    /// subscribes it without counting a recompute.
    pub fn adopt(&mut self, key: MemoKey, reads: Arc<Reads>, durability: Durability) {
        self.touch(key);
        self.unsubscribe(&key);
        for (read, _) in reads.iter() {
            Arc::make_mut(self.subscribers.entry(*read).or_default()).insert(key);
            if matches!(
                read,
                ReadKey::ValidationUniverse(_) | ReadKey::UniverseErrors | ReadKey::FirstHeap
            ) {
                self.validation_contexts.entry(*read).or_default();
            }
        }
        self.by_def.entry(key.def).or_default().insert(key);
        self.records.insert(
            key,
            Record {
                reads,
                durability,
                changed_at: self.revision,
                output: None,
            },
        );
        self.dirty.remove(&key);
        self.confirm_contexts(key);
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
        self.touch(*key);
        self.confirm_contexts(*key);
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
                    Arc::make_mut(set).remove(key);
                    if set.is_empty() {
                        self.subscribers.remove(read);
                        self.validation_contexts.remove(read);
                    }
                }
            }
        }
    }

    /// Physical backing bound for retaining this index across source revisions.
    /// Values and session interners remain with their separately admitted owners.
    /// Shared read/index allocations are conservatively counted per holder;
    /// inspection allocates nothing and does not touch verification stamps.
    pub fn retained_bytes(&self) -> Option<u64> {
        fn hash<K, V>(capacity: usize) -> Option<usize> {
            if capacity == 0 {
                return Some(0);
            }
            // Rust std hashbrown raw capacity_to_buckets/calculate_layout_for:
            // 7/8 load, power-of-two buckets, control group and alignment slack.
            let buckets = capacity
                .checked_mul(8)?
                .div_ceil(7)
                .checked_next_power_of_two()?
                .max(4);
            buckets
                .checked_mul(size_of::<(K, V)>().checked_add(1)?)?
                .checked_add(16)?
                .checked_add(std::mem::align_of::<(K, V)>())
        }
        fn set<T>(capacity: usize) -> Option<usize> {
            hash::<T, ()>(capacity)
        }
        fn arc<T: ?Sized>(value: &T) -> Option<usize> {
            Some(
                std::alloc::Layout::new::<[usize; 2]>()
                    .extend(std::alloc::Layout::for_value(value))
                    .ok()?
                    .0
                    .pad_to_align()
                    .size(),
            )
        }
        let mut total = size_of::<Self>()
            .checked_add(hash::<MemoKey, Record>(self.records.capacity())?)?
            .checked_add(hash::<ReadKey, Arc<HashSet<MemoKey>>>(
                self.subscribers.capacity(),
            )?)?
            .checked_add(hash::<ReadKey, ContextInvalidation>(
                self.validation_contexts.capacity(),
            )?)?
            .checked_add(hash::<DefId, HashSet<MemoKey>>(self.by_def.capacity())?)?
            .checked_add(hash::<MemoKey, Invalidation>(self.dirty.capacity())?)?;
        for record in self.records.values() {
            total = total.checked_add(arc(&*record.reads)?)?;
            for (_, bytes) in record.reads.allocations() {
                total = total.checked_add(bytes)?;
            }
        }
        for subscribers in self.subscribers.values() {
            total = total
                .checked_add(arc(&**subscribers)?)?
                .checked_add(set::<MemoKey>(subscribers.capacity())?)?;
        }
        for context in self.validation_contexts.values() {
            total = total.checked_add(set::<MemoKey>(context.confirmed.capacity())?)?;
            if let Some(pending) = &context.pending {
                total = total
                    .checked_add(arc(&**pending)?)?
                    .checked_add(set::<MemoKey>(pending.capacity())?)?;
            }
        }
        for keys in self.by_def.values() {
            total = total.checked_add(set::<MemoKey>(keys.capacity())?)?;
        }
        if let Some(touched) = &self.touched_memos {
            total = total.checked_add(set::<MemoKey>(touched.capacity())?)?;
        }
        let mut stats = crate::NativeMetadataWeight::default();
        stats.tree::<QueryKind, u64>(self.stats.recomputed.len());
        (total as u64).checked_add(stats.bytes())
    }

    /// The bytes the bookkeeping holds, for `--mem-usage`.
    pub fn heap_bytes(&self) -> usize {
        let entry = std::mem::size_of::<MemoKey>() + std::mem::size_of::<Record>();
        let read = std::mem::size_of::<(ReadKey, ContentHash)>();
        self.validation_contexts.capacity()
            * (std::mem::size_of::<ReadKey>() + std::mem::size_of::<ContextInvalidation>())
            + self
                .validation_contexts
                .iter()
                .map(|(read, context)| {
                    let confirmations =
                        context.confirmed.capacity() * std::mem::size_of::<MemoKey>();
                    let frozen = context
                        .pending
                        .as_ref()
                        .filter(|keys| {
                            self.subscribers
                                .get(read)
                                .is_none_or(|current| !Arc::ptr_eq(keys, current))
                        })
                        .map_or(0, |keys| {
                            std::mem::size_of::<HashSet<MemoKey>>()
                                + 2 * std::mem::size_of::<usize>()
                                + keys.capacity() * std::mem::size_of::<MemoKey>()
                        });
                    confirmations + frozen
                })
                .sum::<usize>()
            + self.records.len() * entry
            + self
                .records
                .values()
                .map(|r| r.reads.len() * read)
                .sum::<usize>()
            + self
                .subscribers
                .values()
                .map(|s| {
                    std::mem::size_of::<ReadKey>()
                        + std::mem::size_of::<Arc<HashSet<MemoKey>>>()
                        + std::mem::size_of::<HashSet<MemoKey>>()
                        + 2 * std::mem::size_of::<usize>()
                        + s.capacity() * std::mem::size_of::<MemoKey>()
                })
                .sum::<usize>()
    }
}

/// The `--timings` count for a dirty mark caused by a read of `kind`.
pub fn invalidation_count(kind: ReadKind) -> Count {
    match kind {
        ReadKind::Signature | ReadKind::DeclaredSignature | ReadKind::PrivateSummary => {
            Count::InvalidatedSignature
        }
        ReadKind::FailedSignature => Count::InvalidatedFailedSignature,
        ReadKind::Lookup => Count::InvalidatedLookup,
        ReadKind::NameSet => Count::InvalidatedNameSet,
        ReadKind::Representation => Count::InvalidatedRepresentation,
        ReadKind::ExecutedBody | ReadKind::LayoutBody => Count::InvalidatedExecutedBody,
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
    fn retained_index_bound_covers_physical_storage_and_preserves_changed_only_work() {
        for width in [1, 4, 16] {
            let ids = (0..width)
                .map(|i| def(&format!("carry{i}")))
                .collect::<Vec<_>>();
            let source = def("source");
            let ((mut graph, bound), allocations) = crate::test_alloc::measured(|| {
                let mut graph = MemoGraph::default();
                for &id in &ids {
                    graph.adopt(
                        MemoKey::new(QueryKind::Signature, id),
                        reads(&[
                            (ReadKey::Signature(source), "old"),
                            (ReadKey::UniverseErrors, "old"),
                        ]),
                        Durability::Low,
                    );
                }
                graph.begin_touch_tracking();
                let mut change = ChangeSet::default();
                change.read(ReadKey::UniverseErrors, Durability::Low);
                graph.apply(&change);
                // Mutating subscriptions leaves the pending reverse snapshot live.
                graph.adopt(
                    MemoKey::new(QueryKind::Signature, ids[0]),
                    reads(&[(ReadKey::Signature(source), "current")]),
                    Durability::Low,
                );
                graph.record_recompute(QueryKind::Signature);
                let before = graph.stats().clone();
                let (bound, sizing) =
                    crate::test_alloc::measured_independent(|| graph.retained_bytes());
                assert_eq!(sizing.allocations, 0);
                assert_eq!(*graph.stats(), before);
                (graph, bound.unwrap())
            });
            assert!(allocations.live >= 0);
            assert!(
                allocations.live as u64 <= bound,
                "width {width}: retained {} exceeds {bound}",
                allocations.live
            );
            let untouched = MemoKey::new(QueryKind::Signature, def("unrelated"));
            graph.adopt(
                untouched,
                reads(&[(ReadKey::Environment, "environment")]),
                Durability::Low,
            );
            let before = graph.stats().touched;
            let mut change = ChangeSet::default();
            change.read(ReadKey::Signature(source), Durability::Low);
            graph.apply(&change);
            assert_eq!(graph.stats().touched - before, width as u64);
            assert!(!graph.is_dirty(&untouched));
            assert_eq!(
                graph.subscribers(&ReadKey::Signature(source)).count(),
                width
            );
            let (_, released) = crate::test_alloc::measured(|| drop(graph));
            assert!(
                released.live < 0,
                "dropping the retained index releases its physical backing"
            );
        }
    }

    #[test]
    fn broad_context_changes_keep_subscribers_dirty_without_accessing_their_records() {
        for width in [1, 4, 16] {
            let mut graph = MemoGraph::default();
            let memos = (0..width)
                .map(|i| {
                    graph.adopt_value(
                        MemoKey::new(QueryKind::Lower, def(&format!("root{i}"))),
                        h("body"),
                        Ok(Arc::new(())),
                        reads(&[(ReadKey::UniverseErrors, "old universe")]),
                        Durability::Low,
                    )
                })
                .collect::<Vec<_>>();
            let before = graph.stats().touched;
            graph.begin_touch_tracking();
            let mut change = ChangeSet::default();
            change.read(ReadKey::UniverseErrors, Durability::Low);
            graph.apply(&change);
            assert_eq!(
                graph.stats().touched,
                before,
                "broadcast reads only reverse indices"
            );
            assert!(graph.take_touched_memos().is_empty());
            assert_eq!(
                graph.stats().invalidated[ReadKind::UniverseErrors as usize],
                width
            );
            assert_eq!(graph.dirty_definitions().len() as u64, width);
            assert!(memos.iter().all(|memo| graph.is_dirty(&memo.key)));
            graph.begin_touch_tracking();
            for memo in &memos {
                assert_eq!(
                    graph.check(memo),
                    Verdict::Dirty(Invalidation::Read(ReadKind::UniverseErrors))
                );
                // Only the caller's complete read verification permits this.
                graph.confirm(memo);
                assert_eq!(graph.check(memo), Verdict::Valid);
            }
            assert_eq!(graph.stats().touched - before, 2 * width);
            assert_eq!(graph.take_touched_memos().len() as u64, width);
            assert!(graph.dirty_definitions().is_empty());
            graph.apply(&change);
            assert!(
                memos.iter().all(|memo| graph.is_dirty(&memo.key)),
                "a later change invalidates confirmed members again"
            );
        }
    }

    #[test]
    fn context_snapshots_survive_subscription_replacement_and_keep_scopes_independent() {
        let mut graph = MemoGraph::default();
        let a = MemoKey::new(QueryKind::Lower, def("a"));
        let b = MemoKey::new(QueryKind::Lower, def("b"));
        let c = MemoKey::new(QueryKind::Lower, def("c"));
        let keys = [
            (ReadKey::UniverseErrors, "old"),
            (ReadKey::FirstHeap, "heap"),
        ];
        let first = graph.adopt_value(a, h("a"), Ok(Arc::new(())), reads(&keys), Durability::Low);
        graph.adopt(b, reads(&keys[..1]), Durability::Low);
        let mut change = ChangeSet::default();
        change.read(ReadKey::UniverseErrors, Durability::Low);
        graph.apply(&change);
        graph.adopt(
            b,
            reads(&[(ReadKey::UniverseErrors, "new")]),
            Durability::Low,
        );
        graph.adopt(
            c,
            reads(&[(ReadKey::UniverseErrors, "new")]),
            Durability::Low,
        );
        assert!(graph.is_dirty(&a));
        assert!(!graph.is_dirty(&b));
        assert!(
            !graph.is_dirty(&c),
            "new subscribers observe the current input"
        );
        assert_eq!(graph.dirty_definitions(), vec![a.def]);
        graph.confirm(&first);
        let mut heap = ChangeSet::default();
        heap.read(ReadKey::FirstHeap, Durability::Low);
        graph.apply(&heap);
        assert!(graph.is_dirty(&a));
        assert!(!graph.is_dirty(&b));
        assert!(!graph.is_dirty(&c));
        let mut text = ChangeSet::default();
        text.text(a.def, Durability::Low);
        graph.apply(&text);
        assert_eq!(graph.check(&first), Verdict::Dirty(Invalidation::Text));
        graph.remove(&a);
        assert!(
            graph.dirty_definitions().is_empty(),
            "removed members cannot remain pending"
        );
        graph.apply(&change);
        assert!(graph.is_dirty(&b));
        assert!(graph.is_dirty(&c));
        assert!(!graph.is_dirty(&a));
        graph.remove(&b);
        graph.remove(&c);
        assert!(graph.validation_context_reads().next().is_none());
        assert!(graph.dirty_definitions().is_empty());
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
    fn request_touch_inventory_preserves_raw_operations_and_all_memo_identities() {
        let mut graph = MemoGraph::default();
        let key = MemoKey::new(QueryKind::Check, def("a"));
        graph.begin_touch_tracking();
        let memo = graph.insert(
            key,
            h("a"),
            Ok(Arc::new(())),
            reads(&[]),
            Durability::Low,
            false,
        );
        assert_eq!(graph.check(&memo), Verdict::Valid);
        graph.begin_touch_tracking(); // Multiple input applications are one request.
        assert_eq!(graph.check(&memo), Verdict::Valid);
        graph.remove(&key);
        assert_eq!(graph.stats().touched, 4, "retain every physical operation");
        assert_eq!(graph.take_touched_memos(), vec![key]);
        assert!(graph.take_touched_memos().is_empty());
        graph.begin_touch_tracking();
        let missing = MemoKey::new(QueryKind::Modes, def("b"));
        graph.remove(&missing);
        assert_eq!(graph.stats().touched, 5);
        assert_eq!(graph.take_touched_memos(), vec![missing]);
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
    fn settled_output_equality_backdates_but_provisional_products_never_do() {
        let mut graph = MemoGraph::default();
        let owner = def("owner");
        let key = MemoKey::new(QueryKind::Summary, owner);
        let first = graph.insert_output(
            key,
            h("source1"),
            Ok(Arc::new(())),
            reads(&[]),
            Durability::Low,
            Some(h("output")),
        );
        let mut changes = ChangeSet::default();
        changes.text(owner, Durability::Low);
        graph.apply(&changes);
        let equal = graph.insert_output(
            key,
            h("source2"),
            Ok(Arc::new(())),
            reads(&[(ReadKey::Signature(def("new-dependency")), "new")]),
            Durability::Low,
            Some(h("output")),
        );
        assert_eq!(equal.changed_at, first.changed_at);
        assert_eq!(graph.output(&key), Some(h("output")));
        assert_eq!(
            graph
                .subscribers(&ReadKey::Signature(def("new-dependency")))
                .count(),
            1
        );
        graph.apply(&changes);
        let provisional = graph.insert_output(
            key,
            h("source3"),
            Ok(Arc::new(())),
            reads(&[]),
            Durability::Low,
            None,
        );
        assert_eq!(provisional.changed_at, graph.revision());
        assert_eq!(graph.output(&key), None);
        graph.apply(&changes);
        let settled = graph.insert_output(
            key,
            h("source4"),
            Ok(Arc::new(())),
            reads(&[]),
            Durability::Low,
            Some(h("output")),
        );
        assert_eq!(settled.changed_at, graph.revision());
        graph.apply(&changes);
        let adopted = graph.adopt_output(
            key,
            h("source5"),
            Ok(Arc::new(())),
            reads(&[]),
            Durability::Low,
            Some(h("output")),
        );
        assert_eq!(adopted.changed_at, settled.changed_at);
        graph.apply(&changes);
        let changed = graph.insert_output(
            key,
            h("source6"),
            Ok(Arc::new(())),
            reads(&[]),
            Durability::Low,
            Some(h("different-output")),
        );
        assert_eq!(changed.changed_at, graph.revision());
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
