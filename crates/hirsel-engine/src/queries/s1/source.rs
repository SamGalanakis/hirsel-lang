//! Immutable session views for S1 body demands. Tables are borrowed, never
//! cloned into workers. Ordinary misses use the existing frozen P task, and
//! complete products return for deterministic serial publication.
use super::worker::{Requirement, SelectionEvent, TaskBody, TaskInputs, TaskWorker, TirSource};
use super::*;
use crate::memo::{DepthEvidence, MemoGraph};
use crate::snapshot::SourceContext;
use std::sync::Mutex;

pub(crate) struct FrozenSource<'a> {
    pub(crate) inputs: TaskInputs,
    pub(crate) items: &'a BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
    pub(crate) signatures: &'a Table<hirsel_types::SignatureRef>,
    pub(crate) modes: &'a Table<AnalyzedBody>,
    pub(crate) checked: &'a Table<TypedBody>,
    pub(crate) resolutions: &'a Table<Resolution>,
    pub(crate) lowered: &'a Table<DefinitionRef>,
    pub(crate) graph: &'a MemoGraph,
    pub(crate) declarations: Arc<Declarations>,
    pub(crate) citations: SourceContext<'a>,
    pub(crate) registry: &'a crate::registry::BuiltinRegistry,
    pub(crate) instance_bodies: &'a hirsel_tir::InstanceBodies,
    pub(crate) staged: &'a BTreeMap<DefPath, crate::comptime::StagedConstant>,
    pub(crate) manifests: &'a BTreeMap<hirsel_syntax::PackageId, ContentHash>,
    pub(crate) default_manifest: ContentHash,
    pub(crate) environment: ContentHash,
    pub(crate) signer: Option<&'a dyn SignaturePhase>,
    pub(crate) resolver: Option<&'a dyn ResolverPhase>,
    pub(crate) checker: Option<&'a dyn CheckerPhase>,
    pub(crate) mode_phase: Option<&'a dyn ModesPhase>,
    pub(crate) completed: Mutex<Vec<(parallel::CompactCompleted, ContentHash, Reads)>>,
}
/// Mirror the live generation recorder's adapter detail, including repeated
/// signature requests in order. Canonical Reads alone cannot reconstruct the
/// QueryOutput dependency list after private generator checking.
struct GeneratorRecorder<'a> {
    provider: FrozenProvider<'a>,
    snapshot: &'a SignatureSnapshot,
    detail: &'a mut DependencyReads,
}
impl SignatureProvider for GeneratorRecorder<'_> {
    fn signature(&mut self, definition: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        let result = self.provider.signature(definition);
        if let Err(QueryError::Operational(error)) = &result {
            self.detail.operational_failure = Some(error.clone());
        }
        match self.snapshot.signature(definition) {
            Some(Ok(signature)) => self.detail.signatures.push(SignatureDependency {
                definition: definition.path().clone(),
                fingerprint: signature.fingerprint,
            }),
            _ => {
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
    fn cite_name(&mut self, definition: &DefPath) -> Option<Span> {
        let cited = self.provider.cite_name(definition);
        self.detail.citations.push((definition.clone(), cited));
        cited
    }
}
impl FrozenSource<'_> {
    fn image(&self, path: &DefPath) -> Option<&crate::image::StdImage> {
        self.citations
            .image
            .filter(|image| image.package() == &path.0.package && image.declares(path))
    }
    fn text(&self, path: &DefPath) -> Option<ContentHash> {
        let body = self.implementation(path)?;
        let manifest = self
            .manifests
            .get(&path.0.package)
            .copied()
            .unwrap_or(self.default_manifest);
        Some(ContentHash::of_parts(
            "hirsel.query.text.v1",
            [body.0.as_slice(), manifest.0.as_slice()],
        ))
    }
    fn current<T>(&self, path: &DefPath, entry: &Entry<T>) -> bool {
        entry.consistent
            && !self.graph.is_dirty(&entry.memo.key)
            && self.text(path) == Some(entry.memo.text)
    }
}
impl TirSource for FrozenSource<'_> {
    fn generator_site(&self, path: &DefPath) -> Result<Span, QueryError> {
        let syntax = self
            .items
            .get(path)
            .and_then(|items| items.first())
            .ok_or(QueryError::NotImplemented)?;
        let expression = generation::generator(syntax).ok_or(QueryError::NotImplemented)?;
        Ok(syntax.tree.expressions[expression.0 as usize].origin.span)
    }
    fn prepare_generator(
        &self,
        path: &DefPath,
    ) -> DependencyProduct<generation::task::PreparedGenerator> {
        let mut observed = ReadsBuilder::new();
        let mut products = PhaseProducts::default();
        let mut detail = DependencyReads::default();
        let result = (|| {
            let syntax = self
                .items
                .get(path)
                .and_then(|items| items.first())
                .cloned()
                .ok_or(QueryError::NotImplemented)?;
            let body = generation::generator(&syntax).ok_or(QueryError::NotImplemented)?;
            let input = generation::task::GeneratorInput::new(path, &syntax, body)?;
            let (Some(signature), Some(resolver), Some(checker), Some(modes)) =
                (self.signer, self.resolver, self.checker, self.mode_phase)
            else {
                return Err(QueryError::NotImplemented);
            };
            let checked = input.check(
                self.inputs.snapshot.types(),
                &self.inputs.memo,
                &self.declarations,
                &generation::task::GeneratorPhases {
                    signature,
                    resolver,
                    checker,
                    modes,
                },
                &mut products,
                &mut GeneratorRecorder {
                    provider: FrozenProvider::for_definition(
                        &self.inputs.snapshot,
                        DefId::intern(path),
                        &self.citations,
                        &mut observed,
                    ),
                    snapshot: &self.inputs.snapshot,
                    detail: &mut detail,
                },
            );
            Ok(generation::task::PreparedGenerator {
                syntax,
                body,
                input,
                checked,
                detail: detail.clone(),
                products: Arc::new(products.clone()),
            })
        })();
        if let Some(resolution) = &products.resolution {
            detail.resolver = resolution.reads.clone();
        }
        let mut source = parallel::FrozenReadSource {
            declarations: &self.declarations,
            citations: &self.citations,
            candidates: self
                .inputs
                .snapshot
                .declaration_candidates(DefId::intern(path)),
            environment: self.environment,
        };
        match memos::complete_reads(
            path,
            &detail,
            &Observed {
                products: Arc::new(products),
                merged: observed,
                ..Default::default()
            },
            &mut source,
        ) {
            Ok(reads) => DependencyProduct { result, reads },
            Err(_) => DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            },
        }
    }

    fn inputs(&self) -> &TaskInputs {
        &self.inputs
    }
    fn implementation(&self, path: &DefPath) -> Option<ContentHash> {
        self.items
            .get(path)
            .and_then(|items| items.first())
            .map(|item| item.text_hash)
            .or_else(|| {
                self.image(path)?
                    .inputs(DefId::intern(path))
                    .map(|inputs| inputs.body)
            })
    }
    fn site(&self, path: &DefPath) -> Result<Span, QueryError> {
        self.items
            .get(path)
            .and_then(|items| items.first())
            .map(|item| item.item.name_span)
            .or_else(|| {
                self.declarations
                    .sites(path)
                    .first()
                    .map(|site| site.name_span)
            })
            .or_else(|| {
                self.registry
                    .get(DefId::intern(path))
                    .map(|_| Span::default())
            })
            .ok_or(QueryError::NotImplemented)
    }
    fn body(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
    ) -> DependencyProduct<Arc<TaskBody>> {
        // A generic generated declaration has no executable body. Its closed
        // instance must be registered before a worker checks the generator;
        // never treat its open type parameters as a private constant.
        if self
            .items
            .get(path)
            .and_then(|items| items.first())
            .is_some_and(|syntax| generation::instance_bodied(syntax))
        {
            worker.pending.push(Requirement::Generate(path.clone()));
            return DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            };
        }
        let id = DefId::intern(path);
        if let Some(entry) = self
            .modes
            .get(&id)
            .filter(|entry| self.current(path, entry))
        {
            if let Some(generated) = self
                .checked
                .get(&id)
                .filter(|entry| self.current(path, entry))
                && let Some(depth) = &generated.depth
                && let Err(error) =
                    worker.replay_generator(depth, self.generator_site(path).unwrap_or_default())
            {
                return DependencyProduct {
                    result: Err(error),
                    reads: entry.reads().clone(),
                };
            }
            if entry.result().is_err() {
                worker.phase = Some(QueryKind::Modes);
            }
            return DependencyProduct {
                result: entry.result().map(|output| {
                    Arc::new(TaskBody {
                        analyzed: output.clone(),
                        reads: entry.reads().clone(),
                        dependencies: output.dependencies.clone(),
                    })
                }),
                reads: entry.reads().clone(),
            };
        }
        let Some(syntax) = self
            .items
            .get(path)
            .and_then(|items| items.first())
            .cloned()
        else {
            worker.pending.push(Requirement::Syntax(path.clone()));
            return DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            };
        };
        if generation::generator(&syntax).is_some() {
            if let Some(entry) = self
                .checked
                .get(&id)
                .filter(|entry| self.current(path, entry))
            {
                if let Some(depth) = &entry.depth
                    && let Err(error) = worker
                        .replay_generator(depth, self.generator_site(path).unwrap_or_default())
                {
                    return DependencyProduct {
                        result: Err(error),
                        reads: entry.reads().clone(),
                    };
                }
                if let Err(error) = entry.result() {
                    worker.phase = Some(QueryKind::Check);
                    return DependencyProduct {
                        result: Err(error),
                        reads: entry.reads().clone(),
                    };
                }
            } else {
                return worker.generate_body(path);
            }
        }
        // The own header was observed during snapshot construction. Never
        // normalize/retry it in a worker after freezing the epoch.
        let signature = match self.inputs.snapshot.signature(id) {
            Some(Ok(signature)) => self
                .signatures
                .get(&id)
                .and_then(|entry| entry.result().ok())
                .or_else(|| {
                    self.image(path)?
                        .signature_entry(id)
                        .map(|entry| entry.output.clone())
                })
                .map(Ok)
                .unwrap_or_else(|| Ok(Arc::new(output(signature.clone())))),
            Some(Err(error)) => Err(error.clone()),
            None => Err(QueryError::NotImplemented),
        };
        let task = parallel::Task {
            index: 0,
            definition: path.clone(),
            syntax,
            signature,
            signature_reads: self
                .signatures
                .get(&id)
                .map(|entry| entry.memo.reads.clone())
                .unwrap_or_default(),
            resolution: self
                .resolutions
                .get(&id)
                .filter(|entry| self.current(path, entry))
                .cloned(),
            checked: self
                .checked
                .get(&id)
                .filter(|entry| self.current(path, entry))
                .cloned(),
            text: self.text(path).unwrap_or_default(),
        };
        let context = parallel::Worker {
            types: self.inputs.snapshot.types(),
            memo: &self.inputs.memo,
            snapshot: &self.inputs.snapshot,
            declarations: &self.declarations,
            citations: &self.citations,
            sources: &self.inputs.sources,
            items: self.items,
            resolver: self.resolver,
            checker: self.checker,
            modes: self.mode_phase,
        };
        let done = context.run(task);
        let candidates = self.inputs.snapshot.declaration_candidates(id);
        let reads = done.complete_reads(&mut parallel::FrozenReadSource {
            declarations: &self.declarations,
            citations: &self.citations,
            candidates,
            environment: self.environment,
        });
        let result = done.result();
        if result.is_err() {
            worker.phase = Some(done.phase);
        }
        let reads = match reads {
            Ok(reads) => reads,
            Err(error) => {
                worker.pending.push(Requirement::Receipt(error.0));
                return DependencyProduct {
                    result: Err(QueryError::NotImplemented),
                    reads: Reads::empty(),
                };
            }
        };
        let text = done.text;
        let done = parallel::CompactCompleted::capture(
            done,
            &context,
            self.inputs.snapshot.types().clone(),
            &mut parallel::FrozenReadSource {
                declarations: &self.declarations,
                citations: &self.citations,
                candidates,
                environment: self.environment,
            },
            false,
        );
        self.completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((done, text, reads.clone()));
        DependencyProduct {
            result: result.map(|output| {
                Arc::new(TaskBody {
                    analyzed: output.clone(),
                    reads: reads.clone(),
                    dependencies: output.dependencies.clone(),
                })
            }),
            reads,
        }
    }
    fn dictionary_constant(
        &self,
        path: &DefPath,
    ) -> Result<Option<hirsel_tir::Materialized>, QueryError> {
        if self.items.contains_key(path) || self.image(path).is_some() {
            return Ok(None);
        }
        self.registry
            .dictionary_value(self.inputs.snapshot.types(), &self.inputs.memo, path)
    }
    fn has_staged_declaration(&self, registration: &crate::comptime::StagedRegistration) -> bool {
        self.staged.get(&registration.path).is_some_and(|constant| {
            constant.signature.fingerprint == registration.constant.signature.fingerprint
                && constant.inputs == registration.constant.inputs
        })
    }
    fn staged_constant(&self, path: &DefPath) -> Option<crate::comptime::StagedConstant> {
        self.staged.get(path).cloned()
    }
    fn cached_constant(&self, path: &DefPath) -> Option<crate::comptime::EvaluatedConstant> {
        let constant = self
            .image(path)?
            .record(DefId::intern(path))?
            .constant
            .clone()?;
        (constant.depth.limit() == crate::memo::dependency_limit(self.inputs.limits.call_depth))
            .then_some(constant)
    }
    fn cached_lowering(
        &self,
        path: &DefPath,
    ) -> Option<(DefinitionRef, Reads, DepthEvidence<Span>)> {
        if let Some(entry) = self
            .lowered
            .get(&DefId::intern(path))
            .filter(|entry| self.current(path, entry))
            && let Ok(output) = entry.result()
            && !crate::comptime::has_staged_reads(&output.value)
        {
            let depth = entry.depth.clone().unwrap_or_else(|| {
                DepthEvidence::leaf(crate::memo::dependency_limit(self.inputs.limits.call_depth))
            });
            if depth.limit() == crate::memo::dependency_limit(self.inputs.limits.call_depth) {
                return Some((output.value.clone(), entry.reads().clone(), depth));
            }
        }
        let record = self.image(path)?.record(DefId::intern(path))?;
        let lowering = record.lowering.as_ref()?;
        if lowering.depth.limit() != crate::memo::dependency_limit(self.inputs.limits.call_depth)
            || crate::comptime::has_staged_reads(&lowering.definition)
        {
            return None;
        }
        Some((
            lowering.definition.clone(),
            lowering.execution.clone(),
            lowering.depth.clone(),
        ))
    }
    fn constant_instance(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
        arguments: &[u64],
    ) -> DependencyProduct<MaterializedRef> {
        if arguments.is_empty() {
            return DependencySource::constant(worker, path);
        }
        worker.capture(|worker| {
            worker
                .staged_at(path, arguments)
                .map(|value| Arc::new(value.value))
        })
    }
    fn instance_body(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> DependencyProduct<Option<DefPath>> {
        if self
            .instance_bodies
            .declarations
            .contains(&DefId::intern(path))
            || self
                .items
                .get(path)
                .and_then(|items| items.first())
                .is_some_and(|syntax| generation::instance_bodied(syntax))
        {
            worker.implementation(path);
        }
        match self.instance_bodies.resolve(
            DefId::intern(path),
            self.inputs.snapshot.types(),
            arguments,
        ) {
            Ok(Some(body)) => {
                // Scalar selection checks an instance before returning its
                // executable identity. Keep that order and return its full
                // reads, while deferring first-selection labels to input order.
                let product = self.body(worker, body.path());
                worker.selections.push(SelectionEvent::Checked(
                    body.path().clone(),
                    matches!(product.result, Err(QueryError::Diagnostics(_))),
                ));
                DependencyProduct {
                    result: product.result.map(|_| Some(body.path().clone())),
                    reads: product.reads,
                }
            }
            Ok(None)
                if !self
                    .items
                    .get(path)
                    .and_then(|items| items.first())
                    .is_some_and(|syntax| generation::instance_bodied(syntax)) =>
            {
                DependencyProduct {
                    result: Ok(None),
                    reads: Reads::empty(),
                }
            }
            _ => {
                worker
                    .pending
                    .push(Requirement::Instance(path.clone(), arguments.to_vec()));
                DependencyProduct {
                    result: Err(QueryError::NotImplemented),
                    reads: Reads::empty(),
                }
            }
        }
    }
}

impl QueryEngine {
    /// Freeze only handles and borrowed immutable tables. Demanded ordinary
    /// bodies use the same P worker after hold eviction; no body-table clone.
    pub(crate) fn frozen_s1_source(&mut self) -> Result<FrozenSource<'_>, QueryError> {
        self.note_environment();
        self.prepare_materialization_environment();
        let runtime = self.comptime_runtime_types()?;
        let snapshot = self.signature_inputs();
        let lowering = self
            .adapters
            .lowering
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        let declarations = self.frozen_declarations();
        let environment = self.environment_value();
        Ok(FrozenSource {
            inputs: TaskInputs {
                snapshot,
                memo: self.memo.clone(),
                lowering,
                runtime,
                sources: self.sources.clone(),
                limits: self.comptime_limits,
                verify: self.verify,
            },
            items: &self.items,
            signatures: &self.signatures,
            modes: &self.modes,
            checked: &self.checked,
            resolutions: &self.resolutions,
            lowered: &self.lowered,
            graph: &self.graph,
            declarations,
            citations: SourceContext {
                sources: &self.sources,
                items: &self.items,
                declarations: &self.declarations,
                image: self.image.as_ref().map(|adopted| &*adopted.image),
            },
            registry: &self.registry,
            instance_bodies: &self.instance_bodies,
            staged: &self.staged_constants,
            manifests: &self.package_manifests,
            default_manifest: self.environment.package_manifest_hash,
            environment,
            signer: self.adapters.signature.as_deref(),
            resolver: self.adapters.resolver.as_deref(),
            checker: self.adapters.checker.as_deref(),
            mode_phase: self.adapters.modes.as_deref(),
            completed: Mutex::new(vec![]),
        })
    }
}

impl QueryEngine {
    /// Materialize a constant frontier in parallel, returning results in input
    /// order. Ordinary roots use frozen workers; roots demanding new syntax or
    /// generated registrations are retried from their root on the serial
    /// coordinator until those source callbacks are available in workers.
    /// Every returned worker product is checked against current reads before
    /// publication, including after a preceding root changed registrations.
    pub fn materialize_values(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<Result<hirsel_tir::Materialized, QueryError>> {
        self.materialize_value_outcomes(definitions, jobs)
            .into_iter()
            .map(|outcome| outcome.result)
            .collect()
    }
    /// The same worker schedule with root-local operational provenance.
    pub fn materialize_value_outcomes(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<MaterializationResult> {
        let mut selections = BTreeMap::new();
        loop {
            if let Some(results) =
                self.materialize_values_round(&definitions, jobs, &mut selections)
            {
                if let Some(last) = results.last() {
                    self.last_phase = last.phase;
                    self.last_comptime_limit = last.limit.clone();
                    self.unavailable = matches!(last.result, Err(QueryError::NotImplemented))
                        .then_some(last.phase)
                        .flatten();
                }
                return results;
            }
        }
    }
    fn scalar_materialization_outcome(&mut self, path: &DefPath) -> MaterializationResult {
        self.last_phase = None;
        self.unavailable = None;
        self.last_comptime_limit = None;
        self.execution_stack.push(ReadsBuilder::new());
        let mut result = self.materialize_value(path);
        let reads = self
            .execution_stack
            .pop()
            .expect("root outcome read frame")
            .finish();
        let reads = match reads {
            Ok(reads) => reads,
            Err(_) => {
                result = Err(QueryError::NotImplemented);
                Reads::empty()
            }
        };
        self.merge_execution(&reads);
        let phase = if result.is_ok() {
            Some(QueryKind::Lower)
        } else {
            self.unavailable.or(self.last_phase)
        };
        MaterializationResult {
            definition: path.clone(),
            result,
            phase,
            limit: self.last_comptime_limit.clone(),
            reads: Arc::new(reads),
        }
    }
    fn materialize_values_round(
        &mut self,
        definitions: &[DefPath],
        jobs: hirsel_intern::jobs::Jobs,
        selections: &mut BTreeMap<DefPath, hirsel_diag::Label>,
    ) -> Option<Vec<MaterializationResult>> {
        if definitions.is_empty() {
            return Some(vec![]);
        }
        self.note_environment();
        self.prepare_materialization_environment();
        let mut results: Vec<Option<MaterializationResult>> = vec![None; definitions.len()];
        let mut tasks = vec![];
        for (index, definition) in definitions.iter().enumerate() {
            if let Some(value) = self.constant_values.get(definition).cloned()
                && (self.verify == Verify::Off
                    || value
                        .executed
                        .iter()
                        .any(|(key, _)| matches!(key, ReadKey::ValidationUniverse(_))))
                && self.execution_valid(definition, &value.executed)
            {
                results[index] = Some(MaterializationResult {
                    definition: definition.clone(),
                    result: Ok(value.value),
                    phase: Some(QueryKind::Lower),
                    limit: None,
                    reads: Arc::new(value.executed),
                });
            } else {
                tasks.push((index, definition.clone()));
            }
        }
        if tasks.is_empty() {
            self.last_comptime_limit = None;
            return Some(results.into_iter().map(Option::unwrap).collect());
        }
        let mut shared = worker::TaskShared::new(self.s1.coordinator().clone());
        let source = match self.frozen_s1_source() {
            Ok(source) => source,
            Err(_) => {
                return Some(
                    definitions
                        .iter()
                        .map(|path| self.scalar_materialization_outcome(path))
                        .collect(),
                );
            }
        };
        let completed = hirsel_intern::jobs::map_with_state(
            jobs,
            tasks,
            |_| 1,
            || TaskWorker::new(&source, &shared),
            |worker, (index, path)| {
                let product = worker.root(&path);
                (
                    index,
                    path,
                    product,
                    worker.limit.take(),
                    worker.phase,
                    std::mem::take(&mut worker.registrations),
                    std::mem::take(&mut worker.pending),
                    std::mem::take(&mut worker.selections),
                )
            },
        );
        let bodies = source
            .completed
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());
        let mut bodies = bodies;
        bodies.sort_by(|(left, _, _), (right, _, _)| left.definition().cmp(right.definition()));
        for (body, text, reads) in bodies {
            if self.key_text(body.definition(), QueryKind::Modes).ok() == Some(text)
                && self.execution_valid(body.definition(), &reads)
            {
                let _ = self.merge_compact_completed(body);
            }
        }
        // A declaration demand suspends evaluation before V. Install only
        // registrations whose complete producer reads still hold, then begin
        // another worker round under the new signature universe.
        let mut declarations = BTreeMap::new();
        for (_, path, product, _, _, registrations, pending, _) in &completed {
            if pending
                .iter()
                .any(|requirement| matches!(requirement, Requirement::Staged(_)))
                && self.execution_valid(path, &product.reads)
            {
                for registration in registrations {
                    if !self
                        .staged_constants
                        .get(&registration.path)
                        .is_some_and(|old| {
                            old.signature.fingerprint == registration.constant.signature.fingerprint
                                && old.inputs == registration.constant.inputs
                        })
                    {
                        declarations.insert(registration.path.clone(), registration.clone());
                    }
                }
            }
        }
        if !declarations.is_empty() {
            for registration in declarations.into_values() {
                self.install_staged(registration);
            }
            return None;
        }
        let mut instances = BTreeMap::new();
        for (_, path, product, _, _, _, pending, _) in &completed {
            if !self.execution_valid(path, &product.reads) {
                continue;
            }
            for requirement in pending {
                if let Requirement::Instance(declaration, arguments) = requirement
                    && let Some(key) = hirsel_tir::InstanceBodies::key(&self.types, arguments)
                {
                    instances
                        .entry((declaration.clone(), key))
                        .or_insert_with(|| arguments.clone());
                }
            }
        }
        let mut registered = false;
        for ((declaration, _), arguments) in instances {
            let Ok(syntax) = self.snapshot(&declaration) else {
                break;
            };
            let actuals = hirsel_types::Actuals { arguments };
            match self.register_generated_instance(&declaration, &actuals, syntax.item.name_span) {
                Ok((instance, fresh)) => {
                    if fresh {
                        let key = hirsel_tir::InstanceBodies::key(&self.types, &actuals.arguments)
                            .expect("registered closed actuals");
                        selections.insert(
                            instance,
                            hirsel_diag::Label {
                                span: syntax.item.name_span,
                                message: format!(
                                    "selects instance {}",
                                    key.instance_name(&declaration)
                                ),
                            },
                        );
                    }
                    registered |= fresh;
                }
                Err(_) => break,
            }
        }
        if registered {
            return None;
        }
        let mut generated = shared.take_generators();
        generated.sort_by(|left, right| left.definition.cmp(&right.definition));
        let mut published = false;
        for product in generated {
            if self.implementation_hash(&product.definition) == Some(product.syntax.text_hash)
                && self.execution_valid(&product.definition, &product.reads)
            {
                match self.merge_generated_text(product) {
                    Ok(()) => published = true,
                    Err(_) => break,
                }
            }
        }
        if published {
            return None;
        }
        // Successes reached inside a failed root remain independently valid.
        // Publish only after all worker requests/claims have settled.
        for (path, constant, mut registrations, staged) in shared.into_constants() {
            if self.execution_valid(&path, &constant.executed) {
                registrations.sort_by(|a, b| a.path.cmp(&b.path));
                for registration in registrations {
                    self.install_staged(registration);
                }
                match staged {
                    Some(key) => {
                        self.staged_values.insert(key, constant);
                    }
                    None => {
                        self.constant_values.insert(path, constant);
                    }
                }
            }
        }
        let mut completed: BTreeMap<_, _> = completed
            .into_iter()
            .map(
                |(index, path, product, limit, phase, registrations, pending, selected)| {
                    (
                        index,
                        (
                            path,
                            product,
                            limit,
                            phase,
                            registrations,
                            pending,
                            selected,
                        ),
                    )
                },
            )
            .collect();
        for (index, path) in definitions.iter().enumerate() {
            let Some((_, product, limit, phase, mut registrations, pending, selected)) =
                completed.remove(&index)
            else {
                // A prior root may have changed registrations since preflight.
                // Recheck the hit at its sequential position and clear a limit
                // from an earlier root exactly as scalar materialization does.
                results[index] = Some(self.scalar_materialization_outcome(path));
                continue;
            };
            if pending.is_empty() && self.execution_valid(path, &product.reads) {
                registrations.sort_by(|a, b| a.path.cmp(&b.path));
                for registration in registrations {
                    self.install_staged(registration);
                }
                self.last_comptime_limit = limit;
                results[index] = Some(MaterializationResult {
                    definition: path.clone(),
                    result: product.result,
                    phase,
                    limit: self.last_comptime_limit.clone(),
                    reads: Arc::new(product.reads),
                });
            } else {
                // No suspended failure becomes a cached language verdict. A
                // whole-root retry preserves the requester's original stack.
                results[index] = Some(self.scalar_materialization_outcome(path));
            }
            // A registration's first scalar check owns its selecting label;
            // later hits return the underlying cached diagnostics. Worker
            // completion order must not decide which root receives the label.
            let mut error = results[index]
                .as_ref()
                .unwrap()
                .result
                .as_ref()
                .err()
                .cloned();
            for event in selected {
                match event {
                    SelectionEvent::Failed(mut product) => {
                        let definition = product.definition.clone();
                        let cached = self.checked.get(&DefId::intern(&definition)).cloned();
                        let cached = cached.filter(|entry| {
                            self.key_text(&definition, QueryKind::Check)
                                .ok()
                                .is_some_and(|text| self.memo_valid(&definition, entry, text))
                        });
                        if let Some(cached) = cached {
                            error = cached.result().err();
                        } else if self.execution_valid(&definition, &product.reads) {
                            // Nested first-selection labels are part of the
                            // enclosing rejected check, not its child's memo.
                            if let Some(error) = &error {
                                product.text = Err(error.clone());
                            }
                            let _ = self.merge_generated_text(product);
                        }
                    }
                    SelectionEvent::Checked(instance, failed) => {
                        if let Some(label) = selections.remove(&instance)
                            && failed
                            && let Some(QueryError::Diagnostics(diagnostics)) = &mut error
                        {
                            for diagnostic in diagnostics {
                                diagnostic.secondary.push(label.clone());
                            }
                        }
                    }
                }
            }
            if let Some(error) = error {
                results[index].as_mut().unwrap().result = Err(error);
            }
        }
        Some(results.into_iter().map(Option::unwrap).collect())
    }
}
