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
pub(super) struct NativePlatformInputs<'a> {
    pub request: &'a NativeRequest,
    pub entry: &'a DefPath,
    pub tools: &'a Toolchain,
    pub links: &'a [LinkInput],
    pub policy: &'a NativePlatformPolicy,
}
pub(super) struct NativePlatformSelection {
    pub plan: NativePlan,
    pub platform: Arc<FrozenPlatform>,
}
impl NativePlatformSelection {
    pub fn capture(
        store: Arc<dyn ResultStore>,
        inputs: NativePlatformInputs<'_>,
        limits: CaptureLimits,
        runtime_limits: RuntimeLimits,
        allowance: MemoryReservation,
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
        if request.evidence.is_some() {
            return Err(NativeError::new(
                StageName::Manifest,
                "native proof assessment root is not installed",
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
            .unwrap_or(&inputs.policy.floor)
            .join(&target.triple);
        let (loader, libraries) = inputs
            .tools
            .loader(target)?
            .unwrap_or_else(|| (floor.join(loader_name), floor));
        let auditor = inputs.policy.auditors.join(format!("{}.so", target.triple));
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
        let linker =
            FrozenLinker::snapshot(store.as_ref(), &inputs.policy.linker).map_err(failed)?;
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
        let profile = match request.profile {
            BuildProfile::Default => 0u8,
            BuildProfile::Debug => 1,
            BuildProfile::Release => 2,
            BuildProfile::ReleaseLto => 3,
        };
        let profile_bytes = serde_json::to_vec(&(profile, request.fuel))
            .map_err(|error| failed(error.to_string()))?;
        let profile_options =
            ContentHash::of_parts("hirsel.host.native-profile.v1", [profile_bytes.as_slice()]);
        let mut options = request.profile.options(target.clone());
        options.validation = request.validation;
        options.launch = request.launch.clone();
        Ok(Self {
            plan: NativePlan {
                options,
                policy: profile_options,
                opt: request.profile.opt(),
                debug: request.profile == BuildProfile::Debug,
                verify_backend: true,
                measurement: inputs.tools.runtime_build.measurement,
                fuel: request.fuel,
                requested_run: request.run.clone(),
                backends: vec![platform.backend(
                    ContentHash::of_bytes(&crate::COMPILER_IDENTITY),
                    target.digest(),
                    profile_options,
                    ContentHash::of_bytes(&hirsel_lir::RUNTIME_ABI_VERSION.to_le_bytes()),
                )],
            },
            platform,
        })
    }
}
