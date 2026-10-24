//! Host Link executor over a selected stored tool, pre-codegen kit snapshot and
//! ordinary archive references. The shared native planner owns launcher/glue,
//! manifest and ABI checks; this adapter owns neither compiler bodies nor caches.
use crate::native::{
    self, Backend,
    object_queries::{
        self,
        capture::{CaptureLimits, FrozenLinkKit, FrozenLinker},
    },
    queries::{
        compilation_tasks::CompilationBackend,
        link_tasks::{CodegenPartition, LinkExecution, NativeLinkExecutor},
        roots::{RootFrozenExecutableInputs, RootProducts},
    },
};
use hirsel_engine::{
    runtime::MemoryReservation,
    store::{Reference, ResultStore},
};
use hirsel_syntax::{ContentHash, DefPath};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Retain filename-sensitive linker selection as well as content. These names
/// come from selected inputs and never from a public request's task metadata.
pub(super) struct NamedInput {
    pub name: String,
    pub content: Reference,
}
pub(super) struct LinkInputs {
    pub tool: FrozenLinker,
    pub kit: FrozenLinkKit,
    pub runtime: NamedInput,
    pub foreign: Vec<NamedInput>,
    pub loader: Option<(PathBuf, PathBuf)>,
    pub limits: CaptureLimits,
}
pub(super) struct FrozenPlatform {
    store: Arc<dyn ResultStore>,
    inputs: LinkInputs,
    entry: DefPath,
    backend: Backend,
    requested: Option<Arc<hirsel_run_config::ValidatedRun>>,
    linker_options: ContentHash,
    _allowance: MemoryReservation,
}
impl FrozenPlatform {
    pub fn new(
        store: Arc<dyn ResultStore>,
        inputs: LinkInputs,
        entry: DefPath,
        backend: Backend,
        requested: Option<Arc<hirsel_run_config::ValidatedRun>>,
        allowance: MemoryReservation,
    ) -> Result<Arc<Self>, String> {
        let valid_name = |name: &str| {
            let mut parts = Path::new(name).components();
            matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none()
        };
        if !valid_name(&inputs.runtime.name)
            || inputs.foreign.iter().any(|input| !valid_name(&input.name))
            || inputs.limits.archive_bytes == 0
            || inputs.limits.files == 0
        {
            return Err("invalid selected native platform input bounds or filename".into());
        }
        let mut weight = hirsel_engine::NativeMetadataWeight::default();
        weight.add(std::mem::size_of::<Self>());
        weight.path(&entry);
        weight.add(inputs.runtime.name.capacity());
        weight.vec(&inputs.foreign, |weight, input| {
            weight.add(input.name.capacity())
        });
        if let Some((loader, libraries)) = &inputs.loader {
            weight.add(loader.capacity());
            weight.add(libraries.capacity());
        }
        if weight.bytes() > allowance.bytes() {
            return Err("selected native platform metadata exceeds admission".into());
        }
        let encoded = serde_json::to_vec(&(
            1u32,
            inputs.tool.reference(),
            inputs.kit.reference(),
            backend.name(),
            &entry,
            (&inputs.runtime.name, inputs.runtime.content),
            inputs
                .foreign
                .iter()
                .map(|input| (&input.name, input.content))
                .collect::<Vec<_>>(),
            &inputs.loader,
            requested.as_ref().map(|run| run.identity()),
        ))
        .map_err(|error| error.to_string())?;
        let linker_options =
            ContentHash::of_parts("hirsel.host.frozen-platform.v1", [encoded.as_slice()]);
        Ok(Arc::new(Self {
            store,
            inputs,
            entry,
            backend,
            requested,
            linker_options,
            _allowance: allowance,
        }))
    }
    pub fn backend(
        self: &Arc<Self>,
        backend_version: ContentHash,
        target_features: ContentHash,
        profile_options: ContentHash,
        runtime_abi: ContentHash,
    ) -> CompilationBackend {
        CompilationBackend {
            backend: self.backend.name().into(),
            backend_version,
            target_features,
            profile_options,
            runtime_abi,
            linker_options: self.linker_options,
            runtime_archives: vec![self.inputs.runtime.content],
            foreign_inputs: self
                .inputs
                .foreign
                .iter()
                .map(|input| input.content)
                .collect(),
            executor: self.clone(),
        }
    }
}
impl NativeLinkExecutor for FrozenPlatform {
    fn codegen_partition(&self) -> CodegenPartition {
        match self.backend {
            Backend::Llvm => CodegenPartition::WholeModule,
            Backend::Cranelift => CodegenPartition::PerInstance,
        }
    }
    fn link(&self, native: &mut RootProducts, input: LinkExecution<'_>) -> Result<Vec<u8>, String> {
        if input.key.linker_options != self.linker_options
            || input.inputs.runtime_archives != [self.inputs.runtime.content]
            || !input.inputs.foreign_inputs.iter().copied().eq(self
                .inputs
                .foreign
                .iter()
                .map(|input| input.content))
        {
            return Err("native Link received another selected platform".into());
        }
        let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
        let runtime_dir = scratch.path().join("runtime");
        let foreign_dir = scratch.path().join("foreign");
        std::fs::create_dir(&runtime_dir).map_err(|error| error.to_string())?;
        std::fs::create_dir(&foreign_dir).map_err(|error| error.to_string())?;
        let runtime = runtime_dir.join(&self.inputs.runtime.name);
        object_queries::materialize_link_input(
            self.store.as_ref(),
            self.inputs.runtime.content,
            &runtime,
            false,
        )?;
        let foreign = self
            .inputs
            .foreign
            .iter()
            .enumerate()
            .map(|(index, input)| {
                let directory = foreign_dir.join(index.to_string());
                std::fs::create_dir(&directory).map_err(|error| error.to_string())?;
                let path = directory.join(&input.name);
                object_queries::materialize_link_input(
                    self.store.as_ref(),
                    input.content,
                    &path,
                    false,
                )?;
                Ok(path)
            })
            .collect::<Result<Vec<_>, String>>()?;
        let plan = native.plan_frozen_executable(
            input.root,
            input.ready,
            RootFrozenExecutableInputs {
                entry: &self.entry,
                backend: self.backend,
                objects: input.objects,
                directory: scratch.path(),
                requested: self.requested.as_deref(),
                standalone: self.requested.is_none(),
                platform: native::FrozenExecutableLinkInputs {
                    store: self.store.as_ref(),
                    kit: self.inputs.kit,
                    runtime: &runtime,
                    foreign: &foreign,
                    loader: self.inputs.loader.clone(),
                },
            },
        )?;
        let capture = plan.capture(self.store.as_ref(), self.inputs.tool, self.inputs.limits)?;
        capture.validate(self.store.as_ref())?;
        let executable = scratch.path().join("program");
        capture.link(self.store.as_ref(), &executable)?;
        let limit = self.inputs.limits.archive_bytes;
        let mut bytes = Vec::new();
        std::fs::File::open(executable)
            .map_err(|error| error.to_string())?
            .take(
                limit
                    .checked_add(1)
                    .ok_or("native executable admission overflow")?,
            )
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > limit {
            return Err("native executable exceeds admitted Link bytes".into());
        }
        Ok(bytes)
    }
}
