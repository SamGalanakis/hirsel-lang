//! Hand-built controls for the independent PS-13 projection-evidence rule.
use crate::support::*;
use hirsel_core_contract::Passing;
use hirsel_lir::discharge::{Discharge, Discharged, Form};
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use hirsel_lir::validate::{Inputs, check, validate_discharge};
use std::collections::BTreeSet;

#[test]
fn unchecked_projections_require_the_exact_accepted_site_place_and_rule() {
    for variant in [false, true] {
        let layout = if variant { l::OPT } else { l::SEQBLK };
        let mut t = Lir::<Value>::new(
            &[(Passing::Borrow, layout), (Passing::Borrow, l::U64)],
            Some(l::I64),
        );
        let slot = t.slot(layout, SlotKind::Parameter(0), ROOT);
        let version = t.version_of(slot);
        t.param(version);
        let index = t.plain(l::U64);
        t.param(index);
        let step = if variant {
            ProjectionStep::VariantField {
                variant: 1,
                field: 0,
            }
        } else {
            ProjectionStep::Index(index)
        };
        let at = t.at(ENTRY, slot, &[step], &[]);
        t.builder.unit.addresses[at.index()].source_place = Some(7);
        t.builder.unit.addresses[at.index()].unchecked = vec![0];
        let read = t.read(ENTRY, at, version, ReadMode::Bits, None);
        t.builder.ret(ENTRY, Some(read));
        let operations = t.operations.clone();
        let (unit, table, manifest) = t.finish();
        let site = Discharged {
            body: 0,
            block: 0,
            statement: 0,
            form: Form::Projection {
                place: 7,
                rule: if variant { "MEM-13" } else { "MEM-14" }.into(),
            },
        };
        let plan = BTreeSet::from([site.clone()]);
        let accepted = Discharge([(unit.header.definition.clone(), plan.clone())].into());
        let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
        let validate = |unit: &LirUnit<Value>, discharge: Option<&Discharge>| {
            check(
                unit,
                &Inputs {
                    memo: Default::default(),
                    target: &target,
                    layouts: &table,
                    manifest: &manifest,
                    operations: &operations,
                    discharge,
                },
            )
        };
        assert!(validate(&unit, Some(&accepted)).is_empty());
        let mut reference = unit.clone();
        reference.addresses[at.index()].unchecked.clear();
        assert!(validate(&reference, None).is_empty());
        assert_eq!(
            validate_discharge(&reference, &unit, &plan),
            Ok(plan.clone())
        );
        assert!(validate_discharge(&reference, &unit, &BTreeSet::new()).is_err());
        assert!(validate_discharge(&reference, &reference, &plan).is_err());
        let bad = K::Type("unchecked projection evidence");
        // Without the evidence rule each of these hand-built forgeries is
        // otherwise a valid unit, as the accepted control demonstrates.
        assert!(validate(&unit, None).iter().any(|v| v.kind == bad));
        for changed in 0..4 {
            let mut forged = site.clone();
            match changed {
                0 => forged.body += 1,
                1 => forged.statement += 1,
                2 => {
                    if let Form::Projection { place, .. } = &mut forged.form {
                        *place += 1;
                    }
                }
                _ => {
                    if let Form::Projection { rule, .. } = &mut forged.form {
                        *rule = "OTHER".into();
                    }
                }
            }
            let forged =
                Discharge([(unit.header.definition.clone(), BTreeSet::from([forged]))].into());
            assert!(validate(&unit, Some(&forged)).iter().any(|v| v.kind == bad));
        }
        for unchecked in [vec![1], vec![0, 0]] {
            let mut forged = unit.clone();
            forged.addresses[at.index()].unchecked = unchecked;
            assert!(
                validate(&forged, Some(&accepted))
                    .iter()
                    .any(|v| v.kind == bad)
            );
        }
    }
}
