//! Already-borrowed aggregate leaves stay views in persistent environments.
use crate::fixtures;
use fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{CopyRecipe, ScanStep, TargetSpec};
use hirsel_lir::stage::Rc;

fn nested_view(owning_destination: bool, expired: bool) -> Program<Rc> {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut layouts);
    let owned = layouts.record(&[c.boxed]);
    let borrowed = layouts.record(&[c.boxed]);
    let record = layouts.get_mut(borrowed);
    record.scan = vec![ScanStep::Uncounted { offset: 0 }];
    record.drop.clear();
    record.copy = CopyRecipe::Bits;
    let field = if owning_destination { owned } else { borrowed };
    let environment = layouts.record(&[field]);
    let container = layouts.block(environment);
    let (target, table) = layouts.finish();

    let mut main = F::<Rc>::new("main", c);
    let n = main.int(42);
    let source = main.boxed(n);
    let slot = main.b.slot(BLOCK, c.boxed, SlotKind::Local);
    let address = main.b.address(AddressBase::Slot(slot), &[], BLOCK, c.boxed);
    main.inst(Inst::StorageLive { slot });
    main.inst(Inst::Store {
        address,
        value: source,
        version: source,
    });
    let loan = main.b.loan(address, false);
    main.inst(Inst::BeginLoan {
        loan,
        address,
        mutable: false,
        if_inactive: false,
    });
    let view = main.b.value(BLOCK, c.boxed, Holding::Borrowed(loan));
    main.inst(Inst::Read {
        result: view,
        address,
        reaching: source,
        mode: ReadMode::Borrow,
    });
    let construct = main
        .b
        .construct(ConstructorKind::Tuple, borrowed, &[(0, view)], HEAP);
    let data = main.b.plain(ValueRepr::Aggregate, borrowed);
    let failure = main.propagates();
    main.inst(Inst::Construct {
        result: data,
        construct,
        failure,
    });
    let construct = main
        .b
        .construct(ConstructorKind::Tuple, container, &[(0, data)], HEAP);
    let stored = main.b.owned(BLOCK, container);
    let failure = main.propagates();
    main.inst(Inst::Construct {
        result: stored,
        construct,
        failure,
    });
    if expired {
        main.inst(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }
    let view = main.load(
        stored,
        &[ProjectionStep::Field(0), ProjectionStep::Field(0)],
        BLOCK,
        c.boxed,
    );
    let n = main.unbox(view);
    main.print(n);
    main.release(stored, container);
    if !expired {
        main.inst(Inst::EndLoan {
            loan,
            if_active: false,
        });
    }
    let source_value = main.b.owned(BLOCK, c.boxed);
    main.inst(Inst::Read {
        result: source_value,
        address,
        reaching: source,
        mode: ReadMode::Take,
    });
    main.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    main.release(source_value, c.cell);
    main.ret(None);
    program(target, table.clone(), vec![main.finish(&table)])
}

#[test]
fn nested_borrowed_cells_can_be_stored_in_a_persistent_environment() {
    let run = run(
        &nested_view(false, false),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert_eq!(run.result.unwrap().observation.output, b"42\n");
}

#[test]
fn an_owning_destination_still_requires_a_holder_credit() {
    let run = run(
        &nested_view(true, false),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert_eq!(run.result.unwrap_err().kind, ViolationKind::BorrowedStore);
}

#[test]
fn nested_view_cells_keep_their_original_loan() {
    let run = run(
        &nested_view(false, true),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert_eq!(run.result.unwrap_err().kind, ViolationKind::BorrowExpired);
}
