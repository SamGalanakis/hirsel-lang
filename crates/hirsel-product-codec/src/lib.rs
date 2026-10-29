//! Ordinary immutable products: canonical type components, owned source anchors
//! and bounded semantic decoding. Container integrity never supplies V authority.
#[cfg(test)]
hirsel_guard::install!();

mod base;
mod envelope;
mod indexed_types;
pub use envelope::{AnchorRow, BoundIndexedEnvelope, IndexedEnvelope};
mod tir;
mod tir_serialize;
mod tir_view;
pub use tir_view::{TirBlockView, TirBodyView, TirMatchView, TirView};
mod types;
pub use base::{BaseIndex, SharedTypes};
use bincode::Options;
use hirsel_span::{Anchor, AnchorCodec};
use hirsel_syntax::DefPath;
use hirsel_types::{TableBuilder, TypeInterner};
pub use indexed_types::{CheckedTypeComponent, TypeEdge, TypeRow, TypeTag};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
pub use tir::{CheckedTir, TirBlockRows, TirBodyHeader, TirBodyRows, TirHeader, TirMatchRows};
pub use types::{ProductTypes, TypeRecords, TypeRef};

use envelope::EnvelopeLayout;
use hirsel_intern::persist::Source;
use hirsel_types::TypeId;

/// A transient checked view. The payload remains in the caller's immutable
/// buffer; only a transformation's typed decoder allocates semantic values.
#[derive(Clone, Copy)]
struct RecordView<'a> {
    bytes: &'a [u8],
    layout: EnvelopeLayout,
}
impl<'a> RecordView<'a> {
    fn open(bytes: &'a [u8]) -> Option<Self> {
        Some(Self {
            bytes,
            layout: EnvelopeLayout::open(bytes)?,
        })
    }
    fn payload(self) -> &'a [u8] {
        &self.bytes[self.layout.payload_start..]
    }
    fn type_references(self) -> Option<Vec<TypeRef>> {
        (0..self.layout.types)
            .map(|n| self.layout.type_reference(self.bytes, n))
            .collect()
    }
    fn anchors(
        self,
        mut anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
    ) -> Option<Vec<Anchor>> {
        (0..self.layout.anchors)
            .map(|n| {
                let (owner, occurrence) = self.layout.anchor(self.bytes, n)?.to_owned();
                anchor(&owner, occurrence)
            })
            .collect()
    }
}

/// One portable record retains its complete immutable backing and admission.
/// Every clone reads source/type positions directly from checked directories.
#[derive(Clone)]
pub struct StoredRecord(Arc<StoredRecordOwner>);
struct StoredRecordOwner {
    backing: hirsel_artifact_store::Bytes,
    payload: hirsel_artifact_store::Bytes,
    layout: EnvelopeLayout,
    records: Arc<TypeRecords>,
}
impl Source<TypeId> for StoredRecordOwner {
    fn id(&self, number: u32) -> Option<TypeId> {
        self.records
            .get(self.layout.type_reference(&self.backing, number)?)
    }
}
impl StoredRecord {
    pub fn payload(&self) -> &hirsel_artifact_store::Bytes {
        &self.0.payload
    }
    pub fn decode<T: serde::de::DeserializeOwned>(
        &self,
        anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
    ) -> Option<T> {
        let view = RecordView {
            bytes: &self.0.backing,
            layout: self.0.layout,
        };
        decode_payload(&self.0.payload, self.0.clone(), view.anchors(anchor)?)
    }
}
fn decode_payload<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    types: Arc<dyn Source<TypeId>>,
    anchors: Vec<Anchor>,
) -> Option<T> {
    hirsel_types::with_type_source(types, || {
        hirsel_span::with_anchor_codec(&mut AnchorCodec::Decode(anchors), || {
            indexed_types::inline_names(|| options(bytes.len()).deserialize(bytes))
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
/// Source interner rows retain their existing owners. This conservative bound
/// covers input, transient scalar materialization, path binding, anchor handles
/// and raw byte callers' reference array. Stored owners borrow all payload bytes;
/// complete backing retention remains with the existing shared-charge policy.
fn record_decode_bytes(
    anchors: &[(DefPath, u32)],
    type_count: usize,
    payload: usize,
    encoded: usize,
) -> Option<u64> {
    let mut bytes = encoded
        .checked_add(payload)?
        .checked_add(std::mem::size_of::<StoredRecordOwner>())?;
    let product_types = std::alloc::Layout::new::<[usize; 2]>()
        .extend(std::alloc::Layout::new::<ProductTypes>())
        .ok()?
        .0
        .pad_to_align()
        .size();
    bytes = bytes.checked_add(product_types)?;
    // Serde's cautious vectors can grow geometrically; collecting decoded
    // anchor Options also uses Vec's minimum non-zero allocation of four rows.
    let anchor_slots = anchors.len().checked_mul(2)?.max(4);
    let type_slots = type_count.checked_mul(2)?.max(4);
    bytes =
        bytes
            .checked_add(anchor_slots.checked_mul(
                std::mem::size_of::<(DefPath, u32)>() + std::mem::size_of::<Anchor>(),
            )?)?
            .checked_add(type_slots.checked_mul(std::mem::size_of::<TypeRef>())?)?;
    for (path, _) in anchors {
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
        let record = RecordView::open(bytes)?;
        let mut pending = record.type_references()?;
        while let Some(reference) = pending.pop() {
            let traverse = visit(reference.reference())?;
            let component = types::component(self.store.as_ref(), reference)?;
            if traverse {
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
        let size = table
            .with_encoding(|| {
                hirsel_span::with_anchor_codec(&mut anchors, || {
                    indexed_types::inline_names(|| bincode::serialized_size(value))
                })
            })
            .ok()?;
        if size > hirsel_artifact_store::MAX_RECORD as u64 {
            return None;
        }
        self.indexed_record(table.roots(), anchors, owner, || {
            let mut payload = Vec::with_capacity(usize::try_from(size).ok()?);
            bincode::serialize_into(&mut payload, value).ok()?;
            Some(payload)
        })
    }
    /// Normalize portable sharing and first-use positions before encoding a
    /// schema's indexed payload. Live IDs never determine portable positions.
    pub(crate) fn indexed_record(
        &self,
        roots: &[TypeId],
        anchors: AnchorCodec,
        mut owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
        write: impl FnOnce() -> Option<Vec<u8>>,
    ) -> Option<(Vec<u8>, u64)> {
        let references = self.encode(roots)?;
        let mut unique_types = Vec::new();
        let mut reference_positions = std::collections::HashMap::new();
        let mut type_positions = std::collections::HashMap::new();
        for (&id, reference) in roots.iter().zip(references) {
            let next = u32::try_from(unique_types.len()).ok()?;
            let position = *reference_positions.entry(reference).or_insert_with(|| {
                unique_types.push(reference);
                next
            });
            type_positions.insert(id, position);
        }
        let AnchorCodec::Collect(anchors) = anchors else {
            unreachable!()
        };
        let mut unique_anchors = Vec::new();
        let mut owner_positions = std::collections::HashMap::new();
        let mut anchor_positions = std::collections::BTreeMap::new();
        for anchor in anchors {
            let identity = owner(anchor)?;
            let next = u32::try_from(unique_anchors.len()).ok()?.checked_add(1)?;
            let position = *owner_positions.entry(identity.clone()).or_insert_with(|| {
                unique_anchors.push(identity);
                next
            });
            anchor_positions.insert(anchor, position);
        }
        let payload = hirsel_types::with_type_positions(type_positions, || {
            hirsel_span::with_anchor_codec(&mut AnchorCodec::Encode(anchor_positions), || {
                indexed_types::inline_names(write)
            })
        })?;
        let bytes = IndexedEnvelope::encode(&unique_anchors, &unique_types, &payload)?;
        let working = record_decode_bytes(
            &unique_anchors,
            unique_types.len(),
            payload.len(),
            bytes.len(),
        )?;
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
        let record = RecordView::open(bytes)?;
        let anchors = record.anchors(&mut anchor)?;
        decode_payload(
            record.payload(),
            Arc::new(ProductTypes {
                records: self.clone(),
                references: record.type_references()?,
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
        let layout = EnvelopeLayout::open(&bytes)?;
        let payload = bytes.range(
            layout.payload_start as u64,
            (bytes.len() - layout.payload_start) as u64,
            1,
        )?;
        Some(StoredRecord(Arc::new(StoredRecordOwner {
            backing: bytes,
            layout,
            records: self.clone(),
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
fn checked_test_bytes(
    domain: hirsel_artifact_store::Domain,
    bytes: &[u8],
) -> hirsel_artifact_store::Bytes {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "hirsel-codec-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    let store = hirsel_artifact_store::Store::rebuildable(root.clone());
    let reference = store.put(domain, bytes).unwrap();
    let checked = store.get(reference).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    checked
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_artifact_store::Memory;
    use hirsel_syntax::{ContentHash, ItemPath, PackageId};
    use hirsel_types::{Type, TypeId};
    // Owned transport data exists only for explicit malformed-vector edits.
    struct Record {
        anchors: Vec<(DefPath, u32)>,
        types: Vec<TypeRef>,
        payload: Vec<u8>,
    }
    impl Record {
        fn encode(&self) -> Option<Vec<u8>> {
            IndexedEnvelope::encode(&self.anchors, &self.types, &self.payload)
        }
        fn decode(bytes: &[u8]) -> Option<Self> {
            let view = RecordView::open(bytes)?;
            Some(Self {
                anchors: (0..view.layout.anchors)
                    .map(|n| Some(view.layout.anchor(bytes, n)?.to_owned()))
                    .collect::<Option<_>>()?,
                types: view.type_references()?,
                payload: view.payload().to_vec(),
            })
        }
    }
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
    fn ordinary_encoding_uses_indexed_directories_and_normalizes_portable_sharing() {
        let store = Arc::new(Memory::default());
        let records = TypeRecords::new(TypeInterner::new(None), store);
        let unit = records.types.intern(Type::unit());
        let duplicate = records.types.intern_graph(1, |_| vec![Type::unit()])[0];
        assert_ne!(unit, duplicate);
        let a = Anchor::fresh();
        let b = Anchor::fresh();
        let sa = hirsel_span::Span::new(a, 1, 4).unwrap();
        let sb = hirsel_span::Span::new(b, 1, 4).unwrap();
        let encode = |types, spans| {
            records
                .encode_record(&(types, spans), |anchor| {
                    [a, b].contains(&anchor).then(|| (path(), 0))
                })
                .unwrap()
        };
        let canonical = encode((unit, unit), (sa, sa));
        assert_eq!(encode((unit, duplicate), (sa, sb)), canonical);
        assert_eq!(&canonical[..8], b"HIRREC02");
        let view = RecordView::open(&canonical).unwrap();
        assert_eq!(view.layout.anchors, 1);
        assert_eq!(view.layout.types, 1);
        let current = Anchor::fresh();
        let stored = records.open_record(canonical.into()).unwrap();
        let (ids, spans): ((TypeId, TypeId), (hirsel_span::Span, hirsel_span::Span)) = stored
            .decode(|owner, occurrence| (owner == &path() && occurrence == 0).then_some(current))
            .unwrap();
        assert_eq!(ids, (unit, unit));
        assert_eq!(spans.0, spans.1);
        assert_eq!(spans.0.anchor, current);
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
        let view = RecordView::open(&bytes).unwrap();
        let pointer = view.payload().as_ptr();
        let record = records.open_record(bytes.clone()).unwrap();
        assert_eq!(record.payload().as_ptr(), pointer);
        let second = record.clone();
        assert!(Arc::ptr_eq(&record.0, &second.0));
        assert!(Arc::ptr_eq(&record.0.records, &second.0.records));
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
        let bytes = record.encode().unwrap();
        let view = RecordView::open(&bytes).unwrap();
        assert_eq!(view.payload(), record.payload);
        assert_eq!(view.payload().as_ptr(), bytes[48..].as_ptr());
        for len in [0, 1, 23, bytes.len() - 1] {
            assert!(RecordView::open(&bytes[..len]).is_none());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(RecordView::open(&trailing).is_none());
        let mut forged = bytes;
        forged[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(RecordView::open(&forged).is_none());
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
            let bytes = record.encode().unwrap();
            let decoded = Record::decode(&bytes).unwrap();
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
                record_decode_bytes(
                    &record.anchors,
                    record.types.len(),
                    record.payload.len(),
                    bytes.len()
                )
                .unwrap()
                    >= actual as u64,
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
            let obsolete = bincode::serialize(&legacy).unwrap();
            assert!(
                RecordView::open(&obsolete).is_none(),
                "schema 1 has no fallback"
            );
            let record = Record {
                anchors: vec![],
                types: vec![],
                payload: payload.clone(),
            };
            let encoded = record.encode().unwrap();
            let decoded = Record::decode(&encoded).unwrap();
            assert_eq!(decoded.payload, payload);
            assert_eq!(RecordView::open(&encoded).unwrap().payload(), payload);
            assert!(RecordView::open(&encoded[..encoded.len() - 1]).is_none());
            let mut trailing = encoded;
            trailing.push(0);
            assert!(RecordView::open(&trailing).is_none());
        }
        let mut forged = IndexedEnvelope::encode(&[], &[], &[]).unwrap();
        forged[40..48].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(RecordView::open(&forged).is_none());
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
        let reference = RecordView::open(&record)
            .unwrap()
            .type_references()
            .unwrap()[0];
        let forged = IndexedEnvelope::encode(
            &[],
            &[
                TypeRef {
                    member: u32::MAX,
                    ..reference
                },
                reference,
            ],
            &[],
        )
        .unwrap();
        assert!(
            encoder
                .visit_record_types(&forged, |r| Some(seen.insert(r.output.0)))
                .is_none(),
            "copied component identity cannot validate a new member designation"
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
