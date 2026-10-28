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
            f.scope(f.owner),
            hirsel_types::ParamId(0),
        )),
        fields: vec![].into(),
    };
    let mut value_type = ty(TypeKind::Box(f.scalar));
    value_type.modes.relation =
        hirsel_types::Relation::environment().union(&hirsel_types::Relation::place(origin.clone()));
    value_type.brand.alternatives.push(origin);
    let value_type = session().intern(value_type);
    let parameter = f.parameter(0, value_type, U::Borrow);
    let read = f.read(parameter, value_type, U::Copy);
    let scalar = session().intern(ty(TypeKind::Primitive(P::U8)));
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
    let declared_box = session().intern(declared_box);
    f.register(hirsel_types::Signature {
        metadata: hirsel_types::SignatureMetadata::default(),
        definition: hirsel_syntax::DefId::intern(&path("main")),
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
    let mut h_result = session().get(value_type).unwrap().clone();
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
    let h_result = session().intern(h_result);
    producer
        .functions
        .push(hirsel_check_protocol::body::FunctionPlan {
            owner: f.owner.clone(),
            result: h_result,
        });
    f.plans = Plans::from_body(session(), &f.syntax.tree, &f.checked, &producer, read).unwrap();
    let definition = f.lower(read, "main", value_type).unwrap();
    assert_no_new_rows(&f, &definition);
    // A different declaring site with the same declaration ordinal cannot
    // silently resolve to this function's Parameter(0): neither another site
    // of this item nor the same site of another item.
    for (other, message) in [
        (
            f.scope(NameSite::Expression(ExprId(99))),
            "missing scoped binding",
        ),
        (
            LexicalOwner::first(&path("other")).site(f.owner),
            "another definition's parameter",
        ),
    ] {
        let checked = f.checked.clone();
        let wrong = hirsel_types::map_parameter_references(session(), value_type, |p, _| {
            Ok(match p {
                hirsel_types::ParameterRef::Body { parameter, .. } => {
                    hirsel_types::ParameterRef::body(other, parameter)
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
        let error = f.lower(read, "main", value_type).unwrap_err().message;
        assert!(error.contains(message), "{error}");
        f.checked = checked;
    }
}

#[test]
fn body_adapter_preserves_producer_selected_propagation_tags() {
    let mut f = Fixture::new(P::U8);
    let source_error = f.type_(TypeKind::Primitive(P::U16));
    let source_result = f.type_(TypeKind::Named {
        definition: hirsel_syntax::DefId::intern(&path("SourceResult")),
        arguments: vec![
            hirsel_types::GenericArgument::Type(f.scalar),
            hirsel_types::GenericArgument::Type(source_error),
        ]
        .into(),
    });
    let target_result = f.type_(TypeKind::Named {
        definition: hirsel_syntax::DefId::intern(&path("TargetResult")),
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
        mapper_result: None,
        success_variant: hirsel_types::VariantId(1),
        error_variant: hirsel_types::VariantId(0),
        propagated_error_variant: hirsel_types::VariantId(2),
        error_type: f.scalar,
        propagated_result: target_result,
    });
    let plans =
        Plans::from_body(session(), &f.syntax.tree, &f.checked, &producer, expression).unwrap();
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
    let rows = [(f.owner, hirsel_types::TypeId::DANGLING)];
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
                context: hirsel_types::CaptureContext::Scoped,
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
    let mut part = session().get(f.scalar).unwrap().clone();
    part.modes.relation = Relation::environment();
    let refined = session().intern(part);
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
    let output = Plans::from_body(session(), &f.syntax.tree, &f.checked, &producer, root).unwrap();
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
        Plans::from_body(session(), &f.syntax.tree, &f.checked, &producer, root)
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
                    context: hirsel_types::CaptureContext::Scoped,
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
        let input_rows = checked_rows(&f).len() as u64;
        let before = hirsel_intern::work::read(hirsel_intern::work::Counter::InternedTypeRows);
        let definition = lower(f.input(root, "main", &frames)).unwrap();
        let interned =
            hirsel_intern::work::read(hirsel_intern::work::Counter::InternedTypeRows) - before;
        assert_eq!(definition.bodies.len(), count + 1);
        assert!(
            interned <= input_rows,
            "{count} closures interned {interned} rows from {input_rows} input rows"
        );
        assert_no_new_rows(&f, &definition);
    }
}

/// Every row the fixture's checked tables and frames reach.
fn checked_rows(f: &Fixture) -> std::collections::HashSet<hirsel_types::TypeId> {
    let roots = f
        .checked
        .expressions
        .iter()
        .map(|e| e.ty)
        .chain(f.checked.bindings.iter().map(|b| b.ty))
        .chain(f.checked.patterns.iter().map(|p| p.ty))
        .chain(f.frames.iter().map(|(_, t)| *t))
        .chain([f.unit, f.boolean, f.scalar]);
    hirsel_types::closure(session(), roots)
        .iter()
        .copied()
        .collect()
}
/// Lowering transports checked rows: its type table adds none of its own.
fn assert_no_new_rows(f: &Fixture, definition: &Definition) {
    let known = checked_rows(f);
    for t in definition.types.iter() {
        assert!(known.contains(t), "lowering introduced {t:?}");
    }
}

/// Static companion to the four-engine result_propagation_runtime_26 histories:
/// source and destination role order vary independently, with V checking each
/// lowered body. The existing adapter control separately preserves producer tags.
#[test]
fn propagation_role_orders_lower_once_26() {
    use hirsel_types::{Allocation, AuthoritySummary, Signature, SignatureKind, TypeDefinition};
    for source_error in [0, 1] {
        for target_error in [0, 1] {
            let mut f = Fixture::new(P::U8);
            let mut carrier = |name: &str, error: usize| {
                let representation = f.type_(TypeKind::Variant(
                    (0..2)
                        .map(|index| hirsel_types::Variant {
                            name: if index == error { "Bad" } else { "Good" }.into(),
                            fields: vec![(None, f.scalar)].into(),
                        })
                        .collect::<Vec<_>>()
                        .into(),
                ));
                f.register(Signature {
                    definition: DefId::intern(&path(name)),
                    metadata: Default::default(),
                    fingerprint: ContentHash::default(),
                    kind: SignatureKind::Type(Box::new(TypeDefinition {
                        loop_source: None,
                        format: None,
                        destructor_allocation: Allocation::NoAlloc,
                        generics: vec![],
                        representation: Some(representation),
                        bounds: vec![],
                        authority: AuthoritySummary::inert(),
                        capability: None,
                        implementation_authority: None,
                        serializable: false,
                        serializable_check: None,
                        layout: Default::default(),
                        distinct: false,
                        resource: false,
                        drop: None,
                        fact: None,
                        captures: None,
                    })),
                });
                f.type_(TypeKind::Named {
                    definition: DefId::intern(&path(name)),
                    arguments: vec![].into(),
                })
            };
            let source = carrier("Source", source_error);
            let target = carrier("Target", target_error);
            let operand = f.named_call("operand", vec![], vec![], source, 1);
            let propagation = f.expr(
                ExprKind::Try {
                    value: operand,
                    map_error: None,
                },
                f.scalar,
                U::Take,
                2,
            );
            f.plans.propagation.push((
                propagation,
                TryPlan {
                    success_variant: 1 - source_error as u32,
                    error_variant: source_error as u32,
                    success_type: f.scalar,
                    error_type: f.scalar,
                    result_constructor: Constructor::Variant {
                        definition: path("Target"),
                        variant: target_error as u32,
                    },
                    mapper: None,
                },
            ));
            let statement = f.stmt(StmtKind::Expr(propagation), 2);
            let finish = f.named_call("finish", vec![], vec![], target, 3);
            let body = f.block(vec![statement], finish);
            let definition = f.lower(body, "main", target).unwrap();
            let body = &definition.bodies[0];
            let operands = body
                .blocks
                .iter()
                .filter(|block| {
                    matches!(
                        &block.terminator,
                        Terminator::Call { callee: Callee::Definition { path: callee, .. }, .. }
                            if *callee == path("operand")
                    )
                })
                .count();
            assert_eq!(operands, 1);
            let switch = body
                .blocks
                .iter()
                .find_map(|block| match &block.terminator {
                    Terminator::Switch { arms, .. } => Some(arms),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                switch.iter().map(|(tag, _)| *tag).collect::<Vec<_>>(),
                [1 - source_error as u32, source_error as u32]
            );
            assert!(body.rvalues.iter().any(|value| matches!(value,
                Rvalue::Construct { constructor: Constructor::Variant { definition, variant }, .. }
                    if *definition == path("Target") && *variant == target_error as u32
            )));
        }
    }
}

#[test]
fn direct_literal_uses_the_checked_call_parameter_multiplicity() {
    for once in [false, true] {
        let mut f = Fixture::new(P::U8);
        let body = f.integer(9, 1);
        let many = f.type_(TypeKind::Function(function(vec![], f.scalar)));
        let mut expected = session().get(many).unwrap().clone();
        if once {
            expected.modes.multiplicity = hirsel_types::Multiplicity::Once;
        }
        let expected = session().intern(expected);
        let literal = f.expr(
            ExprKind::Closure {
                signature: FunctionSyntax::default(),
                body,
            },
            many,
            U::Take,
            3,
        );
        f.closure(
            hirsel_types::ClosureInfo {
                context: hirsel_types::CaptureContext::Scoped,
                result_evidence: None,
                expression: literal,
                ty: many,
                captures: vec![],
            },
            f.scalar,
            f.owner,
        );
        let invoke = f.named_call(
            "invoke",
            vec![literal],
            vec![(expected, U::Borrow)],
            f.scalar,
            4,
        );
        let lowered = f.lower(invoke, "main", f.scalar).unwrap();
        let parent = &lowered.bodies[0];
        let callable = parent
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|inst| {
                let InstructionKind::Assign { destination, value } = inst.kind else {
                    return None;
                };
                matches!(parent.rvalues[value.0 as usize], Rvalue::Closure { .. })
                    .then(|| parent.places[destination.0 as usize].ty)
            })
            .expect("the call supplies its original literal");
        assert_eq!(
            session().get(callable).unwrap().modes.multiplicity,
            if once {
                hirsel_types::Multiplicity::Once
            } else {
                hirsel_types::Multiplicity::Many
            }
        );
    }
}
