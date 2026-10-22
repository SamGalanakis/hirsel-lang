//! Ordinary immutable products: canonical type components, owned source anchors
//! and bounded semantic decoding. Container integrity never supplies V authority.
#[cfg(test)]
hirsel_guard::install!();

mod types;
use bincode::Options;
use hirsel_span::{Anchor, AnchorCodec};
use hirsel_syntax::DefPath;
use hirsel_types::{TableBuilder, TypeInterner};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
pub use types::{ProductTypes, TypeRecords, TypeRef};

#[derive(Serialize, Deserialize)]
pub struct Record {
    pub anchors: Vec<(DefPath, u32)>,
    pub types: Vec<TypeRef>,
    #[serde(serialize_with = "serialize_payload")]
    pub payload: Vec<u8>,
}
fn serialize_payload<S: serde::Serializer>(
    payload: &[u8],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(payload)
}
fn anchor_owners(
    codec: AnchorCodec,
    mut owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
) -> Option<Vec<(DefPath, u32)>> {
    let AnchorCodec::Collect(anchors) = codec else {
        unreachable!()
    };
    anchors.into_iter().map(&mut owner).collect()
}
/// Canonical semantic bytes for an output fingerprint. This has no decoder or
/// persisted-provider path; query records use independently referenced types.
pub fn canonical_bytes<T: Serialize>(
    types: Arc<TypeInterner>,
    value: &T,
    owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
) -> Option<Vec<u8>> {
    let mut anchors = AnchorCodec::Collect(vec![]);
    let mut table = TableBuilder::new(types);
    let payload = table
        .with_encoding(|| {
            hirsel_span::with_anchor_codec(&mut anchors, || bincode::serialize(value))
        })
        .ok()?;
    bincode::serialize(&(anchor_owners(anchors, owner)?, table.finish(), payload)).ok()
}
impl TypeRecords {
    pub fn encode_record<T: Serialize>(
        &self,
        value: &T,
        owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
    ) -> Option<Vec<u8>> {
        let mut anchors = AnchorCodec::Collect(vec![]);
        let mut table = TableBuilder::new(self.types.clone());
        let payload = table
            .with_encoding(|| {
                hirsel_span::with_anchor_codec(&mut anchors, || bincode::serialize(value))
            })
            .ok()?;
        let anchors = anchor_owners(anchors, owner)?;
        bincode::serialize(&Record {
            anchors,
            types: self.encode(table.roots())?,
            payload,
        })
        .ok()
    }
    pub fn decode_record<T: serde::de::DeserializeOwned>(
        self: &Arc<Self>,
        bytes: &[u8],
        mut anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
    ) -> Option<T> {
        let record: Record = options(bytes.len()).deserialize(bytes).ok()?;
        let anchors = record
            .anchors
            .iter()
            .map(|(owner, occurrence)| anchor(owner, *occurrence))
            .collect::<Option<_>>()?;
        hirsel_types::with_type_source(
            Arc::new(ProductTypes {
                records: self.clone(),
                references: record.types,
            }),
            || {
                hirsel_span::with_anchor_codec(&mut AnchorCodec::Decode(anchors), || {
                    options(record.payload.len()).deserialize(&record.payload)
                })
            },
        )
        .ok()
    }
}
fn options(len: usize) -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(len as u64)
        .reject_trailing_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_artifact_store::Memory;
    use hirsel_syntax::{ItemPath, PackageId};
    use hirsel_types::{Type, TypeId};
    fn path() -> DefPath {
        DefPath(ItemPath {
            package: PackageId {
                name: "codec".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: "owner".into(),
        })
    }
    #[test]
    fn ordinary_records_relocate_owned_anchors_and_refuse_foreign_or_missing_sites() {
        let store = Arc::new(Memory::default());
        let encoder = TypeRecords::new(TypeInterner::new(None), store.clone());
        let unit = encoder.types.intern(Type::unit());
        let original = Anchor::fresh();
        let span = hirsel_span::Span::new(original, 3, 7).unwrap();
        let record = encoder
            .encode_record(&(unit, span), |a| (a == original).then(|| (path(), 2)))
            .unwrap();
        assert!(encoder.encode_record(&(unit, span), |_| None).is_none());
        let decoder = TypeRecords::new(TypeInterner::new(None), store);
        assert!(
            decoder
                .decode_record::<(TypeId, hirsel_span::Span)>(&record, |_, _| None)
                .is_none()
        );
        let relocated = Anchor::fresh();
        let (decoded, site): (TypeId, hirsel_span::Span) = decoder
            .decode_record(&record, |owner, occurrence| {
                (owner == &path() && occurrence == 2).then_some(relocated)
            })
            .unwrap();
        assert_eq!(decoder.types.row(decoded), &Type::unit());
        assert_eq!(site, hirsel_span::Span::new(relocated, 3, 7).unwrap());
        let mut trailing = record.clone();
        trailing.push(0);
        assert!(
            decoder
                .decode_record::<(TypeId, hirsel_span::Span)>(&trailing, |_, _| Some(relocated))
                .is_none()
        );
    }
}
