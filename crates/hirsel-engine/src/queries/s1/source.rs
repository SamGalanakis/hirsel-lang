//! Immutable session views for S1 body demands. Tables are borrowed, never
//! cloned into workers. Ordinary misses use the existing frozen P task, and
//! complete products return for deterministic serial publication.
use super::worker::{Requirement, SelectionEvent, TaskBody, TaskInputs, TaskWorker, TirSource};
use super::*;
use crate::memo::{DepthEvidence, MemoGraph};
use crate::snapshot::SourceContext;
use std::sync::Mutex;

/// One lowering root with its captured operational provenance and complete
/// execution reads. Publication of its TIR remains the caller's policy.
#[derive(Clone, Debug)]
pub struct LoweringResult<T = DefinitionRef> {
    pub definition: DefPath,
    pub result: Result<QueryOutput<T>, QueryError>,
    pub phase: Option<QueryKind>,
    pub limit: Option<hirsel_tir::ToolingLimit>,
    pub reads: Arc<Reads>,
    pub depth: Option<DepthEvidence<Span>>,
}
#[derive(Clone, Copy)]
enum RootKind {
    Constant,
    Lower,
    CheckLower,
}
#[derive(Clone)]
enum RootValue {
    Constant(hirsel_tir::Materialized),
    Lowered(QueryOutput<DefinitionRef>, DepthEvidence<Span>),
    Checked(QueryOutput<()>, DepthEvidence<Span>),
}
#[derive(Clone)]
struct RootOutcome {
    definition: DefPath,
    result: Result<RootValue, QueryError>,
    phase: Option<QueryKind>,
    limit: Option<hirsel_tir::ToolingLimit>,
    reads: Arc<Reads>,
}
impl RootOutcome {
    fn lowering<T>(
        self,
        value: impl FnOnce(RootValue) -> (QueryOutput<T>, DepthEvidence<Span>),
    ) -> LoweringResult<T> {
        let (result, depth) = match self.result {
            Ok(root) => {
                let (output, depth) = value(root);
                (Ok(output), Some(depth))
            }
            Err(error) => (Err(error), None),
        };
        LoweringResult {
            definition: self.definition,
            result,
            phase: self.phase,
            limit: self.limit,
            reads: self.reads,
            depth,
        }
    }
}
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
    ) -> Option<(QueryOutput<DefinitionRef>, Reads, DepthEvidence<Span>)> {
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
                return Some(((*output).clone(), entry.reads().clone(), depth));
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
            QueryOutput {
                value: lowering.definition.clone(),
                diagnostics: vec![],
                dependencies: lowering.dependencies.clone(),
            },
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
    pub(crate) fn frozen_s1_source(
        &mut self,
        lower_budget: u64,
    ) -> Result<FrozenSource<'_>, QueryError> {
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
                lower_budget,
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
    /// order. Frozen workers resume staged/generated registration rounds;
    /// unavailable syntax or inconsistent receipts retain whole-root retry.
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
        self.s1_root_outcomes(definitions, jobs, RootKind::Constant, false)
            .into_iter()
            .map(|outcome| MaterializationResult {
                definition: outcome.definition,
                result: outcome.result.map(|value| match value {
                    RootValue::Constant(value) => value,
                    _ => unreachable!("constant task"),
                }),
                phase: outcome.phase,
                limit: outcome.limit,
                reads: outcome.reads,
            })
            .collect()
    }
    /// Lower independent roots on the same frozen S1 schedule. Compile-time
    /// values, staged declarations and generated instances use the same rounds
    /// and ordered failure publication as constant materialization.
    pub fn lower_definition_outcomes(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<LoweringResult> {
        self.lower_root_results(definitions, jobs, false)
    }
    fn lower_root_results(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
        prefix: bool,
    ) -> Vec<LoweringResult> {
        self.s1_root_outcomes(definitions, jobs, RootKind::Lower, prefix)
            .into_iter()
            .map(|outcome| {
                outcome.lowering(|value| match value {
                    RootValue::Lowered(output, depth) => (output, depth),
                    _ => unreachable!("lower task"),
                })
            })
            .collect()
    }
    /// Lower an ordered file prefix, returning through its first rejection.
    /// Later speculative roots must not publish declarations or body products.
    pub fn lower_definition_prefix(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<LoweringResult> {
        self.lower_root_results(definitions, jobs, true)
    }
    /// Check an ordered prefix without retaining its completed root TIR.
    /// Each worker drops the body before returning its diagnostics, ordered
    /// dependencies, read receipts and transparent depth to serial publication.
    pub fn check_lower_definition_prefix(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
    ) -> Vec<LoweringResult<()>> {
        self.s1_root_outcomes(definitions, jobs, RootKind::CheckLower, true)
            .into_iter()
            .map(|outcome| {
                outcome.lowering(|value| match value {
                    RootValue::Checked(output, depth) => (output, depth),
                    _ => unreachable!("check lower task"),
                })
            })
            .collect()
    }
    fn s1_root_outcomes(
        &mut self,
        definitions: Vec<DefPath>,
        jobs: hirsel_intern::jobs::Jobs,
        kind: RootKind,
        prefix: bool,
    ) -> Vec<RootOutcome> {
        let mut selections = BTreeMap::new();
        let mut settled = vec![];
        loop {
            let remaining = &definitions[settled.len()..];
            if let Some(results) =
                self.s1_roots_round(remaining, jobs, kind, &mut selections, prefix)
            {
                if let Some(last) = results.last() {
                    self.last_phase = last.phase;
                    self.last_comptime_limit = last.limit.clone();
                    self.unavailable = matches!(last.result, Err(QueryError::NotImplemented))
                        .then_some(last.phase)
                        .flatten();
                }
                let failed = results.last().is_some_and(|result| result.result.is_err());
                settled.extend(results);
                if !prefix || failed || settled.len() == definitions.len() {
                    return settled;
                }
            }
        }
    }
    fn scalar_root_outcome(&mut self, path: &DefPath, kind: RootKind) -> RootOutcome {
        self.last_phase = None;
        self.unavailable = None;
        self.last_comptime_limit = None;
        self.execution_stack.push(ReadsBuilder::new());
        let mut result = match kind {
            RootKind::Constant => self.materialize_value(path).map(RootValue::Constant),
            RootKind::Lower | RootKind::CheckLower => {
                // An image shortcut replays its stored execution, but has no
                // resident memo whose root input or V origin we can inherit.
                // Match worker admission at this outcome boundary.
                let mut root_reads = ReadsBuilder::new();
                if let Some(hash) = self.implementation_hash(path) {
                    root_reads.record(
                        ReadKey::ExecutedBody(DefId::intern(path)),
                        memos::optional(Some(hash)),
                    );
                }
                if self.verify == Verify::On {
                    let snapshot = self.signature_inputs();
                    FrozenProvider::new(&snapshot, path.0.package.clone(), &mut root_reads)
                        .validation_universe();
                }
                self.merge_execution(&root_reads.finish().expect("one root snapshot"));
                self.s1.begin_requests();
                let result = self.lower_to_tir(path);
                let depth = self.s1.finish_requests(self.depth_limit());
                result.map(|output| match kind {
                    RootKind::CheckLower => {
                        RootValue::Checked(worker::discard_lowered_output(output), depth)
                    }
                    _ => RootValue::Lowered(output, depth),
                })
            }
        };
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
        RootOutcome {
            definition: path.clone(),
            result,
            phase,
            limit: self.last_comptime_limit.clone(),
            reads: Arc::new(reads),
        }
    }
    fn s1_roots_round(
        &mut self,
        definitions: &[DefPath],
        jobs: hirsel_intern::jobs::Jobs,
        kind: RootKind,
        selections: &mut BTreeMap<DefPath, hirsel_diag::Label>,
        prefix: bool,
    ) -> Option<Vec<RootOutcome>> {
        if definitions.is_empty() {
            return Some(vec![]);
        }
        self.note_environment();
        self.prepare_materialization_environment();
        let mut results: Vec<Option<RootOutcome>> = vec![None; definitions.len()];
        let mut tasks = vec![];
        for (index, definition) in definitions.iter().enumerate() {
            if matches!(kind, RootKind::Constant)
                && let Some(value) = self.constant_values.get(definition).cloned()
                && (self.verify == Verify::Off
                    || value
                        .executed
                        .iter()
                        .any(|(key, _)| matches!(key, ReadKey::ValidationUniverse(_))))
                && self.execution_valid(definition, &value.executed)
            {
                results[index] = Some(RootOutcome {
                    definition: definition.clone(),
                    result: Ok(RootValue::Constant(value.value)),
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
        let workers = jobs.get().min(tasks.len()).max(1);
        let lower_budget = self.hold_bytes / workers as u64;
        let source = match self.frozen_s1_source(lower_budget) {
            Ok(source) => source,
            Err(_) => {
                let mut outcomes = vec![];
                for path in definitions {
                    let outcome = self.scalar_root_outcome(path, kind);
                    let failed = outcome.result.is_err();
                    outcomes.push(outcome);
                    if prefix && failed {
                        break;
                    }
                }
                return Some(outcomes);
            }
        };
        let mut completed = hirsel_intern::jobs::map_with_state(
            jobs,
            tasks,
            |_| 1,
            || TaskWorker::new(&source, &shared),
            |worker, (index, path)| {
                let product = match kind {
                    RootKind::Constant => {
                        let product = worker.root(&path);
                        DependencyProduct {
                            result: product.result.map(RootValue::Constant),
                            reads: product.reads,
                        }
                    }
                    RootKind::Lower => {
                        let product = worker.lower_root(&path);
                        DependencyProduct {
                            result: product
                                .result
                                .map(|(output, depth)| RootValue::Lowered(output, depth)),
                            reads: product.reads,
                        }
                    }
                    RootKind::CheckLower => {
                        let product = worker.check_lower_root(&path);
                        DependencyProduct {
                            result: product
                                .result
                                .map(|(output, depth)| RootValue::Checked(output, depth)),
                            reads: product.reads,
                        }
                    }
                };
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
        // A worker result is tentative until its input position settles.
        // Only the earliest incomplete/rejected root may drive registration;
        // later work cannot publish source or memo products across that barrier.
        let end = if prefix {
            completed
                .iter()
                .filter(|(_, _, product, _, _, _, pending, _)| {
                    product.result.is_err() || !pending.is_empty()
                })
                // Settle earlier successes before a later demand changes the
                // signature universe. Only a barrier at the first root may
                // publish its own required registrations in this round.
                .map(|(index, ..)| if *index == 0 { 1 } else { *index })
                .min()
                .unwrap_or(definitions.len())
        } else {
            definitions.len()
        };
        completed.retain(|(index, ..)| *index < end);
        let mut demanded: BTreeSet<DefId> = definitions[..end].iter().map(DefId::intern).collect();
        for (_, _, product, _, _, _, _, _) in &completed {
            for (read, _) in product.reads.iter() {
                if let ReadKey::ExecutedBody(definition) = read {
                    demanded.insert(*definition);
                }
            }
        }
        let bodies = source
            .completed
            .into_inner()
            .unwrap_or_else(|e| e.into_inner());
        let mut bodies = bodies;
        bodies.sort_by(|(left, _, _), (right, _, _)| left.definition().cmp(right.definition()));
        for (body, text, reads) in bodies {
            if (!prefix || demanded.contains(&DefId::intern(body.definition())))
                && self.key_text(body.definition(), QueryKind::Modes).ok() == Some(text)
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
            if (!prefix || demanded.contains(&DefId::intern(&product.definition)))
                && self.implementation_hash(&product.definition) == Some(product.syntax.text_hash)
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
            if (!prefix || demanded.contains(&DefId::intern(&path)))
                && self.execution_valid(&path, &constant.executed)
            {
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
        for (index, path) in definitions[..end].iter().enumerate() {
            let Some((_, product, limit, phase, mut registrations, pending, selected)) =
                completed.remove(&index)
            else {
                // A prior root may have changed registrations since preflight.
                // Recheck the hit at its sequential position and clear a limit
                // from an earlier root exactly as scalar materialization does.
                results[index] = Some(self.scalar_root_outcome(path, kind));
                continue;
            };
            if pending.is_empty() && self.execution_valid(path, &product.reads) {
                registrations.sort_by(|a, b| a.path.cmp(&b.path));
                for registration in registrations {
                    self.install_staged(registration);
                }
                self.last_comptime_limit = limit;
                results[index] = Some(RootOutcome {
                    definition: path.clone(),
                    result: product.result,
                    phase,
                    limit: self.last_comptime_limit.clone(),
                    reads: Arc::new(product.reads),
                });
            } else {
                // No suspended failure becomes a cached language verdict. A
                // whole-root retry preserves the requester's original stack.
                results[index] = Some(self.scalar_root_outcome(path, kind));
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
            if prefix && results[index].as_ref().unwrap().result.is_err() {
                results.truncate(index + 1);
                return Some(results.into_iter().map(Option::unwrap).collect());
            }
        }
        results.truncate(end);
        Some(results.into_iter().map(Option::unwrap).collect())
    }
}
