//! Immutable staged declarations derived from one settled ordinary P result.
//! These are private construction products, not accepted namespace publication.
//! The ordered source transaction decides which declarations become visible.
use super::*;
use crate::comptime::{StagedDeclarationInput, StagedRegistration};
use crate::runtime::*;
use crate::store::{Domain, Reference, ResultStore};
use std::{sync::Mutex, task::Poll};

pub const FRONT_STAGE_KIND: u32 = 0x4601_0001;
const OUTPUT: Domain = Domain {
    kind: 0x4601_0002,
    schema: 1,
};
const FAILURE: Domain = Domain {
    kind: 0x4601_0003,
    schema: 1,
};
struct Input {
    owner: DefPath,
    front: QueryId,
}
struct Data {
    result: Result<Vec<StagedRegistration>, QueryError>,
    reads: Arc<Reads>,
}
type Completed = (Result<ProductRef, Reference>, Arc<Data>);
/// Clones retain the original source and declaration admission. A declaration
/// is data until the source transaction accepts its owner and complete reads.
#[derive(Clone)]
pub struct FrontStageOutput {
    query: QueryId,
    data: Arc<Data>,
    owner: Arc<FrontStageQueries>,
}
impl FrontStageOutput {
    pub fn owner(&self) -> &DefPath {
        &self.owner.inputs[&self.query.address.0].owner
    }
    pub fn reads(&self) -> &Arc<Reads> {
        &self.data.reads
    }
    pub fn installation_bytes(&self) -> Result<u64, QueryError> {
        let registrations = self.data.result.as_ref().map_err(Clone::clone)?;
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(
            crate::memo::retained::retained_staged_declaration_bytes(registrations)
                .try_into()
                .map_err(|_| QueryError::NotImplemented)?,
        );
        weight.tree::<DefPath, crate::comptime::StagedConstant>(registrations.len());
        Ok(weight.bytes())
    }
    pub fn declarations(&self) -> Result<impl Iterator<Item = (&DefPath, &Signature)>, QueryError> {
        let registrations = self.data.result.as_ref().map_err(Clone::clone)?;
        Ok(registrations
            .iter()
            .map(|r| (&r.path, &*r.constant.signature)))
    }
}
/// Existing immutable front owners; no copied anchors, codec or signature set.
pub(crate) struct StageSource {
    pub(crate) snapshot: Arc<SignatureSnapshot>,
    pub(crate) memo: Arc<hirsel_types::TypeMemo>,
    pub(crate) records: crate::NativeRecords,
    pub(crate) anchors: Arc<crate::NativeRecordContext>,
    pub(crate) store: Arc<dyn ResultStore>,
}
pub struct FrontStageQueries {
    inputs: BTreeMap<ContentHash, Input>,
    ids: BTreeMap<DefPath, QueryId>,
    front: Arc<FrontBodyQueries>,
    snapshot: Arc<SignatureSnapshot>,
    memo: Arc<hirsel_types::TypeMemo>,
    records: crate::NativeRecords,
    anchors: Arc<crate::NativeRecordContext>,
    store: Arc<dyn ResultStore>,
    outcomes: Mutex<BTreeMap<ContentHash, Completed>>,
    remaining: Mutex<u64>,
    weight: MemoryWeight,
    errors: RuntimeErrors,
    allowance: MemoryReservation,
}
impl FrontStageQueries {
    /// Fixed query/owner rows; output allocations are debited only on demand.
    pub fn input_bytes(definitions: &[DefPath]) -> u64 {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(size_of::<Self>() + 2 * size_of::<usize>());
        weight.tree::<ContentHash, Input>(definitions.len());
        weight.tree::<DefPath, QueryId>(definitions.len());
        for path in definitions {
            weight.path(path);
            weight.path(path);
        }
        weight.bytes()
    }
    pub fn query(&self, path: &DefPath) -> Option<QueryId> {
        self.ids.get(path).copied()
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn retained_bytes(&self) -> u64 {
        self.allowance.bytes() - *self.remaining.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn outcome(self: &Arc<Self>, product: ProductRef) -> Option<FrontStageOutput> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, data) = outcomes.get(&product.address.0)?;
        (*selected == Ok(product)).then(|| FrontStageOutput {
            query: QueryId {
                kind: FRONT_STAGE_KIND,
                address: product.address,
            },
            data: data.clone(),
            owner: self.clone(),
        })
    }
    pub fn failure(&self, reference: Reference) -> Option<QueryError> {
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .find(|(selected, _)| *selected == Err(reference))
            .and_then(|(_, data)| data.result.as_ref().err().cloned())
            .or_else(|| {
                self.front
                    .outcome(reference)
                    .and_then(|o| o.outcome.result.err())
            })
    }
    fn declarations(
        &self,
        owner: &DefPath,
        analyzed: &AnalyzedBody,
    ) -> Result<Vec<StagedRegistration>, QueryError> {
        let signature = self
            .snapshot
            .signature(DefId::intern(owner))
            .ok_or(QueryError::NotImplemented)?
            .map_err(Clone::clone)?;
        let typed = &analyzed.typed;
        let mut registrations = Vec::new();
        for row in &typed.checked.expressions {
            if !matches!(
                typed.syntax.tree.expressions[row.expression.0 as usize].kind,
                hirsel_syntax::ExprKind::Comptime(_)
            ) {
                continue;
            }
            let inputs = hirsel_check_protocol::body::comptime_inputs(
                self.snapshot.types(),
                &typed.syntax,
                &typed.checked,
                row.expression,
            );
            if inputs.is_empty() {
                continue;
            }
            registrations.push(
                StagedDeclarationInput {
                    analyzed,
                    expression: row.expression,
                    ty: row.ty,
                    inputs,
                }
                .prepare(self.snapshot.types(), &self.memo, signature)?,
            );
        }
        Ok(registrations)
    }
    fn complete(&self, id: QueryId, data: Data) -> Result<ProductRef, Reference> {
        let bytes = self
            .records
            .encode(&*self.anchors, &(&data.result, &*data.reads))
            .ok_or(self.errors.resource)?;
        if bytes.len() as u64 > self.weight.scratch {
            return Err(self.errors.resource);
        }
        let mut charge = crate::memo::retained_arc_bytes(&data);
        charge = charge
            .checked_add(match &data.result {
                Ok(registrations) => {
                    crate::memo::retained::retained_staged_declaration_bytes(registrations)
                }
                Err(error) => crate::memo::retained::retained_query_error_bytes(error),
            })
            .ok_or(self.errors.resource)?;
        let mut remaining = self.remaining.lock().unwrap_or_else(|e| e.into_inner());
        let mut outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((result, _)) = outcomes.get(&id.address.0) {
            return *result;
        }
        let mut before = crate::NativeMetadataWeight::default();
        let mut after = crate::NativeMetadataWeight::default();
        if !outcomes.is_empty() {
            before.tree::<ContentHash, Completed>(outcomes.len());
        }
        after.tree::<ContentHash, Completed>(outcomes.len() + 1);
        charge = charge
            .checked_add(
                after
                    .bytes()
                    .checked_sub(before.bytes())
                    .ok_or(self.errors.resource)?,
            )
            .ok_or(self.errors.resource)?;
        let next = remaining.checked_sub(charge).ok_or(self.errors.resource)?;
        let failed = data.result.is_err();
        let record = self
            .store
            .put_content(if failed { FAILURE } else { OUTPUT }, &bytes)
            .ok_or(self.errors.resource)?;
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.stage.action.v1",
            [id.address.0.0.as_slice(), record.output.0.0.as_slice()],
        ));
        let product = ProductRef {
            address: id.address,
            action,
            record,
        };
        let result = if failed { Err(record) } else { Ok(product) };
        outcomes.insert(id.address.0, (result, Arc::new(data)));
        *remaining = next;
        drop(outcomes);
        drop(remaining);
        if !failed {
            self.store.associate_product(id.address, action, record);
        }
        result
    }
}
struct Factory(Arc<FrontStageQueries>);
impl QueryFactory for Factory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if id.kind != FRONT_STAGE_KIND || !self.0.inputs.contains_key(&id.address.0) {
            return Err(self.0.errors.protocol);
        }
        let terminal = self
            .0
            .outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id.address.0)
            .map(|(result, _)| *result);
        Ok(Box::new(Task {
            queries: self.0.clone(),
            id,
            terminal,
        }))
    }
}
struct Task {
    queries: Arc<FrontStageQueries>,
    id: QueryId,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        self.queries.weight
    }
    fn continuation_bytes(&self) -> u64 {
        self.queries.weight.retained
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return finish(result);
        }
        let input = &self.queries.inputs[&self.id.address.0];
        let selected = match cx.read(input.front) {
            Poll::Pending => return QueryPoll::Waiting(vec![input.front]),
            Poll::Ready(Err(error)) => {
                self.terminal = Some(Err(error));
                return QueryPoll::Failed(error);
            }
            Poll::Ready(Ok(product)) => product,
        };
        let front = match self.queries.front.outcome(selected.record) {
            Some(front) => front,
            None => {
                let Some(body) = self.queries.front.body_query(&input.owner) else {
                    return QueryPoll::Failed(self.queries.errors.protocol);
                };
                let selected = match cx.read(body) {
                    Poll::Pending => return QueryPoll::Waiting(vec![body]),
                    Poll::Ready(Err(error)) => {
                        self.terminal = Some(Err(error));
                        return QueryPoll::Failed(error);
                    }
                    Poll::Ready(Ok(product)) => product,
                };
                let Some(front) = self.queries.front.outcome(selected.record) else {
                    return QueryPoll::Failed(self.queries.errors.protocol);
                };
                front
            }
        };
        let result = front.outcome.result.and_then(|analyzed| {
            if has_errors(&analyzed.diagnostics) {
                return Err(QueryError::Diagnostics(analyzed.diagnostics.clone()));
            }
            self.queries.declarations(&input.owner, &analyzed.value)
        });
        let result = self.queries.complete(
            self.id,
            Data {
                result,
                reads: front.outcome.reads,
            },
        );
        self.terminal = Some(result);
        finish(result)
    }
}
fn finish(result: Result<ProductRef, Reference>) -> QueryPoll {
    match result {
        Ok(product) => QueryPoll::Complete(product),
        Err(error) => QueryPoll::Failed(error),
    }
}
impl FrontStageQueries {
    /// Capture only immutable P handles. Construction performs no body work,
    /// registration publication, generator execution or QueryEngine access.
    pub fn new(
        definitions: &[DefPath],
        front: Arc<FrontBodyQueries>,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<Self>, QueryError> {
        let refused = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "staged declaration admission".into(),
                message: "source inputs exceed admission or are unavailable".into(),
            })
        };
        let required = Self::input_bytes(definitions);
        let remaining = allowance
            .bytes()
            .checked_sub(required)
            .ok_or_else(refused)?;
        let mut inputs = BTreeMap::new();
        let mut ids = BTreeMap::new();
        for owner in definitions {
            let selected = front.query(owner).ok_or_else(refused)?;
            let query = QueryId {
                kind: FRONT_STAGE_KIND,
                address: hirsel_artifact_store::Address(ContentHash::of_parts(
                    "hirsel.front.stage.address.v1",
                    [selected.address.0.0.as_slice()],
                )),
            };
            ids.insert(owner.clone(), query);
            inputs.insert(
                query.address.0,
                Input {
                    owner: owner.clone(),
                    front: selected,
                },
            );
        }
        let StageSource {
            snapshot,
            memo,
            records,
            anchors,
            store,
        } = front.stage_source();
        Ok(Arc::new(Self {
            snapshot,
            memo,
            records,
            anchors,
            store,
            front,
            inputs,
            ids,
            outcomes: Mutex::new(BTreeMap::new()),
            remaining: Mutex::new(remaining),
            weight,
            errors,
            allowance,
        }))
    }
}

impl QueryEngine {
    /// Syntactic candidates only. The stage producer determines the actual
    /// size inputs after ordinary P; selecting roots executes no body phase.
    pub fn front_staged_roots(
        &mut self,
        definitions: &[DefPath],
    ) -> Result<Vec<DefPath>, QueryError> {
        let mut roots = Vec::new();
        for path in definitions {
            let signature = self.item_signature(path)?.value;
            if !signature
                .generics()
                .iter()
                .any(|parameter| matches!(parameter, hirsel_types::GenericParameter::Size { .. }))
            {
                continue;
            }
            let syntax = self.snapshot(path)?;
            if syntax
                .tree
                .expressions
                .iter()
                .any(|expression| matches!(expression.kind, hirsel_syntax::ExprKind::Comptime(_)))
            {
                roots.push(path.clone());
            }
        }
        Ok(roots)
    }

    /// Publish only a completed declaration product whose full source reads
    /// still hold. The caller admits the installed copy independently of the
    /// producer's retained output, just as accepted generated syntax does.
    pub fn accept_front_staged(
        &mut self,
        output: &FrontStageOutput,
        mut allowance: MemoryReservation,
    ) -> Result<(), QueryError> {
        let refused = |message: String| {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "staged declaration acceptance".into(),
                message,
            })
        };
        let registrations = output.data.result.as_ref().map_err(Clone::clone)?;
        if let Some(key) = output
            .reads()
            .first_changed(|key| self.observe(output.owner(), key))
        {
            return Err(refused(format!(
                "{}: changed staged declaration read {key:?}",
                output.owner()
            )));
        }
        if registrations
            .iter()
            .any(|row| row.constant.owner != *output.owner())
        {
            return Err(refused(
                "declaration belongs to another source owner".into(),
            ));
        }
        let installed = allowance
            .split(output.installation_bytes()?)
            .ok_or_else(|| {
                refused("installed declarations exceed their source reservation".into())
            })?;
        // Check all reads and reserve the entire copy before publishing any
        // declaration. Source/view clones keep that copy's original grant.
        let registrations = registrations.clone();
        self.sources = self.sources.clone().retain_owner(Arc::new(installed));
        for registration in registrations {
            self.install_staged(registration);
        }
        Ok(())
    }
}
