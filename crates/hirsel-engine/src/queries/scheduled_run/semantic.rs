//! Per-definition semantic products shared by Run closures and precomputation.
//! These producers demand P/Lower/Evaluate directly and never construct launch
//! metadata or require the program Check root to complete.
use super::*;

const OUTPUT: Domain = Domain {
    kind: 0x4600_0034,
    schema: 2,
};

impl FrontRunQueries {
    /// The source-owned semantic node, independent of a launch or Check root.
    pub fn semantic_query(&self, path: &DefPath) -> Option<QueryId> {
        self.semantic_ids.get(path).copied()
    }
    /// Read one exact completed semantic node without demanding any query.
    pub fn semantic_node(
        &self,
        product: ProductRef,
    ) -> Result<hirsel_run_schema::ProgramNode, QueryError> {
        let entry = self
            .semantic_entry(product.clone())
            .ok_or_else(|| operational("semantic product is unavailable"))?;
        let path = self
            .semantic_paths
            .get(&product.address.0)
            .ok_or_else(|| operational("semantic product has another owner"))?
            .path();
        entry.node.program(self, path)
    }
    pub(super) fn semantic_entry(&self, product: ProductRef) -> Option<Arc<GraphEntry>> {
        self.semantic_completed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&product.address.0)
            .and_then(|(selected, entry)| (*selected == product).then(|| entry.clone()))
    }
}

pub(super) fn create(queries: &Arc<FrontRunQueries>, query: QueryId) -> Option<Box<dyn QueryTask>> {
    if query.kind != FRONT_RUN_KIND {
        return None;
    }
    let path = queries.semantic_paths.get(&query.address.0)?.path().clone();
    let terminal = queries
        .semantic_completed
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&query.address.0)
        .map(|(product, _)| Ok(product.clone()));
    Some(Box::new(SemanticTask {
        node: Task {
            queries: queries.clone(),
            query,
            root: path,
            checked: false,
            pending: Vec::new(),
            entries: BTreeMap::new(),
            waiting: None,
            terminal,
        },
    }))
}
struct SemanticTask {
    node: Task,
}
impl QueryTask for SemanticTask {
    fn id(&self) -> QueryId {
        self.node.query
    }
    fn weight(&self) -> MemoryWeight {
        let weight = self.node.queries.weight;
        MemoryWeight {
            retained: weight.retained / 2,
            ..weight
        }
    }
    fn continuation_bytes(&self) -> u64 {
        // Completed nodes live in semantic_completed under the independent
        // source-output reservation, just like settled Lower products.
        self.weight().retained
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let result = if let Some(result) = self.node.terminal.clone() {
            result
        } else {
            let path = self.node.root.clone();
            match self.node.node(cx, &path) {
                Poll::Pending => {
                    return QueryPoll::Waiting(self.node.waiting.into_iter().collect());
                }
                Poll::Ready(Err(error)) => Err(error),
                Poll::Ready(Ok((node, content, dependencies, source))) => self
                    .complete(node, content, dependencies, source)
                    .map_err(|error| self.node.queries.failure_record(error)),
            }
        };
        self.node.terminal = Some(result.clone());
        match result {
            Ok(product) => QueryPoll::Complete(product),
            Err(error) => QueryPoll::Failed(error),
        }
    }
}
impl SemanticTask {
    fn complete(
        &self,
        node: content::Node,
        content: DefinitionContent,
        dependencies: BTreeSet<DefPath>,
        source: Option<source_products::Body>,
    ) -> Result<ProductRef, QueryError> {
        let queries = &self.node.queries;
        let source = source
            .map(|source| source_products::Packed::new(queries, source))
            .transpose()?;
        let entry = Arc::new(GraphEntry {
            source,
            node,
            content: content::Content::new(queries, content)?,
            dependencies: dependencies
                .iter()
                .map(DefId::intern)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        });
        let mut weight = crate::NativeMetadataWeight::default();
        weight.tree::<ContentHash, (ProductRef, Arc<GraphEntry>)>(1);
        weight.add(std::mem::size_of::<GraphEntry>() + 2 * std::mem::size_of::<usize>());
        weight.add(std::mem::size_of_val(&*entry.dependencies));
        entry.node.weight(&mut weight);
        entry.content.weight(&mut weight);
        let charge = weight.bytes();
        if charge > queries.weight.retained / 2 {
            return Err(operational("semantic product exceeds retained admission"));
        }
        let node = entry.node.program(queries, &self.node.root)?;
        let bytes = bincode::serialize(&(
            node.identity,
            node.content,
            node.dependencies,
            entry.content.references(),
            entry.source.as_ref(),
        ))
        .map_err(|error| operational(error.to_string()))?;
        if bytes.len() as u64 > queries.weight.scratch {
            return Err(operational("semantic encoding exceeds scratch admission"));
        }
        let record = queries
            .store
            .put_content(OUTPUT, &bytes)
            .ok_or_else(|| operational("semantic product transport exceeds admission"))?;
        let action = hirsel_artifact_store::Action(ContentHash::of_parts(
            "hirsel.front.semantic.action.v1",
            [
                self.node.query.address.0.0.as_slice(),
                record.output.0.0.as_slice(),
            ],
        ));
        let product = ProductRef {
            address: self.node.query.address,
            action,
            record,
        };
        let mut remaining = queries
            .retention
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *remaining = remaining
            .checked_sub(charge)
            .ok_or_else(|| operational("semantic products exceed admission"))?;
        queries
            .semantic_completed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(product.address.0, (product.clone(), entry));
        queries
            .store
            .associate_product(product.address, product.action, product.record);
        Ok(product)
    }
}
