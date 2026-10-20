use super::support::*;
use super::*;
use hirsel_syntax::*;
use hirsel_tir::*;
use hirsel_types::Relation;
use hirsel_types::{Primitive as P, TypeKind, UseKind as U};

#[test]
fn missing_mode_decisions_and_recovery_nodes_fail_closed() {
    let mut f = Fixture::new(P::U8);
    let e = f.integer(1, 1);
    f.checked.expressions.clear();
    assert!(
        f.lower(e, "main", f.scalar)
            .unwrap_err()
            .message
            .contains("missing expression")
    );
    for kind in [
        ExprKind::Error,
        ExprKind::Heap,
        ExprKind::Result,
        ExprKind::Before(e),
        ExprKind::Comptime(e),
        ExprKind::Unary {
            op: UnaryOp::Unique,
            value: e,
        },
        ExprKind::Binary {
            op: BinaryOp::Range,
            left: e,
            right: e,
        },
    ] {
        let e = f.expr(kind, f.scalar, U::Take, 1);
        assert!(f.lower(e, "main", f.scalar).is_err());
    }
}
#[test]
fn snapshots_cover_core_constructs_and_are_repeatable() {
    let mut f = Fixture::new(P::U8);
    let a = f.integer(3, 1);
    let b = f.integer(1, 2);
    let n = f.binary(BinaryOp::Add, a, b, 3);
    let (s, binding) = f.let_(n);
    let c = f.bool_(true);
    let yes = f.read(binding, f.scalar, U::Copy);
    let no = f.integer(0, 1);
    let e = f.expr(
        ExprKind::If {
            condition: c,
            then_branch: yes,
            else_branch: Some(no),
        },
        f.scalar,
        U::Take,
        4,
    );
    let e = f.block(vec![s], e);
    let d = f.lower(e, "main", f.scalar).unwrap();
    let printed = print::definition(&d);
    assert_eq!(
        printed,
        print::definition(&f.lower(e, "main", f.scalar).unwrap())
    );
    insta::assert_snapshot!("arithmetic_binding_branch_cleanup", printed);
}

#[test]
fn generic_call_printing_does_not_expose_type_provenance() {
    fn snapshot() -> String {
        let mut f = Fixture::new(P::U8);
        let n = f.integer(3, 1);
        let e = f.named_call("generic", vec![n], vec![(f.scalar, U::Take)], f.scalar, 1);
        if let hirsel_types::SignatureKind::Function(function) =
            &mut f.declarations.get_mut(&path("generic")).unwrap().kind
        {
            function
                .generics
                .push(hirsel_types::GenericParameter::Type {
                    name: "T".into(),
                    bounds: vec![].into(),
                });
        }
        f.checked
            .calls
            .iter_mut()
            .find(|r| r.expression == e)
            .unwrap()
            .generic_arguments = vec![hirsel_types::GenericArgument::Type(f.scalar)];
        f.checked.instantiations.push(hirsel_types::Instantiation {
            expression: e,
            callee: path("generic"),
            arguments: vec![hirsel_types::GenericArgument::Type(f.scalar)],
        });
        print::definition(&f.lower(e, "main", f.scalar).unwrap())
    }
    let a = snapshot();
    assert_eq!(a, snapshot());
    assert!(!a.contains("provenance"));
    insta::assert_snapshot!("generic_call", a);
}
#[test]
fn freshly_computed_borrow_argument_remains_a_caller_owner() {
    let mut f = Fixture::new(P::String);
    let value = f.expr(
        ExprKind::Literal(Literal::String("\"payload\"".into())),
        f.scalar,
        U::Take,
        1,
    );
    f.checked
        .expressions
        .iter_mut()
        .find(|r| r.expression == value)
        .unwrap()
        .usage = U::Borrow;
    let e = f.named_call(
        "borrow",
        vec![value],
        vec![(f.scalar, U::Borrow)],
        f.unit,
        1,
    );
    let d = f.lower(e, "main", f.unit).unwrap();
    let body = &d.bodies[0];
    let call = body
        .blocks
        .iter()
        .find_map(|b| {
            if let Terminator::Call {
                arguments,
                on_failure,
                ..
            } = &b.terminator
            {
                Some((arguments, *on_failure))
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(call.0[0].value.usage, U::Borrow);
    assert!(
        body.cleanup_actions(call.1)
            .any(|a| matches!(a,CleanupAction::Drop{place,..} if body.places[place.0 as usize].root == body.places[call.0[0].value.place.0 as usize].root && body.places[place.0 as usize].projection == body.places[call.0[0].value.place.0 as usize].projection))
    );
}

#[test]
fn arena_parameter_origins_resolve_their_declaring_scope_without_retyping() {
    let mut f = Fixture::new(P::U8);
    let origin = hirsel_types::Origin {
        root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::body(
            f.owner,
            hirsel_types::ParamId(0),
        )),
        fields: vec![].into(),
    };
    let mut value_type = ty(TypeKind::Box(f.scalar));
    value_type.modes.relation =
        hirsel_types::Relation::environment().union(&hirsel_types::Relation::place(origin.clone()));
    value_type.brand.alternatives.push(origin);
    let value_type = f.checked.types.push(value_type);
    let parameter = f.parameter(0, value_type, U::Borrow);
    let read = f.read(parameter, value_type, U::Copy);
    let mut declaration_types = hirsel_types::TypeStore::default();
    let scalar = declaration_types.push(ty(TypeKind::Primitive(P::U8)));
    let mut declared_box = ty(TypeKind::Box(scalar));
    let declared_origin = hirsel_types::Origin {
        root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::bound(
            hirsel_types::ParamId(0),
        )),
        fields: vec![].into(),
    };
    declared_box.modes.relation = hirsel_types::Relation::environment()
        .union(&hirsel_types::Relation::place(declared_origin.clone()));
    declared_box.brand.alternatives.push(declared_origin);
    let declared_box = declaration_types.push(declared_box);
    f.register(hirsel_types::Signature {
        metadata: hirsel_types::SignatureMetadata::default(),
        definition: path("main"),
        types: declaration_types,
        kind: hirsel_types::SignatureKind::Function(hirsel_types::FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: vec![hirsel_types::Parameter {
                name: None,
                label: None,
                passing: hirsel_types::PassingMode::Borrow,
                ty: declared_box,
            }]
            .into(),
            result: declared_box,
            error: None,
            allocation: hirsel_types::Allocation::NoAlloc,
            authority: hirsel_types::AuthoritySummary::inert(),
            uses: hirsel_types::Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }),
        fingerprint: ContentHash::default(),
    });
    let mut producer = hirsel_check_protocol::body::Plans::default();
    let mut h_result = f.checked.types.get(value_type).unwrap().clone();
    for origin in &mut h_result.brand.alternatives {
        origin.root = hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::bound(
            hirsel_types::ParamId(0),
        ));
    }
    h_result.modes.relation = h_result.modes.relation.map(|r| match r {
        hirsel_types::Reach::Place(origin) => {
            vec![hirsel_types::Reach::Place(hirsel_types::Origin {
                root: hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::bound(
                    hirsel_types::ParamId(0),
                )),
                fields: origin.fields.clone(),
            })]
        }
        r => vec![r.clone()],
    });
    let h_result = f.checked.types.push(h_result);
    producer
        .functions
        .push(hirsel_check_protocol::body::FunctionPlan {
            owner: f.owner.clone(),
            result: h_result,
        });
    f.plans = Plans::from_body(&f.tree, &f.checked, &producer, read).unwrap();
    let definition = f.lower(read, "main", value_type).unwrap();
    assert_eq!(definition.types, f.checked.types);
    // A different declaring owner with the same declaration ordinal cannot
    // silently resolve to this function's Parameter(0).
    let wrong = hirsel_types::map_parameter_references(&mut f.checked.types, value_type, |p, _| {
        Ok(match p {
            hirsel_types::ParameterRef::Body { parameter, .. } => {
                hirsel_types::ParameterRef::body(NameSite::Expression(ExprId(99)), parameter)
            }
            p => p,
        })
    })
    .unwrap();
    for b in &mut f.checked.bindings {
        if b.ty == value_type {
            b.ty = wrong;
        }
    }
    for e in &mut f.checked.expressions {
        if e.ty == value_type {
            e.ty = wrong;
        }
    }
    assert!(
        f.lower(read, "main", value_type)
            .unwrap_err()
            .message
            .contains("missing scoped binding")
    );
}

#[test]
fn body_adapter_preserves_producer_selected_propagation_tags() {
    let mut f = Fixture::new(P::U8);
    let source_error = f.type_(TypeKind::Primitive(P::U16));
    let source_result = f.type_(TypeKind::Named {
        definition: path("SourceResult"),
        arguments: vec![
            hirsel_types::GenericArgument::Type(f.scalar),
            hirsel_types::GenericArgument::Type(source_error),
        ]
        .into(),
    });
    let target_result = f.type_(TypeKind::Named {
        definition: path("TargetResult"),
        arguments: vec![
            hirsel_types::GenericArgument::Type(f.scalar),
            hirsel_types::GenericArgument::Type(f.scalar),
        ]
        .into(),
    });
    let value = f.named_call("SourceResult", vec![], vec![], source_result, 1);
    let expression = f.expr(
        ExprKind::Try {
            value,
            map_error: None,
        },
        f.scalar,
        U::Take,
        1,
    );
    let mut producer = hirsel_check_protocol::body::Plans::default();
    producer.tries.push(hirsel_check_protocol::body::TryPlan {
        expression,
        result: path("SourceResult"),
        mapper: None,
        mapper_passing: None,
        success_variant: hirsel_types::VariantId(1),
        error_variant: hirsel_types::VariantId(0),
        propagated_error_variant: hirsel_types::VariantId(2),
        error_type: f.scalar,
        propagated_result: target_result,
    });
    let plans = Plans::from_body(&f.tree, &f.checked, &producer, expression).unwrap();
    let propagation = &plans.propagation[0].1;
    assert_eq!(propagation.success_type, f.scalar);
    assert_eq!(propagation.error_type, source_error);
    assert_eq!(
        (propagation.success_variant, propagation.error_variant),
        (1, 0)
    );
    assert_eq!(
        propagation.result_constructor,
        Constructor::Variant {
            definition: path("TargetResult"),
            variant: 2
        }
    );
}

#[test]
fn materialization_cannot_change_the_checked_root_type() {
    let mut f = Fixture::new(P::U32);
    let value = f.integer(7, 1);
    let expression = f.expr(ExprKind::Comptime(value), f.scalar, U::Take, 1);
    f.plans.materialized.push((
        expression,
        Materialized {
            types: f.checked.types.clone(),
            root: ValueNode {
                ty: f.boolean,
                kind: ValueKind::Constant(Constant::Bool(true)),
            },
        },
    ));
    assert!(
        f.lower(expression, "main", f.scalar)
            .unwrap_err()
            .message
            .contains("root disagrees")
    );
}

#[test]
fn production_call_consumption_requires_a_complete_unambiguous_ownership_row() {
    for duplicate in [false, true] {
        let mut f = Fixture::new(P::U8);
        let argument = f.integer(1, 1);
        let call = f.named_call(
            "consume",
            vec![argument],
            vec![(f.scalar, U::Take)],
            f.unit,
            1,
        );
        f.ownership.call_arguments = if duplicate {
            vec![(call, argument, U::Copy), (call, argument, U::Borrow)]
        } else {
            vec![]
        };
        let error = f.lower(call, "main", f.unit).unwrap_err();
        assert!(
            error.message.contains("one ownership consumer use"),
            "{error:?}"
        );
    }
}

#[test]
fn every_call_requires_one_producer_plan_even_for_positional_names() {
    for duplicate in [false, true] {
        let mut f = Fixture::new(P::U8);
        let argument = f.integer(1, 3);
        let call = f.named_call(
            "consume",
            vec![argument],
            vec![(f.scalar, U::Take)],
            f.unit,
            3,
        );
        if duplicate {
            f.checked.calls.push(f.checked.calls[0].clone());
        } else {
            f.checked.calls.clear();
        }
        let error = f.lower(call, "main", f.unit).unwrap_err();
        assert_eq!(error.site, site(3));
        assert_eq!(
            error.message,
            if duplicate {
                "duplicate checked call plan"
            } else {
                "missing checked call plan"
            }
        );
    }
}

#[test]
fn formal_results_have_no_empty_table_or_invalid_type_fallback() {
    let mut f = Fixture::new(P::U8);
    let root = f.integer(1, 1);
    assert_eq!(
        lower(f.input(root, "main", &[])).unwrap_err().message,
        "missing J formal frame result"
    );
    let mut extended = f.checked.types.clone();
    let invalid = extended.push(ty(TypeKind::Primitive(P::U8)));
    let rows = [(f.owner, invalid)];
    assert!(lower(f.input(root, "main", &rows)).is_err());
}

#[test]
fn replacement_requires_one_owned_content_type() {
    for duplicate in [false, true] {
        let mut f = Fixture::new(P::U8);
        let initial = f.integer(1, 1);
        let (binding, name) = f.let_(initial);
        let place = f.read(name, f.scalar, U::Mut);
        let value = f.integer(2, 3);
        let assignment = f.assign(place, AssignOp(None), value, f.scalar, 3);
        if duplicate {
            f.ownership.replacements.push((place, f.scalar, Some(true)));
        } else {
            f.ownership.replacements.clear();
        }
        let unit = f.unit_();
        let root = f.block(vec![binding, assignment], unit);
        let error = f.lower(root, "main", f.unit).unwrap_err();
        assert_eq!(error.site, site(3));
        assert_eq!(
            error.message,
            if duplicate {
                "duplicate owned replacement content type"
            } else {
                "missing owned replacement content type"
            }
        );
    }
}

#[test]
fn captures_require_one_scoped_owner_row() {
    for duplicate in [false, true] {
        let mut f = Fixture::new(P::U8);
        let value = f.integer(9, 1);
        let (statement, binding) = f.let_(value);
        let body = f.read(binding.clone(), f.scalar, U::Copy);
        let function = f.type_(TypeKind::Function(function(vec![], f.scalar)));
        let closure = f.expr(
            ExprKind::Closure {
                signature: FunctionSyntax::default(),
                body,
            },
            function,
            U::Take,
            3,
        );
        f.closure(
            hirsel_types::ClosureInfo {
                result_evidence: None,
                expression: closure,
                ty: function,
                captures: vec![hirsel_types::Capture {
                    binding,
                    usage: U::Copy,
                    ty: f.scalar,
                }],
            },
            f.scalar,
            f.owner,
        );
        if duplicate {
            f.ownership.captures.push(f.ownership.captures[0].clone());
        } else {
            f.ownership.captures.clear();
        }
        let root = f.block(vec![statement], closure);
        let error = f.lower(root, "main", function).unwrap_err();
        assert_eq!(error.site, site(3));
        assert_eq!(
            error.message,
            if duplicate {
                "duplicate scoped capture owner"
            } else {
                "missing scoped capture owner"
            }
        );
    }
}

#[test]
fn indexed_loop_adapter_consumes_the_mode_checked_pattern_type() {
    let mut f = Fixture::new(P::String);
    let mut part = f.checked.types.get(f.scalar).unwrap().clone();
    part.modes.relation = Relation::environment();
    let refined = f.checked.types.push(part);
    let sequence = f.type_(TypeKind::Builtin {
        kind: hirsel_types::BuiltinType::Array,
        arguments: vec![hirsel_types::GenericArgument::Type(f.scalar)].into(),
    });
    let parameter = f.parameter(0, sequence, U::Borrow);
    let source = f.read(parameter, sequence, U::Borrow);
    let (pattern, _) = f.binding(refined, U::Borrow);
    let body = f.unit_();
    let statement = f.stmt(
        StmtKind::For {
            pattern,
            source,
            body,
            label: None,
        },
        1,
    );
    let tail = f.unit_();
    let root = f.block(vec![statement], tail);
    let index_type = f.type_(TypeKind::Primitive(P::U64));
    let producer = hirsel_check_protocol::body::Plans {
        loops: vec![hirsel_check_protocol::body::ForPlan {
            protocol: hirsel_check_protocol::body::ForProtocol::Sequence(
                hirsel_types::BuiltinType::Array,
            ),
            kind: hirsel_check_protocol::body::ForPlanKind::Direct {
                statement,
                execution: hirsel_check_protocol::body::ForExecution::Indexed {
                    length: hirsel_check_protocol::body::LoopLength::Inline {
                        size: hirsel_types::Size::Constant(0),
                        result: index_type,
                    },
                    read: hirsel_check_protocol::body::LoopRead::Index { result: f.scalar },
                },
            },
        }],
        ..Default::default()
    };
    let output = Plans::from_body(&f.tree, &f.checked, &producer, root).unwrap();
    let hirsel_check_protocol::body::ForExecution::Indexed {
        read: hirsel_check_protocol::body::LoopRead::Index { result },
        ..
    } = &output.protocols[0].1
    else {
        panic!()
    };
    assert_eq!(*result, refined);
    f.checked.patterns.clear();
    assert!(
        Plans::from_body(&f.tree, &f.checked, &producer, root)
            .unwrap_err()
            .message
            .contains("pattern")
    );
}

#[test]
fn closure_bodies_copy_at_most_linear_type_rows() {
    // Each closure adds a distinct callable row, reproducing a growing body store.
    for count in [16, 64, 256] {
        let mut f = Fixture::new(P::U8);
        let mut statements = vec![];
        for _ in 0..count {
            let body = f.integer(1, 1);
            let callable = f.type_(TypeKind::Function(function(vec![], f.scalar)));
            let closure = f.expr(
                ExprKind::Closure {
                    signature: FunctionSyntax::default(),
                    body,
                },
                callable,
                U::Take,
                1,
            );
            f.closure(
                hirsel_types::ClosureInfo {
                    result_evidence: None,
                    expression: closure,
                    ty: callable,
                    captures: vec![],
                },
                f.scalar,
                f.owner,
            );
            statements.push(f.let_(closure).0);
        }
        let tail = f.integer(2, 1);
        let root = f.block(statements, tail);
        let mut frames = vec![(f.owner, f.scalar)];
        frames.extend(f.frames.iter().copied());
        let before = hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows);
        let definition = lower(f.input(root, "main", &frames)).unwrap();
        let copied =
            hirsel_intern::work::read(hirsel_intern::work::Counter::CopiedTypeRows) - before;
        assert_eq!(definition.bodies.len(), count + 1);
        assert!(
            copied <= f.checked.types.len() as u64,
            "{count} closures copied {copied} rows from {} input rows",
            f.checked.types.len()
        );
        assert_eq!(definition.types, f.checked.types);
    }
}
