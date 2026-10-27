use super::*;
use crate::{abi_types::FnAbiId, layout_types::LayoutId, objects::UnitKey, testing::UnitBuilder};
use hirsel_core_contract::Passing;

// Raw equation fixtures exercise the solver, whose production caller accepts
// only C-validated bodies. No fabricated validation evidence is constructed.
fn unit(name: &str) -> UnitBuilder<Ready> {
    let hash = ContentHash::of_bytes(name.as_bytes());
    let mut unit = UnitBuilder::new(LirUnit::new(UnitHeader {
        source_count: 0,
        track_caller: false,
        stage: crate::stage::StageKind::Ready,
        lir_version: crate::LIR_VERSION,
        origin: UnitOrigin::Source,
        key: UnitKey(hash),
        definition: hirsel_syntax::DefPath::core(name),
        implementation_revision: hash,
        instance: hash,
        target: ContentHash::default(),
        abi: FnAbiId(0),
    }));
    unit.block(BlockKind::Body, &[]);
    unit
}
fn value(unit: &mut UnitBuilder<Ready>) -> ValueId {
    unit.plain(ValueRepr::Callable, LayoutId(0))
}
fn symbol(unit: &mut UnitBuilder<Ready>, name: &str) -> SymbolId {
    let id = unit.symbol(SymbolKind::Function, hirsel_syntax::DefPath::core(name));
    unit.unit.tables.symbols[id.index()].instance = Some(ContentHash::of_bytes(name.as_bytes()));
    id
}
fn reference(unit: &mut UnitBuilder<Ready>, name: &str) -> ValueId {
    let result = value(unit);
    let symbol = symbol(unit, name);
    unit.push(BlockId(0), Inst::FunctionRef { result, symbol });
    result
}
fn invoke(unit: &mut UnitBuilder<Ready>, target: &str, input: ValueId, output: ValueId) {
    let target = symbol(unit, target);
    let call = unit.call(
        Callee::Direct(target),
        &[(Passing::Borrow, ArgumentValue::Value(input))],
        Some(output),
    );
    unit.term(
        BlockId(0),
        Terminator::Call {
            call,
            normal: None,
            failure: FailureId(0),
        },
    );
}

#[test]
fn closed_callable_equations_preserve_parameters_returns_and_reject_conflicting_callers() {
    let mut identity = unit("identity");
    let formal = value(&mut identity);
    identity.parameter(formal);
    identity.ret(BlockId(0), Some(formal));
    let identity = identity.finish();
    let mut caller = unit("caller");
    let known = reference(&mut caller, "target");
    let result = value(&mut caller);
    invoke(&mut caller, "identity", known, result);
    let caller = caller.finish();
    let mut target = unit("target");
    target.ret(BlockId(0), None);
    let target = target.finish();
    let codes = solve(&[&caller, &identity, &target]).unwrap();
    let expected = Code::Known(Target {
        symbol: caller.tables.symbols[0].clone(),
        abi: None,
        environment: None,
    });
    assert_eq!(codes[1][formal.index()], expected);
    assert_eq!(codes[0][result.index()], expected);

    let mut other = unit("other");
    let conflicting = reference(&mut other, "other");
    let other_result = value(&mut other);
    invoke(&mut other, "identity", conflicting, other_result);
    let other = other.finish();
    let codes = solve(&[&caller, &identity, &target, &other]).unwrap();
    assert_eq!(codes[1][formal.index()], Code::Dynamic);
    assert_eq!(codes[0][result.index()], Code::Dynamic);
    assert_eq!(codes[3][other_result.index()], Code::Dynamic);
    let finite = solve_mode(&[&caller, &identity, &target, &other], true).unwrap();
    assert_eq!(finite[1][formal.index()].targets().unwrap().len(), 2);
    assert_eq!(finite[0][result.index()], finite[1][formal.index()]);
    assert!(solve(&[&caller, &identity]).is_ok());
    assert!(
        solve(&[&caller])
            .unwrap_err()
            .contains("missing diagnostic call target")
    );
    assert!(
        solve(&[&target, &target])
            .unwrap_err()
            .contains("duplicate diagnostic call target")
    );
}

#[test]
fn closed_callable_equations_do_not_let_one_known_caller_mask_an_unbound_root() {
    let mut identity = unit("identity");
    let formal = value(&mut identity);
    identity.parameter(formal);
    identity.ret(BlockId(0), Some(formal));
    let identity = identity.finish();
    let mut caller = unit("caller");
    let known = reference(&mut caller, "caller");
    let result = value(&mut caller);
    invoke(&mut caller, "identity", known, result);
    let caller = caller.finish();
    let mut unknown = unit("unknown_root");
    let parameter = value(&mut unknown);
    unknown.parameter(parameter);
    let result = value(&mut unknown);
    invoke(&mut unknown, "identity", parameter, result);
    let unknown = unknown.finish();
    let codes = solve(&[&caller, &identity, &unknown]).unwrap();
    assert_eq!(codes[2][parameter.index()], Code::Dynamic);
    assert_eq!(codes[1][formal.index()], Code::Dynamic);
    assert_eq!(codes[0][ValueId(1).index()], Code::Dynamic);
    let finite = solve_mode(&[&caller, &identity, &unknown], true).unwrap();
    assert_eq!(finite[2][parameter.index()], Code::Dynamic);
    assert_eq!(finite[1][formal.index()], Code::Dynamic);
    assert_eq!(finite[0][ValueId(1).index()], Code::Dynamic);
}

#[test]
fn closed_callable_equations_resolve_recursive_cycles_and_close_unseeded_cycles() {
    let mut recursive = unit("recursive");
    let formal = value(&mut recursive);
    recursive.parameter(formal);
    let result = value(&mut recursive);
    invoke(&mut recursive, "recursive", formal, result);
    let exit = recursive.block(BlockKind::Body, &[]);
    recursive.ret(exit, Some(formal));
    let recursive = recursive.finish();
    let mut caller = unit("caller");
    let known = reference(&mut caller, "caller");
    let result = value(&mut caller);
    invoke(&mut caller, "recursive", known, result);
    let caller = caller.finish();
    let codes = solve(&[&caller, &recursive]).unwrap();
    assert!(matches!(codes[1][formal.index()], Code::Known(_)));
    assert_eq!(codes[0][result.index()], codes[1][formal.index()]);
    let alone = solve(&[&recursive]).unwrap();
    assert_eq!(alone[0][formal.index()], Code::Dynamic);
    assert_eq!(alone[0][ValueId(1).index()], Code::Dynamic);
}

#[test]
fn closed_callable_equations_bind_capture_ordinals_to_the_actual_captured_code() {
    let mut callee = unit("callee");
    let capture = value(&mut callee);
    callee.unit.captures.push(capture);
    callee.ret(BlockId(0), Some(capture));
    let callee = callee.finish();
    let mut caller = unit("caller");
    let captured = reference(&mut caller, "caller");
    let closure = caller.closure(
        hirsel_syntax::DefPath::core("callee"),
        LayoutId(0),
        &[(captured, CaptureUse::Copy)],
        Placement::Fixed(StorageOwner::Heap(TaskDomainId(0))),
    );
    let code = caller.unit.tables.closures[closure.index()].code.unwrap();
    caller.unit.tables.symbols[code.symbol.index()].instance = Some(callee.header.instance);
    let result = value(&mut caller);
    caller.push(
        BlockId(0),
        Inst::Closure {
            result,
            closure,
            failure: FailureId(0),
        },
    );
    caller.ret(BlockId(0), None);
    let caller = caller.finish();
    let codes = solve(&[&caller, &callee]).unwrap();
    assert_eq!(codes[1][capture.index()], codes[0][captured.index()]);
    assert!(matches!(codes[1][capture.index()], Code::Known(_)));
    let mut wrong = callee.clone();
    wrong.captures.clear();
    assert!(
        solve(&[&caller, &wrong])
            .unwrap_err()
            .contains("capture arity differs")
    );
}

#[test]
fn closed_callable_equations_require_the_exact_persistent_mask_when_binding_captures() {
    let mut callee = unit("callee_with_mask");
    let capture = value(&mut callee);
    callee.unit.captures.push(capture);
    let mask = callee.slot(ValueRepr::Callable, LayoutId(0), SlotKind::Capture(1));
    let mask_value = callee.unit.captures[1];
    callee.unit.tables.init_flags.push(InitFlag {
        value: mask_value,
        slot: mask,
        path: PoolRange::EMPTY,
        environment: Some(LayoutId(0)),
    });
    callee.ret(BlockId(0), Some(capture));
    let callee = callee.finish();
    let mut caller = unit("caller");
    let captured = reference(&mut caller, "caller");
    let closure = caller.closure(
        hirsel_syntax::DefPath::core("callee_with_mask"),
        LayoutId(0),
        &[(captured, CaptureUse::Copy)],
        Placement::Fixed(StorageOwner::Heap(TaskDomainId(0))),
    );
    let code = caller.unit.tables.closures[closure.index()].code.unwrap();
    caller.unit.tables.symbols[code.symbol.index()].instance = Some(callee.header.instance);
    let result = value(&mut caller);
    caller.push(
        BlockId(0),
        Inst::Closure {
            result,
            closure,
            failure: FailureId(0),
        },
    );
    caller.ret(BlockId(0), None);
    let caller = caller.finish();
    let codes = solve(&[&caller, &callee]).unwrap();
    assert_eq!(codes[1][capture.index()], codes[0][captured.index()]);
    assert_eq!(codes[1][mask_value.index()], Code::Dynamic);
    for mutation in 0..3 {
        let mut invalid = callee.clone();
        match mutation {
            0 => invalid.tables.init_flags.clear(),
            1 => invalid.tables.init_flags[0].environment = Some(LayoutId(1)),
            _ => invalid.tables.init_flags[0].value = capture,
        }
        assert!(
            solve(&[&caller, &invalid])
                .unwrap_err()
                .contains("capture arity differs")
        );
    }
}

#[test]
fn finite_callable_equations_converge_when_a_cycle_reverses_predecessor_order() {
    let a = Target {
        symbol: Symbol {
            kind: SymbolKind::Function,
            definition: hirsel_syntax::DefPath::core("a"),
            instance: Some(ContentHash::of_bytes(b"a")),
        },
        abi: None,
        environment: None,
    };
    let b = Target {
        symbol: Symbol {
            definition: hirsel_syntax::DefPath::core("b"),
            instance: Some(ContentHash::of_bytes(b"b")),
            ..a.symbol.clone()
        },
        ..a.clone()
    };
    // x = y union {a}; y = x union {b}. Keeping insertion order would
    // alternate [a,b] and [b,a] forever despite stable membership.
    let mut x = Code::Known(a.clone());
    let mut y = Code::Known(b.clone());
    for _ in 0..3 {
        let next_x = y.merge(&Code::Known(a.clone()), true);
        let next_y = x.merge(&Code::Known(b.clone()), true);
        x = next_x;
        y = next_y;
    }
    assert_eq!(x, y);
    assert_eq!(x.targets(), Some([a.clone(), b.clone()].as_slice()));
    assert_eq!(y.merge(&Code::Known(a), true), x);
    assert_eq!(x.merge(&Code::Known(b), true), y);
}
