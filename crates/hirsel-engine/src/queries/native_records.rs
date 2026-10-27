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
        for (anchor, _, _) in engine.sources.anchors() {
            if let Some(owner) = engine.record_anchor_owner(anchor) {
                if let Some(selected) = engine.record_anchor(&owner.0, owner.1) {
                    anchors.insert(owner.clone(), selected);
                }
                let declaration = engine
                    .sources
                    .anchor(anchor)
                    .and_then(|(file, _)| {
                        engine.generated.iter().find_map(|(path, generated)| {
                            (generated.file == file).then(|| DefId::intern(path))
                        })
                    })
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
}

#[cfg(test)]
mod tests;
