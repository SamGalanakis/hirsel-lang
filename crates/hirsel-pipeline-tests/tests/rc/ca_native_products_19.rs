//! Actual compiler controls for portable local native products. These tests
//! exercise independent Value/Ready validation, not a producer-only oracle.
use crate::rc_support;
use hirsel_lir::products::{Binding, LayoutClosure, UnitBody};
use hirsel_lir::stage::{Stage, Value};
use hirsel_lir::validate::{Inputs, UnitInputs};

fn roundtrip<S: Stage>(
    units: Vec<hirsel_lir::stage::ValidatedUnit<S>>,
    inputs: &Inputs<'_>,
) -> Vec<String> {
    let products: Vec<_> = units
        .iter()
        .map(|u| {
            let (closure, body) = LayoutClosure::capture(u, inputs, &[]).unwrap();
            closure.check(inputs.target).unwrap();
            let closure: LayoutClosure =
                hirsel_codec::from_text(&hirsel_codec::to_text(&closure)).unwrap();
            let body: UnitBody = hirsel_codec::from_text(&hirsel_codec::to_text(&body)).unwrap();
            (closure, body)
        })
        .collect();
    let closures: Vec<_> = products.iter().map(|(c, _)| c).collect();
    let binding = Binding::new(inputs.target, &closures).unwrap();
    let current = Inputs {
        memo: Default::default(),
        target: inputs.target,
        layouts: &binding.table,
        manifest: inputs.manifest,
        operations: inputs.operations,
        discharge: inputs.discharge,
    };
    for (_, body) in &products {
        let raw = binding.materialize::<S>(body).unwrap();
        let checked = hirsel_lir::validate::validate(raw, &current).unwrap();
        let (_, body_again) = LayoutClosure::capture(&checked, &current, &[]).unwrap();
        assert_eq!(
            *body, body_again,
            "canonical body must survive table relocation"
        );
        // The complete content-named body is the contract; numeric aliases
        // in the original program table may legitimately merge when binding.
        assert!(UnitInputs::from(&current).digest(checked.unit()).is_ok());
    }
    // The same closure repeated must preserve cyclic component multiplicity
    // and leave bindings/content keys unchanged.
    let duplicate: Vec<_> = closures.iter().chain(&closures).copied().collect();
    assert_eq!(
        Binding::new(inputs.target, &duplicate).unwrap().table,
        binding.table
    );
    products
        .iter()
        .map(|(c, b)| format!("{}\n{}", hirsel_codec::to_text(c), hirsel_codec::to_text(b)))
        .collect()
}

#[test]
fn native_products_relocate_recursive_layouts_closures_and_finalizers() {
    for source in [
        "resource type Token { n:u64 }\ndrop(x) { () }\nfn main() { let t=Token(n:1u64); }",
        "type Node = End | More(next: box Node)\nfn make()->box Node { box More(next:box End()) }\nfn main() { let n=make(); }",
        "type Captured { n:u64 }\nfn compute(x:Captured)->u64 { let f=fn()->u64 { x.n }; f() }\nfn main()->u64 { compute(Captured(n:3u64)) }",
        "type Item { n:u64 }\nfn first(a:Arena,n:u64)->(box Item) in a { if n==0u64 { box Item(n:6u64) } else { second(a,n-1u64) } }\nfn second(a:Arena,n:u64)->(box Item) in a { first(a,n) }\nfn main() { arena.with(fn(a:Arena) { let x=second(a,2u64); }); }",
    ] {
        let program = rc_support::compile(source);
        let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
        let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
        assert_eq!(lowered.unit_keys.len(), lowered.units.len());
        for (unit, key) in lowered.units.iter().zip(&lowered.unit_keys) {
            assert_eq!(unit.header.instance, key.digest());
            assert_eq!(&unit.header.definition, key.definition());
        }
        let manifest = rc_support::manifest(&program);
        let operations = program.operation_registry();
        let inputs = Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &lowered.table,
            manifest: &manifest,
            operations: &operations,
            discharge: Some(&lowered.native_discharge),
        };
        let units = lowered
            .units
            .iter()
            .cloned()
            .map(|u| hirsel_lir::validate::validate_value(u, &inputs).unwrap())
            .collect();
        roundtrip::<Value>(units, &inputs);
        let ready = hirsel_driver::native::build_program(
            &program,
            manifest.clone(),
            &hirsel_driver::native::Options::new(target.clone()),
        )
        .unwrap();
        let inputs = Inputs {
            memo: Default::default(),
            target: &target,
            layouts: &ready.table,
            manifest: &manifest,
            operations: &operations,
            discharge: Some(&ready.discharged),
        };
        roundtrip(ready.units.clone(), &inputs);
    }
}

#[test]
fn native_products_reject_changed_input_rows_and_corrupt_graph_keys() {
    let program = rc_support::compile("fn main()->u64 { 7u64 }");
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
    let manifest = rc_support::manifest(&program);
    let operations = program.operation_registry();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let checked = hirsel_lir::validate::validate_value(lowered.units[0].clone(), &inputs).unwrap();
    let (mut closure, mut body) = LayoutClosure::capture(&checked, &inputs, &[]).unwrap();
    closure.layouts[0] = hirsel_syntax::ContentHash::default();
    assert!(closure.check(&target).is_err());
    assert!(Binding::new(&target, &[&closure]).is_err());
    let (closure, _) = LayoutClosure::capture(&checked, &inputs, &[]).unwrap();
    let binding = Binding::new(&target, &[&closure]).unwrap();
    body.abis[0] = hirsel_syntax::ContentHash::default();
    assert!(binding.materialize::<Value>(&body).is_err());
    let mut foreign_table = lowered.table.clone();
    foreign_table.fn_abis[checked.unit().header.abi.0 as usize].result_arena = Some(0);
    let changed = Inputs {
        memo: Default::default(),
        layouts: &foreign_table,
        ..inputs
    };
    assert!(LayoutClosure::capture(&checked, &changed, &[]).is_err());
}

#[test]
fn selected_implementation_keys_keep_full_inputs_and_existing_symbol_domains() {
    let types = hirsel_types::TypeInterner::new(None);
    let base = hirsel_layout::instance::InstanceKey::new(
        &rc_support::path("key"),
        0,
        &[],
        &types,
        None,
        None,
    )
    .unwrap();
    let derived = base.derived("selected-test", &[b"a", b"bc"]);
    assert_ne!(derived, base.derived("selected-test", &[b"ab", b"c"]));
    assert_ne!(derived, base.derived("other-test", &[b"a", b"bc"]));
    assert_eq!(
        derived.digest(),
        hirsel_syntax::ContentHash::of_parts(
            "selected-test",
            [base.digest().0.as_slice(), b"a", b"bc"]
        )
    );
    assert_eq!(derived.definition(), base.definition());
    assert_eq!(derived.body(), base.body());
}

#[test]
fn arena_query_graph_settles_local_equations_and_reuses_unchanged_components() {
    use hirsel_driver::native::queries::arena::{
        ArenaQueryCounts, EQUATIONS_DOMAIN, EquationProduct,
    };
    use hirsel_driver::native::queries::graph::{ArenaNode, settle_arena_queries};
    use hirsel_driver::native::queries::{InstanceAddress, extract_local_arena};
    use hirsel_engine::store::{MemStore, ResultStore};
    use hirsel_syntax::ContentHash;
    let source = "type Item { n:u64 }\nfn first(a:Arena,n:u64)->(box Item) in a {if n==0u64 {box Item(n:6u64)} else {second(a,n-1u64)}}\nfn second(a:Arena,n:u64)->(box Item) in a {first(a,n)}\nfn wrapper(a:Arena)->(box Item) in a {second(a,2u64)}\nfn main(){arena.with(fn(a:Arena){let x=wrapper(a);});}";
    let program = rc_support::compile(source);
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
    let manifest = rc_support::manifest(&program);
    let operations = program.operation_registry();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
        operations: &operations,
        discharge: Some(&lowered.native_discharge),
    };
    let store = MemStore::default();
    let mut nodes = Vec::new();
    for (unit, key) in lowered.units.iter().zip(&lowered.unit_keys) {
        let checked = hirsel_lir::validate::validate_value(unit.clone(), &inputs).unwrap();
        let owner = InstanceAddress {
            instance: key.clone(),
            target_abi: target.digest(),
            policy: ContentHash::of_bytes(b"query-test-policy"),
        };
        let equations = extract_local_arena(&owner, &checked, &lowered.table).unwrap();
        let reference = store
            .put_content(EQUATIONS_DOMAIN, &serde_json::to_vec(&equations).unwrap())
            .unwrap();
        nodes.push(ArenaNode {
            equations: EquationProduct { owner, reference },
            callees: equations.callees().to_vec(),
        });
    }
    nodes.sort_by(|a, b| a.equations.owner.canonical_cmp(&b.equations.owner));
    let mut counts = ArenaQueryCounts::default();
    let first = settle_arena_queries(&store, &nodes, &[], 0, &mut counts).unwrap();
    assert!(counts.misses > 0);
    assert_eq!(counts.hits, 0);
    for name in ["first", "second", "wrapper"] {
        let xs = first
            .chosen
            .iter()
            .flat_map(|s| &s.candidates)
            .find(|(a, _)| a.instance.definition().0.name == name)
            .unwrap();
        assert_eq!(xs.1, [0], "{name}");
    }
    let misses = counts.misses;
    let next = settle_arena_queries(&store, &nodes, &[], 0, &mut counts).unwrap();
    assert_eq!(counts.misses, misses);
    assert_eq!(counts.hits, misses);
    assert_eq!(
        first
            .chosen
            .iter()
            .map(|s| s.product.record)
            .collect::<Vec<_>>(),
        next.chosen
            .iter()
            .map(|s| s.product.record)
            .collect::<Vec<_>>()
    );
    let mut missing = nodes.pop().unwrap();
    missing.callees.push((
        rc_support::path("missing-callee"),
        ContentHash::of_bytes(b"missing"),
    ));
    missing.callees.sort();
    missing.callees.dedup();
    nodes.push(missing);
    assert!(settle_arena_queries(&store, &nodes, &[], 0, &mut counts).is_err());
}
