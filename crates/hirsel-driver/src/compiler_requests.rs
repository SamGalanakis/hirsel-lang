//! Explicit compiler entry points use the same immutable requests as Sessions.
use crate::session::Session;
use hirsel_engine::{CheckOutput, Compiler, ToolError};
use std::path::Path;

/// Explicit checks preserve this compiler's policy on immutable request graphs.
pub trait CompilerRequestsExt {
    /// Populate per-definition source products without requiring a launch entry.
    fn precompute_package(&self, manifest: &Path, output: &Path) -> Result<(), ToolError>;
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
    fn precompute_package(&self, manifest: &Path, output: &Path) -> Result<(), ToolError> {
        use crate::requests::{CompileOperation, CompileResult, CompilerRequests};
        use hirsel_engine::{
            Verify,
            memo::Retention,
            store::{DirStore, ResultStore},
        };
        let mut config = self.configuration();
        if config.verify == Verify::Off {
            return Err(ToolError::Runtime(
                "package precomputation requires V".into(),
            ));
        }
        let store = std::sync::Arc::new(DirStore::new(output.to_path_buf()));
        let publication = store.local_product_store().map(|store| store.request());
        config.store = store;
        config.retention = Retention::Batch;
        config.hold_bytes = 0;
        let compiler = self
            .clone()
            .with_owned_engines()
            .with_config(config.clone());
        let mut requests = CompilerRequests::for_host(compiler, config);
        let completed = requests.execute(manifest, CompileOperation::Precompute)?;
        if !matches!(completed.result, CompileResult::Precomputed { .. }) {
            return Err(ToolError::Runtime(
                "precompute source product is unavailable".into(),
            ));
        }
        if let Some(publication) = publication {
            publication
                .finish()
                .map_err(|error| ToolError::Runtime(error.to_string()))?;
        }
        Ok(())
    }
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
