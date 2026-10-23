use hirsel_driver::*;
use hirsel_engine::*;

const ORIGINAL: &str = r#"fn deadline(clock: MonotonicClock) -> Result[Deadline, ClockError] {
 let now = clock.monotonic(clock)?
 let Some(at) = instant.checked_add(clock, now, 2000000000u64) else { return Error(OutOfRange()) }
 Ok(Deadline(clock: clock, at: at))
}
fn child(n: Net, take half: ReadHalf of n, take events: Sender[u64]) { read_half.close(n, take half) }
fn writer(n: Net, take half: WriteHalf of n, mut inbox: Receiver[u64]) { write_half.close(n, take half) }
fn run(tasks: Tasks, n: Net, clock: MonotonicClock, take stream: Stream of n, take events: Sender[u64], take inbox: Receiver[u64], state: Mutex[u64], log: Mutex[u64]) {
  let (r, w) = net.split(n, take stream)
  let bounded = deadline(clock)
  let Ok(until) = take bounded else { return }
  let _ = task.scope_until(tasks, until, fn(s: Scope) {
    let _ = mutex.with(tasks, state, fn(mut v: unique u64) -> u64 uses {} { v })
    let _ = mutex.with(tasks, log, fn(mut v: unique u64) -> u64 uses {} { v })
    let h = task.async(s, fn() { child(n, take r, take events) })
    let j = task.async(s, fn() { let mut receiver = take inbox; writer(n, take w, mut receiver) })
    task.cancel(s, h)
    task.cancel(s, j)
    task.await(s, take h)
    task.await(s, take j)
  })
}
pub fn main(rt: Io) -> Result[(), GrantError] {
  let n = io.net(rt)?
  let tasks = io.tasks(rt)?
  let clock = io.monotonic_clock(rt)?
  let Ok(stream) = net.unix_connect(n, path.of("/fixture/pubsub.sock")) else { return Ok(()) }
  let (tx, rx): (Sender[u64], Receiver[u64]) = channel.open(tasks)
  run(tasks, n, clock, take stream, take tx, take rx, mutex.create(tasks, 0u64), mutex.create(tasks, 0u64))
  Ok(())
}
"#;
const MINIMAL: &str = r#"fn probe(tasks: Tasks, state: Mutex[u64], take tx: Sender[u64]) {
 let _ = task.scope(tasks, fn(s: Scope) {
   let _ = state
   let _ = take tx
 })
}
pub fn main(rt: Io) -> Result[(), GrantError] {
 let tasks = io.tasks(rt)?
 let (tx, rx): (Sender[u64], Receiver[u64]) = channel.open(tasks)
 probe(tasks, mutex.create(tasks, 0u64), take tx)
 Ok(())
}
"#;
const TWINS: &str =
    include_str!("../../../../conformance/CLO/CLO-1-scoped-owning-cleanup-loan-30-run.hir");

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("socket-loan.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    engine
}

#[test]
fn split_socket_loan_30_original_and_twins_on_four_engines() {
    use hirsel_driver::native::*;
    with_compiler_stack(|| {
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let configurations = [
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: false },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let scratch = tempfile::Builder::new()
            .prefix("split-socket-loan-30-")
            .tempdir()
            .unwrap();
        let toolchain =
            Toolchain::for_matrix(&scratch.path().join("runtime"), &configurations).unwrap();
        let net: hirsel_run_schema::NetGrant = serde_json::from_value(serde_json::json!({
            "provider": "net.script@2",
            "rules": [{"address": "/fixture/pubsub.sock", "service": "0", "operation": "unix_connect"}],
            "peers": [{"address": "/fixture/pubsub.sock", "port": 0, "resolves_to": [], "tls": "plain", "connections": [{"open": []}]}]
        })).unwrap();
        for (name, source, entry_name) in
            [("original", ORIGINAL, "main"), ("twins", TWINS, "exercise")]
        {
            let mut engine = checked(source);
            let mut entry = entry_path(&default_context());
            entry.0.name = entry_name.into();
            let mut simulation = SimulationConfig::default();
            let fixture = if name == "original" {
                simulation.grants.insert("MonotonicClock".into());
                simulation.net = Some(net.clone());
                Fixture {
                    monotonic: Some(0),
                    net: Some(net.clone()),
                    ..Default::default()
                }
            } else {
                Fixture::default()
            };
            let execution = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 30,
                    inputs: vec![],
                },
                simulation,
            )
            .unwrap();
            let observed = execution.observation.as_ref().unwrap();
            let reference = Observation::of_reference(observed);
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            assert!(reference.failures.is_empty(), "{name}: {reference:?}");
            if name == "original" {
                let operations: Vec<_> = observed
                    .effects
                    .iter()
                    .filter_map(|effect| {
                        if let hirsel_interp::observation::Effect::CapabilityConsumed(input) =
                            &effect.effect
                        {
                            Some(input.operation.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                assert_eq!(
                    operations.iter().filter(|op| **op == "net.split").count(),
                    1,
                    "{operations:?}"
                );
                assert!(operations.contains(&"read_half.close"), "{operations:?}");
                assert!(operations.contains(&"write_half.close"), "{operations:?}");
            }
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let dir = scratch.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let outcomes = differential_in(
                &validated,
                &entry,
                &reference,
                &configurations,
                &toolchain,
                &dir,
                &fixture,
            );
            assert_eq!(outcomes.len(), 3);
            for (configuration, outcome) in outcomes {
                eprintln!("{name}: {configuration}: {outcome:?}");
                assert_eq!(outcome, Outcome::Agree, "{name}: {configuration}");
            }
        }
    });
}

#[test]
fn split_socket_loan_30_v_rejects_early_capture_loan_end() {
    use hirsel_tir::{CleanupAction, CleanupNode, CleanupNodeId, InstructionKind};
    use hirsel_validate::ViolationKind;
    with_compiler_stack(|| {
        let mut engine = checked(MINIMAL);
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let original = &input.definitions[&path];
        hirsel_validate::assert_valid(original, signatures.types(), &signatures);
        for cleanup in [false, true] {
            let mut forged = (**original).clone();
            let body = &mut forged.bodies[0];
            let loan = body
                .blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .find_map(|i| match i.kind {
                    InstructionKind::BeginLoan { id, place, .. }
                        if body.places[place.0 as usize].root == body.parameters[1] =>
                    {
                        Some(id)
                    }
                    _ => None,
                })
                .expect("the borrowed capture has an access loan");
            let (block, index, root, holder) = body
                .blocks
                .iter()
                .enumerate()
                .find_map(|(b, block)| {
                    block
                        .instructions
                        .iter()
                        .enumerate()
                        .find_map(|(i, instruction)| {
                            let InstructionKind::RunCleanup(root) = instruction.kind else {
                                return None;
                            };
                            body.cleanup_actions(root).find_map(|action| match action {
                                CleanupAction::Drop { place, .. } => Some((b, i, root, *place)),
                                _ => None,
                            })
                        })
                })
                .expect("the scoped owning callback is destroyed");
            if cleanup {
                let id = CleanupNodeId(body.cleanup_nodes.len() as u32);
                body.cleanup_nodes.push(CleanupNode {
                    action: CleanupAction::EndLoan {
                        loan,
                        if_active: false,
                    },
                    parent: body.cleanups[root.0 as usize].head,
                });
                body.cleanups[root.0 as usize].head = Some(id);
            } else {
                let mut end = body.blocks[block].instructions[index].clone();
                end.kind = InstructionKind::EndLoan {
                    loan,
                    if_active: false,
                };
                body.blocks[block].instructions.insert(index, end);
            }
            let report =
                hirsel_validate::validate(&forged, signatures.types(), &signatures, &mut vec![]);
            assert!(
                report.violations.iter().any(|v| v.kind
                    == ViolationKind::ExpiredLoan {
                        loan,
                        place: holder
                    }),
                "cleanup={cleanup}: {:?}",
                report.violations
            );
        }
    });
}

#[test]
fn split_socket_loan_30_borrowed_resource_cannot_be_captured_by_take() {
    with_compiler_stack(|| {
        let source = MINIMAL.replace("take tx: Sender[u64]", "tx: Sender[u64]");
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("borrowed-endpoint.hir", source).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "OWN-1"),
            "{:?}",
            output.diagnostics
        );
    });
}
