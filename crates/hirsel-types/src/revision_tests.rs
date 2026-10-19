use super::*;
use hirsel_syntax::{ItemId, StmtId};
fn scalar() -> Type {
    Type::with_kind(TypeKind::Primitive(Primitive::U32))
}
fn function(result: TypeId, generics: Vec<GenericParameter>) -> Type {
    Type {
        kind: TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: generics.into(),
            result,
            parameters: vec![].into(),
            error: None,
            allocation: Allocation::MayAllocate,
            authority: AuthoritySummary::inert(),
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }),
        ..scalar()
    }
}
fn path(name: &str) -> DefPath {
    DefPath(hirsel_syntax::ItemPath {
        package: hirsel_syntax::PackageId {
            name: "p".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}
#[test]
fn cloned_stores_share_only_immutable_existing_rows() {
    let mut a = TypeStore::default();
    let common = a.push(scalar());
    let mut b = a.clone();
    assert_eq!(a.get(common), b.get(common));
    let a_new = a.push(scalar());
    let b_new = b.push(scalar());
    assert_eq!(a_new.index(), b_new.index());
    assert_ne!(a_new, b_new);
    assert!(
        std::panic::catch_unwind(|| a.get(b_new)).map_or(cfg!(debug_assertions), |v| v.is_none())
    );
    let mut unrelated = TypeStore::default();
    let wrong = unrelated.push(scalar());
    assert!(
        std::panic::catch_unwind(|| a.get(wrong)).map_or(cfg!(debug_assertions), |v| v.is_none())
    );
}
#[test]
fn import_remaps_shared_edges_and_equality_is_rooted() {
    let mut source = TypeStore::default();
    let child = source.push(scalar());
    let root = source.push(Type {
        kind: TypeKind::Record(
            vec![
                Field {
                    name: "x".into(),
                    ty: child,
                    growable: true,
                },
                Field {
                    name: "y".into(),
                    ty: child,
                    growable: false,
                },
            ]
            .into(),
        ),
        ..scalar()
    });
    let mut target = TypeStore::default();
    target.push(Type {
        kind: TypeKind::Error,
        ..scalar()
    });
    let imported = import_type(&source, root, &mut target).unwrap();
    assert_ne!(root, imported);
    let TypeKind::Record(fields) = &target.get(imported).unwrap().kind else {
        panic!()
    };
    assert_eq!(fields[0].ty, fields[1].ty);
    assert!(fields[0].growable);
    assert!(semantic_equal(&source, root, &target, imported).unwrap());
    let mut changed = target.get(imported).unwrap().clone();
    changed.drop_allocation = DropAllocation::MayAllocate;
    let changed = target.push(changed);
    assert!(!semantic_equal(&source, root, &target, changed).unwrap());
}
#[test]
fn graph_helpers_reject_cycles_and_recovery() {
    let source = TypeStore::from_graph(1, |ids| {
        vec![Type {
            kind: TypeKind::Box(ids[0]),
            ..scalar()
        }]
    });
    let id = source.id(0).unwrap();
    assert_eq!(
        import_type(&source, id, &mut TypeStore::default()),
        Err(GraphError::StructuralCycle(id))
    );
    assert_eq!(
        semantic_equal(&source, id, &source, id),
        Err(GraphError::StructuralCycle(id))
    );
    let mut source = TypeStore::default();
    let id = source.push(Type {
        kind: TypeKind::Error,
        ..scalar()
    });
    assert_eq!(
        import_type(&source, id, &mut TypeStore::default()),
        Err(GraphError::RecoveryType(id))
    );
}
#[test]
fn unused_nested_reordered_patterns_keep_types_and_binding_modes() {
    let mut types = TypeStore::default();
    let ty = types.push(scalar());
    let owner = NameSite::Expression(ExprId(8));
    let unused = BindingDecision {
        owner: owner.clone(),
        binding: ResolvedName::Local(LocalId(4)),
        site: NameSite::Pattern(PatternId(2)),
        ty,
        usage: UseKind::Borrow,
        mutable: false,
    };
    let patterns = vec![
        PatternDecision {
            pattern: PatternId(0),
            ty,
            usage: UseKind::Take,
            kind: PatternDecisionKind::Constructor(ContextualName::Constructor(path("Record"))),
            fields: vec![(FieldId(1), PatternId(1)), (FieldId(0), PatternId(2))],
            annotation: None,
        },
        PatternDecision {
            pattern: PatternId(1),
            ty,
            usage: UseKind::Take,
            kind: PatternDecisionKind::Tuple,
            fields: vec![(FieldId(0), PatternId(3))],
            annotation: None,
        },
        PatternDecision {
            pattern: PatternId(2),
            ty,
            usage: UseKind::Borrow,
            kind: PatternDecisionKind::Binding(LocalId(4)),
            fields: vec![],
            annotation: None,
        },
        PatternDecision {
            pattern: PatternId(3),
            ty,
            usage: UseKind::Take,
            kind: PatternDecisionKind::Wildcard,
            fields: vec![],
            annotation: None,
        },
    ];
    let body = CheckedBody {
        tails: vec![],
        places: vec![],
        calls: vec![],
        types,
        bindings: vec![unused],
        patterns,
        resolution: Resolution::default(),
        expressions: vec![],
        instantiations: vec![],
        closures: vec![],
    };
    assert_eq!(body.bindings[0].owner, owner);
    assert!(body.types.get(body.bindings[0].ty).is_some());
    assert_eq!(body.patterns[0].fields[0], (FieldId(1), PatternId(1)));
    assert!(body.expressions.is_empty()); // unused bindings need no expression row
}
#[test]
fn resolution_preserves_all_lookup_channels_and_nominal_roles() {
    let context = FileContext {
        package: path("x").0.package,
        modules: vec![],
    };
    let lookup = LookupOutput {
        manifest: ContentHash::of_bytes(b"manifest-2"),
        result: Err(QueryError::NotImplemented),
    };
    let owner = NameSite::Item(ItemId(0));
    let resolution = Resolution {
        reads: vec![
            DependencyRead::ProviderLookup {
                context,
                package: None,
                segments: vec!["absent".into()],
                output: lookup.clone(),
            },
            DependencyRead::Signature {
                definition: path("absent"),
                fingerprint: None,
            },
        ],
        types: vec![
            (
                TypeExprId(0),
                ResolvedName::TypeParameter(TypeParameterRef::bound(TypeParamId(0))),
            ),
            (TypeExprId(1), ResolvedName::Primitive(Primitive::U32)),
        ],
        contextual: vec![(
            NameSite::Pattern(PatternId(0)),
            ContextualName::Variant {
                owner: path("Result"),
                variant: VariantId(1),
            },
        )],
        paths: vec![(owner.clone(), ResolvedName::Definition(path("Dir")))],
        local_functions: vec![(StmtId(0), LocalId(0))],
        deferred: vec![DeferredName {
            site: NameSite::Expression(ExprId(0)),
            span: Default::default(),
            name: "Ok".into(),
        }],
        generics: vec![GenericBinding {
            owner: owner.clone(),
            name: Symbol::intern("T"),
            parameter: GenericParameterName::Type(TypeParameterRef::bound(TypeParamId(0))),
        }],
        reference_scopes: vec![(NameSite::Type(TypeExprId(0)), owner)],
        ..Default::default()
    };
    assert_eq!(resolution.clone(), resolution);
    assert!(lookup.result.is_err());
    let metadata = SignatureMetadata {
        roles: NominalRoles {
            unsafe_capability: Some(path("Unsafe")),
            result_type: Some(path("Result")),
            tasks_capability: Some(path("Tasks")),
            scope_capability: Some(path("Scope")),
            heap: None,
        },
        ..Default::default()
    };
    assert_ne!(
        metadata.roles.unsafe_capability,
        Some(DefPath(hirsel_syntax::ItemPath {
            modules: vec!["user".into()],
            ..path("Unsafe").0
        }))
    );
    let error = QueryError::Operational(OperationalFailure {
        operation: "lookup".into(),
        message: "offline".into(),
    });
    assert!(!error.cacheable());
    assert!(!QueryError::NotImplemented.cacheable());
    assert!(QueryError::Diagnostics(vec![]).cacheable());
}

#[test]
fn function_identity_ignores_binder_names_but_preserves_labels_and_conventions() {
    let mut store = TypeStore::default();
    let result = store.push(scalar());
    let mut original = function(result, vec![]);
    let TypeKind::Function(f) = &mut original.kind else {
        panic!()
    };
    f.parameters.push(Parameter {
        name: Some("left".into()),
        label: None,
        passing: hirsel_syntax::PassingMode::Borrow,
        ty: result,
    });
    let root = store.push(original.clone());
    let TypeKind::Function(f) = &mut original.kind else {
        panic!()
    };
    f.parameters[0].name = Some("renamed".into());
    let renamed = store.push(original.clone());
    assert!(semantic_equal(&store, root, &store, renamed).unwrap());
    let TypeKind::Function(f) = &mut original.kind else {
        panic!()
    };
    f.parameters[0].label = Some("from".into());
    let labelled = store.push(original.clone());
    assert!(!semantic_equal(&store, root, &store, labelled).unwrap());
    let TypeKind::Function(f) = &mut original.kind else {
        panic!()
    };
    f.parameters[0].label = None;
    f.parameters[0].passing = hirsel_syntax::PassingMode::Take;
    let taken = store.push(original);
    assert!(!semantic_equal(&store, root, &store, taken).unwrap());
    let TypeKind::Function(f) = &store.get(root).unwrap().kind else {
        panic!()
    };
    assert_eq!(f.parameters[0].name.as_deref(), Some("left"));
}

#[test]
fn value_brands_allow_erasure_and_membership_but_never_strengthen_or_change_nested_identity() {
    let mut store = TypeStore::default();
    let brand = |parameter| Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(parameter))),
        fields: vec![].into(),
    };
    let root = |origins| Type {
        kind: TypeKind::Primitive(Primitive::Bytes),
        sendability_requirement: crate::SendabilityRequirement::None,
        allocation_target: None,
        erased_captures: None,
        brand: Brand {
            alternatives: origins,
        },
        ..scalar()
    };
    let erased = store.push(root(vec![].into()));
    let a = store.push(root(vec![brand(0)].into()));
    let b = store.push(root(vec![brand(1)].into()));
    let union = store.push(root(vec![brand(0), brand(1)].into()));
    let nested_a = store.push(Type {
        kind: TypeKind::Tuple(vec![a].into()),
        ..scalar()
    });
    let nested_union = store.push(Type {
        kind: TypeKind::Tuple(vec![union].into()),
        ..scalar()
    });
    for (expected, actual, accepted) in [
        (erased, a, true),
        (union, a, true),
        (union, b, true),
        (a, union, false),
        (a, erased, false),
        (a, b, false),
        (nested_union, nested_a, false),
    ] {
        let result = compatible_type(
            &store,
            expected,
            actual,
            TypeRelation::Value,
            &GenericDeclarations::signature(&[]),
            &mut |_| Ok::<_, crate::QueryError>(None),
        )
        .unwrap();
        assert_eq!(result, accepted, "expected {expected:?}, actual {actual:?}");
    }
}

#[test]
fn lexical_parameter_graphs_keep_inner_binders_and_bind_outer_references() {
    let mut types = TypeStore::default();
    let own = ParameterRef::bound(ParamId(0));
    let outer = ParameterRef::Bound {
        depth: 1,
        parameter: ParamId(0),
    };
    let mut result = scalar();
    result.brand = Brand {
        alternatives: vec![
            Origin {
                root: OriginRoot::Parameter(own),
                fields: vec![].into(),
            },
            Origin {
                root: OriginRoot::Parameter(outer),
                fields: vec![].into(),
            },
        ]
        .into(),
    };
    let result = types.push(result);
    let mut callback = function(result, vec![]);
    callback.brand = Brand {
        alternatives: vec![Origin {
            root: OriginRoot::Parameter(own),
            fields: vec![].into(),
        }]
        .into(),
    };
    let TypeKind::Function(f) = &mut callback.kind else {
        panic!()
    };
    f.authority = AuthoritySummary::Known(vec![AuthorityTerm::Parameter(outer)].into());
    f.uses = Uses::Only(AuthoritySummary::Known(
        vec![
            AuthorityTerm::Parameter(own),
            AuthorityTerm::Parameter(outer),
        ]
        .into(),
    ));
    let callback = types.push(callback);
    let scope = NameSite::Statement(StmtId(7));
    let bound = bind_parameter_scopes(&mut types, callback, &[scope]).unwrap();
    let row = types.get(bound).unwrap();
    let body = ParameterRef::body(scope, ParamId(0));
    assert_eq!(row.brand.alternatives[0].root, OriginRoot::Parameter(body));
    let TypeKind::Function(f) = &row.kind else {
        panic!()
    };
    assert_eq!(
        f.authority,
        AuthoritySummary::Known(vec![AuthorityTerm::Parameter(body)].into())
    );
    assert_eq!(
        types
            .get(f.result)
            .unwrap()
            .brand
            .alternatives
            .iter()
            .map(|o| o.root.clone())
            .collect::<Vec<_>>(),
        vec![OriginRoot::Parameter(own), OriginRoot::Parameter(body)]
    );
    let sibling =
        bind_parameter_scopes(&mut types, callback, &[NameSite::Statement(StmtId(8))]).unwrap();
    assert!(!semantic_equal(&types, bound, &types, sibling).unwrap());
    assert!(matches!(
        bind_parameter_scopes(&mut types, callback, &[]),
        Err(GraphError::UnboundParameter(_))
    ));
}

#[test]
fn call_origin_substitution_reaches_outer_references_inside_callbacks() {
    let mut types = TypeStore::default();
    let inner = ParameterRef::bound(ParamId(0));
    let outer = ParameterRef::Bound {
        depth: 1,
        parameter: ParamId(0),
    };
    let mut result = scalar();
    result.modes.relation = Relation::parameters([inner, outer]);
    result.brand = Brand {
        alternatives: vec![
            Origin {
                root: OriginRoot::Parameter(inner),
                fields: vec![].into(),
            },
            Origin {
                root: OriginRoot::Parameter(outer),
                fields: vec![FieldId(2)].into(),
            },
        ]
        .into(),
    };
    let result = types.push(result);
    let callback = types.push(function(result, vec![]));
    let source = Origin {
        root: OriginRoot::Parameter(ParameterRef::body(NameSite::Item(ItemId(42)), ParamId(3))),
        fields: vec![FieldId(1)].into(),
    };
    let instantiated = instantiate_parameter_origins(
        &mut types,
        callback,
        &std::collections::BTreeMap::from([(
            inner,
            OriginArgument {
                brand: Brand::default(),
                identity: source.clone(),
                relation: Relation::global(),
            },
        )]),
    )
    .unwrap();
    let TypeKind::Function(f) = &types.get(instantiated).unwrap().kind else {
        panic!()
    };
    assert_eq!(
        types.get(f.result).unwrap().modes.relation,
        Relation::parameters([inner])
    );
    let brand = &types.get(f.result).unwrap().brand;
    assert_eq!(brand.alternatives[0].root, OriginRoot::Parameter(inner));
    assert_eq!(
        brand.alternatives[1],
        Origin {
            root: source.root,
            fields: vec![FieldId(1), FieldId(2)].into()
        }
    );
}

#[test]
fn allocator_targets_respect_binders_and_directional_placement() {
    use std::collections::BTreeMap;
    let mut store = TypeStore::default();
    let mut selected = scalar();
    selected.modes.relation = Relation::place(Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(1))),
        fields: vec![].into(),
    });
    let selected = store.push(selected);
    let targets = BTreeMap::from([(ParameterRef::bound(ParamId(1)), AllocationTarget::Heap)]);
    let resolved = instantiate_allocation_targets(&mut store, selected, &targets).unwrap();
    assert_eq!(
        store.get(resolved).unwrap().modes.relation,
        Relation::global()
    );
    let inner = store.push(function(selected, vec![]));
    let unchanged = instantiate_allocation_targets(&mut store, inner, &targets).unwrap();
    assert!(semantic_equal(&store, inner, &store, unchanged).unwrap());
    let unspecified = store.push(scalar());
    let mut lookup = |_: &hirsel_syntax::DefPath| Ok::<_, QueryError>(None);
    assert!(
        compatible_type(
            &store,
            unspecified,
            resolved,
            TypeRelation::Value,
            &GenericDeclarations::signature(&[]),
            &mut lookup
        )
        .unwrap()
    );
    assert!(
        !compatible_type(
            &store,
            unspecified,
            selected,
            TypeRelation::Value,
            &GenericDeclarations::signature(&[]),
            &mut lookup
        )
        .unwrap()
    );
    let mut known = scalar();
    known.allocation_target = Some(AllocationTarget::Heap);
    let known = store.push(known);
    assert!(
        !compatible_type(
            &store,
            known,
            unspecified,
            TypeRelation::Value,
            &GenericDeclarations::signature(&[]),
            &mut lookup
        )
        .unwrap()
    );
}

#[test]
fn argument_brand_selection_preserves_all_origins_and_inner_binders() {
    use std::collections::BTreeMap;
    let mut store = TypeStore::default();
    let parameter = ParameterRef::bound(ParamId(0));
    let mut selected = scalar();
    selected.brand.alternatives.push(Origin {
        root: OriginRoot::ArgumentBrand(parameter),
        fields: vec![FieldId(3)].into(),
    });
    let selected = store.push(selected);
    let source = Origin {
        root: OriginRoot::Local(LocalId(7)),
        fields: vec![FieldId(1)].into(),
    };
    let other = Origin {
        root: OriginRoot::Local(LocalId(8)),
        fields: vec![].into(),
    };
    let arguments = BTreeMap::from([(
        parameter,
        OriginArgument {
            identity: Origin {
                root: OriginRoot::Expression(ExprId(13)),
                fields: vec![].into(),
            },
            brand: Brand {
                alternatives: vec![source.clone(), other.clone()].into(),
            },
            relation: Relation::global(),
        },
    )]);
    let actual = instantiate_parameter_origins(&mut store, selected, &arguments).unwrap();
    let mut expected = vec![source, other];
    for origin in &mut expected {
        origin.fields.push(FieldId(3));
    }
    assert_eq!(store.get(actual).unwrap().brand.alternatives, expected);
    let callable = store.push(function(selected, vec![]));
    let nested = instantiate_parameter_origins(&mut store, callable, &arguments).unwrap();
    assert!(semantic_equal(&store, callable, &store, nested).unwrap());
    let mut invalid = scalar();
    invalid.modes.relation = Relation::place(Origin {
        root: OriginRoot::ArgumentBrand(parameter),
        fields: vec![].into(),
    });
    let invalid = store.push(invalid);
    assert!(matches!(
        instantiate_parameter_origins(&mut store, invalid, &arguments),
        Err(GraphError::UnboundOrigin(OriginRoot::ArgumentBrand(_)))
    ));
}

#[test]
fn sibling_substitution_preserves_unresolved_fields_and_nested_scopes() {
    use std::collections::BTreeMap;
    let mut store = TypeStore::default();
    let origin = Origin {
        root: OriginRoot::Sibling(FieldId(1)),
        fields: vec![FieldId(4)].into(),
    };
    let mut field = scalar();
    field.brand = Brand {
        alternatives: vec![origin.clone()].into(),
    };
    field.modes.relation = Relation::place(origin.clone());
    let field = store.push(field);
    let missing = instantiate_sibling_origins(&mut store, field, &BTreeMap::new()).unwrap();
    assert!(semantic_equal(&store, field, &store, missing).unwrap());
    let parameter = ParameterRef::bound(ParamId(2));
    let fields = BTreeMap::from([(
        FieldId(1),
        Origin {
            root: OriginRoot::Parameter(parameter),
            fields: vec![FieldId(3)].into(),
        },
    )]);
    let actual = instantiate_sibling_origins(&mut store, field, &fields).unwrap();
    let expected = Origin {
        root: OriginRoot::Parameter(parameter),
        fields: vec![FieldId(3), FieldId(4)].into(),
    };
    assert_eq!(
        store.get(actual).unwrap().brand.alternatives,
        vec![expected.clone()]
    );
    assert_eq!(
        store.get(actual).unwrap().modes.relation,
        Relation::place(expected)
    );
    let callable = store.push(function(field, vec![]));
    let actual = instantiate_sibling_origins(&mut store, callable, &fields).unwrap();
    let TypeKind::Function(callable) = &store.get(actual).unwrap().kind else {
        panic!()
    };
    let result = store.get(callable.result).unwrap();
    assert_eq!(
        result.brand.alternatives[0].root,
        OriginRoot::Parameter(ParameterRef::Bound {
            depth: 1,
            parameter: ParamId(2)
        })
    );
}

#[test]
fn mutable_storage_contract_does_not_accept_a_local_read_row() {
    let mut store = TypeStore::default();
    let global = store.push(Type {
        kind: TypeKind::Primitive(Primitive::String),
        ..scalar()
    });
    let mut row = store.get(global).unwrap().clone();
    row.modes.relation = Relation::environment();
    let local = store.push(row);
    let mut lookup = |_: &DefPath| Ok::<_, crate::QueryError>(None);
    assert!(
        compatible_type(
            &store,
            local,
            local,
            TypeRelation::Value,
            &GenericDeclarations::signature(&[]),
            &mut lookup
        )
        .unwrap()
    );
    assert!(
        !compatible_storage_type(
            &store,
            local,
            local,
            &GenericDeclarations::signature(&[]),
            &mut lookup
        )
        .unwrap()
    );
    assert!(
        compatible_storage_type(
            &store,
            local,
            global,
            &GenericDeclarations::signature(&[]),
            &mut lookup
        )
        .unwrap()
    );
}

#[test]
fn dependent_sources_keep_field_paths_and_nested_call_binders() {
    use std::collections::BTreeMap;
    let mut store = TypeStore::default();
    let p = ParameterRef::bound(ParamId(0));
    let mut result = scalar();
    result.kind = TypeKind::Primitive(Primitive::Bytes);
    result.modes.relation = Relation::new([Reach::Place(Origin {
        root: OriginRoot::Parameter(ParameterRef::Bound {
            depth: 1,
            parameter: ParamId(0),
        }),
        fields: vec![FieldId(1)].into(),
    })]);
    let result = store.push(result);
    let callback = store.push(function(result, vec![]));
    let source = Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(2))),
        fields: vec![].into(),
    };
    let actual = store.push(scalar());
    let arguments = BTreeMap::from([(
        p,
        OriginArgument {
            identity: Origin {
                root: OriginRoot::Expression(ExprId(4)),
                fields: vec![].into(),
            },
            brand: Brand::default(),
            relation: Relation::place(source),
        },
    )]);
    let actuals = BTreeMap::from([(p, actual)]);
    let callback =
        instantiate_parameter_sources(&mut store, callback, &arguments, &actuals).unwrap();
    let TypeKind::Function(f) = &store.get(callback).unwrap().kind else {
        panic!()
    };
    // The caller's reference shifts under the callback's binder, and the
    // field path is kept (#77).
    assert_eq!(
        store.get(f.result).unwrap().modes.relation,
        Relation::place(Origin {
            root: OriginRoot::Parameter(ParameterRef::Bound {
                depth: 1,
                parameter: ParamId(2)
            }),
            fields: vec![FieldId(1)].into(),
        })
    );
    let mut inner = scalar();
    inner.modes.relation = Relation::parameters([p]);
    let inner = store.push(inner);
    let callback = store.push(function(inner, vec![]));
    let unchanged =
        instantiate_parameter_sources(&mut store, callback, &arguments, &actuals).unwrap();
    assert!(semantic_equal(&store, callback, &store, unchanged).unwrap());
}

#[test]
fn placed_block_fields_inherit_the_owner_but_plain_and_related_fields_do_not() {
    let mut store = TypeStore::default();
    let plain = store.push(scalar());
    let block = store.push(Type {
        kind: TypeKind::Primitive(Primitive::Bytes),
        ..scalar()
    });
    let explicit = store.push(Type {
        modes: Modes {
            relation: Relation::environment(),
            ..Modes::global()
        },
        kind: TypeKind::Primitive(Primitive::Bytes),
        ..scalar()
    });
    let arena = Relation::place(Origin {
        root: OriginRoot::Local(LocalId(8)),
        fields: vec![].into(),
    });
    let owner = store.push(Type {
        modes: Modes {
            relation: arena.clone(),
            ..Modes::global()
        },
        kind: TypeKind::Tuple(vec![block, plain, explicit].into()),
        ..scalar()
    });
    let lookup = &mut |_: &hirsel_syntax::DefPath| Ok::<_, crate::QueryError>(None);
    let field = placed_part_type(
        &mut store,
        owner,
        block,
        &GenericDeclarations::signature(&[]),
        lookup,
    )
    .unwrap();
    assert_eq!(store.get(field).unwrap().modes.relation, arena);
    assert_eq!(
        store.get(field).unwrap().modes.uniqueness,
        store.get(block).unwrap().modes.uniqueness
    );
    assert_eq!(
        placed_part_type(
            &mut store,
            owner,
            plain,
            &GenericDeclarations::signature(&[]),
            lookup
        )
        .unwrap(),
        plain
    );
    assert_eq!(
        placed_part_type(
            &mut store,
            owner,
            explicit,
            &GenericDeclarations::signature(&[]),
            lookup
        )
        .unwrap(),
        explicit
    );
}

#[test]
fn complete_graph_view_covers_every_edge_and_preserves_import_ownership() {
    use TypeEdgeRole as R;
    let mut store = TypeStore::default();
    let a = store.push(scalar());
    let b = store.push(scalar());
    let c = store.push(scalar());
    let mut callable = match function(b, vec![]).kind {
        TypeKind::Function(f) => f,
        _ => unreachable!(),
    };
    callable.parameters.push(Parameter {
        name: None,
        label: None,
        passing: PassingMode::Borrow,
        ty: a,
    });
    callable.error = Some(c);
    let arguments = vec![
        GenericArgument::Type(a),
        GenericArgument::Size(Size::Constant(2)),
        GenericArgument::Type(b),
    ];
    let cases = vec![
        (TypeKind::Primitive(Primitive::U32), vec![]),
        (
            TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
            vec![],
        ),
        (TypeKind::Error, vec![]),
        (
            TypeKind::Tuple(vec![a, b].into()),
            vec![(a, R::Component), (b, R::Component)],
        ),
        (TypeKind::Box(a), vec![(a, R::Component)]),
        (
            TypeKind::Record(
                vec![Field {
                    name: "a".into(),
                    ty: a,
                    growable: false,
                }]
                .into(),
            ),
            vec![(a, R::Component)],
        ),
        (
            TypeKind::Variant(
                vec![Variant {
                    name: "A".into(),
                    fields: vec![(None, a), (None, b)].into(),
                }]
                .into(),
            ),
            vec![(a, R::Component), (b, R::Component)],
        ),
        (
            TypeKind::Named {
                definition: path("Pair"),
                arguments: arguments.clone().into(),
            },
            vec![(a, R::TypeArgument), (b, R::TypeArgument)],
        ),
        (
            TypeKind::Builtin {
                kind: BuiltinType::FnPtr,
                arguments: arguments.into(),
            },
            vec![(a, R::TypeArgument), (b, R::TypeArgument)],
        ),
        (
            TypeKind::Function(callable),
            vec![
                (a, R::FunctionParameter),
                (b, R::FunctionResult),
                (c, R::FunctionError),
            ],
        ),
    ];
    for (kind, expected) in cases {
        let edges = kind.graph_edges();
        assert_eq!(
            edges.iter().map(|e| (e.ty, e.role)).collect::<Vec<_>>(),
            expected
        );
        for edge in &edges {
            assert_eq!(
                edge.role.parameter_binders(),
                u32::from(matches!(kind, TypeKind::Function(_)))
            );
        }
        if matches!(kind, TypeKind::Error) {
            continue;
        }
        let root = store.push(Type { kind, ..scalar() });
        let mut destination = TypeStore::default();
        let imported = import_type(&store, root, &mut destination).unwrap();
        let imported_edges = destination.get(imported).unwrap().kind.graph_edges();
        assert_eq!(edges.len(), imported_edges.len());
        for (source, target) in edges.iter().zip(imported_edges) {
            assert_eq!(source.role, target.role);
            assert_eq!(destination.id(target.ty.index()), Some(target.ty));
            assert_ne!(source.ty, target.ty);
            assert!(semantic_equal(&store, source.ty, &destination, target.ty).unwrap());
        }
    }
}

#[test]
fn identical_ordinals_in_distinct_binders_are_not_equal_or_interchangeable() {
    let mut types = TypeStore::default();
    let outer = NameSite::Item(ItemId(0));
    let inner = NameSite::Statement(StmtId(0));
    let t = types.push(Type {
        kind: TypeKind::Parameter(TypeParameterRef::body(outer, TypeParamId(0))),
        ..scalar()
    });
    let u = types.push(Type {
        kind: TypeKind::Parameter(TypeParameterRef::body(inner, TypeParamId(0))),
        ..scalar()
    });
    let mut declarations = GenericDeclarations::default();
    for scope in [outer, inner] {
        declarations.insert(
            scope,
            vec![GenericParameter::Type {
                name: "P".into(),
                bounds: vec![Bound::Plain].into(),
            }],
        );
    }
    assert!(!semantic_equal(&types, t, &types, u).unwrap());
    for relation in [
        TypeRelation::Exact,
        TypeRelation::Parameter,
        TypeRelation::Value,
    ] {
        assert!(
            !compatible_type(&types, t, u, relation, &declarations, &mut |_| Ok::<
                _,
                QueryError,
            >(
                None
            ))
            .unwrap()
        );
    }
    let actual = types.push(scalar());
    let source = types.clone();
    let substituted = substitute_type(
        &source,
        t,
        &mut types,
        Substitution {
            binder: Some(GenericBinder::Body(inner)),
            arguments: &[GenericArgument::Type(actual)],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(semantic_equal(&types, t, &types, substituted).unwrap());
}

#[test]
fn a_callable_cycle_cannot_escape_detection_by_increasing_binder_depth() {
    let types = TypeStore::from_graph(1, |ids| vec![function(ids[0], vec![])]);
    let root = types.id(0).unwrap();
    assert_eq!(
        import_type(&types, root, &mut TypeStore::default()),
        Err(GraphError::StructuralCycle(root))
    );
    assert!(!generic_references_valid(
        &types,
        root,
        &GenericDeclarations::default()
    ));
}

#[test]
fn normalization_keeps_enclosing_bounds_for_a_shared_nested_callable() {
    let mut types = TypeStore::default();
    let captured = types.push(Type {
        kind: TypeKind::Parameter(TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        }),
        ..scalar()
    });
    let inner = types.push(function(captured, vec![]));
    let outer = |bounds| {
        function(
            inner,
            vec![GenericParameter::Type {
                name: "T".into(),
                bounds,
            }],
        )
    };
    let sendable = types.push(outer(vec![Bound::Sendable].into()));
    let unconstrained = types.push(outer(vec![].into()));
    let root = types.push(Type {
        kind: TypeKind::Tuple(vec![sendable, unconstrained].into()),
        ..scalar()
    });
    let root = normalize_type_properties(
        &mut types,
        root,
        &GenericDeclarations::default(),
        &mut |_| Ok::<_, QueryError>(None),
    )
    .unwrap();
    let TypeKind::Tuple(parts) = &types.get(root).unwrap().kind else {
        panic!()
    };
    let result = |mut id| {
        for _ in 0..2 {
            let TypeKind::Function(f) = &types.get(id).unwrap().kind else {
                panic!()
            };
            id = f.result;
        }
        types.get(id).unwrap().modes.sendability.clone()
    };
    assert_eq!(result(parts[0]), Sendability::Sendable);
    assert_ne!(result(parts[1]), Sendability::Sendable);
}

#[test]
fn extending_a_working_store_preserves_the_published_snapshot() {
    let mut working = TypeStore::default();
    let child = working.push(scalar());
    working.push(Type {
        kind: TypeKind::Tuple(vec![child].into()),
        ..scalar()
    });
    let published = working.clone();
    let before = format!("{published:?}");
    let copied_before = hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows);
    for _ in 0..32 {
        working.push(Type {
            kind: TypeKind::Box(child),
            ..scalar()
        });
    }
    assert_eq!(format!("{published:?}"), before);
    assert_eq!(published.len(), 2);
    assert_eq!(
        hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows) - copied_before,
        2
    );
    assert_eq!(working.get(child), published.get(child));
}

#[test]
fn semantic_equality_visits_a_wide_graph_linearly() {
    for count in [32, 128, 512] {
        let mut left = TypeStore::default();
        let parts = (0..count)
            .map(|i| {
                left.push(Type {
                    kind: TypeKind::Named {
                        definition: path(&format!("N{i}")),
                        arguments: vec![].into(),
                    },
                    ..scalar()
                })
            })
            .collect();
        let root = left.push(Type {
            kind: TypeKind::Tuple(parts),
            ..scalar()
        });
        let mut right = TypeStore::default();
        let other = import_type(&left, root, &mut right).unwrap();
        let before = hirsel_intern::work::read(hirsel_intern::work::Counter::EqualityRows);
        assert!(semantic_equal(&left, root, &right, other).unwrap());
        let work = hirsel_intern::work::read(hirsel_intern::work::Counter::EqualityRows) - before;
        assert!(
            work <= 10 * (count + 1) as u64,
            "{count} components inspect {work} rows"
        );
    }
}

#[test]
fn graph_transport_in_place_preserves_external_transport_and_snapshot_rows() {
    let mut store = TypeStore::default();
    let parameter = store.push(Type {
        kind: TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
        ..scalar()
    });
    let nested = store.push(function(parameter, vec![]));
    let root = store.push(Type {
        kind: TypeKind::Tuple(vec![parameter, nested, parameter].into()),
        ..scalar()
    });
    let actual = store.push(scalar());
    let snapshot = store.clone();
    let original = format!("{snapshot:?}");
    let arguments = [GenericArgument::Type(actual)];
    let roles = NominalRoles::default();
    let substitution = Substitution {
        binder: Some(GenericBinder::Bound(0)),
        arguments: &arguments,
        authorities: &[AuthoritySummary::inert()],
        roles: &roles,
    };
    let mut external = store.clone();
    let expected = substitute_type(&snapshot, root, &mut external, substitution).unwrap();
    let result = substitute_type_in_place(&mut store, root, substitution).unwrap();
    assert!(semantic_equal(&external, expected, &store, result).unwrap());
    assert_eq!(format!("{snapshot:?}"), original);
    let TypeKind::Tuple(edges) = &store.get(result).unwrap().kind else {
        panic!()
    };
    assert_eq!(edges[0], edges[2]);
    let TypeKind::Function(inner) = &store.get(edges[1]).unwrap().kind else {
        panic!()
    };
    assert_eq!(
        store.get(inner.result).unwrap().kind,
        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0)))
    );
}

#[test]
fn synthetic_callable_import_retains_row_order_and_shared_edges() {
    let mut source = TypeStore::default();
    let result = source.push(scalar());
    let row = function(result, vec![]);
    let before = format!("{source:?}");
    let copied = hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows);
    let mut actual = TypeStore::default();
    let imported = import_type_row(&source, &row, &mut actual).unwrap();
    assert_eq!(
        hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows),
        copied
    );
    assert_eq!(format!("{source:?}"), before);
    let mut legacy_source = source.clone();
    let root = legacy_source.push(row);
    let mut expected = TypeStore::default();
    let legacy = import_type(&legacy_source, root, &mut expected).unwrap();
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    assert!(semantic_equal(&actual, imported, &expected, legacy).unwrap());
}

#[test]
fn origin_visiting_retains_rewriter_order_depth_and_cycle_policy() {
    let mut source = TypeStore::default();
    let origin = Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    };
    let leaf = source.push(Type {
        brand: Brand {
            alternatives: vec![origin.clone()].into(),
        },
        modes: Modes {
            relation: Relation::place(origin),
            ..Modes::global()
        },
        ..scalar()
    });
    let callable = source.push(function(leaf, vec![]));
    let root = source.push(Type {
        kind: TypeKind::Tuple(vec![leaf, callable, leaf].into()),
        ..scalar()
    });
    let mut expected = vec![];
    map_type_origins(&mut source.clone(), root, |origin, depth| {
        expected.push((origin.clone(), depth));
        Ok(origin.clone())
    })
    .unwrap();
    let mut actual = vec![];
    visit_type_origins(&source, root, |origin, depth| {
        actual.push((origin.clone(), depth));
        Ok(())
    })
    .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        actual.iter().map(|(_, depth)| *depth).collect::<Vec<_>>(),
        vec![0, 0, 1, 1]
    );
    let cycle = TypeStore::from_graph(1, |ids| {
        vec![Type {
            kind: TypeKind::Box(ids[0]),
            ..scalar()
        }]
    });
    let root = cycle.id(0).unwrap();
    assert_eq!(
        visit_type_origins(&cycle, root, |_, _| Ok(())),
        Err(GraphError::StructuralCycle(root))
    );
}

#[test]
fn scratch_marks_keep_closed_prefixes_and_never_reuse_row_identity() {
    let mut store = TypeStore::default();
    let leaf = store.push(scalar());
    let root = store.push(Type {
        kind: TypeKind::Box(leaf),
        ..scalar()
    });
    let snapshot = store.clone();
    let mark = store.mark();
    let first = store.push(Type {
        kind: TypeKind::Tuple(vec![root].into()),
        ..scalar()
    });
    store.truncate_to(mark);
    assert_eq!(store, snapshot);
    let copied = hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows);
    let second = store.push(scalar());
    assert_eq!(second.index(), first.index());
    assert_ne!(second, first);
    assert_eq!(
        hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows),
        copied
    );
    store.truncate_to(mark);
    assert_eq!(store.get(root), snapshot.get(root));
    assert_eq!(store.len(), 2);
    let mut other = TypeStore::default();
    other.push(scalar());
    other.push(scalar());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| other.truncate_to(mark))).is_err()
    );
}

fn plain_record() -> (hirsel_syntax::DefPath, SignatureRef) {
    let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
        package: hirsel_syntax::PackageId {
            name: "fixture".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: "Point".into(),
    });
    let mut types = TypeStore::default();
    let x = types.push(scalar());
    let representation = types.push(Type {
        kind: TypeKind::Record(
            vec![Field {
                name: "x".into(),
                ty: x,
                growable: false,
            }]
            .into(),
        ),
        ..scalar()
    });
    let signature = Signature {
        definition: path.clone(),
        types,
        metadata: SignatureMetadata::default(),
        fingerprint: hirsel_syntax::ContentHash::default(),
        kind: SignatureKind::Type(TypeDefinition {
            loop_source: None,
            format: None,
            destructor_allocation: Allocation::MayAllocate,
            generics: vec![],
            representation: Some(representation),
            bounds: vec![],
            authority: AuthoritySummary::inert(),
            capability: None,
            implementation_authority: None,
            serializable: false,
            layout: Layout::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
        }),
    };
    (path, std::sync::Arc::new(signature))
}

#[test]
fn state_selector_transport_preserves_nested_binders_and_field_paths() {
    let mut store = TypeStore::default();
    let scalar = store.push(scalar());
    let mut inner = function(scalar, vec![]);
    let TypeKind::Function(f) = &mut inner.kind else {
        panic!()
    };
    f.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Mut,
        ty: scalar,
    }]
    .into();
    f.state_promises = vec![StatePromise {
        selector: RelationPlace {
            parameter: ParameterRef::bound(ParamId(0)),
            fields: vec![FieldId(2), FieldId(1)].into(),
        },
        advance_only: true,
        preserve_captures: true,
        preserve_unique: false,
        unretained: Default::default(),
    }]
    .into();
    let inner = store.push(inner);
    let outer = store.push(function(inner, vec![]));
    let mapped = bind_parameter_scopes(&mut store, outer, &[NameSite::Item(ItemId(4))]).unwrap();
    let TypeKind::Function(outer) = &store.get(mapped).unwrap().kind else {
        panic!()
    };
    let TypeKind::Function(inner) = &store.get(outer.result).unwrap().kind else {
        panic!()
    };
    assert_eq!(
        inner.state_promises[0].selector.parameter,
        ParameterRef::bound(ParamId(0))
    );
    assert_eq!(
        inner.state_promises[0].selector.fields.to_vec(),
        vec![FieldId(2), FieldId(1)]
    );
    let mut destination = TypeStore::default();
    let imported = import_type(&store, mapped, &mut destination).unwrap();
    assert!(semantic_equal(&store, mapped, &destination, imported).unwrap());
}

#[test]
fn capture_properties_keep_every_incoming_envelope_axis() {
    let mut row = scalar();
    row.modes.relation = Relation::parameters([ParameterRef::bound(ParamId(1))]);
    row.modes.multiplicity = Multiplicity::Once;
    row.drop_allocation = DropAllocation::MayAllocate;
    let evidence =
        CaptureProperties::from_type(&row, AuthoritySummary::Unknown, Sendability::NotSendable);
    assert_eq!(evidence.source_bounds, row.modes.relation);
    assert_eq!(evidence.brands, row.brand);
    assert_eq!(evidence.multiplicity, Multiplicity::Once);
    assert_eq!(evidence.drop_allocation, DropAllocation::MayAllocate);
    assert_eq!(evidence.authority, AuthoritySummary::Unknown);
    assert_eq!(evidence.sendability, Sendability::NotSendable);
}

#[test]
fn state_flags_and_selected_field_identity_change_canonical_bytes() {
    let mut store = TypeStore::default();
    let element = store.push(scalar());
    let value = store.push(Type {
        kind: TypeKind::Record(
            vec![
                Field {
                    name: "a".into(),
                    growable: false,
                    ty: element,
                },
                Field {
                    name: "b".into(),
                    growable: false,
                    ty: element,
                },
            ]
            .into(),
        ),
        ..Type::unit()
    });
    let mut row = function(value, vec![]);
    let TypeKind::Function(f) = &mut row.kind else {
        panic!()
    };
    f.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Mut,
        ty: value,
    }]
    .into();
    f.state_promises = vec![StatePromise {
        selector: RelationPlace {
            parameter: ParameterRef::bound(ParamId(0)),
            fields: vec![FieldId(0)].into(),
        },
        advance_only: false,
        preserve_captures: true,
        preserve_unique: false,
        unretained: Default::default(),
    }]
    .into();
    let first = store.push(row.clone());
    let bytes = encoding::canonical_type_encoding(&store, first).unwrap();
    let TypeKind::Function(f) = &mut row.kind else {
        panic!()
    };
    f.state_promises[0].selector.fields = vec![FieldId(1)].into();
    let field = store.push(row.clone());
    assert_ne!(
        bytes,
        encoding::canonical_type_encoding(&store, field).unwrap()
    );
    let TypeKind::Function(f) = &mut row.kind else {
        panic!()
    };
    f.state_promises[0].selector.fields = vec![FieldId(0)].into();
    f.state_promises[0].advance_only = true;
    let flags = store.push(row);
    assert_ne!(
        bytes,
        encoding::canonical_type_encoding(&store, flags).unwrap()
    );
}

#[test]
fn written_crossing_transport_never_proves_an_unproved_actual() {
    let mut source = TypeStore::default();
    let parameter = source.push(Type {
        kind: TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
        sendability_requirement: SendabilityRequirement::Sendable,
        modes: Modes {
            sendability: Sendability::Sendable,
            ..Modes::global()
        },
        ..scalar()
    });
    let root = source.push(Type {
        kind: TypeKind::Tuple(vec![parameter].into()),
        ..scalar()
    });
    let mut target = TypeStore::default();
    let actual = target.push(Type {
        kind: TypeKind::Primitive(Primitive::Bytes),
        ..scalar()
    });
    let substituted = substitute_type(
        &source,
        root,
        &mut target,
        Substitution {
            binder: Some(GenericBinder::Bound(0)),
            roles: &NominalRoles::default(),
            arguments: &[GenericArgument::Type(actual)],
            authorities: &[],
        },
    )
    .unwrap();
    let TypeKind::Tuple(parts) = &target.get(substituted).unwrap().kind else {
        panic!()
    };
    let part = target.get(parts[0]).unwrap();
    assert_eq!(
        part.sendability_requirement,
        SendabilityRequirement::Sendable
    );
    assert_eq!(part.modes.sendability, Sendability::NotSendable);
    let mut imported = TypeStore::default();
    let imported_root = import_type(&target, substituted, &mut imported).unwrap();
    assert!(semantic_equal(&target, substituted, &imported, imported_root).unwrap());
    let normalized = normalize_type_properties(
        &mut target,
        substituted,
        &GenericDeclarations::default(),
        &mut |_| Ok(None),
    )
    .unwrap();
    let TypeKind::Tuple(parts) = &target.get(normalized).unwrap().kind else {
        panic!()
    };
    assert_eq!(
        target.get(parts[0]).unwrap().sendability_requirement,
        SendabilityRequirement::Sendable
    );
    assert_eq!(
        target.get(parts[0]).unwrap().modes.sendability,
        Sendability::NotSendable
    );
}

#[test]
fn crossing_contracts_change_semantic_encodings_and_plain_contracts_are_redundant() {
    let mut store = TypeStore::default();
    let bytes = store.push(Type {
        kind: TypeKind::Primitive(Primitive::Bytes),
        ..scalar()
    });
    let qualified = store.push(Type {
        sendability_requirement: SendabilityRequirement::Sendable,
        ..store.get(bytes).unwrap().clone()
    });
    assert_ne!(
        encoding::canonical_type_encoding(&store, bytes).unwrap(),
        encoding::canonical_type_encoding(&store, qualified).unwrap()
    );
    let plain = store.push(scalar());
    let qualified_plain = store.push(Type {
        sendability_requirement: SendabilityRequirement::Sendable,
        ..scalar()
    });
    let normal = normalize_type_properties(
        &mut store,
        plain,
        &GenericDeclarations::default(),
        &mut |_| Ok(None),
    )
    .unwrap();
    let qualified_normal = normalize_type_properties(
        &mut store,
        qualified_plain,
        &GenericDeclarations::default(),
        &mut |_| Ok(None),
    )
    .unwrap();
    assert!(semantic_equal(&store, normal, &store, qualified_normal).unwrap());
}

#[test]
fn schema_opt_in_and_bound_change_public_fingerprints_independently() {
    let (_, original) = plain_record();
    let before = encoding::fingerprint(&original, &[]).unwrap();
    let mut opted = (*original).clone();
    let SignatureKind::Type(declaration) = &mut opted.kind else {
        unreachable!()
    };
    declaration.serializable = true;
    assert_ne!(before, encoding::fingerprint(&opted, &[]).unwrap());
    let mut bounded = (*original).clone();
    let SignatureKind::Type(declaration) = &mut bounded.kind else {
        unreachable!()
    };
    declaration.generics.push(GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Serializable].into(),
    });
    assert_ne!(
        encoding::fingerprint(&opted, &[]).unwrap(),
        encoding::fingerprint(&bounded, &[]).unwrap()
    );
}

#[test]
fn private_capability_ceiling_is_not_a_public_signature_hash_input() {
    let (_, original) = plain_record();
    let mut public = (*original).clone();
    public.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
    public.types = TypeStore::default();
    let SignatureKind::Type(declaration) = &mut public.kind else {
        unreachable!()
    };
    declaration.representation = None;
    declaration.capability = Some(CapabilityKind::Global);
    declaration.authority =
        AuthoritySummary::Known(vec![AuthorityTerm::Capability(public.definition.clone())].into());
    let public_hash = encoding::fingerprint(&public, &[]).unwrap();
    let SignatureKind::Type(declaration) = &mut public.kind else {
        unreachable!()
    };
    declaration.implementation_authority = Some(AuthoritySummary::Unknown);
    assert_eq!(public_hash, encoding::fingerprint(&public, &[]).unwrap());
    public.metadata.visibility = hirsel_syntax::Visibility::Private;
    let private_hash = encoding::fingerprint(&public, &[]).unwrap();
    let SignatureKind::Type(declaration) = &mut public.kind else {
        unreachable!()
    };
    declaration.implementation_authority = Some(AuthoritySummary::inert());
    assert_ne!(private_hash, encoding::fingerprint(&public, &[]).unwrap());
}

#[test]
fn cleanup_bound_instantiation_retains_exact_scopes_and_nested_binders() {
    let mut types = TypeStore::default();
    let owner = NameSite::Item(ItemId(10));
    let sibling = NameSite::Statement(StmtId(10));
    let bounded = GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::NoallocDrop].into(),
    };
    let unbounded = GenericParameter::Type {
        name: "U".into(),
        bounds: vec![].into(),
    };
    let declarations = GenericDeclarations::from_binders([
        (GenericBinder::Body(owner), vec![bounded.clone()]),
        (GenericBinder::Body(sibling), vec![unbounded.clone()]),
        (GenericBinder::Bound(0), vec![bounded]),
    ]);
    let body = TypeParameterRef::body(owner, TypeParamId(0));
    let other = TypeParameterRef::body(sibling, TypeParamId(0));
    let dependencies = [
        vec![body],
        vec![other],
        vec![body, other],
        vec![TypeParameterRef::bound(TypeParamId(0))],
        vec![TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        }],
    ];
    let parts: Vec<_> = dependencies
        .iter()
        .map(|parameters| {
            types.push(Type {
                drop_allocation: DropAllocation::DependsOn(parameters.clone().into()),
                ..scalar()
            })
        })
        .collect();
    let tuple = types.push(Type {
        kind: TypeKind::Tuple(parts.clone().into()),
        ..scalar()
    });
    let root = types.push(function(tuple, vec![unbounded]));
    let result = instantiate_cleanup_bounds(&mut types, root, &declarations).unwrap();
    let TypeKind::Function(f) = &types.get(result).unwrap().kind else {
        panic!()
    };
    let TypeKind::Tuple(normalized) = &types.get(f.result).unwrap().kind else {
        panic!()
    };
    for (i, part) in normalized.iter().enumerate() {
        let mut expected = types.get(parts[i]).unwrap().clone();
        if i == 0 || i == 4 {
            expected.drop_allocation = DropAllocation::NoAlloc;
        }
        assert_eq!(types.get(*part).unwrap(), &expected);
    }
    // The old immutable graph keeps its unresolved obligations.
    assert!(matches!(
        types.get(parts[0]).unwrap().drop_allocation,
        DropAllocation::DependsOn(_)
    ));
}

#[test]
fn an_owner_view_seals_to_its_public_signature_and_keeps_its_fingerprint() {
    let declaration = |representation, implementation_authority| TypeDefinition {
        loop_source: None,
        format: None,
        generics: vec![],
        representation,
        bounds: vec![],
        authority: AuthoritySummary::inert(),
        capability: None,
        implementation_authority,
        serializable: false,
        layout: Layout::default(),
        distinct: false,
        resource: false,
        drop: None,
        fact: None,
        destructor_allocation: Allocation::MayAllocate,
    };
    let mut public = Signature {
        definition: path("Hidden"),
        types: TypeStore::default(),
        kind: SignatureKind::Type(declaration(None, None)),
        metadata: SignatureMetadata {
            visibility: hirsel_syntax::Visibility::PublicOpaque,
            ..Default::default()
        },
        fingerprint: Default::default(),
    };
    public.fingerprint = encoding::fingerprint(&public, &[]).unwrap();
    let mut types = TypeStore::default();
    let field = types.push(scalar());
    let view = encoding::owner_view(
        &public,
        types,
        declaration(Some(field), Some(AuthoritySummary::inert())),
    );
    assert_eq!(view.fingerprint, public.fingerprint);
    assert_ne!(view, public);
    assert_eq!(encoding::sealed(&view), public);
    // A view whose public part differs does not seal to the public signature.
    let mut other = view.clone();
    other.metadata.visibility = hirsel_syntax::Visibility::Public;
    assert_ne!(encoding::sealed(&other), public);
}

#[test]
fn caller_tracking_changes_declaration_identity_without_changing_callable_types() {
    let mut types = TypeStore::default();
    let result = types.push(Type::unit());
    let TypeKind::Function(callable) = function(result, vec![]).kind else {
        unreachable!()
    };
    let mut signature = Signature {
        definition: path("helper"),
        types,
        metadata: SignatureMetadata::default(),
        fingerprint: Default::default(),
        kind: SignatureKind::Function(callable),
    };
    let original = signature.clone();
    let before = encoding::fingerprint(&signature, &[]).unwrap();
    signature.metadata.track_caller = true;
    assert_ne!(before, encoding::fingerprint(&signature, &[]).unwrap());
    assert_eq!(original.kind, signature.kind);
    assert_eq!(original.types, signature.types);
}
