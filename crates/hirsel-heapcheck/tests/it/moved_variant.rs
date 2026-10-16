//! Semantic Rc Drop derives active payload liveness from H's cells. Explicit
//! reads and Release still reject a moved payload without a live mask.
use crate::fixtures;

use fixtures::*;
use hirsel_heapcheck::build::{CLEANUP, Layouts, path};
use hirsel_heapcheck::{Config, NoOperations, Observed, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::stage::{Rc, Ready, Stage};

fn source_repro<S: Stage>(snapshot: &str) {
    let program: Program<S> = hirsel_codec::from_text(snapshot).unwrap();
    let entry = program
        .units
        .iter()
        .find(|u| u.header.definition.0.name == "main")
        .unwrap()
        .header
        .definition
        .clone();
    let result = run(&program, &entry, &[], &mut NoOperations, &Config::default());
    let audit = result.result.unwrap_or_else(|v| panic!("{v}"));
    assert!(audit.observation.failures.is_empty());
    assert!(matches!(
        audit.observation.returned,
        Some(Observed::Integer { bits: 1, .. })
    ));
}

#[test]
fn result_string_and_bytes_payloads_pass_at_rc() {
    source_repro::<Rc>(include_str!(
        "../../../../native-tests/heap/moved-variant-payload.rc.lir"
    ));
}

#[test]
fn result_string_and_bytes_payloads_pass_at_ready() {
    source_repro::<Ready>(include_str!(
        "../../../../native-tests/heap/moved-variant-payload.ready.lir"
    ));
}

/// Retain one owning sibling to prove Drop skips only the moved field. Vary
/// active ordinals and niche tags; a whole read still retains the moved bytes
/// as uninitialized, and an explicit Release must refuse to scan them.
fn moved_payload(variant: u32, niche: bool, read_whole: bool) -> Program<Rc> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let shape = if niche {
        l.niche(0, &[c.boxed, c.boxed])
    } else {
        l.variant(&[&[c.boxed, c.boxed], &[c.boxed, c.boxed]])
    };
    let (target, table) = l.finish();
    let mut f = F::<Rc>::new("main", c);
    let five = f.int(5);
    let taken_box = f.boxed(five);
    let seven = f.int(7);
    let sibling = f.boxed(seven);
    let symbol = f.b.symbol(SymbolKind::Type, path("Choice"));
    let construct = f.b.construct(
        ConstructorKind::Variant {
            definition: symbol,
            variant,
        },
        shape,
        &[(0, taken_box), (1, sibling)],
        HEAP,
    );
    let value = f.b.owned(ValueRepr::Aggregate, shape);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: value,
        construct,
        failure,
    });
    let slot = f.b.slot(ValueRepr::Aggregate, shape, SlotKind::Local);
    let whole =
        f.b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, shape);
    f.inst(Inst::StorageLive { slot });
    f.inst(Inst::Store {
        address: whole,
        value,
        version: value,
    });
    let field = f.b.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::VariantField { variant, field: 0 }],
        BLOCK,
        c.boxed,
    );
    let failure = f.propagates();
    f.inst(Inst::Address {
        address: field,
        failure,
    });
    let result = f.b.owned(BLOCK, c.boxed);
    f.inst(Inst::Read {
        result,
        address: field,
        reaching: value,
        mode: ReadMode::Take,
    });
    let n = f.unbox(result);
    f.print(n);
    if read_whole {
        let result = f.b.owned(ValueRepr::Aggregate, shape);
        f.inst(Inst::Read {
            result,
            address: whole,
            reaching: value,
            mode: ReadMode::Take,
        });
        f.release(result, shape);
    } else {
        let failure = f.propagates();
        f.inst(Inst::Drop {
            address: whole,
            if_live: false,
            anchor: CLEANUP,
            failure,
        });
    }
    f.release(result, c.boxed);
    f.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    f.ret(None);
    let main = f.finish(&table);
    program(target, table, vec![main])
}

#[test]
fn semantic_drop_skips_moved_active_fields_and_releases_live_siblings() {
    for (variant, niche) in [(0, false), (1, false), (1, true)] {
        let result = run(
            &moved_payload(variant, niche, false),
            &path("main"),
            &[],
            &mut Printer,
            &Config::default(),
        );
        let audit = result
            .result
            .unwrap_or_else(|v| panic!("variant {variant}, niche {niche}: {v}"));
        assert_eq!(audit.observation.output, b"5\n");
        assert_eq!(result.counters.frees, 2);
    }
}

#[test]
fn reading_a_moved_payload_does_not_restore_its_scan_cells() {
    for (variant, niche) in [(0, false), (1, false)] {
        let result = run(
            &moved_payload(variant, niche, true),
            &path("main"),
            &[],
            &mut Printer,
            &Config::default(),
        );
        let violation = result.result.expect_err("the moved field is scanned");
        assert_eq!(
            violation.kind,
            ViolationKind::UninitializedScan,
            "{violation}"
        );
    }
}
