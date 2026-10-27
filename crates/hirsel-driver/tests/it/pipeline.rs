use crate::source_std;
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_intern::Symbol;
fn path(name: &str) -> DefPath {
    let mut p = entry_path(&default_context());
    p.0.name = name.into();
    p
}
fn edit(engine: &mut QueryEngine, source: &str) {
    if source.contains("std@1/") {
        *engine = source_std::engine();
    }
    let file = engine.sources.add("program.hir", source.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
}
fn count(engine: &QueryEngine, name: &str) -> Vec<usize> {
    [QueryKind::Check, QueryKind::Modes, QueryKind::Lower]
        .into_iter()
        .map(|k| engine.recompute_count(&path(name), k))
        .collect()
}
fn check_all(engine: &mut QueryEngine) {
    for name in ["a", "b", "untouched"] {
        engine.lower_to_tir(&path(name)).unwrap();
    }
}
// Syntax IDs are snapshot-local; compare sites relative to the retained owner.
// TypeId's Debug omits ephemeral provenance, while complete row content is compared.
fn normalize_sites(typed: &TypedBody, mut view: String) -> String {
    let tree = &typed.syntax.tree;
    let base = typed.syntax.item.origin.span.start;
    for (prefix, sites) in [
        (
            "ExprId(",
            tree.expressions
                .iter()
                .map(|e| e.origin.span)
                .collect::<Vec<_>>(),
        ),
        (
            "StmtId(",
            tree.statements.iter().map(|e| e.origin.span).collect(),
        ),
        (
            "PatternId(",
            tree.patterns.iter().map(|e| e.origin.span).collect(),
        ),
        (
            "TypeExprId(",
            tree.types.iter().map(|e| e.origin.span).collect(),
        ),
    ] {
        let mut normalized = String::new();
        let mut remaining = view.as_str();
        while let Some((start, _)) = remaining
            .match_indices(prefix)
            .find(|(i, _)| *i == 0 || !remaining.as_bytes()[i - 1].is_ascii_alphabetic())
        {
            normalized.push_str(&remaining[..start]);
            remaining = &remaining[start + prefix.len()..];
            let end = remaining.find(')').unwrap();
            let id: usize = remaining[..end].parse().unwrap();
            let span = sites[id];
            normalized.push_str(&format!(
                "{prefix}{}:{})",
                span.start as i64 - base as i64,
                span.end as i64 - base as i64
            ));
            remaining = &remaining[end + 1..];
        }
        normalized.push_str(remaining);
        view = normalized;
    }
    view
}
fn artifact_view(typed: &TypedBody) -> String {
    normalize_sites(
        typed,
        format!(
            "{:?}",
            (
                &typed.checked.expressions,
                &typed.checked.bindings,
                &typed.checked.patterns,
                &typed.checked.closures,
                &typed.checked.instantiations,
                &typed.checked.calls,
                &typed.checked.places,
                &typed.plans,
                typed.unit,
                typed.boolean,
            )
        ),
    )
}
fn equal_artifacts(engine: &mut QueryEngine, fresh: &mut QueryEngine, p: &DefPath) {
    let a = engine.check_definition(p).unwrap().value;
    let b = fresh.check_definition(p).unwrap().value;
    assert_eq!(artifact_view(&a), artifact_view(&b));
    let a = engine.mode_definition(p).unwrap().value;
    let b = fresh.mode_definition(p).unwrap().value;
    assert_eq!(artifact_view(&a.typed), artifact_view(&b.typed));
    // The fact sets are hashed: compare them in a stable order.
    let facts = |v: &AnalyzedBody| {
        let sorted = |set: &std::collections::HashSet<hirsel_types::TypeId>| {
            let mut ids: Vec<_> = set.iter().map(|id| format!("{id:?}")).collect();
            ids.sort();
            ids
        };
        let rest = hirsel_types::OwnershipFacts {
            plain_types: Default::default(),
            silent_drop_types: Default::default(),
            ..v.ownership.clone()
        };
        format!(
            "{:?}",
            (
                sorted(&v.ownership.plain_types),
                sorted(&v.ownership.silent_drop_types),
                rest,
                &v.results,
                &v.call_allocations
            )
        )
    };
    assert_eq!(
        normalize_sites(&a.typed, facts(&a)),
        normalize_sites(&b.typed, facts(&b))
    );
}
fn equal_fresh(engine: &mut QueryEngine, source: &str) {
    // One interner, so equal artifacts carry equal type ids.
    let mut fresh = QueryEngine::new(default_environment(), compiler_adapters())
        .with_types(engine.types().clone());
    edit(&mut fresh, source);
    for name in ["a", "b", "untouched"] {
        let p = path(name);
        equal_artifacts(engine, &mut fresh, &p);
        let a = engine.lower_to_tir(&p).unwrap();
        let b = fresh.lower_to_tir(&p).unwrap();
        assert_eq!(a.diagnostics, b.diagnostics);
        assert_eq!(a.value.signature_hash, b.value.signature_hash);
        assert_eq!(a.value.bodies.len(), b.value.bodies.len());
        let normalize = |engine: &mut QueryEngine, d: &hirsel_tir::Definition| {
            // Spans are item-relative; only the process-unique anchor differs.
            let anchor = engine.parse_definition(&p).unwrap().value.anchor;
            let mut d = d.clone();
            for body in &mut d.bodies {
                for block in &mut body.blocks {
                    assert_eq!(block.span.anchor, anchor);
                    block.span.anchor = hirsel_span::Anchor(0);
                    for i in &mut block.instructions {
                        assert_eq!(i.span.anchor, anchor);
                        i.span.anchor = hirsel_span::Anchor(0);
                    }
                }
            }
            hirsel_lower::print::definition(&d)
        };
        assert_eq!(normalize(engine, &a.value), normalize(&mut fresh, &b.value));
        assert_eq!(a.value.types.len(), b.value.types.len());
        for (&id, &other) in a.value.types.iter().zip(b.value.types.iter()) {
            assert!(engine.types().semantic_eq(id, other));
        }
    }
}
#[test]
fn real_phases_edit_body_signature_failed_read_and_recovery() {
    hirsel_engine::with_compiler_stack(
        real_phases_edit_body_signature_failed_read_and_recovery_on_compiler_thread,
    );
}
fn real_phases_edit_body_signature_failed_read_and_recovery_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let initial = "@noalloc fn a() {}\nfn b() { a(); }\nfn untouched() {}\n";
    edit(&mut engine, initial);
    check_all(&mut engine);
    let before_a = count(&engine, "a");
    let before_b = count(&engine, "b");
    let before_other = count(&engine, "untouched");
    let shifted = "@noalloc fn a() { (); (); }\nfn b() { a(); }\nfn untouched() {}\n";
    edit(&mut engine, shifted);
    check_all(&mut engine);
    assert_eq!(
        count(&engine, "a"),
        before_a.iter().map(|n| n + 1).collect::<Vec<_>>()
    );
    assert_eq!(count(&engine, "b"), before_b);
    assert_eq!(count(&engine, "untouched"), before_other);
    equal_fresh(&mut engine, shifted);
    let signature = "fn a() { (); (); }\nfn b() { a(); }\nfn untouched() {}\n";
    edit(&mut engine, signature);
    check_all(&mut engine);
    assert_eq!(
        count(&engine, "b"),
        before_b.iter().map(|n| n + 1).collect::<Vec<_>>()
    );
    assert_eq!(count(&engine, "untouched"), before_other);
    equal_fresh(&mut engine, signature);
    edit(&mut engine, "fn b() { a(); }\nfn untouched() {}\n");
    assert!(matches!(
        engine.lower_to_tir(&path("b")),
        Err(QueryError::Diagnostics(_))
    ));
    let reads = engine
        .dependency_reads(&path("b"), QueryKind::Resolve)
        .unwrap();
    assert!(reads.manifest_lookups.iter().any(|r| matches!((r).as_ref(),hirsel_types::DependencyRead::ProviderLookup { output, .. } if output.result.is_err())));
    edit(&mut engine, signature);
    check_all(&mut engine);
    equal_fresh(&mut engine, signature);
}
#[test]
fn registry_fallback_records_reads_and_respects_program_definitions() {
    hirsel_engine::with_compiler_stack(
        registry_fallback_records_reads_and_respects_program_definitions_on_compiler_thread,
    );
}
fn registry_fallback_records_reads_and_respects_program_definitions_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    assert_eq!(
        engine.environment.builtin_signatures_hash,
        registry::BuiltinRegistry::new().revision()
    );
    edit(&mut engine, "fn inspect(rt: Io) { io.stdout(rt); }\n");
    engine.check_definition(&path("inspect")).unwrap();
    assert!(
        engine
            .dependency_reads(&path("inspect"), QueryKind::Check)
            .unwrap()
            .signatures
            .iter()
            .any(|d| *d.definition.path() == hirsel_syntax::DefPath::core("io.stdout"))
    );
    edit(
        &mut engine,
        "type Io { value: u32 }\nfn inspect(rt: Io) {}\n",
    );
    let signature = engine.item_signature(&path("inspect")).unwrap().value;
    let hirsel_types::SignatureKind::Function(f) = &signature.kind else {
        panic!()
    };
    assert!(
        matches!(&engine.types().get(f.parameters[0].ty).unwrap().kind,hirsel_types::TypeKind::Named { definition, .. } if *definition == hirsel_syntax::DefId::intern(&path("Io")))
    );
    let hidden = hirsel_syntax::Path {
        package: None,
        segments: vec![
            Symbol::intern("$intrinsic"),
            Symbol::intern("unicode_scalars_open"),
        ],
    };
    assert!(engine.lookup(&default_context(), &hidden).result.is_err());
}

#[test]
fn calls_borrow_owned_string_computations_without_transferring_them() {
    hirsel_engine::with_compiler_stack(
        calls_borrow_owned_string_computations_without_transferring_them_on_compiler_thread,
    );
}
fn calls_borrow_owned_string_computations_without_transferring_them_on_compiler_thread() {
    for argument in ["\"hello\"", "copy text", "take text"] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(
            &mut engine,
            &format!(
                "fn emit(out:Stdout, take text:String) -> Result[(),OutputError] {{ out.print(out,{argument}) }}"
            ),
        );
        let result = engine.lower_to_tir(&path("emit"));
        assert!(result.is_ok(), "{argument}: {result:?}");
        let definition = result.unwrap().value;
        let body = &definition.bodies[definition.entry.0 as usize];
        let argument = body
            .blocks
            .iter()
            .find_map(|i| {
                if let hirsel_tir::Terminator::Call { arguments, .. } = &i.terminator {
                    arguments
                        .iter()
                        .find(|a| a.parameter.0 == 1)
                        .map(|a| &a.value)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(argument.usage, hirsel_types::UseKind::Borrow);
        assert!(matches!(
            body.locals[body.places[argument.place.0 as usize].root.0 as usize].storage,
            hirsel_tir::Storage::Temporary
        ));
    }
}

#[test]
fn inferred_closures_satisfy_unrestricted_callable_authority_promises() {
    hirsel_engine::with_compiler_stack(
        inferred_closures_satisfy_unrestricted_callable_authority_promises_on_compiler_thread,
    );
}
fn inferred_closures_satisfy_unrestricted_callable_authority_promises_on_compiler_thread() {
    for (name, source) in [
        ("make", "fn make() -> fn() -> u32 { fn() { 1 } }"),
        (
            "run",
            "fn apply(f:fn()->u32)->u32 { f() }\nfn run()->u32 { apply(fn(){1}) }",
        ),
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, source);
        let result = engine.lower_to_tir(&path(name));
        assert!(result.is_ok(), "{name}: {result:?}");
    }
}

#[test]
fn written_function_binding_uses_restrict_closure_captures() {
    hirsel_engine::with_compiler_stack(
        written_function_binding_uses_restrict_closure_captures_on_compiler_thread,
    );
}
fn written_function_binding_uses_restrict_closure_captures_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "fn make()->fn()->u32 uses {} { let f:fn()->u32 uses {} = fn()->u32 {1}\n f }",
    );
    let result = engine.lower_to_tir(&path("make"));
    assert!(result.is_ok(), "{result:?}");
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "fn make(dir:Dir)->fn()->Dir uses {Dir} { let f:fn()->Dir uses {} = fn(){dir}\n f }",
    );
    let result = engine.mode_definition(&path("make"));
    let diagnostics = match result {
        Ok(output) => output.diagnostics,
        Err(QueryError::Diagnostics(diagnostics)) => diagnostics,
        Err(error) => panic!("{error:?}"),
    };
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].rule.as_str(), "CLO-4");
    assert_eq!(engine.sources.slice(diagnostics[0].primary), Some("dir"));
}

#[test]
fn noalloc_function_values_satisfy_callbacks_that_permit_allocation() {
    hirsel_engine::with_compiler_stack(
        noalloc_function_values_satisfy_callbacks_that_permit_allocation_on_compiler_thread,
    );
}
fn noalloc_function_values_satisfy_callbacks_that_permit_allocation_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "@noalloc fn zero()->u32 {0}\nfn invoke(f:fn()->u32)->u32 {f()}\nfn example()->u32 {invoke(zero)}",
    );
    let result = engine.lower_to_tir(&path("example"));
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn generic_value_evidence_does_not_strengthen_callable_or_container_promises() {
    hirsel_engine::with_compiler_stack(generic_value_evidence_does_not_strengthen_callable_or_container_promises_on_compiler_thread);
}
fn generic_value_evidence_does_not_strengthen_callable_or_container_promises_on_compiler_thread() {
    for source in [
        "fn example(take x:unique String)->Shared[String] { shared.freeze(take x) }",
        "fn second[U:copyable](x:(U,U)) {}\nfn example[T:copyable](x:T) {second((copy x,copy x))}",
        "fn example(take x:unique String)->once fn()->String uses{} { fn(){take x} }",
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, source);
        let output = engine.lower_to_tir(&path("example")).unwrap();
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn generic_bounds_check_each_complete_actual_and_callback_result() {
    hirsel_engine::with_compiler_stack(
        generic_bounds_check_each_complete_actual_and_callback_result_on_compiler_thread,
    );
}
fn generic_bounds_check_each_complete_actual_and_callback_result_on_compiler_thread() {
    for (source, accepted) in [
        (
            "fn cross[T:sendable](take x:T, take y:T) {}\nfn example(take x:unique String,take y:unique String) {cross(take x,take y)}",
            true,
        ),
        (
            "fn cross[T:sendable](take x:T, take y:T) {}\nfn example(take x:unique String,take y:String) {cross(take x,take y)}",
            false,
        ),
        (
            "fn cross[R:sendable](take f:once fn()->R) {}\nfn example(take x:unique String) {cross(fn(){take x})}",
            true,
        ),
        (
            "fn cross[R:sendable](take f:once fn()->R) {}\nfn example(take x:String) {cross(fn(){take x})}",
            false,
        ),
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, source);
        let output = engine.mode_definition(&path("example"));
        if accepted {
            let output = output.unwrap_or_else(|e| panic!("{source}: {e:?}"));
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
        } else {
            let Err(hirsel_types::QueryError::Diagnostics(diagnostics)) = output else {
                panic!("{source}: expected diagnostic");
            };
            assert!(
                diagnostics.iter().any(|d| d.rule.as_str() == "MODE-4"),
                "{source}: {diagnostics:?}"
            );
        }
    }
}

#[test]
fn shared_properties_survive_substitution_projection_and_ownership_refinement() {
    hirsel_engine::with_compiler_stack(shared_properties_survive_substitution_projection_and_ownership_refinement_on_compiler_thread);
}
fn shared_properties_survive_substitution_projection_and_ownership_refinement_on_compiler_thread() {
    for source in [
        "@noalloc fn wrap[T](take x:T)->(T,()){(take x,())}\n@noalloc fn example(take x:Inline[u32,4]) {let y=wrap(take x)}",
        "flat type Item {text:String,n:u64}\nfn example(xs:List[Item])->u64 {xs[0].n}",
        "fn example(take a:String, take b:unique String)->bool {a==b}",
        "fn example(tasks:Tasks) {let xs:List[u32]=[1]; let m:Mutex[List[u32]]=mutex.create(tasks,take xs)}",
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, source);
        let result = engine
            .lower_to_tir(&path("example"))
            .unwrap_or_else(|error| panic!("{source}: {error:?}"));
        assert!(
            result.diagnostics.is_empty(),
            "{source}: {:?}",
            result.diagnostics
        );
    }
}

#[test]
fn outer_parameter_authority_is_instantiated_in_callback_arguments_and_results() {
    hirsel_engine::with_compiler_stack(outer_parameter_authority_is_instantiated_in_callback_arguments_and_results_on_compiler_thread);
}
fn outer_parameter_authority_is_instantiated_in_callback_arguments_and_results_on_compiler_thread()
{
    for source in [
        "fn apply(action:fn()->() uses {Dir}, callback:fn()->() uses {action}) {callback()}\nfn example(action:fn()->() uses {Dir}) {apply(action,fn(){action()})}",
        "fn maker(take action:fn()->() uses {Dir})->fn()->() uses {action} {fn(){action()}}\nfn example(take action:fn()->() uses {Dir})->fn()->() uses {action} {maker(take action)}",
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, source);
        let output = engine
            .lower_to_tir(&path("example"))
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn local_result_rejects_an_undeclared_parameter_even_through_an_alias() {
    hirsel_engine::with_compiler_stack(
        local_result_rejects_an_undeclared_parameter_even_through_an_alias_on_compiler_thread,
    );
}
fn local_result_rejects_an_undeclared_parameter_even_through_an_alias_on_compiler_thread() {
    for value in [
        "b",
        "{ let alias = b; alias }",
        "std@1/bytes.replace(b, b\"x\", b\"y\")",
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(
            &mut engine,
            &format!("fn example(a: Bytes, b: Bytes) -> Bytes in a {{ {value} }}"),
        );
        let result = engine.lower_to_tir(&path("example"));
        let Err(QueryError::Diagnostics(diagnostics)) = result else {
            panic!("{value}: expected source diagnostic, got {result:?}")
        };
        assert!(
            diagnostics.iter().any(|d| d.rule.as_str() == "MODE-1"),
            "{value}: {diagnostics:?}"
        );
    }
}

#[test]
fn conditional_local_results_retain_alternative_sources() {
    hirsel_engine::with_compiler_stack(
        conditional_local_results_retain_alternative_sources_on_compiler_thread,
    );
}
fn conditional_local_results_retain_alternative_sources_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "fn choose(a:Bytes,b:Bytes,pick:bool)->Bytes in {a, b} {if pick {a} else {b}}\nfn example(take a:Bytes,take b:Bytes)->Bytes {choose(a,b,true)}",
    );
    for name in ["choose", "example"] {
        engine.lower_to_tir(&path(name)).unwrap();
    }
    edit(
        &mut engine,
        "fn choose(a:Bytes,b:Bytes,pick:bool)->Bytes in {a, b} {match pick {true=>a,false=>b}}\nfn example(take a:Bytes,take b:Bytes)->Bytes {choose(a,b,true)}",
    );
    for name in ["choose", "example"] {
        engine.lower_to_tir(&path(name)).unwrap();
    }
}

#[test]
fn erased_iterator_state_retains_authority_and_sendability() {
    hirsel_engine::with_compiler_stack(
        erased_iterator_state_retains_authority_and_sendability_on_compiler_thread,
    );
}
fn erased_iterator_state_retains_authority_and_sendability_on_compiler_thread() {
    for source in [
        "fn probe() { let it: Iter[u64] = iter.from(0u64, fn(mut n: u64) -> Option[u64] { None }) }",
        "fn inert[T: inert](take value: T) {}\nfn next(mut n: u32) -> Option[u32] { None }\nfn probe() { let it = iter.from(0u32, next); inert(take it) }",
        "fn send[T: sendable](take value: T) {}\nfn next(mut n: u32) -> Option[u32] { None }\nfn probe() { let it = iter.from(0u32, next); send(take it) }",
        "fn inert[T: inert](take value: T) {}\nfn probe() { let it = iter.from(0u32, fn(mut n: u32) -> Option[u32] { None }); let outer = iter.from(take it, fn(mut inner: Iter[u32]) -> Option[u32] { iter.next(mut inner) }); inert(take outer) }",
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, source);
        engine
            .lower_to_tir(&path("probe"))
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
    }
    let source = "fn inert[T: inert](take value: T) {}\nfn next(mut d: Dir) -> Option[u32] { None }\nfn probe(dir: Dir) { let it = iter.from(dir, next); inert(take it) }";
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(&mut engine, source);
    let Err(QueryError::Diagnostics(ds)) = engine.lower_to_tir(&path("probe")) else {
        panic!("authority-bearing iterator passed inert")
    };
    assert!(ds.iter().any(|d| d.rule.as_str() == "CAP-2"), "{ds:?}");
}

#[test]
fn heap_is_a_typed_allocator_value_through_aliases() {
    hirsel_engine::with_compiler_stack(
        heap_is_a_typed_allocator_value_through_aliases_on_compiler_thread,
    );
}
fn heap_is_a_typed_allocator_value_through_aliases_on_compiler_thread() {
    for source in [
        "@noalloc fn probe() { let destination = heap; let alias = destination; }",
        "fn probe() -> String { copy_to(heap, \"value\") }",
        "fn probe() -> String { let destination = heap; copy_to(destination, \"value\") }",
        "fn inert[T: inert](value:T) {}\nfn probe() { inert(heap) }",
        "fn probe() -> bool { let text: String = copy_to(heap, \"value\"); text == \"value\" }",
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, source);
        engine
            .lower_to_tir(&path("probe"))
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
    }
    // 2a938017 makes heap a neutral place, never a consumable scope grant.
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "fn inert[T: inert](take value:T) {}\nfn probe() { inert(heap) }",
    );
    let Err(QueryError::Diagnostics(diagnostics)) = engine.lower_to_tir(&path("probe")) else {
        panic!("heap cannot supply a taken arena grant")
    };
    assert!(
        diagnostics.iter().any(|d| d.rule.as_str() == "MEM-4"
            && d.message == "heap is not a consumable arena scope grant"),
        "{diagnostics:?}"
    );
}

#[test]
fn consuming_sequence_loops_extract_owned_elements_through_cursor_calls() {
    hirsel_engine::with_compiler_stack(
        consuming_sequence_loops_extract_owned_elements_through_cursor_calls_on_compiler_thread,
    );
}
fn consuming_sequence_loops_extract_owned_elements_through_cursor_calls_on_compiler_thread() {
    for container in ["Array[Bytes]", "List[Bytes]", "Inline[Bytes,3]"] {
        let source = format!(
            "fn probe(take xs:{container}) {{ for item in take xs {{ let _ = take item }} }}"
        );
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, &source);
        let output = engine
            .lower_to_tir(&path("probe"))
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
        assert!(output.value.bodies.iter().flat_map(|b| &b.blocks).any(|b| matches!(&b.terminator, hirsel_tir::Terminator::Call { callee: hirsel_tir::Callee::Definition { path, .. }, arguments, .. } if path.0.name.ends_with("Consume_open") && arguments[0].value.usage == hirsel_types::UseKind::Take)));
    }
}

#[test]
fn local_result_calls_select_actual_lifetime_independently_of_identity() {
    hirsel_engine::with_compiler_stack(
        local_result_calls_select_actual_lifetime_independently_of_identity_on_compiler_thread,
    );
}
fn local_result_calls_select_actual_lifetime_independently_of_identity_on_compiler_thread() {
    for source in [
        "fn probe() -> Bytes { let b:Bytes = b\"a\"; std@1/bytes.replace(b, b\"a\", b\"b\") }",
        "fn probe(take b:Bytes) -> Bytes { std@1/bytes.replace(b, b\"a\", b\"b\") }",
        "fn probe(b:Bytes) -> Bytes in b { std@1/bytes.replace(b, b\"a\", b\"b\") }",
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, source);
        let output = engine
            .lower_to_tir(&path("probe"))
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn branded_calls_replay_argument_identity_from_storage() {
    hirsel_engine::with_compiler_stack(
        branded_calls_replay_argument_identity_from_storage_on_compiler_thread,
    );
}
fn branded_calls_replay_argument_identity_from_storage_on_compiler_thread() {
    for source in [
        "fn make(x:i64)->Option[u8 of x] {None}\nfn inspect(x:i64,p:Option[u8 of x]) {}\nfn probe(x:i64) {let y = -x; let p=make(y); inspect(y,p)}",
        "fn target(x:u8)->u8 {x}\nfn inspect(f:fn(u8)->u8,p:u8 of f) {}\nfn probe(p:u8 of target) {inspect(target,p)}",
        "type Log {dir:Dir,file:File of .dir}\nfn inspect(d:Dir,f:File of d) {}\nfn probe(log:Log) { {let Log(dir:d,file:value)=log; inspect(d,value)}; inspect(log.dir,log.file)}",
        "type Log {dir:Dir,file:File of .dir}\nfn inspect(d:Dir,f:File of d) {}\nfn probe(take log:Log) {let Log(dir:d,file:value)=take log; inspect(d,value)}",
        "type Log {file:File of .dir,dir:Dir}\nfn inspect(d:Dir,f:File of d) {}\nfn probe(take log:Log) {let Log(file:value,dir:d)=take log; inspect(d,value)}",
        "type Log {dir:Dir,file:File of .dir}\nfn inspect(d:Dir,f:File of d) {}\nfn probe(log:Log) {let alias=log; inspect(alias.dir,alias.file)}",
        "type Log {dir:Dir,file:File of .dir}\nfn inspect(d:Dir,f:File of d) {}\nfn probe(take log:Log) {let moved=take log; inspect(moved.dir,moved.file)}",
        "type Log { dir:Dir, file:File of .dir }\nfn probe(d:Dir,other:Dir,take value:File of d)->Log {Log(dir:d,file:take value)}",
        "type Log { dir:Dir, file:File of .dir }\nfn inspect(d:Dir,f:File of d) {}\nfn probe(log:Log) {inspect(log.dir,log.file)}",
        include_str!("../../../../conformance/PROV/PROV-1-open-moved-result.hir"),
        "fn probe() {let s:String=\"abc\"; let positions=string.positions(s); for p in positions {let _=string.slice(s,p,p)}}",
        "const FIRST:String=\"first\"; fn inspect(s:String,p:StrPos of s){}\nfn probe(p:StrPos of FIRST){inspect(FIRST,p)}",
        "fn probe(s:Slots[u64],id:SlotId[u64] of s)->Option[u64] {slots.get(s,id)}",
        "fn probe(s:String,start:StrPos of s,end:StrPos of s)->String in s {string.slice(s,start,end)}",
        "fn probe(s:String) { let positions=string.positions(s); for p in positions { let _=string.slice(s,p,p) } }",
        "fn probe(xs:Map[u64,u64]) {for x in xs {let _=x}}",
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, source);
        engine
            .lower_to_tir(&path("probe"))
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
    }
}

#[test]
fn closure_calls_keep_the_captured_alias_identity() {
    hirsel_engine::with_compiler_stack(
        closure_calls_keep_the_captured_alias_identity_on_compiler_thread,
    );
}
fn closure_calls_keep_the_captured_alias_identity_on_compiler_thread() {
    for (body, rejects_end_origin) in [
        (
            "let holder=Holder(action:fn(){use_pos(alias,p)}); holder.action()",
            false,
        ),
        (
            "let action=fn(){use_pos(alias,p)}; action(); action()",
            false,
        ),
        (
            "let action:once fn()=fn(){use_pos(alias,p);let _=take alias}; action()",
            false,
        ),
        // A later loop iteration must not create a capture from an ended source.
        (
            "for i in 0u8..2u8 { let action:once (fn()) in action=fn(){use_pos(alias,p)}; action() }",
            true,
        ),
    ] {
        assert_captured_alias_identity(body, rejects_end_origin);
    }
}

#[test]
fn generic_relays_preserve_owned_capture_identity_scopes() {
    hirsel_engine::with_compiler_stack(
        generic_relays_preserve_owned_capture_identity_scopes_on_compiler_thread,
    );
}
fn generic_relays_preserve_owned_capture_identity_scopes_on_compiler_thread() {
    for body in [
        "let holder=Holder(action:fn(){use_pos(alias,p)}); let forwarded=relay(take holder); forwarded.action()",
        "let holder=Holder(action:fn(){use_pos(alias,p)}); let first=relay(take holder); let second=relay(take first); second.action()",
    ] {
        assert_captured_alias_identity(body, false);
    }
}

fn assert_captured_alias_identity(body: &str, rejects_end_origin: bool) {
    use hirsel_types::SignatureProvider;
    let source = format!(
        "type Holder[F] {{action:F}}\nfn relay[F](take f:F)->F {{f}}\nfn use_pos(s:Dir,p:u64 of s) {{}}\nfn probe(s:Dir,p:u64 of s) {{let alias=s; {body}}}"
    );
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(&mut engine, &source);
    let output = engine
        .lower_to_tir(&path("probe"))
        .unwrap_or_else(|e| panic!("{source}: {e:?}"));
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in registry::BuiltinRegistry::new().signatures() {
        signatures.insert(signature.definition.clone(), signature.as_ref().clone());
    }
    for name in ["probe", "use_pos", "Holder", "relay"] {
        let signature = engine
            .signature(hirsel_syntax::DefId::intern(&path(name)))
            .unwrap();
        signatures.insert(path(name), signature.as_ref().clone());
    }
    assert!(
        hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
            .violations
            .is_empty()
    );
    let mut ended = output.value.as_ref().clone();
    let outer = &mut ended.bodies[ended.entry.0 as usize];
    let source = hirsel_tir::PlaceId(
        outer
            .places
            .iter()
            .position(|p| p.root == outer.parameters[0] && p.projection.is_empty())
            .unwrap() as u32,
    );
    let invocation = outer
        .blocks
        .iter_mut()
        .find(|b| {
            matches!(
                b.terminator,
                hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Indirect { .. },
                    ..
                }
            )
        })
        .unwrap();
    let hirsel_tir::Terminator::Call {
        callee: hirsel_tir::Callee::Indirect { value: callee, .. },
        ..
    } = &invocation.terminator
    else {
        unreachable!()
    };
    let callee = callee.place;
    invocation.instructions.push(hirsel_tir::Instruction {
        kind: hirsel_tir::InstructionKind::EndOrigin(source),
        span: Default::default(),
        on_failure: hirsel_tir::CleanupId(0),
    });
    ended.types = hirsel_tir::definition_types(engine.types(), &ended);
    let report = hirsel_validate::validate(&ended, engine.types(), &signatures, &mut vec![]);
    if rejects_end_origin {
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.kind == hirsel_validate::ViolationKind::InvalidOrigin(callee)),
            "{body}: {report:?}"
        );
    } else {
        assert!(report.violations.is_empty(), "{report:?}");
    }
    let mut changed = output.value.as_ref().clone();
    let nested = changed
        .bodies
        .iter_mut()
        .find(|b| {
            b.locals
                .iter()
                .any(|l| l.storage == hirsel_tir::Storage::Capture)
        })
        .unwrap();
    let capture = nested
        .places
        .iter()
        .position(|p| {
            nested.locals[p.root.0 as usize].storage == hirsel_tir::Storage::Capture
                && p.binding.is_some()
        })
        .unwrap();
    let place = hirsel_tir::PlaceId(capture as u32);
    nested.blocks[nested.entry.0 as usize].instructions.insert(
        0,
        hirsel_tir::Instruction {
            kind: hirsel_tir::InstructionKind::BindOrigin {
                source: place,
                place,
                kind: hirsel_tir::OriginBindingKind::Fresh,
            },
            span: Default::default(),
            on_failure: hirsel_tir::CleanupId(0),
        },
    );
    changed.types = hirsel_tir::definition_types(engine.types(), &changed);
    let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
    assert!(
        report
            .violations
            .iter()
            .any(|v| matches!(v.kind, hirsel_validate::ViolationKind::InvalidOrigin(_))),
        "{report:?}"
    );
}

#[test]
fn sibling_projection_cannot_claim_another_records_identity() {
    hirsel_engine::with_compiler_stack(
        sibling_projection_cannot_claim_another_records_identity_on_compiler_thread,
    );
}
fn sibling_projection_cannot_claim_another_records_identity_on_compiler_thread() {
    use hirsel_types::{OriginRoot, ParamId, ParameterRef, SignatureProvider};
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "type Log {dir:Dir,file:File of .dir}\nfn inspect(d:Dir,f:File of d) {}\nfn probe(log:Log,other:Log) {inspect(log.dir,log.file)}",
    );
    let output = engine.lower_to_tir(&path("probe")).unwrap();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in registry::BuiltinRegistry::new().signatures() {
        signatures.insert(signature.definition.clone(), signature.as_ref().clone());
    }
    for name in ["probe", "inspect", "Log"] {
        signatures.insert(
            path(name),
            engine
                .signature(hirsel_syntax::DefId::intern(&path(name)))
                .unwrap()
                .as_ref()
                .clone(),
        );
    }
    let mut changed = output.value.as_ref().clone();
    let body = &mut changed.bodies[changed.entry.0 as usize];
    let original = OriginRoot::Parameter(ParameterRef::body(body.parameter_scope, ParamId(0)));
    let forged = OriginRoot::Parameter(ParameterRef::body(body.parameter_scope, ParamId(1)));
    let mut count = 0;
    for place in &mut body.places {
        if place.root == body.parameters[0] && !place.projection.is_empty() {
            place.binding.as_mut().unwrap().root = forged.clone();
            place.ty = hirsel_types::map_type_origins(engine.types(), place.ty, |origin, _| {
                let mut origin = origin.clone();
                if origin.root == original {
                    origin.root = forged.clone();
                }
                Ok(origin)
            })
            .unwrap();
            count += 1;
        }
    }
    assert!(count >= 2);
    changed.types = hirsel_tir::definition_types(engine.types(), &changed);
    let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
    assert!(
        report
            .violations
            .iter()
            .any(|v| matches!(v.kind, hirsel_validate::ViolationKind::InvalidOrigin(_))),
        "{report:?}"
    );
}

#[test]
fn sibling_constructor_checks_the_supplied_owner_operand() {
    hirsel_engine::with_compiler_stack(
        sibling_constructor_checks_the_supplied_owner_operand_on_compiler_thread,
    );
}
fn sibling_constructor_checks_the_supplied_owner_operand_on_compiler_thread() {
    use hirsel_types::SignatureProvider;
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "type Log {dir:Dir,file:File of .dir}\nfn probe(d:Dir,other:Dir,take value:File of d)->Log {Log(dir:d,file:take value)}",
    );
    let output = engine.lower_to_tir(&path("probe")).unwrap();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in registry::BuiltinRegistry::new().signatures() {
        signatures.insert(signature.definition.clone(), signature.as_ref().clone());
    }
    for name in ["probe", "Log"] {
        signatures.insert(
            path(name),
            engine
                .signature(hirsel_syntax::DefId::intern(&path(name)))
                .unwrap()
                .as_ref()
                .clone(),
        );
    }
    let mut changed = output.value.as_ref().clone();
    let body = &mut changed.bodies[changed.entry.0 as usize];
    let other = hirsel_tir::PlaceId(
        body.places
            .iter()
            .position(|p| p.root == body.parameters[1] && p.projection.is_empty())
            .unwrap() as u32,
    );
    let fields = body
        .rvalues
        .iter_mut()
        .find_map(|r| match r {
            hirsel_tir::Rvalue::Construct {
                constructor: hirsel_tir::Constructor::Record(owner),
                fields,
                ..
            } if *owner == path("Log") => Some(fields),
            _ => None,
        })
        .unwrap();
    fields.iter_mut().find(|f| f.field == 0).unwrap().value = hirsel_tir::Operand {
        place: other,
        usage: hirsel_types::UseKind::Copy,
    };
    changed.types = hirsel_tir::definition_types(engine.types(), &changed);
    let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
    assert!(
        report.violations.iter().any(|v| v.kind
            == hirsel_validate::ViolationKind::TypeMismatch {
                entity: "constructor field"
            }),
        "{report:?}"
    );
}

#[test]
fn destructuring_derives_sibling_brands_instead_of_trusting_field_types() {
    hirsel_engine::with_compiler_stack(
        destructuring_derives_sibling_brands_instead_of_trusting_field_types_on_compiler_thread,
    );
}
fn destructuring_derives_sibling_brands_instead_of_trusting_field_types_on_compiler_thread() {
    use hirsel_types::{OriginRoot, ParamId, ParameterRef, SignatureProvider};
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "type Log {dir:Dir,file:File of .dir}\nfn inspect(d:Dir,f:File of d) {}\nfn probe(take log:Log,other:Dir) {let Log(dir:d,file:value)=take log; inspect(d,value)}",
    );
    let output = engine.lower_to_tir(&path("probe")).unwrap();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in registry::BuiltinRegistry::new().signatures() {
        signatures.insert(signature.definition.clone(), signature.as_ref().clone());
    }
    for name in ["probe", "inspect", "Log"] {
        signatures.insert(
            path(name),
            engine
                .signature(hirsel_syntax::DefId::intern(&path(name)))
                .unwrap()
                .as_ref()
                .clone(),
        );
    }
    let mut changed = output.value.as_ref().clone();
    let body = &mut changed.bodies[changed.entry.0 as usize];
    let fields = body
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| {
            if let hirsel_tir::InstructionKind::BindFields { fields, .. } = &i.kind {
                Some(fields.clone())
            } else {
                None
            }
        })
        .unwrap();
    let file = *fields
        .iter()
        .find(|p| {
            body.places[p.0 as usize].projection.last() == Some(&hirsel_tir::Projection::Field(1))
        })
        .unwrap();
    for ty in body
        .locals
        .iter_mut()
        .map(|l| &mut l.ty)
        .chain(body.places.iter_mut().map(|p| &mut p.ty))
    {
        *ty = hirsel_types::map_type_origins(engine.types(), *ty, |origin, _| {
            let mut origin = origin.clone();
            if matches!(origin.root, OriginRoot::Local(..)) {
                origin.root =
                    OriginRoot::Parameter(ParameterRef::body(body.parameter_scope, ParamId(1)));
            }
            Ok(origin)
        })
        .unwrap();
    }
    changed.types = hirsel_tir::definition_types(engine.types(), &changed);
    let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.kind == hirsel_validate::ViolationKind::InvalidProjection(file)),
        "{report:?}"
    );
}

#[test]
fn computed_unary_values_do_not_retain_the_operand_identity() {
    hirsel_engine::with_compiler_stack(
        computed_unary_values_do_not_retain_the_operand_identity_on_compiler_thread,
    );
}
fn computed_unary_values_do_not_retain_the_operand_identity_on_compiler_thread() {
    for expression in ["-x", "~x"] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = format!(
            "fn inspect(x:i64,p:u8 of x) {{}}\nfn probe(x:i64,p:u8 of x) {{inspect({expression},p)}}"
        );
        edit(&mut engine, &source);
        let result = engine.lower_to_tir(&path("probe"));
        let Err(QueryError::Diagnostics(diagnostics)) = result else {
            panic!("{source}: {result:?}");
        };
        assert!(
            diagnostics.iter().any(|d| d.rule.as_str() == "PROV-1"),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn named_function_identity_requires_named_definition_evidence() {
    hirsel_engine::with_compiler_stack(
        named_function_identity_requires_named_definition_evidence_on_compiler_thread,
    );
}
fn named_function_identity_requires_named_definition_evidence_on_compiler_thread() {
    use hirsel_types::SignatureProvider;
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "fn target(x:u8,y:u8)->u8 {x}\nfn other(x:u8,y:u8)->u8 {y}\nfn inspect(f:fn(u8,u8)->u8,p:u8 of f) {}\nfn probe(p:u8 of target) {inspect(target,p)}",
    );
    let output = engine.lower_to_tir(&path("probe")).unwrap();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in registry::BuiltinRegistry::new().signatures() {
        signatures.insert(signature.definition.clone(), signature.as_ref().clone());
    }
    for name in ["probe", "inspect", "target", "other"] {
        signatures.insert(
            path(name),
            engine
                .signature(hirsel_syntax::DefId::intern(&path(name)))
                .unwrap()
                .as_ref()
                .clone(),
        );
    }
    assert_eq!(output.value.bodies.len(), 1);
    let mut changed = output.value.as_ref().clone();
    let reference = changed.bodies[0]
        .rvalues
        .iter_mut()
        .find_map(|value| match value {
            hirsel_tir::Rvalue::FunctionReference { definition, .. } => Some(definition),
            _ => None,
        })
        .expect("named function reference");
    *reference = path("other");
    changed.types = hirsel_tir::definition_types(engine.types(), &changed);
    let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.body == Some(changed.entry)
                && matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::InvalidOrigin(_)
                        | hirsel_validate::ViolationKind::TypeMismatch {
                            entity: "call argument"
                        }
                )),
        "{report:?}"
    );
}

#[test]
fn expiring_one_stored_capture_does_not_invalidate_another_field() {
    hirsel_engine::with_compiler_stack(
        expiring_one_stored_capture_does_not_invalidate_another_field_on_compiler_thread,
    );
}
fn expiring_one_stored_capture_does_not_invalidate_another_field_on_compiler_thread() {
    use hirsel_types::SignatureProvider;
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "type Pair[A,B] {first:A,second:B}\nfn use_pos(s:Dir,p:u64 of s) {}\nfn probe(s:Dir,p:u64 of s) {let alias=s;let pair=Pair(first:fn(){use_pos(alias,p)},second:fn(){});pair.second()}",
    );
    let output = engine.lower_to_tir(&path("probe")).unwrap();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in registry::BuiltinRegistry::new().signatures() {
        signatures.insert(signature.definition.clone(), signature.as_ref().clone());
    }
    for name in ["probe", "use_pos", "Pair"] {
        signatures.insert(
            path(name),
            engine
                .signature(hirsel_syntax::DefId::intern(&path(name)))
                .unwrap()
                .as_ref()
                .clone(),
        );
    }
    let mut changed = output.value.as_ref().clone();
    let outer = &mut changed.bodies[changed.entry.0 as usize];
    let source = hirsel_tir::PlaceId(
        outer
            .places
            .iter()
            .position(|p| p.root == outer.parameters[0] && p.projection.is_empty())
            .unwrap() as u32,
    );
    let invocation = outer
        .blocks
        .iter_mut()
        .find(|b| {
            matches!(
                b.terminator,
                hirsel_tir::Terminator::Call {
                    callee: hirsel_tir::Callee::Indirect { .. },
                    ..
                }
            )
        })
        .unwrap();
    invocation.instructions.push(hirsel_tir::Instruction {
        kind: hirsel_tir::InstructionKind::EndOrigin(source),
        span: Default::default(),
        on_failure: hirsel_tir::CleanupId(0),
    });
    changed.types = hirsel_tir::definition_types(engine.types(), &changed);
    let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
    assert!(report.violations.is_empty(), "{report:?}");
}

#[test]
fn mutable_iteration_alias_access_requires_its_live_loan() {
    hirsel_engine::with_compiler_stack(
        mutable_iteration_alias_access_requires_its_live_loan_on_compiler_thread,
    );
}
fn mutable_iteration_alias_access_requires_its_live_loan_on_compiler_thread() {
    use hirsel_tir::{InstructionKind, Projection, Rvalue};
    use hirsel_types::SignatureProvider;
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "fn probe(mut xs:Array[u8]) {for x in mut xs {let y=x; x+=1}}",
    );
    let output = engine.lower_to_tir(&path("probe")).unwrap();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in registry::BuiltinRegistry::new().signatures() {
        signatures.insert(signature.definition.clone(), signature.as_ref().clone());
    }
    signatures.insert(
        path("probe"),
        engine
            .signature(hirsel_syntax::DefId::intern(&path("probe")))
            .unwrap()
            .as_ref()
            .clone(),
    );
    let report = hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![]);
    assert!(report.violations.is_empty(), "{report:?}");
    let mut changed = output.value.as_ref().clone();
    let body = &mut changed.bodies[changed.entry.0 as usize];
    let (block, instruction, place, loan) = body
        .blocks
        .iter()
        .enumerate()
        .find_map(|(b, block)| {
            block.instructions.iter().enumerate().find_map(|(i, ins)| {
                let InstructionKind::Assign { value, .. } = ins.kind else {
                    return None;
                };
                let Rvalue::Use(value) = &body.rvalues[value.0 as usize] else {
                    return None;
                };
                let p = &body.places[value.place.0 as usize];
                let loan = p.loan?;
                p.projection
                    .iter()
                    .any(|p| matches!(p, Projection::Index(_)))
                    .then_some((b, i, value.place, loan))
            })
        })
        .unwrap();
    let mut end = body.blocks[block].instructions[instruction].clone();
    end.kind = InstructionKind::EndLoan {
        loan,
        if_active: false,
    };
    body.blocks[block].instructions.insert(instruction, end);
    changed.types = hirsel_tir::definition_types(engine.types(), &changed);
    let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.kind == hirsel_validate::ViolationKind::ExpiredLoan { loan, place }),
        "{report:?}"
    );
    changed.bodies[changed.entry.0 as usize].places[place.0 as usize].loan = None;
    changed.types = hirsel_tir::definition_types(engine.types(), &changed);
    let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.kind == hirsel_validate::ViolationKind::InvalidOrigin(place)),
        "{report:?}"
    );
    let mut changed = output.value.as_ref().clone();
    changed.bodies[changed.entry.0 as usize].places[place.0 as usize].loan = None;
    changed.types = hirsel_tir::definition_types(engine.types(), &changed);
    let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
    assert!(
        report
            .violations
            .iter()
            .any(|v| matches!(v.kind, hirsel_validate::ViolationKind::LoanConflict { .. })),
        "{report:?}"
    );
}

#[test]
fn borrowed_closure_and_holder_relay_retain_source_loans() {
    hirsel_engine::with_compiler_stack(
        borrowed_closure_and_holder_relay_retain_source_loans_on_compiler_thread,
    );
}
fn borrowed_closure_and_holder_relay_retain_source_loans_on_compiler_thread() {
    use hirsel_types::SignatureProvider;
    for (source, names) in [
        (
            "fn inspect(s:String,p:StrPos of s){}\nfn probe(s:String,p:StrPos of s){let action=fn(){inspect(s,p)};action()}",
            vec!["probe", "inspect"],
        ),
        (
            "type Holder[F]{action:F}\nfn relay[F](f:F)->F in f{f}\nfn probe(holder:Holder[fn()]){let first=relay(holder);let second=relay(first);second.action()}",
            vec!["probe", "Holder", "relay"],
        ),
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut engine, source);
        let output = engine
            .lower_to_tir(&path("probe"))
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
        let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
        for signature in registry::BuiltinRegistry::new().signatures() {
            signatures.insert(signature.definition.clone(), signature.as_ref().clone());
        }
        for name in names {
            signatures.insert(
                path(name),
                engine
                    .signature(hirsel_syntax::DefId::intern(&path(name)))
                    .unwrap()
                    .as_ref()
                    .clone(),
            );
        }
        let mut changed = output.value.as_ref().clone();
        let outer = &mut changed.bodies[changed.entry.0 as usize];
        let loan = outer
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|i| match i.kind {
                hirsel_tir::InstructionKind::BeginLoan { id, place, .. }
                    if outer.places[place.0 as usize].root == outer.parameters[0] =>
                {
                    Some(id)
                }
                _ => None,
            })
            .expect("source loan");
        let invocation = outer
            .blocks
            .iter_mut()
            .find(|b| {
                matches!(
                    b.terminator,
                    hirsel_tir::Terminator::Call {
                        callee: hirsel_tir::Callee::Indirect { .. },
                        ..
                    }
                )
            })
            .unwrap();
        let hirsel_tir::Terminator::Call {
            callee: hirsel_tir::Callee::Indirect { value: callee, .. },
            ..
        } = &invocation.terminator
        else {
            unreachable!()
        };
        let callee = callee.place;
        invocation.instructions.push(hirsel_tir::Instruction {
            kind: hirsel_tir::InstructionKind::EndLoan {
                loan,
                if_active: false,
            },
            span: Default::default(),
            on_failure: hirsel_tir::CleanupId(0),
        });
        changed.types = hirsel_tir::definition_types(engine.types(), &changed);
        let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| v.kind
                == hirsel_validate::ViolationKind::ExpiredLoan {
                    loan,
                    place: callee
                }),
            "{source}: {report:?}"
        );
    }
}

#[test]
fn cached_phase_artifacts_keep_frames_cursors_and_ownership_with_their_body() {
    hirsel_engine::with_compiler_stack(
        cached_phase_artifacts_keep_frames_cursors_and_ownership_with_their_body_on_compiler_thread,
    );
}
fn cached_phase_artifacts_keep_frames_cursors_and_ownership_with_their_body_on_compiler_thread() {
    let source = "resource type Token { value:u32 } drop(x) { let _ = x.value }\nconst VALUE:u32 = comptime (2 + 3)\nfn inspect(xs:List[String]) { for x in xs { let scoped = fn() { let _ = x }; scoped(); } }\n";
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(&mut engine, source);
    for name in ["Token", "VALUE", "inspect"] {
        engine.lower_to_tir(&path(name)).unwrap();
    }
    let before = engine.check_definition(&path("inspect")).unwrap().value;
    let before_view = artifact_view(&before);
    let analyzed = engine.mode_definition(&path("inspect")).unwrap().value;
    assert_eq!(analyzed.typed.plans.loops.len(), 1);
    assert!(!analyzed.ownership.captures.is_empty());
    assert!(analyzed.results.len() >= 2);
    let cached = engine.check_definition(&path("inspect")).unwrap().value;
    assert_eq!(before_view, artifact_view(&cached));
    let rejected = source.replace("scoped();", "scoped(); let wrong:u32 = true;");
    edit(&mut engine, &rejected);
    assert!(matches!(
        engine.lower_to_tir(&path("inspect")),
        Err(QueryError::Diagnostics(_))
    ));
    for next in [
        source.replace("let _ = x };", "let _ = x; let _ = x };"),
        source.to_owned(),
    ] {
        edit(&mut engine, &next);
        let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
        edit(&mut fresh, &next);
        for name in ["Token", "VALUE", "inspect"] {
            engine.lower_to_tir(&path(name)).unwrap();
            fresh.lower_to_tir(&path(name)).unwrap();
            equal_artifacts(&mut engine, &mut fresh, &path(name));
        }
        assert_eq!(
            engine.materialize_constant(&path("VALUE")).unwrap(),
            hirsel_tir::Constant::Integer(5)
        );
    }
}

#[test]
fn missing_capability_arguments_keep_declared_mutation_ordinals() {
    hirsel_engine::with_compiler_stack(
        missing_capability_arguments_keep_declared_mutation_ordinals_on_compiler_thread,
    );
}
fn missing_capability_arguments_keep_declared_mutation_ordinals_on_compiler_thread() {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "fn use_dir(dir: Dir, file: File of dir) {}\nfn need(io: Io, mut dir: Dir, file: File of dir) {}\nfn example(io: Io, mut dir: Dir, file: File of dir) {\n  need(mut dir, file)\n  use_dir(dir, file)\n}\n",
    );
    let diagnostics = match engine.mode_definition(&path("example")) {
        Ok(out) => out.diagnostics,
        Err(QueryError::Diagnostics(ds)) => ds,
        Err(error) => panic!("{error:?}"),
    };
    let sites: Vec<_> = diagnostics
        .iter()
        .map(|d| {
            (
                d.rule.as_str(),
                engine.sources.location(d.primary).unwrap().line,
            )
        })
        .collect();
    assert_eq!(sites, [("CAP-1", 4), ("PROV-1", 5)]);
}

#[test]
fn an_element_of_a_capture_has_its_own_name_but_cannot_take_the_captures() {
    use hirsel_types::SignatureProvider;
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    edit(
        &mut engine,
        "fn probe(xs:List[u8],i:u64)->u8 {(fn() {xs[i]})()}",
    );
    let output = engine.lower_to_tir(&path("probe")).unwrap();
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    for signature in registry::BuiltinRegistry::new().signatures() {
        signatures.insert(signature.definition.clone(), signature.as_ref().clone());
    }
    signatures.insert(
        path("probe"),
        engine
            .signature(hirsel_syntax::DefId::intern(&path("probe")))
            .unwrap()
            .as_ref()
            .clone(),
    );
    let mut changed = output.value.as_ref().clone();
    let nested = changed
        .bodies
        .iter_mut()
        .find(|b| {
            b.locals
                .iter()
                .any(|l| l.storage == hirsel_tir::Storage::Capture)
        })
        .unwrap();
    // The element read is a fresh introduction on a place inside the capture.
    let (element, environment) = nested
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| match i.kind {
            hirsel_tir::InstructionKind::BindOrigin {
                place,
                kind: hirsel_tir::OriginBindingKind::Fresh,
                ..
            } => {
                let target = &nested.places[place.0 as usize];
                let captured = nested
                    .places
                    .iter()
                    .find(|p| p.root == target.root && p.projection.is_empty())?;
                (nested.locals[target.root.0 as usize].storage == hirsel_tir::Storage::Capture
                    && !target.projection.is_empty())
                .then(|| (place, captured.binding.clone().unwrap()))
            }
            _ => None,
        })
        .unwrap();
    // Naming the element as the captured binding would mint a new incarnation
    // of an identity the environment owns.
    nested.places[element.0 as usize].binding = Some(environment);
    changed.types = hirsel_tir::definition_types(engine.types(), &changed);
    let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.kind == hirsel_validate::ViolationKind::InvalidOrigin(element)),
        "{report:?}"
    );
}

#[test]
fn fresh_alias_join_identity_is_independently_checked() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        for branch in [
            "if c{r=copy z}",
            "match c{true=>{r=copy z},false=>{}}",
            "while c{r=copy z;c=false}",
        ] {
            let source = format!(
                "type R{{n:u64}}\nfn make(r:R)->Option[u8 of r]{{None}}\nfn inspect(r:R,p:Option[u8 of r])->(){{}}\nfn probe(take x:R,take z:R,flag:bool){{let mut c=flag;let mut r=copy x;{branch};let q=make(r);inspect(r,q)}}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, &source);
            let output = engine.lower_to_tir(&path("probe")).unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            for signature in registry::BuiltinRegistry::new().signatures() {
                signatures.insert(signature.definition.clone(), signature.as_ref().clone());
            }
            for name in ["probe", "make", "inspect", "R"] {
                let signature = engine
                    .signature(hirsel_syntax::DefId::intern(&path(name)))
                    .unwrap();
                signatures.insert(path(name), signature.as_ref().clone());
            }
            let report =
                hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![]);
            assert!(report.violations.is_empty(), "{branch}: {report:?}");
            let mut changed = output.value.as_ref().clone();
            let body = &mut changed.bodies[changed.entry.0 as usize];
            let instruction = body.blocks.iter_mut().flat_map(|b| &mut b.instructions).find(|i| {
                let hirsel_tir::InstructionKind::BindOrigin { place, kind: hirsel_tir::OriginBindingKind::Fresh, .. } = i.kind else { return false };
                let access = &body.places[place.0 as usize];
                matches!(engine.types().get(access.ty).unwrap().kind, hirsel_types::TypeKind::Named { ref definition, .. } if *definition == hirsel_syntax::DefId::intern(&path("R")))
                    && engine.sources.slice(i.span).is_some_and(|text| text.trim_end_matches(';') == branch)
            }).unwrap_or_else(|| panic!("{branch}: published fresh R identity at the join"));
            let hirsel_tir::InstructionKind::BindOrigin { kind, .. } = &mut instruction.kind else {
                unreachable!()
            };
            *kind = hirsel_tir::OriginBindingKind::Alias;
            changed.types = hirsel_tir::definition_types(engine.types(), &changed);
            let report =
                hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::TypeMismatch {
                        entity: "call result"
                    } | hirsel_validate::ViolationKind::InvalidOrigin(_)
                )),
                "{branch}: {report:?}"
            );
        }
    });
}

#[test]
fn moved_aliases_have_no_join_identity_introduction() {
    hirsel_engine::with_compiler_stack(|| {
        for branch in [
            "if c{let _=take r}",
            "match c{true=>{let _=take r},false=>{}}",
            "while c{r=copy z;let _=take r;break}",
            "for i in 0u8..2u8{r=copy z;let _=take r;break}",
            "let xs:List[u8]=[1u8,2u8];for i in xs{r=copy z;let _=take r;break}",
            "let xs:List[u8]=[1u8,2u8];for i in take xs{r=copy z;let _=take r;break}",
        ] {
            let source = format!(
                "type R{{n:Bytes}}\nfn make(r:R)->Option[u8 of r]{{None}}\nfn inspect(r:R,p:Option[u8 of r]){{}}\nfn probe(take x:R,take z:R,flag:bool){{let mut c=flag;let mut r=copy x;let p=make(r);{branch};inspect(x,p)}}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, &source);
            let analyzed = engine.mode_definition(&path("probe")).unwrap();
            let checked = &analyzed.value.typed.checked;
            let root = checked.bindings.iter().find_map(|b| {
                matches!(engine.types().get(b.ty).unwrap().kind, hirsel_types::TypeKind::Named { ref definition, .. } if *definition == hirsel_syntax::DefId::intern(&path("R")))
                    .then(|| match b.binding { hirsel_types::ResolvedName::Local(l) => Some(hirsel_types::OriginRoot::Local(analyzed.value.typed.syntax.owner(), l)), _ => None }).flatten()
            }).expect("local r");
            assert!(
                analyzed
                    .value
                    .ownership
                    .identity_joins
                    .iter()
                    .all(|(_, _, origin)| origin.root != root),
                "{source}"
            );
            engine
                .lower_to_tir(&path("probe"))
                .unwrap_or_else(|error| panic!("{source}: {error:?}"));
        }
    });
}

#[test]
fn copied_field_join_partitions_are_independently_checked() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        for branch in [
            "if c{r.n=3}",
            "match c{true=>{r.n=3},false=>{}}",
            "while c{r.n=3;c=false}",
        ] {
            let source = format!(
                "type R{{n:u64,other:u64}}\nfn make_n(r:R)->Option[u8 of r.n]{{None}}\nfn make_other(r:R)->Option[u8 of r.other]{{None}}\nfn inspect_n(r:R,p:Option[u8 of r.n]){{}}\nfn inspect_other(r:R,p:Option[u8 of r.other]){{}}\nfn probe(take x:R,flag:bool){{let mut c=flag;let mut r=copy x;let p=make_other(r);{branch};inspect_other(r,p);let q=make_n(r);inspect_n(r,q)}}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, &source);
            let output = engine
                .lower_to_tir(&path("probe"))
                .unwrap_or_else(|error| panic!("{source}: {error:?}"));
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            for signature in registry::BuiltinRegistry::new().signatures() {
                signatures.insert(signature.definition.clone(), signature.as_ref().clone());
            }
            for name in [
                "probe",
                "make_n",
                "make_other",
                "inspect_n",
                "inspect_other",
                "R",
            ] {
                let signature = engine
                    .signature(hirsel_syntax::DefId::intern(&path(name)))
                    .unwrap();
                signatures.insert(path(name), signature.as_ref().clone());
            }
            let report =
                hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![]);
            assert!(report.violations.is_empty(), "{source}: {report:?}");
            let mut changed = output.value.as_ref().clone();
            let body = &mut changed.bodies[changed.entry.0 as usize];
            let places = &body.places;
            let mut removed = 0;
            for block in &mut body.blocks {
                block.instructions.retain(|instruction| {
                    let hirsel_tir::InstructionKind::BindOrigin {
                        place,
                        kind: hirsel_tir::OriginBindingKind::Fresh,
                        ..
                    } = instruction.kind
                    else {
                        return true;
                    };
                    let remove = !places[place.0 as usize].projection.is_empty()
                        && engine
                            .sources
                            .slice(instruction.span)
                            .is_some_and(|text| text.trim_end_matches(';') == branch);
                    removed += usize::from(remove);
                    !remove
                });
            }
            assert_eq!(removed, 1, "{branch}");
            changed.types = hirsel_tir::definition_types(engine.types(), &changed);
            let report =
                hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::TypeMismatch {
                        entity: "call result"
                    } | hirsel_validate::ViolationKind::InvalidOrigin(_)
                )),
                "{branch}: {report:?}"
            );
        }
    });
}

#[test]
fn closure_authority_allows_capabilities_derived_from_permitted_inputs() {
    hirsel_engine::with_compiler_stack(|| {
        for body in [
            "let _=(fn(){out.print(rt,\"ok\\n\")})()",
            "let f=fn(){out.print(rt,\"ok\\n\")};let _=f()",
            "let f=fn() uses {Stdout}{out.print(rt,\"ok\\n\")};let _=f()",
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, &format!("fn probe(rt:Stdout){{{body}}}"));
            engine
                .lower_to_tir(&path("probe"))
                .unwrap_or_else(|error| panic!("{body}: {error:?}"));
        }
    });
}

#[test]
fn whole_owned_replacement_requires_a_fresh_receiving_origin() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        for replacement in ["s=copy s", "s=copy other", "s=take other"] {
            let source = format!(
                "fn probe(){{let mut s:Slots[u64]=slots.empty();let other:Slots[u64]=slots.empty();{replacement};let id=slots.insert(mut s,10u64);let _=slots.get(s,id)}}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, &source);
            let output = engine.lower_to_tir(&path("probe")).unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            for signature in registry::BuiltinRegistry::new().signatures() {
                signatures.insert(signature.definition.clone(), signature.as_ref().clone());
            }
            let signature = engine
                .signature(hirsel_syntax::DefId::intern(&path("probe")))
                .unwrap();
            signatures.insert(path("probe"), signature.as_ref().clone());
            assert!(
                hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                    .violations
                    .is_empty()
            );
            let mut changed = output.value.as_ref().clone();
            let body = &mut changed.bodies[changed.entry.0 as usize];
            let mut removed = 0;
            for block in &mut body.blocks {
                block.instructions.retain(|instruction| {
                    let remove = matches!(
                        instruction.kind,
                        hirsel_tir::InstructionKind::BindOrigin {
                            kind: hirsel_tir::OriginBindingKind::Fresh,
                            ..
                        }
                    ) && engine
                        .sources
                        .slice(instruction.span)
                        .is_some_and(|text| text.trim_end_matches(';') == replacement);
                    removed += usize::from(remove);
                    !remove
                });
            }
            assert_eq!(removed, 1, "{replacement}");
            changed.types = hirsel_tir::definition_types(engine.types(), &changed);
            let report =
                hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::TypeMismatch {
                        entity: "call result"
                    } | hirsel_validate::ViolationKind::InvalidOrigin(_)
                )),
                "{replacement}: {report:?}"
            );
        }
    });
}

#[test]
fn record_constructors_independently_require_every_declared_field() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        for declaration in ["type R{x:u8}", "type R[T]{x:T}"] {
            let source = format!("{declaration}\nfn probe(){{let x=R(x:1u8)}}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, &source);
            let output = engine.lower_to_tir(&path("probe")).unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            for name in ["R", "probe"] {
                let signature = engine
                    .signature(hirsel_syntax::DefId::intern(&path(name)))
                    .unwrap();
                signatures.insert(path(name), signature.as_ref().clone());
            }
            assert!(
                hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                    .violations
                    .is_empty()
            );
            let mut changed = output.value.as_ref().clone();
            let body = &mut changed.bodies[changed.entry.0 as usize];
            let mut removed = 0;
            for value in &mut body.rvalues {
                if let hirsel_tir::Rvalue::Construct {
                    constructor: hirsel_tir::Constructor::Record(_),
                    fields,
                    ..
                } = value
                {
                    removed += fields.len();
                    fields.clear();
                }
            }
            assert_eq!(removed, 1);
            changed.types = hirsel_tir::definition_types(engine.types(), &changed);
            let report =
                hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::Arity {
                        entity: "constructor fields",
                        expected: 1,
                        actual: 0
                    }
                )),
                "{declaration}: {report:?}"
            );
        }
    });
}

#[test]
fn call_argument_mapping_independently_rejects_missing_and_duplicate_ordinals() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(
            &mut engine,
            "fn rename(from x:u8,to y:u8){}\nfn probe(){rename(to:2,from:1)}",
        );
        let output = engine.lower_to_tir(&path("probe")).unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
        for name in ["rename", "probe"] {
            let signature = engine
                .signature(hirsel_syntax::DefId::intern(&path(name)))
                .unwrap();
            signatures.insert(path(name), signature.as_ref().clone());
        }
        assert!(
            hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        for duplicate in [false, true] {
            let mut changed = output.value.as_ref().clone();
            let body = &mut changed.bodies[changed.entry.0 as usize];
            let arguments = body
                .blocks
                .iter_mut()
                .find_map(|block| {
                    if let hirsel_tir::Terminator::Call { arguments, .. } = &mut block.terminator {
                        Some(arguments)
                    } else {
                        None
                    }
                })
                .unwrap();
            assert_eq!(arguments.len(), 2);
            if duplicate {
                arguments[1].parameter = arguments[0].parameter;
            } else {
                arguments.pop();
            }
            changed.types = hirsel_tir::definition_types(engine.types(), &changed);
            let report =
                hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::Arity {
                        entity: "call arguments",
                        ..
                    } | hirsel_validate::ViolationKind::Duplicate {
                        entity: "call parameter",
                        ..
                    }
                )),
                "{duplicate}: {report:?}"
            );
        }
    });
}

#[test]
fn branch_results_are_independently_checked_against_the_receiving_type() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        for branch in ["if b { 1u8 } else { 2 }", "match b { true=>1u8, false=>2 }"] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(
                &mut engine,
                &format!("fn probe(b:bool){{ let x={branch} }}"),
            );
            let output = engine.lower_to_tir(&path("probe")).unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            signatures.insert(
                path("probe"),
                engine
                    .signature(hirsel_syntax::DefId::intern(&path("probe")))
                    .unwrap()
                    .as_ref()
                    .clone(),
            );
            assert!(
                hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                    .violations
                    .is_empty()
            );
            let mut changed = output.value.as_ref().clone();
            let body = &mut changed.bodies[changed.entry.0 as usize];
            let value = body
                .rvalues
                .iter_mut()
                .find(|value| {
                    matches!(
                        value,
                        hirsel_tir::Rvalue::Constant(hirsel_tir::Constant::Integer(2))
                    )
                })
                .unwrap();
            *value = hirsel_tir::Rvalue::Constant(hirsel_tir::Constant::Bool(false));
            changed.types = hirsel_tir::definition_types(engine.types(), &changed);
            let report =
                hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| v.kind
                    == hirsel_validate::ViolationKind::TypeMismatch { entity: "constant" }),
                "{branch}: {report:?}"
            );
        }
    });
}

#[test]
fn match_payload_projections_keep_the_physical_type_and_validate_it_independently() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(
            &mut engine,
            "fn make()->Result[unique String,()]{Ok(\"x\")}\nfn probe(){match make(){Ok(text)=>(),Error(e)=>()}}",
        );
        let output = engine.lower_to_tir(&path("probe")).unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
        for name in ["make", "probe"] {
            signatures.insert(
                path(name),
                engine
                    .signature(hirsel_syntax::DefId::intern(&path(name)))
                    .unwrap()
                    .as_ref()
                    .clone(),
            );
        }
        let result = hirsel_syntax::DefPath::core("Result");
        signatures.insert(
            result.clone(),
            engine
                .signature(hirsel_syntax::DefId::intern(&result))
                .unwrap()
                .as_ref()
                .clone(),
        );
        assert!(
            hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        let mut changed = output.value.as_ref().clone();
        let body = &mut changed.bodies[changed.entry.0 as usize];
        let physical = body
            .places
            .iter()
            .position(|place| {
                place.projection.iter().any(|p| {
                    matches!(
                        p,
                        hirsel_tir::Projection::VariantField {
                            variant: 0,
                            field: 0
                        }
                    )
                })
            })
            .unwrap();
        let mut row = engine
            .types()
            .get(body.places[physical].ty)
            .unwrap()
            .clone();
        assert_eq!(row.modes.uniqueness, hirsel_types::Uniqueness::Unique);
        row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
        row.modes.sendability = hirsel_types::Sendability::NotSendable;
        body.places[physical].ty = engine.types().intern(row);
        changed.types = hirsel_tir::definition_types(engine.types(), &changed);
        let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| v.kind
                == hirsel_validate::ViolationKind::InvalidProjection(hirsel_tir::PlaceId(
                    physical as u32
                ))),
            "{report:?}"
        );
    });
}

#[test]
fn projected_unique_receipts_require_current_deep_owner_uniqueness() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(
            &mut engine,
            "type Rec {items:unique Bytes}\nfn need(take x:unique Bytes){}\nfn probe(take r:unique Rec){let Rec(items:items)=take r\n need(take items)}",
        );
        let output = engine.lower_to_tir(&path("probe")).unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
        for name in ["Rec", "need", "probe"] {
            signatures.insert(
                path(name),
                engine
                    .signature(hirsel_syntax::DefId::intern(&path(name)))
                    .unwrap()
                    .as_ref()
                    .clone(),
            );
        }
        assert!(
            hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        let mut copied = output.value.as_ref().clone();
        let entry = &mut copied.bodies[copied.entry.0 as usize];
        let first_take = entry
            .rvalues
            .iter_mut()
            .find_map(|value| match value {
                hirsel_tir::Rvalue::Use(operand)
                    if operand.usage == hirsel_types::UseKind::Take =>
                {
                    Some(operand)
                }
                _ => None,
            })
            .unwrap();
        first_take.usage = hirsel_types::UseKind::Copy;
        copied.types = hirsel_tir::definition_types(engine.types(), &copied);
        let report = hirsel_validate::validate(&copied, engine.types(), &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "value modes"
                }
            )),
            "copied owner: {report:?}"
        );

        let mut boundary = output.value.as_ref().clone();
        let entry = &mut boundary.bodies[boundary.entry.0 as usize];
        let projected = entry
            .places
            .iter()
            .position(|place| !place.projection.is_empty())
            .unwrap();
        let root = entry.places[projected].root;
        let original = entry.locals[root.0 as usize].ty;
        let mut row = engine.types().get(original).unwrap().clone();
        row.kind = hirsel_types::TypeKind::Builtin {
            kind: hirsel_types::BuiltinType::Shared,
            arguments: vec![hirsel_types::GenericArgument::Type(original)].into(),
        };
        let shared = engine.types().intern(row);
        entry.locals[root.0 as usize].ty = shared;
        for place in &mut entry.places {
            if place.root == root && place.projection.is_empty() {
                place.ty = shared;
            }
        }
        boundary.types = hirsel_tir::definition_types(engine.types(), &boundary);
        let report = hirsel_validate::validate(&boundary, engine.types(), &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| v.kind
                == hirsel_validate::ViolationKind::InvalidProjection(hirsel_tir::PlaceId(
                    projected as u32
                ))),
            "Shared boundary: {report:?}"
        );

        let mut changed = output.value.as_ref().clone();
        let roots:Vec<_>=changed.types.iter().map(|&id| (id, engine.types().row(id))).filter_map(|(id, row)|matches!(&row.kind,hirsel_types::TypeKind::Named{definition,..} if definition.path() == &path("Rec")).then_some(id)).collect();
        let mut replacements = std::collections::HashMap::new();
        for old in roots {
            let mut row = engine.types().get(old).unwrap().clone();
            row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
            replacements.insert(old, engine.types().intern(row));
        }
        for body in &mut changed.bodies {
            for local in &mut body.locals {
                if let Some(ty) = replacements.get(&local.ty) {
                    local.ty = *ty;
                }
            }
            for place in &mut body.places {
                if let Some(ty) = replacements.get(&place.ty) {
                    place.ty = *ty;
                }
            }
        }
        let mut signature = signatures.get(&path("probe")).unwrap().clone();
        let hirsel_types::SignatureKind::Function(function) = &signature.kind else {
            panic!()
        };
        let mut row = engine
            .types()
            .get(function.parameters[0].ty)
            .unwrap()
            .clone();
        row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
        let ty = engine.types().intern(row);
        let hirsel_types::SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        function.parameters[0].ty = ty;
        signatures.insert(path("probe"), signature);
        changed.types = hirsel_tir::definition_types(engine.types(), &changed);
        let report = hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "value modes"
                }
            )),
            "{report:?}"
        );
    });
}

#[test]
fn aggregate_uniqueness_requires_owned_unique_parts_independently() {
    hirsel_engine::with_compiler_stack(|| {
        for (source, nominal) in [
            (
                "type R{items:List[Bytes]}\nfn probe(take xs:List[Bytes]){let r=R(items:take xs)}",
                true,
            ),
            (
                "fn probe(take xs:List[Bytes]){let r:box List[Bytes]=box take xs}",
                false,
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, source);
            let output = engine.lower_to_tir(&path("probe")).unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            for name in if nominal {
                vec!["R", "probe"]
            } else {
                vec!["probe"]
            } {
                signatures.insert(
                    path(name),
                    engine
                        .signature(hirsel_syntax::DefId::intern(&path(name)))
                        .unwrap()
                        .as_ref()
                        .clone(),
                );
            }
            assert!(
                hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                    .violations
                    .is_empty()
            );
            if !nominal {
                let mut payload_contract = output.value.as_ref().clone();
                let roots: Vec<_> = payload_contract
                    .types
                    .iter()
                    .map(|&id| (id, engine.types().row(id)))
                    .filter_map(|(id, row)| {
                        matches!(row.kind, hirsel_types::TypeKind::Box(_)).then_some(id)
                    })
                    .collect();
                let mut replacements = std::collections::HashMap::new();
                for old in roots {
                    let mut row = engine.types().get(old).unwrap().clone();
                    let hirsel_types::TypeKind::Box(payload) = row.kind else {
                        unreachable!()
                    };
                    let mut part = engine.types().get(payload).unwrap().clone();
                    part.modes.uniqueness = hirsel_types::Uniqueness::Unique;
                    row.kind = hirsel_types::TypeKind::Box(engine.types().intern(part));
                    replacements.insert(old, engine.types().intern(row));
                }
                for body in &mut payload_contract.bodies {
                    for local in &mut body.locals {
                        if let Some(ty) = replacements.get(&local.ty) {
                            local.ty = *ty;
                        }
                    }
                    for place in &mut body.places {
                        if let Some(ty) = replacements.get(&place.ty) {
                            place.ty = *ty;
                        }
                    }
                }
                payload_contract.types =
                    hirsel_tir::definition_types(engine.types(), &payload_contract);
                let report = hirsel_validate::validate(
                    &payload_contract,
                    engine.types(),
                    &signatures,
                    &mut vec![],
                );
                assert!(
                    report.violations.iter().any(|v| matches!(
                        v.kind,
                        hirsel_validate::ViolationKind::TypeMismatch {
                            entity: "constructor field"
                        }
                    )),
                    "unique Box payload: {report:?}"
                );
            }
            let mut forged = output.value.as_ref().clone();
            let roots:Vec<_>=forged.types.iter().map(|&id| (id, engine.types().row(id))).filter_map(|(id, row)|(if nominal { matches!(&row.kind,hirsel_types::TypeKind::Named{definition,..}if definition.path() == &path("R")) } else { matches!(row.kind,hirsel_types::TypeKind::Box(_)) }).then_some(id)).collect();
            let mut replacements = std::collections::HashMap::new();
            for old in roots {
                let mut row = engine.types().get(old).unwrap().clone();
                row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
                replacements.insert(old, engine.types().intern(row));
            }
            for body in &mut forged.bodies {
                for local in &mut body.locals {
                    if let Some(ty) = replacements.get(&local.ty) {
                        local.ty = *ty;
                    }
                }
                for place in &mut body.places {
                    if let Some(ty) = replacements.get(&place.ty) {
                        place.ty = *ty;
                    }
                }
            }
            forged.types = hirsel_tir::definition_types(engine.types(), &forged);
            let report =
                hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::TypeMismatch {
                        entity: "unique construction"
                    }
                )),
                "{report:?}"
            );
        }
    });
}

#[test]
fn nonrecord_names_cannot_forge_nullary_record_constructions() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        for name in ["u8", "bool", "List"] {
            let source = format!("fn probe(){{let x={name}}}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("category.hir", source).unwrap();
            let diagnostics =
                check_snapshot(&mut engine, file, &default_context()).unwrap_or_else(|error| {
                    match error {
                        QueryError::Diagnostics(ds) => ds,
                        error => panic!("{name}: {error:?}"),
                    }
                });
            assert_eq!(diagnostics[0].rule.as_str(), "LANG-7");
            assert_eq!(engine.sources.slice(diagnostics[0].primary), Some(name));
        }
        for declaration in ["type S=u8", "type S=(u8,u8)", "type S=Only()"] {
            let source = format!("type Empty{{}}\n{declaration}\nfn probe(){{let x=Empty}}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, &source);
            let output = engine.lower_to_tir(&path("probe")).unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            for name in ["Empty", "S", "probe"] {
                let signature = engine
                    .signature(hirsel_syntax::DefId::intern(&path(name)))
                    .unwrap();
                signatures.insert(path(name), signature.as_ref().clone());
            }
            assert!(
                hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                    .violations
                    .is_empty()
            );
            let mut changed = output.value.as_ref().clone();
            let replacements: Vec<_> = changed.types.iter().map(|&id| (id, engine.types().row(id))).filter_map(|(id, row)| {
                if matches!(&row.kind, hirsel_types::TypeKind::Named {definition,..} if *definition == hirsel_syntax::DefId::intern(&path("Empty"))) {
                    let mut row = row.clone();
                    row.kind = hirsel_types::TypeKind::Named {definition: hirsel_syntax::DefId::intern(&path("S")), arguments:vec![].into()};
                    Some((id,row))
                } else { None }
            }).collect();
            let replacements: std::collections::HashMap<_, _> = replacements
                .into_iter()
                .map(|(old, row)| (old, engine.types().intern(row)))
                .collect();
            let body = &mut changed.bodies[changed.entry.0 as usize];
            for ty in body
                .locals
                .iter_mut()
                .map(|row| &mut row.ty)
                .chain(body.places.iter_mut().map(|row| &mut row.ty))
            {
                if let Some(new) = replacements.get(ty) {
                    *ty = *new;
                }
            }
            let mut constructed = 0;
            for value in &mut body.rvalues {
                if let hirsel_tir::Rvalue::Construct {
                    constructor: hirsel_tir::Constructor::Record(owner),
                    ..
                } = value
                {
                    *owner = path("S");
                    constructed += 1;
                }
            }
            assert_eq!(constructed, 1);
            changed.types = hirsel_tir::definition_types(engine.types(), &changed);
            let report =
                hirsel_validate::validate(&changed, engine.types(), &signatures, &mut vec![]);
            assert!(
                report
                    .violations
                    .iter()
                    .any(|v| v.kind == hirsel_validate::ViolationKind::InvalidConstructor),
                "{declaration}: {report:?}"
            );
        }
    });
}

#[test]
fn physical_projection_contracts_are_independently_checked() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        for (source, names, local) in [
            (
                "type R{xs:unique List[Bytes]}\nfn probe(take r:R){let built=box r.xs}",
                vec!["R", "probe"],
                false,
            ),
            (
                "type Pair{left:Bytes,right:Bytes}\nfn view(x:Bytes)->Bytes in x{copy x}\nfn bump(mut x:Bytes){}\nfn probe(mut x:Bytes,mut y:Bytes){let slot=Pair(left:view(x),right:view(y));let mut Pair(left:a,right:b)=take slot;a=b\"fresh\";bump(mut x)}",
                vec!["Pair", "view", "bump", "probe"],
                true,
            ),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, source);
            let output = engine.lower_to_tir(&path("probe")).unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            for name in names {
                signatures.insert(
                    path(name),
                    engine
                        .signature(hirsel_syntax::DefId::intern(&path(name)))
                        .unwrap()
                        .as_ref()
                        .clone(),
                );
            }
            assert!(
                hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                    .violations
                    .is_empty()
            );
            let mut forged = output.value.as_ref().clone();
            let body = &mut forged.bodies[forged.entry.0 as usize];
            let projected = body
                .places
                .iter()
                .position(|p| {
                    !p.projection.is_empty()
                        && if local {
                            engine
                                .types()
                                .get(p.ty)
                                .unwrap()
                                .modes
                                .relation
                                .has_environment()
                        } else {
                            engine.types().get(p.ty).unwrap().modes.uniqueness
                                == hirsel_types::Uniqueness::Unique
                        }
                })
                .unwrap();
            let mut row = engine
                .types()
                .get(body.places[projected].ty)
                .unwrap()
                .clone();
            if local {
                row.modes.relation = hirsel_types::Relation::global();
            } else {
                row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
            }
            body.places[projected].ty = engine.types().intern(row);
            forged.types = hirsel_tir::definition_types(engine.types(), &forged);
            let report =
                hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| v.kind
                    == hirsel_validate::ViolationKind::InvalidProjection(hirsel_tir::PlaceId(
                        projected as u32
                    ))),
                "{report:?}"
            );
        }
    });
}

#[test]
fn computed_part_shares_cannot_forge_unique_aggregate_receipts() {
    hirsel_engine::with_compiler_stack(|| {
        for value in ["box ({r}).xs", "(({r}).xs,1u8)"] {
            let source =
                format!("type R{{xs:unique List[Bytes]}}\nfn probe(take r:R){{let built={value}}}");
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, &source);
            let output = engine.lower_to_tir(&path("probe")).unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            for name in ["R", "probe"] {
                signatures.insert(
                    path(name),
                    engine
                        .signature(hirsel_syntax::DefId::intern(&path(name)))
                        .unwrap()
                        .as_ref()
                        .clone(),
                );
            }
            assert!(
                hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                    .violations
                    .is_empty()
            );
            let mut forged = output.value.as_ref().clone();
            let roots: Vec<_> = forged
                .types
                .iter()
                .map(|&id| (id, engine.types().row(id)))
                .filter_map(|(id, row)| match &row.kind {
                    hirsel_types::TypeKind::Box(_) => Some(id),
                    hirsel_types::TypeKind::Tuple(parts) if parts.len() == 2 => Some(id),
                    _ => None,
                })
                .collect();
            assert!(!roots.is_empty());
            let replacements: std::collections::HashMap<_, _> = roots
                .into_iter()
                .map(|old| {
                    let mut row = engine.types().get(old).unwrap().clone();
                    row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
                    (old, engine.types().intern(row))
                })
                .collect();
            for body in &mut forged.bodies {
                for ty in body
                    .locals
                    .iter_mut()
                    .map(|row| &mut row.ty)
                    .chain(body.places.iter_mut().map(|row| &mut row.ty))
                {
                    if let Some(new) = replacements.get(ty) {
                        *ty = *new;
                    }
                }
            }
            forged.types = hirsel_tir::definition_types(engine.types(), &forged);
            let report =
                hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::TypeMismatch {
                        entity: "unique construction"
                    }
                )),
                "{value}: {report:?}"
            );
        }
    });
}

#[test]
fn shared_handle_qualifiers_publish_canonical_copy_receipts() {
    hirsel_engine::with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        edit(
            &mut engine,
            "fn probe(take s:unique Shared[Bytes]){let copied=copy s}",
        );
        let signature = engine
            .signature(hirsel_syntax::DefId::intern(&path("probe")))
            .unwrap();
        let hirsel_types::SignatureKind::Function(header) = &signature.kind else {
            panic!()
        };
        assert_eq!(
            engine
                .types()
                .get(header.parameters[0].ty)
                .unwrap()
                .modes
                .uniqueness,
            hirsel_types::Uniqueness::Aliased
        );
        let output = engine.lower_to_tir(&path("probe")).unwrap();
        let signature = engine
            .signature(hirsel_syntax::DefId::intern(&path("probe")))
            .unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
        signatures.insert(path("probe"), signature.as_ref().clone());
        let report =
            hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![]);
        assert!(report.violations.is_empty(), "{report:?}");
        let mut forged = output.value.as_ref().clone();
        let body = &mut forged.bodies[forged.entry.0 as usize];
        let destination = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|instruction| match instruction.kind {
                hirsel_tir::InstructionKind::Assign { destination, value }
                    if matches!(
                        body.rvalues[value.0 as usize],
                        hirsel_tir::Rvalue::Use(hirsel_tir::Operand {
                            usage: hirsel_types::UseKind::Copy,
                            ..
                        })
                    ) =>
                {
                    Some(destination)
                }
                _ => None,
            })
            .unwrap();
        let place = &body.places[destination.0 as usize];
        let root = place.root;
        let mut row = engine.types().get(place.ty).unwrap().clone();
        assert_eq!(row.modes.uniqueness, hirsel_types::Uniqueness::Aliased);
        row.modes.uniqueness = hirsel_types::Uniqueness::Unique;
        let changed = engine.types().intern(row);
        body.locals[root.0 as usize].ty = changed;
        for place in &mut body.places {
            if place.root == root && place.projection.is_empty() {
                place.ty = changed;
            }
        }
        forged.types = hirsel_tir::definition_types(engine.types(), &forged);
        let report = hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "value modes"
                }
            )),
            "{report:?}"
        );
    });
}

#[test]
fn nested_view_calls_preserve_mut_handback_and_reject_forged_strengthening() {
    hirsel_engine::with_compiler_stack(|| {
        for mutable in [true, false] {
            let source = if mutable {
                "fn inspect(take x:box List[Bytes]){}\nfn probe(){let xs:List[Bytes]=[b\"x\"];let x=box unique xs;inspect(take x)}"
            } else {
                "fn inspect(x:box List[Bytes]){}\nfn probe(){let xs:List[Bytes]=[b\"x\"];let x:box List[Bytes]=box xs;inspect(x)}"
            };
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            edit(&mut engine, source);
            let output = engine.lower_to_tir(&path("probe")).unwrap();
            let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
            let mut inspect = engine
                .signature(hirsel_syntax::DefId::intern(&path("inspect")))
                .unwrap()
                .as_ref()
                .clone();
            signatures.insert(
                path("probe"),
                engine
                    .signature(hirsel_syntax::DefId::intern(&path("probe")))
                    .unwrap()
                    .as_ref()
                    .clone(),
            );
            signatures.insert(path("inspect"), inspect.clone());
            assert!(
                hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                    .violations
                    .is_empty()
            );
            let mut forged = output.value.as_ref().clone();
            let hirsel_types::SignatureKind::Function(header) = &mut inspect.kind else {
                panic!()
            };
            if mutable {
                header.parameters[0].passing = hirsel_syntax::PassingMode::Mut;
                let body = &mut forged.bodies[forged.entry.0 as usize];
                let argument = body
                    .blocks
                    .iter_mut()
                    .find_map(|block| match &mut block.terminator {
                        hirsel_tir::Terminator::Call { arguments, .. } => arguments.first_mut(),
                        _ => None,
                    })
                    .unwrap();
                argument.value.usage = hirsel_types::UseKind::Mut;
            } else {
                let mut boxed = engine.types().get(header.parameters[0].ty).unwrap().clone();
                let hirsel_types::TypeKind::Box(child) = boxed.kind else {
                    panic!()
                };
                let mut payload = engine.types().get(child).unwrap().clone();
                payload.modes.uniqueness = hirsel_types::Uniqueness::Unique;
                boxed.kind = hirsel_types::TypeKind::Box(engine.types().intern(payload));
                let ty = engine.types().intern(boxed);
                header.parameters[0].ty = hirsel_types::normalize_type_properties(
                    engine.types(),
                    ty,
                    &hirsel_types::GenericDeclarations::default(),
                    &mut |_| Ok::<_, QueryError>(None),
                )
                .unwrap();
            }
            signatures.insert(path("inspect"), inspect);
            forged.types = hirsel_tir::definition_types(engine.types(), &forged);
            let report =
                hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::TypeMismatch {
                        entity: "call argument"
                    }
                )),
                "mutable={mutable}: {report:?}"
            );
        }
    });
}

#[test]
fn tir_rejects_source_replacement_when_the_iteration_loan_is_still_live() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("last-use.hir", String::from(
            "fn value() { let mut s: Set[u64] = set.empty(); let empty: Set[u64] = set.empty(); for x in s { s = copy empty; break } }",
        )).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let definition = input
            .definitions
            .iter()
            .find(|(p, _)| p.0.name == "value")
            .unwrap()
            .1;
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        assert!(
            hirsel_validate::validate(definition, engine.types(), &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        let mut broken = definition.as_ref().clone();
        for body in &mut broken.bodies {
            let loan = body
                .blocks
                .iter()
                .find_map(|b| match &b.terminator {
                    hirsel_tir::Terminator::Call {
                        callee: hirsel_tir::Callee::Definition { path, .. },
                        arguments,
                        ..
                    } if path.0.name == "SetElements_open" => {
                        body.places[arguments[0].value.place.0 as usize].loan
                    }
                    _ => None,
                })
                .unwrap();
            for block in &mut body.blocks {
                block.instructions.retain(|i| {
                    !matches!(i.kind,
                    hirsel_tir::InstructionKind::EndLoan { loan: ending, .. } if ending == loan)
                });
            }
        }
        broken.types = hirsel_tir::definition_types(engine.types(), &broken);
        let rejected = hirsel_validate::validate(&broken, engine.types(), &signatures, &mut vec![]);
        assert!(
            rejected
                .violations
                .iter()
                .any(|v| matches!(v.kind, hirsel_validate::ViolationKind::LoanConflict { .. })),
            "{rejected:?}"
        );
    });
}
