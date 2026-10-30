//! Explicit whole-module LLVM emission, using the same admitted Ready products
//! and fragment transport as per-instance Codegen. No whole-program controller.
use super::*;
use hirsel_engine::runtime::QueryId;

const SUMMARY: Domain = Domain {
    kind: 0x4f00_000a,
    schema: 1,
};

/// Complete canonical owner selection plus the normal backend/profile keys.
/// Body outputs stay in the action, so edits preserve the logical address.
#[derive(Clone, Debug)]
pub struct WholeCodegenKey {
    units: Arc<[CodegenKey]>,
}
impl WholeCodegenKey {
    pub fn new(mut units: Vec<CodegenKey>) -> Result<Self, String> {
        units.sort_by(|a, b| a.owner.canonical_cmp(&b.owner));
        let first = units.first().ok_or("whole-module Codegen has no owners")?;
        if !matches!(first.backend.as_str(), "llvm" | "cranelift")
            || units.windows(2).any(|pair| pair[0].owner == pair[1].owner)
            || units.iter().any(|unit| {
                unit.backend != first.backend
                    || unit.backend_version != first.backend_version
                    || unit.target_features != first.target_features
                    || unit.profile_options != first.profile_options
                    || unit.runtime_abi != first.runtime_abi
                    || unit.owner.target_abi != first.owner.target_abi
                    || unit.owner.policy != first.owner.policy
            })
        {
            return Err("grouped Codegen needs unique owners of one backend profile".into());
        }
        Ok(Self {
            units: units.into(),
        })
    }
    pub fn units(&self) -> &[CodegenKey] {
        &self.units
    }
    pub fn retained_bytes(&self) -> u64 {
        self.units
            .iter()
            .fold(std::mem::size_of::<Self>() as u64, |total, unit| {
                total
                    .saturating_add(std::mem::size_of::<CodegenKey>() as u64)
                    .saturating_add(unit.owner.instance.encoding().len() as u64)
                    .saturating_add(unit.backend.len() as u64)
            })
    }
    pub fn query_id(&self) -> QueryId {
        let addresses = self
            .units
            .iter()
            .map(|key| key.address().0)
            .collect::<Vec<_>>();
        QueryId {
            kind: super::tasks::CODEGEN_KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.codegen.whole.address.v1",
                addresses.iter().map(|address| address.0.as_slice()),
            )),
        }
    }
}

pub(super) fn codegen<N: NativeProducts, L>(
    queries: &mut ObjectQueries<N, L>,
    key: &WholeCodegenKey,
) -> Result<FragmentProduct, String> {
    let selected = key
        .units
        .iter()
        .map(|unit| queries.native.ready(&unit.owner))
        .collect::<Result<Vec<_>, _>>()?;
    if selected
        .iter()
        .any(|ready| !ready.optimizer_reads.is_empty())
    {
        return Err("whole-module Ready has unregistered optimizer reads".into());
    }
    let mut reads = Vec::new();
    for ready in &selected {
        reads.extend([
            ready
                .product
                .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                .map_err(|e| e.to_string())?
                .reference(),
            ready.ready,
            ready.layout_closure,
            ready.emission_context,
            ready.unit_summary,
        ]);
    }
    if let Some(product) = queries.native.execution_read() {
        reads.push(
            product
                .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                .map_err(|e| e.to_string())?
                .reference(),
        );
    }
    let summaries = selected
        .iter()
        .map(|ready| ready.unit_summary)
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&summaries).map_err(|e| e.to_string())?;
    let summary = queries
        .store
        .put_content(SUMMARY, &bytes)
        .ok_or("whole-module summary publication failed")?;
    if SUMMARY.reference(&bytes) != Some(summary) {
        return Err("whole-module summary identity differs".into());
    }
    let owner = key
        .units
        .iter()
        .map(|unit| instance_bytes(&unit.owner))
        .collect::<Vec<_>>();
    let owner = serde_json::to_vec(&owner).map_err(|e| e.to_string())?;
    fragment_checked(
        queries.store.as_ref(),
        FragmentIdentity {
            address: key.query_id().address,
            owner,
            summary,
        },
        &reads,
        queries.verify_store,
        || {
            queries.native.with_ready(&selected, |units, inputs| {
                if units.len() != key.units.len()
                    || !inputs.available_externally.is_empty()
                    || units.iter().zip(key.units()).any(|(unit, key)| {
                        unit.unit().header.instance != key.owner.instance.digest()
                            || unit.unit().header.definition != *key.owner.instance.definition()
                    })
                {
                    return Err(
                        "whole-module materialization differs from its complete owner selection"
                            .into(),
                    );
                }
                let debug = selected[0].debug;
                if selected.iter().any(|ready| ready.debug != debug) {
                    return Err("whole-module debug settings differ".into());
                }
                let profile = hirsel_lir::debug::Profile {
                    debug,
                    sources: None,
                };
                match key.units[0].backend.as_str() {
                    "llvm" => hirsel_llvm::emit_object_profile(units, inputs, profile)
                        .map_err(|e| e.to_string()),
                    "cranelift" => hirsel_cranelift::emit_object_profile(units, inputs, profile)
                        .map_err(|e| e.to_string()),
                    _ => Err("unknown grouped Codegen backend".into()),
                }
            })
        },
    )
}
