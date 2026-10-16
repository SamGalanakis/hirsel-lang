use super::support::*;
use super::*;
use hirsel_interp::{Outcome, TraceEvent, Value};
use hirsel_syntax::MatchArm;
use hirsel_syntax::*;
use hirsel_tir::*;
use hirsel_types::{GuardedPatternUse, Relation};
use hirsel_types::{PatternDecision, PatternDecisionKind, Primitive as P, TypeKind, UseKind as U};

#[test]
fn unary_and_every_eager_binary_operator() {
    for op in [
        BinaryOp::Add,
        BinaryOp::Sub,
        BinaryOp::Mul,
        BinaryOp::Div,
        BinaryOp::Rem,
        BinaryOp::Shl,
        BinaryOp::Shr,
        BinaryOp::BitAnd,
        BinaryOp::BitOr,
        BinaryOp::BitXor,
        BinaryOp::Eq,
        BinaryOp::Ne,
        BinaryOp::Lt,
        BinaryOp::Le,
        BinaryOp::Gt,
        BinaryOp::Ge,
    ] {
        let mut f = Fixture::new(P::U8);
        let a = f.integer(4, 1);
        let b = f.integer(1, 1);
        let e = f.binary(op, a, b, 1);
        let result = if matches!(
            op,
            BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
        ) {
            f.boolean
        } else {
            f.scalar
        };
        execute(vec![f.lower(e, "main", result).unwrap()], vec![]);
    }
    for op in [UnaryOp::Neg, UnaryOp::Not, UnaryOp::BitNot] {
        let mut f = Fixture::new(if op == UnaryOp::Not { P::Bool } else { P::I8 });
        let e = if op == UnaryOp::Not {
            f.bool_(true)
        } else {
            f.integer(1, 1)
        };
        let e = f.expr(ExprKind::Unary { op, value: e }, f.scalar, U::Take, 1);
        execute(vec![f.lower(e, "main", f.scalar).unwrap()], vec![]);
    }
}

#[test]
fn failing_later_argument_cleans_earlier_temporaries_before_named_locals() {
    let mut f = Fixture::new(P::U8);
    let string = f.type_(TypeKind::Primitive(P::String));
    let n = f.expr(
        ExprKind::Literal(Literal::String("\"payload\"".into())),
        string,
        U::Take,
        1,
    );
    let (s, _) = f.let_(n);
    let a = f.expr(
        ExprKind::Literal(Literal::String("\"payload\"".into())),
        string,
        U::Take,
        2,
    );
    let x = f.integer(255, 3);
    let one = f.integer(1, 3);
    let bad = f.binary(BinaryOp::Add, x, one, 7);
    let e = f.named_call(
        "never_entered",
        vec![a, bad],
        vec![(string, U::Take), (f.scalar, U::Take)],
        f.scalar,
        9,
    );
    let e = f.block(vec![s], e);
    let d = f.lower(e, "main", f.scalar).unwrap();
    let execution = execute(vec![d.clone()], vec![]);
    let Outcome::Failed(failure) = execution.outcome else {
        panic!()
    };
    assert_eq!(failure.line, 7);
    assert!(
        !execution
            .trace
            .iter()
            .any(|t| matches!(t,TraceEvent::Enter{path,..} if path.0.name=="never_entered"))
    );
    let cleanup = execution
        .trace
        .iter()
        .filter_map(|t| {
            if let TraceEvent::Cleanup {
                action: CleanupAction::Drop { place, .. },
                ..
            } = t
            {
                Some(
                    d.bodies[0].locals[d.bodies[0].places[place.0 as usize].root.0 as usize]
                        .storage,
                )
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let first_local = cleanup.iter().position(|s| *s == Storage::Local).unwrap();
    assert!(
        cleanup[..first_local]
            .iter()
            .all(|s| *s == Storage::Temporary)
    );
}

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
fn tuple_record_and_box_layout_snapshots() {
    for construct in ["tuple", "record", "box"] {
        let mut f = Fixture::new(P::U8);
        let a = f.integer(3, 1);
        let b = f.integer(8, 2);
        let ty = f.type_(if construct == "box" {
            TypeKind::Box(f.scalar)
        } else if construct == "record" {
            TypeKind::Record(
                vec![
                    hirsel_types::Field {
                        growable: false,
                        name: "a".into(),
                        ty: f.scalar,
                    },
                    hirsel_types::Field {
                        growable: false,
                        name: "b".into(),
                        ty: f.scalar,
                    },
                ]
                .into(),
            )
        } else {
            TypeKind::Tuple(vec![f.scalar, f.scalar].into())
        });
        let e = if construct == "tuple" {
            f.expr(ExprKind::Tuple(vec![a, b]), ty, U::Take, 1)
        } else if construct == "box" {
            f.expr(
                ExprKind::Unary {
                    op: UnaryOp::Box,
                    value: a,
                },
                ty,
                U::Take,
                1,
            )
        } else {
            f.nominal("Pair", ty);
            let e = f.named_call(
                "Pair",
                vec![b, a],
                vec![(f.scalar, U::Take), (f.scalar, U::Take)],
                ty,
                1,
            );
            f.call_plan(
                e,
                hirsel_types::CallTarget::Constructor(hirsel_types::ContextualName::Constructor(
                    path("Pair"),
                )),
                vec![1, 0],
            );
            e
        };
        let d = f.lower(e, "main", ty).unwrap();
        insta::assert_snapshot!(format!("construct_{construct}"), print::definition(&d));
        if construct == "tuple" {
            assert!(matches!(
                returned(execute(vec![d], vec![])),
                Value::Tuple(_)
            ));
        }
    }
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
fn snapshot_shape_comptime_todo_and_local_function() {
    let mut f = Fixture::new(P::U8);
    let shape = f.type_(TypeKind::Builtin {
        kind: hirsel_types::BuiltinType::Shape,
        arguments: vec![hirsel_types::GenericArgument::Type(f.scalar)].into(),
    });
    let e = f.expr(ExprKind::Shape(TypeExprId(0)), shape, U::Take, 1);
    f.plans.shapes.push((TypeExprId(0), f.scalar));
    let d = f.lower(e, "main", shape).unwrap();
    assert!(
        d.bodies[0]
            .rvalues
            .iter()
            .any(|r| matches!(r, Rvalue::Shape(_)))
    );
    let e = f.expr(ExprKind::Comptime(e), f.scalar, U::Take, 1);
    f.plans.constants.push((e, Constant::Integer(7)));
    let d = f.lower(e, "main", f.scalar).unwrap();
    assert_eq!(bits(execute(vec![d.clone()], vec![])), 7);
    let e = f.expr(ExprKind::Todo(Some("pending".into())), f.unit, U::Take, 9);
    let d = f.lower(e, "main", f.unit).unwrap();
    assert!(
        matches!(execute(vec![d.clone()],vec![]).outcome,Outcome::Failed(f) if f.rule.as_str()=="LANG-6" && f.line==9)
    );
    let mut f = Fixture::new(P::U8);
    let body = f.integer(1, 1);
    let function = f.type_(TypeKind::Function(hirsel_types::FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: f.scalar,
        error: None,
        allocation: hirsel_types::Allocation::NoAlloc,
        authority: hirsel_types::AuthoritySummary::inert(),
        uses: hirsel_types::Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }));
    f.checked.closures.push(hirsel_types::ClosureInfo {
        result_evidence: None,
        expression: body,
        ty: function,
        captures: vec![],
    });
    let stmt = f.stmt(
        StmtKind::Function {
            name: hirsel_intern::Symbol(0),
            signature: FunctionSyntax::default(),
            body,
        },
        1,
    );
    f.frames.push((NameSite::Statement(stmt), f.scalar));
    f.checked.bindings.push(hirsel_types::BindingDecision {
        owner: f.owner.clone(),
        binding: hirsel_types::ResolvedName::Local(hirsel_types::LocalId(0)),
        site: NameSite::Statement(stmt),
        ty: function,
        usage: U::Take,
        mutable: false,
    });
    let unit = f.unit_();
    let root = f.block(vec![stmt], unit);
    let d = f.lower(root, "main", f.unit).unwrap();
    assert_eq!(d.bodies.len(), 2);
    assert!(
        d.bodies[0]
            .rvalues
            .iter()
            .any(|r| matches!(r, Rvalue::Closure { captures, .. } if captures.is_empty()))
    );
}

#[test]
fn guarded_moving_pattern_commits_only_after_true_guard() {
    for matches in [true, false] {
        let mut f = Fixture::new(P::U8);
        let n = f.integer(7, 1);
        let (p, b) = f.binding(f.scalar, U::Take);
        let guard = f.bool_(matches);
        let body = f.read(b, f.scalar, U::Take);
        let q = PatternId(f.tree.patterns.len() as u32);
        f.tree.patterns.push(Pattern {
            origin: origin(1),
            kind: PatternKind::Wildcard,
        });
        f.checked.patterns.push(PatternDecision {
            pattern: q,
            ty: f.scalar,
            usage: U::Borrow,
            kind: PatternDecisionKind::Wildcard,
            fields: vec![],
            annotation: None,
        });
        let otherwise = f.integer(9, 1);
        let e = f.expr(
            ExprKind::Match {
                value: n,
                arms: vec![
                    MatchArm {
                        pattern: p,
                        guard: Some(guard),
                        body,
                    },
                    MatchArm {
                        pattern: q,
                        guard: None,
                        body: otherwise,
                    },
                ],
            },
            f.scalar,
            U::Take,
            1,
        );
        assert!(
            f.lower(e, "main", f.scalar)
                .unwrap_err()
                .message
                .contains("guarded-pattern")
        );
        f.ownership.guarded_patterns.push(GuardedPatternUse {
            pattern: p,
            test: U::Borrow,
            commit: U::Take,
        });
        let d = f.lower(e, "main", f.scalar).unwrap();
        assert_eq!(
            bits(execute(vec![d.clone()], vec![])),
            if matches { 7 } else { 9 }
        );
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
fn iterator_protocol_uses_checked_tags_and_mutates_its_receiver() {
    let mut g = Fixture::new(P::U8);
    let option = g.type_(TypeKind::Variant(
        vec![
            hirsel_types::Variant {
                name: "Some".into(),
                fields: vec![(None, g.scalar)].into(),
            },
            hirsel_types::Variant {
                name: "None".into(),
                fields: vec![].into(),
            },
        ]
        .into(),
    ));
    g.nominal("Option", option);
    let parameter = g.parameter(0, g.scalar, U::Mut);
    let original = g.read(parameter.clone(), g.scalar, U::Copy);
    let (save, old) = g.let_(original);
    let old = g.read(old, g.scalar, U::Take);
    let some = g.named_call("Option", vec![old], vec![(g.scalar, U::Take)], option, 1);
    g.call_plan(
        some,
        hirsel_types::CallTarget::Constructor(hirsel_types::ContextualName::Variant {
            owner: path("Option"),
            variant: hirsel_types::VariantId(0),
        }),
        vec![0],
    );
    let lhs = g.read(parameter.clone(), g.scalar, U::Copy);
    let one = g.integer(1, 1);
    let increment = g.assign(lhs, AssignOp(Some(BinaryOp::Add)), one, g.scalar, 1);
    let yes = g.block(vec![save, increment], some);
    let none = g.named_call("Option", vec![], vec![], option, 1);
    g.call_plan(
        none,
        hirsel_types::CallTarget::Constructor(hirsel_types::ContextualName::Variant {
            owner: path("Option"),
            variant: hirsel_types::VariantId(1),
        }),
        vec![],
    );
    let current = g.read(parameter, g.scalar, U::Copy);
    let three = g.integer(3, 1);
    let condition = g.binary(BinaryOp::Lt, current, three, 1);
    let root = g.expr(
        ExprKind::If {
            condition,
            then_branch: yes,
            else_branch: Some(none),
        },
        option,
        U::Take,
        1,
    );
    let next = g.lower(root, "advance", option).unwrap();
    let mut f = Fixture::new(P::U8);
    let option = f.type_(TypeKind::Variant(
        vec![
            hirsel_types::Variant {
                name: "Some".into(),
                fields: vec![(None, f.scalar)].into(),
            },
            hirsel_types::Variant {
                name: "None".into(),
                fields: vec![].into(),
            },
        ]
        .into(),
    ));
    let zero = f.integer(0, 1);
    let (sum, sum_binding) = f.let_(zero);
    let zero = f.integer(0, 1);
    let (cursor, cursor_binding) = f.let_(zero);
    let source = f.read(cursor_binding.clone(), f.scalar, U::Borrow);
    let (pattern, item) = f.binding(f.scalar, U::Take);
    let lhs = f.read(sum_binding.clone(), f.scalar, U::Copy);
    let item = f.read(item, f.scalar, U::Copy);
    let add = f.assign(lhs, AssignOp(Some(BinaryOp::Add)), item, f.scalar, 2);
    let unit = f.unit_();
    let body = f.block(vec![add], unit);
    let loop_ = f.stmt(
        StmtKind::For {
            pattern,
            source,
            body,
        },
        2,
    );
    f.checked.instantiations.push(hirsel_types::Instantiation {
        expression: source,
        callee: path("advance"),
        arguments: vec![],
    });
    f.plans.protocols.push((
        loop_,
        hirsel_check_protocol::body::ForExecution::Advance {
            operation: path("advance"),
            step: hirsel_types::OptionLayout {
                result: option,
                item: f.scalar,
                definition: path("Option"),
                some_variant: hirsel_types::VariantId(0),
                none_variant: hirsel_types::VariantId(1),
            },
        },
    ));
    let sum_read = f.read(sum_binding, f.scalar, U::Copy);
    let cursor_read = f.read(cursor_binding, f.scalar, U::Copy);
    let result = f.binary(BinaryOp::Add, sum_read, cursor_read, 3);
    let root = f.block(vec![sum, cursor, loop_], result);
    f.nominal("Option", option);
    f.declare("advance", vec![(f.scalar, U::Mut)], option);
    let definition = f.lower(root, "main", f.scalar).unwrap();
    assert_eq!(bits(execute(vec![definition.clone(), next], vec![])), 6);
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
fn formal_frame_result_rows_override_pre_mode_results_and_require_complete_owners() {
    let mut f = Fixture::new(P::U8);
    let root = f.integer(7, 1);
    let result = f
        .checked
        .types
        .push(f.checked.types.get(f.scalar).unwrap().clone());
    f.result_types.push((f.owner.clone(), result));
    let definition = f.lower(root, "main", f.scalar).unwrap();
    assert_eq!(definition.bodies[0].result, result);
    assert_eq!(bits(execute(vec![definition], vec![])), 7);
    f.result_types.push((f.owner.clone(), result));
    assert!(
        f.lower(root, "main", f.scalar)
            .unwrap_err()
            .message
            .contains("formal frame result")
    );
    f.result_types = vec![(NameSite::Expression(root), result)];
    assert!(
        f.lower(root, "main", f.scalar)
            .unwrap_err()
            .message
            .contains("missing J")
    );

    let mut f = Fixture::new(P::U8);
    let body = f.integer(9, 1);
    let function = f.type_(TypeKind::Function(hirsel_types::FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: f.scalar,
        error: None,
        allocation: hirsel_types::Allocation::NoAlloc,
        authority: hirsel_types::AuthoritySummary::inert(),
        uses: hirsel_types::Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }));
    let closure = f.expr(
        ExprKind::Closure {
            signature: FunctionSyntax::default(),
            body,
        },
        function,
        U::Take,
        1,
    );
    f.closure(
        hirsel_types::ClosureInfo {
            result_evidence: None,
            expression: closure,
            ty: function,
            captures: vec![],
        },
        f.scalar,
        f.owner,
    );
    let child_result = f
        .checked
        .types
        .push(f.checked.types.get(f.scalar).unwrap().clone());
    f.result_types = vec![
        (f.owner.clone(), function),
        (NameSite::Expression(closure), child_result),
    ];
    let definition = f.lower(closure, "main", function).unwrap();
    assert_eq!(definition.bodies[1].result, child_result);
    f.result_types.pop();
    assert!(
        f.lower(closure, "main", function)
            .unwrap_err()
            .message
            .contains("missing J")
    );
}

#[test]
fn copy_capture_preserves_the_source_type_and_j_selected_aliased_capture_type() {
    let mut f = Fixture::new(P::U8);
    let mut source_type = ty(TypeKind::Box(f.scalar));
    source_type.modes.uniqueness = hirsel_types::Uniqueness::Unique;
    let source_type = f.checked.types.push(source_type);
    let capture_type = f.type_(TypeKind::Box(f.scalar));
    let value = f.integer(4, 1);
    let value = f.expr(
        ExprKind::Unary {
            op: UnaryOp::Box,
            value,
        },
        source_type,
        U::Take,
        1,
    );
    let (let_, binding) = f.let_(value);
    let body = f.integer(7, 1);
    let function = f.type_(TypeKind::Function(hirsel_types::FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: vec![].into(),
        result: f.scalar,
        error: None,
        allocation: hirsel_types::Allocation::NoAlloc,
        authority: hirsel_types::AuthoritySummary::inert(),
        uses: hirsel_types::Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }));
    let closure = f.expr(
        ExprKind::Closure {
            signature: FunctionSyntax::default(),
            body,
        },
        function,
        U::Take,
        1,
    );
    f.closure(
        hirsel_types::ClosureInfo {
            result_evidence: None,
            expression: closure,
            ty: function,
            captures: vec![hirsel_types::Capture {
                binding,
                ty: capture_type,
                usage: U::Copy,
            }],
        },
        f.scalar,
        f.owner,
    );
    let call = f.expr(
        ExprKind::Call {
            callee: closure,
            arguments: vec![],
        },
        f.scalar,
        U::Take,
        1,
    );
    f.indirect_call_plan(call, vec![]);

    let root = f.block(vec![let_], call);
    let definition = f.lower(root, "main", f.scalar).unwrap();
    let child = &definition.bodies[1];
    assert_eq!(
        child
            .locals
            .iter()
            .find(|l| l.storage == Storage::Capture)
            .unwrap()
            .ty,
        capture_type
    );
    assert!(
        definition.bodies[0]
            .rvalues
            .iter()
            .any(|r| matches!(r, Rvalue::Closure { captures, .. } if captures[0].usage == U::Copy))
    );
    assert_eq!(bits(execute(vec![definition], vec![])), 7);
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

#[test]
fn finite_materialized_trees_lower_past_32_and_retain_source_field_order() {
    with_large_stack(|| {
        for depth in [31, 32, 33, 200] {
            let mut f = Fixture::new(P::U32);
            let mut root = f.scalar;
            let mut node = ValueNode {
                ty: root,
                kind: ValueKind::Constant(Constant::Integer(7)),
            };
            let mut expression = f.integer(7, 1);
            for _ in 0..depth {
                root = f.type_(TypeKind::Tuple(vec![root, f.scalar].into()));
                let right = f.integer(9, 1);
                expression = f.expr(ExprKind::Tuple(vec![expression, right]), root, U::Take, 1);
                node = ValueNode {
                    ty: root,
                    kind: ValueKind::Construct {
                        constructor: Constructor::Tuple,
                        fields: vec![
                            node,
                            ValueNode {
                                ty: f.scalar,
                                kind: ValueKind::Constant(Constant::Integer(9)),
                            },
                        ],
                    },
                };
            }
            let expression = f.expr(ExprKind::Comptime(expression), root, U::Take, 1);
            f.plans.materialized.push((
                expression,
                Materialized {
                    types: f.checked.types.clone(),
                    root: node,
                },
            ));
            let definition = f.lower(expression, "main", root).unwrap();
            let Outcome::Returned { mut value, .. } = execute(vec![definition], vec![]).outcome
            else {
                panic!("expected return")
            };
            for _ in 0..depth {
                let Value::Tuple(mut fields) = value else {
                    panic!("expected tuple")
                };
                assert_eq!(
                    fields.pop().unwrap(),
                    Some(Value::Integer(
                        hirsel_interp::Integer::new(P::U32, 9).unwrap()
                    ))
                );
                value = fields.pop().unwrap().unwrap();
            }
            assert_eq!(
                value,
                Value::Integer(hirsel_interp::Integer::new(P::U32, 7).unwrap())
            );
        }
    });
}
