//! Immutable session views for S1 body demands. Tables are borrowed, never
//! cloned into workers. Ordinary misses use the existing frozen P task, and
//! complete products return for deterministic serial publication.
use super::worker::{Requirement, TaskBody, TaskInputs, TaskWorker, TirSource};
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
    pub(crate) manifests: &'a BTreeMap<hirsel_syntax::PackageId, ContentHash>,
    pub(crate) default_manifest: ContentHash,
    pub(crate) environment: ContentHash,
    pub(crate) resolver: Option<&'a dyn ResolverPhase>,
    pub(crate) checker: Option<&'a dyn CheckerPhase>,
    pub(crate) mode_phase: Option<&'a dyn ModesPhase>,
    pub(crate) completed: Mutex<Vec<(parallel::Completed, Reads)>>,
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
        let id = DefId::intern(path);
        if let Some(entry) = self
            .modes
            .get(&id)
            .filter(|entry| self.current(path, entry))
        {
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
            worker.pending.push(Requirement::Generate(path.clone()));
            return DependencyProduct {
                result: Err(QueryError::NotImplemented),
                reads: Reads::empty(),
            };
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
        self.completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((done, reads.clone()));
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
    fn supports_staged_declarations(&self) -> bool {
        false
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
        worker.pending.push(Requirement::ConstantInstance(
            path.clone(),
            arguments.to_vec(),
        ));
        DependencyProduct {
            result: Err(QueryError::NotImplemented),
            reads: Reads::empty(),
        }
    }
    fn instance_body(
        &self,
        worker: &mut TaskWorker<'_, Self>,
        path: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> DependencyProduct<Option<DefPath>> {
        match self.instance_bodies.resolve(
            DefId::intern(path),
            self.inputs.snapshot.types(),
            arguments,
        ) {
            Ok(body) => DependencyProduct {
                result: Ok(body.map(|definition| definition.path().clone())),
                reads: Reads::empty(),
            },
            Err(_) => {
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
            manifests: &self.package_manifests,
            default_manifest: self.environment.package_manifest_hash,
            environment,
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
        if definitions.is_empty() {
            return vec![];
        }
        self.note_environment();
        self.prepare_materialization_environment();
        let mut results: Vec<Option<Result<hirsel_tir::Materialized, QueryError>>> =
            vec![None; definitions.len()];
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
                results[index] = Some(Ok(value.value));
            } else {
                tasks.push((index, definition.clone()));
            }
        }
        if tasks.is_empty() {
            self.last_comptime_limit = None;
            return results.into_iter().map(Option::unwrap).collect();
        }
        let shared = worker::TaskShared::new(self.s1.coordinator().clone());
        let source = match self.frozen_s1_source() {
            Ok(source) => source,
            Err(_) => {
                return definitions
                    .iter()
                    .map(|path| self.materialize_value(path))
                    .collect();
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
                    std::mem::take(&mut worker.registrations),
                    std::mem::take(&mut worker.pending),
                )
            },
        );
        let bodies = source
            .completed
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());
        let mut bodies = bodies;
        bodies.sort_by(|(left, _), (right, _)| left.definition.cmp(&right.definition));
        for (body, reads) in bodies {
            if self.key_text(&body.definition, QueryKind::Modes).ok() == Some(body.text)
                && self.execution_valid(&body.definition, &reads)
            {
                let _ = self.merge_completed(body);
            }
        }
        // Successes reached inside a failed root remain independently valid.
        // Publish only after all worker requests/claims have settled.
        for (path, constant, mut registrations) in shared.into_constants() {
            if self.execution_valid(&path, &constant.executed) {
                registrations.sort_by(|a, b| a.path.cmp(&b.path));
                for registration in registrations {
                    self.install_staged(registration);
                }
                self.constant_values.insert(path, constant);
            }
        }
        let mut completed: BTreeMap<_, _> = completed
            .into_iter()
            .map(|(index, path, product, limit, registrations, pending)| {
                (index, (path, product, limit, registrations, pending))
            })
            .collect();
        for (index, path) in definitions.iter().enumerate() {
            let Some((_, product, limit, mut registrations, pending)) = completed.remove(&index)
            else {
                // A prior root may have changed registrations since preflight.
                // Recheck the hit at its sequential position and clear a limit
                // from an earlier root exactly as scalar materialization does.
                results[index] = Some(self.materialize_value(path));
                continue;
            };
            if pending.is_empty() && self.execution_valid(path, &product.reads) {
                registrations.sort_by(|a, b| a.path.cmp(&b.path));
                for registration in registrations {
                    self.install_staged(registration);
                }
                self.last_comptime_limit = limit;
                results[index] = Some(product.result);
            } else {
                // No suspended failure becomes a cached language verdict. A
                // whole-root retry preserves the requester's original stack.
                results[index] = Some(self.materialize_value(path));
            }
        }
        results.into_iter().map(Option::unwrap).collect()
    }
}
