use crate::support;
use hirsel_interp::*;
use hirsel_tir::*;
use hirsel_types::{ParamId, TypeKind, UseKind};
use support::*;
struct Update;
impl Operations for Update {
    fn resolve(&self, path: &hirsel_syntax::DefPath) -> Option<hirsel_interp::OperationId> {
        let _ = path;
        (true).then_some(hirsel_interp::OperationId {
            index: 0,
            name: "test",
        })
    }
    fn call(
        &mut self,
        _: OperationCall<'_>,
        _: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        Ok(Some(OperationResult::Complete {
            consumed: vec![],
            released: vec![],
            value: Value::Unit,
            arguments: vec![(
                0,
                Value::Handle {
                    kind: "counter".into(),
                    id: 9,
                },
            )],
        }))
    }
}
#[test]
fn operation_updates_only_mut_argument_places() {
    for usage in [UseKind::Mut, UseKind::Borrow, UseKind::Copy, UseKind::Take] {
        let mut b = Builder::new();
        let p = b.parameter(TypeKind::Named {
            definition: hirsel_syntax::DefId::intern(&path("Counter")),
            arguments: vec![].into(),
        });
        let result = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        let loan = LoanId(0);
        let failure = if matches!(usage, UseKind::Mut | UseKind::Borrow) {
            b.emit(
                InstructionKind::BeginLoan {
                    id: loan,
                    place: p,
                    mutable: usage == UseKind::Mut,
                    if_inactive: false,
                },
                1,
                CleanupId(0),
            );
            b.cleanup(vec![
                CleanupAction::EndLoan {
                    loan: loan,
                    if_active: false,
                },
                drop(p),
            ])
        } else {
            CleanupId(0)
        };
        let access = if matches!(usage, UseKind::Mut | UseKind::Borrow) {
            b.through_loan(p, loan)
        } else {
            p
        };
        b.block(
            Terminator::Call {
                callee: Callee::Definition {
                    path: path("update"),
                    arguments: vec![],
                },
                arguments: vec![CallArgument {
                    parameter: ParamId(0),
                    value: Operand {
                        place: access,
                        usage,
                    },
                }],
                returns: Some(CallReturn {
                    destination: result,
                    next: BlockId(1),
                }),
                on_failure: failure,
            },
            1,
        );
        if matches!(usage, UseKind::Mut | UseKind::Borrow) {
            b.emit(
                InstructionKind::EndLoan {
                    loan: loan,
                    if_active: false,
                },
                1,
                CleanupId(0),
            );
        }
        let d = b.finish("main", p);
        let e = run_with_operations(
            &mut Provider {
                definitions: vec![d],
                queries: vec![],
            },
            &crate::support::session(),
            &sources(),
            &RunConfiguration {
                entry: path("main"),
                hash_seed: 0,
                inputs: vec![],
            },
            vec![Value::Handle {
                kind: "counter".into(),
                id: 1,
            }],
            Limits::default(),
            &mut Update,
        );
        if usage == UseKind::Mut {
            assert!(matches!(
                e.unwrap().outcome,
                Outcome::Returned {
                    value: Value::Handle { id: 9, .. },
                    ..
                }
            ));
        } else {
            assert!(matches!(
                e,
                Err(ExecutionError {
                    error: Error::InvalidTir(_),
                    ..
                })
            ));
        }
    }
}

#[test]
fn operation_releases_untransferred_inputs_in_reverse_source_order_before_displaced_values() {
    struct Complete {
        dropped: Vec<u64>,
        releases: bool,
    }
    impl Operations for Complete {
        fn immediate(&self, _: hirsel_interp::OperationId) -> bool {
            true
        }
        fn resolve(&self, path: &hirsel_syntax::DefPath) -> Option<hirsel_interp::OperationId> {
            let _ = path;
            (true).then_some(hirsel_interp::OperationId {
                index: 0,
                name: "test",
            })
        }
        fn call(
            &mut self,
            _: OperationCall<'_>,
            _: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            Ok(Some(OperationResult::Complete {
                value: Value::Unit,
                arguments: vec![],
                consumed: vec![],
                released: if self.releases {
                    [3, 4]
                        .map(|id| Value::Handle {
                            kind: "counter".into(),
                            id,
                        })
                        .to_vec()
                } else {
                    vec![]
                },
            }))
        }
        fn drop_handle(
            &mut self,
            _: &str,
            id: u64,
            _: &mut OperationContext<'_>,
        ) -> Result<(), Error> {
            self.dropped.push(id);
            Ok(())
        }
    }
    let mut b = Builder::new();
    let kind = || TypeKind::Named {
        definition: hirsel_syntax::DefId::intern(&path("Counter")),
        arguments: vec![].into(),
    };
    let first = b.parameter(kind());
    let second = b.parameter(kind());
    let result = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("complete"),
                arguments: vec![],
            },
            // Declaration order differs from source evaluation order.
            arguments: vec![
                CallArgument {
                    parameter: ParamId(1),
                    value: take(first),
                },
                CallArgument {
                    parameter: ParamId(0),
                    value: take(second),
                },
            ],
            returns: Some(CallReturn {
                destination: result,
                next: BlockId(1),
            }),
            on_failure: CleanupId(0),
        },
        1,
    );
    let main = b.finish("main", result);
    for releases in [false, true] {
        let mut operations = Complete {
            dropped: vec![],
            releases,
        };
        let execution = run_with_operations(
            &mut Provider {
                definitions: vec![main.clone()],
                queries: vec![],
            },
            &crate::support::session(),
            &sources(),
            &RunConfiguration {
                entry: path("main"),
                hash_seed: 0,
                inputs: vec![],
            },
            [1, 2]
                .map(|id| Value::Handle {
                    kind: "counter".into(),
                    id,
                })
                .to_vec(),
            Limits::default(),
            &mut operations,
        )
        .unwrap();
        assert!(matches!(execution.outcome, Outcome::Returned { .. }));
        assert_eq!(
            operations.dropped,
            if releases {
                vec![2, 1, 4, 3]
            } else {
                vec![2, 1]
            }
        );
    }
}

#[test]
fn api_execution_errors_retain_exact_prior_stdout_and_original_cause() {
    struct Broken(Error);
    impl Operations for Broken {
        fn resolve(&self, path: &hirsel_syntax::DefPath) -> Option<hirsel_interp::OperationId> {
            let _ = path;
            (true).then_some(hirsel_interp::OperationId {
                index: 0,
                name: "test",
            })
        }
        fn call(
            &mut self,
            _: OperationCall<'_>,
            context: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            context.stdout.extend_from_slice(b"prior\0output\n");
            Err(self.0.clone())
        }
    }
    for cause in [
        Error::InvalidTir("broken operation".into()),
        Error::Unavailable("missing operation".into()),
        Error::Query(hirsel_types::QueryError::NotImplemented),
    ] {
        let mut b = Builder::new();
        let value = b.local(TypeKind::Tuple(vec![].into()), Storage::Temporary);
        b.block(
            Terminator::Call {
                callee: Callee::Definition {
                    path: path("broken"),
                    arguments: vec![],
                },
                arguments: vec![],
                returns: Some(CallReturn {
                    destination: value,
                    next: BlockId(1),
                }),
                on_failure: CleanupId(0),
            },
            1,
        );
        let definition = b.finish("main", value);
        let original = format!("{cause:?}");
        let error = run_with_operations(
            &mut Provider {
                definitions: vec![definition],
                queries: vec![],
            },
            &crate::support::session(),
            &sources(),
            &RunConfiguration {
                entry: path("main"),
                hash_seed: 0,
                inputs: vec![],
            },
            vec![],
            Limits::default(),
            &mut Broken(cause),
        )
        .unwrap_err();
        assert_eq!(error.stdout, b"prior\0output\n");
        assert_eq!(format!("{:?}", error.error), original);
    }
}

#[test]
fn immediate_completion_rejects_a_return_from_a_diverging_operation() {
    struct Complete;
    impl Operations for Complete {
        fn immediate(&self, _: OperationId) -> bool {
            true
        }
        fn call(
            &mut self,
            _: OperationCall<'_>,
            _: &mut OperationContext<'_>,
        ) -> Result<Option<OperationResult>, Error> {
            Ok(Some(OperationResult::returned(Value::Unit)))
        }
    }
    let mut b = Builder::new();
    b.block(
        Terminator::Call {
            callee: Callee::Definition {
                path: path("diverging"),
                arguments: vec![],
            },
            arguments: vec![],
            returns: None,
            on_failure: CleanupId(0),
        },
        1,
    );
    let error = run_with_operations(
        &mut Provider {
            definitions: vec![b.definition("main")],
            queries: vec![],
        },
        &session(),
        &sources(),
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 0,
            inputs: vec![],
        },
        vec![],
        Limits::default(),
        &mut Complete,
    )
    .unwrap_err();
    assert!(
        matches!(error.error, Error::InvalidTir(ref detail) if detail == "diverging operation returned")
    );
}
