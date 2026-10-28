//! Checked indexed type rows. Semantic graph edges are member indices, never
//! archived pointers. Scalar labels retain the type schema's explicit tags.
use crate::types::{DOMAIN, TypeRef};
use hirsel_artifact_store::{Bytes, MAX_RECORD};
use hirsel_types::{Type, TypeId, TypeKind};
use std::collections::HashSet;

const MAGIC: &[u8; 8] = b"HIRTY002";
const HEADER: usize = 32;
const REFERENCE: usize = 44;
const ROW: usize = 32;

/// The checked discriminant of an archived type row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum TypeTag {
    Primitive,
    Tuple,
    Parameter,
    Named,
    Record,
    Variant,
    Function,
    Box,
    Builtin,
    Error,
}
impl TypeTag {
    fn of(kind: &TypeKind) -> Self {
        match kind {
            TypeKind::Primitive(_) => Self::Primitive,
            TypeKind::Tuple(_) => Self::Tuple,
            TypeKind::Parameter(_) => Self::Parameter,
            TypeKind::Named { .. } => Self::Named,
            TypeKind::Record(_) => Self::Record,
            TypeKind::Variant(_) => Self::Variant,
            TypeKind::Function(_) => Self::Function,
            TypeKind::Box(_) => Self::Box,
            TypeKind::Builtin { .. } => Self::Builtin,
            TypeKind::Error => Self::Error,
        }
    }
    fn read(tag: u32) -> Option<Self> {
        Some(match tag {
            0 => Self::Primitive,
            1 => Self::Tuple,
            2 => Self::Parameter,
            3 => Self::Named,
            4 => Self::Record,
            5 => Self::Variant,
            6 => Self::Function,
            7 => Self::Box,
            8 => Self::Builtin,
            9 => Self::Error,
            _ => return None,
        })
    }
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}
fn size_at(bytes: &[u8], at: usize) -> Option<usize> {
    u64_at(bytes, at)?.try_into().ok()
}
fn put64(bytes: &mut [u8], at: usize, value: usize) -> Option<()> {
    bytes
        .get_mut(at..at.checked_add(8)?)?
        .copy_from_slice(&u64::try_from(value).ok()?.to_le_bytes());
    Some(())
}
fn put32(bytes: &mut [u8], at: usize, value: u32) -> Option<()> {
    bytes
        .get_mut(at..at.checked_add(4)?)?
        .copy_from_slice(&value.to_le_bytes());
    Some(())
}
fn aligned(value: usize) -> Option<usize> {
    Some(value.checked_add(3)? & !3)
}
fn label(row: &Type) -> Option<Vec<u8>> {
    let mut row = row.clone();
    row.kind = row.kind.map_edges(|_| TypeId::DANGLING);
    hirsel_types::with_session_ids(|| bincode::serialize(&row)).ok()
}

#[derive(Clone, Copy)]
pub(crate) enum OrderedEdge<N> {
    Member(N),
    External(TypeRef),
}

/// Keep Hirsel's SCC key ordering and resolve key ties by the complete rooted
/// encoding. A bucket/hash collision can change work, never canonical output.
pub(crate) fn canonical_order<N: Copy + Eq + std::hash::Hash>(
    nodes: &[N],
    keys: &std::collections::HashMap<N, hirsel_syntax::ContentHash>,
    edges: impl Fn(N) -> Option<Vec<OrderedEdge<N>>>,
    label: impl Fn(N) -> Option<Vec<u8>>,
) -> Option<Vec<N>> {
    let minimum = nodes
        .iter()
        .map(|n| keys.get(n))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .min()?;
    let mut best: Option<(Vec<u8>, Vec<N>)> = None;
    for &root in nodes.iter().filter(|n| keys.get(n) == Some(minimum)) {
        let mut order = Vec::new();
        let mut seen = HashSet::new();
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if !seen.insert(node) {
                continue;
            }
            order.push(node);
            pending.extend(
                edges(node)?
                    .into_iter()
                    .rev()
                    .filter_map(|edge| match edge {
                        OrderedEdge::Member(n) => Some(n),
                        OrderedEdge::External(_) => None,
                    }),
            );
        }
        if order.len() != nodes.len() {
            return None;
        }
        if nodes
            .iter()
            .filter(|n| keys.get(n) == Some(minimum))
            .count()
            == 1
        {
            return Some(order);
        }
        let positions: std::collections::HashMap<_, _> =
            order.iter().enumerate().map(|(p, &n)| (n, p)).collect();
        let mut encoding = Vec::new();
        for &node in &order {
            let label = label(node)?;
            encoding.extend_from_slice(&(label.len() as u64).to_le_bytes());
            encoding.extend_from_slice(&label);
            let edges = edges(node)?;
            encoding.extend_from_slice(&(edges.len() as u64).to_le_bytes());
            for edge in edges {
                match edge {
                    OrderedEdge::Member(n) => {
                        encoding.push(0);
                        encoding.extend_from_slice(
                            &u32::try_from(*positions.get(&n)?).ok()?.to_le_bytes(),
                        );
                    }
                    OrderedEdge::External(reference) => {
                        encoding.push(1);
                        encoding.extend_from_slice(&reference.output.0);
                        encoding.extend_from_slice(&reference.len.to_le_bytes());
                        encoding.extend_from_slice(&reference.member.to_le_bytes());
                    }
                }
            }
        }
        if best
            .as_ref()
            .is_none_or(|(previous, _)| encoding < *previous)
        {
            best = Some((encoding, order));
        }
    }
    best.map(|(_, order)| order)
}

/// One immutable checked component. Opening never interns a TypeId or hydrates
/// a TypeTable. Clones of its backing retain source and admission owners.
#[derive(Clone)]
pub struct CheckedTypeComponent {
    bytes: Bytes,
    count: u32,
    dependencies: u32,
    directory: usize,
    cyclic: bool,
}
/// A borrowed row keeps the checked component alive for the entire access.
#[derive(Clone, Copy)]
pub struct TypeRow<'a> {
    component: &'a CheckedTypeComponent,
    label: &'a [u8],
    edges: &'a [u8],
    tag: TypeTag,
}
/// A checked graph edge. Resolving an external designation requires opening
/// that child independently; this enum conveys no validation of child bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeEdge {
    Member(u32),
    External(TypeRef),
}
impl CheckedTypeComponent {
    pub(crate) fn encode(
        rows: &[&Type],
        positions: &std::collections::HashMap<TypeId, u32>,
        dependencies: &[TypeRef],
        cyclic: bool,
    ) -> Option<Vec<u8>> {
        if rows.is_empty() || (!cyclic && rows.len() != 1) {
            return None;
        }
        let directory = HEADER.checked_add(dependencies.len().checked_mul(REFERENCE)?)?;
        let data = directory.checked_add(rows.len().checked_mul(ROW)?)?;
        if data > MAX_RECORD {
            return None;
        }
        let mut bytes = vec![0; data];
        bytes[..8].copy_from_slice(MAGIC);
        put32(&mut bytes, 8, rows.len().try_into().ok()?)?;
        put32(&mut bytes, 12, dependencies.len().try_into().ok()?)?;
        put32(&mut bytes, 16, u32::from(cyclic))?;
        for (n, reference) in dependencies.iter().enumerate() {
            let at = HEADER + n * REFERENCE;
            bytes[at..at + 32].copy_from_slice(&reference.output.0);
            put64(&mut bytes, at + 32, reference.len.try_into().ok()?)?;
            put32(&mut bytes, at + 40, reference.member)?;
        }
        for (n, row) in rows.iter().enumerate() {
            let descriptor = directory + n * ROW;
            let label = label(row)?;
            let start = bytes.len();
            let end = start.checked_add(label.len())?;
            let edges = row.kind.graph_edges();
            let edge_start = aligned(end)?;
            let next = edge_start.checked_add(edges.len().checked_mul(4)?)?;
            if next > MAX_RECORD {
                return None;
            }
            bytes.extend_from_slice(&label);
            bytes.resize(next, 0);
            put64(&mut bytes, descriptor, start)?;
            put64(&mut bytes, descriptor + 8, label.len())?;
            put64(&mut bytes, descriptor + 16, edge_start)?;
            put32(&mut bytes, descriptor + 24, edges.len().try_into().ok()?)?;
            put32(&mut bytes, descriptor + 28, TypeTag::of(&row.kind) as u32)?;
            for (edge, target) in edges.iter().enumerate() {
                put32(
                    &mut bytes,
                    edge_start + edge * 4,
                    *positions.get(&target.ty)?,
                )?;
            }
        }
        let len = bytes.len();
        put64(&mut bytes, 24, len)?;
        Some(bytes)
    }

    /// Check exact framed content identity, complete framing, canonical layout,
    /// row tags and labels, member bounds, dependency order and SCC numbering.
    /// No serialized flag can skip these checks. The caller admits the backing
    /// and validation scratch before opening untrusted content.
    pub fn open(bytes: Bytes, reference: TypeRef) -> Option<Self> {
        if DOMAIN.reference(&bytes)? != reference.reference()
            || bytes.get(..8)? != MAGIC
            || size_at(&bytes, 24)? != bytes.len()
            || u32_at(&bytes, 20)? != 0
        {
            return None;
        }
        let count = u32_at(&bytes, 8)?;
        let dependencies = u32_at(&bytes, 12)?;
        let cyclic = match u32_at(&bytes, 16)? {
            0 => false,
            1 => true,
            _ => return None,
        };
        if count == 0 || reference.member >= count || (!cyclic && count != 1) {
            return None;
        }
        let directory = HEADER.checked_add((dependencies as usize).checked_mul(REFERENCE)?)?;
        let data = directory.checked_add((count as usize).checked_mul(ROW)?)?;
        if data > bytes.len() {
            return None;
        }
        let component = Self {
            bytes,
            count,
            dependencies,
            directory,
            cyclic,
        };
        component.validate(data)?;
        Some(component)
    }
    pub fn len(&self) -> u32 {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        false
    }
    pub fn is_cyclic(&self) -> bool {
        self.cyclic
    }
    pub fn dependency_count(&self) -> u32 {
        self.dependencies
    }
    pub fn dependency(&self, index: u32) -> Option<TypeRef> {
        if index >= self.dependencies {
            return None;
        }
        let bytes = &*self.bytes;
        let at = HEADER + index as usize * REFERENCE;
        Some(TypeRef {
            output: hirsel_syntax::ContentHash(bytes.get(at..at + 32)?.try_into().ok()?),
            len: u64_at(bytes, at + 32)?,
            member: u32_at(bytes, at + 40)?,
        })
    }
    pub fn row(&self, member: u32) -> Option<TypeRow<'_>> {
        if member >= self.count {
            return None;
        }
        let bytes = &*self.bytes;
        let descriptor = self.directory + member as usize * ROW;
        let label = size_at(bytes, descriptor)?;
        let label_end = label.checked_add(size_at(bytes, descriptor + 8)?)?;
        let edges = size_at(bytes, descriptor + 16)?;
        let edge_end =
            edges.checked_add((u32_at(bytes, descriptor + 24)? as usize).checked_mul(4)?)?;
        Some(TypeRow {
            component: self,
            label: bytes.get(label..label_end)?,
            edges: bytes.get(edges..edge_end)?,
            tag: TypeTag::read(u32_at(bytes, descriptor + 28)?)?,
        })
    }
    pub fn backing(&self) -> &Bytes {
        &self.bytes
    }

    fn validate(&self, mut cursor: usize) -> Option<()> {
        let bytes = &*self.bytes;
        let mut distinct = HashSet::new();
        for n in 0..self.dependencies {
            let reference = self.dependency(n)?;
            if reference.len > MAX_RECORD as u64 || !distinct.insert(reference) {
                return None;
            }
        }
        let positions = self.count.checked_add(self.dependencies)?;
        let mut graph = Vec::with_capacity(self.count as usize);
        let mut next_dependency = 0;
        for member in 0..self.count {
            let row = self.row(member)?;
            let descriptor = self.directory + member as usize * ROW;
            let start = size_at(bytes, descriptor)?;
            let len = size_at(bytes, descriptor + 8)?;
            let end = start.checked_add(len)?;
            let edge_start = size_at(bytes, descriptor + 16)?;
            let edge_count = u32_at(bytes, descriptor + 24)?;
            let edge_end = edge_start.checked_add((edge_count as usize).checked_mul(4)?)?;
            // rkyv d963abc2 validator.rs:135-157 checks overflow, bounds and
            // alignment. Cap'n Proto b5ee48b9 layout.c++:3024-3055 requires the
            // next object at the read head. Here canonical byte/row arrays
            // replace pointer subtrees; integer reads require no Rust alignment.
            if start != cursor
                || edge_start != aligned(end)?
                || edge_end > bytes.len()
                || bytes.get(end..edge_start)?.iter().any(|&b| b != 0)
            {
                return None;
            }
            let tag = row.tag();
            let label = bytes.get(start..end)?;
            let value: Type =
                hirsel_types::with_session_ids(|| crate::options(label.len()).deserialize(label))
                    .ok()?;
            if TypeTag::of(&value.kind) != tag
                || value.kind.graph_edges().len() != edge_count as usize
                || value
                    .kind
                    .graph_edges()
                    .iter()
                    .any(|e| e.ty != TypeId::DANGLING)
                || hirsel_types::with_session_ids(|| bincode::serialize(&value))
                    .ok()?
                    .as_slice()
                    != label
            {
                return None;
            }
            let mut internal = Vec::new();
            for edge in 0..edge_count {
                let position = u32_at(bytes, edge_start + edge as usize * 4)?;
                if position >= positions {
                    return None;
                }
                if position < self.count {
                    internal.push(position as usize);
                } else {
                    let dependency = position - self.count;
                    if dependency > next_dependency {
                        return None;
                    }
                    if dependency == next_dependency {
                        next_dependency += 1;
                    }
                }
            }
            graph.push(internal);
            cursor = edge_end;
        }
        if cursor != bytes.len()
            || next_dependency != self.dependencies
            || (self.count > 1 || graph[0].contains(&0)) != self.cyclic
            || hirsel_graph::components(&graph).len() != 1
        {
            return None;
        }
        // Reuse the producer's exact SCC key rule; no interner or worker IDs
        // enter labels. Root ties compare complete rooted ordered graph encodings.
        let nodes: Vec<_> = (0..self.count).collect();
        let keys = hirsel_graph::scc_keys(
            &nodes,
            |n| graph[n as usize].iter().map(|&n| n as u32).collect(),
            |n, writer| {
                let row = self.row(n).expect("bounded row");
                writer.bytes(row.label());
                for position in row.positions() {
                    if position < self.count {
                        writer.bytes(&[0]);
                    } else {
                        writer.bytes(&[1]);
                        writer.bytes(
                            &bincode::serialize(
                                &self
                                    .dependency(position - self.count)
                                    .expect("checked dependency"),
                            )
                            .expect("reference"),
                        );
                    }
                }
            },
        );
        let order = canonical_order(
            &nodes,
            &keys,
            |n| {
                let row = self.row(n)?;
                (0..row.edge_count())
                    .map(|edge| {
                        Some(match row.edge(edge)? {
                            TypeEdge::Member(n) => OrderedEdge::Member(n),
                            TypeEdge::External(reference) => OrderedEdge::External(reference),
                        })
                    })
                    .collect()
            },
            |n| Some(self.row(n)?.label().to_vec()),
        )?;
        (order == nodes).then_some(())
    }
}
impl TypeRow<'_> {
    fn label(&self) -> &[u8] {
        self.label
    }
    fn positions(&self) -> impl Iterator<Item = u32> + '_ {
        self.edges
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four-byte edge")))
    }
    #[inline]
    pub fn tag(&self) -> TypeTag {
        self.tag
    }
    #[inline]
    pub fn edge_count(&self) -> u32 {
        (self.edges.len() / 4) as u32
    }
    pub fn edge(&self, index: u32) -> Option<TypeEdge> {
        if index >= self.edge_count() {
            return None;
        }
        let position = u32_at(self.edges, index as usize * 4)?;
        Some(if position < self.component.count {
            TypeEdge::Member(position)
        } else {
            TypeEdge::External(self.component.dependency(position - self.component.count)?)
        })
    }
    /// Materialize one row for a transformation or the existing interner API.
    /// Read-only tag/edge traversal does not call this or construct a table.
    pub fn materialize(&self, mut resolve: impl FnMut(TypeEdge) -> Option<TypeId>) -> Option<Type> {
        let mut value: Type = hirsel_types::with_session_ids(|| {
            crate::options(self.label().len()).deserialize(self.label())
        })
        .ok()?;
        let mut edges = Vec::with_capacity(self.edge_count() as usize);
        for n in 0..self.edge_count() {
            edges.push(resolve(self.edge(n)?)?);
        }
        let mut edges = edges.into_iter();
        value.kind = value
            .kind
            .map_edges(|_| edges.next().expect("checked edge count"));
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TypeRecords;
    use hirsel_artifact_store::{Contents, Memory};
    use hirsel_types::TypeInterner;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn fixture() -> (Arc<Memory>, Arc<TypeRecords>, TypeRef) {
        let store = Arc::new(Memory::default());
        let types = TypeInterner::new(None);
        let unit = types.intern(Type::unit());
        let boxed = types.intern(Type::with_kind(TypeKind::Box(unit)));
        let records = TypeRecords::new(types, store.clone());
        let reference = records.encode(&[boxed]).unwrap()[0];
        (store, records, reference)
    }
    #[test]
    fn canonical_root_ties_use_exact_encodings_instead_of_allocation_order() {
        let keys = std::collections::HashMap::from([
            (0, hirsel_syntax::ContentHash::default()),
            (1, hirsel_syntax::ContentHash::default()),
        ]);
        let order = |nodes: &[u32]| {
            canonical_order(
                nodes,
                &keys,
                |n| Some(vec![OrderedEdge::Member(1 - n)]),
                |n| Some(vec![b'A' + n as u8]),
            )
            .unwrap()
        };
        assert_eq!(order(&[0, 1]), vec![0, 1]);
        assert_eq!(order(&[1, 0]), vec![0, 1]);
    }
    #[test]
    fn row_access_preserves_backing_and_admission_without_interning() {
        struct Lease(Arc<AtomicBool>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let (store, records, reference) = fixture();
        let released = Arc::new(AtomicBool::new(false));
        let bytes = store
            .get_content(reference.reference())
            .unwrap()
            .retain_owner(Arc::new(Lease(released.clone())));
        let pointer = bytes.as_ptr();
        let checked = CheckedTypeComponent::open(bytes, reference).unwrap();
        let empty = TypeInterner::new(None);
        let before = empty.len();
        let row = checked.row(0).unwrap();
        assert_eq!(row.tag(), TypeTag::Box);
        assert_eq!(row.edge_count(), 1);
        let Some(TypeEdge::External(child)) = row.edge(0) else {
            panic!()
        };
        assert!(row.edge(1).is_none());
        assert!(checked.row(1).is_none());
        assert_eq!(empty.len(), before);
        let leaf = records.open_component(child).unwrap();
        assert_eq!(leaf.row(child.member()).unwrap().tag(), TypeTag::Tuple);
        assert_eq!(leaf.row(child.member()).unwrap().edge_count(), 0);
        let held = checked.clone();
        drop(checked);
        drop(records);
        drop(store);
        assert_eq!(held.backing().as_ptr(), pointer);
        assert_eq!(held.row(0).unwrap().tag(), TypeTag::Box);
        assert!(!released.load(Ordering::SeqCst));
        drop(held);
        assert!(released.load(Ordering::SeqCst));
    }
    #[test]
    fn checked_identity_never_validates_child_bytes_or_forged_content() {
        let (store, records, reference) = fixture();
        let component = records.open_component(reference).unwrap();
        let Some(TypeEdge::External(child)) = component.row(0).unwrap().edge(0) else {
            panic!()
        };
        let bytes = store.get_content(child.reference()).unwrap();
        let mut corrupt = bytes.to_vec();
        corrupt[0] ^= 1;
        assert!(CheckedTypeComponent::open(corrupt.into(), child).is_none());
        let mut wrong = child;
        wrong.len += 1;
        assert!(CheckedTypeComponent::open(bytes.clone(), wrong).is_none());
        wrong = child;
        wrong.member = u32::MAX;
        assert!(CheckedTypeComponent::open(bytes, wrong).is_none());
        let mut foreign = child.reference();
        foreign.domain.schema -= 1;
        assert!(TypeRef::from_reference(foreign, 0).is_none());
    }
    #[test]
    fn checksummed_rows_reject_wrap_overlap_bad_tags_counts_and_members() {
        let (store, _, reference) = fixture();
        let bytes = store.get_content(reference.reference()).unwrap();
        let directory = HEADER + REFERENCE;
        let edge_start = size_at(&bytes, directory + 16).unwrap();
        for fault in 0..11 {
            let mut bad = bytes.to_vec();
            match fault {
                0 => {
                    put64(&mut bad, directory, usize::MAX).unwrap();
                }
                1 => {
                    put64(&mut bad, directory + 8, usize::MAX).unwrap();
                }
                2 => {
                    put64(&mut bad, directory + 16, usize::MAX).unwrap();
                }
                3 => {
                    put64(&mut bad, directory, directory).unwrap();
                }
                4 => {
                    put64(&mut bad, directory + 16, edge_start - 1).unwrap();
                }
                5 => {
                    put32(&mut bad, directory + 24, u32::MAX).unwrap();
                }
                6 => {
                    put32(&mut bad, directory + 28, u32::MAX).unwrap();
                }
                7 => {
                    put32(&mut bad, directory + 28, TypeTag::Tuple as u32).unwrap();
                }
                8 => {
                    put32(&mut bad, edge_start, u32::MAX).unwrap();
                }
                9 => {
                    put32(&mut bad, edge_start, 0).unwrap();
                }
                10 => {
                    bad.push(0);
                    let len = bad.len();
                    put64(&mut bad, 24, len).unwrap();
                }
                _ => unreachable!(),
            }
            let actual = DOMAIN.reference(&bad).unwrap();
            let forged = TypeRef::from_reference(actual, 0).unwrap();
            assert!(
                CheckedTypeComponent::open(bad.into(), forged).is_none(),
                "fault {fault}"
            );
        }
        for end in 0..bytes.len() {
            let truncated = bytes[..end].to_vec();
            let actual = DOMAIN.reference(&truncated).unwrap();
            assert!(
                CheckedTypeComponent::open(
                    truncated.into(),
                    TypeRef::from_reference(actual, 0).unwrap()
                )
                .is_none()
            );
        }
    }
    #[test]
    fn checksummed_noncanonical_dependency_order_is_rejected() {
        let store = Arc::new(Memory::default());
        let types = TypeInterner::new(None);
        let a = types.intern(Type::unit());
        let b = types.intern(Type::boolean());
        let pair = types.intern(Type::with_kind(TypeKind::Tuple(vec![a, b].into())));
        let records = TypeRecords::new(types, store.clone());
        let reference = records.encode(&[pair]).unwrap()[0];
        let mut bytes = store.get_content(reference.reference()).unwrap().to_vec();
        let directory = HEADER + 2 * REFERENCE;
        let edge_start = size_at(&bytes, directory + 16).unwrap();
        for n in 0..REFERENCE {
            bytes.swap(HEADER + n, HEADER + REFERENCE + n);
        }
        put32(&mut bytes, edge_start, 2).unwrap();
        put32(&mut bytes, edge_start + 4, 1).unwrap();
        let reference = TypeRef::from_reference(DOMAIN.reference(&bytes).unwrap(), 0).unwrap();
        assert!(CheckedTypeComponent::open(bytes.into(), reference).is_none());
    }
}
