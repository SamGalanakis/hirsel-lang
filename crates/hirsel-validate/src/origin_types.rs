//! Per-body immutable type-origin queries. Proof identities remain flow facts.
use hirsel_types::{GraphError, Origin, Reach, TypeId, TypeInterner};
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
    rc::Rc,
};

#[derive(Clone)]
struct Summary {
    may_hold_origin: bool,
    // The flag distinguishes relation places (arena-filtered) from brands.
    origins: Rc<[(Origin, u32, bool)]>,
}
#[derive(Default)]
pub(super) struct Types {
    // Each cache belongs to one body; rows are immutable session rows.
    summaries: RefCell<HashMap<TypeId, Summary>>,
    empty: Rc<[(Origin, u32, bool)]>,
}
impl Types {
    fn summary(&self, store: &TypeInterner, root: TypeId) -> Result<Summary, GraphError> {
        let mut summaries = self.summaries.borrow_mut();
        if let Some(summary) = summaries.get(&root) {
            return Ok(summary.clone());
        }
        let mut pending = vec![(root, false)];
        let mut active = HashSet::new();
        while let Some((id, exit)) = pending.pop() {
            if summaries.contains_key(&id) {
                continue;
            }
            let row = store.get(id).ok_or(GraphError::MissingType(id))?;
            if exit {
                let mut origins = BTreeSet::new();
                origins.extend(
                    row.brand
                        .alternatives
                        .iter()
                        .cloned()
                        .map(|o| (o, 0, false)),
                );
                origins.extend(row.modes.relation.reaches().iter().filter_map(|r| {
                    if let Reach::Place(o) = r {
                        Some((o.clone(), 0, true))
                    } else {
                        None
                    }
                }));
                for edge in row.kind.graph_edges() {
                    let child = &summaries[&edge.ty];
                    if child.may_hold_origin {
                        origins.extend(child.origins.iter().map(|(o, depth, place)| {
                            (o.clone(), depth + edge.role.parameter_binders(), *place)
                        }));
                    }
                }
                let summary = Summary {
                    may_hold_origin: !origins.is_empty(),
                    origins: if origins.is_empty() {
                        self.empty.clone()
                    } else {
                        origins.into_iter().collect::<Vec<_>>().into()
                    },
                };
                summaries.insert(id, summary);
                active.remove(&id);
            } else {
                if !active.insert(id) {
                    return Err(GraphError::StructuralCycle(id));
                }
                hirsel_intern::work::count(
                    hirsel_intern::work::Counter::ValidateOriginDependencyRows,
                );
                pending.push((id, true));
                pending.extend(
                    row.kind
                        .graph_edges()
                        .into_iter()
                        .rev()
                        .map(|e| (e.ty, false)),
                );
            }
        }
        Ok(summaries[&root].clone())
    }
    pub(super) fn visit(
        &self,
        store: &TypeInterner,
        root: TypeId,
        arena: &dyn Fn(&Origin) -> bool,
        mut visit: impl FnMut(&Origin, u32) -> Result<(), GraphError>,
    ) -> Result<(), GraphError> {
        let summary = match self.summary(store, root) {
            Ok(summary) => summary,
            // Structural validation normally excludes this. Preserve the original
            // walk's partial visitation and error policy for malformed graphs.
            Err(_) => return hirsel_types::visit_type_origins_where(store, root, arena, visit),
        };
        if !summary.may_hold_origin {
            return Ok(());
        }
        for (origin, depth, place) in summary.origins.iter() {
            hirsel_intern::work::count(hirsel_intern::work::Counter::ValidateOriginDependencyRows);
            if !place || arena(origin) {
                visit(origin, *depth)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_types::{
        Allocation, AuthoritySummary, Brand, FunctionType, OriginRoot, ParamId, ParameterRef,
        Relation, Type, TypeKind, Uses,
    };

    #[test]
    fn queries_preserve_brands_arena_filter_and_nested_binder_depths() {
        let session = hirsel_types::TypeInterner::new(None);
        let store = &*session;
        let origin = Origin {
            root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
            fields: vec![].into(),
        };
        let branded = store.intern(Type {
            brand: Brand {
                alternatives: vec![origin.clone()].into(),
            },
            ..Type::unit()
        });
        let placed = store.intern(Type {
            modes: hirsel_types::Modes {
                relation: Relation::place(origin),
                ..hirsel_types::Modes::global()
            },
            ..Type::unit()
        });
        let callable = store.intern(Type {
            kind: TypeKind::Function(FunctionType {
                state_promises: Default::default(),
                generics: vec![].into(),
                parameters: vec![].into(),
                result: branded,
                error: Some(placed),
                allocation: Allocation::NoAlloc,
                authority: AuthoritySummary::Unknown,
                uses: Uses::Unrestricted,
                contracts: vec![].into(),
                incomplete: false,
                foreign: None,
            }),
            ..Type::unit()
        });
        let root = store.intern(Type {
            kind: TypeKind::Tuple(vec![branded, placed, callable, callable].into()),
            ..Type::unit()
        });
        let cache = Types::default();
        for arena in [false, true, false] {
            let mut expected = BTreeSet::new();
            hirsel_types::visit_type_origins_where(
                store,
                root,
                |_| arena,
                |o, d| {
                    expected.insert((o.clone(), d));
                    Ok(())
                },
            )
            .unwrap();
            let mut actual = BTreeSet::new();
            cache
                .visit(store, root, &|_| arena, |o, d| {
                    actual.insert((o.clone(), d));
                    Ok(())
                })
                .unwrap();
            assert_eq!(actual, expected);
            assert!(actual.iter().any(|(_, depth)| *depth == 1));
        }
    }

    #[test]
    fn repeated_plain_subtrees_are_inspected_once_per_body() {
        use hirsel_intern::work::{self, Counter};
        let session = hirsel_types::TypeInterner::new(None);
        let store = &*session;
        let leaf = store.intern(Type::unit());
        let root = store.intern(Type {
            kind: TypeKind::Tuple(vec![leaf; 1024].into()),
            ..Type::unit()
        });
        let cache = Types::default();
        let before = work::read(Counter::ValidateOriginDependencyRows);
        for _ in 0..1024 {
            cache
                .visit(store, root, &|_| true, |_, _| {
                    panic!("plain subtree has an origin")
                })
                .unwrap();
        }
        assert_eq!(
            work::read(Counter::ValidateOriginDependencyRows) - before,
            2
        );
    }
}
