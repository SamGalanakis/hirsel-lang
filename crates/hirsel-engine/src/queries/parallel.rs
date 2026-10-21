//! Frozen per-definition front work, joined before deterministic memo publication.
//! Go's compile queue (`cmd/compile/internal/gc/compile.go:166-184`) separates
//! worker-local function work from completed products. Hirsel uses jobs::map
//! and publishes by DefPath because subscriber and diagnostic order is semantic.
use super::*;
use hirsel_intern::jobs::{self, Jobs};
use hirsel_types::SignatureRef;

/// One input's outcome, including the phase that completed or stopped it.
/// Results from `mode_definitions` always retain the caller's input order.
#[derive(Clone, Debug)]
pub struct ModeResult {
    pub definition: DefPath,
    pub result: Result<Arc<QueryOutput<AnalyzedBody>>, QueryError>,
    pub phase: Option<QueryKind>,
}

struct Task {
    index: usize,
    definition: DefPath,
    syntax: Arc<ItemSyntax>,
    signature: Result<Arc<QueryOutput<SignatureRef>>, QueryError>,
    resolution: Option<Arc<Entry<Resolution>>>,
    checked: Option<Arc<Entry<TypedBody>>>,
    text: ContentHash,
}
struct Stage<T> {
    result: Result<Arc<QueryOutput<T>>, QueryError>,
    detail: DependencyReads,
    observed: Observed,
    unavailable: Option<QueryKind>,
}
impl<T> Stage<T> {
    fn complete(
        &mut self,
        mut result: Result<QueryOutput<T>, QueryError>,
        products: PhaseProducts,
    ) {
        self.observed.products = Arc::new(products);
        prepare_result(&mut self.detail, &mut result, &self.observed.products);
        self.result = result.map(Arc::new);
    }
}
enum Product<T> {
    Hit(Arc<Entry<T>>),
    Computed(Stage<T>),
}
impl<T> Product<T> {
    fn result(&self) -> Result<Arc<QueryOutput<T>>, QueryError> {
        match self {
            Self::Hit(entry) => entry.result(),
            Self::Computed(stage) => stage.result.clone(),
        }
    }
    fn detail(&self) -> &DependencyReads {
        match self {
            Self::Hit(entry) => &entry.detail,
            Self::Computed(stage) => &stage.detail,
        }
    }
}
struct Completed {
    index: usize,
    definition: DefPath,
    text: ContentHash,
    resolution: Option<Product<Resolution>>,
    checked: Option<Product<TypedBody>>,
    modes: Option<Product<AnalyzedBody>>,
    result: Result<Arc<QueryOutput<AnalyzedBody>>, QueryError>,
    phase: QueryKind,
}

/// Only immutable phase handles and inputs cross the worker boundary. In
/// particular this has neither QueryEngine nor an interpreter behind a lock.
struct Worker<'a> {
    types: &'a hirsel_types::TypeInterner,
    memo: &'a hirsel_types::TypeMemo,
    snapshot: &'a SignatureSnapshot,
    declarations: &'a Declarations,
    citations: &'a dyn CitationSource,
    sources: &'a SourceMap,
    items: &'a BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    resolver: Option<&'a dyn ResolverPhase>,
    checker: Option<&'a dyn CheckerPhase>,
    modes: Option<&'a dyn ModesPhase>,
}
impl Worker<'_> {
    fn run(&self, task: Task) -> Completed {
        let mut done = Completed {
            index: task.index,
            definition: task.definition.clone(),
            text: task.text,
            resolution: None,
            checked: None,
            modes: None,
            result: Err(QueryError::NotImplemented),
            phase: QueryKind::Resolve,
        };
        let owner = DefId::intern(&task.definition);
        // A checked memo already includes the complete resolution reads. A
        // partial hit needs no resolver work, but remains the same task path.
        let checked = if let Some(entry) = task.checked {
            Product::Hit(entry)
        } else {
            let resolved = if let Some(entry) = task.resolution {
                Product::Hit(entry)
            } else {
                let _timing = timings::phase(Phase::Resolve);
                let mut stage = Stage {
                    result: Err(QueryError::NotImplemented),
                    detail: DependencyReads::default(),
                    observed: Observed::default(),
                    unavailable: None,
                };
                let mut products = PhaseProducts::default();
                let mut provider = WorkerRecorder::new(self, owner, &mut stage);
                let result = match self.resolver {
                    Some(adapter) => adapter.resolve(
                        self.types,
                        self.memo,
                        task.syntax.clone(),
                        self.declarations,
                        &mut products,
                        &mut provider,
                    ),
                    None => Err(QueryError::NotImplemented),
                };
                stage.complete(result, products);
                Product::Computed(stage)
            };
            let resolution = resolved.result();
            let unavailable = match &resolved {
                Product::Computed(stage) => stage.unavailable,
                _ => None,
            };
            let mut initial = resolved.detail().clone();
            done.resolution = Some(resolved);
            let resolution = match resolution {
                Ok(value) => value,
                Err(error) => {
                    if matches!(error, QueryError::NotImplemented) {
                        done.phase = unavailable.unwrap_or(done.phase);
                    }
                    done.result = Err(error);
                    return done;
                }
            };
            done.phase = QueryKind::Signature;
            let signature = match &task.signature {
                Ok(value) => value,
                Err(error) => {
                    done.result = Err(error.clone());
                    return done;
                }
            };
            done.phase = QueryKind::Check;
            initial.signatures.extend(resolution.dependencies.clone());
            initial.signatures.extend(signature.dependencies.clone());
            initial.signatures.push(SignatureDependency {
                definition: task.definition.clone(),
                fingerprint: signature.value.fingerprint,
            });
            let _timing = timings::phase(Phase::Check);
            timings::count(Count::Definitions);
            let mut stage = Stage {
                result: Err(QueryError::NotImplemented),
                detail: initial,
                observed: Observed::default(),
                unavailable: None,
            };
            let mut provider = WorkerRecorder::new(self, owner, &mut stage);
            let mut result = match self.checker {
                Some(adapter) => adapter.check(
                    self.types,
                    self.memo,
                    task.syntax,
                    resolution.value.clone(),
                    &signature.value,
                    &mut provider,
                ),
                None => Err(QueryError::NotImplemented),
            };
            if let Ok(output) = &mut result {
                arm_fixes::place(
                    output,
                    &mut stage.observed.representations,
                    self.sources,
                    self.items,
                );
            }
            stage.complete(result, PhaseProducts::default());
            Product::Computed(stage)
        };
        done.phase = QueryKind::Check;
        let body = checked.result();
        let unavailable = match &checked {
            Product::Computed(stage) => stage.unavailable,
            _ => None,
        };
        let initial = checked.detail().clone();
        done.checked = Some(checked);
        let body = match body {
            Ok(value) => value,
            Err(error) => {
                if matches!(error, QueryError::NotImplemented) {
                    done.phase = unavailable.unwrap_or(done.phase);
                }
                done.result = Err(error);
                return done;
            }
        };
        done.phase = QueryKind::Signature;
        let signature = match task.signature {
            Ok(value) => value,
            Err(error) => {
                done.result = Err(error);
                return done;
            }
        };
        done.phase = QueryKind::Modes;
        let _timing = timings::phase(Phase::Modes);
        let mut stage = Stage {
            result: Err(QueryError::NotImplemented),
            detail: initial,
            observed: Observed::default(),
            unavailable: None,
        };
        let mut products = PhaseProducts::default();
        let mut provider = WorkerRecorder::new(self, owner, &mut stage);
        let result = match self.modes {
            Some(adapter) => adapter.check_modes(
                self.types,
                self.memo,
                Arc::new(body.value.clone()),
                &signature.value,
                &mut products,
                &mut provider,
            ),
            None => Err(QueryError::NotImplemented),
        };
        stage.complete(result, products);
        if matches!(stage.result, Err(QueryError::NotImplemented)) {
            done.phase = stage.unavailable.unwrap_or(done.phase);
        }
        let product = Product::Computed(stage);
        done.modes = Some(product);
        done
    }
}

/// Preserve adapter-level detail as well as the provider's canonical typed
/// reads. The latter is authoritative for memo verification; tooling and
/// semantic closure also consume the former.
struct WorkerRecorder<'a> {
    provider: FrozenProvider<'a>,
    snapshot: &'a SignatureSnapshot,
    detail: &'a mut DependencyReads,
    unavailable: &'a mut Option<QueryKind>,
}
impl<'a> WorkerRecorder<'a> {
    fn new<T>(worker: &'a Worker<'_>, owner: DefId, stage: &'a mut Stage<T>) -> Self {
        Self {
            provider: FrozenProvider::for_definition(
                worker.snapshot,
                owner,
                worker.citations,
                &mut stage.observed.merged,
            ),
            snapshot: worker.snapshot,
            detail: &mut stage.detail,
            unavailable: &mut stage.unavailable,
        }
    }
}
impl SignatureProvider for WorkerRecorder<'_> {
    fn signature(&mut self, definition: DefId) -> Result<SignatureRef, QueryError> {
        let result = self.provider.signature(definition);
        if matches!(result, Err(QueryError::NotImplemented))
            && self.snapshot.signature(definition).is_some()
        {
            *self.unavailable = Some(QueryKind::Signature);
        }
        if let Err(QueryError::Operational(error)) = &result {
            self.detail.operational_failure = Some(error.clone());
        }
        match &result {
            Ok(signature) => self.detail.signatures.push(SignatureDependency {
                definition: definition.path().clone(),
                fingerprint: signature.fingerprint,
            }),
            Err(_) => {
                self.detail
                    .failed_signatures
                    .push(definition.path().clone());
                self.detail.failed_signature_keys.push((
                    definition.path().clone(),
                    self.snapshot.failed_signature_key(definition),
                ));
            }
        }
        result
    }
    fn cite_name(&mut self, definition: &DefPath) -> Option<Span> {
        let cited = self.provider.cite_name(definition);
        self.detail.citations.push((definition.clone(), cited));
        cited
    }
    fn lookup(
        &mut self,
        context: &FileContext,
        path: &hirsel_syntax::Path,
    ) -> hirsel_types::LookupOutput {
        let output = self.provider.lookup(context, path);
        self.detail
            .manifest_lookups
            .push(hirsel_types::DependencyRead::ProviderLookup {
                context: context.clone(),
                package: path.package.clone(),
                segments: path
                    .segments
                    .iter()
                    .map(|s| s.as_str().to_owned())
                    .collect(),
                output: output.clone(),
            });
        if let Err(QueryError::Operational(error)) = &output.result {
            self.detail.operational_failure = Some(error.clone());
        }
        if let Some(path) = lookup_path(context, path) {
            self.detail.lookups.push((path, output.result.is_ok()));
        }
        output
    }
}

impl QueryEngine {
    /// Fused resolve/check/modes work for one frontier. Every worker count uses
    /// jobs::map; only preparation and memo publication mutate the engine.
    /// Declarations that execute generators keep their ordinary serial S1 path.
    pub fn mode_definitions(&mut self, definitions: Vec<DefPath>, jobs: Jobs) -> Vec<ModeResult> {
        let mut outputs = Vec::with_capacity(definitions.len());
        let mut frontier = Vec::new();
        for definition in definitions {
            self.last_phase = None;
            self.unavailable = None;
            match self
                .materialize(&definition)
                .and_then(|_| self.snapshot(&definition))
            {
                Ok(syntax) if generation::generator(&syntax).is_none() => frontier.push(definition),
                state => {
                    let materialization_phase = self.unavailable.or(self.last_phase);
                    let materialization_failed = state.is_err();
                    // Generators can change source/signature inputs. Publish
                    // the preceding frontier before serial S1 work, then build
                    // a fresh immutable context for the following frontier.
                    outputs
                        .extend(self.mode_source_definitions(std::mem::take(&mut frontier), jobs));
                    self.last_phase = None;
                    self.unavailable = None;
                    let result = state.and_then(|_| self.mode_query(&definition));
                    let phase = if materialization_failed {
                        materialization_phase
                    } else if result.is_ok() {
                        Some(QueryKind::Modes)
                    } else {
                        self.unavailable.or(self.last_phase)
                    };
                    outputs.push(ModeResult {
                        definition,
                        result,
                        phase,
                    });
                }
            }
        }
        outputs.extend(self.mode_source_definitions(frontier, jobs));
        if let Some(last) = outputs.last() {
            self.last_phase = last.phase;
            self.unavailable = if matches!(last.result, Err(QueryError::NotImplemented)) {
                last.phase
            } else {
                None
            };
        }
        outputs
    }
    fn mode_source_definitions(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: Jobs,
    ) -> Vec<ModeResult> {
        let mut outputs: Vec<Option<ModeResult>> = (0..definitions.len()).map(|_| None).collect();
        let mut sources = Vec::new();
        let mut first = BTreeMap::new();
        let mut duplicates = Vec::new();
        for (index, definition) in definitions.into_iter().enumerate() {
            if let Some(&previous) = first.get(&definition) {
                duplicates.push((index, previous));
                continue;
            }
            first.insert(definition.clone(), index);
            self.last_phase = None;
            self.unavailable = None;
            let syntax = self
                .materialize(&definition)
                .and_then(|_| self.snapshot(&definition));
            match syntax {
                Ok(syntax) if generation::generator(&syntax).is_none() => {
                    sources.push((index, definition, syntax))
                }
                Ok(_) => {
                    let result = self.mode_query(&definition);
                    outputs[index] = Some(ModeResult {
                        definition,
                        result,
                        phase: self.unavailable.or(self.last_phase),
                    });
                }
                Err(error) => {
                    outputs[index] = Some(ModeResult {
                        definition,
                        result: Err(error),
                        phase: self.unavailable.or(self.last_phase),
                    })
                }
            }
        }
        if sources.is_empty() {
            for (index, previous) in duplicates {
                outputs[index] = outputs[previous].clone();
            }
            return outputs
                .into_iter()
                .map(|result| result.expect("one outcome per input"))
                .collect();
        }
        let mut tasks = Vec::new();
        for (index, definition, syntax) in sources {
            let id = DefId::intern(&definition);
            let text = match self.key_text(&definition, QueryKind::Modes) {
                Ok(text) => text,
                Err(error) => {
                    outputs[index] = Some(ModeResult {
                        definition,
                        result: Err(error),
                        phase: None,
                    });
                    continue;
                }
            };
            if let Some(entry) = self.modes.get(&id).cloned()
                && self.memo_valid(&definition, &entry, text)
            {
                outputs[index] = Some(ModeResult {
                    definition,
                    result: self.mode_result(entry),
                    phase: Some(QueryKind::Modes),
                });
                continue;
            }
            if let Some(entry) = self.load_mode(&definition) {
                self.modes.insert(id, entry.clone());
                outputs[index] = Some(ModeResult {
                    definition,
                    result: self.mode_result(entry),
                    phase: Some(QueryKind::Modes),
                });
                continue;
            }
            self.syntax_views(&definition)
                .expect("preflight materialized the syntax");
            let checked = self
                .checked
                .get(&id)
                .cloned()
                .filter(|entry| self.memo_valid(&definition, entry, text));
            let resolution = if checked.is_none() {
                self.resolutions
                    .get(&id)
                    .cloned()
                    .filter(|entry| self.memo_valid(&definition, entry, text))
            } else {
                None
            };
            tasks.push(Task {
                index,
                definition,
                syntax,
                signature: Err(QueryError::NotImplemented),
                resolution,
                checked,
                text,
            });
        }
        // Validation of dirty predecessors may normalize a signature. Freeze
        // only afterwards, and never retry a failed header after freezing: a
        // transient provider failure must not mix two universes in one task.
        let saved_phase = (self.last_phase, self.unavailable);
        let snapshot = self.signature_snapshot();
        let declarations = self.frozen_declarations();
        (self.last_phase, self.unavailable) = saved_phase;
        for task in &mut tasks {
            let id = DefId::intern(&task.definition);
            task.signature = match snapshot.signature(id) {
                Some(Ok(signature)) => self
                    .signatures
                    .get(&id)
                    .and_then(|entry| entry.result().ok())
                    .or_else(|| {
                        self.image_signature(&task.definition)
                            .map(|image| image.output.clone())
                    })
                    .map(Ok)
                    .unwrap_or_else(|| Ok(Arc::new(output(signature.clone())))),
                Some(Err(error)) => Err(error.clone()),
                None => Err(QueryError::NotImplemented),
            };
        }
        let completed = {
            let citations = self.citation_source();
            let worker = Worker {
                types: &self.types,
                memo: &self.memo,
                snapshot: &snapshot,
                declarations: &declarations,
                citations: &citations,
                sources: &self.sources,
                items: &self.items,
                resolver: self.adapters.resolver.as_deref(),
                checker: self.adapters.checker.as_deref(),
                modes: self.adapters.modes.as_deref(),
            };
            jobs::map(
                jobs,
                tasks,
                |task| task.syntax.tree.expressions.len() as u64,
                |task| worker.run(task),
            )
        };
        let mut completed = completed;
        completed.sort_by(|a, b| a.definition.cmp(&b.definition).then(a.index.cmp(&b.index)));
        for done in completed {
            let mut inherited = None;
            if let Some(product) = done.resolution {
                let entry = self.merge_stage(
                    &done.definition,
                    QueryKind::Resolve,
                    done.text,
                    product,
                    None,
                );
                inherited = Some(entry.memo.reads.clone());
                keep(
                    &mut self.resolutions,
                    &mut self.graph,
                    entry,
                    self.s1.failure_is_own(),
                );
            }
            if let Some(product) = done.checked {
                let entry = self.merge_stage(
                    &done.definition,
                    QueryKind::Check,
                    done.text,
                    product,
                    inherited.as_deref(),
                );
                inherited = Some(entry.memo.reads.clone());
                keep(
                    &mut self.checked,
                    &mut self.graph,
                    entry,
                    self.s1.failure_is_own(),
                );
            }
            let result = if let Some(product) = done.modes {
                let entry = self.merge_stage(
                    &done.definition,
                    QueryKind::Modes,
                    done.text,
                    product,
                    inherited.as_deref(),
                );
                let result = self
                    .store_mode(&done.definition, &entry)
                    .and_then(|_| self.mode_result(entry.clone()));
                if matches!(result, Err(QueryError::Operational(_))) {
                    self.forget(&MemoKey::new(
                        QueryKind::Modes,
                        DefId::intern(&done.definition),
                    ));
                } else {
                    keep(
                        &mut self.modes,
                        &mut self.graph,
                        entry,
                        self.s1.failure_is_own(),
                    );
                }
                result
            } else {
                done.result
            };
            outputs[done.index] = Some(ModeResult {
                definition: done.definition,
                result,
                phase: Some(done.phase),
            });
        }
        for (index, previous) in duplicates {
            outputs[index] = outputs[previous].clone();
        }
        let outputs: Vec<_> = outputs
            .into_iter()
            .map(|result| result.expect("one outcome per input"))
            .collect();
        if let Some(last) = outputs.last() {
            self.last_phase = last.phase;
            self.unavailable = matches!(last.result, Err(QueryError::NotImplemented))
                .then_some(last.phase)
                .flatten();
        }
        outputs
    }
    fn merge_stage<T>(
        &mut self,
        definition: &DefPath,
        kind: QueryKind,
        text: ContentHash,
        product: Product<T>,
        inherited: Option<&Reads>,
    ) -> Arc<Entry<T>> {
        match product {
            Product::Hit(entry) => entry,
            Product::Computed(mut stage) => {
                // Front work cannot execute/reenter the engine. Claim the phase
                // only while publishing so ordinary counters and active-state
                // bookkeeping are shared with the serial query path.
                self.start(definition, kind)
                    .expect("front workers cannot reenter queries");
                if let Some(reads) = inherited {
                    stage.observed.merged.merge(reads);
                }
                Arc::new(self.finish_prepared(
                    MemoKey::new(kind, DefId::intern(definition)),
                    text,
                    stage.detail,
                    stage.result,
                    stage.observed,
                ))
            }
        }
    }
}
