//! Pure per-id memos over immutable interned rows: graph validity and the
//! normal forms that equality modulo names, modes or shape compares.
use crate::interner::Entry;
use crate::*;
use std::collections::HashSet;
use std::sync::OnceLock;

impl TypeKind {
    /// This kind with every `graph_edges` edge replaced, in that order.
    pub fn map_edges(&self, mut edge: impl FnMut(TypeId) -> TypeId) -> TypeKind {
        self.try_map_edges(|id| Ok::<_, std::convert::Infallible>(edge(id)))
            .unwrap()
    }

    /// Keep shared child arrays until an edge actually changes. Most modes
    /// rewrites only touch qualifiers, or find the row already normalized.
    pub(crate) fn try_map_edges<E>(
        &self,
        mut edge: impl FnMut(TypeId) -> Result<TypeId, E>,
    ) -> Result<TypeKind, E> {
        let mut kind = self.clone();
        match (self, &mut kind) {
            (Self::Tuple(old), Self::Tuple(ts)) => {
                for (i, &id) in old.iter().enumerate() {
                    let new = edge(id)?;
                    if new != id {
                        ts[i] = new;
                    }
                }
            }
            (Self::Box(_), Self::Box(t)) => *t = edge(*t)?,
            (Self::Record(old), Self::Record(fs)) => {
                for (i, field) in old.iter().enumerate() {
                    let new = edge(field.ty)?;
                    if new != field.ty {
                        fs[i].ty = new;
                    }
                }
            }
            (Self::Variant(old), Self::Variant(vs)) => {
                for (i, variant) in old.iter().enumerate() {
                    for (j, &(_, id)) in variant.fields.iter().enumerate() {
                        let new = edge(id)?;
                        if new != id {
                            vs[i].fields[j].1 = new;
                        }
                    }
                }
            }
            (
                Self::Named { arguments: old, .. } | Self::Builtin { arguments: old, .. },
                Self::Named { arguments, .. } | Self::Builtin { arguments, .. },
            ) => {
                for (i, argument) in old.iter().enumerate() {
                    if let GenericArgument::Type(id) = argument {
                        let new = edge(*id)?;
                        if new != *id {
                            arguments[i] = GenericArgument::Type(new);
                        }
                    }
                }
            }
            (Self::Function(old), Self::Function(f)) => {
                for (i, parameter) in old.parameters.iter().enumerate() {
                    let new = edge(parameter.ty)?;
                    if new != parameter.ty {
                        f.parameters[i].ty = new;
                    }
                }
                f.result = edge(f.result)?;
                if let Some(error) = &mut f.error {
                    *error = edge(*error)?;
                }
            }
            _ => (),
        }
        Ok(kind)
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
/// slot `memo` selects; a normal-form row is its own normal form. `sem`
/// selects the semantic form, the one `InternerStats` counts.
fn normalize(
    t: &TypeInterner,
    root: TypeId,
    memo: fn(&Entry) -> &OnceLock<TypeId>,
    local: fn(&Type) -> Type,
    sem: bool,
) -> TypeId {
    if let Some(id) = t.entry(root).and_then(|e| memo(e).get()) {
        if sem {
            t.count_sem(true);
        }
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
        if sem {
            t.count_sem(false);
        }
        hirsel_intern::work::count(hirsel_intern::work::Counter::TypeNormalRows);
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
    normalize(t, id, |e| &e.sem, semantic_row, true)
}
pub(crate) fn strip_modes(t: &TypeInterner, id: TypeId) -> TypeId {
    normalize(t, id, |e| &e.strip_modes, modeless_row, false)
}
pub(crate) fn shape(t: &TypeInterner, id: TypeId) -> TypeId {
    normalize(t, id, |e| &e.shape, shape_row, false)
}
/// The receiving-contract normal form: the semantic form without the current
/// sendability summary of rows whose contract does not state it. A directional
/// comparison compares exact nested rows in this form, as main's walk did by
/// giving each such actual row its expected row's summary.
pub(crate) fn contract(t: &TypeInterner, id: TypeId) -> TypeId {
    normalize(t, id, |e| &e.contract, contract_row, false)
}
fn contract_row(row: &Type) -> Type {
    let mut row = semantic_row(row);
    let stated = matches!(
        row.kind,
        TypeKind::Function(_)
            | TypeKind::Builtin {
                kind: BuiltinType::Iter,
                ..
            }
    ) && row.sendability_requirement != SendabilityRequirement::Sendable;
    if !stated {
        row.modes.sendability = Sendability::Sendable;
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_rewrites_preserve_unchanged_children_and_callback_order() {
        let t = TypeInterner::new(None);
        let a = t.intern(Type::with_kind(TypeKind::Primitive(Primitive::U8)));
        let b = t.intern(Type::with_kind(TypeKind::Primitive(Primitive::U16)));
        let original = TypeKind::Tuple([a, b, a].into());
        let mut visited = vec![];
        let unchanged = original.map_edges(|id| {
            visited.push(id);
            id
        });
        assert_eq!(visited, [a, b, a]);
        let (TypeKind::Tuple(before), TypeKind::Tuple(after)) = (&original, &unchanged) else {
            unreachable!()
        };
        assert_eq!(before.storage_identity(), after.storage_identity());
        let changed = original.map_edges(|id| if id == a { b } else { id });
        let TypeKind::Tuple(children) = changed else {
            unreachable!()
        };
        assert_eq!(children.as_slice(), [b, b, b]);
        assert_eq!(before.as_slice(), [a, b, a]);
        assert_ne!(before.storage_identity(), children.storage_identity());
        visited.clear();
        assert_eq!(
            original.try_map_edges(|id| {
                visited.push(id);
                if id == b { Err("stop") } else { Ok(id) }
            }),
            Err("stop")
        );
        assert_eq!(visited, [a, b]);
    }
}
