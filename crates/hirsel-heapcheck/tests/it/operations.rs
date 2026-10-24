//! Semantic operation results acquire storage and holder credits under H's ledger.
use crate::fixtures;
use fixtures::run;
use hirsel_core_contract::{OperationId, Passing};
use hirsel_heapcheck::build::{CLEANUP, Layouts, fixture_unit, path};
use hirsel_heapcheck::{Config, OpOutcome, OpValue, Operations, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::*;
use hirsel_lir::stage::Ready;
use hirsel_lir::testing::UnitBuilder;

#[derive(Clone, Copy)]
enum ResultKind {
    Fresh,
    Share,
    Transfer,
    Variant,
    BadRange,
    BadArgument,
    BadTag,
    BadElement,
    NumericWork(bool),
}
#[derive(Clone)]
struct Model(ResultKind);
impl Operations for Model {
    fn domain(&self) -> hirsel_heapcheck::OperationDomain {
        hirsel_heapcheck::OperationDomain::Simulation
    }
    fn fork_simulation(&self) -> Option<Box<dyn Operations>> {
        Some(Box::new(self.clone()))
    }

    fn call(&mut self, op: &OperationId, args: &[OpValue], output: &mut Vec<u8>) -> OpOutcome {
        match op.definition.0.name.as_str() {
            "fresh" => OpOutcome::Value(OpValue::ByteSequence(b"abc".to_vec())),
            "range" => {
                assert_eq!(args, &[OpValue::Bytes(b"abc".to_vec())]);
                let range = OpValue::Slice {
                    argument: u32::from(matches!(self.0, ResultKind::BadArgument)),
                    start: 1,
                    length: if matches!(self.0, ResultKind::BadRange) {
                        3
                    } else {
                        2
                    },
                };
                OpOutcome::Value(
                    if matches!(self.0, ResultKind::Variant | ResultKind::BadTag) {
                        OpValue::Variant {
                            tag: if matches!(self.0, ResultKind::BadTag) {
                                7
                            } else {
                                1
                            },
                            value: Box::new(range),
                        }
                    } else {
                        range
                    },
                )
            }
            "show" => {
                let [OpValue::Bytes(bytes)] = args else {
                    return OpOutcome::Unsupported;
                };
                output.extend_from_slice(bytes);
                OpOutcome::Value(OpValue::Unit)
            }
            _ => OpOutcome::Unsupported,
        }
    }
}
fn operation(b: &mut UnitBuilder<Ready>, name: &str) -> Callee {
    b.operation(OperationId {
        definition: path(name),
        declaration_digest: Default::default(),
    })
}
fn call(
    b: &mut UnitBuilder<Ready>,
    block: BlockId,
    callee: Callee,
    args: &[(Passing, ArgumentValue)],
    result: Option<ValueId>,
    failure: FailureId,
) -> BlockId {
    let call = b.call(callee, args, result);
    let next = b.block(BlockKind::Body, &[]);
    let normal = b.edge(next, &[]);
    b.term(
        block,
        Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        },
    );
    next
}
fn program(kind: ResultKind) -> Program<Ready> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let byte = layouts.scalar(ScalarKind::U8);
    let word = layouts.scalar(ScalarKind::U64);
    let pointer = layouts.scalar(ScalarKind::Pointer);
    let payload = layouts.record(&[word]);
    let r = &mut layouts.table.layouts[payload.0 as usize];
    r.kind = LayoutKind::Sequence { element: byte };
    r.size = SizeSpec::Dynamic(vec![
        SizeTerm::Constant(16),
        SizeTerm::ReadLength {
            offset: 0,
            scalar: ScalarKind::U64,
        },
        SizeTerm::Add,
    ]);
    r.copy = CopyRecipe::NotCopyable;
    let slice = layouts.record(&[pointer, word, word]);
    let r = &mut layouts.table.layouts[slice.0 as usize];
    r.kind = LayoutKind::Slice { element: byte };
    r.scan = vec![ScanStep::SliceOwner { offset: 0 }];
    r.drop = vec![DropStep::Release { offset: 0 }];
    r.copy = CopyRecipe::Dup;
    let word_slice = layouts.record(&[pointer, word, word]);
    let r = &mut layouts.table.layouts[word_slice.0 as usize];
    r.kind = LayoutKind::Slice { element: word };
    r.scan = vec![ScanStep::SliceOwner { offset: 0 }];
    r.drop = vec![DropStep::Release { offset: 0 }];
    r.copy = CopyRecipe::Dup;
    let variant = layouts.variant(&[&[slice], &[slice]]);
    let (target, table) = layouts.finish();
    let mut b = UnitBuilder::<Ready>::new(fixture_unit(path("main")));
    let first = b.block(BlockKind::Body, &[]);
    let failed = b.block(BlockKind::FailureCleanup, &[]);
    b.term(failed, Terminator::Propagate);
    let edge = b.edge(failed, &[]);
    let failure = b.failure(edge, None, &[]);
    let source = b.owned(ValueRepr::Aggregate, slice);
    let fresh = operation(&mut b, "fresh");
    let mut current = call(&mut b, first, fresh, &[], Some(source), failure);
    let (returned, layout) = if matches!(kind, ResultKind::Fresh) {
        (source, slice)
    } else {
        let layout = if matches!(kind, ResultKind::BadElement) {
            word_slice
        } else if matches!(kind, ResultKind::Variant | ResultKind::BadTag) {
            variant
        } else {
            slice
        };
        let value = b.owned(ValueRepr::Aggregate, layout);
        let range = if let ResultKind::NumericWork(core) = kind {
            b.operation(OperationId {
                definition: if core {
                    hirsel_syntax::DefPath::core("bytes.integer")
                } else {
                    path("integer")
                },
                declaration_digest: Default::default(),
            })
        } else {
            operation(&mut b, "range")
        };
        let passing = if matches!(kind, ResultKind::Transfer | ResultKind::Variant) {
            Passing::Take
        } else {
            Passing::Borrow
        };
        current = call(
            &mut b,
            current,
            range,
            &[(passing, ArgumentValue::Value(source))],
            Some(value),
            failure,
        );
        if passing == Passing::Borrow {
            b.push(
                current,
                Inst::Release {
                    value: source,
                    layout: slice,
                    live: MaskRef::All,
                    anchor: CLEANUP,
                    failure,
                },
            );
        }
        (value, layout)
    };
    if !matches!(kind, ResultKind::Variant | ResultKind::BadTag) {
        let show = operation(&mut b, "show");
        current = call(
            &mut b,
            current,
            show,
            &[(Passing::Borrow, ArgumentValue::Value(returned))],
            None,
            failure,
        );
    }
    b.push(
        current,
        Inst::Release {
            value: returned,
            layout,
            live: MaskRef::All,
            anchor: CLEANUP,
            failure,
        },
    );
    b.term(current, Terminator::Return(None));
    Program {
        target,
        layouts: table.clone(),
        units: vec![b.finish_with_layouts(&table)],
        statics: vec![],
    }
}
#[test]
fn fresh_shared_transferred_and_variant_byte_results_have_clean_terminal_audits() {
    for kind in [
        ResultKind::Fresh,
        ResultKind::Share,
        ResultKind::Transfer,
        ResultKind::Variant,
    ] {
        let p = program(kind);
        let result = run(&p, &path("main"), &[], &mut Model(kind), &Config::default());
        let audit = result.result.unwrap();
        assert_eq!(
            audit.observation.output,
            match kind {
                ResultKind::Fresh => b"abc".as_slice(),
                ResultKind::Variant => b"",
                _ => b"bc",
            }
        );
        assert!(audit.observation.failures.is_empty());
    }
}
#[test]
fn byte_result_range_argument_element_and_variant_tag_forgeries_are_rejected() {
    for (kind, expected) in [
        (ResultKind::BadRange, ViolationKind::OutOfExtent),
        (ResultKind::BadArgument, ViolationKind::Malformed),
        (ResultKind::BadTag, ViolationKind::TypeConfusion),
        (ResultKind::BadElement, ViolationKind::TypeConfusion),
    ] {
        let p = program(kind);
        let result = run(&p, &path("main"), &[], &mut Model(kind), &Config::default());
        assert_eq!(result.result.unwrap_err().kind, expected);
    }
}

#[derive(Clone, Copy)]
enum HandleControl {
    Valid,
    TakenAddress,
    ExclusiveAlias,
    ExclusiveValid,
    ClosedValue,
    ClosedValid,
    Transfer,
    BorrowedTransfer,
}
#[derive(Clone)]
struct HandlesModel;
impl Operations for HandlesModel {
    fn domain(&self) -> hirsel_heapcheck::OperationDomain {
        hirsel_heapcheck::OperationDomain::Simulation
    }
    fn fork_simulation(&self) -> Option<Box<dyn Operations>> {
        Some(Box::new(self.clone()))
    }
    fn call(&mut self, op: &OperationId, args: &[OpValue], _: &mut Vec<u8>) -> OpOutcome {
        match op.definition.0.name.as_str() {
            "open" => OpOutcome::Value(OpValue::OpenHandle(41)),
            "snapshot" => OpOutcome::Value(OpValue::Handle(41)),
            "transfer" => OpOutcome::Value(OpValue::TakenHandle { argument: 0 }),
            "consume" => {
                assert_eq!(args, &[OpValue::Handle(41)]);
                OpOutcome::Complete {
                    result: Ok(OpValue::Unit),
                    consumed: vec![0],
                }
            }
            "borrow" | "mutate" => {
                assert_eq!(args, &[OpValue::Handle(41)]);
                OpOutcome::Value(OpValue::Unit)
            }
            _ => OpOutcome::Unsupported,
        }
    }
}

#[test]
fn atomic_cells_do_not_alias_a_closed_host_handle() {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = fixtures::common(&mut layouts);
    let handle = layouts.scalar(ScalarKind::U64);
    layouts.get_mut(handle).kind = LayoutKind::Handle;
    let (target, table) = layouts.finish();
    let mut f = fixtures::F::<Ready>::new("main", c);
    let open = operation(&mut f.b, "open");
    let host = f.b.owned(ValueRepr::Handle, handle);
    f.call(open, &[], Some(host));
    let consume = operation(&mut f.b, "consume");
    f.call(
        consume,
        &[(Passing::Take, ArgumentValue::Value(host))],
        None,
    );
    let value = f.int(7);
    let construct = f.b.construct(
        ConstructorKind::Tuple,
        c.cell,
        &[(0, value)],
        fixtures::HEAP,
    );
    let image = f.b.plain(ValueRepr::Aggregate, c.cell);
    let scalar_size = f.int(8);
    let scalar_offset = f.int(0);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: image,
        construct,
        failure,
    });
    let create = f.b.operation(OperationId {
        definition: hirsel_syntax::DefPath::core("$intrinsic.AtomicCreate"),
        declaration_digest: Default::default(),
    });
    let store = f.b.operation(OperationId {
        definition: hirsel_syntax::DefPath::core("$intrinsic.AtomicStore"),
        declaration_digest: Default::default(),
    });
    // The old model used the bare cell index as its handle: the 42nd cell
    // was mistaken for host handle 41, which the consume above closed.
    for _ in 0..42 {
        let atomic = f.b.plain(ValueRepr::Handle, handle);
        f.call(
            create.clone(),
            &[
                (Passing::Borrow, ArgumentValue::Value(image)),
                (Passing::Borrow, ArgumentValue::Value(scalar_size)),
                (Passing::Borrow, ArgumentValue::Value(scalar_offset)),
                (Passing::Borrow, ArgumentValue::Value(scalar_size)),
            ],
            Some(atomic),
        );
        f.call(
            store.clone(),
            &[
                (Passing::Borrow, ArgumentValue::Value(atomic)),
                (Passing::Borrow, ArgumentValue::Value(image)),
            ],
            None,
        );
    }
    f.ret(None);
    let p = fixtures::program(target, table.clone(), vec![f.finish(&table)]);
    let run = run(
        &p,
        &path("main"),
        &[],
        &mut HandlesModel,
        &Config::default(),
    );
    assert!(run.result.is_ok(), "{run:?}");
}

fn handle_control(kind: HandleControl) -> Program<Ready> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let handle = layouts.scalar(ScalarKind::U64);
    let record = layouts.get_mut(handle);
    record.kind = LayoutKind::Handle;
    record.drop = vec![DropStep::CloseHandle { offset: 0 }];
    record.copy = CopyRecipe::NotCopyable;
    let (target, table) = layouts.finish();
    let mut b = UnitBuilder::<Ready>::new(fixture_unit(path("main")));
    let entry = b.block(BlockKind::Body, &[]);
    let failed = b.block(BlockKind::FailureCleanup, &[]);
    b.term(failed, Terminator::Propagate);
    let edge = b.edge(failed, &[]);
    let failure = b.failure(edge, None, &[]);
    let file = b.owned(ValueRepr::Handle, handle);
    let open = operation(&mut b, "open");
    let mut block = call(&mut b, entry, open, &[], Some(file), failure);
    let file = if matches!(
        kind,
        HandleControl::Transfer | HandleControl::BorrowedTransfer
    ) {
        let returned = b.owned(ValueRepr::Handle, handle);
        let transfer = operation(&mut b, "transfer");
        block = call(
            &mut b,
            block,
            transfer,
            &[(
                if matches!(kind, HandleControl::Transfer) {
                    Passing::Take
                } else {
                    Passing::Borrow
                },
                ArgumentValue::Value(file),
            )],
            Some(returned),
            failure,
        );
        returned
    } else {
        file
    };
    if matches!(
        kind,
        HandleControl::ClosedValue | HandleControl::ClosedValid
    ) {
        let borrowed = b.plain(ValueRepr::Handle, handle);
        // Hand-built old identity retained as a non-owning scalar snapshot.
        let snapshot = operation(&mut b, "snapshot");
        block = call(&mut b, block, snapshot, &[], Some(borrowed), failure);
        let borrow = operation(&mut b, "borrow");
        if matches!(kind, HandleControl::ClosedValid) {
            block = call(
                &mut b,
                block,
                borrow,
                &[(Passing::Borrow, ArgumentValue::Value(borrowed))],
                None,
                failure,
            );
        }
        let consume = operation(&mut b, "consume");
        block = call(
            &mut b,
            block,
            consume,
            &[(Passing::Take, ArgumentValue::Value(file))],
            None,
            failure,
        );
        if matches!(kind, HandleControl::ClosedValue) {
            block = call(
                &mut b,
                block,
                borrow,
                &[(Passing::Borrow, ArgumentValue::Value(borrowed))],
                None,
                failure,
            );
        }
    } else {
        let slot = b.slot(ValueRepr::Handle, handle, SlotKind::Local);
        let address = b.address(AddressBase::Slot(slot), &[], ValueRepr::Handle, handle);
        b.push(block, Inst::StorageLive { slot });
        b.push(block, Inst::Address { address, failure });
        let version = b.plain(ValueRepr::Handle, handle);
        b.push(
            block,
            Inst::Store {
                address,
                value: file,
                version,
            },
        );
        let mut current_version = version;
        if matches!(
            kind,
            HandleControl::ExclusiveAlias | HandleControl::ExclusiveValid
        ) {
            let loan = b.loan(address, false);
            if matches!(kind, HandleControl::ExclusiveAlias) {
                b.push(
                    block,
                    Inst::BeginLoan {
                        loan,
                        address,
                        mutable: false,
                        if_inactive: false,
                    },
                );
            }
            let mutable = b.plain(ValueRepr::Handle, handle);
            let mutate = operation(&mut b, "mutate");
            block = call(
                &mut b,
                block,
                mutate,
                &[(
                    Passing::Mut,
                    ArgumentValue::Address {
                        address,
                        version: mutable,
                    },
                )],
                None,
                failure,
            );
            if matches!(kind, HandleControl::ExclusiveAlias) {
                b.push(
                    block,
                    Inst::EndLoan {
                        loan,
                        if_active: false,
                    },
                );
            }
            current_version = mutable;
        }
        let consume = operation(&mut b, "consume");
        block = call(
            &mut b,
            block,
            consume,
            &[(
                Passing::Take,
                ArgumentValue::Address {
                    address,
                    version: current_version,
                },
            )],
            None,
            failure,
        );
        if matches!(kind, HandleControl::TakenAddress) {
            let borrow = operation(&mut b, "borrow");
            block = call(
                &mut b,
                block,
                borrow,
                &[(Passing::Borrow, ArgumentValue::Address { address, version })],
                None,
                failure,
            );
        }
        b.push(
            block,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
    }
    b.term(block, Terminator::Return(None));
    Program {
        target,
        layouts: table.clone(),
        units: vec![b.finish_with_layouts(&table)],
        statics: vec![],
    }
}
fn handle_result(kind: HandleControl) -> Result<(), ViolationKind> {
    let program = handle_control(kind);
    run(
        &program,
        &path("main"),
        &[],
        &mut HandlesModel,
        &Config::default(),
    )
    .result
    .map(|_| ())
    .map_err(|error| error.kind)
}
#[test]
fn operation_taken_addresses_are_uninitialized_with_an_accepted_transfer_twin() {
    assert_eq!(handle_result(HandleControl::Valid), Ok(()));
    assert_eq!(
        handle_result(HandleControl::TakenAddress),
        Err(ViolationKind::UninitializedRead)
    );
}
#[test]
fn operation_mut_addresses_reject_another_live_loan_with_an_accepted_exclusive_twin() {
    assert_eq!(handle_result(HandleControl::ExclusiveValid), Ok(()));
    assert_eq!(
        handle_result(HandleControl::ExclusiveAlias),
        Err(ViolationKind::WriteShared)
    );
}
#[test]
fn operation_closed_handles_cannot_be_reused_with_an_accepted_open_twin() {
    assert_eq!(handle_result(HandleControl::ClosedValid), Ok(()));
    assert_eq!(
        handle_result(HandleControl::ClosedValue),
        Err(ViolationKind::UseAfterFree)
    );
}

#[derive(Clone)]
struct AggregateModel {
    wrong_scalar: bool,
}
fn aggregate_value(wrong_scalar: bool) -> OpValue {
    OpValue::Variant {
        tag: 1,
        value: Box::new(OpValue::Sequence(vec![OpValue::Aggregate(vec![
            OpValue::Bits {
                scalar: if wrong_scalar {
                    ScalarKind::U8
                } else {
                    ScalarKind::U64
                },
                bits: 9,
            },
            OpValue::Variant {
                tag: 0,
                value: Box::new(OpValue::Unit),
            },
        ])])),
    }
}
impl Operations for AggregateModel {
    fn call(&mut self, op: &OperationId, args: &[OpValue], _: &mut Vec<u8>) -> OpOutcome {
        match op.definition.0.name.as_str() {
            "aggregate" => OpOutcome::Value(aggregate_value(self.wrong_scalar)),
            "inspect" => {
                assert_eq!(args, &[aggregate_value(false)]);
                OpOutcome::Value(OpValue::Unit)
            }
            _ => OpOutcome::Unsupported,
        }
    }
}
#[test]
fn nested_aggregate_sequence_operands_and_results_stay_under_the_ledger() {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let word = l.scalar(ScalarKind::U64);
    let tag = l.variant(&[&[]]);
    let item = l.record(&[word, tag]);
    let stride = l.get(item).size.storage().unwrap();
    let payload = l.record(&[]);
    let record = l.get_mut(payload);
    record.kind = LayoutKind::Sequence { element: item };
    record.align = 8;
    record.size = SizeSpec::Dynamic(vec![
        SizeTerm::Constant(16),
        SizeTerm::ReadLength {
            offset: 0,
            scalar: ScalarKind::U64,
        },
        SizeTerm::Constant(stride),
        SizeTerm::Multiply,
        SizeTerm::Add,
    ]);
    let list = l.block(payload);
    let variant = l.variant(&[&[], &[list]]);
    let (target, table) = l.finish();
    let mut b = UnitBuilder::<Ready>::new(fixture_unit(path("main")));
    let entry = b.block(BlockKind::Body, &[]);
    let failed = b.block(BlockKind::FailureCleanup, &[]);
    b.term(failed, Terminator::Propagate);
    let edge = b.edge(failed, &[]);
    let failure = b.failure(edge, None, &[]);
    let value = b.owned(ValueRepr::Aggregate, variant);
    let fresh = operation(&mut b, "aggregate");
    let block = call(&mut b, entry, fresh, &[], Some(value), failure);
    let inspect = operation(&mut b, "inspect");
    let block = call(
        &mut b,
        block,
        inspect,
        &[(Passing::Borrow, ArgumentValue::Value(value))],
        None,
        failure,
    );
    b.push(
        block,
        Inst::Release {
            value,
            layout: variant,
            live: MaskRef::All,
            anchor: CLEANUP,
            failure,
        },
    );
    b.term(block, Terminator::Return(None));
    let program = Program {
        target,
        layouts: table.clone(),
        units: vec![b.finish_with_layouts(&table)],
        statics: vec![],
    };
    for wrong_scalar in [false, true] {
        let result = run(
            &program,
            &path("main"),
            &[],
            &mut AggregateModel { wrong_scalar },
            &Config::default(),
        )
        .result;
        if wrong_scalar {
            assert_eq!(result.unwrap_err().kind, ViolationKind::TypeConfusion);
        } else {
            assert!(result.unwrap().observation.failures.is_empty());
        }
    }
}

#[test]
fn operation_handle_transfer_preserves_identity_and_rejects_a_borrowed_source() {
    assert_eq!(handle_result(HandleControl::Transfer), Ok(()));
    assert_eq!(
        handle_result(HandleControl::BorrowedTransfer),
        Err(ViolationKind::Malformed)
    );
}

struct BufferModel;
impl Operations for BufferModel {
    fn call(&mut self, op: &OperationId, args: &[OpValue], _: &mut Vec<u8>) -> OpOutcome {
        match op.definition.0.name.as_str() {
            "buffer" => OpOutcome::Value(OpValue::Sequence(vec![
                OpValue::Bits {
                    scalar: ScalarKind::U8,
                    bits: 7
                };
                3
            ])),
            "update" => OpOutcome::Updated {
                value: OpValue::Unit,
                arguments: vec![(
                    0,
                    OpValue::Elements {
                        scalar: ScalarKind::U8,
                        bits: vec![1, 2, 3],
                    },
                )],
            },
            "inspect_buffer" => {
                match args {
                    [
                        OpValue::Elements {
                            scalar: ScalarKind::U8,
                            bits,
                        },
                    ] => assert_eq!(bits, &[1, 2, 3]),
                    [OpValue::Sequence(values)] => assert_eq!(
                        values,
                        &[1u128, 2, 3]
                            .into_iter()
                            .map(|bits| OpValue::Bits {
                                scalar: ScalarKind::U8,
                                bits
                            })
                            .collect::<Vec<_>>()
                    ),
                    _ => panic!("unexpected scalar buffer: {args:?}"),
                }
                OpOutcome::Value(OpValue::Unit)
            }
            _ => OpOutcome::Unsupported,
        }
    }
}
#[test]
fn scalar_buffer_updates_audit_inline_and_counted_storage_and_require_mut_access() {
    for inline in [false, true] {
        for mutable in [false, true] {
            let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
            let byte = l.scalar(ScalarKind::U8);
            let buffer = if inline {
                let id = l.record(&[]);
                let r = l.get_mut(id);
                r.kind = LayoutKind::Inline {
                    element: byte,
                    count: 3,
                };
                r.size = SizeSpec::Fixed(3);
                id
            } else {
                let payload = l.record(&[]);
                let r = l.get_mut(payload);
                r.kind = LayoutKind::Sequence { element: byte };
                r.align = 8;
                r.size = SizeSpec::Dynamic(vec![
                    SizeTerm::Constant(16),
                    SizeTerm::ReadLength {
                        offset: 0,
                        scalar: ScalarKind::U64,
                    },
                    SizeTerm::Add,
                ]);
                l.block(payload)
            };
            let (target, table) = l.finish();
            let repr = if inline {
                ValueRepr::Aggregate
            } else {
                ValueRepr::Block
            };
            let mut b = UnitBuilder::<Ready>::new(fixture_unit(path("main")));
            let entry = b.block(BlockKind::Body, &[]);
            let failed = b.block(BlockKind::FailureCleanup, &[]);
            b.term(failed, Terminator::Propagate);
            let edge = b.edge(failed, &[]);
            let failure = b.failure(edge, None, &[]);
            let value = b.owned(repr, buffer);
            let fresh = operation(&mut b, "buffer");
            let mut block = call(&mut b, entry, fresh, &[], Some(value), failure);
            let slot = b.slot(repr, buffer, SlotKind::Local);
            let address = b.address(AddressBase::Slot(slot), &[], repr, buffer);
            b.push(block, Inst::StorageLive { slot });
            b.push(block, Inst::Address { address, failure });
            let before = b.plain(repr, buffer);
            b.push(
                block,
                Inst::Store {
                    address,
                    value,
                    version: before,
                },
            );
            let after = b.plain(repr, buffer);
            let update = operation(&mut b, "update");
            block = call(
                &mut b,
                block,
                update,
                &[(
                    if mutable {
                        Passing::Mut
                    } else {
                        Passing::Borrow
                    },
                    ArgumentValue::Address {
                        address,
                        version: if mutable { after } else { before },
                    },
                )],
                None,
                failure,
            );
            let returned = b.owned(repr, buffer);
            b.push(
                block,
                Inst::Read {
                    result: returned,
                    address,
                    reaching: after,
                    mode: ReadMode::Take,
                },
            );
            let inspect = operation(&mut b, "inspect_buffer");
            block = call(
                &mut b,
                block,
                inspect,
                &[(Passing::Borrow, ArgumentValue::Value(returned))],
                None,
                failure,
            );
            b.push(
                block,
                Inst::Release {
                    value: returned,
                    layout: buffer,
                    live: MaskRef::All,
                    anchor: CLEANUP,
                    failure,
                },
            );
            b.push(
                block,
                Inst::StorageDead {
                    slot,
                    if_live: false,
                },
            );
            b.term(block, Terminator::Return(None));
            let p = Program {
                target,
                layouts: table.clone(),
                units: vec![b.finish_with_layouts(&table)],
                statics: vec![],
            };
            let result = run(&p, &path("main"), &[], &mut BufferModel, &Config::default()).result;
            if mutable {
                assert!(result.is_ok(), "{result:?}");
            } else {
                assert_eq!(result.unwrap_err().kind, ViolationKind::Malformed);
            }
        }
    }
}

#[test]
fn terminal_handle_close_failures_are_language_failures_with_a_clean_twin() {
    struct CloseModel(bool);
    impl Operations for CloseModel {
        fn call(&mut self, _: &OperationId, _: &[OpValue], _: &mut Vec<u8>) -> OpOutcome {
            OpOutcome::Value(OpValue::OpenHandle(41))
        }
        fn close_handle(
            &mut self,
            handle: u64,
            output: &mut Vec<u8>,
        ) -> Result<Option<String>, String> {
            assert_eq!(handle, 41);
            if self.0 {
                output.extend_from_slice(b"unmet client\n");
                Ok(Some("LANG-9".into()))
            } else {
                Ok(None)
            }
        }
    }
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let handle = layouts.scalar(ScalarKind::U64);
    let record = layouts.get_mut(handle);
    record.kind = LayoutKind::Handle;
    record.drop = vec![DropStep::CloseHandle { offset: 0 }];
    record.copy = CopyRecipe::NotCopyable;
    layouts.table.fn_abis[0].result = Some(hirsel_lir::abi_types::AbiResult {
        borrowed_parameters: vec![],
        layout: handle,
        register: None,
        slots: vec![hirsel_lir::abi::slot(&layouts.target, ScalarKind::U64)],
    });
    let (target, table) = layouts.finish();
    let mut b = UnitBuilder::<Ready>::new(fixture_unit(path("main")));
    let entry = b.block(BlockKind::Body, &[]);
    let failed = b.block(BlockKind::FailureCleanup, &[]);
    b.term(failed, Terminator::Propagate);
    let edge = b.edge(failed, &[]);
    let failure = b.failure(edge, None, &[]);
    let value = b.owned(ValueRepr::Handle, handle);
    let open = operation(&mut b, "open");
    let next = call(&mut b, entry, open, &[], Some(value), failure);
    b.term(next, Terminator::Return(Some(value)));
    let program = Program {
        target,
        layouts: table.clone(),
        units: vec![b.finish_with_layouts(&table)],
        statics: vec![],
    };
    for full_recount_every_step in [false, true] {
        for fails in [false, true] {
            let run = hirsel_heapcheck::run(
                &program,
                &path("main"),
                &[],
                &mut CloseModel(fails),
                &Config {
                    full_recount_every_step,
                    ..Config::default()
                },
            );
            let audit = run.result.unwrap();
            assert_eq!(audit.observation.failures.len(), usize::from(fails));
            if fails {
                assert_eq!(audit.observation.failures[0].rule, "LANG-9");
                assert_eq!(audit.observation.failures[0].definition, path("main"));
                assert_eq!(audit.observation.failures[0].site, (0, 0));
                assert_eq!(run.output, b"unmet client\n");
                assert_eq!(audit.observation.output, run.output);
            }
        }
    }
}

// Isolate the work policy from result semantics: a minimal mock provider returns
// the same borrowed byte slice for the core and foreign operation identities.
// Real numeric signatures/results are exercised by native corpus conformance.
struct NumericWorkModel {
    length: usize,
    fresh: bool,
    called: bool,
}
impl Operations for NumericWorkModel {
    fn call(&mut self, op: &OperationId, args: &[OpValue], output: &mut Vec<u8>) -> OpOutcome {
        match op.definition.0.name.as_str() {
            "fresh" => {
                self.fresh = true;
                OpOutcome::Value(OpValue::ByteSequence(vec![b'1'; self.length]))
            }
            "integer" => {
                self.called = true;
                OpOutcome::Value(OpValue::Slice {
                    argument: 0,
                    start: 0,
                    length: self.length as u64,
                })
            }
            "show" => {
                let [OpValue::Bytes(bytes)] = args else {
                    return OpOutcome::Unsupported;
                };
                output.extend_from_slice(bytes);
                OpOutcome::Value(OpValue::Unit)
            }
            _ => OpOutcome::Unsupported,
        }
    }
}
#[test]
fn numeric_work_is_exact_and_exhaustion_precedes_provider_entry() {
    for length in [0, 1, 19, 20, 128, 8192] {
        let execute = |core, config: &Config| {
            let mut model = NumericWorkModel {
                length,
                fresh: false,
                called: false,
            };
            let result = run(
                &program(ResultKind::NumericWork(core)),
                &path("main"),
                &[],
                &mut model,
                config,
            );
            (result, model)
        };
        let (plain, _) = execute(false, &Config::default());
        let (metered, _) = execute(true, &Config::default());
        assert!(plain.result.is_ok(), "{plain:?}");
        assert!(metered.result.is_ok(), "{metered:?}");
        assert_eq!(plain.output, metered.output);
        assert_eq!(metered.steps - plain.steps, length as u64 + 1);
        let exact = Config {
            step_limit: metered.steps,
            ..Config::default()
        };
        assert!(execute(true, &exact).0.result.is_ok());
        if length == 8192 {
            let short = Config {
                step_limit: length as u64,
                ..Config::default()
            };
            let (result, model) = execute(true, &short);
            assert_eq!(result.violation(), Some(ViolationKind::Unsupported));
            assert_eq!(result.steps, short.step_limit + 1);
            assert!(model.fresh && !model.called);
            assert!(result.output.is_empty());
            assert!(execute(false, &short).0.result.is_ok());
        }
    }
}
