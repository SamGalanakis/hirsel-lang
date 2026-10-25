//! Immediate list completions keep the scheduled transfer and step boundaries.
use crate::support::path;
use crate::support::*;
use hirsel_interp::{library::*, *};
use hirsel_tir::*;
use hirsel_types::{BuiltinType, GenericArgument, ParamId, TypeKind, UseKind};

#[derive(Clone, Copy, Debug)]
enum Stop {
    None,
    Before,
    After,
    Duplicate,
}

struct Lists {
    library: Library,
    immediate: bool,
    stop: Stop,
    calls: usize,
    dropped: Vec<u64>,
}
impl Operations for Lists {
    fn resolve(&self, path: &hirsel_syntax::DefPath) -> Option<OperationId> {
        self.library.resolve(path)
    }
    fn immediate(&self, id: OperationId) -> bool {
        self.immediate && self.library.immediate(id)
    }
    fn call(
        &mut self,
        call: OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        self.calls += 1;
        let failure = hirsel_interp::Failure {
            rule: const { hirsel_diag::RuleId::of("LANG-9") },
            line: context.sources.location(call.site).unwrap().line,
            site: call.site,
            kind: "fixture stop".into(),
        };
        if matches!(self.stop, Stop::Before) {
            return Ok(Some(OperationResult::Failed(failure)));
        }
        let mut result = self.library.call(call, context)?;
        if matches!(self.stop, Stop::Duplicate) {
            let Some(OperationResult::Complete { consumed, .. }) = &mut result else {
                unreachable!()
            };
            consumed.push(1);
        }
        if matches!(self.stop, Stop::After) {
            return Ok(Some(OperationResult::Failed(failure)));
        }
        assert!(matches!(
            result,
            Some(OperationResult::Complete {
                value: Value::Unit,
                ..
            })
        ));
        Ok(result)
    }
    fn drop_handle(
        &mut self,
        kind: &str,
        id: u64,
        _: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        assert_eq!(kind, "fixture-resource");
        self.dropped.push(id);
        Ok(())
    }
}

fn fixture(pushes: usize, reverse: bool, usage: UseKind) -> Definition {
    let mut b = Builder::new();
    let resource = || TypeKind::Named {
        definition: (&path("Resource")).into(),
        arguments: vec![].into(),
    };
    let item_type = b.type_id(resource());
    let list = b.parameter(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(item_type)].into(),
    });
    let items: Vec<_> = (0..pushes).map(|_| b.parameter(resource())).collect();
    for (index, &item) in items.iter().enumerate() {
        let loan = LoanId(index as u32);
        b.emit(
            InstructionKind::BeginLoan {
                id: loan,
                place: list,
                mutable: true,
                if_inactive: false,
            },
            10 + index as u32 * 3,
            CleanupId(0),
        );
        let access = b.through_loan(list, loan);
        let mut cleanup = vec![CleanupAction::EndLoan {
            loan,
            if_active: false,
        }];
        cleanup.extend(items.iter().rev().copied().map(drop));
        cleanup.push(drop(list));
        let failure = b.cleanup(cleanup);
        let result = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        let mut arguments = vec![
            CallArgument {
                parameter: ParamId(0),
                value: Operand {
                    place: access,
                    usage: UseKind::Mut,
                },
            },
            CallArgument {
                parameter: ParamId(1),
                value: Operand { place: item, usage },
            },
        ];
        if reverse {
            arguments.reverse();
        }
        b.block(
            Terminator::Call {
                callee: Callee::Definition {
                    path: hirsel_syntax::DefPath::core("list.push"),
                    arguments: vec![],
                },
                arguments,
                returns: Some(CallReturn {
                    destination: result,
                    next: BlockId(index as u32 + 1),
                }),
                on_failure: failure,
            },
            11 + index as u32 * 3,
        );
        b.emit(
            InstructionKind::EndLoan {
                loan,
                if_active: false,
            },
            12 + index as u32 * 3,
            CleanupId(0),
        );
    }
    let unit = b.unit();
    b.finish("main", unit)
}

fn execute(
    definition: &Definition,
    pushes: usize,
    immediate: bool,
    stop: Stop,
    steps: Option<u64>,
) -> Result<(Execution, Vec<u64>, usize), ExecutionError> {
    let configuration = RunConfiguration {
        entry: path("main"),
        hash_seed: 42,
        inputs: vec![],
    };
    let mut lists = Lists {
        library: Library::new(&configuration, Default::default()).unwrap(),
        immediate,
        stop,
        calls: 0,
        dropped: vec![],
    };
    let mut arguments = vec![collection::sequence(BuiltinType::List, [])];
    arguments.extend((1..=pushes).map(|id| Value::Handle {
        kind: "fixture-resource".into(),
        id: id as u64,
    }));
    let mut runtime = runtime();
    runtime.trace_mode = TraceMode::Full;
    run_with_runtime(
        &mut Provider {
            definitions: vec![definition.clone()],
            queries: vec![],
        },
        &sources(),
        &configuration,
        arguments,
        Limits {
            steps,
            call_depth: 8,
            source: None,
        },
        &mut lists,
        &runtime,
    )
    .map(|execution| (execution, lists.dropped, lists.calls))
}

#[test]
fn interp_immediate_cost_6_zero_one_many_pushes_return_unit_and_drop_each_owner_once() {
    // Rust's no_spurious_drop_after_call vector extended to list ownership:
    // each consumed handle is dropped only when its owning list is released.
    for pushes in [0, 1, 7] {
        for reverse in [false, true] {
            let definition = fixture(pushes, reverse, UseKind::Take);
            let slow = execute(&definition, pushes, false, Stop::None, None).unwrap();
            let fast = execute(&definition, pushes, true, Stop::None, None).unwrap();
            assert_eq!(fast, slow);
            assert_eq!(
                fast.0.outcome,
                Outcome::Returned {
                    value: Value::Unit,
                    exit_code: 0
                }
            );
            assert_eq!(fast.2, pushes);
            let mut dropped = fast.1;
            dropped.sort_unstable();
            assert_eq!(dropped, (1..=pushes as u64).collect::<Vec<_>>());
        }
    }
}

#[test]
fn interp_immediate_cost_6_failure_on_either_side_of_transfer_keeps_first_site_and_cleanup() {
    for stop in [Stop::Before, Stop::After] {
        for reverse in [false, true] {
            let definition = fixture(3, reverse, UseKind::Take);
            let slow = execute(&definition, 3, false, stop, None).unwrap();
            let fast = execute(&definition, 3, true, stop, None).unwrap();
            assert_eq!(fast, slow);
            assert_eq!(fast.2, 1);
            let Outcome::Failed(failure) = &fast.0.outcome else {
                panic!("{fast:?}")
            };
            assert_eq!(failure.site, site(11));
            assert_eq!(failure.kind, "fixture stop");
            assert_eq!(fast.0.failures.len(), 1);
            let mut dropped = fast.1;
            dropped.sort_unstable();
            assert_eq!(dropped, [1, 2, 3]);
        }
    }
}

#[test]
fn interp_immediate_cost_6_all_step_cuts_match_scheduled_completion() {
    for stop in [Stop::None, Stop::Before, Stop::After] {
        let definition = fixture(3, true, UseKind::Take);
        let full = execute(&definition, 3, false, stop, None).unwrap();
        for cut in 0..=full.0.steps {
            let slow = execute(&definition, 3, false, stop, Some(cut)).unwrap();
            let fast = execute(&definition, 3, true, stop, Some(cut)).unwrap();
            assert_eq!(fast, slow, "{stop:?}, cut {cut}");
            if cut < full.0.steps {
                assert!(matches!(fast.0.outcome, Outcome::ToolingLimit(_)));
                assert_eq!(fast.0.steps, cut);
            } else {
                assert_eq!(fast, full);
            }
        }
    }
}

#[test]
fn interp_immediate_cost_6_wrong_take_mode_is_refused_before_completion() {
    let definition = fixture(1, false, UseKind::Copy);
    for immediate in [false, true] {
        let error = execute(&definition, 1, immediate, Stop::None, None).unwrap_err();
        assert!(
            matches!(error.error, Error::InvalidTir(ref message) if message == "operation transferred an argument it does not own")
        );
    }
}

#[test]
fn interp_immediate_cost_6_duplicate_parameters_and_mut_aliases_are_refused() {
    for alias in [false, true] {
        let mut definition = fixture(1, false, UseKind::Take);
        let Terminator::Call { arguments, .. } = &mut definition.bodies[0].blocks[0].terminator
        else {
            unreachable!()
        };
        if alias {
            arguments[1].value = arguments[0].value.clone();
        } else {
            arguments[1].parameter = arguments[0].parameter;
        }
        for immediate in [false, true] {
            let error = execute(&definition, 1, immediate, Stop::None, None).unwrap_err();
            assert!(
                matches!(error.error, Error::InvalidTir(_)),
                "{alias}: {error:?}"
            );
        }
    }
}

#[test]
fn interp_immediate_cost_6_duplicate_consumption_is_refused() {
    let definition = fixture(1, false, UseKind::Take);
    for immediate in [false, true] {
        let error = execute(&definition, 1, immediate, Stop::Duplicate, None).unwrap_err();
        assert!(
            matches!(error.error, Error::InvalidTir(ref message) if message == "operation transferred an argument it does not own")
        );
    }
}
