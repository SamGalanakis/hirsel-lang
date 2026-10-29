//! Canonical immutable type components. Ordinary query records carry only
//! directly used references. Decoding a reference reuses already interned
//! components and never decodes another product's complete TypeTable.
use crate::{CheckedTypeComponent, TypeEdge};
use hirsel_artifact_store::{Contents, Domain, Output, Reference};
use hirsel_intern::persist::Source;
use hirsel_syntax::ContentHash;
use hirsel_types::{TypeId, TypeInterner};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub(super) const DOMAIN: Domain = Domain { kind: 2, schema: 2 };

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TypeRef {
    pub(super) output: ContentHash,
    pub(super) len: u64,
    pub(super) member: u32,
}
impl TypeRef {
    /// A portable designation, with no verifier or content authority.
    pub fn from_reference(reference: Reference, member: u32) -> Option<Self> {
        (reference.domain == DOMAIN && reference.len <= hirsel_artifact_store::MAX_RECORD as u64)
            .then_some(Self {
                output: reference.output.0,
                len: reference.len,
                member,
            })
    }
    pub fn member(self) -> u32 {
        self.member
    }
    pub(super) fn key(self) -> (ContentHash, u64) {
        (self.output, self.len)
    }
    pub fn reference(self) -> Reference {
        Reference {
            output: Output(self.output),
            domain: DOMAIN,
            len: self.len,
        }
    }
}

pub struct TypeRecords {
    pub types: Arc<TypeInterner>,
    pub store: Arc<dyn Contents>,
    encoded: Mutex<HashMap<TypeId, TypeRef>>,
    pub(super) base: HashMap<TypeRef, TypeId>,
    decoded: Mutex<HashMap<(ContentHash, u64), Arc<[TypeId]>>>,
}
impl TypeRecords {
    /// The content domain of every published type record.
    pub const DOMAIN: Domain = DOMAIN;
    /// Open a component for read-only row access without interning its graph.
    pub fn open_component(&self, reference: TypeRef) -> Option<CheckedTypeComponent> {
        component(self.store.as_ref(), reference)
    }
    pub fn new(types: Arc<TypeInterner>, store: Arc<dyn Contents>) -> Arc<Self> {
        Self::with_base(types, store, HashMap::new())
    }
    pub(super) fn with_base(
        types: Arc<TypeInterner>,
        store: Arc<dyn Contents>,
        base: HashMap<TypeRef, TypeId>,
    ) -> Arc<Self> {
        Arc::new(Self {
            types,
            store,
            encoded: Mutex::new(
                base.iter()
                    .map(|(reference, id)| (*id, *reference))
                    .collect(),
            ),
            base,
            decoded: Mutex::new(HashMap::new()),
        })
    }
    pub fn encode(&self, roots: &[TypeId]) -> Option<Vec<TypeRef>> {
        let mut known = self.encoded.lock().unwrap_or_else(|e| e.into_inner());
        let mut nodes = Vec::new();
        let mut numbers = HashMap::new();
        let mut todo = roots.to_vec();
        todo.reverse();
        while let Some(id) = todo.pop() {
            if known.contains_key(&id) || numbers.contains_key(&id) {
                continue;
            }
            let row = self.types.get(id)?;
            numbers.insert(id, nodes.len());
            nodes.push(id);
            todo.extend(row.kind.graph_edges().into_iter().rev().map(|e| e.ty));
        }
        let graph: Vec<Vec<usize>> = nodes
            .iter()
            .map(|id| {
                self.types
                    .row(*id)
                    .kind
                    .graph_edges()
                    .iter()
                    .filter_map(|e| numbers.get(&e.ty).copied())
                    .collect()
            })
            .collect();
        // Reuse the existing canonical SCC graph keys to choose a stable root.
        // External cached edges are local labels; internal edges remain graph edges.
        for members in hirsel_graph::components(&graph) {
            let set: HashSet<_> = members.iter().copied().collect();
            let component_nodes: Vec<_> = members.iter().map(|&n| nodes[n]).collect();
            let keys = hirsel_graph::scc_keys(
                &component_nodes,
                |id| {
                    self.types
                        .row(id)
                        .kind
                        .graph_edges()
                        .iter()
                        .filter_map(|e| {
                            numbers.get(&e.ty).filter(|n| set.contains(n)).map(|_| e.ty)
                        })
                        .collect()
                },
                |id, writer| {
                    writer.bytes(
                        &crate::indexed_types::label(self.types.row(id)).expect("type label"),
                    );
                    for edge in self.types.row(id).kind.graph_edges() {
                        match known.get(&edge.ty) {
                            Some(reference) => {
                                writer.bytes(&[1]);
                                writer
                                    .bytes(&bincode::serialize(reference).expect("type reference"));
                            }
                            None => writer.bytes(&[0]),
                        }
                    }
                },
            );
            let order = crate::indexed_types::canonical_order(
                &component_nodes,
                &keys,
                |id| {
                    self.types
                        .row(id)
                        .kind
                        .graph_edges()
                        .iter()
                        .map(|edge| {
                            Some(if numbers.get(&edge.ty).is_some_and(|n| set.contains(n)) {
                                crate::indexed_types::OrderedEdge::Member(edge.ty)
                            } else {
                                crate::indexed_types::OrderedEdge::External(*known.get(&edge.ty)?)
                            })
                        })
                        .collect()
                },
                |id| crate::indexed_types::label(self.types.row(id)),
            )?;
            let root = *numbers.get(order.first()?)?;
            let mut positions: HashMap<_, _> = order
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i as u32))
                .collect();
            let mut dependencies = Vec::new();
            let mut external = HashMap::new();
            for &id in &order {
                for edge in self.types.row(id).kind.graph_edges() {
                    if positions.contains_key(&edge.ty) {
                        continue;
                    }
                    let reference = *known.get(&edge.ty)?;
                    let next = (order.len() + dependencies.len()).try_into().ok()?;
                    let position = *external.entry(reference).or_insert_with(|| {
                        dependencies.push(reference);
                        next
                    });
                    positions.insert(edge.ty, position);
                }
            }
            let rows: Vec<_> = order.iter().map(|id| self.types.row(*id)).collect();
            let bytes = CheckedTypeComponent::encode(
                &rows,
                &positions,
                &dependencies,
                order.len() > 1 || graph[root].contains(&root),
            )?;
            let output = self.store.put_content(DOMAIN, &bytes)?;
            for (member, id) in order.into_iter().enumerate() {
                known.insert(
                    id,
                    TypeRef {
                        output: output.output.0,
                        len: output.len,
                        member: member as u32,
                    },
                );
            }
        }
        roots.iter().map(|id| known.get(id).copied()).collect()
    }
    pub(super) fn get(&self, reference: TypeRef) -> Option<TypeId> {
        if let Some(id) = self.base.get(&reference) {
            return Some(*id);
        }
        if reference.len > hirsel_artifact_store::MAX_RECORD as u64 {
            return None;
        }
        let mut known = self.decoded.lock().unwrap_or_else(|e| e.into_inner());
        if !known.contains_key(&reference.key()) {
            // Explicit DFS avoids growing a Rust/serde call stack with a deep
            // chain of components. External references must form a DAG.
            let mut active = HashSet::from([reference.output]);
            let mut stack = vec![(
                reference,
                component(self.store.as_ref(), reference)?,
                0usize,
            )];
            while let Some((_, component, next)) = stack.last_mut() {
                if let Some(dependency) = component.dependency((*next).try_into().ok()?) {
                    *next += 1;
                    if self.base.contains_key(&dependency) {
                        continue;
                    }
                    if let Some(ids) = known.get(&dependency.key()) {
                        ids.get(dependency.member as usize)?;
                    } else {
                        if !active.insert(dependency.output) {
                            return None;
                        }
                        stack.push((
                            dependency,
                            self::component(self.store.as_ref(), dependency)?,
                            0,
                        ));
                    }
                    continue;
                }
                let (reference, component, _) = stack.pop()?;
                let dependencies: HashMap<_, _> = (0..component.dependency_count())
                    .map(|n| {
                        let d = component.dependency(n)?;
                        let id = self
                            .base
                            .get(&d)
                            .copied()
                            .or_else(|| known.get(&d.key())?.get(d.member as usize).copied())?;
                        Some((d, id))
                    })
                    .collect::<Option<_>>()?;
                let count = component.len() as usize;
                let cyclic = component.is_cyclic();
                let resolve = |local: &[TypeId]| {
                    (0..component.len())
                        .map(|member| {
                            component
                                .row(member)
                                .expect("checked row")
                                .materialize(|edge| match edge {
                                    TypeEdge::Member(n) => local.get(n as usize).copied(),
                                    TypeEdge::External(reference) => {
                                        dependencies.get(&reference).copied()
                                    }
                                })
                                .expect("validated component indices")
                        })
                        .collect::<Vec<_>>()
                };
                let ids: Arc<[TypeId]> = if cyclic {
                    self.types.intern_graph(count, resolve).into()
                } else {
                    Arc::from([self.types.intern(resolve(&[TypeId::DANGLING]).remove(0))])
                };
                active.remove(&reference.output);
                known.insert(reference.key(), ids);
            }
        }
        known
            .get(&reference.key())?
            .get(reference.member as usize)
            .copied()
    }
}
pub(super) fn component(store: &dyn Contents, reference: TypeRef) -> Option<CheckedTypeComponent> {
    CheckedTypeComponent::open(store.get_content(reference.reference())?, reference)
}
pub struct ProductTypes {
    pub records: Arc<TypeRecords>,
    pub references: Vec<TypeRef>,
}
impl Source<TypeId> for ProductTypes {
    fn id(&self, number: u32) -> Option<TypeId> {
        self.records.get(*self.references.get(number as usize)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_artifact_store::{Bytes, Memory};
    use hirsel_types::{Primitive, Type, TypeKind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Counted {
        inner: Memory,
        reads: AtomicUsize,
    }
    impl Contents for Counted {
        fn get_content(&self, reference: Reference) -> Option<Bytes> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.inner.get_content(reference)
        }
        fn put_content(&self, domain: Domain, bytes: &[u8]) -> Option<Reference> {
            self.inner.put_content(domain, bytes)
        }
    }
    fn graph(reverse: bool) -> (Arc<TypeInterner>, TypeId, TypeId, TypeId) {
        let types = TypeInterner::new(None);
        let (unit, byte) = if reverse {
            let byte = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U8)));
            (types.intern(Type::unit()), byte)
        } else {
            let unit = types.intern(Type::unit());
            (
                unit,
                types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U8))),
            )
        };
        let pair = types.intern(Type::with_kind(TypeKind::Tuple(vec![byte, unit].into())));
        let unused = types.intern(Type::with_kind(TypeKind::Box(byte)));
        (types, pair, byte, unused)
    }
    #[test]
    fn canonical_components_ignore_interner_order_and_previous_root_requests() {
        let store = Arc::new(Counted::default());
        let (a, pair_a, _, _) = graph(false);
        let (b, pair_b, byte_b, _) = graph(true);
        let a = TypeRecords::new(a, store.clone());
        let b = TypeRecords::new(b, store.clone());
        let expected = a.encode(&[pair_a]).unwrap();
        b.encode(&[byte_b]).unwrap();
        assert_eq!(expected, b.encode(&[pair_b]).unwrap());
        let decoder = TypeRecords::new(TypeInterner::new(None), store);
        let loaded = decoder.get(expected[0]).unwrap();
        assert_eq!(decoder.encode(&[loaded]).unwrap(), expected);
    }
    #[test]
    fn cyclic_component_bytes_ignore_root_sharing_and_worker_history() {
        fn cycle(reverse: bool, split: bool) -> (Arc<TypeInterner>, Vec<TypeId>) {
            let types = TypeInterner::new(None);
            let unit = types.intern(Type::unit());
            if reverse {
                types.intern(Type::boolean());
            }
            let external = types.intern(Type::with_kind(TypeKind::Box(unit)));
            let other = if split {
                types.intern(Type::with_kind(TypeKind::Box(types.intern(Type::unit()))))
            } else {
                external
            };
            let ids = types.intern_graph(2, |ids| {
                let (a, b) = if reverse {
                    (ids[1], ids[0])
                } else {
                    (ids[0], ids[1])
                };
                let a_row = Type::with_kind(TypeKind::Tuple(vec![b, external, other].into()));
                let b_row = Type::with_kind(TypeKind::Box(a));
                if reverse {
                    vec![b_row, a_row]
                } else {
                    vec![a_row, b_row]
                }
            });
            let roots = if reverse { vec![ids[1], ids[0]] } else { ids };
            (types, roots)
        }
        let store = Arc::new(Memory::default());
        let (types, roots) = cycle(false, false);
        let expected = TypeRecords::new(types, store.clone())
            .encode(&roots)
            .unwrap();
        for reverse in [false, true] {
            for split in [false, true] {
                let (types, roots) = cycle(reverse, split);
                let records = TypeRecords::new(types, store.clone());
                records.encode(&[roots[1]]).unwrap();
                assert_eq!(records.encode(&roots).unwrap(), expected);
                let checked = records.open_component(expected[0]).unwrap();
                assert_eq!(checked.len(), 2);
                assert_eq!(checked.dependency_count(), 1, "canonical external sharing");
                let decoder = TypeRecords::new(TypeInterner::new(None), store.clone());
                let restored: Vec<_> = expected.iter().map(|&r| decoder.get(r).unwrap()).collect();
                assert_eq!(decoder.encode(&restored).unwrap(), expected);
            }
        }
    }
    #[test]
    fn lazy_type_source_decodes_only_demanded_components_and_reuses_rows() {
        let store = Arc::new(Counted::default());
        let (types, pair, _, unused) = graph(false);
        let encoder = TypeRecords::new(types, store.clone());
        let references = encoder.encode(&[pair, unused]).unwrap();
        let decoder = TypeRecords::new(TypeInterner::new(None), store.clone());
        let source = Arc::new(ProductTypes {
            records: decoder.clone(),
            references,
        });
        assert_eq!(store.reads.load(Ordering::Relaxed), 0);
        let pair: TypeId = hirsel_types::with_type_source(source.clone(), || {
            bincode::deserialize(&0u32.to_le_bytes())
        })
        .unwrap();
        assert_eq!(
            store.reads.load(Ordering::Relaxed),
            3,
            "tuple plus its two demanded children"
        );
        let repeated: TypeId =
            hirsel_types::with_type_source(source, || bincode::deserialize(&0u32.to_le_bytes()))
                .unwrap();
        assert_eq!(repeated, pair);
        assert_eq!(store.reads.load(Ordering::Relaxed), 3);
        assert_eq!(decoder.types.intern(decoder.types.row(pair).clone()), pair);
    }
    #[test]
    fn cyclic_components_preserve_internal_edges_and_reject_forged_members() {
        let store = Arc::new(Counted::default());
        let types = TypeInterner::new(None);
        let ids = types.intern_graph(2, |ids| {
            vec![
                Type::with_kind(TypeKind::Box(ids[1])),
                Type::with_kind(TypeKind::Tuple(vec![ids[0]].into())),
            ]
        });
        let encoder = TypeRecords::new(types, store.clone());
        let refs = encoder.encode(&ids).unwrap();
        assert_eq!(refs[0].output, refs[1].output);
        assert_ne!(refs[0].member, refs[1].member);
        let decoder = TypeRecords::new(TypeInterner::new(None), store);
        let a = decoder.get(refs[0]).unwrap();
        let b = decoder.get(refs[1]).unwrap();
        assert_eq!(decoder.types.row(a).kind.graph_edges()[0].ty, b);
        assert_eq!(decoder.types.row(b).kind.graph_edges()[0].ty, a);
        let mut invalid = refs[0];
        invalid.member = u32::MAX;
        assert!(decoder.get(invalid).is_none());
        invalid = refs[0];
        invalid.len += 1;
        assert!(
            decoder.get(invalid).is_none(),
            "a cached digest does not waive length validation"
        );
    }
    #[test]
    fn missing_truncated_wrong_count_and_forged_cycle_components_are_misses() {
        let store = Arc::new(Counted::default());
        let (types, pair, _, _) = graph(false);
        let encoder = TypeRecords::new(types, store.clone());
        let reference = encoder.encode(&[pair]).unwrap()[0];
        let bytes = store.inner.get_content(reference.reference()).unwrap();
        for fault in 0..7 {
            let mut bytes = bytes.to_vec();
            match fault {
                0 => {
                    bytes.pop();
                }
                1 => {
                    bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
                }
                2 => {
                    bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
                }
                3 => {
                    bytes[32..64].copy_from_slice(&ContentHash::of_bytes(b"absent").0);
                }
                4 => {
                    bytes[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
                }
                5 => {
                    bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
                }
                6 => {
                    bytes[20] = 1;
                }
                _ => unreachable!(),
            }
            let output = store.put_content(DOMAIN, &bytes).unwrap();
            let forged = TypeRef {
                output: output.output.0,
                len: output.len,
                member: 0,
            };
            let decoder = TypeRecords::new(TypeInterner::new(None), store.clone());
            let before = decoder.types.len();
            assert!(decoder.get(forged).is_none(), "fault {fault}");
            assert!(decoder.types.len() <= before + 2);
        }
    }
}
