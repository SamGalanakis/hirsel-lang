//! Canonical per-definition type tables: the persisted form of interned rows.
//! A table numbers rows in first-encounter order from its roots, so its bytes
//! depend only on the graphs' content, never on interner numbering or the
//! order rows were interned in.
use crate::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// A missing edge target: an id no row of the encoded graph has.
const MISSING: u32 = u32::MAX;

/// Rows whose edges are positions in this table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeTable {
    rows: Vec<Type>,
}
impl TypeTable {
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// The closure of `roots`: every row reachable from them, in first-encounter
/// (depth-first preorder, edges in `graph_edges` order) order. Missing ids are
/// left out. This is `tir::Definition.types` and its table's order.
pub fn closure(t: &TypeInterner, roots: impl IntoIterator<Item = TypeId>) -> Arc<[TypeId]> {
    let mut seen = std::collections::HashSet::new();
    let mut order = vec![];
    let mut pending: Vec<TypeId> = roots.into_iter().collect();
    pending.reverse();
    while let Some(id) = pending.pop() {
        let Some(row) = t.get(id) else { continue };
        if !seen.insert(id) {
            continue;
        }
        order.push(id);
        pending.extend(row.kind.graph_edges().into_iter().rev().map(|e| e.ty));
    }
    order.into()
}

/// Encode the graphs of `roots` as a canonical table, and the table position
/// of each root (`u32::MAX` for an id with no row).
pub fn encode_table(t: &TypeInterner, roots: &[TypeId]) -> (TypeTable, Vec<u32>) {
    let order = closure(t, roots.iter().copied());
    let index: HashMap<TypeId, u32> = order
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i as u32))
        .collect();
    let position = |id: TypeId| index.get(&id).copied().unwrap_or(MISSING);
    let rows = order
        .iter()
        .map(|id| {
            let mut row = t.row(*id).clone();
            row.kind = row.kind.map_edges(|edge| TypeId::raw(position(edge)));
            row
        })
        .collect();
    (
        TypeTable { rows },
        roots.iter().map(|id| position(*id)).collect(),
    )
}

/// Intern `table`'s rows, returning the id of each position. Acyclic tables
/// intern bottom-up, so every row gets the id an equal row already has.
pub fn decode_table(t: &TypeInterner, table: &TypeTable) -> Vec<TypeId> {
    let n = table.rows.len();
    let edges = |i: usize| -> Vec<u32> {
        table.rows[i]
            .kind
            .graph_edges()
            .into_iter()
            .map(|e| e.ty.raw_index())
            .filter(|&p| (p as usize) < n)
            .collect()
    };
    // Postorder over positions; a back edge means an invalid cyclic graph.
    let mut state = vec![0u8; n];
    let mut order = Vec::with_capacity(n);
    let mut cyclic = false;
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        let mut stack = vec![(start, false)];
        while let Some((i, exit)) = stack.pop() {
            if exit {
                state[i] = 2;
                order.push(i);
                continue;
            }
            match state[i] {
                2 => continue,
                1 => {
                    cyclic = true;
                    continue;
                }
                _ => (),
            }
            state[i] = 1;
            stack.push((i, true));
            for p in edges(i).into_iter().rev() {
                if state[p as usize] == 1 {
                    cyclic = true;
                } else if state[p as usize] == 0 {
                    stack.push((p as usize, false));
                }
            }
        }
    }
    let resolve = |row: &Type, ids: &dyn Fn(u32) -> TypeId| {
        let mut row = row.clone();
        row.kind = row.kind.map_edges(|edge| ids(edge.raw_index()));
        row
    };
    if cyclic {
        return t.intern_graph(n, |ids| {
            table
                .rows
                .iter()
                .map(|row| {
                    resolve(row, &|p| {
                        ids.get(p as usize).copied().unwrap_or(TypeId::DANGLING)
                    })
                })
                .collect()
        });
    }
    let mut ids = vec![TypeId::DANGLING; n];
    for i in order {
        let row = resolve(&table.rows[i], &|p| {
            ids.get(p as usize).copied().unwrap_or(TypeId::DANGLING)
        });
        ids[i] = t.intern(row);
    }
    ids
}

/// How `TypeId` serializes on this thread: as a table position.
enum Scope {
    Encode(HashMap<TypeId, u32>),
    Decode(Vec<TypeId>),
    /// Table rows, whose edges already are positions.
    Raw,
}
thread_local! {
    static SCOPE: RefCell<Vec<Scope>> = const { RefCell::new(vec![]) };
}
fn scoped<R>(scope: Scope, f: impl FnOnce() -> R) -> R {
    SCOPE.with_borrow_mut(|s| s.push(scope));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            SCOPE.with_borrow_mut(|s| s.pop());
        }
    }
    let _pop = Pop;
    f()
}

/// Serialize with `TypeId`s written as positions in the table of `roots`.
/// Persisted forms (TIR, materialized values, the image) serialize their
/// TypeIds only inside such a scope, beside the table itself.
pub fn with_table_encoding<R>(t: &TypeInterner, roots: &[TypeId], f: impl FnOnce() -> R) -> R {
    let order = closure(t, roots.iter().copied());
    let index = order
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i as u32))
        .collect();
    scoped(Scope::Encode(index), f)
}

/// Deserialize with table positions read as the ids `decode_table` gave them.
pub fn with_table_decoding<R>(ids: &[TypeId], f: impl FnOnce() -> R) -> R {
    scoped(Scope::Decode(ids.to_vec()), f)
}

impl TypeId {
    /// A table position as an edge of a table row; never an interner id.
    fn raw(position: u32) -> Self {
        TypeId::from_raw(position)
    }
    fn raw_index(self) -> u32 {
        self.to_raw()
    }
}

impl serde::Serialize for TypeId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let position = SCOPE.with_borrow(|scopes| match scopes.last() {
            Some(Scope::Encode(index)) => Some(index.get(self).copied().unwrap_or(MISSING)),
            Some(Scope::Raw) => Some(self.raw_index()),
            _ => None,
        });
        match position {
            Some(position) => serializer.serialize_u32(position),
            None => Err(serde::ser::Error::custom(
                "TypeId serialized outside a type table scope",
            )),
        }
    }
}
impl<'de> serde::Deserialize<'de> for TypeId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let position = u32::deserialize(deserializer)?;
        SCOPE
            .with_borrow(|scopes| match scopes.last() {
                Some(Scope::Decode(ids)) => Some(
                    ids.get(position as usize)
                        .copied()
                        .unwrap_or(TypeId::DANGLING),
                ),
                Some(Scope::Raw) => Some(TypeId::raw(position)),
                _ => None,
            })
            .ok_or_else(|| serde::de::Error::custom("TypeId decoded outside a type table scope"))
    }
}

impl serde::Serialize for TypeTable {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        scoped(Scope::Raw, || self.rows.serialize(serializer))
    }
}
impl<'de> serde::Deserialize<'de> for TypeTable {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let rows = scoped(Scope::Raw, || Vec::<Type>::deserialize(deserializer))?;
        Ok(Self { rows })
    }
}
