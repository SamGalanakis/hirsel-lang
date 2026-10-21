//! Shared program checks never stand in for a stage's independent unit check.
use crate::support::{self, ENTRY, Lir, l};
use hirsel_core_contract::Passing;
use hirsel_lir::{
    LayoutTable, LirUnit, TargetSpec,
    stage::{Placed, Rc, Ready, Reuse, Stage, Value},
    validate::{self, Inputs, ValidationContext, ViolationKind},
};

fn identity<S: Stage>() -> (LirUnit<S>, LayoutTable, hirsel_core_contract::Manifest) {
    let mut t = Lir::<S>::new(&[(Passing::Borrow, l::I64)], Some(l::I64));
    let value = t.plain(l::I64);
    t.param(value);
    t.builder.ret(ENTRY, Some(value));
    t.finish()
}

fn stage<S: Stage>() {
    let (unit, table, manifest) = identity::<S>();
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let operations = support::registry();
    let inputs = Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let context = ValidationContext::new(&inputs);
    let ordinary = validate::validate(unit.clone(), &inputs).unwrap();
    for _ in 0..2 {
        let shared = context.validate(unit.clone(), &inputs).unwrap();
        assert_eq!(shared.unit(), ordinary.unit());
        assert_eq!(shared.receipt(), ordinary.receipt());
    }
    // A previous verdict for this same unit cannot hide a later mutation.
    let mut mutant = unit.clone();
    mutant.parameters.clear();
    let errors = context
        .validate(mutant.clone(), &inputs)
        .unwrap_err()
        .violations;
    assert!(!errors.is_empty());
    assert_eq!(errors, validate::check(&mutant, &inputs));
    assert!(context.check(&unit, &inputs).is_empty());

    // A changed table at another address must never use the cached verdict or
    // digest. Rebind the unit's identity so it reaches layout validation.
    let mut changed = table.clone();
    changed.layouts[l::I64.0 as usize].align = 3;
    mutant = unit.clone();
    let changed_inputs = Inputs {
        layouts: &changed,
        ..inputs
    };
    let errors = context.check(&mutant, &changed_inputs);
    assert!(
        errors
            .iter()
            .any(|v| matches!(v.kind, ViolationKind::Layout(_)))
    );
    assert_eq!(errors, validate::check(&mutant, &changed_inputs));

    // Target and manifest identity are independent of the table borrow too.
    let other_target = TargetSpec::aarch64_unknown_linux_gnu();
    let changed_inputs = Inputs {
        target: &other_target,
        ..inputs
    };
    let errors = context.check(&unit, &changed_inputs);
    assert!(!errors.is_empty());
    assert_eq!(errors, validate::check(&unit, &changed_inputs));
    let other_manifest = hirsel_core_contract::Manifest::skeleton([]);
    let changed_inputs = Inputs {
        manifest: &other_manifest,
        ..inputs
    };
    let shared = context.validate(unit.clone(), &changed_inputs).unwrap();
    let ordinary = validate::validate(unit, &changed_inputs).unwrap();
    assert_eq!(shared.receipt(), ordinary.receipt());
}

#[test]
fn shared_checks_preserve_receipts_and_reject_mutations_at_every_stage() {
    stage::<Value>();
    stage::<Placed>();
    stage::<Rc>();
    stage::<Reuse>();
    stage::<Ready>();
}

#[test]
fn program_validation_context_is_sync() {
    fn sync<T: Sync>() {}
    sync::<ValidationContext<'_>>();
}

#[test]
fn preflight_checks_without_units_and_raw_errors_remain_schema_first() {
    let (mut unit, mut table, manifest) = identity::<Ready>();
    table.layouts[l::I64.0 as usize].align = 3;
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let operations = support::registry();
    let inputs = Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let context = ValidationContext::new(&inputs);
    // No unit validation has run: even an empty build must reject this table.
    let errors = context.preflight().unwrap_err();
    assert!(!errors.is_empty());
    assert!(std::ptr::eq(errors, context.preflight().unwrap_err()));
    assert!(
        context
            .check(&unit, &inputs)
            .iter()
            .all(|v| matches!(v.kind, ViolationKind::Layout(_)))
    );
    unit.header.lir_version = 0;
    // A cached preflight failure cannot hide the raw unit's schema failure.
    let errors = validate::check(&unit, &inputs);
    assert_eq!(errors.len(), 1);
    assert!(matches!(errors[0].kind, ViolationKind::Schema(_)));
    assert_eq!(context.check(&unit, &inputs), errors);
}

#[test]
fn input_evidence_covers_exact_rows_and_ignores_unrelated_program_changes() {
    use hirsel_core_contract::{Disposition, Manifest, OperationId};
    use hirsel_lir::{LayoutId, LayoutKeys, layout_types::LayoutKind, validate::UnitInputs};
    let (mut unit, table, manifest) = identity::<Ready>();
    unit.tables.operations.clear();
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let evidence = |table: &LayoutTable, manifest: &Manifest| {
        UnitInputs {
            target: &target,
            layouts: table,
            manifest,
        }
        .digest(&unit)
        .unwrap()
    };
    let before = evidence(&table, &manifest);
    let mut unrelated = table.clone();
    unrelated.layouts.push(table.layouts[0].clone());
    unrelated.fn_abis.push(table.fn_abis[0].clone());
    assert_eq!(before, evidence(&unrelated, &manifest));
    let empty = Manifest::skeleton([]);
    let unrelated_operation = OperationId {
        definition: support::path("unrelated"),
        declaration_digest: support::hash("unrelated"),
    };
    let unrelated_manifest = Manifest::skeleton(
        manifest
            .rows()
            .map(|r| r.id.clone())
            .chain([unrelated_operation]),
    );
    assert_eq!(before, evidence(&table, &unrelated_manifest));

    let keys = LayoutKeys::new(&table);
    let mut changed = table.clone();
    changed.layouts[l::I64.0 as usize].align = 3;
    assert_ne!(before, evidence(&changed, &manifest));
    // Producer-stamped keys are still the old keys: they are never authority
    // for the validation evidence.
    assert_eq!(keys, LayoutKeys::new(&table));
    let mut changed = table.clone();
    changed.fn_abis[unit.header.abi.0 as usize].caller_site = true;
    assert_ne!(before, evidence(&changed, &manifest));

    // Traverse recursive references exactly once, including an indirect row
    // that appears in neither the unit's types nor its FnAbi.
    let mut cyclic = table.clone();
    let child = LayoutId(cyclic.layouts.len() as u32);
    cyclic
        .layouts
        .push(table.layouts[l::I64.0 as usize].clone());
    cyclic.layouts[l::I64.0 as usize].kind = LayoutKind::Box { payload: child };
    cyclic.layouts[child.0 as usize].kind = LayoutKind::Box { payload: l::I64 };
    let inputs = UnitInputs {
        target: &target,
        layouts: &cyclic,
        manifest: &manifest,
    };
    assert_eq!(inputs.closure(&unit).unwrap().layouts, [l::I64, child]);
    let recursive = inputs.digest(&unit).unwrap();
    cyclic.layouts[child.0 as usize].align += 1;
    assert_ne!(recursive, evidence(&cyclic, &manifest));

    let operation = OperationId {
        definition: support::path("witness"),
        declaration_digest: support::hash("witness"),
    };
    unit.tables.operations.push(operation.clone());
    let mut used = Manifest::skeleton([operation.clone()]);
    let inputs = UnitInputs {
        target: &target,
        layouts: &table,
        manifest: &used,
    };
    let original = inputs.digest(&unit).unwrap();
    let mut row = used.row(&operation).unwrap().clone();
    row.dispositions.native = Disposition::RuntimePrimitive("another primitive".into());
    used.replace(row).unwrap();
    let inputs = UnitInputs {
        target: &target,
        layouts: &table,
        manifest: &used,
    };
    assert_ne!(original, inputs.digest(&unit).unwrap());
    let inputs = UnitInputs {
        manifest: &empty,
        ..inputs
    };
    assert_ne!(original, inputs.digest(&unit).unwrap());
}

#[test]
fn the_input_closure_includes_implicit_slice_storage_and_rejects_foreign_indices() {
    use hirsel_lir::{
        LayoutId, LayoutIndex,
        ir::{LirType, ValueRepr},
        layout_types::LayoutKind,
        validate::UnitInputs,
    };
    let (mut unit, mut table, manifest) = identity::<Ready>();
    unit.types.push(LirType {
        repr: ValueRepr::Aggregate,
        layout: l::SLICE,
    });
    let element = match table.layouts[l::SLICE.0 as usize].kind {
        LayoutKind::Slice { element } => element,
        _ => unreachable!(),
    };
    let storage = match LayoutIndex::new(&table).sequence(element) {
        Some(id) => id,
        None => {
            let id = LayoutId(table.layouts.len() as u32);
            let mut row = table.layouts[element.0 as usize].clone();
            row.kind = LayoutKind::Sequence { element };
            table.layouts.push(row);
            id
        }
    };
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = UnitInputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let index = LayoutIndex::new(&table);
    assert!(
        inputs
            .closure_with_index(&unit, &index)
            .unwrap()
            .layouts
            .contains(&storage)
    );
    let evidence = inputs.digest_with_index(&unit, &index).unwrap();
    assert_eq!(evidence, inputs.digest(&unit).unwrap());
    assert_eq!(
        index.scalar(hirsel_lir::layout_types::ScalarKind::I64),
        Some(l::I64)
    );
    let mut changed = table.clone();
    changed.layouts[storage.0 as usize].align += 1;
    let changed_inputs = UnitInputs {
        layouts: &changed,
        ..inputs
    };
    assert!(changed_inputs.digest_with_index(&unit, &index).is_err());
    assert_ne!(evidence, changed_inputs.digest(&unit).unwrap());
}
