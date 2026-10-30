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
    /// Every encoded edge names a row of this canonical table. Validate before
    /// interning untrusted front-end records; image/V decoders may deliberately
    /// retain dangling edges to diagnose them instead.
    pub fn valid_indices(&self) -> bool {
        self.valid_positions(self.rows.len())
    }
    /// Component rows can also name a separately resolved external prefix.
    pub fn valid_positions(&self, count: usize) -> bool {
        self.rows.iter().all(|row| {
            row.kind
                .graph_edges()
                .iter()
                .all(|edge| (edge.ty.raw_index() as usize) < count)
        })
    }
    pub fn edge_positions(&self) -> Vec<Vec<u32>> {
        self.rows
            .iter()
            .map(|row| {
                row.kind
                    .graph_edges()
                    .iter()
                    .map(|edge| edge.ty.raw_index())
                    .collect()
            })
            .collect()
    }
    /// Resolve checked component positions without decoding its rows again.
    pub fn resolve_positions(&self, ids: &[TypeId]) -> Option<Vec<Type>> {
        if !self.valid_positions(ids.len()) {
            return None;
        }
        Some(
            self.rows
                .iter()
                .map(|row| {
                    let mut row = row.clone();
                    row.kind = row.kind.map_edges(|edge| ids[edge.raw_index() as usize]);
                    row
                })
                .collect(),
        )
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

/// Intern `table`'s rows, returning the id of each position. Rows intern
/// bottom-up, so every row gets the id an equal row already has; only rows on
/// or above a cycle (an invalid graph) are interned together as a graph.
pub fn decode_table(t: &TypeInterner, table: &TypeTable) -> Vec<TypeId> {
    let n = table.rows.len();
    let edges = |i: usize| -> Vec<usize> {
        table.rows[i]
            .kind
            .graph_edges()
            .into_iter()
            .map(|e| e.ty.raw_index() as usize)
            .filter(|&p| p < n)
            .collect()
    };
    // Postorder over positions, marking rows on a cycle (a back edge closes
    // the path from its target) and then every row reaching one.
    let mut state = vec![0u8; n];
    let mut cyclic = vec![false; n];
    let mut order = Vec::with_capacity(n);
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        let mut path: Vec<usize> = vec![];
        let mut stack = vec![(start, false)];
        while let Some((i, exit)) = stack.pop() {
            if exit {
                state[i] = 2;
                path.pop();
                cyclic[i] |= edges(i).into_iter().any(|p| cyclic[p]);
                order.push(i);
                continue;
            }
            if state[i] != 0 {
                continue;
            }
            state[i] = 1;
            path.push(i);
            stack.push((i, true));
            for p in edges(i).into_iter().rev() {
                match state[p] {
                    0 => stack.push((p, false)),
                    1 => {
                        let from = path.iter().rposition(|&q| q == p).expect("on the path");
                        for &q in &path[from..] {
                            cyclic[q] = true;
                        }
                    }
                    _ => (),
                }
            }
        }
    }
    let resolve = |row: &Type, ids: &dyn Fn(usize) -> TypeId| {
        let mut row = row.clone();
        row.kind = row.kind.map_edges(|edge| ids(edge.raw_index() as usize));
        row
    };
    let mut ids = vec![TypeId::DANGLING; n];
    for &i in order.iter().filter(|&&i| !cyclic[i]) {
        let row = resolve(&table.rows[i], &|p| {
            ids.get(p).copied().unwrap_or(TypeId::DANGLING)
        });
        ids[i] = t.intern(row);
    }
    let graph: Vec<usize> = (0..n).filter(|&i| cyclic[i]).collect();
    if !graph.is_empty() {
        let slot: std::collections::HashMap<usize, usize> =
            graph.iter().enumerate().map(|(k, &i)| (i, k)).collect();
        let interned = t.intern_graph(graph.len(), |graph_ids| {
            graph
                .iter()
                .map(|&i| {
                    resolve(&table.rows[i], &|p| match slot.get(&p) {
                        Some(&k) => graph_ids[k],
                        None => ids.get(p).copied().unwrap_or(TypeId::DANGLING),
                    })
                })
                .collect()
        });
        for (k, &i) in graph.iter().enumerate() {
            ids[i] = interned[k];
        }
    }
    ids
}

impl TypeBase {
    /// The immutable base holding a canonical table's rows (the image's std
    /// rows), and the base id of each table position.
    pub fn from_table(table: &TypeTable) -> (Arc<TypeBase>, Vec<TypeId>) {
        let mut assigned = vec![];
        let base = TypeBase::build(table.rows.len(), |ids| {
            assigned = ids.to_vec();
            table
                .rows
                .iter()
                .map(|row| {
                    let mut row = row.clone();
                    row.kind = row.kind.map_edges(|edge| {
                        ids.get(edge.raw_index() as usize)
                            .copied()
                            .unwrap_or(TypeId::DANGLING)
                    });
                    row
                })
                .collect()
        });
        (base, assigned)
    }
}

/// A canonical table that grows as values serialize (the std image's one
/// table): each id gets the next position the first time a serialization
/// meets it, and `finish` adds every row reachable from those. A
/// deterministic serialization order numbers rows the same in every session.
pub struct TableBuilder {
    types: Arc<TypeInterner>,
    index: HashMap<TypeId, u32>,
    order: Vec<TypeId>,
}
impl TableBuilder {
    pub fn new(types: Arc<TypeInterner>) -> Self {
        Self {
            types,
            index: HashMap::new(),
            order: vec![],
        }
    }
    /// A builder whose first positions are `base`'s rows in base order, so a
    /// base built from its table gives those rows the ids `base` gives them
    /// and values naming them stay valid over either base.
    pub fn over(types: Arc<TypeInterner>, base: &TypeBase) -> Self {
        let mut builder = Self::new(types);
        for position in 0..base.len() as u32 {
            builder.position(TypeBase::id(position));
        }
        builder
    }
    fn position(&mut self, id: TypeId) -> u32 {
        if !self.types.contains(id) {
            return MISSING;
        }
        let next = self.order.len() as u32;
        *self.index.entry(id).or_insert_with(|| {
            self.order.push(id);
            next
        })
    }
    /// Serialize in `f` with each `TypeId` written as its position here.
    pub fn with_encoding<R>(&mut self, f: impl FnOnce() -> R) -> R {
        let builder = std::mem::replace(self, Self::new(self.types.clone()));
        SCOPE.with_borrow_mut(|s| s.push(Scope::Grow(builder)));
        struct Restore<'a>(&'a mut TableBuilder);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                if let Some(Scope::Grow(builder)) = SCOPE.with_borrow_mut(|s| s.pop()) {
                    *self.0 = builder;
                }
            }
        }
        let _restore = Restore(self);
        f()
    }
    /// How many rows have a position.
    pub fn len(&self) -> usize {
        self.order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    /// Direct ids named by the encoded value, before traversing their rows.
    pub fn roots(&self) -> &[TypeId] {
        &self.order
    }
    /// The table: every positioned row, then the rows they reach, each in
    /// first-encounter order, with edges written as positions.
    pub fn finish(mut self) -> TypeTable {
        let mut rows = Vec::with_capacity(self.order.len());
        let mut next = 0;
        while next < self.order.len() {
            let mut row = self.types.row(self.order[next]).clone();
            row.kind = row.kind.map_edges(|edge| TypeId::raw(self.position(edge)));
            rows.push(row);
            next += 1;
        }
        TypeTable { rows }
    }
}

impl TypeTable {
    /// The immutable base holding this table's rows: position p becomes base
    /// row p, so values decoded under `with_base_decoding` name them.
    pub fn into_base(self) -> Arc<TypeBase> {
        let len = self.rows.len();
        let mut rows = Some(self.rows);
        TypeBase::build(len, |ids| {
            let mut rows = rows.take().expect("built once");
            for row in &mut rows {
                row.kind = row.kind.map_edges(|edge| {
                    ids.get(edge.raw_index() as usize)
                        .copied()
                        .unwrap_or(TypeId::DANGLING)
                });
            }
            rows
        })
    }
}

/// Deserialize in `f` with each table position read as the base row it is
/// (`TypeTable::into_base`) in a base of `len` rows.
pub fn with_base_decoding<R>(len: usize, f: impl FnOnce() -> R) -> R {
    scoped(Scope::Base(len as u32), f)
}

/// Serialize or deserialize in `f` with each `TypeId` written as the id it
/// is: a form only the session that wrote it reads back (a spill file).
pub fn with_session_ids<R>(f: impl FnOnce() -> R) -> R {
    scoped(Scope::Raw, f)
}

/// How `TypeId` serializes on this thread: as a table position.
enum Scope {
    Encode(HashMap<TypeId, u32>),
    Grow(TableBuilder),
    Decode(Vec<TypeId>),
    Source(Arc<dyn hirsel_intern::persist::Source<TypeId>>),
    /// Positions of a base built by `TypeTable::into_base`.
    Base(u32),
    /// Table rows, whose edges already are positions.
    Raw,
    /// Every id serialized, in order, each written as 0.
    Collect {
        ids: Vec<TypeId>,
        limit: usize,
        refused: bool,
    },
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

/// Serialize while recording every `TypeId` written (each as 0): the roots a
/// persisted form whose ids are not otherwise enumerable encodes its table of.
pub fn with_table_collection<R>(f: impl FnOnce() -> R) -> (R, Vec<TypeId>) {
    with_bounded_table_collection(usize::MAX, f).expect("unbounded type collection")
}

pub fn collection_scope_working_bytes(additional: usize) -> Option<u64> {
    SCOPE.with_borrow(|scopes| {
        let needed = scopes.len().checked_add(additional)?;
        let mut capacity = scopes.capacity();
        if needed <= capacity {
            return Some(0);
        }
        while capacity < needed {
            capacity = capacity.checked_mul(2)?.max(4);
        }
        u64::try_from(capacity.checked_mul(size_of::<Scope>())?.checked_mul(2)?).ok()
    })
}

pub fn with_bounded_table_collection<R>(
    limit: usize,
    f: impl FnOnce() -> R,
) -> Option<(R, Vec<TypeId>)> {
    SCOPE.with_borrow_mut(|s| {
        s.push(Scope::Collect {
            ids: vec![],
            limit,
            refused: false,
        })
    });
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            SCOPE.with_borrow_mut(|s| s.pop());
        }
    }
    let pop = Pop;
    let result = f();
    let ids = SCOPE.with_borrow_mut(|s| match s.last_mut() {
        Some(Scope::Collect { ids, refused, .. }) => (!*refused).then(|| std::mem::take(ids)),
        _ => unreachable!("the collection scope is innermost"),
    });
    drop(pop);
    Some((result, ids?))
}

/// Deserialize with table positions read as the ids `decode_table` gave them.
pub fn with_table_decoding<R>(ids: &[TypeId], f: impl FnOnce() -> R) -> R {
    scoped(Scope::Decode(ids.to_vec()), f)
}

/// Encode a component with explicitly numbered local and outgoing edges.
pub fn with_type_positions<R>(positions: HashMap<TypeId, u32>, f: impl FnOnce() -> R) -> R {
    scoped(Scope::Encode(positions), f)
}

/// Resolve only type positions actually consumed by the semantic decoder.
pub fn with_type_source<R>(
    source: Arc<dyn hirsel_intern::persist::Source<TypeId>>,
    f: impl FnOnce() -> R,
) -> R {
    scoped(Scope::Source(source), f)
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
        let mut exhausted = false;
        let position = SCOPE.with_borrow_mut(|scopes| match scopes.last_mut() {
            Some(Scope::Encode(index)) => Some(index.get(self).copied().unwrap_or(MISSING)),
            Some(Scope::Grow(builder)) => Some(builder.position(*self)),
            Some(Scope::Raw) => Some(self.raw_index()),
            Some(Scope::Collect {
                ids,
                limit,
                refused,
            }) => {
                if ids.len() == *limit {
                    *refused = true;
                    exhausted = true;
                    None
                } else {
                    ids.push(*self);
                    Some(0)
                }
            }
            _ => None,
        });
        if exhausted {
            return Err(serde::ser::Error::custom(
                "type collection admission exhausted",
            ));
        }
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
        let source = SCOPE.with_borrow(|scopes| match scopes.last() {
            Some(Scope::Source(source)) => Some(source.clone()),
            _ => None,
        });
        if let Some(source) = source {
            return source
                .id(position)
                .ok_or_else(|| serde::de::Error::custom("missing type product"));
        }
        SCOPE
            .with_borrow(|scopes| match scopes.last() {
                Some(Scope::Decode(ids)) => Some(
                    ids.get(position as usize)
                        .copied()
                        .unwrap_or(TypeId::DANGLING),
                ),
                Some(Scope::Base(len)) => Some(if position < *len {
                    TypeBase::id(position)
                } else {
                    TypeId::DANGLING
                }),
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
