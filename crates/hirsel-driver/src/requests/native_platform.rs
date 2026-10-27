//! Capture compiler-selected platform inputs and emission policy before native
//! root selection. This layer uses the ordinary native library resolver and
//! captured platform producer; it performs no source or native compiler work.
use super::{
    NativeRequest,
    native_bootstrap::NativePlan,
    native_link::{FrozenPlatform, LinkInputs},
};
use crate::native::{
    self, Backend, BuildProfile, NativeError, StageName, Toolchain,
    object_queries::{
        capture::{CaptureLimits, FrozenLinkKit, FrozenLinker},
        platform::{CapturedPlatform, PlatformCaptureInputs, RuntimeSelection},
        runtime::RuntimeLimits,
    },
};
use hirsel_engine::{LinkInput, runtime::MemoryReservation, store::ResultStore};
use hirsel_syntax::{ContentHash, DefPath};
use std::{path::PathBuf, sync::Arc};

/// Explicit package selection, captured by the host before input observation.
/// No tool or loader is discovered by an admitted producer's factory.
#[derive(Clone)]
pub(super) struct NativePlatformPolicy {
    pub linker: PathBuf,
    pub auditors: PathBuf,
    pub floor: PathBuf,
}
impl NativePlatformPolicy {
    pub fn capture() -> Result<Self, NativeError> {
        let selected = |name| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .ok_or_else(|| NativeError::new(StageName::Link, format!("missing {name}")))
        };
        Ok(Self {
            linker: selected("HIRSEL_CAPTURE_LINKER")?,
            auditors: selected("HIRSEL_AUDITOR_PACKAGE")?,
            floor: selected("HIRSEL_GLIBC_FLOOR")?,
        })
    }
}
/// The operation supplies emission intent independently of launch records.
/// A bound executable build can also carry a validated run.
#[derive(Clone, Copy)]
pub(super) enum NativePurpose {
    Build,
    Run,
}
impl NativePurpose {
    pub fn opt(self, request: &NativeRequest) -> native::OptLevel {
        if let Some(optimization) = request.optimization {
            return optimization;
        }
        match (self, request.profile, request.backend) {
            // Preserve tool::run's interactive Cranelift policy. Executable
            // builds retain their explicit profile policy on both backends.
            (Self::Run, BuildProfile::Default, Backend::Cranelift) => native::OptLevel::None,
            _ => request.profile.opt(),
        }
    }
}
pub(super) fn profile_options(
    request: &NativeRequest,
    purpose: NativePurpose,
) -> Result<ContentHash, NativeError> {
    let profile = match request.profile {
        BuildProfile::Default => 0u8,
        BuildProfile::Debug => 1,
        BuildProfile::Release => 2,
        BuildProfile::ReleaseLto => 3,
    };
    let opt = match purpose.opt(request) {
        native::OptLevel::None => 0u8,
        native::OptLevel::Default => 1,
        native::OptLevel::Speed => 2,
    };
    let bytes = serde_json::to_vec(&(profile, request.fuel, opt))
        .map_err(|error| NativeError::new(StageName::Emission, error.to_string()))?;
    Ok(ContentHash::of_parts(
        "hirsel.host.native-profile.v2",
        [bytes.as_slice()],
    ))
}
pub(super) struct NativePlatformInputs<'a> {
    pub purpose: NativePurpose,
    pub verify_store: hirsel_engine::store::VerifyStore,
    pub request: &'a NativeRequest,
    pub entry: &'a DefPath,
    pub tools: &'a Toolchain,
    pub links: &'a [LinkInput],
    pub policy: Option<&'a NativePlatformPolicy>,
}
pub(super) struct NativePlatformSelection {
    pub plan: NativePlan,
    pub platform: Option<Arc<FrozenPlatform>>,
}
impl NativePlatformSelection {
    pub fn capture(
        store: Arc<dyn ResultStore>,
        inputs: NativePlatformInputs<'_>,
        limits: CaptureLimits,
        runtime_limits: RuntimeLimits,
        allowance: impl Into<Arc<MemoryReservation>>,
    ) -> Result<Self, NativeError> {
        let request = inputs.request;
        let failed = |message| NativeError::new(StageName::Link, message);
        if matches!(
            request.profile,
            BuildProfile::Release | BuildProfile::ReleaseLto
        ) && request.backend == Backend::Cranelift
        {
            return Err(NativeError::new(
                StageName::Emission,
                "release builds use LLVM; select LLVM or a non-release profile",
            ));
        }
        if request
            .run
            .as_ref()
            .is_some_and(|run| run.record().inputs != request.launch)
        {
            return Err(NativeError::new(
                StageName::Manifest,
                "selected native launch inputs differ from the validated run",
            ));
        }
        let target = &request.target;
        if let super::NativeOutput::Objects { roots } = &request.output {
            if roots.is_empty()
                || !request.test_entries.is_empty()
                || request.run.is_some()
                || matches!(inputs.purpose, NativePurpose::Run)
            {
                return Err(failed(
                    "object emission needs explicit roots and no launch selection".into(),
                ));
            }
            let profile_options = profile_options(request, inputs.purpose)?;
            let mut options = request.profile.options(target.clone());
            options.validation = request.validation;
            return Ok(Self {
                plan: NativePlan {
                    options,
                    policy: profile_options,
                    verify_store: if request.validation == native::Validation::EveryStage {
                        hirsel_engine::store::VerifyStore::Recompute
                    } else {
                        inputs.verify_store
                    },
                    opt: inputs.purpose.opt(request),
                    debug: request.profile == BuildProfile::Debug,
                    verify_backend: true,
                    measurement: inputs.tools.runtime_build.measurement,
                    fuel: request.fuel,
                    requested_run: None,
                    test_entries: Vec::new(),
                    emit_only: true,
                    backends: vec![
                        crate::native::queries::compilation_tasks::CompilationBackend {
                            backend: request.backend.name().into(),
                            backend_version: native::backend_version(),
                            target_features: target.digest(),
                            profile_options,
                            runtime_abi: ContentHash::of_bytes(
                                &hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes(),
                            ),
                            linker_options: ContentHash::of_bytes(
                                b"native object output has no linker",
                            ),
                            runtime_archives: vec![],
                            foreign_inputs: vec![],
                            executor: Arc::new(ObjectOutput(request.backend)),
                        },
                    ],
                },
                platform: None,
            });
        }
        let policy = inputs
            .policy
            .ok_or_else(|| failed("native executable has no selected platform policy".into()))?;
        let (loader_name, emulator) = match target.triple.as_str() {
            "x86_64-unknown-linux-gnu" => ("ld-linux-x86-64.so.2", None),
            "aarch64-unknown-linux-gnu" => (
                "ld-linux-aarch64.so.1",
                Some(
                    inputs
                        .tools
                        .qemu_aarch64
                        .clone()
                        .ok_or_else(|| failed("missing selected AArch64 emulator".into()))?,
                ),
            ),
            _ => {
                return Err(failed(
                    "native executable capture requires a supported ELF target".into(),
                ));
            }
        };
        let floor = inputs
            .tools
            .glibc_floor
            .as_ref()
            .unwrap_or(&policy.floor)
            .join(&target.triple);
        let (loader, libraries) = inputs
            .tools
            .loader(target)?
            .unwrap_or_else(|| (floor.join(loader_name), floor));
        let auditor = policy.auditors.join(format!("{}.so", target.triple));
        let stripped = inputs
            .tools
            .archive(&target.triple)
            .ok_or_else(|| failed(format!("no runtime archive for {}", target.triple)))?;
        let archive = native::runtime_archive(
            inputs.tools,
            &target.triple,
            stripped,
            request.profile == BuildProfile::Debug,
        )?;
        let foreign = native::selected_foreign_inputs(inputs.tools, target, inputs.links)?;
        let linker = FrozenLinker::snapshot(store.as_ref(), &policy.linker).map_err(failed)?;
        let kit =
            FrozenLinkKit::snapshot(store.as_ref(), &inputs.tools.kit, target, linker, limits)
                .map_err(failed)?;
        let captured = CapturedPlatform::capture(
            store.as_ref(),
            target,
            PlatformCaptureInputs {
                linker,
                kit,
                runtime_archive: archive,
                foreign: &foreign,
                runtime: RuntimeSelection {
                    loader: &loader,
                    auditor: &auditor,
                    library_path: libraries.as_os_str(),
                    emulator: emulator.as_deref(),
                    timeout: inputs.tools.launch_timeout,
                    events_bytes: runtime_limits.name_bytes as u64,
                    limits: runtime_limits,
                },
                limits,
            },
        )
        .map_err(failed)?;
        let platform = FrozenPlatform::new(
            store,
            LinkInputs {
                platform: captured,
                emulator,
                timeout: inputs.tools.launch_timeout,
                runtime_limits,
                limits,
            },
            inputs.entry.clone(),
            request.backend,
            target.clone(),
            request.run.clone(),
            allowance,
        )
        .map_err(failed)?;
        let profile_options = profile_options(request, inputs.purpose)?;
        let mut options = request.profile.options(target.clone());
        options.validation = request.validation;
        options.launch = request.launch.clone();
        Ok(Self {
            plan: NativePlan {
                options,
                policy: profile_options,
                verify_store: if request.validation == native::Validation::EveryStage {
                    hirsel_engine::store::VerifyStore::Recompute
                } else {
                    inputs.verify_store
                },
                opt: inputs.purpose.opt(request),
                debug: request.profile == BuildProfile::Debug,
                verify_backend: true,
                measurement: inputs.tools.runtime_build.measurement,
                fuel: request.fuel,
                requested_run: request.run.clone(),
                test_entries: request.test_entries.clone(),
                emit_only: matches!(request.output, super::NativeOutput::Objects { .. }),
                backends: vec![platform.backend(
                    native::backend_version(),
                    target.digest(),
                    profile_options,
                    ContentHash::of_bytes(&hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes()),
                )],
            },
            platform: Some(platform),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_backend_optimization_is_explicit_in_codegen_profile_identity() {
        for backend in [Backend::Cranelift, Backend::Llvm] {
            let mut request = NativeRequest {
                definition: None,
                test_entries: Vec::new(),
                supplemental_sources: Vec::new(),
                output: super::super::NativeOutput::Executable,
                optimization: None,
                backend,
                profile: BuildProfile::Default,
                target: native::TargetSpec::x86_64_unknown_linux_gnu(),
                validation: native::Validation::EveryStage,
                launch: Default::default(),
                run: None,
                fuel: false,
                evidence: None,
            };
            assert_eq!(
                NativePurpose::Build.opt(&request),
                native::OptLevel::Default
            );
            let run = profile_options(&request, NativePurpose::Run).unwrap();
            let build = profile_options(&request, NativePurpose::Build).unwrap();
            match backend {
                Backend::Cranelift => {
                    assert_eq!(NativePurpose::Run.opt(&request), native::OptLevel::None);
                    assert_ne!(
                        run, build,
                        "interactive None cannot reuse a Default Codegen action"
                    );
                }
                Backend::Llvm => {
                    assert_eq!(NativePurpose::Run.opt(&request), native::OptLevel::Default);
                    assert_eq!(
                        run, build,
                        "equal effective policy shares compilation products"
                    );
                }
            }
            request.profile = BuildProfile::Debug;
            assert_eq!(NativePurpose::Build.opt(&request), native::OptLevel::None);
            assert_ne!(
                run,
                profile_options(&request, NativePurpose::Build).unwrap()
            );
            let unfueled = profile_options(&request, NativePurpose::Run).unwrap();
            request.fuel = true;
            assert_ne!(
                unfueled,
                profile_options(&request, NativePurpose::Run).unwrap()
            );
        }
    }
}

/// Only chooses the ordinary backend partition. Compilation's emit-only mode
/// never creates a Link plan, so calling this method is a protocol error.
struct ObjectOutput(Backend);
impl native::queries::link_tasks::NativeLinkExecutor for ObjectOutput {
    fn codegen_partition(&self) -> native::queries::link_tasks::CodegenPartition {
        match self.0 {
            Backend::Llvm => native::queries::link_tasks::CodegenPartition::WholeModule,
            Backend::Cranelift => native::queries::link_tasks::CodegenPartition::PerInstance,
        }
    }
    fn link(
        &self,
        _: &mut native::queries::roots::RootProducts,
        _: native::queries::link_tasks::LinkExecution<'_>,
    ) -> Result<Vec<u8>, String> {
        Err("object-only compilation cannot execute Link".into())
    }
}
