//! Per-instance misses use the real importer and independent source inventory.
use crate::rc_support as support;
use hirsel_layout::layout::LayoutBuilder;
use hirsel_lir::{
    ir::{Symbol, SymbolKind},
    validate::Inputs,
};
use hirsel_rc::instance::{DiscoveryContext, discover_instance, seed_instances};
use std::collections::BTreeSet;

#[test]
fn explicit_instance_import_keeps_source_invocations_without_program_bodies() {
    let mut program = support::compile(
        "resource type R {n:u64} drop(x) {let _=x.n}\nfn keep[T](take x:T)->T {x}\nfn recur(n:u64)->u64 {if n==0u64 {0u64}else{recur(n-1u64)}}\npub fn main()->u64 {let r=R(n:1u64);let f=fn(x:u64)->u64{keep(x)};f(recur(2u64))}",
    );
    let target = support::target();
    let manifest = support::manifest(&program);
    let mut operations = program.operation_registry();
    let context = DiscoveryContext::of(&program).unwrap();
    let bodies = std::mem::take(&mut program.definitions);
    let owners =
        hirsel_rc::library::owners(&program.types, &program.signatures).over(&program.owners);
    let mut layouts = LayoutBuilder::new(&target, &owners);
    layouts.prepare_native_controls();
    let (mut instances, mut pending) = seed_instances(&program, &context, &mut layouts).unwrap();
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(instances.view(id).key.encoding().to_vec())
            || instances.roles[id.index()].is_library()
        {
            continue;
        }
        let original = bodies[instances.view(id).key.definition()].clone();
        let (updated, required) =
            discover_instance(&program, &context, &mut layouts, instances, id, original).unwrap();
        instances = updated;
        pending.extend(required);
    }
    // The selection and expected source body come from the instance plan,
    // before the candidate Value exists.
    for id in instances.ids() {
        if instances.roles[id.index()].is_library() {
            continue;
        }
        let view = instances.view(id);
        let symbol = Symbol {
            kind: SymbolKind::Function,
            definition: view.key.definition().clone(),
            instance: Some(view.key.digest()),
        };
        let evidence = hirsel_rc::invocations::bind_source(
            &bodies[view.key.definition()],
            operations.invocations[view.key.definition()].clone(),
            &symbol,
            view.body,
        )
        .unwrap();
        operations.invocation_bodies.push(evidence.body);
    }
    let mut units = Vec::new();
    let mut next = 0;
    while next < instances.len() {
        let id = hirsel_rc::instance::InstanceId(next as u32);
        next += 1;
        let output =
            hirsel_rc::import::import_instance(&program, &mut instances, id, None, &mut layouts)
                .unwrap();
        assert!(output.discharged.is_none());
        for request in output.required {
            hirsel_rc::library::instantiate(&mut layouts, &mut instances, request).unwrap();
        }
        let statics =
            hirsel_rc::import::instance_statics(&program, &instances, &output.unit, &mut layouts)
                .unwrap();
        assert!(statics.is_empty());
        units.push(output.unit);
    }
    assert!(program.definitions.is_empty());
    assert!(units.len() >= 5);
    let table = layouts.finish();
    let inputs = Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
        memo: Default::default(),
    };
    for unit in &units {
        let violations = hirsel_lir::validate::check(unit, &inputs);
        assert!(
            violations.is_empty(),
            "{}: {violations:?}",
            unit.header.definition
        );
    }
    // Removing independent source associations refuses an otherwise unchanged
    // imported candidate; its own invocation markers grant no authority.
    operations.invocation_bodies.clear();
    let refused_inputs = Inputs {
        operations: &operations,
        ..inputs
    };
    assert!(
        units
            .iter()
            .filter(|unit| !hirsel_lir::validate::check(unit, &refused_inputs).is_empty())
            .count()
            >= 5
    );
}
