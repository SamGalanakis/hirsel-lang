//! Explicit, admitted graph inspection over completed semantic producers.
use super::*;

pub struct FrontSemanticGraph {
    graph: hirsel_run_schema::ProgramGraph,
    _owner: Arc<FrontRunQueries>,
    _allowance: MemoryReservation,
}
impl FrontSemanticGraph {
    pub fn revision(
        &self,
        entry: &DefPath,
    ) -> Result<hirsel_run_schema::ProgramRevision, QueryError> {
        self.graph
            .revision(&entry.to_string())
            .map_err(|error| operational(error.to_string()))
    }
}
impl FrontRunQueries {
    /// Captured descriptor sizes only. This opens no content and runs no query.
    pub fn semantic_graph_decode_bytes(&self) -> Result<u64, QueryError> {
        let completed = self
            .semantic_completed
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if completed.len() != self.semantic_ids.len() {
            return Err(operational("semantic graph is not completely settled"));
        }
        let count = completed
            .len()
            .checked_add(1)
            .ok_or(QueryError::NotImplemented)?;
        let mut metadata = crate::NativeMetadataWeight::default();
        metadata.add(std::mem::size_of::<FrontSemanticGraph>());
        metadata.tree::<String, hirsel_run_schema::ProgramNode>(count);
        metadata.add(
            count
                .checked_mul(2 * std::mem::size_of::<hirsel_run_schema::ProgramNode>())
                .ok_or(QueryError::NotImplemented)?,
        );
        // Rooted encoding borrows node names; its closure set and pending
        // edges remain within this same reader's working reservation.
        metadata.tree::<&str, ()>(count);
        let mut bytes = metadata
            .bytes()
            .checked_add(1024)
            .ok_or(QueryError::NotImplemented)?;
        for (address, (_, entry)) in completed.iter() {
            let path = self
                .semantic_paths
                .get(address)
                .ok_or(QueryError::NotImplemented)?
                .path();
            bytes = bytes
                .checked_add(
                    entry
                        .node
                        .decode_bytes(path)
                        .ok_or(QueryError::NotImplemented)?,
                )
                .and_then(|n| {
                    n.checked_add(
                        ((entry.node.dependencies.len() + 1) * 2 * std::mem::size_of::<&str>())
                            as u64,
                    )
                })
                .ok_or(QueryError::NotImplemented)?;
        }
        Ok(bytes)
    }
    pub fn semantic_graph(
        self: &Arc<Self>,
        allowance: MemoryReservation,
    ) -> Result<FrontSemanticGraph, QueryError> {
        if self.semantic_graph_decode_bytes()? > allowance.bytes() {
            return Err(operational(
                "semantic graph decode exceeds caller admission",
            ));
        }
        let mut nodes = vec![super::super::program_revision::runtime_node()];
        // Completed entries and their content references are immutable. Retain
        // the source owner before releasing the lock and reading their bytes.
        let completed = self
            .semantic_completed
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for (address, (_, entry)) in completed.iter() {
            let path = self
                .semantic_paths
                .get(address)
                .ok_or(QueryError::NotImplemented)?
                .path();
            nodes.push(entry.node.program(self, path)?);
        }
        drop(completed);
        let graph =
            hirsel_run_schema::ProgramGraph::new(nodes).map_err(|e| operational(e.to_string()))?;
        let mut actual = crate::NativeMetadataWeight::default();
        actual.add(std::mem::size_of::<FrontSemanticGraph>());
        actual.tree::<String, hirsel_run_schema::ProgramNode>(graph.nodes().len());
        for node in graph.nodes() {
            actual.add(node.identity.capacity().saturating_mul(2));
            actual.vec(&node.dependencies, |weight, name| {
                weight.add(name.capacity())
            });
            actual.vec(&node.content, |weight, field| weight.flat_vec(field));
        }
        if actual.bytes() > allowance.bytes() {
            return Err(operational(
                "semantic graph retained data exceeds caller admission",
            ));
        }
        Ok(FrontSemanticGraph {
            graph,
            _owner: self.clone(),
            _allowance: allowance,
        })
    }
}
