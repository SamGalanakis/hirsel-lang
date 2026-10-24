//! Demand-callable requests use the batch transform's sole rewrite and ABI codec.
use crate::rc_support as support;
use hirsel_lir::{
    ir::{Callee, Symbol, SymbolKind},
    products::{Binding, LayoutClosure},
    validate::{Inputs, validate_value},
};
use hirsel_rc::closure_calls::{CallableEntry, CallableRecipe, rewrite_instance};
use std::collections::HashMap;

#[test]
fn callable_requests_preserve_refusal_names_and_rebind_canonical_abis() {
    let program = support::compile(
        "fn plus(n:u64)->u64 uses {} {n+1u64}\nfn invoke(n:u64,f:fn(u64)->u64 uses {})->u64 uses {f} {f(n)}\npub fn main()->u64 {invoke(1u64,plus)}",
    );
    let target = support::target();
    let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
    let manifest = support::manifest(&program);
    let operations = lowered.operation_registry(&program);
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
        operations: &operations,
        discharge: Some(&lowered.native_discharge),
    };
    let entries = lowered
        .instances
        .ids()
        .map(|id| {
            let unit = &lowered.units[id.index()];
            (
                Symbol {
                    kind: SymbolKind::Function,
                    definition: unit.header.definition.clone(),
                    instance: Some(unit.header.instance),
                },
                CallableEntry {
                    key: lowered.instances.view(id).key.clone(),
                    abi: unit.header.abi,
                    origin: unit.header.origin,
                    op_free: false,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let invoke = entries
        .values()
        .find(|entry| entry.key.definition().0.name == "invoke")
        .unwrap();
    let original = lowered
        .units
        .iter()
        .find(|unit| unit.header.definition.0.name == "main")
        .unwrap();
    let mut selected = original.clone();
    let call = selected.tables.calls.iter().position(|call| {
        matches!(call.callee, Callee::Direct(symbol) if selected.tables.symbols[symbol.index()].definition == *invoke.key.definition())
    }).unwrap();
    let Callee::Direct(specialized) = selected.tables.calls[call].callee else {
        unreachable!()
    };
    let expected_digest = selected.tables.symbols[specialized.index()]
        .instance
        .unwrap();
    assert_ne!(expected_digest, invoke.key.digest());
    let original_symbol = selected
        .tables
        .symbols
        .iter()
        .position(|symbol| {
            symbol.definition == *invoke.key.definition()
                && symbol.instance == Some(invoke.key.digest())
        })
        .unwrap();
    selected.tables.calls[call].callee =
        Callee::Direct(hirsel_lir::ir::SymbolId(original_symbol as u32));
    let before = hirsel_codec::to_text(&selected);
    let mut requests = Vec::new();
    rewrite_instance(
        &mut selected,
        &lowered.table,
        &entries,
        &[],
        0,
        |entry, request| {
            assert_eq!(entry.key, invoke.key);
            requests.push(request.clone());
            false
        },
    );
    assert_eq!(requests.len(), 1);
    assert_eq!(before, hirsel_codec::to_text(&selected));
    assert_eq!(requests[0].key.digest(), expected_digest);
    let mut accepted = selected.clone();
    rewrite_instance(
        &mut accepted,
        &lowered.table,
        &entries,
        &[],
        0,
        |_, request| {
            assert_eq!(request.key, requests[0].key);
            true
        },
    );
    assert_eq!(
        hirsel_codec::to_text(&accepted),
        hirsel_codec::to_text(original)
    );
    validate_value(accepted, &inputs).unwrap();
    let request = &requests[0];
    let recipe = CallableRecipe::capture(&invoke.key, request, &lowered.table).unwrap();
    let bytes = serde_json::to_vec(&recipe).unwrap();
    let restored: CallableRecipe = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(restored.source().unwrap(), invoke.key);
    assert_eq!(restored.key().unwrap(), request.key);
    let closures = lowered
        .units
        .iter()
        .rev()
        .map(|unit| {
            let unit = validate_value(unit.clone(), &inputs).unwrap();
            LayoutClosure::capture(&unit, &inputs, &[]).unwrap().0
        })
        .collect::<Vec<_>>();
    let binding = Binding::new(&target, &closures.iter().collect::<Vec<_>>()).unwrap();
    let rebound = restored.bind(&request.key, &binding).unwrap();
    assert_eq!(rebound.key, request.key);
    assert_eq!(rebound.seeds[0].1.symbol, request.seeds[0].1.symbol);
    assert_eq!(
        binding.table.fn_abis[rebound.seeds[0].1.abi.0 as usize].key,
        lowered.table.fn_abis[request.seeds[0].1.abi.0 as usize].key
    );
    assert!(restored.bind(&invoke.key, &binding).is_err());
    let empty = Binding::new(&target, &[]).unwrap();
    assert!(restored.bind(&request.key, &empty).is_err());
    let mut malformed = serde_json::to_value(&recipe).unwrap();
    malformed["schema"] = 99.into();
    let malformed: CallableRecipe = serde_json::from_value(malformed).unwrap();
    assert!(malformed.key().is_err());
    let mut body = lowered
        .units
        .iter()
        .find(|unit| unit.header.instance == invoke.key.digest())
        .unwrap()
        .clone();
    let dynamic = body
        .tables
        .calls
        .iter()
        .filter(|call| matches!(call.callee, Callee::Indirect(_)))
        .count();
    assert_eq!(dynamic, 1);
    rewrite_instance(
        &mut body,
        &lowered.table,
        &entries,
        &rebound
            .seeds
            .iter()
            .map(|(p, target)| {
                (
                    *p,
                    hirsel_rc::closure_calls::CallableTarget {
                        abi: request.seeds[0].1.abi,
                        ..target.clone()
                    },
                )
            })
            .collect::<Vec<_>>(),
        0,
        |_, _| false,
    );
    assert!(
        body.tables
            .calls
            .iter()
            .all(|call| !matches!(call.callee, Callee::Indirect(_)))
    );
    validate_value(body, &inputs).unwrap();
}

#[test]
fn task_capture_requests_preserve_existing_adapter_identity_and_owned_finish() {
    use hirsel_lir::{LayoutKeys, stage::Value};
    use hirsel_rc::closure_calls::{
        CallableTarget, TaskCaptureRecipe, TaskCaptureRequest, rewrite_task_capture,
    };
    let program = support::compile(
        "fn main(tasks:Tasks)->u64 {task.scope(tasks,fn(s:Scope)->u64 {let n=42u64;let child=task.async(s,fn()->u64 {n});task.await(s,take child)})}",
    );
    let target = support::target();
    let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
    let manifest = support::manifest(&program);
    let operations = lowered.operation_registry(&program);
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
        operations: &operations,
        discharge: Some(&lowered.native_discharge),
    };
    let id = lowered
        .instances
        .ids()
        .find(|id| {
            lowered
                .instances
                .view(*id)
                .key
                .definition()
                .is_core("$native.task_main")
        })
        .unwrap();
    let source = &lowered.instances.view(id).key;
    let body = &lowered.units[id.index()];
    assert!(
        body.tables
            .calls
            .iter()
            .any(|call| matches!(call.callee, Callee::Indirect(_)))
    );
    let target_body = lowered
        .units
        .iter()
        .take(lowered.instances.len())
        .find(|unit| {
            unit.header.origin == hirsel_lir::ir::UnitOrigin::Source
                && unit.parameters.is_empty()
                && !unit.captures.is_empty()
        })
        .unwrap();
    let symbol = Symbol {
        kind: SymbolKind::Function,
        definition: target_body.header.definition.clone(),
        instance: Some(target_body.header.instance),
    };
    let request = TaskCaptureRequest::new(
        source,
        CallableTarget {
            symbol: symbol.clone(),
            abi: target_body.header.abi,
            environment: true,
        },
    )
    .unwrap();
    let expected = hirsel_syntax::ContentHash::of_parts(
        "hirsel.task-capture-target-specialization.v1",
        [
            source.digest().0.as_slice(),
            symbol.definition.fingerprint().0.as_slice(),
            symbol.instance.unwrap().0.as_slice(),
            &[1],
        ],
    );
    assert_eq!(request.key.digest(), expected);
    assert!(
        lowered
            .units
            .iter()
            .any(|unit| unit.header.instance == expected),
        "the unchanged batch transform must select exactly this task capture"
    );
    let recipe = TaskCaptureRecipe::capture(source, &request, &lowered.table).unwrap();
    let recipe: TaskCaptureRecipe =
        serde_json::from_slice(&serde_json::to_vec(&recipe).unwrap()).unwrap();
    assert_eq!(recipe.source().unwrap(), *source);
    assert_eq!(recipe.key().unwrap(), request.key);
    let checked = validate_value(body.clone(), &inputs).unwrap();
    let (closure, body) = LayoutClosure::capture(&checked, &inputs, &[]).unwrap();
    let target_checked = validate_value(target_body.clone(), &inputs).unwrap();
    let target_closure = LayoutClosure::capture(&target_checked, &inputs, &[])
        .unwrap()
        .0;
    let binding = Binding::new(&target, &[&target_closure, &closure]).unwrap();
    let rebound = recipe.bind(&request.key, &binding).unwrap();
    assert_eq!(
        LayoutKeys::new(&binding.table)
            .fn_abi(&binding.table.fn_abis[rebound.target.abi.0 as usize]),
        LayoutKeys::new(&lowered.table)
            .fn_abi(&lowered.table.fn_abis[request.target.abi.0 as usize])
    );
    assert!(recipe.bind(source, &binding).is_err());
    assert!(
        recipe
            .bind(&request.key, &Binding::new(&target, &[]).unwrap())
            .is_err()
    );
    let mut unit = binding.materialize::<Value>(&body).unwrap();
    let entries = HashMap::from([(
        symbol,
        CallableEntry {
            key: lowered
                .instances
                .view(
                    lowered
                        .instances
                        .ids()
                        .find(|id| {
                            lowered.instances.view(*id).key.digest() == target_body.header.instance
                        })
                        .unwrap(),
                )
                .key
                .clone(),
            abi: rebound.target.abi,
            origin: target_body.header.origin,
            op_free: false,
        },
    )]);
    rewrite_task_capture(&mut unit, &binding.table, &entries, &rebound, |_, _| false).unwrap();
    unit.header.instance = rebound.key.digest();
    assert!(
        !unit
            .tables
            .calls
            .iter()
            .any(|call| matches!(call.callee, Callee::Indirect(_)))
    );
    assert!(
        unit.tables
            .calls
            .iter()
            .any(|call| matches!(call.callee, Callee::Bound { .. }))
    );
    assert!(
        unit.tables
            .calls
            .iter()
            .any(|call| matches!(call.callee, Callee::Operation(op)
        if unit.tables.operations[op.index()].definition.is_core("$intrinsic.TaskFinish")))
    );
    validate_value(
        unit,
        &Inputs {
            memo: Default::default(),
            layouts: &binding.table,
            ..inputs
        },
    )
    .unwrap();
    let mut malformed = serde_json::to_value(&recipe).unwrap();
    malformed["target"]["parameter"] = 1.into();
    assert!(
        serde_json::from_value::<TaskCaptureRecipe>(malformed)
            .unwrap()
            .key()
            .is_err()
    );
    let mut wrong_kind = request.target.clone();
    wrong_kind.symbol.kind = SymbolKind::Constant;
    assert!(TaskCaptureRequest::new(source, wrong_kind).is_err());
}
