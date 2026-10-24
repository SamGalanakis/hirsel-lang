//! Read-only execution access to the ordinary settled Lower owner. Missing
//! products remain errors; this adapter cannot demand a query or evaluate data.
use super::TirProducts;
use hirsel_engine::FrontLowerQueries;
use hirsel_syntax::DefPath;
use hirsel_tir::{DefinitionRef, MaterializedRef};
use hirsel_types::{GenericArgument, QueryError};
use std::sync::Arc;

pub(super) struct SettledProducts(pub Arc<FrontLowerQueries>);
impl TirProducts for SettledProducts {
    fn definition(&self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        self.0.settled_definition(path)
    }
    fn constant(&self, path: &DefPath, actuals: &[u64]) -> Result<MaterializedRef, QueryError> {
        self.0.settled_constant(path, actuals)
    }
    fn constant_definitions(&self) -> Result<Vec<DefPath>, QueryError> {
        Ok(self.0.settled_constants())
    }
    fn instance_body(
        &self,
        path: &DefPath,
        actuals: &[GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        self.0.settled_instance(path, actuals)
    }
}
