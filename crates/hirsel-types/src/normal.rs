//! Pure per-id memos over immutable interned rows: graph validity and the
//! normal forms that equality modulo names, modes or shape compares.
use crate::interner::Entry;
use crate::*;
use std::collections::HashSet;
use std::sync::OnceLock;

impl TypeKind {
    /// This kind with every `graph_edges` edge replaced, in that order.
    pub fn map_edges(&self, mut edge: impl FnMut(TypeId) -> TypeId) -> TypeKind {
        let mut kind = self.clone();
        match &mut kind {
            TypeKind::Tuple(ts) => ts.iter_mut().for_each(|t| *t = edge(*t)),
            TypeKind::Box(t) => *t = edge(*t),
            TypeKind::Record(fs) => fs.iter_mut().for_each(|f| f.ty = edge(f.ty)),
            TypeKind::Variant(vs) => vs
                .iter_mut()
                .flat_map(|v| v.fields.iter_mut())
                .for_each(|f| f.1 = edge(f.1)),
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                for argument in arguments.iter_mut() {
                    if let GenericArgument::Type(t) = argument {
                        *t = edge(*t);
                    }
                }
            }
            TypeKind::Function(f) => {
                for parameter in f.parameters.iter_mut() {
                    parameter.ty = edge(parameter.ty);
                }
                f.result = edge(f.result);
                if let Some(error) = &mut f.error {
                    *error = edge(*error);
                }
            }
            TypeKind::Primitive(_) | TypeKind::Parameter(_) | TypeKind::Error => (),
        }
        kind
    }
}

pub(crate) fn validate(t: &TypeInterner, root: TypeId) -> Result<(), GraphError> {
    let verdict = |id: TypeId| -> Option<Result<(), GraphError>> {
        match t.entry(id) {
            None => Some(Err(GraphError::MissingType(id))),
            Some(entry) => entry.valid.get().cloned(),
        }
    };
    let mut pending = vec![(root, false)];
    let mut active: Vec<TypeId> = vec![];
    let mut on_path = HashSet::new();
    while let Some((id, exit)) = pending.pop() {
        let Some(entry) = t.entry(id) else {
            if id == root {
                return Err(GraphError::MissingType(id));
            }
            continue;
        };
        if exit {
            let result = entry
                .row
                .kind
                .graph_edges()
                .into_iter()
                .map(|e| verdict(e.ty).expect("children are decided before their parent"))
                .find(Result::is_err)
                .unwrap_or(Ok(()));
            let _ = entry.valid.set(result);
            t.count_valid();
            on_path.remove(&id);
            active.pop();
            continue;
        }
        if entry.valid.get().is_some() {
            continue;
        }
        if on_path.contains(&id) {
            let error = Err(GraphError::StructuralCycle(id));
            // Every row on the path reaches the cycle.
            for id in &active {
                if let Some(entry) = t.entry(*id) {
                    let _ = entry.valid.set(error.clone());
                }
            }
            return error;
        }
        if matches!(entry.row.kind, TypeKind::Error) {
            let _ = entry.valid.set(Err(GraphError::RecoveryType(id)));
            t.count_valid();
            continue;
        }
        on_path.insert(id);
        active.push(id);
        pending.push((id, true));
        pending.extend(
            entry
                .row
                .kind
                .graph_edges()
                .into_iter()
                .rev()
                .map(|edge| (edge.ty, false)),
        );
    }
    verdict(root).expect("the root is decided")
}

/// Map a valid graph bottom-up: each row's normal form is `local` of the row
/// with its edges replaced by their children's normal forms. Memoized in the
/// slot `memo` selects; a normal-form row is its own normal form.
fn normalize(
    t: &TypeInterner,
    root: TypeId,
    memo: fn(&Entry) -> &OnceLock<TypeId>,
    local: fn(&Type) -> Type,
) -> TypeId {
    if let Some(id) = t.entry(root).and_then(|e| memo(e).get()) {
        t.count_sem(true);
        return *id;
    }
    if !t.valid(root) {
        return root;
    }
    let mut pending = vec![(root, false)];
    while let Some((id, exit)) = pending.pop() {
        let entry = t.entry(id).expect("a valid graph's rows exist");
        if memo(entry).get().is_some() {
            continue;
        }
        if !exit {
            pending.push((id, true));
            pending.extend(
                entry
                    .row
                    .kind
                    .graph_edges()
                    .into_iter()
                    .rev()
                    .map(|edge| (edge.ty, false)),
            );
            continue;
        }
        let mut row = local(&entry.row);
        row.kind = row.kind.map_edges(|child| {
            *memo(t.entry(child).expect("a valid graph's rows exist"))
                .get()
                .expect("children are normalized before their parent")
        });
        let normal = t.intern(row);
        t.count_sem(false);
        let _ = memo(entry).set(normal);
        if let Some(entry) = t.entry(normal) {
            let _ = memo(entry).set(normal);
        }
    }
    *memo(t.entry(root).expect("the root exists"))
        .get()
        .expect("the root is normalized")
}

/// The row facts semantic equality ignores, erased from one row.
pub(crate) fn semantic_row(row: &Type) -> Type {
    let mut row = row.clone();
    if let Some(ErasedCaptures::FromArguments(ps)) = &mut row.erased_captures {
        ps.sort();
        ps.dedup();
    }
    if row.erased_captures == Some(ErasedCaptures::Known(AuthoritySummary::Unknown)) {
        row.erased_captures = None;
    }
    if let TypeKind::Function(f) = &mut row.kind {
        for parameter in f.parameters.iter_mut() {
            parameter.name = None;
        }
    }
    row
}

fn modeless_row(row: &Type) -> Type {
    let mut row = semantic_row(row);
    row.modes = Modes::global();
    row.sendability_requirement = SendabilityRequirement::None;
    row.brand = Brand::default();
    row
}

fn shape_row(row: &Type) -> Type {
    let kind = match &row.kind {
        TypeKind::Function(f) => TypeKind::Function(FunctionType {
            state_promises: SharedSlice::default(),
            generics: f.generics.clone(),
            parameters: f
                .parameters
                .iter()
                .map(|p| Parameter {
                    name: None,
                    ..p.clone()
                })
                .collect(),
            result: f.result,
            error: f.error,
            allocation: Allocation::MayAllocate,
            authority: AuthoritySummary::Unknown,
            uses: Uses::Unrestricted,
            contracts: SharedSlice::default(),
            incomplete: false,
            foreign: None,
        }),
        kind => kind.clone(),
    };
    Type::with_kind(kind)
}

pub(crate) fn sem(t: &TypeInterner, id: TypeId) -> TypeId {
    normalize(t, id, |e| &e.sem, semantic_row)
}
pub(crate) fn strip_modes(t: &TypeInterner, id: TypeId) -> TypeId {
    normalize(t, id, |e| &e.strip_modes, modeless_row)
}
pub(crate) fn shape(t: &TypeInterner, id: TypeId) -> TypeId {
    normalize(t, id, |e| &e.shape, shape_row)
}
