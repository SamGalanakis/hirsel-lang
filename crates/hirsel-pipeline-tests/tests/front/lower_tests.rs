//! hirsel-lower's unit tests that execute the lowered definitions in the interpreter. They
//! build their input with the lowering fixture of `crates/hirsel-lower/src/support.rs`.
#[path = "../../../hirsel-lower/src/support.rs"]
mod support;
use hirsel_intern::Symbol;
use hirsel_interp::{Execution, Limits, Outcome, TraceEvent, Value, run};
use hirsel_lower::*;
use hirsel_span::{SourceMap, Span};
use hirsel_syntax::MatchArm;
use hirsel_syntax::*;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_types::{Primitive as P, UseKind as U};
use std::sync::Arc;
use support::*;

impl Fixture {
    pub fn call_plan(
        &mut self,
        expression: ExprId,
        target: hirsel_types::CallTarget,
        ordinals: Vec<u32>,
    ) {
        let ExprKind::Call { callee, arguments } =
            &self.syntax.tree.expressions[expression.0 as usize].kind
        else {
            panic!("call fixture")
        };
        let parameters = if let Some(row) = self
            .checked
            .calls
            .iter()
            .find(|r| r.expression == expression)
        {
            let mut parameters = row
                .arguments
                .iter()
                .map(|a| a.parameter.clone().unwrap())
                .collect::<Vec<_>>();
            parameters.sort_by_key(|(i, _)| *i);
            parameters.into_iter().map(|(_, p)| p).collect::<Vec<_>>()
        } else {
            let hirsel_types::TypeKind::Function(f) = &session()
                .get(self.checked.expressions[callee.0 as usize].ty)
                .unwrap()
                .kind
            else {
                panic!("callable fixture")
            };
            f.parameters.to_vec()
        };
        let row = hirsel_types::CheckedCall {
            expression,
            target,
            arguments: arguments
                .iter()
                .zip(ordinals)
                .map(|(a, i)| hirsel_types::CheckedArgument {
                    expression: a.value,
                    parameter: Some((i, parameters[i as usize].clone())),
                })
                .collect(),
            generic_arguments: vec![],
            contextual_contracts: vec![],
            result: self.checked.expressions[expression.0 as usize].ty,
            state: hirsel_types::CallState::Complete,
        };
        self.checked.calls.retain(|r| r.expression != expression);
        self.checked.calls.push(row);
    }
    pub fn indirect_call_plan(&mut self, expression: ExprId, ordinals: Vec<u32>) {
        let ExprKind::Call { callee, .. } =
            self.syntax.tree.expressions[expression.0 as usize].kind
        else {
            panic!("call fixture")
        };
        self.call_plan(
            expression,
            hirsel_types::CallTarget::Value(callee),
            ordinals,
        );
    }
    pub fn nominal(&mut self, name: &str, representation: TypeId) {
        self.register(Signature {
            metadata: SignatureMetadata::default(),
            definition: DefId::intern(&path(name)),
            kind: SignatureKind::Type(Box::new(TypeDefinition {
                loop_source: None,
                format: None,
                generics: vec![],
                representation: Some(representation),
                destructor_allocation: Allocation::NoAlloc,
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
            })),
            fingerprint: ContentHash::default(),
        });
    }
}

/// Runs deep fixtures on a thread with the compiler's stack size, without
/// linking the driver.
pub fn with_large_stack<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(hirsel_intern::jobs::COMPILER_STACK_BYTES)
            .spawn_scoped(scope, work)
            .unwrap_or_else(|error| hirsel_guard::spawn_failed(128 * 1024 * 1024, error))
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

pub struct Provider(pub Vec<Definition>);

impl TirProvider for Provider {
    fn constant(
        &mut self,
        _path: &hirsel_syntax::DefPath,
    ) -> std::result::Result<hirsel_tir::MaterializedRef, hirsel_types::QueryError> {
        Err(hirsel_types::QueryError::NotImplemented)
    }
    fn constant_definitions(
        &mut self,
    ) -> std::result::Result<Vec<hirsel_syntax::DefPath>, hirsel_types::QueryError> {
        Ok(vec![])
    }

    fn definition(&mut self, path: &DefPath) -> std::result::Result<DefinitionRef, QueryError> {
        self.0
            .iter()
            .find(|d| d.path.path() == path)
            .cloned()
            .map(Arc::new)
            .ok_or_else(|| QueryError::NotImplemented)
    }
}

pub fn execute(definitions: Vec<Definition>, arguments: Vec<hirsel_interp::Value>) -> Execution {
    let mut sources = SourceMap::default();
    let file = sources.add("fixture.hir", "x\n".repeat(100)).unwrap();
    sources.move_anchor(fixture_anchor(), file, 0);
    run(
        &mut Provider(definitions),
        session_arc(),
        &sources,
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 0,
            inputs: vec![],
        },
        arguments,
        Limits::default(),
    )
    .unwrap()
}

pub fn returned(execution: Execution) -> hirsel_interp::Value {
    match execution.outcome {
        hirsel_interp::Outcome::Returned { value, .. } => value,
        outcome => panic!("unexpected {outcome:?}"),
    }
}

pub fn bits(execution: Execution) -> u128 {
    match returned(execution) {
        hirsel_interp::Value::Integer(i) => i.bits(),
        value => panic!("expected integer, got {value:?}"),
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
fn snapshot_shape_comptime_todo_and_local_function() {
    let mut f = Fixture::new(P::U8);
    let shape = f.type_(TypeKind::Builtin {
        kind: hirsel_types::BuiltinType::TypeShape,
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
        context: hirsel_types::CaptureContext::Scoped,
        result_evidence: None,
        expression: body,
        ty: function,
        captures: vec![],
    });
    let stmt = f.stmt(
        StmtKind::Function {
            name: Symbol::intern("item"),
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
        let q = PatternId(f.syntax.tree.patterns.len() as u32);
        f.syntax.tree.patterns.push(Pattern {
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
            label: None,
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
fn formal_frame_result_rows_override_pre_mode_results_and_require_complete_owners() {
    let mut f = Fixture::new(P::U8);
    let root = f.integer(7, 1);
    let mut row = session().row(f.scalar).clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let result = session().intern(row);
    assert_ne!(result, f.scalar);
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
            context: hirsel_types::CaptureContext::Scoped,
            result_evidence: None,
            expression: closure,
            ty: function,
            captures: vec![],
        },
        f.scalar,
        f.owner,
    );
    let mut row = session().row(f.scalar).clone();
    row.modes.uniqueness = Uniqueness::Unique;
    let child_result = session().intern(row);
    assert_ne!(child_result, f.scalar);
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

// The annotation selects an owning copy; the unannotated twin borrows.
// Both read the original box after the closure's block has destroyed it.
fn source_capture_preserves_selected_types(annotation: &str, context: CaptureContext, usage: U) {
    use hirsel_engine::DefinitionQueries;

    let source = format!(
        "type P {{ a: u8 }}\nfn main() -> u8 {{ let t: unique box P = box P(a: 4u8); let n = {{ let c{annotation} = fn() -> u8 {{ t.a }}; c() }}; n + t.a }}"
    );
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let file_context = hirsel_engine::default_context();
    let file = engine.sources.add("capture.hir", source.clone()).unwrap();
    let parsed = engine.parse_file(file, &file_context).unwrap();
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let path = hirsel_engine::entry_path(&file_context);
    let analyzed = engine.mode_definition(&path).unwrap();
    assert!(
        analyzed.diagnostics.is_empty(),
        "{:?}",
        analyzed.diagnostics
    );
    let checked = &analyzed.value.typed.checked;
    let [closure] = checked.closures.as_slice() else {
        panic!("expected one source closure");
    };
    assert_eq!(closure.context, context);
    let [capture] = closure.captures.as_slice() else {
        panic!("expected one boxed capture");
    };
    assert_eq!(capture.usage, usage);
    let [mode] = analyzed.value.ownership.captures.as_slice() else {
        panic!("expected one capture mode");
    };
    assert_eq!((mode.context, mode.usage), (context, usage));
    let source_type = checked
        .bindings
        .iter()
        .find(|b| b.owner == NameSite::Item && b.binding == capture.binding)
        .unwrap()
        .ty;
    let types = engine.types();
    assert!(matches!(types.row(source_type).kind, TypeKind::Box(_)));
    assert_eq!(types.row(source_type).modes.uniqueness, Uniqueness::Unique);
    if usage == U::Copy {
        assert_eq!(types.row(capture.ty).modes.uniqueness, Uniqueness::Aliased);
    }
    let boxed = analyzed
        .value
        .typed
        .syntax
        .tree
        .expressions
        .iter()
        .position(|e| {
            matches!(
                e.kind,
                ExprKind::Unary {
                    op: UnaryOp::Box,
                    ..
                }
            )
        })
        .unwrap();
    assert_eq!(checked.expressions[boxed].ty, source_type);

    let definition = engine.lower_to_tir(&path).unwrap();
    let snapshot = engine.signature_snapshot();
    let signatures = snapshot.validation(&path.0.package);
    hirsel_validate::assert_valid(&definition.value, engine.types(), signatures);
    let parent = &definition.value.bodies[definition.value.entry.0 as usize];
    let (child, operand) = parent
        .rvalues
        .iter()
        .find_map(|r| match r {
            Rvalue::Closure {
                context: actual,
                body,
                captures,
                ..
            } => {
                assert_eq!(*actual, context);
                let [operand] = captures.as_slice() else {
                    panic!("expected one operand")
                };
                Some((*body, operand))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(operand.usage, usage);
    assert_eq!(parent.places[operand.place.0 as usize].ty, source_type);
    let child = &definition.value.bodies[child.0 as usize];
    let field = child
        .locals
        .iter()
        .find(|l| l.storage == Storage::Capture)
        .unwrap();
    assert_eq!(field.ty, capture.ty);
    assert_eq!(
        crate::check_support::bits(crate::check_support::execute_source(&source)),
        8
    );
}

#[test]
fn source_copy_capture_preserves_selected_types() {
    source_capture_preserves_selected_types(": fn() -> u8", CaptureContext::Escaping, U::Copy);
}

#[test]
fn source_borrow_capture_preserves_selected_types() {
    source_capture_preserves_selected_types("", CaptureContext::Scoped, U::Borrow);
}

#[test]
fn copy_capture_preserves_the_source_type_and_j_selected_aliased_capture_type() {
    copy_capture_preserves_selected_types(true);
}

#[test]
fn immutable_copy_capture_keeps_unique_source_and_j_selected_aliased_capture_type() {
    copy_capture_preserves_selected_types(false);
}

fn copy_capture_preserves_selected_types(mutable: bool) {
    let mut f = Fixture::new(P::U8);
    let mut source_type = ty(TypeKind::Box(f.scalar));
    source_type.modes.uniqueness = hirsel_types::Uniqueness::Unique;
    let source_type = session().intern(source_type);
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
    let StmtKind::Let { mutable: mode, .. } = &mut f.syntax.tree.statements[let_.0 as usize].kind
    else {
        unreachable!()
    };
    *mode = mutable;
    f.checked.bindings.last_mut().unwrap().mutable = mutable;
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
            // A counted copy owns its capture; Scoped would claim a borrow.
            context: hirsel_types::CaptureContext::Escaping,
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
    assert_eq!(f.ownership.captures[0].context, CaptureContext::Escaping);
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
    let parent = &definition.bodies[0];
    let operand = parent
        .rvalues
        .iter()
        .find_map(|r| match r {
            Rvalue::Closure {
                context, captures, ..
            } => {
                assert_eq!(*context, CaptureContext::Escaping);
                Some(&captures[0])
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(operand.usage, U::Copy);
    // MODE-2: copying ends current uniqueness. Since d75d714d, a mutable
    // binding's receiving storage is aliased; its checked source type stays unique.
    let source_place_type = parent.places[operand.place.0 as usize].ty;
    assert_eq!(
        source_place_type,
        if mutable { capture_type } else { source_type }
    );
    assert_eq!(
        session().row(source_place_type).modes.uniqueness,
        if mutable {
            Uniqueness::Aliased
        } else {
            Uniqueness::Unique
        }
    );
    assert_eq!(
        session().row(source_type).modes.uniqueness,
        Uniqueness::Unique
    );
    assert_eq!(
        session().row(capture_type).modes.uniqueness,
        Uniqueness::Aliased
    );
    assert_eq!(bits(execute(vec![definition], vec![])), 7);
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
            f.plans
                .materialized
                .push((expression, Materialized { root: node }));
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
