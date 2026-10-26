use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_span::{SourceMap, Span};
use hirsel_syntax::{ContentHash, DefPath, ItemPath};
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::*;

struct Fixture(Definition);
impl TirProvider for Fixture {
    fn constant(
        &mut self,
        _path: &hirsel_syntax::DefPath,
    ) -> Result<hirsel_tir::MaterializedRef, hirsel_types::QueryError> {
        Err(hirsel_types::QueryError::NotImplemented)
    }
    fn constant_definitions(
        &mut self,
    ) -> Result<Vec<hirsel_syntax::DefPath>, hirsel_types::QueryError> {
        Ok(vec![])
    }

    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        (|| {
            assert_eq!(path, self.0.path.path());
            Ok(self.0.clone())
        })()
        .map(std::sync::Arc::new)
    }
}
fn fixture(fail: bool) -> (Fixture, SourceMap, RunConfiguration) {
    let context = default_context();
    let path = DefPath(ItemPath {
        package: context.package,
        modules: vec![],
        name: "main".into(),
    });
    let mut sources = SourceMap::default();
    let file = sources.add("runtime.hir", "\n\nfixture\n".into()).unwrap();
    let span = Span::new(sources.file_anchor(file), 2, 9).unwrap();
    let types = hirsel_engine::registry::session_types();
    let unit = types.intern(Type {
        drop_allocation: hirsel_types::DropAllocation::MayAllocate,
        ..Type::with_kind(TypeKind::Tuple(vec![].into()))
    });
    let body = Body {
        arena_scope_grant: None,
        scoped_parameters: Vec::new(),
        parameter_scope: hirsel_types::LexicalOwner::first(&path)
            .site(hirsel_types::NameSite::Item),
        locals: vec![Local {
            ty: unit,
            storage: Storage::Temporary,
            scope: ScopeId(0),
            span,
        }],
        places: vec![Place {
            loan: None,
            binding: None,
            root: LocalId(0),
            projection: vec![].into(),
            ty: unit,
        }],
        scopes: vec![Scope { parent: None, span }],
        blocks: vec![Block {
            instructions: vec![Instruction {
                kind: InstructionKind::Assign {
                    destination: PlaceId(0),
                    value: RvalueId(0),
                },
                span,
                on_failure: CleanupId(0),
            }],
            terminator: if fail {
                Terminator::Fail {
                    rule: const { hirsel_diag::RuleId::of("FAIL-1") },
                    cleanup: CleanupId(0),
                }
            } else {
                Terminator::Return {
                    value: Operand {
                        place: PlaceId(0),
                        usage: UseKind::Take,
                    },
                    cleanup: CleanupId(0),
                }
            },
            span,
        }],
        rvalues: vec![Rvalue::Constant(Constant::Unit)],
        cleanup_nodes: vec![],
        cleanups: vec![Cleanup::default()],
        entry: BlockId(0),
        parameters: vec![],
        result: unit,
        borrow_cursors: vec![],
        tail_sites: vec![],
        matches: vec![],
    };
    let mut definition = Definition {
        caller_bodies: vec![],
        protocol_associations: vec![],
        types: std::sync::Arc::from([]),
        path: hirsel_syntax::DefId::intern(&path),
        inferred_unique: Default::default(),
        signature_hash: ContentHash::default(),
        bodies: vec![body],
        entry: BodyId(0),
        contracts: vec![],
    };
    definition.types = hirsel_tir::definition_types(&types, &definition);
    (
        Fixture(definition),
        sources,
        RunConfiguration {
            entry: path,
            hash_seed: 123,
            inputs: vec![],
        },
    )
}
#[test]
fn invalid_tir_remains_a_tool_error() {
    hirsel_engine::with_compiler_stack(invalid_tir_remains_a_tool_error_on_compiler_thread);
}
fn invalid_tir_remains_a_tool_error_on_compiler_thread() {
    let (mut provider, sources, config) = fixture(false);
    provider.0.bodies.clear();
    assert!(matches!(
        InterpreterAdapter::default().run_query(&config, &mut provider, &mut vec![], &sources),
        Err(RuntimeError::Tool(_))
    ));
}

#[test]
fn library_launch_uses_declared_runtime_types_and_exact_simulated_output() {
    hirsel_engine::with_compiler_stack(
        library_launch_uses_declared_runtime_types_and_exact_simulated_output_on_compiler_thread,
    );
}
fn library_launch_uses_declared_runtime_types_and_exact_simulated_output_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add(
            "launch.hir",
            "pub fn main(rt: Io) -> Result[(),OutputError] {}\n".into(),
        )
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    let signature = engine
        .item_signature(&entry_path(&default_context()))
        .unwrap()
        .value;
    let registry = registry::BuiltinRegistry::new();
    let output = registry
        .get(&hirsel_syntax::DefPath::core("out.print"))
        .unwrap();
    let stdout_signature = registry
        .get(&hirsel_syntax::DefPath::core("io.stdout"))
        .unwrap();
    let SignatureKind::Function(print) = &output.kind else {
        panic!()
    };
    let SignatureKind::Function(project) = &stdout_signature.kind else {
        panic!()
    };
    // This hand-built TIR fixture declares the registry's exact operation result.
    // Source normalizer/body compatibility is separately inventoried by conformance.
    let mut declared = (*signature).clone();
    // The engine's interner extends the registry's base, so registry ids hold.
    let result = print.result;
    let SignatureKind::Function(owner) = &mut declared.kind else {
        panic!()
    };
    owner.result = result;
    declared.fingerprint =
        hirsel_types::encoding::fingerprint(engine.types(), &declared, &[]).unwrap();
    let signature = std::sync::Arc::new(declared);
    let SignatureKind::Function(owner) = &signature.kind else {
        panic!()
    };
    let text = print.parameters[1].ty;
    let projected = project.result;
    let TypeKind::Named { arguments, .. } = &engine.types().row(projected).kind else {
        panic!()
    };
    let GenericArgument::Type(stdout) = arguments[0] else {
        panic!()
    };
    let span = Span::new(engine.sources.file_anchor(file), 0, 1).unwrap();
    let local_types = [owner.parameters[0].ty, text, projected, owner.result];
    let argument = |index: u32, usage| CallArgument {
        parameter: ParamId(index),
        value: Operand {
            place: PlaceId(index),
            usage,
        },
    };
    let mut definition = Definition {
        caller_bodies: vec![],
        protocol_associations: vec![],
        types: std::sync::Arc::from([]),
        path: signature.definition,
        inferred_unique: Default::default(),
        signature_hash: signature.fingerprint,
        entry: BodyId(0),
        contracts: vec![],
        bodies: vec![Body {
            arena_scope_grant: None,
            scoped_parameters: Vec::new(),
            parameter_scope: hirsel_types::LexicalOwner::first(signature.definition.path())
                .site(hirsel_types::NameSite::Item),
            locals: local_types
                .iter()
                .enumerate()
                .map(|(i, t)| Local {
                    ty: *t,
                    storage: if i == 0 {
                        Storage::Parameter
                    } else {
                        Storage::Temporary
                    },
                    scope: ScopeId(0),
                    span,
                })
                .collect(),
            places: local_types
                .iter()
                .enumerate()
                .map(|(i, t)| Place {
                    loan: None,
                    binding: None,
                    root: LocalId(i as u32),
                    projection: vec![].into(),
                    ty: *t,
                })
                .chain(std::iter::once(Place {
                    loan: Some(LoanId(0)),
                    binding: None,
                    root: LocalId(1),
                    projection: vec![].into(),
                    ty: text,
                }))
                .chain(std::iter::once(Place {
                    loan: None,
                    binding: None,
                    root: LocalId(2),
                    projection: vec![Projection::VariantField {
                        variant: 0,
                        field: 0,
                    }]
                    .into(),
                    ty: stdout,
                }))
                .collect(),
            scopes: vec![Scope { parent: None, span }],
            blocks: vec![
                Block {
                    instructions: vec![Instruction {
                        kind: InstructionKind::Assign {
                            destination: PlaceId(1),
                            value: RvalueId(0),
                        },
                        span,
                        on_failure: CleanupId(0),
                    }],
                    terminator: Terminator::Call {
                        callee: Callee::Definition {
                            path: hirsel_syntax::DefPath::core("io.stdout"),
                            arguments: vec![],
                        },
                        arguments: vec![argument(0, UseKind::Borrow)],
                        returns: Some(CallReturn {
                            destination: PlaceId(2),
                            next: BlockId(3),
                        }),
                        on_failure: CleanupId(1),
                    },
                    span,
                },
                Block {
                    instructions: vec![Instruction {
                        kind: InstructionKind::BeginLoan {
                            id: LoanId(0),
                            place: PlaceId(1),
                            mutable: false,
                            if_inactive: false,
                        },
                        span,
                        on_failure: CleanupId(1),
                    }],
                    terminator: Terminator::Call {
                        callee: Callee::Definition {
                            path: hirsel_syntax::DefPath::core("out.print"),
                            arguments: vec![],
                        },
                        arguments: vec![
                            CallArgument {
                                parameter: ParamId(0),
                                value: Operand {
                                    place: PlaceId(5),
                                    usage: UseKind::Copy,
                                },
                            },
                            CallArgument {
                                parameter: ParamId(1),
                                value: Operand {
                                    place: PlaceId(4),
                                    usage: UseKind::Borrow,
                                },
                            },
                        ],
                        returns: Some(CallReturn {
                            destination: PlaceId(3),
                            next: BlockId(2),
                        }),
                        on_failure: CleanupId(2),
                    },
                    span,
                },
                Block {
                    instructions: vec![Instruction {
                        kind: InstructionKind::EndLoan {
                            loan: LoanId(0),
                            if_active: false,
                        },
                        span,
                        on_failure: CleanupId(2),
                    }],
                    terminator: Terminator::Return {
                        value: Operand {
                            place: PlaceId(3),
                            usage: UseKind::Take,
                        },
                        cleanup: CleanupId(1),
                    },
                    span,
                },
                Block {
                    instructions: vec![],
                    terminator: Terminator::Match {
                        table: MatchTableId(0),
                        from: 0,
                    },
                    span,
                },
                Block {
                    instructions: vec![],
                    // This fixture explicitly requires its configured stdout grant.
                    terminator: Terminator::Fail {
                        rule: const { hirsel_diag::RuleId::of("CAP-2") },
                        cleanup: CleanupId(1),
                    },
                    span,
                },
            ],
            rvalues: vec![Rvalue::Constant(Constant::String("hello\n".into()))],
            cleanup_nodes: vec![
                CleanupNode {
                    action: CleanupAction::Drop {
                        place: PlaceId(1),
                        if_live: false,
                    },
                    parent: None,
                },
                CleanupNode {
                    action: CleanupAction::EndLoan {
                        loan: LoanId(0),
                        if_active: false,
                    },
                    parent: Some(CleanupNodeId(0)),
                },
            ],
            cleanups: vec![
                Cleanup::default(),
                Cleanup {
                    head: Some(CleanupNodeId(0)),
                },
                Cleanup {
                    head: Some(CleanupNodeId(1)),
                },
            ],
            entry: BlockId(0),
            parameters: vec![LocalId(0)],
            result: owner.result,
            borrow_cursors: vec![],
            tail_sites: vec![],
            matches: vec![MatchTable {
                scrutinee: PlaceId(2),
                patterns: vec![
                    MatchPattern {
                        place: PlaceId(5),
                        kind: MatchPatternKind::Wildcard,
                    },
                    MatchPattern {
                        place: PlaceId(2),
                        kind: MatchPatternKind::Variant {
                            definition: hirsel_syntax::DefPath::core("Result"),
                            variant: 0,
                            fields: vec![(0, MatchPatternId(0))],
                        },
                    },
                    MatchPattern {
                        place: PlaceId(2),
                        kind: MatchPatternKind::Wildcard,
                    },
                ],
                arms: vec![
                    MatchArm {
                        pattern: MatchPatternId(1),
                        guard: None,
                        target: BlockId(1),
                    },
                    MatchArm {
                        pattern: MatchPatternId(2),
                        guard: None,
                        target: BlockId(4),
                    },
                ],
            }],
        }],
    };
    definition.types = hirsel_tir::definition_types(engine.types(), &definition);
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    signatures.extend_signatures(registry.signatures().map(|s| (**s).clone()));
    signatures.insert(signature.definition, (*signature).clone());
    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
    let runtime = engine.runtime_types().unwrap();
    let config = RunConfiguration {
        entry: signature.definition.path().clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut adapter = InterpreterAdapter::default();
    adapter.prepare(signature, runtime, SimulationConfig::default());
    let mut bytes = vec![];
    assert_eq!(
        adapter
            .run_query(
                &config,
                &mut Fixture(definition),
                &mut bytes,
                &engine.sources
            )
            .unwrap(),
        RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
    );
    assert_eq!(bytes, b"hello\n");
}

#[test]
fn generic_sizes_execute_in_forwarded_calls_and_escaping_closures() {
    hirsel_engine::with_compiler_stack(
        generic_sizes_execute_in_forwarded_calls_and_escaping_closures_on_compiler_thread,
    );
}
fn generic_sizes_execute_in_forwarded_calls_and_escaping_closures_on_compiler_thread() {
    let source = r#"
fn output_error[E](take e:E) {}
resource type Sized[n:size] {payload:Inline[u32,n], stdout:Stdout}
    drop(x) {let _=out.print(x.stdout,if n==3 {"drop three\n"} else {"drop one\n"})}
fn make[n:size](xs:Inline[u32,n],stdout:Stdout)->Sized[n] {Sized(payload:xs,stdout:stdout)}
@noalloc fn length[T:plain, n:size](xs:Inline[T,n])->u64 {n}
@noalloc fn forwarded[T:plain, n:size](xs:Inline[T,n])->u64 {length(xs)}
fn delayed[T:plain, n:size](xs:Inline[T,n])->(fn()->u64 uses {}) {fn(){n}}
pub fn main(rt:Io,args:Args)->Result[(),()] {
    let a:Inline[u32,3]=[1,2,3]
    let b:Inline[u32,1]=[4]
    let three:Sized[3]=make(a,io.stdout(rt)? grant_error)
    let one:Sized[1]=make(b,io.stdout(rt)? grant_error)
    let first=delayed(a)
    let second=delayed(b)
    let ok=forwarded(a)==3 && forwarded(b)==1 && first()==3 && second()==1
    out.print(io.stdout(rt)? grant_error,if ok {"sizes ok\n"} else {"wrong sizes\n"})?output_error
    Ok(())
}

@noalloc
fn grant_error(refusal: GrantError) -> () uses {} { () }
"#;
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("sizes.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert_eq!(checked.completion, CheckCompletion::Complete);
    let mut stdout = vec![];
    let result = run_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry_path(&default_context()),
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
        &mut stdout,
    )
    .unwrap();
    assert_eq!(
        result,
        RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
    );
    assert_eq!(stdout, b"sizes ok\ndrop one\ndrop three\n");
}

#[test]
fn borrowed_branch_results_execute_both_alternatives() {
    hirsel_engine::with_compiler_stack(
        borrowed_branch_results_execute_both_alternatives_on_compiler_thread,
    );
}
fn borrowed_branch_results_execute_both_alternatives_on_compiler_thread() {
    let source = r#"
fn output_error[E](take e:E) {}
fn choose(a:Bytes,b:Bytes,pick:bool)->Bytes in {a, b} {if pick {a} else {b}}
pub fn main(rt:Io,args:Args)->Result[(),()] {
    let a=choose(b"first",b"second",true)
    let b=choose(b"first",b"second",false)
    out.print(io.stdout(rt)? grant_error,if a==b"first" && b==b"second" {"branches ok\n"} else {"wrong\n"})?output_error
    Ok(())
}

@noalloc
fn grant_error(refusal: GrantError) -> () uses {} { () }
"#;
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("branches.hir", source.into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert_eq!(checked.completion, CheckCompletion::Complete);
    let mut stdout = vec![];
    let result = run_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry_path(&default_context()),
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
        &mut stdout,
    )
    .unwrap();
    assert_eq!(
        result,
        RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
    );
    assert_eq!(stdout, b"branches ok\n");
}

#[test]
fn ambient_heap_token_executes_without_an_allocation_or_capability() {
    hirsel_engine::with_compiler_stack(
        ambient_heap_token_executes_without_an_allocation_or_capability_on_compiler_thread,
    );
}
fn ambient_heap_token_executes_without_an_allocation_or_capability_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("heap.hir", "@noalloc pub fn main() -> Result[(),OutputError] { let destination = heap; let alias = destination; Ok(()) }".into()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let result = run_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry_path(&default_context()),
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
        &mut vec![],
    )
    .unwrap();
    assert_eq!(
        result,
        RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
    );
}

#[test]
fn consuming_cursors_release_yielded_and_remaining_resources_exactly_once() {
    hirsel_engine::with_compiler_stack(
        consuming_cursors_release_yielded_and_remaining_resources_exactly_once_on_compiler_thread,
    );
}
fn consuming_cursors_release_yielded_and_remaining_resources_exactly_once_on_compiler_thread() {
    for container in [
        "Array[Token]",
        "List[Token]",
        "Inline[Token,3]",
        "Inline[Token,0]",
    ] {
        for (action, expected, fails) in [
            ("let _ = take item", "one\ntwo\nthree\n", false),
            ("let _ = take item; continue", "one\ntwo\nthree\n", false),
            ("let _ = take item; break", "one\nthree\ntwo\n", false),
            (
                "let _ = take item; return Ok(())",
                "one\nthree\ntwo\n",
                false,
            ),
            ("let _ = take item; explode(0)", "one\nthree\ntwo\n", true),
        ] {
            let empty = container.ends_with(",0]");
            let elements = if empty {
                ""
            } else {
                "Token(stdout:io.stdout(rt)?,number:1),Token(stdout:io.stdout(rt)?,number:2),Token(stdout:io.stdout(rt)?,number:3)"
            };
            let source = format!(
                r#"
resource type Token {{stdout:Stdout, number:u8}}
    drop(x) {{let _ = out.print(x.stdout, if x.number==1 {{"one\n"}} else {{if x.number==2 {{"two\n"}} else {{"three\n"}}}})}}
fn explode(divisor:u8) {{ let _ = 1u8 / divisor }}
pub fn main(rt:Io) -> Result[(),GrantError] {{
    let xs:{container} = [{elements}]
    for item in take xs {{ {action} }}
    Ok(())
}}
"#
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("consume.hir", source.clone()).unwrap();
            let checked = check_progress(&mut engine, file, &default_context())
                .unwrap_or_else(|e| panic!("{source}: {e:?}"));
            assert!(
                checked.diagnostics.is_empty(),
                "{source}: {:?}",
                checked.diagnostics
            );
            let mut stdout = vec![];
            let result = run_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry_path(&default_context()),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
                &mut stdout,
            )
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
            if fails && !empty {
                assert!(matches!(result, RunOutcome::Failed { .. }));
            } else {
                assert_eq!(
                    result,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                );
            }
            assert_eq!(
                stdout,
                if empty { b"" } else { expected.as_bytes() },
                "{container}: {action}"
            );
        }
    }
}

#[test]
fn observed_api_failure_retains_stdout_and_inner_cause() {
    hirsel_engine::with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("unavailable.hir", "fn charge[T: copyable](x: T) -> u8 { match arena.bounded(64, fn(a: Arena) -> u8 { let c = copy_to(a, x); 0u8 }) { Ok(n) => n, Error(e) => 1u8 } }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let _ = out.print(io.stdout(rt)?, \"before unavailable\\n\"); let n = charge(7u8); Ok(()) }".into()).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let result = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry_path(&default_context()),
                hash_seed: 0,
                inputs: vec![],
            },
            Default::default(),
        )
        .unwrap();
        // a51a3c77 preserves the inner refusal instead of erasing its reason.
        assert!(
            matches!(
                result.outcome,
                Err(RuntimeError::Tool(ref message))
                    if message == "launch unavailable: a bounded-arena charge of a generic type"
            ),
            "{:?}",
            result.outcome
        );
        assert!(result.observation.is_none());
        assert_eq!(result.tooling_output, b"before unavailable\n");
    });
}

#[test]
fn strict_and_mixed_replacement_failures_keep_the_new_value_in_named_cleanup_order() {
    hirsel_engine::with_compiler_stack(|| {
        for source in [
            include_str!("../../../../conformance/DROP/DROP-2-moved-storage-failure-strict.hir"),
            include_str!("../../../../conformance/DROP/DROP-2-moved-storage-failure-mixed.hir"),
        ] {
            let expected_line = source
                .lines()
                .find_map(|line| line.strip_prefix("// expect: fail SEM-1 at line "))
                .expect("fixture must expect SEM-1 failure")
                .parse::<usize>()
                .unwrap();
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("replace-failure.hir", source.into())
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let mut stdout = vec![];
            let result = run_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry_path(&default_context()),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
                &mut stdout,
            )
            .unwrap();
            assert!(
                matches!(result, RunOutcome::Failed { primary: hirsel_run_schema::Failure { ref rule, ref site, .. }, .. } if rule.as_str()=="SEM-1" && site.line == expected_line as u64),
                "{result:?}"
            );
            assert_eq!(stdout, b"old\nlater\nnew\n");
        }
    });
}

#[test]
fn all_four_projection_controls_propagate_not_granted_without_dummy_capabilities() {
    hirsel_engine::with_compiler_stack(|| {
        for (operation, source) in [
            ("stdout", include_str!("../fixtures/grants/stdout.hir")),
            ("tasks", include_str!("../fixtures/grants/tasks.hir")),
            ("net", include_str!("../fixtures/grants/net.hir")),
            ("random", include_str!("../fixtures/grants/random.hir")),
        ] {
            for granted in [false, true] {
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine.sources.add("grant.hir", source.into()).unwrap();
                let checked = check_progress(&mut engine, file, &default_context()).unwrap();
                assert!(
                    checked.diagnostics.is_empty(),
                    "{operation}: {:?}",
                    checked.diagnostics
                );
                assert_eq!(checked.completion, CheckCompletion::Complete);
                let config = SimulationConfig {
                    projections: if granted {
                        [operation.to_owned()].into()
                    } else {
                        Default::default()
                    },
                    ..Default::default()
                };
                let mut stdout = vec![];
                let result = run_snapshot(
                    &mut engine,
                    &RunConfiguration {
                        entry: entry_path(&default_context()),
                        hash_seed: 0,
                        inputs: vec![],
                    },
                    config,
                    &mut stdout,
                )
                .unwrap();
                assert_eq!(
                    result,
                    RunOutcome::Returned(if granted {
                        hirsel_run_schema::Returned::Ok
                    } else {
                        hirsel_run_schema::Returned::Error
                    }),
                    "{operation}"
                );
                assert!(stdout.is_empty());
            }
        }
    });
}
