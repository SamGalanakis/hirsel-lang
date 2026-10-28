//! Bounded physical-work metadata across immutable source views. This ledger
//! holds counters only; it cannot validate, reuse or invalidate compiler work.
use super::{CompileInspection, DefinitionWork, RequestWork};
use hirsel_engine::{
    FrontBodyQueries, FrontLowerQueries, QueryKind, Reads, ToolError,
    memo::{MemoKey, MemoStats},
    runtime::{MemoryReservation, ProcessBudget, RuntimeEvent, RuntimeObserver},
};
use hirsel_syntax::DefPath;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub(super) const KINDS: [QueryKind; 6] = [
    QueryKind::Resolve,
    QueryKind::Signature,
    QueryKind::Check,
    QueryKind::Modes,
    QueryKind::Lower,
    QueryKind::Evaluate,
];
#[derive(Clone, Copy, Default)]
pub(super) struct Counters {
    pub phases: [u64; 6],
    pub validations: u64,
    pub lower_calls: u64,
}
impl Counters {
    fn since(self, previous: Self) -> Option<Self> {
        let mut phases = [0; 6];
        for (index, count) in phases.iter_mut().enumerate() {
            *count = self.phases[index].checked_sub(previous.phases[index])?;
        }
        Some(Self {
            phases,
            validations: self.validations.checked_sub(previous.validations)?,
            lower_calls: self.lower_calls.checked_sub(previous.lower_calls)?,
        })
    }
    fn add(&mut self, delta: Self) -> Option<()> {
        for (count, delta) in self.phases.iter_mut().zip(delta.phases) {
            *count = count.checked_add(delta)?;
        }
        self.validations = self.validations.checked_add(delta.validations)?;
        self.lower_calls = self.lower_calls.checked_add(delta.lower_calls)?;
        Some(())
    }
}
struct State {
    memory: Option<MemoryReservation>,
    counts: BTreeMap<DefPath, Counters>,
    stats: MemoStats,
    complete: bool,
}
pub(super) struct InspectionLedger {
    budget: Arc<ProcessBudget>,
    capacity: u64,
    state: Mutex<State>,
}
impl InspectionLedger {
    pub fn new(budget: Arc<ProcessBudget>, capacity: u64) -> Arc<Self> {
        Arc::new(Self {
            budget,
            capacity,
            state: Mutex::new(State {
                memory: None,
                counts: BTreeMap::new(),
                stats: MemoStats::default(),
                complete: true,
            }),
        })
    }
    pub fn admit(&self) -> Result<(), ToolError> {
        {
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if !state.complete {
                return Err(unavailable());
            }
            if state.memory.is_some() {
                return Ok(());
            }
        }
        // Admission can evict an idle graph whose meter flushes into this
        // ledger. Reserve without its lock, then publish the reservation.
        let memory =
            super::host_selector::GraphBudget::reserve_product(&self.budget, self.capacity)
                .map_err(ToolError::Io);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.complete {
            return Err(unavailable());
        }
        if state.memory.is_none() {
            state.memory = Some(memory?);
        }
        Ok(())
    }

    fn add(
        &self,
        delta: &BTreeMap<DefPath, Counters>,
        stats: Option<&MemoStats>,
    ) -> Result<(), ToolError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.complete || state.memory.is_none() {
            return Err(unavailable());
        }
        let update = (|| {
            let mut metadata = hirsel_engine::NativeMetadataWeight::default();
            metadata.add(std::mem::size_of::<Self>());
            let new_rows = delta
                .keys()
                .filter(|path| !state.counts.contains_key(*path))
                .count();
            metadata.tree::<DefPath, Counters>(state.counts.len().checked_add(new_rows)?);
            for path in state.counts.keys().chain(
                delta
                    .keys()
                    .filter(|path| !state.counts.contains_key(*path)),
            ) {
                metadata.path(path);
            }
            if metadata.bytes() > self.capacity {
                return None;
            }
            for (path, counts) in delta {
                state.counts.entry(path.clone()).or_default().add(*counts)?;
            }
            if let Some(stats) = stats {
                state.stats.touched = state.stats.touched.checked_add(stats.touched)?;
                state.stats.invalidated_text = state
                    .stats
                    .invalidated_text
                    .checked_add(stats.invalidated_text)?;
                for (total, delta) in state.stats.invalidated.iter_mut().zip(stats.invalidated) {
                    *total = total.checked_add(delta)?;
                }
            }
            Some(())
        })();
        if update.is_none() {
            state.complete = false;
            return Err(unavailable());
        }
        Ok(())
    }
    fn definitions(
        &self,
        paths: &[DefPath],
    ) -> Result<(MemoStats, Vec<DefinitionWork>), ToolError> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.complete {
            return Err(unavailable());
        }
        let mut stats = state.stats.clone();
        for counts in state.counts.values() {
            for (kind, count) in KINDS.into_iter().zip(counts.phases) {
                let total = stats.recomputed.entry(kind).or_default();
                *total = total.checked_add(count).ok_or_else(unavailable)?;
            }
        }
        let definitions = paths
            .iter()
            .map(|path| {
                let counts = state.counts.get(path).copied().unwrap_or_default();
                DefinitionWork {
                    definition: path.clone(),
                    recomputations: KINDS.into_iter().zip(counts.phases).collect(),
                    body_validations: counts.validations,
                    lower_calls: counts.lower_calls,
                }
            })
            .collect();
        Ok((stats, definitions))
    }
}
pub(super) struct SourceInspection {
    pub counts: BTreeMap<DefPath, Counters>,
    pub memos: Vec<(MemoKey, Arc<Reads>)>,
    pub stats: MemoStats,
    pub initial: RequestWork,
    pub owner: Arc<MemoryReservation>,
}
struct Previous {
    store_adoptions: u64,
    /// Cumulative front refusals and memo operations already reported.
    refusals: (MemoStats, u64),
    counts: BTreeMap<DefPath, Counters>,
    initial: Option<RequestWork>,
    stats: Option<MemoStats>,
}
pub(super) struct GraphMeter {
    ledger: Arc<InspectionLedger>,
    front: Arc<FrontBodyQueries>,
    lower: Arc<FrontLowerQueries>,
    s0: BTreeMap<DefPath, Counters>,
    memos: Vec<(MemoKey, Arc<Reads>)>,
    previous: Mutex<Previous>,
    hold_bytes: u64,
    _source: Arc<MemoryReservation>,
}
impl GraphMeter {
    /// Current/S0 counters and flush temporaries have a fixed, known namespace.
    /// Bound their actual tree/path layout under the existing source partition.
    pub fn input_bytes(definitions: &[DefPath], memos: usize) -> Option<u64> {
        let mut metadata = hirsel_engine::NativeMetadataWeight::default();
        metadata.add(std::mem::size_of::<Self>());
        for _ in 0..4 {
            metadata.tree::<DefPath, Counters>(definitions.len());
            for path in definitions {
                metadata.path(path);
            }
        }
        metadata.add(memos.checked_mul(std::mem::size_of::<(MemoKey, Arc<Reads>)>())?);
        Some(metadata.bytes())
    }
    pub fn new(
        ledger: Arc<InspectionLedger>,
        front: Arc<FrontBodyQueries>,
        lower: Arc<FrontLowerQueries>,
        source: SourceInspection,
    ) -> Arc<Self> {
        let hold_bytes = source.initial.hold_bytes;
        Arc::new(Self {
            ledger,
            front,
            lower,
            s0: source.counts,
            memos: source.memos,
            hold_bytes,
            previous: Mutex::new(Previous {
                store_adoptions: 0,
                refusals: Default::default(),
                counts: BTreeMap::new(),
                initial: Some(source.initial),
                stats: Some(source.stats),
            }),
            _source: source.owner,
        })
    }
    pub fn flush(&self) -> Result<RequestWork, ToolError> {
        let mut previous = self
            .previous
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut current = self.s0.clone();
        for (path, kind, count) in self.front.work_snapshot() {
            let index = KINDS
                .iter()
                .position(|selected| *selected == kind)
                .ok_or_else(unavailable)?;
            let slot = &mut current.entry(path).or_default().phases[index];
            *slot = slot.checked_add(count).ok_or_else(unavailable)?;
        }
        for (path, work) in self.lower.work_snapshot() {
            let counts = current.entry(path).or_default();
            counts.phases[4] = counts.phases[4]
                .checked_add(work.lowerings)
                .ok_or_else(unavailable)?;
            counts.phases[5] = counts.phases[5]
                .checked_add(work.initializers)
                .ok_or_else(unavailable)?;
            counts.validations = counts
                .validations
                .checked_add(work.validation_runs)
                .ok_or_else(unavailable)?;
            counts.lower_calls = counts
                .lower_calls
                .checked_add(work.lowerings)
                .ok_or_else(unavailable)?;
        }
        let delta = current
            .iter()
            .map(|(path, counts)| {
                counts
                    .since(previous.counts.get(path).copied().unwrap_or_default())
                    .map(|delta| (path.clone(), delta))
                    .ok_or_else(unavailable)
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        // Front refusals are this graph's invalidations; S0's own stats and
        // touched rows are reported once, with the graph's first request.
        let (refusals, operations) = self.front.refusal_stats();
        let mut stats = previous.stats.take().unwrap_or_default();
        stats.invalidated_text = refusals
            .invalidated_text
            .checked_sub(previous.refusals.0.invalidated_text)
            .and_then(|delta| stats.invalidated_text.checked_add(delta))
            .ok_or_else(unavailable)?;
        for ((total, now), before) in stats
            .invalidated
            .iter_mut()
            .zip(refusals.invalidated)
            .zip(previous.refusals.0.invalidated)
        {
            *total = now
                .checked_sub(before)
                .and_then(|delta| total.checked_add(delta))
                .ok_or_else(unavailable)?;
        }
        self.ledger.add(&delta, Some(&stats))?;
        let operation_delta = operations
            .checked_sub(previous.refusals.1)
            .ok_or_else(unavailable)?;
        previous.refusals = (refusals, operations);
        let first = previous.initial.is_some();
        let adoptions = self.front.store_adoptions();
        let adoption_delta = adoptions
            .checked_sub(previous.store_adoptions)
            .ok_or_else(unavailable)?;
        let mut work = previous.initial.take().unwrap_or_default();
        work.memo_operations = work
            .memo_operations
            .checked_add(operation_delta)
            .ok_or_else(unavailable)?;
        if first {
            // Every front and Lower memo this graph validated, refused and
            // recomputed, or computed. Front rows the carry adopted unscanned,
            // because no change reached their reads, are not touched.
            let mut touched: std::collections::BTreeSet<MemoKey> =
                std::mem::take(&mut work.touched_memos)
                    .into_iter()
                    .collect();
            let clean = self.front.clean_memos();
            touched.extend(
                self.front
                    .memo_snapshot()
                    .into_iter()
                    .map(|(key, _)| key)
                    .filter(|key| !clean.contains(key)),
            );
            let clean_lower = self.lower.clean_memos();
            touched.extend(
                self.lower
                    .memo_keys()
                    .into_iter()
                    .filter(|key| !clean_lower.contains(key)),
            );
            work.touched_memos = touched.into_iter().collect();
        }
        work.store_adoptions = work
            .store_adoptions
            .checked_add(adoption_delta)
            .ok_or_else(unavailable)?;
        previous.store_adoptions = adoptions;
        previous.stats = None;
        previous.counts = current;
        work.hold_bytes = self.hold_bytes;
        work.recomputations = delta
            .values()
            .flat_map(|counts| counts.phases)
            .try_fold(0u64, u64::checked_add)
            .ok_or_else(unavailable)?;
        Ok(work)
    }
    pub fn inspect(&self, definitions: &[DefPath]) -> Result<Arc<CompileInspection>, ToolError> {
        let (stats, definitions) = self.ledger.definitions(definitions)?;
        let mut memos: BTreeMap<_, _> = self.memos.iter().cloned().collect();
        memos.extend(self.front.memo_snapshot());
        memos.extend(self.lower.memo_snapshot().ok_or_else(unavailable)?);
        Ok(Arc::new(CompileInspection::new(
            stats,
            memos.into_iter().collect(),
            definitions,
        )))
    }
    pub fn counts(
        &self,
        queries: &[(DefPath, QueryKind)],
    ) -> Result<Vec<(DefPath, QueryKind, u64)>, ToolError> {
        let (_, definitions) = self.ledger.definitions(
            &queries
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>(),
        )?;
        queries
            .iter()
            .zip(definitions)
            .map(|((path, kind), definition)| {
                let count = definition
                    .recomputations
                    .into_iter()
                    .find(|(selected, _)| selected == kind)
                    .map(|(_, count)| count)
                    .ok_or_else(unavailable)?;
                Ok((path.clone(), *kind, count))
            })
            .collect()
    }
}
impl Drop for GraphMeter {
    fn drop(&mut self) {
        if self.flush().is_err() {
            self.ledger
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .complete = false;
        }
    }
}
/// A runtime may outlive its host/readiness. Hold the counter meter until its
/// workers have joined; the callback never reenters or blocks producer work.
pub(super) struct MeterOwner(pub Arc<GraphMeter>);
impl RuntimeObserver for MeterOwner {
    fn record(&self, _: RuntimeEvent) {
        let _ = &self.0;
    }
}
fn unavailable() -> ToolError {
    ToolError::Runtime(
        "compiler physical-work metadata exceeds admission or is inconsistent".into(),
    )
}
