//! A many-callable borrows its environment on each invocation (CLO-4, MODE-2).
use crate::rc_support::target;
use hirsel_driver::{compiler_adapters, native::*};
use hirsel_engine::*;

// Keep the compact call-site spelling for the independently shrunk variants.
const WITNESS: &str = r#"
fn invoke(take f: sendable fn() -> ()) {
  let g = unique copy f
  g()
}
pub fn main(t: Test) -> Result[(), ()] {
  let tasks = test.tasks(t)
  let m = mutex.create(tasks, unique 0u64)
  invoke(fn() { mutex.with(tasks, m, fn(mut n: unique u64) uses {} { n += 1 }) })
  test.check(t, mutex.with(tasks, m, fn(mut n: unique u64) -> u64 uses {} { n }) == 1)
  Ok(())
}
"#;

const RESOURCE: &str = r#"
resource type Guard { tasks: Tasks, m: Mutex[u64] }
drop(g) { mutex.with(g.tasks, g.m, fn(mut n: unique u64) uses {} { n += 1 }) }
fn invoke(take f: once fn() -> ()) { f() }
pub fn main(t: Test) -> Result[(), ()] {
  let tasks = test.tasks(t)
  let m = mutex.create(tasks, unique 0u64)
  let guard = Guard(tasks: tasks, m: copy m)
  invoke(fn() { let released = take guard })
  test.check(t, mutex.with(tasks, m, fn(mut n: unique u64) -> u64 uses {} { n }) == 1)
  Ok(())
}
"#;

fn cases() -> Vec<(&'static str, String)> {
    vec![
        ("witness", WITNESS.into()),
        ("direct", WITNESS.replace("let g = unique copy f\n  g()", "f()")),
        ("plain", WITNESS.replace("unique copy f", "copy f")),
        ("unsendable", WITNESS.replace("sendable fn", "fn")),
        ("twice", WITNESS.replace("  g()", "  g()\n  g()").replace("== 1", "== 2")),
        ("scoped", WITNESS.replace("take f: sendable fn", "f: fn")),
        ("once", WITNESS.replace("sendable fn", "sendable once fn").replace("let g = unique copy f", "let g = take f")),
        ("taken", WITNESS.replace("sendable fn", "sendable once fn").replace("let g = unique copy f", "let g = take f").replace("mutex.with(tasks, m, fn(mut n: unique u64) uses {} { n += 1 })", "{ let owned = take m; mutex.with(tasks, owned, fn(mut n: unique u64) uses {} { n += 1 }) }").replace("  invoke(", "  let saved = copy m\n  invoke(").replace("test.check(t, mutex.with(tasks, m,", "test.check(t, mutex.with(tasks, saved,")),
        ("record-capture", format!("type Holder {{ m: Mutex[u64] }}\n{}", WITNESS.replace("  invoke(", "  let holder = Holder(m: take m)\n  invoke(").replace("mutex.with(tasks, m,", "mutex.with(tasks, holder.m,"))),
        ("record-copy", r#"
type Holder { m: Mutex[u64] }
pub fn main(t: Test) -> Result[(), ()] {
  let tasks = test.tasks(t)
  let m = mutex.create(tasks, unique 0u64)
  let first = Holder(m: copy m)
  { let second = unique copy first; mutex.with(tasks, second.m, fn(mut n: unique u64) uses {} { n += 1 }) }
  test.check(t, mutex.with(tasks, first.m, fn(mut n: unique u64) -> u64 uses {} { n }) == 1)
  test.check(t, mutex.with(tasks, m, fn(mut n: unique u64) -> u64 uses {} { n }) == 1)
  Ok(())
}
"#.into()),
        ("resource-once", RESOURCE.into()),
        ("returned", WITNESS.replace("fn invoke(take f: sendable fn() -> ()) {\n  let g = unique copy f\n  g()\n}", "fn invoke(take f: sendable fn() -> ()) -> sendable fn() -> () { unique copy f }")
            .replace("  invoke(", "  let g = invoke(").replace("  test.check", "  g()\n  test.check")),
        ("atomic", WITNESS.replace("mutex.create(tasks, unique 0u64)", "atomic.create(tasks, 0u64)")
            .replace("mutex.with(tasks, m, fn(mut n: unique u64) uses {} { n += 1 })", "atomic.store(tasks, m, 1)")
            .replace("mutex.with(tasks, m, fn(mut n: unique u64) -> u64 uses {} { n })", "atomic.load(tasks, m)")),
        ("shared", WITNESS.replace("mutex.create(tasks, unique 0u64)", "shared.freeze(unique 7u64)")
            .replace("mutex.with(tasks, m, fn(mut n: unique u64) uses {} { n += 1 })", "failure.check(shared.with(m, fn(n: u64) -> u64 uses {} { n }) == 7, TextInvariant())")
            .replace("mutex.with(tasks, m, fn(mut n: unique u64) -> u64 uses {} { n }) == 1", "shared.with(m, fn(n: u64) -> u64 uses {} { n }) == 7")),
    ]
}

#[test]
fn reference_checked_h_llvm_cranelift() {
    with_compiler_stack(|| {
        let configurations = [
            Engine::Heap { campaign: true },
            Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
            Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        ]
        .map(|engine| Configuration {
            options: Options::new(target()),
            engine,
        });
        let directory = tempfile::tempdir().unwrap();
        let tools =
            Toolchain::for_matrix(&directory.path().join("runtime"), &configurations).unwrap();
        let expected = Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: vec![],
            failures: vec![],
            roots: vec![],
        };
        let mut failures = vec![];
        for (name, source) in cases() {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("closure-resource.hir", source).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&context);
            let launch = RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let reference = run_observed_snapshot(&mut engine, &launch, Default::default());
            eprintln!(
                "{name}: interpreter outcome: {:?}",
                reference.as_ref().map(|r| &r.outcome)
            );
            if !reference.as_ref().is_ok_and(|r| {
                r.observation
                    .as_ref()
                    .is_some_and(|o| Observation::of_reference(o) == expected)
            }) {
                failures.push(format!("{name}: interpreter: {reference:?}"));
            }
            let checked_run = run_observed_checked(&mut engine, &launch, Default::default());
            eprintln!(
                "{name}: checked interpreter outcome: {:?}",
                checked_run.as_ref().map(|r| &r.outcome)
            );
            if !checked_run.as_ref().is_ok_and(|r| {
                r.observation
                    .as_ref()
                    .is_some_and(|o| Observation::of_reference(o) == expected)
            }) {
                failures.push(format!("{name}: checked interpreter: {checked_run:?}"));
            }
            let input = engine.export_native_input().unwrap();

            let validated = engine.import_native_input(input).unwrap();
            let case_dir = directory.path().join(name);
            std::fs::create_dir_all(&case_dir).unwrap();
            for (configuration, outcome) in differential(
                &validated,
                &entry,
                &expected,
                &configurations,
                &tools,
                &case_dir,
            ) {
                eprintln!("{name}: {configuration}: {outcome:?}");
                if outcome != Outcome::Agree {
                    failures.push(format!("{name}: {configuration}: {outcome:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

#[test]
fn v_rejects_releasing_a_many_callables_environment_field() {
    use hirsel_tir::*;
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let context = default_context();
        let file = engine
            .sources
            .add("capture-owner.hir", WITNESS.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        for signature in input.signatures.values() {
            signatures.insert(signature.definition, (**signature).clone());
        }
        let entry = entry_path(&context);
        let original = &input.definitions[&entry];
        assert!(
            hirsel_validate::validate(original, &input.types, &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        let mut probes = 0;
        for (body_index, body) in original.bodies.iter().enumerate() {
            for (local_index, local) in body.locals.iter().enumerate() {
                if local.storage != Storage::Capture
                    || !matches!(
                        &input.types.row(local.ty).kind,
                        hirsel_types::TypeKind::Builtin {
                            kind: hirsel_types::BuiltinType::Mutex,
                            ..
                        }
                    )
                {
                    continue;
                }
                let place = PlaceId(
                    body.places
                        .iter()
                        .position(|p| {
                            p.root == LocalId(local_index as u32)
                                && p.projection.is_empty()
                                && p.loan.is_none()
                        })
                        .unwrap() as u32,
                );
                for (conditional, cleanup_chain) in
                    [(false, false), (true, false), (false, true), (true, true)]
                {
                    let mut forged = (**original).clone();
                    let target = &mut forged.bodies[body_index];
                    let cleanup = target.blocks[target.entry.0 as usize]
                        .instructions
                        .first()
                        .map(|i| i.on_failure)
                        .unwrap_or(CleanupId(0));
                    let span = target.blocks[target.entry.0 as usize].span;
                    if cleanup_chain {
                        for cleanup in &mut target.cleanups {
                            let node = CleanupNodeId(target.cleanup_nodes.len() as u32);
                            target.cleanup_nodes.push(CleanupNode {
                                action: CleanupAction::Drop {
                                    place,
                                    if_live: conditional,
                                },
                                parent: cleanup.head,
                            });
                            cleanup.head = Some(node);
                        }
                    } else {
                        target.blocks[target.entry.0 as usize].instructions.insert(
                            0,
                            Instruction {
                                kind: InstructionKind::Drop {
                                    place,
                                    if_live: conditional,
                                },
                                span,
                                on_failure: cleanup,
                            },
                        );
                    }
                    let report =
                        hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![]);
                    assert!(report.violations.iter().any(|v| matches!(v.kind, hirsel_validate::ViolationKind::BorrowedMove(p) if p == place)), "{report:?}");
                    probes += 1;
                }
            }
        }
        assert_eq!(probes, 4);
    });
}

#[test]
fn noncopyable_environment_cannot_be_copied() {
    for source in [
        WITNESS.replace("sendable fn", "sendable once fn"),
        RESOURCE.replace("{ f() }", "{ let g = unique copy f; g() }"),
    ] {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("once-copy.hir", source).unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|d| d.rule == hirsel_diag::RuleId::of("OWN-5")),
            "{:?}",
            checked.diagnostics
        );
    }
}
