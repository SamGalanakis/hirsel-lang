//! Independent H controls: suppression is explicit and never enters the finalizer.
use crate::fixtures;
use fixtures::*;
use hirsel_heapcheck::build::{Layouts, path};
use hirsel_heapcheck::{Config, Program, ViolationKind};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::stage::Ready;

fn program(missing: bool, twice: bool) -> Program<Ready> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let resource = l.resource(&[c.boxed], Some(path("unreachable_drop")));
    let (target, table) = l.finish();
    let mut f = F::<Ready>::new("main", c);
    let n = f.int(5);
    let x = f.boxed(n);
    let construct =
        f.b.construct(ConstructorKind::Tuple, resource, &[(0, x)], HEAP);
    let r = f.b.owned(ValueRepr::Aggregate, resource);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: r,
        construct,
        failure,
    });
    let slot = f.b.slot(ValueRepr::Aggregate, resource, SlotKind::Local);
    let at =
        f.b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, resource);
    f.inst(Inst::StorageLive { slot });
    f.inst(Inst::Store {
        address: at,
        value: r,
        version: r,
    });
    if !missing {
        f.inst(Inst::Defuse { address: at });
        if twice {
            f.inst(Inst::Defuse { address: at });
        }
    }
    let field = f.b.address(
        AddressBase::Slot(slot),
        &[ProjectionStep::Field(0)],
        BLOCK,
        c.boxed,
    );
    let value = f.b.owned(BLOCK, c.boxed);
    f.inst(Inst::Read {
        result: value,
        address: field,
        reaching: r,
        mode: ReadMode::Take,
    });
    f.release(value, c.boxed);
    f.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    f.ret(None);
    Program {
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
        target,
        layouts: table.clone(),
        units: vec![f.finish(&table)],
        statics: vec![],
    }
}

#[test]
fn defusing_a_resource_runs_zero_finalizers_and_releases_all_fields() {
    for config in [
        Config::default(),
        Config {
            full_recount_every_step: true,
            ..Config::default()
        },
    ] {
        let run = run(
            &program(false, false),
            &path("main"),
            &[],
            &mut Printer,
            &config,
        );
        assert_eq!(run.counters.finalizers, 0);
        let audit = run.result.unwrap();
        assert!(audit.observation.output.is_empty());
    }
}
#[test]
fn h_rejects_duplicate_defuse() {
    let result = run(
        &program(false, true),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    )
    .result;
    assert!(matches!(result, Err(v) if v.kind == ViolationKind::DuplicateFinalizer));
}
#[test]
fn h_rejects_a_live_shell_after_field_distribution_without_defuse() {
    let result = run(
        &program(true, false),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    )
    .result;
    assert!(matches!(result, Err(v) if v.kind == ViolationKind::LeakedSlot));
}

/// A parent and all its zero-sized children occupy byte offset zero. Their
/// separate logical paths must survive child takes and explicit finalization.
fn child_occurrences(count: u32, empty: bool, twice: bool) -> Program<Ready> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let fields = if empty { Vec::new() } else { vec![c.u64] };
    let child = l.resource(&fields, Some(path("Child.drop")));
    let parent = l.resource(
        &vec![child; count as usize],
        Some(path("unreachable_parent_drop")),
    );
    let (target, table) = l.finish();
    let mut f = F::<Ready>::new("main", c);
    let mut children = Vec::new();
    for index in 0..count {
        let fields = if empty {
            Vec::new()
        } else {
            vec![(0, f.int(u128::from(index)))]
        };
        let construct = f.b.construct(ConstructorKind::Tuple, child, &fields, HEAP);
        let result = f.b.owned(ValueRepr::Aggregate, child);
        let failure = f.propagates();
        f.inst(Inst::Construct {
            result,
            construct,
            failure,
        });
        children.push((index, result));
    }
    let construct =
        f.b.construct(ConstructorKind::Tuple, parent, &children, HEAP);
    let value = f.b.owned(ValueRepr::Aggregate, parent);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: value,
        construct,
        failure,
    });
    let slot = f.b.slot(ValueRepr::Aggregate, parent, SlotKind::Local);
    let at =
        f.b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, parent);
    f.inst(Inst::StorageLive { slot });
    f.inst(Inst::Store {
        address: at,
        value,
        version: value,
    });
    f.inst(Inst::Defuse { address: at });
    for index in 0..count {
        let field = f.b.address(
            AddressBase::Slot(slot),
            &[ProjectionStep::Field(index)],
            ValueRepr::Aggregate,
            child,
        );
        // Finalize in place first: the negative twin must detect a repeated
        // entry on this child path even though its parent was defused.
        f.inst(Inst::FinalizeBegin {
            address: field,
            anchor: hirsel_heapcheck::build::CLEANUP,
        });
        f.inst(Inst::FinalizeEnd {
            address: field,
            anchor: hirsel_heapcheck::build::CLEANUP,
        });
        if twice {
            f.inst(Inst::FinalizeBegin {
                address: field,
                anchor: hirsel_heapcheck::build::CLEANUP,
            });
        }
        let moved = f.b.owned(ValueRepr::Aggregate, child);
        f.inst(Inst::Read {
            result: moved,
            address: field,
            reaching: value,
            mode: ReadMode::Take,
        });
        let child_slot = f.b.slot(ValueRepr::Aggregate, child, SlotKind::Local);
        let child_at = f.b.address(
            AddressBase::Slot(child_slot),
            &[],
            ValueRepr::Aggregate,
            child,
        );
        f.inst(Inst::StorageLive { slot: child_slot });
        f.inst(Inst::Store {
            address: child_at,
            value: moved,
            version: moved,
        });
        f.inst(Inst::StorageDead {
            slot: child_slot,
            if_live: false,
        });
    }
    f.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    f.ret(None);
    Program {
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
        target,
        layouts: table.clone(),
        units: vec![f.finish(&table)],
        statics: vec![],
    }
}

#[test]
fn parent_and_child_at_zero_are_distinct_occurrences() {
    let run = run(
        &child_occurrences(1, false, false),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(run.result.is_ok(), "{:?}", run.result);
    assert_eq!(run.counters.finalizers, 1);
}

#[test]
fn both_zero_sized_siblings_finalize_once_after_the_parent_is_defused() {
    let run = run(
        &child_occurrences(2, true, false),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(run.result.is_ok(), "{:?}", run.result);
    assert_eq!(run.counters.finalizers, 2);
}

#[test]
fn a_child_path_still_rejects_double_finalization() {
    let run = run(
        &child_occurrences(2, true, true),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(matches!(run.result, Err(v) if v.kind == ViolationKind::DuplicateFinalizer));
}

#[test]
fn a_whole_empty_resource_move_across_a_call_keeps_its_finalizer() {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let empty = l.resource(&[], Some(path("Empty.drop")));
    let (target, table) = l.finish();
    let mut callee = F::<Ready>::new("consume", c);
    let parameter = callee.b.owned(ValueRepr::Aggregate, empty);
    callee.b.parameter(parameter);
    let slot = callee.b.slot(ValueRepr::Aggregate, empty, SlotKind::Local);
    let at = callee
        .b
        .address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, empty);
    callee.inst(Inst::StorageLive { slot });
    callee.inst(Inst::Store {
        address: at,
        value: parameter,
        version: parameter,
    });
    callee.inst(Inst::FinalizeBegin {
        address: at,
        anchor: hirsel_heapcheck::build::CLEANUP,
    });
    callee.inst(Inst::FinalizeEnd {
        address: at,
        anchor: hirsel_heapcheck::build::CLEANUP,
    });
    callee.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    callee.ret(None);

    let mut main = F::<Ready>::new("main", c);
    let construct = main.b.construct(ConstructorKind::Tuple, empty, &[], HEAP);
    let value = main.b.owned(ValueRepr::Aggregate, empty);
    let failure = main.propagates();
    main.inst(Inst::Construct {
        result: value,
        construct,
        failure,
    });
    let slot = main.b.slot(ValueRepr::Aggregate, empty, SlotKind::Local);
    let at = main
        .b
        .address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, empty);
    main.inst(Inst::StorageLive { slot });
    main.inst(Inst::Store {
        address: at,
        value,
        version: value,
    });
    let moved = main.b.owned(ValueRepr::Aggregate, empty);
    main.inst(Inst::Read {
        result: moved,
        address: at,
        reaching: value,
        mode: ReadMode::Take,
    });
    main.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    let consume = main.b.direct(path("consume"));
    main.call(
        consume,
        &[(
            hirsel_core_contract::Passing::Take,
            ArgumentValue::Value(moved),
        )],
        None,
    );
    main.ret(None);
    let program = Program {
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
        target,
        layouts: table.clone(),
        units: vec![main.finish(&table), callee.finish(&table)],
        statics: vec![],
    };
    let run = run(
        &program,
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(run.result.is_ok(), "{:?}", run.result);
    assert_eq!(run.counters.finalizers, 1);
}

fn indexed_occurrences(same_index: bool) -> Program<Ready> {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep, LayoutKind};
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let child = l.resource(&[], Some(path("Child.drop")));
    let inline = l.record(&[]);
    let record = l.get_mut(inline);
    record.kind = LayoutKind::Inline {
        element: child,
        count: 2,
    };
    record.copy = CopyRecipe::NotCopyable;
    record.drop = vec![DropStep::RepeatReverse {
        offset: 0,
        count: 2,
        stride: 0,
        element: child,
    }];
    let (target, table) = l.finish();
    let mut f = F::<Ready>::new("main", c);
    let mut fields = Vec::new();
    for index in 0..2 {
        let construct = f.b.construct(ConstructorKind::Tuple, child, &[], HEAP);
        let result = f.b.owned(ValueRepr::Aggregate, child);
        let failure = f.propagates();
        f.inst(Inst::Construct {
            result,
            construct,
            failure,
        });
        fields.push((index, result));
    }
    let construct =
        f.b.construct(ConstructorKind::Sequence, inline, &fields, HEAP);
    let value = f.b.owned(ValueRepr::Aggregate, inline);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: value,
        construct,
        failure,
    });
    let slot = f.b.slot(ValueRepr::Aggregate, inline, SlotKind::Local);
    let at =
        f.b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, inline);
    f.inst(Inst::StorageLive { slot });
    f.inst(Inst::Store {
        address: at,
        value,
        version: value,
    });
    for index in [0, if same_index { 0 } else { 1 }] {
        // Each loop iteration creates a different SSA constant.
        let index = f.int(index);
        let at = f.b.address(
            AddressBase::Slot(slot),
            &[ProjectionStep::Index(index)],
            ValueRepr::Aggregate,
            child,
        );
        let failure = f.propagates();
        f.inst(Inst::Address {
            address: at,
            failure,
        });
        f.inst(Inst::FinalizeBegin {
            address: at,
            anchor: hirsel_heapcheck::build::CLEANUP,
        });
        f.inst(Inst::FinalizeEnd {
            address: at,
            anchor: hirsel_heapcheck::build::CLEANUP,
        });
    }
    f.inst(Inst::StorageDead {
        slot,
        if_live: false,
    });
    f.ret(None);
    Program {
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
        target,
        layouts: table.clone(),
        units: vec![f.finish(&table)],
        statics: vec![],
    }
}

#[test]
fn different_ssa_indices_with_equal_values_name_the_same_resource() {
    let bad = run(
        &indexed_occurrences(true),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(matches!(bad.result, Err(v) if v.kind == ViolationKind::DuplicateFinalizer));
    let twin = run(
        &indexed_occurrences(false),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(twin.result.is_ok(), "{:?}", twin.result);
    assert_eq!(twin.counters.finalizers, 2);
}

fn zero_sized_sibling_drop_program<S: hirsel_lir::stage::Stage>(take_first: bool) -> Program<S> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let child = l.resource(&[], Some(path("Child.drop")));
    let parent = l.record(&[child, child]);
    let child_ref = l.uncounted(child);
    let (target, mut table) = l.finish();
    let mut fin = F::<S>::new("Child.drop", c);
    source_finalizer(&mut fin.b, &mut table, child_ref);
    let this = fin.b.plain(ValueRepr::Uncounted, child_ref);
    fin.b.parameter(this);
    let five = fin.int(5);
    fin.print(five);
    fin.ret(None);
    let mut main = F::<S>::new("main", c);
    let mut fields = Vec::new();
    for index in 0..2 {
        let construct = main.b.construct(ConstructorKind::Tuple, child, &[], HEAP);
        let result = main.b.owned(ValueRepr::Aggregate, child);
        let failure = main.propagates();
        main.inst(Inst::Construct {
            result,
            construct,
            failure,
        });
        fields.push((index, result));
    }
    let construct = main
        .b
        .construct(ConstructorKind::Tuple, parent, &fields, HEAP);
    let value = main.b.owned(ValueRepr::Aggregate, parent);
    let failure = main.propagates();
    main.inst(Inst::Construct {
        result: value,
        construct,
        failure,
    });
    if take_first {
        let slot = main.b.slot(ValueRepr::Aggregate, parent, SlotKind::Local);
        let at = main
            .b
            .address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, parent);
        main.inst(Inst::StorageLive { slot });
        main.inst(Inst::Store {
            address: at,
            value,
            version: value,
        });
        let field = main.b.address(
            AddressBase::Slot(slot),
            &[ProjectionStep::Field(0)],
            ValueRepr::Aggregate,
            child,
        );
        let moved = main.b.owned(ValueRepr::Aggregate, child);
        main.inst(Inst::Read {
            result: moved,
            address: field,
            reaching: value,
            mode: ReadMode::Take,
        });
        main.release(moved, child);
        let failure = main.propagates();
        main.inst(Inst::Drop {
            address: at,
            if_live: true,
            anchor: hirsel_heapcheck::build::CLEANUP,
            failure,
        });
        main.inst(Inst::StorageDead {
            slot,
            if_live: false,
        });
    } else {
        main.release(value, parent);
    }
    main.ret(None);
    Program {
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
        target,
        layouts: table.clone(),
        units: vec![main.finish(&table), fin.finish(&table)],
        statics: vec![],
    }
}

#[test]
fn the_drop_program_executes_both_zero_sized_sibling_finalizers() {
    let program = zero_sized_sibling_drop_program::<Ready>(false);
    let run = run(
        &program,
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(run.result.is_ok(), "{:?}", run.result);
    assert_eq!(run.output, b"5\n5\n");
    assert_eq!(run.counters.finalizers, 2);
}

#[test]
fn a_taken_zero_sized_sibling_does_not_hide_or_redrop_its_neighbor() {
    let program = zero_sized_sibling_drop_program::<hirsel_lir::stage::Rc>(true);
    let run = run(
        &program,
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(run.result.is_ok(), "{:?}", run.result);
    assert_eq!(run.output, b"5\n5\n");
    assert_eq!(run.counters.finalizers, 2);
}

fn empty_resource_replace(overwrite: bool) -> Program<Ready> {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let empty = l.resource(&[], Some(path("Empty.drop")));
    let (target, table) = l.finish();
    let mut f = F::<Ready>::new("main", c);
    let slot = f.b.slot(ValueRepr::Aggregate, empty, SlotKind::Local);
    let displaced = f.b.slot(ValueRepr::Aggregate, empty, SlotKind::Displaced);
    let at =
        f.b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, empty);
    let old_at = f.b.address(
        AddressBase::Slot(displaced),
        &[],
        ValueRepr::Aggregate,
        empty,
    );
    f.inst(Inst::StorageLive { slot });
    f.inst(Inst::StorageLive { slot: displaced });
    for replacement in [false, true] {
        let construct = f.b.construct(ConstructorKind::Tuple, empty, &[], HEAP);
        let value = f.b.owned(ValueRepr::Aggregate, empty);
        let failure = f.propagates();
        f.inst(Inst::Construct {
            result: value,
            construct,
            failure,
        });
        if replacement && !overwrite {
            let displaced_version = f.b.plain(ValueRepr::Aggregate, empty);
            f.inst(Inst::Replace {
                address: at,
                value,
                displaced,
                version: value,
                displaced_version,
                if_live: false,
                failure,
            });
        } else {
            f.inst(Inst::Store {
                address: at,
                value,
                version: value,
            });
        }
    }
    for (slot, address) in [(displaced, old_at), (slot, at)] {
        f.inst(Inst::FinalizeBegin {
            address,
            anchor: hirsel_heapcheck::build::CLEANUP,
        });
        f.inst(Inst::FinalizeEnd {
            address,
            anchor: hirsel_heapcheck::build::CLEANUP,
        });
        f.inst(Inst::StorageDead {
            slot,
            if_live: false,
        });
    }
    f.ret(None);
    Program {
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
        target,
        layouts: table.clone(),
        units: vec![f.finish(&table)],
        statics: vec![],
    }
}

#[test]
fn empty_resource_replacement_moves_the_old_obligation_and_rejects_overwriting_it() {
    let good = run(
        &empty_resource_replace(false),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(good.result.is_ok(), "{:?}", good.result);
    assert_eq!(good.counters.finalizers, 2);
    let bad = run(
        &empty_resource_replace(true),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(matches!(bad.result, Err(v) if v.kind == ViolationKind::StoreOverInitialized));
}

fn variant_resource_paths(resource_variant: bool, present: bool) -> Program<Ready> {
    use hirsel_lir::layout_types::{DropStep, LayoutKind};
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut l);
    let child = l.resource(&[], Some(path("Child.drop")));
    let variant = l.variant(&[&[child], &[]]);
    let wrapper = l.resource(&[variant], Some(path("unreachable_parent_drop")));
    let parent = if resource_variant {
        let fin = l.get(wrapper).finalizer.clone();
        let record = l.get_mut(variant);
        record.kind = LayoutKind::Resource;
        record.finalizer = fin;
        record.drop.insert(0, DropStep::Finalize);
        variant
    } else {
        wrapper
    };
    let (target, table) = l.finish();
    let mut f = F::<Ready>::new("main", c);
    let fields = if present {
        let construct = f.b.construct(ConstructorKind::Tuple, child, &[], HEAP);
        let result = f.b.owned(ValueRepr::Aggregate, child);
        let failure = f.propagates();
        f.inst(Inst::Construct {
            result,
            construct,
            failure,
        });
        vec![(0, result)]
    } else {
        vec![]
    };
    let definition = f.b.symbol(SymbolKind::Type, path("Choice"));
    let ordinal = if present { 0 } else { 1 };
    let construct = f.b.construct(
        ConstructorKind::Variant {
            definition,
            variant: ordinal,
        },
        variant,
        &fields,
        HEAP,
    );
    let value = f.b.owned(ValueRepr::Aggregate, variant);
    let failure = f.propagates();
    f.inst(Inst::Construct {
        result: value,
        construct,
        failure,
    });
    let value = if resource_variant {
        value
    } else {
        let construct =
            f.b.construct(ConstructorKind::Tuple, parent, &[(0, value)], HEAP);
        let result = f.b.owned(ValueRepr::Aggregate, parent);
        let failure = f.propagates();
        f.inst(Inst::Construct {
            result,
            construct,
            failure,
        });
        result
    };
    let source = f.b.slot(ValueRepr::Aggregate, parent, SlotKind::Local);
    let destination = f.b.slot(ValueRepr::Aggregate, parent, SlotKind::Local);
    let src =
        f.b.address(AddressBase::Slot(source), &[], ValueRepr::Aggregate, parent);
    let dst = f.b.address(
        AddressBase::Slot(destination),
        &[],
        ValueRepr::Aggregate,
        parent,
    );
    f.inst(Inst::StorageLive { slot: source });
    f.inst(Inst::StorageLive { slot: destination });
    f.inst(Inst::Store {
        address: src,
        value,
        version: value,
    });
    let moved = f.b.owned(ValueRepr::Aggregate, parent);
    f.inst(Inst::Read {
        result: moved,
        address: src,
        reaching: value,
        mode: ReadMode::Take,
    });
    f.inst(Inst::Store {
        address: dst,
        value: moved,
        version: moved,
    });
    f.inst(Inst::StorageDead {
        slot: source,
        if_live: false,
    });
    f.inst(Inst::Defuse { address: dst });
    if present {
        let mut steps = if resource_variant {
            vec![]
        } else {
            vec![ProjectionStep::Field(0)]
        };
        steps.push(ProjectionStep::VariantField {
            variant: 0,
            field: 0,
        });
        let at = f.b.address(
            AddressBase::Slot(destination),
            &steps,
            ValueRepr::Aggregate,
            child,
        );
        let failure = f.propagates();
        f.inst(Inst::Address {
            address: at,
            failure,
        });
        f.inst(Inst::FinalizeBegin {
            address: at,
            anchor: hirsel_heapcheck::build::CLEANUP,
        });
        f.inst(Inst::FinalizeEnd {
            address: at,
            anchor: hirsel_heapcheck::build::CLEANUP,
        });
    }
    f.inst(Inst::StorageDead {
        slot: destination,
        if_live: false,
    });
    f.ret(None);
    Program {
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
        target,
        layouts: table.clone(),
        units: vec![f.finish(&table)],
        statics: vec![],
    }
}

#[test]
fn a_resource_variant_whole_move_transports_its_zero_sized_child() {
    let run = run(
        &variant_resource_paths(true, true),
        &path("main"),
        &[],
        &mut Printer,
        &Config::default(),
    );
    assert!(run.result.is_ok(), "{:?}", run.result);
    assert_eq!(run.counters.finalizers, 1);
}

#[test]
fn defuse_requires_only_the_resource_paths_in_the_active_variant() {
    for present in [false, true] {
        let run = run(
            &variant_resource_paths(false, present),
            &path("main"),
            &[],
            &mut Printer,
            &Config::default(),
        );
        assert!(run.result.is_ok(), "{:?}", run.result);
        assert_eq!(run.counters.finalizers, u64::from(present));
    }
}
