//! Typed native descriptor transport through the ordinary product codec. The
//! engine owns anchor rebinding; native producers do not duplicate its catalog.
use crate::{QueryEngine, store::ResultStore};
use hirsel_product_codec::TypeRecords;
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;

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
    pub fn encode<T: Serialize>(&self, engine: &QueryEngine, value: &T) -> Option<Vec<u8>> {
        if !Arc::ptr_eq(&self.types, engine.types()) {
            return None;
        }
        self.records
            .encode_record(value, |anchor| engine.record_anchor_owner(anchor))
    }
    pub fn decode<T: DeserializeOwned>(&self, engine: &QueryEngine, bytes: &[u8]) -> Option<T> {
        if !Arc::ptr_eq(&self.types, engine.types()) {
            return None;
        }
        self.records.decode_record(bytes, |owner, occurrence| {
            engine.record_anchor(owner, occurrence)
        })
    }
}
