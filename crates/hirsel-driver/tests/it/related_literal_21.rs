//! Written literal contracts must survive checking, lowering and independent V.
use hirsel_engine::*;

const RELATED: &str = "fn use_read(read:fn(value:List[String])->List[String] in value uses {}, xs:List[String])->List[String] in xs uses {} {read(xs)}\nfn f(xs:List[String])->List[String] in xs {use_read(fn(value:List[String])->List[String] in value uses {} {copy value},xs)}\npub fn main()->Result[(),u64] {Ok(())}";
const ONCE: &str = "fn relay[T](take x:T)->T uses{} {take x}\nfn need(take f:once fn()->u64 uses{}){}\npub fn main(){need(relay(fn()->u64{7u64}))}";

fn accepted(source: &str) {
    let mut engine = crate::source_std::engine();
    let file = engine
        .sources
        .add("related-literal.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    assert!(
        output.diagnostics.is_empty(),
        "{source}\n{:?}",
        output.diagnostics
    );
    let input = engine.export_native_input().unwrap();
    engine.import_native_input(input).unwrap();
}

#[test]
fn related_literal_21_named_renamed_and_two_parameter_controls() {
    with_compiler_stack(|| {
        accepted(&RELATED.replace("fn(value:List[String])->List[String] in value uses {} {copy value}", "read_list").replace("fn f(", "fn read_list(value:List[String])->List[String] in value uses {} {copy value}\nfn f("));
        accepted(&RELATED.replace(
            "fn(value:List[String])->List[String] in value uses {} {copy value}",
            "fn(renamed:List[String])->List[String] in renamed uses {} {copy renamed}",
        ));
        accepted(
            "fn use_read(read:fn(a:List[String],b:List[String])->List[String] in b uses {}, xs:List[String], ys:List[String])->List[String] in ys uses {} {read(xs,ys)}\nfn f(xs:List[String],ys:List[String])->List[String] in ys {use_read(fn(first:List[String],second:List[String])->List[String] in second uses {} {copy second},xs,ys)}\npub fn main(){}",
        );
        accepted(
            "fn need(read:fn(a:List[String],b:List[String])->List[String] in {a,b} uses {}){}\npub fn main(){need(fn(first:List[String],second:List[String])->List[String] in {first,second} uses {} {copy second})}",
        );
    });
}

#[test]
fn related_literal_21_true_relation_mismatches_have_human_and_json_diagnostics() {
    with_compiler_stack(|| {
        let sources = [
            (RELATED.replace("in value uses {} {copy value}", "in env uses {} {copy value}"), "MODE-1"),
            ("fn use_read(read:fn(a:List[String],b:List[String])->List[String] in a uses {}){}\nfn f(){use_read(fn(x:List[String],y:List[String])->List[String] in y uses {} {copy y})}".into(), "MODE-1"),
            ("fn use_read(read:fn(value:List[String])->List[String] in value uses {}){}\nfn f(captured:List[String]){use_read(fn(value:List[String])->List[String] in {env,value} uses {} {copy captured})}".into(), "MODE-1"),
            ("fn use_read(read:fn(value:List[String])->List[String] in value uses {}){}\nfn f(captured:List[String]){use_read(fn(value:List[String])->List[String] in value uses {} {copy captured})}".into(), "MODE-1"),
            ("fn use_read(read:fn(a:List[String],b:List[String])->List[String] in a uses {}){}\nfn f(){use_read(fn(x:List[String],y:List[String])->List[String] in x uses {} {copy y})}".into(), "MODE-1"),
            ("fn use_read(read:@noalloc fn(value:List[String])->List[String] in value uses {}){}\nfn f(){use_read(fn(value:List[String])->List[String] in value uses {} {[\"allocated\"]})}".into(), "MEM-5"),
        ];
        for (source, rule) in sources {
            let mut engine = crate::source_std::engine();
            let file = engine.sources.add("negative.hir", source.clone()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(!output.diagnostics.is_empty(), "accepted {source}");
            assert!(
                output.diagnostics.iter().any(|d| d.rule.as_str() == rule),
                "{source}\n{:?}",
                output.diagnostics
            );
            for d in &output.diagnostics {
                let human = d.render_human(&output.sources);
                let json = d.to_json(&output.sources);
                assert!(human.contains(&d.message));
                assert_eq!(json["message"], d.message);
                assert_eq!(json["rule"], d.rule.as_str());
                if source.contains("in y uses {} {copy y}") {
                    assert!(d.message.contains("in y`"), "{human}");
                    assert!(d.message.contains("in a`"), "{human}");
                }
                if source.contains("in value uses {} {copy captured}") {
                    assert_eq!(output.sources.slice(d.primary), Some("copy captured"));
                    assert!(
                        d.message.contains("source its relation does not name"),
                        "{human}"
                    );
                }
                assert!(output.sources.slice(d.primary).is_some());
                eprintln!("{source}\n{human}\n{json}");
            }
        }
    });
}

#[test]
fn related_literal_21_environment_and_once_controls() {
    with_compiler_stack(|| {
        accepted(
            "fn use_read(read:fn()->List[String] in env uses {})->u64 uses {} {list.len(read())}\nfn f(captured:List[String])->u64 {use_read(fn()->List[String] in env uses {} {copy captured})}\npub fn main(){}",
        );
        accepted(&ONCE.replace("need(relay(fn()->u64{7u64}))", "need(fn()->u64{7u64})"));
        accepted(&ONCE.replace(
            "need(relay(fn()->u64{7u64}))",
            "let f:once fn()->u64 uses{}=fn()->u64{7u64};need(relay(take f))",
        ));
        accepted(&ONCE.replace("once fn()", "fn()"));
        accepted(&ONCE.replace(
            "need(relay(fn()->u64{7u64}))",
            "let n=7u64;need(relay(fn()->u64{take n}))",
        ));
    });
}

// Reuse the existing Rust Option ports (generic_result_v_21 and the std
// CLO-2 option corpus); these concrete vectors add related literal operands.
const RUNTIME: &str = r#"
fn relay[T](take x:T)->T uses{} {take x}
fn need(take f:once fn()->u64 uses{})->u64 uses{} {f()}
fn use_read(read:fn(value:List[String])->List[String] in value uses {}, xs:List[String])->List[String] in xs uses {} {read(xs)}
fn read_named(value:List[String])->List[String] in value uses {} {copy value}
fn mapped(read:fn(value:List[String])->List[String] in value uses {}, xs:List[String], present:bool)->Option[List[String]] in xs uses {} {
 if present {Some(read(xs))} else {None}
}
pub fn main()->Result[(),u64] {
 let xs=["hello","world"]
 if use_read(fn(value:List[String])->List[String] in value uses {} {copy value},xs)!=xs {return Error(1)}
 if use_read(fn(renamed:List[String])->List[String] in renamed uses {} {copy renamed},xs)!=use_read(read_named,xs) {return Error(2)}
 if mapped(fn(value:List[String])->List[String] in value uses {} {copy value},xs,true)!=Some(copy xs) {return Error(3)}
 if mapped(fn(value:List[String])->List[String] in value uses {} {copy value},xs,false)!=None {return Error(4)}
 if need(relay(fn()->u64{7u64}))!=7u64 {return Error(5)}
 Ok(())
}
"#;

#[test]
fn related_literal_21_interpreter_h_llvm_cranelift() {
    with_compiler_stack(|| {
        use hirsel_driver::native::*;
        let mut engine = crate::source_std::engine();
        let file = engine.sources.add("runtime.hir", RUNTIME.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
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
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty(), "{reference:?}");
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
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
        let scratch = tempfile::Builder::new()
            .prefix("related-literal-21-")
            .tempdir()
            .unwrap();
        let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
        let outcomes = differential(
            &validated,
            &entry,
            &reference,
            &matrix,
            &toolchain,
            scratch.path(),
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            assert!(
                matches!(outcome, Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

#[test]
fn related_literal_21_v_rejects_forged_multiplicity() {
    with_compiler_stack(|| {
        use hirsel_tir::*;
        use hirsel_types::*;
        let source = "fn need(take f:once fn()->u64 uses{}){}\npub fn main(){let n=7u64;need(fn()->u64{take n})}";
        let mut engine = crate::source_std::engine();
        let file = engine.sources.add("forgery.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let mut forged = (*input.definitions[&entry_path(&default_context())]).clone();
        hirsel_validate::assert_valid(&forged, engine.types(), &signatures);
        let body = &mut forged.bodies[0];
        let destination = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|i| {
                let InstructionKind::Assign { destination, value } = i.kind else {
                    return None;
                };
                matches!(body.rvalues[value.0 as usize], Rvalue::Closure { .. })
                    .then_some(destination)
            })
            .unwrap();
        let root = body.places[destination.0 as usize].root;
        let mut row = engine
            .types()
            .get(body.locals[root.0 as usize].ty)
            .unwrap()
            .clone();
        assert_eq!(row.modes.multiplicity, Multiplicity::Once);
        row.modes.multiplicity = Multiplicity::Many;
        let bogus = engine.types().intern(row);
        body.locals[root.0 as usize].ty = bogus;
        for place in &mut body.places {
            if place.root == root && place.projection.is_empty() {
                place.ty = bogus;
            }
        }
        forged.types = definition_types(engine.types(), &forged);
        let violations =
            hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]).violations;
        assert!(
            violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "closure multiplicity"
                }
            )),
            "{violations:?}"
        );
    });
}

#[test]
fn related_literal_21_v_rejects_relabelled_result_source() {
    with_compiler_stack(|| {
        use hirsel_tir::*;
        use hirsel_types::*;
        let source = "pub fn main(){let _=fn(first:List[String],second:List[String])->List[String] in second uses {} {copy second}}";
        let mut engine = crate::source_std::engine();
        let file = engine
            .sources
            .add("source-forgery.hir", source.into())
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let mut forged = (*input.definitions[&entry_path(&default_context())]).clone();
        hirsel_validate::assert_valid(&forged, engine.types(), &signatures);
        let body = &mut forged.bodies[0];
        let (destination, child) = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|i| {
                let InstructionKind::Assign { destination, value } = i.kind else {
                    return None;
                };
                let Rvalue::Closure { body: child, .. } = body.rvalues[value.0 as usize] else {
                    return None;
                };
                Some((destination, child))
            })
            .unwrap();
        assert!(matches!(
            engine
                .types()
                .get(body.places[destination.0 as usize].ty)
                .unwrap()
                .kind,
            TypeKind::Function(_)
        ));
        // Keep every parent receiving row coherent with the forged header.
        // Only the child's executable return still selects parameter 1.
        let relabel = |ty| {
            let mut row = engine.types().get(ty).unwrap().clone();
            if let TypeKind::Function(header) = &mut row.kind {
                assert_eq!(header.parameters.len(), 2);
                let mut result = engine.types().get(header.result).unwrap().clone();
                result.modes.relation = Relation::place(Origin {
                    root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
                    fields: vec![].into(),
                });
                header.result = engine.types().intern(result);
            }
            engine.types().intern(row)
        };
        for local in &mut body.locals {
            local.ty = relabel(local.ty);
        }
        for place in &mut body.places {
            place.ty = relabel(place.ty);
        }
        let child = &mut forged.bodies[child.0 as usize];
        let mut result = engine.types().get(child.result).unwrap().clone();
        result.modes.relation = Relation::place(Origin {
            root: OriginRoot::Parameter(ParameterRef::body(child.parameter_scope, ParamId(0))),
            fields: vec![].into(),
        });
        child.result = engine.types().intern(result);
        forged.types = definition_types(engine.types(), &forged);
        let violations =
            hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]).violations;
        assert!(
            violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::InvalidResultSource(_)
                    | hirsel_validate::ViolationKind::TypeMismatch { entity: "return" }
            )),
            "{violations:?}"
        );
    });
}
