//! Rebuilding a rooted type graph with adjusted rows. Interned rows are
//! immutable, so a phase that changes rows (finished drop summaries, adjusted
//! modes) interns new rows and maps every edge to them.
use hirsel_types::{GenericArgument, Type, TypeId, TypeInterner, TypeKind};
use std::collections::HashMap;

/// Every edge of `kind`, in `graph_edges` order, mutably.
pub fn edges_mut(kind: &mut TypeKind, mut f: impl FnMut(&mut TypeId)) {
    match kind {
        TypeKind::Tuple(ts) => ts.iter_mut().for_each(f),
        TypeKind::Record(fs) => fs.iter_mut().for_each(|field| f(&mut field.ty)),
        TypeKind::Variant(vs) => vs
            .iter_mut()
            .for_each(|v| v.fields.iter_mut().for_each(|(_, t)| f(t))),
        TypeKind::Box(t) => f(t),
        TypeKind::Function(function) => {
            for p in function.parameters.iter_mut() {
                f(&mut p.ty);
            }
            f(&mut function.result);
            if let Some(t) = &mut function.error {
                f(t);
            }
        }
        TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
            for a in arguments.iter_mut() {
                if let GenericArgument::Type(t) = a {
                    f(t);
                }
            }
        }
        TypeKind::Primitive(_) | TypeKind::Parameter(_) | TypeKind::Error => {}
    }
}

/// The graph reachable from `roots` with each row replaced by `adjust(id,
/// row)` and its edges mapped to the replacements. Returns old id → new id
/// for every reachable row. Acyclic graphs intern bottom-up, so equal rows
/// keep one id; a cyclic (invalid) graph is interned as one hand-built graph.
pub fn rebuild(
    t: &TypeInterner,
    roots: impl IntoIterator<Item = TypeId>,
    mut adjust: impl FnMut(TypeId, &Type) -> Type,
) -> HashMap<TypeId, TypeId> {
    let order = hirsel_types::closure(t, roots);
    // Post-order over the closure; a back edge means a cycle.
    let mut state: HashMap<TypeId, bool> = HashMap::new();
    let mut post = Vec::with_capacity(order.len());
    let mut cyclic = false;
    for &root in order.iter() {
        if state.contains_key(&root) {
            continue;
        }
        let mut stack = vec![(root, false)];
        while let Some((id, expanded)) = stack.pop() {
            if expanded {
                state.insert(id, true);
                post.push(id);
                continue;
            }
            match state.get(&id) {
                Some(true) => continue,
                Some(false) => {
                    cyclic = true;
                    continue;
                }
                None => {}
            }
            state.insert(id, false);
            stack.push((id, true));
            let Some(row) = t.get(id) else { continue };
            for edge in row.kind.graph_edges().into_iter().rev() {
                match state.get(&edge.ty) {
                    Some(false) => cyclic = true,
                    Some(true) => {}
                    None => stack.push((edge.ty, false)),
                }
            }
        }
    }
    let mut map = HashMap::with_capacity(order.len());
    if !cyclic {
        for id in post {
            let Some(row) = t.get(id) else {
                map.insert(id, id);
                continue;
            };
            let mut row = adjust(id, row);
            edges_mut(&mut row.kind, |edge| {
                if let Some(mapped) = map.get(edge) {
                    *edge = *mapped;
                }
            });
            map.insert(id, t.intern(row));
        }
        return map;
    }
    let present: Vec<_> = order.iter().copied().filter(|id| t.contains(*id)).collect();
    let index: HashMap<_, _> = present.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let rows: Vec<Type> = present.iter().map(|id| adjust(*id, t.row(*id))).collect();
    let ids = t.intern_graph(rows.len(), |ids| {
        let mut rows = rows;
        for row in &mut rows {
            edges_mut(&mut row.kind, |edge| {
                if let Some(i) = index.get(edge) {
                    *edge = ids[*i];
                }
            });
        }
        rows
    });
    present.into_iter().zip(ids).collect()
}
