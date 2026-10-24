//! Iterator dispatch preserves the primitive and audits its compiler adapter.
use crate::rc_support::{compile, dump, manifest, path, target, unit_named};
use hirsel_lir::ir::*;

#[test]
fn next_keeps_its_declared_invocation_identity() {
    let program = compile(
        r#"pub fn first(mut it: Iter[u64]) -> Option[u64] { iter.next(mut it) }
fn step(mut state: u64) -> Option[u64] { state = state + 1u64; Some(state) }
pub fn main() -> u64 {
  let mut it: Iter[u64] = iter.from(0u64, step)
  match first(mut it) { Some(value) => value, None() => 0u64 }
}"#,
    );
    let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
    let unit = unit_named(&lowered, "first");
    let reference = program.operation_registry();
    let rows = &reference.invocations[&unit.header.definition];
    assert_eq!(
        hirsel_lir::invocations::check(unit, rows, 0),
        Ok(()),
        "{}",
        dump(unit)
    );
    // A different concrete source target still cannot satisfy the invocation.
    let mut wrong_target = unit.clone();
    let Callee::Direct(symbol) = wrong_target.tables.calls[0].callee else {
        panic!("the primitive must retain its declared entry");
    };
    wrong_target.tables.symbols[symbol.index()].definition = path("step");
    assert!(hirsel_lir::invocations::check(&wrong_target, rows, 0).is_err());

    let ready = hirsel_driver::native::build_program(
        &program,
        manifest(&program),
        &hirsel_driver::native::Options::new(target()),
    )
    .unwrap();
    let heap = hirsel_heapcheck::Program {
        target: target(),
        layouts: ready.table.clone(),
        units: ready.raw_units(),
        statics: ready.statics.clone(),
        invocation_bodies: ready.operations.invocation_bodies.clone(),
        pretransfer_adapters: ready.operations.pretransfer_adapters.clone(),
        invocations: ready.operations.invocations.clone(),
    };
    let run = |heap: &hirsel_heapcheck::Program<_>| {
        hirsel_heapcheck::run(
            heap,
            &path("main"),
            &[],
            &mut hirsel_heapcheck::NoOperations,
            &hirsel_heapcheck::Config::default(),
        )
    };
    let clean = run(&heap).result.unwrap().observation;
    assert!(clean.failures.is_empty());
    assert_eq!(
        clean.returned,
        Some(hirsel_heapcheck::Observed::Integer {
            scalar: hirsel_lir::layout_types::ScalarKind::U64,
            bits: 1,
        })
    );
    let index = heap
        .units
        .iter()
        .position(|unit| unit.header.definition.is_core("iter.next"))
        .unwrap();
    let canonical = &heap.units[index];
    hirsel_lir::invocations::check_library_with_layouts(canonical, &ready.operations, &ready.table)
        .unwrap();
    for forgery in 0..5 {
        let mut corrupted = heap.clone();
        let unit = &mut corrupted.units[index];
        match forgery {
            0 => unit.tables.calls[0].forward_caller = false,
            1 => unit.tables.calls[0].abi = unit.header.abi,
            2 => unit.tables.calls[0].callee = Callee::Indirect(unit.parameters[0]),
            3 => unit.instructions.push(Inst::FrameEnter {
                failure: FailureId(0),
            }),
            4 => {
                let slot = unit
                    .slots
                    .iter_mut()
                    .find(|slot| slot.kind == SlotKind::Parameter(0))
                    .unwrap();
                slot.kind = SlotKind::Local;
            }
            _ => unreachable!(),
        }
        assert!(
            hirsel_lir::invocations::check_library_with_layouts(
                unit,
                &ready.operations,
                &ready.table,
            )
            .is_err(),
            "C accepted iterator forgery {forgery}"
        );
        assert_eq!(
            run(&corrupted).violation(),
            Some(hirsel_heapcheck::ViolationKind::FrameImbalance),
            "H accepted iterator forgery {forgery}"
        );
    }
}
