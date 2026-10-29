//! Build readiness over the bootstrap's exact completed native dependency
//! scope. Reopening descriptors and publishing an executable invoke no source,
//! Instance, Ready or Codegen producer and convey no new V/C authority.
use super::{NativeExecution, NativeRequest, native_bootstrap::NativeBootstrap};
use crate::native::{self, NativeError, StageName, object_queries};
use hirsel_engine::{FrontNativeProducts, runtime::ProductRef, store::ResultStore};
use hirsel_syntax::{ContentHash, DefId};
use std::sync::{Arc, Mutex};

struct OutputOwner {
    _allowance: hirsel_engine::runtime::MemoryReservation,
    _directory: tempfile::TempDir,
    _bootstrap: Arc<NativeBootstrap>,
    _compilation: Arc<native::queries::compilation_tasks::CompilationTasks>,
    _source: Arc<FrontNativeProducts>,
    _platform: Arc<super::native_link::FrozenPlatform>,
}

/// The compiler installs this only for its selected Build policy. Launch
/// readiness additionally requires the separately produced closed runtime.
pub(super) struct NativeBuildReadiness {
    bootstrap: Arc<NativeBootstrap>,
    request: NativeRequest,
    store: Arc<dyn ResultStore>,
    toolchain: Arc<native::Toolchain>,
    platform: Option<Arc<super::native_link::FrozenPlatform>>,
    completed: Mutex<Option<Arc<NativeExecution>>>,
    objects: Mutex<Option<Arc<super::NativeObjectOutput>>>,
}
impl NativeBuildReadiness {
    pub fn new(
        bootstrap: Arc<NativeBootstrap>,
        request: NativeRequest,
        toolchain: Arc<native::Toolchain>,
        platform: Option<Arc<super::native_link::FrozenPlatform>>,
    ) -> Result<Self, NativeError> {
        let store = bootstrap.selected_store();
        Ok(Self {
            bootstrap,
            request,
            store,
            toolchain,
            platform,
            completed: Mutex::new(None),
            objects: Mutex::new(None),
        })
    }
    pub fn objects(
        &self,
        selected: ProductRef,
    ) -> Result<Arc<super::NativeObjectOutput>, NativeError> {
        let failed = |message| NativeError::new(StageName::Emission, message);
        if !matches!(self.request.output, super::NativeOutput::Objects { .. }) {
            return Err(failed("native request did not select object output".into()));
        }
        let (compilation, product, source) = self.bootstrap.outcome(selected).map_err(failed)?;
        let mut completed = self
            .objects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let products = compilation
            .outputs(product.clone())
            .map_err(failed)?
            .to_vec();
        for selected in &products {
            let fragment = object_queries::fragment_product(self.store.as_ref(), selected.clone())
                .map_err(failed)?;
            // Reopen the actual completed backend bytes, with the ordinary
            // fragment/object/domain checks. No output is treated as a Link.
            object_queries::load_object(self.store.as_ref(), fragment.object).map_err(failed)?;
        }
        if let Some(output) = completed.as_ref() {
            return Ok(output.clone());
        }
        let instances = compilation.emitted_instances(product).map_err(failed)?;
        let mut weight = hirsel_engine::NativeMetadataWeight::default();
        weight.add(
            std::mem::size_of::<super::NativeObjectOutput>() + 6 * std::mem::size_of::<usize>(),
        );
        weight.flat_vec(&products);
        weight.add(std::mem::size_of::<Self>());
        request_metadata(&mut weight, &self.request);
        let allowance = self
            .bootstrap
            .reserve_output(weight.bytes())
            .map_err(failed)?;
        let output = Arc::new(super::NativeObjectOutput {
            products,
            instances,
            store: self.store.clone(),
            _owner: Arc::new((allowance, self.bootstrap.clone(), compilation, source)),
        });
        *completed = Some(output.clone());
        Ok(output)
    }
    pub fn materialize(&self, selected: ProductRef) -> Result<Arc<NativeExecution>, NativeError> {
        if !matches!(self.request.output, super::NativeOutput::Executable) {
            return Err(NativeError::new(
                StageName::Link,
                "object output is not executable",
            ));
        }
        let failed = |message| NativeError::new(StageName::Link, message);
        let platform = self
            .platform
            .as_ref()
            .ok_or_else(|| failed("native executable has no platform".into()))?;
        let (compilation, product, source) = self.bootstrap.outcome(selected).map_err(failed)?;
        let mut completed = self
            .completed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(execution) = completed.as_ref() {
            let artifact =
                object_queries::linked_product(self.store.as_ref(), execution.product.clone())
                    .map_err(failed)?;
            object_queries::materialize_executable(
                self.store.as_ref(),
                &artifact,
                &execution.executable,
            )
            .map_err(failed)?;
            platform
                .validate_image(&execution.executable)
                .map_err(failed)?;
            return Ok(execution.clone());
        }
        let [linked] = compilation.outputs(product.clone()).map_err(failed)? else {
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
        let observation = compilation.observation(product.clone()).map_err(failed)?;
        // Report the removals the completed discovery actually validated;
        // the assessed plan alone never certifies a report.
        let report = match (&self.request.evidence, self.bootstrap.proofs()) {
            (None, _) => vec![],
            (Some(_), Some(proofs)) => {
                proofs.report_discharged(&compilation.discharged(product).map_err(failed)?)
            }
            (Some(_), None) => {
                return Err(failed("native evidence has no selected assessment".into()));
            }
        };
        let signature = source
            .signatures
            .signature(DefId::intern(&source.run.entry))
            .and_then(Result::ok)
            .cloned()
            .ok_or_else(|| failed("selected native source signature is unavailable".into()))?;
        let directory = tempfile::tempdir().map_err(|error| failed(error.to_string()))?;
        let executable = directory.path().join("program");
        let artifact =
            object_queries::linked_product(self.store.as_ref(), linked.clone()).map_err(failed)?;
        object_queries::materialize_executable(self.store.as_ref(), &artifact, &executable)
            .map_err(failed)?;
        platform.validate_image(&executable).map_err(failed)?;
        let mut copied = hirsel_engine::NativeMetadataWeight::default();
        // Inline Arc/header storage plus actual backing capacities. Signatures,
        // types and source text remain shared with the retained producer owner.
        copied.add(std::mem::size_of::<OutputOwner>() + 3 * std::mem::size_of::<usize>());
        copied.add(std::mem::size_of::<NativeExecution>() + 3 * std::mem::size_of::<usize>());
        copied.add(std::mem::size_of::<Self>());
        copied.add(executable.capacity());
        copied.path(&source.run.entry);
        copied.source_snapshot(&source.roots.sources);
        copied.program_revision(&source.run.revision);
        copied.code_reach(&source.run.code);
        copied.callees(&source.run.callees);
        copied.foreign_use(&source.run.foreign);
        copied.vec(&observation.failure_rules, |w, rule| w.add(rule.capacity()));
        copied.vec(&report, |w, line| w.add(line.capacity()));
        copied.tree::<u64, (DefId, hirsel_lir::ir::SiteRange)>(observation.sites.len());
        copied.map(&observation.anchors, |w, path, anchors| {
            w.path(path);
            w.flat_vec(anchors);
        });
        request_metadata(&mut copied, &self.request);
        // Readiness retains its own request copy in addition to the returned
        // descriptor's copy. Both backing stores share this original charge.
        request_metadata(&mut copied, &self.request);
        let allowance = self
            .bootstrap
            .reserve_output(copied.bytes())
            .map_err(failed)?;
        let owner = Arc::new(OutputOwner {
            _allowance: allowance,
            _directory: directory,
            _bootstrap: self.bootstrap.clone(),
            _compilation: compilation.clone(),
            _source: source.clone(),
            _platform: platform.clone(),
        });
        let execution = Arc::new(NativeExecution {
            executable,
            product: linked.clone(),
            platform: platform.clone(),
            store: self.store.clone(),
            observation,
            report,
            entry: source.run.entry.clone(),
            signature,
            sources: source.roots.sources.clone().retain_owner(owner.clone()),
            revision: source.run.revision.clone(),
            request: self.request.clone(),
            types: source.signatures.types().clone(),
            code: source.run.code.clone(),
            callees: source.run.callees.clone(),
            toolchain: self.toolchain.clone(),
            run: self.request.run.clone(),
            foreign_unreplayable: source.roots.foreign_unreplayable,
            foreign: source.run.foreign.clone(),
            owner,
        });
        *completed = Some(execution.clone());
        Ok(execution)
    }
}

/// Canonical identity of selected evidence: every record field in sidecar
/// order and the trusted checkers. Equal identities assess equal sidecars.
pub(super) fn evidence_identity(evidence: &native::Evidence) -> ContentHash {
    let records = evidence
        .records
        .iter()
        .map(|record| {
            (
                record.revision,
                &record.obligation,
                record.checksum,
                record.checker.to_string(),
                &record.budget.unit,
                record.budget.count,
                record.verdict.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let trusted = evidence
        .trusted
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&(records, trusted))
        .unwrap_or_else(|error| unreachable!("evidence identity encodes: {error}"));
    ContentHash::of_parts("hirsel.host.native-evidence.v1", [bytes.as_slice()])
}

pub(super) fn request_metadata(
    weight: &mut hirsel_engine::NativeMetadataWeight,
    request: &NativeRequest,
) {
    if let Some(path) = &request.definition {
        weight.path(path);
    }
    if let super::NativeOutput::Objects { roots } = &request.output {
        weight.vec(roots, |weight, path| weight.path(path));
    }
    weight.vec(&request.test_entries, |weight, (path, _)| weight.path(path));
    weight.vec(&request.supplemental_sources, |weight, source| {
        weight.add(source.name.capacity());
        weight.add(source.context.package.name.capacity());
        weight.add(source.context.package.version.capacity());
        weight.vec(&source.context.modules, |weight, module| {
            weight.add(module.capacity())
        });
        // Text is shared caller input. Parsed source storage is charged with
        // the front source inventory before any native factory is installed.
    });
    let target = &request.target;
    weight.add(target.triple.capacity());
    weight.add(target.cpu.capacity());
    weight.vec(&target.features, |w, feature| w.add(feature.capacity()));
    weight.add(target.llvm_data_layout.capacity());
    if let Some(scheduler) = &request.launch.scheduler {
        weight.add(scheduler.algorithm.capacity());
    }
    if let Some(scripts) = &request.launch.fixture_file_faults {
        weight.map(scripts, |w, _, queue| {
            w.add(
                queue
                    .capacity()
                    .saturating_mul(std::mem::size_of::<hirsel_run_schema::FileFault>()),
            );
        });
    }
    // Validated run, evidence and toolchain Arcs retain shared caller inputs;
    // they are not cloned backing storage of this execution descriptor.
}
