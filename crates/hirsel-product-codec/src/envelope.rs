//! Indexed ordinary envelope framing. Payload schemas validate their own rows;
//! opening an envelope never supplies semantic or verifier acceptance.
use crate::{CheckedTypeComponent, TypeRef};
use hirsel_artifact_store::{Bytes, Contents, MAX_RECORD, Reference};
use hirsel_syntax::{DefPath, ItemPath, PackageId};

const MAGIC: &[u8; 8] = b"HIRREC02";
const HEADER: usize = 48;
const ANCHOR: usize = 16;
const TYPE: usize = 44;

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn number(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}
fn size(bytes: &[u8], at: usize) -> Option<usize> {
    number(bytes, at)?.try_into().ok()
}
fn set_number(bytes: &mut [u8], at: usize, value: usize) -> Option<()> {
    bytes
        .get_mut(at..at.checked_add(8)?)?
        .copy_from_slice(&u64::try_from(value).ok()?.to_le_bytes());
    Some(())
}
fn string(bytes: &mut Vec<u8>, value: &str) -> Option<()> {
    if bytes.len().checked_add(4)?.checked_add(value.len())? > MAX_RECORD {
        return None;
    }
    bytes.extend_from_slice(&u32::try_from(value.len()).ok()?.to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
    Some(())
}
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Cursor<'a> {
    fn word(&mut self) -> Option<u32> {
        let n = word(self.bytes, self.at)?;
        self.at = self.at.checked_add(4)?;
        Some(n)
    }
    fn string(&mut self) -> Option<&'a str> {
        let n = self.word()? as usize;
        let end = self.at.checked_add(n)?;
        let value = std::str::from_utf8(self.bytes.get(self.at..end)?).ok()?;
        self.at = end;
        Some(value)
    }
}

/// One source owner, borrowed directly from a checked envelope. A lexical owner
/// designates a source occurrence; it grants no source access or V authority.
#[derive(Clone, Copy)]
pub struct AnchorRow<'a> {
    pub package: &'a str,
    pub version: &'a str,
    pub name: &'a str,
    pub occurrence: u32,
    modules: &'a [u8],
    module_count: u32,
}
impl<'a> AnchorRow<'a> {
    fn read(bytes: &'a [u8]) -> Option<Self> {
        let mut cursor = Cursor { bytes, at: 0 };
        let package = cursor.string()?;
        let version = cursor.string()?;
        let module_count = cursor.word()?;
        // Every module has a four-byte length, even an empty string. Refuse
        // impossible counts before iterating or allocating any owned paths.
        if module_count as usize > bytes.len().checked_sub(cursor.at)? / 4 {
            return None;
        }
        let start = cursor.at;
        for _ in 0..module_count {
            cursor.string()?;
        }
        let modules = bytes.get(start..cursor.at)?;
        let name = cursor.string()?;
        let occurrence = cursor.word()?;
        (cursor.at == bytes.len()).then_some(Self {
            package,
            version,
            modules,
            module_count,
            name,
            occurrence,
        })
    }
    pub fn modules(self) -> impl ExactSizeIterator<Item = &'a str> {
        struct Modules<'a> {
            cursor: Cursor<'a>,
            remaining: u32,
        }
        impl<'a> Iterator for Modules<'a> {
            type Item = &'a str;
            fn next(&mut self) -> Option<Self::Item> {
                if self.remaining == 0 {
                    return None;
                }
                self.remaining -= 1;
                Some(self.cursor.string().expect("checked module string"))
            }
            fn size_hint(&self) -> (usize, Option<usize>) {
                (self.remaining as usize, Some(self.remaining as usize))
            }
        }
        impl ExactSizeIterator for Modules<'_> {}
        Modules {
            cursor: Cursor {
                bytes: self.modules,
                at: 0,
            },
            remaining: self.module_count,
        }
    }
    pub fn to_owned(self) -> (DefPath, u32) {
        (
            DefPath(ItemPath {
                package: PackageId {
                    name: self.package.into(),
                    version: self.version.into(),
                },
                modules: self.modules().map(str::to_owned).collect(),
                name: self.name.into(),
            }),
            self.occurrence,
        )
    }
}

/// A domain/content-checked immutable envelope with directly indexed source and
/// type rows. The payload is a borrowed range, not a decoded semantic product.
#[derive(Clone)]
pub struct IndexedEnvelope {
    bytes: Bytes,
    reference: Reference,
    layout: EnvelopeLayout,
}
#[derive(Clone, Copy)]
pub(super) struct EnvelopeLayout {
    pub anchors: u32,
    pub types: u32,
    type_start: usize,
    pub payload_start: usize,
}
impl IndexedEnvelope {
    /// Canonical fixed-endian framing. Source/type positions preserve semantic
    /// first-use order; their producing payload schema owns that numbering.
    pub fn encode(
        anchors: &[(DefPath, u32)],
        types: &[TypeRef],
        payload: &[u8],
    ) -> Option<Vec<u8>> {
        let type_start = HEADER.checked_add(anchors.len().checked_mul(ANCHOR)?)?;
        let data_start = type_start.checked_add(types.len().checked_mul(TYPE)?)?;
        if data_start.checked_add(payload.len())? > MAX_RECORD {
            return None;
        }
        let mut bytes = vec![0; data_start];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[16..20].copy_from_slice(&u32::try_from(anchors.len()).ok()?.to_le_bytes());
        bytes[20..24].copy_from_slice(&u32::try_from(types.len()).ok()?.to_le_bytes());
        set_number(&mut bytes, 24, type_start)?;
        for (n, reference) in types.iter().enumerate() {
            if reference.len > MAX_RECORD as u64 {
                return None;
            }
            let at = type_start + n * TYPE;
            bytes[at..at + 32].copy_from_slice(&reference.output.0);
            bytes[at + 32..at + 40].copy_from_slice(&reference.len.to_le_bytes());
            bytes[at + 40..at + 44].copy_from_slice(&reference.member.to_le_bytes());
        }
        for (n, (path, occurrence)) in anchors.iter().enumerate() {
            let start = bytes.len();
            string(&mut bytes, &path.0.package.name)?;
            string(&mut bytes, &path.0.package.version)?;
            if bytes.len().checked_add(4)? > MAX_RECORD {
                return None;
            }
            bytes.extend_from_slice(&u32::try_from(path.0.modules.len()).ok()?.to_le_bytes());
            for module in &path.0.modules {
                string(&mut bytes, module)?;
            }
            string(&mut bytes, &path.0.name)?;
            if bytes.len().checked_add(4)? > MAX_RECORD {
                return None;
            }
            bytes.extend_from_slice(&occurrence.to_le_bytes());
            let len = bytes.len().checked_sub(start)?;
            set_number(&mut bytes, HEADER + n * ANCHOR, start)?;
            set_number(&mut bytes, HEADER + n * ANCHOR + 8, len)?;
        }
        let payload_start = bytes.len();
        if payload_start.checked_add(payload.len())? > MAX_RECORD {
            return None;
        }
        bytes.extend_from_slice(payload);
        let len = bytes.len();
        set_number(&mut bytes, 8, len)?;
        set_number(&mut bytes, 32, payload_start)?;
        set_number(&mut bytes, 40, payload.len())?;
        Some(bytes)
    }
    pub fn open(bytes: Bytes, expected: Reference) -> Option<Self> {
        if !bytes.matches_reference(expected) {
            return None;
        }
        let layout = EnvelopeLayout::open(&bytes)?;
        Some(Self {
            bytes,
            reference: expected,
            layout,
        })
    }
    pub fn anchor_count(&self) -> u32 {
        self.layout.anchors
    }
    pub fn reference(&self) -> Reference {
        self.reference
    }
    pub fn type_count(&self) -> u32 {
        self.layout.types
    }
    pub fn anchor(&self, index: u32) -> Option<AnchorRow<'_>> {
        self.layout.anchor(&self.bytes, index)
    }
    /// A designation, not validation of the child. Use `type_component` before
    /// accessing its typed rows; a parent digest never checks child bytes.
    pub fn type_reference(&self, index: u32) -> Option<TypeRef> {
        self.layout.type_reference(&self.bytes, index)
    }
    pub fn type_component(&self, index: u32, store: &dyn Contents) -> Option<CheckedTypeComponent> {
        let reference = self.type_reference(index)?;
        CheckedTypeComponent::open(store.get_content(reference.reference())?, reference)
    }
    pub fn payload(&self) -> Bytes {
        self.bytes
            .range(
                self.layout.payload_start as u64,
                (self.bytes.len() - self.layout.payload_start) as u64,
                1,
            )
            .expect("checked payload extent")
    }
    pub fn backing(&self) -> &Bytes {
        &self.bytes
    }
}

impl EnvelopeLayout {
    pub fn open(bytes: &[u8]) -> Option<Self> {
        let view = bytes;
        if view.len() > MAX_RECORD {
            return None;
        }
        if view.get(..8)? != MAGIC || size(view, 8)? != view.len() {
            return None;
        }
        let anchors = word(view, 16)?;
        let types = word(view, 20)?;
        let type_start = HEADER.checked_add((anchors as usize).checked_mul(ANCHOR)?)?;
        let mut cursor = type_start.checked_add((types as usize).checked_mul(TYPE)?)?;
        let payload_start = size(view, 32)?;
        if size(view, 24)? != type_start
            || cursor > payload_start
            || payload_start.checked_add(size(view, 40)?)? != view.len()
        {
            return None;
        }
        for n in 0..anchors as usize {
            let at = HEADER + n * ANCHOR;
            let start = size(view, at)?;
            let end = start.checked_add(size(view, at + 8)?)?;
            if start != cursor || end > payload_start {
                return None;
            }
            AnchorRow::read(view.get(start..end)?)?;
            cursor = end;
        }
        if cursor != payload_start {
            return None;
        }
        let layout = Self {
            anchors,
            types,
            type_start,
            payload_start,
        };
        for n in 0..types {
            if layout.type_reference(view, n)?.len > MAX_RECORD as u64 {
                return None;
            }
        }
        Some(layout)
    }
    pub fn anchor<'a>(&self, bytes: &'a [u8], index: u32) -> Option<AnchorRow<'a>> {
        if index >= self.anchors {
            return None;
        }
        let at = HEADER + index as usize * ANCHOR;
        let start = size(bytes, at)?;
        let end = start.checked_add(size(bytes, at + 8)?)?;
        AnchorRow::read(bytes.get(start..end)?)
    }
    pub fn type_reference(&self, bytes: &[u8], index: u32) -> Option<TypeRef> {
        if index >= self.types {
            return None;
        }
        let at = self.type_start + index as usize * TYPE;
        Some(TypeRef {
            output: hirsel_syntax::ContentHash(bytes.get(at..at + 32)?.try_into().ok()?),
            len: number(bytes, at + 32)?,
            member: word(bytes, at + 40)?,
        })
    }
}

/// A checked envelope rebound to the consumer's current sources. Type positions
/// resolve through checked component rows on demand, without a copied reference
/// vector or a complete TypeTable. Binding supplies no verifier acceptance.
#[derive(Clone)]
pub struct BoundIndexedEnvelope {
    envelope: std::sync::Arc<IndexedEnvelope>,
    types: std::sync::Arc<IndexedTypes>,
    anchors: std::sync::Arc<[hirsel_span::Anchor]>,
}
struct IndexedTypes {
    envelope: std::sync::Arc<IndexedEnvelope>,
    records: std::sync::Arc<crate::TypeRecords>,
}
impl hirsel_intern::persist::Source<hirsel_types::TypeId> for IndexedTypes {
    fn id(&self, number: u32) -> Option<hirsel_types::TypeId> {
        self.records.get(self.envelope.type_reference(number)?)
    }
}
impl IndexedEnvelope {
    pub fn bind(
        self,
        records: std::sync::Arc<crate::TypeRecords>,
        mut anchor: impl FnMut(&DefPath, u32) -> Option<hirsel_span::Anchor>,
    ) -> Option<BoundIndexedEnvelope> {
        let anchors = (0..self.anchor_count())
            .map(|n| {
                let (path, occurrence) = self.anchor(n)?.to_owned();
                anchor(&path, occurrence)
            })
            .collect::<Option<std::sync::Arc<[_]>>>()?;
        let envelope = std::sync::Arc::new(self);
        Some(BoundIndexedEnvelope {
            types: std::sync::Arc::new(IndexedTypes {
                envelope: envelope.clone(),
                records,
            }),
            envelope,
            anchors,
        })
    }
}
impl BoundIndexedEnvelope {
    pub fn envelope(&self) -> &IndexedEnvelope {
        &self.envelope
    }
    pub(crate) fn matches_bindings(
        &self,
        types: &std::sync::Arc<hirsel_types::TypeInterner>,
        mut anchor: impl FnMut(&DefPath, u32) -> Option<hirsel_span::Anchor>,
    ) -> bool {
        std::sync::Arc::ptr_eq(types, &self.types.records.types)
            && self.anchors.iter().enumerate().all(|(n, bound)| {
                self.envelope.anchor(n as u32).is_some_and(|owner| {
                    let (path, occurrence) = owner.to_owned();
                    anchor(&path, occurrence) == Some(*bound)
                })
            })
    }
    pub(crate) fn bind_types(&self) -> Option<()> {
        for n in 0..self.envelope.type_count() {
            self.types.records.get(self.envelope.type_reference(n)?)?;
        }
        Some(())
    }
    pub(crate) fn decode<T: serde::de::DeserializeOwned>(&self, bytes: &[u8]) -> Option<T> {
        hirsel_types::with_type_source(self.types.clone(), || {
            hirsel_span::with_anchor_codec(
                &mut hirsel_span::AnchorCodec::DecodeShared(self.anchors.clone()),
                || {
                    crate::indexed_types::inline_names(|| {
                        crate::options(bytes.len()).deserialize(bytes)
                    })
                },
            )
        })
        .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProductTypes, TypeRecords, TypeTag};
    use hirsel_artifact_store::{Domain, Memory};
    use hirsel_types::{Type, TypeId, TypeInterner, TypeKind};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    const DOMAIN: Domain = Domain {
        kind: 0x5245_4344,
        schema: 2,
    };
    fn path() -> DefPath {
        DefPath(ItemPath {
            package: PackageId {
                name: "codec".into(),
                version: "1".into(),
            },
            modules: vec!["module".into(), "nested".into()],
            name: "owner".into(),
        })
    }
    fn fixture() -> (Arc<Memory>, Vec<u8>, TypeRef) {
        let store = Arc::new(Memory::default());
        let types = TypeInterner::new(None);
        let unit = types.intern(Type::unit());
        let boxed = types.intern(Type::with_kind(TypeKind::Box(unit)));
        let encoder = TypeRecords::new(types, store.clone());
        let reference = encoder.encode(&[boxed]).unwrap()[0];
        let bytes =
            IndexedEnvelope::encode(&[(path(), 7)], &[reference], &0u32.to_le_bytes()).unwrap();
        (store, bytes, reference)
    }
    #[test]
    fn indexed_sources_and_types_use_the_real_checked_component_path() {
        let (store, bytes, reference) = fixture();
        let expected = store.put_content(DOMAIN, &bytes).unwrap();
        let envelope =
            IndexedEnvelope::open(store.get_content(expected).unwrap(), expected).unwrap();
        assert_eq!(envelope.reference(), expected);
        assert_eq!(envelope.anchor_count(), 1);
        assert_eq!(envelope.type_count(), 1);
        let anchor = envelope.anchor(0).unwrap();
        assert_eq!(anchor.package, "codec");
        assert_eq!(anchor.modules().collect::<Vec<_>>(), ["module", "nested"]);
        assert_eq!(anchor.to_owned(), (path(), 7));
        assert!(envelope.anchor(1).is_none());
        assert_eq!(envelope.type_reference(0), Some(reference));
        assert!(envelope.type_reference(1).is_none());
        let component = envelope.type_component(0, &*store).unwrap();
        assert_eq!(
            component.row(reference.member()).unwrap().tag(),
            TypeTag::Box
        );
        let decoder = TypeRecords::new(TypeInterner::new(None), store.clone());
        let id: TypeId = hirsel_types::with_type_source(
            Arc::new(ProductTypes {
                records: decoder.clone(),
                references: vec![envelope.type_reference(0).unwrap()],
            }),
            || bincode::deserialize(&envelope.payload()),
        )
        .unwrap();
        let TypeKind::Box(child) = decoder.types.row(id).kind else {
            panic!()
        };
        assert_eq!(decoder.types.row(child), &Type::unit());
    }
    #[test]
    fn envelope_children_keep_the_complete_backing_and_admission_alive() {
        struct Lease(Arc<AtomicBool>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let (_, bytes, _) = fixture();
        let expected = DOMAIN.reference(&bytes).unwrap();
        let released = Arc::new(AtomicBool::new(false));
        let bytes = Bytes::from(bytes).retain_owner(Arc::new(Lease(released.clone())));
        let mut parent = Vec::new();
        bytes.visit_retained_allocations(|id, n| parent.push((id, n)));
        let envelope = IndexedEnvelope::open(bytes, expected).unwrap();
        let payload = envelope.payload();
        let mut child = Vec::new();
        payload.visit_retained_allocations(|id, n| child.push((id, n)));
        assert_eq!(parent, child);
        assert!(parent.iter().map(|(_, n)| *n).sum::<usize>() > payload.len());
        drop(envelope);
        assert_eq!(&*payload, 0u32.to_le_bytes());
        assert!(!released.load(Ordering::SeqCst));
        drop(payload);
        assert!(released.load(Ordering::SeqCst));
    }
    #[test]
    fn framing_rejects_wraparound_overlaps_utf8_counts_and_trailing_bytes() {
        let (_, bytes, _) = fixture();
        let anchor_start = size(&bytes, HEADER).unwrap();
        let module_count = anchor_start + 4 + 5 + 4 + 1;
        for fault in 0..12 {
            let mut bad = bytes.clone();
            match fault {
                0 => bad[0] ^= 1,
                1 => {
                    bad[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
                }
                2 => {
                    bad[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
                }
                3 => {
                    set_number(&mut bad, 24, usize::MAX).unwrap();
                }
                4 => {
                    set_number(&mut bad, 32, usize::MAX).unwrap();
                }
                5 => {
                    set_number(&mut bad, 40, usize::MAX).unwrap();
                }
                6 => {
                    set_number(&mut bad, HEADER, HEADER).unwrap();
                }
                7 => {
                    set_number(&mut bad, HEADER + 8, usize::MAX).unwrap();
                }
                8 => bad[anchor_start + 4] = 0xff,
                9 => {
                    bad[module_count..module_count + 4].copy_from_slice(&u32::MAX.to_le_bytes());
                }
                10 => {
                    bad.push(0);
                }
                11 => {
                    let at = HEADER + ANCHOR + 32;
                    bad[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
                }
                _ => unreachable!(),
            }
            let reference = DOMAIN.reference(&bad).unwrap();
            assert!(
                IndexedEnvelope::open(crate::checked_test_bytes(DOMAIN, &bad), reference).is_none(),
                "fault {fault}"
            );
        }
        for end in 0..bytes.len() {
            let truncated = bytes[..end].to_vec();
            let reference = DOMAIN.reference(&truncated).unwrap();
            assert!(IndexedEnvelope::open(truncated.into(), reference).is_none());
        }
        let mut reference = DOMAIN.reference(&bytes).unwrap();
        reference.len -= 1;
        assert!(IndexedEnvelope::open(bytes.clone().into(), reference).is_none());
        reference = DOMAIN.reference(&bytes).unwrap();
        reference.domain.schema += 1;
        assert!(IndexedEnvelope::open(bytes.into(), reference).is_none());
    }
    #[test]
    fn parent_integrity_does_not_validate_missing_corrupt_or_forged_child_members() {
        let (store, bytes, child) = fixture();
        let envelope =
            IndexedEnvelope::open(bytes.clone().into(), DOMAIN.reference(&bytes).unwrap()).unwrap();
        assert!(envelope.type_component(0, &Memory::default()).is_none());
        struct Lying(Bytes);
        impl Contents for Lying {
            fn get_content(&self, _: Reference) -> Option<Bytes> {
                Some(self.0.clone())
            }
            fn put_content(&self, _: Domain, _: &[u8]) -> Option<Reference> {
                None
            }
        }
        let mut corrupt = store.get_content(child.reference()).unwrap().to_vec();
        corrupt[0] ^= 1;
        assert!(envelope.type_component(0, &Lying(corrupt.into())).is_none());
        let mut forged = child;
        forged.member = u32::MAX;
        let bytes = IndexedEnvelope::encode(&[], &[forged], &[]).unwrap();
        let envelope =
            IndexedEnvelope::open(bytes.clone().into(), DOMAIN.reference(&bytes).unwrap()).unwrap();
        assert!(envelope.type_component(0, &*store).is_none());
    }
    #[test]
    fn bulk_payloads_are_borrowed_and_bounded_at_all_wire_lengths() {
        for len in [0, 1, 127, 8192] {
            let payload: Vec<_> = (0..len).map(|n| n as u8).collect();
            let bytes = IndexedEnvelope::encode(&[], &[], &payload).unwrap();
            let expected = DOMAIN.reference(&bytes).unwrap();
            let bytes = Bytes::from(bytes);
            let pointer = bytes.as_ptr();
            let envelope = IndexedEnvelope::open(bytes, expected).unwrap();
            let borrowed = envelope.payload();
            assert_eq!(&*borrowed, payload);
            assert_eq!(borrowed.as_ptr(), pointer.wrapping_add(HEADER));
            assert_eq!(envelope.anchor_count(), 0);
            assert_eq!(envelope.type_count(), 0);
        }
        assert!(IndexedEnvelope::encode(&[], &[], &vec![0; MAX_RECORD]).is_none());
    }
}
