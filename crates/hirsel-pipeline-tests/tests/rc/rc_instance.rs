//! Instance expansion from the driver's GEN-5 graph, checked against what the
//! source declarations say each instance must be: every expectation below is
//! written from the program text, never read back from the expander.
use crate::rc_support as support;

use hirsel_rc::instance::{InstanceId, InstanceRole};
use hirsel_types::{Primitive, TypeKind};
use support::*;

/// The primitive each type actual of an instance names, in ordinal order.
fn type_actuals(lowered: &hirsel_rc::import::Lowered, id: InstanceId) -> Vec<String> {
    let view = lowered.instances.view(id);
    view.substitution
        .types
        .iter()
        .map(
            |(_, ty)| match &lowered.instances.types.get(*ty).unwrap().kind {
                TypeKind::Primitive(p) => format!("{p:?}"),
                TypeKind::Builtin { kind, .. } => format!("{kind:?}"),
                TypeKind::Named { definition, .. } => definition.path().0.name.to_string(),
                other => format!("{other:?}"),
            },
        )
        .collect()
}

#[test]
fn generic_functions_get_one_instance_per_type_actual() {
    // Declared: keep[T] is called at u64 and at u8; twice[T] calls keep at its
    // own T, reached at u64 only.
    let (program, lowered) = lower_valid(
        "fn keep[T](take x: T) -> T { x }\nfn twice[T](take x: T) -> T { keep(take x) }\nfn main() -> u64 { let a = keep(take 5u64); let b = keep(take 1u8); twice(take a) }",
    );
    let mut keeps: Vec<Vec<String>> = instances_of(&lowered, "keep")
        .into_iter()
        .map(|id| type_actuals(&lowered, id))
        .collect();
    keeps.sort();
    assert_eq!(
        keeps,
        vec![
            vec![format!("{:?}", Primitive::U64)],
            vec![format!("{:?}", Primitive::U8)]
        ]
    );
    let twice = instances_of(&lowered, "twice");
    assert_eq!(twice.len(), 1);
    assert_eq!(
        type_actuals(&lowered, twice[0]),
        vec![format!("{:?}", Primitive::U64)]
    );
    // One generic TIR body shared by both views.
    let ks = instances_of(&lowered, "keep");
    assert!(std::sync::Arc::ptr_eq(
        lowered.instances.view(ks[0]).definition().unwrap(),
        lowered.instances.view(ks[1]).definition().unwrap()
    ));
    // Each instance's ABI is its own actual's layout.
    let layouts: Vec<_> = ks
        .iter()
        .map(|id| {
            lowered.table.fn_abis[lowered.instances.abis[id.index()].0 as usize].parameters[0]
                .layout
        })
        .collect();
    assert_ne!(layouts[0], layouts[1]);
    assert!(violations(&program, &lowered).is_empty());
}

#[test]
fn size_parameters_are_folded_constants_of_the_instance() {
    let (program, lowered) = lower_valid(
        "fn width[n:size](xs: Inline[u8, n]) -> u64 { n }\nfn main() -> u64 { let a: Inline[u8, 3] = [1u8, 2u8, 3u8]; let b: Inline[u8, 2] = [1u8, 2u8]; width(a) + width(b) }",
    );
    let mut sizes: Vec<u64> = instances_of(&lowered, "width")
        .into_iter()
        .map(|id| {
            let s = &lowered.instances.view(id).substitution.sizes;
            assert_eq!(s.len(), 1);
            s[0].1
        })
        .collect();
    sizes.sort();
    assert_eq!(sizes, vec![2, 3]);
    // The body's `n` is the instance's constant.
    for id in instances_of(&lowered, "width") {
        let n = lowered.instances.view(id).substitution.sizes[0].1;
        let unit = &lowered.units[id.index()];
        assert!(
            unit.tables
                .constants
                .iter()
                .any(|c| c.value == hirsel_lir::ir::ConstantValue::Integer(u128::from(n))),
            "{}",
            dump(unit)
        );
    }
    assert!(violations(&program, &lowered).is_empty());
}

#[test]
fn closures_are_instances_of_their_parent_with_their_capture_environment() {
    // Declared: one closure literal in `apply` capturing `k` (u64) by copy.
    let (program, lowered) = lower_valid(
        "fn apply(n: u64) -> u64 { let k = 3u64; let f = fn(x: u64) -> u64 { x + k }; f(n) }\nfn main() -> u64 { apply(1u64) }",
    );
    let apply = instances_of(&lowered, "apply");
    assert_eq!(apply.len(), 2, "the function and its closure body");
    let parent = apply
        .iter()
        .copied()
        .find(|&id| lowered.instances.roles[id.index()] == InstanceRole::Function)
        .unwrap();
    let closure = apply
        .iter()
        .copied()
        .find(|&id| lowered.instances.roles[id.index()] == InstanceRole::Closure { parent })
        .expect("a closure instance owned by apply");
    let view = lowered.instances.view(closure);
    assert_eq!(view.captures.len(), 1);
    assert_ne!(view.body, lowered.instances.view(parent).body);
    let environment = lowered.instances.environments[closure.index()].expect("an environment");
    let record = &lowered.table.layouts[environment.0 as usize];
    assert_eq!(record.fields.len(), 1, "one captured u64");
    // The closure's own unit receives its capture by version.
    assert_eq!(lowered.units[closure.index()].captures.len(), 1);
    assert!(violations(&program, &lowered).is_empty());
}

#[test]
fn finalizers_are_instances_of_their_resource() {
    // Declared: resource R has a drop body; resource S has none.
    let (program, lowered) = lower_valid(
        "resource type R {n:u64} drop(x) { let _ = x.n }\nresource type S {n:u64}\nfn main() -> u64 { let r = R(n: 1u64); let s = S(n: 2u64); r.n + s.n }",
    );
    let finalizers: Vec<_> = lowered
        .instances
        .ids()
        .filter(|&id| lowered.instances.roles[id.index()] == InstanceRole::Finalizer)
        .collect();
    assert_eq!(finalizers.len(), 1);
    let abi = &lowered.table.fn_abis[lowered.instances.abis[finalizers[0].index()].0 as usize];
    assert_eq!(abi.parameters.len(), 1, "the finalized value");
    assert_eq!(
        abi.parameters[0].convention,
        hirsel_core_contract::Passing::Borrow
    );
    assert!(abi.result.is_none());
    assert!(violations(&program, &lowered).is_empty());
}

#[test]
fn actuals_differing_only_in_relation_share_one_instance() {
    // The driver already exports global actuals for these calls, so the
    // relation-only variant is forged: a second edge to `count` whose actual is
    // `Bytes in env` must key to the same instance (instance keys erase
    // relations, brands, uniqueness and sendability).
    let source = "fn count[T](x: T) -> u64 { 1u64 }\nfn a(q: Bytes) -> u64 { count(q) }\nfn main() -> u64 { 0u64 }";
    let mut program = compile(source);
    let edge = program
        .requirements
        .iter()
        .find(|r| r.callee.0.name == "count")
        .unwrap()
        .clone();
    let hirsel_types::GenericArgument::Type(actual) = edge.arguments[0] else {
        panic!("a type actual");
    };
    let types = program.types.clone();
    let mut related = edge.clone();
    let mut row = types.get(actual).unwrap().clone();
    row.modes.relation = hirsel_types::Relation::new([hirsel_types::Reach::Environment]);
    let id = types.intern(row);
    related.arguments = vec![hirsel_types::GenericArgument::Type(id)];
    program.requirements.push(related);
    let mut qualified = edge.clone();
    let mut row = types.get(actual).unwrap().clone();
    row.sendability_requirement = hirsel_types::SendabilityRequirement::Sendable;
    let id = types.intern(row);
    qualified.arguments = vec![hirsel_types::GenericArgument::Type(id)];
    program.requirements.push(qualified);
    let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
    assert_eq!(instances_of(&lowered, "count").len(), 1);
    // Twin: a different type actual is a different instance.
    let mut other = edge.clone();
    let id = types.intern(hirsel_types::Type::boolean());
    other.arguments = vec![hirsel_types::GenericArgument::Type(id)];
    program.requirements.push(other);
    let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
    assert_eq!(instances_of(&lowered, "count").len(), 2);
    assert!(violations(&program, &lowered).is_empty());
}

#[test]
fn polymorphic_recursion_through_a_cycle_reaches_a_fixpoint() {
    // A generic cycle at one actual: f[T] -> g[T] -> f[T].
    let (program, lowered) = lower_valid(
        "fn f[T](take x: T, n: u64) -> u64 { if n == 0u64 { 0u64 } else { g(take x, n - 1u64) } }\nfn g[T](take x: T, n: u64) -> u64 { f(take x, n) }\nfn main() -> u64 { f(take 1u8, 3u64) }",
    );
    assert_eq!(instances_of(&lowered, "f").len(), 1);
    assert_eq!(instances_of(&lowered, "g").len(), 1);
    assert!(violations(&program, &lowered).is_empty());
}

#[test]
fn a_captured_nested_generic_callee_uses_its_lexical_producer() {
    lower_valid(
        "fn outer(xs: Inline[u8, 3]) -> u64 { fn length[n: size](ys: Inline[u8, n]) -> u64 { n } (fn() { length(xs) })() }\nfn main() -> u64 { let xs: Inline[u8, 3] = [1u8, 2u8, 3u8]; outer(xs) }",
    );
}

#[test]
fn captured_generic_calls_keep_producer_binders_outside_caller_binders() {
    let (_, lowered) = lower_valid(include_str!(
        "../../../../conformance/GEN/GEN-1-captured-nested-generic-call-run.hir"
    ));
    let callee = instances_of(&lowered, "with_scopes")
        .into_iter()
        .find(|id| {
            lowered.instances.actuals[id.index()].arguments
                == [hirsel_types::GenericArgument::Size(
                    hirsel_types::Size::Constant(2),
                )]
        })
        .expect("the length instance at m=2");
    let actuals = &lowered.instances.actuals[callee.index()];
    assert_eq!(actuals.outer.len(), 1, "only the declaring outer binder");
    assert_eq!(
        actuals.outer[0].1,
        [hirsel_types::GenericArgument::Size(
            hirsel_types::Size::Constant(3)
        )]
    );
    assert_ne!(actuals.binder, Some(actuals.outer[0].0));
    let InstanceRole::Closure { parent } = lowered.instances.roles[callee.index()] else {
        panic!()
    };
    assert_eq!(
        lowered.instances.roles[parent.index()],
        InstanceRole::Function
    );
    assert!(lowered.instances.environments[callee.index()].is_none());
    assert!(lowered.units[callee.index()].captures.is_empty());
    assert!(
        lowered.instances.data_environments.is_empty(),
        "these names carry no runtime data"
    );
}

#[test]
fn c_still_refuses_a_code_less_function_captured_as_a_value() {
    let (program, mut lowered) = lower_valid(
        "fn outer(xs: Inline[u8, 3]) -> u64 { let bias: u64 = 2; let length = fn() -> u64 { bias }; (fn() { length() })() }\nfn main() -> u64 { 0u64 }",
    );
    // Hand-forge the old representation from its accepted monomorphic twin:
    // the captured callable has environment data but no code.
    let producer = lowered
        .units
        .iter_mut()
        .find(|unit| unit.tables.closures.len() == 2)
        .unwrap();
    producer.tables.closures[0].code = None;
    assert!(
        violations(&program, &lowered)
            .iter()
            .flat_map(|(_, vs)| vs)
            .any(|v| v.kind
                == hirsel_lir::validate::ViolationKind::CallShape(
                    "a code-less closure outside a bound call"
                ))
    );
}

#[test]
fn nested_generic_environment_is_ordinary_data_at_direct_calls() {
    lower_valid(
        "fn outer(xs: Inline[u8, 3]) -> u64 { let bias: u64 = 2; fn length[n: size](ys: Inline[u8, n]) -> u64 { n + bias } (fn() { length(xs) })() }\nfn main() -> u64 { 0u64 }",
    );
}

#[test]
fn ordinary_nested_generic_captures_validate() {
    for source in [
        include_str!("../../../../conformance/GEN/GEN-1-capture-data-minimal-run.hir"),
        include_str!("../../../../conformance/GEN/GEN-1-capture-data-owned-run.hir"),
        include_str!("../../../../conformance/GEN/GEN-1-capture-data-once-run.hir"),
        include_str!("../../../../conformance/GEN/GEN-1-capture-data-borrow-run.hir"),
        include_str!("../../../../conformance/GEN/GEN-1-capture-data-finalizer-run.hir"),
        include_str!("../../../../conformance/GEN/GEN-1-capture-data-nested-run.hir"),
    ] {
        let (_, lowered) = lower_valid(source);
        assert!(
            lowered
                .units
                .iter()
                .all(|u| u.tables.closures.iter().all(|c| c.code.is_some()))
        );
        for (&instance, captures) in &lowered.instances.direct_captures {
            let unit = &lowered.units[instance.index()];
            let abi = &lowered.table.fn_abis[unit.header.abi.0 as usize];
            assert!(!captures.is_empty());
            assert_eq!(abi.shape, hirsel_lir::abi_types::CallableShape::Definition);
            assert!(!abi.environment);
            assert!(unit.captures.is_empty());
            assert_eq!(
                abi.parameters.len(),
                lowered.instances.view(instance).parameters.len() + captures.len()
            );
        }
    }
}

/// A lexical generic name carries only its captured data at a direct call.
/// It must not claim the callable-only CLO-1 contract of its source name.
#[test]
fn nested_generic_data_captures_have_no_callable_contract() {
    let source = "fn relay(xs: Inline[u8, 3]) -> u64 { let bias = 2u64; fn length[n: size](ys: Inline[u8, n]) -> u64 { n + bias } fn forward[m: size](ys: Inline[u8, m]) -> u64 { length(ys) + m } forward(xs) }\nfn main() -> u64 { relay([1u8, 2u8, 3u8]) }";
    let (program, mut lowered) = lower_valid(source);
    let (&instance, captures) = lowered
        .instances
        .direct_captures
        .iter()
        .find(|(_, captures)| {
            captures.iter().any(|capture| {
                matches!(
                    lowered.table.layouts[capture.layout.0 as usize].kind,
                    hirsel_lir::layout_types::LayoutKind::Aggregate
                )
            })
        })
        .expect("forward captures length's ordinary environment record");
    assert_eq!(captures.len(), 1);
    let abi_id = lowered.instances.abis[instance.index()];
    let abi = &mut lowered.table.fn_abis[abi_id.0 as usize];
    assert!(abi.nonretaining.is_empty());
    assert!(abi.nonretaining_captures.is_empty());
    assert!(abi.scoped_entries.is_empty());

    // Read-only data nonretention is legal; it does not claim a callable entry.
    abi.nonretaining.push(hirsel_lir::abi_types::ParameterPath {
        parameter: 1,
        fields: Vec::new(),
    });
    abi.scoped_contract = hirsel_lir::abi::scoped_contract_digest(&abi.nonretaining, &[]);
    let errors = violations(&program, &lowered);
    assert!(errors.is_empty(), "{errors:?}");

    let abi = &mut lowered.table.fn_abis[abi_id.0 as usize];
    abi.scoped_entries.push(hirsel_lir::abi_types::ScopedEntry {
        parameter: abi.nonretaining[0].clone(),
        identity: abi.key.callable,
    });
    let errors = violations(&program, &lowered);
    assert!(
        errors.iter().flat_map(|(_, v)| v).any(|v| {
            matches!(&v.kind, hirsel_lir::validate::ViolationKind::Layout(error)
                if error.contains("invalid nested scoped entry selector"))
        }),
        "{errors:?}"
    );

    // The data permission still cannot certify a consuming entry.
    let abi = &mut lowered.table.fn_abis[abi_id.0 as usize];
    abi.scoped_entries.clear();
    abi.parameters[1].convention = hirsel_core_contract::Passing::Take;
    assert!(
        violations(&program, &lowered)
            .iter()
            .flat_map(|(_, v)| v)
            .any(|v| {
                matches!(&v.kind, hirsel_lir::validate::ViolationKind::Layout(error)
            if error.contains("invalid scoped nonretaining contract"))
            })
    );
}
