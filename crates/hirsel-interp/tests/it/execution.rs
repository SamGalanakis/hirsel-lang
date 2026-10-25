use crate::support;
use hirsel_diag::RuleId;
use hirsel_interp::{
    Error, Integer, Limits, OperationCall, OperationContext, OperationResult, Operations, Outcome,
    TraceEvent, Value,
};
use hirsel_syntax::BinaryOp as B;
use hirsel_tir::*;
use hirsel_types::{ParamId, Primitive as P, TypeKind};
use support::*;
fn int(k: P, n: u128) -> Value {
    Value::Integer(Integer::new(k, n).unwrap())
}
fn returned(e: &hirsel_interp::Execution, want: Value) {
    assert_eq!(
        e.outcome,
        Outcome::Returned {
            value: want,
            exit_code: 0
        }
    );
    assert!(e.stdout.is_empty());
}
fn rule(r: &str) -> RuleId {
    RuleId::new(r).unwrap()
}
fn fail(e: &hirsel_interp::Execution, r: &str, line: u32) {
    let Outcome::Failed(f) = &e.outcome else {
        panic!("expected failure: {e:?}")
    };
    assert_eq!(f.rule, rule(r));
    assert_eq!(f.line, line);
}
fn cleanup_events(e: &hirsel_interp::Execution) -> Vec<(usize, CleanupAction)> {
    e.trace
        .iter()
        .filter_map(|t| {
            if let TraceEvent::Cleanup { depth, action } = t {
                Some((*depth, action.clone()))
            } else {
                None
            }
        })
        .collect()
}

fn countdown() -> Definition {
    let mut b = Builder::new();
    let n = b.parameter(TypeKind::Primitive(P::U32));
    let zero = b.int(P::U32, 0);
    let test = b.binary(
        B::Eq,
        n,
        zero,
        TypeKind::Primitive(P::Bool),
        3,
        CleanupId(0),
    );
    b.block(
        Terminator::Branch {
            condition: copy(test),
            yes: BlockId(1),
            no: BlockId(2),
        },
        4,
    );
    let one = b.int(P::U32, 1);
    b.block(
        Terminator::Return {
            value: take(one),
            cleanup: CleanupId(0),
        },
        5,
    );
    let one = b.int(P::U32, 1);
    let next_n = b.binary(B::Sub, n, one, TypeKind::Primitive(P::U32), 6, CleanupId(0));
    let result = b.local(TypeKind::Primitive(P::U32), Storage::Temporary);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("countdown"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: take(next_n),
            }],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(3),
            }),
            on_failure: CleanupId(0),
        },
        7,
    );
    let answer = b.binary(
        B::Add,
        result,
        one,
        TypeKind::Primitive(P::U32),
        8,
        CleanupId(0),
    );
    b.finish("countdown", answer)
}
#[test]
fn recursion_has_an_explicit_depth_limit_and_no_host_call_recursion() {
    let e = execute(
        vec![countdown()],
        vec![int(P::U32, 20)],
        Limits {
            steps: Some(10000),
            call_depth: 21,
            source: None,
        },
    );
    returned(&e, int(P::U32, 21));
    let e = execute(
        vec![countdown()],
        vec![int(P::U32, 20)],
        Limits {
            steps: Some(10000),
            call_depth: 20,
            source: None,
        },
    );
    tooling_limit(&e, ToolingLimitKind::CallDepth, 1);
    let e = execute(
        vec![countdown()],
        vec![int(P::U32, 1023)],
        Limits {
            steps: Some(100000),
            call_depth: 3000,
            source: None,
        },
    );
    returned(&e, int(P::U32, 1024));
    let e = execute(
        vec![countdown()],
        vec![int(P::U32, 1024)],
        Limits {
            steps: Some(100000),
            call_depth: 3000,
            source: None,
        },
    );
    fail(&e, "LANG-10", 7);
    assert!(matches!(&e.outcome, Outcome::Failed(f) if f.kind == "Depth"));
}
fn loop_program() -> Vec<Definition> {
    let mut b = Builder::new();
    let n = b.int(P::U32, 5);
    let sum = b.int(P::U32, 0);
    let zero = b.int(P::U32, 0);
    let one = b.int(P::U32, 1);
    b.block(Terminator::Goto(BlockId(1)), 2);
    let test = b.binary(
        B::Gt,
        n,
        zero,
        TypeKind::Primitive(P::Bool),
        3,
        CleanupId(0),
    );
    b.block(
        Terminator::Branch {
            condition: take(test),
            yes: BlockId(2),
            no: BlockId(4),
        },
        4,
    );
    let result = b.local(TypeKind::Primitive(P::U32), Storage::Temporary);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("add"),
                arguments: vec![],
            },
            arguments: vec![
                CallArgument {
                    parameter: ParamId(0),
                    value: copy(sum),
                },
                CallArgument {
                    parameter: ParamId(1),
                    value: copy(n),
                },
            ],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(3),
            }),
            on_failure: CleanupId(0),
        },
        5,
    );
    b.replace(sum, take(result), 6);
    let next = b.binary(B::Sub, n, one, TypeKind::Primitive(P::U32), 7, CleanupId(0));
    b.replace(n, take(next), 7);
    b.block(Terminator::Goto(BlockId(1)), 8);
    let main = b.finish("main", sum);
    let mut c = Builder::new();
    let a = c.parameter(TypeKind::Primitive(P::U32));
    let z = c.parameter(TypeKind::Primitive(P::U32));
    let answer = c.binary(B::Add, a, z, TypeKind::Primitive(P::U32), 10, CleanupId(0));
    let add = c.finish("add", answer);
    vec![main, add]
}
/// `loop_program` with the charges lowering emits (CT-4): each entered iteration
/// (line 4) and each call to `add` (line 5), five of each.
fn charged_loop_program() -> Vec<Definition> {
    let mut definitions = loop_program();
    let charge = |kind, line| Instruction {
        kind: InstructionKind::Charge(SourceEvent { kind, source: line }),
        span: site(line),
        on_failure: CleanupId(0),
    };
    let body = &mut definitions[0].bodies[0].blocks[2];
    assert!(matches!(body.terminator, Terminator::Call { .. }));
    body.instructions
        .insert(0, charge(SourceEventKind::Iteration, 4));
    body.instructions.push(charge(SourceEventKind::Call, 5));
    definitions
}
fn charged(budget: Option<u64>, checked: bool) -> hirsel_interp::Execution {
    let definitions = charged_loop_program();
    let runtime = hirsel_interp::aggregate::RuntimeTypes {
        checked: checked.then(hirsel_interp::checked::CheckedRuntime::default),
        ..runtime()
    };
    assert_fixtures(&definitions, &runtime);
    hirsel_interp::run_with_runtime(
        &mut Provider {
            definitions,
            queries: vec![],
        },
        &sources(),
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 42,
            inputs: vec![],
        },
        vec![],
        Limits {
            steps: None,
            call_depth: 8,
            source: budget,
        },
        &mut hirsel_interp::NoOperations,
        &runtime,
    )
    .unwrap()
}

// Clang checks a step before it spends one (ExprConstant.cpp:969-980) and rustc
// counts at calls and loop back edges (ctfe_limit.rs:22-38). Here exactly B units
// succeed, the next event is denied before it executes at its own site, and the
// specialized dispatch form and checked (general) stepping agree on every budget.
#[test]
fn a_source_budget_admits_exactly_its_units_on_both_dispatch_paths() {
    for checked in [false, true] {
        for budget in 0..=12 {
            let e = charged(Some(budget), checked);
            if budget >= 10 {
                returned(&e, int(P::U32, 15));
                assert_eq!(e.charged, 10, "budget {budget}");
                continue;
            }
            let Outcome::Exhausted(denied) = &e.outcome else {
                panic!("budget {budget}: {:?}", e.outcome)
            };
            assert_eq!((denied.budget, e.charged), (budget, budget));
            let (kind, line) = if budget % 2 == 0 {
                (SourceEventKind::Iteration, 4)
            } else {
                (SourceEventKind::Call, 5)
            };
            assert_eq!(denied.event, SourceEvent { kind, source: line });
            assert_eq!(denied.site, site(line));
            assert_eq!(denied.definition, path("main"));
            // Denied before it executes: an exhausted call never entered `add`.
            assert_eq!(
                e.trace
                    .iter()
                    .filter(|t| matches!(t, TraceEvent::Enter { path: entered, .. } if *entered == path("add")))
                    .count() as u64,
                budget / 2,
                "budget {budget}"
            );
        }
    }
}

// Charges are not steps (PROG-3): without a source budget they cost nothing, and the
// charged program steps and traces exactly as the uncharged one.
#[test]
fn charges_leave_interpreter_steps_and_traces_unchanged() {
    let plain = execute(loop_program(), vec![], Limits::default());
    for checked in [false, true] {
        for budget in [None, Some(10)] {
            let e = charged(budget, checked);
            assert_eq!(e.outcome, plain.outcome);
            assert_eq!(e.steps, plain.steps);
            assert_eq!(e.charged, budget.unwrap_or(0));
            if !checked {
                assert_eq!(e.trace, plain.trace);
            }
        }
    }
}

#[test]
fn step_budget_counts_instructions_and_terminators_and_runs_site_cleanup() {
    let mut b = Builder::new();
    let u = b.unit();
    let c = b.cleanup(vec![drop(u)]);
    b.block(
        Terminator::Return {
            value: take(u),
            cleanup: c,
        },
        9,
    );
    let d = b.definition("main");
    let e = execute(
        vec![d.clone()],
        vec![],
        Limits {
            steps: Some(1),
            call_depth: 1,
            source: None,
        },
    );
    tooling_limit(&e, ToolingLimitKind::Steps, 9);
    assert_eq!(e.steps, 1);
    assert_eq!(cleanup_events(&e), vec![(1, drop(u))]);
    let unbounded = execute(
        vec![d.clone()],
        vec![],
        Limits {
            steps: None,
            call_depth: 1,
            source: None,
        },
    );
    returned(&unbounded, Value::Unit);
    assert_eq!(unbounded.steps, 2);
    let e = execute(
        vec![d],
        vec![],
        Limits {
            steps: Some(2),
            call_depth: 1,
            source: None,
        },
    );
    returned(&e, Value::Unit);
    assert_eq!(e.steps, 2);
    let mut b = Builder::new();
    b.block(Terminator::Goto(BlockId(0)), 7);
    let e = execute(
        vec![b.definition("main")],
        vec![],
        Limits {
            steps: Some(100),
            call_depth: 1,
            source: None,
        },
    );
    tooling_limit(&e, ToolingLimitKind::Steps, 7);
    assert_eq!(e.steps, 100);
}

// Steps through the resolved form count, trace and stop as general steps do: a run needs
// exactly as many steps as its full trace has instruction and terminator events, and any
// smaller budget stops after exactly that many, with the same events up to its limit, at
// the site of the step it could not take. The corpus never pins step counts.
#[test]
fn a_step_budget_stops_after_exactly_its_steps_with_the_unbounded_trace_so_far() {
    let counted = |e: &hirsel_interp::Execution| {
        e.trace
            .iter()
            .filter(|t| {
                matches!(
                    t,
                    TraceEvent::Instruction { .. } | TraceEvent::Terminator { .. }
                )
            })
            .count() as u64
    };
    let limits = |steps| Limits {
        steps,
        call_depth: 8,
        source: None,
    };
    let unbounded = execute(loop_program(), vec![], limits(None));
    returned(&unbounded, int(P::U32, 15));
    let steps = counted(&unbounded);
    assert_eq!(unbounded.steps, steps);
    let exact = execute(loop_program(), vec![], limits(Some(steps)));
    returned(&exact, int(P::U32, 15));
    assert_eq!(exact.trace, unbounded.trace);
    for budget in 1..steps {
        let short = execute(loop_program(), vec![], limits(Some(budget)));
        let Outcome::ToolingLimit(limit) = &short.outcome else {
            panic!("budget {budget}: {:?}", short.outcome)
        };
        assert_eq!(limit.kind, ToolingLimitKind::Steps);
        assert_eq!((short.steps, counted(&short)), (budget, budget));
        let at = short
            .trace
            .iter()
            .position(|t| matches!(t, TraceEvent::ToolingLimit(_)))
            .unwrap();
        assert_eq!(short.trace[..at], unbounded.trace[..at]);
        let next = unbounded
            .trace
            .iter()
            .filter_map(|t| match t {
                TraceEvent::Instruction { site, .. } | TraceEvent::Terminator { site, .. } => {
                    Some(*site)
                }
                _ => None,
            })
            .nth(budget as usize);
        assert_eq!(limit.site, next, "budget {budget}");
    }
}

#[test]
fn depth_failure_occurs_after_entry_and_cleans_transferred_arguments() {
    // Intentionally incomplete TIR: call-depth guard fires before reachable Unreachable.
    // This fixture exercises tooling policy and is not publishable strict-valid TIR.
    let mut c = Builder::new();
    let param = c.parameter(TypeKind::Primitive(P::U8));
    let cleanup = c.cleanup(vec![drop(param)]);
    let scratch = c.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    c.emit(
        InstructionKind::StorageLive(c.body.places[scratch.0 as usize].root),
        22,
        cleanup,
    );
    c.block(Terminator::Unreachable, 23);
    let callee = c.definition("callee");
    let mut b = Builder::new();
    let param = b.int(P::U8, 7);
    let local = b.int(P::U8, 8);
    let cleanup = b.cleanup(vec![drop(local)]);
    let result = b.local(TypeKind::Tuple(vec![].into()), Storage::ReturnSlot);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("callee"),
                arguments: vec![],
            },
            arguments: vec![CallArgument {
                parameter: ParamId(0),
                value: take(param),
            }],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: cleanup,
        },
        4,
    );
    let main = b.finish("main", result);
    let e = execute_unvalidated(
        vec![main, callee],
        vec![],
        Limits {
            steps: Some(100),
            call_depth: 1,
            source: None,
        },
    );
    tooling_limit(&e, ToolingLimitKind::CallDepth, 22);
    assert_eq!(cleanup_events(&e), vec![(2, drop(param)), (1, drop(local))]);
}
#[test]
fn external_float_inputs_are_normalized_even_when_returned_directly() {
    let mut b = Builder::new();
    let input = b.parameter(TypeKind::Primitive(P::F64));
    returned(
        &execute(
            vec![b.finish("main", input)],
            vec![Value::F64(0xfff0_0000_0000_0001)],
            Limits::default(),
        ),
        Value::F64(0x7ff8_0000_0000_0000),
    );
}

#[test]
fn cleanup_zero_can_hold_actions_and_control_edges_have_no_failure_list() {
    let mut b = Builder::new();
    let value = b.int(P::U8, 9);
    b.body.set_cleanup(CleanupId(0), vec![drop(value)]);
    let e = execute(vec![b.finish("main", value)], vec![], Limits::default());
    returned(&e, int(P::U8, 9));
    assert_eq!(cleanup_events(&e), vec![(1, drop(value))]);
    let mut b = Builder::new();
    b.body.cleanups.clear();
    b.block(Terminator::Goto(BlockId(0)), 7);
    let e = execute(
        vec![b.definition("main")],
        vec![],
        Limits {
            steps: Some(5),
            call_depth: 1,
            source: None,
        },
    );
    tooling_limit(&e, ToolingLimitKind::Steps, 7);
    assert!(cleanup_events(&e).is_empty());
}

fn tooling_limit(execution: &hirsel_interp::Execution, kind: ToolingLimitKind, line: u32) {
    let Outcome::ToolingLimit(limit) = &execution.outcome else {
        panic!("expected tooling limit")
    };
    assert_eq!(limit.kind, kind);
    assert_eq!(limit.site, Some(site(line)));
    assert!(
        !execution
            .trace
            .iter()
            .any(|event| matches!(event, TraceEvent::Failure(_)))
    );
}

#[test]
fn effect_only_tracing_retains_no_events_for_a_long_pure_loop() {
    let mut b = Builder::new();
    let yes = b.constant(TypeKind::Primitive(P::Bool), Constant::Bool(true));
    b.block(Terminator::Goto(BlockId(1)), 2);
    b.emit(
        InstructionKind::Check {
            condition: copy(yes),
            rule: rule("FAIL-1"),
        },
        7,
        CleanupId(0),
    );
    b.block(Terminator::Goto(BlockId(1)), 7);
    let definitions = vec![b.definition("main")];
    assert_fixtures(&definitions, &crate::support::runtime());
    let run = RunConfiguration {
        entry: path("main"),
        hash_seed: 0,
        inputs: vec![],
    };
    let limits = Limits {
        steps: Some(16_384),
        call_depth: 1,
        source: None,
    };
    let execute_mode = |trace_mode| {
        hirsel_interp::run_with_runtime(
            &mut Provider {
                definitions: definitions.clone(),
                queries: vec![],
            },
            &sources(),
            &run,
            vec![],
            limits,
            &mut hirsel_interp::NoOperations,
            &hirsel_interp::aggregate::RuntimeTypes {
                trace_mode,
                ..crate::support::runtime()
            },
        )
        .unwrap()
    };
    let full = execute_mode(hirsel_interp::TraceMode::Full);
    let effects = execute_mode(hirsel_interp::TraceMode::Effects);
    assert!(full.trace.len() > limits.steps.unwrap() as usize);
    assert_eq!(
        effects.trace.capacity(),
        0,
        "pure steps must not allocate a retained trace"
    );
    assert_eq!(effects.outcome, full.outcome);
    assert_eq!(effects.steps, full.steps);
    assert_eq!(effects.stdout, full.stdout);
    assert_eq!(effects.failures, full.failures);
}

#[test]
fn effect_only_tracing_preserves_anchored_operations_and_failure_observations() {
    struct Add {
        fail: bool,
    }
    impl Operations for Add {
        fn resolve(&self, path: &hirsel_syntax::DefPath) -> Option<hirsel_interp::OperationId> {
            let _ = path;
            (path == &support::path("add")).then_some(hirsel_interp::OperationId {
                index: 0,
                name: "test",
            })
        }
        fn call(
            &mut self,
            call: OperationCall<'_>,
            context: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            if call.path != &path("add") {
                return Ok(None);
            }
            context.stdout.extend_from_slice(b"add\n");
            context.consume(CapabilityEvent {
                capability: path("Test"),
                operation: "add".into(),
                input: vec![],
                output: vec![],
            })?;
            if self.fail {
                return Ok(Some(OperationResult::Failed(hirsel_interp::Failure {
                    rule: rule("FAIL-1"),
                    line: 5,
                    site: call.site,
                    kind: "test failure".into(),
                })));
            }
            let [Value::Integer(left), Value::Integer(right)] = call.arguments else {
                panic!("integer pair")
            };
            Ok(Some(OperationResult::returned(int(
                P::U32,
                left.bits() + right.bits(),
            ))))
        }
    }
    let definitions = loop_program();
    assert_fixtures(&definitions, &crate::support::runtime());
    let run = RunConfiguration {
        entry: path("main"),
        hash_seed: 0,
        inputs: vec![],
    };
    for fail in [false, true] {
        let execute_mode = |trace_mode| {
            hirsel_interp::run_with_runtime(
                &mut Provider {
                    definitions: definitions.clone(),
                    queries: vec![],
                },
                &sources(),
                &run,
                vec![],
                Limits::default(),
                &mut Add { fail },
                &hirsel_interp::aggregate::RuntimeTypes {
                    trace_mode,
                    ..crate::support::runtime()
                },
            )
            .unwrap()
        };
        let full = execute_mode(hirsel_interp::TraceMode::Full);
        let effects = execute_mode(hirsel_interp::TraceMode::Effects);
        let observed = effects.observable(&sources()).unwrap();
        assert_eq!(Some(&observed), full.observable(&sources()).as_ref());
        assert_eq!(observed.effects.len(), if fail { 1 } else { 5 });
        assert!(
            effects
                .trace
                .iter()
                .all(|e| matches!(e, TraceEvent::Effect(_)))
        );
        assert_eq!(effects.outcome, full.outcome);
        assert_eq!(effects.steps, full.steps);
        assert_eq!(effects.failures, full.failures);
        assert_eq!(effects.stdout, full.stdout);
    }
}

#[test]
fn operation_decisions_are_static_at_repeated_call_sites_including_declines() {
    use std::cell::Cell;
    struct StaticDecision {
        resolved: Cell<usize>,
        called: usize,
        intercept: bool,
    }
    impl Operations for StaticDecision {
        fn resolve(&self, path: &hirsel_syntax::DefPath) -> Option<hirsel_interp::OperationId> {
            assert_eq!(path, &support::path("add"));
            self.resolved.set(self.resolved.get() + 1);
            self.intercept.then_some(hirsel_interp::OperationId {
                index: 0,
                name: "add",
            })
        }
        fn call(
            &mut self,
            call: OperationCall<'_>,
            _: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            self.called += 1;
            let [Value::Integer(a), Value::Integer(b)] = call.arguments else {
                panic!("arguments")
            };
            Ok(Some(OperationResult::returned(Value::Integer(
                Integer::new(P::U32, a.bits() + b.bits()).unwrap(),
            ))))
        }
    }
    for intercept in [false, true] {
        let mut operations = StaticDecision {
            resolved: Cell::new(0),
            called: 0,
            intercept,
        };
        let mut provider = Provider {
            definitions: loop_program(),
            queries: vec![],
        };
        let execution = hirsel_interp::run_with_runtime(
            &mut provider,
            &sources(),
            &RunConfiguration {
                entry: path("main"),
                hash_seed: 0,
                inputs: vec![],
            },
            vec![],
            Limits::default(),
            &mut operations,
            &runtime(),
        )
        .unwrap();
        returned(&execution, int(P::U32, 15));
        assert_eq!(operations.resolved.get(), 1);
        assert_eq!(operations.called, if intercept { 5 } else { 0 });
    }
}

#[test]
fn call_only_operation_providers_keep_interception_and_tir_fallback() {
    struct Legacy {
        calls: usize,
        intercept: bool,
    }
    impl Operations for Legacy {
        fn call(
            &mut self,
            call: OperationCall<'_>,
            _: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            self.calls += 1;
            if !self.intercept {
                return Ok(None);
            }
            let [Value::Integer(a), Value::Integer(b)] = call.arguments else {
                panic!("arguments")
            };
            Ok(Some(OperationResult::returned(Value::Integer(
                Integer::new(P::U32, a.bits() + b.bits()).unwrap(),
            ))))
        }
    }
    for intercept in [false, true] {
        let mut operations = Legacy {
            calls: 0,
            intercept,
        };
        let mut provider = Provider {
            definitions: loop_program(),
            queries: vec![],
        };
        let execution = hirsel_interp::run_with_runtime(
            &mut provider,
            &sources(),
            &RunConfiguration {
                entry: path("main"),
                hash_seed: 0,
                inputs: vec![],
            },
            vec![],
            Limits::default(),
            &mut operations,
            &runtime(),
        )
        .unwrap();
        returned(&execution, int(P::U32, 15));
        assert_eq!(operations.calls, 5);
    }
}

#[test]
fn step_profile_counts_calls_and_bound_without_changing_execution() {
    let run = || execute(loop_program(), vec![], Limits::default());
    let plain = run();
    let (profiled, counts) = hirsel_interp::with_step_profile(&path("main"), run);
    assert_eq!(plain, profiled);
    let (_, excluded) = hirsel_interp::with_step_profile(&path("comptime"), run);
    assert!(
        excluded.is_empty(),
        "unrelated compile-time entry must be excluded"
    );
    assert_eq!(counts.values().sum::<u64>(), profiled.steps);
    assert!(
        counts.len() >= 2,
        "callee must have its own count: {counts:?}"
    );
    let (bounded, counts) = hirsel_interp::with_step_profile(&path("main"), || {
        execute(
            loop_program(),
            vec![],
            Limits {
                steps: Some(17),
                ..Limits::default()
            },
        )
    });
    assert_eq!(counts.values().sum::<u64>(), bounded.steps);
    assert!(matches!(bounded.outcome, Outcome::ToolingLimit(_)));
}

#[test]
fn step_profile_scopes_restore_after_nested_calls_and_unwind() {
    let run = || execute(loop_program(), vec![], Limits::default());
    let ((first, last), outer) = hirsel_interp::with_step_profile(&path("main"), || {
        let first = run();
        let (inner, counts) = hirsel_interp::with_step_profile(&path("main"), run);
        assert_eq!(counts.values().sum::<u64>(), inner.steps);
        assert!(
            std::panic::catch_unwind(|| hirsel_interp::with_step_profile(
                &path("main"),
                || panic!("probe")
            ))
            .is_err()
        );
        (first, run())
    });
    assert_eq!(outer.values().sum::<u64>(), first.steps + last.steps);
    let (_, disabled) = hirsel_interp::with_step_profile(&path("main"), || ());
    assert!(disabled.is_empty());
}
