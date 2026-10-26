//! Host Link executor over a selected stored tool, pre-codegen kit snapshot and
//! ordinary archive references. The shared native planner owns launcher/glue,
//! manifest and ABI checks; this adapter owns neither compiler bodies nor caches.
use crate::native::{
    self, Backend,
    object_queries::{
        self,
        capture::CaptureLimits,
        platform::CapturedPlatform,
        runtime::{RuntimeFiles, RuntimeLimits},
    },
    queries::{
        compilation_tasks::CompilationBackend,
        link_tasks::{CodegenPartition, LinkExecution, NativeLinkExecutor},
        roots::{RootCapturedExecutableInputs, RootProducts},
    },
};
use hirsel_engine::{runtime::MemoryReservation, store::ResultStore};
use hirsel_syntax::{ContentHash, DefPath};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// The capability comes only from selected-input capture and closed target
/// resolution. Its original metadata reservation transfers into this executor.
pub(super) struct LinkInputs {
    pub platform: CapturedPlatform,
    pub emulator: Option<PathBuf>,
    pub timeout: Duration,
    pub runtime_limits: RuntimeLimits,
    pub limits: CaptureLimits,
}
pub(crate) struct FrozenPlatform {
    store: Arc<dyn ResultStore>,
    inputs: LinkInputs,
    entry: DefPath,
    backend: Backend,
    target: native::TargetSpec,
    requested: Option<Arc<hirsel_run_config::ValidatedRun>>,
    linker_options: ContentHash,
    _allowance: Arc<MemoryReservation>,
}
impl FrozenPlatform {
    pub fn new(
        store: Arc<dyn ResultStore>,
        inputs: LinkInputs,
        entry: DefPath,
        backend: Backend,
        target: native::TargetSpec,
        requested: Option<Arc<hirsel_run_config::ValidatedRun>>,
        allowance: impl Into<Arc<MemoryReservation>>,
    ) -> Result<Arc<Self>, String> {
        let allowance = allowance.into();
        if inputs.limits.archive_bytes == 0 || inputs.limits.files == 0 {
            return Err("invalid selected native platform input bounds".into());
        }
        inputs
            .platform
            .runtime()
            .validate(store.as_ref(), &target, inputs.runtime_limits)?;
        let mut weight = hirsel_engine::NativeMetadataWeight::default();
        weight.add(std::mem::size_of::<Self>());
        weight.path(&entry);
        weight.add(target.triple.capacity());
        if let Some(emulator) = &inputs.emulator {
            weight.add(emulator.capacity());
        }
        let metadata = weight
            .bytes()
            .checked_add(inputs.platform.retained_metadata_bytes())
            .ok_or("selected native platform metadata overflow")?;
        if metadata > allowance.bytes() {
            return Err("selected native platform metadata exceeds admission".into());
        }
        let encoded = serde_json::to_vec(&(
            2u32,
            inputs.platform.identity(),
            target.digest(),
            backend.name(),
            &entry,
            requested.as_ref().map(|run| run.identity()),
        ))
        .map_err(|error| error.to_string())?;
        let linker_options =
            ContentHash::of_parts("hirsel.host.frozen-platform.v2", [encoded.as_slice()]);
        Ok(Arc::new(Self {
            store,
            inputs,
            entry,
            backend,
            target,
            requested,
            linker_options,
            _allowance: allowance,
        }))
    }
    pub fn identity(&self) -> ContentHash {
        self.linker_options
    }
    /// Run this for reopened Link hits too, before exposing any executable.
    pub fn validate_image(&self, image: &Path) -> Result<(), String> {
        self.inputs.platform.validate_image(
            self.store.as_ref(),
            &self.target,
            image,
            self.inputs.emulator.as_deref(),
            self.inputs.timeout,
            self.inputs.runtime_limits,
        )
    }
    pub fn runtime_files(&self, image: &Path) -> Result<RuntimeFiles, String> {
        self.validate_image(image)?;
        self.inputs.platform.runtime().materialize(
            self.store.as_ref(),
            &self.target,
            self.inputs.runtime_limits,
        )
    }
    pub fn executable(&self, path: PathBuf) -> Result<native::Runnable, String> {
        let runtime = self.runtime_files(&path)?;
        Ok(native::Runnable::closed(
            path,
            runtime,
            self.inputs.emulator.clone(),
        ))
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
            runtime_archives: vec![self.inputs.platform.runtime_archive().content],
            foreign_inputs: self
                .inputs
                .platform
                .foreign()
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
            || input.inputs.runtime_archives != [self.inputs.platform.runtime_archive().content]
            || !input.inputs.foreign_inputs.iter().copied().eq(self
                .inputs
                .platform
                .foreign()
                .iter()
                .map(|input| input.content))
        {
            return Err("native Link received another selected platform".into());
        }
        let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
        let tests = !native.test_entries(input.root)?.is_empty();
        let inputs = RootCapturedExecutableInputs {
            entry: &self.entry,
            backend: self.backend,
            objects: input.objects,
            directory: scratch.path(),
            requested: self.requested.as_deref(),
            standalone: self.requested.is_none(),
            platform: native::CapturedExecutableLinkInputs {
                store: self.store.as_ref(),
                platform: &self.inputs.platform,
                loader: None,
            },
        };
        let plan = if tests {
            native.plan_captured_tests(input.root, input.ready, inputs)
        } else {
            native.plan_captured_executable(input.root, input.ready, inputs)
        }?;
        // Foreign code compiled against another data ABI is PROG-4's refusal.
        let refused = |error: String| plan.refusal(&error).unwrap_or(error);
        let capture = plan
            .capture(
                self.store.as_ref(),
                &self.inputs.platform,
                self.inputs.limits,
            )
            .map_err(refused)?;
        capture.validate(self.store.as_ref())?;
        let executable = scratch.path().join("program");
        capture
            .link(self.store.as_ref(), &executable)
            .map_err(refused)?;
        self.validate_image(&executable)?;
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
