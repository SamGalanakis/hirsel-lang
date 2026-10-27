//! Compiler-owned selection of the diagnostic source/native root. Ordinary
//! native builds retain their existing factories, products and byte encodings.
use super::{
    OwnershipRequest,
    native_routing::NativeRouting,
    ownership_admission::{Admission, Footprint, SourceInputs},
    ownership_bootstrap::{self, OwnershipBootstrap},
};
use crate::native::{
    self,
    queries::{ownership_tasks::OwnershipTasks, ready::ObservedReady},
};
use hirsel_engine::{
    FrontNativeProducts, FrontNativeQueries, LoadedProgram, ToolError,
    runtime::*,
    store::{Reference, VerifyStore},
};
use hirsel_syntax::{ContentHash, DefPath};
use std::sync::{Arc, OnceLock};

pub struct OwnershipExecution {
    pub entry: DefPath,
    pub sources: hirsel_span::SourceMap,
    pub revision: hirsel_run_schema::ProgramRevision,
    pub product: ProductRef,
    units: Vec<ProductRef>,
    tasks: Arc<OwnershipTasks>,
    root: ProductRef,
    link: Arc<native::root_source::NativeLinkSource>,
    _owner: Arc<OutputOwner>,
}
struct OutputOwner {
    _bootstrap: Arc<OwnershipBootstrap>,
    _source: Arc<FrontNativeProducts>,
    _allowance: ownership_bootstrap::OutputReservation,
}
/// Objects remain bound to one selected diagnostic completion and retain its
/// original owners and an independent debit from the existing output grant.
pub struct OwnershipObjects {
    objects: native::ownership_trace::LeafObjects,
    root: ProductRef,
    _owner: Arc<OwnershipExecution>,
    _allowance: ownership_bootstrap::OutputReservation,
}
impl OwnershipObjects {
    pub fn objects(&self) -> &[hirsel_lir::stage::ObjectArtifact] {
        self.objects.objects()
    }
    pub fn metadata(&self) -> &hirsel_run_schema::ownership_trace::native::Units {
        self.objects.metadata()
    }
}
impl OwnershipExecution {
    pub fn units(&self) -> &[ProductRef] {
        &self.units
    }
    /// Emit from the exact source/instance/arena diagnostic closure, preserving
    /// original Value marker ids through checked rebinding. Ordinary objects
    /// and a second source compilation cannot substitute for these products.
    pub fn emit(self: &Arc<Self>, backend: native::Backend) -> Result<OwnershipObjects, String> {
        let objects = self.tasks.with_link_context(
            self.root,
            &self.link,
            |program, units, inputs, plans| {
                native::ownership_trace::emit_relocated(
                    program,
                    &self.entry,
                    units,
                    inputs,
                    plans,
                    &self.link.sites(),
                    backend,
                )
                .map_err(|error| error.to_string())
            },
        )?;
        let bytes = objects
            .retained_bytes()
            .saturating_add(std::mem::size_of::<OwnershipObjects>() as u64);
        let allowance = self._owner._bootstrap.reserve_output(bytes)?;
        Ok(OwnershipObjects {
            objects,
            root: self.root,
            _owner: self.clone(),
            _allowance: allowance,
        })
    }
    pub fn launch(
        &self,
        objects: &OwnershipObjects,
        tools: &native::Toolchain,
        directory: &std::path::Path,
        event_limit: usize,
    ) -> Result<native::ownership_trace::LeafExecution, String> {
        if objects.root != self.root || !std::ptr::eq(&*objects._owner, self) {
            return Err("ownership objects belong to another selected completion".into());
        }
        self.tasks
            .with_link_context(self.root, &self.link, |program, _, _, _| {
                objects
                    .objects
                    .launch(program, tools, directory, event_limit)
                    .map_err(|error| error.to_string())
            })
    }
    /// Bind explicit grants or a replay to the same immutable source facts
    /// used by this diagnostic compilation. This performs no compiler queries.
    pub fn launch_with(
        &self,
        objects: &OwnershipObjects,
        tools: &native::Toolchain,
        directory: &std::path::Path,
        event_limit: usize,
        launch: crate::HostLaunch<'_>,
    ) -> Result<native::ownership_trace::LeafExecution, String> {
        use crate::{BoundHostRun, launch_binding::LaunchSelection};
        if objects.root != self.root || !std::ptr::eq(&*objects._owner, self) {
            return Err("ownership objects belong to another selected completion".into());
        }
        if matches!(launch, crate::HostLaunch::Callee { .. }) {
            return Err("ownership launch does not execute runner callees".into());
        }
        let source = &self._owner._source;
        let selection = LaunchSelection::read(launch).map_err(|e| e.to_string())?;
        if selection.requires_replayable(launch) && !source.run.foreign.unmodeled.is_empty() {
            return Err("ownership recording requires replayable source effects".into());
        }
        let signature = source
            .signatures
            .signature(hirsel_syntax::DefId::intern(&self.entry))
            .and_then(Result::ok)
            .ok_or("ownership launch has no selected source signature")?;
        let bound = selection
            .bind(
                launch,
                source.signatures.types(),
                signature,
                &self.revision,
                source.run.code.clone(),
            )
            .map_err(|e| e.to_string())?;
        let (run, tape) = match (bound, launch) {
            (
                BoundHostRun::Live(bound),
                crate::HostLaunch::Live {
                    record,
                    revocations,
                    ..
                },
            ) => {
                hirsel_rt::host_launch::recording_level(bound.run(), record.is_some())
                    .map_err(|e| e.to_string())?;
                (
                    bound.into_run(),
                    native::Tape::Live {
                        record,
                        revocations,
                    },
                )
            }
            (BoundHostRun::Replay { run, .. }, crate::HostLaunch::Replay(path)) => {
                (run, native::Tape::Replay(path))
            }
            _ => return Err("ownership launch binding changed form".into()),
        };
        self.tasks
            .with_link_context(self.root, &self.link, |program, _, _, _| {
                objects
                    .objects
                    .launch_selected(program, tools, directory, event_limit, Some(&run), tape)
                    .map_err(|e| e.to_string())
            })
    }
    /// Decode the ordinary launch record against this exact selected source
    /// and object owner. Ownership rows and exit codes cannot invent a failure
    /// rule, source attribution or a clean runtime audit.
    pub fn observe_run(
        &self,
        objects: &OwnershipObjects,
        launched: &native::Launched,
    ) -> Result<hirsel_run_schema::RunOutcome, String> {
        if objects.root != self.root || !std::ptr::eq(&*objects._owner, self) {
            return Err("ownership objects belong to another selected completion".into());
        }
        self.tasks
            .with_link_context(self.root, &self.link, |program, _, _, _| {
                let sites = self.link.sites();
                match native::observe_run(launched, program, &sites)
                    .map_err(|error| error.to_string())?
                {
                    native::Observed::Ran(observation, waits) => observation
                        .outcome(&sites, waits)
                        .map_err(|error| error.to_string()),
                    native::Observed::Refused(reason) => {
                        Ok(hirsel_run_schema::RunOutcome::Refused(reason))
                    }
                    native::Observed::Stopped(outcome) => Ok(outcome),
                }
            })
    }
    /// Each selected body re-enters C before a diagnostic consumer sees it.
    /// This read never runs source discovery or compiler transformations.
    pub fn with_unit<R>(
        &self,
        unit: ProductRef,
        use_unit: impl FnOnce(&ObservedReady) -> Result<R, String>,
    ) -> Result<R, String> {
        if !self.units.contains(&unit) {
            return Err("unselected ownership diagnostic output".into());
        }
        let observed = self.tasks.materialize(unit)?;
        use_unit(&observed)
    }
}
pub(super) struct OwnershipSelection {
    source: Arc<FrontNativeQueries>,
    bootstrap: Arc<OwnershipBootstrap>,
    routing: Arc<NativeRouting>,
    request_key: ContentHash,
    sources: hirsel_span::SourceMap,
    completed: OnceLock<Arc<OwnershipExecution>>,
}
fn request_key(request: &OwnershipRequest) -> Result<ContentHash, ToolError> {
    let bytes = serde_json::to_vec(&(
        &request.definition,
        request.target.digest(),
        format!("{:?}", request.validation),
        &request.launch,
    ))
    .map_err(|e| ToolError::Runtime(e.to_string()))?;
    Ok(ContentHash::of_parts(
        "hirsel.host.ownership-request.v1",
        [bytes.as_slice()],
    ))
}
impl OwnershipSelection {
    pub fn new(
        program: &mut LoadedProgram,
        inputs: SourceInputs,
        request: &OwnershipRequest,
        verify_store: VerifyStore,
        footprint: &Footprint,
        allowance: &mut MemoryReservation,
        weight: MemoryWeight,
        errors: RuntimeErrors,
    ) -> Result<Arc<Self>, ToolError> {
        let sources = program.engine.sources.clone();
        let mut split = |bytes| {
            allowance.split(bytes).ok_or_else(|| {
                ToolError::Runtime("ownership partition exceeds original admission".into())
            })
        };
        let source_error = |error| ToolError::Query {
            error,
            phase: None,
            sources: sources.clone(),
        };
        let catalogue = program
            .engine
            .front_native_definition_queries_with_inputs(
                inputs.front,
                inputs.lower.clone(),
                inputs.catalogue,
                weight,
                split(footprint.catalogue)?,
                errors,
            )
            .map_err(source_error)?;
        let source = program
            .engine
            .front_native_queries(
                hirsel_engine::FrontNativePlan {
                    entry: inputs.entry,
                    roots: vec![],
                    run: inputs.run,
                    definitions: catalogue,
                },
                weight,
                split(footprint.source)?,
                errors,
            )
            .map_err(|error| program.error(error))?;
        let routing = NativeRouting::new(
            ownership_bootstrap::KINDS,
            errors.protocol,
            split(Footprint::ROUTING)?,
        )
        .map_err(|error| ToolError::Runtime(error.into()))?;
        let mut options = native::Options::new(request.target.clone());
        options.validation = request.validation;
        options.launch = request.launch.clone();
        let request_key = request_key(request)?;
        let bootstrap = OwnershipBootstrap::new(
            ownership_bootstrap::Source {
                source: source.clone(),
                lower: inputs.lower,
                definitions: inputs.definitions,
                store: inputs.store,
            },
            ownership_bootstrap::Plan {
                options,
                policy: request_key,
                verify_store,
                admission: Admission {
                    instances: split(footprint.tasks)?,
                    arena: split(footprint.tasks)?,
                    compilation: split(footprint.tasks)?,
                },
            },
            routing.clone(),
            MemoryWeight {
                retained: weight.retained.min(1 << 20),
                scratch: weight.scratch.min(16 << 20),
                stack: 0,
            },
            split(footprint.bootstrap)?,
            errors,
        )
        .map_err(ToolError::Runtime)?;
        Ok(Arc::new(Self {
            source,
            bootstrap,
            routing,
            request_key,
            sources,
            completed: OnceLock::new(),
        }))
    }
    pub fn accepts(&self, request: &OwnershipRequest) -> bool {
        request_key(request).ok() == Some(self.request_key)
    }
    pub fn query(&self) -> QueryId {
        self.bootstrap.query()
    }
    pub fn queries(&self, definitions: &[DefPath]) -> Vec<QueryId> {
        let mut queries = vec![self.query(), self.source.query()];
        for path in definitions {
            queries.extend(self.source.run_query(path));
            queries.extend(self.source.definition_query(path));
        }
        queries
    }
    pub fn factories(&self) -> Vec<(u32, Arc<dyn QueryFactory>)> {
        let mut factories = self.routing.factories();
        factories.extend([
            (hirsel_engine::FRONT_RUN_KIND, self.source.run_factory()),
            (
                hirsel_engine::FRONT_NATIVE_DEFINITION_KIND,
                self.source.definition_factory(),
            ),
            (hirsel_engine::FRONT_NATIVE_ROOT_KIND, self.source.factory()),
            (ownership_bootstrap::KIND, self.bootstrap.factory()),
        ]);
        factories
    }
    pub fn materialize(&self, selected: ProductRef) -> Result<Arc<OwnershipExecution>, ToolError> {
        let (compilation, completion, source) = self
            .bootstrap
            .outcome(selected)
            .map_err(ToolError::Runtime)?;
        let (tasks, root) = compilation
            .outcome(completion)
            .map_err(ToolError::Runtime)?;
        let units = tasks.outputs(root).map_err(ToolError::Runtime)?;
        if let Some(completed) = self.completed.get() {
            if completed.product != selected {
                return Err(ToolError::Runtime(
                    "ownership readiness received another completion".into(),
                ));
            }
            return Ok(completed.clone());
        }
        let mut copied = hirsel_engine::NativeMetadataWeight::default();
        copied.add(
            std::mem::size_of::<OwnershipExecution>()
                + std::mem::size_of::<OutputOwner>()
                + 6 * std::mem::size_of::<usize>(),
        );
        copied.flat_vec(&units);
        copied.path(&source.run.entry);
        copied.source_snapshot(&source.roots.sources);
        copied.program_revision(&source.run.revision);
        let owner = Arc::new(OutputOwner {
            _bootstrap: self.bootstrap.clone(),
            _source: source.clone(),
            _allowance: self
                .bootstrap
                .reserve_output(copied.bytes())
                .map_err(ToolError::Runtime)?,
        });
        let execution = Arc::new(OwnershipExecution {
            entry: source.run.entry.clone(),
            sources: source.roots.sources.clone().retain_owner(owner.clone()),
            revision: source.run.revision.clone(),
            product: selected,
            units,
            tasks,
            root,
            link: self.bootstrap.link_source().map_err(ToolError::Runtime)?,
            _owner: owner,
        });
        self.completed
            .set(execution.clone())
            .map_err(|_| ToolError::Runtime("ownership readiness completed twice".into()))?;
        Ok(execution)
    }
    pub fn failure(&self, reference: Reference) -> Option<ToolError> {
        if let Some(error) = self.source.failure(reference) {
            return Some(ToolError::Query {
                error,
                phase: None,
                sources: self.sources.clone(),
            });
        }
        self.bootstrap.failure(reference).map(ToolError::Runtime)
    }
}
