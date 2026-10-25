//! MODE-2 contracts use current value evidence at aggregate and mutable call boundaries.
use hirsel_engine::{QueryEngine, check_snapshot, default_context, default_environment};

fn check(source: &str, rejected: Option<u32>) {
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("mode-obligations.hir", source.into())
        .unwrap();
    let diagnostics = match check_snapshot(&mut engine, file, &default_context()) {
        Ok(diagnostics) | Err(hirsel_types::QueryError::Diagnostics(diagnostics)) => diagnostics,
        Err(error) => panic!("{source}\n{error:?}"),
    };
    if let Some(line) = rejected {
        let first = diagnostics
            .iter()
            .find(|d| d.severity == hirsel_diag::Severity::Error)
            .unwrap_or_else(|| panic!("missing MODE-2: {source}"));
        assert_eq!(first.rule.as_str(), "MODE-2", "{source}\n{diagnostics:?}");
        assert_eq!(
            engine.sources.location(first.primary).unwrap().line,
            line,
            "{source}\n{diagnostics:?}"
        );
    } else {
        assert!(diagnostics.is_empty(), "{source}\n{diagnostics:?}");
    }
}

#[test]
fn mode_obligations_record_branch_twins() {
    for share in [false, true] {
        let source = format!(
            "type Holder {{ items: unique Bytes }}\nfn build(take x:unique Bytes, choose:bool)->Holder {{\n if choose {{ {} }}\n Holder(items:take x)\n}}",
            if share {
                "let kept=copy x"
            } else {
                "let n=bytes.len(x)"
            }
        );
        check(&source, share.then_some(4));
    }
}

#[test]
fn mode_obligations_loop_zero_one_many_twins() {
    for count in [0, 1, 3] {
        for share in [false, true] {
            let source = format!(
                "fn looped()->Result[unique List[u64],()] {{\n let mut xs:List[u64]=[]\n {}\n for i in 0u64..{count}u64 {{ list.push(mut xs,i) }}\n Ok(take xs)\n}}",
                if share {
                    "let kept=copy xs"
                } else {
                    "let n=list.len(xs)"
                }
            );
            check(&source, share.then_some(5));
        }
    }
}

#[test]
fn mode_obligations_nested_container_twins() {
    for (ty, literal) in [
        ("unique box unique List[Bytes]", "box unique [b\"x\"]"),
        ("unique (unique List[Bytes],u8)", "(unique [b\"x\"],1u8)"),
    ] {
        for share in [false, true] {
            let source = format!(
                "fn replace(mut x:{ty}) {{}}\nfn probe() {{\n let mut x:{ty}={literal}\n {}\n replace(mut x)\n}}",
                if share {
                    "let kept=copy x"
                } else {
                    "let n=0u8"
                }
            );
            check(&source, share.then_some(5));
        }
    }
}

#[test]
fn mode_obligations_nested_matches_h_heap_llvm_cranelift() {
    use hirsel_driver::native::*;
    use hirsel_engine::{
        RunConfiguration, check_progress, entry_path, run_observed_snapshot, with_compiler_stack,
    };
    with_compiler_stack(|| {
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: true },
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
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mode-obligations");
        let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
        for (name, source) in [
            (
                "box",
                format!(
                    "{}\npub fn main() -> Result[(),()] {{ probe(); Ok(()) }}",
                    include_str!("../../../../conformance/MODE/MODE-2-nested-view-mut-ok.hir")
                ),
            ),
            (
                "tuple",
                r#"fn replace(mut x:(unique List[Bytes],u8)) {}
                   pub fn main() -> Result[(),()] {
                       let mut x:(unique List[Bytes],u8)=(unique [b"x"],1u8)
                       replace(mut x)
                       Ok(())
                   }"#
                .into(),
            ),
            (
                "unique-box",
                r#"fn replace(mut x:unique box unique List[Bytes]) {}
                   pub fn main() -> Result[(),()] {
                       let mut x:unique box unique List[Bytes]=box unique [b"x"]
                       replace(mut x)
                       Ok(())
                   }"#
                .into(),
            ),
            (
                "unique-tuple",
                r#"fn replace(mut x:unique (unique List[Bytes],u8)) {}
                   pub fn main() -> Result[(),()] {
                       let mut x:unique (unique List[Bytes],u8)=(unique [b"x"],1u8)
                       replace(mut x)
                       Ok(())
                   }"#
                .into(),
            ),
            (
                "record",
                r#"type Holder { items:unique Bytes }
                   fn build(take x:unique Bytes,choose:bool)->Holder {
                       if choose { let n=bytes.len(x) }
                       Holder(items:take x)
                   }
                   pub fn main() -> Result[(),()] {
                       let yes=build(b"x",true)
                       let no=build(b"y",false)
                       Ok(())
                   }"#
                .into(),
            ),
            (
                "loop",
                r#"fn looped(n:u64)->Result[unique List[u64],()] {
                       let mut xs:List[u64]=[]
                       for i in 0u64..n { list.push(mut xs,i) }
                       Ok(take xs)
                   }
                   pub fn main() -> Result[(),()] {
                       let zero=looped(0u64)
                       let one=looped(1u64)
                       let many=looped(3u64)
                       Ok(())
                   }"#
                .into(),
            ),
        ] {
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let file = engine
                .sources
                .add(format!("mode-{name}.hir"), source.into())
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&default_context());
            let run = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                Default::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(run.observation.as_ref().unwrap());
            assert!(reference.output.is_empty(), "{name}");
            let input = engine.export_native_input().unwrap();
            let input = engine.import_native_input(input).unwrap();
            std::fs::create_dir_all(directory.join(name)).unwrap();
            let outcomes = differential(
                &input,
                &entry,
                &reference,
                &matrix,
                &toolchain,
                &directory.join(name),
            );
            assert_eq!(outcomes.len(), 3);
            for (configuration, outcome) in outcomes {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name} {configuration}: {outcome:?}"
                );
            }
        }
    });
}

/// Reuse real lowering only to obtain a well-formed graph, then hand V a forged
/// graph/signature directly. No source-checker outcome can bless these changes.
#[test]
fn mode_obligations_forged_nested_contracts_rejected_by_v() {
    use hirsel_tir::Terminator;
    use hirsel_types::{
        Brand, Origin, OriginRoot, ParamId, ParameterRef, SignatureKind, TypeKind, Uniqueness,
    };
    let source = include_str!("../../../../conformance/MODE/MODE-2-nested-view-mut-ok.hir");
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine
        .sources
        .add("forged-mode.hir", source.into())
        .unwrap();
    assert!(
        check_snapshot(&mut engine, file, &default_context())
            .unwrap()
            .is_empty()
    );
    let input = engine.export_native_input().unwrap();
    let mut probe = hirsel_engine::entry_path(&default_context());
    probe.0.name = "probe".into();
    let original = (**input.definitions.get(&probe).unwrap()).clone();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in input.signatures.values() {
        signatures.insert(signature.definition, (**signature).clone());
    }
    hirsel_validate::assert_valid(&original, engine.types(), &signatures);
    let mut replace = probe.clone();
    replace.0.name = "replace".into();
    let callee = input.signatures.get(&replace).unwrap();
    let types = engine.types();
    for forge in [
        "weakened hand-back",
        "missing nested brand",
        "stale argument row",
    ] {
        let mut definition = original.clone();
        let mut forged_signatures = signatures.clone();
        if forge == "stale argument row" {
            let body = &mut definition.bodies[definition.entry.0 as usize];
            let place = body
                .blocks
                .iter()
                .find_map(|b| match &b.terminator {
                    Terminator::Call { arguments, .. } => Some(arguments[0].value.place),
                    _ => None,
                })
                .unwrap();
            let root = body.places[place.0 as usize].root;
            let mut row = types.get(body.places[place.0 as usize].ty).unwrap().clone();
            let TypeKind::Box(part) = row.kind else {
                panic!()
            };
            let mut nested = types.get(part).unwrap().clone();
            nested.modes.uniqueness = Uniqueness::Aliased;
            nested.modes.sendability = hirsel_types::Sendability::NotSendable;
            row.kind = TypeKind::Box(types.intern(nested));
            let ty = types.intern(row);
            body.locals[root.0 as usize].ty = ty;
            for access in &mut body.places {
                if access.root == root && access.projection.is_empty() {
                    access.ty = ty;
                }
            }
            definition.types = hirsel_tir::definition_types(types, &definition);
        } else {
            let mut signature = (**callee).clone();
            let SignatureKind::Function(function) = &mut signature.kind else {
                panic!()
            };
            let mut row = types.get(function.parameters[0].ty).unwrap().clone();
            let TypeKind::Box(part) = row.kind else {
                panic!()
            };
            let mut nested = types.get(part).unwrap().clone();
            if forge == "weakened hand-back" {
                nested.modes.uniqueness = Uniqueness::Aliased;
                nested.modes.sendability = hirsel_types::Sendability::NotSendable;
            } else {
                nested.brand = Brand {
                    alternatives: vec![Origin {
                        root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
                        fields: vec![].into(),
                    }]
                    .into(),
                };
            }
            row.kind = TypeKind::Box(types.intern(nested));
            function.parameters[0].ty = types.intern(row);
            forged_signatures.insert(signature.definition, signature);
        }
        let report = hirsel_validate::validate(&definition, types, &forged_signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "call argument"
                }
            )),
            "{forge}: {:?}",
            report.violations
        );
    }
}
