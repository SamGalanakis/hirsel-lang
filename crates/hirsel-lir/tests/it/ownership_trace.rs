//! The external trace construct is adversarial input, separate from LIR authority.
use crate::support::{self, ENTRY, Lir, ROOT, l};
use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::ownership_trace::*;
use hirsel_lir::stage::Rc;

fn fixture_with_table() -> (LirUnit<Rc>, Plan, hirsel_lir::layout_types::LayoutTable) {
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
    (unit, plan, table)
}

fn fixture() -> (LirUnit<Rc>, Plan) {
    let (unit, plan, _) = fixture_with_table();
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

#[test]
fn ownership_trace_elide_emits_a_recipe_even_without_a_rewrite() {
    let (unit, input, table) = fixture_with_table();
    let ordinary = hirsel_rc::elide::elide(unit.clone(), &table).unwrap();
    let (after, output, recipe) =
        hirsel_rc::elide::elide_observed(unit.clone(), &table, &input).unwrap();
    assert_eq!(
        hirsel_codec::to_text(&after),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(input.observations, output.observations);
    assert_eq!(recipe.relocations.len(), input.observations.len());
    check_transform(&unit, &input, &after, &output, Some(&recipe)).unwrap();
}

#[test]
fn ownership_trace_elide_uses_the_actual_editor_block_compaction() {
    let (mut unit, mut input, table) = fixture_with_table();
    unit.blocks.insert(
        0,
        Block {
            kind: BlockKind::Body,
            parameters: PoolRange::EMPTY,
            instructions: PoolRange::EMPTY,
            anchor: SourceAnchorId(0),
        },
    );
    unit.terminators.insert(0, Terminator::Unreachable);
    unit.entry.0 += 1;
    for edge in &mut unit.tables.edges {
        edge.target.0 += 1;
    }
    for observation in &mut input.observations {
        match &mut observation.point {
            Point::Instruction { block, .. } | Point::Return(block) => block.0 += 1,
            Point::Entry => (),
            other => panic!("unexpected fixture point: {other:?}"),
        }
    }
    input.unit = unit.digest();
    check(&unit, &input).unwrap();
    let (after, output, recipe) =
        hirsel_rc::elide::elide_observed(unit.clone(), &table, &input).unwrap();
    assert_eq!(after.entry, ENTRY);
    assert_eq!(output.observations.len(), input.observations.len());
    assert_ne!(input.observations, output.observations);
    check_transform(&unit, &input, &after, &output, Some(&recipe)).unwrap();
}

fn elision_fixture(
    observe_copy: bool,
) -> (
    LirUnit<Rc>,
    Plan,
    hirsel_lir::layout_types::LayoutTable,
    LirUnit<Rc>,
) {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::BLK)], Some(l::BLK));
    let value = t.owned(l::BLK);
    t.param(value);
    let slot = t.slot(l::BLK, SlotKind::Local, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let address = t.at(ENTRY, slot, &[], &[support::Undo::Release(value)]);
    let version = t.store(ENTRY, address, value, slot);
    let copied = t.owned(l::BLK);
    let cleanup = t.cleanup(&[support::Undo::Drop(slot)]);
    let failure = t.failure(cleanup);
    t.builder.push(
        ENTRY,
        Inst::Copy {
            result: copied,
            address,
            reaching: version,
            failure,
        },
    );
    let taken = t.read(ENTRY, address, version, ReadMode::Take, None);
    t.release(ENTRY, taken, &[copied]);
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(ENTRY, Some(copied));
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
    let ordinary = hirsel_rc::elide::elide(unit.clone(), &table).unwrap();
    let violations = hirsel_lir::validate::check(&ordinary, &inputs);
    assert!(
        violations.is_empty(),
        "ordinary elision must remain valid: {violations:?}"
    );
    assert!(
        ordinary.instructions.len() < unit.instructions.len(),
        "the fixture must exercise a real elision"
    );
    let site = unit.tables.source_anchors[0].site;
    let point = if observe_copy {
        Point::Instruction {
            block: ENTRY,
            index: unit.blocks[0]
                .instructions
                .get(&unit.instructions)
                .unwrap()
                .iter()
                .position(|instruction| matches!(instruction, Inst::Copy { .. }))
                .unwrap() as u32,
            moment: Moment::After,
        }
    } else {
        Point::Entry
    };
    let marker = if observe_copy {
        ObserveOwnership {
            id: MarkerId(0),
            site,
            kind: Kind::Create,
            token: Location::Value(copied),
            owner: Location::Value(copied),
        }
    } else {
        ObserveOwnership {
            id: MarkerId(0),
            site,
            kind: Kind::Move,
            token: Location::Parameter(0),
            owner: Location::Value(value),
        }
    };
    let plan = Plan::new(&unit, vec![Observation { point, marker }]);
    (unit, plan, table, ordinary)
}

#[test]
fn ownership_trace_elide_preserves_unaffected_identity_during_a_real_elision() {
    let (before, input, table, ordinary) = elision_fixture(false);
    let (after, output, recipe) =
        hirsel_rc::elide::elide_observed(before.clone(), &table, &input).unwrap();
    assert_eq!(
        hirsel_codec::to_text(&after),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(input.observations, output.observations);
    check_transform(&before, &input, &after, &output, Some(&recipe)).unwrap();
}

#[test]
fn ownership_trace_elide_refuses_to_silently_delete_an_observed_copy() {
    let (before, input, table, _) = elision_fixture(true);
    let error = hirsel_rc::elide::elide_observed(before, &table, &input).unwrap_err();
    assert!(error.contains("observed marker MarkerId(0)"), "{error}");
    assert!(error.contains("explicit semantic transport"), "{error}");
}

#[test]
fn ownership_trace_legalize_emits_a_recipe_for_unchanged_resource_flow() {
    let (unit, mut input, table) = fixture_with_table();
    let unit = unit.into_stage::<hirsel_lir::stage::Ready>();
    input.unit = unit.digest();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let ordinary = hirsel_rc::legalize::legalize_indexed(unit.clone(), &index).unwrap();
    let (after, output, recipe) =
        hirsel_rc::legalize::legalize_observed(unit.clone(), &index, &input).unwrap();
    assert_eq!(
        hirsel_codec::to_text(&after),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(input.observations, output.observations);
    check_transform(&unit, &input, &after, &output, Some(&recipe)).unwrap();
}

fn scalar_legalization_fixture(
    observe_call: bool,
) -> (
    LirUnit<hirsel_lir::stage::Ready>,
    Plan,
    hirsel_lir::layout_types::LayoutTable,
    LirUnit<hirsel_lir::stage::Ready>,
) {
    use hirsel_lir::stage::{Ready, Reuse};
    let mut t = Lir::<Reuse>::new(
        &[(Passing::Take, l::BLK), (Passing::Borrow, l::I64)],
        Some(l::BLK),
    );
    let resource = t.owned(l::BLK);
    t.param(resource);
    let scalar = t.plain(l::I64);
    t.param(scalar);
    let normal = t.builder.block(BlockKind::Body, &[]);
    let failure = t.releasing(&[resource]);
    let abi = t.abi(&[(Passing::Borrow, l::I64)], Some(l::I32));
    t.declare_operation(
        super::operation_abi::scalar_signature(
            "int.truncate",
            &[(
                hirsel_syntax::PassingMode::Borrow,
                hirsel_types::Primitive::I64,
            )],
            Some(hirsel_types::Primitive::I32),
        ),
        Default::default(),
        abi,
    );
    let scalar_result = t
        .call_to(
            ENTRY,
            Callee::Operation(OperationRef(0)),
            abi,
            &[(Passing::Borrow, l::I64)],
            Some(l::I32),
            &[ArgumentValue::Value(scalar)],
            (normal, &[]),
            failure,
        )
        .unwrap();
    t.builder.ret(normal, Some(resource));
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
        .into_raw()
        .into_stage::<Ready>();
    let ordinary = hirsel_rc::legalize::legalize(unit.clone(), &table).unwrap();
    let violations = hirsel_lir::validate::check(&ordinary, &inputs);
    assert!(
        violations.is_empty(),
        "legalized output must independently pass Ready: {violations:?}"
    );
    assert!(ordinary.instructions.iter().any(|inst| matches!(
        inst,
        Inst::Convert {
            mode: ConvertMode::Truncate,
            ..
        }
    )));
    assert!(
        !ordinary
            .terminators
            .iter()
            .any(|term| matches!(term, Terminator::Call { .. }))
    );
    let site = unit.tables.source_anchors[0].site;
    let mut observations = vec![Observation {
        point: Point::Entry,
        marker: ObserveOwnership {
            id: MarkerId(0),
            site,
            kind: Kind::Move,
            token: Location::Parameter(0),
            owner: Location::Value(resource),
        },
    }];
    if observe_call {
        observations.push(Observation {
            point: Point::CallReturned(ENTRY),
            marker: ObserveOwnership {
                id: MarkerId(1),
                site,
                kind: Kind::Move,
                token: Location::Return,
                owner: Location::Value(scalar_result),
            },
        });
    }
    observations.push(Observation {
        point: Point::Return(normal),
        marker: ObserveOwnership {
            id: MarkerId(2),
            site,
            kind: Kind::Move,
            token: Location::Value(resource),
            owner: Location::Return,
        },
    });
    let plan = Plan::new(&unit, observations);
    (unit, plan, table, ordinary)
}

#[test]
fn ownership_trace_legalize_preserves_markers_while_a_scalar_call_becomes_code() {
    let (before, input, table, ordinary) = scalar_legalization_fixture(false);
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (after, output, recipe) =
        hirsel_rc::legalize::legalize_observed(before.clone(), &index, &input).unwrap();
    assert_eq!(
        hirsel_codec::to_text(&after),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(input.observations, output.observations);
    check_transform(&before, &input, &after, &output, Some(&recipe)).unwrap();
}

#[test]
fn ownership_trace_legalize_refuses_to_lose_a_marker_on_an_expanded_call() {
    let (before, input, table, _) = scalar_legalization_fixture(true);
    let index = hirsel_lir::LayoutIndex::new(&table);
    let error = hirsel_rc::legalize::legalize_observed(before, &index, &input).unwrap_err();
    assert!(error.contains("InvalidPoint(MarkerId(1))"), "{error}");
}

fn placement_resource_fixture() -> (
    LirUnit<hirsel_lir::stage::Value>,
    Plan,
    hirsel_lir::layout_types::LayoutTable,
) {
    let mut t = Lir::<hirsel_lir::stage::Value>::new(&[], Some(l::RESBLK));
    let resource = t.new_block_of(ENTRY, l::RES, &[]);
    t.builder.ret(ENTRY, Some(resource));
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
    let index = unit.blocks[0]
        .instructions
        .get(&unit.instructions)
        .unwrap()
        .iter()
        .position(|instruction| matches!(instruction, Inst::Construct { .. }))
        .unwrap() as u32;
    let plan = Plan::new(
        &unit,
        vec![
            Observation {
                point: Point::Instruction {
                    block: ENTRY,
                    index,
                    moment: Moment::After,
                },
                marker: ObserveOwnership {
                    id: MarkerId(0),
                    site,
                    kind: Kind::Create,
                    token: Location::Value(resource),
                    owner: Location::Value(resource),
                },
            },
            Observation {
                point: Point::Return(ENTRY),
                marker: ObserveOwnership {
                    id: MarkerId(1),
                    site,
                    kind: Kind::Move,
                    token: Location::Value(resource),
                    owner: Location::Return,
                },
            },
        ],
    );
    (unit, plan, table)
}

#[test]
fn ownership_trace_placement_keeps_resource_identity_while_binding_its_destination() {
    let (before, input, table) = placement_resource_fixture();
    assert!(
        before
            .tables
            .constructs
            .iter()
            .all(|site| site.placement.is_none())
    );
    let options = hirsel_rc::placement::Options {
        regions: true,
        noalloc: false,
    };
    let ordinary = hirsel_rc::placement::place(before.clone(), &table, &options).unwrap();
    let (after, output, recipe) =
        hirsel_rc::placement::place_observed(before.clone(), &table, &options, &input).unwrap();
    assert!(
        after
            .tables
            .constructs
            .iter()
            .all(|site| site.placement.is_some())
    );
    assert_eq!(
        hirsel_codec::to_text(&after),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(input.observations, output.observations);
    check_transform(&before, &input, &after, &output, Some(&recipe)).unwrap();
}

#[test]
fn ownership_trace_placement_checks_incoming_sites_instead_of_rebuilding_markers() {
    let (before, mut input, table) = placement_resource_fixture();
    input.observations[0].marker.site.start += 1;
    let options = hirsel_rc::placement::Options {
        regions: true,
        noalloc: false,
    };
    let error = hirsel_rc::placement::place_observed(before, &table, &options, &input).unwrap_err();
    assert!(error.contains("InvalidSite(MarkerId(0))"), "{error}");
}

#[test]
fn ownership_trace_rc_insertion_reports_all_three_resource_boundaries() {
    let (value, input, table) = placement_resource_fixture();
    let placement = hirsel_rc::placement::Options {
        regions: true,
        noalloc: false,
    };
    let (placed, input, _) =
        hirsel_rc::placement::place_observed(value, &table, &placement, &input).unwrap();
    let proofs = hirsel_rc::insert::Proofs::default();
    let ordinary = hirsel_rc::insert::insert(placed.clone(), &table, &proofs).unwrap();
    let (after, output, recipes) =
        hirsel_rc::insert::insert_observed(placed, &table, &proofs, &input).unwrap();
    assert_eq!(
        hirsel_codec::to_text(&after),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(input.observations, output.observations);
    assert_eq!(recipes.len(), 3);
    assert_eq!(recipes[0].before, input.unit);
    assert_eq!(recipes[2].after, output.unit);
    assert!(
        recipes
            .windows(2)
            .all(|pair| pair[0].after == pair[1].before)
    );
    assert!(
        recipes
            .iter()
            .all(|recipe| recipe.relocations.len() == input.observations.len())
    );
}

fn ssa_copy_fixture(
    observe_copy: bool,
) -> (
    LirUnit<hirsel_lir::stage::Placed>,
    Plan,
    hirsel_lir::layout_types::LayoutTable,
    LirUnit<Rc>,
) {
    use hirsel_lir::stage::Placed;
    let mut t = Lir::<Placed>::new(&[(Passing::Take, l::BLK)], Some(l::PAIR));
    let original = t.owned(l::BLK);
    t.param(original);
    let copied = t.owned(l::BLK);
    t.builder.push(
        ENTRY,
        Inst::CopyValue {
            result: copied,
            value: original,
        },
    );
    let symbol = t.symbol(SymbolKind::Type);
    let site = t.construct_site(
        ConstructorKind::Record(symbol),
        l::PAIR,
        &[(0, original), (1, copied)],
        t.heap(),
    );
    let aggregate = t.owned(l::PAIR);
    // Construct takes both fields and disposes them itself on failure.
    let cleanup = t.cleanup(&[]);
    let failure = t.failure(cleanup);
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: aggregate,
            construct: site,
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(aggregate));
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
    let ordinary = hirsel_rc::insert::insert(unit.clone(), &table, &Default::default()).unwrap();
    let violations = hirsel_lir::validate::check(&ordinary, &inputs);
    assert!(
        violations.is_empty(),
        "ordinary RC output must independently validate: {violations:?}"
    );
    assert!(
        ordinary
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Inst::Dup { .. }))
    );
    assert!(
        !ordinary
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Inst::CopyValue { .. }))
    );
    let source = unit.tables.source_anchors[0].site;
    let mut observations = vec![
        Observation {
            point: Point::Entry,
            marker: ObserveOwnership {
                id: MarkerId(0),
                site: source,
                kind: Kind::Move,
                token: Location::Parameter(0),
                owner: Location::Value(original),
            },
        },
        Observation {
            point: Point::Instruction {
                block: ENTRY,
                index: 1,
                moment: Moment::After,
            },
            marker: ObserveOwnership {
                id: MarkerId(1),
                site: source,
                kind: Kind::Create,
                token: Location::Value(aggregate),
                owner: Location::Value(aggregate),
            },
        },
        Observation {
            point: Point::Return(ENTRY),
            marker: ObserveOwnership {
                id: MarkerId(2),
                site: source,
                kind: Kind::Move,
                token: Location::Value(aggregate),
                owner: Location::Return,
            },
        },
    ];
    if observe_copy {
        observations.insert(
            1,
            Observation {
                point: Point::Instruction {
                    block: ENTRY,
                    index: 0,
                    moment: Moment::After,
                },
                marker: ObserveOwnership {
                    id: MarkerId(3),
                    site: source,
                    kind: Kind::Create,
                    token: Location::Value(copied),
                    owner: Location::Value(copied),
                },
            },
        );
    }
    let plan = Plan::new(&unit, observations);
    (unit, plan, table, ordinary)
}

#[test]
fn ownership_trace_rc_insertion_tracks_shifted_markers_after_a_real_ssa_copy() {
    let (before, input, table, ordinary) = ssa_copy_fixture(false);
    let (after, output, recipes) =
        hirsel_rc::insert::insert_observed(before.clone(), &table, &Default::default(), &input)
            .unwrap();
    assert_eq!(
        hirsel_codec::to_text(&after),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(recipes.len(), 3);
    assert_ne!(input.observations[1].point, output.observations[1].point);
    assert_eq!(input.observations[1].marker, output.observations[1].marker);
    let composed = recipe(&input, &output);
    check_transform(&before, &input, &after, &output, Some(&composed)).unwrap();
}

#[test]
fn ownership_trace_rc_insertion_refuses_to_turn_an_observed_create_into_a_move() {
    let (before, input, table, _) = ssa_copy_fixture(true);
    let error = hirsel_rc::insert::insert_observed(before, &table, &Default::default(), &input)
        .unwrap_err();
    assert!(error.contains("observed SSA copy MarkerId(3)"), "{error}");
    assert!(error.contains("explicit semantic transport"), "{error}");
}

#[test]
fn ownership_trace_reuse_checks_each_subpass_and_the_disabled_boundary() {
    let (before, input, table) = fixture_with_table();
    let index = hirsel_lir::LayoutIndex::new(&table);
    for enabled in [true, false] {
        let options = hirsel_rc::reuse::Options { reuse: enabled };
        let ordinary = hirsel_rc::reuse::reuse_indexed(before.clone(), &options, &index).unwrap();
        let (after, output, recipes) =
            hirsel_rc::reuse::reuse_observed(before.clone(), &options, &index, &input).unwrap();
        assert_eq!(
            hirsel_codec::to_text(&after),
            hirsel_codec::to_text(&ordinary)
        );
        assert_eq!(recipes.len(), if enabled { 3 } else { 1 });
        assert_eq!(recipes.first().unwrap().before, input.unit);
        assert_eq!(recipes.last().unwrap().after, output.unit);
        for adjacent in recipes.windows(2) {
            assert_eq!(adjacent[0].after, adjacent[1].before);
        }
        assert!(
            recipes
                .iter()
                .all(|recipe| recipe.relocations.len() == input.observations.len())
        );
        assert_eq!(input.observations, output.observations);
        check_transform(
            &before,
            &input,
            &after,
            &output,
            Some(&recipe(&input, &output)),
        )
        .unwrap();
    }
}

#[test]
fn ownership_trace_reuse_keeps_distinct_lifetimes_when_storage_is_recycled() {
    let mut t = Lir::<Rc>::new(&[], Some(l::BLK));
    let donor = t.new_block(ENTRY, &[]);
    t.release(ENTRY, donor, &[]);
    let recipient = t.new_block(ENTRY, &[]);
    t.builder.ret(ENTRY, Some(recipient));
    let operations = t.operations.clone();
    let (before, table, manifest) = t.finish();
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let before = hirsel_lir::validate::validate(before, &inputs)
        .unwrap()
        .into_raw();
    let source = before.tables.source_anchors[0].site;
    let mut observations = vec![];
    for (index, inst) in before.blocks[0]
        .instructions
        .get(&before.instructions)
        .unwrap()
        .iter()
        .enumerate()
    {
        let (kind, value, moment) = match *inst {
            Inst::Construct { result, .. } => (Kind::Create, result, Moment::After),
            Inst::Release { value, .. } => (Kind::Drop, value, Moment::Before),
            _ => continue,
        };
        observations.push(Observation {
            point: Point::Instruction {
                block: ENTRY,
                index: index as u32,
                moment,
            },
            marker: ObserveOwnership {
                id: MarkerId(observations.len() as u32),
                site: source,
                kind,
                token: Location::Value(value),
                owner: Location::Value(value),
            },
        });
    }
    observations.push(Observation {
        point: Point::Return(ENTRY),
        marker: ObserveOwnership {
            id: MarkerId(3),
            site: source,
            kind: Kind::Move,
            token: Location::Value(recipient),
            owner: Location::Return,
        },
    });
    let input = Plan::new(&before, observations);
    let index = hirsel_lir::LayoutIndex::new(&table);
    let options = hirsel_rc::reuse::Options::default();
    let ordinary = hirsel_rc::reuse::reuse_indexed(before.clone(), &options, &index).unwrap();
    let violations = hirsel_lir::validate::check(&ordinary, &inputs);
    assert!(
        violations.is_empty(),
        "ordinary reuse must validate: {violations:?}"
    );
    assert!(
        ordinary
            .instructions
            .iter()
            .any(|inst| matches!(inst, Inst::Reset { value, .. } if *value == donor))
    );
    assert!(
        ordinary
            .instructions
            .iter()
            .any(|inst| matches!(inst, Inst::Reuse { result, .. } if *result == recipient))
    );
    let (after, output, recipes) =
        hirsel_rc::reuse::reuse_observed(before.clone(), &options, &index, &input).unwrap();
    assert_eq!(
        hirsel_codec::to_text(&after),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(recipes.len(), 3);
    assert_eq!(input.observations, output.observations);
    assert_ne!(
        output.observations[0].marker.token,
        output.observations[2].marker.token
    );
    check_transform(
        &before,
        &input,
        &after,
        &output,
        Some(&recipe(&input, &output)),
    )
    .unwrap();
}

#[test]
fn ownership_trace_reuse_refuses_a_stale_input_before_rewriting() {
    let (before, mut input, table) = fixture_with_table();
    input.observations[0].marker.site.start += 1;
    let index = hirsel_lir::LayoutIndex::new(&table);
    let error =
        hirsel_rc::reuse::reuse_observed(before, &Default::default(), &index, &input).unwrap_err();
    assert!(error.contains("InvalidSite(MarkerId(0))"), "{error}");
}

fn drop_transport_fixture() -> (LirUnit<Rc>, Plan, LirUnit<hirsel_lir::stage::Ready>, Plan) {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::BLK)], None);
    let value = t.owned(l::BLK);
    t.param(value);
    let slot = t.slot(l::BLK, SlotKind::Local, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let address = t.at(ENTRY, slot, &[], &[support::Undo::Release(value)]);
    t.store(ENTRY, address, value, slot);
    let cleanup = t.cleanup(&[]);
    let failure = t.failure(cleanup);
    let anchor = t.cleanup_anchor();
    t.builder.push(
        ENTRY,
        Inst::Drop {
            address,
            if_live: false,
            anchor,
            failure,
        },
    );
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(ENTRY, None);
    let operations = t.operations.clone();
    let (before, table, manifest) = t.finish();
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let before = hirsel_lir::validate::validate(before, &inputs)
        .unwrap()
        .into_raw();
    let after = hirsel_rc::cleanup::lower(before.clone(), &table, &Default::default()).unwrap();
    let violations = hirsel_lir::validate::check(&after, &inputs);
    assert!(
        violations.is_empty(),
        "actual cleanup output must validate: {violations:?}"
    );
    let old_index = before.blocks[0]
        .instructions
        .get(&before.instructions)
        .unwrap()
        .iter()
        .position(|inst| matches!(inst, Inst::Drop { .. }))
        .unwrap() as u32;
    let new_index = after.blocks[0].instructions.get(&after.instructions).unwrap().windows(2)
        .position(|pair| matches!(pair, [Inst::Read { address: a, mode: ReadMode::Take, .. }, Inst::Release { .. }] if *a == address))
        .expect("actual cleanup emits the whole read/release pair") as u32 + 1;
    let marker = ObserveOwnership {
        id: MarkerId(0),
        site: before.tables.cleanup_anchors[anchor.index()].site,
        kind: Kind::Drop,
        token: Location::Address(address),
        owner: Location::Address(address),
    };
    let input = Plan::new(
        &before,
        vec![Observation {
            point: Point::Instruction {
                block: ENTRY,
                index: old_index,
                moment: Moment::Before,
            },
            marker: marker.clone(),
        }],
    );
    let output = Plan::new(
        &after,
        vec![Observation {
            point: Point::ReleaseDrop {
                block: ENTRY,
                index: new_index,
            },
            marker,
        }],
    );
    (before, input, after, output)
}

#[test]
fn ownership_trace_drop_transport_preserves_the_semantic_address_and_action() {
    let (before, input, after, output) = drop_transport_fixture();
    check(&before, &input).unwrap();
    check(&after, &output).unwrap();
    assert_eq!(input.observations[0].marker, output.observations[0].marker);
    check_transform(
        &before,
        &input,
        &after,
        &output,
        Some(&recipe(&input, &output)),
    )
    .unwrap();
    let decoded: Plan = hirsel_codec::from_text(&hirsel_codec::to_text(&output)).unwrap();
    assert_eq!(decoded, output);
}

#[test]
fn ownership_trace_drop_transport_refuses_a_different_released_value() {
    let (_, _, mut after, mut output) = drop_transport_fixture();
    let Point::ReleaseDrop { block, index } = output.observations[0].point else {
        unreachable!()
    };
    let at = after.blocks[block.index()].instructions.start as usize + index as usize;
    let Inst::Release { value, .. } = &mut after.instructions[at] else {
        unreachable!()
    };
    *value = ValueId(0);
    output.unit = after.digest();
    assert_eq!(
        check(&after, &output).unwrap_err(),
        Error::InvalidBinding(MarkerId(0))
    );
}

#[test]
fn ownership_trace_drop_transport_refuses_a_masked_release() {
    let (_, _, mut after, mut output) = drop_transport_fixture();
    let Point::ReleaseDrop { block, index } = output.observations[0].point else {
        unreachable!()
    };
    let at = after.blocks[block.index()].instructions.start as usize + index as usize;
    let Inst::Release { live, .. } = &mut after.instructions[at] else {
        unreachable!()
    };
    *live = MaskRef::Static(MaskId(0));
    output.unit = after.digest();
    assert_eq!(
        check(&after, &output).unwrap_err(),
        Error::InvalidBinding(MarkerId(0))
    );
}

#[test]
fn ownership_trace_drop_transport_refuses_a_different_cleanup_with_the_same_source() {
    let (before, input, mut after, mut output) = drop_transport_fixture();
    let Point::ReleaseDrop { block, index } = output.observations[0].point else {
        unreachable!()
    };
    let at = after.blocks[block.index()].instructions.start as usize + index as usize;
    let Inst::Release { anchor, .. } = after.instructions[at] else {
        unreachable!()
    };
    after.tables.cleanup_anchors[anchor.index()].action += 1;
    output.unit = after.digest();
    check(&after, &output).unwrap();
    assert_eq!(
        check_transform(
            &before,
            &input,
            &after,
            &output,
            Some(&recipe(&input, &output))
        )
        .unwrap_err(),
        Error::InvalidTransport(MarkerId(0)),
    );
}

#[test]
fn ownership_trace_drop_transport_refuses_to_certify_a_conditional_drop_as_unconditional() {
    let (mut before, mut input, _, _) = drop_transport_fixture();
    let Point::Instruction { block, index, .. } = input.observations[0].point else {
        unreachable!()
    };
    let at = before.blocks[block.index()].instructions.start as usize + index as usize;
    let Inst::Drop { if_live, .. } = &mut before.instructions[at] else {
        unreachable!()
    };
    *if_live = true;
    input.unit = before.digest();
    assert_eq!(
        check(&before, &input).unwrap_err(),
        Error::InvalidBinding(MarkerId(0))
    );
}
