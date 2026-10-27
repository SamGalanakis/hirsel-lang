//! Explicit compiler entry points use the same immutable requests as Sessions.
use crate::session::Session;
use hirsel_engine::{CheckOutput, Compiler, ToolError};
use std::path::Path;

pub trait CompilerRequestsExt {
    fn check_path(&self, path: &Path) -> Result<CheckOutput, ToolError>;
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
        {
            if let Some(publication) = publication {
                let _ = publication.finish();
            }
        }
        output
    }
}
