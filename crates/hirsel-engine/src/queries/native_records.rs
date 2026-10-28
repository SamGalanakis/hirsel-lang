//! Typed native descriptor transport through the ordinary product codec. The
//! engine owns anchor rebinding; native producers do not duplicate its catalog.
use crate::{QueryEngine, store::ResultStore};
use hirsel_product_codec::TypeRecords;
use hirsel_span::Anchor;
use hirsel_syntax::{DefId, DefPath};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, sync::Arc};

/// Current source identity data used by the ordinary type/anchor codec. This
/// grants neither source V nor native C authority.
pub trait NativeRecordSource {
    fn record_types(&self) -> &Arc<hirsel_types::TypeInterner>;
    fn record_owner(&self, anchor: Anchor) -> Option<(DefPath, u32)>;
    fn record_source_anchor(&self, owner: &DefPath, occurrence: u32) -> Option<Anchor>;
}
impl NativeRecordSource for QueryEngine {
    fn record_types(&self) -> &Arc<hirsel_types::TypeInterner> {
        self.types()
    }
    fn record_owner(&self, anchor: Anchor) -> Option<(DefPath, u32)> {
        self.record_anchor_owner(anchor)
    }
    fn record_source_anchor(&self, owner: &DefPath, occurrence: u32) -> Option<Anchor> {
        self.record_anchor(owner, occurrence)
    }
}

/// Immutable source bindings for native workers, captured through the engine's
/// sole existing owner/anchor selection. Contains no QueryEngine, Definition or
/// signature collection. Unknown source generations remain unrepresentable.
pub struct NativeRecordContext {
    types: Arc<hirsel_types::TypeInterner>,
    owners: BTreeMap<Anchor, (DefPath, u32)>,
    anchors: BTreeMap<(DefPath, u32), Anchor>,
    source_definitions: BTreeMap<Anchor, DefId>,
}
impl NativeRecordContext {
    pub(crate) fn input_bytes(&self) -> Option<u64> {
        let slot = (std::mem::size_of::<(Anchor, (DefPath, u32))>()
            + std::mem::size_of::<((DefPath, u32), Anchor)>()
            + std::mem::size_of::<(Anchor, DefId)>()
            + 18 * std::mem::size_of::<usize>()) as u64;
        (self.owners.len() as u64)
            .checked_mul(slot)?
            .checked_add(std::mem::size_of::<Self>() as u64)
    }

    /// The declaration owning this current source generation. A generated
    /// codec owner is content-bound and can differ from this declaration;
    /// selector ordinals are neither codec occurrences nor source bases.
    pub fn source_definition(&self, anchor: Anchor) -> Option<&DefPath> {
        self.source_definitions.get(&anchor).map(|id| id.path())
    }

    pub fn capture(engine: &QueryEngine) -> Self {
        let mut owners = BTreeMap::new();
        let mut anchors = BTreeMap::new();
        let mut source_definitions = BTreeMap::new();
        // Scalar owner selection takes the first item in DefPath/occurrence
        // order. Index that same selection once, instead of scanning every
        // declaration for every source anchor.
        for (path, items) in &engine.items {
            for item in items {
                if engine.sources.anchor(item.anchor).is_some() {
                    owners
                        .entry(item.anchor)
                        .or_insert_with(|| (path.clone(), item.occurrence));
                }
            }
        }
        let mut generated = BTreeMap::new();
        for (path, source) in &engine.generated {
            generated.entry(source.file).or_insert_with(|| {
                (
                    super::persistent::generated_codec_owner(path, &source.text),
                    DefId::intern(path),
                )
            });
        }
        // Registration can repeat identical text while inferred headers still
        // cite its earlier placed source. Only the same logical source and
        // exact bytes may share its content-bound codec owner.
        let mut generations = BTreeMap::new();
        for (file, binding) in &generated {
            if let Some(source) = engine.sources.file(*file) {
                generations
                    .entry((source.logical_path(), source.text()))
                    .or_insert_with(|| binding.clone());
            }
        }
        let mut current_anchors = BTreeMap::new();
        for (anchor, file, base) in engine.sources.anchors() {
            if let Some((owner, _)) = generated.get(&file) {
                current_anchors.insert((owner.clone(), base), anchor);
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for (_, file, _) in engine.sources.anchors() {
            if generated.contains_key(&file) || !seen.insert(file) {
                continue;
            }
            if let Some(source) = engine.sources.file(file)
                && let Some(binding) = generations.get(&(source.logical_path(), source.text()))
            {
                generated.insert(file, binding.clone());
            }
        }
        for (anchor, file, base) in engine.sources.anchors() {
            let owner = owners.get(&anchor).cloned().or_else(|| {
                generated
                    .get(&file)
                    .map(|(path, _)| (path.clone(), base))
                    .or_else(|| engine.catalog.owner(anchor))
            });
            if let Some(owner) = owner {
                anchors.entry(owner.clone()).or_insert_with(|| {
                    engine
                        .items
                        .get(&owner.0)
                        .and_then(|items| items.iter().find(|item| item.occurrence == owner.1))
                        .map(|item| item.anchor)
                        .or_else(|| engine.catalog.anchor(&owner.0, owner.1))
                        .or_else(|| current_anchors.get(&owner).copied())
                        .unwrap_or(anchor)
                });
                let declaration = generated
                    .get(&file)
                    .map(|(_, declaration)| *declaration)
                    .unwrap_or_else(|| DefId::intern(&owner.0));
                source_definitions.insert(anchor, declaration);
                owners.insert(anchor, owner);
            }
        }
        Self {
            types: engine.types().clone(),
            owners,
            anchors,
            source_definitions,
        }
    }
}
impl NativeRecordSource for NativeRecordContext {
    fn record_types(&self) -> &Arc<hirsel_types::TypeInterner> {
        &self.types
    }
    fn record_owner(&self, anchor: Anchor) -> Option<(DefPath, u32)> {
        self.owners.get(&anchor).cloned()
    }
    fn record_source_anchor(&self, owner: &DefPath, occurrence: u32) -> Option<Anchor> {
        self.anchors.get(&(owner.clone(), occurrence)).copied()
    }
}

/// One type/anchor codec over the selected provider and the owning interner.
/// Encoded recipes are ordinary data, never source V or native C authority.
#[derive(Clone)]
pub struct NativeRecords {
    records: Arc<TypeRecords>,
    types: Arc<hirsel_types::TypeInterner>,
}
impl NativeRecords {
    /// Initial inline allocations of a fresh record codec. Its type interner
    /// and store are shared. Valid only before any record encode or decode;
    /// consumers that populate its caches must admit that work separately.
    pub fn fresh_metadata_bytes() -> u64 {
        (size_of::<TypeRecords>()
            + size_of::<super::persistent::Contents>()
            + 4 * size_of::<usize>()) as u64
    }
    pub fn new(types: Arc<hirsel_types::TypeInterner>, store: Arc<dyn ResultStore>) -> Self {
        Self {
            records: TypeRecords::new(types.clone(), Arc::new(super::persistent::Contents(store))),
            types,
        }
    }
    pub fn encode<T: Serialize>(
        &self,
        source: &(impl NativeRecordSource + ?Sized),
        value: &T,
    ) -> Option<Vec<u8>> {
        if !Arc::ptr_eq(&self.types, source.record_types()) {
            return None;
        }
        self.records
            .encode_record(value, |anchor| source.record_owner(anchor))
    }
    /// Canonical ordinary bytes and pre-admission for outer record decoding.
    /// The caller must add its typed payload's physical layout bound.
    pub fn encode_with_decode_bytes<T: Serialize>(
        &self,
        source: &(impl NativeRecordSource + ?Sized),
        value: &T,
    ) -> Option<(Vec<u8>, u64)> {
        if !Arc::ptr_eq(&self.types, source.record_types()) {
            return None;
        }
        self.records
            .encode_record_with_decode_bytes(value, |anchor| source.record_owner(anchor))
    }
    pub fn decode<T: DeserializeOwned>(
        &self,
        source: &(impl NativeRecordSource + ?Sized),
        bytes: &[u8],
    ) -> Option<T> {
        if !Arc::ptr_eq(&self.types, source.record_types()) {
            return None;
        }
        self.records.decode_record(bytes, |owner, occurrence| {
            source.record_source_anchor(owner, occurrence)
        })
    }

    pub(crate) fn visit_record_types(
        &self,
        bytes: &[u8],
        visit: impl FnMut(hirsel_artifact_store::Reference) -> Option<bool>,
    ) -> Option<()> {
        self.records.visit_record_types(bytes, visit)
    }
}

#[cfg(test)]
mod tests;
