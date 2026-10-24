//! Typed native descriptor transport through the ordinary product codec. The
//! engine owns anchor rebinding; native producers do not duplicate its catalog.
use crate::{QueryEngine, store::ResultStore};
use hirsel_product_codec::TypeRecords;
use hirsel_span::Anchor;
use hirsel_syntax::DefPath;
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
}
impl NativeRecordContext {
    pub fn capture(engine: &QueryEngine) -> Self {
        let mut owners = BTreeMap::new();
        let mut anchors = BTreeMap::new();
        for (anchor, _, _) in engine.sources.anchors() {
            if let Some(owner) = engine.record_anchor_owner(anchor) {
                if let Some(selected) = engine.record_anchor(&owner.0, owner.1) {
                    anchors.insert(owner.clone(), selected);
                }
                owners.insert(anchor, owner);
            }
        }
        Self {
            types: engine.types().clone(),
            owners,
            anchors,
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
pub struct NativeRecords {
    records: Arc<TypeRecords>,
    types: Arc<hirsel_types::TypeInterner>,
}
impl NativeRecords {
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
