//! Related data calls through body-local borrowed views.
use crate::rc_support::*;

const JOIN: &str =
    include_str!("../../../../conformance/MODE/MODE-1-borrow-cursor-compose-join-run.hir");
const LOOP: &str =
    include_str!("../../../../conformance/MODE/MODE-1-borrow-cursor-compose-loop-run.hir");
const OBJECT: &str =
    include_str!("../../../../conformance/MODE/MODE-1-borrow-cursor-compose-object-run.hir");
const IMMUTABLE: &str = r#"
fn view(a: List[u64]) -> List[u64] in a { a }
fn f(a: List[u64]) -> List[u64] in a { let c = a; view(c) }
pub fn main() -> Result[(),u64] {
    if list.len(f([1,2])) == 2 { Ok(()) } else { Error(1) }
}
"#;

#[test]
fn compose_13_immutable_related_result() {
    lower_valid(IMMUTABLE);
}

#[test]
fn compose_13_immutable_expired_source_is_rejected_by_v() {
    use hirsel_tir::*;
    let program = compile(IMMUTABLE);
    let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
    for (path, signature) in &program.signatures {
        signatures.insert(path.clone(), (**signature).clone());
    }
    let mut definition = (**program
        .definitions
        .iter()
        .find(|(p, _)| p.0.name == "f")
        .unwrap()
        .1)
        .clone();
    let body = &mut definition.bodies[0];
    let (loan, span, cleanup) = body
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| {
            if let InstructionKind::BeginLoan { id, .. } = i.kind {
                Some((id, i.span, i.on_failure))
            } else {
                None
            }
        })
        .unwrap();
    let next = body
        .blocks
        .iter()
        .find_map(|b| {
            if let Terminator::Call {
                returns: Some(ret), ..
            } = &b.terminator
            {
                Some(ret.next)
            } else {
                None
            }
        })
        .unwrap();
    body.blocks[next.0 as usize].instructions.insert(
        0,
        Instruction {
            kind: InstructionKind::EndLoan {
                loan,
                if_active: false,
            },
            span,
            on_failure: cleanup,
        },
    );
    let verdict = hirsel_validate::validate(&definition, &program.types, &signatures, &mut vec![]);
    assert!(
        verdict.violations.iter().any(|v| matches!(v.kind,
        hirsel_validate::ViolationKind::ExpiredLoan { loan: ended, .. } if ended == loan)),
        "{verdict:?}"
    );
}

#[test]
#[ignore = "native: immutable related-result interpreter, heap, LLVM and Cranelift parity"]
fn compose_13_immutable_native_parity() {
    native_parity(&[("immutable", IMMUTABLE)]);
}

fn native_parity(cases: &[(&str, &str)]) {
    use hirsel_driver::native::*;
    use hirsel_driver::*;
    use hirsel_engine::*;
    let options = Options::new(target());
    let matrix = vec![
        Configuration {
            options: options.clone(),
            engine: Engine::Heap { campaign: false },
        },
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
    let directory =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("borrow-cursor-compose-13");
    std::fs::create_dir_all(&directory).unwrap();
    let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
    for &(name, source) in cases {
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("compose.hir", source.into()).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&context);
            let reference = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            eprintln!("{name}: interpreter exit0");
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            let directory = directory.join(name);
            std::fs::create_dir_all(&directory).unwrap();
            let mut executions = 0;
            for (configuration, outcome) in differential(
                &validated, &entry, &reference, &matrix, &toolchain, &directory,
            ) {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}"
                );
                eprintln!("{name}: {configuration}: agrees");
                executions += 1;
            }
            assert_eq!(executions, 3);
        });
    }
}

#[test]
fn compose_13_join_related_result() {
    lower_valid(JOIN);
}

#[test]
fn compose_13_loop_snapshot_and_object_results() {
    lower_valid(LOOP);
    lower_valid(OBJECT);
    lower_valid(&JOIN.replace(
        "let mut c: List[u64] in {a, b} = a",
        "let mut c = if flag { b } else { a }",
    ));
}

#[test]
fn compose_13_owned_and_foreign_controls() {
    for source in [
        JOIN.replace("-> List[u64] in {a, b}", "-> List[u64] in a"),
        IMMUTABLE.replace("let c = a", "let c = copy a"),
        IMMUTABLE.replace("let c = a", "let owned = copy a; let c = owned"),
        include_str!("../../../../conformance/MODE/MODE-1-borrow-cursor-owned-getter-bad.hir")
            .into(),
    ] {
        let error = try_compile(&source).expect_err("owned or unrelated storage escaped");
        assert!(error.contains("MODE-1"), "{source}\n{error}");
    }
}

#[test]
fn compose_13_rebind_has_no_rc_and_call_result_is_owned() {
    use hirsel_lir::ir::{Holding, Inst, Terminator};
    let (_, lowered) = lower_valid(JOIN);
    let f = unit_named(&lowered, "f");
    assert!(
        !f.instructions.iter().any(|i| matches!(
            i,
            Inst::Dup { .. } | Inst::Release { .. } | Inst::Copy { .. } | Inst::CopyValue { .. }
        )),
        "{}",
        dump(f)
    );
    let returned = f
        .terminators
        .iter()
        .filter_map(|b| match *b {
            Terminator::Return(Some(value)) => Some(value),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!returned.is_empty());
    for value in returned {
        assert_eq!(f.values[value.index()].holding, Holding::Owned);
    }
    // The result's counted share belongs to view(), independently of cursor transport.
    let view = unit_named(&lowered, "view");
    assert!(
        view.instructions.iter().any(|i| matches!(
            i,
            Inst::Copy { .. } | Inst::CopyValue { .. } | Inst::Dup { .. }
        )),
        "{}",
        dump(view)
    );
}

#[test]
#[ignore = "native: joined, inferred, snapshot and array related-result parity"]
fn compose_13_join_loop_native_parity() {
    let inferred = JOIN.replace(
        "let mut c: List[u64] in {a, b} = a",
        "let mut c = if flag { b } else { a }",
    );
    native_parity(&[
        ("join", JOIN),
        ("inferred", &inferred),
        ("loop-snapshot", LOOP),
    ]);
}

#[test]
fn compose_13_join_independent_v_adversarial_controls() {
    use hirsel_tir::*;
    use hirsel_validate::ViolationKind;
    // Keep an unrelated borrowed parameter live at call entry. It is outside
    // the result's {a,b} relation and outside the cursor's fixed cover.
    let source = JOIN
        .replace("b: List[u64], flag", "b: List[u64], z: List[u64], flag")
        .replace("f([1], [2, 3],", "f([1], [2, 3], [9],");
    let program = compile(&source);
    let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
    for (path, signature) in &program.signatures {
        signatures.insert(path.clone(), (**signature).clone());
    }
    let definition = program
        .definitions
        .iter()
        .find(|(p, _)| p.0.name == "f")
        .unwrap()
        .1;
    let validate =
        |d: &Definition| hirsel_validate::validate(d, &program.types, &signatures, &mut vec![]);
    let mut detached = (**definition).clone();
    detached.bodies[0].borrow_cursors.clear();
    assert!(
        validate(&detached).violations.is_empty(),
        "{:?}",
        validate(&detached)
    );
    for mutation in 0..4 {
        let mut d = (**definition).clone();
        let body = &mut d.bodies[0];
        let call = body
            .blocks
            .iter()
            .position(|b| matches!(b.terminator, Terminator::Call { .. }))
            .unwrap();
        let next = match &body.blocks[call].terminator {
            Terminator::Call {
                returns: Some(ret), ..
            } => ret.next,
            _ => unreachable!(),
        };
        let exemplar = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .next()
            .unwrap()
            .clone();
        let first_loan = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|i| match i.kind {
                InstructionKind::BeginLoan { id, .. } => Some(id),
                _ => None,
            })
            .unwrap();
        match mutation {
            0 => {
                // End a genuine incoming loan before consuming the result union.
                body.blocks[next.0 as usize].instructions.insert(
                    0,
                    Instruction {
                        kind: InstructionKind::EndLoan {
                            loan: first_loan,
                            if_active: true,
                        },
                        ..exemplar
                    },
                );
            }
            1 => {
                // Substitute a different, live, independently loaned argument.
                let root = body.parameters[2];
                let place = PlaceId(
                    body.places
                        .iter()
                        .position(|p| p.root == root && p.projection.is_empty() && p.loan.is_none())
                        .unwrap() as u32,
                );
                let loan = LoanId(
                    body.blocks
                        .iter()
                        .flat_map(|b| &b.instructions)
                        .filter_map(|i| match i.kind {
                            InstructionKind::BeginLoan { id, .. } => Some(id.0),
                            _ => None,
                        })
                        .max()
                        .unwrap()
                        + 1,
                );
                body.blocks[call].instructions.push(Instruction {
                    kind: InstructionKind::BeginLoan {
                        id: loan,
                        place,
                        mutable: false,
                        if_inactive: false,
                    },
                    ..exemplar
                });
                let mut access = body.places[place.0 as usize].clone();
                access.loan = Some(loan);
                let access_id = PlaceId(body.places.len() as u32);
                body.places.push(access);
                if let Terminator::Call { arguments, .. } = &mut body.blocks[call].terminator {
                    arguments[0].value.place = access_id;
                }
            }
            2 => {
                // A claimed current origin cannot rename the union to one root.
                let carrier = body.borrow_cursors[0].carrier;
                let forged = body
                    .places
                    .iter()
                    .find(|p| p.root == body.parameters[0] && p.binding.is_some())
                    .unwrap()
                    .binding
                    .clone();
                for p in &mut body.places {
                    if p.root == carrier && p.binding.is_some() {
                        p.binding = forged.clone();
                    }
                }
            }
            _ => {
                body.blocks[call].instructions.push(Instruction {
                    kind: InstructionKind::EndScope(ScopeId(0)),
                    ..exemplar
                });
            }
        }
        let verdict = validate(&d);
        assert!(
            verdict.violations.iter().any(|v| match mutation {
                0 => matches!(v.kind, ViolationKind::ExpiredLoan { .. }),
                1 => matches!(v.kind, ViolationKind::InvalidResultSource(_)),
                2 => matches!(
                    v.kind,
                    ViolationKind::InvalidOrigin(_) | ViolationKind::InvalidResultSource(_)
                ),
                _ => matches!(v.kind, ViolationKind::LoanOutlivesSource { .. }),
            }),
            "mutation {mutation}: {verdict:?}"
        );
    }
}

#[test]
#[ignore = "native: Object hash lookup and related recursive result parity"]
fn compose_13_object_native_parity() {
    native_parity(&[("object", OBJECT)]);
}

#[test]
fn compose_13_rebind_keeps_only_the_result_count() {
    use hirsel_driver::native::*;
    use hirsel_engine::*;
    with_compiler_stack(|| {
        let mut counts = vec![];
        for rounds in [0, 64] {
            let source = format!(
                r#"
fn view(a: List[u64]) -> List[u64] in a {{ a }}
fn f(a: List[u64]) -> List[u64] in a {{
    let mut c = a
    for i in 0u64..{rounds}u64 {{ c = a }}
    view(c)
}}
pub fn main() -> Result[(), u64] {{
    if list.len(f([1,2])) == 2 {{ Ok(()) }} else {{ Error(1) }}
}}
"#
            );
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("result-count.hir", source).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            let mut options = Options::new(target());
            options.elide = false;
            options.reuse = false;
            options.regions = false;
            let ready = build(&validated, &options).unwrap();
            let heap = run_heap(
                &ready,
                &entry_path(&context),
                &Sites::new(validated.snapshot()),
            )
            .unwrap();
            assert_eq!(heap.observation.exit_code, Some(0));
            eprintln!("{rounds} rebinds: {:?}", heap.counters);
            counts.push((
                heap.counters.dups,
                heap.counters.releases,
                heap.counters.allocations,
                heap.counters.frees,
            ));
        }
        assert_eq!(counts[0], counts[1], "rebinding changed owning counts");
        assert_eq!(counts[0].0, 1, "the returned share must acquire one count");
        assert_eq!(
            counts[0].1, 2,
            "the input and returned share must both release"
        );
        assert_eq!(counts[0].2, counts[0].3, "all allocations must be freed");
    });
}

#[test]
fn compose_13_checked_interpreter_retains_underlying_bounds() {
    use hirsel_engine::*;
    for (name, source) in [
        ("immutable", IMMUTABLE),
        ("join", JOIN),
        ("loop", LOOP),
        ("object", OBJECT),
    ] {
        with_compiler_stack(|| {
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine
                .sources
                .add("checked-compose.hir", source.into())
                .unwrap();
            let context = default_context();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let execution = run_observed_checked(
                &mut engine,
                &RunConfiguration {
                    entry: entry_path(&context),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let observation = execution
                .observation
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: {execution:?}"));
            assert_eq!(
                hirsel_driver::native::Observation::of_reference(observation).exit_code,
                Some(0),
                "{name}: {execution:?}"
            );
        });
    }
}

const MUT_ALIAS: &str = r#"
type Value = Leaf() | Object(Map[String,Value])
fn descend(value: Value) -> Option[Value] in value uses {} {
  match value {
    Object(members) => {
      let Some(child) = core@1/map.get(members, "key") else {
        return None()
      }
      descend(child)
    },
    _ => {
      let mut target = value
      Some(copy target)
    },
  }
}

pub fn main()->Result[(),u64] {let v:Value=Leaf(); let _=descend(v); Ok(())}
"#;
const MUT_ALIAS_BRANCHES: &str =
    include_str!("../../../../conformance/MODE/MODE-1-borrow-cursor-compose-mut-alias-run.hir");
const MUT_ALIAS_SOURCE_BAD: &str = include_str!(
    "../../../../conformance/MODE/MODE-1-borrow-cursor-compose-mut-alias-source-bad.hir"
);

#[test]
fn compose_14_mutable_alias_related_result() {
    for source in [
        MUT_ALIAS.to_string(),
        MUT_ALIAS.replace("let mut target", "let target"),
        MUT_ALIAS.replace(
            "let mut target = value",
            "let mut target = value; target = value",
        ),
        MUT_ALIAS_BRANCHES.into(),
        MUT_ALIAS.replace(
            "let Some(child) = core@1/map.get(members, \"key\") else {\n        return None()\n      }\n      descend(child)",
            "descend(value)",
        ),
    ] {
        lower_valid(&source);
    }
}

#[test]
fn compose_14_mutable_alias_source_and_write_controls() {
    let error = try_compile(MUT_ALIAS_SOURCE_BAD)
        .err()
        .expect("source mutated while alias is live");
    assert!(error.contains("MODE-1"), "{error}");
    let error = try_compile(
        r#"
fn f(a: List[u64]) -> u64 {
    let mut target = a
    list.push(mut target, 1)
    list.len(target)
}
pub fn main() -> () { let _ = f([0]); }
"#,
    )
    .expect_err("mutable view binding granted referent write permission");
    assert!(error.contains("LANG-15"), "{error}");
}

#[test]
fn compose_14_mutable_alias_independent_v_controls() {
    use hirsel_tir::*;
    use hirsel_validate::ViolationKind;
    let program = compile(MUT_ALIAS);
    let mut signatures = hirsel_validate::SignatureMap::new(program.types.clone());
    for (path, signature) in &program.signatures {
        signatures.insert(path.clone(), (**signature).clone());
    }
    let original = program
        .definitions
        .iter()
        .find(|(p, _)| p.0.name == "descend")
        .unwrap()
        .1;
    let validate =
        |d: &Definition| hirsel_validate::validate(d, &program.types, &signatures, &mut vec![]);
    let mut detached = (**original).clone();
    detached.bodies[0].borrow_cursors.clear();
    assert!(validate(&detached).violations.is_empty());
    for expire_storage in [false, true] {
        let mut definition = (**original).clone();
        let body = &mut definition.bodies[0];
        let initializer = body.borrow_cursors[0].initializer;
        let Rvalue::Use(source) = &body.rvalues[initializer.0 as usize] else {
            panic!("expected borrowed cursor initializer")
        };
        let source = &body.places[source.place.0 as usize];
        let loan = source.loan.unwrap();
        let source_scope = body.locals[source.root.0 as usize].scope;
        let (block, index) = body.blocks.iter().enumerate().find_map(|(b, block)| {
            block.instructions.iter().enumerate().find_map(|(i, instruction)| {
                matches!(instruction.kind, InstructionKind::Assign { value, .. } if value == initializer)
                    .then_some((b, i))
            })
        }).unwrap();
        let exemplar = body.blocks[block].instructions[index].clone();
        body.blocks[block].instructions.insert(
            index + 1,
            Instruction {
                kind: if expire_storage {
                    InstructionKind::EndScope(source_scope)
                } else {
                    InstructionKind::EndLoan {
                        loan,
                        if_active: false,
                    }
                },
                ..exemplar
            },
        );
        let verdict = validate(&definition);
        assert!(
            verdict.violations.iter().any(|v| if expire_storage {
                matches!(v.kind, ViolationKind::LoanOutlivesSource { loan: id, .. } if id == loan)
            } else {
                matches!(v.kind, ViolationKind::ExpiredLoan { loan: id, .. } if id == loan)
            }),
            "expire_storage={expire_storage}: {verdict:?}"
        );
    }
}

#[test]
#[ignore = "native: mutable alias interpreter, heap, LLVM and Cranelift parity"]
fn compose_14_mutable_alias_native_parity() {
    native_parity(&[
        ("mut-alias-minimal", MUT_ALIAS),
        ("mut-alias-branches", MUT_ALIAS_BRANCHES),
    ]);
}

#[test]
fn compose_14_mutable_alias_checked_interpreter() {
    use hirsel_engine::*;
    for source in [MUT_ALIAS, MUT_ALIAS_BRANCHES] {
        with_compiler_stack(|| {
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine
                .sources
                .add("mutable-alias.hir", source.into())
                .unwrap();
            let context = default_context();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let execution = run_observed_checked(
                &mut engine,
                &RunConfiguration {
                    entry: entry_path(&context),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let observation = execution
                .observation
                .as_ref()
                .unwrap_or_else(|| panic!("{execution:?}"));
            assert_eq!(
                hirsel_driver::native::Observation::of_reference(observation).exit_code,
                Some(0),
                "{execution:?}"
            );
        });
    }
}
