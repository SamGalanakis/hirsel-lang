//! Compiler dependency suspension uses Rust's saved future state. No evaluator
//! frame, checked state, operand or provider read is reconstructed on Pending.
use hirsel_syntax::DefPath;
use hirsel_tir::{DefinitionRef, MaterializedRef, TirProvider};
use hirsel_types::{GenericArgument, QueryError};
use std::future::Future;

/// An immutable program whose dependency reads may suspend. These futures need
/// not be Send: the compiler keeps interpreter state on its creating worker.
/// Pending is a compiler dependency, distinct from a language operation wait.
pub trait AsyncTirProvider {
    fn definition(
        &mut self,
        path: &DefPath,
    ) -> impl Future<Output = Result<DefinitionRef, QueryError>>;
    fn constant(
        &mut self,
        path: &DefPath,
    ) -> impl Future<Output = Result<MaterializedRef, QueryError>>;
    fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> impl Future<Output = Result<MaterializedRef, QueryError>>;
    fn constant_definitions(&mut self) -> impl Future<Output = Result<Vec<DefPath>, QueryError>>;
    fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> impl Future<Output = Result<Option<DefPath>, QueryError>>;
}

/// Runtime interpreters have an already completed program. They use the same
/// evaluator, with immediately ready dependency reads and no executor or wait.
pub struct ImmediateTirProvider<'a, P: ?Sized> {
    provider: &'a mut P,
}
impl<'a, P: ?Sized> ImmediateTirProvider<'a, P> {
    pub fn new(provider: &'a mut P) -> Self {
        Self { provider }
    }
}
impl<P: TirProvider + ?Sized> AsyncTirProvider for ImmediateTirProvider<'_, P> {
    async fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        self.provider.definition(path)
    }
    async fn constant(&mut self, path: &DefPath) -> Result<MaterializedRef, QueryError> {
        self.provider.constant(path)
    }
    async fn constant_instance(
        &mut self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<MaterializedRef, QueryError> {
        self.provider.constant_instance(path, arguments)
    }
    async fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        self.provider.constant_definitions()
    }
    async fn instance_body(
        &mut self,
        path: &DefPath,
        arguments: &[GenericArgument],
    ) -> Result<Option<DefPath>, QueryError> {
        self.provider.instance_body(path, arguments)
    }
}
