//! Map/Set observations through the real pipeline, checked independently by H.
#[path = "../../../hirsel-driver/tests/support/source_std.rs"]
mod source_std;
use crate::rc_support as support;
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{Options, TargetSpec, build_program, manifest, program};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use hirsel_heapcheck::{Config, NoOperations, Observed};

fn execute(body: &str, expected: u64) {
    let source = format!("{body}\npub fn main() -> Result[(), u64] {{ Error(value()) }}\n");
    with_compiler_stack(move || {
        let mut engine = if source.contains("std@1/") {
            source_std::engine()
        } else {
            QueryEngine::new(default_environment(), compiler_adapters())
        };
        let file = engine.sources.add("map-test.hir", source.clone()).unwrap();
        let check = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            !check
                .diagnostics
                .iter()
                .any(|d| d.severity == hirsel_diag::Severity::Error),
            "{source}\n{:?}",
            check.diagnostics
        );
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let p = program(&input);
        let ready = build_program(
            &p,
            manifest(input.snapshot()).unwrap(),
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap_or_else(|e| panic!("{source}\n{e}"));
        let p = hirsel_heapcheck::Program {
            target: ready.target.clone(),
            layouts: ready.table.clone(),
            units: ready.units.iter().map(|u| u.unit().clone()).collect(),
            statics: ready.statics.clone(),
        };
        let campaign = hirsel_heapcheck::fault_campaign(
            &p,
            &support::path("main"),
            &[],
            &mut || Box::new(NoOperations),
            &Config::default(),
        );
        assert!(
            campaign.first_violation().is_none(),
            "{source}\n{:?}",
            campaign.first_violation()
        );
        let run = campaign.clean;
        let observation = run
            .result
            .unwrap_or_else(|v| panic!("{source}\n{v}"))
            .observation;
        assert!(
            observation.failures.is_empty(),
            "{source}\n{:?}",
            observation.failures
        );
        match observation.returned {
            Some(Observed::Variant { fields, .. }) => assert!(
                matches!(fields.first(), Some(Some(Observed::Integer {bits, ..})) if *bits == u128::from(expected)),
                "{fields:?}"
            ),
            other => panic!("{other:?}"),
        }
    });
}
#[test]
fn keyed_reads_and_replacement_preserve_positions() {
    execute(
        "fn value() -> u64 {\n  let mut m: Map[u64, u64] = map.empty()\n  map.insert(mut m, 2, 3)\n  map.insert(mut m, 1, 5)\n  map.insert(mut m, 2, 7)\n  let mut sum: u64 = 0\n  for (key, val) in m { sum = sum * 10 + val }\n  let v = match map.get(m, 2) { Some(x) => x, None => 0 }\n  sum + v\n}",
        82,
    );
}

#[test]
fn borrowed_set_iteration_can_replace_its_source_after_the_last_advance() {
    execute(
        "fn value() -> u64 { let mut s: Set[u64] = set.empty(); set.insert(mut s, 1); let mut empty: Set[u64] = set.empty(); set.insert(mut empty, 7); for x in s { s = copy empty; break }; let mut sum: u64 = 0; for x in s { sum += x }; sum }",
        7,
    );
    execute(
        "fn value() -> u64 { let mut s: Set[u64] = set.empty(); set.insert(mut s, 1); let empty: Set[u64] = set.empty(); for x in s { s = copy empty; return 42 }; 0 }",
        42,
    );
    execute(
        "fn value() -> u64 { let mut s: Set[u64] = set.empty(); set.insert(mut s, 1); let empty: Set[u64] = set.empty(); for x in s { break }; s = copy empty; let mut sum: u64 = 0; for x in s { sum += x }; sum }",
        0,
    );
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("next-use.hir", String::from(
        "fn probe() { let mut s: Set[u64] = set.empty(); let empty: Set[u64] = set.empty(); for x in s { s = copy empty; continue } }",
    )).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(
        checked.diagnostics[0].rule.as_str(),
        "MODE-1",
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn duplicate_set_insertion_and_contains() {
    execute(
        "fn value() -> u64 {\n  let mut s: Set[u64] = set.empty()\n  set.insert(mut s, 3)\n  set.insert(mut s, 5)\n  set.insert(mut s, 3)\n  let mut sum: u64 = 0\n  for x in s { sum = sum * 10 + x }\n  if set.contains(s, 3) && !set.contains(s, 8) { sum } else { 0 }\n}",
        35,
    );
}

#[test]
fn indexed_maps_grow_and_probe_without_changing_order() {
    let inserts = (0..22)
        .map(|i| format!("  map.insert(mut m, {i}, {})\n", i + 10))
        .collect::<String>();
    let source = "fn get_value(m: Map[u64, u64], k: u64) -> u64 {\n  match map.get(m, k) { Some(x) => x, None => 0 }\n}\nfn order(m: Map[u64, u64]) -> u64 {\n  let mut sum: u64 = 0\n  for (k, v) in m { sum = sum + k }\n  sum\n}\nfn value() -> u64 {\n  let mut m: Map[u64, u64] = map.empty()\n__INSERTS__\n  map.insert(mut m, 3, 100)\n  let sum = order(m)\n  let hit = get_value(m, 3)\n  let miss = get_value(m, 99)\n  sum + hit + miss\n}".replace("__INSERTS__", &inserts);
    execute(&source, 331);
}

#[test]
fn same_entries_and_nested_set_keys_ignore_only_set_order() {
    execute(
        "fn value() -> u64 {\n let mut a: Map[u64, u64] = map.empty()\n let mut b: Map[u64, u64] = map.empty()\n map.insert(mut a, 1, 10)\n map.insert(mut a, 2, 20)\n map.insert(mut b, 2, 20)\n map.insert(mut b, 1, 10)\n let mut x: Set[u64] = set.empty()\n let mut y: Set[u64] = set.empty()\n set.insert(mut x, 1)\n set.insert(mut x, 2)\n set.insert(mut y, 2)\n set.insert(mut y, 1)\n let mut outer: Set[Set[u64]] = set.empty()\n set.insert(mut outer, copy x)\n set.insert(mut outer, copy y)\n if a != b && std@1/map.same_entries(a, b) && x == y && set.contains(outer, y) { 1 } else { 0 }\n}",
        1,
    );
}
#[test]
fn consuming_map_cursor_transfers_before_break() {
    execute(
        "fn value() -> u64 {\n let mut m: Map[u64, u64] = map.empty()\n map.insert(mut m, 2, 20)\n map.insert(mut m, 1, 10)\n let mut acc: u64 = 0\n for (k, v) in take m { acc = k + v; break }\n acc\n}",
        22,
    );
}
#[test]
fn borrowed_set_cursor_releases_iteration_loan_on_break_and_return() {
    execute(
        "fn first(s: Set[u64]) -> u64 {\n for x in s { return x }\n 0\n}\nfn value() -> u64 {\n let mut s: Set[u64] = set.empty()\n set.insert(mut s, 4)\n set.insert(mut s, 5)\n let mut acc: u64 = 0\n for x in s { acc = x; break }\n acc + first(s)\n}",
        8,
    );
}

#[test]
fn static_containers_probe_and_nested_keys_materialize() {
    execute(
        "const M: Map[u64, u64] = build()\nfn build() -> Map[u64,u64] { let mut m: Map[u64,u64] = map.empty()\n map.insert(mut m, 2, 20)\n map.insert(mut m, 1, 10)\n take m }\nconst S: Set[Map[u64,u64]] = build_set()\nfn build_set() -> Set[Map[u64,u64]] { let mut s: Set[Map[u64,u64]] = set.empty()\n set.insert(mut s, copy M)\n take s }\nfn value() -> u64 { if set.contains(S, M) { match map.get(M, 2) { Some(x) => x, None => 0 } } else { 0 } }",
        20,
    );
}

#[test]
fn consuming_shared_and_static_maps_preserves_other_holders() {
    execute(
        "fn build() -> Map[u64,u64] { let mut m: Map[u64,u64] = map.empty()\n map.insert(mut m, 1, 10)\n map.insert(mut m, 2, 20)\n take m }\nconst M: Map[u64,u64] = build()\nfn consume(take m: Map[u64,u64]) -> u64 { let mut acc: u64 = 0\n for (k,v) in take m { acc = acc + v; break }\n acc }\nfn value() -> u64 { let a = build()\n let b = copy a\n let first = consume(take a)\n let second = consume(take b)\n let third = consume(copy M)\n let last = match map.get(M, 2) { Some(x) => x, None => 0 }\n first + second + third + last }",
        50,
    );
}

#[test]
fn heap_independently_rejects_undeclared_borrowed_returns_and_early_loan_end() {
    use hirsel_lir::ir::{Inst, PoolRange, Terminator};
    let source = "pub fn main() -> Result[(),u64] { let mut s: Set[u64] = set.empty()\n set.insert(mut s, 7)\n for x in s { return Error(x) }\n Ok(()) }";
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("borrowed-return.hir", source.into())
            .unwrap();
        check_progress(&mut engine, file, &default_context()).unwrap();
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let ready = hirsel_driver::native::build(
            &input,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let baseline = hirsel_heapcheck::Program {
            target: ready.target,
            layouts: ready.table,
            units: ready.units.iter().map(|u| u.unit().clone()).collect(),
            statics: ready.statics,
        };
        for mutation in 0..4 {
            let mut p = baseline.clone();
            let open = p
                .units
                .iter_mut()
                .find(|u| u.header.definition.0.name == "SetElements_open")
                .unwrap();
            if mutation == 1 {
                p.layouts.fn_abis[open.header.abi.0 as usize]
                    .result
                    .as_mut()
                    .unwrap()
                    .borrowed_parameters
                    .clear();
            }
            if mutation == 3 {
                // Two fields retain the same loan. Both must be checked before
                // the callee retires it; the caller then keeps both views alive.
                let layout = p.layouts.fn_abis[open.header.abi.0 as usize]
                    .result
                    .as_ref()
                    .unwrap()
                    .layout;
                let record = &mut p.layouts.layouts[layout.0 as usize];
                record.fields[2].layout = record.fields[0].layout;
                record
                    .scan
                    .push(hirsel_lir::layout_types::ScanStep::Uncounted {
                        offset: record.fields[2].offset,
                    });
                for site in &mut open.tables.constructs {
                    if site.layout == layout {
                        let fields = site.fields.get(&open.pools.fields).unwrap();
                        let source = fields.iter().find(|f| f.field == 0).unwrap().value;
                        let mut fields = fields.to_vec();
                        fields.iter_mut().find(|f| f.field == 2).unwrap().value = source;
                        site.fields = PoolRange::push(&mut open.pools.fields, fields);
                    }
                }
            }
            if mutation == 2 {
                let loan = open
                    .instructions
                    .iter()
                    .find_map(|i| {
                        if let Inst::BeginLoan { loan, .. } = i {
                            Some(*loan)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let mut instructions = Vec::new();
                for (i, block) in open.blocks.iter_mut().enumerate() {
                    let mut code = block.instructions.get(&open.instructions).unwrap().to_vec();
                    if matches!(open.terminators[i], Terminator::Return(_)) {
                        code.push(Inst::EndLoan {
                            loan,
                            if_active: false,
                        });
                    }
                    block.instructions = PoolRange::push(&mut instructions, code);
                }
                open.instructions = instructions;
            }
            let run = hirsel_heapcheck::run(
                &p,
                &support::path("main"),
                &[],
                &mut NoOperations,
                &Config::default(),
            );
            match mutation {
                0 | 3 => assert!(run.result.is_ok(), "{:?}", run.result.err()),
                1 => assert_eq!(
                    run.violation(),
                    Some(hirsel_heapcheck::ViolationKind::BorrowedReturn),
                    "{:?}",
                    run.result.as_ref().err()
                ),
                2 => assert_eq!(
                    run.violation(),
                    Some(hirsel_heapcheck::ViolationKind::BorrowExpired),
                    "{:?}",
                    run.result.as_ref().err()
                ),
                _ => unreachable!(),
            }
        }
    });
}

#[test]
fn insertion_detaches_a_static_container_and_retains_its_constant() {
    execute(
        "fn build() -> Map[u64,u64] { let mut m: Map[u64,u64] = map.empty()\n map.insert(mut m, 1, 10)\n take m }\nconst M: Map[u64,u64] = build()\nfn get_value(m: Map[u64,u64], k: u64) -> u64 { match map.get(m,k) { Some(x) => x, None => 0 } }\nfn value() -> u64 { let mut m = copy M\n map.insert(mut m, 1, 30)\n map.insert(mut m, 2, 20)\n let first = get_value(m,1)\n let second = get_value(M,1)\n first + second }",
        40,
    );
}

#[test]
fn conditional_borrowed_cursor_sources_keep_the_selected_loan() {
    execute(
        "fn first(a: Set[u64], b: Set[u64], choose: bool) -> u64 { for x in (if choose { a } else { b }) { return x }\n 0 }\nfn value() -> u64 { let mut a: Set[u64] = set.empty()\n let mut b: Set[u64] = set.empty()\n set.insert(mut a, 7)\n set.insert(mut b, 9)\n first(a,b,true) }",
        7,
    );
}

#[test]
fn a_shallow_detached_static_map_copies_entries_while_a_fresh_map_moves_them() {
    execute(
        "fn build() -> Map[u64,u64] { let mut m: Map[u64,u64] = map.empty()\n map.insert(mut m, 1, 10)\n map.insert(mut m, 2, 20)\n take m }\nconst M: Map[u64,u64] = build()\nfn inspect(mut m: Map[u64,u64]) { for (k,v) in m { let _ = v } }\nfn first(take m: Map[u64,u64]) -> u64 { for (k,v) in take m { return v }\n 0 }\nfn get_value(m: Map[u64,u64]) -> u64 { match map.get(m,1) { Some(v) => v, None => 0 } }\nfn value() -> u64 { let mut m = copy M\n inspect(mut m)\n let copied = first(take m)\n let moved = first(build())\n copied + moved + get_value(M) }",
        30,
    );
}

fn compile_cursor_case(source: &str) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("cursor-case.hir", source.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let p = program(&input);
        build_program(
            &p,
            manifest(input.snapshot()).unwrap(),
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
    });
}

#[test]
fn a_local_consuming_set_cursor_can_copy_its_last_item() {
    compile_cursor_case(include_str!(
        "../../../../conformance/MODE/MODE-1-local-cursor-item-copy-run.hir"
    ));
}

#[test]
fn a_consuming_resource_map_cursor_drops_its_remaining_entries_on_break() {
    compile_cursor_case(include_str!(
        "../../../../conformance/SEM/SEM-4-map-resource-consume-break-run.hir"
    ));
}

#[test]
fn a_borrowed_set_cursor_copies_counted_string_elements() {
    execute(
        "fn value() -> u64 { let mut s: Set[String] = set.empty(); set.insert(mut s, \"abc\"); let mut n: u64 = 0; for x in s { n += string.byte_len(x) }; n }",
        3,
    );
}

#[test]
fn a_consuming_map_cursor_drops_a_resource_with_counted_fields() {
    execute(
        "resource type R { text: String } drop(x) {}\nfn value() -> u64 { let mut m: Map[u8,R] = map.empty(); map.insert(mut m, 1, R(text: \"abc\")); for (k,v) in take m { let _ = string.byte_len(v.text); break }; 0 }",
        0,
    );
}

#[test]
fn a_counted_string_copy_returned_from_a_borrow_parameter_has_no_view() {
    execute(
        "fn view(x: String) -> String in x { x }\nfn value() -> u64 { string.byte_len(view(\"abc\")) }",
        3,
    );
}

#[test]
fn copying_variant_keys_with_and_without_holders_preserves_their_active_arm() {
    execute(
        "type Chain = End | Link(next: box Chain)
fn value() -> u64 { let mut m: Map[Chain,u64] = map.empty(); map.insert(mut m, End, 1); match map.get(m, End) { Some(v) => v, None => 0 } }",
        1,
    );
    execute(
        "type Key = Plain(value: u64) | Owned(value: box u64)\nfn value() -> u64 { let mut m: Map[Key,u64] = map.empty(); map.insert(mut m, Plain(value: 7), 11); map.insert(mut m, Owned(value: box 7), 13); let a = match map.get(m, Plain(value: 7)) { Some(v) => v, None => 0 }; let b = match map.get(m, Owned(value: box 7)) { Some(v) => v, None => 0 }; a + b }",
        24,
    );
}

#[test]
fn a_niche_key_after_a_plain_field_uses_its_declared_tag_offset() {
    execute(
        "type Key = End | Node(number: u64, pointer: box u64)\nfn value() -> u64 { let mut m: Map[Key,u64] = map.empty(); map.insert(mut m, Node(number: 7, pointer: box 8), 13); match map.get(m, Node(number: 7, pointer: box 8)) { Some(v) => v, None => 0 } }",
        13,
    );
}
