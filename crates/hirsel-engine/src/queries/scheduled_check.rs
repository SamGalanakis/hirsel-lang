//! One ordered source check transaction, over actual P/GEN-5/Lower tasks.
//! Worker completion is separate from file publication: ordinary P diagnostics
//! accumulate, unavailable P withholds the file suffix, and a Lower failure
//! withholds its later roots. This replaces no default until all source-rule
//! and registration producers are installed.
use super::*;
use crate::runtime::*;
use crate::store::{Domain, Reference, ResultStore};
use std::{sync::Mutex, task::Poll};
pub const FRONT_CHECK_KIND: u32 = 0x4600_000a;
const OUTPUT: Domain = Domain {
    kind: 0x4600_000b,
    schema: 1,
};
/// Engine-selected snapshots and source order. Clients submit operations and
/// frozen input observations; the compiler host constructs this plan.
pub struct FrontCheckPlan {
    pub files: Vec<(FileId, FileContext, LoweringPolicy)>,
    pub front: Arc<FrontBodyQueries>,
    pub lower: Arc<FrontLowerQueries>,
    pub growth: Arc<FrontGrowthQueries>,
    pub unsafe_ffi: Vec<UnsafeFfiPackage>,
}
#[derive(Clone)]
struct FileInput {
    paths: Vec<(DefPath, bool)>,
    lowering: LoweringPolicy,
    diagnostics: Vec<Diagnostic>,
    error: Option<QueryError>,
    phase: Option<QueryKind>,
}
struct Data {
    checked: CheckOutput,
    front: Vec<(DefPath, ProductRef)>,
    lowered: Vec<(DefPath, ProductRef)>,
    reads: Arc<Reads>,
}
/// Source and receipt owners remain admitted until the last inspection or
/// execution consumer releases the completed check.
pub struct FrontCheckOutput {
    pub checked: CheckOutput,
    pub front: Vec<(DefPath, ProductRef)>,
    pub lowered: Vec<(DefPath, ProductRef)>,
    pub reads: Arc<Reads>,
    _owner: Arc<FrontCheckQueries>,
}
pub struct FrontCheckQueries {
    query: QueryId,
    files: Vec<FileInput>,
    plan: FrontCheckPlan,
    constants: BTreeSet<DefId>,
    sources: SourceMap,
    records: crate::NativeRecords,
    anchors: crate::NativeRecordContext,
    store: Arc<dyn ResultStore>,
    completed: Mutex<Option<(ProductRef, Arc<Data>)>>,
    failures: Mutex<BTreeMap<ContentHash, (Reference, QueryError, Option<QueryKind>)>>,
    weight: MemoryWeight,
    errors: RuntimeErrors,
    _allowance: MemoryReservation,
}
impl FrontCheckQueries {
    /// Ordered file/path cursors and product-reference rows. The actual
    /// definition bodies remain charged to their own producer owners.
    pub fn input_bytes(definitions: &[DefPath], files: usize, weight: MemoryWeight) -> Option<u64> {
        let path_slot = (std::mem::size_of::<(DefPath, bool)>()
            + 2 * std::mem::size_of::<(DefPath, ProductRef)>()
            + std::mem::size_of::<DefId>()
            + 128) as u64;
        let base = weight.retained.checked_add(
            (files as u64).checked_mul((2 * std::mem::size_of::<FileInput>() + 128) as u64)?,
        )?;
        definitions.iter().try_fold(base, |total, path| {
            total.checked_add(
                path_slot.checked_add(bincode::serialized_size(path).ok()?.checked_mul(3)?)?,
            )
        })
    }
    pub fn query(&self) -> QueryId {
        self.query
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn outcome(self: &Arc<Self>, product: ProductRef) -> Option<FrontCheckOutput> {
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, data) = completed.as_ref()?;
        if *selected != product {
            return None;
        }
        Some(FrontCheckOutput {
            checked: CheckOutput {
                completion: data.checked.completion,
                sources: data.checked.sources.clone(),
                diagnostics: data.checked.diagnostics.clone(),
                unsafe_ffi: data.checked.unsafe_ffi.clone(),
            },
            front: data.front.clone(),
            lowered: data.lowered.clone(),
            reads: data.reads.clone(),
            _owner: self.clone(),
        })
    }
    /// Inspect acceptance without cloning the ordered file/product inventory.
    /// Conditional host continuations use this only after reading this root.
    pub fn accepted(&self, product: ProductRef) -> Option<bool> {
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, data) = completed.as_ref()?;
        (*selected == product).then(|| {
            data.checked.completion == CheckCompletion::Complete
                && !has_errors(&data.checked.diagnostics)
        })
    }
    pub fn failure(&self, reference: Reference) -> Option<(QueryError, Option<QueryKind>)> {
        let failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let (selected, error, phase) = failures.get(&reference.output.0)?;
        (*selected == reference).then(|| (error.clone(), *phase))
    }
}
struct Factory(Arc<FrontCheckQueries>);
impl QueryFactory for Factory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if id != self.0.query {
            return Err(self.0.errors.protocol);
        }
        Ok(Box::new(Task {
            queries: self.0.clone(),
            files: self.0.files.iter().cloned().map(File::new).collect(),
            stage: Stage::Front,
            file: 0,
            root: 0,
            reads: ReadsBuilder::new(),
            front: Vec::new(),
            lowered: Vec::new(),
            terminal: None,
        }))
    }
}
struct File {
    input: FileInput,
    diagnostics: Vec<Diagnostic>,
    error: Option<QueryError>,
    phase: Option<QueryKind>,
    comptime: Vec<DefPath>,
    rest: Vec<DefPath>,
    source_rules: bool,
}
impl File {
    fn new(input: FileInput) -> Self {
        Self {
            diagnostics: input.diagnostics.clone(),
            error: input.error.clone(),
            phase: input.phase,
            input,
            comptime: Vec::new(),
            rest: Vec::new(),
            source_rules: false,
        }
    }
    fn clean(&self) -> bool {
        self.error.is_none() && !has_errors(&self.diagnostics)
    }
    fn error(&mut self, error: QueryError, phase: Option<QueryKind>) {
        match error {
            QueryError::Diagnostics(diagnostics) => self.diagnostics.extend(diagnostics),
            other => {
                self.error = Some(other);
                self.phase = phase;
            }
        }
    }
}
#[derive(Clone, Copy)]
enum Stage {
    Front,
    Growth,
    Comptime,
    Rest,
    Done,
}
struct Task {
    queries: Arc<FrontCheckQueries>,
    files: Vec<File>,
    stage: Stage,
    file: usize,
    root: usize,
    reads: ReadsBuilder,
    front: Vec<(DefPath, ProductRef)>,
    lowered: Vec<(DefPath, ProductRef)>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.queries.query
    }
    fn weight(&self) -> MemoryWeight {
        self.queries.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return finish(result);
        }
        match self.advance(cx) {
            Ok(Some(waiting)) => QueryPoll::Waiting(waiting),
            result => {
                let result = result.and_then(|_| self.complete());
                self.terminal = Some(result);
                finish(result)
            }
        }
    }
}
fn finish(result: Result<ProductRef, Reference>) -> QueryPoll {
    match result {
        Ok(product) => QueryPoll::Complete(product),
        Err(error) => QueryPoll::Failed(error),
    }
}
impl Task {
    fn advance(&mut self, cx: &mut dyn QueryContext) -> Result<Option<Vec<QueryId>>, Reference> {
        loop {
            match self.stage {
                Stage::Front => {
                    // Request immutable ordinary bodies together, but publish only
                    // in source/file order. No generator or Lower is started here.
                    let mut pending = Vec::new();
                    for file in &self.files {
                        if file.input.error.is_none() && !has_errors(&file.input.diagnostics) {
                            for (path, _) in &file.input.paths {
                                let id = self
                                    .queries
                                    .plan
                                    .front
                                    .query(path)
                                    .ok_or(self.queries.errors.protocol)?;
                                if cx.read(id).is_pending() {
                                    pending.push(id);
                                }
                            }
                        }
                    }
                    // Every Pending read in this poll must be returned to
                    // S5. Inspect and publish the completed products in source
                    // order only after the prefetched ordinary bodies settle.
                    if !pending.is_empty() {
                        return Ok(Some(pending));
                    }
                    while self.file < self.files.len() {
                        let file = &mut self.files[self.file];
                        if file.input.error.is_some()
                            || has_errors(&file.input.diagnostics)
                            || file.error.is_some()
                            || self.root == file.input.paths.len()
                        {
                            self.file += 1;
                            self.root = 0;
                            continue;
                        }
                        let (path, executable) = &file.input.paths[self.root];
                        let id = self
                            .queries
                            .plan
                            .front
                            .query(path)
                            .ok_or(self.queries.errors.protocol)?;
                        let selected = match cx.read(id) {
                            Poll::Pending => return Ok(Some(vec![id])),
                            Poll::Ready(selected) => selected,
                        };
                        let reference = selected.map(|p| p.record).unwrap_or_else(|r| r);
                        let Some(body) = self.queries.plan.front.outcome(reference) else {
                            return Err(reference);
                        };
                        self.reads.merge(&body.outcome.reads);
                        if let Some(summary) = &body.outcome.summary {
                            file.source_rules |= summary
                                .summary
                                .calls
                                .iter()
                                .any(|call| call.callee.path().is_core("region.with"));
                        }
                        match body.outcome.result {
                            Ok(output) => {
                                file.diagnostics.extend(output.diagnostics.iter().cloned());
                                if let Ok(product) = selected {
                                    self.front.push((path.clone(), product));
                                }
                                if *executable {
                                    if lowering::LoweringUse::capture(&output.value.typed)
                                        .needed_with(|id| self.queries.constants.contains(&id))
                                    {
                                        file.comptime.push(path.clone());
                                    } else {
                                        file.rest.push(path.clone());
                                    }
                                }
                            }
                            Err(error) => file.error(error, body.outcome.phase),
                        }
                        self.root += 1;
                    }
                    self.stage = Stage::Growth;
                    self.file = 0;
                    self.root = 0;
                }
                Stage::Growth => {
                    if self.files.iter().any(File::clean) {
                        let id = self.queries.plan.growth.query();
                        let selected = match cx.read(id) {
                            Poll::Pending => return Ok(Some(vec![id])),
                            Poll::Ready(selected) => selected,
                        };
                        match selected {
                            Ok(product) => {
                                let output = self
                                    .queries
                                    .plan
                                    .growth
                                    .outcome(product)
                                    .ok_or(self.queries.errors.protocol)?;
                                self.reads.merge(&output.reads);
                                for file in self.files.iter_mut().filter(|file| file.clean()) {
                                    file.diagnostics.extend(output.diagnostics.iter().cloned());
                                }
                            }
                            Err(reference) => {
                                let (error, phase) = if let Some(output) =
                                    self.queries.plan.front.outcome(reference)
                                {
                                    (
                                        output
                                            .outcome
                                            .result
                                            .err()
                                            .ok_or(self.queries.errors.protocol)?,
                                        output.outcome.phase,
                                    )
                                } else {
                                    (
                                        self.queries
                                            .plan
                                            .growth
                                            .failure(reference)
                                            .ok_or(reference)?,
                                        Some(QueryKind::Check),
                                    )
                                };
                                for file in self.files.iter_mut().filter(|file| file.clean()) {
                                    file.error(error.clone(), phase);
                                }
                            }
                        }
                        // These rules require their own actual demand producers.
                        // Until installed, report the normal incomplete phase;
                        // never accept a program after omitting a source rule.
                        if self.files.iter().any(|file| file.source_rules) {
                            for file in self.files.iter_mut().filter(|file| file.clean()) {
                                file.error(QueryError::NotImplemented, Some(QueryKind::Check));
                            }
                        }
                    }
                    self.stage = Stage::Comptime;
                }
                Stage::Comptime | Stage::Rest => {
                    let rest = matches!(self.stage, Stage::Rest);
                    while self.file < self.files.len() {
                        let file = &mut self.files[self.file];
                        let paths = if rest { &file.rest } else { &file.comptime };
                        if !file.clean()
                            || (rest && file.input.lowering != LoweringPolicy::Reachable)
                            || self.root == paths.len()
                        {
                            self.file += 1;
                            self.root = 0;
                            continue;
                        }
                        let path = &paths[self.root];
                        let id = self
                            .queries
                            .plan
                            .lower
                            .query(path)
                            .ok_or(self.queries.errors.protocol)?;
                        let selected = match cx.read(id) {
                            Poll::Pending => return Ok(Some(vec![id])),
                            Poll::Ready(selected) => selected,
                        };
                        match selected {
                            Ok(product) => {
                                let output = self
                                    .queries
                                    .plan
                                    .lower
                                    .outcome(product)
                                    .ok_or(self.queries.errors.protocol)?;
                                self.reads.merge(&output.reads);
                                file.diagnostics.extend(output.output.diagnostics);
                                self.lowered.push((path.clone(), product));
                            }
                            Err(reference) => file.error(
                                self.queries
                                    .plan
                                    .lower
                                    .failure(reference)
                                    .ok_or(reference)?,
                                Some(QueryKind::Lower),
                            ),
                        }
                        self.root += 1;
                    }
                    self.file = 0;
                    self.root = 0;
                    self.stage = if rest { Stage::Done } else { Stage::Rest };
                }
                Stage::Done => return Ok(None),
            }
        }
    }
    fn complete(&mut self) -> Result<ProductRef, Reference> {
        let queries = &self.queries;
        let reads = std::mem::take(&mut self.reads)
            .finish()
            .map_err(|_| queries.errors.protocol)?;
        let stopped = self.files.iter().find_map(|file| match &file.error {
            Some(error) if *error != QueryError::NotImplemented => {
                Some((error.clone(), file.phase))
            }
            _ => None,
        });
        if let Some((error, phase)) = stopped {
            let bytes = queries
                .records
                .encode(
                    &queries.anchors,
                    &(&error, phase.map(QueryKind::phase_name), &reads),
                )
                .ok_or(queries.errors.resource)?;
            let reference = queries
                .store
                .put_content(OUTPUT, &bytes)
                .ok_or(queries.errors.resource)?;
            queries
                .failures
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(reference.output.0, (reference, error, phase));
            return Err(reference);
        }
        let incomplete = self
            .files
            .iter()
            .filter(|file| file.error == Some(QueryError::NotImplemented))
            .filter_map(|file| file.phase)
            .last();
        let completion = incomplete.map_or(CheckCompletion::Complete, |phase| {
            CheckCompletion::Incomplete { phase }
        });
        let diagnostics = crate::select_diagnostics(
            self.files
                .iter()
                .flat_map(|file| file.diagnostics.iter().cloned())
                .collect(),
            Some(&queries.sources),
        );
        let bytes = queries
            .records
            .encode(
                &queries.anchors,
                &(
                    incomplete.map(QueryKind::phase_name),
                    &diagnostics,
                    &self
                        .front
                        .iter()
                        .map(|(path, p)| (path, p.record))
                        .collect::<Vec<_>>(),
                    &self
                        .lowered
                        .iter()
                        .map(|(path, p)| (path, p.record))
                        .collect::<Vec<_>>(),
                    &reads,
                ),
            )
            .ok_or(queries.errors.resource)?;
        if bytes.len() as u64 > queries.weight.retained {
            return Err(queries.errors.resource);
        }
        let record = queries
            .store
            .put_content(OUTPUT, &bytes)
            .ok_or(queries.errors.resource)?;
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.check.action.v1",
            [
                queries.query.address.0.0.as_slice(),
                record.output.0.0.as_slice(),
            ],
        ));
        let product = ProductRef {
            address: queries.query.address,
            action,
            record,
        };
        let checked = CheckOutput {
            completion,
            sources: queries.sources.clone(),
            diagnostics,
            unsafe_ffi: queries.plan.unsafe_ffi.clone(),
        };
        let data = Arc::new(Data {
            checked,
            front: std::mem::take(&mut self.front),
            lowered: std::mem::take(&mut self.lowered),
            reads: Arc::new(reads),
        });
        *queries.completed.lock().unwrap_or_else(|e| e.into_inner()) = Some((product, data));
        queries
            .store
            .associate_product(product.address, product.action, product.record);
        Ok(product)
    }
}
impl QueryEngine {
    /// Capture the ordered file inputs. S0 uses the existing index/association
    /// checks; no body, constant, lower or old phase controller executes here.
    pub fn front_check_queries(
        &mut self,
        plan: FrontCheckPlan,
        weight: MemoryWeight,
        allowance: MemoryReservation,
        errors: RuntimeErrors,
    ) -> Result<Arc<FrontCheckQueries>, QueryError> {
        let refusal = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "ordered check admission".into(),
                message: "source check plan exceeds admission or lacks a required producer".into(),
            })
        };
        let mut files = Vec::new();
        let mut constants = BTreeSet::new();
        for (file, context, lowering) in &plan.files {
            let mut input = FileInput {
                paths: Vec::new(),
                lowering: *lowering,
                diagnostics: Vec::new(),
                error: None,
                phase: None,
            };
            match self
                .index_source(*file, context)
                .and_then(|()| crate::tools::association_diagnostics(self, *file, context))
            {
                Ok(diagnostics) => input.diagnostics = diagnostics,
                Err(QueryError::Diagnostics(diagnostics)) => input.diagnostics = diagnostics,
                Err(error) => {
                    input.error = Some(error);
                    input.phase = self.unavailable.or(self.last_phase);
                }
            }
            if input.error.is_none() && !has_errors(&input.diagnostics) {
                for item in self.source_items(*file) {
                    if item.facts.constant {
                        constants.insert(DefId::intern(&item.definition));
                    }
                    if plan.front.query(&item.definition).is_none() {
                        return Err(refusal());
                    }
                    input
                        .paths
                        .push((item.definition.clone(), item.facts.executable));
                    if item.facts.fact {
                        input.paths.push((
                            hirsel_check_protocol::fact::fact_path(&item.definition),
                            true,
                        ));
                    }
                }
            }
            files.push(input);
        }
        let definitions: Vec<_> = files
            .iter()
            .flat_map(|file| file.paths.iter().map(|(path, _)| path.clone()))
            .collect();
        if FrontCheckQueries::input_bytes(&definitions, files.len(), weight)
            .is_none_or(|required| required > allowance.bytes())
        {
            return Err(refusal());
        }
        let mut file_identity = Vec::new();
        for file in &files {
            file_identity.push(file.lowering as u8);
            for (path, _) in &file.paths {
                file_identity.extend(path.fingerprint().0);
            }
        }
        let query = QueryId {
            kind: FRONT_CHECK_KIND,
            address: hirsel_artifact_store::Address(ContentHash::of_parts(
                "hirsel.front.check.address.v1",
                [
                    self.environment_value().0.as_slice(),
                    file_identity.as_slice(),
                ],
            )),
        };
        let store = self
            .result_store
            .clone()
            .filter(|store| store.enabled())
            .ok_or_else(refusal)?;
        Ok(Arc::new(FrontCheckQueries {
            query,
            files,
            plan,
            constants,
            sources: self.sources.clone(),
            records: crate::NativeRecords::new(self.types.clone(), store.clone()),
            anchors: crate::NativeRecordContext::capture(self),
            store,
            completed: Mutex::new(None),
            failures: Mutex::new(BTreeMap::new()),
            weight,
            errors,
            _allowance: allowance,
        }))
    }
}
