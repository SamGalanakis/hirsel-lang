use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use hirsel_lir::ahead::CpuWorkerProtocol;

const SOURCE: &str = r#"
pub fn main(rt: Io) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let outcome = core@1/cpu.map_scoped(tasks, 4,
    fn() -> List[u64] uses {} { [1u64, 2u64, 3u64, 4u64] },
    fn(take job: u64) -> u64 uses {} { job + 7u64 })
  match outcome {
    Ok(rows) => failure.check(rows == [8u64, 9u64, 10u64, 11u64], Bounds()),
    Error(_) => (),
  }
  Ok(())
}
"#;

fn ready(source: &str) -> native::ReadyProgram {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("cpu.hir", source.to_owned()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    native::build(
        &validated,
        &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
    )
    .unwrap()
}

#[test]
fn cpu_worker_ownership_protocol_binds_the_shared_loan_job_move_and_result_sink() {
    with_compiler_stack(|| {
        let program = ready(SOURCE);
        let worker = program
            .units
            .iter()
            .find(|unit| unit.unit().header.definition.is_core("$native.cpu_worker"))
            .unwrap();
        let protocol = CpuWorkerProtocol::derive(worker, &program.table).unwrap_or_else(|| {
            panic!(
                "unproved CPU worker: {:#?}\nlayouts: {:#?}",
                worker.unit(),
                program.table
            )
        });
        assert!(protocol.check(worker, &program.table));
        let plan = hirsel_driver::native::ahead_plan(&program);
        assert!(
            plan.roots.values().any(|root| root.cpu.is_some()),
            "{:?}",
            plan.refusals
        );
        let all: Vec<_> = program.units.iter().collect();
        assert!(hirsel_lir::ahead::validate_cpu_workers(
            &all,
            &program.table,
            &program.target,
            &plan,
            program.fuel
        ));
        for field in 0..4 {
            let mut bad = plan.clone();
            let root = bad
                .roots
                .values_mut()
                .find(|root| root.cpu.is_some())
                .unwrap();
            match field {
                0 => root.recipe.callable_offset = root.recipe.live_word_offset,
                1 => root.recipe.source_abi = worker.unit().header.abi,
                2 => root.cpu.as_mut().unwrap().spine.scalar_calls.clear(),
                _ => {
                    root.cpu.as_mut().unwrap().binding.callback_digest =
                        hirsel_syntax::ContentHash::default()
                }
            }
            assert!(
                !hirsel_lir::ahead::validate_cpu_workers(
                    &all,
                    &program.table,
                    &program.target,
                    &bad,
                    program.fuel
                ),
                "changed CPU field {field}"
            );
        }
        for backend in [
            hirsel_driver::native::Backend::Llvm,
            hirsel_driver::native::Backend::Cranelift,
        ] {
            let objects = hirsel_driver::native::emit_backend_objects(
                &program,
                backend,
                hirsel_lir::emission::OptLevel::Default,
                hirsel_driver::native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
            hirsel_driver::native::check_ahead_objects(&program, &objects).unwrap();
            let certificates =
                hirsel_driver::native::continuation::ahead_certificates(&program, &objects);
            assert!(
                certificates.iter().any(|certificate| certificate.kind
                    == hirsel_lir::continuation::AheadKind::CpuWorker)
            );
            let mut damaged = objects.clone();
            let metadata = damaged
                .iter_mut()
                .find(|object| {
                    object
                        .exports
                        .iter()
                        .any(|symbol| symbol.name.starts_with("hirsel_ahead_"))
                })
                .unwrap();
            metadata.bytes[0] ^= 1;
            assert!(hirsel_driver::native::check_ahead_objects(&program, &damaged).is_err());
        }
        let claim = protocol.claim.index();
        let mut changed = worker.clone().into_raw();
        let control = &changed.tables.calls[claim];
        let action = control
            .arguments
            .get(&changed.pools.call_arguments)
            .unwrap()
            .iter()
            .find(|arg| arg.parameter == 2)
            .unwrap();
        let hirsel_lir::ir::ArgumentValue::Value(action) = action.value else {
            panic!("claim action")
        };
        let constant = changed
            .instructions
            .iter()
            .find_map(|inst| match inst {
                hirsel_lir::ir::Inst::Const { result, constant } if *result == action => {
                    Some(*constant)
                }
                _ => None,
            })
            .unwrap();
        changed.tables.constants[constant.index()].value =
            hirsel_lir::ir::ConstantValue::Integer(1);
        let inputs = hirsel_lir::validate::Inputs {
            target: &program.target,
            layouts: &program.table,
            manifest: &program.manifest,
            operations: &program.operations,
            discharge: Some(&program.discharged),
            memo: Default::default(),
        };
        let changed = hirsel_lir::validate::validate_ready(changed, &inputs).unwrap();
        assert!(
            CpuWorkerProtocol::derive(&changed, &program.table).is_none(),
            "leave is not a job claim"
        );
        for field in 0..10 {
            let mut bad = protocol.clone();
            match field {
                0 => bad.digest = hirsel_syntax::ContentHash::of_bytes(b"changed worker"),
                1 => bad.shared_mapper = bad.jobs,
                2 => bad.jobs = bad.shared_mapper,
                3 => bad.live = bad.jobs,
                4 => bad.mapper = bad.claim,
                5 => bad.claim = bad.finish,
                6 => bad.result = bad.jobs,
                7 => bad.job_layout = bad.environment,
                8 => bad.result_layout = bad.environment,
                _ => bad.ordinal = bad.live,
            }
            assert!(!bad.check(worker, &program.table), "changed field {field}");
        }
    });
}
