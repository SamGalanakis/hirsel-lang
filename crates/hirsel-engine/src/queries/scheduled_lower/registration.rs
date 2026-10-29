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
    schema: 2,
};
const FAILURE: Domain = Domain {
    kind: 0x4601_0007,
    schema: 2,
};
struct Input {
    key: EntryKey,
    definition: DefId,
    syntax: Reference,
    implementation: ContentHash,
    site: Span,
    instance_reads: Reads,
    checked_prefix: Vec<SignatureDependency>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    output: Reference,
    decode_bytes: u64,
}
struct Data {
    text: Result<String, QueryError>,
    reads: Reads,
    dependencies: Vec<SignatureDependency>,
    depth: DepthEvidence<Span>,
    acceptance: Option<hirsel_validate::ExecutionReceipt>,
    _allowance: Option<MemoryReservation>,
}
type Completed = (ProductRef, Arc<Data>);
/// Opaque text/evidence transport, without namespace or execution authority.
#[derive(Clone)]
pub struct FrontRegistrationOutput {
    definition: DefId,
    data: Arc<Data>,
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
    pub fn dependencies(&self) -> &[SignatureDependency] {
        &self.data.dependencies
    }
    pub fn depth(&self) -> &DepthEvidence<Span> {
        &self.data.depth
    }
    pub(in crate::queries) fn acceptance(&self) -> Option<&hirsel_validate::ExecutionReceipt> {
        self.data.acceptance.as_ref()
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
    remaining: Mutex<MemoryReservation>,
    total: u64,
    _inputs: MemoryReservation,
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
        self.total
            - self
                .remaining
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .bytes()
    }
    pub fn outcome(self: &Arc<Self>, product: ProductRef) -> Option<FrontRegistrationOutput> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, data) = outcomes.get(&product.address.0)?;
        (*selected == product).then(|| FrontRegistrationOutput {
            definition: self.inputs[&product.address.0].definition,
            data: data.clone(),
        })
    }
    pub fn failed(self: &Arc<Self>, reference: Reference) -> Option<FrontRegistrationOutput> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (address, (_, data)) = outcomes.iter().find(|(_, (p, _))| p.record == reference)?;
        Some(FrontRegistrationOutput {
            definition: self.inputs[address].definition,
            data: data.clone(),
        })
    }
    async fn run(self: Arc<Self>, id: QueryId, reads: QueryReads) -> Result<ProductRef, Reference> {
        let input = &self.inputs[&id.address.0];
        let mut data = if let Some(data) = self.restore(input) {
            Arc::new(data)
        } else {
            let mut source = Source {
                owner: self.lower.clone(),
                reads,
                observed: Rc::new(RefCell::new(ReadsBuilder::new())),
                selections: Rc::new(RefCell::new(Vec::new())),
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
            source.merge(&input.instance_reads);
            let mut dependencies = Vec::new();
            let text = if source.depth.borrow().limit() == 0 {
                Err(source.overflow(input.site))
            } else {
                self.compute(input, &mut source, &mut dependencies).await
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
            Arc::new(Data {
                text,
                reads: observed,
                dependencies,
                depth,
                acceptance,
                _allowance: None,
            })
        };
        let (bytes, record_decode_bytes) = self
            .lower
            .records
            .encode_with_decode_bytes(
                &self.lower.anchors,
                &(
                    input.definition,
                    &data.text,
                    &data.reads,
                    &data.depth,
                    &data.dependencies,
                ),
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
        weight.flat_vec(&data.dependencies);
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
        let allowance = remaining
            .split(weight.bytes())
            .ok_or(self.errors.resource)?;
        Arc::get_mut(&mut data)
            .expect("unpublished generator output")
            ._allowance = Some(allowance);
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
        if !failed
            && let Some(decode_bytes) = weight
                .bytes()
                .checked_mul(2)
                .and_then(|bytes| bytes.checked_add(record_decode_bytes))
        {
            self.remember(input, record, &data, decode_bytes);
        }
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.address.0, (product, data));
        if failed { Err(record) } else { Ok(product) }
    }
    fn storage_key(input: &Input) -> EntryKey {
        EntryKey(
            ContentHash::of_parts(
                "hirsel.front.registration.variants.v1",
                [input.key.0.0.as_slice()],
            ),
            input.key.1,
            input.key.2,
        )
    }
    fn observe(&self, input: &Input, read: &ReadKey) -> Option<ContentHash> {
        if *read == ReadKey::ExecutedBody(input.definition) {
            Some(memos::optional(Some(input.implementation)))
        } else {
            self.lower.observe(input.definition.path(), read)
        }
    }
    fn restore(&self, input: &Input) -> Option<Data> {
        use crate::store::Entry;
        if self.lower.verify != Verify::On || !self.lower.store.query_cache_enabled() {
            return None;
        }
        let key = Self::storage_key(input);
        let variant = Entry::load(&*self.lower.store, &key).resolve_current(
            &*self.lower.store,
            &key,
            |read| self.observe(input, read),
        )?;
        let stored: Stored = bincode::deserialize(&variant.payload).ok()?;
        if stored.output.domain != OUTPUT
            || stored.output.len > self.weight.scratch
            || stored.decode_bytes > self.weight.scratch
        {
            return None;
        }
        let acceptance = if self.lower.verify == Verify::On {
            Some(self.lower.store.verifier()?.execution(
                persistent::evaluation_subject(&key, &variant.payload),
                |package| self.lower.snapshot.validation_input_digest(package),
            )?)
        } else {
            None
        };
        let bytes = self.lower.store.get_content(stored.output)?;
        let (definition, text, reads, depth, dependencies): (
            DefId,
            Result<String, QueryError>,
            Reads,
            DepthEvidence<Span>,
            Vec<SignatureDependency>,
        ) = self.lower.records.decode(&self.lower.anchors, &bytes)?;
        if definition != input.definition
            || text.is_err()
            || reads
                .first_changed(|read| self.observe(input, read))
                .is_some()
            || depth.bounded
            || depth.first.len() != dependency_limit(self.lower.limits.call_depth) as usize + 1
            || depth
                .first
                .iter()
                .flatten()
                .any(|site| self.lower.sources.slice(*site).is_none())
        {
            return None;
        }
        let persisted = persistent::persisted_execution_reads(&reads, acceptance.as_ref())?;
        if persisted != variant.reads {
            return None;
        }
        Some(Data {
            text,
            reads,
            dependencies,
            depth,
            acceptance,
            _allowance: None,
        })
    }
    fn remember(&self, input: &Input, output: Reference, data: &Data, decode_bytes: u64) {
        use crate::store::{Entry, VariantRef};
        if !self.lower.store.query_cache_enabled() || data.acceptance.is_none() {
            return;
        }
        let Some(reads) =
            persistent::persisted_execution_reads(&data.reads, data.acceptance.as_ref())
        else {
            return;
        };
        let key = Self::storage_key(input);
        let Ok(payload) = bincode::serialize(&Stored {
            output,
            decode_bytes,
        }) else {
            return;
        };
        // The local execution association binds both the checked output
        // reference and its measured decode bound before any restoration.
        if let Some(receipt) = &data.acceptance
            && let Some(cache) = self.lower.store.verifier()
        {
            cache.publish_execution(persistent::evaluation_subject(&key, &payload), receipt);
        }
        let Some(variant) = VariantRef::publish(&*self.lower.store, &key, reads, &payload) else {
            return;
        };
        let mut entry = Entry::load(&*self.lower.store, &key);
        entry.insert(variant);
        self.lower
            .store
            .put_query(key.2, &key, entry.action(&key), &entry.encode_for(&key));
    }
    async fn compute(
        &self,
        input: &Input,
        source: &mut Source,
        dependencies: &mut Vec<SignatureDependency>,
    ) -> Result<String, QueryError> {
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
        let prepared = self.lower.front.prepare_generator(
            Arc::new(syntax),
            &*self.signer,
            !input.instance_reads.is_empty(),
        );
        source.merge(&prepared.reads);
        let PreparedGenerator {
            input: generator,
            checked,
            detail,
            ..
        } = prepared.result?;
        let checked = checked?;
        dependencies.extend_from_slice(&input.checked_prefix);
        dependencies.extend_from_slice(&detail.signatures);
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
        mut allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontRegistrationQueries>, QueryError> {
        let refused = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
            operation: "generator registration admission".into(), message: "generator input inventory exceeds admission or differs from its source snapshot".into(),
        })
        };
        let mut required = FrontRegistrationQueries::input_bytes(definitions);
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
            // Generator declarations are absent from the ordinary-body front.
            // Capture their original header requests here, before workers run.
            let signature = self.signature_output(definition)?;
            let prefix_len = signature
                .dependencies
                .len()
                .checked_add(1)
                .ok_or_else(refused)?;
            required = required
                .checked_add(
                    prefix_len
                        .checked_mul(size_of::<SignatureDependency>())
                        .ok_or_else(refused)? as u64,
                )
                .ok_or_else(refused)?;
            if required > allowance.bytes() {
                return Err(refused());
            }
            let mut checked_prefix = Vec::with_capacity(prefix_len);
            checked_prefix.extend_from_slice(&signature.dependencies);
            checked_prefix.push(SignatureDependency {
                definition: DefId::intern(definition),
                fingerprint: signature.value.fingerprint,
            });
            let instance_reads = self.front_instance_reads(definition)?;
            for (_, bytes) in instance_reads.allocations() {
                required = required.checked_add(bytes as u64).ok_or_else(refused)?;
            }
            if required > allowance.bytes() {
                return Err(refused());
            }
            inputs.insert(
                key.0,
                Input {
                    key,
                    definition: DefId::intern(definition),
                    syntax: archived,
                    implementation: syntax.text_hash,
                    site: syntax.item.name_span,
                    instance_reads,
                    checked_prefix,
                },
            );
        }
        let total = allowance.bytes();
        let inputs_allowance = allowance.split(required).ok_or_else(refused)?;
        Ok(Arc::new(FrontRegistrationQueries {
            inputs,
            lower,
            signer,
            outcomes: Mutex::new(BTreeMap::new()),
            weight,
            errors,
            remaining: Mutex::new(allowance),
            total,
            _inputs: inputs_allowance,
        }))
    }
}
