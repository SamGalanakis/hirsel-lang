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
    ready_with_specialization(source, regions, true)
}

fn ready_with_specialization(
    source: &str,
    regions: bool,
    specialize: bool,
) -> native::ReadyProgram {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("cpu.hir", source.to_owned()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let mut options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
    options.regions = regions;
    options.scoped_specialization = specialize;
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
        for field in 0..7 {
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
                5 => {
                    root.cpu.as_mut().unwrap().binding.caller_digest =
                        hirsel_syntax::ContentHash::default()
                }
                _ => {
                    let worker = &mut root.cpu.as_mut().unwrap().binding.worker;
                    worker.observations[0] = worker.finish;
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

#[test]
fn canonical_caller_keeps_an_ordinary_call_to_a_cpu_mapper() {
    use hirsel_lir::{ahead::*, emission::*, ir::*};
    use std::collections::{BTreeMap, BTreeSet};
    with_compiler_stack(|| {
        let source = SOURCE
            .replace("  let outcome =", "  let probe = fn(take job: u64) -> u64 uses {} { let xs = [job]; xs[0] + 7u64 }(1u64)\n  failure.check(probe == 8u64, Bounds())\n  let outcome =")
            .replace("fn(take job: u64) -> u64 uses {} { job + 7u64 }", "fn(take job: u64) -> u64 uses {} { let xs = [job]; xs[0] + 7u64 }");
        let mut program = ready_with_regions(&source, false);
        let initial = native::ahead_plan(&program);
        let binding = &initial
            .roots
            .values()
            .find(|root| root.cpu.as_ref().is_some_and(|cpu| cpu.mapper.is_some()))
            .unwrap()
            .cpu
            .as_ref()
            .unwrap()
            .binding;
        let caller_id = binding.caller;
        let mapper_id = binding.callback.instance.unwrap();
        let index = program
            .units
            .iter()
            .position(|u| u.unit().header.instance == caller_id)
            .unwrap();
        let mut caller = program.units[index].clone().into_raw();
        let call = caller
            .tables
            .calls
            .iter()
            .position(|call| call.callee.value().is_some())
            .unwrap();
        let closure = caller.tables.calls[call].callee.value().unwrap();
        let code = SymbolId(caller.tables.symbols.len() as u32);
        caller.tables.symbols.push(binding.callback.clone());
        // Refine the identical local probe to the mapper's compatible named
        // body, retaining strict C admission before testing physical routing.
        caller.tables.calls[call].callee = Callee::Bound { code, closure };
        let inputs = hirsel_lir::validate::Inputs {
            target: &program.target,
            layouts: &program.table,
            manifest: &program.manifest,
            operations: &program.operations,
            discharge: Some(&program.discharged),
            memo: Default::default(),
        };
        program.units[index] = hirsel_lir::validate::validate_ready(caller, &inputs).unwrap();
        let mut plan = native::ahead_plan(&program);
        let cpu_root = plan
            .roots
            .values()
            .find(|root| root.cpu.as_ref().is_some_and(|cpu| cpu.mapper.is_some()))
            .unwrap()
            .clone();
        let caller = program.units[index].unit();
        // A fully canonical adapter has no private frame edges. The same
        // callee still needs a CPU alternate for the other root in this plan.
        let body = ScalarBody {
            body: caller.digest(),
            calls: BTreeSet::new(),
            fuel_points: hirsel_lir::fuel::fuel_points(caller),
        };
        let graph = ScalarGraph {
            root: caller_id,
            bodies: BTreeMap::from([(caller_id, body.clone())]),
            identity: hirsel_syntax::ContentHash::of_bytes(b"canonical adapter"),
        };
        let spine = Spine {
            private_instructions: vec![false; caller.instructions.len()],
            scalar_calls: BTreeSet::new(),
            reached_calls: BTreeSet::new(),
            suspended: BTreeMap::new(),
            barriers: (0..caller.instructions.len()).collect(),
            deferred_releases: BTreeMap::new(),
        };
        plan.bodies.insert(caller_id, body);
        plan.roots.insert(
            caller_id,
            RootPlan {
                graph: graph.clone(),
                recipe: cpu_root.recipe.clone(),
                host: Some(HostReturnGraph {
                    graph,
                    spines: BTreeMap::from([(caller_id, spine)]),
                    reads: BTreeMap::new(),
                }),
                cpu: None,
            },
        );
        let all: Vec<_> = program.units.iter().collect();
        assert!(validate_cpu_workers(
            &all,
            &program.table,
            &program.target,
            &plan,
            program.fuel
        ));
        let mut bad = plan.clone();
        let host = bad.roots.get_mut(&caller_id).unwrap();
        host.graph
            .bodies
            .get_mut(&caller_id)
            .unwrap()
            .calls
            .insert(mapper_id);
        host.graph.bodies.extend(
            cpu_root
                .cpu
                .as_ref()
                .unwrap()
                .mapper
                .as_ref()
                .unwrap()
                .graph
                .bodies
                .clone(),
        );
        assert!(
            !validate_cpu_workers(&all, &program.table, &program.target, &bad, program.fuel),
            "a non-CPU frame graph cannot borrow CPU allocation frames"
        );
        let full_plan = plan.clone();
        let cpu_units: Vec<_> = program
            .units
            .iter()
            .filter(|u| {
                full_plan.bodies.contains_key(&u.unit().header.instance)
                    && u.unit().header.instance != caller_id
            })
            .collect();
        plan.roots.retain(|id, _| *id == caller_id);
        let caller_units = vec![&program.units[index]];
        let empty = BTreeSet::new();
        let inputs = EmitInputs {
            ahead: Some(&plan),
            continuation: None,
            alternate: true,
            index: None,
            input: &program.input,
            target: &program.target,
            layouts: &program.table,
            keys: &program.keys,
            manifest: &program.manifest,
            statics: &program.statics,
            inline_instances: &empty,
            available_externally: &empty,
            callbacks: !program.callbacks.is_empty(),
            tracking: program.tracking(),
            foreign: &program.foreign_dispatch,
            opt: OptLevel::Default,
            measurement: false,
            fuel: program.fuel,
            verify_backend: true,
        };
        let symbol = &plan.symbols[&mapper_id];
        let ordinary = symbol_name(&symbol.definition, Some(&mapper_id));
        let alternate = alternate_name(&symbol.definition, mapper_id);
        let cpu_inputs = EmitInputs {
            ahead: Some(&full_plan),
            ..inputs.clone()
        };
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let (cpu, caller) = match backend {
                native::Backend::Llvm => (
                    hirsel_llvm::emit_ahead_object(&cpu_units, &cpu_inputs)
                        .unwrap()
                        .0,
                    hirsel_llvm::emit_ahead_object(&caller_units, &inputs)
                        .unwrap()
                        .0,
                ),
                native::Backend::Cranelift => (
                    hirsel_cranelift::emit_ahead_object(&cpu_units, &cpu_inputs)
                        .unwrap()
                        .0,
                    hirsel_cranelift::emit_ahead_object(&caller_units, &inputs)
                        .unwrap()
                        .0,
                ),
            };
            assert!(cpu.exports.iter().any(|s| s.name == alternate));
            assert!(caller.imports.iter().any(|s| s.name == ordinary));
            assert!(
                !caller.imports.iter().any(|s| s.name == alternate),
                "{backend:?}: canonical caller selected a CPU-only alternate"
            );
        }
    });
}

#[test]
fn cpu_saved_summary_checks_live_result_ordinal_and_retained_owner_on_both_emitters() {
    use hirsel_lir::ahead::validate_cpu_workers;
    with_compiler_stack(|| {
        for mode in 0..4 {
            let owned = matches!(mode, 1 | 2);
            let source = if mode == 2 {
                SOURCE.replace(
                    "fn() -> List[u64] uses {} { [1u64, 2u64, 3u64, 4u64] }",
                    "fn() -> List[unique List[u64]] uses {} { [[1u64], [2u64], [3u64], [4u64]] }")
                    .replace("fn(take job: u64) -> u64 uses {} { job + 7u64 }",
                        "fn(take job: unique List[u64]) -> unique List[u64] uses {} { take job }")
                    .replace("[8u64, 9u64, 10u64, 11u64]", "[[1u64], [2u64], [3u64], [4u64]]")
            } else if owned {
                SOURCE
                    .replace(
                        "fn(take job: u64) -> u64 uses {} { job + 7u64 }",
                        "fn(take job: u64) -> unique List[u64] uses {} { [job + 7u64] }",
                    )
                    .replace(
                        "[8u64, 9u64, 10u64, 11u64]",
                        "[[8u64], [9u64], [10u64], [11u64]]",
                    )
            } else {
                SOURCE.replace(
                    "job + 7u64",
                    "fn(x: u64) -> u64 uses {} { let xs = [x]; xs[0] + 7u64 }(job)",
                )
            };
            let program = ready_with_specialization(&source, false, mode != 3);
            let plan = native::ahead_plan(&program);
            let (&id, root) = plan
                .roots
                .iter()
                .find(|(_, root)| root.cpu.as_ref().is_some_and(|cpu| cpu.mapper.is_some()))
                .unwrap_or_else(|| panic!("mode {mode}: {:?}", plan.refusals));
            let cpu = root.cpu.as_ref().unwrap();
            let summary = cpu.summary.as_ref().unwrap();
            assert_eq!(summary.retained, owned);
            if mode == 2 {
                assert!(cpu.mapper.as_ref().unwrap().allocations.is_empty());
                assert_eq!(root.handoff_bytes(64), 80);
            }
            assert_eq!(summary.call, cpu.binding.worker.mapper);
            assert_eq!(summary.ordinal, cpu.binding.worker.ordinal);
            if !owned {
                assert!(
                    cpu.mapper.as_ref().unwrap().graph.bodies.len() >= 2,
                    "the allocating helper must remain a real Ready call"
                );
            }
            let all: Vec<_> = program.units.iter().collect();
            assert!(validate_cpu_workers(
                &all,
                &program.table,
                &program.target,
                &plan,
                program.fuel
            ));
            for field in 0..7 {
                let mut forged = plan.clone();
                let cpu = forged.roots.get_mut(&id).unwrap().cpu.as_mut().unwrap();
                let summary = cpu.summary.as_mut().unwrap();
                match field {
                    0 => summary.call = cpu.binding.worker.finish,
                    1 => summary.value = hirsel_lir::ir::ValueId(u32::MAX),
                    2 => summary.layout = cpu.binding.worker.environment,
                    3 => summary.bytes += 1,
                    4 => summary.ordinal = cpu.binding.worker.live,
                    5 => summary.retained = !summary.retained,
                    _ => cpu.summary = None,
                }
                assert!(
                    !validate_cpu_workers(
                        &all,
                        &program.table,
                        &program.target,
                        &forged,
                        program.fuel
                    ),
                    "forged summary field {field}"
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
                assert!(objects.iter().any(|object| {
                    object
                        .imports
                        .iter()
                        .any(|symbol| symbol.name == "hirsel_rt_v2_cpu_ahead_publish")
                }));
                for object in objects.iter().filter(|object| {
                    object.ahead.as_ref().is_some_and(|evidence| {
                        hirsel_syntax::ContentHash::of_bytes(&object.bytes) == evidence.object
                    })
                }) {
                    for helper in root
                        .cpu
                        .as_ref()
                        .unwrap()
                        .mapper
                        .as_ref()
                        .unwrap()
                        .graph
                        .bodies
                        .keys()
                    {
                        let symbol = &plan.symbols[helper];
                        let ordinary =
                            hirsel_lir::emission::symbol_name(&symbol.definition, Some(helper));
                        assert!(
                            !object.imports.iter().any(|symbol| symbol.name == ordinary),
                            "{backend:?} private helper retained its ordinary code word: {ordinary}"
                        );
                    }
                }
                let evidence = objects
                    .iter()
                    .find_map(|object| object.ahead.as_ref())
                    .unwrap();
                assert_eq!(root.graph.bodies.len(), if owned { 2 } else { 3 });
                let frames: std::collections::BTreeMap<_, _> =
                    evidence.frames.iter().copied().collect();
                // This fixture is one worker -> mapper -> optional helper chain.
                // Every real caller frame and the naked handoff must fit together.
                let expected = 16 + 80 + root.graph.bodies.keys().map(|id| frames[id]).sum::<u64>();
                assert!(
                    evidence.bounds.contains(&(id, expected)),
                    "{backend:?} helper frame composition: expected {expected}, bounds {:?}",
                    evidence.bounds
                );
                assert!(
                    native::continuation::ahead_certificates(&program, &objects)
                        .iter()
                        .any(|certificate| certificate.root == id)
                );
            }
            assert!(program.units.iter().all(|unit| {
                unit.unit()
                    .tables
                    .calls
                    .iter()
                    .enumerate()
                    .all(|(index, _)| {
                        plan.cpu_start(unit.unit(), hirsel_lir::ir::CallId(index as u32))
                            .is_none()
                    })
            }));
        }
        let captured = SOURCE.replace(
            "job + 7u64",
            "{ let ys = [job]; fn() -> u64 uses {} { ys[0] + 7u64 }() }",
        );
        let captured = ready_with_specialization(&captured, false, false);
        assert!(
            !native::ahead_plan(&captured)
                .roots
                .values()
                .any(|root| root.cpu.is_some()),
            "a captured owner still needs its own environment handoff"
        );
        let nested = SOURCE
            .replace(
                "fn(take job: u64) -> u64 uses {} { job + 7u64 }",
                "fn(take job: u64) -> unique List[List[u64]] uses {} { [[job + 7u64]] }",
            )
            .replace(
                "[8u64, 9u64, 10u64, 11u64]",
                "[[[8u64]], [[9u64]], [[10u64]], [[11u64]]]",
            );
        let program = ready_with_regions(&nested, false);
        let plan = native::ahead_plan(&program);
        assert!(
            plan.refusals
                .values()
                .any(|reason| reason == "CPU summary needs a complete retained-owner recipe"),
            "{:?}",
            plan.refusals
        );
    });
}

#[test]
fn cpu_save_roots_check_inputs_mapper_locals_results_and_parent_frames() {
    use hirsel_lir::ahead::{CpuSavePoint, validate_cpu_workers};
    with_compiler_stack(|| {
        let source = SOURCE.replace("job + 7u64", "{ let kept = [job]; let answer = fn(x: u64) -> u64 uses {} { let child = [x]; child[0] + 7u64 }(job); kept[0] + answer - job }");
        let program = ready_with_specialization(&source, false, false);
        let plan = native::ahead_plan(&program);
        let (&id, root) = plan
            .roots
            .iter()
            .find(|(_, root)| root.cpu.as_ref().is_some_and(|cpu| cpu.mapper.is_some()))
            .unwrap_or_else(|| panic!("{:?}", plan.refusals));
        let cpu = root.cpu.as_ref().unwrap();
        let roots = cpu.roots.as_ref().unwrap();
        assert_eq!(roots.frames.len(), root.graph.bodies.len());
        assert!(roots.parents.len() >= 2, "worker, mapper and a real helper");
        let parent = roots
            .parents
            .iter()
            .find_map(|(&(caller, call), _)| (caller != id).then_some((caller, call)))
            .unwrap();
        let unit = program
            .units
            .iter()
            .find(|u| u.unit().header.instance == parent.0)
            .unwrap()
            .unit();
        let block = unit.terminators.iter().position(|term| matches!(term, hirsel_lir::ir::Terminator::Call { call, .. } if *call == parent.1)).unwrap();
        let point = CpuSavePoint::Terminator(hirsel_lir::ir::BlockId(block as u32));
        let frame = &roots.frames[&parent.0][&point];
        assert!(
            frame
                .values
                .iter()
                .any(|(_, layout, _)| !hirsel_lir::ahead::scalar_image(&program.table, *layout))
                || frame
                    .slots
                    .iter()
                    .any(|(_, layout)| !hirsel_lir::ahead::scalar_image(&program.table, *layout)),
            "parent's kept allocation survives the helper save"
        );
        let loan = roots
            .frames
            .iter()
            .find_map(|(body, points)| {
                points
                    .iter()
                    .find_map(|(point, frame)| (!frame.loans.is_empty()).then_some((*body, *point)))
            })
            .expect("worker's shared mapper loan remains live across its call");
        let all: Vec<_> = program.units.iter().collect();
        assert!(validate_cpu_workers(
            &all,
            &program.table,
            &program.target,
            &plan,
            program.fuel
        ));
        let mut accepted = Vec::new();
        for kind in 0..10 {
            let mut bad = plan.clone();
            let roots = bad
                .roots
                .get_mut(&id)
                .unwrap()
                .cpu
                .as_mut()
                .unwrap()
                .roots
                .as_mut()
                .unwrap();
            match kind {
                0 => roots.input.slot = roots.mapper.slot,
                1 => roots.mapper.offset += 8,
                2 => roots.result.0 = hirsel_lir::ir::ValueId(u32::MAX),
                3 => {
                    roots
                        .frames
                        .get_mut(&parent.0)
                        .unwrap()
                        .get_mut(&point)
                        .unwrap()
                        .values
                        .clear();
                }
                4 => {
                    roots
                        .frames
                        .get_mut(&parent.0)
                        .unwrap()
                        .get_mut(&point)
                        .unwrap()
                        .slots
                        .clear();
                }
                5 => {
                    roots.parents.remove(&parent);
                }
                6 => {
                    roots.frames.remove(&parent.0);
                }
                7 => {
                    roots.frames.get_mut(&parent.0).unwrap().remove(&point);
                }
                8 => roots.input.layout = cpu.binding.worker.environment,
                _ => roots
                    .frames
                    .get_mut(&loan.0)
                    .unwrap()
                    .get_mut(&loan.1)
                    .unwrap()
                    .loans
                    .clear(),
            }
            assert_ne!(bad, plan, "forge {kind} changes actual roots");
            if validate_cpu_workers(&all, &program.table, &program.target, &bad, program.fuel) {
                accepted.push(kind);
            }
        }
        assert!(
            accepted.is_empty(),
            "accepted forged save-root kinds {accepted:?}"
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
                    .any(|c| c.root == id)
            );
        }
    });
}

#[test]
fn cpu_saved_growth_checks_inputs_and_composes_private_helper_frames() {
    with_compiler_stack(|| {
        let source = SOURCE
            .replace("fn() -> List[u64] uses {} { [1u64, 2u64, 3u64, 4u64] }",
                "fn() -> List[unique List[u64]] uses {} { [[1u64], [2u64], [3u64], [4u64]] }")
            .replace("fn(take job: u64) -> u64 uses {} { job + 7u64 }",
                "fn(take job: unique List[u64]) -> unique List[u64] uses {} { let mut out = take job; list.reserve(mut out, 128u64); take out }")
            .replace("[8u64, 9u64, 10u64, 11u64]", "[[1u64], [2u64], [3u64], [4u64]]");
        let program = ready_with_regions(&source, false);
        let plan = native::ahead_plan(&program);
        let (&id, root) = plan
            .roots
            .iter()
            .find(|(_, r)| {
                r.cpu
                    .as_ref()
                    .and_then(|c| c.mapper.as_ref())
                    .is_some_and(|m| !m.growth.is_empty())
            })
            .unwrap_or_else(|| panic!("{:?}", plan.refusals));
        let mapper = root.cpu.as_ref().unwrap().mapper.as_ref().unwrap();
        let (&key, recipe) = mapper.growth.iter().next().unwrap();
        assert_eq!(recipe.size, 8);
        assert_eq!(recipe.align, 8);
        let (_, offset) = plan.cpu_growth(key.0, key.1, &program.table).unwrap();
        let wrapper = program
            .table
            .layout(root.cpu.as_ref().unwrap().binding.worker.environment)
            .unwrap();
        assert_eq!(
            offset,
            hirsel_lir::layout::payload_offset(wrapper.align).unwrap() + wrapper.fields[4].offset
        );
        let all: Vec<_> = program.units.iter().collect();
        assert!(hirsel_lir::ahead::validate_cpu_workers(
            &all,
            &program.table,
            &program.target,
            &plan,
            program.fuel
        ));
        for field in 0..8 {
            let mut forged = plan.clone();
            let growth = &mut forged
                .roots
                .get_mut(&id)
                .unwrap()
                .cpu
                .as_mut()
                .unwrap()
                .mapper
                .as_mut()
                .unwrap()
                .growth;
            let recipe = growth.get_mut(&key).unwrap();
            match field {
                0 => recipe.address = hirsel_lir::ir::AddressId(u32::MAX),
                1 => recipe.capacity = hirsel_lir::ir::ValueId(u32::MAX),
                2 => recipe.layout = recipe.element,
                3 => recipe.element = recipe.layout,
                4 => recipe.size += 1,
                5 => recipe.align *= 2,
                6 => {
                    growth.clear();
                }
                _ => {
                    let recipe = growth.remove(&key).unwrap();
                    growth.insert((key.0, usize::MAX), recipe);
                }
            }
            // Recompute the producer's identity. Rejection must come from
            // reconstruction of the actual Ready body, not a stale digest.
            let mapper = forged
                .roots
                .get_mut(&id)
                .unwrap()
                .cpu
                .as_mut()
                .unwrap()
                .mapper
                .as_mut()
                .unwrap();
            mapper.graph.identity = hirsel_syntax::ContentHash::of_parts(
                "hirsel.ahead.cpu-mapper.v1",
                [
                    program.table.digest().0.as_slice(),
                    program.target.digest().0.as_slice(),
                    mapper.helpers.0.as_slice(),
                    &[u8::from(program.fuel)],
                    format!(
                        "{:?}{:?}{:?}{:?}{:?}{:?}",
                        mapper.graph.root,
                        mapper.graph.bodies,
                        mapper.allocations,
                        mapper.growth,
                        mapper.cleanup,
                        mapper.spines
                    )
                    .as_bytes(),
                ],
            );
            assert!(
                !hirsel_lir::ahead::validate_cpu_workers(
                    &all,
                    &program.table,
                    &program.target,
                    &forged,
                    program.fuel
                ),
                "forged growth {field}"
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
            let evidence = objects.iter().find_map(|o| o.ahead.as_ref()).unwrap();
            let frames: std::collections::BTreeMap<_, _> =
                evidence.frames.iter().copied().collect();
            assert_eq!(root.graph.bodies.len(), 3, "worker -> mapper -> reserve");
            let expected = 16 + 80 + root.graph.bodies.keys().map(|id| frames[id]).sum::<u64>();
            assert!(
                evidence.bounds.contains(&(id, expected)),
                "{backend:?}: {:?}",
                evidence.bounds
            );
            assert!(
                objects.iter().any(|o| o
                    .imports
                    .iter()
                    .any(|s| s.name == "hirsel_rt_v2_cpu_ahead_grow")),
                "{backend:?}"
            );
            assert!(
                native::continuation::ahead_certificates(&program, &objects)
                    .iter()
                    .any(|c| c.root == id),
                "{backend:?}"
            );
        }
        let mixed = source.replace(
            "  Ok(())",
            r#"
  let _ = core@1/cpu.map_scoped(tasks, 4,
    fn() -> List[(u64, u64)] uses {} { [(1u64, 2u64)] },
    fn(take pair: (u64, u64)) -> unique List[u64] uses {} {
      let mut out = [1u64]; list.reserve(mut out, 128u64); take out
    })
  Ok(())"#,
        );
        let mixed = ready_with_regions(&mixed, false);
        let plan = native::ahead_plan(&mixed);
        let offsets: std::collections::BTreeSet<_> = plan
            .cpu_workers
            .values()
            .map(|worker| mixed.table.layout(worker.environment).unwrap().fields[4].offset)
            .collect();
        assert!(
            offsets.len() >= 2,
            "the two original worker ordinal locations differ"
        );
        assert!(
            plan.refusals
                .values()
                .any(|reason| reason == "ambiguous CPU growth ordinal custody"),
            "{:?}",
            plan.refusals
        );
        let all: Vec<_> = mixed.units.iter().collect();
        assert!(hirsel_lir::ahead::validate_cpu_workers(
            &all,
            &mixed.table,
            &mixed.target,
            &plan,
            mixed.fuel
        ));
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            native::emit_backend_objects(
                &mixed,
                backend,
                hirsel_lir::emission::OptLevel::Default,
                native::Partition::Whole,
                false,
                None,
            )
            .unwrap();
        }
    });
}
