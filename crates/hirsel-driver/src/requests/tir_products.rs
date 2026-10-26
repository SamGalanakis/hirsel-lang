//! Read-only execution access to the ordinary settled Lower owner. Missing
//! products remain errors; this adapter cannot demand a query or evaluate data.
use super::{TirExecution, TirProducts};
use hirsel_engine::{FrontLowerQueries, FrontRunOutput, QueryKind, ToolError};
use hirsel_syntax::DefPath;
use hirsel_tir::{DefinitionRef, MaterializedRef};
use hirsel_types::{GenericArgument, QueryError};
use std::sync::Arc;

struct SettledProducts {
    lower: Arc<FrontLowerQueries>,
    _facts: Arc<FrontRunOutput>,
}
/// Called only after the selected run root settled its actual checked closure.
/// Reads here decode ordinary owned products; they cannot run a producer.
pub(super) fn materialize(
    facts: Arc<FrontRunOutput>,
    lower: Arc<FrontLowerQueries>,
) -> Result<Arc<TirExecution>, ToolError> {
    let sources = lower.sources().clone().retain_owner(facts.clone());
    let query_error = |error| ToolError::Query {
        error,
        phase: Some(QueryKind::Lower),
        sources: sources.clone(),
    };
    let definition = lower
        .settled_definition(&facts.entry)
        .map_err(&query_error)?;
    let signature = lower.signature(&facts.entry).map_err(&query_error)?;
    Ok(Arc::new(TirExecution {
        entry: facts.entry.clone(),
        definition,
        signature,
        sources,
        runtime: facts.runtime.clone(),
        checked_runtime: facts.checked_runtime.clone(),
        code: facts.code.clone(),
        callees: facts.callees.clone(),
        entries: facts.entries.clone(),
        revision: facts.revision.clone(),
        foreign_unreplayable: facts.semantic.foreign_unreplayable(),
        foreign: facts.foreign.clone(),
        products: Arc::new(SettledProducts {
            lower,
            _facts: facts,
        }),
    }))
}
impl TirProducts for SettledProducts {
    fn definition(&self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        self.lower.settled_definition(path)
    }
    fn constant(&self, path: &DefPath, actuals: &[u64]) -> Result<MaterializedRef, QueryError> {
        self.lower.settled_constant(path, actuals)
    }
    fn constant_definitions(&self) -> Result<Vec<DefPath>, QueryError> {
        Ok(self.lower.settled_constants())
    }
    fn instance_body(
        &self,
        path: &DefPath,
        actuals: &[GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        self.lower.settled_instance(path, actuals)
    }
}
