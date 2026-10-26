//! Per-definition native source observations from actual admitted P/Lower/
//! Evaluate products. This is ordinary data; V's token is passed through from
//! the exact Lower completion and no native C/Ready authority is constructed.
use super::*;
use crate::runtime::*;
use crate::store::{Domain, Reference, ResultStore};
use hirsel_types::{SignatureKind, SignatureRef};
use std::{sync::Mutex, task::Poll};

pub const FRONT_NATIVE_DEFINITION_KIND: u32 = 0x4600_0017;
const OUTPUT: Domain = Domain {
    kind: 0x4600_0018,
    schema: 1,
};
const FAILURE: Domain = Domain {
    kind: 0x4600_0019,
    schema: 1,
};
struct SourceOwnership {
    _allowance: MemoryReservation,
    _front: Arc<FrontBodyQueries>,
    _lower: Arc<FrontLowerQueries>,
}
struct Input {
    signature: SignatureRef,
    representation: Option<crate::representation::OwnerRecord>,
}
/// Prepared bodyless catalogue inputs. Sizing and installation consume the
/// same private data; the caller cannot supply source V or native authority.
pub struct FrontNativeDefinitionInputs {
    definitions: Vec<DefPath>,
    snapshot: Arc<SignatureSnapshot>,
    original_context: crate::NativeOriginalContext,
    inputs: BTreeMap<DefPath, Input>,
    ids: BTreeMap<DefPath, QueryId>,
    records: crate::NativeRecords,
    anchors: crate::NativeRecordContext,
    store: Arc<dyn ResultStore>,
    required: u64,
}
impl FrontNativeDefinitionInputs {
    pub fn input_bytes(&self) -> u64 {
        self.required
    }
}
struct Data {
    product: Arc<crate::NativeDiscoveryProduct>,
    reads: Arc<Reads>,
    original: Option<Arc<crate::NativeOriginalProduct>>,
}
/// Shares the settled product its owner charged to the output budget, so a
/// reader holds handles rather than another decoded copy.
pub struct FrontNativeDefinitionOutput {
    pub product: Arc<crate::NativeDiscoveryProduct>,
    pub reads: Arc<Reads>,
    pub original: Option<Arc<crate::NativeOriginalProduct>>,
    _owner: Arc<FrontNativeDefinitionQueries>,
}
pub struct FrontNativeDefinitionQueries {
    selection: Option<(Arc<FrontRunQueries>, DefPath)>,
    inputs: BTreeMap<DefPath, Input>,
    ids: BTreeMap<DefPath, QueryId>,
    original_context: Arc<crate::NativeOriginalContext>,
    front: Arc<FrontBodyQueries>,
    lower: Arc<FrontLowerQueries>,
    records: crate::NativeRecords,
    anchors: crate::NativeRecordContext,
    store: Arc<dyn ResultStore>,
    completed: Mutex<BTreeMap<ContentHash, (ProductRef, Arc<Data>)>>,
    failures: Mutex<BTreeMap<ContentHash, (Reference, QueryError)>>,
    remaining: Mutex<u64>,
    weight: MemoryWeight,
    errors: RuntimeErrors,
    _ownership: Arc<SourceOwnership>,
}
impl FrontNativeDefinitionQueries {
    pub(crate) fn with_selection(
        mut self: Arc<Self>,
        run: Arc<FrontRunQueries>,
        root: DefPath,
    ) -> Result<Arc<Self>, QueryError> {
        if !run.matches_inputs(&self.front, &self.lower) {
            return Err(native_selection_error(
                "native catalogue and Run have different immutable inputs",
            ));
        }
        let query = run
            .query(&root)
            .ok_or_else(|| native_selection_error("native selection root is missing"))?;
        let this = Arc::get_mut(&mut self).ok_or_else(|| {
            native_selection_error("native selection requires an unshared catalogue")
        })?;
        for id in this.ids.values_mut() {
            id.address = hirsel_artifact_store::Address(ContentHash::of_parts(
                "hirsel.front.native-selected-address.v1",
                [id.address.0.0.as_slice(), query.address.0.0.as_slice()],
            ));
        }
        this.selection = Some((run, root));
        Ok(self)
    }
    pub(crate) fn inputs_contains(&self, path: &DefPath) -> bool {
        self.inputs.contains_key(path)
    }
    pub(crate) fn constant_reads(&self, path: &DefPath) -> Option<Arc<Reads>> {
        self.lower.settled_reads(path, QueryKind::Evaluate)
    }
    pub fn original_context(&self) -> Arc<crate::NativeOriginalContext> {
        self.original_context.clone()
    }
    pub fn signatures(&self) -> Arc<SignatureSnapshot> {
        self.lower.signatures()
    }

    pub fn input_bytes(definitions: &[DefPath]) -> Option<u64> {
        definitions
            .iter()
            .try_fold(std::mem::size_of::<Self>() as u64, |total, path| {
                total
                    .checked_add(bincode::serialized_size(path).ok()?.checked_mul(3)?)?
                    .checked_add(
                        (std::mem::size_of::<Input>() + 16 * std::mem::size_of::<usize>()) as u64,
                    )
            })
    }
    pub fn query(&self, path: &DefPath) -> Option<QueryId> {
        self.ids.get(path).copied()
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn outcome(self: &Arc<Self>, product: ProductRef) -> Option<FrontNativeDefinitionOutput> {
        let values = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, data) = values.get(&product.address.0)?;
        (*selected == product).then(|| FrontNativeDefinitionOutput {
            product: data.product.clone(),
            original: data.original.clone(),
            reads: data.reads.clone(),
            _owner: self.clone(),
        })
    }
    pub fn failure(&self, reference: Reference) -> Option<QueryError> {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&reference.output.0)
            .and_then(|(selected, error)| (*selected == reference).then(|| error.clone()))
            .or_else(|| self.lower.failure(reference))
            .or_else(|| {
                self.front
                    .outcome(reference)
                    .and_then(|o| o.outcome.result.err())
            })
    }
    fn fail(&self, error: QueryError) -> Reference {
        let Some(bytes) = self.records.encode(&self.anchors, &error) else {
            return self.errors.resource;
        };
        if bytes.len() as u64 > self.weight.scratch {
            return self.errors.resource;
        }
        let Some(reference) = self.store.put_content(FAILURE, &bytes) else {
            return self.errors.resource;
        };
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(reference.output.0, (reference, error));
        reference
    }
}
struct Factory(Arc<FrontNativeDefinitionQueries>);
impl QueryFactory for Factory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        let path = self
            .0
            .ids
            .iter()
            .find_map(|(path, query)| (*query == id).then(|| path.clone()))
            .ok_or(self.0.errors.protocol)?;
        let terminal = self
            .0
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id.address.0)
            .map(|(p, _)| Ok(*p));
        Ok(Box::new(Task {
            owner: self.0.clone(),
            path,
            id,
            waiting: None,
            terminal,
        }))
    }
}
struct Task {
    owner: Arc<FrontNativeDefinitionQueries>,
    path: DefPath,
    id: QueryId,
    waiting: Option<QueryId>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        self.owner.weight
    }
    /// A settled output is charged to the owner's output budget, so only
    /// this frame holds the runtime grant.
    fn continuation_bytes(&self) -> u64 {
        self.owner.weight.retained
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let result = if let Some(result) = self.terminal {
            result
        } else {
            match self.advance(cx) {
                Poll::Pending => return QueryPoll::Waiting(self.waiting.into_iter().collect()),
                Poll::Ready(result) => {
                    self.terminal = Some(result);
                    result
                }
            }
        };
        match result {
            Ok(product) => QueryPoll::Complete(product),
            Err(error) => QueryPoll::Failed(error),
        }
    }
}
impl Task {
    fn read(
        &mut self,
        cx: &mut dyn QueryContext,
        id: QueryId,
    ) -> Poll<Result<ProductRef, Reference>> {
        match cx.read(id) {
            Poll::Pending => {
                self.waiting = Some(id);
                Poll::Pending
            }
            Poll::Ready(result) => {
                self.waiting = None;
                Poll::Ready(result)
            }
        }
    }
    fn advance(&mut self, cx: &mut dyn QueryContext) -> Poll<Result<ProductRef, Reference>> {
        let owner = self.owner.clone();
        if let Some((run, root)) = &owner.selection {
            let Some(id) = run.query(root) else {
                return Poll::Ready(Err(owner.errors.protocol));
            };
            let product = match self.read(cx, id) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(product)) => product,
            };
            let Some(output) = run.outcome(product) else {
                return Poll::Ready(Err(owner.errors.protocol));
            };
            if !output.semantic.semantic_contains(&self.path) {
                return Poll::Ready(Err(owner.fail(native_selection_error(
                    "definition is outside the native source selection",
                ))));
            }
        }
        let input = &owner.inputs[&self.path];
        let Some(id) = owner.front.query(&self.path) else {
            return Poll::Ready(Err(owner.errors.protocol));
        };
        let selected = match self.read(cx, id) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(r)) => return Poll::Ready(Err(r)),
            Poll::Ready(Ok(p)) => p,
        };
        let Some(front) = owner.front.outcome(selected.record) else {
            return Poll::Ready(Err(owner.errors.protocol));
        };
        let analyzed = match &front.outcome.result {
            Ok(body) => body,
            Err(error) => return Poll::Ready(Err(owner.fail(error.clone()))),
        };
        if has_errors(&analyzed.diagnostics) {
            return Poll::Ready(Err(
                owner.fail(QueryError::Diagnostics(analyzed.diagnostics.clone()))
            ));
        }
        let syntax = analyzed.value.typed.syntax.clone();
        let executable = generation::executable(&syntax)
            && native_declaration(owner.lower.signatures().types(), &input.signature);
        let mut reads = ReadsBuilder::new();
        reads.merge(&front.outcome.reads);
        let lower = if executable {
            let Some(id) = owner.lower.query(&self.path) else {
                return Poll::Ready(Err(owner.errors.protocol));
            };
            let selected = match self.read(cx, id) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(r)) => return Poll::Ready(Err(r)),
                Poll::Ready(Ok(p)) => p,
            };
            let Some(output) = owner.lower.outcome(selected) else {
                return Poll::Ready(Err(owner.errors.protocol));
            };
            reads.merge(&output.reads);
            Some(output)
        } else {
            None
        };
        let finalizer = if let Some(path) = input
            .representation
            .as_ref()
            .and_then(|r| r.representation.declaration.drop.as_ref())
        {
            let Some(id) = owner.lower.query(path) else {
                return Poll::Ready(Err(owner.errors.protocol));
            };
            let selected = match self.read(cx, id) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(r)) => return Poll::Ready(Err(r)),
                Poll::Ready(Ok(p)) => p,
            };
            let Some(output) = owner.lower.outcome(selected) else {
                return Poll::Ready(Err(owner.errors.protocol));
            };
            reads.merge(&output.reads);
            Some((path.clone(), output.output.value.entry))
        } else {
            None
        };
        let constant = if matches!(&input.signature.kind,SignatureKind::Const{generics,..} if generics.is_empty())
        {
            let Some(id) = owner.lower.evaluate_query(&self.path) else {
                return Poll::Ready(Err(owner.errors.protocol));
            };
            match self.read(cx, id) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(r)) => return Poll::Ready(Err(r)),
                Poll::Ready(Ok(_)) => {}
            }
            let Some(observed) = owner.lower.settled_reads(&self.path, QueryKind::Evaluate) else {
                return Poll::Ready(Err(owner.errors.protocol));
            };
            reads.merge(&observed);
            match owner.lower.settled_constant(&self.path, &[]) {
                Ok(value) => Some((*value).clone()),
                Err(error) => return Poll::Ready(Err(owner.fail(error))),
            }
        } else {
            None
        };
        Poll::Ready((|| {
            let reads = Arc::new(reads.finish().map_err(|_| owner.errors.protocol)?);
            let (source, contracts, requirements) = self
                .complete(&front, &lower, finalizer, constant)
                .map_err(|e| owner.fail(e))?;
            let ordinary = lower.as_ref().map(FrontLowerOutput::native_product);
            let lower_identity = source
                .lower
                .as_ref()
                .map(|v| (v.reference(), v.observation_identity()));
            let bytes = owner
                .records
                .encode(
                    &owner.anchors,
                    &(
                        &source.definition,
                        &source.signature,
                        &source.signature_dependencies,
                        &source.source.syntax,
                        &source.summary,
                        source.summary_identity,
                        (ordinary.as_ref().map(|p| p.body), lower_identity),
                        &source.audit,
                        &source.materialized,
                        source.representation.as_ref().map(|r| {
                            (
                                &r.definition,
                                r.representation_revision,
                                &r.generics,
                                r.private_shape,
                                &r.layout_promises,
                                r.resource,
                                &r.finalizer,
                                r.unsafe_ffi_package,
                            )
                        }),
                        &source.constant,
                        &contracts,
                        &requirements,
                        &*reads,
                    ),
                )
                .ok_or(owner.errors.resource)?;
            if bytes.len() as u64 > owner.weight.scratch {
                return Err(owner.errors.resource);
            }
            let charge = (bytes.len() as u64)
                .checked_mul(2)
                .ok_or(owner.errors.resource)?;
            let mut remaining = owner.remaining.lock().unwrap_or_else(|e| e.into_inner());
            *remaining = remaining.checked_sub(charge).ok_or(owner.errors.resource)?;
            drop(remaining);
            let record = owner
                .store
                .put_content(OUTPUT, &bytes)
                .ok_or(owner.errors.resource)?;
            let action = hirsel_artifact_store::Action(ContentHash::of_parts(
                "hirsel.front.native-definition-action.v1",
                [self.id.address.0.0.as_slice(), record.output.0.0.as_slice()],
            ));
            let product = ProductRef {
                address: self.id.address,
                action,
                record,
            };
            owner
                .completed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    self.id.address.0,
                    (
                        product,
                        Arc::new(Data {
                            product: Arc::new(crate::NativeDiscoveryProduct {
                                product: Some(product),
                                source,
                                contracts,
                                requirements,
                            }),
                            reads,
                            original: ordinary.map(Arc::new),
                        }),
                    ),
                );
            owner
                .store
                .associate_product(self.id.address, action, record);
            Ok(product)
        })())
    }
    fn complete(
        &self,
        front: &FrontBodyOutput,
        lower: &Option<FrontLowerOutput>,
        finalizer: Option<(DefPath, hirsel_tir::BodyId)>,
        constant: Option<hirsel_tir::Materialized>,
    ) -> Result<
        (
            crate::NativeDefinitionProduct,
            Option<crate::NativeBodyContracts>,
            InstanceGraph,
        ),
        QueryError,
    > {
        let owner = &self.owner;
        let input = &owner.inputs[&self.path];
        let analyzed = front.outcome.result.as_ref().map_err(Clone::clone)?;
        let (syntax, signature) = owner
            .front
            .native_input(&self.path)
            .ok_or(QueryError::NotImplemented)?;
        let summary = front
            .outcome
            .summary
            .clone()
            .ok_or(QueryError::NotImplemented)?;
        let representation = input
            .representation
            .clone()
            .map(|record| {
                crate::representation::completed_owner_record(
                    owner.lower.signatures().types(),
                    &self.path,
                    &input.signature,
                    record,
                    finalizer,
                )
            })
            .transpose()?;
        let executable = generation::executable(&syntax)
            && native_declaration(owner.lower.signatures().types(), &input.signature);
        let source = crate::NativeDefinitionProduct {
            definition: self.path.clone(),
            signature: input.signature.clone(),
            signature_dependencies: signature.dependencies.clone(),
            source: crate::NativeSource {
                implementation_revision: syntax.text_hash,
                syntax,
                generated: None,
            },
            summary_identity: summaries::summary_identity(
                owner.lower.signatures().types().clone(),
                &summary.summary,
                &summary.growth,
                summary.checked.as_deref(),
            ),
            summary,
            lower: lower.as_ref().and_then(|o| o.verified.clone()),
            audit: executable.then(|| {
                crate::SourceAudit::completed(
                    owner.lower.signatures().types(),
                    &input.signature,
                    analyzed.value.typed.syntax.text_hash,
                    &analyzed.value,
                    representation.as_ref(),
                )
            }),
            materialized: lower
                .as_ref()
                .map_or_else(Vec::new, |o| o.materialized.clone()),
            representation,
            constant,
        };
        let body = lower.as_ref().map(|o| &*o.output.value);
        let requirements = crate::generic_graph::native_definition(
            owner.lower.signatures().types(),
            &source,
            body,
            &mut |path| {
                if !owner.inputs.contains_key(path) {
                    return Ok(None);
                }
                if let Some((run, root)) = &owner.selection {
                    let selected = run.query(root).and_then(|id| run.completed_product(id));
                    let Some(selected) = selected.and_then(|p| run.outcome(p)) else {
                        return Err(QueryError::NotImplemented);
                    };
                    if !selected.semantic.semantic_contains(path) {
                        return Ok(None);
                    }
                }
                if !native_declaration(
                    owner.lower.signatures().types(),
                    &owner.inputs[path].signature,
                ) {
                    return Ok(None);
                }
                let (syntax, _) = owner
                    .front
                    .native_input(path)
                    .ok_or(QueryError::NotImplemented)?;
                Ok(Some(syntax.owner().site(hirsel_types::NameSite::Item)))
            },
        )?;
        let contracts = body.map(crate::NativeBodyContracts::of).transpose()?;
        Ok((source, contracts, requirements))
    }
}
impl QueryEngine {
    /// Capture only existing ordinary source headers. Generated namespaces must
    /// supply their actual accepted registration view; they are never invented.
    pub fn front_native_definition_queries(
        &mut self,
        definitions: &[DefPath],
        front: Arc<FrontBodyQueries>,
        lower: Arc<FrontLowerQueries>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontNativeDefinitionQueries>, QueryError> {
        let inputs = self.front_native_definition_inputs(definitions, allowance.bytes())?;
        self.front_native_definition_queries_with_inputs(
            front, lower, inputs, weight, allowance, errors,
        )
    }

    pub fn front_native_definition_inputs(
        &mut self,
        definitions: &[DefPath],
        limit: u64,
    ) -> Result<FrontNativeDefinitionInputs, QueryError> {
        let refuse = || {
            QueryError::Operational(hirsel_types::OperationalFailure{operation:"native source admission".into(),message:"native source inputs exceed admission or differ from the pinned source namespace".into()})
        };
        let mut required =
            FrontNativeDefinitionQueries::input_bytes(definitions).ok_or_else(refuse)?;
        let snapshot = self.signature_inputs();
        let original_context = self.native_original_context(definitions)?;
        let store = self
            .result_store
            .clone()
            .filter(|s| s.enabled())
            .ok_or_else(refuse)?;
        let records = crate::NativeRecords::new(self.types.clone(), store.clone());
        let anchors = crate::NativeRecordContext::capture(self);
        let mut inputs = BTreeMap::new();
        let mut ids = BTreeMap::new();
        let namespace = serde_json::to_vec(definitions).map_err(|_| refuse())?;
        for path in definitions {
            if self.generated.contains_key(path) {
                return Err(native_selection_error(format!(
                    "generated native catalogue input: {path}"
                )));
            }
            let signature = snapshot
                .signature(DefId::intern(path))
                .ok_or_else(refuse)?
                .map_err(Clone::clone)?
                .clone();
            let representation = self.owner_record(path);
            if let Some(record) = &representation {
                let bytes = records
                    .encode(
                        &anchors,
                        &(
                            &record.representation.declaration,
                            &record.dependencies,
                            record.text_hash,
                        ),
                    )
                    .ok_or_else(refuse)?;
                required = required
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(refuse)?;
                if self.implementation_hash(path) != Some(record.text_hash) {
                    return Err(native_selection_error(format!(
                        "native representation revision differs from source: {path}"
                    )));
                }
            }
            let bytes = serde_json::to_vec(path).map_err(|_| refuse())?;
            let id = QueryId {
                kind: FRONT_NATIVE_DEFINITION_KIND,
                address: hirsel_artifact_store::Address(ContentHash::of_parts(
                    "hirsel.front.native-definition-address.v1",
                    [
                        self.environment_value().0.as_slice(),
                        namespace.as_slice(),
                        bytes.as_slice(),
                    ],
                )),
            };
            ids.insert(path.clone(), id);
            inputs.insert(
                path.clone(),
                Input {
                    signature,
                    representation,
                },
            );
        }
        if required > limit {
            return Err(native_selection_error(format!(
                "native catalogue inputs exceed admission: required {required}, reserved {}",
                limit
            )));
        }
        Ok(FrontNativeDefinitionInputs {
            definitions: definitions.to_vec(),
            snapshot,
            original_context,
            inputs,
            ids,
            records,
            anchors,
            store,
            required,
        })
    }

    pub fn front_native_definition_queries_with_inputs(
        &mut self,
        front: Arc<FrontBodyQueries>,
        lower: Arc<FrontLowerQueries>,
        inputs: FrontNativeDefinitionInputs,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontNativeDefinitionQueries>, QueryError> {
        if !Arc::ptr_eq(&inputs.snapshot, &self.signature_inputs())
            || !Arc::ptr_eq(&inputs.snapshot, &lower.signatures())
        {
            return Err(native_selection_error(
                "native catalogue and Lower signature snapshots differ",
            ));
        }
        if inputs
            .definitions
            .iter()
            .any(|path| front.query(path).is_none() || lower.query(path).is_none())
        {
            return Err(native_selection_error(
                "native catalogue is outside front/Lower namespace",
            ));
        }
        if inputs.required > allowance.bytes() {
            return Err(native_selection_error(
                "native catalogue inputs exceed original admission",
            ));
        }
        let FrontNativeDefinitionInputs {
            definitions: _,
            snapshot: _,
            original_context,
            inputs,
            ids,
            records,
            anchors,
            store,
            required,
        } = inputs;
        let output_budget = allowance.bytes() - required;
        let ownership = Arc::new(SourceOwnership {
            _front: front.clone(),
            _lower: lower.clone(),
            _allowance: allowance,
        });
        let original_context = Arc::new(original_context.retain_owner(ownership.clone()));
        Ok(Arc::new(FrontNativeDefinitionQueries {
            selection: None,
            inputs,
            ids,
            original_context,
            front,
            lower,
            records,
            anchors,
            store,
            completed: Mutex::new(BTreeMap::new()),
            failures: Mutex::new(BTreeMap::new()),
            remaining: Mutex::new(output_budget),
            weight,
            errors,
            _ownership: ownership,
        }))
    }
}

fn native_selection_error(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "native source selection".into(),
        message: message.into(),
    })
}
pub(crate) fn native_declaration(
    types: &hirsel_types::TypeInterner,
    signature: &SignatureRef,
) -> bool {
    !matches!(&signature.kind,SignatureKind::Function(function) if function.generics.is_empty() && function.parameters.iter().map(|p|p.ty).chain([function.result]).any(|ty|hirsel_types::mentions_compile_time_only(types,ty)))
}
