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
    mutant.header.layouts = changed.digest();
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
