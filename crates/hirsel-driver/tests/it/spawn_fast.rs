//! L2's ordered store/load gate, including byte-equal host tapes with the
//! compiler's private spawn hints present or cleared after Ready validation.
use hirsel_driver::native::{self, Backend, OptLevel, Options, Partition, TargetSpec, Toolchain};
use hirsel_lir::ir::{ArgumentValue, Callee, ConstantValue, Inst};
use hirsel_lir::validate::{Inputs, validate_ready};

fn clear_spawn_hints(program: &mut native::ReadyProgram) {
    let inputs = Inputs {
        target: &program.target,
        layouts: &program.table,
        manifest: &program.manifest,
        operations: &program.operations,
        discharge: Some(&program.discharged),
        memo: Default::default(),
    };
    let mut cleared = 0;
    for validated in std::mem::take(&mut program.units) {
        let mut unit = validated.into_raw();
        for call in &unit.tables.calls {
            let Callee::Operation(op) = call.callee else {
                continue;
            };
            if !unit.tables.operations[op.index()]
                .definition
                .is_core("$intrinsic.TaskSpawn")
            {
                continue;
            }
            let arg = call
                .arguments
                .get(&unit.pools.call_arguments)
                .unwrap()
                .iter()
                .find(|a| a.parameter == 2)
                .unwrap();
            let ArgumentValue::Value(v) = arg.value else {
                panic!("spawn flags are bits")
            };
            let constant = unit
                .instructions
                .iter()
                .find_map(|i| match *i {
                    Inst::Const { result, constant } if result == v => Some(constant),
                    _ => None,
                })
                .unwrap();
            if unit.tables.constants[constant.index()].value != ConstantValue::Integer(0) {
                cleared += 1;
            }
            unit.tables.constants[constant.index()].value = ConstantValue::Integer(0);
        }
        program.units.push(validate_ready(unit, &inputs).unwrap());
    }
    assert!(
        cleared > 0,
        "this gate must exercise an actual compiler hint"
    );
}

#[test]
fn spawn_await_preserves_the_queued_sibling_load_and_ordered_tape_in_both_backends() {
    hirsel_engine::with_compiler_stack(|| {
        let dir = std::env::temp_dir().join(format!("hirsel-spawn-fast-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let output = dir.join("linked");
        std::fs::create_dir_all(&output).unwrap();
        let root = output.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let source = dir.join("gate.hir");
        std::fs::write(
            &source,
            std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../conformance/CONC/CONC-1-spawn-await-queued-sibling-run.hir"),
            )
            .unwrap(),
        )
        .unwrap();
        let mut loaded = hirsel_driver::load_program(&source).unwrap();
        loaded.check_for_execution().unwrap();
        let snapshot = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(snapshot).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let mut ready = native::build(&validated, &Options::new(target.clone())).unwrap();
        let tools = Toolchain::in_tree(&dir.join("runtime"), &[&target]).unwrap();
        let mut tapes = Vec::new();
        for fast in [true, false] {
            if !fast {
                clear_spawn_hints(&mut ready);
            }
            let mut engines = Vec::new();
            for backend in [Backend::Llvm, Backend::Cranelift] {
                let objects = native::emit_backend_objects(
                    &ready,
                    backend,
                    OptLevel::Default,
                    Partition::Whole,
                    false,
                    None,
                )
                .unwrap();
                let executable =
                    native::link_objects(&ready, &loaded.entry, &objects, &tools, &output).unwrap();
                let tape = dir.join(format!("tape-{fast}-{backend:?}"));
                let result = std::process::Command::new(executable)
                    .env_clear()
                    .env("HIRSEL_RT_ROOT", &root)
                    .env("HIRSEL_RT_TAPE", &tape)
                    .output()
                    .unwrap();
                assert!(result.status.success(), "{backend:?} {fast}: {result:?}");
                engines.push((result.stdout, result.stderr, std::fs::read(tape).unwrap()));
            }
            tapes.push(engines);
        }
        assert_eq!(tapes[0], tapes[1]);
        std::fs::remove_dir_all(dir).unwrap();
    });
}
