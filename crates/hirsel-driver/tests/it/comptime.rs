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

#[test]
fn materialized_constant_publishes_and_validates() {
    hirsel_engine::with_compiler_stack(
        materialized_constant_publishes_and_validates_on_compiler_thread,
    );
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
fn evaluation_step_diagnostics_retry_after_budget_changes() {
    hirsel_engine::with_compiler_stack(
        evaluation_step_diagnostics_retry_after_budget_changes_on_compiler_thread,
    );
}
fn evaluation_step_diagnostics_retry_after_budget_changes_on_compiler_thread() {
    let mut engine = engine("const VALUE: u32 = comptime (2 + 3)\n");
    engine.comptime_limits.steps = Some(1);
    let Err(QueryError::Diagnostics(ds)) = engine.lower_to_tir(&path("VALUE")) else {
        panic!("expected step exhaustion")
    };
    assert_eq!(ds[0].rule.as_str(), "CT-1");
    assert_eq!(
        engine.last_comptime_limit.as_ref().unwrap().kind,
        hirsel_tir::ToolingLimitKind::Steps
    );
    engine.comptime_limits.steps = Some(100);
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

#[test]
fn comptime_and_runtime_use_the_same_scalar_semantics() {
    hirsel_engine::with_compiler_stack(
        comptime_and_runtime_use_the_same_scalar_semantics_on_compiler_thread,
    );
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
fn constant_reads_do_not_reconstruct_the_materialized_value_in_the_reading_frame() {
    with_compiler_stack(|| {
        for (ty, initializer) in [
            ("u32", "7"),
            ("String in heap", "\"saved\""),
            ("(u32, String)", "(7, \"saved\")"),
        ] {
            let mut engine = engine(&format!(
                "const DATA: {ty} = {initializer}\nfn read() -> {ty} {{ DATA }}\n"
            ));
            let definition = engine.lower_to_tir(&path("read")).unwrap().value;
            assert_eq!(
                definition.bodies[0]
                    .rvalues
                    .iter()
                    .filter(|r| matches!(r, hirsel_tir::Rvalue::ReadConstant { .. }))
                    .count(),
                1
            );
            assert!(
                definition.bodies[0].rvalues.iter().all(|r| matches!(
                    r,
                    hirsel_tir::Rvalue::ReadConstant { .. } | hirsel_tir::Rvalue::Use(_)
                )),
                "{:?}",
                definition.bodies[0].rvalues
            );
        }
    });
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
        engine.comptime_limits.steps = Some(1);
        let Err(QueryError::Diagnostics(ds)) = engine.constant_instance(&constant, &[2]) else {
            panic!("expected CT-1 after budget change")
        };
        assert_eq!(ds[0].rule.as_str(), "CT-1");
        engine.comptime_limits.steps = Some(1000);
        assert_eq!(
            engine.constant_instance(&constant, &[2]).unwrap().root.kind,
            ValueKind::Constant(Constant::Integer(8))
        );
    });
}

#[test]
fn a_failed_size_instantiation_retains_its_selecting_site() {
    with_compiler_stack(|| {
        let source = include_str!("../../../../conformance/CT/CT-1-size-instantiation-bad.hir");
        let mut engine = engine(source);
        let file = engine.sources.add("comptime.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let diagnostic = output
            .diagnostics
            .iter()
            .find(|d| d.rule.as_str() == "CT-1")
            .unwrap();
        assert!(
            engine
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
                    |label| engine.sources.slice(label.span) == Some("inverse(second)")
                        && label.message.contains("[0]")
                ),
            "{diagnostic:?}"
        );
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
