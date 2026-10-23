//! MEM-4 source grant controls and reachable independent V validation.
use hirsel_driver::*;
use hirsel_engine::*;

fn check(source: &str, expected: Option<&str>, export: bool) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("sealed-regions-33.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    if let Some(rule) = expected {
        assert!(
            errors.iter().any(|d| d.rule.as_str() == rule),
            "{source}\n{errors:?}"
        );
    } else {
        assert!(errors.is_empty(), "{source}\n{errors:?}");
        if export {
            let input = engine.export_native_input().unwrap();
            engine.import_native_input(input).unwrap();
        }
    }
}

#[test]
fn sealed_regions_33_contextual_scalar_grant() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){let r = arena.with(fn(a: Arena)->Region[u64]{let root=7u64;arena.seal(take a,take root)});}",
            None,
            true,
        )
    });
}

#[test]
fn sealed_regions_33_borrow_only_callback_preserved() {
    with_compiler_stack(|| {
        check(
            "fn read(a:Arena)->u64{7u64}\npub fn main(){let n=arena.with(read);}",
            None,
            true,
        )
    });
}

#[test]
fn sealed_regions_33_ordinary_borrow_is_not_a_grant() {
    with_compiler_stack(|| {
        check(
            "fn bad(a:Arena)->Region[u64]{let root=7u64;arena.seal(take a,take root)}\npub fn main(){}",
            Some("OWN-1"),
            false,
        )
    });
}

#[test]
fn sealed_regions_33_consumed_grant_stays_dead() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){arena.with(fn(a:Arena){let root=7u64;let r=arena.seal(take a,take root);let xs=copy_to(a,[1u64]);});}",
            Some("OWN-2"),
            false,
        )
    });
}

#[test]
fn sealed_regions_33_grant_forwards_through_take_helper() {
    with_compiler_stack(|| {
        check(
            "fn seal(take a:Arena in a)->Region[u64]{let root=7u64;arena.seal(take a,take root)}\npub fn main(){let r=arena.with(fn(a:Arena)->Region[u64]{seal(take a)});}",
            None,
            true,
        )
    });
}

#[test]
fn sealed_regions_33_root_and_alias_source_controls() {
    with_compiler_stack(|| {
        for root in [
            "let root: List[u64] in heap=[7u64];",
            "let root: List[u64] in a=[7u64];",
        ] {
            let source = format!(
                "pub fn main(){{let r=arena.with(fn(a:Arena)->Region[List[u64]]{{{root}arena.seal(take a,take root)}});}}"
            );
            check(&source, None, true);
        }
        check(
            "pub fn main(){let r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];let alias=copy root;let r=arena.seal(take a,take root);let n=list.len(alias);r});}",
            Some("MEM-4"),
            false,
        );
        check(
            "pub fn main(){let r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];let alias=copy root;let n=list.len(alias);arena.seal(take a,take root)});}",
            None,
            true,
        );
        check(
            "pub fn main(){let root=7u64;let r=arena.seal(heap,take root);}",
            Some("MEM-4"),
            false,
        );
    });
}

#[test]
fn sealed_regions_33_region_read_source_controls() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in heap=[7u64];arena.seal(take a,take root)});let n=region.with(mut r,fn(root:List[u64])->u64{list.len(root)});}",
            None,
            true,
        );
        check(
            "pub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in heap=[7u64];arena.seal(take a,take root)});let escaped=region.with(mut r,fn(root:List[u64])->List[u64]{root});}",
            Some("MODE-1"),
            false,
        );
    });
}

#[test]
fn sealed_regions_33_resource_payload_depths() {
    with_compiler_stack(|| {
        for root in [
            "R(n:7u64)",
            "Some(R(n:7u64))",
            "box R(n:7u64)",
            "[R(n:7u64)]",
            "Wrap(value:R(n:7u64))",
            "fn()->u64{let owned=take payload;owned.n}",
        ] {
            let setup = if root.starts_with("fn") {
                "let payload=R(n:7u64);"
            } else {
                ""
            };
            let source = format!(
                "resource type R {{n:u64}}\ndrop(x) {{}}\ntype Wrap[T]{{value:T}}\npub fn main(){{arena.with(fn(a:Arena){{{setup}let root={root};let r=arena.seal(take a,take root);}});}}"
            );
            check(&source, Some("MEM-4"), false);
        }
    });
}

#[test]
fn sealed_regions_33_forwarded_heap_is_not_a_grant() {
    with_compiler_stack(|| {
        check(
            "fn seal(take a:Arena in a)->Region[u64]{let root=7u64;arena.seal(take a,take root)}\npub fn main(){let r=seal(heap);}",
            Some("MEM-4"),
            false,
        )
    });
}

#[test]
fn sealed_regions_33_seal_allocation_effect() {
    with_compiler_stack(|| {
        check(
            "@noalloc fn bad(take a:Arena in a)->Region[u64]{let root=7u64;arena.seal(take a,take root)}\npub fn main(){}",
            Some("MEM-5"),
            false,
        );
        check(
            "pub fn main(){let r=arena.with(fn(a:Arena)->Region[u64]{let root=7u64;let r=arena.seal(take a,take root);let other=8u64;let next=arena.seal(take a,take other);r});}",
            Some("OWN-2"),
            false,
        );
    });
}

#[test]
fn sealed_regions_33_owned_callable_environment() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){arena.with(fn(a:Arena){let payload:List[u64] in heap=[7u64];let root:once fn()->u64=fn()->u64{let owned=take payload;list.len(owned)};let r=arena.seal(take a,take root);});}",
            None,
            true,
        );
        check(
            "pub fn main(){arena.with(fn(a:Arena){let payload:List[u64] in heap=[7u64];let root:fn()->u64=fn()->u64{list.len(payload)};let r=arena.seal(take a,take root);});}",
            None,
            true,
        );
    });
}

#[test]
fn sealed_regions_33_many_callable_mode_control() {
    with_compiler_stack(|| {
        check(
            "fn many(f:fn()->u64)->u64{f()}\npub fn main(){let f:fn()->u64=fn()->u64{7u64};let n=many(f);}",
            None,
            true,
        )
    });
}

#[test]
fn sealed_regions_33_once_callable_stays_once_for_ordinary_receiver() {
    with_compiler_stack(|| {
        check(
            "fn many(f:fn()->u64)->u64{f()}\npub fn main(){let payload:Bytes=b\"x\";let f:once fn()->u64=fn()->u64{let owned=take payload;bytes.len(owned)};let n=many(f);}",
            Some("MODE-3"),
            false,
        )
    });
}

#[test]
fn sealed_regions_33_generic_two_module_importer() {
    with_compiler_stack(|| {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sealed_regions_33/main.hir.json");
        let mut program = load_program(&manifest).unwrap();
        program.check_for_execution().unwrap();
        let input = program.engine.export_native_input().unwrap();
        program.engine.import_native_input(input).unwrap();
    });
}

#[test]
fn sealed_regions_33_read_callback_closure_cannot_retain_root() {
    with_compiler_stack(|| {
        let prefix = "pub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in heap=[7u64];arena.seal(take a,take root)});";
        check(
            &format!(
                "{prefix}let f=region.with(mut r,fn(root:List[u64])->fn()->u64{{let n=list.len(root);fn()->u64{{n}}}});}}"
            ),
            None,
            true,
        );
        check(
            &format!(
                "{prefix}let f=region.with(mut r,fn(root:List[u64])->fn()->u64{{fn()->u64{{list.len(root)}}}});}}"
            ),
            Some("CLO-4"),
            false,
        );
    });
}

#[test]
fn sealed_regions_33_read_root_cannot_be_taken() {
    with_compiler_stack(|| {
        let prefix = "fn consume(take xs:List[u64]){}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in heap=[7u64];arena.seal(take a,take root)});";
        check(
            &format!(
                "{prefix}region.with(mut r,fn(root:List[u64]){{let independent=copy_to(heap,root);consume(take independent);}});}}"
            ),
            None,
            true,
        );
        check(
            &format!("{prefix}region.with(mut r,fn(root:List[u64]){{consume(take root);}});}}"),
            Some("OWN-1"),
            false,
        );
    });
}

#[test]
fn sealed_regions_33_read_callback_allocation_effect() {
    with_compiler_stack(|| {
        check(
            "@noalloc fn len(root:List[u64])->u64{list.len(root)}\n@noalloc fn read(mut r:Region[List[u64]])->u64{region.with(mut r,len)}\npub fn main(){}",
            None,
            true,
        );
        check(
            "@noalloc fn read(mut r:Region[List[u64]])->List[u64]{region.with(mut r,fn(root:List[u64])->List[u64]{copy_to(heap,root)})}\npub fn main(){}",
            Some("MEM-5"),
            false,
        );
    });
}

#[test]
fn sealed_regions_33_captured_arena_alias_liveness() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){let r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];let alias=copy root;let later=fn()->u64{list.len(alias)};let n=later();arena.seal(take a,take root)});}",
            None,
            true,
        );
        check(
            "pub fn main(){let r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];let alias=copy root;let later=fn()->u64{list.len(alias)};let r=arena.seal(take a,take root);let n=later();r});}",
            Some("MEM-4"),
            false,
        );
    });
}

fn read_capture_envelope(result: &str, envelope: &str) {
    with_compiler_stack(|| {
        let prefix = "type Holder{callback:fn()->u64}\ntype Choice=Empty|Held(callback:fn()->u64)\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in heap=[7u64];arena.seal(take a,take root)});";
        let good = envelope.replace("VALUE", "n");
        let bad = envelope.replace("VALUE", "list.len(root)");
        check(
            &format!(
                "{prefix}let escaped=region.with(mut r,fn(root:List[u64])->{result}{{let n=list.len(root);{good}}});}}"
            ),
            None,
            true,
        );
        check(
            &format!(
                "{prefix}let escaped=region.with(mut r,fn(root:List[u64])->{result}{{{bad}}});}}"
            ),
            Some("CLO-4"),
            false,
        );
    });
}
#[test]
fn sealed_regions_33_read_record_capture_envelope() {
    read_capture_envelope("Holder", "Holder(callback:fn()->u64{VALUE})");
}
#[test]
fn sealed_regions_33_read_variant_capture_envelope() {
    read_capture_envelope("Choice", "Held(callback:fn()->u64{VALUE})");
}
#[test]
fn sealed_regions_33_read_list_capture_envelope() {
    read_capture_envelope("List[Holder]", "[Holder(callback:fn()->u64{VALUE})]");
}

#[test]
fn sealed_regions_33_read_root_cannot_be_mutated() {
    with_compiler_stack(|| {
        let prefix = "pub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in heap=[7u64];arena.seal(take a,take root)});";
        check(
            &format!(
                "{prefix}region.with(mut r,fn(root:List[u64]){{let mut independent=copy_to(heap,root);list.push(mut independent,4u64);}});}}"
            ),
            None,
            true,
        );
        check(
            &format!(
                "{prefix}region.with(mut r,fn(root:List[u64]){{list.push(mut root,4u64);}});}}"
            ),
            Some("OWN-1"),
            false,
        );
    });
}

#[test]
fn sealed_regions_33_region_is_noncopyable() {
    with_compiler_stack(|| {
        let prefix = "pub fn main(){let r=arena.with(fn(a:Arena)->Region[u64]{let root=7u64;arena.seal(take a,take root)});";
        check(&format!("{prefix}let moved=take r;}}"), None, true);
        check(
            &format!("{prefix}let copied=copy r;}}"),
            Some("OWN-5"),
            false,
        );
    });
}

#[test]
fn sealed_regions_33_region_task_mode_evidence() {
    with_compiler_stack(|| {
        for (root_type, root_value, rule) in [
            ("u64", "7u64", None),
            ("unique List[u64]", "unique [7u64]", None),
            ("List[u64]", "[7u64]", Some("CLO-3")),
        ] {
            check(
                &format!(
                    "fn cross(tasks:Tasks)->u64{{task.scope(tasks,fn(s:Scope)->u64{{let r:Region[{root_type}]=arena.with(fn(a:Arena)->Region[{root_type}]{{let root:{root_type}={root_value};arena.seal(take a,take root)}});let child=task.async(s,fn()->u64{{let mut owned=take r;region.with(mut owned,fn(view:{root_type})->u64{{7u64}})}});task.await(s,take child)}})}}\npub fn main(){{}}"
                ),
                rule,
                rule.is_none(),
            );
        }
    });
}

#[test]
fn sealed_regions_33_nested_scope_may_consume_an_outer_owned_grant() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){let r=arena.with(fn(outer:Arena)->Region[List[u64]]{let root:List[u64] in outer=[7u64];arena.with(fn(inner:Arena)->Region[List[u64]]{let owned=arena.seal(take outer,take root);let marker:List[u64] in inner=[1u64];let n=list.len(marker);owned})});}",
            None,
            true,
        );
    });
}
#[test]
fn sealed_regions_33_tuple_payload_keeps_its_designated_arena_proof() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){let r=arena.with(fn(a:Arena)->Region[(List[u64],u64)]{let xs:List[u64] in a=[7u64];let root=(take xs,1u64);arena.seal(take a,take root)});}",
            None,
            true,
        )
    });
}
#[test]
fn sealed_regions_33_tuple_payload_cannot_hide_a_foreign_arena() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){arena.with(fn(a:Arena){arena.with(fn(b:Arena){let xs:List[u64] in b=[7u64];let root=(take xs,1u64);let r=arena.seal(take a,take root);});});}",
            Some("MEM-4"),
            false,
        )
    });
}
#[test]
fn sealed_regions_33_many_scope_callback_is_received_by_once_formal() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){let callback:fn(Arena)->u64=fn(a:Arena)->u64{7u64};let n=arena.with(callback);}",
            None,
            true,
        )
    });
}

#[test]
fn sealed_regions_33_sorted_root_keeps_its_closed_comparator_brand() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){let r=arena.with(fn(a:Arena)->Region[SortedMap[u64,Bytes] of u64.order]{let mut root:SortedMap[u64,Bytes] of u64.order in a=sorted_map.empty(u64.order);sorted_map.insert(u64.order,mut root,7u64,copy_to(heap,b\"abc\"));arena.seal(take a,take root)});}",
            None,
            true,
        );
    });
}

#[test]
fn sealed_regions_33_sorted_root_does_not_change_comparator_identity() {
    with_compiler_stack(|| {
        check(
            "fn reverse(a:u64,b:u64)->i32{if a<b{1i32}else{if a>b{-1i32}else{0i32}}}\npub fn main(){let wrong=Order(compare:reverse);let mut r=arena.with(fn(a:Arena)->Region[SortedMap[u64,Bytes] of u64.order]{let mut root:SortedMap[u64,Bytes] of u64.order in a=sorted_map.empty(u64.order);sorted_map.insert(u64.order,mut root,7u64,b\"abc\");arena.seal(take a,take root)});let n=region.with(mut r,fn(view:SortedMap[u64,Bytes] of u64.order)->u64{sorted_map.len(wrong,view)});}",
            Some("PROV-1"),
            false,
        );
    });
}
