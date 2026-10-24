//! Demand-driven ordinary Lower and constant evaluation. Rust futures retain
//! preparation and interpreter state on the admitted home worker. Publication
//! into a source namespace remains a separate ordered transaction.
use super::*;
use crate::comptime::{EvaluationInput, PreparingSource, ValuePreparation};
use crate::memo::{DepthEvidence, dependency_limit};
use crate::runtime::*;
use crate::store::{Domain, EntryKey, Reference, ResultStore};
use hirsel_tir::{DefinitionRef, Materialized, MaterializedRef};
use std::{
    cell::RefCell,
    future::{Future, poll_fn},
    pin::Pin,
    rc::Rc,
    sync::Mutex,
    task::{Context, Poll, Waker},
};

pub const FRONT_LOWER_KIND: u32 = 0x4600_0003;
pub const FRONT_EVALUATE_KIND: u32 = 0x4600_0004;
const FAILURE: Domain = Domain {
    kind: 0x4600_0005,
    schema: 2,
};
const LOWER_DESCRIPTOR: Domain = Domain {
    kind: 0x4600_0006,
    schema: 2,
};
const CONSTANT_DESCRIPTOR: Domain = Domain {
    kind: 0x4600_0007,
    schema: 2,
};
/// Physical producer work, including work before an eventual failure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrontLowerWork {
    pub lowerings: u64,
    pub initializers: u64,
    pub validation_runs: u64,
}
struct Input {
    lower: EntryKey,
    evaluate: EntryKey,
    implementation: ContentHash,
    site: Span,
}
#[derive(Clone)]
enum Value {
    Lower(LowerProduct),
    Constant(Materialized, Vec<(hirsel_syntax::ExprId, Materialized)>),
}
#[derive(Clone)]
struct LowerProduct {
    definition: DefId,
    body: Reference,
    diagnostics: Vec<Diagnostic>,
    dependencies: Vec<SignatureDependency>,
    verified: Option<hirsel_validate::VerifiedDefinitionOutput>,
}
impl LowerProduct {
    fn decode(&self, owner: &FrontLowerQueries) -> Option<QueryOutput<DefinitionRef>> {
        let bytes = owner.store.get_content(self.body)?;
        let definition: DefinitionRef = owner.records.decode(&owner.anchors, &bytes)?;
        (definition.path == self.definition).then(|| QueryOutput {
            value: definition,
            diagnostics: self.diagnostics.clone(),
            dependencies: self.dependencies.clone(),
        })
    }
}
#[derive(Clone)]
struct Outcome {
    result: Result<Value, QueryError>,
    reads: Reads,
    depth: DepthEvidence<Span>,
}
/// A held Lower product keeps immutable source/output admission alive. The
/// ordinary archive is data; native consumers independently validate it.
#[derive(Clone)]
pub struct FrontLowerOutput {
    pub output: QueryOutput<DefinitionRef>,
    pub reads: Reads,
    pub depth: DepthEvidence<Span>,
    pub body: Reference,
    pub implementation_revision: ContentHash,
    pub signature: ContentHash,
    pub verified: Option<hirsel_validate::VerifiedDefinitionOutput>,
    owner: Arc<FrontLowerQueries>,
}
impl FrontLowerOutput {
    pub fn native_product(&self) -> crate::NativeOriginalProduct {
        crate::NativeOriginalProduct {
            definition: self.output.value.path.path().clone(),
            implementation_revision: self.implementation_revision,
            signature: self.signature,
            body: self.body,
            verified: self.verified.clone(),
        }
    }
    pub fn store(&self) -> &Arc<dyn ResultStore> {
        &self.owner.store
    }
}
pub struct FrontLowerQueries {
    inputs: BTreeMap<DefPath, Input>,
    front: Arc<FrontBodyQueries>,
    snapshot: Arc<SignatureSnapshot>,
    memo: Arc<hirsel_types::TypeMemo>,
    lowering: Arc<dyn LoweringPhase>,
    runtime: Arc<hirsel_interp::aggregate::RuntimeTypes>,
    instances: Arc<hirsel_tir::InstanceBodies>,
    sources: SourceMap,
    limits: hirsel_interp::Limits,
    verify: Verify,
    anchors: crate::NativeRecordContext,
    records: crate::NativeRecords,
    store: Arc<dyn ResultStore>,
    outcomes: Mutex<BTreeMap<(u32, ContentHash), (Reference, Outcome)>>,
    work: Mutex<BTreeMap<DefPath, FrontLowerWork>>,
    weight: MemoryWeight,
    max_reads: usize,
    errors: RuntimeErrors,
    _allowance: MemoryReservation,
}
impl FrontLowerQueries {
    pub fn work(&self, path: &DefPath) -> FrontLowerWork {
        self.work
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .copied()
            .unwrap_or_default()
    }
    fn count(&self, path: &DefPath, count: impl FnOnce(&mut FrontLowerWork)) {
        count(
            self.work
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(path.clone())
                .or_default(),
        );
    }
    pub fn query(&self, path: &DefPath) -> Option<QueryId> {
        self.id(path, FRONT_LOWER_KIND)
    }
    pub fn evaluate_query(&self, path: &DefPath) -> Option<QueryId> {
        self.id(path, FRONT_EVALUATE_KIND)
    }
    fn id(&self, path: &DefPath, kind: u32) -> Option<QueryId> {
        let input = self.inputs.get(path)?;
        let key = if kind == FRONT_LOWER_KIND {
            &input.lower
        } else {
            &input.evaluate
        };
        Some(QueryId {
            kind,
            address: hirsel_artifact_store::Address(key.0),
        })
    }
    fn path(&self, id: QueryId) -> Option<DefPath> {
        self.inputs
            .keys()
            .find(|p| self.id(p, id.kind) == Some(id))
            .cloned()
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn LocalQueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn outcome(self: &Arc<Self>, product: ProductRef) -> Option<FrontLowerOutput> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (reference, outcome) = outcomes.get(&(FRONT_LOWER_KIND, product.address.0))?;
        if *reference != product.record {
            return None;
        }
        let Value::Lower(lower) = outcome.result.as_ref().ok()? else {
            return None;
        };
        let lower = lower.clone();
        let reads = outcome.reads.clone();
        let depth = outcome.depth.clone();
        drop(outcomes);
        let output = lower.decode(self)?;
        let input = self.inputs.get(lower.definition.path())?;
        let signature = self
            .snapshot
            .signature(output.value.path)?
            .as_ref()
            .ok()?
            .fingerprint;
        Some(FrontLowerOutput {
            output,
            reads,
            depth,
            body: lower.body,
            implementation_revision: input.implementation,
            signature,
            verified: lower.verified,
            owner: self.clone(),
        })
    }
    /// Native adaptation reads compact metadata only. Neither an unchanged
    /// Definition nor a native Value/Ready body is reconstructed here.
    pub fn native_product(&self, product: ProductRef) -> Option<crate::NativeOriginalProduct> {
        let outcome = self.selected(
            QueryId {
                kind: FRONT_LOWER_KIND,
                address: product.address,
            },
            Ok(product),
        )?;
        let Value::Lower(lower) = outcome.result.ok()? else {
            return None;
        };
        let path = lower.definition.path();
        Some(crate::NativeOriginalProduct {
            definition: path.clone(),
            implementation_revision: self.inputs.get(path)?.implementation,
            signature: self.snapshot.signature(lower.definition)?.ok()?.fingerprint,
            body: lower.body,
            verified: lower.verified,
        })
    }
    pub fn failure(&self, reference: Reference) -> Option<QueryError> {
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .find(|(r, _)| *r == reference)?
            .1
            .result
            .as_ref()
            .err()
            .cloned()
    }
    fn selected(&self, id: QueryId, selected: Result<ProductRef, Reference>) -> Option<Outcome> {
        let reference = selected.map(|p| p.record).unwrap_or_else(|r| r);
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(id.kind, id.address.0))
            .filter(|(r, _)| *r == reference)
            .map(|(_, o)| o.clone())
    }
    async fn run(
        self: Arc<Self>,
        id: QueryId,
        path: DefPath,
        reads: QueryReads,
    ) -> Result<ProductRef, Reference> {
        let mut source = Source {
            owner: self.clone(),
            reads,
            observed: Rc::new(RefCell::new(ReadsBuilder::new())),
            depth: Rc::new(RefCell::new(DepthEvidence::leaf(dependency_limit(
                self.limits.call_depth,
            )))),
            framed: id.kind == FRONT_EVALUATE_KIND,
        };
        let result = if source.framed && source.depth.borrow().limit() == 0 {
            Err(source.overflow(self.inputs[&path].site))
        } else {
            self.compute(id.kind, &path, &mut source).await
        };
        let mut depth = source.depth.borrow().clone();
        if source.framed {
            depth.first[0] = None;
        }
        let observed = std::mem::take(&mut *source.observed.borrow_mut())
            .finish()
            .map_err(|_| self.errors.protocol)?;
        let input = &self.inputs[&path];
        let key = if id.kind == FRONT_LOWER_KIND {
            &input.lower
        } else {
            &input.evaluate
        };
        let bytes = match &result {
            Ok(Value::Lower(lower)) => self.records.encode(
                &self.anchors,
                &(
                    lower.definition,
                    lower.body,
                    &lower.diagnostics,
                    &lower.dependencies,
                    &observed,
                    &depth,
                ),
            ),
            Ok(Value::Constant(value, substitutions)) => self
                .records
                .encode(&self.anchors, &(value, substitutions, &observed, &depth)),
            Err(error) => self
                .records
                .encode(&self.anchors, &(&path, error, &observed, &depth)),
        }
        .ok_or(self.errors.resource)?;
        if bytes.len() as u64 > self.weight.retained {
            return Err(self.errors.resource);
        }
        // Descriptor domains are explicit: an S5 transport descriptor must
        // never overwrite a scalar Lower/Evaluate archive of another schema.
        let domain = if result.is_err() {
            FAILURE
        } else if id.kind == FRONT_LOWER_KIND {
            LOWER_DESCRIPTOR
        } else {
            CONSTANT_DESCRIPTOR
        };
        let record = self
            .store
            .put_content(domain, &bytes)
            .ok_or(self.errors.resource)?;
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.scheduled-action.v1",
            [
                key.0.0.as_slice(),
                key.1.0.as_slice(),
                crate::store::payload_reads_hash(&observed).0.as_slice(),
                domain.kind.to_le_bytes().as_slice(),
                domain.schema.to_le_bytes().as_slice(),
            ],
        ));
        let failed = result.is_err();
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                (id.kind, id.address.0),
                (
                    record,
                    Outcome {
                        result,
                        reads: observed,
                        depth,
                    },
                ),
            );
        if failed {
            return Err(record);
        }
        self.store.associate_product(id.address, action, record);
        Ok(ProductRef {
            address: id.address,
            action,
            record,
        })
    }
    async fn compute(
        &self,
        kind: u32,
        path: &DefPath,
        source: &mut Source,
    ) -> Result<Value, QueryError> {
        let front = self.front.query(path).ok_or(QueryError::NotImplemented)?;
        let selected = source.reads.read(front).await;
        let reference = selected.map(|p| p.record).unwrap_or_else(|r| r);
        let body = self
            .front
            .outcome(reference)
            .ok_or(QueryError::NotImplemented)?;
        source.merge(&body.outcome.reads);
        let analyzed = body.outcome.result?;
        let signature = source.signature(path)?;
        let mut observed = ReadsBuilder::new();
        let staged = crate::comptime::prepare_staged_types(
            self.snapshot.types(),
            &analyzed.value,
            &mut FrozenProvider::new(&self.snapshot, path.0.package.clone(), &mut observed),
        );
        source.merge(&observed.finish().map_err(|_| QueryError::NotImplemented)?);
        let staged = staged?;
        let mut values = Values {
            source: source.clone(),
            pending: None,
        };
        let mut preparation = ValuePreparation::new(&analyzed.value);
        let constants = if kind == FRONT_LOWER_KIND
            && matches!(signature.kind, hirsel_types::SignatureKind::Const { .. })
        {
            let (_, constants) = source.constant_value(path).await?;
            constants
        } else {
            poll_fn(|_| {
                preparation
                    .poll(
                        self.snapshot.types(),
                        &analyzed.value,
                        &staged,
                        &signature,
                        &mut values,
                    )
                    .map(|r| r.map(|v| v.to_vec()))
            })
            .await?
        };
        if kind == FRONT_EVALUATE_KIND {
            let ItemKind::Const {
                value: expression, ..
            } = analyzed.value.typed.syntax.item.kind
            else {
                return Err(QueryError::NotImplemented);
            };
            let value = if let Some((_, value)) = constants.iter().find(|(id, _)| *id == expression)
            {
                value.clone()
            } else {
                let ty = analyzed
                    .value
                    .typed
                    .checked
                    .expressions
                    .iter()
                    .find(|r| r.expression == expression)
                    .ok_or(QueryError::NotImplemented)?
                    .ty;
                evaluate(
                    source.clone(),
                    analyzed.clone(),
                    staged,
                    signature,
                    expression,
                    expression,
                    ty,
                    constants.clone(),
                )
                .await?
                .result?
            };
            return Ok(Value::Constant(value, constants));
        }
        self.count(path, |work| work.lowerings += 1);
        let output = self.lowering.lower(
            self.snapshot.types(),
            &self.memo,
            &analyzed.value,
            &Comptime {
                materialized: constants,
                staged,
            },
            &signature,
        )?;
        let definition = Arc::new(output.value);
        for dependency in &output.dependencies {
            source.record(
                ReadKey::Signature(dependency.definition),
                dependency.fingerprint,
            );
        }
        let mut verified = None;
        if self.verify == Verify::On {
            let mut observed = ReadsBuilder::new();
            FrozenProvider::new(&self.snapshot, path.0.package.clone(), &mut observed)
                .validation_universe();
            source.merge(&observed.finish().map_err(|_| QueryError::NotImplemented)?);
            let (report, ran, _, evidence) = self
                .snapshot
                .validate_source_artifact(&definition, |anchor| self.anchors.record_owner(anchor));
            if ran {
                timings::count(Count::VRuns);
                self.count(path, |work| work.validation_runs += 1);
            }
            if let Some(error) = hirsel_validate::tooling_limit_error(&report) {
                return Err(error);
            }
            if !report.violations.is_empty() {
                return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "TIR validation".into(),
                    message: format!("{:?}", report.violations),
                }));
            }
            if let Some(evidence) = evidence
                && let Some(cache) = self.store.verifier()
            {
                verified = cache.definition_output_from_evidence(
                    &definition,
                    self.snapshot.types().clone(),
                    self.snapshot.source_verification_policy(),
                    self.snapshot.validation(&path.0.package),
                    &evidence,
                );
            }
        }
        if has_errors(&output.diagnostics) {
            return Err(QueryError::Diagnostics(output.diagnostics));
        }
        let bytes = self
            .records
            .encode(&self.anchors, &definition)
            .ok_or(QueryError::NotImplemented)?;
        if bytes.len() as u64 > self.weight.retained {
            return Err(QueryError::NotImplemented);
        }
        let body = self
            .store
            .put_content(crate::NATIVE_ORIGINAL_BODY, &bytes)
            .ok_or(QueryError::NotImplemented)?;
        let mut dependencies = analyzed.dependencies.clone();
        dependencies.extend(output.dependencies);
        Ok(Value::Lower(LowerProduct {
            definition: definition.path,
            body,
            diagnostics: output.diagnostics,
            dependencies,
            verified,
        }))
    }
}
struct Factory(Arc<FrontLowerQueries>);
impl LocalQueryFactory for Factory {
    fn weight(&self, id: QueryId) -> Result<MemoryWeight, Reference> {
        if !matches!(id.kind, FRONT_LOWER_KIND | FRONT_EVALUATE_KIND) || self.0.path(id).is_none() {
            return Err(self.0.errors.protocol);
        }
        Ok(self.0.weight)
    }
    fn create(&self, id: QueryId) -> Result<Box<dyn LocalQueryTask>, Reference> {
        let weight = self.weight(id)?;
        let path = self.0.path(id).ok_or(self.0.errors.protocol)?;
        let owner = self.0.clone();
        Ok(Box::new(LocalFutureTask::new(
            id,
            weight,
            self.0.max_reads,
            self.0.errors,
            move |reads| owner.run(id, path, reads),
        )?))
    }
}
#[derive(Clone)]
struct Source {
    owner: Arc<FrontLowerQueries>,
    reads: QueryReads,
    observed: Rc<RefCell<ReadsBuilder>>,
    depth: Rc<RefCell<DepthEvidence<Span>>>,
    framed: bool,
}
impl Source {
    fn overflow(&self, site: Span) -> QueryError {
        let limit = hirsel_tir::ToolingLimit {
            kind: hirsel_tir::ToolingLimitKind::CallDepth,
            limit: dependency_limit(self.owner.limits.call_depth) as u64,
            site: Some(site),
        };
        QueryError::Operational(hirsel_types::OperationalFailure {
            operation: "compile-time tooling limit".into(),
            message: format!("{limit:?}"),
        })
    }
    fn merge(&self, reads: &Reads) {
        self.observed.borrow_mut().merge(reads);
    }
    fn record(&self, key: ReadKey, hash: ContentHash) {
        self.observed.borrow_mut().record(key, hash);
    }
    fn signature(&self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError> {
        let mut observed = ReadsBuilder::new();
        let result = SignatureProvider::signature(
            &mut FrozenProvider::new(&self.owner.snapshot, path.0.package.clone(), &mut observed),
            DefId::intern(path),
        );
        self.merge(&observed.finish().map_err(|_| QueryError::NotImplemented)?);
        result
    }
    async fn value(&self, path: &DefPath, kind: u32, site: Span) -> Result<Value, QueryError> {
        let id = self
            .owner
            .id(path, kind)
            .ok_or(QueryError::NotImplemented)?;
        let selected = self.reads.read(id).await;
        let outcome = self
            .owner
            .selected(id, selected)
            .ok_or(QueryError::NotImplemented)?;
        self.merge(&outcome.reads);
        let n = u32::from(self.framed);
        let overflow = if kind == FRONT_EVALUATE_KIND {
            self.depth.borrow_mut().observe_frame(&outcome.depth, site);
            outcome.depth.request(n, site).err()
        } else {
            self.depth.borrow_mut().observe_transparent(&outcome.depth);
            outcome.depth.first[n.min(outcome.depth.limit()) as usize]
        };
        if let Some(site) = overflow {
            return Err(self.overflow(site));
        }
        self.record(
            ReadKey::ExecutedBody(DefId::intern(path)),
            memos::optional(Some(self.owner.inputs[path].implementation)),
        );
        outcome.result
    }
    async fn constant_value(
        &self,
        path: &DefPath,
    ) -> Result<(Materialized, Vec<(hirsel_syntax::ExprId, Materialized)>), QueryError> {
        self.constant_at(
            path,
            self.owner
                .inputs
                .get(path)
                .ok_or(QueryError::NotImplemented)?
                .site,
        )
        .await
    }
    async fn constant_at(
        &self,
        path: &DefPath,
        site: Span,
    ) -> Result<(Materialized, Vec<(hirsel_syntax::ExprId, Materialized)>), QueryError> {
        match self.value(path, FRONT_EVALUATE_KIND, site).await? {
            Value::Constant(v, c) => Ok((v, c)),
            _ => Err(QueryError::NotImplemented),
        }
    }
}
impl s1::AsyncDependencySource for Source {
    async fn definition(&mut self, path: &DefPath) -> s1::DependencyProduct<DefinitionRef> {
        let result = match self.value(path, FRONT_LOWER_KIND, Span::default()).await {
            Ok(Value::Lower(lower)) => {
                for d in &lower.dependencies {
                    self.record(ReadKey::ExecutedSignature(d.definition), d.fingerprint);
                }
                lower
                    .decode(&self.owner)
                    .map(|output| output.value)
                    .ok_or(QueryError::NotImplemented)
            }
            Err(e) => Err(e),
            _ => Err(QueryError::NotImplemented),
        };
        s1::DependencyProduct {
            result,
            reads: Reads::empty(),
        }
    }
    async fn constant(&mut self, path: &DefPath) -> s1::DependencyProduct<MaterializedRef> {
        let result = self.constant_value(path).await.map(|(v, _)| Arc::new(v));
        s1::DependencyProduct {
            result,
            reads: Reads::empty(),
        }
    }
    async fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> s1::DependencyProduct<MaterializedRef> {
        if arguments.is_empty() {
            return s1::AsyncDependencySource::constant(self, path).await;
        }
        s1::DependencyProduct {
            result: Err(QueryError::NotImplemented),
            reads: Reads::empty(),
        }
    }
    async fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[hirsel_types::GenericArgument],
    ) -> s1::DependencyProduct<Option<DefPath>> {
        let result = self
            .owner
            .instances
            .resolve(DefId::intern(path), self.owner.snapshot.types(), arguments)
            .map(|selected| selected.map(|id| id.path().clone()));
        s1::DependencyProduct {
            result,
            reads: Reads::empty(),
        }
    }
}
type Evaluation =
    Pin<Box<dyn Future<Output = Result<s1::DependencyProduct<Materialized>, QueryError>>>>;
struct Values {
    source: Source,
    pending: Option<Evaluation>,
}
impl PreparingSource for Values {
    fn signature(
        &mut self,
        path: &DefPath,
    ) -> Poll<Result<hirsel_types::SignatureRef, QueryError>> {
        Poll::Ready(self.source.signature(path))
    }
    fn constant(&mut self, path: &DefPath, site: Span) -> Poll<Result<Materialized, QueryError>> {
        if self.pending.is_none() {
            let source = self.source.clone();
            let path = path.clone();
            self.pending = Some(Box::pin(async move {
                Ok(s1::DependencyProduct {
                    result: source.constant_at(&path, site).await.map(|(v, _)| v),
                    reads: Reads::empty(),
                })
            }));
        }
        self.advance()
    }
    fn evaluate(&mut self, input: EvaluationInput<'_>) -> Poll<Result<Materialized, QueryError>> {
        if self.pending.is_none() {
            self.pending = Some(Box::pin(evaluate(
                self.source.clone(),
                Arc::new(QueryOutput {
                    value: input.analyzed.clone(),
                    diagnostics: vec![],
                    dependencies: vec![],
                }),
                input.staged.to_vec(),
                Arc::new(input.signature.clone()),
                input.expression,
                input.child,
                input.ty,
                input.constants.to_vec(),
            )));
        }
        self.advance()
    }
}
impl Values {
    fn advance(&mut self) -> Poll<Result<Materialized, QueryError>> {
        match self
            .pending
            .as_mut()
            .unwrap()
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.pending = None;
                Poll::Ready(result.and_then(|p| {
                    self.source.merge(&p.reads);
                    p.result
                }))
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
async fn evaluate(
    mut source: Source,
    analyzed: Arc<QueryOutput<AnalyzedBody>>,
    staged: Vec<(hirsel_syntax::ExprId, hirsel_types::TypeId)>,
    signature: hirsel_types::SignatureRef,
    expression: hirsel_syntax::ExprId,
    child: hirsel_syntax::ExprId,
    ty: hirsel_types::TypeId,
    constants: Vec<(hirsel_syntax::ExprId, Materialized)>,
) -> Result<s1::DependencyProduct<Materialized>, QueryError> {
    let owner = source.owner.clone();
    let site = analyzed.value.typed.syntax.tree.expressions[expression.0 as usize]
        .origin
        .span;
    owner.count(signature.definition.path(), |work| work.initializers += 1);
    let prepared = EvaluationInput {
        analyzed: &analyzed.value,
        staged: &staged,
        signature: &signature,
        expression,
        child,
        ty,
        constants: &constants,
        sizes: &[],
    }
    .lower(owner.snapshot.types(), &owner.memo, &*owner.lowering)?;
    let admission =
        s1::ValidatedEvaluation::recorded(prepared, &owner.snapshot, owner.verify, site);
    source.merge(&admission.reads);
    let runtime = (*owner.runtime).clone();
    let product = admission
        .result?
        .run_async(&owner.sources, owner.limits, &runtime, &mut source)
        .await;
    source.merge(&product.value.reads);
    let result = product.value.result.and_then(|(value, ty)| {
        let mut observed = ReadsBuilder::new();
        let result = crate::materialized::reify_value(
            owner.snapshot.types(),
            &owner.memo,
            &mut Materializer {
                provider: FrozenProvider::new(
                    &owner.snapshot,
                    signature.definition.path().0.package.clone(),
                    &mut observed,
                ),
                source: &source,
            },
            value,
            ty,
            site,
        );
        source.merge(&observed.finish().map_err(|_| QueryError::NotImplemented)?);
        result
    });
    Ok(s1::DependencyProduct {
        result,
        reads: Reads::empty(),
    })
}
struct Materializer<'a> {
    provider: FrozenProvider<'a>,
    source: &'a Source,
}
impl crate::materialized::MaterializationSource for Materializer<'_> {
    fn signature(&mut self, id: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        SignatureProvider::signature(&mut self.provider, id)
    }
    fn body_view(&mut self, id: DefId) -> Result<hirsel_types::SignatureRef, QueryError> {
        let public = SignatureProvider::signature(&mut self.provider, id)?;
        let private = self.provider.body_view(id)?;
        if !Arc::ptr_eq(&public, &private)
            && let Some(input) = self.source.owner.inputs.get(id.path())
        {
            self.source.record(
                ReadKey::ExecutedBody(id),
                memos::optional(Some(input.implementation)),
            );
        }
        Ok(private)
    }
}
impl QueryEngine {
    /// Capture input identities and immutable phase handles. This does no body
    /// checking, lowering or interpretation, and never calls the S1 controller.
    pub fn front_lower_queries(
        &mut self,
        definitions: &[DefPath],
        front: Arc<FrontBodyQueries>,
        weight: MemoryWeight,
        max_reads: usize,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontLowerQueries>, QueryError> {
        let refusal = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "front Lower admission".into(),
                message: "immutable Lower inputs exceed admission or lack phase/source bindings"
                    .into(),
            })
        };
        if weight
            .retained
            .checked_add(8192)
            .and_then(|b| b.checked_mul(2))
            .and_then(|b| b.checked_mul(definitions.len() as u64))
            .is_none_or(|b| b > allowance.bytes())
        {
            return Err(refusal());
        }
        let store = self
            .result_store
            .clone()
            .filter(|s| s.enabled())
            .ok_or_else(refusal)?;
        let lowering = self.adapters.lowering.clone().ok_or_else(refusal)?;
        let mut inputs = BTreeMap::new();
        for path in definitions {
            if front.query(path).is_none() {
                return Err(refusal());
            }
            let syntax = self.snapshot(path)?;
            inputs.insert(
                path.clone(),
                Input {
                    lower: self.store_key(path, QueryKind::Lower).ok_or_else(refusal)?,
                    evaluate: self
                        .store_key(path, QueryKind::Evaluate)
                        .ok_or_else(refusal)?,
                    implementation: syntax.text_hash,
                    site: syntax.item.name_span,
                },
            );
        }
        let snapshot = self.signature_inputs();
        let runtime = self.comptime_runtime_types()?;
        Ok(Arc::new(FrontLowerQueries {
            inputs,
            front,
            snapshot,
            memo: self.memo.clone(),
            lowering,
            runtime,
            instances: Arc::new(self.instance_bodies(&definitions.iter().cloned().collect())),
            sources: self.sources.clone(),
            limits: self.comptime_limits,
            verify: self.verify,
            anchors: crate::NativeRecordContext::capture(self),
            records: crate::NativeRecords::new(self.types.clone(), store.clone()),
            store,
            outcomes: Mutex::new(BTreeMap::new()),
            work: Mutex::new(BTreeMap::new()),
            weight,
            max_reads,
            errors,
            _allowance: allowance,
        }))
    }
}
