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
        let manifest = rc_support::manifest(&program);
        let operations = lowered.operation_registry(&program);
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
    let operations = lowered.operation_registry(&program);
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
    body.abi_keys_mut()[0] = hirsel_syntax::ContentHash::default();
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
