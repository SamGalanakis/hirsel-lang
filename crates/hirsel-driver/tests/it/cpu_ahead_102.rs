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
    ready_with_regions(source, true)
}

fn ready_with_regions(source: &str, regions: bool) -> native::ReadyProgram {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("cpu.hir", source.to_owned()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let mut options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
    options.regions = regions;
    native::build(&validated, &options).unwrap()
}

#[test]
fn cpu_worker_custody_accepts_owned_inputs_and_results_without_admitting_allocation() {
    with_compiler_stack(|| {
        let source = SOURCE
            .replace(
                "fn() -> List[u64] uses {} { [1u64, 2u64, 3u64, 4u64] }",
                "fn() -> List[unique List[u64]] uses {} { [[1u64], [2u64], [3u64], [4u64]] }",
            )
            .replace(
                "fn(take job: u64) -> u64 uses {} { job + 7u64 }",
                "fn(take job: unique List[u64]) -> unique List[u64] uses {} { [job[0] + 7u64] }",
            )
            .replace(
                "[8u64, 9u64, 10u64, 11u64]",
                "[[8u64], [9u64], [10u64], [11u64]]",
            );
        let program = ready(&source);
        let worker = program
            .units
            .iter()
            .find(|u| u.unit().header.definition.is_core("$native.cpu_worker"))
            .unwrap();
        let protocol = CpuWorkerProtocol::derive(worker, &program.table).unwrap();
        assert!(protocol.check(worker, &program.table));
        for layout in [protocol.job_layout, protocol.result_layout] {
            assert!(hirsel_lir::ahead::cpu_owned_layout(&program.table, layout));
            assert!(!hirsel_lir::ahead::scalar_image(&program.table, layout));
        }
        let plan = native::ahead_plan(&program);
        assert_eq!(plan.cpu_workers.get(&protocol.worker), Some(&protocol));
        assert!(
            plan.roots
                .get(&protocol.worker)
                .is_some_and(|root| root.cpu.as_ref().is_some_and(|cpu| cpu.mapper.is_some())),
            "{:?}",
            plan.refusals
        );
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let objects = native::emit_backend_objects(
                &program,
                backend,
                hirsel_lir::emission::OptLevel::Default,
                native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
            native::check_ahead_objects(&program, &objects).unwrap();
            assert!(
                native::continuation::ahead_certificates(&program, &objects)
                    .iter()
                    .any(|certificate| certificate.root == protocol.worker)
            );
        }
        assert!(
            program.units.iter().all(|u| u
                .unit()
                .tables
                .calls
                .iter()
                .enumerate()
                .all(|(i, _)| plan
                    .cpu_start(u.unit(), hirsel_lir::ir::CallId(i as u32))
                    .is_none())),
            "allocating custody does not authorize admission"
        );
        let mut forged = protocol.clone();
        forged.result = forged.jobs;
        assert!(
            !forged.check(worker, &program.table),
            "input custody is not result custody"
        );
        let mut forged = protocol.clone();
        forged.job_layout = forged.environment;
        assert!(
            !forged.check(worker, &program.table),
            "a callback environment is not a movable job"
        );
    });
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
        assert!(
            program.units.iter().any(|unit| {
                unit.unit().tables.calls.iter().enumerate().any(|(i, _)| {
                    plan.cpu_start(unit.unit(), hirsel_lir::ir::CallId(i as u32))
                        .is_some()
                })
            }),
            "the certified map selects its start helper"
        );
        let all: Vec<_> = program.units.iter().collect();
        assert!(hirsel_lir::ahead::validate_cpu_workers(
            &all,
            &program.table,
            &program.target,
            &plan,
            program.fuel
        ));
        for field in 0..6 {
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
                3 => {
                    root.cpu.as_mut().unwrap().binding.callback_digest =
                        hirsel_syntax::ContentHash::default()
                }
                4 => {
                    root.cpu.as_mut().unwrap().binding.map_digest =
                        hirsel_syntax::ContentHash::default()
                }
                _ => {
                    root.cpu.as_mut().unwrap().binding.caller_digest =
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
        let binding = &plan
            .roots
            .values()
            .find_map(|r| r.cpu.as_ref())
            .unwrap()
            .binding;
        let map = program
            .units
            .iter()
            .find(|u| u.unit().header.instance == binding.map)
            .unwrap();
        let claim = binding.preclaim.index();
        let mut changed = map.clone().into_raw();
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
            hirsel_lir::ahead::CpuMapperBinding::derive(
                program
                    .units
                    .iter()
                    .find(|u| u.unit().header.instance == binding.caller)
                    .unwrap(),
                binding.invocation,
                &changed,
                worker,
                program
                    .units
                    .iter()
                    .find(|u| Some(u.unit().header.instance) == binding.callback.instance)
                    .unwrap(),
                &program.table,
                &std::collections::BTreeMap::from([(
                    binding.callback.instance.unwrap(),
                    binding.environment
                )])
            )
            .is_none(),
            "leave is not a job claim"
        );
        for field in 0..10 {
            let mut bad = protocol.clone();
            match field {
                0 => bad.digest = hirsel_syntax::ContentHash::of_bytes(b"changed worker"),
                1 => bad.shared_mapper = bad.jobs,
                2 => bad.jobs = bad.shared_mapper,
                3 => bad.live = bad.jobs,
                4 => bad.mapper = bad.finish,
                5 => bad.claim = Some(bad.finish),
                6 => bad.result = bad.jobs,
                7 => bad.job_layout = bad.environment,
                8 => bad.result_layout = bad.environment,
                _ => bad.ordinal = bad.live,
            }
            assert!(!bad.check(worker, &program.table), "changed field {field}");
        }
    });
}

#[test]
fn cpu_allocating_mapper_graph_binds_heap_handoffs_cleanup_and_both_final_frames() {
    with_compiler_stack(|| {
        let source = SOURCE.replace("job + 7u64", "{ let xs = [job]; xs[0] + 7u64 }");
        let regional = ready(&source);
        assert!(
            !native::ahead_plan(&regional)
                .roots
                .values()
                .any(|root| root.cpu.is_some()),
            "a region allocation still needs its own handoff"
        );
        let program = ready_with_regions(&source, false);
        let plan = native::ahead_plan(&program);
        let (root_id, root) = plan
            .roots
            .iter()
            .find(|(_, root)| root.cpu.as_ref().is_some_and(|cpu| cpu.mapper.is_some()))
            .unwrap_or_else(|| panic!("{:?}", plan.refusals));
        let mapper = root.cpu.as_ref().unwrap().mapper.as_ref().unwrap();
        assert!(!mapper.allocations.is_empty());
        assert!(!mapper.cleanup.is_empty());
        let units: Vec<_> = program.units.iter().collect();
        assert!(hirsel_lir::ahead::validate_cpu_mapper_graph(
            &units,
            &program.table,
            &program.target,
            mapper,
            program.fuel,
            mapper.helpers
        ));
        assert!(hirsel_lir::ahead::validate_cpu_workers(
            &units,
            &program.table,
            &program.target,
            &plan,
            program.fuel
        ));
        for field in 0..5 {
            let mut forged = mapper.clone();
            match field {
                0 => forged.allocations.clear(),
                1 => forged.cleanup.clear(),
                2 => {
                    forged
                        .graph
                        .bodies
                        .get_mut(&forged.graph.root)
                        .unwrap()
                        .calls
                        .insert(hirsel_syntax::ContentHash::of_bytes(b"missing callee"));
                }
                3 => forged
                    .spines
                    .values_mut()
                    .find(|s| !s.barriers.is_empty())
                    .unwrap()
                    .barriers
                    .clear(),
                _ => forged.helpers = hirsel_syntax::ContentHash::of_bytes(b"wrong helpers"),
            }
            assert!(
                !hirsel_lir::ahead::validate_cpu_mapper_graph(
                    &units,
                    &program.table,
                    &program.target,
                    &forged,
                    program.fuel,
                    mapper.helpers
                ),
                "forged field {field}"
            );
        }
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let objects = native::emit_backend_objects(
                &program,
                backend,
                hirsel_lir::emission::OptLevel::Default,
                native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
            native::check_ahead_objects(&program, &objects).unwrap();
            assert!(
                objects.iter().any(|o| o
                    .imports
                    .iter()
                    .any(|s| s.name == "hirsel_rt_v2_cpu_ahead_alloc")),
                "{backend:?} allocation bypassed its handoff"
            );
            let evidence = objects.iter().find_map(|o| o.ahead.as_ref()).unwrap();
            assert!(
                evidence
                    .bounds
                    .iter()
                    .any(|(id, n)| id == root_id && *n > 80)
            );
            let certificates = native::continuation::ahead_certificates(&program, &objects);
            assert!(certificates.iter().any(|c| c.root == *root_id));
            let mut forged = objects.clone();
            for object in &mut forged {
                if let Some(evidence) = &mut object.ahead {
                    evidence.frames.pop();
                }
            }
            assert!(native::check_ahead_objects(&program, &forged).is_err());
        }
        assert_eq!(root.handoff_bytes(64), 80);
    });
}
