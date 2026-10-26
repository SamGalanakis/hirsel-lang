//! Independently check actual specialized isolate callback bindings.
//! Until completed-begin and cleanup stages are certified, its caller retains
//! the existing opaque boundary. A dynamic phi never creates a known target.
use hirsel_driver::native;
use hirsel_engine::with_compiler_stack;
use hirsel_lir::ir::{Callee, UnitOrigin};

const PROGRAM: &str = r#"
fn first() -> u64 { 3u64 }
fn second() -> u64 { 5u64 }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let value: Result[u64, Failure] = task.isolate(tasks, CALLBACK)
  match value {
    Ok(n) => { let _ = out.print(output, f"{n}\n") },
    Error(_) => failure.check(false, Arithmetic()),
  }
  Ok(())
}
"#;

fn check(callback: &str, known: bool) {
    with_compiler_stack(|| {
        let program = super::runtime50_continuation::ready(&PROGRAM.replace("CALLBACK", callback));
        let wrappers: Vec<_> = program
            .units
            .iter()
            .map(|u| u.unit())
            .filter(|u| u.header.definition.is_core("task.isolate"))
            .collect();
        assert!(!wrappers.is_empty());
        let specialized = wrappers.iter().any(|wrapper| {
            wrapper.tables.calls.iter().any(|call| {
                let Callee::Bound { code, .. } = call.callee else {
                    return false;
                };
                !wrapper.tables.symbols[code.index()]
                    .definition
                    .0
                    .package
                    .package_is("core", "1")
            })
        });
        assert_eq!(specialized, known);
        let mut proved = 0;
        for caller in program.units.iter().map(|u| u.unit()) {
            for (at, call) in caller.tables.calls.iter().enumerate() {
                let Callee::Direct(symbol) = call.callee else {
                    continue;
                };
                let symbol = &caller.tables.symbols[symbol.index()];
                let Some(wrapper) = wrappers
                    .iter()
                    .find(|u| Some(u.header.instance) == symbol.instance)
                else {
                    continue;
                };
                // The proof derives whether each proposed binding matches
                // actual SSA independently of specialization discovery.
                for construction in &caller.tables.closures {
                    let Some(code) = construction.code else {
                        continue;
                    };
                    let target = &caller.tables.symbols[code.symbol.index()];
                    let Some(callback) = program
                        .units
                        .iter()
                        .map(|u| u.unit())
                        .find(|u| Some(u.header.instance) == target.instance)
                    else {
                        continue;
                    };
                    let mut wrapper = (**wrapper).clone();
                    let Some((index, closure)) = wrapper.tables.calls.iter().enumerate().find_map(
                        |(index, call)| match call.callee {
                            Callee::Indirect(closure) | Callee::Bound { closure, .. } => {
                                Some((index, closure))
                            }
                            _ => None,
                        },
                    ) else {
                        continue;
                    };
                    if matches!(wrapper.tables.calls[index].callee, Callee::Indirect(_)) {
                        let symbol = hirsel_lir::ir::SymbolId(wrapper.tables.symbols.len() as u32);
                        wrapper.tables.symbols.push(target.clone());
                        wrapper.tables.calls[index].callee = Callee::Bound {
                            code: symbol,
                            closure,
                        };
                        wrapper.tables.calls[index].abi = callback.header.abi;
                    }
                    assert_eq!(wrapper.header.origin, UnitOrigin::Library);
                    let environment = program.ahead_environments[&callback.header.instance];
                    let proof = hirsel_lir::continuation::isolate::IsolateCallback::prove(
                        caller,
                        hirsel_lir::ir::CallId(at as u32),
                        &wrapper,
                        callback,
                        environment,
                        &program.table,
                    );
                    if !known {
                        assert!(proof.is_none(), "a dynamic phi target was accepted");
                        continue;
                    }
                    assert!(
                        proof.is_some(),
                        "the independently derived callback: {target:?}"
                    );
                    assert!(
                        hirsel_lir::continuation::isolate::IsolateCallback::prove(
                            caller,
                            hirsel_lir::ir::CallId(at as u32),
                            &wrapper,
                            callback,
                            None,
                            &program.table,
                        )
                        .is_none(),
                        "a wrong environment descriptor was accepted"
                    );
                    let mut forged = wrapper.clone();
                    let borrowed = proof.as_ref().unwrap().loan;
                    forged.tables.loans[borrowed.index()].mutable = true;
                    assert!(
                        hirsel_lir::continuation::isolate::IsolateCallback::prove(
                            caller,
                            hirsel_lir::ir::CallId(at as u32),
                            &forged,
                            callback,
                            environment,
                            &program.table,
                        )
                        .is_none(),
                        "a mutable callback environment loan was accepted"
                    );
                    let mut forged = wrapper.clone();
                    let at = proof.as_ref().unwrap().callback;
                    if let Callee::Bound { code, .. } = forged.tables.calls[at.index()].callee {
                        forged.tables.symbols[code.index()].instance = Some(forged.header.instance);
                    }
                    assert!(
                        hirsel_lir::continuation::isolate::IsolateCallback::prove(
                            caller,
                            proof.as_ref().unwrap().invocation,
                            &forged,
                            callback,
                            environment,
                            &program.table,
                        )
                        .is_none(),
                        "another callback body was accepted"
                    );
                    proved += 1;
                }
            }
        }
        assert_eq!(proved != 0, known);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        let boundaries: Vec<_> = plans
            .plans
            .values()
            .flat_map(|p| &p.states)
            .filter(|s| s.boundary.operation.is_core("task.isolate"))
            .collect();
        assert_eq!(
            native::continuation::classify(&hirsel_syntax::DefPath::core("task.isolate")),
            Some(hirsel_lir::continuation::CalleeClass::Boundary)
        );
        assert!(boundaries.iter().all(|s| s.callee.is_none()));
        assert!(!hirsel_lir::continuation::leaves_at_boundary(
            &hirsel_syntax::DefPath::core("task.isolate")
        ));
    });
}

#[test]
fn known_isolate_callback_identity_requires_the_transferred_parameter() {
    check("fn() -> u64 { first() }", true);
}

#[test]
fn dynamic_isolate_callback_stays_opaque() {
    check(
        "if arg.len(args) == 0u64 { fn() -> u64 { first() } } else { fn() -> u64 { second() } }",
        false,
    );
}

#[test]
fn isolated_protocol_anchors_reject_missing_cleanup_and_end() {
    with_compiler_stack(|| {
        let source = r#"
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let output = io.stdout(rt)?
  let value: Result[String, Failure] = task.isolate(tasks, fn() -> String { "held" })
  match value {
    Ok(text) => { let _ = out.print(output, text) },
    Error(_) => failure.check(false, Arithmetic()),
  }
  Ok(())
}
"#;
        let program = super::runtime50_continuation::ready(source);
        let mut checked = 0;
        for caller in &program.units {
            for (at, call) in caller.unit().tables.calls.iter().enumerate() {
                let Callee::Direct(symbol) = call.callee else {
                    continue;
                };
                let symbol = &caller.unit().tables.symbols[symbol.index()];
                if !symbol.definition.is_core("task.isolate") {
                    continue;
                }
                let wrapper = program
                    .units
                    .iter()
                    .find(|unit| Some(unit.unit().header.instance) == symbol.instance)
                    .unwrap();
                let Some((code, _)) = wrapper.unit().tables.calls.iter().find_map(|call| {
                    if let Callee::Bound { code, closure } = call.callee {
                        Some((code, closure))
                    } else {
                        None
                    }
                }) else {
                    continue;
                };
                let symbol = &wrapper.unit().tables.symbols[code.index()];
                let callback = program
                    .units
                    .iter()
                    .find(|unit| Some(unit.unit().header.instance) == symbol.instance)
                    .unwrap();
                let protocol = hirsel_lir::continuation::isolate::IsolateProtocol::derive(
                    caller,
                    hirsel_lir::ir::CallId(at as u32),
                    wrapper,
                    callback,
                    program.ahead_environments[&callback.unit().header.instance],
                    &program.table,
                )
                .expect("actual validated isolate protocol");
                protocol
                    .validate(caller, wrapper, callback, &program.table)
                    .unwrap();
                assert!(protocol.result.is_some());
                assert!(!protocol.cleanup.is_empty());
                assert!(!protocol.ends.is_empty());
                assert!(!protocol.terminals.is_empty());
                let reject = |forged: &hirsel_lir::continuation::isolate::IsolateProtocol| {
                    assert!(
                        forged
                            .validate(caller, wrapper, callback, &program.table)
                            .is_err()
                    );
                };
                let mut forged = protocol.clone();
                forged.cleanup.pop();
                reject(&forged);
                let mut forged = protocol.clone();
                forged.ends.pop();
                reject(&forged);
                let mut forged = protocol.clone();
                forged.terminals.pop();
                reject(&forged);
                let mut forged = protocol.clone();
                forged.admission.instruction += 1;
                reject(&forged);
                let mut forged = protocol.clone();
                forged.callback.normal = None;
                reject(&forged);
                let mut forged = protocol.clone();
                forged.result.as_mut().unwrap().layout =
                    hirsel_lir::layout_types::LayoutId(u32::MAX);
                reject(&forged);
                checked += 1;
            }
        }
        assert!(checked > 0);
    });
}

#[test]
fn entered_isolate_mark_storage_rejects_forged_cells_without_admitting_callback() {
    with_compiler_stack(|| {
        use hirsel_lir::continuation::isolate::IsolateProtocol;
        use hirsel_lir::continuation::{MAX_FRAME_BYTES, RuntimeResultSource};
        use hirsel_lir::ir::{CallId, ValueId};
        let source = PROGRAM.replace("CALLBACK", "fn() -> u64 { first() }");
        let program = super::runtime50_continuation::ready(&source);
        let mut checked = 0;
        for caller in &program.units {
            for (at, call) in caller.unit().tables.calls.iter().enumerate() {
                let Callee::Direct(symbol) = call.callee else {
                    continue;
                };
                let symbol = &caller.unit().tables.symbols[symbol.index()];
                if !symbol.definition.is_core("task.isolate") {
                    continue;
                }
                let wrapper = program
                    .units
                    .iter()
                    .find(|u| Some(u.unit().header.instance) == symbol.instance)
                    .unwrap();
                let Some(code) = wrapper.unit().tables.calls.iter().find_map(|call| {
                    if let Callee::Bound { code, .. } = call.callee {
                        Some(code)
                    } else {
                        None
                    }
                }) else {
                    continue;
                };
                let target = &wrapper.unit().tables.symbols[code.index()];
                let callback = program
                    .units
                    .iter()
                    .find(|u| Some(u.unit().header.instance) == target.instance)
                    .unwrap();
                let proof = IsolateProtocol::derive(
                    caller,
                    CallId(at as u32),
                    wrapper,
                    callback,
                    program.ahead_environments[&callback.unit().header.instance],
                    &program.table,
                )
                .unwrap();
                proof
                    .validate(caller, wrapper, callback, &program.table)
                    .unwrap();
                let cell = proof.begin_result(wrapper, &program.table, 17).unwrap();
                assert_eq!(cell.offset, 24);
                let operation = proof.begin_operation(wrapper, &program.table, 17).unwrap();
                assert_eq!(operation.kind.tag(), 24);
                assert!(proof.check_begin_operation(wrapper, &program.table, 17, &operation));
                for mutation in 0..5 {
                    let mut bad = operation.clone();
                    match mutation {
                        0 => bad.kind = hirsel_lir::continuation::RuntimeOpKind::ScopeEnter,
                        1 => bad.helper = "hirsel_rt_v2_task_scope_enter".into(),
                        2 => bad.results.clear(),
                        3 => bad.results.push(cell.clone()),
                        4 => bad.results[0].offset = u64::MAX - 7,
                        _ => unreachable!(),
                    }
                    assert!(
                        !proof.check_begin_operation(wrapper, &program.table, 17, &bad),
                        "typed operation mutation {mutation}"
                    );
                }
                assert!(proof.check_begin_result(wrapper, &program.table, 17, &cell));
                assert!(
                    !proof.check_begin_result(wrapper, &program.table, 25, &cell),
                    "overlap with a larger live area"
                );
                let reject = |claimed: &hirsel_lir::continuation::RuntimeResultSlot| {
                    assert!(!proof.check_begin_result(wrapper, &program.table, 17, claimed));
                };
                let mut bad = cell.clone();
                bad.offset += 1;
                reject(&bad);
                let mut bad = cell.clone();
                bad.offset -= 8;
                reject(&bad);
                let mut bad = cell.clone();
                bad.bytes = 16;
                reject(&bad);
                let mut bad = cell.clone();
                bad.align = 4;
                reject(&bad);
                let mut bad = cell.clone();
                bad.layout = hirsel_lir::layout_types::LayoutId(u32::MAX);
                reject(&bad);
                let mut bad = cell.clone();
                bad.source = RuntimeResultSource::CallResult(ValueId(u32::MAX));
                reject(&bad);
                let mut forged = proof.clone();
                forged.mark = ValueId(u32::MAX);
                assert!(forged.begin_result(wrapper, &program.table, 17).is_none());
                assert!(!forged.check_begin_result(wrapper, &program.table, 17, &cell));
                let mut forged = proof.clone();
                forged.begin.call = proof.callback.call;
                assert!(forged.begin_result(wrapper, &program.table, 17).is_none());
                assert!(!forged.check_begin_result(wrapper, &program.table, 17, &cell));
                for after in [0, MAX_FRAME_BYTES, u64::MAX] {
                    assert!(proof.begin_result(wrapper, &program.table, after).is_none());
                    assert!(!proof.check_begin_result(wrapper, &program.table, after, &cell));
                }
                let last = proof
                    .begin_result(wrapper, &program.table, MAX_FRAME_BYTES - 8)
                    .unwrap();
                assert!(proof.check_begin_result(
                    wrapper,
                    &program.table,
                    MAX_FRAME_BYTES - 8,
                    &last
                ));
                checked += 1;
            }
        }
        assert!(checked > 0);
        let plans = native::continuation::plans(&program);
        native::continuation::validate(&program, &plans).unwrap();
        assert!(
            plans
                .plans
                .values()
                .flat_map(|p| &p.states)
                .filter(|s| s.boundary.operation.is_core("task.isolate"))
                .all(|s| s.callee.is_none()),
            "storage evidence must not enable nesting"
        );
    });
}
