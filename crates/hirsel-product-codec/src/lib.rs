//! Ordinary immutable products: canonical type components, owned source anchors
//! and bounded semantic decoding. Container integrity never supplies V authority.
#[cfg(test)]
hirsel_guard::install!();

mod base;
mod indexed_types;
mod types;
pub use base::{BaseIndex, SharedTypes};
use bincode::Options;
use hirsel_span::{Anchor, AnchorCodec};
use hirsel_syntax::DefPath;
use hirsel_types::{TableBuilder, TypeInterner};
pub use indexed_types::{CheckedTypeComponent, TypeEdge, TypeRow, TypeTag};
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

// bincode 1.3.3 de/read.rs:131-135 forwards a checked slice to Serde's
// borrowed-byte visitor. This reads the existing Vec wire format verbatim.
#[derive(Deserialize)]
struct RecordView<'a> {
    anchors: Vec<(DefPath, u32)>,
    types: Vec<TypeRef>,
    #[serde(borrow)]
    payload: &'a [u8],
}

/// One portable record's immutable payload, type positions and source owners.
/// The byte range retains its full backing and admission through every clone.
/// Opening checks the envelope; each typed read resolves current source anchors
/// and checked type components independently, without transferring V authority.
#[derive(Clone)]
pub struct StoredRecord(Arc<StoredRecordOwner>);
struct StoredRecordOwner {
    anchors: Vec<(DefPath, u32)>,
    types: Arc<ProductTypes>,
    payload: hirsel_artifact_store::Bytes,
}
impl StoredRecord {
    pub fn payload(&self) -> &hirsel_artifact_store::Bytes {
        &self.0.payload
    }
    pub fn decode<T: serde::de::DeserializeOwned>(
        &self,
        mut anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
    ) -> Option<T> {
        let anchors = self
            .0
            .anchors
            .iter()
            .map(|(owner, occurrence)| anchor(owner, *occurrence))
            .collect::<Option<_>>()?;
        decode_payload(&self.0.payload, self.0.types.clone(), anchors)
    }
}

fn decode_payload<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    types: Arc<ProductTypes>,
    anchors: Vec<Anchor>,
) -> Option<T> {
    hirsel_types::with_type_source(types, || {
        hirsel_span::with_anchor_codec(&mut AnchorCodec::Decode(anchors), || {
            options(bytes.len()).deserialize(bytes)
        })
    })
    .ok()
}
/// Serialize an opaque byte payload using the format's bulk byte operation.
pub fn serialize_payload<S: serde::Serializer>(
    payload: &[u8],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(payload)
}
// serde-rs/bytes f685179, src/bytebuf.rs:202-259: request bulk bytes while
// preserving sequence-format support. Keep the public Vec and Serde's existing
// bounded Vec sequence fallback; strings remain invalid for this payload.
/// Decode bytes or the equivalent sequence without accepting string payloads.
pub fn deserialize_payload<'de, D: serde::Deserializer<'de>>(
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
/// Peak outer-record decode storage, excluding the caller's typed payload.
/// Source interner rows retain their existing owners; this covers the copied
/// input/payload, anchor paths, anchor handles and type-reference array.
fn record_decode_bytes(record: &Record, encoded: usize) -> Option<u64> {
    let mut bytes = encoded
        .checked_add(record.payload.len())?
        .checked_add(std::mem::size_of::<Record>())?;
    let product_types = std::alloc::Layout::new::<[usize; 2]>()
        .extend(std::alloc::Layout::new::<ProductTypes>())
        .ok()?
        .0
        .pad_to_align()
        .size();
    bytes = bytes.checked_add(product_types)?;
    // Serde's cautious vectors can grow geometrically; collecting decoded
    // anchor Options also uses Vec's minimum non-zero allocation of four rows.
    let anchor_slots = record.anchors.len().checked_mul(2)?.max(4);
    let type_slots = record.types.len().checked_mul(2)?.max(4);
    bytes =
        bytes
            .checked_add(anchor_slots.checked_mul(
                std::mem::size_of::<(DefPath, u32)>() + std::mem::size_of::<Anchor>(),
            )?)?
            .checked_add(type_slots.checked_mul(std::mem::size_of::<TypeRef>())?)?;
    for (path, _) in &record.anchors {
        bytes = bytes
            .checked_add(path.0.package.name.len())?
            .checked_add(path.0.package.version.len())?
            .checked_add(path.0.name.len())?
            .checked_add(
                path.0
                    .modules
                    .len()
                    .checked_mul(2)?
                    .max(4)
                    .checked_mul(std::mem::size_of::<String>())?,
            )?;
        for module in &path.0.modules {
            bytes = bytes.checked_add(module.len())?;
        }
    }
    bytes.try_into().ok()
}
impl TypeRecords {
    /// Visit the complete canonical type closure of an ordinary record, without
    /// interning its payload. The visitor admits and copies a component before
    /// traversal. Return false only for a component whose closure was already
    /// copied, or None to abandon the entire copy.
    pub fn visit_record_types(
        &self,
        bytes: &[u8],
        mut visit: impl FnMut(hirsel_artifact_store::Reference) -> Option<bool>,
    ) -> Option<()> {
        let record: RecordView<'_> = options(bytes.len()).deserialize(bytes).ok()?;
        let mut pending = record.types;
        while let Some(reference) = pending.pop() {
            if visit(reference.reference())? {
                let component = types::component(self.store.as_ref(), reference)?;
                pending.extend(
                    (0..component.dependency_count())
                        .map(|n| component.dependency(n))
                        .collect::<Option<Vec<_>>>()?,
                );
            }
        }
        Some(())
    }

    /// The ordinary record plus metadata to pre-admit its outer decode buffers.
    /// Typed payload layout must be supplied by its producing phase separately.
    pub fn encode_record_with_decode_bytes<T: Serialize>(
        &self,
        value: &T,
        owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
    ) -> Option<(Vec<u8>, u64)> {
        let mut anchors = AnchorCodec::Collect(vec![]);
        let mut table = TableBuilder::new(self.types.clone());
        let payload = table
            .with_encoding(|| {
                hirsel_span::with_anchor_codec(&mut anchors, || bincode::serialize(value))
            })
            .ok()?;
        let record = Record {
            anchors: anchor_owners(anchors, owner)?,
            types: self.encode(table.roots())?,
            payload,
        };
        let bytes = bincode::serialize(&record).ok()?;
        let working = record_decode_bytes(&record, bytes.len())?;
        Some((bytes, working))
    }
    pub fn encode_record<T: Serialize>(
        &self,
        value: &T,
        owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
    ) -> Option<Vec<u8>> {
        self.encode_record_with_decode_bytes(value, owner)
            .map(|(bytes, _)| bytes)
    }

    pub fn decode_record<T: serde::de::DeserializeOwned>(
        self: &Arc<Self>,
        bytes: &[u8],
        mut anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
    ) -> Option<T> {
        let record: RecordView<'_> = options(bytes.len()).deserialize(bytes).ok()?;
        let anchors = record
            .anchors
            .iter()
            .map(|(owner, occurrence)| anchor(owner, *occurrence))
            .collect::<Option<_>>()?;
        decode_payload(
            record.payload,
            Arc::new(ProductTypes {
                records: self.clone(),
                references: record.types,
            }),
            anchors,
        )
    }

    /// Open once when retaining an ordinary stored record. All typed consumers
    /// of its clones share the same portable owner and exact payload range.
    pub fn open_record(
        self: &Arc<Self>,
        bytes: hirsel_artifact_store::Bytes,
    ) -> Option<StoredRecord> {
        let record: RecordView<'_> = options(bytes.len()).deserialize(&bytes).ok()?;
        let offset = (record.payload.as_ptr() as usize).checked_sub(bytes.as_ptr() as usize)?;
        let payload = bytes.range(offset as u64, record.payload.len() as u64, 1)?;
        Some(StoredRecord(Arc::new(StoredRecordOwner {
            anchors: record.anchors,
            types: Arc::new(ProductTypes {
                records: self.clone(),
                references: record.types,
            }),
            payload,
        })))
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
    use hirsel_syntax::{ContentHash, ItemPath, PackageId};
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
    fn stored_record_views_share_backing_and_recheck_current_source_bindings() {
        let store = Arc::new(Memory::default());
        let records = TypeRecords::new(TypeInterner::new(None), store);
        let unit = records.types.intern(Type::unit());
        let original = Anchor::fresh();
        let span = hirsel_span::Span::new(original, 3, 7).unwrap();
        let encoded = records
            .encode_record(&(unit, span), |a| (a == original).then(|| (path(), 2)))
            .unwrap();
        let owner = Arc::new(());
        let bytes = hirsel_artifact_store::Bytes::from(encoded).retain_owner(owner.clone());
        let view: RecordView<'_> = options(bytes.len()).deserialize(&bytes).unwrap();
        let pointer = view.payload.as_ptr();
        let record = records.open_record(bytes.clone()).unwrap();
        assert_eq!(record.payload().as_ptr(), pointer);
        let second = record.clone();
        assert!(Arc::ptr_eq(&record.0, &second.0));
        assert!(Arc::ptr_eq(&record.0.types, &second.0.types));
        drop(bytes);
        for current in [Anchor::fresh(), Anchor::fresh()] {
            let (decoded, site): (TypeId, hirsel_span::Span) = second
                .decode(|owner, occurrence| {
                    (owner == &path() && occurrence == 2).then_some(current)
                })
                .unwrap();
            assert_eq!(decoded, unit);
            assert_eq!(site, hirsel_span::Span::new(current, 3, 7).unwrap());
        }
        assert!(
            second
                .decode::<(TypeId, hirsel_span::Span)>(|_, _| None)
                .is_none()
        );
        let held = second.payload().range(0, 1, 1).unwrap();
        drop((record, second, records));
        assert_eq!(Arc::strong_count(&owner), 2);
        drop(held);
        assert_eq!(Arc::strong_count(&owner), 1);
    }

    #[test]
    fn borrowed_record_payload_keeps_existing_bounds_and_exact_consumption() {
        let record = Record {
            anchors: vec![],
            types: vec![],
            payload: vec![7; 8192],
        };
        let bytes = bincode::serialize(&record).unwrap();
        let view: RecordView<'_> = options(bytes.len()).deserialize(&bytes).unwrap();
        assert_eq!(view.payload, record.payload);
        assert_eq!(view.payload.as_ptr(), bytes[24..].as_ptr());
        for len in [0, 1, 23, bytes.len() - 1] {
            assert!(
                options(len)
                    .deserialize::<RecordView<'_>>(&bytes[..len])
                    .is_err()
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(
            options(trailing.len())
                .deserialize::<RecordView<'_>>(&trailing)
                .is_err()
        );
        let mut forged = bytes;
        forged[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(
            options(forged.len())
                .deserialize::<RecordView<'_>>(&forged)
                .is_err()
        );
    }
    #[test]
    fn captured_record_decode_layout_covers_small_and_growing_transport_vectors() {
        for rows in [0, 1, 2, 17, 129] {
            let record = Record {
                anchors: (0..rows).map(|n| (path(), n)).collect(),
                types: (0..rows)
                    .map(|n| TypeRef {
                        output: ContentHash::default(),
                        len: 0,
                        member: n,
                    })
                    .collect(),
                payload: vec![7; rows as usize],
            };
            let bytes = bincode::serialize(&record).unwrap();
            let decoded: Record = options(bytes.len()).deserialize(&bytes).unwrap();
            let handles = decoded
                .anchors
                .iter()
                .map(|_| Some(Anchor(0)))
                .collect::<Option<Vec<_>>>()
                .unwrap();
            let mut actual = bytes.len()
                + decoded.payload.capacity()
                + std::mem::size_of::<Record>()
                + decoded.anchors.capacity() * std::mem::size_of::<(DefPath, u32)>()
                + decoded.types.capacity() * std::mem::size_of::<TypeRef>()
                + handles.capacity() * std::mem::size_of::<Anchor>()
                + std::mem::size_of::<ProductTypes>()
                + 2 * std::mem::size_of::<usize>();
            for (path, _) in &decoded.anchors {
                actual += path.0.package.name.capacity()
                    + path.0.package.version.capacity()
                    + path.0.name.capacity()
                    + path.0.modules.capacity() * std::mem::size_of::<String>()
                    + path.0.modules.iter().map(String::capacity).sum::<usize>();
            }
            assert!(
                record_decode_bytes(&record, bytes.len()).unwrap() >= actual as u64,
                "captured layout must cover decoded capacities for {rows} rows"
            );
        }
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
    fn copying_record_types_preserves_transitive_and_cyclic_components() {
        use hirsel_artifact_store::Contents;
        use hirsel_types::TypeKind;
        let source = Arc::new(Memory::default());
        let encoder = TypeRecords::new(TypeInterner::new(None), source.clone());
        let unit = encoder.types.intern(Type::unit());
        let boxed = encoder.types.intern(Type::with_kind(TypeKind::Box(unit)));
        let cycle = encoder.types.intern_graph(2, |ids| {
            vec![
                Type::with_kind(TypeKind::Tuple(vec![boxed, ids[1]].into())),
                Type::with_kind(TypeKind::Box(ids[0])),
            ]
        });
        let record = encoder.encode_record(&cycle[0], |_| None).unwrap();
        let copied = Arc::new(Memory::default());
        let mut seen = std::collections::HashSet::new();
        encoder
            .visit_record_types(&record, |reference| {
                if !seen.insert(reference.output.0) {
                    return Some(false);
                }
                let bytes = source.get_content(reference)?;
                (copied.put_content(reference.domain, &bytes)? == reference).then_some(true)
            })
            .unwrap();
        assert_eq!(
            seen.len(),
            3,
            "the cycle shares one component above two dependencies"
        );
        assert!(encoder.visit_record_types(&record, |_| None).is_none());
        drop(encoder);
        drop(source);
        let decoder = TypeRecords::new(TypeInterner::new(None), copied);
        let root: TypeId = decoder.decode_record(&record, |_, _| None).unwrap();
        let TypeKind::Tuple(fields) = &decoder.types.row(root).kind else {
            panic!()
        };
        let TypeKind::Box(back) = decoder.types.row(fields[1]).kind else {
            panic!()
        };
        assert_eq!(back, root);
        let TypeKind::Box(leaf) = decoder.types.row(fields[0]).kind else {
            panic!()
        };
        assert_eq!(decoder.types.row(leaf), &Type::unit());
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
