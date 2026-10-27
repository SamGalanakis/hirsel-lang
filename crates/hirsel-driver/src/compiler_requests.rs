//! Explicit compiler entry points use the same immutable requests as Sessions.
use crate::session::Session;
use hirsel_engine::{CheckOutput, Compiler, ToolError};
use std::path::Path;

/// Explicit checks preserve this compiler's policy on immutable request graphs.
pub trait CompilerRequestsExt {
    /// Check a source or package, returning its current source diagnostics.
    fn check_path(&self, path: &Path) -> Result<CheckOutput, ToolError>;
    /// Check the source, then observe the accepted launch entry's unsafe reach.
    fn check_safe_path(
        &self,
        path: &Path,
    ) -> Result<
        (
            CheckOutput,
            Option<(hirsel_syntax::DefPath, Option<hirsel_engine::UnsafeReach>)>,
        ),
        ToolError,
    >;
}
impl CompilerRequestsExt for Compiler {
    fn check_path(&self, path: &Path) -> Result<CheckOutput, ToolError> {
        let config = self.configuration();
        let compiler = self.clone().with_config(config.clone());
        let publication = config
            .store
            .local_product_store()
            .map(|store| store.request());
        let output = Session::open_with_compiler(path, compiler).check();
        if output
            .as_ref()
            .is_ok_and(|output| output.completion == hirsel_engine::CheckCompletion::Complete)
            && let Some(publication) = publication
        {
            let _ = publication.finish();
        }
        output
    }
    fn check_safe_path(
        &self,
        path: &Path,
    ) -> Result<
        (
            CheckOutput,
            Option<(hirsel_syntax::DefPath, Option<hirsel_engine::UnsafeReach>)>,
        ),
        ToolError,
    > {
        let config = self.configuration();
        let compiler = self.clone().with_config(config.clone());
        let publication = config
            .store
            .local_product_store()
            .map(|store| store.request());
        let output = Session::open_with_compiler(path, compiler).check_safe();
        if output.as_ref().is_ok_and(|(checked, _)| {
            checked.completion == hirsel_engine::CheckCompletion::Complete
        }) && let Some(publication) = publication
        {
            let _ = publication.finish();
        }
        output
    }
}
