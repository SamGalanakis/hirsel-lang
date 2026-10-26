//! Private generator execution on an admitted home worker. Text remains a
//! tentative product: only the ordered source transaction may publish syntax.
use super::*;
use crate::queries::generation::task::PreparedGenerator;

pub const FRONT_REGISTRATION_KIND: u32 = 0x4601_0004;
const INPUT: Domain = Domain {
    kind: 0x4601_0005,
    schema: 1,
};
const OUTPUT: Domain = Domain {
    kind: 0x4601_0006,
    schema: 1,
};
const FAILURE: Domain = Domain {
    kind: 0x4601_0007,
    schema: 1,
};
struct Input {
    definition: DefId,
    syntax: Reference,
    implementation: ContentHash,
    site: Span,
}
struct Data {
    text: Result<String, QueryError>,
    reads: Reads,
    depth: DepthEvidence<Span>,
    acceptance: Option<hirsel_validate::ExecutionReceipt>,
}
type Completed = (ProductRef, Arc<Data>);
/// Opaque text/evidence transport, without namespace or execution authority.
#[derive(Clone)]
pub struct FrontRegistrationOutput {
    definition: DefId,
    data: Arc<Data>,
    _owner: Arc<FrontRegistrationQueries>,
}
impl FrontRegistrationOutput {
    pub fn definition(&self) -> &DefPath {
        self.definition.path()
    }
    pub fn text(&self) -> Result<&str, QueryError> {
        self.data.text.as_deref().map_err(Clone::clone)
    }
    pub fn reads(&self) -> &Reads {
        &self.data.reads
    }
    pub fn depth(&self) -> &DepthEvidence<Span> {
        &self.data.depth
    }
    pub fn has_execution_acceptance(&self) -> bool {
        self.data.acceptance.is_some()
    }
}
pub struct FrontRegistrationQueries {
    inputs: BTreeMap<ContentHash, Input>,
    lower: Arc<FrontLowerQueries>,
    signer: Arc<dyn SignaturePhase>,
    outcomes: Mutex<BTreeMap<ContentHash, Completed>>,
    weight: MemoryWeight,
    errors: RuntimeErrors,
    remaining: Mutex<u64>,
    allowance: MemoryReservation,
}
impl FrontRegistrationQueries {
    pub fn input_bytes(definitions: &[DefPath]) -> u64 {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(size_of::<Self>() + 2 * size_of::<usize>());
        weight.tree::<ContentHash, Input>(definitions.len());
        weight.tree::<ContentHash, Completed>(definitions.len());
        weight.bytes()
    }
    pub fn query(&self, path: &DefPath) -> Option<QueryId> {
        self.inputs.iter().find_map(|(address, input)| {
            (input.definition.path() == path).then_some(QueryId {
                kind: FRONT_REGISTRATION_KIND,
                address: hirsel_artifact_store::Address(*address),
            })
        })
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn LocalQueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn retained_bytes(&self) -> u64 {
        self.allowance.bytes() - *self.remaining.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn outcome(self: &Arc<Self>, product: ProductRef) -> Option<FrontRegistrationOutput> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, data) = outcomes.get(&product.address.0)?;
        (*selected == product).then(|| FrontRegistrationOutput {
            definition: self.inputs[&product.address.0].definition,
            data: data.clone(),
            _owner: self.clone(),
        })
    }
    pub fn failed(self: &Arc<Self>, reference: Reference) -> Option<FrontRegistrationOutput> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (address, (_, data)) = outcomes.iter().find(|(_, (p, _))| p.record == reference)?;
        Some(FrontRegistrationOutput {
            definition: self.inputs[address].definition,
            data: data.clone(),
            _owner: self.clone(),
        })
    }
    async fn run(self: Arc<Self>, id: QueryId, reads: QueryReads) -> Result<ProductRef, Reference> {
        let input = &self.inputs[&id.address.0];
        let mut source = Source {
            owner: self.lower.clone(),
            reads,
            observed: Rc::new(RefCell::new(ReadsBuilder::new())),
            refusal: Rc::new(Cell::new(None)),
            depth: Rc::new(RefCell::new(DepthEvidence::leaf(dependency_limit(
                self.lower.limits.call_depth,
            )))),
            framed: true,
            evidence: Rc::new(RefCell::new(Evidence::new(
                self.lower.verify,
                self.weight.retained,
            ))),
        };
        source.record(
            ReadKey::ExecutedBody(input.definition),
            memos::optional(Some(input.implementation)),
        );
        let text = if source.depth.borrow().limit() == 0 {
            Err(source.overflow(input.site))
        } else {
            self.compute(input, &mut source).await
        };
        if let Some(reference) = source.refusal.get() {
            return Err(reference);
        }
        let observed = std::mem::take(&mut *source.observed.borrow_mut())
            .finish()
            .map_err(|_| self.errors.protocol)?;
        let acceptance = text
            .as_ref()
            .ok()
            .and_then(|_| source.evidence.borrow().finish(&observed));
        let mut depth = source.depth.borrow().clone();
        depth.first[0] = None;
        let data = Arc::new(Data {
            text,
            reads: observed,
            depth,
            acceptance,
        });
        let bytes = self
            .lower
            .records
            .encode(
                &self.lower.anchors,
                &(input.definition, &data.text, &data.reads, &data.depth),
            )
            .ok_or(self.errors.resource)?;
        if bytes.len() as u64 > self.weight.scratch {
            return Err(self.errors.resource);
        }
        let mut weight = crate::NativeMetadataWeight::default();
        weight.arc(&data);
        match &data.text {
            Ok(text) => weight.add(text.capacity()),
            Err(error) => weight.add(
                crate::memo::retained::retained_query_error_bytes(error)
                    .try_into()
                    .map_err(|_| self.errors.resource)?,
            ),
        }
        for (_, bytes) in data.reads.allocations() {
            weight.add(bytes);
        }
        weight.add(size_of_val(&*data.depth.first));
        if let Some(acceptance) = &data.acceptance {
            weight.add(
                acceptance
                    .retained_bytes()
                    .ok_or(self.errors.resource)?
                    .try_into()
                    .map_err(|_| self.errors.resource)?,
            );
        }
        let mut remaining = self.remaining.lock().unwrap_or_else(|e| e.into_inner());
        *remaining = remaining
            .checked_sub(weight.bytes())
            .ok_or(self.errors.resource)?;
        drop(remaining);
        let failed = data.text.is_err();
        let record = self
            .lower
            .store
            .put_content(if failed { FAILURE } else { OUTPUT }, &bytes)
            .ok_or(self.errors.resource)?;
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.registration.action.v1",
            [
                id.address.0.0.as_slice(),
                data.reads.digest().0.as_slice(),
                record.output.0.0.as_slice(),
            ],
        ));
        let product = ProductRef {
            address: id.address,
            action,
            record,
        };
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.address.0, (product, data));
        if failed { Err(record) } else { Ok(product) }
    }
    async fn compute(&self, input: &Input, source: &mut Source) -> Result<String, QueryError> {
        let bytes = self
            .lower
            .store
            .get_content(input.syntax)
            .ok_or(QueryError::NotImplemented)?;
        let (mut syntax, occurrence): (ItemSyntax, u32) = self
            .lower
            .records
            .decode(&self.lower.anchors, &bytes)
            .ok_or(QueryError::NotImplemented)?;
        syntax.occurrence = occurrence;
        if syntax.item.definition != *input.definition.path()
            || syntax.text_hash != input.implementation
            || !syntax.structurally_valid(|span| self.lower.sources.slice(span).is_some())
        {
            return Err(QueryError::NotImplemented);
        }
        if crate::queries::generation::instance_bodied(&syntax) {
            return Err(QueryError::NotImplemented);
        }
        let prepared = self
            .lower
            .front
            .prepare_generator(Arc::new(syntax), &*self.signer);
        source.merge(&prepared.reads);
        let PreparedGenerator {
            input: generator,
            checked,
            detail,
            ..
        } = prepared.result?;
        let checked = checked?;
        source.execution_signatures(detail.signatures.iter());
        let mut observed = ReadsBuilder::new();
        let staged = crate::comptime::prepare_staged_types(
            self.lower.snapshot.types(),
            &checked.analyzed,
            &mut FrozenProvider::new(
                &self.lower.snapshot,
                input.definition.path().0.package.clone(),
                &mut observed,
            ),
        );
        source.merge(&observed.finish().map_err(|_| QueryError::NotImplemented)?);
        let staged = staged?;
        let mut values = Values {
            source: source.clone(),
            pending: None,
        };
        let mut preparation = ValuePreparation::new(&checked.analyzed);
        let constants = poll_fn(|_| {
            preparation
                .poll(
                    self.lower.snapshot.types(),
                    &checked.analyzed,
                    &staged,
                    &checked.signature,
                    &mut values,
                )
                .map(|r| r.map(|v| v.to_vec()))
        })
        .await?;
        let value = generator.value;
        let site = checked.analyzed.typed.syntax.tree.expressions[value.0 as usize]
            .origin
            .span;
        let lowered = EvaluationInput {
            analyzed: &checked.analyzed,
            staged: &staged,
            signature: &checked.signature,
            expression: value,
            child: value,
            ty: checked.ty,
            constants: &constants,
            sizes: &[],
        }
        .lower(
            self.lower.snapshot.types(),
            &self.lower.memo,
            &*self.lower.lowering,
        )?;
        let admission = s1::ValidatedEvaluation::recorded(
            lowered,
            &self.lower.snapshot,
            self.lower.verify,
            site,
        );
        source.merge(&admission.reads);
        source.evidence.borrow_mut().executed = true;
        let runtime = (*self.lower.runtime).clone();
        let result = admission
            .result?
            .run_async(&self.lower.sources, self.lower.limits, &runtime, source)
            .await;
        source.merge(&result.value.reads);
        source.evidence.borrow_mut().accepted(result.acceptance);
        generator.decode(result.value.result?.0)
    }
}
struct Factory(Arc<FrontRegistrationQueries>);
impl LocalQueryFactory for Factory {
    fn weight(&self, id: QueryId) -> Result<MemoryWeight, Reference> {
        if id.kind != FRONT_REGISTRATION_KIND || !self.0.inputs.contains_key(&id.address.0) {
            return Err(self.0.errors.protocol);
        }
        Ok(self.0.weight)
    }
    fn create(&self, id: QueryId) -> Result<Box<dyn LocalQueryTask>, Reference> {
        let owner = self.0.clone();
        Ok(Box::new(LowerTask(LocalFutureTask::new(
            id,
            self.weight(id)?,
            self.0.lower.max_reads,
            self.0.errors,
            move |reads| owner.run(id, reads),
        )?)))
    }
}
impl QueryEngine {
    /// Capture generator declarations only. Running a query evaluates privately;
    /// its output cannot install source until the source transaction accepts it.
    pub fn front_registration_queries(
        &mut self,
        definitions: &[DefPath],
        lower: Arc<FrontLowerQueries>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontRegistrationQueries>, QueryError> {
        let refused = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
            operation: "generator registration admission".into(), message: "generator input inventory exceeds admission or differs from its source snapshot".into(),
        })
        };
        let required = FrontRegistrationQueries::input_bytes(definitions);
        if required > allowance.bytes() || !Arc::ptr_eq(&self.types, lower.snapshot.types()) {
            return Err(refused());
        }
        let signer = self.adapters.signature.clone().ok_or_else(refused)?;
        let mut inputs = BTreeMap::new();
        for definition in definitions {
            self.materialize(definition)?;
            let syntax = self.snapshot(definition)?;
            if crate::queries::generation::generator(&syntax).is_none() {
                continue;
            }
            let key = self
                .store_key(definition, QueryKind::Generate)
                .ok_or_else(refused)?;
            let bytes = lower
                .records
                .encode(&lower.anchors, &(&syntax, syntax.occurrence))
                .ok_or_else(refused)?;
            let archived = lower.store.put_content(INPUT, &bytes).ok_or_else(refused)?;
            inputs.insert(
                key.0,
                Input {
                    definition: DefId::intern(definition),
                    syntax: archived,
                    implementation: syntax.text_hash,
                    site: syntax.item.name_span,
                },
            );
        }
        Ok(Arc::new(FrontRegistrationQueries {
            inputs,
            lower,
            signer,
            outcomes: Mutex::new(BTreeMap::new()),
            weight,
            errors,
            remaining: Mutex::new(allowance.bytes() - required),
            allowance,
        }))
    }
}
