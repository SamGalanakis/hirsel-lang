//! Explicit compiler entry points use the same immutable requests as Sessions.
use crate::session::Session;
use hirsel_engine::{CheckOutput, Compiler, ToolError};
use hirsel_syntax::DefPath;
use std::path::Path;

pub trait CompilerRequestsExt {
    fn check_path(&self, path: &Path) -> Result<CheckOutput, ToolError>;
    fn check_safe_path(
        &self,
        path: &Path,
    ) -> Result<
        (
            CheckOutput,
            Option<(DefPath, Option<hirsel_engine::UnsafeReach>)>,
        ),
        ToolError,
    >;
}
impl CompilerRequestsExt for Compiler {
    fn check_path(&self, path: &Path) -> Result<CheckOutput, ToolError> {
        let publication = self
            .configuration()
            .store
            .local_product_store()
            .map(|store| store.request());
        let output = Session::open_with_compiler(path, self.clone()).check();
        if output
            .as_ref()
            .is_ok_and(|output| output.completion == hirsel_engine::CheckCompletion::Complete)
        {
            if let Some(publication) = publication {
                let _ = publication.finish();
            }
        }
        output
    }
    fn check_safe_path(
        &self,
        path: &Path,
    ) -> Result<
        (
            CheckOutput,
            Option<(DefPath, Option<hirsel_engine::UnsafeReach>)>,
        ),
        ToolError,
    > {
        let publication = self
            .configuration()
            .store
            .local_product_store()
            .map(|store| store.request());
        let output = Session::open_with_compiler(path, self.clone()).check_safe();
        if output
            .as_ref()
            .is_ok_and(|(output, _)| output.completion == hirsel_engine::CheckCompletion::Complete)
        {
            if let Some(publication) = publication {
                let _ = publication.finish();
            }
        }
        output
    }
}
