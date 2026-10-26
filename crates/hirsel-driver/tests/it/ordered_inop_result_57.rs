//! Retained ABI output is independent of the native stack's result temporary.
use hirsel_engine::with_compiler_stack;
use hirsel_lir::continuation::{check_ordered_host_result, core_name, ordered_host_result};
use hirsel_lir::ir::CallId;

#[test]
fn ordered_host_result_cells_refuse_forged_value_extent_alignment_and_overlap() {
    with_compiler_stack(|| {
        let program = super::runtime50_continuation::ready(
            r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let c=io.monotonic_clock(rt)?
  let output=io.stdout(rt)?
  match clock.monotonic(c) {
    Ok(at)=>{let _=clock.sleep_until(c,at)},
    Error(_)=>{},
  }
  let _=out.print(output,"x")
  Ok(())
}
"#,
        );
        let mut seen = std::collections::BTreeSet::new();
        for unit in &program.units {
            let u = unit.unit();
            for (i, call) in u.tables.calls.iter().enumerate() {
                let id = CallId(i as u32);
                let Some(cell) = ordered_host_result(u, &program.table, id, 17) else {
                    continue;
                };
                let hirsel_lir::ir::Callee::Operation(op) = call.callee else {
                    unreachable!();
                };
                seen.insert(core_name(&u.tables.operations[op.index()].definition).unwrap());
                assert!(check_ordered_host_result(u, &program.table, id, 17, &cell));
                assert!(cell.offset >= 17 && cell.offset % cell.align == 0);
                for field in 0..5 {
                    let mut bad = cell.clone();
                    match field {
                        0 => bad.offset = 0,
                        1 => bad.bytes += 1,
                        2 => bad.align *= 2,
                        3 => {
                            bad.source = hirsel_lir::continuation::RuntimeResultSource::CallResult(
                                hirsel_lir::ir::ValueId(u32::MAX),
                            )
                        }
                        _ => bad.offset += bad.align,
                    }
                    assert!(
                        !check_ordered_host_result(u, &program.table, id, 17, &bad),
                        "accepted mutated field {field}"
                    );
                }
                assert!(ordered_host_result(u, &program.table, id, 4096).is_none());
                assert!(ordered_host_result(u, &program.table, id, u64::MAX).is_none());
            }
        }
        assert_eq!(
            seen,
            ["clock.sleep_until".to_owned(), "out.print".to_owned()].into()
        );
    });
}

#[test]
fn ordered_await_result_slot_keeps_its_exact_none_layout_and_unique_mutable_loan() {
    with_compiler_stack(|| {
        for source in [
            super::runtime50_continuation::TWO_BOUNDARY,
            r#"
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let value=task.scope(tasks,fn(s:Scope)->String {
    let child=task.async(s,fn()->String {"held"})
    task.await(s,take child)
  })
  let _=out.print(output,value)
  Ok(())
}
"#,
        ] {
            let program = super::runtime50_continuation::ready(source);
            let plans = hirsel_driver::native::continuation::plans(&program);
            let mut proved = 0;
            for unit in &program.units {
                let u = unit.unit();
                if !u.header.definition.is_core("task.await") {
                    continue;
                }
                let p = plans.get(u.header.instance).expect("await wrapper plan");
                assert_eq!(
                    p.representation,
                    hirsel_lir::continuation::Representation::Resumable,
                    "{p:?}"
                );
                for state in &p.states {
                    let Some(cell) = hirsel_lir::continuation::ordered_join_result(
                        u,
                        &program.table,
                        state.boundary.block,
                        &p.storage,
                    ) else {
                        continue;
                    };
                    let hirsel_lir::continuation::RuntimeResultSource::ArgumentSlot {
                        slot,
                        loan,
                        ..
                    } = cell.source
                    else {
                        unreachable!();
                    };
                    assert!(hirsel_lir::continuation::check_ordered_join_result(
                        u,
                        &program.table,
                        state.boundary.block,
                        &p.storage,
                        &cell
                    ));
                    let mut forged = cell.clone();
                    forged.bytes += 1;
                    assert!(!hirsel_lir::continuation::check_ordered_join_result(
                        u,
                        &program.table,
                        state.boundary.block,
                        &p.storage,
                        &forged
                    ));
                    assert!(u.tables.loans[loan.index()].mutable);
                    assert!(state.loans.contains(&loan));
                    let mut wrong_storage = p.storage.clone();
                    wrong_storage
                        .iter_mut()
                        .find(|s| s.slot == slot)
                        .unwrap()
                        .offset += 1;
                    assert!(
                        hirsel_lir::continuation::ordered_join_result(
                            u,
                            &program.table,
                            state.boundary.block,
                            &wrong_storage
                        )
                        .is_none()
                    );
                    let mut no_loan = u.clone();
                    no_loan.tables.loans[loan.index()].mutable = false;
                    assert!(
                        hirsel_lir::continuation::ordered_join_result(
                            &no_loan,
                            &program.table,
                            state.boundary.block,
                            &p.storage
                        )
                        .is_none()
                    );
                    proved += 1;
                }
            }
            assert!(proved > 0, "no reviewed await result image proved");
        }
    });
}
