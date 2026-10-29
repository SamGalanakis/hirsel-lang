use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_tir::Constant;
fn path(name: &str) -> DefPath {
    let mut p = entry_path(&default_context());
    p.0.name = name.into();
    p
}
fn engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("comptime.hir", source.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    engine
}

fn lowering_text(
    engine: &QueryEngine,
    value: Result<QueryOutput<hirsel_tir::DefinitionRef>, QueryError>,
) -> Result<(String, String, Vec<SignatureDependency>), String> {
    value
        .map(|output| {
            let cx = hirsel_proof_export::Cx::new(
                std::rc::Rc::new(|_| hirsel_proof_export::Location::unknown()),
                engine.types().clone(),
            );
            (
                hirsel_codec::to_text_with(
                    &hirsel_proof_export::DefinitionSection {
                        path: output.value.path.path().clone(),
                        locations: vec![],
                        tir: (*output.value).clone(),
                        tir_digest: hirsel_syntax::ContentHash::default(),
                        references: output.dependencies.clone(),
                        operations: vec![],
                        obligations: vec![],
                    },
                    cx,
                )
                .0,
                placed_evidence(&engine.sources, format!("{:?}", output.diagnostics)),
                output.dependencies,
            )
        })
        .map_err(|error| placed_evidence(&engine.sources, format!("{error:?}")))
}

fn materialized_constant_publishes_and_validates_on_compiler_thread() {
    let mut engine = engine("const VALUE: u32 = comptime (2 + 3)\n");
    let definition = engine.lower_to_tir(&path("VALUE")).unwrap().value;
    assert_eq!(
        definition.bodies[0].rvalues,
        vec![hirsel_tir::Rvalue::Constant(Constant::Integer(5))]
    );
}
#[test]
fn dependent_constants_materialize_before_their_readers_and_refresh_after_edits() {
    hirsel_engine::with_compiler_stack(dependent_constants_materialize_before_their_readers_and_refresh_after_edits_on_compiler_thread);
}
fn dependent_constants_materialize_before_their_readers_and_refresh_after_edits_on_compiler_thread()
{
    let mut engine =
        engine("const FIRST: u32 = comptime 3\nconst SECOND: u32 = comptime (FIRST + FIRST)\n");
    assert_eq!(
        engine.materialize_constant(&path("SECOND")).unwrap(),
        Constant::Integer(6)
    );
    let file = engine
        .sources
        .add(
            "comptime.hir",
            "const FIRST: u32 = comptime 8\nconst SECOND: u32 = comptime (FIRST + FIRST)\n".into(),
        )
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    assert_eq!(
        engine.materialize_constant(&path("SECOND")).unwrap(),
        Constant::Integer(16)
    );
}
#[test]
fn evaluation_budget_diagnostics_retry_after_budget_changes() {
    hirsel_engine::with_compiler_stack(
        evaluation_budget_diagnostics_retry_after_budget_changes_on_compiler_thread,
    );
}
fn evaluation_budget_diagnostics_retry_after_budget_changes_on_compiler_thread() {
    let mut engine = engine("fn five() -> u32 { 2 + 3 }\nconst VALUE: u32 = comptime five()\n");
    engine.comptime_limits.budget = 0;
    let Err(QueryError::Diagnostics(ds)) = engine.lower_to_tir(&path("VALUE")) else {
        panic!("expected budget exhaustion")
    };
    assert_eq!(ds[0].rule.as_str(), "CT-1");
    // Exhaustion is a language result, not a tooling limit.
    assert!(engine.last_comptime_limit.is_none());
    engine.comptime_limits.budget = 1;
    assert!(engine.lower_to_tir(&path("VALUE")).is_ok());
    assert_eq!(
        engine.materialize_constant(&path("VALUE")).unwrap(),
        Constant::Integer(5)
    );
}
#[test]
fn static_errors_keep_their_ordinary_owner() {
    hirsel_engine::with_compiler_stack(static_errors_keep_their_ordinary_owner_on_compiler_thread);
}
fn static_errors_keep_their_ordinary_owner_on_compiler_thread() {
    let mut engine = engine("const VALUE: u32 = comptime missing\n");
    let Err(QueryError::Diagnostics(ds)) = engine.lower_to_tir(&path("VALUE")) else {
        panic!("expected name error")
    };
    assert_eq!(ds[0].rule.as_str(), "LANG-4");
}

fn comptime_and_runtime_use_the_same_scalar_semantics_on_compiler_thread() {
    for expression in [
        "(2 + 3)",
        "if true { 7 } else { 9 }",
        "if false { 1 / 0 } else { 11 }",
    ] {
        let mut engine = engine(&format!(
            "const VALUE: u32 = comptime {expression}\nfn runtime() -> unique u32 {{ {expression} }}\n"
        ));
        let constant = engine.materialize_constant(&path("VALUE")).unwrap();
        let sources = engine.sources.clone();
        let runtime = engine.runtime_types().unwrap();
        let execution = hirsel_interp::run_with_runtime(
            &mut engine,
            &sources,
            &RunConfiguration {
                entry: path("runtime"),
                hash_seed: 0,
                inputs: vec![],
            },
            vec![],
            Default::default(),
            &mut hirsel_interp::NoOperations,
            &runtime,
        )
        .unwrap();
        let hirsel_interp::Outcome::Returned {
            value: hirsel_interp::Value::Integer(value),
            ..
        } = execution.outcome
        else {
            panic!("expected scalar return")
        };
        assert_eq!(constant, Constant::Integer(value.bits()));
    }
}
#[test]
fn pure_library_operations_are_reused_without_root_capabilities() {
    hirsel_engine::with_compiler_stack(
        pure_library_operations_are_reused_without_root_capabilities_on_compiler_thread,
    );
}
fn pure_library_operations_are_reused_without_root_capabilities_on_compiler_thread() {
    let mut engine = engine("const VALUE: u32 = comptime int.div_floor(9, 2)\n");
    assert_eq!(
        engine.materialize_constant(&path("VALUE")).unwrap(),
        Constant::Integer(4)
    );
}
#[test]
fn nested_comptime_evaluations_materialize_inside_out() {
    hirsel_engine::with_compiler_stack(
        nested_comptime_evaluations_materialize_inside_out_on_compiler_thread,
    );
}
fn nested_comptime_evaluations_materialize_inside_out_on_compiler_thread() {
    let mut engine = engine("const VALUE: u32 = comptime (comptime (2 + 3) + 4)\n");
    assert_eq!(
        engine.materialize_constant(&path("VALUE")).unwrap(),
        Constant::Integer(9)
    );
}

#[test]
fn compiler_dependency_depth_is_bounded_without_a_language_failure() {
    hirsel_engine::with_compiler_stack(
        compiler_dependency_depth_is_bounded_without_a_language_failure_on_compiler_thread,
    );
}
fn compiler_dependency_depth_is_bounded_without_a_language_failure_on_compiler_thread() {
    for count in [32, 33] {
        let mut source = String::new();
        for i in 0..count - 1 {
            source.push_str(&format!("const C{i}: u32 = comptime C{}\n", i + 1));
        }
        source.push_str(&format!("const C{}: u32 = comptime 1\n", count - 1));
        let mut engine = engine(&source);
        let result = engine.materialize_constant(&path("C0"));
        if count == 32 {
            assert_eq!(result.unwrap(), Constant::Integer(1));
            assert!(engine.last_comptime_limit.is_none());
        } else {
            let Err(QueryError::Operational(error)) = result else {
                panic!("expected dependency guard")
            };
            assert_eq!(error.operation, "compile-time tooling limit");
            assert_eq!(
                engine.last_comptime_limit.as_ref().unwrap().kind,
                hirsel_tir::ToolingLimitKind::CallDepth
            );
            assert_eq!(engine.last_comptime_limit.as_ref().unwrap().limit, 32);
        }
    }
}

#[test]
fn same_signature_function_body_edits_invalidate_materialized_values() {
    hirsel_engine::with_compiler_stack(
        same_signature_function_body_edits_invalidate_materialized_values_on_compiler_thread,
    );
}
fn same_signature_function_body_edits_invalidate_materialized_values_on_compiler_thread() {
    let mut engine =
        engine("fn value() -> unique u32 { 7 }\nconst VALUE: u32 = comptime value()\n");
    assert_eq!(
        engine.materialize_constant(&path("VALUE")).unwrap(),
        Constant::Integer(7)
    );
    let file = engine
        .sources
        .add(
            "comptime.hir",
            "fn value() -> unique u32 { 9 }\nconst VALUE: u32 = comptime value()\n".into(),
        )
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    assert_eq!(
        engine.materialize_constant(&path("VALUE")).unwrap(),
        Constant::Integer(9)
    );
}

#[test]
fn same_signature_body_edits_retry_prior_compile_time_language_failures() {
    hirsel_engine::with_compiler_stack(
        same_signature_body_edits_retry_prior_compile_time_language_failures_on_compiler_thread,
    );
}
fn same_signature_body_edits_retry_prior_compile_time_language_failures_on_compiler_thread() {
    let mut engine =
        engine("fn value() -> unique u32 { 1 / 0 }\nconst VALUE: u32 = comptime value()\n");
    let Err(QueryError::Diagnostics(ds)) = engine.lower_to_tir(&path("VALUE")) else {
        panic!("expected comptime failure")
    };
    assert_eq!(ds[0].rule.as_str(), "CT-1");
    let file = engine
        .sources
        .add(
            "comptime.hir",
            "fn value() -> unique u32 { 7 }\nconst VALUE: u32 = comptime value()\n".into(),
        )
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    engine.lower_to_tir(&path("VALUE")).unwrap();
}

#[test]
fn compile_time_constants_do_not_depend_on_unexecuted_finalizer_bodies() {
    hirsel_engine::with_compiler_stack(
        compile_time_constants_do_not_depend_on_unexecuted_finalizer_bodies_on_compiler_thread,
    );
}
fn compile_time_constants_do_not_depend_on_unexecuted_finalizer_bodies_on_compiler_thread() {
    let mut engine = engine(
        "resource type Unused { value: u32 }\ndrop(x) { let _ = VALUE }\nconst VALUE: u32 = comptime 7\n",
    );
    assert_eq!(
        engine.materialize_constant(&path("VALUE")).unwrap(),
        Constant::Integer(7)
    );
    assert_eq!(engine.recompute_count(&path("Unused"), QueryKind::Lower), 0);
}

#[test]
fn executed_compile_time_finalizers_use_the_runtime_scheduler_and_failure_policy() {
    hirsel_engine::with_compiler_stack(executed_compile_time_finalizers_use_the_runtime_scheduler_and_failure_policy_on_compiler_thread);
}
fn executed_compile_time_finalizers_use_the_runtime_scheduler_and_failure_policy_on_compiler_thread()
 {
    let mut engine = engine(
        "resource type Token { value: u32 }\ndrop(x) { let _ = 1u32 / 0u32 }\nfn value() -> unique u32 { let token = Token(value: 0); 7 }\nconst VALUE: u32 = comptime value()\n",
    );
    let result = engine.materialize_constant(&path("VALUE"));
    let Err(QueryError::Diagnostics(ds)) = result else {
        panic!("expected executed finalizer failure: {result:?}")
    };
    assert_eq!(ds[0].rule.as_str(), "CT-1");
    assert!(ds[0].notes.iter().any(|n| n.contains("SEM-1")));
}

#[test]
fn compiled_constant_installation_is_dependency_first_with_source_order_ties() {
    with_compiler_stack(|| {
        use hirsel_tir::TirProvider;
        let mut engine = engine(
            "const FIRST:u32=comptime SECOND\nconst INDEPENDENT:u32=1\nconst SECOND:u32=7\n",
        );
        let order = engine.constant_definitions().unwrap();
        let order: Vec<_> = order
            .into_iter()
            .filter(|p| p.0.package == path("FIRST").0.package)
            .collect();
        assert_eq!(order, [path("INDEPENDENT"), path("SECOND"), path("FIRST")]);
        assert_eq!(
            engine.materialize_constant(&path("FIRST")).unwrap(),
            Constant::Integer(7)
        );
    });
}

#[test]
fn source_comptime_values_have_no_data_nesting_cutoff() {
    hirsel_engine::with_compiler_stack(|| {
        for depth in [31, 32, 33] {
            let mut ty = "u32".to_string();
            let mut value = "7u32".to_string();
            for _ in 0..depth {
                ty = format!("({ty},)");
                value = format!("({value},)");
            }
            let mut engine = engine(&format!("const VALUE: {ty} = comptime {value}\n"));
            engine.lower_to_tir(&path("VALUE")).unwrap();
            assert!(engine.last_comptime_limit.is_none());
        }
    });
}

#[test]
fn size_constant_instances_cache_separately_and_refresh_executed_dependencies() {
    with_compiler_stack(|| {
        use hirsel_tir::{Rvalue, TirProvider, ValueKind};
        let source = |factor| {
            format!(
                "fn square(n:u64)->u64{{n*n*{factor}u64}}\nfn scaled[n:size](xs:Inline[u8,n])->u64{{let _=xs;comptime square(n)}}\n"
            )
        };
        let mut engine = engine(&source(1));
        let definition = engine.lower_to_tir(&path("scaled")).unwrap().value;
        let constant = definition.bodies[0]
            .rvalues
            .iter()
            .find_map(|value| match value {
                Rvalue::ReadConstant {
                    definition,
                    arguments,
                } if !arguments.is_empty() => Some(definition.clone()),
                _ => None,
            })
            .unwrap();
        for (size, expected) in [(2, 4), (4, 16), (2, 4)] {
            let value = engine.constant_instance(&constant, &[size]).unwrap();
            assert_eq!(
                value.root.kind,
                ValueKind::Constant(Constant::Integer(expected))
            );
        }
        let file = engine.sources.add("comptime.hir", source(2)).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        assert_eq!(
            engine.constant_instance(&constant, &[2]).unwrap().root.kind,
            ValueKind::Constant(Constant::Integer(8))
        );
        assert_eq!(
            engine.constant_instance(&constant, &[4]).unwrap().root.kind,
            ValueKind::Constant(Constant::Integer(32))
        );
        // Each size instance's evaluation charges its one call.
        engine.comptime_limits.budget = 0;
        let Err(QueryError::Diagnostics(ds)) = engine.constant_instance(&constant, &[2]) else {
            panic!("expected CT-1 after budget change")
        };
        assert_eq!(ds[0].rule.as_str(), "CT-1");
        engine.comptime_limits.budget = 1;
        assert_eq!(
            engine.constant_instance(&constant, &[2]).unwrap().root.kind,
            ValueKind::Constant(Constant::Integer(8))
        );
    });
}

#[test]
fn a_failed_size_instantiation_retains_its_selecting_site() {
    use hirsel_driver::requests::{CompileOperation, CompileResult, CompilerRequests};
    with_compiler_stack(|| {
        let source = include_str!("../../../../conformance/CT/CT-1-size-instantiation-bad.hir");
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(directory.path().join("comptime.hir"), source).unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"standalone","version":"0"},"sources":[{"path":"comptime.hir","module":[]}],"dependencies":[]}"#).unwrap();
        for jobs in [1, 2, 8] {
            let config = CompileConfig {
                jobs: hirsel_intern::jobs::Jobs::new(jobs),
                store: std::sync::Arc::new(store::MemStore::default()),
                hold_bytes: 0,
                ..Default::default()
            };
            for warm in [false, true] {
                let mut requests = CompilerRequests::for_host(
                    COMPILER.with_config(config.clone()),
                    config.clone(),
                );
                let output = requests
                    .execute(
                        &manifest,
                        CompileOperation::Check {
                            lowering: LoweringPolicy::Comptime,
                        },
                    )
                    .unwrap_or_else(|error| panic!("jobs={jobs} warm={warm}: {error}"));
                let CompileResult::Check(checked) = &output.result else {
                    panic!("expected checked source");
                };
                let diagnostic = checked
                    .diagnostics
                    .iter()
                    .find(|d| d.rule.as_str() == "CT-1")
                    .unwrap();
                assert!(
                    checked
                        .sources
                        .slice(diagnostic.primary)
                        .unwrap()
                        .starts_with("comptime")
                );
                assert!(
                    diagnostic
                        .secondary
                        .iter()
                        .any(
                            |label| checked.sources.slice(label.span) == Some("inverse(second)")
                                && label.message.contains("[0]")
                        ),
                    "{diagnostic:?}"
                );
            }
        }
    });
}

#[test]
fn staged_constants_survive_proof_export_and_unrelated_item_insertion() {
    with_compiler_stack(|| {
        let source = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\npub fn main()->Result[(),()]{let xs:Inline[u8,2]=[1,2];let _=scaled(xs);Ok(())}\n";
        let mut first = engine(source);
        let text = first.export_tir(&path("main")).unwrap();
        let export = first.import_tir(&text).unwrap();
        assert!(export.skipped.is_empty(), "{:?}", export.skipped);
        let constant = export
            .signatures
            .iter()
            .find(|s| s.path.0.name.starts_with("$comptime:scaled:"))
            .unwrap();
        assert_eq!(constant.public.generics().len(), 1);
        assert!(
            export
                .constants
                .iter()
                .any(|c| c.path == hirsel_tir::constant_instance_path(&constant.path, &[2]))
        );
        assert_eq!(hirsel_proof_export::export(&export.to_input()), text);
        let mut second = engine(&format!("fn unrelated()->u64{{3}}\n{source}"));
        let export = second.proof_export_input(&path("main")).unwrap();
        assert!(export.signatures.contains_key(&constant.path));
        assert_eq!(
            export.signatures[&constant.path].public.fingerprint,
            constant.public.fingerprint
        );
    });
}
#[test]
fn generated_bodies_regenerate_after_shape_and_generator_edits() {
    hirsel_engine::with_compiler_stack(
        generated_bodies_regenerate_after_shape_and_generator_edits_on_compiler_thread,
    );
}
fn generated_bodies_regenerate_after_shape_and_generator_edits_on_compiler_thread() {
    // CT-5: the text is cached under what the generator executed and the shape it read.
    let source = |fields: &str, three: &str| {
        format!(
            "type P {{ {fields} }}\n\
             fn gen[T](s: TypeShape[T]) -> Result[Body[fn(T) -> u64 uses {{}}], String] {{\n\
             let Record(fields) = type_shape.info(s).body else {{ return Error(\"record\") }}\n\
             if list.len(fields) == 2 {{ Ok(codegen.body(\"fn(p: standalone@0/P) -> u64 uses {{}} {{ p.x + p.y }}\")) }}\n\
             else {{ Ok(codegen.body(\"{three}\")) }}\n\
             }}\n\
             fn sum(p: P) -> u64 uses {{}} = comptime gen(shape(P))\n"
        )
    };
    let text = |engine: &mut QueryEngine, source: String| {
        let file = engine.sources.add("comptime.hir", source).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        engine.check_definition(&path("sum")).unwrap();
        engine.generated_source(&path("sum")).unwrap().text.clone()
    };
    let three = "fn(p: standalone@0/P) -> u64 uses {} { p.x + p.y + p.z }";
    let mut edited_engine = engine(&source("x: u64, y: u64", three));
    edited_engine.check_definition(&path("sum")).unwrap();
    let two = edited_engine
        .generated_source(&path("sum"))
        .unwrap()
        .text
        .clone();
    assert!(two.ends_with("{ p.x + p.y }"), "{two}");
    assert_eq!(
        text(&mut edited_engine, source("x: u64, y: u64, z: u64", three)),
        three
    );
    let edited = "fn(q: standalone@0/P) -> u64 uses {} { q.z }";
    assert_eq!(
        text(&mut edited_engine, source("x: u64, y: u64, z: u64", edited)),
        edited
    );
    let fresh = |source: String| {
        let mut engine = engine(&source);
        engine.check_definition(&path("sum")).unwrap();
        engine.generated_source(&path("sum")).unwrap().text.clone()
    };
    assert_eq!(fresh(source("x: u64, y: u64, z: u64", edited)), edited);
}
#[test]
fn generated_body_diagnostics_name_the_literal_parameter() {
    hirsel_engine::with_compiler_stack(
        generated_body_diagnostics_name_the_literal_parameter_on_compiler_thread,
    );
}
fn generated_body_diagnostics_name_the_literal_parameter_on_compiler_thread() {
    // CT-5: the body binds the declared parameter `t` by its literal's name `tag`,
    // so OWN-1 names and points at `tag`, the binding the generated text uses.
    let mut engine = engine(
        "type Tag { name: String }\n\
         fn renaming() -> Result[Body[fn(Tag) -> () uses {}], String] {\n\
         Ok(codegen.body(\"fn(tag: standalone@0/Tag) -> () uses {} { tag.name = \\\"x\\\" }\"))\n\
         }\n\
         fn rename(t: Tag) -> () uses {} = comptime renaming()\n",
    );
    let diagnostics = match engine.mode_definition(&path("rename")) {
        Ok(out) => out.diagnostics,
        Err(QueryError::Diagnostics(ds)) => ds,
        Err(e) => panic!("{e:?}"),
    };
    let own = diagnostics
        .iter()
        .find(|d| d.rule.as_str() == "OWN-1")
        .unwrap_or_else(|| panic!("{diagnostics:?}"));
    assert!(
        own.message.contains("`tag` is a borrowed parameter"),
        "{own:?}"
    );
    let generated = engine.generated_source(&path("rename")).unwrap().file;
    let provided = own
        .secondary
        .iter()
        .find(|l| l.message.starts_with("provided"))
        .unwrap_or_else(|| panic!("{own:?}"));
    assert_eq!(
        engine.sources.resolve(provided.span).file,
        generated,
        "{own:?}"
    );
    assert_eq!(engine.sources.slice(provided.span), Some("tag"), "{own:?}");
}

#[test]
fn generic_generated_instances_follow_their_declaration() {
    hirsel_engine::with_compiler_stack(
        generic_generated_instances_follow_their_declaration_on_compiler_thread,
    );
}
fn generic_generated_instances_follow_their_declaration_on_compiler_thread() {
    // CT-5: evaluation selects width[Point] lazily; editing the declaration
    // retires that instance, so the next evaluation generates it again.
    let source = |generator: &str| {
        format!(
            "type Point {{ x: u64 }}\n\
             fn two[T](s: TypeShape[T]) -> Result[Body[fn(T) -> u64 uses {{}}], String] {{\n\
             Ok(codegen.body(\"fn(v: standalone@0/Point) -> u64 uses {{}} {{ 2 }}\"))\n\
             }}\n\
             fn twelve[T](s: TypeShape[T]) -> Result[Body[fn(T) -> u64 uses {{}}], String] {{\n\
             Ok(codegen.body(\"fn(v: standalone@0/Point) -> u64 uses {{}} {{ 12 }}\"))\n\
             }}\n\
             fn width[T](v: T) -> u64 uses {{}} = comptime {generator}(shape(T))\n\
             const VALUE: u64 = comptime width(Point(x: 1))\n"
        )
    };
    let mut engine = engine(&source("two"));
    assert_eq!(
        engine.materialize_constant(&path("VALUE")).unwrap(),
        Constant::Integer(2)
    );
    let file = engine
        .sources
        .add("comptime.hir", source("twelve"))
        .unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    assert_eq!(
        engine.materialize_constant(&path("VALUE")).unwrap(),
        Constant::Integer(12)
    );
}
/// Lowered helper hits must replay the compile-time requests that produced
/// their constants at the new caller's logical dependency height.
#[test]
fn cached_helper_lowering_preserves_dependency_depth() {
    with_compiler_stack(|| {
        let source = "const LEAF: u32 = 1u32\nfn helper() -> u32 { comptime LEAF }\nconst C2: u32 = comptime helper()\nconst C1: u32 = comptime C2\nconst C0: u32 = comptime C1\n";
        for warm in [false, true] {
            let mut engine = engine(source);
            engine.comptime_limits.call_depth = 3;
            if warm {
                engine.lower_to_tir(&path("helper")).unwrap();
            }
            let result = engine.materialize_constant(&path("C0"));
            assert!(
                matches!(result, Err(QueryError::Operational(_))),
                "warm={warm}: {result:?}"
            );
            let limit = engine.last_comptime_limit.as_ref().unwrap();
            assert_eq!(limit.kind, hirsel_tir::ToolingLimitKind::CallDepth);
            assert_eq!(limit.limit, 3);
            let site = engine.sources.resolve(limit.site.unwrap());
            assert_eq!(
                site.start,
                source.find("comptime LEAF").unwrap() as u32 + "comptime ".len() as u32
            );
        }
    });
}

/// Anchor numbers belong to one source map. Compare every error field and
/// limit site after replacing them by their actual source path and base.
fn placed_evidence(sources: &hirsel_span::SourceMap, text: String) -> String {
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("Anchor(") {
        out.push_str(&rest[..at]);
        rest = &rest[at + "Anchor(".len()..];
        let end = rest.find(')').unwrap();
        let anchor = hirsel_span::Anchor(rest[..end].parse().unwrap());
        let place = sources
            .anchor(anchor)
            .map(|(file, base)| (sources.file(file).unwrap().path.clone(), base));
        out.push_str(&format!("Anchor({place:?})"));
        rest = &rest[end + 1..];
    }
    out + rest
}
fn outcome_text(
    engine: &QueryEngine,
    result: Result<hirsel_tir::Materialized, QueryError>,
) -> Result<String, String> {
    result
        .map(|value| materialized_text(engine, &value))
        .map_err(|error| placed_evidence(&engine.sources, format!("{error:?}")))
}
fn materialized_text(engine: &QueryEngine, value: &hirsel_tir::Materialized) -> String {
    let context = hirsel_proof_export::Cx::new(
        std::rc::Rc::new(|_| hirsel_proof_export::Location::unknown()),
        engine.types().clone(),
    );
    hirsel_codec::to_text_with(value, context).0
}
fn frontier_text(
    engine: &mut QueryEngine,
    paths: Vec<DefPath>,
    jobs: usize,
) -> Vec<Result<String, String>> {
    engine
        .materialize_values(paths, hirsel_intern::jobs::Jobs::new(jobs))
        .into_iter()
        .map(|value| outcome_text(engine, value))
        .collect()
}

/// The production source, checker, V and interpreter must agree with scalar
/// evaluation under a cold frontier, duplicate roots, partial hits and edits.
#[test]
fn frozen_frontier_values_refresh_and_preserve_input_order() {
    with_compiler_stack(|| {
        let source = |leaf| {
            format!(
                "fn helper() -> u32 {{ {leaf} }}\nconst LEAF: u32 = comptime helper()\nconst A: u32 = comptime (LEAF + 1)\nconst B: u32 = comptime (LEAF + 2)\n"
            )
        };
        let paths = vec![path("B"), path("A"), path("B"), path("LEAF")];
        for jobs in [1, 2, 8] {
            let mut candidate = engine(&source(3));
            candidate.materialize_value(&path("LEAF")).unwrap();
            for leaf in [3, 9] {
                let text = source(leaf);
                let file = candidate.sources.add("comptime.hir", text.clone()).unwrap();
                candidate.parse_file(file, &default_context()).unwrap();
                let mut control = engine(&text);
                let expected: Vec<_> = paths
                    .iter()
                    .map(|p| {
                        let result = control.materialize_value(p);
                        outcome_text(&control, result)
                    })
                    .collect();
                for _ in 0..2 {
                    assert_eq!(
                        frontier_text(&mut candidate, paths.clone(), jobs),
                        expected,
                        "jobs={jobs} leaf={leaf}"
                    );
                }
            }
        }
    });
}

#[test]
fn frozen_frontier_cycles_and_failures_match_serial() {
    with_compiler_stack(|| {
        let source = "const GOOD: u32 = comptime 7\nconst A: u32 = comptime B\nconst B: u32 = comptime C\nconst C: u32 = comptime A\nconst BAD: u32 = comptime missing\n";
        let paths = vec![path("C"), path("A"), path("GOOD"), path("B"), path("BAD")];
        for jobs in [1, 2, 8] {
            for warm in [false, true] {
                let mut control = engine(source);
                let expected: Vec<_> = paths
                    .iter()
                    .map(|p| {
                        let result = control.materialize_value(p);
                        outcome_text(&control, result)
                    })
                    .collect();
                let mut candidate = engine(source);
                if warm {
                    candidate.materialize_value(&path("GOOD")).unwrap();
                }
                let actual = frontier_text(&mut candidate, paths.clone(), jobs);
                assert_eq!(actual, expected, "jobs={jobs} warm={warm}");
            }
        }
    });
}

#[test]
fn frozen_frontier_depth_and_last_limit_match_serial_with_partial_hits() {
    with_compiler_stack(|| {
        let source = "const LEAF: u32 = 1u32\nfn helper() -> u32 { comptime LEAF }\nconst C2: u32 = comptime helper()\nconst C1: u32 = comptime C2\nconst C0: u32 = comptime C1\nconst GOOD: u32 = comptime 7\n";
        for jobs in [1, 2, 8] {
            for warm in [false, true] {
                for names in [["C0", "GOOD"], ["GOOD", "C0"]] {
                    let paths: Vec<_> = names.map(path).into();
                    let mut control = engine(source);
                    control.comptime_limits.call_depth = 3;
                    let expected: Vec<_> = paths
                        .iter()
                        .map(|p| {
                            let result = control.materialize_value(p);
                            outcome_text(&control, result)
                        })
                        .collect();
                    let mut candidate = engine(source);
                    candidate.comptime_limits.call_depth = 3;
                    candidate.materialize_value(&path("GOOD")).unwrap();
                    if warm {
                        candidate.lower_to_tir(&path("helper")).unwrap();
                    }
                    let actual = frontier_text(&mut candidate, paths, jobs);
                    assert_eq!(actual, expected, "jobs={jobs} warm={warm} names={names:?}");
                    assert_eq!(
                        placed_evidence(
                            &candidate.sources,
                            format!("{:?}", candidate.last_comptime_limit)
                        ),
                        placed_evidence(
                            &control.sources,
                            format!("{:?}", control.last_comptime_limit)
                        ),
                        "jobs={jobs} warm={warm} names={names:?}"
                    );
                }
            }
        }
    });
}

/// A rendezvous inside the real evaluation-lowering adapter proves independent
/// roots execute on distinct workers. A serial or whole-query-locked facade
/// times out here instead of accidentally passing the value differential.
#[test]
fn frozen_frontier_evaluation_tasks_overlap() {
    use std::sync::{Arc, Condvar, Mutex};
    struct Overlap {
        inner: Arc<dyn LoweringPhase>,
        workers: Mutex<Vec<std::thread::ThreadId>>,
        ready: Condvar,
    }
    impl LoweringPhase for Overlap {
        fn lower(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            comptime: &Comptime,
            signature: &hirsel_types::Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
            self.inner.lower(t, m, body, comptime, signature)
        }
        fn lower_evaluation(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
            signature: &hirsel_types::Signature,
            expression: hirsel_syntax::ExprId,
            result: hirsel_types::TypeId,
            constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
        ) -> Result<hirsel_tir::Definition, QueryError> {
            let mut workers = self.workers.lock().unwrap();
            let thread = std::thread::current().id();
            if !workers.contains(&thread) {
                workers.push(thread);
            }
            self.ready.notify_all();
            let (workers, _) = self
                .ready
                .wait_timeout_while(workers, std::time::Duration::from_secs(10), |workers| {
                    workers.len() < 2
                })
                .unwrap();
            assert!(workers.len() >= 2, "evaluation work did not overlap");
            drop(workers);
            self.inner
                .lower_evaluation(t, m, body, staged, signature, expression, result, constants)
        }
        fn entry_body(&self) -> hirsel_tir::BodyId {
            self.inner.entry_body()
        }
    }
    with_compiler_stack(|| {
        for jobs in [2, 8] {
            let mut adapters = compiler_adapters();
            let overlap = Arc::new(Overlap {
                inner: adapters.lowering.take().unwrap(),
                workers: Mutex::new(vec![]),
                ready: Condvar::new(),
            });
            adapters.lowering = Some(overlap.clone());
            let mut candidate = QueryEngine::new(default_environment(), adapters);
            let file = candidate
                .sources
                .add(
                    "overlap.hir",
                    "const A: u32 = comptime 11\nconst B: u32 = comptime 12\n".into(),
                )
                .unwrap();
            candidate.parse_file(file, &default_context()).unwrap();
            let values = candidate.materialize_values(
                vec![path("A"), path("B")],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert!(values.iter().all(Result::is_ok), "{values:?}");
            assert!(overlap.workers.lock().unwrap().len() >= 2);
        }
    });
}

/// Staged registrations require a new snapshot round, but their evaluations
/// must return to workers. A caller-thread retry is observable here.
#[test]
fn frozen_frontier_staged_rounds_evaluate_on_workers() {
    use std::sync::Arc;
    struct WorkerOnly(Arc<dyn LoweringPhase>, bool);
    impl LoweringPhase for WorkerOnly {
        fn lower(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            comptime: &Comptime,
            signature: &hirsel_types::Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
            self.0.lower(t, m, body, comptime, signature)
        }
        fn lower_evaluation(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
            signature: &hirsel_types::Signature,
            expression: hirsel_syntax::ExprId,
            result: hirsel_types::TypeId,
            constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
        ) -> Result<hirsel_tir::Definition, QueryError> {
            if self.1 {
                assert_eq!(
                    std::thread::current().name(),
                    Some("hirsel-query"),
                    "staged demand fell back to caller evaluation"
                );
            }
            self.0
                .lower_evaluation(t, m, body, staged, signature, expression, result, constants)
        }
        fn entry_body(&self) -> hirsel_tir::BodyId {
            self.0.entry_body()
        }
    }
    thread_local! {
        static REQUIRE_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    fn worker_adapters() -> PhaseAdapters {
        let mut adapters = compiler_adapters();
        adapters.lowering = Some(Arc::new(WorkerOnly(
            adapters.lowering.take().unwrap(),
            REQUIRE_WORKER.with(|required| required.get()),
        )));
        adapters
    }
    with_compiler_stack(|| {
        let staged = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst A:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst B:u64=comptime {let xs:Inline[u8,4]=[1,2,3,4];scaled(xs)}\n";
        let generated = "type Seed {x:u32}\nfn body_text()->String{\"fn()->u32 uses {}{11u32}\"}\nfn make(s:TypeShape[Seed])->Result[Body[fn()->u32 uses {}],String]{Ok(codegen.body(body_text()))}\nfn chosen()->u32 uses {}=comptime make(shape(Seed))\nconst A:u32=comptime chosen()\nconst B:u32=comptime chosen()\n";
        let generic = "type Point {x:u64}\nfn make[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{Ok(codegen.body(\"fn(p:standalone@0/Point)->u64 uses {}{12u64}\"))}\nfn width[T](p:T)->u64 uses {}=comptime make(shape(T))\nconst A:u64=comptime width(Point(x:1))\nconst B:u64=comptime width(Point(x:2))\n";
        for source in [staged, generated, generic] {
            for jobs in [1, 2, 8] {
                let paths = vec![path("B"), path("A"), path("B")];
                let mut control = engine(source);
                let expected: Vec<_> = paths
                    .iter()
                    .map(|p| {
                        let result = control.materialize_value(p);
                        outcome_text(&control, result)
                    })
                    .collect();
                assert!(expected.iter().all(Result::is_ok), "{expected:?}");
                use hirsel_driver::requests::{CompileOperation, CompileResult, CompilerRequests};
                let directory = tempfile::tempdir().unwrap();
                let manifest = directory.path().join("hirsel.json");
                std::fs::write(directory.path().join("comptime.hir"), source).unwrap();
                std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"standalone","version":"0"},"sources":[{"path":"comptime.hir","module":[]}],"dependencies":[]}"#).unwrap();
                let config = CompileConfig {
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    store: Arc::new(store::DirStore::new(directory.path().join("products"))),
                    hold_bytes: 0,
                    ..Default::default()
                };
                REQUIRE_WORKER.with(|required| required.set(jobs > 1));
                let compiler = COMPILER.with_config(config.clone()).with_phase_adapters(
                    worker_adapters,
                    hirsel_syntax::ContentHash::of_bytes(b"hirsel.staged-request-worker.v1").0,
                );
                let mut candidate = CompilerRequests::for_host(compiler, config);
                for warm in [false, true] {
                    let output = candidate
                        .execute(&manifest, CompileOperation::Precompute)
                        .unwrap_or_else(|error| panic!("jobs={jobs} warm={warm}: {error}"));
                    let CompileResult::Precomputed { sources, .. } = &output.result else {
                        panic!("expected source precompute");
                    };
                    let actual: Vec<Result<String, String>> = paths
                        .iter()
                        .map(|path| {
                            let bound = sources.source_product_bytes(path).unwrap().unwrap();
                            let budget = hirsel_engine::runtime::ProcessBudget::new(bound);
                            let products = sources
                                .source_products(path, budget.reserve(bound).unwrap())
                                .unwrap()
                                .unwrap();
                            let context = hirsel_proof_export::Cx::new(
                                std::rc::Rc::new(|_| hirsel_proof_export::Location::unknown()),
                                sources.signatures().types().clone(),
                            );
                            Ok(hirsel_codec::to_text_with(products.constant().unwrap(), context).0)
                        })
                        .collect();
                    assert_eq!(actual, expected, "jobs={jobs} warm={warm}");
                    if source == generated {
                        let scalar = control.check_definition(&path("chosen")).unwrap();
                        let bound = sources.dependency_decode_bytes(&path("chosen")).unwrap();
                        let budget = hirsel_engine::runtime::ProcessBudget::new(bound);
                        let worker = sources
                            .dependencies(&path("chosen"), budget.reserve(bound).unwrap())
                            .unwrap();
                        assert!(
                            scalar
                                .dependencies
                                .iter()
                                .any(|d| *d.definition.path() == path("make"))
                        );
                        assert_eq!(
                            worker.dependencies(),
                            &*scalar.dependencies,
                            "checked dependencies jobs={jobs} warm={warm}"
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn frozen_frontier_outcomes_capture_each_stopping_phase_and_limit() {
    with_compiler_stack(|| {
        for jobs in [1, 2, 8] {
            let mut adapters = compiler_adapters();
            adapters.checker = None;
            let mut candidate = QueryEngine::new(default_environment(), adapters);
            let file = candidate
                .sources
                .add(
                    "outcomes.hir",
                    "const ABSENT:u32=comptime 2\nconst BAD:u32=comptime missing\n".into(),
                )
                .unwrap();
            candidate.parse_file(file, &default_context()).unwrap();
            let outcomes = candidate.materialize_value_outcomes(
                vec![path("ABSENT"), path("BAD")],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert!(matches!(
                outcomes[0].result,
                Err(QueryError::NotImplemented)
            ));
            assert_eq!(outcomes[0].phase, Some(QueryKind::Check));
            assert!(matches!(
                outcomes[1].result,
                Err(QueryError::Diagnostics(_))
            ));
            assert_eq!(outcomes[1].phase, Some(QueryKind::Resolve));
            assert_eq!(candidate.last_phase, outcomes[1].phase);
            assert!(outcomes.iter().all(|outcome| outcome.limit.is_none()));
            assert!(outcomes.iter().all(|outcome| !outcome.reads.is_empty()));

            let source = "const LEAF:u32=1u32\nconst C1:u32=comptime LEAF\nconst C0:u32=comptime C1\nconst GOOD:u32=comptime 7\n";
            let mut candidate = engine(source);
            candidate.comptime_limits.call_depth = 2;
            let outcomes = candidate.materialize_value_outcomes(
                vec![path("C0"), path("GOOD")],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert!(matches!(
                outcomes[0].result,
                Err(QueryError::Operational(_))
            ));
            assert_eq!(outcomes[0].phase, Some(QueryKind::Lower));
            assert_eq!(
                outcomes[0].limit.as_ref().unwrap().kind,
                hirsel_tir::ToolingLimitKind::CallDepth
            );
            assert!(outcomes[1].result.is_ok());
            assert!(outcomes[1].limit.is_none());
            assert!(candidate.last_comptime_limit.is_none());
            assert_eq!(outcomes[1].phase, Some(QueryKind::Lower));
            assert!(outcomes[0].reads.iter().any(|(key, _)| *key
                == memo::ReadKey::ExecutedBody(hirsel_syntax::DefId::intern(&path("C0")))));
        }
    });
}

#[test]
fn frozen_frontier_generator_failures_keep_scalar_cycle_and_literal_sites() {
    with_compiler_stack(|| {
        let make = |body: &str| {
            format!(
                "type Seed {{x:u32}}\nfn make(s:TypeShape[Seed])->Result[Body[fn()->u32 uses {{}}],String]{{{body}}}\nfn chosen()->u32 uses {{}}=comptime make(shape(Seed))\nconst A:u32=comptime chosen()\nconst B:u32=comptime chosen()\n"
            )
        };
        for (rule, source) in [
            (
                "CT-4",
                make("let _=A;Ok(codegen.body(\"fn()->u32 uses {}{11u32}\"))"),
            ),
            ("CT-1", make("Error(\"expected generator failure\")")),
            (
                "LANG-7",
                make("Ok(codegen.body(\"fn()->u64 uses {}{11u64}\"))"),
            ),
        ] {
            let paths = vec![path("B"), path("A"), path("B")];
            let mut control = engine(&source);
            let expected: Vec<_> = paths
                .iter()
                .map(|p| {
                    let result = control.materialize_value(p);
                    outcome_text(&control, result)
                })
                .collect();
            assert!(
                expected
                    .iter()
                    .all(|result| result.as_ref().is_err_and(|error| error.contains(rule))),
                "{expected:?}; expected {rule}"
            );
            for jobs in [1, 2, 8] {
                let mut candidate = engine(&source);
                let actual = frontier_text(&mut candidate, paths.clone(), jobs);
                assert_eq!(actual, expected, "jobs={jobs} source={source}");
            }
        }
    });
}

#[test]
fn frozen_frontier_generator_helper_edits_refresh_text_and_execution_reads() {
    with_compiler_stack(|| {
        let source = "type Seed {x:u32}\nfn body_text()->String{\"fn()->u32 uses {}{11u32}\"}\nfn make(s:TypeShape[Seed])->Result[Body[fn()->u32 uses {}],String]{Ok(codegen.body(body_text()))}\nfn chosen()->u32 uses {}=comptime make(shape(Seed))\nconst A:u32=comptime chosen()\nconst B:u32=comptime chosen()\n";
        let paths = vec![path("B"), path("A"), path("B")];
        for jobs in [1, 2, 8] {
            let mut candidate = engine(source);
            for revision in [
                source.to_owned(),
                source.replace("11u32", "23u32"),
                source.replace("codegen.body(body_text())", "codegen.body(missing_text())"),
                source.to_owned(),
            ] {
                let file = candidate
                    .sources
                    .add("comptime.hir", revision.clone())
                    .unwrap();
                candidate.parse_file(file, &default_context()).unwrap();
                let mut control = engine(&revision);
                let expected: Vec<_> = paths
                    .iter()
                    .map(|p| {
                        let result = control.materialize_value(p);
                        outcome_text(&control, result)
                    })
                    .collect();
                let actual = frontier_text(&mut candidate, paths.clone(), jobs);
                assert_eq!(actual, expected, "jobs={jobs} source={revision}");
                if revision.contains("missing_text()") {
                    assert!(actual.iter().all(Result::is_err));
                }
                if actual.iter().all(Result::is_ok) {
                    let scalar = control.check_definition(&path("chosen")).unwrap();
                    let worker = candidate.check_definition(&path("chosen")).unwrap();
                    assert_eq!(worker.dependencies, scalar.dependencies);
                    let reads = candidate
                        .memo_reads(&path("chosen"), QueryKind::Check)
                        .unwrap();
                    assert!(
                        reads.iter().any(|(key, _)| *key
                            == memo::ReadKey::ExecutedBody(hirsel_syntax::DefId::intern(&path(
                                "body_text"
                            )))),
                        "jobs={jobs} revision={revision}\nworker={reads:?}\nscalar={:?}",
                        control.memo_reads(&path("chosen"), QueryKind::Check)
                    );
                }
            }
        }
    });
}

#[test]
fn frozen_frontier_generic_generator_failures_keep_selecting_site_labels() {
    with_compiler_stack(|| {
        for (rule, body, nested) in [
            ("CT-1", "Error(\"generic generator failed\")", false),
            ("CT-4", "let _=A;Error(\"unreachable\")", false),
            (
                "CT-1",
                "let _=inner(Point(x:0));Error(\"unreachable\")",
                true,
            ),
            (
                "LANG-7",
                "Ok(codegen.body(\"fn(p:standalone@0/Point)->u32 uses {}{12u32}\"))",
                false,
            ),
        ] {
            let source = format!(
                "type Point {{x:u64}}\nfn make[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {{}}],String]{{{body}}}\nfn width[T](p:T)->u64 uses {{}}=comptime make(shape(T))\nconst A:u64=comptime width(Point(x:1))\nconst B:u64=comptime width(Point(x:2))\n"
            );
            let source = if nested {
                source
                    + "fn inner_make[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{Error(\"inner failed\")}\nfn inner[T](p:T)->u64 uses {}=comptime inner_make(shape(T))\n"
            } else {
                source
            };
            for jobs in [1, 2, 8] {
                for warm in [false, true] {
                    let paths = vec![path("B"), path("A"), path("B")];
                    let mut control = engine(&source);
                    let mut candidate = engine(&source);
                    if warm {
                        assert!(control.materialize_value(&path("A")).is_err());
                        assert!(candidate.materialize_value(&path("A")).is_err());
                    }
                    let expected: Vec<_> = paths
                        .iter()
                        .map(|p| {
                            let result = control.materialize_value(p);
                            outcome_text(&control, result)
                        })
                        .collect();
                    assert!(
                        expected
                            .iter()
                            .all(|result| result.as_ref().is_err_and(|error| error.contains(rule)))
                    );
                    if !warm {
                        assert!(
                            expected[0]
                                .as_ref()
                                .unwrap_err()
                                .contains("selects instance")
                        );
                    }
                    let actual = frontier_text(&mut candidate, paths, jobs);
                    assert_eq!(actual, expected, "jobs={jobs} warm={warm} rule={rule}");
                }
            }
        }
    });
}

#[test]
fn frozen_frontier_releases_completed_typed_bodies_before_next_root() {
    use std::sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    };
    struct ObservedModes {
        inner: Arc<dyn ModesPhase>,
        bodies: Mutex<Vec<Weak<TypedBody>>>,
        peak: AtomicUsize,
    }
    impl ModesPhase for ObservedModes {
        fn check_modes(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: Arc<TypedBody>,
            signature: &hirsel_types::Signature,
            products: &mut PhaseProducts,
            provider: &mut dyn hirsel_types::SignatureProvider,
        ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
            let output = self
                .inner
                .check_modes(t, m, body, signature, products, provider)?;
            let mut bodies = self.bodies.lock().unwrap();
            bodies.retain(|body| body.strong_count() != 0);
            bodies.push(Arc::downgrade(&output.value.typed));
            self.peak.fetch_max(bodies.len(), Ordering::Relaxed);
            Ok(output)
        }
    }
    with_compiler_stack(|| {
        for jobs in [1, 2, 8] {
            let mut adapters = compiler_adapters();
            let observed = Arc::new(ObservedModes {
                inner: adapters.modes.take().unwrap(),
                bodies: Mutex::new(vec![]),
                peak: AtomicUsize::new(0),
            });
            adapters.modes = Some(observed.clone());
            let mut candidate = QueryEngine::new(default_environment(), adapters);
            let source: String = (0..64)
                .map(|i| format!("const task{i}:u64=comptime {i}u64\n"))
                .collect();
            let file = candidate.sources.add("bounded-s1.hir", source).unwrap();
            candidate.parse_file(file, &default_context()).unwrap();
            let paths: Vec<_> = (0..64).rev().map(|i| path(&format!("task{i}"))).collect();
            let outcomes = candidate
                .materialize_value_outcomes(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
            assert!(outcomes.iter().all(|outcome| outcome.result.is_ok()));
            assert_eq!(
                outcomes.iter().map(|o| &o.definition).collect::<Vec<_>>(),
                paths.iter().collect::<Vec<_>>()
            );
            let peak = observed.peak.load(Ordering::Relaxed);
            assert!(
                peak <= jobs,
                "jobs={jobs}: {peak} typed bodies accumulated across64 S1 roots"
            );
            assert!(
                observed
                    .bodies
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|body| body.strong_count() == 0)
            );
            for path in paths {
                assert!(
                    candidate
                        .definition_summary(hirsel_syntax::DefId::intern(&path))
                        .is_ok()
                );
                assert_eq!(candidate.recompute_count(&path, QueryKind::Check), 1);
                assert_eq!(candidate.recompute_count(&path, QueryKind::Modes), 1);
            }
        }
    });
}

#[test]
fn frozen_lowering_frontier_matches_scalar_for_nonconstant_comptime_and_edits() {
    with_compiler_stack(|| {
        let source = "const SEED:u32=7u32\nfn helper()->u32{SEED}\nfn A()->u32{comptime helper()}\nfn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nfn B()->u64{comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}}\nfn BAD()->u32{comptime missing()}\n";
        let paths = vec![path("B"), path("A"), path("BAD"), path("A")];
        for jobs in [1, 2, 8] {
            let mut candidate = engine(source);
            for revision in [
                source.to_owned(),
                source.replace("7u32", "23u32"),
                source.replace("missing()", "helper()"),
                source.to_owned(),
            ] {
                let file = candidate
                    .sources
                    .add("comptime.hir", revision.clone())
                    .unwrap();
                candidate.parse_file(file, &default_context()).unwrap();
                let mut control = engine(&revision);
                let expected: Vec<_> = paths
                    .iter()
                    .map(|path| {
                        let value = control.lower_to_tir(path);
                        lowering_text(&control, value)
                    })
                    .collect();
                assert!(
                    expected[0].is_ok() && expected[1].is_ok(),
                    "expected accepted nonconstant roots: {expected:?}"
                );
                let actual = candidate
                    .lower_definition_outcomes(paths.clone(), hirsel_intern::jobs::Jobs::new(jobs));
                assert_eq!(
                    actual
                        .iter()
                        .map(|outcome| &outcome.definition)
                        .collect::<Vec<_>>(),
                    paths.iter().collect::<Vec<_>>()
                );
                for outcome in &actual {
                    assert!(!outcome.reads.is_empty());
                    assert!(outcome.limit.is_none());
                    assert_eq!(
                        outcome.phase,
                        Some(if outcome.result.is_ok() {
                            QueryKind::Lower
                        } else {
                            QueryKind::Resolve
                        })
                    );
                    if outcome.definition == path("A") {
                        for name in ["A", "helper", "SEED"] {
                            assert!(
                                outcome.reads.iter().any(|(key, _)| *key
                                    == memo::ReadKey::ExecutedBody(hirsel_syntax::DefId::intern(
                                        &path(name)
                                    ))),
                                "missing execution read {name}"
                            );
                        }
                    }
                }
                let actual: Vec<_> = actual
                    .into_iter()
                    .map(|outcome| lowering_text(&candidate, outcome.result))
                    .collect();
                assert_eq!(actual, expected, "jobs={jobs} source={revision}");
            }
        }
    });
}

#[test]
fn frozen_lowering_frontier_keeps_lower_warnings_and_current_hit_metadata() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct WarningLower {
        inner: Arc<dyn LoweringPhase>,
        guard: Arc<AtomicBool>,
    }
    impl LoweringPhase for WarningLower {
        fn lower(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            comptime: &Comptime,
            signature: &hirsel_types::Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
            if self.guard.load(Ordering::Relaxed) {
                assert_eq!(
                    std::thread::current().name(),
                    Some("hirsel-job"),
                    "nonconstant root fell back to caller lowering"
                );
            }
            let mut output = self.inner.lower(t, m, body, comptime, signature)?;
            let mut warning = hirsel_diag::Diagnostic::error(
                hirsel_diag::RuleId::of("LANG-1"),
                body.typed.syntax.item.name_span,
                "lowering fixture warning",
            );
            warning.severity = hirsel_diag::Severity::Warning;
            output.diagnostics.push(warning);
            Ok(output)
        }
        fn lower_evaluation(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
            signature: &hirsel_types::Signature,
            expression: hirsel_syntax::ExprId,
            result: hirsel_types::TypeId,
            constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
        ) -> Result<hirsel_tir::Definition, QueryError> {
            if self.guard.load(Ordering::Relaxed) {
                assert_eq!(
                    std::thread::current().name(),
                    Some("hirsel-job"),
                    "nonconstant evaluation fell back to caller"
                );
            }
            self.inner
                .lower_evaluation(t, m, body, staged, signature, expression, result, constants)
        }
        fn entry_body(&self) -> hirsel_tir::BodyId {
            self.inner.entry_body()
        }
    }
    let make = || {
        let mut adapters = compiler_adapters();
        let guard = Arc::new(AtomicBool::new(false));
        adapters.lowering = Some(Arc::new(WarningLower {
            inner: adapters.lowering.take().unwrap(),
            guard: guard.clone(),
        }));
        let mut engine = QueryEngine::new(default_environment(), adapters);
        let file = engine
            .sources
            .add(
                "comptime.hir",
                "const SEED:u32=7u32\nfn A()->u32{comptime SEED}\n".into(),
            )
            .unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        (engine, guard)
    };
    with_compiler_stack(|| {
        for jobs in [1, 2, 8] {
            for warm in [false, true] {
                let (mut control, _) = make();
                let expected = control.lower_to_tir(&path("A")).unwrap();
                assert_eq!(expected.diagnostics.len(), 1);
                assert_eq!(
                    expected.diagnostics[0].severity,
                    hirsel_diag::Severity::Warning
                );
                let expected_depth = control
                    .lower_definition_outcomes(vec![path("A")], hirsel_intern::jobs::Jobs::new(1))
                    .remove(0)
                    .depth
                    .unwrap();
                let scalar_reads = control.memo_reads(&path("A"), QueryKind::Lower).unwrap();
                let (mut candidate, guard) = make();
                if warm {
                    assert_eq!(
                        candidate
                            .lower_to_tir(&path("A"))
                            .unwrap()
                            .diagnostics
                            .len(),
                        1
                    );
                }
                guard.store(jobs > 1, Ordering::Relaxed);
                let outcomes = candidate.lower_definition_outcomes(
                    vec![path("A"), path("A")],
                    hirsel_intern::jobs::Jobs::new(jobs),
                );
                for outcome in outcomes {
                    assert_eq!(outcome.phase, Some(QueryKind::Lower));
                    assert!(outcome.limit.is_none());
                    let depth = outcome
                        .depth
                        .as_ref()
                        .expect("successful lower returns depth");
                    assert_eq!(
                        placed_evidence(&candidate.sources, format!("{depth:?}")),
                        placed_evidence(&control.sources, format!("{expected_depth:?}")),
                        "jobs={jobs} warm={warm}"
                    );
                    assert!(
                        depth.first.iter().any(Option::is_some),
                        "constant requests must survive in lower depth evidence"
                    );
                    let output = outcome.result.unwrap();
                    assert_eq!(output.dependencies, expected.dependencies);
                    assert_eq!(
                        placed_evidence(&candidate.sources, format!("{:?}", output.diagnostics)),
                        placed_evidence(&control.sources, format!("{:?}", expected.diagnostics))
                    );
                    for (key, value) in scalar_reads.iter() {
                        if matches!(
                            key,
                            memo::ReadKey::ValidationView(_)
                                | memo::ReadKey::UniverseErrors
                                | memo::ReadKey::FirstHeap
                        ) {
                            continue;
                        }
                        assert_eq!(
                            outcome.reads.get(key),
                            Some(*value),
                            "jobs={jobs} warm={warm} missing {key:?}"
                        );
                    }
                    assert!(
                        outcome
                            .reads
                            .iter()
                            .any(|(key, _)| matches!(key, memo::ReadKey::ValidationUniverse(_)))
                    );
                }
                let compact = candidate.check_lower_definition_prefix(
                    vec![path("A"), path("A")],
                    hirsel_intern::jobs::Jobs::new(jobs),
                );
                assert_eq!(compact.len(), 2);
                for outcome in compact {
                    assert_eq!(outcome.phase, Some(QueryKind::Lower));
                    assert!(outcome.limit.is_none());
                    assert_eq!(
                        placed_evidence(
                            &candidate.sources,
                            format!("{:?}", outcome.depth.as_ref().unwrap())
                        ),
                        placed_evidence(&control.sources, format!("{expected_depth:?}"))
                    );
                    let output = outcome.result.unwrap();
                    assert_eq!(output.dependencies, expected.dependencies);
                    assert_eq!(
                        placed_evidence(&candidate.sources, format!("{:?}", output.diagnostics)),
                        placed_evidence(&control.sources, format!("{:?}", expected.diagnostics))
                    );
                    assert!(
                        outcome
                            .reads
                            .iter()
                            .any(|(key, _)| matches!(key, memo::ReadKey::ValidationUniverse(_)))
                    );
                }
            }
        }
    });
}

#[test]
fn frozen_frontier_hold_budgets_preserve_staged_generated_cycle_and_depth_outcomes() {
    with_compiler_stack(|| {
        let cases = [
            (
                8,
                "const LEAF:u32=1u32\nfn helper()->u32{comptime LEAF}\nconst A:u32=comptime helper()\nconst B:u32=comptime A\n",
            ),
            (
                2,
                "const LEAF:u32=1u32\nfn helper()->u32{comptime LEAF}\nconst A:u32=comptime helper()\nconst B:u32=comptime A\n",
            ),
            (8, "const A:u32=comptime B\nconst B:u32=comptime A\n"),
            (
                8,
                "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst A:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst B:u64=comptime A\n",
            ),
            (
                8,
                "type Point{x:u64}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{Ok(codegen.body(\"fn(v:standalone@0/Point)->u64 uses {}{12}\"))}\nfn width[T](v:T)->u64 uses {}=comptime gen(shape(T))\nconst A:u64=comptime width(Point(x:1))\nconst B:u64=comptime A\n",
            ),
        ];
        let roots = vec![path("B"), path("A"), path("B")];
        for (limit, text) in cases {
            let mut control = engine(text);
            control.comptime_limits.call_depth = limit;
            let expected: Vec<_> = roots
                .iter()
                .map(|root| {
                    let result = control.materialize_value(root);
                    let limit = format!("{:?}", control.last_comptime_limit);
                    (
                        outcome_text(&control, result),
                        placed_evidence(&control.sources, limit),
                    )
                })
                .collect();
            for jobs in [1, 2, 8] {
                for bytes in [0, 1, 8192] {
                    let mut candidate = engine(text).with_config(CompileConfig {
                        hold_bytes: bytes,
                        retention: memo::Retention::Resident,
                        ..CompileConfig::for_memory_cap(None)
                    });
                    candidate.comptime_limits.call_depth = limit;
                    for warm in [false, true] {
                        let actual: Vec<_> = candidate
                            .materialize_value_outcomes(
                                roots.clone(),
                                hirsel_intern::jobs::Jobs::new(jobs),
                            )
                            .into_iter()
                            .map(|outcome| {
                                (
                                    outcome_text(&candidate, outcome.result),
                                    placed_evidence(
                                        &candidate.sources,
                                        format!("{:?}", outcome.limit),
                                    ),
                                )
                            })
                            .collect();
                        assert_eq!(
                            actual, expected,
                            "jobs={jobs} budget={bytes} warm={warm} source={text}"
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn frozen_lowering_prefix_withholds_later_generated_and_staged_publication() {
    with_compiler_stack(|| {
        let source = "type Point{x:u64}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{Ok(codegen.body(\"fn(v:standalone@0/Point)->u64 uses {}{12}\"))}\nfn width[T](v:T)->u64 uses {}=comptime gen(shape(T))\nfn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nfn FIRST()->u64{comptime {let zero=0u64;1u64/zero}}\nconst LATER:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)+width(Point(x:1))}\n";
        for jobs in [1, 2, 8] {
            let mut control = engine(source);
            let expected = control.lower_to_tir(&path("FIRST")).unwrap_err();
            let mut candidate = engine(source);
            let outcomes = candidate.check_lower_definition_prefix(
                vec![path("FIRST"), path("LATER")],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert_eq!(outcomes.len(), 1);
            assert_eq!(
                placed_evidence(
                    &candidate.sources,
                    format!("{:?}", outcomes[0].result.as_ref().unwrap_err())
                ),
                placed_evidence(&control.sources, format!("{expected:?}"))
            );
            assert_eq!(
                candidate.generated_sources().count(),
                control.generated_sources().count(),
                "jobs={jobs}: suffix generated source escaped the prefix"
            );
            assert!(
                candidate
                    .memo_reads(&path("LATER"), QueryKind::Summary)
                    .is_none(),
                "jobs={jobs}: later root published a check after the first failure"
            );
            assert!(
                candidate
                    .memo_reads(&path("scaled"), QueryKind::Summary)
                    .is_none(),
                "jobs={jobs}: later staged helper published"
            );
            assert!(
                candidate
                    .memo_reads(&path("gen"), QueryKind::Summary)
                    .is_none(),
                "jobs={jobs}: later generic generator published"
            );
        }
    });
}

#[test]
fn frozen_lowering_prefix_keeps_demanded_closure_and_final_outcomes_across_edits() {
    with_compiler_stack(|| {
        let source = "type Point{x:u64}\nconst SEED:u64=12\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{let _=SEED;Ok(codegen.body(\"fn(v:standalone@0/Point)->u64 uses {}{12}\"))}\nfn width[T](v:T)->u64 uses {}=comptime gen(shape(T))\nfn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nfn FIRST()->u64{comptime LATER}\nconst LATER:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)+width(Point(x:1))}\nfn BAD()->u64{comptime {let zero=0u64;1u64/zero}}\nfn UNUSED()->u64{comptime 99}\n";
        let roots = vec![
            path("FIRST"),
            path("LATER"),
            path("FIRST"),
            path("BAD"),
            path("UNUSED"),
        ];
        for jobs in [1, 2, 8] {
            let mut candidate = engine(source);
            for revision in [
                source.to_owned(),
                source.replace("SEED:u64=12", "SEED:u64=23"),
                source.replace("1u64/zero", "1u64+zero"),
            ] {
                let file = candidate
                    .sources
                    .add("comptime.hir", revision.clone())
                    .unwrap();
                candidate.parse_file(file, &default_context()).unwrap();
                let mut control = engine(&revision);
                let mut expected = vec![];
                let mut depths = vec![];
                for root in &roots {
                    let result = control.lower_to_tir(root);
                    let failed = result.is_err();
                    expected.push(lowering_text(&control, result));
                    depths.push(if failed {
                        None
                    } else {
                        control
                            .lower_definition_outcomes(
                                vec![root.clone()],
                                hirsel_intern::jobs::Jobs::ONE,
                            )
                            .remove(0)
                            .depth
                    });
                    if failed {
                        break;
                    }
                }
                for warm in [false, true] {
                    let actual = candidate.lower_definition_prefix(
                        roots.clone(),
                        hirsel_intern::jobs::Jobs::new(jobs),
                    );
                    assert_eq!(actual.len(), expected.len(), "jobs={jobs} warm={warm}");
                    for (outcome, depth) in actual.iter().zip(&depths) {
                        assert!(!outcome.reads.is_empty());
                        assert_eq!(outcome.phase, Some(QueryKind::Lower));
                        assert!(outcome.limit.is_none());
                        assert_eq!(
                            placed_evidence(&candidate.sources, format!("{:?}", outcome.depth)),
                            placed_evidence(&control.sources, format!("{depth:?}")),
                            "jobs={jobs} warm={warm} root={} revision={revision}",
                            outcome.definition
                        );
                    }
                    let actual: Vec<_> = actual
                        .into_iter()
                        .map(|outcome| lowering_text(&candidate, outcome.result))
                        .collect();
                    assert_eq!(
                        actual, expected,
                        "jobs={jobs} warm={warm} revision={revision}"
                    );
                    assert_eq!(
                        candidate.generated_sources().count(),
                        control.generated_sources().count()
                    );
                    if expected.len() < roots.len() {
                        assert!(
                            candidate
                                .memo_reads(&path("UNUSED"), QueryKind::Summary)
                                .is_none(),
                            "suffix summary must remain unpublished"
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn frozen_lowering_prefix_generator_rejections_keep_serial_selection_and_limits() {
    with_compiler_stack(|| {
        for body in [
            "Error(\"failed\")",
            "let _=A;Error(\"unreachable\")",
            "Ok(codegen.body(\"fn(v:standalone@0/Point)->u32 uses {}{12u32}\"))",
        ] {
            let source = format!(
                "type Point{{x:u64}}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {{}}],String]{{{body}}}\nfn width[T](v:T)->u64 uses {{}}=comptime gen(shape(T))\nconst A:u64=comptime width(Point(x:1))\nconst B:u64=comptime width(Point(x:2))\nconst UNUSED:u64=99\n"
            );
            for jobs in [1, 2, 8] {
                for warm in [false, true] {
                    let mut control = engine(&source);
                    let mut candidate = engine(&source);
                    if warm {
                        assert!(control.lower_to_tir(&path("A")).is_err());
                        assert!(candidate.lower_to_tir(&path("A")).is_err());
                    }
                    let expected = control.lower_to_tir(&path("B"));
                    let actual = candidate.lower_definition_prefix(
                        vec![path("B"), path("A"), path("UNUSED")],
                        hirsel_intern::jobs::Jobs::new(jobs),
                    );
                    assert_eq!(actual.len(), 1);
                    assert_eq!(
                        lowering_text(&candidate, actual[0].result.clone()),
                        lowering_text(&control, expected),
                        "jobs={jobs} warm={warm} source={source}"
                    );
                    assert_eq!(
                        placed_evidence(&candidate.sources, format!("{:?}", actual[0].limit)),
                        placed_evidence(
                            &control.sources,
                            format!("{:?}", control.last_comptime_limit)
                        )
                    );
                    assert!(
                        candidate
                            .memo_reads(&path("UNUSED"), QueryKind::Summary)
                            .is_none()
                    );
                }
            }
        }
        let source = "const LEAF:u64=1\nconst INNER:u64=comptime LEAF\nfn FIRST()->u64{comptime INNER}\nconst UNUSED:u64=99\n";
        for jobs in [1, 2, 8] {
            for limit in [0, 1, 2, 3] {
                let mut control = engine(source);
                let mut candidate = engine(source);
                control.comptime_limits.call_depth = limit;
                candidate.comptime_limits.call_depth = limit;
                let expected = control.lower_to_tir(&path("FIRST"));
                let failed = expected.is_err();
                let actual = candidate.lower_definition_prefix(
                    vec![path("FIRST"), path("UNUSED")],
                    hirsel_intern::jobs::Jobs::new(jobs),
                );
                assert_eq!(actual.len(), if failed { 1 } else { 2 });
                assert_eq!(
                    lowering_text(&candidate, actual[0].result.clone()),
                    lowering_text(&control, expected)
                );
                assert_eq!(
                    placed_evidence(&candidate.sources, format!("{:?}", actual[0].limit)),
                    placed_evidence(
                        &control.sources,
                        format!("{:?}", control.last_comptime_limit)
                    )
                );
            }
        }
    });
}

#[test]
fn frozen_s1_prefix_tir_receipts_and_diagnostics_are_invariant_under_jobs_and_salts() {
    fn run() -> String {
        with_compiler_stack(|| {
            let cases = [
                "const LEAF:u64=7\nfn helper()->u64{comptime LEAF}\nfn A()->u64{comptime helper()}\nconst B:u64=comptime A()\n",
                "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst A:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst B:u64=comptime A\n",
                "type Point{x:u64}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{Ok(codegen.body(\"fn(v:standalone@0/Point)->u64 uses {}{12}\"))}\nfn width[T](v:T)->u64 uses {}=comptime gen(shape(T))\nconst A:u64=comptime width(Point(x:1))\nconst B:u64=comptime A\n",
                "type Seed{x:u64}\nfn gen(s:TypeShape[Seed])->Result[Body[fn()->u64 uses {}],String]{let _=A;Error(\"unreachable\")}\nfn chosen()->u64 uses {}=comptime gen(shape(Seed))\nconst A:u64=comptime chosen()\nconst B:u64=comptime A\n",
            ];
            let mut transcript = String::new();
            for source in cases {
                let mut expected = None;
                for jobs in [1, 2, 8] {
                    for budget in [0, 1, 8192, 1 << 20] {
                        let roots = vec![path("B"), path("A"), path("B")];
                        let mut scalar = engine(source);
                        let mut scalar_values = Vec::new();
                        for root in &roots {
                            let value = scalar.lower_to_tir(root);
                            let failed = value.is_err();
                            scalar_values.push(lowering_text(&scalar, value));
                            if failed {
                                break;
                            }
                        }
                        let mut candidate = engine(source).with_config(CompileConfig {
                            hold_bytes: budget,
                            retention: memo::Retention::Resident,
                            ..CompileConfig::for_memory_cap(None)
                        });
                        for warm in [false, true] {
                            let outcomes = candidate.lower_definition_prefix(
                                roots.clone(),
                                hirsel_intern::jobs::Jobs::new(jobs),
                            );
                            let mut current = String::new();
                            assert_eq!(outcomes.len(), scalar_values.len());
                            for (outcome, expected_value) in
                                outcomes.into_iter().zip(&scalar_values)
                            {
                                let value = lowering_text(&candidate, outcome.result);
                                assert_eq!(
                                    &value, expected_value,
                                    "scalar cold jobs={jobs} budget={budget} warm={warm}"
                                );
                                current.push_str(&format!("{value:?}\n{:?}\n", outcome.phase));
                                current.push_str(&placed_evidence(
                                    &candidate.sources,
                                    format!(
                                        "{:?}\n{:?}\n{:?}\n",
                                        outcome.limit, outcome.depth, outcome.reads
                                    ),
                                ));
                            }
                            if let Some(expected) = &expected {
                                assert_eq!(
                                    &current, expected,
                                    "jobs={jobs} budget={budget} warm={warm} source={source}"
                                );
                            } else {
                                expected = Some(current);
                            }
                        }
                    }
                }
                transcript.push_str(&expected.unwrap());
            }
            transcript
        })
    }
    let Some(outputs) = hirsel_intern::under_salts(
        "comptime::frozen_s1_prefix_tir_receipts_and_diagnostics_are_invariant_under_jobs_and_salts",
        &[1, 2, 3],
        run,
    ) else {
        return;
    };
    assert!(outputs[0].contains("CT-4"));
    assert!(outputs[0].contains("DefinitionSection") || outputs[0].contains("Definition"));
    assert!(outputs[0].contains("ValidationUniverse"));
    assert!(outputs.iter().all(|output| output == &outputs[0]));
}

#[test]
fn compact_lowering_prefix_preserves_all_output_facts_and_barriers() {
    fn facts<T>(engine: &QueryEngine, outcome: LoweringResult<T>) -> String {
        let output = outcome
            .result
            .map(|output| (output.diagnostics, output.dependencies));
        placed_evidence(
            &engine.sources,
            format!(
                "{:?} {output:?} {:?} {:?} {:?} {:?}",
                outcome.definition, outcome.phase, outcome.limit, outcome.depth, outcome.reads
            ),
        )
    }
    with_compiler_stack(|| {
        let cases = [
            "const LEAF:u64=7\nfn A()->u64{comptime LEAF}\nconst B:u64=comptime A()\n",
            "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst A:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst B:u64=comptime A\n",
            "type Point{x:u64}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{Ok(codegen.body(\"fn(v:standalone@0/Point)->u64 uses {}{12}\"))}\nfn width[T](v:T)->u64 uses {}=comptime gen(shape(T))\nconst A:u64=comptime width(Point(x:1))\nconst B:u64=comptime A\n",
            "const A:u64=comptime B\nconst B:u64=comptime A\n",
        ];
        for source in cases {
            for jobs in [1, 2, 8] {
                let mut control = engine(source);
                let mut candidate = engine(source);
                for revision in [
                    source.to_owned(),
                    source.replace("7", "23"),
                    source.to_owned(),
                ] {
                    for engine in [&mut control, &mut candidate] {
                        let file = engine
                            .sources
                            .add("comptime.hir", revision.clone())
                            .unwrap();
                        engine.parse_file(file, &default_context()).unwrap();
                    }
                    for warm in [false, true] {
                        let roots = vec![path("B"), path("A"), path("B")];
                        let expected = control
                            .lower_definition_prefix(
                                roots.clone(),
                                hirsel_intern::jobs::Jobs::new(jobs),
                            )
                            .into_iter()
                            .map(|outcome| facts(&control, outcome))
                            .collect::<Vec<_>>();
                        let actual = candidate
                            .check_lower_definition_prefix(
                                roots,
                                hirsel_intern::jobs::Jobs::new(jobs),
                            )
                            .into_iter()
                            .map(|outcome| facts(&candidate, outcome))
                            .collect::<Vec<_>>();
                        assert_eq!(
                            actual, expected,
                            "jobs={jobs} warm={warm} source={revision}"
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn frozen_s1_prefix_resident_and_second_process_store_equal_cold_after_edits() {
    use std::{path::PathBuf, sync::Arc};
    fn run(
        engine: &mut QueryEngine,
        source: &str,
        jobs: usize,
        limit: usize,
        verify: Verify,
        manifest: u8,
        require_restore: bool,
    ) -> String {
        engine.verify = verify;
        engine.comptime_limits.call_depth = limit;
        engine.environment.package_manifest_hash =
            hirsel_syntax::ContentHash::of_bytes(&[manifest]);
        let file = engine.sources.add("comptime.hir", source.into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        // This ordinary helper is a real persisted P hit before the S1 demand.
        let _ = engine.mode_definition(&path("plain"));
        if require_restore {
            assert_eq!(
                engine.recompute_count(&path("plain"), QueryKind::Check),
                0,
                "child must restore the persisted helper before S1 expansion"
            );
        }
        let outcomes = engine.lower_definition_prefix(
            vec![path("A"), path("B"), path("A")],
            hirsel_intern::jobs::Jobs::new(jobs),
        );
        let mut transcript = String::new();
        for outcome in outcomes {
            transcript.push_str(&format!(
                "{:?}\n{:?}\n",
                lowering_text(engine, outcome.result),
                outcome.phase
            ));
            transcript.push_str(&placed_evidence(
                &engine.sources,
                format!(
                    "{:?}\n{:?}\n{:?}\n",
                    outcome.limit, outcome.depth, outcome.reads
                ),
            ));
        }
        transcript
    }
    if let Some(root) = std::env::var_os("HIRSEL_S1_O2_ROOT") {
        let root = PathBuf::from(root);
        let jobs: usize = std::env::var("HIRSEL_S1_O2_JOBS").unwrap().parse().unwrap();
        let source = std::fs::read_to_string(root.join("source.hir")).unwrap();
        let settings: Vec<usize> = std::fs::read_to_string(root.join("settings"))
            .unwrap()
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect();
        let output = with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_config(CompileConfig {
                    store: Arc::new(store::DirStore::new(root.join("store"))),
                    retention: memo::Retention::Resident,
                    hold_bytes: 0,
                    ..CompileConfig::for_memory_cap(None)
                });
            let output = run(
                &mut engine,
                &source,
                jobs,
                settings[0],
                if settings[1] == 0 {
                    Verify::Off
                } else {
                    Verify::On
                },
                settings[2] as u8,
                settings[3] == 0,
            );
            output
        });
        std::fs::write(root.join(format!("output-{jobs}")), output).unwrap();
        return;
    }
    with_compiler_stack(|| {
        let fixture = crate::image_fixture::Fixture::new();
        let root = &fixture.0;
        let base = "pub opaque type Hidden:plain uses {}{value:u64}\nconst LEAF:u64=7\nfn plain()->u64{LEAF}\nfn private(v:Hidden)->u64{v.value}\nfn body_text()->String{\"fn(v:standalone@0/Hidden)->u64 uses {}{12}\"}\nfn gen[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses {}],String]{Ok(codegen.body(body_text()))}\nfn width[T](v:T)->u64 uses {}=comptime gen(shape(T))\nfn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst A:u64=comptime plain()+private(Hidden(value:1))\nconst B:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)+width(Hidden(value:1))}\n";
        let edits = [
            (base.to_owned(), 8, Verify::On, 0),
            (base.replace("LEAF:u64=7", "LEAF:u64=23"), 8, Verify::On, 0),
            (base.replace("uses {}{12}", "uses {}{19}"), 8, Verify::On, 0),
            (
                base.replace("fn plain()->u64", "fn plain()->u32"),
                8,
                Verify::On,
                0,
            ),
            (base.replace("value:u64", "value:u32"), 8, Verify::On, 0),
            (format!("fn earlier_item(){{}}\n{base}"), 8, Verify::On, 0),
            (base.replace("{LEAF}", "{absent}"), 8, Verify::On, 0),
            (
                format!("{base}\nfn plain()->u64{{LEAF}}\n"),
                8,
                Verify::On,
                0,
            ),
            (base.to_owned(), 2, Verify::On, 0),
            (base.to_owned(), 8, Verify::Off, 0),
            (base.to_owned(), 8, Verify::On, 0),
            (base.to_owned(), 8, Verify::On, 1),
            (base.to_owned(), 8, Verify::On, 0),
        ];
        let mut resident = QueryEngine::new(default_environment(), compiler_adapters())
            .with_config(CompileConfig {
                store: Arc::new(store::DirStore::new(root.join("store"))),
                retention: memo::Retention::Resident,
                hold_bytes: 0,
                ..CompileConfig::for_memory_cap(None)
            });
        for (revision, (source, limit, verify, manifest)) in edits.into_iter().enumerate() {
            let mut cold = QueryEngine::new(default_environment(), compiler_adapters())
                .with_config(CompileConfig {
                    store: Arc::new(store::NoneStore),
                    retention: memo::Retention::Resident,
                    hold_bytes: 0,
                    ..CompileConfig::for_memory_cap(None)
                });
            let expected = run(&mut cold, &source, 1, limit, verify, manifest, false);
            if revision == 0 {
                assert_eq!(
                    expected
                        .lines()
                        .filter(|line| line.starts_with("Ok("))
                        .count(),
                    3,
                    "base fixture must accept all three requested roots: {expected}"
                );
            }
            let actual = run(&mut resident, &source, 8, limit, verify, manifest, false);
            assert_eq!(actual, expected, "resident revision={revision}");
            std::fs::write(root.join("source.hir"), &source).unwrap();
            std::fs::write(
                root.join("settings"),
                format!(
                    "{limit} {} {manifest} {revision}",
                    if verify == Verify::Off { 0 } else { 1 }
                ),
            )
            .unwrap();
            for jobs in [1, 2, 8] {
                let child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["comptime::frozen_s1_prefix_resident_and_second_process_store_equal_cold_after_edits", "--exact", "--nocapture", "--test-threads=1"])
                    .env("HIRSEL_S1_O2_ROOT", root).env("HIRSEL_S1_O2_JOBS", jobs.to_string()).output().unwrap();
                assert!(
                    child.status.success(),
                    "revision={revision} jobs={jobs}\n{}\n{}",
                    String::from_utf8_lossy(&child.stdout),
                    String::from_utf8_lossy(&child.stderr)
                );
                let actual = std::fs::read_to_string(root.join(format!("output-{jobs}"))).unwrap();
                assert_eq!(
                    actual, expected,
                    "second-process revision={revision} jobs={jobs}"
                );
            }
        }
        assert!(
            std::fs::read_dir(root.join("store"))
                .unwrap()
                .next()
                .is_some(),
            "the oracle must exercise durable products"
        );
    });
}

#[test]
fn batch_staged_exports_keep_cached_instances_after_unrelated_edits() {
    use std::sync::Arc;
    fn make(jobs: usize) -> QueryEngine {
        QueryEngine::new(default_environment(), compiler_adapters()).with_config(CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(jobs),
            hold_bytes: 0,
            retention: memo::Retention::Batch,
            store: Arc::new(store::NoneStore),
            ..CompileConfig::for_memory_cap(None)
        })
    }
    fn check(engine: &mut QueryEngine, source: &str) {
        let file = engine
            .sources
            .add("staged-export.hir", source.into())
            .unwrap();
        let checked = check_progress(engine, file, &default_context()).unwrap();
        assert_eq!(checked.completion, CheckCompletion::Complete);
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }
    with_compiler_stack(|| {
        let source = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst VALUE:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst GOOD:u64=comptime 7\n";
        for jobs in [1, 2, 8] {
            let mut warm = make(jobs);
            check(&mut warm, source);
            for revision in [
                source.replace("comptime 7", "comptime 9"),
                source.replace("n*n", "n+n+1"),
                format!("fn earlier_item(){{}}\n{source}"),
                source.to_owned(),
            ] {
                check(&mut warm, &revision);
                let mut cold = make(jobs);
                check(&mut cold, &revision);
                let expected = cold.export_tir(&path("VALUE")).unwrap();
                let actual = warm.export_tir(&path("VALUE")).unwrap();
                assert!(expected.contains("$constant:$comptime:scaled"));
                assert_eq!(
                    warm.materialize_constant(&path("VALUE")).unwrap(),
                    Constant::Integer(if revision.contains("n+n+1") { 5 } else { 4 })
                );
                assert!(
                    actual == expected,
                    "jobs={jobs}: full export mismatch after {revision}; first differing lines {:?}",
                    actual.lines().zip(expected.lines()).find(|(a, b)| a != b)
                );
            }
        }
    });
}

#[test]
fn frozen_lowering_prefix_publishes_exact_staged_instances_through_cache_hits() {
    fn selected_constants(engine: &mut QueryEngine) -> Vec<(DefPath, String)> {
        let input = engine
            .native_input_for(&[path("FIRST"), path("scaled")])
            .unwrap();
        input
            .snapshot()
            .constants
            .iter()
            .filter(|(path, _)| path.0.package == default_context().package)
            .map(|(path, value)| (path.clone(), materialized_text(engine, value)))
            .collect()
    }
    with_compiler_stack(|| {
        let source = "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst HELPER:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nconst FIRST:u64=comptime HELPER\nfn BAD()->u64{comptime {let zero=0u64;1u64/zero}}\nfn LATER()->u64{comptime {let xs:Inline[u8,3]=[1,2,3];scaled(xs)}}\n";
        for jobs in [1, 2, 8] {
            for bytes in [0, 8192] {
                for compact in [false, true] {
                    let mut control = engine(source);
                    control.lower_to_tir(&path("FIRST")).unwrap();
                    control.lower_to_tir(&path("BAD")).unwrap_err();
                    let expected = selected_constants(&mut control);
                    assert!(
                        expected.iter().any(|(path, _)| path
                            .0
                            .name
                            .contains("$constant:$comptime:scaled:2:2"))
                    );
                    assert!(
                        !expected.iter().any(|(path, _)| path
                            .0
                            .name
                            .contains("$constant:$comptime:scaled:2:3"))
                    );
                    let mut candidate = engine(source).with_config(CompileConfig {
                        hold_bytes: bytes,
                        ..CompileConfig::for_memory_cap(None)
                    });
                    for warm in [false, true] {
                        let roots = vec![path("FIRST"), path("FIRST"), path("BAD"), path("LATER")];
                        let outcomes: Vec<_> = if compact {
                            candidate
                                .check_lower_definition_prefix(
                                    roots,
                                    hirsel_intern::jobs::Jobs::new(jobs),
                                )
                                .into_iter()
                                .map(|outcome| outcome.result.map(|_| ()))
                                .collect()
                        } else {
                            candidate
                                .lower_definition_prefix(
                                    roots,
                                    hirsel_intern::jobs::Jobs::new(jobs),
                                )
                                .into_iter()
                                .map(|outcome| outcome.result.map(|_| ()))
                                .collect()
                        };
                        assert_eq!(outcomes.len(), 3);
                        assert!(outcomes[0].is_ok() && outcomes[1].is_ok() && outcomes[2].is_err());
                        let actual = selected_constants(&mut candidate);
                        assert_eq!(
                            actual, expected,
                            "jobs={jobs} bytes={bytes} compact={compact} warm={warm}: staged values escaped or were dropped"
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn frozen_lowering_physical_work_counts_helpers_retries_and_discarded_suffixes() {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    struct CountLower {
        inner: Arc<dyn LoweringPhase>,
        calls: Mutex<BTreeMap<DefPath, u64>>,
    }
    impl LoweringPhase for CountLower {
        fn lower(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            comptime: &Comptime,
            signature: &hirsel_types::Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
            *self
                .calls
                .lock()
                .unwrap()
                .entry(body.typed.syntax.item.definition.clone())
                .or_default() += 1;
            self.inner.lower(t, m, body, comptime, signature)
        }
        fn lower_evaluation(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
            signature: &hirsel_types::Signature,
            expression: hirsel_syntax::ExprId,
            result: hirsel_types::TypeId,
            constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
        ) -> Result<hirsel_tir::Definition, QueryError> {
            self.inner
                .lower_evaluation(t, m, body, staged, signature, expression, result, constants)
        }
        fn entry_body(&self) -> hirsel_tir::BodyId {
            self.inner.entry_body()
        }
    }
    with_compiler_stack(|| {
        let sources = [
            "const BASE:u64=7\nfn helper()->u64{BASE}\nfn FIRST()->u64{comptime {helper()+helper()}}\nfn BAD()->u64{comptime {let zero=0u64;1u64/zero}}\nfn LATER()->u64{11u64}\n",
            "fn scaled[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime n*n}\nconst FIRST:u64=comptime {let xs:Inline[u8,2]=[1,2];scaled(xs)}\nfn BAD()->u64{comptime {let zero=0u64;1u64/zero}}\nfn LATER()->u64{11u64}\n",
        ];
        for source in sources {
            for jobs in [1, 2, 8] {
                for budget in [0, 1 << 20] {
                    let mut candidate = engine(source).with_config(CompileConfig {
                        hold_bytes: budget,
                        ..CompileConfig::for_memory_cap(None)
                    });
                    let counter = Arc::new(CountLower {
                        inner: candidate.adapters.lowering.take().unwrap(),
                        calls: Mutex::new(BTreeMap::new()),
                    });
                    candidate.adapters.lowering = Some(counter.clone());
                    for warm in [false, true] {
                        let outcomes = candidate.check_lower_definition_prefix(
                            vec![path("FIRST"), path("BAD"), path("LATER")],
                            hirsel_intern::jobs::Jobs::new(jobs),
                        );
                        assert_eq!(outcomes.len(), 2);
                        assert!(outcomes[0].result.is_ok() && outcomes[1].result.is_err());
                        for (path, calls) in counter.calls.lock().unwrap().iter() {
                            assert_eq!(
                                candidate.s1_lower_work(path).adapter_calls,
                                *calls,
                                "jobs={jobs} budget={budget} warm={warm} {path}: count actual adapter work before publication filtering"
                            );
                        }
                        assert!(candidate.s1_lower_work(&path("BAD")).query_attempts > 0);
                        assert_eq!(
                            candidate.s1_lower_work(&path("BAD")).adapter_calls,
                            0,
                            "failed preparation is a query attempt without a Lower adapter call"
                        );
                        assert!(
                            candidate.s1_lower_work(&path("LATER")).adapter_calls > 0,
                            "discarded suffix work must still be counted"
                        );
                        if source.contains("fn scaled") {
                            assert!(
                                candidate.s1_lower_work(&path("FIRST")).query_attempts > 1,
                                "the staged registration fixture must actually retry"
                            );
                        } else {
                            assert!(
                                candidate.s1_lower_work(&path("helper")).adapter_calls > 0,
                                "helper work must be included"
                            );
                        }
                    }
                }
            }
        }
    });
}

#[test]
fn frozen_lowering_workers_share_only_unoccupied_scalar_hold_bytes() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct CountHelper {
        inner: Arc<dyn LoweringPhase>,
        calls: AtomicUsize,
        workers: bool,
    }
    impl LoweringPhase for CountHelper {
        fn lower(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            comptime: &Comptime,
            signature: &hirsel_types::Signature,
        ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
            if self.workers {
                assert_eq!(std::thread::current().name(), Some("hirsel-job"));
            }
            if body.typed.syntax.item.definition.0.name.as_str() == "helper" {
                self.calls.fetch_add(1, Ordering::Relaxed);
            }
            self.inner.lower(t, m, body, comptime, signature)
        }
        fn lower_evaluation(
            &self,
            t: &hirsel_types::TypeInterner,
            m: &hirsel_types::TypeMemo,
            body: &AnalyzedBody,
            staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
            signature: &hirsel_types::Signature,
            expression: hirsel_syntax::ExprId,
            result: hirsel_types::TypeId,
            constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
        ) -> Result<hirsel_tir::Definition, QueryError> {
            if self.workers {
                assert_eq!(std::thread::current().name(), Some("hirsel-job"));
            }
            self.inner
                .lower_evaluation(t, m, body, staged, signature, expression, result, constants)
        }
        fn entry_body(&self) -> hirsel_tir::BodyId {
            self.inner.entry_body()
        }
    }
    with_compiler_stack(|| {
        let source = format!(
            "fn filler()->u64{{{}7u64}}\nfn helper()->u64{{7u64}}\nfn A()->u64{{let a=comptime helper();let b=comptime helper();a+b}}\nfn B()->u64{{let a=comptime helper();let b=comptime helper();a+b}}\n",
            (0..64)
                .map(|i| format!("let _={i}u64; "))
                .collect::<String>()
        );
        for jobs in [1, 2, 8] {
            for occupied in [false, true] {
                let config = |hold_bytes| CompileConfig {
                    hold_bytes,
                    retention: memo::Retention::Batch,
                    ..CompileConfig::for_memory_cap(None)
                };
                let mut candidate = engine(&source).with_config(config(u64::MAX));
                drop(candidate.lower_to_tir(&path("filler")).unwrap());
                let bytes = candidate.lower_hold_stats().bytes;
                assert!(bytes > 0);
                candidate =
                    candidate.with_config(config(if occupied { bytes } else { bytes + (1 << 20) }));
                assert_eq!(
                    candidate.available_hold_bytes(),
                    if occupied { 0 } else { 1 << 20 }
                );
                let counter = Arc::new(CountHelper {
                    inner: candidate.adapters.lowering.take().unwrap(),
                    calls: AtomicUsize::new(0),
                    workers: jobs > 1,
                });
                candidate.adapters.lowering = Some(counter.clone());
                let outcomes = candidate.check_lower_definition_prefix(
                    vec![path("A"), path("B")],
                    hirsel_intern::jobs::Jobs::new(jobs),
                );
                assert_eq!(outcomes.len(), 2);
                assert!(
                    outcomes.iter().all(|outcome| outcome.result.is_ok()),
                    "{outcomes:?}"
                );
                assert_eq!(
                    counter.calls.load(Ordering::Relaxed),
                    if occupied { 4 } else { 2 },
                    "jobs={jobs} occupied={occupied}: only free bytes can retain a helper within a root"
                );
                assert!(candidate.lower_hold_stats().bytes <= candidate.hold_bytes());
            }
        }
    });
}
