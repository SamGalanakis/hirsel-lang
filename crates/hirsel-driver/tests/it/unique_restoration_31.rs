//! Release-backed confined-result restoration and its conservative boundary.
use hirsel_driver::*;
use hirsel_engine::*;

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("unique-restoration-31.hir", source.into())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    engine
}

fn runtime(source: &str) {
    use hirsel_driver::native::*;
    let mut engine = checked(source);
    let entry = entry_path(&default_context());
    let run = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 29,
            inputs: vec![],
        },
        Default::default(),
    )
    .unwrap();
    let expected = Observation::of_reference(run.observation.as_ref().unwrap());
    if source.contains("resource type Bomb") {
        assert_eq!(expected.exit_code, None);
        assert_eq!(expected.failures.len(), 1);
        assert_eq!(expected.failures[0].rule, "SEM-1");
        assert_eq!(expected.failures[0].definition.0.name, "Bomb");
        assert_eq!(
            (expected.failures[0].line, expected.failures[0].column),
            (2, 19)
        );
        assert!(!expected.blocked && expected.budget.is_none() && expected.signaled.is_none());
    } else {
        assert_eq!(expected.exit_code, Some(0), "{expected:?}");
        assert!(expected.failures.is_empty(), "{expected:?}");
    }
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    options.validation = Validation::EveryStage;
    let ready = build(&validated, &options).unwrap();
    let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
    assert_eq!(heap.observation, expected, "checked H");
    assert_eq!(heap.counters.live_heap_bytes, 0);
    let matrix = [
        Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
        Configuration {
            options,
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
    ];
    let scratch = tempfile::Builder::new()
        .prefix("unique-restoration-31-29-")
        .tempdir()
        .unwrap();
    let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
    let outcomes = differential(
        &validated,
        &entry,
        &expected,
        &matrix,
        &toolchain,
        scratch.path(),
    );
    assert_eq!(outcomes.len(), 2);
    for (configuration, outcome) in outcomes {
        assert!(
            matches!(outcome, Outcome::Agree),
            "{configuration}: {outcome:?}"
        );
    }
}

const CASES: &[(&str, &str, &str)] = &[
    (
        "branch-retained.hir",
        include_str!("../fixtures/unique-restoration-31/branch-retained.hir"),
        "MODE-1",
    ),
    (
        "cleanup-repaired.hir",
        include_str!("../fixtures/unique-restoration-31/cleanup-repaired.hir"),
        "accept",
    ),
    (
        "closure-capture.hir",
        include_str!("../fixtures/unique-restoration-31/closure-capture.hir"),
        "MODE-1",
    ),
    (
        "dead-copy.hir",
        include_str!("../fixtures/unique-restoration-31/dead-copy.hir"),
        "MODE-2",
    ),
    (
        "deep-copy-result.hir",
        include_str!("../fixtures/unique-restoration-31/deep-copy-result.hir"),
        "accept",
    ),
    (
        "deep-copy-root.hir",
        include_str!("../fixtures/unique-restoration-31/deep-copy-root.hir"),
        "accept",
    ),
    (
        "escape-take.hir",
        include_str!("../fixtures/unique-restoration-31/escape-take.hir"),
        "MODE-2",
    ),
    (
        "failing-cleanup.hir",
        include_str!("../fixtures/unique-restoration-31/failing-cleanup.hir"),
        "MODE-2",
    ),
    (
        "live-saved-deep-copy.hir",
        include_str!("../fixtures/unique-restoration-31/live-saved-deep-copy.hir"),
        "MODE-1",
    ),
    (
        "lookup-leaf.hir",
        include_str!("../fixtures/unique-restoration-31/lookup-leaf.hir"),
        "accept",
    ),
    (
        "loop-carried.hir",
        include_str!("../fixtures/unique-restoration-31/loop-carried.hir"),
        "MODE-2",
    ),
    (
        "miss-lookup.hir",
        include_str!("../fixtures/unique-restoration-31/miss-lookup.hir"),
        "accept",
    ),
    (
        "nested-payload.hir",
        include_str!("../fixtures/unique-restoration-31/nested-payload.hir"),
        "MODE-1",
    ),
    (
        "no-share.hir",
        include_str!("../fixtures/unique-restoration-31/no-share.hir"),
        "accept",
    ),
    (
        "opaque-return.hir",
        include_str!("../fixtures/unique-restoration-31/opaque-return.hir"),
        "MODE-2",
    ),
    (
        "ordinary-dead-copy.hir",
        include_str!("../fixtures/unique-restoration-31/ordinary-dead-copy.hir"),
        "MODE-2",
    ),
    (
        "same-statement.hir",
        include_str!("../fixtures/unique-restoration-31/same-statement.hir"),
        "MODE-1",
    ),
    (
        "scoped-dead-copy.hir",
        include_str!("../fixtures/unique-restoration-31/scoped-dead-copy.hir"),
        "accept",
    ),
    (
        "scoped-leaf.hir",
        include_str!("../fixtures/unique-restoration-31/scoped-leaf.hir"),
        "accept",
    ),
    (
        "unique-copy-control.hir",
        include_str!("../fixtures/unique-restoration-31/unique-copy-control.hir"),
        "accept",
    ),
];

#[test]
fn unique_restoration_31_matrix() {
    with_compiler_stack(|| {
        for (name, source, expected) in CASES {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add(name, (*source).into()).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            if *expected == "accept" {
                assert!(
                    checked.diagnostics.is_empty(),
                    "{name}: {:?}",
                    checked.diagnostics
                );
                let input = engine.export_native_input().unwrap();
                engine.import_native_input(input).unwrap();
            } else {
                assert!(
                    checked
                        .diagnostics
                        .iter()
                        .any(|d| d.rule.as_str() == *expected),
                    "{name}: {:?}",
                    checked.diagnostics
                );
            }
        }
    });
}

#[test]
fn unique_restoration_31_accepted_four_engines() {
    with_compiler_stack(|| {
        for (name, source, expected) in CASES {
            if *expected != "accept" {
                continue;
            }
            eprintln!("running {name}");
            let program = if *name == "unique-copy-control.hir" {
                format!(
                    "{}\npub fn main() -> Result[(),u64] {{ if value() == 21 {{ Ok(()) }} else {{ Error(1) }} }}",
                    source.replace("fn main()", "fn value()")
                )
            } else if *name == "cleanup-repaired.hir" {
                format!(
                    "{source}\npub fn main() -> Result[(),u64] {{ let mut m: unique Map[u64,List[u64]] = map.empty(); probe(mut m); Ok(()) }}"
                )
            } else {
                format!("{source}\npub fn main() -> Result[(),u64] {{ probe(); Ok(()) }}")
            };
            runtime(&program);
        }
    });
}

#[test]
fn unique_restoration_31_v_requires_actual_release() {
    use hirsel_tir::*;
    with_compiler_stack(|| {
        let mut engine = checked(include_str!(
            "../fixtures/unique-restoration-31/lookup-leaf.hir"
        ));
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let original = input.definitions[&path].as_ref();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let baseline =
            hirsel_validate::validate(original, engine.types(), &signatures, &mut vec![]);
        assert!(baseline.violations.is_empty(), "{:?}", baseline.violations);
        let body = &original.bodies[0];
        let (block, index, place) = body
            .blocks
            .iter()
            .enumerate()
            .find_map(|(b, block)| {
                block
                    .instructions
                    .iter()
                    .enumerate()
                    .find_map(|(i, instruction)| {
                        if let InstructionKind::Drop {
                            place,
                            if_live: false,
                        } = instruction.kind
                        {
                            Some((b, i, place))
                        } else {
                            None
                        }
                    })
            })
            .expect("explicit confined release");
        for mutant in ["missing", "forged", "reordered"] {
            let mut forged = original.clone();
            let body = &mut forged.bodies[0];
            let release = body.blocks[block].instructions[index].clone();
            body.blocks[block].instructions[index].kind = InstructionKind::RunCleanup(CleanupId(0));
            if mutant == "forged" {
                // A storage flag cannot stand in for decrementing the holder.
                body.blocks[block].instructions[index].kind = InstructionKind::StorageDead {
                    place,
                    if_live: false,
                };
            } else if mutant == "reordered" {
                let Terminator::Call {
                    returns: Some(CallReturn { next, .. }),
                    ..
                } = body.blocks[block].terminator
                else {
                    panic!("{:?}", body.blocks[block])
                };
                // Release after the demand rather than before call entry.
                body.blocks[next.0 as usize].instructions.insert(0, release);
            }
            let violations =
                hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![])
                    .violations;
            assert!(!violations.is_empty(), "V accepted {mutant}");
            if mutant != "forged" {
                assert!(
                    violations.iter().any(|v| matches!(
                        v.kind,
                        hirsel_validate::ViolationKind::TypeMismatch {
                            entity: "call argument"
                        }
                    )),
                    "{mutant}: {violations:?}"
                );
            }
        }
    });
}

#[test]
fn unique_restoration_31_v_rejects_share_instead_of_deep_copy() {
    use hirsel_tir::*;
    with_compiler_stack(|| {
        let source = "fn probe() { let mut m: unique Map[u64,List[u64]] = map.empty(); map.insert(mut m,0,[1]); let saved = map.get(m,0); let detached = unique saved; let _ = map.remove(mut m,0); let _ = detached }";
        let mut engine = checked(source);
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let mut forged = (*input.definitions[&path]).clone();
        let mut changed = false;
        for rvalue in &mut forged.bodies[0].rvalues {
            if let Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } = rvalue
            {
                *rvalue = Rvalue::Use(Operand {
                    place: value.place,
                    usage: hirsel_types::UseKind::Copy,
                });
                changed = true;
            }
        }
        assert!(changed);
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let violations =
            hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]).violations;
        assert!(
            violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "value modes"
                }
            )),
            "{violations:?}"
        );
    });
}

#[test]
fn unique_restoration_31_named_producer_unique_consumer_four_engines() {
    with_compiler_stack(|| {
        runtime(
            r#"
fn lookup(m: Map[u64,List[u64]], key: u64) -> Option[List[u64]] in m { map.get(m,key) }
fn consume(take m: unique Map[u64,List[u64]]) -> u64 { if map.get(m,0) == Some([17u64]) { 17 } else { 0 } }
pub fn main() -> Result[(),u64] {
    let mut m: unique Map[u64,List[u64]] = map.empty()
    map.insert(mut m,0,[17u64])
    let saved = lookup(m,0)
    let detached = unique saved
    let n = consume(take m)
    if detached == Some([17u64]) && n == 17 { Ok(()) } else { Error(1) }
}
"#,
        )
    });
}

#[test]
fn unique_restoration_31_excluded_payloads_stay_unproved() {
    with_compiler_stack(|| {
        for source in [
            "fn probe[T: data + copyable](mut m: unique Map[u64,T]) { let saved = map.get(m,0); let _ = unique saved; let _ = map.remove(mut m,0) }",
            "fn probe(mut m: unique Map[u64,List[Shared[List[u64]]]]) { let saved = map.get(m,0); let _ = unique saved; let _ = map.remove(mut m,0) }",
            "fn probe() { let mut m: unique Map[u64,List[u64]] = map.empty(); map.insert(mut m,0,[1]); let alias = copy m; let saved = map.get(m,0); let _ = unique saved; let _ = map.remove(mut m,0); let _ = alias }",
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("excluded.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "MODE-2"),
                "{source}\n{:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn unique_restoration_31_v_retains_independent_holders() {
    use hirsel_tir::*;
    with_compiler_stack(|| {
        for (name, source, _) in CASES.iter().filter(|(name, _, _)| {
            matches!(
                *name,
                "branch-retained.hir"
                    | "loop-carried.hir"
                    | "nested-payload.hir"
                    | "closure-capture.hir"
                    | "escape-take.hir"
                    | "opaque-return.hir"
                    | "failing-cleanup.hir"
            )
        }) {
            let repaired = match *name {
                "loop-carried.hir" => source.replace("kept = map.get", "kept = unique map.get"),
                "opaque-return.hir" => {
                    source.replace("let _ = map.remove", "m = unique m; let _ = map.remove")
                }
                "failing-cleanup.hir" => source.replace("let kept = copy m", "let kept = unique m"),
                _ => source.replace("let saved = map.get", "let saved = unique map.get"),
            };
            let mut engine = checked(&repaired);
            let input = engine.export_native_input().unwrap();
            let mut path = entry_path(&default_context());
            path.0.name = "probe".into();
            let mut forged = (*input.definitions[&path]).clone();
            let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
            signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
            let baseline =
                hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
            assert!(
                baseline.violations.is_empty(),
                "{name}: {:?}",
                baseline.violations
            );
            let mut changes = 0;
            for rvalue in &mut forged.bodies[0].rvalues {
                if let Rvalue::Unary {
                    op: hirsel_syntax::UnaryOp::Unique,
                    value,
                } = rvalue
                {
                    *rvalue = Rvalue::Use(Operand {
                        place: value.place,
                        usage: hirsel_types::UseKind::Copy,
                    });
                    changes += 1;
                }
            }
            assert!(changes > 0, "{name}: no deep copy");
            let violations =
                hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![])
                    .violations;
            assert!(
                violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::TypeMismatch { .. }
                        | hirsel_validate::ViolationKind::LoanConflict { .. }
                )),
                "{name}: {violations:?}"
            );
        }
    });
}

#[test]
fn unique_restoration_31_v_rejects_branch_only_release() {
    use hirsel_tir::*;
    with_compiler_stack(|| {
        let source = "fn probe(b:bool) { let mut m: unique Map[u64,List[u64]] = map.empty(); map.insert(mut m,0,[1]); if b { let saved = map.get(m,0); let _ = unique saved } else {}; let _ = map.remove(mut m,0) }";
        let mut engine = checked(source);
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let mut forged = (*input.definitions[&path]).clone();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let baseline = hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
        assert!(baseline.violations.is_empty(), "{:?}", baseline.violations);
        let body = &mut forged.bodies[0];
        let (cleanup, holder) = body
            .blocks
            .iter()
            .find_map(|block| {
                block.instructions.windows(2).find_map(|pair| {
                    let (InstructionKind::RunCleanup(cleanup), InstructionKind::EndScope(_)) =
                        (&pair[0].kind, &pair[1].kind)
                    else {
                        return None;
                    };
                    body.cleanup_actions(*cleanup).find_map(|action| {
                        let CleanupAction::Drop { place, .. } = action else {
                            return None;
                        };
                        let root = body.places[place.0 as usize].root;
                        (body.locals[root.0 as usize].storage == Storage::Local
                            && matches!(
                                engine.types().row(body.locals[root.0 as usize].ty).kind,
                                hirsel_types::TypeKind::Builtin {
                                    kind: hirsel_types::BuiltinType::Inline,
                                    ..
                                } | hirsel_types::TypeKind::Named { .. }
                                    | hirsel_types::TypeKind::Variant(_)
                            ))
                        .then_some((*cleanup, *place))
                    })
                })
            })
            .expect("branch holder scope cleanup");
        let actions = body
            .cleanup_actions(cleanup)
            .filter(
                |action| !matches!(action, CleanupAction::Drop { place, .. } if *place == holder),
            )
            .cloned()
            .collect();
        body.set_cleanup(cleanup, actions);
        let violations =
            hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]).violations;
        assert!(
            violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "call argument"
                }
            )),
            "{violations:?}"
        );
    });
}

#[test]
fn unique_restoration_31_v_rejects_failure_before_replacement() {
    use hirsel_tir::*;
    with_compiler_stack(|| {
        let mut engine = checked(include_str!(
            "../fixtures/unique-restoration-31/cleanup-repaired.hir"
        ));
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let mut forged = (*input.definitions[&path]).clone();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let baseline = hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
        assert!(baseline.violations.is_empty(), "{:?}", baseline.violations);
        let mut replacements = 0;
        for block in &mut forged.bodies[0].blocks {
            for instruction in &mut block.instructions {
                if matches!(instruction.kind, InstructionKind::Replace { .. }) {
                    // Bomb's potentially failing cleanup now runs before any
                    // fresh graph has repaired the shared mutable parameter.
                    instruction.kind = InstructionKind::RunCleanup(CleanupId(0));
                    replacements += 1;
                }
            }
        }
        assert_eq!(replacements, 1);
        let violations =
            hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]).violations;
        assert!(
            violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "mutable unique hand-back"
                }
            )),
            "{violations:?}"
        );
    });
}

#[test]
fn unique_restoration_31_diagnostics_name_unproved_evidence() {
    with_compiler_stack(|| {
        for (name, expected) in [
            ("dead-copy.hir", "release of its confined holder"),
            ("loop-carried.hir", "previous iteration"),
            ("escape-take.hir", "escapes the confined borrow"),
            (
                "opaque-return.hir",
                "function-value call has an escaped origin",
            ),
            (
                "ordinary-dead-copy.hir",
                "copying `m` shares counted storage",
            ),
        ] {
            let (_, source, _) = CASES.iter().find(|(n, _, _)| *n == name).unwrap();
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add(name, (*source).into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            let diagnostic = output
                .diagnostics
                .iter()
                .find(|d| d.rule.as_str() == "MODE-2")
                .unwrap();
            assert!(
                diagnostic
                    .secondary
                    .iter()
                    .any(|label| label.message.contains(expected)),
                "{name}: {diagnostic:?}"
            );
            assert!(
                diagnostic
                    .notes
                    .iter()
                    .any(|note| note.contains("scope that ends")
                        && note.contains("m = unique m")
                        && note.contains("unrelated result")),
                "{name}: {diagnostic:?}"
            );
        }
    });
}
