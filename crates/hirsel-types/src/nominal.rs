//! Declaration-path memoization for finite instantiated nominal graphs.
use crate::*;

/// Operational budget for strictly growing polymorphic recursion. Ordinary
/// graph width, depth and regular recursive cycles do not consume this budget.
pub const MAX_POLYMORPHIC_EXPANSIONS: usize = 128;

pub fn type_proof_limit() -> QueryError {
    QueryError::Operational(OperationalFailure {
        operation: "type-proof-tooling-limit".into(),
        message: format!(
            "non-regular polymorphic recursion exceeded {MAX_POLYMORPHIC_EXPANSIONS} growing declaration expansions"
        ),
    })
}

// A constructor header with type edges erased. Sizes and declaration identity
// remain exact; a generic argument permutation is not constructor growth.
fn head(mut kind: TypeKind, marker: TypeId) -> TypeKind {
    match &mut kind {
        TypeKind::Tuple(ts) => ts.iter_mut().for_each(|t| *t = marker),
        TypeKind::Record(fs) => fs.iter_mut().for_each(|f| f.ty = marker),
        TypeKind::Variant(vs) => vs
            .iter_mut()
            .for_each(|v| v.fields.iter_mut().for_each(|f| f.1 = marker)),
        TypeKind::Box(t) => *t = marker,
        TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
            arguments.iter_mut().for_each(|a| {
                if let GenericArgument::Type(t) = a {
                    *t = marker;
                }
            })
        }
        TypeKind::Function(f) => {
            f.parameters.iter_mut().for_each(|p| p.ty = marker);
            f.result = marker;
            if f.error.is_some() {
                f.error = Some(marker);
            }
        }
        _ => (),
    }
    kind
}
// Homeomorphic embedding, using a finite pair worklist instead of the host
// stack. Some(true) includes a strict constructor insertion; None cannot embed.
fn height(store: &TypeStore, root: TypeId) -> usize {
    let mut heights = std::collections::BTreeMap::new();
    let mut pending = vec![(root, false)];
    while let Some((t, exit)) = pending.pop() {
        if heights.contains_key(&t) {
            continue;
        }
        let children = store
            .get(t)
            .map(|r| r.kind.graph_edges())
            .unwrap_or_default();
        if exit {
            heights.insert(
                t,
                1 + children.iter().map(|c| heights[&c.ty]).max().unwrap_or(0),
            );
        } else {
            pending.push((t, true));
            pending.extend(children.iter().map(|c| (c.ty, false)));
        }
    }
    heights[&root]
}
fn embedding(older: &TypeStore, old: TypeId, newer: &TypeStore, new: TypeId) -> Option<bool> {
    // A shrinking actual cannot embed the older argument. This also keeps
    // finite nested generic aliases out of the pair-product worklist.
    if height(older, old) > height(newer, new) {
        return None;
    }
    let mut memo = std::collections::BTreeMap::new();
    let mut pending = vec![(old, new, false)];
    while let Some((a, b, exit)) = pending.pop() {
        if memo.contains_key(&(a, b)) {
            continue;
        }
        if semantic_equal(older, a, newer, b).unwrap_or(false) {
            memo.insert((a, b), Some(false));
            continue;
        }
        let (Some(ar), Some(br)) = (older.get(a), newer.get(b)) else {
            memo.insert((a, b), None);
            continue;
        };
        let ac = ar.kind.graph_edges();
        let bc = br.kind.graph_edges();
        let coupled = head(ar.kind.clone(), old) == head(br.kind.clone(), old);
        if !exit {
            pending.push((a, b, true));
            pending.extend(bc.iter().map(|c| (a, c.ty, false)));
            if coupled {
                pending.extend(ac.iter().zip(&bc).map(|(a, b)| (a.ty, b.ty, false)));
            }
            continue;
        }
        let children = coupled
            .then(|| {
                ac.iter()
                    .zip(&bc)
                    .map(|(a, b)| memo[&(a.ty, b.ty)])
                    .collect::<Option<Vec<_>>>()
            })
            .flatten();
        let result = if let Some(children) = children {
            Some(children.into_iter().any(|grew| grew))
        } else if bc.iter().any(|child| memo[&(a, child.ty)].is_some()) {
            Some(true)
        } else {
            None
        };
        memo.insert((a, b), result);
    }
    memo[&(old, new)]
}

/// All old arguments embed in the new arguments of the same declaration, with
/// at least one strict constructor insertion. This includes growth inside a
/// fixed outer constructor. Sizes cannot grow (LANG-7). Shrinking finite actuals
/// and regular cycles/permutations spend no polymorphic expansion budget.
pub fn nominal_arguments_grow(
    older: &TypeStore,
    old: TypeId,
    newer: &TypeStore,
    new: TypeId,
) -> bool {
    let (
        Some(Type {
            kind:
                TypeKind::Named {
                    definition: a,
                    arguments: aa,
                },
            ..
        }),
        Some(Type {
            kind:
                TypeKind::Named {
                    definition: b,
                    arguments: ba,
                },
            ..
        }),
    ) = (older.get(old), newer.get(new))
    else {
        return false;
    };
    if a != b || aa.len() != ba.len() {
        return false;
    }
    let mut grew = false;
    for (a, b) in aa.iter().zip(ba.iter()) {
        match (a, b) {
            (GenericArgument::Size(a), GenericArgument::Size(b)) if a == b => (),
            (GenericArgument::Type(a), GenericArgument::Type(b)) => {
                match embedding(older, *a, newer, *b) {
                    Some(growth) => grew |= growth,
                    None => return false,
                }
            }
            _ => return false,
        }
    }
    grew
}

/// A proof's finite instantiation memo and active declaration path. Context
/// distinguishes inherited uniqueness or ownership state when the property
/// depends on it. Equality and growth inspect the caller's rooted scratch.
#[derive(Default)]
pub struct NominalExpansions {
    memo: Vec<(TypeId, usize)>,
    active: Vec<(TypeId, usize, bool)>,
    growing: usize,
}
impl NominalExpansions {
    /// False assumes an already visited instantiation coinductively. The proof
    /// must still check every other owned component before publishing success.
    pub fn enter(
        &mut self,
        root: TypeId,
        context: usize,
        mut equal: impl FnMut(TypeId, TypeId) -> bool,
        mut grows: impl FnMut(TypeId, TypeId) -> bool,
    ) -> Result<bool, QueryError> {
        if self
            .memo
            .iter()
            .any(|(t, c)| *c == context && equal(*t, root))
        {
            return Ok(false);
        }
        let growth = self.active.iter().rev().any(|(t, _, _)| grows(*t, root));
        if growth && self.growing == MAX_POLYMORPHIC_EXPANSIONS {
            return Err(type_proof_limit());
        }
        self.growing += usize::from(growth);
        self.memo.push((root, context));
        self.active.push((root, context, growth));
        Ok(true)
    }
    pub fn leave(&mut self, root: TypeId, context: usize) {
        let (t, c, growth) = self.active.pop().expect("nominal proof exit without entry");
        assert_eq!((t, c), (root, context));
        self.growing -= usize::from(growth);
    }
}

/// Why a representation walk stopped before reaching a structural row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepresentationFailure {
    MissingType,
    MissingDeclaration,
    NotTypeDeclaration,
    MissingRepresentation,
    RecursiveRepresentation,
    Graph(GraphError),
    Query(QueryError),
}

/// The last row reached, plus the exact failure. A caller may retain that row
/// conservatively, but operational query failures must still propagate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepresentationError {
    pub root: TypeId,
    pub failure: RepresentationFailure,
}

/// Expand successive nominal representations in caller-owned storage.
/// `steps: Some(1)` performs one substitution; `None` reaches a structural row.
/// Zero steps returns the input without a declaration read. One phase-owned
/// callback supplies the declaration and actual argument authorities, so both
/// can use the same mutable query context without competing borrows. It receives
/// the scratch store rather than requiring a store clone. This preserves J's
/// evaluated authority and physical layout walks' conservative Unknown authority.
/// The guard spends its budget only on active polymorphic constructor growth.
pub fn expand_representation(
    store: &mut TypeStore,
    mut root: TypeId,
    lookup: &mut impl FnMut(
        &TypeStore,
        &DefPath,
        &[GenericArgument],
    ) -> Result<Option<(SignatureRef, Vec<AuthoritySummary>)>, QueryError>,
    steps: Option<usize>,
) -> Result<TypeId, RepresentationError> {
    let mut expansions = NominalExpansions::default();
    let mut remaining = steps;
    loop {
        let error = |failure| RepresentationError { root, failure };
        let row = store
            .get(root)
            .ok_or_else(|| error(RepresentationFailure::MissingType))?;
        if remaining == Some(0) {
            return Ok(root);
        }
        let TypeKind::Named {
            definition,
            arguments,
        } = &row.kind
        else {
            return Ok(root);
        };
        let definition = definition.clone();
        let arguments = arguments.clone();
        if !expansions
            .enter(
                root,
                0,
                |a, b| semantic_equal(store, a, store, b).unwrap_or(false),
                |a, b| nominal_arguments_grow(store, a, store, b),
            )
            .map_err(|e| error(RepresentationFailure::Query(e)))?
        {
            return Err(error(RepresentationFailure::RecursiveRepresentation));
        }
        let (signature, authorities) = lookup(store, &definition, &arguments)
            .map_err(|e| error(RepresentationFailure::Query(e)))?
            .ok_or_else(|| error(RepresentationFailure::MissingDeclaration))?;
        let SignatureKind::Type(declaration) = &signature.kind else {
            return Err(error(RepresentationFailure::NotTypeDeclaration));
        };
        let representation = declaration
            .representation
            .ok_or_else(|| error(RepresentationFailure::MissingRepresentation))?;
        root = substitute_type(
            &signature.types,
            representation,
            store,
            Substitution {
                binder: Some(GenericBinder::Bound(0)),
                arguments: &arguments,
                authorities: &authorities,
                roles: &signature.metadata.roles,
            },
        )
        .map_err(|e| error(RepresentationFailure::Graph(e)))?;
        remaining = remaining.map(|n| n - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarations::tests::{nominal, path, row};

    fn with_unknown_authorities(
        signature: Option<SignatureRef>,
        arguments: &[GenericArgument],
    ) -> Option<(SignatureRef, Vec<AuthoritySummary>)> {
        signature.map(|signature| (signature, vec![AuthoritySummary::Unknown; arguments.len()]))
    }

    #[test]
    fn representation_steps_preserve_nested_actuals_without_copying_the_store() {
        let mut declaration = TypeStore::default();
        let parameter = declaration.push(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let signature = nominal(declaration, Some(parameter), vec![]);
        let mut store = TypeStore::default();
        let scalar = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let mut root = scalar;
        for _ in 0..200 {
            root = store.push(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(root)].into(),
            }));
        }
        let before = hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows);
        assert_eq!(
            expand_representation(
                &mut store,
                root,
                &mut |_, _, _| panic!("zero-step lookup"),
                Some(0)
            ),
            Ok(root)
        );
        let mut reads = 0;
        let mut lookup = |_: &TypeStore, _: &DefPath, arguments: &[GenericArgument]| {
            reads += 1;
            Ok(with_unknown_authorities(Some(signature.clone()), arguments))
        };
        let one = expand_representation(&mut store, root, &mut lookup, Some(1)).unwrap();
        assert!(matches!(
            store.get(one).unwrap().kind,
            TypeKind::Named { .. }
        ));
        let full = expand_representation(&mut store, one, &mut lookup, None).unwrap();
        assert_eq!(
            store.get(full).unwrap().kind,
            store.get(scalar).unwrap().kind
        );
        assert_eq!(reads, 200);
        assert_eq!(
            hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows),
            before
        );
    }

    #[test]
    fn representation_stops_keep_the_last_row_and_exact_failure() {
        let mut store = TypeStore::default();
        let root = store.push(row(TypeKind::Named {
            definition: path("Wrap"),
            arguments: vec![].into(),
        }));
        let opaque = nominal(TypeStore::default(), None, vec![]);
        for (declaration, failure) in [
            (None, RepresentationFailure::MissingDeclaration),
            (Some(opaque), RepresentationFailure::MissingRepresentation),
        ] {
            assert_eq!(
                expand_representation(
                    &mut store,
                    root,
                    &mut |_, _, arguments| Ok(with_unknown_authorities(
                        declaration.clone(),
                        arguments
                    )),
                    None
                ),
                Err(RepresentationError { root, failure })
            );
        }
        assert_eq!(
            expand_representation(
                &mut store,
                root,
                &mut |_, _, _| Err(type_proof_limit()),
                None
            ),
            Err(RepresentationError {
                root,
                failure: RepresentationFailure::Query(type_proof_limit())
            })
        );
        let mut declaration = TypeStore::default();
        let parameter = declaration.push(row(TypeKind::Parameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))));
        let malformed = nominal(declaration, Some(parameter), vec![]);
        assert_eq!(
            expand_representation(
                &mut store,
                root,
                &mut |_, _, arguments| Ok(with_unknown_authorities(
                    Some(malformed.clone()),
                    arguments
                )),
                None
            ),
            Err(RepresentationError {
                root,
                failure: RepresentationFailure::Graph(GraphError::MissingArgument(0))
            })
        );
        // The accepted twin supplies the missing argument.
        let scalar = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let supplied = store.push(row(TypeKind::Named {
            definition: path("Wrap"),
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }));
        assert!(
            expand_representation(
                &mut store,
                supplied,
                &mut |_, _, arguments| Ok(with_unknown_authorities(
                    Some(malformed.clone()),
                    arguments
                )),
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn representation_cycles_stop_and_polymorphic_growth_remains_operational() {
        for growing in [false, true] {
            let mut declaration = TypeStore::default();
            let parameter = declaration.push(row(TypeKind::Parameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))));
            let argument = if growing {
                declaration.push(row(TypeKind::Box(parameter)))
            } else {
                parameter
            };
            let next = declaration.push(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(argument)].into(),
            }));
            let signature = nominal(declaration, Some(next), vec![]);
            let mut store = TypeStore::default();
            let scalar = store.push(row(TypeKind::Primitive(Primitive::U8)));
            let root = store.push(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(scalar)].into(),
            }));
            // A single step is useful even for a recursive nominal declaration.
            let one = expand_representation(
                &mut store,
                root,
                &mut |_, _, arguments| {
                    Ok(with_unknown_authorities(Some(signature.clone()), arguments))
                },
                Some(1),
            )
            .unwrap();
            let error = expand_representation(
                &mut store,
                one,
                &mut |_, _, arguments| {
                    Ok(with_unknown_authorities(Some(signature.clone()), arguments))
                },
                None,
            )
            .unwrap_err();
            assert_eq!(
                error.failure,
                if growing {
                    RepresentationFailure::Query(type_proof_limit())
                } else {
                    RepresentationFailure::RecursiveRepresentation
                }
            );
            assert!(matches!(
                store.get(error.root).unwrap().kind,
                TypeKind::Named { .. }
            ));
        }
    }

    #[test]
    fn representation_substitution_preserves_caller_supplied_authority() {
        let mut declaration = TypeStore::default();
        let unit = declaration.push(row(TypeKind::Tuple(vec![].into())));
        let callable = declaration.push(row(TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: vec![].into(),
            result: unit,
            error: None,
            allocation: Allocation::NoAlloc,
            authority: AuthoritySummary::Known(
                vec![AuthorityTerm::TypeParameter(TypeParameterRef::Bound {
                    depth: 1,
                    parameter: TypeParamId(0),
                })]
                .into(),
            ),
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        })));
        let signature = nominal(declaration, Some(callable), vec![]);
        let mut store = TypeStore::default();
        let argument = store.push(row(TypeKind::Primitive(Primitive::Bytes)));
        let root = store.push(row(TypeKind::Named {
            definition: path("Wrap"),
            arguments: vec![GenericArgument::Type(argument)].into(),
        }));
        for authority in [AuthoritySummary::Unknown, AuthoritySummary::inert()] {
            let expanded = expand_representation(
                &mut store,
                root,
                &mut |store, _, arguments| {
                    // The callback reads actuals from caller-owned scratch.
                    assert!(
                        matches!(arguments[0], GenericArgument::Type(t) if store.get(t).is_some())
                    );
                    Ok(Some((
                        signature.clone(),
                        vec![authority.clone(); arguments.len()],
                    )))
                },
                None,
            )
            .unwrap();
            let TypeKind::Function(function) = &store.get(expanded).unwrap().kind else {
                panic!("expanded callable");
            };
            assert_eq!(function.authority, authority);
        }
    }

    #[test]
    fn only_strict_active_argument_growth_spends_the_operational_budget() {
        let mut store = TypeStore::default();
        let mut actual = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let mut proof = NominalExpansions::default();
        for growing in 0..=MAX_POLYMORPHIC_EXPANSIONS + 1 {
            let root = store.push(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(actual)].into(),
            }));
            let result = proof.enter(
                root,
                0,
                |a, b| semantic_equal(&store, a, &store, b).unwrap_or(false),
                |a, b| nominal_arguments_grow(&store, a, &store, b),
            );
            if growing <= MAX_POLYMORPHIC_EXPANSIONS {
                assert_eq!(result, Ok(true));
            } else {
                assert_eq!(result, Err(type_proof_limit()));
            }
            actual = store.push(row(TypeKind::Builtin {
                kind: BuiltinType::List,
                arguments: vec![GenericArgument::Type(actual)].into(),
            }));
        }
    }

    #[test]
    fn regular_recursive_properties_check_every_component_and_nonregular_recursion_is_operational()
    {
        for growing in [false, true] {
            let mut declaration = TypeStore::default();
            let parameter = declaration.push(row(TypeKind::Parameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))));
            let actual = if growing {
                declaration.push(row(TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments: vec![GenericArgument::Type(parameter)].into(),
                }))
            } else {
                parameter
            };
            let next = declaration.push(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(actual)].into(),
            }));
            let representation = declaration.push(row(TypeKind::Box(next)));
            let signature = nominal(declaration, Some(representation), vec![]);
            let mut store = TypeStore::default();
            let scalar = store.push(row(TypeKind::Primitive(Primitive::U8)));
            let root = store.push(row(TypeKind::Named {
                definition: path("Wrap"),
                arguments: vec![GenericArgument::Type(scalar)].into(),
            }));
            let generics = GenericDeclarations::default();
            for bound in [Bound::Data, Bound::Key] {
                let result = structural_equality(&store, root, bound, &generics, &mut |_| {
                    Ok(Some(signature.clone()))
                });
                assert_eq!(
                    result,
                    if growing {
                        Err(type_proof_limit())
                    } else {
                        Ok(true)
                    }
                );
            }
            let copyable = structural_property(
                &store,
                root,
                StructuralProperty::Copyable,
                &generics,
                &mut |_| Ok(Some(signature.clone())),
            );
            assert_eq!(
                copyable,
                if growing {
                    Err(type_proof_limit())
                } else {
                    Ok(true)
                }
            );
            assert_eq!(
                structural_property(
                    &store,
                    root,
                    StructuralProperty::Plain,
                    &generics,
                    &mut |_| Ok(Some(signature.clone()))
                ),
                Ok(false)
            );
        }
    }

    #[test]
    fn growth_inside_a_fixed_constructor_is_detected_and_shrinking_is_not() {
        let mut store = TypeStore::default();
        let scalar = store.push(row(TypeKind::Primitive(Primitive::U8)));
        let boxed = store.push(row(TypeKind::Box(scalar)));
        let old_actual = store.push(row(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }));
        let new_actual = store.push(row(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(boxed)].into(),
        }));
        let old = store.push(row(TypeKind::Named {
            definition: path("Wrap"),
            arguments: vec![GenericArgument::Type(old_actual)].into(),
        }));
        let new = store.push(row(TypeKind::Named {
            definition: path("Wrap"),
            arguments: vec![GenericArgument::Type(new_actual)].into(),
        }));
        assert!(nominal_arguments_grow(&store, old, &store, new));
        assert!(!nominal_arguments_grow(&store, new, &store, old));
    }
}
