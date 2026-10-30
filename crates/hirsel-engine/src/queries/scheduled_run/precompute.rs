//! Check first, then demand every source-owned semantic/body product. The
//! continuation retains only its position and fold, never a launch or revision.
use super::*;
const OUTPUT: Domain = Domain {
    kind: 0x4600_0036,
    schema: 1,
};
#[derive(Clone, Copy)]
pub struct FrontPrecomputeOutput {
    pub definitions: u64,
    pub digest: ContentHash,
}
pub(super) struct Selection {
    query: QueryId,
    roots: Vec<(QueryId, bool)>,
    definitions: u64,
}
impl FrontRunQueries {
    pub fn with_precompute(
        mut self: Arc<Self>,
        check: Arc<FrontCheckQueries>,
    ) -> Result<Arc<Self>, QueryError> {
        if !self.source_products || !check.matches_inputs(&self.front, &self.lower) {
            return Err(operational(
                "precompute requires matching source-only producers",
            ));
        }
        let this = Arc::get_mut(&mut self)
            .ok_or_else(|| operational("precompute requires an unshared source view"))?;
        let roots = this
            .semantic_ids
            .iter()
            .map(|(path, query)| (*query, this.inputs[path].source))
            .collect::<Vec<_>>();
        let bytes = roots
            .capacity()
            .checked_mul(std::mem::size_of::<(QueryId, bool)>())
            .ok_or(QueryError::NotImplemented)? as u64;
        let mut remaining = this
            .retention
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *remaining = remaining
            .checked_sub(bytes)
            .ok_or_else(|| operational("precompute inventory exceeds admission"))?;
        drop(remaining);
        let query = QueryId {
            kind: FRONT_RUN_KIND,
            address: hirsel_artifact_store::Address(ContentHash::of_parts(
                "hirsel.front.precompute.address.v1",
                [check.query().address.0.0.as_slice()],
            )),
        };
        this.check = Some(check);
        let definitions = roots.iter().filter(|(_, source)| *source).count() as u64;
        this.precompute = Some(Selection {
            query,
            roots,
            definitions,
        });
        Ok(self)
    }
    pub fn precompute_query(&self) -> Option<QueryId> {
        self.precompute.as_ref().map(|selection| selection.query)
    }
    pub fn precompute_outcome(&self, product: ProductRef) -> Option<FrontPrecomputeOutput> {
        self.precomputed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .and_then(|(selected, output)| (*selected == product).then_some(*output))
    }
}
pub(super) fn create(queries: &Arc<FrontRunQueries>, query: QueryId) -> Option<Box<dyn QueryTask>> {
    if queries.precompute_query() != Some(query) {
        return None;
    }
    let terminal = queries
        .precomputed
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(|(product, _)| Ok(product.clone()));
    Some(Box::new(Task {
        queries: queries.clone(),
        query,
        checked: false,
        index: 0,
        digest: ContentHash::default(),
        waiting: None,
        terminal,
    }))
}
struct Task {
    queries: Arc<FrontRunQueries>,
    query: QueryId,
    checked: bool,
    index: usize,
    digest: ContentHash,
    waiting: Option<QueryId>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.query
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            retained: self.queries.weight.retained / 2,
            ..self.queries.weight
        }
    }
    fn continuation_bytes(&self) -> u64 {
        self.weight().retained
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let result = if let Some(result) = self.terminal.clone() {
            result
        } else {
            match self.advance(cx) {
                Poll::Pending => return QueryPoll::Waiting(self.waiting.into_iter().collect()),
                Poll::Ready(result) => result,
            }
        };
        self.terminal = Some(result.clone());
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
        query: QueryId,
    ) -> Poll<Result<ProductRef, Reference>> {
        match cx.read(query) {
            Poll::Pending => {
                self.waiting = Some(query);
                Poll::Pending
            }
            Poll::Ready(result) => {
                self.waiting = None;
                Poll::Ready(result)
            }
        }
    }
    fn advance(&mut self, cx: &mut dyn QueryContext) -> Poll<Result<ProductRef, Reference>> {
        if !self.checked {
            let Some(check) = self.queries.check.clone() else {
                return Poll::Ready(Err(self.queries.errors.protocol));
            };
            let product = match self.read(cx, check.query()) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(product)) => product,
            };
            let Some(output) = check.outcome(product) else {
                return Poll::Ready(Err(self.queries.errors.protocol));
            };
            if has_errors(&output.checked.diagnostics) {
                return Poll::Ready(Err(self
                    .queries
                    .failure_record(QueryError::Diagnostics(output.checked.diagnostics))));
            }
            if output.checked.completion != CheckCompletion::Complete {
                return Poll::Ready(Err(self.queries.failure_record(QueryError::NotImplemented)));
            }
            self.checked = true;
        }
        let queries = self.queries.clone();
        let Some(selection) = &queries.precompute else {
            return Poll::Ready(Err(queries.errors.protocol));
        };
        while let Some((query, source)) = selection.roots.get(self.index).copied() {
            let product = match self.read(cx, query) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(product)) => product,
            };
            if source {
                let archive = match product.archive(ArchiveAdmission::Retained) {
                    Ok(archive) => archive,
                    Err(_) => return Poll::Ready(Err(queries.errors.resource)),
                };
                self.digest = ContentHash::of_parts(
                    "hirsel.front.precompute.fold.v1",
                    [
                        self.digest.0.as_slice(),
                        archive.reference().output.0.0.as_slice(),
                    ],
                );
            }
            self.index += 1;
        }
        Poll::Ready((|| {
            let output = FrontPrecomputeOutput {
                definitions: selection.definitions,
                digest: self.digest,
            };
            let bytes = bincode::serialize(&(output.definitions, output.digest))
                .map_err(|_| queries.errors.resource)?;
            let record = queries
                .store
                .put_content(OUTPUT, &bytes)
                .ok_or(queries.errors.resource)?;
            let action = hirsel_artifact_store::Action(ContentHash::of_parts(
                "hirsel.front.precompute.action.v1",
                [
                    self.query.address.0.0.as_slice(),
                    record.output.0.0.as_slice(),
                ],
            ));
            let product = ProductRef::import_archive(
                self.query.address,
                action,
                queries
                    .admit_archive(record, bytes)
                    .map_err(|_| queries.errors.resource)?,
            );
            *queries
                .precomputed
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some((product.clone(), output));
            queries
                .store
                .associate_product(product.address, product.action, record);
            Ok(product)
        })())
    }
}
