include!("tests/modules.rs");
mod corpus;
mod source_support;
mod support;
use super::*;
use hirsel_intern::Symbol;
use hirsel_syntax::{DefId, ExprKind, PassingMode, UnaryOp};
use support::*;

fn local() -> Modes {
    Modes {
        relation: Relation::environment(),
        ..Modes::global()
    }
}
fn unique() -> Modes {
    Modes {
        uniqueness: Uniqueness::Unique,
        ..Modes::global()
    }
}
fn resource_signature() -> Signature {
    Signature {
        definition: DefId::intern(&path("Token")),
        metadata: Default::default(),
        fingerprint: Default::default(),
        kind: SignatureKind::Type(TypeDefinition {
            loop_source: None,
            format: None,
            destructor_allocation: Allocation::NoAlloc,
            generics: vec![],
            representation: None,
            bounds: vec![],
            authority: AuthoritySummary::inert(),
            capability: None,
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Default::default(),
            distinct: false,
            resource: true,
            drop: None,
            fact: None,
            captures: None,
        }),
    }
}
#[test]
fn branch_moves_join_but_early_returns_do_not_reach_the_join() {
    for exits in [false, true] {
        let mut f = Fixture::new();
        let x = f.parameter(f.bytes, PassingMode::Take);
        let read = f.read(x);
        let take = f.unary(UnaryOp::Take, read);
        let statement = f.stmt(hirsel_syntax::StmtKind::Expr(take));
        let tail = if exits {
            Some(f.expr(ExprKind::Return(None), f.unit))
        } else {
            None
        };
        let branch = f.expr(
            ExprKind::Block {
                label: None,
                statements: vec![statement],
                tail,
            },
            f.unit,
        );
        let condition = f.literal(f.scalar);
        let expr = f.expr(
            ExprKind::If {
                condition,
                then_branch: branch,
                else_branch: None,
            },
            f.unit,
        );
        f.expr_stmt(expr);
        let again = f.read(x);
        f.expr_stmt(again);
        let out = f.check();
        assert_eq!(
            out.diagnostics.first().map(|d| d.rule.as_str()),
            (!exits).then_some("OWN-2"),
            "{:?}",
            out.diagnostics
        );
    }
}
#[test]
fn loops_reach_a_fixed_point_and_reinitialisation_repairs_a_move() {
    for reinitialise in [false, true] {
        let mut f = Fixture::new();
        let fresh = f.literal(f.bytes);
        let x = f.bind(fresh, true);
        let read = f.read(x);
        let take = f.unary(UnaryOp::Take, read);
        let statement = f.stmt(hirsel_syntax::StmtKind::Expr(take));
        let mut statements = vec![statement];
        if reinitialise {
            let read = f.read(x);
            let fresh = f.literal(f.bytes);
            let assign = f.stmt(hirsel_syntax::StmtKind::Assign {
                place: read,
                value: fresh,
                op: hirsel_syntax::AssignOp(None),
            });
            statements.push(assign);
        }
        let body = f.expr(
            ExprKind::Block {
                label: None,
                statements,
                tail: None,
            },
            f.unit,
        );
        let condition = f.literal(f.scalar);
        let loop_ = f.stmt(hirsel_syntax::StmtKind::While {
            condition,
            body,
            label: None,
        });
        f.statements.push(loop_);
        let again = f.read(x);
        f.expr_stmt(again);
        let out = f.check();
        assert_eq!(
            out.diagnostics.first().map(|d| d.rule.as_str()),
            (!reinitialise).then_some("OWN-2"),
            "{:?}",
            out.diagnostics
        );
    }
}

#[test]
fn unique_decisions_cover_places_and_fresh_temporaries() {
    let mut f = Fixture::new();
    let x = f.parameter(f.bytes, PassingMode::Borrow);
    let read = f.read(x);
    let copied = f.unary(UnaryOp::Unique, read);
    f.bind(copied, false);
    let fresh = f.literal(f.bytes);
    let unchanged = f.unary(UnaryOp::Unique, fresh);
    f.bind(unchanged, false);
    let out = f.check();
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    assert_eq!(
        out.value.unique,
        vec![
            UniqueDecision {
                expression: copied,
                action: UniqueAction::DeepCopy
            },
            UniqueDecision {
                expression: unchanged,
                action: UniqueAction::AlreadyUnique
            }
        ]
    );
}
#[test]
fn guarded_moving_patterns_test_borrows_and_commit_takes() {
    use hirsel_syntax::{MatchArm, Pattern, PatternId, PatternKind};
    for (moving, unreachable) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut f = Fixture::new();
        let x = f.parameter(f.bytes, PassingMode::Take);
        let dead_unique = if unreachable {
            let result = f.expr(ExprKind::Return(None), f.unit);
            f.expr_stmt(result);
            let read = f.read(x);
            let unique = f.unary(UnaryOp::Unique, read);
            f.expr_stmt(unique);
            Some(unique)
        } else {
            None
        };
        let read = f.read(x);
        let source = if moving {
            f.unary(UnaryOp::Take, read)
        } else {
            read
        };
        let pattern = PatternId(0);
        let symbol = Symbol::intern("payload");
        f.syntax.tree.patterns.push(Pattern {
            origin: origin(300),
            kind: PatternKind::Binding(symbol),
        });
        f.body.patterns.push(PatternDecision {
            pattern,
            ty: f.bytes,
            usage: UseKind::Borrow,
            kind: PatternDecisionKind::Binding(LocalId(0)),
            fields: vec![],
            annotation: None,
        });
        f.body.bindings.push(BindingDecision {
            owner: NameSite::Item,
            binding: ResolvedName::Local(LocalId(0)),
            site: NameSite::Pattern(pattern),
            ty: f.bytes,
            usage: UseKind::Borrow,
            mutable: false,
        });
        let guard = f.literal(f.scalar);
        let body = f.expr(ExprKind::Tuple(vec![]), f.unit);
        let match_ = f.expr(
            ExprKind::Match {
                value: source,
                arms: vec![MatchArm {
                    pattern,
                    guard: Some(guard),
                    body,
                }],
            },
            f.unit,
        );
        f.expr_stmt(match_);
        let out = f.check();
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        if let Some(expression) = dead_unique {
            assert_eq!(
                out.value.unique,
                vec![UniqueDecision {
                    expression,
                    action: UniqueAction::DeepCopy
                }]
            );
        }
        assert_eq!(
            out.value.guarded_patterns,
            vec![GuardedPatternUse {
                pattern,
                test: UseKind::Borrow,
                commit: if moving {
                    UseKind::Take
                } else {
                    UseKind::Borrow
                }
            }]
        );
        assert_eq!(
            f.body.patterns[0].usage,
            if moving {
                UseKind::Take
            } else {
                UseKind::Borrow
            }
        );
    }
}
#[test]
fn deep_bodies_use_worklists_instead_of_the_host_stack() {
    let mut f = Fixture::new();
    let mut expr = f.expr(ExprKind::Tuple(vec![]), f.unit);
    for _ in 0..6000 {
        expr = f.expr(ExprKind::Before(expr), f.unit);
    }
    f.tail = Some(expr);
    let out = f.check();
    assert!(out.diagnostics.is_empty());
}

#[test]
fn availability_precedes_mode_errors_at_the_enclosing_call() {
    let mut f = Fixture::new();
    let x = f.parameter(f.bytes, PassingMode::Take);
    let read = f.read(x);
    let take = f.unary(UnaryOp::Take, read);
    f.expr_stmt(take);
    let again = f.read(x);
    let arg = f.unary(UnaryOp::Mut, again);
    let call = f.call(
        "require",
        vec![arg],
        vec![PassingMode::Mut],
        vec![unique()],
        Modes::global(),
    );
    f.expr_stmt(call);
    let out = f.check();
    assert_eq!(out.diagnostics.len(), 1, "{:?}", out.diagnostics);
    assert_eq!(out.diagnostics[0].rule.as_str(), "OWN-2");
}
#[test]
fn recovery_types_suppress_dependent_mode_judgments() {
    let mut f = Fixture::new();
    let error = f.new_type(TypeKind::Error, Modes::global());
    let x = f.parameter(f.scalar, PassingMode::Borrow);
    f.body.bindings[x].ty = error;
    let read = f.read(x);
    let take = f.unary(UnaryOp::Take, read);
    f.expr_stmt(take);
    let out = f.check();
    assert!(out.diagnostics.is_empty());
}

#[test]
fn published_shared_storage_is_immutable_but_the_handle_can_be_replaced() {
    for nested in [false, true] {
        let mut f = Fixture::new();
        let shared = f.new_type(
            TypeKind::Builtin {
                kind: BuiltinType::Shared,
                arguments: vec![GenericArgument::Type(f.bytes)].into(),
            },
            Modes::global(),
        );
        let x = f.parameter(shared, PassingMode::Mut);
        let lhs = f.read(x);
        let lhs = if nested {
            let field = Symbol::intern("root");
            let field = f.expr(ExprKind::Field { base: lhs, field }, f.bytes);
            f.body.places.push(SourcePlace {
                expression: field,
                root: SourceRoot::Value(field),
                projections: vec![SourceProjection {
                    expression: field,
                    kind: SourceProjectionKind::Field(FieldId(0)),
                }]
                .into(),
                access: SourceAccess::Read,
            });
            field
        } else {
            lhs
        };
        let rhs = f.literal(if nested { f.bytes } else { shared });
        let statement = f.stmt(hirsel_syntax::StmtKind::Assign {
            place: lhs,
            value: rhs,
            op: hirsel_syntax::AssignOp(None),
        });
        f.statements.push(statement);
        let out = f.check();
        assert_eq!(
            out.diagnostics.first().map(|d| d.rule.as_str()),
            nested.then_some("OWN-7")
        );
    }
}

proptest::proptest! {
    #[test]
    fn interning_and_provider_load_order_do_not_change_decisions(
        padding in 0usize..48,
        reverse in proptest::bool::ANY,
        orders in proptest::collection::vec(proptest::bool::ANY, 1..24),
    ) {
        fn run(padding:usize,reverse:bool,orders:&[bool])->(OwnershipFacts,Vec<hirsel_diag::Diagnostic>,Vec<SignatureDependency>,Vec<(UseKind,Modes)>,Vec<(ExprId,hirsel_types::Type)>,std::collections::HashSet<TypeId>,std::collections::HashSet<TypeId>) {
            let mut f=Fixture::new();for i in 0..padding {Symbol::intern(&format!("noise{i}"));}
            let record=f.new_type(TypeKind::Record(vec![Field {name:"a".into(),ty:f.bytes,growable:false},Field {name:"b".into(),ty:f.bytes,growable:false}].into()),unique());let x=f.parameter(record,PassingMode::Take);let read=f.read(x);let field=Symbol::intern("a");let projection=f.expr(ExprKind::Field {base:read,field},f.bytes);let owned=f.unary(UnaryOp::Unique,projection);let local=f.bind(owned,true);
            for (index,swap) in orders.iter().enumerate() {let read=f.read(local);let read=if *swap{f.unary(UnaryOp::Mut,read)}else{read};let call=f.call(&format!("callee{index}"),vec![read],vec![if *swap{PassingMode::Mut}else{PassingMode::Borrow}],vec![local_modes()],Modes::global());f.expr_stmt(call);}
            let mut rows:Vec<_>=std::mem::take(&mut f.provider.signatures).into_iter().collect();if reverse{rows.reverse();}for (path,signature) in rows{f.provider.signatures.insert(path,signature);}
            let mut out=f.check();let physical=std::mem::take(&mut out.value.physical_projections).into_iter().map(|(e,ty)|(e,session().get(ty).unwrap().clone())).collect();let plain=std::mem::take(&mut out.value.plain_types);let silent=std::mem::take(&mut out.value.silent_drop_types);let decisions=f.body.expressions.iter().map(|e|(e.usage,session().get(e.ty).unwrap().modes.clone())).collect();(out.value,out.diagnostics,out.dependencies,decisions,physical,plain,silent)
        }
        fn local_modes()->Modes { Modes {relation:Relation::environment(),..Modes::global()} }
        proptest::prop_assert_eq!(run(0,false,&orders),run(padding,reverse,&orders));
    }
}

#[test]
fn scoped_capture_loans_overlap_in_both_argument_orders() {
    for closure_first in [false, true] {
        let source = if closure_first {
            "fn both(f: fn() -> u64 uses {}, mut x: u64) { x = f() }\nfn example(mut x:u64) { both(fn() {x},mut x) }"
        } else {
            "fn both(mut x: u64, f: fn() -> u64 uses {}) { x = f() }\nfn example(mut x:u64) { both(mut x,fn() {x}) }"
        };
        let ds = corpus::source_diagnostics(source);
        assert_eq!(ds[0].rule.as_str(), "OWN-3", "{ds:?}");
    }
}
#[test]
fn exclusive_loans_precede_required_modes() {
    let mut f = Fixture::new();
    let x = f.parameter(f.bytes, PassingMode::Mut);
    let a = f.read(x);
    let b = f.read(x);
    let b = f.unary(UnaryOp::Mut, b);
    let call = f.call(
        "both",
        vec![a, b],
        vec![PassingMode::Borrow, PassingMode::Mut],
        vec![
            local(),
            Modes {
                relation: Relation::environment(),
                ..unique()
            },
        ],
        Modes::global(),
    );
    f.expr_stmt(call);
    let out = f.check();
    assert_eq!(out.diagnostics.len(), 1);
    assert_eq!(out.diagnostics[0].rule.as_str(), "OWN-3");
}
#[test]
fn borrowed_access_precedes_mode_requirements_at_a_call() {
    let mut f = Fixture::new();
    let x = f.parameter(f.bytes, PassingMode::Borrow);
    let read = f.read(x);
    let arg = f.unary(UnaryOp::Mut, read);
    let call = f.call(
        "require",
        vec![arg],
        vec![PassingMode::Mut],
        vec![unique()],
        Modes::global(),
    );
    f.expr_stmt(call);
    let out = f.check();
    assert_eq!(out.diagnostics.len(), 1, "{:?}", out.diagnostics);
    assert_eq!(out.diagnostics[0].rule.as_str(), "OWN-1");
}
#[test]
fn generic_nominal_copyability_uses_actual_owned_fields() {
    for resource in [false, true] {
        let mut f = Fixture::new();
        let ids = session().intern_graph(2, |ids| {
            vec![
                ty(TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0)))),
                ty(TypeKind::Record(
                    vec![Field {
                        name: "value".into(),
                        ty: ids[0],
                        growable: false,
                    }]
                    .into(),
                )),
            ]
        });
        let repr = ids[1];
        let mut declaration = resource_signature();
        declaration.definition = DefId::intern(&path("Wrapper"));
        if let SignatureKind::Type(td) = &mut declaration.kind {
            td.resource = false;
            td.representation = Some(repr);
            td.generics = vec![GenericParameter::Type {
                name: "T".into(),
                bounds: vec![].into(),
            }];
        }
        f.provider
            .signatures
            .insert(path("Wrapper"), std::sync::Arc::new(declaration));
        let argument = if resource {
            f.provider
                .signatures
                .insert(path("Token"), std::sync::Arc::new(resource_signature()));
            f.new_type(
                TypeKind::Named {
                    definition: DefId::intern(&path("Token")),
                    arguments: vec![].into(),
                },
                Modes::global(),
            )
        } else {
            f.scalar
        };
        let wrapper = f.new_type(
            TypeKind::Named {
                definition: DefId::intern(&path("Wrapper")),
                arguments: vec![GenericArgument::Type(argument)].into(),
            },
            Modes::global(),
        );
        let x = f.parameter(wrapper, PassingMode::Take);
        let read = f.read(x);
        let copy = f.unary(UnaryOp::Copy, read);
        f.expr_stmt(copy);
        let out = f.check();
        assert_eq!(
            out.diagnostics.first().map(|d| d.rule.as_str()),
            resource.then_some("OWN-5")
        );
    }
}

#[test]
fn plain_pattern_payload_retains_its_declared_modes() {
    let mut f = Fixture::new();
    let modes = Modes {
        sendability: Sendability::Sendable,
        ..Modes::global()
    };
    let scalar = f.new_type(TypeKind::Primitive(Primitive::U8), modes.clone());
    let tuple = f.new_type(TypeKind::Tuple(vec![scalar].into()), Modes::global());
    let literal = f.expr(
        ExprKind::Literal(hirsel_syntax::Literal::Integer("7u8".into())),
        scalar,
    );
    let source = f.expr(ExprKind::Tuple(vec![literal]), tuple);
    let index = f.bind(source, false);
    let NameSite::Pattern(root) = f.body.bindings[index].site else {
        unreachable!()
    };
    let child = hirsel_syntax::PatternId(f.syntax.tree.patterns.len() as u32);
    let name = Symbol::intern("payload");
    f.syntax.tree.patterns.push(hirsel_syntax::Pattern {
        origin: f.syntax.tree.patterns[root.0 as usize].origin.clone(),
        kind: hirsel_syntax::PatternKind::Binding(name),
    });
    f.syntax.tree.patterns[root.0 as usize].kind = hirsel_syntax::PatternKind::Tuple(vec![child]);
    f.body.patterns[0].kind = PatternDecisionKind::Tuple;
    f.body.patterns[0].fields = vec![(FieldId(0), child)];
    let ResolvedName::Local(local) = f.body.bindings[index].binding else {
        unreachable!()
    };
    f.body.bindings[index].site = NameSite::Pattern(child);
    f.body.bindings[index].ty = scalar;
    f.body.patterns.push(PatternDecision {
        pattern: child,
        ty: scalar,
        usage: UseKind::Copy,
        kind: PatternDecisionKind::Binding(local),
        fields: vec![],
        annotation: None,
    });
    let read = f.read(index);
    f.expr_stmt(read);
    let out = f.check();
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    let pattern = f.body.patterns.iter().find(|p| p.pattern == child).unwrap();
    assert_eq!(session().get(pattern.ty).unwrap().modes, modes);
    assert_eq!(
        session().get(f.body.bindings[index].ty).unwrap().modes,
        modes
    );
}

#[test]
fn call_consumption_is_separate_from_evaluating_fresh_copied_and_taken_arguments() {
    for operator in [None, Some(UnaryOp::Copy), Some(UnaryOp::Take)] {
        let mut f = Fixture::new();
        let argument = if let Some(operator) = operator {
            let binding = f.parameter(f.bytes, PassingMode::Take);
            let read = f.read(binding);
            f.unary(operator, read)
        } else {
            f.literal(f.bytes)
        };
        let call = f.call(
            "borrow",
            vec![argument],
            vec![PassingMode::Borrow],
            vec![Modes::global()],
            Modes::global(),
        );
        f.expr_stmt(call);
        let out = f.check();
        // `copy` of a last use passed to a borrow is redundant (an OWN-4 warning).
        assert!(
            out.diagnostics
                .iter()
                .all(|d| d.severity != hirsel_diag::Severity::Error),
            "{:?}",
            out.diagnostics
        );
        assert_eq!(
            out.diagnostics.len(),
            usize::from(operator == Some(UnaryOp::Copy))
        );
        assert_eq!(
            out.value.call_arguments,
            vec![(call, argument, UseKind::Borrow)]
        );
        assert_eq!(
            f.usage(argument),
            if operator == Some(UnaryOp::Copy) {
                UseKind::Copy
            } else {
                UseKind::Take
            }
        );
    }
}

#[test]
fn call_consumption_records_plain_copies_and_explicit_mut_and_take() {
    for (passing, operator, expected) in [
        (PassingMode::Borrow, None, UseKind::Copy),
        (PassingMode::Take, Some(UnaryOp::Take), UseKind::Take),
        (PassingMode::Mut, Some(UnaryOp::Mut), UseKind::Mut),
    ] {
        let mut f = Fixture::new();
        let binding = f.parameter(f.scalar, PassingMode::Mut);
        let read = f.read(binding);
        let argument = operator.map_or(read, |operator| f.unary(operator, read));
        let call = f.call(
            "consume",
            vec![argument],
            vec![passing],
            vec![Modes::global()],
            Modes::global(),
        );
        f.expr_stmt(call);
        let out = f.check();
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        assert_eq!(out.value.call_arguments, vec![(call, argument, expected)]);
    }
}

#[test]
fn slot_id_plainness_does_not_inspect_the_slot_payload() {
    let mut f = Fixture::new();
    let id = f.new_type(
        TypeKind::Builtin {
            kind: BuiltinType::SlotId,
            arguments: vec![GenericArgument::Type(f.bytes)].into(),
        },
        Modes::global(),
    );
    let binding = f.parameter(id, PassingMode::Borrow);
    let read = f.read(binding);
    f.expr_stmt(read);
    let out = f.check();
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    assert_eq!(f.usage(read), UseKind::Copy);
    assert_eq!(f.modes(read).sendability, Sendability::Sendable);
}

#[test]
fn invoking_a_once_capture_requires_owning_it_even_in_a_once_callback() {
    for (body, expected) in [
        ("action()", Some("OWN-1")),
        ("let owned = take action; owned()", None),
    ] {
        let source = format!(
            "fn invoke(f:once fn() uses {{}}) {{}}\nfn example(take action:once fn() uses {{}}) {{invoke(fn(){{{body}}})}}"
        );
        let diagnostics = corpus::source_diagnostics(&source);
        assert_eq!(
            diagnostics.first().map(|d| d.rule.as_str()),
            expected,
            "{source}: {diagnostics:?}"
        );
    }
}
