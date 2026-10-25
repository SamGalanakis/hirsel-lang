//! Native graph policy and ownership over the selected ordinary source graph.
//! Reuse recaptures linker/kit/runtime resolution before choosing the old key.
use super::{
    NativeRequest,
    native_assembly::{NativeAssembly, NativeAssemblyPlan, NativeProofPlan, NativeSourceInputs},
    native_bootstrap::NativeAdmission,
    native_platform::{
        NativePlatformInputs, NativePlatformPolicy, NativePlatformSelection, NativePurpose,
    },
    native_products::NativeBuildReadiness,
};
use crate::native::{self, NativeError, object_queries};
use hirsel_engine::{
    FrontNativeDefinitionQueries, FrontNativeQueries, LinkInput, LoadedProgram, ToolError,
    runtime::{MemoryReservation, MemoryWeight, QueryId, RuntimeErrors},
    store::ResultStore,
};
use hirsel_syntax::{ContentHash, DefPath};
use std::sync::Arc;

/// These bytes are split out of the existing snapshot partition. Constructors
/// still bound actual retained metadata and outputs against each partition.
pub(super) struct Footprint {
    catalogue: u64,
    source: u64,
    bootstrap: u64,
    tasks: u64,
    /// Checked headers and the PS9 export; zero unless evidence is selected.
    proofs: u64,
}
impl Footprint {
    const ROUTING: u64 = 16 << 10;
    const PLATFORM: u64 = 64 << 10;
    const POLICY: u64 = 64 << 10;
    pub fn minimum(definitions: &[DefPath]) -> Option<u64> {
        Self::new(definitions, 0, false)?.bytes()
    }
    pub fn new(definitions: &[DefPath], slack: u64, proofs: bool) -> Option<Self> {
        let bootstrap = definitions.iter().try_fold(32 << 10, |bytes: u64, path| {
            bytes.checked_add(serde_json::to_vec(path).ok()?.len() as u64 + 128)
        })?;
        // Proof capture shares the native slack; its constructors refuse
        // metadata beyond this partition instead of growing any cap.
        let share = slack / if proofs { 6 } else { 5 };
        Some(Self {
            catalogue: FrontNativeDefinitionQueries::input_bytes(definitions)?
                .checked_add(share)?,
            source: FrontNativeQueries::input_bytes(definitions)?.checked_add(share)?,
            bootstrap,
            tasks: share,
            proofs: if proofs { share } else { 0 },
        })
    }
    pub fn bytes(&self) -> Option<u64> {
        [
            self.catalogue,
            self.source,
            self.bootstrap,
            self.tasks,
            self.tasks,
            self.tasks,
            self.proofs,
            Self::ROUTING,
            Self::POLICY,
            Self::PLATFORM,
            Self::PLATFORM,
        ]
        .into_iter()
        .try_fold(0u64, u64::checked_add)
    }
}

pub(super) struct NativeSelection {
    pub assembly: Arc<NativeAssembly>,
    pub readiness: Arc<NativeBuildReadiness>,
    _allowance: MemoryReservation,
    key: ContentHash,
    request: NativeRequest,
    purpose: NativePurpose,
    verify_store: hirsel_engine::store::VerifyStore,
    entry: DefPath,
    tools: Arc<native::Toolchain>,
    links: Vec<LinkInput>,
    policy: NativePlatformPolicy,
    platform: Arc<super::native_link::FrozenPlatform>,
    validation: Arc<MemoryReservation>,
    store: Arc<dyn ResultStore>,
    limits: object_queries::capture::CaptureLimits,
    runtime_limits: object_queries::runtime::RuntimeLimits,
}
impl NativeSelection {
    pub fn new(
        program: &mut LoadedProgram,
        source: NativeSourceInputs,
        request: NativeRequest,
        purpose: NativePurpose,
        verify_store: hirsel_engine::store::VerifyStore,
        tools: Arc<native::Toolchain>,
        footprint: &Footprint,
        allowance: &mut MemoryReservation,
        weight: MemoryWeight,
        errors: RuntimeErrors,
        transport_bytes: u64,
    ) -> Result<Arc<Self>, ToolError> {
        let failed = |message| ToolError::Runtime(message);
        let mut split = |bytes| {
            allowance.split(bytes).ok_or_else(|| {
                failed("native snapshot partition exceeds its original admission".into())
            })
        };
        let policy = NativePlatformPolicy::capture().map_err(native_error)?;
        let entry = source.entry.clone();
        let links = program.engine.foreign_links.clone();
        let limits = object_queries::capture::CaptureLimits {
            archive_bytes: transport_bytes,
            files: 4096,
        };
        let runtime_limits = object_queries::runtime::RuntimeLimits {
            aliases: 4096,
            name_bytes: 64 << 10,
        };
        let validation = Arc::new(split(Footprint::PLATFORM)?);
        let policy_allowance = split(Footprint::POLICY)?;
        let mut copied = hirsel_engine::NativeMetadataWeight::default();
        copied.add(std::mem::size_of::<Self>() + 3 * std::mem::size_of::<usize>());
        super::native_products::request_metadata(&mut copied, &request);
        copied.path(&entry);
        copied.add(policy.linker.capacity());
        copied.add(policy.auditors.capacity());
        copied.add(policy.floor.capacity());
        copied.vec(&links, |w, input| match input {
            LinkInput::Library(name) => w.add(name.capacity()),
            LinkInput::File(path) => w.add(path.capacity()),
        });
        if copied.bytes() > policy_allowance.bytes() {
            return Err(failed(
                "native selected policy metadata exceeds its original admission".into(),
            ));
        }
        let store = source.store.clone();
        let NativePlatformSelection { plan, platform } = NativePlatformSelection::capture(
            store.clone(),
            NativePlatformInputs {
                purpose,
                verify_store,
                request: &request,
                entry: &entry,
                tools: &tools,
                links: &links,
                policy: &policy,
            },
            limits,
            runtime_limits,
            split(Footprint::PLATFORM)?,
        )
        .map_err(native_error)?;
        let proofs = match &request.evidence {
            None => None,
            Some(evidence) => Some(NativeProofPlan {
                evidence: evidence.clone(),
                headers: split(footprint.proofs / 2)?,
                export: split(footprint.proofs - footprint.proofs / 2)?,
            }),
        };
        let assembly = NativeAssembly::new(
            program,
            source,
            NativeAssemblyPlan {
                proofs,
                native: plan,
                tasks: NativeAdmission {
                    instances: split(footprint.tasks)?,
                    arena: split(footprint.tasks)?,
                    compilation: split(footprint.tasks)?,
                },
                catalogue: split(footprint.catalogue)?,
                source: split(footprint.source)?,
                routing: split(Footprint::ROUTING)?,
                bootstrap: split(footprint.bootstrap)?,
                // Native continuations use the same bounded frames already
                // qualified by the ordinary bootstrap controls. Source frames
                // are independently planned for their captured inventory.
                weight: MemoryWeight {
                    retained: weight.retained.min(1 << 20),
                    scratch: weight.scratch.min(16 << 20),
                    stack: 0,
                },
                source_weight: weight,
                errors,
            },
        )?;
        let readiness = Arc::new(
            assembly
                .readiness(request.clone(), tools.clone(), platform.clone())
                .map_err(native_error)?,
        );
        Ok(Arc::new(Self {
            _allowance: policy_allowance,
            key: request_key(&request, purpose)?,
            request,
            purpose,
            verify_store,
            entry,
            tools,
            links,
            policy,
            platform,
            validation,
            store,
            limits,
            runtime_limits,
            assembly,
            readiness,
        }))
    }
    pub fn accepts(&self, request: &NativeRequest, purpose: NativePurpose) -> bool {
        if request_key(request, purpose).ok() != Some(self.key) {
            return false;
        }
        // This must snapshot the current linker and kit as well as target
        // inputs. Hashing old reads misses search-path additions and aliases.
        NativePlatformSelection::capture(
            self.store.clone(),
            NativePlatformInputs {
                purpose: self.purpose,
                verify_store: self.verify_store,
                request: &self.request,
                entry: &self.entry,
                tools: &self.tools,
                links: &self.links,
                policy: &self.policy,
            },
            self.limits,
            self.runtime_limits,
            self.validation.clone(),
        )
        .is_ok_and(|selected| selected.platform.identity() == self.platform.identity())
    }
    pub fn query(&self) -> QueryId {
        self.assembly.query()
    }
}
fn native_error(error: NativeError) -> ToolError {
    ToolError::Runtime(error.to_string())
}
fn request_key(request: &NativeRequest, purpose: NativePurpose) -> Result<ContentHash, ToolError> {
    let bytes = serde_json::to_vec(&(
        &request.definition,
        &request.test_entries,
        &request.supplemental_sources,
        request.backend.name(),
        super::native_platform::profile_options(request, purpose).map_err(native_error)?,
        request.target.digest(),
        match request.validation {
            native::Validation::Ready => 0u8,
            native::Validation::EveryStage => 1,
        },
        &request.launch,
        request.run.as_ref().map(|run| run.identity()),
        request.fuel,
        request
            .evidence
            .as_deref()
            .map(super::native_products::evidence_identity),
    ))
    .map_err(|error| ToolError::Runtime(error.to_string()))?;
    Ok(ContentHash::of_parts(
        "hirsel.host.native-request.v1",
        [bytes.as_slice()],
    ))
}

#[cfg(test)]
mod tests;
