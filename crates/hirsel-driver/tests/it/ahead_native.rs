//! Ordinary scalar tasks: final object custody, partitions and four-engine semantics.
use hirsel_driver::native;
use hirsel_engine::{QueryEngine, check_progress, default_context, with_compiler_stack};
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_syntax::ContentHash;
use std::collections::BTreeSet;

const SOURCE: &str = r#"
fn work(seed:u64)->u64 uses {} {
  let mut x=seed
  for i in 0u64..64u64 {
    x=int.wrapping_add(int.wrapping_mul(x ^ (x >> 7),0x9e3779b97f4a7c15u64),i)
  }
  x
}
pub fn main(rt:Io)->Result[(),GrantError] {
  let tasks=io.tasks(rt)?
  let output=io.stdout(rt)?
  let sum=task.scope(tasks,fn(s:Scope)->u64 {
    let mut set:TaskSet[Result[u64,()]] of s=task.set(s)
    for seed in 1u64..5u64 {
      task.add(s,mut set,task.async(s,fn()->Result[u64,()] {Ok(work(seed))}))
    }
    let mut sum=0u64
    if let Ok(rows)=std@1/task.all(s,take set) {for x in rows {sum=sum ^ x}}
    sum
  })
  let _=out.print(output,f"{sum}\n")
  Ok(())
}
"#;

fn checked() -> (QueryEngine, hirsel_engine::ValidatedNativeInput) {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("main.hir", SOURCE.to_owned()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let snapshot = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(snapshot).unwrap();
    (engine, validated)
}
fn metadata(
    program: &native::ReadyProgram,
    plan: &hirsel_lir::ahead::AheadPlan,
    bounds: &std::collections::BTreeMap<ContentHash, u64>,
) -> hirsel_lir::stage::ObjectArtifact {
    let empty = BTreeSet::new();
    let inputs = EmitInputs {
        ahead: Some(plan),
        alternate: true,
        index: None,
        input: &program.input,
        target: &program.target,
        layouts: &program.table,
        keys: &program.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &program.inline_instances,
        available_externally: &empty,
        callbacks: !program.callbacks.is_empty(),
        tracking: program.tracking(),
        foreign: &program.foreign_dispatch,
        opt: OptLevel::Default,
        measurement: false,
        fuel: program.fuel,
        verify_backend: true,
    };
    let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
        &program.table,
        &program.finalizers,
    );
    hirsel_cranelift::emit_ahead_metadata(&inputs, plan, bounds, &registry).unwrap()
}

#[test]
fn both_backends_emit_real_alternate_dags_across_partitions_and_reject_corruption() {
    with_compiler_stack(|| {
        let (_, validated) = checked();
        let program = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let plan = native::ahead_plan(&program);
        assert_eq!(plan.roots.len(), 1, "{:?}", plan.refusals);
        assert_eq!(plan.bodies.len(), 3);
        assert_eq!(plan.spawns.len(), 1);
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            for partition in [
                native::Partition::Whole,
                native::Partition::Singleton,
                native::Partition::Random(37),
            ] {
                let objects = native::emit_backend_objects(
                    &program,
                    backend,
                    OptLevel::Default,
                    partition,
                    false,
                    None,
                )
                .unwrap();
                native::check_ahead_objects(&program, &objects).unwrap();
                let code = objects
                    .iter()
                    .position(|o| {
                        o.ahead
                            .as_ref()
                            .is_some_and(|e| ContentHash::of_bytes(&o.bytes) == e.object)
                    })
                    .unwrap();
                let meta = objects
                    .iter()
                    .position(|o| {
                        o.exports
                            .iter()
                            .any(|s| s.name.starts_with("hirsel_ahead_"))
                    })
                    .unwrap();
                // The same real source DAG now carries logical invocations;
                // they remain admitted on private stacks, never all-refused.
                for helper in [
                    "hirsel_rt_v2_depth_enter_at_site",
                    "hirsel_rt_v2_depth_leave",
                ] {
                    assert!(
                        objects[code].imports.iter().any(|s| s.name == helper),
                        "{backend:?}: missing certified depth helper {helper}"
                    );
                }
                assert!(program.units.iter().any(|u| {
                    u.unit()
                        .instructions
                        .iter()
                        .any(|inst| matches!(inst, hirsel_lir::ir::Inst::FrameEnter { .. }))
                }));
                let evidence = objects[code].ahead.as_ref().unwrap();
                assert_eq!(evidence.frames.len(), 3);
                assert_eq!(evidence.bounds.len(), 1);
                for (id, symbol) in plan
                    .symbols
                    .iter()
                    .filter(|(id, _)| plan.bodies.contains_key(id))
                {
                    let name = hirsel_lir::ahead::alternate_name(&symbol.definition, *id);
                    assert_eq!(
                        objects
                            .iter()
                            .flat_map(|o| &o.exports)
                            .filter(|s| s.name == name)
                            .count(),
                        1
                    );
                }
                // Every altered certificate is rejected before a linker runs.
                for mutation in 0..7 {
                    let mut bad = objects.clone();
                    match mutation {
                        0 => {
                            bad[meta].ahead.as_mut().unwrap().plan =
                                ContentHash::of_bytes(b"stale body/fuel/layout/helper/target")
                        }
                        1 => bad[code].bytes[0] ^= 1,
                        2 => bad[code].ahead.as_mut().unwrap().frames[0].1 += 16,
                        3 => bad[meta].bytes[0] ^= 1,
                        4 => {
                            bad[code].reproducibility.options =
                                ContentHash::of_bytes(b"stale options")
                        }
                        5 => {
                            bad.remove(code);
                        }
                        _ => {
                            // Rename a real getter import in the final object,
                            // keeping exact object/sidecar hashes coherent. The
                            // unproved machine helper must still block entries.
                            let old = b"hirsel_rt_v2_ahead_state\0";
                            let at = bad[code]
                                .bytes
                                .windows(old.len())
                                .position(|w| w == old)
                                .unwrap();
                            bad[code].bytes[at + 13] = b'x';
                            let import = bad[code]
                                .imports
                                .iter_mut()
                                .find(|s| s.name == "hirsel_rt_v2_ahead_state")
                                .unwrap();
                            import.name = "hirsel_rt_v2_xhead_state".into();
                            let mut evidence = bad[code].ahead.clone().unwrap();
                            evidence.object = ContentHash::of_bytes(&bad[code].bytes);
                            bad[code].ahead = Some(evidence.clone());
                            bad[meta].ahead = Some(evidence);
                        }
                    }
                    assert!(
                        native::check_ahead_objects(&program, &bad).is_err(),
                        "{backend:?}/{partition:?}/{mutation}"
                    );
                }
                // Missing evidence permits only immutable null-entry metadata.
                let mut refused = objects.clone();
                let mut evidence = evidence.clone();
                evidence.frames.clear();
                evidence.bounds.clear();
                refused[code].ahead = Some(evidence.clone());
                refused[meta] = metadata(&program, &plan, &Default::default());
                refused[meta].ahead = Some(evidence);
                native::check_ahead_objects(&program, &refused).unwrap();
                assert!(
                    !refused[meta]
                        .imports
                        .iter()
                        .any(|s| s.name.ends_with("_ahead"))
                );
                if matches!(backend, native::Backend::Llvm) {
                    // Strip only the evidence section's name in a real emitted
                    // object. Missing records are refusal, malformed bytes error.
                    let mut missing = refused.clone();
                    use object::{Object, ObjectSection};
                    let bytes = &missing[code].bytes;
                    let file = object::File::parse(bytes.as_slice()).unwrap();
                    let section = file.section_by_name(".stack_sizes").unwrap();
                    let index = section.index().0;
                    let (stack_at, _) = section.file_range().unwrap();
                    let u64_at =
                        |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
                    let header = u64_at(40);
                    let strings_index =
                        u16::from_le_bytes(bytes[62..64].try_into().unwrap()) as usize;
                    let strings = u64_at(header + strings_index * 64 + 24);
                    let name = u32::from_le_bytes(
                        bytes[header + index * 64..header + index * 64 + 4]
                            .try_into()
                            .unwrap(),
                    ) as usize;
                    let at = strings + name;
                    assert_eq!(&bytes[at..at + 13], b".stack_sizes\0");
                    let mut malformed = bytes.clone();
                    malformed[stack_at as usize] = 1;
                    assert!(hirsel_lir::object::llvm_stack_sizes(&malformed).is_err());
                    missing[code].bytes[at + 1] = b'x';
                    let mut evidence = missing[code].ahead.clone().unwrap();
                    evidence.object = ContentHash::of_bytes(&missing[code].bytes);
                    missing[code].ahead = Some(evidence.clone());
                    missing[meta].ahead = Some(evidence);
                    assert!(
                        hirsel_lir::object::llvm_stack_sizes(&missing[code].bytes)
                            .unwrap()
                            .is_empty()
                    );
                    native::check_ahead_objects(&program, &missing).unwrap();
                }
            }
        }
    });
}

#[test]
fn bounded_ordinary_spawn_twin_agrees_on_reference_checked_heap_and_both_natives() {
    with_compiler_stack(|| {
        let (mut engine, validated) = checked();
        let entry = hirsel_engine::entry_path(&default_context());
        let run = hirsel_engine::run_observed_snapshot(
            &mut engine,
            &hirsel_engine::RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            Default::default(),
        )
        .unwrap();
        let reference = native::Observation::of_reference(run.observation.as_ref().unwrap());
        assert_eq!(reference.output, b"18226251250264336309\n");
        let matrix = [
            native::Engine::Heap { campaign: false },
            native::Engine::Native {
                opt: OptLevel::Default,
                partition: native::Partition::Singleton,
                runtime: native::Runtime::Diagnostic,
            },
            native::Engine::Cranelift {
                partition: native::Partition::Random(37),
                runtime: native::Runtime::Diagnostic,
            },
        ]
        .into_iter()
        .map(|engine| native::Configuration {
            options: native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        })
        .collect::<Vec<_>>();
        let dir = tempfile::tempdir().unwrap();
        let tools = native::Toolchain::for_matrix(&dir.path().join("runtime"), &matrix).unwrap();
        for (configuration, outcome) in
            native::differential(&validated, &entry, &reference, &matrix, &tools, dir.path())
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

#[test]
fn certificates_bind_transitive_bodies_layouts_fuel_helpers_and_target_options() {
    with_compiler_stack(|| {
        let (_, validated) = checked();
        let mut program = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let helper = ContentHash::of_bytes(b"helper assembly v1");
        let proposed = |p: &native::ReadyProgram,
                        fuel,
                        helpers,
                        table: &hirsel_lir::layout_types::LayoutTable,
                        target: &native::TargetSpec| {
            hirsel_lir::ahead::AheadPlan::build(
                &p.units.iter().collect::<Vec<_>>(),
                table,
                target,
                &p.ahead_environments,
                fuel,
                helpers,
            )
        };
        let original = proposed(&program, false, helper, &program.table, &program.target);
        assert_eq!(original.roots.len(), 1);
        assert_ne!(
            original.identity,
            proposed(&program, true, helper, &program.table, &program.target).identity
        );
        assert_ne!(
            original.identity,
            proposed(
                &program,
                false,
                ContentHash::of_bytes(b"helper assembly v2"),
                &program.table,
                &program.target
            )
            .identity
        );
        let mut layout = program.table.clone();
        layout.layout_version += 1;
        assert_ne!(
            original.identity,
            proposed(&program, false, helper, &layout, &program.target).identity
        );
        let mut target = program.target.clone();
        target.features.push("+avx2".into());
        let refused = proposed(&program, false, helper, &program.table, &target);
        assert!(refused.roots.is_empty());
        assert_ne!(original.identity, refused.identity);
        target = native::TargetSpec::aarch64_unknown_linux_gnu();
        assert!(
            proposed(&program, false, helper, &program.table, &target)
                .roots
                .is_empty()
        );
        // Keep the wrapper/instance names fixed and change only the actual
        // validated transitive callee body, as a stale body cache would.
        let i = program
            .units
            .iter()
            .position(|u| u.unit().header.definition.0.name == "work")
            .unwrap();
        let mut unit = program.units.remove(i).into_raw();
        let constant = unit
            .tables
            .constants
            .iter_mut()
            .find(|c| matches!(c.value, hirsel_lir::ir::ConstantValue::Integer(_)))
            .unwrap();
        if let hirsel_lir::ir::ConstantValue::Integer(n) = &mut constant.value {
            *n += 1;
        }
        let inputs = hirsel_lir::validate::Inputs {
            target: &program.target,
            layouts: &program.table,
            manifest: &program.manifest,
            operations: &program.operations,
            discharge: Some(&program.discharged),
            memo: Default::default(),
        };
        program
            .units
            .push(hirsel_lir::validate::validate_ready(unit, &inputs).unwrap());
        let changed = proposed(&program, false, helper, &program.table, &program.target);
        assert_eq!(changed.roots.len(), 1);
        assert_eq!(
            original.roots.keys().collect::<Vec<_>>(),
            changed.roots.keys().collect::<Vec<_>>()
        );
        assert_ne!(original.identity, changed.identity);
    });
}

#[test]
fn depth_helpers_have_the_certified_fixed_machine_extent_in_every_runtime_variant() {
    use std::collections::BTreeMap;
    let target = native::TargetSpec::x86_64_unknown_linux_gnu();
    let variants = (0..8)
        .map(|mask| native::RuntimeBuild {
            diagnostic: mask & 1 != 0,
            system_allocator: mask & 2 != 0,
            measurement: mask & 4 != 0,
            memcheck: false,
            ownership_trace: false,
        })
        .collect::<Vec<_>>();
    let scratch = tempfile::tempdir().unwrap();
    let tools =
        native::Toolchain::in_tree_with_runtimes(scratch.path(), &[&target], &variants).unwrap();
    let helpers = [
        "hirsel_rt_v2_depth_enter_at_site",
        "hirsel_rt_v2_depth_leave",
    ];
    for archive in tools.runtime.values() {
        let output = std::process::Command::new(tools.clang.with_file_name("llvm-objdump"))
            .arg("--no-show-raw-insn")
            .arg(format!("--disassemble-symbols={}", helpers.join(",")))
            .arg(archive)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let mut bodies = BTreeMap::<&str, Vec<(u64, &str)>>::new();
        let mut current = None;
        for line in text.lines() {
            if let Some(helper) = helpers.iter().find(|h| line.ends_with(&format!("<{h}>:"))) {
                current = Some(*helper);
                continue;
            }
            if !line.is_empty() && !line.starts_with(char::is_whitespace) {
                current = None;
            }
            if let Some(helper) = current
                && let Some((address, instruction)) = line.split_once(':')
                && let Ok(address) = u64::from_str_radix(address.trim(), 16)
            {
                bodies
                    .entry(helper)
                    .or_default()
                    .push((address, instruction.trim()));
            }
        }
        for helper in helpers {
            let body = bodies.get(helper).expect("missing compiled depth helper");
            assert!(!body.is_empty());
            assert!(body.iter().any(|(_, i)| *i == "retq"));
            for (pc, instruction) in body {
                let opcode = instruction.split_whitespace().next().unwrap();
                assert!(
                    matches!(
                        opcode,
                        "movq"
                            | "movl"
                            | "movb"
                            | "movw"
                            | "movzbl"
                            | "movzwl"
                            | "cmpq"
                            | "cmpl"
                            | "cmpw"
                            | "testl"
                            | "xorl"
                            | "incq"
                            | "incw"
                            | "decq"
                            | "decw"
                            | "shlq"
                            | "jne"
                            | "je"
                            | "jae"
                            | "jmp"
                            | "retq"
                    ),
                    "{}: {helper}: nonleaf instruction {instruction}",
                    archive.display()
                );
                // No stack/red-zone use, callee-save clobber, call, trap or
                // indirect/backward edge can hide an unbounded helper span.
                for register in [
                    "%rsp", "%esp", "%rbp", "%ebp", "%rbx", "%ebx", "%r12", "%r13", "%r14", "%r15",
                ] {
                    assert!(!instruction.contains(register), "{helper}: {instruction}");
                }
                if opcode.starts_with('j') {
                    assert!(
                        instruction.contains(&format!("<{helper}+")),
                        "{instruction}"
                    );
                    let destination = instruction
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .trim_start_matches("0x");
                    assert!(
                        u64::from_str_radix(destination, 16).unwrap() > *pc,
                        "cyclic helper: {instruction}"
                    );
                }
            }
        }
    }
    // Neither helper changes SP: the sole call extent is the pushed return.
    assert_eq!(hirsel_rt::DEPTH_LEAF_STACK_BYTES, 8);
}
