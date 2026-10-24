//! Body-free completion at the worker boundary. Like Go's function queue,
//! the worker releases its working function before accepting another task.
//! Immutable receipts and summaries survive until canonical serial publication.
use super::*;

#[derive(Clone)]
pub(super) enum FrontResult {
    Resident(ModeResult),
    Compact(PublishedResult),
}
impl FrontResult {
    pub(super) fn phase(&self) -> Option<QueryKind> {
        match self {
            Self::Resident(o) => o.phase,
            Self::Compact(o) => o.phase,
        }
    }
    pub(super) fn unavailable(&self) -> bool {
        match self {
            Self::Resident(o) => matches!(o.result, Err(QueryError::NotImplemented)),
            Self::Compact(o) => matches!(o.result, Err(QueryError::NotImplemented)),
        }
    }
}
pub(super) enum WorkerCompleted {
    Resident(Completed),
    Compact(CompactCompleted),
}
impl WorkerCompleted {
    pub(super) fn definition(&self) -> &DefPath {
        match self {
            Self::Resident(o) => &o.definition,
            Self::Compact(o) => o.definition(),
        }
    }
    pub(super) fn index(&self) -> usize {
        match self {
            Self::Resident(o) => o.index,
            Self::Compact(o) => o.index,
        }
    }
}
pub(super) fn missing_summary() -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "definition publication".into(),
        message: "completed front result has no coherent summary".into(),
    })
}
fn inconsistent() -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "front receipt capture".into(),
        message: "completed phase reads disagree".into(),
    })
}

struct CompactStage {
    computed: bool,
    error: Option<QueryError>,
    reads: Arc<Reads>,
    detail: DependencyReads,
    products: Arc<PhaseProducts>,
    consistent: bool,
}
impl CompactStage {
    fn capture<T>(
        owner: &DefPath,
        product: Product<T>,
        inherited: &Reads,
        source: &mut impl memos::ReadSource,
    ) -> Self {
        let (computed, result, detail, products, reads) = match product {
            Product::Hit(entry) => (
                false,
                entry.result(),
                entry.detail.clone(),
                entry.products.clone(),
                Ok(entry.memo.reads.clone()),
            ),
            Product::Computed(mut stage) => {
                stage.observed.merged.merge(inherited);
                let reads = memos::complete_reads(owner, &stage.detail, &stage.observed, source)
                    .map(Arc::new);
                (
                    true,
                    stage.result,
                    stage.detail,
                    stage.observed.products,
                    reads,
                )
            }
        };
        let consistent = reads.is_ok();
        Self {
            computed,
            error: result.err(),
            reads: reads.unwrap_or_else(|_| Arc::new(Reads::empty())),
            detail,
            products,
            consistent,
        }
    }
}

pub(crate) struct CompactCompleted {
    pub(super) index: usize,
    text: ContentHash,
    resolution: Option<Product<Resolution>>,
    checked: Option<CompactStage>,
    modes: Option<CompactStage>,
    checked_summary: Option<CheckedSummary>,
    dead_arms: Option<crate::tools::DeadArmUse>,
    lowering_inputs: Option<lowering::LoweringUse>,
    payload: Option<Vec<u8>>,
    store_write: Option<Result<(), QueryError>>,
    outcome: PublishedResult,
}
impl CompactCompleted {
    /// Drain the encoded bytes before this worker accepts another task. Only
    /// the provider verdict returns to canonical serial memo publication.
    pub(super) fn write_payload(&mut self, writer: Option<&persistent::EncodedWriter>) {
        if let Some(payload) = self.payload.take()
            && let Some(writer) = writer
            && let Some(stage) = &self.modes
        {
            self.store_write = Some(writer.write(&stage.reads, payload));
        }
    }

    pub(crate) fn definition(&self) -> &DefPath {
        &self.outcome.definition
    }

    pub(crate) fn capture(
        done: Completed,
        worker: &Worker<'_>,
        types: Arc<hirsel_types::TypeInterner>,
        source: &mut impl memos::ReadSource,
        records: Option<&persistent::TypeRecords>,
    ) -> Self {
        let result = done.result();
        let diagnostics = match &result {
            Ok(output) => output.diagnostics.clone(),
            Err(QueryError::Diagnostics(ds)) => ds.clone(),
            Err(_) => vec![],
        };
        let checked_output = done.checked.as_ref().and_then(|p| p.result().ok());
        let checked_summary = checked_output.as_ref().map(|output| {
            let (summary, growth) = summarize(worker.types, &output.value);
            CheckedSummary { summary, growth }
        });
        let dead_arms = checked_output.as_ref().map(|output| {
            dead_arms::body_use(
                worker.types,
                worker
                    .snapshot
                    .signature(DefId::intern(&done.definition))
                    .and_then(Result::ok)
                    .map(|signature| &**signature),
                &output.value,
            )
        });
        let lowering_inputs = result
            .as_ref()
            .ok()
            .map(|output| lowering::LoweringUse::capture(&output.value.typed));
        let diagnostic_phases: Vec<_> = diagnostics
            .iter()
            .map(|d| {
                let mode = done.modes.as_ref().and_then(|p| {
                    let products = match p {
                        Product::Hit(entry) => &entry.products,
                        Product::Computed(stage) => &stage.observed.products,
                    };
                    products
                        .mode_diagnostics
                        .iter()
                        .find(|(other, _)| other.rule == d.rule && other.primary == d.primary)
                        .map(|(_, phase)| phase.clone())
                });
                fn contains<T>(p: &Product<T>, d: &Diagnostic) -> bool {
                    let result = p.result();
                    let ds = match &result {
                        Ok(output) => &output.diagnostics,
                        Err(QueryError::Diagnostics(ds)) => ds,
                        Err(_) => return false,
                    };
                    ds.iter()
                        .any(|other| other.rule == d.rule && other.primary == d.primary)
                }
                let phase = mode.unwrap_or_else(|| {
                    if done.resolution.as_ref().is_some_and(|p| contains(p, d)) {
                        "name resolution".into()
                    } else if done.checked.as_ref().is_some_and(|p| contains(p, d)) {
                        "body checking".into()
                    } else {
                        done.phase.phase_name().into()
                    }
                });
                (d.clone(), phase)
            })
            .collect();
        // Off stores stop before Body::capture or canonical type encoding.
        let payload = if let Some(records) = records {
            done.modes.as_ref().and_then(|mode| {
                persistent::capture_mode(
                    records,
                    types,
                    worker.items,
                    &result,
                    checked_output
                        .as_ref()
                        .zip(done.checked.as_ref())
                        .map(|(output, checked)| (&**output, checked.detail())),
                    diagnostic_phases.clone(),
                    mode.detail().clone(),
                )
            })
        } else {
            None
        };
        let mut inherited = Arc::new(Reads::empty());
        let mut consistent = true;
        if let Some(product) = &done.resolution {
            match product.complete_reads(&done.definition, &inherited, source) {
                Ok(reads) => inherited = Arc::new(reads),
                Err(_) => consistent = false,
            }
        }
        let checked = done.checked.map(|product| {
            let mut stage = CompactStage::capture(&done.definition, product, &inherited, source);
            // An inconsistent predecessor cannot grant a coherent descendant.
            stage.consistent &= consistent;
            inherited = stage.reads.clone();
            consistent &= stage.consistent;
            stage
        });
        let modes = done.modes.map(|product| {
            let mut stage = CompactStage::capture(&done.definition, product, &inherited, source);
            // An inconsistent predecessor cannot grant a coherent descendant.
            stage.consistent &= consistent;
            inherited = stage.reads.clone();
            consistent &= stage.consistent;
            stage
        });
        if done.phase == QueryKind::Signature {
            match inherited.union(&done.signature_reads) {
                Ok(reads) => inherited = Arc::new(reads),
                Err(_) => consistent = false,
            }
        }
        let result = if consistent {
            result.map(|output| {
                let (summary, growth) = summarize(worker.types, &output.value.typed);
                PublishedDefinition {
                    summary: Arc::new(PublishedSummary {
                        definition: DefId::intern(&done.definition),
                        summary,
                        growth,
                        storage: None,
                        checked: checked_summary.clone(),
                        reads: inherited.clone(),
                        text: done.text,
                    }),
                    diagnostics,
                }
            })
        } else {
            Err(inconsistent())
        };
        Self {
            index: done.index,
            text: done.text,
            resolution: done.resolution,
            checked,
            modes,
            checked_summary,
            dead_arms,
            lowering_inputs,
            payload: consistent.then_some(payload).flatten(),
            store_write: None,
            outcome: PublishedResult {
                definition: done.definition,
                result,
                phase: Some(done.phase),
                reads: inherited,
                diagnostic_phases,
            },
        }
    }
}

impl QueryEngine {
    pub(crate) fn merge_compact_completed(&mut self, done: CompactCompleted) -> PublishedResult {
        // Retire older aggregate products before installing the task's fresh
        // lint/lowering inputs. Standalone query users need no aggregate.
        let changes = self
            .front_changes
            .is_active()
            .then(|| self.invalidate_front_definition(DefId::intern(done.definition())));
        let CompactCompleted {
            text,
            resolution,
            checked,
            modes,
            checked_summary,
            dead_arms,
            lowering_inputs,
            payload,
            store_write,
            mut outcome,
            ..
        } = done;
        debug_assert!(
            payload.is_none(),
            "worker payload must be drained before publication"
        );
        let path = &outcome.definition;
        let id = DefId::intern(path);
        if let Some(product) = resolution {
            let entry = self.merge_stage(path, QueryKind::Resolve, text, product, None);
            keep(
                &mut self.resolutions,
                &mut self.graph,
                entry,
                self.s1.failure_is_own(),
            );
        }
        if let Some(stage) = checked {
            if stage.consistent
                && let Some(summary) = checked_summary
            {
                if let Some(inputs) = dead_arms {
                    self.front_products.dead_arms.insert(id, inputs);
                }
                self.start(path, QueryKind::CheckedSummary)
                    .expect("front workers cannot reenter queries");
                self.record_checked_summary(
                    id,
                    summary,
                    text,
                    stage.reads.clone(),
                    stage.detail.clone(),
                    false,
                );
                self.active
                    .remove(&(path.clone(), QueryKind::CheckedSummary));
            }
            if let Some(entry) =
                self.merge_compact_stage::<TypedBody>(path, QueryKind::Check, text, stage)
            {
                keep(
                    &mut self.checked,
                    &mut self.graph,
                    entry,
                    self.s1.failure_is_own(),
                );
            }
        }
        if let Some(stage) = modes {
            let mut store_failed = false;
            if let Some(write) = store_write {
                self.finish_mode_write(path);
                if let Err(error) = write {
                    outcome.result = Err(error);
                    store_failed = true;
                }
            }
            let detail = stage.detail.clone();
            if let Some(entry) =
                self.merge_compact_stage::<AnalyzedBody>(path, QueryKind::Modes, text, stage)
            {
                keep(
                    &mut self.modes,
                    &mut self.graph,
                    entry,
                    self.s1.failure_is_own(),
                );
            }
            if store_failed {
                self.forget(&MemoKey::new(QueryKind::Modes, id));
            }
            if let Ok(published) = &outcome.result {
                self.start(path, QueryKind::Summary)
                    .expect("front workers cannot reenter queries");
                self.record_summary_products(
                    published.summary.clone(),
                    detail,
                    false,
                    Arc::new(PhaseProducts {
                        mode_diagnostics: outcome.diagnostic_phases.clone(),
                        ..Default::default()
                    }),
                );
                if let Some(inputs) = lowering_inputs {
                    self.front_products.lowering_inputs.insert(id, inputs);
                }
                self.active.remove(&(path.clone(), QueryKind::Summary));
            }
        }
        let representations: Vec<_> = outcome
            .reads
            .iter()
            .filter_map(|(key, _)| {
                if let ReadKey::Representation(id) = key {
                    Some(id.path().clone())
                } else {
                    None
                }
            })
            .collect();
        for path in representations {
            self.read_layout_body(&path);
        }
        if outcome.result.is_ok()
            && self
                .resolutions
                .get(&id)
                .is_some_and(|entry| entry.memo.value.is_ok())
        {
            self.forget(&MemoKey::new(QueryKind::Resolve, id));
        }
        self.last_phase = outcome.phase;
        self.unavailable = matches!(outcome.result, Err(QueryError::NotImplemented))
            .then_some(outcome.phase)
            .flatten();
        if let Some(changes) = changes {
            self.retain_front_completion(&outcome, &changes);
        }
        if let Err(error) = self.store_publication(&outcome) {
            outcome.result = Err(error);
        }
        outcome
    }

    /// Successes charge actual phase work, then retire their body key. Compact
    /// summary keys own the surviving subscriptions. Failures retain the same
    /// typed error entry, products and cacheability rules as scalar publication.
    fn merge_compact_stage<T>(
        &mut self,
        path: &DefPath,
        kind: QueryKind,
        text: ContentHash,
        stage: CompactStage,
    ) -> Option<Arc<Entry<T>>> {
        let key = MemoKey::new(kind, DefId::intern(path));
        if !stage.computed {
            if stage.error.is_none() {
                self.forget(&key);
            }
            return None;
        }
        self.start(path, kind)
            .expect("front workers cannot reenter queries");
        self.active.remove(&(path.clone(), kind));
        let durability = stage.reads.durability(|read| self.durability_of(read));
        if let Some(error) = stage.error {
            if kind == QueryKind::Check {
                self.retire_after_check_rejection(key.def);
            }
            let memo = self
                .graph
                .insert(key, text, Err(error), stage.reads, durability, false);
            Some(Arc::new(Entry {
                memo,
                detail: stage.detail,
                products: stage.products,
                covered: vec![],
                depth: None,
                acceptance: None,
                verification: Default::default(),
                selections: vec![],
                consistent: stage.consistent,
            }))
        } else {
            self.graph
                .insert(key, text, Ok(Arc::new(())), stage.reads, durability, false);
            self.forget(&key);
            None
        }
    }
}
