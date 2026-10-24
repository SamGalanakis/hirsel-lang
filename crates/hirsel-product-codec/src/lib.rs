//! Ordinary immutable products: canonical type components, owned source anchors
//! and bounded semantic decoding. Container integrity never supplies V authority.
#[cfg(test)]
hirsel_guard::install!();

mod base;
mod types;
pub use base::{BaseIndex, SharedTypes};
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
    #[serde(
        serialize_with = "serialize_payload",
        deserialize_with = "deserialize_payload"
    )]
    pub payload: Vec<u8>,
}
fn serialize_payload<S: serde::Serializer>(
    payload: &[u8],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(payload)
}
// serde-rs/bytes f685179, src/bytebuf.rs:202-259: request bulk bytes while
// preserving sequence-format support. Keep the public Vec and Serde's existing
// bounded Vec sequence fallback; strings remain invalid for this payload.
fn deserialize_payload<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    struct Payload;
    impl<'de> serde::de::Visitor<'de> for Payload {
        type Value = Vec<u8>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a byte array")
        }
        fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
            Ok(bytes.to_vec())
        }
        fn visit_byte_buf<E: serde::de::Error>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
            Ok(bytes)
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            sequence: A,
        ) -> Result<Self::Value, A::Error> {
            Vec::deserialize(serde::de::value::SeqAccessDeserializer::new(sequence))
        }
    }
    deserializer.deserialize_byte_buf(Payload)
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
struct DecodeOptions {
    limit: usize,
}
fn options(limit: usize) -> DecodeOptions {
    DecodeOptions { limit }
}
impl DecodeOptions {
    fn deserialize<'de, T: Deserialize<'de>>(&self, bytes: &'de [u8]) -> bincode::Result<T> {
        // bincode 1.3.3 internal.rs:114 substitutes Infinite for slice limits.
        // Enforce the caller's bound before decoding or copying a byte buffer;
        // SliceReader also checks each claimed length against remaining input.
        if bytes.len() > self.limit {
            return Err(bincode::ErrorKind::SizeLimit.into());
        }
        bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(self.limit as u64)
            .reject_trailing_bytes()
            .deserialize(bytes)
    }
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
    fn record_payload_accepts_bulk_bytes_and_preserves_wire_limits() {
        use serde::de::value::{
            BorrowedBytesDeserializer, BorrowedStrDeserializer, Error, SeqDeserializer,
        };
        // serde-rs/bytes tests/test_serde.rs:3-17's empty/ABC byte vectors,
        // plus non-ASCII bytes and the legacy sequence-format fallback.
        for bytes in [b"".as_slice(), b"ABC".as_slice(), &[0, 127, 128, 255]] {
            assert_eq!(
                deserialize_payload(BorrowedBytesDeserializer::<Error>::new(bytes)).unwrap(),
                bytes
            );
            assert_eq!(
                deserialize_payload(SeqDeserializer::<_, Error>::new(bytes.iter().copied()))
                    .unwrap(),
                bytes
            );
        }
        assert!(deserialize_payload(BorrowedStrDeserializer::<Error>::new("ABC")).is_err());
        #[derive(Serialize, Deserialize)]
        struct LegacyRecord {
            anchors: Vec<(DefPath, u32)>,
            types: Vec<TypeRef>,
            payload: Vec<u8>,
        }
        for len in [0, 1, 127, 8192] {
            let payload: Vec<_> = (0..len).map(|n| n as u8).collect();
            let legacy = LegacyRecord {
                anchors: vec![],
                types: vec![],
                payload: payload.clone(),
            };
            let encoded = bincode::serialize(&legacy).unwrap();
            let record: Record = options(encoded.len()).deserialize(&encoded).unwrap();
            assert_eq!(record.payload, payload);
            assert_eq!(bincode::serialize(&record).unwrap(), encoded);
            let old: LegacyRecord =
                bincode::deserialize(&bincode::serialize(&record).unwrap()).unwrap();
            assert_eq!(old.payload, payload);
            assert!(
                options(encoded.len() - 1)
                    .deserialize::<Record>(&encoded)
                    .is_err()
            );
            assert!(
                options(encoded.len())
                    .deserialize::<Record>(&encoded[..encoded.len() - 1])
                    .is_err()
            );
            let mut trailing = encoded.clone();
            trailing.push(0);
            assert!(
                options(trailing.len())
                    .deserialize::<Record>(&trailing)
                    .is_err()
            );
        }
        let mut forged = vec![0; 24];
        forged[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(
            options(forged.len())
                .deserialize::<Record>(&forged)
                .is_err()
        );
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
