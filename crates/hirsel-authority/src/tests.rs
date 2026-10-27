use super::test_support::*;
use super::*;
use hirsel_intern::Symbol;
use hirsel_syntax::{ExprKind, Literal, StmtKind, UnaryOp};
use proptest::prelude::*;

#[test]
fn projected_paths_preserve_elaborated_scalar_types_without_symbols() {
    let mut f = Fixture::new();
    let scalar = f.ty(TypeKind::Primitive(Primitive::U64));
    let bytes = f.ty(TypeKind::Primitive(Primitive::Bytes));
    let representation = f.ty(TypeKind::Record(
        vec![
            Field {
                name: "n".into(),
                ty: scalar,
                growable: false,
            },
            Field {
                name: "items".into(),
                ty: bytes,
                growable: false,
            },
        ]
        .into(),
    ));
    let rec = f.nominal(
        "Rec",
        None,
        vec![],
        AuthoritySummary::inert(),
        Some(representation),
        false,
    );
    let rec = f.adjusted(rec, |t| t.modes.uniqueness = Uniqueness::Unique);
    let root = f.binding(
        ResolvedName::Parameter(ParamId(0)),
        rec,
        NameSite::Item,
        UseKind::Mut,
        true,
    );
    let projected = f.reference(ResolvedName::Parameter(ParamId(0)), scalar, NameSite::Item);
    f.body.places.push(SourcePlace {
        expression: projected,
        root: SourceRoot::Binding {
            owner: NameSite::Item,
            binding: ResolvedName::Parameter(ParamId(0)),
        },
        projections: vec![SourceProjection {
            expression: projected,
            kind: SourceProjectionKind::Field(FieldId(0)),
        }]
        .into(),
        access: SourceAccess::Read,
    });
    let expected = session().get(scalar).unwrap().clone();
    let block = f.block(vec![root, projected], None, f.unit);
    let out = f.finish(block);
    clean(&out);
    let ty = out
        .body
        .expressions
        .iter()
        .find(|r| r.expression == projected)
        .unwrap()
        .ty;
    assert_eq!(session().get(ty).unwrap(), &expected);
}

fn cross(f: &mut Fixture, value: ExprId) -> ExprId {
    let t = f.ty(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))));
    let mut fun = function(f.unit);
    fun.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Sendable].into(),
    }]
    .into();
    fun.parameters = vec![parameter(t, PassingMode::Take)].into();
    f.call("cross", fun, vec![value], None)
}
fn inert(f: &mut Fixture, value: ExprId) -> ExprId {
    let t = f.ty(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))));
    let mut fun = function(f.unit);
    fun.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![Bound::Inert].into(),
    }]
    .into();
    fun.parameters = vec![parameter(t, PassingMode::Borrow)].into();
    f.call("inert", fun, vec![value], None)
}
fn scoped_call(
    f: &mut Fixture,
    closure: ExprId,
    restriction: Uses,
    role: Option<Intrinsic>,
) -> ExprId {
    let mut cb = function(f.unit);
    if let Some(row) = f.body.expressions.iter().find(|r| r.expression == closure) {
        if let TypeKind::Function(actual) = &session().get(row.ty).unwrap().kind {
            cb.parameters = actual.parameters.clone();
        }
    }
    cb.uses = restriction;
    // The parameter promises no `@noalloc`, so it receives either callback
    // and the call's allocation is the CLO-2 join with the actual.
    cb.allocation = Allocation::MayAllocate;
    let t = f.ty(TypeKind::Function(cb));
    let t = f.adjusted(t, |t| t.modes.relation = Relation::environment());
    let mut fun = function(f.unit);
    fun.parameters = vec![parameter(t, PassingMode::Borrow)].into();
    f.call("scoped", fun, vec![closure], role)
}

#[test]
fn brand_families_share_the_same_rule_and_keep_field_paths() {
    for kind in [
        BuiltinType::SortedMap,
        BuiltinType::SlotId,
        BuiltinType::Task,
    ] {
        let mut f = Fixture::new();
        let t = f.ty(TypeKind::Builtin {
            kind,
            arguments: vec![].into(),
        });
        let t = f.adjusted(t, |t| {
            t.brand = Brand {
                alternatives: vec![Origin {
                    root: OriginRoot::Local(lexical(), LocalId(4)),
                    fields: vec![FieldId(2)].into(),
                }]
                .into(),
            }
        });
        let arg = f.expr(ExprKind::Literal(Literal::Bool(true)), t);
        let other = f.adjusted(t, |t| {
            t.brand.alternatives[0].fields = vec![FieldId(3)].into()
        });
        let mut fun = function(f.unit);
        fun.parameters = vec![parameter(other, PassingMode::Borrow)].into();
        let root = f.call("family", fun, vec![arg], None);
        rejected(&f.finish(root), "PROV-1");
    }
}
/// A root's current uniqueness is I's (MODE-2); J checks every occurrence's
/// structure, so a Receiver, which never crosses, fails the second.
#[test]
fn every_repeated_generic_occurrence_is_checked_before_weakening() {
    for second_unique in [false, true] {
        let mut f = Fixture::new();
        let bytes = f.ty(TypeKind::Primitive(Primitive::Bytes));
        let unique = f.adjusted(bytes, |t| t.modes.uniqueness = Uniqueness::Unique);
        let receiver = f.ty(TypeKind::Builtin {
            kind: BuiltinType::Receiver,
            arguments: vec![GenericArgument::Type(unique)].into(),
        });
        let t = f.ty(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))));
        let first = f.binding(
            ResolvedName::Parameter(ParamId(0)),
            unique,
            NameSite::Item,
            UseKind::Take,
            false,
        );
        let second = f.binding(
            ResolvedName::Parameter(ParamId(1)),
            if second_unique { unique } else { receiver },
            NameSite::Item,
            UseKind::Take,
            false,
        );
        let mut fun = function(f.unit);
        fun.generics = vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![Bound::Sendable].into(),
        }]
        .into();
        fun.parameters = vec![
            parameter(t, PassingMode::Borrow),
            parameter(t, PassingMode::Borrow),
        ]
        .into();
        let root = f.call("repeat", fun, vec![first, second], None);
        f.body.instantiations.push(Instantiation {
            expression: root,
            callee: path("repeat"),
            arguments: vec![GenericArgument::Type(unique)],
        });
        let out = f.finish(root);
        if second_unique {
            clean(&out)
        } else {
            rejected(&out, "MODE-4")
        }
    }
}
#[test]
fn provider_failures_are_preserved_outside_the_result() {
    let mut f = Fixture::new();
    std::sync::Arc::make_mut(&mut f.body.resolution.reads).push(
        DependencyRead::ProviderLookup {
            context: (hirsel_syntax::FileContext {
                package: path("owner").0.package,
                modules: vec![],
            })
            .into(),
            package: None,
            segments: (vec!["earlier".repeat(128)]).into(),
            output: (LookupOutput {
                manifest: Default::default(),
                result: Err(QueryError::NotImplemented),
            })
            .into(),
        }
        .shared(),
    );
    let prior_reads = f.body.resolution.reads.clone();
    let missing = f.ty(TypeKind::Named {
        definition: DefId::intern(&path("Missing")),
        arguments: vec![].into(),
    });
    let arg = f.expr(ExprKind::Literal(Literal::Bool(true)), missing);
    let root = inert(&mut f, arg);
    f.provider.failures.insert(
        DefId::intern(&path("Missing")),
        QueryError::Operational(OperationalFailure {
            operation: "signature".into(),
            message: "offline".into(),
        }),
    );
    let out = f.finish(root);
    assert!(out.body.resolution.reads.iter().eq(out.reads.iter()));
    assert!(
        std::ptr::eq(&prior_reads[0], &out.body.resolution.reads[0]),
        "authority copied H lookup payloads when appending its failed reads"
    );
    assert!(matches!(out.failure, Some(QueryError::Operational(_))));
    assert!(
        out.reads.contains(
            &DependencyRead::Signature {
                definition: hirsel_syntax::DefId::intern(&path("Missing")),
                fingerprint: None
            }
            .shared()
        )
    );
}
proptest! {
    #[test]
    fn brand_joins_are_canonical_and_deterministic(a in prop::collection::vec(0u32..12, 0..30), b in prop::collection::vec(0u32..12, 0..30)) {
        let make = |ids: &[u32]| Brand { alternatives: ids.iter().map(|id| origin(OriginRoot::Local(lexical(), LocalId(*id)))).collect() };
        let first = brand_join(&make(&a), &make(&b)); let mut reversed = a.clone(); reversed.reverse();
        prop_assert_eq!(&first, &brand_join(&make(&b), &make(&reversed)));
        prop_assert_eq!(&first, &brand_join(&first, &first));
    }
    #[test]
    fn complete_pass_is_deterministic_across_table_order(unique in any::<bool>(), reverse in any::<bool>()) {
        let build = |reorder: bool| {
            let mut f = Fixture::new(); let bytes = f.ty(TypeKind::Primitive(Primitive::Bytes)); let bytes = f.adjusted(bytes, |t| if unique { t.modes.uniqueness = Uniqueness::Unique });
            let arg = f.binding(ResolvedName::Local(LocalId(0)), bytes, NameSite::Item, UseKind::Take, false); let root = cross(&mut f, arg);
            if reorder { f.body.expressions.reverse(); f.body.bindings.reverse(); f.body.resolution.expressions.reverse(); }
            f.finish(root)
        };
        let a = build(false); let b = build(reverse);
        prop_assert_eq!(a.diagnostics, b.diagnostics); prop_assert_eq!(a.reads, b.reads);
        prop_assert_eq!(a.dependencies, b.dependencies);
    }
}

#[test]
fn function_joins_keep_the_weakest_allocation_promise() {
    for allocates in [false, true] {
        let mut f = Fixture::new();
        let body = f.expr(ExprKind::Tuple(vec![]), f.unit);
        let first = f.closure(body, function(f.unit));
        let mut function = function(f.unit);
        if allocates {
            function.allocation = Allocation::MayAllocate;
        }
        let second = f.closure(body, function);
        let boolean = f.ty(TypeKind::Primitive(Primitive::Bool));
        let condition = f.expr(ExprKind::Literal(Literal::Bool(true)), boolean);
        let ty = f
            .body
            .expressions
            .iter()
            .find(|e| e.expression == first)
            .unwrap()
            .ty;
        let branch = f.expr(
            ExprKind::If {
                condition,
                then_branch: first,
                else_branch: Some(second),
            },
            ty,
        );
        let root = scoped_call(&mut f, branch, Uses::Unrestricted, None);
        let out = f.finish(root);
        clean(&out);
        assert_eq!(
            out.call_allocations,
            vec![(
                root,
                if allocates {
                    Allocation::MayAllocate
                } else {
                    Allocation::NoAlloc
                }
            )]
        );
    }
}

fn launched_handle(f: &mut Fixture) -> (ExprId, TypeId) {
    let handle = f.ty(TypeKind::Builtin {
        kind: BuiltinType::Task,
        arguments: vec![GenericArgument::Type(f.unit)].into(),
    });
    let call = f.call(
        "async",
        function(handle),
        vec![],
        Some(Intrinsic::TaskAsync),
    );
    (call, handle)
}
fn consume_handle(f: &mut Fixture, handle: ExprId, ty: TypeId, detach: bool) -> ExprId {
    let unit = f.expr(ExprKind::Tuple(vec![]), f.unit);
    let mut function = function(f.unit);
    function.parameters = vec![
        parameter(f.unit, PassingMode::Borrow),
        parameter(ty, PassingMode::Take),
    ]
    .into();
    f.call(
        if detach { "detach" } else { "await" },
        function,
        vec![unit, handle],
        Some(if detach {
            Intrinsic::TaskDetach
        } else {
            Intrinsic::TaskAwait
        }),
    )
}
#[test]
fn task_handle_consumption_is_required_on_every_reachable_branch() {
    for both in [false, true] {
        let mut f = Fixture::new();
        let (launch, ty) = launched_handle(&mut f);
        let local = f.binding(
            ResolvedName::Local(LocalId(0)),
            ty,
            NameSite::Item,
            UseKind::Take,
            false,
        );
        let pattern = match f.body.bindings.last().unwrap().site {
            NameSite::Pattern(p) => p,
            _ => unreachable!(),
        };
        let binding = f.statement(StmtKind::Let {
            pattern,
            ty: None,
            value: launch,
            mutable: false,
            otherwise: None,
        });
        let first = consume_handle(&mut f, local, ty, false);
        let second = if both {
            consume_handle(&mut f, local, ty, true)
        } else {
            f.expr(ExprKind::Tuple(vec![]), f.unit)
        };
        let boolean = f.ty(TypeKind::Primitive(Primitive::Bool));
        let condition = f.expr(ExprKind::Literal(Literal::Bool(true)), boolean);
        let branch = f.expr(
            ExprKind::If {
                condition,
                then_branch: first,
                else_branch: Some(second),
            },
            f.unit,
        );
        let root = f.expr(
            ExprKind::Block {
                label: None,
                statements: vec![binding],
                tail: Some(branch),
            },
            f.unit,
        );
        let out = f.finish(root);
        if both {
            clean(&out)
        } else {
            rejected(&out, "CONC-2")
        }
    }
}
#[test]
fn task_handles_are_checked_before_early_return_and_not_after_unreachable_code() {
    for consumes in [false, true] {
        let mut f = Fixture::new();
        let (handle, ty) = launched_handle(&mut f);
        let consumed = if consumes {
            consume_handle(&mut f, handle, ty, true)
        } else {
            handle
        };
        let ret = f.expr(ExprKind::Return(None), f.unit);
        let unreachable = launched_handle(&mut f).0;
        let root = f.block(vec![consumed, ret, unreachable], None, f.unit);
        let out = f.finish(root);
        if consumes {
            clean(&out)
        } else {
            rejected(&out, "CONC-2")
        }
    }
}

proptest! {
    #[test]
    fn capture_owners_and_nested_origins_are_deterministic(reverse in any::<bool>(), take in any::<bool>()) {
        let build = |reverse: bool| {
            let mut f = Fixture::new();
            let scalar = f.ty(TypeKind::Primitive(Primitive::U64));
            let value = f.binding(ResolvedName::Parameter(ParamId(0)), scalar, NameSite::Item, UseKind::Borrow, false);
            let body = if take {f.expr(ExprKind::Unary {op: UnaryOp::Take, value}, scalar)} else {value};
            let mut cb = function(scalar); cb.parameters = vec![parameter(scalar, PassingMode::Borrow)].into();
            let closure = f.closure(body, cb);
            let root = scoped_call(&mut f, closure, Uses::Unrestricted, None);
            if reverse {f.body.bindings.reverse(); f.body.expressions.reverse(); f.body.resolution.expressions.reverse();}
            let out = f.finish(root); clean(&out);
            let captures: Vec<_> = out.captures.iter().flat_map(|table| table.captures.iter().map(|capture| (table.expression, table.context, capture.owner.clone(), capture.binding.clone(), capture.usage, session().get(capture.ty).unwrap().clone()))).collect();
            let results: Vec<_> = out.result_types.iter().map(|(owner, ty)| (owner.clone(), session().get(*ty).unwrap().clone())).collect();
            (out.capture_modes(), out.call_allocations, captures, results)
        };
        prop_assert_eq!(build(false), build(reverse));
    }
}

#[test]
fn local_named_functions_publish_empty_and_nonempty_capture_rows() {
    for captures_outer in [false, true] {
        let mut f = Fixture::new();
        let scalar = f.ty(TypeKind::Primitive(Primitive::U64));
        let outer = f.binding(
            ResolvedName::Parameter(ParamId(0)),
            scalar,
            NameSite::Item,
            UseKind::Borrow,
            false,
        );
        let statement = f.statement(StmtKind::Expr(outer));
        let own = f.binding(
            ResolvedName::Parameter(ParamId(0)),
            scalar,
            NameSite::Statement(statement),
            UseKind::Borrow,
            false,
        );
        let unit = f.expr(ExprKind::Tuple(vec![]), f.unit);
        let body = f.block(
            if captures_outer {
                vec![own, outer]
            } else {
                vec![own]
            },
            Some(unit),
            f.unit,
        );
        let name = Symbol::intern("local");
        let mut callback = function(f.unit);
        callback.parameters = vec![parameter(scalar, PassingMode::Borrow)].into();
        let function_type = f.ty(TypeKind::Function(callback));
        f.body.bindings.push(BindingDecision {
            owner: NameSite::Item,
            binding: ResolvedName::Local(LocalId(7)),
            site: NameSite::Statement(statement),
            ty: function_type,
            usage: UseKind::Borrow,
            mutable: false,
        });
        f.body
            .resolution
            .local_functions
            .push((statement, LocalId(7)));
        f.tree.statements[statement.0 as usize].kind = StmtKind::Function {
            name,
            signature: syntax_function(),
            body,
        };
        let root = f.expr(
            ExprKind::Block {
                label: None,
                statements: vec![statement],
                tail: Some(unit),
            },
            f.unit,
        );
        let out = f.finish(root);
        clean(&out);
        assert_eq!(out.captures.len(), 1);
        assert_eq!(out.captures[0].captures.len(), usize::from(captures_outer));
        assert_eq!(out.body.closures[0].expression, body);
        assert!(matches!(
            session()
                .get(
                    out.body
                        .expressions
                        .iter()
                        .find(|r| r.expression == body)
                        .unwrap()
                        .ty
                )
                .unwrap()
                .kind,
            TypeKind::Tuple(_)
        ));
        assert!(matches!(
            session().get(out.body.closures[0].ty).unwrap().kind,
            TypeKind::Function(_)
        ));
        if captures_outer {
            assert_eq!(
                out.capture_modes()
                    .iter()
                    .map(|r| (r.expression, r.ordinal, r.owner))
                    .collect::<Vec<_>>(),
                vec![(body, 0, NameSite::Item)]
            );
        }
        for owner in [NameSite::Item, NameSite::Statement(statement)] {
            assert!(
                out.body
                    .bindings
                    .iter()
                    .any(|b| b.owner == owner && b.binding == ResolvedName::Parameter(ParamId(0)))
            );
        }
    }
}

#[test]
fn loop_control_cannot_consume_a_task_handle_in_unreachable_code() {
    for breaks in [false, true] {
        for consumes in [false, true] {
            let mut f = Fixture::new();
            let (launch, ty) = launched_handle(&mut f);
            let local = f.binding(
                ResolvedName::Local(LocalId(0)),
                ty,
                NameSite::Item,
                UseKind::Take,
                false,
            );
            let pattern = match f.body.bindings.last().unwrap().site {
                NameSite::Pattern(p) => p,
                _ => unreachable!(),
            };
            let bind = f.statement(StmtKind::Let {
                pattern,
                ty: None,
                value: launch,
                mutable: false,
                otherwise: None,
            });
            let control = f.expr(
                if breaks {
                    ExprKind::Break {
                        label: None,
                        value: None,
                    }
                } else {
                    ExprKind::Continue(None)
                },
                f.unit,
            );
            let unreachable = consume_handle(&mut f, local, ty, true);
            let loop_body = f.block(vec![control, unreachable], None, f.unit);
            let boolean = f.ty(TypeKind::Primitive(Primitive::Bool));
            let condition = f.expr(ExprKind::Literal(Literal::Bool(true)), boolean);
            let loop_statement = f.statement(StmtKind::While {
                condition,
                body: loop_body,
                label: None,
            });
            let tail = if consumes {
                Some(consume_handle(&mut f, local, ty, true))
            } else {
                None
            };
            let root = f.expr(
                ExprKind::Block {
                    label: None,
                    statements: vec![bind, loop_statement],
                    tail,
                },
                f.unit,
            );
            let out = f.finish(root);
            if consumes {
                super::test_support::clean(&out)
            } else {
                super::test_support::rejected(&out, "CONC-2")
            }
        }
    }
}

#[test]
fn scoped_callback_uses_parameters_substitute_the_actual_authority() {
    for allowed in [false, true] {
        let mut f = Fixture::new();
        let clock = f.capability("MonotonicClock", CapabilityKind::Global);
        let argument = f.binding(
            ResolvedName::Parameter(ParamId(0)),
            clock,
            NameSite::Item,
            UseKind::Borrow,
            false,
        );
        let mut callback = function(f.unit);
        callback.parameters = vec![parameter(clock, PassingMode::Borrow)].into();
        callback.uses = Uses::Only(AuthoritySummary::Known(
            vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0)))].into(),
        ));
        let callback = f.ty(TypeKind::Function(callback));
        let callback = f.adjusted(callback, |t| t.modes.relation = Relation::environment());
        let callee = f.binding(
            ResolvedName::Local(LocalId(0)),
            callback,
            NameSite::Item,
            UseKind::Borrow,
            false,
        );
        if let SignatureKind::Function(signature) = &mut f.signature.kind {
            signature.uses = Uses::Only(if allowed {
                AuthoritySummary::Known(
                    vec![AuthorityTerm::Capability(path("MonotonicClock"))].into(),
                )
            } else {
                AuthoritySummary::inert()
            });
        }
        let root = f.expr(
            ExprKind::Call {
                callee,
                arguments: vec![hirsel_syntax::Argument {
                    label: None,
                    value: argument,
                }],
            },
            f.unit,
        );
        let out = f.finish(root);
        if allowed {
            clean(&out)
        } else {
            rejected(&out, "CLO-2")
        }
    }
}

#[test]
fn function_uses_parameter_limits_resolve_its_declared_authority() {
    for same in [false, true] {
        let mut f = Fixture::new();
        let clock = f.capability("MonotonicClock", CapabilityKind::Global);
        let tasks = f.capability("Tasks", CapabilityKind::Global);
        let selected = if same { clock } else { tasks };
        let argument = f.binding(
            ResolvedName::Parameter(ParamId(0)),
            selected,
            NameSite::Item,
            UseKind::Borrow,
            false,
        );
        if let SignatureKind::Function(signature) = &mut f.signature.kind {
            signature.parameters = vec![parameter(clock, PassingMode::Borrow)].into();
            signature.uses = Uses::Only(AuthoritySummary::Known(
                vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0)))].into(),
            ));
        }
        let mut operation = function(f.unit);
        operation.parameters = vec![parameter(selected, PassingMode::Borrow)].into();
        operation.authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Parameter(hirsel_types::ParameterRef::bound(
                ParamId(0),
            ))]
            .into(),
        );
        let root = f.call("operation", operation, vec![argument], None);
        let out = f.finish(root);
        if same {
            clean(&out)
        } else {
            rejected(&out, "CAP-1")
        }
    }
}

#[test]
fn contextual_callback_uses_are_published_on_the_checked_function_value() {
    for uses in [Uses::Unrestricted, Uses::Only(AuthoritySummary::inert())] {
        let mut f = Fixture::new();
        let body = f.block(vec![], None, f.unit);
        let closure = f.closure(body, function(f.unit));
        let root = scoped_call(&mut f, closure, uses.clone(), None);
        let out = f.finish(root);
        clean(&out);
        let checked = &out.body.closures[0];
        let TypeKind::Function(function) = &session().get(checked.ty).unwrap().kind else {
            panic!("closure type");
        };
        assert_eq!(function.uses, uses);
        let expression = out
            .body
            .expressions
            .iter()
            .find(|row| row.expression == closure)
            .unwrap();
        assert!(session().semantic_eq(expression.ty, checked.ty));
    }
}

#[test]
fn call_instantiation_ignores_implicit_loop_edges_on_the_same_expression() {
    let mut f = Fixture::new();
    let scalar = f.ty(TypeKind::Primitive(Primitive::U64));
    let bytes = f.ty(TypeKind::Primitive(Primitive::Bytes));
    let generic = f.ty(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))));
    let mut fun = function(generic);
    fun.generics = vec![GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    }]
    .into();
    let call = f.call("source", fun, vec![], None);
    f.body.instantiations.extend([
        Instantiation {
            expression: call,
            callee: path("implicit_loop_operation"),
            arguments: vec![GenericArgument::Type(bytes)],
        },
        Instantiation {
            expression: call,
            callee: path("source"),
            arguments: vec![GenericArgument::Type(scalar)],
        },
    ]);
    let out = f.finish(call);
    clean(&out);
    let ty = out
        .body
        .expressions
        .iter()
        .find(|r| r.expression == call)
        .unwrap()
        .ty;
    assert!(matches!(
        session().get(ty).unwrap().kind,
        TypeKind::Primitive(Primitive::U64)
    ));
}

#[test]
fn ownership_markers_preserve_instantiated_nested_provenance() {
    for op in [UnaryOp::Take, UnaryOp::Copy, UnaryOp::Unique, UnaryOp::Mut] {
        for container in ["tuple", "box", "list"] {
            let mut f = Fixture::new();
            let scalar = f.ty(TypeKind::Primitive(Primitive::U64));
            let owner = f.binding(
                ResolvedName::Local(LocalId(8)),
                scalar,
                NameSite::Item,
                UseKind::Copy,
                false,
            );
            let bytes = f.ty(TypeKind::Primitive(Primitive::Bytes));
            let payload = f.adjusted(bytes, |t| {
                t.brand = brand(OriginRoot::Parameter(ParameterRef::bound(ParamId(0))))
            });
            let kind = match container {
                "tuple" => TypeKind::Tuple(vec![payload].into()),
                "box" => TypeKind::Box(payload),
                _ => TypeKind::Builtin {
                    kind: BuiltinType::List,
                    arguments: vec![GenericArgument::Type(payload)].into(),
                },
            };
            let result = f.ty(kind);
            let mut fun = function(result);
            fun.parameters = vec![parameter(scalar, PassingMode::Borrow)].into();
            let call = f.call("source", fun, vec![owner], None);
            let marked = f.expr(ExprKind::Unary { op, value: call }, result);
            let out = f.finish(marked);
            clean(&out);
            let ty = out
                .body
                .expressions
                .iter()
                .find(|r| r.expression == marked)
                .unwrap()
                .ty;
            let payload = match &session().get(ty).unwrap().kind {
                TypeKind::Tuple(ts) => ts[0],
                TypeKind::Box(t) => *t,
                TypeKind::Builtin { arguments, .. } => {
                    let GenericArgument::Type(t) = arguments[0] else {
                        panic!()
                    };
                    t
                }
                _ => panic!(),
            };
            assert_eq!(
                session().get(payload).unwrap().brand,
                brand(OriginRoot::Local(lexical(), LocalId(8)))
            );
        }
    }
}

fn dependent_callable_receipt(actual_sendability: Sendability) -> AuthorityOutput {
    let mut f = Fixture::new();
    let scope = NameSite::Item;
    let dependency = TypeParameterRef::body(scoped(scope), TypeParamId(0));
    let declaration = GenericParameter::Type {
        name: "T".into(),
        bounds: vec![].into(),
    };
    if let SignatureKind::Function(own) = &mut f.signature.kind {
        own.generics = vec![declaration.clone()].into();
    }
    f.body
        .resolution
        .generic_declarations
        .insert(scoped(scope), vec![declaration].into());
    let callback = f.ty(TypeKind::Function(function(f.unit)));
    let actual = f.adjusted(callback, |t| t.modes.sendability = actual_sendability);
    let expected = f.adjusted(callback, |t| {
        t.modes.sendability = Sendability::DependsOn(vec![dependency].into());
    });
    let value = f.binding(
        ResolvedName::Parameter(ParamId(0)),
        actual,
        scope,
        UseKind::Take,
        false,
    );
    let mut consumer = function(f.unit);
    consumer.parameters = vec![parameter(expected, PassingMode::Take)].into();
    let root = f.call("receive", consumer, vec![value], None);
    f.finish(root)
}

#[test]
fn a_dependent_callable_receives_the_same_capture_obligation() {
    let dependency = TypeParameterRef::body(scoped(NameSite::Item), TypeParamId(0));
    clean(&dependent_callable_receipt(Sendability::DependsOn(
        vec![dependency].into(),
    )));
    clean(&dependent_callable_receipt(Sendability::Sendable));
}

#[test]
fn a_dependent_callable_refuses_missing_capture_evidence() {
    rejected(
        &dependent_callable_receipt(Sendability::NotSendable),
        "MODE-4",
    );
}

#[test]
fn nongeneric_calls_skip_refinement_walks_but_generic_calls_keep_them() {
    for generic_call in [false, true] {
        let mut f = Fixture::new();
        let scalar = f.ty(TypeKind::Primitive(Primitive::U64));
        let tuple = f.ty(TypeKind::Tuple(vec![scalar; 64].into()));
        let argument = f.binding(
            ResolvedName::Parameter(ParamId(0)),
            tuple,
            NameSite::Item,
            UseKind::Borrow,
            false,
        );
        let formal = if generic_call {
            f.ty(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0))))
        } else {
            tuple
        };
        let mut callee = function(f.unit);
        callee.parameters = vec![parameter(formal, PassingMode::Borrow)].into();
        if generic_call {
            callee.generics = vec![GenericParameter::Type {
                name: "T".into(),
                bounds: vec![].into(),
            }]
            .into();
        }
        let call = f.call("observe", callee, vec![argument], None);
        if generic_call {
            f.body.instantiations.push(Instantiation {
                expression: call,
                callee: path("observe"),
                arguments: vec![GenericArgument::Type(tuple)],
            });
        }
        let before = super::walk::generic_refinement_rows();
        let out = f.finish(call);
        clean(&out);
        let visited = super::walk::generic_refinement_rows() - before;
        let checked = out
            .body
            .calls
            .iter()
            .find(|r| r.expression == call)
            .unwrap();
        if generic_call {
            assert!(visited > 0, "generic occurrence refinement must still run");
            let GenericArgument::Type(actual) = checked.generic_arguments[0] else {
                panic!("type actual")
            };
            assert!(session().semantic_eq(actual, tuple));
        } else {
            assert!(checked.generic_arguments.is_empty());
            assert_eq!(
                visited, 0,
                "there is no generic slot for any header walk to refine"
            );
        }
    }
}

#[test]
fn selected_callable_receipt_keeps_actual_parameter_authority() {
    for allowed in [false, true] {
        let mut f = Fixture::new();
        let dir = f.capability("Dir", CapabilityKind::Global);
        let net = f.capability("Net", CapabilityKind::Global);
        let mut callback = function(f.unit);
        callback.parameters = vec![
            parameter(dir, PassingMode::Borrow),
            parameter(net, PassingMode::Borrow),
        ]
        .into();
        callback.authority = AuthoritySummary::Known(
            vec![
                AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0))),
                AuthorityTerm::Parameter(ParameterRef::bound(ParamId(1))),
            ]
            .into(),
        );
        callback.uses = Uses::Unrestricted;
        let callback = f.ty(TypeKind::Function(callback));
        let callee = f.binding(
            ResolvedName::Parameter(ParamId(0)),
            callback,
            NameSite::Item,
            UseKind::Borrow,
            false,
        );
        let d = f.binding(
            ResolvedName::Parameter(ParamId(1)),
            dir,
            NameSite::Item,
            UseKind::Borrow,
            false,
        );
        let n = f.binding(
            ResolvedName::Parameter(ParamId(2)),
            net,
            NameSite::Item,
            UseKind::Borrow,
            false,
        );
        if let SignatureKind::Function(signature) = &mut f.signature.kind {
            signature.parameters = vec![
                parameter(callback, PassingMode::Borrow),
                parameter(dir, PassingMode::Borrow),
                parameter(net, PassingMode::Borrow),
            ]
            .into();
            let mut terms = vec![
                AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0))),
                AuthorityTerm::Capability(path("Dir")),
            ];
            if allowed {
                terms.push(AuthorityTerm::Capability(path("Net")));
            }
            signature.uses = Uses::Only(AuthoritySummary::Known(terms.into()));
        }
        let root = f.expr(
            ExprKind::Call {
                callee,
                arguments: vec![
                    hirsel_syntax::Argument {
                        label: None,
                        value: d,
                    },
                    hirsel_syntax::Argument {
                        label: None,
                        value: n,
                    },
                ],
            },
            f.unit,
        );
        let out = f.finish(root);
        if allowed {
            clean(&out)
        } else {
            rejected(&out, "CAP-1")
        }
    }
}
