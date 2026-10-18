//! Refresh source-owned Rc/Ready heap fixtures after a native schema change.

hirsel_guard::install!();

use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use hirsel_lir::validate::{Inputs, validate};
use hirsel_lir::{LayoutTable, LirUnit, Rc, Ready, TargetSpec};
use hirsel_rc::instance::InstanceId;

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(arguments.len(), 2, "source file and output prefix");
    with_compiler_stack(move || {
        let source = std::fs::read_to_string(&arguments[0]).unwrap();
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("fixture.hir", source).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let raw = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(raw).unwrap();
        let program = hirsel_driver::native::program(&validated);
        let manifest = hirsel_driver::native::manifest(validated.snapshot()).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let lower = hirsel_rc::import::lower(&program, &target).unwrap();
        let table: LayoutTable = lower.table;
        let inputs = Inputs {
            discharge: None,
            operations: &program.operation_registry(),
            target: &target,
            layouts: &table,
            manifest: &manifest,
        };
        let mut rc: Vec<LirUnit<Rc>> = Vec::new();
        for (index, unit) in lower.units.into_iter().enumerate() {
            let unit = validate(unit, &inputs).unwrap().into_raw();
            let placement = hirsel_rc::placement::Options::for_instance(
                true,
                &lower.instances,
                InstanceId(index as u32),
            );
            let placed = hirsel_rc::placement::place(unit, &table, &placement).unwrap();
            let placed = validate(placed, &inputs).unwrap().into_raw();
            let proofs =
                hirsel_rc::insert::Proofs::for_instance(&lower.instances, InstanceId(index as u32));
            let inserted = hirsel_rc::insert::insert(placed, &table, &proofs).unwrap();
            let inserted = validate(inserted, &inputs).unwrap().into_raw();
            let elided = hirsel_rc::elide::elide(inserted, &table).unwrap();
            rc.push(validate(elided, &inputs).unwrap().into_raw());
        }
        let rc_program = hirsel_heapcheck::Program {
            target: target.clone(),
            layouts: table,
            units: rc,
            statics: lower.statics,
        };
        std::fs::write(
            format!("{}.rc.lir", arguments[1]),
            hirsel_codec::to_text(&rc_program),
        )
        .unwrap();
        let built =
            hirsel_driver::native::build(&validated, &hirsel_driver::native::Options::new(target))
                .unwrap();
        let ready: hirsel_heapcheck::Program<Ready> = hirsel_heapcheck::Program {
            target: built.target.clone(),
            layouts: built.table.clone(),
            units: built.raw_units(),
            statics: built.statics.clone(),
        };
        std::fs::write(
            format!("{}.ready.lir", arguments[1]),
            hirsel_codec::to_text(&ready),
        )
        .unwrap();
    });
}
