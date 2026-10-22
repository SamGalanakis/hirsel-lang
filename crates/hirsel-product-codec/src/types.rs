//! Canonical immutable type components. Ordinary query records carry only
//! directly used references. Decoding a reference reuses already interned
//! components and never decodes another product's complete TypeTable.
use bincode::Options;
use hirsel_artifact_store::{Contents, Domain, Output, Reference};
use hirsel_intern::persist::Source;
use hirsel_syntax::ContentHash;
use hirsel_types::{TypeId, TypeInterner, TypeTable};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

const DOMAIN: Domain = Domain { kind: 2, schema: 1 };

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TypeRef {
    output: ContentHash,
    len: u64,
    member: u32,
}
impl TypeRef {
    fn key(self) -> (ContentHash, u64) {
        (self.output, self.len)
    }
    fn reference(self) -> Reference {
        Reference {
            output: Output(self.output),
            domain: DOMAIN,
            len: self.len,
        }
    }
}
#[derive(Serialize, Deserialize)]
struct Component {
    dependencies: Vec<TypeRef>,
    count: u32,
    cyclic: bool,
    rows: Vec<u8>,
}

pub struct TypeRecords {
    pub types: Arc<TypeInterner>,
    pub store: Arc<dyn Contents>,
    encoded: Mutex<HashMap<TypeId, TypeRef>>,
    decoded: Mutex<HashMap<(ContentHash, u64), Arc<[TypeId]>>>,
}
impl TypeRecords {
    pub fn new(types: Arc<TypeInterner>, store: Arc<dyn Contents>) -> Arc<Self> {
        Arc::new(Self {
            types,
            store,
            encoded: Mutex::new(HashMap::new()),
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
                    let mut row = self.types.row(id).clone();
                    row.kind = row.kind.map_edges(|_| TypeId::DANGLING);
                    writer.bytes(
                        &hirsel_types::with_session_ids(|| bincode::serialize(&row))
                            .expect("type label"),
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
            let root = *members.iter().min_by_key(|&&n| keys[&nodes[n]])?;
            let mut order = Vec::new();
            let mut seen = HashSet::new();
            let mut stack = vec![root];
            while let Some(n) = stack.pop() {
                if !seen.insert(n) {
                    continue;
                }
                order.push(nodes[n]);
                stack.extend(graph[n].iter().rev().copied().filter(|n| set.contains(n)));
            }
            let mut positions: HashMap<_, _> = order
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i as u32))
                .collect();
            let mut dependencies = Vec::new();
            for &id in &order {
                for edge in self.types.row(id).kind.graph_edges() {
                    if positions.contains_key(&edge.ty) {
                        continue;
                    }
                    positions.insert(edge.ty, (order.len() + dependencies.len()).try_into().ok()?);
                    dependencies.push(*known.get(&edge.ty)?);
                }
            }
            let rows = hirsel_types::with_type_positions(positions, || {
                let rows: Vec<_> = order.iter().map(|id| self.types.row(*id)).collect();
                bincode::serialize(&rows)
            })
            .ok()?;
            let component = Component {
                count: order.len().try_into().ok()?,
                cyclic: order.len() > 1 || graph[root].contains(&root),
                dependencies,
                rows,
            };
            let bytes = bincode::serialize(&component).ok()?;
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
    fn component(&self, reference: TypeRef) -> Option<Component> {
        let bytes = self.store.get_content(reference.reference())?;
        let component: Component = options(bytes.len()).deserialize(&bytes).ok()?;
        if component.count == 0
            || component.count as usize > component.rows.len()
            || (!component.cyclic && component.count != 1)
        {
            return None;
        }
        Some(component)
    }
    fn get(&self, reference: TypeRef) -> Option<TypeId> {
        if reference.len > hirsel_artifact_store::MAX_RECORD as u64 {
            return None;
        }
        let mut known = self.decoded.lock().unwrap_or_else(|e| e.into_inner());
        if !known.contains_key(&reference.key()) {
            // Explicit DFS avoids growing a Rust/serde call stack with a deep
            // chain of components. External references must form a DAG.
            let mut active = HashSet::from([reference.output]);
            let mut stack = vec![(reference, self.component(reference)?, 0usize)];
            while let Some((_, component, next)) = stack.last_mut() {
                if let Some(dependency) = component.dependencies.get(*next).copied() {
                    *next += 1;
                    if let Some(ids) = known.get(&dependency.key()) {
                        ids.get(dependency.member as usize)?;
                    } else {
                        if !active.insert(dependency.output) {
                            return None;
                        }
                        stack.push((dependency, self.component(dependency)?, 0));
                    }
                    continue;
                }
                let (reference, component, _) = stack.pop()?;
                let dependencies: Vec<_> = component
                    .dependencies
                    .iter()
                    .map(|d| known.get(&d.key())?.get(d.member as usize).copied())
                    .collect::<Option<_>>()?;
                let table: TypeTable = options(component.rows.len())
                    .deserialize(&component.rows)
                    .ok()?;
                let count = component.count as usize;
                if table.len() != count
                    || !table.valid_positions(count.checked_add(dependencies.len())?)
                {
                    return None;
                }
                let local_graph: Vec<Vec<usize>> = table
                    .edge_positions()
                    .iter()
                    .map(|edges| {
                        edges
                            .iter()
                            .filter_map(|&n| ((n as usize) < count).then_some(n as usize))
                            .collect()
                    })
                    .collect();
                let cyclic = count > 1 || local_graph[0].contains(&0);
                if cyclic != component.cyclic || hirsel_graph::components(&local_graph).len() != 1 {
                    return None;
                }
                let resolve = |local: &[TypeId]| {
                    let ids: Vec<_> = local.iter().chain(&dependencies).copied().collect();
                    table
                        .resolve_positions(&ids)
                        .expect("validated component indices")
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
fn options(len: usize) -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(len as u64)
        .reject_trailing_bytes()
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
        for fault in 0..4 {
            let mut component: Component = options(bytes.len()).deserialize(&bytes).unwrap();
            match fault {
                0 => component.rows.pop().map(|_| ()).unwrap(),
                1 => component.count += 1,
                2 => component.cyclic = true,
                3 => component.dependencies[0].output = ContentHash::of_bytes(b"absent"),
                _ => unreachable!(),
            }
            let bytes = bincode::serialize(&component).unwrap();
            let output = store.put_content(DOMAIN, &bytes).unwrap();
            let forged = TypeRef {
                output: output.output.0,
                len: output.len,
                member: 0,
            };
            let decoder = TypeRecords::new(TypeInterner::new(None), store.clone());
            let before = decoder.types.len();
            assert!(decoder.get(forged).is_none(), "fault {fault}");
            // Valid external dependencies can be interned before a corrupt
            // parent's local rows are rejected, but the parent never publishes.
            assert!(decoder.types.len() <= before + 2);
        }
    }
}
