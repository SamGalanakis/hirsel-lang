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
