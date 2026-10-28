use super::*;
use hirsel_syntax::{LexicalOwner, SiteRef, StmtId};
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
/// The lexical owner of the sites these tests name.
fn owner() -> LexicalOwner {
    LexicalOwner::first(&path("f"))
}
fn site(site: NameSite) -> SiteRef {
    owner().site(site)
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
fn equal_rows_intern_to_one_id_and_rows_are_lossless() {
    let t = TypeInterner::new(None);
    let common = t.intern(scalar());
    assert_eq!(t.intern(scalar()), common);
    let named = |name: &str| {
        let mut f = function(common, vec![]);
        let TypeKind::Function(callable) = &mut f.kind else {
            unreachable!()
        };
        callable.parameters = vec![Parameter {
            name: Some(name.into()),
            label: None,
            passing: PassingMode::Borrow,
            ty: common,
        }]
        .into();
        f
    };
    let x = t.intern(named("x"));
    let y = t.intern(named("y"));
    assert_ne!(x, y, "parameter names survive interning");
    assert!(t.semantic_eq(x, y), "and semantic equality erases them");
    assert_eq!(t.sem(x), t.sem(y));
    let TypeKind::Function(f) = &t.row(x).kind else {
        unreachable!()
    };
    assert_eq!(f.parameters[0].name.as_deref(), Some("x"));
    assert!(t.get(TypeId::DANGLING).is_none());
}
#[test]
fn shared_edges_survive_and_equality_is_rooted() {
    let t = TypeInterner::new(None);
    let child = t.intern(scalar());
    let root = t.intern(Type {
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
    // An unrelated recovery row does not affect a rooted comparison.
    t.intern(Type {
        kind: TypeKind::Error,
        ..scalar()
    });
    let TypeKind::Record(fields) = &t.row(root).kind else {
        panic!()
    };
    assert_eq!(fields[0].ty, fields[1].ty);
    assert!(fields[0].growable);
    assert!(semantic_equal(&t, root, root).unwrap());
    let mut changed = t.row(root).clone();
    changed.drop_allocation = DropAllocation::MayAllocate;
    let changed = t.intern(changed);
    assert!(!semantic_equal(&t, root, changed).unwrap());
}
#[test]
fn graph_helpers_reject_cycles_and_recovery() {
    let t = TypeInterner::new(None);
    let id = t.intern_graph(1, |ids| {
        vec![Type {
            kind: TypeKind::Box(ids[0]),
            ..scalar()
        }]
    })[0];
    assert_eq!(t.validate(id), Err(GraphError::StructuralCycle(id)));
    assert_eq!(
        substitute_fresh(&t, id, Substitution::default()),
        Err(GraphError::StructuralCycle(id))
    );
    assert_eq!(
        semantic_equal(&t, id, id),
        Err(GraphError::StructuralCycle(id))
    );
    let id = t.intern(Type {
        kind: TypeKind::Error,
        ..scalar()
    });
    assert_eq!(t.validate(id), Err(GraphError::RecoveryType(id)));
    assert_eq!(
        substitute_fresh(
            &t,
            id,
            Substitution {
                binder: Some(GenericBinder::Bound(0)),
                ..Default::default()
            }
        ),
        Err(GraphError::RecoveryType(id))
    );
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
    let owner = NameSite::Item;
    let resolution = Resolution {
        reads: std::sync::Arc::new(
            vec![
                DependencyRead::ProviderLookup {
                    context: (context).into(),
                    package: None,
                    segments: (vec!["absent".into()]).into(),
                    output: (lookup.clone()).into(),
                }
                .shared(),
                DependencyRead::Signature {
                    definition: hirsel_syntax::DefId::intern(&path("absent")),
                    fingerprint: None,
                }
                .shared(),
            ]
            .into(),
        ),
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
    let cloned = resolution.clone();
    assert_eq!(cloned, resolution);
    assert_eq!(
        std::sync::Arc::as_ptr(&cloned.reads),
        std::sync::Arc::as_ptr(&resolution.reads),
        "resolution clone copied dependency evidence"
    );
    let mut edited = cloned;
    std::sync::Arc::make_mut(&mut edited.reads).push(
        DependencyRead::Signature {
            definition: hirsel_syntax::DefId::intern(&path("later")),
            fingerprint: None,
        }
        .shared(),
    );
    assert_eq!(
        resolution.reads.len(),
        2,
        "later phases must preserve earlier evidence"
    );
    assert_eq!(edited.reads.len(), 3);
    assert_eq!(
        serde_json::to_vec(&resolution.reads).unwrap(),
        serde_json::to_vec(resolution.reads.as_ref()).unwrap()
    );
    let decoded: std::sync::Arc<SharedPath<SharedRead>> =
        serde_json::from_slice(&serde_json::to_vec(&resolution.reads).unwrap()).unwrap();
    assert_eq!(decoded, resolution.reads);
    assert!(lookup.result.is_err());
    let metadata = SignatureMetadata {
        roles: NominalRoles {
            unsafe_capability: Some(path("Unsafe")),
            result_type: Some(path("Result")),
            tasks_capability: Some(path("Tasks")),
            scope_capability: Some(path("Scope")),
            heap: None,
        }
        .into(),
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
    let store = TypeInterner::new(None);
    let result = store.intern(scalar());
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
    let root = store.intern(original.clone());
    let TypeKind::Function(f) = &mut original.kind else {
        panic!()
    };
    f.parameters[0].name = Some("renamed".into());
    let renamed = store.intern(original.clone());
    assert!(semantic_equal(&store, root, renamed).unwrap());
    let TypeKind::Function(f) = &mut original.kind else {
        panic!()
    };
    f.parameters[0].label = Some("from".into());
    let labelled = store.intern(original.clone());
    assert!(!semantic_equal(&store, root, labelled).unwrap());
    let TypeKind::Function(f) = &mut original.kind else {
        panic!()
    };
    f.parameters[0].label = None;
    f.parameters[0].passing = hirsel_syntax::PassingMode::Take;
    let taken = store.intern(original);
    assert!(!semantic_equal(&store, root, taken).unwrap());
    let TypeKind::Function(f) = &store.get(root).unwrap().kind else {
        panic!()
    };
    assert_eq!(f.parameters[0].name.as_deref(), Some("left"));
}

#[test]
fn value_brands_allow_erasure_and_membership_but_never_strengthen_or_change_nested_identity() {
    let store = TypeInterner::new(None);
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
    let erased = store.intern(root(vec![].into()));
    let a = store.intern(root(vec![brand(0)].into()));
    let b = store.intern(root(vec![brand(1)].into()));
    let union = store.intern(root(vec![brand(0), brand(1)].into()));
    let nested_a = store.intern(Type {
        kind: TypeKind::Tuple(vec![a].into()),
        ..scalar()
    });
    let nested_union = store.intern(Type {
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
    let types = TypeInterner::new(None);
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
    let result = types.intern(result);
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
    let callback = types.intern(callback);
    let scope = site(NameSite::Statement(StmtId(7)));
    let bound = bind_parameter_scopes(&types, callback, &[scope]).unwrap();
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
        bind_parameter_scopes(&types, callback, &[site(NameSite::Statement(StmtId(8)))]).unwrap();
    assert!(!semantic_equal(&types, bound, sibling).unwrap());
    assert!(matches!(
        bind_parameter_scopes(&types, callback, &[]),
        Err(GraphError::UnboundParameter(_))
    ));
}

#[test]
fn call_origin_substitution_reaches_outer_references_inside_callbacks() {
    let types = TypeInterner::new(None);
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
    let result = types.intern(result);
    let callback = types.intern(function(result, vec![]));
    let source = Origin {
        root: OriginRoot::Parameter(ParameterRef::body(site(NameSite::Item), ParamId(3))),
        fields: vec![FieldId(1)].into(),
    };
    let instantiated = instantiate_parameter_origins(
        &types,
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
    let store = TypeInterner::new(None);
    let mut selected = scalar();
    selected.modes.relation = Relation::place(Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(1))),
        fields: vec![].into(),
    });
    let selected = store.intern(selected);
    let targets = BTreeMap::from([(ParameterRef::bound(ParamId(1)), AllocationTarget::Heap)]);
    let resolved = instantiate_allocation_targets(&store, selected, &targets).unwrap();
    assert_eq!(
        store.get(resolved).unwrap().modes.relation,
        Relation::global()
    );
    let inner = store.intern(function(selected, vec![]));
    let unchanged = instantiate_allocation_targets(&store, inner, &targets).unwrap();
    assert!(semantic_equal(&store, inner, unchanged).unwrap());
    let unspecified = store.intern(scalar());
    let mut lookup = |_: hirsel_syntax::DefId| Ok::<_, QueryError>(None);
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
    let known = store.intern(known);
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
    let store = TypeInterner::new(None);
    let parameter = ParameterRef::bound(ParamId(0));
    let mut selected = scalar();
    selected.brand.alternatives.push(Origin {
        root: OriginRoot::ArgumentBrand(parameter),
        fields: vec![FieldId(3)].into(),
    });
    let selected = store.intern(selected);
    let source = Origin {
        root: OriginRoot::Local(owner(), LocalId(7)),
        fields: vec![FieldId(1)].into(),
    };
    let other = Origin {
        root: OriginRoot::Local(owner(), LocalId(8)),
        fields: vec![].into(),
    };
    let arguments = BTreeMap::from([(
        parameter,
        OriginArgument {
            identity: Origin {
                root: OriginRoot::Expression(owner(), ExprId(13)),
                fields: vec![].into(),
            },
            brand: Brand {
                alternatives: vec![source.clone(), other.clone()].into(),
            },
            relation: Relation::global(),
        },
    )]);
    let actual = instantiate_parameter_origins(&store, selected, &arguments).unwrap();
    let mut expected = vec![source, other];
    for origin in &mut expected {
        origin.fields.push(FieldId(3));
    }
    assert_eq!(store.get(actual).unwrap().brand.alternatives, expected);
    let callable = store.intern(function(selected, vec![]));
    let nested = instantiate_parameter_origins(&store, callable, &arguments).unwrap();
    assert!(semantic_equal(&store, callable, nested).unwrap());
    let mut invalid = scalar();
    invalid.modes.relation = Relation::place(Origin {
        root: OriginRoot::ArgumentBrand(parameter),
        fields: vec![].into(),
    });
    let invalid = store.intern(invalid);
    assert!(matches!(
        instantiate_parameter_origins(&store, invalid, &arguments),
        Err(GraphError::UnboundOrigin(OriginRoot::ArgumentBrand(_)))
    ));
}

#[test]
fn sibling_substitution_preserves_unresolved_fields_and_nested_scopes() {
    use std::collections::BTreeMap;
    let store = TypeInterner::new(None);
    let origin = Origin {
        root: OriginRoot::Sibling(FieldId(1)),
        fields: vec![FieldId(4)].into(),
    };
    let mut field = scalar();
    field.brand = Brand {
        alternatives: vec![origin.clone()].into(),
    };
    field.modes.relation = Relation::place(origin.clone());
    let field = store.intern(field);
    let missing = instantiate_sibling_origins(&store, field, &BTreeMap::new()).unwrap();
    assert!(semantic_equal(&store, field, missing).unwrap());
    let parameter = ParameterRef::bound(ParamId(2));
    let fields = BTreeMap::from([(
        FieldId(1),
        Origin {
            root: OriginRoot::Parameter(parameter),
            fields: vec![FieldId(3)].into(),
        },
    )]);
    let actual = instantiate_sibling_origins(&store, field, &fields).unwrap();
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
    let callable = store.intern(function(field, vec![]));
    let actual = instantiate_sibling_origins(&store, callable, &fields).unwrap();
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
    let store = TypeInterner::new(None);
    let global = store.intern(Type {
        kind: TypeKind::Primitive(Primitive::String),
        ..scalar()
    });
    let mut row = store.get(global).unwrap().clone();
    row.modes.relation = Relation::environment();
    let local = store.intern(row);
    let mut lookup = |_: DefId| Ok::<_, crate::QueryError>(None);
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
    let store = TypeInterner::new(None);
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
    let result = store.intern(result);
    let callback = store.intern(function(result, vec![]));
    let source = Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(2))),
        fields: vec![].into(),
    };
    let actual = store.intern(scalar());
    let arguments = BTreeMap::from([(
        p,
        OriginArgument {
            identity: Origin {
                root: OriginRoot::Expression(owner(), ExprId(4)),
                fields: vec![].into(),
            },
            brand: Brand::default(),
            relation: Relation::place(source),
        },
    )]);
    let actuals = BTreeMap::from([(p, actual)]);
    let callback = instantiate_parameter_sources(&store, callback, &arguments, &actuals).unwrap();
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
    let inner = store.intern(inner);
    let callback = store.intern(function(inner, vec![]));
    let unchanged = instantiate_parameter_sources(&store, callback, &arguments, &actuals).unwrap();
    assert!(semantic_equal(&store, callback, unchanged).unwrap());
}

#[test]
fn placed_block_fields_inherit_the_owner_but_plain_and_related_fields_do_not() {
    let store = TypeInterner::new(None);
    let plain = store.intern(scalar());
    let block = store.intern(Type {
        kind: TypeKind::Primitive(Primitive::Bytes),
        ..scalar()
    });
    let explicit = store.intern(Type {
        modes: Modes {
            relation: Relation::environment(),
            ..Modes::global()
        },
        kind: TypeKind::Primitive(Primitive::Bytes),
        ..scalar()
    });
    let arena = Relation::place(Origin {
        root: OriginRoot::Local(owner(), LocalId(8)),
        fields: vec![].into(),
    });
    let owner = store.intern(Type {
        modes: Modes {
            relation: arena.clone(),
            ..Modes::global()
        },
        kind: TypeKind::Tuple(vec![block, plain, explicit].into()),
        ..scalar()
    });
    let lookup = &mut |_: hirsel_syntax::DefId| Ok::<_, crate::QueryError>(None);
    let field = placed_part_type(
        &store,
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
            &store,
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
            &store,
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
    let store = TypeInterner::new(None);
    let a = store.intern(scalar());
    let b = store.intern(scalar());
    let c = store.intern(scalar());
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
                definition: DefId::intern(&path("Pair")),
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
        let root = store.intern(Type { kind, ..scalar() });
        assert_eq!(store.row(root).kind.graph_edges(), edges);
    }
}

#[test]
fn identical_ordinals_in_distinct_binders_are_not_equal_or_interchangeable() {
    let types = TypeInterner::new(None);
    let outer = site(NameSite::Item);
    let inner = site(NameSite::Statement(StmtId(0)));
    let t = types.intern(Type {
        kind: TypeKind::Parameter(TypeParameterRef::body(outer, TypeParamId(0))),
        ..scalar()
    });
    let u = types.intern(Type {
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
    assert!(!semantic_equal(&types, t, u).unwrap());
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
    let actual = types.intern(scalar());
    let substituted = substitute_fresh(
        &types,
        t,
        Substitution {
            binder: Some(GenericBinder::Body(inner)),
            arguments: &[GenericArgument::Type(actual)],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(semantic_equal(&types, t, substituted).unwrap());
}

#[test]
fn identical_local_ordinals_in_two_items_are_not_equal_or_interchangeable() {
    // Two items, and two occurrences of one path, number their sites from
    // zero: the same local binder ordinal names a different binder in each.
    let types = TypeInterner::new(None);
    let first = LexicalOwner::first(&path("a")).site(NameSite::Statement(StmtId(0)));
    let second = LexicalOwner::first(&path("b")).site(NameSite::Statement(StmtId(0)));
    let duplicate = LexicalOwner {
        occurrence: 1,
        ..LexicalOwner::first(&path("a"))
    }
    .site(NameSite::Statement(StmtId(0)));
    let mut declarations = GenericDeclarations::default();
    let parameters: Vec<_> = [first, second, duplicate]
        .into_iter()
        .map(|scope| {
            declarations.insert(
                scope,
                vec![GenericParameter::Type {
                    name: "P".into(),
                    bounds: vec![Bound::Plain].into(),
                }],
            );
            types.intern(Type {
                kind: TypeKind::Parameter(TypeParameterRef::body(scope, TypeParamId(0))),
                ..scalar()
            })
        })
        .collect();
    let t = parameters[0];
    for &u in &parameters[1..] {
        assert!(!semantic_equal(&types, t, u).unwrap());
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
    }
    // Substituting the other item's binder leaves this item's parameter alone;
    // substituting its own replaces it.
    let actual = types.intern(scalar());
    for (scope, replaced) in [(second, false), (duplicate, false), (first, true)] {
        let substituted = substitute_fresh(
            &types,
            t,
            Substitution {
                binder: Some(GenericBinder::Body(scope)),
                arguments: &[GenericArgument::Type(actual)],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(semantic_equal(&types, t, substituted).unwrap(), !replaced);
        assert_eq!(
            types.get(substituted).unwrap().kind == TypeKind::Primitive(Primitive::U32),
            replaced
        );
    }
}

#[test]
fn a_callable_cycle_cannot_escape_detection_by_increasing_binder_depth() {
    let types = TypeInterner::new(None);
    let root = types.intern_graph(1, |ids| vec![function(ids[0], vec![])])[0];
    assert_eq!(types.validate(root), Err(GraphError::StructuralCycle(root)));
    assert!(!generic_references_valid(
        &types,
        root,
        &GenericDeclarations::default()
    ));
}

#[test]
fn normalization_keeps_enclosing_bounds_for_a_shared_nested_callable() {
    let types = TypeInterner::new(None);
    let captured = types.intern(Type {
        kind: TypeKind::Parameter(TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        }),
        ..scalar()
    });
    let inner = types.intern(function(captured, vec![]));
    let outer = |bounds| {
        function(
            inner,
            vec![GenericParameter::Type {
                name: "T".into(),
                bounds,
            }],
        )
    };
    let sendable = types.intern(outer(vec![Bound::Sendable].into()));
    let unconstrained = types.intern(outer(vec![].into()));
    let root = types.intern(Type {
        kind: TypeKind::Tuple(vec![sendable, unconstrained].into()),
        ..scalar()
    });
    let root =
        normalize_type_properties(&types, root, &GenericDeclarations::default(), &mut |_| {
            Ok::<_, QueryError>(None)
        })
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
fn semantic_equality_visits_a_wide_graph_linearly() {
    for count in [32, 128, 512] {
        let left = TypeInterner::new(None);
        let parts = (0..count)
            .map(|i| {
                left.intern(Type {
                    kind: TypeKind::Named {
                        definition: DefId::intern(&path(&format!("N{i}"))),
                        arguments: vec![].into(),
                    },
                    ..scalar()
                })
            })
            .collect();
        let root = left.intern(Type {
            kind: TypeKind::Tuple(parts),
            ..scalar()
        });
        // Rows sharing their parts compare by normal form: each row is
        // normalized once, and a repeated comparison normalizes nothing.
        let before = left.stats().sem_computed;
        assert!(semantic_equal(&left, root, root).unwrap());
        let other = left.intern(Type {
            kind: TypeKind::Box(root),
            ..scalar()
        });
        assert!(!semantic_equal(&left, root, other).unwrap());
        let work = left.stats().sem_computed - before;
        assert!(
            work <= (count + 2) as u64,
            "{count} components normalize {work} rows"
        );
        let before = left.stats().sem_computed;
        assert!(!semantic_equal(&left, root, other).unwrap());
        assert_eq!(left.stats().sem_computed, before);
    }
}

#[test]
fn graph_transport_in_place_preserves_external_transport_and_snapshot_rows() {
    let store = TypeInterner::new(None);
    let parameter = store.intern(Type {
        kind: TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
        ..scalar()
    });
    let nested = store.intern(function(parameter, vec![]));
    let root = store.intern(Type {
        kind: TypeKind::Tuple(vec![parameter, nested, parameter].into()),
        ..scalar()
    });
    let actual = store.intern(scalar());
    let arguments = [GenericArgument::Type(actual)];
    let roles = NominalRoles::default();
    let substitution = Substitution {
        binder: Some(GenericBinder::Bound(0)),
        arguments: &arguments,
        authorities: &[AuthoritySummary::inert()],
        roles: &roles,
    };
    let expected = substitute_fresh(&store, root, substitution).unwrap();
    let result = substitute(&store, &TypeMemo::new(), root, &substitution).unwrap();
    assert_eq!(expected, result, "substitution is exact: one result row");
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
fn origin_visiting_retains_rewriter_order_depth_and_cycle_policy() {
    let source = TypeInterner::new(None);
    let origin = Origin {
        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
        fields: vec![].into(),
    };
    let leaf = source.intern(Type {
        brand: Brand {
            alternatives: vec![origin.clone()].into(),
        },
        modes: Modes {
            relation: Relation::place(origin),
            ..Modes::global()
        },
        ..scalar()
    });
    let callable = source.intern(function(leaf, vec![]));
    let root = source.intern(Type {
        kind: TypeKind::Tuple(vec![leaf, callable, leaf].into()),
        ..scalar()
    });
    let mut expected = vec![];
    map_type_origins(&source.clone(), root, |origin, depth| {
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
    let root = source.intern_graph(1, |ids| {
        vec![Type {
            kind: TypeKind::Box(ids[0]),
            ..scalar()
        }]
    })[0];
    assert_eq!(
        visit_type_origins(&source, root, |_, _| Ok(())),
        Err(GraphError::StructuralCycle(root))
    );
}

fn plain_record(types: &TypeInterner) -> (hirsel_syntax::DefPath, SignatureRef) {
    let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
        package: hirsel_syntax::PackageId {
            name: "fixture".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: "Point".into(),
    });
    let x = types.intern(scalar());
    let representation = types.intern(Type {
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
        definition: DefId::intern(&path),
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
            serializable_check: None,
            layout: Layout::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
        }),
    };
    (path, std::sync::Arc::new(signature))
}

#[test]
fn state_selector_transport_preserves_nested_binders_and_field_paths() {
    let store = TypeInterner::new(None);
    let scalar = store.intern(scalar());
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
    let inner = store.intern(inner);
    let outer = store.intern(function(inner, vec![]));
    let mapped = bind_parameter_scopes(&store, outer, &[site(NameSite::Item)]).unwrap();
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
    assert!(semantic_equal(&store, mapped, mapped).unwrap());
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
    let store = TypeInterner::new(None);
    let element = store.intern(scalar());
    let value = store.intern(Type {
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
    let first = store.intern(row.clone());
    let bytes = encoding::canonical_type_encoding(&store, first).unwrap();
    let TypeKind::Function(f) = &mut row.kind else {
        panic!()
    };
    f.state_promises[0].selector.fields = vec![FieldId(1)].into();
    let field = store.intern(row.clone());
    assert_ne!(
        bytes,
        encoding::canonical_type_encoding(&store, field).unwrap()
    );
    let TypeKind::Function(f) = &mut row.kind else {
        panic!()
    };
    f.state_promises[0].selector.fields = vec![FieldId(0)].into();
    f.state_promises[0].advance_only = true;
    let flags = store.intern(row);
    assert_ne!(
        bytes,
        encoding::canonical_type_encoding(&store, flags).unwrap()
    );
}

#[test]
fn written_crossing_transport_never_proves_an_unproved_actual() {
    let source = TypeInterner::new(None);
    let parameter = source.intern(Type {
        kind: TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))),
        sendability_requirement: SendabilityRequirement::Sendable,
        modes: Modes {
            sendability: Sendability::Sendable,
            ..Modes::global()
        },
        ..scalar()
    });
    let root = source.intern(Type {
        kind: TypeKind::Tuple(vec![parameter].into()),
        ..scalar()
    });
    let target = std::sync::Arc::clone(&source);
    let actual = target.intern(Type {
        kind: TypeKind::Primitive(Primitive::Bytes),
        ..scalar()
    });
    let substituted = substitute_fresh(
        &target,
        root,
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
    assert!(semantic_equal(&target, substituted, substituted).unwrap());
    let normalized = normalize_type_properties(
        &target,
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
    let store = TypeInterner::new(None);
    let bytes = store.intern(Type {
        kind: TypeKind::Primitive(Primitive::Bytes),
        ..scalar()
    });
    let qualified = store.intern(Type {
        sendability_requirement: SendabilityRequirement::Sendable,
        ..store.get(bytes).unwrap().clone()
    });
    assert_ne!(
        encoding::canonical_type_encoding(&store, bytes).unwrap(),
        encoding::canonical_type_encoding(&store, qualified).unwrap()
    );
    let plain = store.intern(scalar());
    let qualified_plain = store.intern(Type {
        sendability_requirement: SendabilityRequirement::Sendable,
        ..scalar()
    });
    let normal =
        normalize_type_properties(&store, plain, &GenericDeclarations::default(), &mut |_| {
            Ok(None)
        })
        .unwrap();
    let qualified_normal = normalize_type_properties(
        &store,
        qualified_plain,
        &GenericDeclarations::default(),
        &mut |_| Ok(None),
    )
    .unwrap();
    assert!(semantic_equal(&store, normal, qualified_normal).unwrap());
}

#[test]
fn schema_opt_in_and_bound_change_public_fingerprints_independently() {
    let types = TypeInterner::new(None);
    let (_, original) = plain_record(&types);
    let before = encoding::fingerprint(&types, &original, &[]).unwrap();
    let mut opted = (*original).clone();
    let SignatureKind::Type(declaration) = &mut opted.kind else {
        unreachable!()
    };
    declaration.serializable = true;
    assert_ne!(before, encoding::fingerprint(&types, &opted, &[]).unwrap());
    let mut bounded = (*original).clone();
    let SignatureKind::Type(declaration) = &mut bounded.kind else {
        unreachable!()
    };
    declaration.generics.push(GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Serializable].into(),
    });
    assert_ne!(
        encoding::fingerprint(&types, &opted, &[]).unwrap(),
        encoding::fingerprint(&types, &bounded, &[]).unwrap()
    );
}

#[test]
fn private_capability_ceiling_is_not_a_public_signature_hash_input() {
    let types = TypeInterner::new(None);
    let (_, original) = plain_record(&types);
    let mut public = (*original).clone();
    public.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
    let SignatureKind::Type(declaration) = &mut public.kind else {
        unreachable!()
    };
    declaration.representation = None;
    declaration.capability = Some(CapabilityKind::Global);
    declaration.authority = AuthoritySummary::Known(
        vec![AuthorityTerm::Capability(public.definition.path().clone())].into(),
    );
    let public_hash = encoding::fingerprint(&types, &public, &[]).unwrap();
    let SignatureKind::Type(declaration) = &mut public.kind else {
        unreachable!()
    };
    declaration.implementation_authority = Some(AuthoritySummary::Unknown);
    assert_eq!(
        public_hash,
        encoding::fingerprint(&types, &public, &[]).unwrap()
    );
    public.metadata.visibility = hirsel_syntax::Visibility::Private;
    let private_hash = encoding::fingerprint(&types, &public, &[]).unwrap();
    let SignatureKind::Type(declaration) = &mut public.kind else {
        unreachable!()
    };
    declaration.implementation_authority = Some(AuthoritySummary::inert());
    assert_ne!(
        private_hash,
        encoding::fingerprint(&types, &public, &[]).unwrap()
    );
}

#[test]
fn cleanup_bound_instantiation_retains_exact_scopes_and_nested_binders() {
    let types = TypeInterner::new(None);
    let owner = site(NameSite::Item);
    let sibling = site(NameSite::Statement(StmtId(10)));
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
            types.intern(Type {
                drop_allocation: DropAllocation::DependsOn(parameters.clone().into()),
                ..scalar()
            })
        })
        .collect();
    let tuple = types.intern(Type {
        kind: TypeKind::Tuple(parts.clone().into()),
        ..scalar()
    });
    let root = types.intern(function(tuple, vec![unbounded]));
    let result = instantiate_cleanup_bounds(&types, root, &declarations).unwrap();
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
        serializable_check: None,
        layout: Layout::default(),
        distinct: false,
        resource: false,
        drop: None,
        fact: None,
        captures: None,
        destructor_allocation: Allocation::MayAllocate,
    };
    let types = TypeInterner::new(None);
    let mut public = Signature {
        definition: DefId::intern(&path("Hidden")),
        kind: SignatureKind::Type(declaration(None, None)),
        metadata: SignatureMetadata {
            visibility: hirsel_syntax::Visibility::PublicOpaque,
            ..Default::default()
        },
        fingerprint: Default::default(),
    };
    public.fingerprint = encoding::fingerprint(&types, &public, &[]).unwrap();
    let field = types.intern(scalar());
    let view = encoding::owner_view(
        &public,
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
    let types = TypeInterner::new(None);
    let result = types.intern(Type::unit());
    let TypeKind::Function(callable) = function(result, vec![]).kind else {
        unreachable!()
    };
    let mut signature = Signature {
        definition: DefId::intern(&path("helper")),
        metadata: SignatureMetadata::default(),
        fingerprint: Default::default(),
        kind: SignatureKind::Function(callable),
    };
    let original = signature.clone();
    let before = encoding::fingerprint(&types, &signature, &[]).unwrap();
    signature.metadata.track_caller = true;
    assert_ne!(
        before,
        encoding::fingerprint(&types, &signature, &[]).unwrap()
    );
    assert_eq!(original.kind, signature.kind);
}

#[test]
fn signature_types_names_each_directly_named_row_in_encoding_order() {
    let types = TypeInterner::new(None);
    let (_, record) = plain_record(&types);
    let SignatureKind::Type(declaration) = &record.kind else {
        unreachable!()
    };
    // The representation's field row is reached through it, not named.
    assert_eq!(
        encoding::signature_types(&types, &record),
        vec![declaration.representation.unwrap()]
    );
    let unit = types.intern(Type::unit());
    let scalar = types.intern(scalar());
    let TypeKind::Function(mut callable) = function(unit, vec![]).kind else {
        unreachable!()
    };
    callable.parameters = vec![Parameter {
        name: None,
        label: None,
        passing: PassingMode::Borrow,
        ty: scalar,
    }]
    .into();
    callable.error = Some(scalar);
    let signature = Signature {
        definition: DefId::intern(&path("f")),
        metadata: SignatureMetadata::default(),
        fingerprint: Default::default(),
        kind: SignatureKind::Function(callable),
    };
    let roots = encoding::signature_types(&types, &signature);
    assert!(
        roots.contains(&unit) && roots.contains(&scalar),
        "{roots:?}"
    );
    assert!(roots.iter().all(|id| types.contains(*id)));
}

#[test]
fn substitution_of_root_evidence_retains_unchanged_child_arrays() {
    fn storage(kind: &TypeKind) -> usize {
        match kind {
            TypeKind::Tuple(parts) => parts.storage_identity(),
            TypeKind::Record(fields) => fields.storage_identity(),
            TypeKind::Variant(variants) => variants.storage_identity(),
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                arguments.storage_identity()
            }
            TypeKind::Function(f) => f.parameters.storage_identity(),
            _ => unreachable!(),
        }
    }
    let t = TypeInterner::new(None);
    let leaf = t.intern(scalar());
    let mut callable = function(leaf, vec![]).kind;
    let TypeKind::Function(f) = &mut callable else {
        unreachable!()
    };
    f.parameters = vec![Parameter {
        name: Some("value".into()),
        label: None,
        passing: hirsel_syntax::PassingMode::Borrow,
        ty: leaf,
    }]
    .into();
    let kinds = [
        TypeKind::Tuple(vec![leaf; 64].into()),
        TypeKind::Record(
            vec![Field {
                name: "field".into(),
                ty: leaf,
                growable: false,
            }]
            .into(),
        ),
        TypeKind::Variant(
            vec![Variant {
                name: "Case".into(),
                fields: vec![(None, leaf)].into(),
            }]
            .into(),
        ),
        TypeKind::Named {
            definition: DefId::intern(&path("Shell")),
            arguments: vec![GenericArgument::Type(leaf)].into(),
        },
        TypeKind::Builtin {
            kind: BuiltinType::Inline,
            arguments: vec![
                GenericArgument::Type(leaf),
                GenericArgument::Size(Size::Constant(4)),
            ]
            .into(),
        },
        callable,
    ];
    for kind in kinds {
        let source = t.intern(Type {
            drop_allocation: DropAllocation::DependsOn(
                vec![TypeParameterRef::bound(TypeParamId(0))].into(),
            ),
            kind,
            ..scalar()
        });
        let old = t.row(source).clone();
        let result = substitute(
            &t,
            &TypeMemo::new(),
            source,
            &Substitution {
                binder: Some(GenericBinder::Bound(0)),
                arguments: &[GenericArgument::Type(leaf)],
                ..Default::default()
            },
        )
        .unwrap();
        assert_ne!(source, result, "root evidence must change");
        assert_eq!(t.row(result).drop_allocation, DropAllocation::NoAlloc);
        assert_eq!(t.row(result).kind, old.kind);
        assert_eq!(t.row(source), &old, "source snapshot remains immutable");
        assert_eq!(
            storage(&t.row(result).kind),
            storage(&old.kind),
            "changing root evidence must not copy unchanged child storage"
        );
    }
}

#[test]
fn closed_generic_transport_does_not_reintern_unchanged_subgraphs() {
    let t = TypeInterner::new(None);
    let mut root = t.intern(scalar());
    for _ in 0..128 {
        root = t.intern(Type::with_kind(TypeKind::Tuple(vec![root, root].into())));
    }
    let roles = NominalRoles {
        result_type: Some(path("Result")),
        ..Default::default()
    };
    let before = t.stats().hits;
    for _ in 0..5 {
        assert_eq!(
            substitute(
                &t,
                &TypeMemo::new(),
                root,
                &Substitution {
                    binder: Some(GenericBinder::Bound(0)),
                    roles: &roles,
                    ..Default::default()
                }
            ),
            Ok(root)
        );
    }
    assert_eq!(
        t.stats().hits,
        before,
        "closed transport must not rebuild or reintern every unchanged child"
    );
}

#[test]
fn substitution_pruning_matches_full_transport_for_binders_roles_and_errors() {
    let t = TypeInterner::new(None);
    let byte = t.intern(scalar());
    let parameter = t.intern(Type::with_kind(TypeKind::Parameter(
        TypeParameterRef::bound(TypeParamId(0)),
    )));
    let outer = t.intern(Type::with_kind(TypeKind::Parameter(
        TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        },
    )));
    let body = t.intern(Type::with_kind(TypeKind::Parameter(
        TypeParameterRef::body(site(NameSite::Item), TypeParamId(0)),
    )));
    let mut summary = scalar();
    summary.drop_allocation = DropAllocation::DependsOn(vec![].into());
    summary.modes.sendability = Sendability::DependsOn(vec![].into());
    let summary = t.intern(summary);
    let mut authority = scalar();
    authority.erased_captures = Some(ErasedCaptures::Known(AuthoritySummary::Known(
        vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
            TypeParamId(0),
        ))]
        .into(),
    )));
    let authority = t.intern(authority);
    let sized = t.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::Inline,
        arguments: vec![
            GenericArgument::Type(byte),
            GenericArgument::Size(Size::Parameter(SizeParameterRef::bound(SizeParamId(0)))),
        ]
        .into(),
    }));
    let malformed = t.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::Inline,
        arguments: vec![
            GenericArgument::Size(Size::Parameter(SizeParameterRef::bound(SizeParamId(0)))),
            GenericArgument::Type(TypeId::DANGLING),
        ]
        .into(),
    }));
    let result = t.intern(Type::with_kind(TypeKind::Named {
        definition: DefId::intern(&path("Result")),
        arguments: vec![GenericArgument::Type(byte), GenericArgument::Type(summary)].into(),
    }));
    let mut roots = vec![
        byte,
        parameter,
        outer,
        body,
        summary,
        authority,
        sized,
        malformed,
        result,
        TypeId::DANGLING,
    ];
    for child in [
        byte, parameter, outer, body, summary, authority, sized, result,
    ] {
        let callable = t.intern(function(child, vec![]));
        roots.extend([
            callable,
            t.intern(Type::with_kind(TypeKind::Tuple(
                vec![child, callable].into(),
            ))),
        ]);
    }
    roots.push(t.intern(Type::with_kind(TypeKind::Error)));
    roots.push(t.intern_graph(1, |ids| vec![Type::with_kind(TypeKind::Box(ids[0]))])[0]);
    for root in roots {
        for roles in [
            NominalRoles::default(),
            NominalRoles {
                result_type: Some(path("Result")),
                ..Default::default()
            },
        ] {
            for binder in [
                GenericBinder::Bound(0),
                GenericBinder::Bound(1),
                GenericBinder::Body(site(NameSite::Item)),
            ] {
                for arguments in [
                    vec![],
                    vec![GenericArgument::Type(byte)],
                    vec![GenericArgument::Size(Size::Constant(3))],
                ] {
                    let s = Substitution {
                        binder: Some(binder),
                        arguments: &arguments,
                        authorities: &[AuthoritySummary::inert()],
                        roles: &roles,
                    };
                    let pruned = substitute(&t, &TypeMemo::new(), root, &s);
                    let original = crate::parameters::with_full_rewrites(|| {
                        substitute(&t, &TypeMemo::new(), root, &s)
                    });
                    assert_eq!(
                        pruned, original,
                        "substitute {root:?} {binder:?} {arguments:?}"
                    );
                    let pruned = crate::graph::expand(&t, root, &arguments, &roles);
                    let original = crate::parameters::with_full_rewrites(|| {
                        crate::graph::expand(&t, root, &arguments, &roles)
                    });
                    assert_eq!(pruned, original, "expand {root:?} {arguments:?}");
                }
            }
        }
    }
}

#[test]
fn placed_payload_comparisons_preserve_the_owner_and_explicit_arena() {
    let store = TypeInterner::new(None);
    let bytes = store.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let place = |n| {
        Relation::place(Origin {
            root: OriginRoot::Local(owner(), LocalId(n)),
            fields: vec![].into(),
        })
    };
    let box_row = |part, relation| {
        let mut row = Type::with_kind(TypeKind::Box(part));
        row.modes.relation = relation;
        store.intern(row)
    };
    let a = place(1);
    let b = place(2);
    let placed_bytes = |relation| {
        let mut row = store.get(bytes).unwrap().clone();
        row.modes.relation = relation;
        store.intern(row)
    };
    let implicit = box_row(bytes, a.clone());
    let explicit = box_row(placed_bytes(a.clone()), a.clone());
    let global = box_row(bytes, Relation::global());
    let other = box_row(bytes, b.clone());
    let context = GenericDeclarations::signature(&[]);
    let mut lookup = |_: DefId| Ok::<_, QueryError>(None);
    let mut memo = ContextMemo::new(&store, &context, &mut lookup);
    assert!(memo.view(implicit, explicit, TypeRelation::Value).unwrap());
    assert!(!memo.view(global, explicit, TypeRelation::Value).unwrap());
    assert!(!memo.view(other, explicit, TypeRelation::Parameter).unwrap());
    let local_a = box_row(bytes, a.union(&Relation::environment()));
    let local_b = box_row(bytes, b.union(&Relation::environment()));
    assert!(
        !memo
            .view(local_a, local_b, TypeRelation::Parameter)
            .unwrap()
    );
    let local = box_row(bytes, Relation::environment());
    assert!(memo.view(local, local_a, TypeRelation::Parameter).unwrap());
    let nested_implicit = box_row(implicit, a.clone());
    let nested_explicit = box_row(explicit, a);
    assert!(
        memo.view(nested_implicit, nested_explicit, TypeRelation::Value)
            .unwrap()
    );
}

#[test]
fn an_unrelated_borrow_accepts_structural_payloads_in_the_actual_environment() {
    let store = TypeInterner::new(None);
    let text = store.intern(Type::with_kind(TypeKind::Primitive(Primitive::String)));
    let row = Type::with_kind(TypeKind::Tuple(vec![text, text].into()));
    let expected = store.intern(row.clone());
    let mut local = row;
    local.modes.relation = Relation::environment();
    let actual = store.intern(local);
    let context = GenericDeclarations::signature(&[]);
    let mut lookup = |_: DefId| Ok::<_, QueryError>(None);
    let mut memo = ContextMemo::new(&store, &context, &mut lookup);
    assert!(
        memo.view(expected, actual, TypeRelation::Parameter)
            .unwrap()
    );
    // Taking the same local aggregate into global storage still escapes.
    assert!(!memo.view(expected, actual, TypeRelation::Value).unwrap());
    assert!(
        reference::compatible_view_type_in(
            &store,
            expected,
            actual,
            TypeRelation::Parameter,
            &context,
            &mut lookup,
        )
        .unwrap()
    );
}

#[test]
fn appended_resolution_reads_share_prior_lookup_payloads_and_keep_ordered_bytes() {
    let mut resolution = Resolution::default();
    for i in 0..32 {
        std::sync::Arc::make_mut(&mut resolution.reads).push(
            DependencyRead::ProviderLookup {
                context: (FileContext {
                    package: path("owner").0.package,
                    modules: vec!["nested".repeat(16)],
                })
                .into(),
                package: None,
                segments: (vec![format!("missing_{i}").repeat(16)]).into(),
                output: (LookupOutput {
                    manifest: ContentHash::of_bytes(b"selected"),
                    result: Err(QueryError::NotImplemented),
                })
                .into(),
            }
            .shared(),
        );
    }
    let original_bytes = serde_json::to_vec(&resolution.reads).unwrap();
    let original_first = &resolution.reads[0] as *const SharedRead;
    let mut branch = resolution.clone();
    std::sync::Arc::make_mut(&mut branch.reads).push(
        DependencyRead::Signature {
            definition: hirsel_syntax::DefId::intern(&path("later")),
            fingerprint: None,
        }
        .shared(),
    );
    assert_eq!(resolution.reads.len(), 32);
    assert_eq!(branch.reads.len(), 33);
    assert_eq!(
        serde_json::to_vec(&resolution.reads).unwrap(),
        original_bytes
    );
    assert!(
        std::ptr::eq(original_first, &branch.reads[0]),
        "appending a phase read deep-copied the prior immutable lookup log"
    );
    let expected: Vec<_> = branch.reads.iter().cloned().collect();
    assert_eq!(
        serde_json::to_vec(&branch.reads).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    drop(resolution);
    assert!(std::ptr::eq(original_first, &branch.reads[0]));
    assert_eq!(
        serde_json::to_vec(&branch.reads).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
}
