//! The external trace construct is adversarial input, separate from LIR authority.
use crate::support::{self, ENTRY, Lir, ROOT, l};
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::ownership_trace::*;
use hirsel_lir::stage::Rc;

fn fixture() -> (LirUnit<Rc>, Plan) {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::BLK)], Some(l::BLK));
    let value = t.owned(l::BLK);
    t.param(value);
    let slot = t.slot(l::BLK, SlotKind::Local, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let address = t.at(ENTRY, slot, &[], &[support::Undo::Release(value)]);
    let version = t.store(ENTRY, address, value, slot);
    let result = t.read(ENTRY, address, version, ReadMode::Take, None);
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(ENTRY, Some(result));
    let operations = t.operations.clone();
    let (unit, table, manifest) = t.finish();
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let unit = hirsel_lir::validate::validate(unit, &inputs)
        .unwrap()
        .into_raw();
    let site = unit.tables.source_anchors[0].site;
    let mut observations = vec![Observation {
        point: Point::Entry,
        marker: ObserveOwnership {
            id: MarkerId(0),
            site,
            kind: Kind::Move,
            token: Location::Parameter(0),
            owner: Location::Value(value),
        },
    }];
    for (index, inst) in unit.blocks[ENTRY.index()]
        .instructions
        .get(&unit.instructions)
        .unwrap()
        .iter()
        .enumerate()
    {
        let (token, owner) = match *inst {
            Inst::Store { .. } => (Location::Value(value), Location::Address(address)),
            Inst::Read { .. } => (Location::Address(address), Location::Value(result)),
            _ => continue,
        };
        observations.push(Observation {
            point: Point::Instruction {
                block: ENTRY,
                index: index as u32,
                moment: Moment::After,
            },
            marker: ObserveOwnership {
                id: MarkerId(observations.len() as u32),
                site,
                kind: Kind::Move,
                token,
                owner,
            },
        });
    }
    observations.push(Observation {
        point: Point::Return(ENTRY),
        marker: ObserveOwnership {
            id: MarkerId(3),
            site,
            kind: Kind::Move,
            token: Location::Value(result),
            owner: Location::Return,
        },
    });
    let plan = Plan::new(&unit, observations);
    (unit, plan)
}

fn recipe(input: &Plan, output: &Plan) -> Recipe {
    Recipe {
        before: input.unit,
        after: output.unit,
        relocations: output
            .observations
            .iter()
            .map(|o| Relocation {
                id: o.marker.id,
                point: o.point,
                token: o.marker.token,
                owner: o.marker.owner,
            })
            .collect(),
    }
}

#[test]
fn ownership_trace_external_plan_never_changes_ordinary_lir() {
    let (unit, plan) = fixture();
    let text = hirsel_codec::to_text(&unit);
    let digest = unit.digest();
    let checked = check(&unit, &plan).unwrap();
    assert_eq!(
        checked.observation(MarkerId(2)),
        Some(&plan.observations[2])
    );
    assert_eq!(hirsel_codec::to_text(&unit), text);
    assert_eq!(unit.digest(), digest);
    assert!(!text.contains("ObserveOwnership"));
}

#[test]
fn ownership_trace_sidecar_roundtrip_is_raw_and_must_be_checked() {
    let (unit, plan) = fixture();
    let decoded: Plan = hirsel_codec::from_text(&hirsel_codec::to_text(&plan)).unwrap();
    assert_eq!(decoded, plan);
    check(&unit, &decoded).unwrap();
    let mut stale = decoded;
    stale.unit = support::hash("another unit");
    assert_eq!(check(&unit, &stale).unwrap_err(), Error::StalePlan);
}

#[test]
fn ownership_trace_even_an_unchanged_pass_must_emit_a_recipe() {
    let (unit, plan) = fixture();
    assert_eq!(
        check_transform(&unit, &plan, &unit, &plan, None).unwrap_err(),
        Error::MissingRecipe
    );
    check_transform(&unit, &plan, &unit, &plan, Some(&recipe(&plan, &plan))).unwrap();
}

#[test]
fn ownership_trace_dropped_marker_fails_at_the_rewriting_boundary() {
    let (unit, plan) = fixture();
    let r = recipe(&plan, &plan);
    let mut dropped = plan.clone();
    dropped.observations.remove(2);
    assert_eq!(
        check_transform(&unit, &plan, &unit, &dropped, Some(&r)).unwrap_err(),
        Error::MissingMarker(MarkerId(2))
    );
}

#[test]
fn ownership_trace_forged_marker_cannot_join_the_inventory() {
    let (unit, plan) = fixture();
    let mut input = plan.clone();
    input.observations.remove(0);
    assert_eq!(
        check_transform(&unit, &input, &unit, &plan, Some(&recipe(&input, &input))).unwrap_err(),
        Error::ForgedMarker(MarkerId(0))
    );
}

#[test]
fn ownership_trace_duplicate_marker_is_not_a_second_event() {
    let (unit, mut plan) = fixture();
    plan.observations.push(plan.observations[1].clone());
    assert_eq!(
        check(&unit, &plan).unwrap_err(),
        Error::DuplicateMarker(MarkerId(1))
    );
}

#[test]
fn ownership_trace_a_recipe_cannot_omit_duplicate_or_invent_an_input() {
    let (unit, plan) = fixture();
    let mut r = recipe(&plan, &plan);
    r.relocations.remove(2);
    assert_eq!(
        check_transform(&unit, &plan, &unit, &plan, Some(&r)).unwrap_err(),
        Error::MissingRelocation(MarkerId(2))
    );
    r.relocations.push(r.relocations[0].clone());
    assert_eq!(
        check_transform(&unit, &plan, &unit, &plan, Some(&r)).unwrap_err(),
        Error::DuplicateRelocation(MarkerId(0))
    );
    r.relocations.last_mut().unwrap().id = MarkerId(99);
    assert_eq!(
        check_transform(&unit, &plan, &unit, &plan, Some(&r)).unwrap_err(),
        Error::ForgedRelocation(MarkerId(99))
    );
}

#[test]
fn ownership_trace_relocation_binds_both_unit_identities() {
    let (unit, plan) = fixture();
    for before in [false, true] {
        let mut r = recipe(&plan, &plan);
        if before {
            r.before = support::hash("wrong input");
        } else {
            r.after = support::hash("wrong output");
        }
        assert_eq!(
            check_transform(&unit, &plan, &unit, &plan, Some(&r)).unwrap_err(),
            Error::StaleRecipe
        );
    }
}

#[test]
fn ownership_trace_a_valid_but_wrong_operation_is_misattribution() {
    let (unit, plan) = fixture();
    let mut wrong = plan.clone();
    let source = wrong.observations[2].clone();
    wrong.observations[1].point = source.point;
    wrong.observations[1].marker.token = source.marker.token;
    wrong.observations[1].marker.owner = source.marker.owner;
    wrong.observations.remove(2);
    // Each placement is locally legal, but the pass's unchanged recipe proves
    // that id1 was a store, not the subsequent take from that storage.
    check(&unit, &wrong).unwrap();
    assert_eq!(
        check_transform(&unit, &plan, &unit, &wrong, Some(&recipe(&plan, &plan))).unwrap_err(),
        Error::ChangedMarker(MarkerId(1))
    );
}

#[test]
fn ownership_trace_source_kind_and_operand_cannot_be_forged() {
    let (unit, plan) = fixture();
    let mut wrong = plan.clone();
    wrong.observations[1].marker.owner = Location::Return;
    assert_eq!(
        check(&unit, &wrong).unwrap_err(),
        Error::InvalidBinding(MarkerId(1))
    );
    wrong = plan.clone();
    wrong.observations[1].marker.kind = Kind::Create;
    assert_eq!(
        check(&unit, &wrong).unwrap_err(),
        Error::InvalidBinding(MarkerId(1))
    );
    wrong = plan.clone();
    wrong.observations[1].marker.site.start = 123;
    assert_eq!(
        check(&unit, &wrong).unwrap_err(),
        Error::InvalidSite(MarkerId(1))
    );
}

#[test]
fn ownership_trace_suborder_changes_require_an_explicit_recipe() {
    let (unit, plan) = fixture();
    let mut wrong = plan.clone();
    wrong.observations.swap(1, 2);
    assert_eq!(
        check_transform(&unit, &plan, &unit, &wrong, Some(&recipe(&plan, &plan))).unwrap_err(),
        Error::ChangedOrder
    );
}

#[test]
fn ownership_trace_invalid_point_and_future_version_fail_closed() {
    let (unit, mut plan) = fixture();
    plan.observations[0].point = Point::Return(BlockId(u32::MAX));
    assert_eq!(
        check(&unit, &plan).unwrap_err(),
        Error::InvalidPoint(MarkerId(0))
    );
    plan.version += 1;
    assert_eq!(
        check(&unit, &plan).unwrap_err(),
        Error::Version(VERSION + 1)
    );
}

#[test]
fn ownership_trace_relocated_instruction_is_checked_against_new_lir() {
    let (unit, plan) = fixture();
    let mut after = unit.clone();
    let range = &mut after.blocks[ENTRY.index()].instructions;
    let end = (range.start + range.len) as usize;
    after
        .instructions
        .insert(end, Inst::EndScope { scope: ROOT });
    range.len += 1;
    let added_index = range.len - 1;
    for b in after.blocks.iter_mut().skip(1) {
        b.instructions.start += 1;
    }
    let output = Plan::new(&after, plan.observations.clone());
    let r = recipe(&plan, &output);
    check_transform(&unit, &plan, &after, &output, Some(&r)).unwrap();
    let mut forged = output.clone();
    forged.observations[1].point = Point::Instruction {
        block: ENTRY,
        index: added_index,
        moment: Moment::After,
    };
    assert_eq!(
        check(&after, &forged).unwrap_err(),
        Error::InvalidBinding(MarkerId(1))
    );
}

#[test]
fn ownership_trace_renumbering_a_duplicate_does_not_make_it_a_new_event() {
    let (unit, mut plan) = fixture();
    let mut extra = plan.observations[1].clone();
    extra.marker.id = MarkerId(99);
    plan.observations.push(extra);
    assert_eq!(
        check(&unit, &plan).unwrap_err(),
        Error::DuplicateObservation(MarkerId(99))
    );
}

#[test]
fn ownership_trace_source_must_belong_to_the_observed_operation() {
    let (mut unit, mut plan) = fixture();
    let mut other = unit.tables.source_anchors[0].clone();
    other.site.start = 77;
    other.site.end = 78;
    unit.tables.source_anchors.push(other.clone());
    plan.unit = unit.digest();
    plan.observations[1].marker.site = other.site;
    assert_eq!(
        check(&unit, &plan).unwrap_err(),
        Error::InvalidSite(MarkerId(1))
    );
}

#[test]
fn ownership_trace_cannot_authorize_a_missing_semantic_release() {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::BLK)], None);
    let value = t.owned(l::BLK);
    t.param(value);
    t.builder.ret(ENTRY, None);
    let operations = t.operations.clone();
    let (unit, table, manifest) = t.finish();
    let plan = Plan::new(
        &unit,
        vec![Observation {
            point: Point::Return(ENTRY),
            marker: ObserveOwnership {
                id: MarkerId(0),
                site: unit.tables.source_anchors[0].site,
                kind: Kind::Drop,
                token: Location::Value(value),
                owner: Location::Value(value),
            },
        }],
    );
    assert_eq!(
        check(&unit, &plan).unwrap_err(),
        Error::InvalidBinding(MarkerId(0))
    );
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let rejected = hirsel_lir::validate::validate(unit, &inputs).unwrap_err();
    assert!(
        rejected
            .violations
            .iter()
            .any(|v| v.kind == support::K::Leak(value))
    );
}

#[test]
fn ownership_trace_creation_and_release_bind_to_their_actual_instructions() {
    let mut t = Lir::<Rc>::new(&[], None);
    let value = t.new_block(ENTRY, &[]);
    t.release(ENTRY, value, &[]);
    t.builder.ret(ENTRY, None);
    let operations = t.operations.clone();
    let (unit, table, manifest) = t.finish();
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let unit = hirsel_lir::validate::validate(unit, &inputs)
        .unwrap()
        .into_raw();
    let site = unit.tables.source_anchors[0].site;
    let observations = unit.blocks[ENTRY.index()]
        .instructions
        .get(&unit.instructions)
        .unwrap()
        .iter()
        .enumerate()
        .filter_map(|(index, inst)| {
            let (kind, moment) = match inst {
                Inst::Construct { .. } => (Kind::Create, Moment::After),
                Inst::Release { .. } => (Kind::Drop, Moment::Before),
                _ => return None,
            };
            Some(Observation {
                point: Point::Instruction {
                    block: ENTRY,
                    index: index as u32,
                    moment,
                },
                marker: ObserveOwnership {
                    id: MarkerId(index as u32),
                    site,
                    kind,
                    token: Location::Value(value),
                    owner: Location::Value(value),
                },
            })
        })
        .collect();
    let plan = Plan::new(&unit, observations);
    assert_eq!(plan.observations.len(), 2);
    check(&unit, &plan).unwrap();
    let mut early = plan.clone();
    if let Point::Instruction { moment, .. } = &mut early.observations[0].point {
        *moment = Moment::Before;
    }
    assert_eq!(
        check(&unit, &early).unwrap_err(),
        Error::InvalidBinding(early.observations[0].marker.id)
    );
}

#[test]
fn ownership_trace_a_recipe_cannot_move_markers_into_another_instance() {
    let (unit, plan) = fixture();
    let mut other = unit.clone();
    other.header.instance = support::hash("another instance");
    let output = Plan::new(&other, plan.observations.clone());
    assert_eq!(
        check_transform(&unit, &plan, &other, &output, Some(&recipe(&plan, &output))).unwrap_err(),
        Error::ChangedUnit
    );
}

#[test]
fn ownership_trace_a_forged_recipe_cannot_retarget_a_logical_token() {
    let (unit, plan) = fixture();
    let mut output = plan.clone();
    let store = output.observations[1].clone();
    let read = output.observations[2].clone();
    output.observations[1] = Observation {
        point: read.point,
        marker: ObserveOwnership {
            id: store.marker.id,
            ..read.marker
        },
    };
    output.observations[2] = Observation {
        point: store.point,
        marker: ObserveOwnership {
            id: read.marker.id,
            ..store.marker
        },
    };
    check(&unit, &output).unwrap();
    // Even a recipe that repeats the producer's wrong mapping cannot authorize
    // changing which logical value id1 describes.
    let forged = recipe(&plan, &output);
    assert_eq!(
        check_transform(&unit, &plan, &unit, &output, Some(&forged)).unwrap_err(),
        Error::IdentityRewrite(MarkerId(1))
    );
}

#[test]
fn ownership_trace_same_address_number_cannot_hide_a_changed_owner() {
    let (unit, plan) = fixture();
    let mut after = unit.clone();
    let mut slot = after.slots[0].clone();
    slot.kind = SlotKind::Temporary;
    after.slots.push(slot);
    after.addresses[0].base = AddressBase::Slot(SlotId(1));
    let output = Plan::new(&after, plan.observations.clone());
    check(&after, &output).unwrap();
    assert_eq!(
        check_transform(&unit, &plan, &after, &output, Some(&recipe(&plan, &output))).unwrap_err(),
        Error::IdentityRewrite(MarkerId(1))
    );
}
