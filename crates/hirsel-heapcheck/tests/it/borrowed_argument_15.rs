//! Borrow provenance is local to a call frame, including aggregate cells.
use crate::fixtures::*;
use hirsel_core_contract::Passing;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, ViolationKind};
use hirsel_lir::abi_types::*;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{CopyRecipe, ScanStep, TargetSpec};
use hirsel_lir::stage::Rc;

fn borrowed_argument(expire_before: bool, expire_after: bool) -> Program<Rc> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut layouts);
    let view_layout = layouts.record(&[c.boxed]);
    let record = layouts.get_mut(view_layout);
    record.scan = vec![ScanStep::Uncounted { offset: 0 }];
    record.drop.clear();
    record.copy = CopyRecipe::Bits;
    let (target, mut table) = layouts.finish();

    let mut echo = F::<Rc>::new("echo", c);
    source_finalizer(&mut echo.b, &mut table, view_layout);
    table.fn_abis[echo.b.unit.header.abi.0 as usize].parameters[0].passing =
        ParameterPassing::ReadOnlyAddress;
    table.fn_abis[echo.b.unit.header.abi.0 as usize].result = Some(AbiResult {
        borrowed_parameters: vec![0],
        layout: view_layout,
        register: None,
        slots: vec![],
    });
    let parameter = echo.b.plain(ValueRepr::Aggregate, view_layout);
    echo.b.unit.parameters.push(parameter);
    echo.ret(Some(parameter));
    let echo = echo.finish(&table);

    let mut main = F::<Rc>::new("main", c);
    let n = main.int(42);
    let owned = main.boxed(n);
    let slot = main.b.slot(BLOCK, c.boxed, SlotKind::Local);
    let at = main.b.address(AddressBase::Slot(slot), &[], BLOCK, c.boxed);
    main.inst(Inst::StorageLive { slot });
    main.inst(Inst::Store {
        address: at,
        value: owned,
        version: owned,
    });
    let loan = main.b.loan(at, false);
    main.inst(Inst::BeginLoan {
        loan,
        address: at,
        mutable: false,
        if_inactive: false,
    });
    let view = main.b.value(BLOCK, c.boxed, Holding::Borrowed(loan));
    main.inst(Inst::Read {
        result: view,
        address: at,
        reaching: owned,
        mode: ReadMode::Borrow,
    });
    let construct = main
        .b
        .construct(ConstructorKind::Tuple, view_layout, &[(0, view)], HEAP);
    let aggregate = main.b.plain(ValueRepr::Aggregate, view_layout);
    let failure = main.propagates();
    main.inst(Inst::Construct {
        result: aggregate,
        construct,
        failure,
    });
    if expire_before {
        main.inst(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }
    let callee = main.b.direct(path("echo"));
    let returned = main.b.plain(ValueRepr::Aggregate, view_layout);
    main.call(
        callee,
        &[(Passing::Borrow, ArgumentValue::Value(aggregate))],
        Some(returned),
    );
    if expire_after {
        main.inst(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }
    let block = main.load(returned, &[ProjectionStep::Field(0)], BLOCK, c.boxed);
    let number = main.unbox(block);
    main.print(number);
    if !expire_before && !expire_after {
        main.inst(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }
    let owned_again = main.b.owned(BLOCK, c.boxed);
    main.inst(Inst::Read {
        result: owned_again,
        address: at,
        reaching: owned,
        mode: ReadMode::Take,
    });
    main.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    main.release(owned_again, c.cell);
    main.ret(None);
    let main = main.finish(&table);
    program(target, table, vec![main, echo])
}

#[test]
fn borrowed_aggregate_argument_and_return_keep_the_callers_loan() {
    let program = borrowed_argument(false, false);
    let run = run(
        &program,
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(run.result.is_ok(), "{:?}", run.result);
    assert_eq!(run.output, b"42\n");
}

#[test]
fn expired_aggregate_argument_still_traps_before_call() {
    let program = borrowed_argument(true, false);
    let run = run(
        &program,
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    let violation = run.result.unwrap_err();
    assert_eq!(violation.kind, ViolationKind::BorrowExpired);
    assert_eq!(violation.site.unwrap().definition, path("main"));
}

#[test]
fn returned_aggregate_still_traps_after_the_callers_loan_ends() {
    let program = borrowed_argument(false, true);
    let run = run(
        &program,
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    let violation = run.result.unwrap_err();
    assert_eq!(violation.kind, ViolationKind::BorrowExpired);
    assert_eq!(violation.site.unwrap().definition, path("main"));
    assert!(run.output.is_empty());
}
