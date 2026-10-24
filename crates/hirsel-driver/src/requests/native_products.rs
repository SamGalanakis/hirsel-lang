//! Build readiness over the bootstrap's exact completed native dependency
//! scope. Reopening descriptors and publishing an executable invoke no source,
//! Instance, Ready or Codegen producer and convey no new V/C authority.
use super::{NativeExecution, NativeRequest, native_bootstrap::NativeBootstrap};
use crate::native::{self, NativeError, StageName, object_queries};
use hirsel_engine::{FrontNativeProducts, runtime::ProductRef, store::ResultStore};
use hirsel_syntax::DefId;
use std::sync::Arc;

struct OutputOwner {
    _directory: tempfile::TempDir,
    _bootstrap: Arc<NativeBootstrap>,
    _compilation: Arc<native::queries::compilation_tasks::CompilationTasks>,
    _source: Arc<FrontNativeProducts>,
}

/// The compiler installs this only for its selected Build policy. Launch
/// readiness additionally requires the separately produced closed runtime.
pub(super) struct NativeBuildReadiness {
    bootstrap: Arc<NativeBootstrap>,
    request: NativeRequest,
    store: Arc<dyn ResultStore>,
    toolchain: Arc<native::Toolchain>,
}
impl NativeBuildReadiness {
    pub fn new(
        bootstrap: Arc<NativeBootstrap>,
        request: NativeRequest,
        store: Arc<dyn ResultStore>,
        toolchain: Arc<native::Toolchain>,
    ) -> Result<Self, NativeError> {
        // Evidence requires its actual assessment/discharge producer. An
        // uninstalled proof root can never become an empty successful report.
        if request.evidence.is_some() {
            return Err(NativeError::new(
                StageName::Manifest,
                "native proof assessment root is not installed",
            ));
        }
        Ok(Self {
            bootstrap,
            request,
            store,
            toolchain,
        })
    }
    pub fn materialize(&self, selected: ProductRef) -> Result<Arc<NativeExecution>, NativeError> {
        let failed = |message| NativeError::new(StageName::Link, message);
        let (compilation, product, source) = self.bootstrap.outcome(selected).map_err(failed)?;
        let [linked] = compilation.outputs(product).map_err(failed)? else {
            return Err(failed(
                "native Build readiness requires one selected backend".into(),
            ));
        };
        if self
            .request
            .definition
            .as_ref()
            .is_some_and(|entry| entry != &source.run.entry)
        {
            return Err(failed(
                "native Build readiness received another source entry".into(),
            ));
        }
        let observation = compilation.observation(product).map_err(failed)?;
        let signature = source
            .signatures
            .signature(DefId::intern(&source.run.entry))
            .and_then(Result::ok)
            .cloned()
            .ok_or_else(|| failed("selected native source signature is unavailable".into()))?;
        let directory = tempfile::tempdir().map_err(|error| failed(error.to_string()))?;
        let executable = directory.path().join("program");
        let artifact =
            object_queries::linked_product(self.store.as_ref(), *linked).map_err(failed)?;
        object_queries::materialize_executable(self.store.as_ref(), &artifact, &executable)
            .map_err(failed)?;
        let owner = Arc::new(OutputOwner {
            _directory: directory,
            _bootstrap: self.bootstrap.clone(),
            _compilation: compilation.clone(),
            _source: source.clone(),
        });
        Ok(Arc::new(NativeExecution {
            executable,
            product: *linked,
            store: self.store.clone(),
            observation,
            report: vec![],
            entry: source.run.entry.clone(),
            signature,
            sources: source.roots.sources.clone().retain_owner(owner.clone()),
            revision: source.run.revision.clone(),
            request: self.request.clone(),
            types: source.signatures.types().clone(),
            code: source.run.code.clone(),
            callees: source.run.callees.clone(),
            toolchain: self.toolchain.clone(),
            run: None,
            foreign_unreplayable: source.roots.foreign_unreplayable,
            foreign: source.run.foreign.clone(),
            owner,
        }))
    }
}
