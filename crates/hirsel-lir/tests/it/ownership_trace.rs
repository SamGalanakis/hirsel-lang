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
        cleanups: output
            .cleanups
            .iter()
            .enumerate()
            .map(|(index, row)| CleanupRelocation {
                before: input.cleanups.get(index).cloned(),
                after: row.clone(),
            })
            .collect(),
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
    let (before, input, after, output, _) = drop_transport_fixture_with_table();
    (before, input, after, output)
}

fn drop_transport_fixture_with_table() -> (
    LirUnit<Rc>,
    Plan,
    LirUnit<hirsel_lir::stage::Ready>,
    Plan,
    hirsel_lir::layout_types::LayoutTable,
) {
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
    (before, input, after, output, table)
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

#[test]
fn ownership_trace_cleanup_lowering_reports_the_whole_release_it_actually_emits() {
    let (before, input, after, expected, table) = drop_transport_fixture_with_table();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (observed, output, recipe) =
        hirsel_rc::cleanup::lower_observed(before.clone(), &Default::default(), &index, &input)
            .unwrap();
    assert_eq!(
        hirsel_codec::to_text(&observed),
        hirsel_codec::to_text(&after)
    );
    assert_eq!(output, expected);
    assert_eq!(recipe.relocations.len(), 1);
    assert_eq!(recipe.relocations[0].point, expected.observations[0].point);
    check_transform(&before, &input, &observed, &output, Some(&recipe)).unwrap();
}

#[test]
fn ownership_trace_cleanup_lowering_refuses_a_stale_or_forged_input() {
    let (before, input, _, _, table) = drop_transport_fixture_with_table();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let mut forged = input.clone();
    let mut extra = forged.observations[0].clone();
    extra.marker.id = MarkerId(9);
    forged.observations.push(extra);
    let error =
        hirsel_rc::cleanup::lower_observed(before.clone(), &Default::default(), &index, &forged)
            .unwrap_err();
    assert!(
        error.contains("DuplicateObservation") || error.contains("Forged"),
        "{error}"
    );
    let mut stale = input;
    stale.unit = support::hash("another unit");
    let error = hirsel_rc::cleanup::lower_observed(before, &Default::default(), &index, &stale)
        .unwrap_err();
    assert!(error.contains("StalePlan"), "{error}");
}

#[test]
fn ownership_trace_ready_checks_both_passes_after_a_semantic_drop_moves() {
    let (before, input, _, _, table) = drop_transport_fixture_with_table();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let ordinary =
        hirsel_rc::ready::ready_indexed(before.clone(), &Default::default(), &index).unwrap();
    let (observed, output, recipes) =
        hirsel_rc::ready::ready_observed(before.clone(), &Default::default(), &index, &input)
            .unwrap();
    assert_eq!(
        hirsel_codec::to_text(&observed),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(recipes.len(), 2);
    assert_eq!(recipes[0].before, input.unit);
    assert_eq!(recipes[0].after, recipes[1].before);
    assert_eq!(recipes[1].after, output.unit);
    assert!(recipes.iter().all(|recipe| recipe.relocations.len() == 1));
    assert_eq!(input.observations[0].marker, output.observations[0].marker);
    assert!(matches!(
        output.observations[0].point,
        Point::ReleaseDrop { .. }
    ));
    check(&observed, &output).unwrap();
    // A dropped or forged marker cannot be introduced at the composed boundary.
    let mut missing = output.clone();
    missing.observations.clear();
    assert_eq!(
        check_transform(
            &before,
            &input,
            &observed,
            &missing,
            Some(&recipe(&input, &missing))
        )
        .unwrap_err(),
        Error::MissingMarker(MarkerId(0))
    );
    let mut forged = output;
    forged.observations[0].marker.id = MarkerId(9);
    assert!(
        check_transform(
            &before,
            &input,
            &observed,
            &forged,
            Some(&recipe(&input, &forged))
        )
        .is_err()
    );
}

#[test]
fn ownership_trace_pipeline_preserves_resource_identity_and_every_pass_recipe() {
    let (before, input, table) = placement_resource_fixture();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let placement = hirsel_rc::placement::Options {
        regions: true,
        noalloc: false,
    };
    let proofs = hirsel_rc::insert::Proofs::default();
    let finalizers = hirsel_rc::cleanup::Finalizers::default();
    for elide in [false, true] {
        for reuse in [false, true] {
            let reuse = hirsel_rc::reuse::Options { reuse };
            let options = hirsel_rc::observed::Options {
                placement: &placement,
                proofs: &proofs,
                elide,
                reuse: &reuse,
                finalizers: &finalizers,
            };
            let ordinary = hirsel_rc::placement::place(before.clone(), &table, &placement).unwrap();
            let ordinary = hirsel_rc::insert::insert(ordinary, &table, &proofs).unwrap();
            let ordinary = if elide {
                hirsel_rc::elide::elide(ordinary, &table).unwrap()
            } else {
                ordinary
            };
            let ordinary = hirsel_rc::reuse::reuse_indexed(ordinary, &reuse, &index).unwrap();
            let ordinary = hirsel_rc::ready::ready_indexed(ordinary, &finalizers, &index).unwrap();
            let observed =
                hirsel_rc::observed::lower(before.clone(), &input, &index, &options).unwrap();
            assert_eq!(
                hirsel_codec::to_text(&observed.unit),
                hirsel_codec::to_text(&ordinary)
            );
            assert_eq!(input.observations, observed.plan.observations);
            assert_eq!(observed.recipes.len(), if reuse.reuse { 10 } else { 8 });
            assert_eq!(observed.recipes[0].before, input.unit);
            assert_eq!(observed.recipes.last().unwrap().after, observed.plan.unit);
            assert!(
                observed
                    .recipes
                    .windows(2)
                    .all(|pair| pair[0].after == pair[1].before)
            );
            for recipe in &observed.recipes {
                assert_eq!(recipe.relocations.len(), input.observations.len());
                assert!(
                    recipe
                        .relocations
                        .iter()
                        .zip(&input.observations)
                        .all(|(relocation, observation)| relocation.id == observation.marker.id)
                );
            }
            check(&observed.unit, &observed.plan).unwrap();
        }
    }
}

#[test]
fn ownership_trace_pipeline_refuses_forged_attribution_at_the_first_boundary() {
    let (before, mut input, table) = placement_resource_fixture();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let placement = hirsel_rc::placement::Options::default();
    let proofs = hirsel_rc::insert::Proofs::default();
    let reuse = hirsel_rc::reuse::Options::default();
    let finalizers = hirsel_rc::cleanup::Finalizers::default();
    input.observations[0].marker.site.start += 1;
    let result = hirsel_rc::observed::lower(
        before,
        &input,
        &index,
        &hirsel_rc::observed::Options {
            placement: &placement,
            proofs: &proofs,
            elide: true,
            reuse: &reuse,
            finalizers: &finalizers,
        },
    );
    assert!(matches!(result, Err(ref error) if error.contains("InvalidSite(MarkerId(0))")));
}

fn finalizer_transport_fixture() -> (
    LirUnit<Rc>,
    Plan,
    hirsel_lir::layout_types::LayoutTable,
    hirsel_rc::cleanup::Finalizers,
) {
    let mut t = Lir::<Rc>::new(&[], None);
    let finalizer_abi = t.abi(&[(Passing::Borrow, l::RES)], None);
    let slot = t.slot(l::RES, SlotKind::Local, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let address = t.at(ENTRY, slot, &[], &[]);
    let scalar = t.int(ENTRY);
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let construct = t.construct_site(
        ConstructorKind::Record(symbol),
        l::RES,
        &[(0, scalar)],
        placement,
    );
    let resource = t.owned(l::RES);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: resource,
            construct,
            failure,
        },
    );
    t.store(ENTRY, address, resource, slot);
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
    let index = before.blocks[0]
        .instructions
        .get(&before.instructions)
        .unwrap()
        .iter()
        .position(|instruction| matches!(instruction, Inst::Drop { .. }))
        .unwrap() as u32;
    let plan = Plan::new(
        &before,
        vec![Observation {
            point: Point::Instruction {
                block: ENTRY,
                index,
                moment: Moment::Before,
            },
            marker: ObserveOwnership {
                id: MarkerId(0),
                site: before.tables.cleanup_anchors[anchor.index()].site,
                kind: Kind::Drop,
                token: Location::Address(address),
                owner: Location::Address(address),
            },
        }],
    );
    let finalizers = hirsel_rc::cleanup::Finalizers(std::collections::BTreeMap::from([(
        l::RES,
        hirsel_rc::cleanup::Finalizer {
            origin: UnitOrigin::Source,
            definition: support::path("Res"),
            instance: support::hash("Res.drop"),
            abi: finalizer_abi,
        },
    )]));
    let ordinary = hirsel_rc::cleanup::lower(before.clone(), &table, &finalizers).unwrap();
    let violations = hirsel_lir::validate::check(&ordinary, &inputs);
    assert!(
        violations.is_empty(),
        "ordinary finalizer output: {violations:?}"
    );
    (before, plan, table, finalizers)
}

#[test]
fn ownership_trace_finalizer_has_one_identity_on_all_exclusive_completion_paths() {
    let (before, input, table, finalizers) = finalizer_transport_fixture();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let ordinary = hirsel_rc::cleanup::lower_indexed(before.clone(), &finalizers, &index).unwrap();
    let (after, output, recipe) =
        hirsel_rc::cleanup::lower_observed(before.clone(), &finalizers, &index, &input).unwrap();
    assert_eq!(
        hirsel_codec::to_text(&after),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(input.observations[0].marker, output.observations[0].marker);
    assert_eq!(output.observations.len(), 1);
    assert!(matches!(
        output.observations[0].point,
        Point::FinalizerDrop { .. }
    ));
    let checked = check_transform(&before, &input, &after, &output, Some(&recipe)).unwrap();
    let commits = checked.commits(&after, MarkerId(0)).unwrap();
    assert_eq!(commits.len(), 3, "normal, failure, and rejected frame");
    for commit in commits {
        let Point::Instruction {
            block,
            index,
            moment: Moment::After,
        } = commit
        else {
            panic!("{commit:?}")
        };
        assert!(matches!(
            after.blocks[block.index()]
                .instructions
                .get(&after.instructions)
                .unwrap()[index as usize],
            Inst::FinalizeEnd { .. }
        ));
    }
    let decoded: Plan = hirsel_codec::from_text(&hirsel_codec::to_text(&output)).unwrap();
    assert_eq!(decoded, output);
}

#[test]
fn ownership_trace_finalizer_refuses_a_missing_failure_completion() {
    let (before, input, table, finalizers) = finalizer_transport_fixture();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (mut after, mut output, _) =
        hirsel_rc::cleanup::lower_observed(before, &finalizers, &index, &input).unwrap();
    let Point::FinalizerDrop { block } = output.observations[0].point else {
        unreachable!()
    };
    let Terminator::Call { failure, .. } = after.terminators[block.index()] else {
        unreachable!()
    };
    let edge = after.tables.failures[failure.index()].cleanup;
    let target = after.tables.edges[edge.index()].target;
    let span = after.blocks[target.index()].instructions;
    let index = span
        .get(&after.instructions)
        .unwrap()
        .iter()
        .position(|instruction| matches!(instruction, Inst::FinalizeEnd { .. }))
        .unwrap();
    after.instructions[span.start as usize + index] = Inst::FrameLeave;
    output.unit = after.digest();
    assert!(check(&after, &output).is_err());
}

#[test]
fn ownership_trace_finalizer_refuses_a_rejection_with_the_wrong_cleanup_action() {
    let (before, input, table, finalizers) = finalizer_transport_fixture();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (mut after, mut output, _) =
        hirsel_rc::cleanup::lower_observed(before, &finalizers, &index, &input).unwrap();
    let Point::FinalizerDrop { block } = output.observations[0].point else {
        unreachable!()
    };
    let failure = after.blocks[block.index()]
        .instructions
        .get(&after.instructions)
        .unwrap()
        .iter()
        .find_map(|instruction| match instruction {
            Inst::FrameEnter { failure } => Some(*failure),
            _ => None,
        })
        .unwrap();
    let target = after.tables.edges[after.tables.failures[failure.index()].cleanup.index()].target;
    let span = after.blocks[target.index()].instructions;
    let index = span
        .get(&after.instructions)
        .unwrap()
        .iter()
        .position(|instruction| matches!(instruction, Inst::FinalizeEnd { .. }))
        .unwrap();
    let mut wrong = after.tables.cleanup_anchors[0].clone();
    wrong.action += 1;
    let anchor = CleanupAnchorId(after.tables.cleanup_anchors.len() as u32);
    after.tables.cleanup_anchors.push(wrong);
    if let Inst::FinalizeEnd { anchor: at, .. } =
        &mut after.instructions[span.start as usize + index]
    {
        *at = anchor;
    }
    output.unit = after.digest();
    assert!(check(&after, &output).is_err());
}

#[test]
fn ownership_trace_finalizer_cannot_replace_a_semantic_drop_with_only_its_normal_exit() {
    let (before, input, table, finalizers) = finalizer_transport_fixture();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (after, mut output, _) =
        hirsel_rc::cleanup::lower_observed(before.clone(), &finalizers, &index, &input).unwrap();
    let normal = check(&after, &output)
        .unwrap()
        .commits(&after, MarkerId(0))
        .unwrap()[0];
    output.observations[0].point = normal;
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
        Error::InvalidTransport(MarkerId(0))
    );
}

#[test]
fn ownership_trace_finalizer_commit_api_refuses_a_retargeted_body() {
    let (before, input, table, finalizers) = finalizer_transport_fixture();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (after, output, _) =
        hirsel_rc::cleanup::lower_observed(before, &finalizers, &index, &input).unwrap();
    let checked = check(&after, &output).unwrap();
    let mut changed = after.clone();
    changed.header.implementation_revision = support::hash("other source");
    assert_eq!(
        checked.commits(&changed, MarkerId(0)).unwrap_err(),
        Error::StalePlan
    );
    assert_eq!(
        checked.commits(&after, MarkerId(9)).unwrap_err(),
        Error::MissingMarker(MarkerId(9))
    );
}

fn native_semantic_plan(unit: &LirUnit<Rc>) -> Plan {
    let mut observations = vec![];
    for (block, info) in unit.blocks.iter().enumerate() {
        for (index, instruction) in info
            .instructions
            .get(&unit.instructions)
            .unwrap()
            .iter()
            .enumerate()
        {
            let (kind, token, owner, site, moment) = match *instruction {
                Inst::Construct {
                    result, construct, ..
                } => (
                    Kind::Create,
                    Location::Value(result),
                    Location::Value(result),
                    unit.tables.source_anchors
                        [unit.tables.constructs[construct.index()].anchor.index()]
                    .site,
                    Moment::After,
                ),
                Inst::Store { address, value, .. }
                    if unit.values[value.index()].holding == Holding::Owned =>
                {
                    (
                        Kind::Move,
                        Location::Value(value),
                        Location::Address(address),
                        unit.tables.source_anchors[info.anchor.index()].site,
                        Moment::After,
                    )
                }
                Inst::Drop {
                    address,
                    anchor,
                    if_live: false,
                    ..
                } => (
                    Kind::Drop,
                    Location::Address(address),
                    Location::Address(address),
                    unit.tables.cleanup_anchors[anchor.index()].site,
                    Moment::Before,
                ),
                _ => continue,
            };
            observations.push(Observation {
                point: Point::Instruction {
                    block: BlockId(block as u32),
                    index: index as u32,
                    moment,
                },
                marker: ObserveOwnership {
                    id: MarkerId(100 + observations.len() as u32),
                    site,
                    kind,
                    token,
                    owner,
                },
            });
        }
    }
    Plan::new(unit, observations)
}

fn checked_native_fixture<S: hirsel_lir::stage::Stage>(
    unit: LirUnit<S>,
    table: &hirsel_lir::layout_types::LayoutTable,
) -> hirsel_lir::stage::ValidatedUnit<S> {
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let operations = support::registry();
    let manifest = support::manifest(hirsel_core_contract::Disposition::RuntimePrimitive(
        "hirsel_rt_op".into(),
    ));
    hirsel_lir::validate::validate(
        unit,
        &hirsel_lir::validate::Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &operations,
            target: &target,
            layouts: table,
            manifest: &manifest,
        },
    )
    .unwrap()
}

#[test]
fn ownership_trace_relocated_native_plan_keeps_original_ids_across_finalizer_exits() {
    let (before, _, table, finalizers) = finalizer_transport_fixture();
    let input = native_semantic_plan(&before);
    assert_eq!(input.observations.len(), 3);
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (after, output, _) =
        hirsel_rc::cleanup::lower_observed(before, &finalizers, &index, &input).unwrap();
    let after = checked_native_fixture(after, &table);
    let plan = native::LeafPlan::from_relocated(&after, &table, &output).unwrap();
    assert_eq!(plan.plan(), &output);
    let mut drop_ids = vec![];
    for (block, code) in after.unit().blocks.iter().enumerate() {
        for i in 0..code.instructions.len {
            for marker in plan.at(BlockId(block as u32), i, Moment::After) {
                if marker.kind == 2 {
                    drop_ids.push(marker.id);
                }
            }
        }
    }
    assert_eq!(drop_ids, vec![MarkerId(102); 3]);
}

#[test]
fn ownership_trace_relocated_native_plan_refuses_missing_and_duplicate_coverage() {
    let (before, _, table, finalizers) = finalizer_transport_fixture();
    let input = native_semantic_plan(&before);
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (after, output, _) =
        hirsel_rc::cleanup::lower_observed(before, &finalizers, &index, &input).unwrap();
    let after = checked_native_fixture(after, &table);
    let mut missing = output.clone();
    missing.observations.remove(0);
    check(after.unit(), &missing).unwrap();
    let error = native::LeafPlan::from_relocated(&after, &table, &missing).unwrap_err();
    assert!(error.contains("omits a Ready transition"), "{error}");
    let mut erased_live = output.clone();
    let row = &mut erased_live.observations[0];
    row.point = Point::Unreachable {
        origin: hirsel_syntax::ContentHash::of_bytes(b"forged history"),
        marker: row.marker.id,
        site: row.marker.site,
    };
    check(after.unit(), &erased_live).unwrap();
    let error = native::LeafPlan::from_relocated(&after, &table, &erased_live).unwrap_err();
    assert!(error.contains("omits a Ready transition"), "{error}");
    let mut duplicate = output.clone();
    let mut extra = output.observations[2].clone();
    extra.marker.id = MarkerId(999);
    extra.point = check(after.unit(), &output)
        .unwrap()
        .commits(after.unit(), MarkerId(102))
        .unwrap()[0];
    duplicate.observations.push(extra);
    check(after.unit(), &duplicate).unwrap();
    let error = native::LeafPlan::from_relocated(&after, &table, &duplicate).unwrap_err();
    assert!(error.contains("twice"), "{error}");
}

#[test]
fn ownership_trace_relocated_native_release_does_not_invent_a_temporary_move() {
    let mut t = Lir::<Rc>::new(&[], None);
    let slot = t.slot(l::BLK, SlotKind::Local, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let address = t.at(ENTRY, slot, &[], &[]);
    let value = t.new_block(ENTRY, &[]);
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
    let (before, table, _) = t.finish();
    let input = native_semantic_plan(&before);
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (after, output, _) =
        hirsel_rc::cleanup::lower_observed(before, &Default::default(), &index, &input).unwrap();
    let after = checked_native_fixture(after, &table);
    let plan = native::LeafPlan::from_relocated(&after, &table, &output).unwrap();
    let observation = output
        .observations
        .iter()
        .find(|o| matches!(o.point, Point::ReleaseDrop { .. }))
        .unwrap();
    let Point::ReleaseDrop { block, index } = observation.point else {
        unreachable!()
    };
    assert_eq!(plan.at(block, index - 1, Moment::After).count(), 0);
    let markers: Vec<_> = plan.at(block, index, Moment::Before).collect();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].id, observation.marker.id);
    assert_eq!(markers[0].kind, 2);
    assert_eq!(
        markers[0].token,
        native::NativeLocation {
            kind: 1,
            index: slot.0
        }
    );
}

#[test]
fn ownership_trace_value_producer_starts_before_every_lowering_recipe() {
    let (before, _, table, finalizers) = finalizer_transport_fixture();
    let mut before = before.into_stage::<hirsel_lir::stage::Value>();
    for construct in &mut before.tables.constructs {
        construct.placement = None;
    }
    let before = checked_native_fixture(before, &table);
    let input = native::LeafPlan::prepare_value(&before, &table).unwrap();
    assert_eq!(
        input
            .observations
            .iter()
            .map(|o| o.marker.kind)
            .collect::<Vec<_>>(),
        vec![Kind::Create, Kind::Move, Kind::Drop]
    );
    let original: Vec<_> = input
        .observations
        .iter()
        .map(|o| o.marker.clone())
        .collect();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let placement = hirsel_rc::placement::Options::default();
    let proofs = hirsel_rc::insert::Proofs::default();
    let reuse = hirsel_rc::reuse::Options::default();
    let lowered = hirsel_rc::observed::lower(
        before.into_raw(),
        &input,
        &index,
        &hirsel_rc::observed::Options {
            placement: &placement,
            proofs: &proofs,
            elide: true,
            reuse: &reuse,
            finalizers: &finalizers,
        },
    )
    .unwrap();
    assert_eq!(lowered.recipes.len(), 10);
    assert_eq!(
        lowered
            .plan
            .observations
            .iter()
            .map(|o| o.marker.clone())
            .collect::<Vec<_>>(),
        original
    );
    assert_eq!(lowered.recipes[0].before, input.unit);
    assert_eq!(lowered.recipes.last().unwrap().after, lowered.plan.unit);
    let ready = checked_native_fixture(lowered.unit, &table);
    let native = native::LeafPlan::from_relocated(&ready, &table, &lowered.plan).unwrap();
    assert_eq!(native.plan(), &lowered.plan);
    let drops = check(ready.unit(), &lowered.plan)
        .unwrap()
        .commits(ready.unit(), MarkerId(2))
        .unwrap();
    assert_eq!(drops.len(), 3);
}

#[test]
fn ownership_trace_value_producer_keeps_owned_entry_return_through_native_binding() {
    let mut t = Lir::<hirsel_lir::stage::Value>::new(&[(Passing::Take, l::BLK)], Some(l::BLK));
    let value = t.owned(l::BLK);
    t.param(value);
    t.builder.ret(ENTRY, Some(value));
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table);
    let plan = native::LeafPlan::prepare_value(&unit, &table).unwrap();
    assert_eq!(
        plan.observations
            .iter()
            .map(|o| (o.point, o.marker.kind, o.marker.token, o.marker.owner))
            .collect::<Vec<_>>(),
        vec![
            (
                Point::Entry,
                Kind::Move,
                Location::Parameter(0),
                Location::Value(value)
            ),
            (
                Point::Return(ENTRY),
                Kind::Move,
                Location::Value(value),
                Location::Return
            ),
        ]
    );
    check(unit.unit(), &plan).unwrap();
    let ready = checked_native_fixture(
        unit.into_raw().into_stage::<hirsel_lir::stage::Ready>(),
        &table,
    );
    let rebound = Plan {
        unit: ready.unit().digest(),
        ..plan
    };
    let native = native::LeafPlan::from_relocated(&ready, &table, &rebound).unwrap();
    assert_eq!(native.markers().count(), 2);
    assert_eq!(native.at_point(Point::Entry).count(), 1);
    assert_eq!(native.at_point(Point::Return(ENTRY)).count(), 1);
    let mut missing = rebound.clone();
    missing.observations.pop();
    assert!(native::LeafPlan::from_relocated(&ready, &table, &missing).is_err());
}

#[test]
fn ownership_trace_value_producer_records_owned_call_commit_points_and_refuses_forgery() {
    let mut t = Lir::<hirsel_lir::stage::Value>::new(&[(Passing::Take, l::BLK)], Some(l::BLK));
    let value = t.owned(l::BLK);
    t.param(value);
    let normal = t.builder.block(BlockKind::Body, &[]);
    let failure = t.builder.propagate();
    let result = t
        .call(
            ENTRY,
            &[(Passing::Take, l::BLK)],
            Some(l::BLK),
            &[ArgumentValue::Value(value)],
            (normal, &[]),
            failure,
        )
        .unwrap();
    t.builder.ret(normal, Some(result));
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table);
    let plan = native::LeafPlan::prepare_value(&unit, &table).unwrap();
    let expected = [
        (Point::Entry, Location::Parameter(0), Location::Value(value)),
        (
            Point::CallEntered(ENTRY),
            Location::Value(value),
            Location::Parameter(0),
        ),
        (
            Point::CallReturned(ENTRY),
            Location::Return,
            Location::Value(result),
        ),
        (
            Point::Return(normal),
            Location::Value(result),
            Location::Return,
        ),
    ];
    assert_eq!(plan.observations.len(), expected.len());
    for (observation, (point, token, owner)) in plan.observations.iter().zip(expected) {
        assert_eq!(
            (
                observation.point,
                observation.marker.token,
                observation.marker.owner,
                observation.marker.kind
            ),
            (point, token, owner, Kind::Move)
        );
    }
    check(unit.unit(), &plan).unwrap();
    let mut forged = plan.clone();
    forged.observations[1].marker.owner = Location::Parameter(1);
    assert!(matches!(
        check(unit.unit(), &forged),
        Err(Error::InvalidBinding(MarkerId(1)))
    ));
    let mut wrong_result = plan.clone();
    wrong_result.observations[2].point = Point::CallReturned(normal);
    assert!(check(unit.unit(), &wrong_result).is_err());
}

fn native_plan_binding_case(corrupt_body: bool) {
    use hirsel_lir::{
        products::{Binding, LayoutClosure},
        stage::Ready,
    };
    let (before, _, table, finalizers) = finalizer_transport_fixture();
    let original = native_semantic_plan(&before);
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (ready, plan, _) =
        hirsel_rc::cleanup::lower_observed(before, &finalizers, &index, &original).unwrap();
    let ready = checked_native_fixture(ready, &table);
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let operations = support::registry();
    let manifest = support::manifest(hirsel_core_contract::Disposition::RuntimePrimitive(
        "hirsel_rt_op".into(),
    ));
    let source_inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let (closure, body) = LayoutClosure::capture(&ready, &source_inputs, &[]).unwrap();
    let binding = Binding::new(&target, &[&closure]).unwrap();
    let mut rebound = binding.materialize::<Ready>(&body).unwrap();
    if corrupt_body {
        let integer = rebound
            .tables
            .constants
            .iter_mut()
            .find_map(|constant| {
                if let ConstantValue::Integer(value) = &mut constant.value {
                    Some(value)
                } else {
                    None
                }
            })
            .unwrap();
        *integer ^= 1;
    }
    let bound_inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &binding.table,
        manifest: &manifest,
    };
    let rebound = hirsel_lir::validate::validate_ready(rebound, &bound_inputs).unwrap();
    if corrupt_body {
        let error = native::rebind_plan(&ready, &source_inputs, &rebound, &bound_inputs, &plan)
            .unwrap_err();
        assert!(error.contains("changed canonical body"), "{error}");
    } else {
        assert_ne!(ready.unit().digest(), rebound.unit().digest());
        let relocated =
            native::rebind_plan(&ready, &source_inputs, &rebound, &bound_inputs, &plan).unwrap();
        assert_eq!(relocated.observations, plan.observations);
        assert_eq!(relocated.unit, rebound.unit().digest());
        let commits = check(rebound.unit(), &relocated)
            .unwrap()
            .commits(rebound.unit(), MarkerId(102))
            .unwrap();
        assert_eq!(
            commits.len(),
            3,
            "all physical finalizer exits retain the same id"
        );
        let mut missing = plan.clone();
        missing.observations.pop();
        assert!(
            native::rebind_plan(&ready, &source_inputs, &rebound, &bound_inputs, &missing).is_err()
        );
    }
}

#[test]
fn ownership_trace_native_binding_preserves_original_ids_and_every_finalizer_commit() {
    native_plan_binding_case(false);
}

#[test]
fn ownership_trace_native_binding_refuses_a_different_independently_checked_body() {
    native_plan_binding_case(true);
}

#[test]
fn ownership_trace_parameter_slot_entry_refuses_wrong_parameter_local_slot_and_projection() {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::BLK)], Some(l::BLK));
    let slot = t.slot(l::BLK, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let address = t.at(ENTRY, slot, &[], &[support::Undo::Drop(slot)]);
    let result = t.read(ENTRY, address, version, ReadMode::Take, None);
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
    let plan = Plan::new(
        &unit,
        vec![Observation {
            point: Point::Entry,
            marker: ObserveOwnership {
                id: MarkerId(0),
                site: unit.tables.source_anchors[unit.blocks[ENTRY.index()].anchor.index()].site,
                kind: Kind::Move,
                token: Location::Parameter(0),
                owner: Location::Address(address),
            },
        }],
    );
    check(&unit, &plan).unwrap();
    let mut wrong_parameter = plan.clone();
    wrong_parameter.observations[0].marker.token = Location::Parameter(1);
    assert_eq!(
        check(&unit, &wrong_parameter).unwrap_err(),
        Error::InvalidBinding(MarkerId(0))
    );
    // Equal layout and payload do not turn a local or another parameter into
    // the original incoming owner. The ordinary checker remains independent.
    for kind in [SlotKind::Local, SlotKind::Parameter(1)] {
        let mut wrong = unit.clone();
        wrong.slots[slot.index()].kind = kind;
        let forged = Plan::new(&wrong, plan.observations.clone());
        assert_eq!(
            check(&wrong, &forged).unwrap_err(),
            Error::InvalidBinding(MarkerId(0))
        );
    }
    let mut projected = unit.clone();
    projected.addresses[address.index()].projection.len = 1;
    let forged = Plan::new(&projected, plan.observations.clone());
    assert_eq!(
        check(&projected, &forged).unwrap_err(),
        Error::InvalidBinding(MarkerId(0))
    );
}

#[test]
fn ownership_trace_conditional_drop_requires_its_explicit_moment_and_current_schema() {
    let (mut unit, mut plan, _, _) = drop_transport_fixture();
    let Point::Instruction {
        block,
        index,
        moment,
    } = &mut plan.observations[0].point
    else {
        unreachable!()
    };
    let at = unit.blocks[block.index()].instructions.start as usize + *index as usize;
    let Inst::Drop { if_live, .. } = &mut unit.instructions[at] else {
        unreachable!()
    };
    *if_live = true;
    *moment = Moment::IfLive;
    plan.unit = unit.digest();
    check(&unit, &plan).unwrap();
    let decoded: Plan = hirsel_codec::from_text(&hirsel_codec::to_text(&plan)).unwrap();
    assert_eq!(decoded, plan);
    let mut old = decoded;
    old.version = 3;
    assert_eq!(check(&unit, &old).unwrap_err(), Error::Version(3));
    unit.instructions[at] = match unit.instructions[at] {
        Inst::Drop {
            address,
            anchor,
            failure,
            ..
        } => Inst::Drop {
            address,
            if_live: false,
            anchor,
            failure,
        },
        _ => unreachable!(),
    };
    plan.unit = unit.digest();
    assert_eq!(
        check(&unit, &plan).unwrap_err(),
        Error::InvalidBinding(MarkerId(0))
    );
}

#[test]
fn ownership_trace_finalizer_failure_join_keeps_one_id_and_checks_both_exit_arguments() {
    let (mut before, mut plan, table, finalizers) = finalizer_transport_fixture();
    let source = before
        .values
        .iter()
        .enumerate()
        .find_map(|(index, info)| (info.holding == Holding::Plain).then_some(ValueId(index as u32)))
        .unwrap();
    let parameter = ValueId(before.values.len() as u32);
    before.values.push(before.values[source.index()].clone());
    let Point::Instruction { block, index, .. } = plan.observations[0].point else {
        unreachable!()
    };
    let at = before.blocks[block.index()].instructions.start as usize + index as usize;
    let Inst::Drop { failure, .. } = before.instructions[at] else {
        unreachable!()
    };
    let edge = before.tables.failures[failure.index()].cleanup;
    let target = before.tables.edges[edge.index()].target;
    before.blocks[target.index()].parameters =
        PoolRange::push(&mut before.pools.values, [parameter]);
    before.tables.edges[edge.index()].arguments =
        PoolRange::push(&mut before.pools.values, [source]);
    plan.observations.push(Observation {
        point: Point::Edge(edge),
        marker: ObserveOwnership {
            id: MarkerId(99),
            site: before.tables.source_anchors[before.blocks[target.index()].anchor.index()].site,
            kind: Kind::Move,
            token: Location::Value(source),
            owner: Location::Value(parameter),
        },
    });
    plan.unit = before.digest();
    check(&before, &plan).unwrap();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let (after, output, recipe) =
        hirsel_rc::cleanup::lower_observed(before.clone(), &finalizers, &index, &plan).unwrap();
    check_transform(&before, &plan, &after, &output, Some(&recipe)).unwrap();
    let row = output
        .observations
        .iter()
        .find(|row| row.marker.id == MarkerId(99))
        .unwrap();
    assert!(matches!(row.point, Point::FinalizerDrop { .. }));
    assert_eq!(row.marker, plan.observations[1].marker);
    let points = check(&after, &output)
        .unwrap()
        .commits(&after, MarkerId(99))
        .unwrap();
    assert_eq!(
        points.len(),
        1,
        "both failure exits converge before the original join"
    );
    for point in points {
        let Point::Edge(edge) = point else {
            panic!("failure transfer must commit on an edge")
        };
        let mut wrong = after.clone();
        wrong.tables.edges[edge.index()].arguments = PoolRange::EMPTY;
        let forged = Plan::new(&wrong, output.observations.clone());
        assert_eq!(
            check(&wrong, &forged).unwrap_err(),
            Error::InvalidBinding(MarkerId(99))
        );
    }
}

#[test]
fn ownership_trace_edge_observation_cannot_use_an_unreferenced_pool_row() {
    let (mut unit, plan) = fixture();
    let row = &plan.observations[1].marker;
    let Location::Value(value) = row.token else {
        unreachable!()
    };
    let target = BlockId(unit.blocks.len() as u32);
    let parameters = PoolRange::push(&mut unit.pools.values, [value]);
    unit.blocks.push(Block {
        kind: BlockKind::Body,
        parameters,
        instructions: PoolRange::EMPTY,
        anchor: SourceAnchorId(0),
    });
    unit.terminators.push(Terminator::Return(Some(value)));
    let arguments = PoolRange::push(&mut unit.pools.values, [value]);
    let id = EdgeId(unit.tables.edges.len() as u32);
    unit.tables.edges.push(Edge { target, arguments });
    let marker = ObserveOwnership {
        id: MarkerId(99),
        site: unit.tables.source_anchors[0].site,
        kind: Kind::Move,
        token: Location::Value(value),
        owner: Location::Value(value),
    };
    let plan = Plan::new(
        &unit,
        vec![Observation {
            point: Point::Edge(id),
            marker,
        }],
    );
    assert_eq!(
        check(&unit, &plan).unwrap_err(),
        Error::InvalidPoint(MarkerId(99))
    );
    // The same table row becomes an executed transfer only with a CFG edge.
    unit.terminators[ENTRY.index()] = Terminator::Jump(id);
    let plan = Plan::new(&unit, plan.observations);
    check(&unit, &plan).unwrap();
}

fn once_capture_value_fixture() -> (
    hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Value>,
    hirsel_lir::layout_types::LayoutTable,
    ValueId,
    ValueId,
    ValueId,
) {
    use hirsel_lir::abi_types::{CallMultiplicity, CallableShape};
    use hirsel_lir::stage::Value;
    let mut t = Lir::<Value>::new(
        &[(Passing::Take, l::BLK), (Passing::Take, l::BLK)],
        Some(l::ERASED_ONCE),
    );
    let first = t.owned(l::BLK);
    let second = t.owned(l::BLK);
    t.param(first);
    t.param(second);
    let abi = t.abi_shaped(CallableShape::Closure, &[], Some(l::BLK));
    t.table.fn_abis[abi.0 as usize].multiplicity = CallMultiplicity::Once;
    let code = t.symbol(SymbolKind::Function);
    let captures = PoolRange::push(
        &mut t.builder.unit.pools.captures,
        [
            CaptureValue {
                value: first,
                usage: CaptureUse::Take,
            },
            CaptureValue {
                value: second,
                usage: CaptureUse::Take,
            },
        ],
    );
    t.builder.unit.tables.closures.push(ClosureSite {
        receipts: None,
        scoped: false,
        code: Some(ClosureCode { symbol: code, abi }),
        environment: l::PAIR,
        captures,
        destination: DestinationClass::Heap,
        placement: None,
        anchor: SourceAnchorId(0),
    });
    let result = t.owned(l::ERASED_ONCE);
    // Closure owns its Take captures on both success and failure; C checks
    // that rejection consumes them too, so caller cleanup must not reuse them.
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Closure {
            result,
            closure: ClosureId(0),
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(result));
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table);
    (unit, table, first, second, result)
}

#[test]
fn ownership_trace_value_once_captures_keep_original_operands_and_ordinals() {
    let (unit, table, first, second, result) = once_capture_value_fixture();
    let plan = native::LeafPlan::prepare_value(&unit, &table).unwrap();
    let captures = plan
        .observations
        .iter()
        .filter(|row| matches!(row.marker.owner, Location::Capture { .. }))
        .collect::<Vec<_>>();
    assert_eq!(captures.len(), 2);
    for (ordinal, (row, value)) in captures.iter().zip([first, second]).enumerate() {
        assert_eq!(row.marker.kind, Kind::Move);
        assert_eq!(row.marker.token, Location::Value(value));
        assert_eq!(
            row.marker.owner,
            Location::Capture {
                closure: result,
                capture: ordinal as u32
            }
        );
        let mut forged = plan.clone();
        forged
            .observations
            .iter_mut()
            .find(|other| other.marker.id == row.marker.id)
            .unwrap()
            .marker
            .owner = Location::Capture {
            closure: result,
            capture: 1 - ordinal as u32,
        };
        assert!(matches!(
            check(unit.unit(), &forged),
            Err(Error::InvalidBinding(_))
        ));
        let mut missing = plan.clone();
        missing
            .observations
            .retain(|other| other.marker.id != row.marker.id);
        assert!(matches!(
            check_transform(
                unit.unit(),
                &plan,
                unit.unit(),
                &missing,
                Some(&recipe(&plan, &missing))
            ),
            Err(Error::MissingMarker(_))
        ));
    }
    assert!(
        !plan
            .observations
            .iter()
            .any(|row| row.marker.kind == Kind::Create)
    );
    assert!(
        plan.observations
            .iter()
            .any(|row| row.point == Point::Return(ENTRY)
                && row.marker.token == Location::Value(result)
                && row.marker.owner == Location::Return)
    );
}

#[test]
fn ownership_trace_once_captures_survive_all_recipes_but_refuse_missing_native_target() {
    let (unit, table, _, _, _) = once_capture_value_fixture();
    let input = native::LeafPlan::prepare_value(&unit, &table).unwrap();
    let operations = support::registry();
    let manifest = support::manifest(hirsel_core_contract::Disposition::RuntimePrimitive(
        "hirsel_rt_op".into(),
    ));
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let placement = hirsel_rc::placement::Options::default();
    let proofs = hirsel_rc::insert::Proofs::default();
    let reuse = hirsel_rc::reuse::Options::default();
    let finalizers = Default::default();
    let output = hirsel_rc::observed::lower_validated(
        unit.into_raw(),
        &input,
        &inputs,
        &hirsel_rc::observed::Options {
            placement: &placement,
            proofs: &proofs,
            elide: true,
            reuse: &reuse,
            finalizers: &finalizers,
        },
    )
    .unwrap();
    assert_eq!(output.recipes.len(), 10);
    assert_eq!(
        output
            .plan
            .observations
            .iter()
            .map(|row| &row.marker)
            .collect::<Vec<_>>(),
        input
            .observations
            .iter()
            .map(|row| &row.marker)
            .collect::<Vec<_>>()
    );
    assert_eq!(output.recipes[0].before, input.unit);
    assert_eq!(output.recipes.last().unwrap().after, output.plan.unit);
    let ready = checked_native_fixture(output.unit, &table);
    check(ready.unit(), &output.plan).unwrap();
    let plans = std::collections::BTreeMap::from([(ready.unit().digest(), output.plan)]);
    let refusal = native::Request::Relocated(&plans)
        .prepare(&[&ready], &table)
        .unwrap_err();
    assert!(
        refusal.contains("unspecialized diagnostic call target"),
        "{refusal}"
    );
    let mut raw = ready.into_raw();
    let code = raw.tables.closures[0].code.unwrap();
    raw.tables.symbols[code.symbol.index()].instance = Some(hirsel_syntax::ContentHash([91; 32]));
    let ready = checked_native_fixture(raw, &table);
    let refusal = native::Request::Leaf
        .prepare(&[&ready], &table)
        .unwrap_err();
    assert!(
        refusal.contains("missing diagnostic call target"),
        "{refusal}"
    );
}

#[test]
fn ownership_trace_once_invocation_borrows_environment_and_retains_both_caller_cleanups() {
    use hirsel_lir::abi_types::{CallMultiplicity, CallableShape};
    let mut t = Lir::<hirsel_lir::stage::Value>::new(&[(Passing::Take, l::ERASED_ONCE)], None);
    let slot = t.slot(l::ERASED_ONCE, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let address = t.at(ENTRY, slot, &[], &[support::Undo::Drop(slot)]);
    let loan = t.loan(address, false);
    t.begin(ENTRY, loan);
    let view = t.read(ENTRY, address, version, ReadMode::Borrow, Some(loan));
    let abi = t.abi_shaped(CallableShape::Closure, &[], None);
    t.table.fn_abis[abi.0 as usize].multiplicity = CallMultiplicity::Once;
    let cleanup = [support::Undo::End(loan), support::Undo::Drop(slot)];
    let next = t.cleanup_exit(&cleanup, None);
    let failure = t.cleanup(&cleanup);
    t.call_to(
        ENTRY,
        Callee::Indirect(view),
        abi,
        &[],
        None,
        &[],
        (next, &[]),
        failure,
    );
    let (raw, table, _) = t.finish();
    let unit = checked_native_fixture(raw, &table);
    let plan = native::LeafPlan::prepare_value(&unit, &table).unwrap();
    assert!(
        !plan
            .observations
            .iter()
            .any(|row| matches!(row.point, Point::CallEntered(_) | Point::CallReturned(_)))
    );
    assert!(
        !plan
            .observations
            .iter()
            .any(|row| row.marker.kind == Kind::Create)
    );
    let drop_count = unit
        .unit()
        .instructions
        .iter()
        .filter(|row| matches!(row, Inst::Drop { .. }))
        .count();
    assert!(drop_count >= 2);
    assert_eq!(
        plan.observations
            .iter()
            .filter(|row| row.marker.kind == Kind::Drop)
            .count(),
        drop_count
    );
    let mut many = table.clone();
    many.fn_abis[abi.0 as usize].multiplicity = CallMultiplicity::Many;
    assert!(
        native::LeafPlan::prepare_value(&unit, &many)
            .unwrap_err()
            .contains("indirect call transport")
    );
    let mut forged = plan.clone();
    let dropped = forged
        .observations
        .iter_mut()
        .find(|row| row.marker.kind == Kind::Drop)
        .unwrap();
    dropped.marker.token = Location::Value(view);
    assert!(matches!(
        check(unit.unit(), &forged),
        Err(Error::InvalidBinding(_))
    ));
}

fn alias_fixture() -> (
    hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Value>,
    hirsel_lir::layout_types::LayoutTable,
    Plan,
    ValueId,
    LoanId,
) {
    use hirsel_lir::abi_types::{CallMultiplicity, CallableShape};
    use hirsel_lir::stage::Value;
    let mut t = Lir::<Value>::new(&[], None);
    let captured = t.int(ENTRY);
    let slot = t.slot(l::ERASED_ONCE, SlotKind::Temporary, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let address = t.at(ENTRY, slot, &[], &[]);
    let abi = t.abi_shaped(CallableShape::Closure, &[], None);
    t.table.fn_abis[abi.0 as usize].multiplicity = CallMultiplicity::Once;
    let code = t.symbol(SymbolKind::Function);
    let captures = PoolRange::push(
        &mut t.builder.unit.pools.captures,
        [CaptureValue {
            value: captured,
            usage: CaptureUse::Copy,
        }],
    );
    t.builder.unit.tables.closures.push(ClosureSite {
        receipts: None,
        scoped: false,
        code: Some(ClosureCode { symbol: code, abi }),
        environment: l::ENV,
        captures,
        destination: DestinationClass::Heap,
        placement: None,
        anchor: SourceAnchorId(0),
    });
    let callable = t.owned(l::ERASED_ONCE);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Closure {
            result: callable,
            closure: ClosureId(0),
            failure,
        },
    );
    let version = t.store(ENTRY, address, callable, slot);
    let loan = t.loan(address, false);
    t.begin(ENTRY, loan);
    let view = t.read(ENTRY, address, version, ReadMode::Borrow, Some(loan));
    let cleanup = [support::Undo::End(loan), support::Undo::Drop(slot)];
    let next = t.cleanup_exit(&cleanup, None);
    let failure = t.cleanup(&cleanup);
    t.call_to(
        ENTRY,
        Callee::Indirect(view),
        abi,
        &[],
        None,
        &[],
        (next, &[]),
        failure,
    );
    let (raw, table, _) = t.finish();
    let unit = checked_native_fixture(raw, &table);
    let alias = Observation {
        point: Point::CallEntered(ENTRY),
        marker: ObserveOwnership {
            id: MarkerId(100),
            site: unit.unit().tables.source_anchors[0].site,
            kind: Kind::Alias,
            token: Location::Value(view),
            owner: Location::Address(address),
        },
    };
    let plan = Plan::new(unit.unit(), vec![alias.clone()]);
    (unit, table, plan, callable, loan)
}

#[test]
fn ownership_trace_alias_binds_live_owned_caller_slot_without_transferring_lineage() {
    let (unit, _, plan, callable, loan) = alias_fixture();
    let alias = plan.observations[0].clone();
    let Location::Value(view) = alias.marker.token else {
        panic!("alias view")
    };
    check(unit.unit(), &plan).unwrap();
    let decoded: Plan = hirsel_codec::from_text(&hirsel_codec::to_text(&plan)).unwrap();
    assert_eq!(decoded, plan);
    check_transform(
        unit.unit(),
        &plan,
        unit.unit(),
        &plan,
        Some(&recipe(&plan, &plan)),
    )
    .unwrap();
    let absent = Plan::new(unit.unit(), vec![]);
    assert_eq!(
        check_transform(
            unit.unit(),
            &plan,
            unit.unit(),
            &absent,
            Some(&recipe(&plan, &absent))
        )
        .unwrap_err(),
        Error::MissingMarker(MarkerId(100))
    );
    let mut forged = plan.clone();
    forged.observations[0].marker.owner = Location::Value(callable);
    assert_eq!(
        check(unit.unit(), &forged).unwrap_err(),
        Error::InvalidBinding(MarkerId(100))
    );
    let mut wrong = unit.unit().clone();
    let read = wrong
        .instructions
        .iter()
        .position(|inst| matches!(inst, Inst::Read { result, .. } if *result == view))
        .unwrap();
    wrong.instructions[read] = Inst::EndLoan {
        loan,
        if_active: false,
    };
    let stale_view = Plan::new(&wrong, vec![alias.clone()]);
    assert_eq!(
        check(&wrong, &stale_view).unwrap_err(),
        Error::InvalidBinding(MarkerId(100))
    );
    let mut plain = unit.unit().clone();
    plain.values[callable.index()].holding = Holding::Plain;
    let unowned = Plan::new(&plain, vec![alias]);
    assert_eq!(
        check(&plain, &unowned).unwrap_err(),
        Error::InvalidBinding(MarkerId(100))
    );
}

#[test]
fn ownership_trace_alias_survives_every_recipe_without_changing_ordinary_ready() {
    let (unit, table, input, _, _) = alias_fixture();
    let ordinary = hirsel_codec::to_text(unit.unit());
    let operations = support::registry();
    let manifest = support::manifest(hirsel_core_contract::Disposition::RuntimePrimitive(
        "hirsel_rt_op".into(),
    ));
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let placement = hirsel_rc::placement::Options::default();
    let proofs = hirsel_rc::insert::Proofs::default();
    let reuse = hirsel_rc::reuse::Options::default();
    let finalizers = Default::default();
    let options = hirsel_rc::observed::Options {
        placement: &placement,
        proofs: &proofs,
        elide: true,
        reuse: &reuse,
        finalizers: &finalizers,
    };
    check(unit.unit(), &input).unwrap();
    assert_eq!(hirsel_codec::to_text(unit.unit()), ordinary);
    let unobserved = Plan::new(unit.unit(), vec![]);
    let without =
        hirsel_rc::observed::lower_validated(unit.unit().clone(), &unobserved, &inputs, &options)
            .unwrap();
    let output =
        hirsel_rc::observed::lower_validated(unit.into_raw(), &input, &inputs, &options).unwrap();
    assert_eq!(
        hirsel_codec::to_text(&output.unit),
        hirsel_codec::to_text(&without.unit)
    );
    assert_eq!(output.recipes.len(), 10);
    assert_eq!(output.plan.observations.len(), 1);
    assert_eq!(
        output.plan.observations[0].marker,
        input.observations[0].marker
    );
    assert_eq!(output.plan.observations[0].marker.kind, Kind::Alias);
    let ready = checked_native_fixture(output.unit, &table);
    check(ready.unit(), &output.plan).unwrap();
    assert!(native::LeafPlan::from_relocated(&ready, &table, &output.plan).is_err());
}

#[test]
fn ownership_trace_alias_refuses_ended_owner_scope_and_reactivated_stale_view() {
    let (unit, _, plan, _, loan) = alias_fixture();
    for suffix in [
        vec![Inst::EndScope { scope: ROOT }],
        vec![
            Inst::EndLoan {
                loan,
                if_active: false,
            },
            Inst::BeginLoan {
                loan,
                address: unit.unit().tables.loans[loan.index()].address,
                mutable: false,
                if_inactive: false,
            },
        ],
    ] {
        let mut raw = unit.unit().clone();
        let mut code = raw.blocks[ENTRY.index()]
            .instructions
            .get(&raw.instructions)
            .unwrap()
            .to_vec();
        code.extend(suffix);
        raw.blocks[ENTRY.index()].instructions = PoolRange::push(&mut raw.instructions, code);
        let forged = Plan::new(&raw, plan.observations.clone());
        assert_eq!(
            check(&raw, &forged).unwrap_err(),
            Error::InvalidBinding(MarkerId(100))
        );
    }
}

#[test]
fn ownership_trace_call_inventory_separates_implicit_cleanup_without_losing_physical_targets() {
    use hirsel_lir::stage::Ready;
    let (before, _, table, finalizers) = finalizer_transport_fixture();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let caller = hirsel_rc::ready::ready_indexed(before, &finalizers, &index).unwrap();
    let caller = checked_native_fixture(caller, &table);
    let symbol = caller
        .unit()
        .tables
        .symbols
        .iter()
        .find(|symbol| symbol.kind == SymbolKind::Function)
        .unwrap();
    // Cleanup deliberately uses Function symbols, just like source calls.
    let mut t = Lir::<Ready>::new(&[(Passing::Borrow, l::RES)], None);
    let slot = t.slot(l::RES, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    t.builder.ret(ENTRY, None);
    let (mut target, target_table, _) = t.finish();
    target.header.definition = symbol.definition.clone();
    target.header.instance = symbol.instance.unwrap();
    let target = checked_native_fixture(target, &target_table);
    let selected = [&caller, &target];
    let calls = native::local_calls(&selected).unwrap();
    assert!(calls.values().all(|targets| targets.is_empty()));
    let sites = native::local_call_sites(&selected).unwrap();
    assert_eq!(
        sites[&caller.unit().digest().0]
            .values()
            .copied()
            .collect::<Vec<_>>(),
        vec![target.unit().digest().0]
    );
    assert!(
        native::local_calls(&[&caller])
            .unwrap_err()
            .contains("missing diagnostic call target")
    );

    // The same borrowed receiver and callee remain ordinary without the
    // matching finalization protocol. Symbol identity is never a shortcut.
    let mut t = Lir::<Ready>::new(&[(Passing::Borrow, l::RES)], None);
    let slot = t.slot(l::RES, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let address = t.at(ENTRY, slot, &[], &[]);
    let loan = t.loan(address, false);
    t.begin(ENTRY, loan);
    let receiver = t.read(ENTRY, address, version, ReadMode::Borrow, Some(loan));
    let normal = t.builder.block(BlockKind::Body, &[]);
    let failure = t.cleanup(&[support::Undo::End(loan)]);
    t.call(
        ENTRY,
        &[(Passing::Borrow, l::RES)],
        None,
        &[ArgumentValue::Value(receiver)],
        (normal, &[]),
        failure,
    );
    t.end(normal, loan);
    t.builder.ret(normal, None);
    let (mut ordinary, ordinary_table, _) = t.finish();
    ordinary.header.definition = support::path("ordinary");
    ordinary.tables.symbols[0] = symbol.clone();
    let ordinary = checked_native_fixture(ordinary, &ordinary_table);
    let calls = native::local_calls(&[&ordinary, &target]).unwrap();
    assert_eq!(
        calls[&ordinary.unit().digest().0]
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![target.unit().digest().0]
    );
}

#[test]
fn ownership_trace_nested_source_construction_transfers_fields_without_a_shell_token() {
    use hirsel_lir::stage::Value;
    let mut t = Lir::<Value>::new(
        &[(Passing::Take, l::BLK), (Passing::Take, l::BLK)],
        Some(l::PAIR),
    );
    let first = t.owned(l::BLK);
    let second = t.owned(l::BLK);
    t.param(first);
    t.param(second);
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let construct = t.construct_site(
        ConstructorKind::Record(symbol),
        l::PAIR,
        &[(0, first), (1, second)],
        placement,
    );
    t.builder.unit.tables.constructs[construct.index()].placement = None;
    let result = t.owned(l::PAIR);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result,
            construct,
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(result));
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table);
    let plan = native::LeafPlan::prepare_value(&unit, &table).unwrap();
    check(unit.unit(), &plan).unwrap();
    assert_eq!(plan.observations.len(), 5);
    assert!(
        plan.observations
            .iter()
            .all(|row| row.marker.kind == Kind::Move)
    );
    for (value, field) in [(first, 0), (second, 1)] {
        let row = plan
            .observations
            .iter()
            .find(|row| row.marker.token == Location::Value(value))
            .unwrap();
        assert_eq!(
            row.marker.owner,
            Location::Field {
                aggregate: result,
                field
            }
        );
        let mut forged = plan.clone();
        let forged_row = forged
            .observations
            .iter_mut()
            .find(|candidate| candidate.marker.id == row.marker.id)
            .unwrap();
        forged_row.marker.owner = Location::Field {
            aggregate: result,
            field: 1 - field,
        };
        assert!(
            check(unit.unit(), &forged).is_err(),
            "equal-layout field ordinals remain distinct"
        );
    }
    let index = hirsel_lir::LayoutIndex::new(&table);
    let placement = hirsel_rc::placement::Options::default();
    let proofs = hirsel_rc::insert::Proofs::default();
    let reuse = hirsel_rc::reuse::Options::default();
    let finalizers = Default::default();
    let output = hirsel_rc::observed::lower(
        unit.into_raw(),
        &plan,
        &index,
        &hirsel_rc::observed::Options {
            placement: &placement,
            proofs: &proofs,
            elide: true,
            reuse: &reuse,
            finalizers: &finalizers,
        },
    )
    .unwrap();
    assert_eq!(output.recipes.len(), 10);
    assert_eq!(
        output
            .plan
            .observations
            .iter()
            .map(|row| &row.marker)
            .collect::<Vec<_>>(),
        plan.observations
            .iter()
            .map(|row| &row.marker)
            .collect::<Vec<_>>()
    );
    let ready = checked_native_fixture(output.unit, &table);
    let native = native::LeafPlan::from_relocated(&ready, &table, &output.plan).unwrap();
    for row in output
        .plan
        .observations
        .iter()
        .filter(|row| matches!(row.marker.owner, Location::Field { .. }))
    {
        let marker = native
            .markers()
            .find(|marker| marker.id == row.marker.id)
            .unwrap();
        let Location::Field { aggregate, field } = row.marker.owner else {
            unreachable!()
        };
        assert_eq!(
            marker.owner,
            native::NativeLocation {
                kind: field + 8,
                index: aggregate.0
            }
        );
    }
}

fn compound_drop_fixture(
    alternatives: bool,
) -> (
    hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>,
    Plan,
    Plan,
    CleanupAnchorId,
    hirsel_lir::layout_types::LayoutTable,
) {
    compound_drop_fixture_with_parent(alternatives, false)
}

fn compound_drop_fixture_with_parent(
    alternatives: bool,
    parent: bool,
) -> (
    hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>,
    Plan,
    Plan,
    CleanupAnchorId,
    hirsel_lir::layout_types::LayoutTable,
) {
    compound_drop_fixture_with_children(alternatives, parent, l::RES)
}

fn compound_drop_fixture_with_children(
    alternatives: bool,
    parent: bool,
    child: hirsel_lir::LayoutId,
) -> (
    hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>,
    Plan,
    Plan,
    CleanupAnchorId,
    hirsel_lir::layout_types::LayoutTable,
) {
    compound_drop_fixture_with_layout(alternatives, parent, child, l::PAIR)
}

fn compound_drop_fixture_with_layout(
    alternatives: bool,
    parent: bool,
    child: hirsel_lir::LayoutId,
    container: hirsel_lir::LayoutId,
) -> (
    hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>,
    Plan,
    Plan,
    CleanupAnchorId,
    hirsel_lir::layout_types::LayoutTable,
) {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep};
    let mut t = Lir::<Rc>::new(&[(Passing::Take, container)], None);
    if container == l::OPT {
        let carrier = &mut t.table.layouts[container.0 as usize];
        carrier.tag.as_mut().unwrap().variants[1].fields[0].layout = child;
        carrier.copy = CopyRecipe::NotCopyable;
        carrier.drop = vec![DropStep::ActiveVariant {
            tag_offset: 0,
            form: hirsel_lir::layout_types::TagForm::Ordinal,
            variants: vec![(
                1,
                vec![DropStep::Child {
                    offset: 8,
                    layout: child,
                }],
            )],
        }];
    } else {
        let pair = &mut t.table.layouts[container.0 as usize];
        pair.fields[0].layout = child;
        pair.fields[1].layout = child;
        pair.copy = CopyRecipe::NotCopyable;
        pair.drop = vec![
            DropStep::Child {
                offset: 8,
                layout: child,
            },
            DropStep::Child {
                offset: 0,
                layout: child,
            },
        ];
        if parent {
            pair.kind = hirsel_lir::layout_types::LayoutKind::Resource;
            pair.finalizer = Some(hirsel_lir::layout_types::FinalizerRef {
                definition: support::path("Pair"),
                body: 1,
                unsafe_ffi_package: false,
            });
            pair.drop.insert(0, DropStep::Finalize);
        }
        pair.scan.clear();
        if child == l::BLK {
            pair.scan = vec![
                hirsel_lir::layout_types::ScanStep::Counted { offset: 0 },
                hirsel_lir::layout_types::ScanStep::Counted { offset: 8 },
            ];
            pair.drop = vec![
                DropStep::Release { offset: 8 },
                DropStep::Release { offset: 0 },
            ];
            if parent {
                pair.drop.insert(0, DropStep::Finalize);
            }
        }
    }
    let abi = t.abi(&[(Passing::Borrow, l::RES)], None);
    let parent_abi = t.abi(&[(Passing::Borrow, container)], None);
    let slot = t.slot(container, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let address = t.at(ENTRY, slot, &[], &[support::Undo::Drop(slot)]);
    let anchor = t.cleanup_anchor();
    let bodies = if alternatives {
        let yes = t.builder.block(BlockKind::Body, &[]);
        let no = t.builder.block(BlockKind::Body, &[]);
        t.branch(ENTRY, (yes, &[]), (no, &[]));
        vec![yes, no]
    } else {
        vec![ENTRY]
    };
    for &body in &bodies {
        let failure = t.fails();
        t.builder.push(
            body,
            Inst::Drop {
                address,
                if_live: false,
                anchor,
                failure,
            },
        );
        t.builder.push(
            body,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
        t.builder.ret(body, None);
    }
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table).into_raw();
    let plan = Plan::new(
        &unit,
        bodies
            .iter()
            .enumerate()
            .map(|(id, &block)| {
                let index = unit.blocks[block.index()]
                    .instructions
                    .get(&unit.instructions)
                    .unwrap()
                    .iter()
                    .position(|instruction| matches!(instruction, Inst::Drop { .. }))
                    .unwrap() as u32;
                Observation {
                    point: Point::Instruction {
                        block,
                        index,
                        moment: Moment::Before,
                    },
                    marker: ObserveOwnership {
                        id: MarkerId(id as u32),
                        kind: Kind::Drop,
                        site: unit.tables.cleanup_anchors[anchor.index()].site,
                        token: Location::Address(address),
                        owner: Location::Address(address),
                    },
                }
            })
            .collect(),
    );
    let mut finalizers = hirsel_rc::cleanup::Finalizers(std::collections::BTreeMap::from([(
        l::RES,
        hirsel_rc::cleanup::Finalizer {
            origin: UnitOrigin::Source,
            definition: support::path("Res"),
            instance: support::hash("Res.drop"),
            abi,
        },
    )]));
    if parent {
        finalizers.0.insert(
            container,
            hirsel_rc::cleanup::Finalizer {
                origin: UnitOrigin::Source,
                definition: support::path("Pair"),
                instance: support::hash("Pair.drop"),
                abi: parent_abi,
            },
        );
    }
    let (after, output, _) = hirsel_rc::cleanup::lower_observed(
        unit,
        &finalizers,
        &hirsel_lir::LayoutIndex::new(&table),
        &plan,
    )
    .unwrap();
    let after = checked_native_fixture(after, &table);
    (after, output, plan, anchor, table)
}

#[test]
fn ownership_trace_compound_drop_preserves_one_marker_for_all_actual_child_finalizers() {
    let (after, output, plan, anchor, _) = compound_drop_fixture(false);
    assert_eq!(output.observations.len(), 1);
    assert_eq!(output.observations[0].marker, plan.observations[0].marker);
    assert!(matches!(
        output.observations[0].point,
        Point::CompoundDrop { .. }
    ));
    let checked = check(after.unit(), &output).unwrap();
    let commits = checked.commits(after.unit(), MarkerId(0)).unwrap();
    assert!(
        commits.len() >= 6,
        "both children retain all exclusive completion paths: {commits:?}"
    );
    for point in commits {
        let Point::Instruction {
            block,
            index,
            moment: Moment::After,
        } = point
        else {
            panic!("not an actual completion")
        };
        assert!(matches!(
            after.unit().blocks[block.index()]
                .instructions
                .get(&after.unit().instructions)
                .unwrap()[index as usize],
            Inst::FinalizeEnd { .. }
        ));
    }
    let mut forged = output.clone();
    let Point::CompoundDrop {
        normal, failure, ..
    } = &mut forged.observations[0].point
    else {
        unreachable!()
    };
    *failure = *normal;
    assert!(check(after.unit(), &forged).is_err());

    let mut prefix_only = output.clone();
    let first_end = checked.commits(after.unit(), MarkerId(0)).unwrap()[0];
    let Point::Instruction { block, index, .. } = first_end else {
        unreachable!()
    };
    if let Point::CompoundDrop {
        normal, index: cut, ..
    } = &mut prefix_only.observations[0].point
    {
        *normal = block;
        *cut = index + 1;
    }
    assert!(
        check(after.unit(), &prefix_only).is_err(),
        "a checked prefix cannot omit the other child"
    );

    let mut missing = after.into_raw();
    let end = missing
        .instructions
        .iter_mut()
        .find(|instruction| {
            matches!(instruction,
        Inst::FinalizeEnd { anchor: action, .. } if *action == anchor)
        })
        .unwrap();
    *end = Inst::FrameLeave;
    let mut missing_plan = output;
    missing_plan.unit = missing.digest();
    assert!(
        check(&missing, &missing_plan).is_err(),
        "a missing physical completion cannot be inferred"
    );
}

#[test]
fn ownership_trace_compound_drop_keeps_same_anchor_alternatives_separate() {
    let (after, output, input, _, _) = compound_drop_fixture(true);
    assert_eq!(output.observations.len(), 2);
    let checked = check(after.unit(), &output).unwrap();
    let mut entries = Vec::new();
    for (old, row) in input.observations.iter().zip(&output.observations) {
        assert_eq!(row.marker, old.marker);
        let Point::CompoundDrop { block, .. } = row.point else {
            panic!("missing typed bundle")
        };
        entries.push(block);
        assert!(checked.commits(after.unit(), row.marker.id).unwrap().len() >= 6);
    }
    assert_ne!(entries[0], entries[1]);
    let first = checked.commits(after.unit(), MarkerId(0)).unwrap()[0];
    let Point::Instruction { block, index, .. } = first else {
        unreachable!()
    };
    let mut truncated = output.clone();
    if let Point::CompoundDrop {
        normal, index: cut, ..
    } = &mut truncated.observations[0].point
    {
        *normal = block;
        *cut = index + 1;
    }
    assert!(check(after.unit(), &truncated).is_err());
}

#[test]
fn ownership_trace_native_static_field_locations_keep_declaration_paths_and_refuse_absent_addresses()
 {
    let mut t = Lir::<hirsel_lir::stage::Ready>::new(&[(Passing::Borrow, l::PAIR)], None);
    let slot = t.slot(l::PAIR, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let address = t.at(ENTRY, slot, &[ProjectionStep::Field(1)], &[]);
    t.builder.ret(ENTRY, None);
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table);
    assert_eq!(
        native::projected_location(
            unit.unit(),
            &table,
            native::NativeLocation {
                kind: 7,
                index: address.0
            }
        )
        .unwrap(),
        Some(native::NativePath {
            root: native::NativeLocation {
                kind: 1,
                index: slot.0
            },
            fields: vec![native::NativeProjection::Field(1)]
        })
    );
    assert!(
        native::projected_location(
            unit.unit(),
            &table,
            native::NativeLocation {
                kind: 7,
                index: u32::MAX
            }
        )
        .is_err()
    );
    assert!(
        native::projected_location(
            unit.unit(),
            &table,
            native::NativeLocation {
                kind: 1,
                index: slot.0
            }
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn ownership_trace_native_compound_completions_keep_original_marker_and_exact_children() {
    let (after, output, _, _, table) = compound_drop_fixture(true);
    let checked = check(after.unit(), &output).unwrap();
    let points: Vec<_> = output
        .observations
        .iter()
        .flat_map(|row| checked.commits(after.unit(), row.marker.id).unwrap())
        .collect();
    let ready = native::LeafPlan::prepare(&after, &table).unwrap();
    let mut rows = output.observations.clone();
    for row in &ready.plan().observations {
        if points.contains(&row.point) {
            continue;
        }
        let mut row = row.clone();
        row.marker.id = MarkerId(rows.len() as u32);
        rows.push(row);
    }
    let plan = Plan::new(after.unit(), rows);
    let native = native::LeafPlan::from_relocated(&after, &table, &plan).unwrap();
    for row in &output.observations {
        let members = native
            .markers()
            .filter(|m| m.id == row.marker.id)
            .collect::<Vec<_>>();
        assert_eq!(members.len(), 2);
        let mut paths = std::collections::BTreeSet::new();
        for point in checked.commits(after.unit(), row.marker.id).unwrap() {
            let markers = native.at_point(point).collect::<Vec<_>>();
            assert_eq!(markers.len(), 1);
            let marker = markers[0];
            assert_eq!(marker.id, row.marker.id);
            assert_eq!(marker.kind, 2);
            assert_eq!(marker.site, row.marker.site);
            assert_eq!(marker.token, marker.owner);
            let path = native::projected_location(after.unit(), &table, marker.token)
                .unwrap()
                .unwrap();
            paths.insert(path.fields);
        }
        assert_eq!(
            paths,
            std::collections::BTreeSet::from([
                vec![native::NativeProjection::Field(0)],
                vec![native::NativeProjection::Field(1)]
            ])
        );
    }
    let mut missing = plan.clone();
    missing.observations.remove(0);
    assert!(native::LeafPlan::from_relocated(&after, &table, &missing).is_err());
    let mut forged = plan.clone();
    forged.observations[0].marker.owner = Location::Value(ValueId(0));
    assert!(native::LeafPlan::from_relocated(&after, &table, &forged).is_err());
}

#[test]
fn ownership_trace_compound_failure_boundary_stays_before_independent_cleanup_splice() {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep};
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::PAIR), (Passing::Take, l::PAIR)], None);
    let pair = &mut t.table.layouts[l::PAIR.0 as usize];
    pair.fields[0].layout = l::RES;
    pair.fields[1].layout = l::RES;
    pair.copy = CopyRecipe::NotCopyable;
    pair.drop = vec![
        DropStep::Child {
            offset: 8,
            layout: l::RES,
        },
        DropStep::Child {
            offset: 0,
            layout: l::RES,
        },
    ];
    pair.scan.clear();
    let abi = t.abi(&[(Passing::Borrow, l::RES)], None);
    let first = t.slot(l::PAIR, SlotKind::Parameter(0), ROOT);
    let first_version = t.version_of(first);
    t.param(first_version);
    let second = t.slot(l::PAIR, SlotKind::Parameter(1), ROOT);
    let second_version = t.version_of(second);
    t.param(second_version);
    // Lowering visits the source drop first. Its failure successor is then
    // spliced into a distinct compound cleanup with its own original anchor.
    let cleanup = t.builder.block(BlockKind::FailureCleanup, &[]);
    let body = t.builder.block(BlockKind::Body, &[]);
    let undo = [support::Undo::Drop(second), support::Undo::Drop(first)];
    let first_address = t.at(ENTRY, first, &[], &undo);
    let second_address = t.at(ENTRY, second, &[], &undo);
    t.builder.jump(ENTRY, body, &[]);
    let anchor = t.cleanup_anchor();
    let independent = t.cleanup_anchor();
    let failed = t.failure(cleanup);
    t.builder.push(
        body,
        Inst::Drop {
            address: first_address,
            if_live: false,
            anchor,
            failure: failed,
        },
    );
    t.builder.push(
        body,
        Inst::StorageDead {
            slot: first,
            if_live: false,
        },
    );
    let failure = t.fails();
    t.builder.push(
        body,
        Inst::Drop {
            address: second_address,
            if_live: false,
            anchor: independent,
            failure,
        },
    );
    t.builder.push(
        body,
        Inst::StorageDead {
            slot: second,
            if_live: false,
        },
    );
    t.builder.ret(body, None);
    t.builder.push(
        cleanup,
        Inst::StorageDead {
            slot: first,
            if_live: false,
        },
    );
    let failure = t.fails();
    t.builder.push(
        cleanup,
        Inst::Drop {
            address: second_address,
            if_live: false,
            anchor: independent,
            failure,
        },
    );
    t.builder.push(
        cleanup,
        Inst::StorageDead {
            slot: second,
            if_live: false,
        },
    );
    t.builder.term(cleanup, Terminator::Propagate);
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table).into_raw();
    let plan = Plan::new(
        &unit,
        vec![Observation {
            point: Point::Instruction {
                block: body,
                index: 0,
                moment: Moment::Before,
            },
            marker: ObserveOwnership {
                id: MarkerId(0),
                site: unit.tables.cleanup_anchors[anchor.index()].site,
                kind: Kind::Drop,
                token: Location::Address(first_address),
                owner: Location::Address(first_address),
            },
        }],
    );
    let finalizers = hirsel_rc::cleanup::Finalizers(std::collections::BTreeMap::from([(
        l::RES,
        hirsel_rc::cleanup::Finalizer {
            origin: UnitOrigin::Source,
            definition: support::path("Res"),
            instance: support::hash("Res.drop"),
            abi,
        },
    )]));
    let (after, output, _) = hirsel_rc::cleanup::lower_observed(
        unit,
        &finalizers,
        &hirsel_lir::LayoutIndex::new(&table),
        &plan,
    )
    .unwrap();
    let after = checked_native_fixture(after, &table);
    let checked = check(after.unit(), &output).unwrap();
    assert_eq!(output.observations[0].marker, plan.observations[0].marker);
    let commits = checked.commits(after.unit(), MarkerId(0)).unwrap();
    assert!(commits.len() >= 6);
    for point in commits {
        let Point::Instruction { block, index, .. } = point else {
            panic!("not a physical completion")
        };
        assert!(
            matches!(after.unit().blocks[block.index()].instructions.get(&after.unit().instructions).unwrap()[index as usize],
            Inst::FinalizeEnd { anchor: action, .. } if action == anchor)
        );
    }
}

#[test]
fn ownership_trace_arena_control_relocates_original_unique_lifetimes() {
    let mut t = Lir::<hirsel_lir::stage::Value>::new(&[], None);
    t.builder.unit.header.origin = UnitOrigin::Library;
    let token = t.plain(l::ARENA);
    t.builder.push(
        ENTRY,
        Inst::ArenaOpen {
            result: token,
            budget: None,
        },
    );
    let slot = t.slot(l::BLK, SlotKind::Local, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let address = t.at(ENTRY, slot, &[], &[]);
    let resource = t.new_block(ENTRY, &[]);
    t.store(ENTRY, address, resource, slot);
    let failure = t.fails();
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
    let (unit, table, _) = t.finish();
    let before = checked_native_fixture(unit, &table);
    let input = native::LeafPlan::prepare_value(&before, &table).unwrap();
    assert_eq!(input.observations.len(), 3);
    let options = hirsel_rc::placement::Options {
        regions: false,
        noalloc: false,
    };
    let ordinary = hirsel_rc::placement::place(before.unit().clone(), &table, &options).unwrap();
    let (after, output, recipe) =
        hirsel_rc::placement::place_observed(before.unit().clone(), &table, &options, &input)
            .unwrap();
    let after = checked_native_fixture(after, &table);
    assert_eq!(
        hirsel_codec::to_text(after.unit()),
        hirsel_codec::to_text(&ordinary)
    );
    check_transform(before.unit(), &input, after.unit(), &output, Some(&recipe)).unwrap();
    for (old, new) in input.observations.iter().zip(&output.observations) {
        assert_eq!(old.marker, new.marker);
        assert_ne!(old.point, new.point);
    }
    let mut missing = output.clone();
    missing.observations.pop();
    assert!(check_transform(before.unit(), &input, after.unit(), &missing, Some(&recipe)).is_err());
    let mut stale = output.clone();
    stale.observations[0].point = input.observations[0].point;
    assert!(check(after.unit(), &stale).is_err());
    let mut forged = output.clone();
    let mut extra = forged.observations[0].clone();
    extra.marker.id = MarkerId(999);
    extra.point = Point::Instruction {
        block: after.unit().entry,
        index: 0,
        moment: Moment::After,
    };
    forged.observations.push(extra);
    assert!(check(after.unit(), &forged).is_err());
}

#[test]
fn ownership_trace_nested_arena_entries_keep_original_lifetimes_through_ready() {
    let (before, _, table, finalizers) = finalizer_transport_fixture();
    let mut before = before.into_stage::<hirsel_lir::stage::Value>();
    before.header.origin = UnitOrigin::Library;
    for construct in &mut before.tables.constructs {
        construct.placement = None;
    }
    let arena_type = before.types.iter().position(|ty| ty.layout == l::ARENA);
    let arena_type = arena_type
        .map(|index| LirTypeId(index as u32))
        .unwrap_or_else(|| {
            let index = LirTypeId(before.types.len() as u32);
            before.types.push(LirType {
                repr: ValueRepr::Arena,
                layout: l::ARENA,
            });
            index
        });
    for _ in 0..2 {
        let token = ValueId(before.values.len() as u32);
        before.values.push(ValueInfo {
            ty: arena_type,
            holding: Holding::Plain,
        });
        let at = before.blocks[before.entry.index()].instructions.start as usize;
        before.instructions.insert(
            at,
            Inst::ArenaOpen {
                result: token,
                budget: None,
            },
        );
        for (index, block) in before.blocks.iter_mut().enumerate() {
            if index == before.entry.index() {
                block.instructions.len += 1;
            } else if block.instructions.start as usize >= at {
                block.instructions.start += 1;
            }
        }
    }
    let before = checked_nested_arena_fixture(before, &table);
    let input = native::LeafPlan::prepare_value(&before, &table).unwrap();
    assert_eq!(input.observations.len(), 3);
    let index = hirsel_lir::LayoutIndex::new(&table);
    let lowered = hirsel_rc::observed::lower(
        before.unit().clone(),
        &input,
        &index,
        &hirsel_rc::observed::Options {
            placement: &hirsel_rc::placement::Options::default(),
            proofs: &hirsel_rc::insert::Proofs::default(),
            elide: true,
            reuse: &hirsel_rc::reuse::Options::default(),
            finalizers: &finalizers,
        },
    )
    .unwrap();
    assert_eq!(lowered.recipes.len(), 10);
    assert_eq!(lowered.recipes[0].before, input.unit);
    assert_eq!(lowered.recipes.last().unwrap().after, lowered.plan.unit);
    assert_eq!(
        lowered
            .plan
            .observations
            .iter()
            .map(|row| &row.marker)
            .collect::<Vec<_>>(),
        input
            .observations
            .iter()
            .map(|row| &row.marker)
            .collect::<Vec<_>>()
    );
    let ready = checked_nested_arena_fixture(lowered.unit, &table);
    native::LeafPlan::from_relocated(&ready, &table, &lowered.plan).unwrap();
}

fn checked_nested_arena_fixture<S: hirsel_lir::stage::Stage>(
    unit: LirUnit<S>,
    table: &hirsel_lir::layout_types::LayoutTable,
) -> hirsel_lir::stage::ValidatedUnit<S> {
    // The independently declared finalizer is body 1 of Res, as in the
    // fixture layout; a Library caller must carry that invocation reference.
    let mut operations = support::registry();
    operations
        .invocation_bodies
        .push(hirsel_lir::invocations::ReferenceBody {
            definition: support::path("Res"),
            instance: support::hash("Res.drop"),
            body: 1,
        });
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let manifest = support::manifest(hirsel_core_contract::Disposition::RuntimePrimitive(
        "hirsel_rt_op".into(),
    ));
    hirsel_lir::validate::validate(
        unit,
        &hirsel_lir::validate::Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &operations,
            target: &target,
            layouts: table,
            manifest: &manifest,
        },
    )
    .unwrap()
}

#[test]
fn ownership_trace_alias_checks_the_live_owner_across_a_split_block() {
    let (unit, table, plan, _, loan) = alias_fixture();
    let mut raw = unit.unit().clone();
    let code = raw.blocks[ENTRY.index()].instructions;
    let split = code
        .get(&raw.instructions)
        .unwrap()
        .iter()
        .position(|inst| matches!(inst, Inst::BeginLoan { .. }))
        .unwrap();
    let called = BlockId(raw.blocks.len() as u32);
    raw.blocks.push(Block {
        kind: BlockKind::Body,
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::new(code.start + split as u32, code.len - split as u32),
        anchor: raw.blocks[ENTRY.index()].anchor,
    });
    raw.blocks[ENTRY.index()].instructions.len = split as u32;
    raw.terminators.push(raw.terminators[ENTRY.index()].clone());
    let edge = EdgeId(raw.tables.edges.len() as u32);
    raw.tables.edges.push(Edge {
        target: called,
        arguments: PoolRange::EMPTY,
    });
    raw.terminators[ENTRY.index()] = Terminator::Jump(edge);
    let checked = checked_native_fixture(raw.clone(), &table);
    let mut observations = plan.observations.clone();
    observations[0].point = Point::CallEntered(called);
    let plan = Plan::new(checked.unit(), observations.clone());
    check(checked.unit(), &plan).unwrap();
    for suffix in [
        vec![Inst::EndScope { scope: ROOT }],
        vec![Inst::EndLoan {
            loan,
            if_active: false,
        }],
    ] {
        let mut bad = raw.clone();
        let mut code = bad.blocks[called.index()]
            .instructions
            .get(&bad.instructions)
            .unwrap()
            .to_vec();
        code.extend(suffix);
        bad.blocks[called.index()].instructions = PoolRange::push(&mut bad.instructions, code);
        let forged = Plan::new(&bad, observations.clone());
        assert_eq!(
            check(&bad, &forged).unwrap_err(),
            Error::InvalidBinding(MarkerId(100))
        );
    }
    let mut joined = raw.clone();
    let extra = BlockId(joined.blocks.len() as u32);
    joined.blocks.push(Block {
        kind: BlockKind::Body,
        parameters: PoolRange::EMPTY,
        instructions: PoolRange::EMPTY,
        anchor: joined.blocks[ENTRY.index()].anchor,
    });
    joined.terminators.push(Terminator::Jump(edge));
    let _ = extra;
    let forged = Plan::new(&joined, observations);
    assert_eq!(
        check(&joined, &forged).unwrap_err(),
        Error::InvalidBinding(MarkerId(100))
    );
}

#[test]
fn ownership_trace_alias_proves_the_callable_parameters_actual_entry_version() {
    use hirsel_lir::abi_types::{CallMultiplicity, CallableShape};
    let mut t = Lir::<hirsel_lir::stage::Value>::new(&[(Passing::Take, l::ERASED_ONCE)], None);
    let slot = t.slot(l::ERASED_ONCE, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let address = t.address(AddressBase::Slot(slot), &[], l::ERASED_ONCE);
    let cleanup = t.cleanup(&[support::Undo::Drop(slot)]);
    let failure = t.failure(cleanup);
    t.builder.push(ENTRY, Inst::Address { address, failure });
    let abi = t.abi_shaped(CallableShape::Closure, &[], None);
    t.table.fn_abis[abi.0 as usize].multiplicity = CallMultiplicity::Once;
    let loan = t.loan(address, false);
    t.begin(ENTRY, loan);
    let view = t.read(ENTRY, address, version, ReadMode::Borrow, Some(loan));
    let cleanup = [support::Undo::End(loan), support::Undo::Drop(slot)];
    let next = t.cleanup_exit(&cleanup, None);
    let failure = t.cleanup(&cleanup);
    let target = t.symbol(SymbolKind::Function);
    t.call_to(
        ENTRY,
        Callee::Bound {
            code: target,
            closure: view,
        },
        abi,
        &[],
        None,
        &[],
        (next, &[]),
        failure,
    );
    let (raw, table, _) = t.finish();
    let unit = checked_native_fixture(raw, &table);
    let alias = Observation {
        point: Point::CallEntered(ENTRY),
        marker: ObserveOwnership {
            id: MarkerId(100),
            site: unit.unit().tables.source_anchors[0].site,
            kind: Kind::Alias,
            token: Location::Value(view),
            owner: Location::Address(address),
        },
    };
    check(unit.unit(), &Plan::new(unit.unit(), vec![alias.clone()])).unwrap();
    let mut dynamic = unit.unit().clone();
    dynamic.tables.calls[0].callee = Callee::Indirect(view);
    let dynamic = checked_native_fixture(dynamic, &table);
    let local = native::LeafPlan::prepare_value(&dynamic, &table).unwrap();
    assert!(
        !local
            .observations
            .iter()
            .any(|row| row.marker.kind == Kind::Alias)
    );
    let program = native::LeafPlan::prepare_value_for_program(&dynamic, &table).unwrap();
    let mut aliases = program
        .observations
        .iter()
        .filter(|row| row.marker.kind == Kind::Alias);
    assert_eq!(aliases.next().unwrap().marker.token, Location::Value(view));
    assert!(aliases.next().is_none());
    let mut forged = unit.unit().clone();
    forged.slots[slot.index()].kind = SlotKind::Parameter(1);
    assert_eq!(
        check(&forged, &Plan::new(&forged, vec![alias])).unwrap_err(),
        Error::InvalidBinding(MarkerId(100))
    );
}

#[test]
fn ownership_trace_staged_return_keeps_original_token_and_refuses_forged_storage() {
    let mut t = Lir::<hirsel_lir::stage::Value>::new(&[(Passing::Take, l::BLK)], Some(l::BLK));
    t.builder.unit.header.origin = UnitOrigin::Library;
    let original = t.owned(l::BLK);
    t.param(original);
    let arena = t.plain(l::ARENA);
    t.builder.push(
        ENTRY,
        Inst::ArenaOpen {
            result: arena,
            budget: None,
        },
    );
    t.builder.ret(ENTRY, Some(original));
    let (raw, table, _) = t.finish();
    let before = checked_native_fixture(raw, &table);
    let input = native::LeafPlan::prepare_value(&before, &table).unwrap();
    let options = hirsel_rc::placement::Options {
        regions: false,
        noalloc: false,
    };
    let ordinary = hirsel_rc::placement::place(before.unit().clone(), &table, &options).unwrap();
    let (placed, plan, recipe) =
        hirsel_rc::placement::place_observed(before.unit().clone(), &table, &options, &input)
            .unwrap();
    assert_eq!(
        hirsel_codec::to_text(&placed),
        hirsel_codec::to_text(&ordinary)
    );
    check_transform(before.unit(), &input, &placed, &plan, Some(&recipe)).unwrap();
    let row = plan
        .observations
        .iter()
        .find(|row| matches!(row.point, Point::StagedReturn { .. }))
        .unwrap();
    let old = input
        .observations
        .iter()
        .find(|old| old.marker.id == row.marker.id)
        .unwrap();
    assert_eq!(row.marker, old.marker);
    let Point::StagedReturn { block, .. } = row.point else {
        unreachable!()
    };
    let mut forged = plan.clone();
    forged
        .observations
        .iter_mut()
        .find(|other| other.marker.id == row.marker.id)
        .unwrap()
        .point = Point::StagedReturn {
        block,
        address: AddressId(u32::MAX),
    };
    assert!(check(&placed, &forged).is_err());
    let mut omitted = plan.clone();
    omitted
        .observations
        .retain(|other| other.marker.id != row.marker.id);
    assert!(check_transform(before.unit(), &input, &placed, &omitted, Some(&recipe)).is_err());
}

#[test]
fn ownership_trace_compound_resource_covers_parent_children_and_rejects_a_parent_only_cut() {
    let (after, output, initial, _, _) = compound_drop_fixture_with_parent(false, true);
    assert_eq!(
        output.observations[0].marker,
        initial.observations[0].marker
    );
    let checked = check(after.unit(), &output).unwrap();
    let commits = checked.commits(after.unit(), MarkerId(0)).unwrap();
    assert!(
        commits.len() >= 9,
        "parent and both children need every failure completion"
    );
    let mut prefix = output.clone();
    let Point::Instruction { block, index, .. } = commits[0] else {
        unreachable!()
    };
    let Point::CompoundDrop {
        normal, index: cut, ..
    } = &mut prefix.observations[0].point
    else {
        unreachable!()
    };
    *normal = block;
    *cut = index + 1;
    assert!(check(after.unit(), &prefix).is_err());
    let Point::Instruction { block, index, .. } = commits[0] else {
        unreachable!()
    };
    let mut missing = after.into_raw();
    let physical = missing.blocks[block.index()].instructions.start + index;
    assert!(matches!(
        missing.instructions[physical as usize],
        Inst::FinalizeEnd { .. }
    ));
    missing.instructions[physical as usize] = Inst::FrameLeave;
    let missing_plan = Plan::new(&missing, output.observations);
    assert!(check(&missing, &missing_plan).is_err());
}

#[test]
fn ownership_trace_parallel_edge_cycles_preserve_logical_markers_and_stage_all_sources() {
    use hirsel_lir::stage::Ready;
    let mut t = Lir::<Ready>::new(&[(Passing::Take, l::BLK), (Passing::Take, l::BLK)], None);
    let a = t.owned(l::BLK);
    let b = t.owned(l::BLK);
    t.param(a);
    t.param(b);
    let x = t.owned(l::BLK);
    let y = t.owned(l::BLK);
    let body = t.builder.block(BlockKind::Body, &[x, y]);
    let left = t.owned(l::BLK);
    let right = t.owned(l::BLK);
    let done = t.builder.block(BlockKind::Body, &[left, right]);
    let entry = t.builder.edge(body, &[a, b]);
    t.builder.term(ENTRY, Terminator::Jump(entry));
    t.branch(body, (body, &[y, x]), (done, &[x, y]));
    t.release(done, left, &[right]);
    t.release(done, right, &[]);
    t.builder.ret(done, None);
    let (raw, table, _) = t.finish();
    let cycle = match raw.terminators[body.index()] {
        Terminator::Branch { yes, .. } => yes,
        _ => unreachable!(),
    };
    let unit = checked_native_fixture(raw, &table);
    let expected = native::LeafPlan::prepare(&unit, &table).unwrap();
    let original = expected.plan().clone();
    let relocated = native::LeafPlan::from_relocated(&unit, &table, &original).unwrap();
    assert_eq!(relocated.plan(), &original);
    let markers: Vec<_> = relocated.at_point(Point::Edge(cycle)).collect();
    assert_eq!(markers.len(), 4);
    assert!(
        markers[..2]
            .iter()
            .all(|marker| marker.owner.kind == u32::MAX && marker.token.kind == 0)
    );
    assert!(
        markers[2..]
            .iter()
            .all(|marker| marker.token.kind == u32::MAX && marker.owner.kind == 0)
    );
    assert_eq!(markers[0].token.index, y.0);
    assert_eq!(markers[1].token.index, x.0);
    assert_eq!(markers[2].owner.index, x.0);
    assert_eq!(markers[3].owner.index, y.0);
    let max = original
        .observations
        .iter()
        .map(|row| row.marker.id.0)
        .max()
        .unwrap();
    assert!(markers.iter().all(|marker| marker.id.0 > max));
    assert_eq!(
        markers
            .iter()
            .map(|marker| marker.id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    let mut missing = original.clone();
    let at = missing
        .observations
        .iter()
        .position(|row| row.point == Point::Edge(cycle))
        .unwrap();
    missing.observations.remove(at);
    assert!(native::LeafPlan::from_relocated(&unit, &table, &missing).is_err());
    let mut forged = original;
    let row = forged
        .observations
        .iter_mut()
        .find(|row| row.point == Point::Edge(cycle))
        .unwrap();
    row.marker.owner = row.marker.token;
    assert!(native::LeafPlan::from_relocated(&unit, &table, &forged).is_err());
}

#[test]
fn ownership_source_variant_carriers_never_create_a_shell_resource() {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep};
    use hirsel_lir::stage::Value;
    for variant in [0, 1] {
        let parameters: &[(Passing, hirsel_lir::layout_types::LayoutId)] = if variant == 0 {
            &[]
        } else {
            &[(Passing::Take, l::RES)]
        };
        let mut t = Lir::<Value>::new(parameters, Some(l::OPT));
        let carrier = &mut t.table.layouts[l::OPT.0 as usize];
        carrier.tag.as_mut().unwrap().variants[1].fields[0].layout = l::RES;
        carrier.copy = CopyRecipe::NotCopyable;
        carrier.drop = vec![DropStep::ActiveVariant {
            tag_offset: 0,
            form: hirsel_lir::layout_types::TagForm::Ordinal,
            variants: vec![(
                1,
                vec![DropStep::Child {
                    offset: 8,
                    layout: l::RES,
                }],
            )],
        }];
        let fields = if variant == 0 {
            Vec::new()
        } else {
            let token = t.owned(l::RES);
            t.param(token);
            vec![(0, token)]
        };
        let symbol = t.symbol(SymbolKind::Type);
        let placement = t.heap();
        let construct = t.construct_site(
            ConstructorKind::Variant {
                definition: symbol,
                variant,
            },
            l::OPT,
            &fields,
            placement,
        );
        t.builder.unit.tables.constructs[construct.index()].placement = None;
        let result = t.owned(l::OPT);
        let failure = t.fails();
        t.builder.push(
            ENTRY,
            Inst::Construct {
                result,
                construct,
                failure,
            },
        );
        t.builder.ret(ENTRY, Some(result));
        let (unit, table, _) = t.finish();
        let unit = checked_native_fixture(unit, &table);
        let plan = native::LeafPlan::prepare_value(&unit, &table).unwrap();
        check(unit.unit(), &plan).unwrap();
        assert_eq!(plan.observations.len(), if variant == 0 { 1 } else { 3 });
        assert!(
            plan.observations
                .iter()
                .all(|row| row.marker.kind == Kind::Move)
        );
        if let Some((field, token)) = fields.first() {
            let row = plan
                .observations
                .iter()
                .find(|row| row.marker.token == Location::Value(*token))
                .unwrap();
            assert_eq!(
                row.marker.owner,
                Location::Field {
                    aggregate: result,
                    field: *field
                }
            );
            let mut forged = plan.clone();
            forged
                .observations
                .iter_mut()
                .find(|at| at.marker.id == row.marker.id)
                .unwrap()
                .marker
                .owner = Location::Field {
                aggregate: result,
                field: 1,
            };
            assert!(check(unit.unit(), &forged).is_err());
        }
        let index = hirsel_lir::LayoutIndex::new(&table);
        let placement = hirsel_rc::placement::Options::default();
        let proofs = hirsel_rc::insert::Proofs::default();
        let reuse = hirsel_rc::reuse::Options::default();
        let finalizers = Default::default();
        let output = hirsel_rc::observed::lower(
            unit.into_raw(),
            &plan,
            &index,
            &hirsel_rc::observed::Options {
                placement: &placement,
                proofs: &proofs,
                elide: true,
                reuse: &reuse,
                finalizers: &finalizers,
            },
        )
        .unwrap();
        assert_eq!(output.recipes.len(), 10);
        assert_eq!(
            output
                .plan
                .observations
                .iter()
                .map(|row| &row.marker)
                .collect::<Vec<_>>(),
            plan.observations
                .iter()
                .map(|row| &row.marker)
                .collect::<Vec<_>>()
        );
        let ready = checked_native_fixture(output.unit, &table);
        assert!(
            native::LeafPlan::from_relocated(&ready, &table, &output.plan).is_err(),
            "native variants still need their active-arm transport proof"
        );
    }
}

#[test]
fn ownership_mutable_actual_formal_relation_survives_recipes_and_rejects_forgery() {
    use hirsel_lir::stage::Value;
    let mut t = Lir::<Value>::new(&[(Passing::Mut, l::RES)], None);
    let slot = t.slot(l::RES, SlotKind::Parameter(0), ROOT);
    let incoming = t.version_of(slot);
    t.param(incoming);
    let address = t.at(ENTRY, slot, &[], &[]);
    let version = t.version_of(slot);
    let normal = t.builder.block(BlockKind::Body, &[]);
    let failure = t.builder.propagate();
    t.call(
        ENTRY,
        &[(Passing::Mut, l::RES)],
        None,
        &[ArgumentValue::Address { address, version }],
        (normal, &[]),
        failure,
    );
    t.builder.ret(normal, None);
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table);
    let plan = native::LeafPlan::prepare_value(&unit, &table).unwrap();
    assert_eq!(plan.observations.len(), 1);
    let alias = &plan.observations[0];
    assert_eq!(alias.marker.kind, Kind::Alias);
    assert_eq!(alias.marker.token, Location::Address(address));
    assert_eq!(alias.marker.owner, Location::Parameter(0));
    let mut forged = plan.clone();
    forged.observations[0].marker.owner = Location::Parameter(1);
    assert!(check(unit.unit(), &forged).is_err());
    let empty = Plan::new(unit.unit(), vec![]);
    assert_eq!(
        check_transform(
            unit.unit(),
            &plan,
            unit.unit(),
            &empty,
            Some(&recipe(&plan, &empty))
        )
        .unwrap_err(),
        Error::MissingMarker(alias.marker.id)
    );
    let index = hirsel_lir::LayoutIndex::new(&table);
    let placement = hirsel_rc::placement::Options::default();
    let proofs = hirsel_rc::insert::Proofs::default();
    let reuse = hirsel_rc::reuse::Options::default();
    let finalizers = Default::default();
    let output = hirsel_rc::observed::lower(
        unit.into_raw(),
        &plan,
        &index,
        &hirsel_rc::observed::Options {
            placement: &placement,
            proofs: &proofs,
            elide: true,
            reuse: &reuse,
            finalizers: &finalizers,
        },
    )
    .unwrap();
    assert_eq!(output.recipes.len(), 10);
    assert_eq!(
        output.plan.observations[0].marker,
        plan.observations[0].marker
    );
    let ready = checked_native_fixture(output.unit, &table);
    let native = native::LeafPlan::from_relocated(&ready, &table, &output.plan).unwrap();
    let physical: Vec<_> = native
        .markers()
        .filter(|marker| marker.id == alias.marker.id)
        .collect();
    assert_eq!(physical.len(), 1);
    assert_eq!(physical[0].kind, 6);
    assert_eq!(
        physical[0].token,
        native::NativeLocation {
            kind: native::MUTABLE_PARAMETER,
            index: 0
        }
    );
}

#[test]
fn ownership_resource_selection_is_immutable_and_reproved_by_native_requests() {
    use hirsel_lir::stage::Value;
    for layout in [l::BLK, l::RES, l::OPT] {
        let mut t = Lir::<Value>::new(&[(Passing::Take, layout)], Some(layout));
        if layout == l::OPT {
            use hirsel_lir::layout_types::{CopyRecipe, DropStep, TagForm};
            let carrier = &mut t.table.layouts[layout.0 as usize];
            carrier.tag.as_mut().unwrap().variants[1].fields[0].layout = l::RES;
            carrier.copy = CopyRecipe::NotCopyable;
            carrier.drop = vec![DropStep::ActiveVariant {
                tag_offset: 0,
                form: TagForm::Ordinal,
                variants: vec![(
                    1,
                    vec![DropStep::Child {
                        offset: 8,
                        layout: l::RES,
                    }],
                )],
            }];
        }
        let value = t.owned(layout);
        t.param(value);
        t.builder.ret(ENTRY, Some(value));
        let (unit, table, _) = t.finish();
        let unit = checked_native_fixture(unit, &table);
        let owned = native::LeafPlan::prepare_value_for_program(&unit, &table).unwrap();
        assert_eq!(owned.selection, Selection::Owned);
        assert_eq!(owned.observations.len(), 2);
        let plan = native::LeafPlan::prepare_resources_value_for_program(&unit, &table).unwrap();
        assert_eq!(plan.selection, Selection::Resources);
        assert_eq!(
            plan.observations.len(),
            if layout != l::BLK { 2 } else { 0 }
        );
        let mut changed = plan.clone();
        changed.selection = Selection::Owned;
        assert_eq!(
            check_transform(
                unit.unit(),
                &plan,
                unit.unit(),
                &changed,
                Some(&recipe(&plan, &changed))
            )
            .unwrap_err(),
            Error::ChangedSelection
        );
        let decoded: Plan = hirsel_codec::from_text(&hirsel_codec::to_text(&plan)).unwrap();
        assert_eq!(plan, decoded);
        let index = hirsel_lir::LayoutIndex::new(&table);
        let placement = hirsel_rc::placement::Options::default();
        let proofs = hirsel_rc::insert::Proofs::default();
        let reuse = hirsel_rc::reuse::Options::default();
        let finalizers = Default::default();
        let output = hirsel_rc::observed::lower(
            unit.into_raw(),
            &plan,
            &index,
            &hirsel_rc::observed::Options {
                placement: &placement,
                proofs: &proofs,
                elide: true,
                reuse: &reuse,
                finalizers: &finalizers,
            },
        )
        .unwrap();
        assert_eq!(output.recipes.len(), 10);
        assert_eq!(output.plan.selection, Selection::Resources);
        let ready = checked_native_fixture(output.unit, &table);
        let plans =
            std::collections::BTreeMap::from([(ready.unit().digest(), output.plan.clone())]);
        let prepared = native::Request::Resources(&plans)
            .prepare(&[&ready], &table)
            .unwrap();
        for prepared in prepared.values() {
            for marker in prepared.markers() {
                assert_eq!(
                    prepared.optional_carrier(&table, marker.id),
                    layout == l::OPT
                );
            }
        }
        assert!(
            native::Request::Relocated(&plans)
                .prepare(&[&ready], &table)
                .is_err()
        );
        if layout != l::BLK {
            let mut missing = output.plan;
            missing.observations.clear();
            assert!(native::LeafPlan::from_relocated(&ready, &table, &missing).is_err());
        }
    }
}

#[test]
fn ownership_resource_selection_ignores_data_release_but_refuses_hidden_resource_payloads() {
    let mut t = Lir::<hirsel_lir::stage::Ready>::new(&[(Passing::Take, l::BLK)], None);
    let data = t.owned(l::BLK);
    t.param(data);
    let failure = t.fails();
    t.release_with(ENTRY, data, failure);
    t.builder.ret(ENTRY, None);
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table);
    let plan = Plan::selected(unit.unit(), Selection::Resources, vec![]);
    let prepared = native::LeafPlan::from_relocated(&unit, &table, &plan).unwrap();
    assert_eq!(prepared.markers().count(), 0);

    // Returning a counted Resource-bearing block is valid ordinary LIR. Its
    // empty diagnostic plan must still fail the independent coverage check.
    let mut t =
        Lir::<hirsel_lir::stage::Ready>::new(&[(Passing::Take, l::RESBLK)], Some(l::RESBLK));
    let resource = t.owned(l::RESBLK);
    t.param(resource);
    t.builder.ret(ENTRY, Some(resource));
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table);
    let missing = Plan::selected(unit.unit(), Selection::Resources, vec![]);
    assert!(
        native::LeafPlan::from_relocated(&unit, &table, &missing).is_err(),
        "a counted block can hide Resource descendants"
    );
}

fn release_cleanup_fixture() -> (
    LirUnit<hirsel_lir::stage::Placed>,
    Plan,
    hirsel_lir::layout_types::LayoutTable,
) {
    release_cleanup_fixture_with(l::BLK)
}

fn release_cleanup_fixture_with(
    data_layout: hirsel_lir::LayoutId,
) -> (
    LirUnit<hirsel_lir::stage::Placed>,
    Plan,
    hirsel_lir::layout_types::LayoutTable,
) {
    use hirsel_lir::stage::Placed;
    use support::Undo;
    let mut t = Lir::<Placed>::new(
        &[(Passing::Take, l::RES), (Passing::Take, data_layout)],
        None,
    );
    let resource = t.slot(l::RES, SlotKind::Parameter(0), ROOT);
    let data = t.slot(data_layout, SlotKind::Parameter(1), ROOT);
    let resource_version = t.version_of(resource);
    let data_version = t.version_of(data);
    t.param(resource_version);
    t.param(data_version);
    let undo = [Undo::Drop(data), Undo::Drop(resource)];
    t.at(ENTRY, resource, &[], &undo);
    let data_address = t.at(ENTRY, data, &[], &undo);
    let taken = t.read(ENTRY, data_address, data_version, ReadMode::Take, None);
    t.store(ENTRY, data_address, taken, data);
    let exit = t.cleanup_exit(&[Undo::Drop(data), Undo::Drop(resource)], None);
    t.builder.jump(ENTRY, exit, &[]);
    let (before, table, _) = t.finish();
    let before = checked_native_fixture(before, &table).into_raw();
    let code = before.blocks[exit.index()]
        .instructions
        .get(&before.instructions)
        .unwrap();
    let (index, address, anchor) = code
        .iter()
        .enumerate()
        .find_map(|(index, inst)| {
            if let Inst::Drop {
                address, anchor, ..
            } = *inst
                && before.addresses[address.index()].base == AddressBase::Slot(resource)
            {
                Some((index as u32, address, anchor))
            } else {
                None
            }
        })
        .unwrap();
    let input = Plan::new(
        &before,
        vec![Observation {
            point: Point::Instruction {
                block: exit,
                index,
                moment: Moment::Before,
            },
            marker: ObserveOwnership {
                id: MarkerId(71),
                site: before.tables.cleanup_anchors[anchor.index()].site,
                kind: Kind::Drop,
                token: Location::Address(address),
                owner: Location::Address(address),
            },
        }],
    );
    (before, input, table)
}

#[test]
fn ownership_trace_release_relocates_resource_drops_after_silent_cleanup_is_removed() {
    let (before, input, table) = release_cleanup_fixture();
    let ordinary = hirsel_rc::insert::insert(before.clone(), &table, &Default::default()).unwrap();
    let (after, output, recipes) =
        hirsel_rc::insert::insert_observed(before.clone(), &table, &Default::default(), &input)
            .unwrap();
    let after = checked_native_fixture(after, &table).into_raw();
    assert_eq!(
        hirsel_codec::to_text(&after),
        hirsel_codec::to_text(&ordinary)
    );
    assert_eq!(recipes.len(), 3);
    assert_ne!(input.observations[0].point, output.observations[0].point);
    assert_eq!(input.observations[0].marker, output.observations[0].marker);
    check_transform(
        &before,
        &input,
        &after,
        &output,
        Some(&recipe(&input, &output)),
    )
    .unwrap();
    let mut stale = output.clone();
    stale.observations[0].point = input.observations[0].point;
    assert!(check(&after, &stale).is_err());
    let missing = Plan::new(&after, vec![]);
    assert_eq!(
        check_transform(
            &before,
            &input,
            &after,
            &missing,
            Some(&recipe(&input, &missing))
        )
        .unwrap_err(),
        Error::MissingMarker(MarkerId(71))
    );
}

#[test]
fn ownership_trace_additional_cleanup_requires_an_introduced_release_and_the_same_resource() {
    for data_layout in [l::BLK, l::PAIR] {
        let (placed, input, table) = release_cleanup_fixture_with(data_layout);
        let before = checked_native_fixture(placed.clone().into_stage::<Rc>(), &table).into_raw();
        let mut input = input;
        input.unit = before.digest();
        let (after, output, _) = hirsel_rc::insert::insert_observed(
            placed,
            &table,
            &Default::default(),
            &Plan::new(
                &before.clone().into_stage::<hirsel_lir::stage::Placed>(),
                input.observations.clone(),
            ),
        )
        .unwrap();
        let after = checked_native_fixture(after, &table).into_raw();
        let mut triggers = Vec::new();
        let mut generated = None;
        for (block, info) in after.blocks.iter().enumerate() {
            for (index, instruction) in info
                .instructions
                .get(&after.instructions)
                .unwrap()
                .iter()
                .enumerate()
            {
                let point = Point::Instruction {
                    block: BlockId(block as u32),
                    index: index as u32,
                    moment: Moment::Before,
                };
                match *instruction {
                    Inst::Release { failure, .. } => {
                        triggers.push((point, failure));
                    }
                    Inst::Drop {
                        address,
                        failure,
                        if_live,
                        ..
                    } if after.addresses[address.index()].base == AddressBase::Slot(SlotId(1)) => {
                        triggers.push((
                            match point {
                                Point::Instruction { block, index, .. } => Point::Instruction {
                                    block,
                                    index,
                                    moment: if if_live {
                                        Moment::IfLive
                                    } else {
                                        Moment::Before
                                    },
                                },
                                _ => unreachable!(),
                            },
                            failure,
                        ));
                    }
                    Inst::Drop {
                        address, anchor, ..
                    } if after.tables.cleanup_anchors[anchor.index()].cleanup == STATE_RELEASE
                        && after.addresses[address.index()].base
                            == AddressBase::Slot(SlotId(0)) =>
                    {
                        generated = Some((
                            point,
                            address,
                            after.tables.cleanup_anchors[anchor.index()].site,
                        ));
                    }
                    _ => (),
                }
            }
        }
        let (point, token, site) = generated.unwrap();
        let Point::Instruction { block: cleanup, .. } = point else {
            unreachable!()
        };
        let trigger = triggers
            .into_iter()
            .find_map(|(point, failure)| {
                let edge = after.tables.failures[failure.index()].cleanup;
                (after.tables.edges[edge.index()].target == cleanup).then_some(point)
            })
            .unwrap();
        let extra = CleanupObservation {
            marker: MarkerId(71),
            point,
            token: Location::Address(token),
            site,
            trigger: CleanupOrigin {
                unit: after.digest(),
                point: trigger,
            },
        };
        assert_eq!(output.cleanups, vec![extra.clone()]);
        let transport = recipe(&input, &output);
        check_transform(&before, &input, &after, &output, Some(&transport)).unwrap();
        check_transform(
            &after,
            &output,
            &after,
            &output,
            Some(&recipe(&output, &output)),
        )
        .unwrap();
        let mut omitted = output.clone();
        omitted.cleanups.clear();
        assert!(
            check_transform(
                &after,
                &output,
                &after,
                &omitted,
                Some(&recipe(&output, &omitted))
            )
            .is_err()
        );
        let mut duplicated = output.clone();
        duplicated.cleanups.push(extra.clone());
        assert!(check(&after, &duplicated).is_err());
        let text = hirsel_codec::to_text(&output);
        assert_eq!(output, hirsel_codec::from_text::<Plan>(&text).unwrap());
        let mut missing = transport.clone();
        missing.cleanups.clear();
        assert!(check_transform(&before, &input, &after, &output, Some(&missing)).is_err());
        let mut wrong = output.clone();
        wrong.cleanups[0].marker = MarkerId(999);
        assert!(check(&after, &wrong).is_err());
        let mut wrong = output.clone();
        let data_address = after
            .addresses
            .iter()
            .position(|info| info.base == AddressBase::Slot(SlotId(1)))
            .unwrap();
        wrong.cleanups[0].token = Location::Address(AddressId(data_address as u32));
        assert!(check(&after, &wrong).is_err());
        let mut wrong = output.clone();
        wrong.cleanups[0].site.start += 1;
        assert!(check(&after, &wrong).is_err());
        let mut wrong = output.clone();
        wrong.cleanups[0].point = output.observations[0].point;
        assert!(check(&after, &wrong).is_err());
        let mut wrong = output.clone();
        wrong.cleanups[0].trigger.point = Point::Return(after.entry);
        assert!(check(&after, &wrong).is_err());
        let mut wrong = transport;
        wrong.cleanups[0].before = Some(extra);
        assert!(check_transform(&before, &input, &after, &output, Some(&wrong)).is_err());
    }
}

#[test]
fn ownership_trace_native_variant_field_locations_keep_the_checked_arm_and_declaration() {
    let mut t = Lir::<hirsel_lir::stage::Ready>::new(&[(Passing::Borrow, l::OPT)], None);
    let slot = t.slot(l::OPT, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let address = t.at(
        ENTRY,
        slot,
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        &[],
    );
    t.builder.ret(ENTRY, None);
    let (unit, table, _) = t.finish();
    let unit = checked_native_fixture(unit, &table);
    assert_eq!(
        native::projected_location(
            unit.unit(),
            &table,
            native::NativeLocation {
                kind: 7,
                index: address.0
            }
        )
        .unwrap(),
        Some(native::NativePath {
            root: native::NativeLocation {
                kind: 1,
                index: slot.0
            },
            fields: vec![native::NativeProjection::VariantField {
                variant: 1,
                field: 0
            }]
        })
    );
}

#[test]
fn ownership_trace_data_child_release_requires_its_exact_take_and_complete_cleanup_cut() {
    let (after, output, _, anchor, _) = compound_drop_fixture_with_children(false, true, l::BLK);
    let checked = check(after.unit(), &output).unwrap();
    let commits = checked.commits(after.unit(), MarkerId(0)).unwrap();
    assert_eq!(commits.len(), 3, "only the parent has a Resource finalizer");
    let Point::Instruction { block, index, .. } = commits[0] else {
        unreachable!()
    };
    let mut prefix = output.clone();
    let Point::CompoundDrop {
        normal, index: cut, ..
    } = &mut prefix.observations[0].point
    else {
        unreachable!()
    };
    *normal = block;
    *cut = index + 1;
    assert!(
        check(after.unit(), &prefix).is_err(),
        "a cut cannot omit the data child releases"
    );
    let raw = after.into_raw();
    let physical = raw
        .instructions
        .iter()
        .position(|inst| matches!(inst, Inst::Release { anchor: action, .. } if *action == anchor))
        .unwrap();
    for forged_anchor in [false, true] {
        let mut forged = raw.clone();
        if forged_anchor {
            let mut wrong = forged.tables.cleanup_anchors[anchor.index()].clone();
            wrong.cleanup += 1;
            let next = CleanupAnchorId(forged.tables.cleanup_anchors.len() as u32);
            forged.tables.cleanup_anchors.push(wrong);
            let Inst::Release { anchor, .. } = &mut forged.instructions[physical] else {
                unreachable!()
            };
            *anchor = next;
        } else {
            let Location::Address(parent) = output.observations[0].marker.token else {
                unreachable!()
            };
            let Inst::Read { address, .. } = &mut forged.instructions[physical - 1] else {
                unreachable!()
            };
            *address = parent;
        }
        let forged_plan = Plan::selected(&forged, output.selection, output.observations.clone());
        assert!(
            check(&forged, &forged_plan).is_err(),
            "forged child release accepted, anchor={forged_anchor}"
        );
    }
}

#[test]
fn ownership_trace_staged_return_covers_failed_region_exit_release() {
    let mut t = Lir::<hirsel_lir::stage::Value>::new(&[], Some(l::BLK));
    t.builder.unit.header.origin = UnitOrigin::Library;
    let arena = t.plain(l::ARENA);
    t.builder.push(
        ENTRY,
        Inst::ArenaOpen {
            result: arena,
            budget: None,
        },
    );
    let normal = t.builder.block(BlockKind::Body, &[]);
    let failure = t.builder.propagate();
    let original = t
        .call(ENTRY, &[], Some(l::BLK), &[], (normal, &[]), failure)
        .unwrap();
    t.builder.ret(normal, Some(original));
    let (raw, table, _) = t.finish();
    let before = checked_native_fixture(raw, &table);
    let input = native::LeafPlan::prepare_value(&before, &table).unwrap();
    let index = hirsel_lir::LayoutIndex::new(&table);
    let lowered = hirsel_rc::observed::lower(
        before.unit().clone(),
        &input,
        &index,
        &hirsel_rc::observed::Options {
            placement: &hirsel_rc::placement::Options {
                regions: false,
                noalloc: false,
            },
            proofs: &Default::default(),
            elide: true,
            reuse: &Default::default(),
            finalizers: &Default::default(),
        },
    )
    .unwrap();
    let ready = checked_native_fixture(lowered.unit, &table);
    let native = native::LeafPlan::from_relocated(&ready, &table, &lowered.plan).unwrap();
    let staged = lowered
        .plan
        .observations
        .iter()
        .find(|row| matches!(row.point, Point::StagedReturn { .. }))
        .unwrap();
    let commits = check(ready.unit(), &lowered.plan)
        .unwrap()
        .commits(ready.unit(), staged.marker.id)
        .unwrap();
    assert_eq!(
        commits.len(),
        5,
        "normal store/take/return and failure take/release"
    );
    let failure_take = commits[3];
    let failure_release = commits[4];
    assert_eq!(native.at_point(failure_take).count(), 1);
    let releases = native.at_point(failure_release).collect::<Vec<_>>();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].kind, 2);
    for corruption in ["version", "released value", "anchor"] {
        let mut forged = ready.unit().clone();
        let Point::Instruction { block, index, .. } = failure_take else {
            unreachable!()
        };
        let offset = forged.blocks[block.index()].instructions.start as usize + index as usize;
        match corruption {
            "version" => {
                let Inst::Read { reaching, .. } = &mut forged.instructions[offset] else {
                    unreachable!()
                };
                *reaching = original;
            }
            "released value" => {
                let Inst::Release { value, .. } = &mut forged.instructions[offset + 1] else {
                    unreachable!()
                };
                *value = original;
            }
            "anchor" => {
                let Inst::Release { anchor, .. } = forged.instructions[offset + 1] else {
                    unreachable!()
                };
                forged.tables.cleanup_anchors[anchor.index()].site.end += 1;
            }
            _ => unreachable!(),
        }
        let forged_plan = Plan {
            unit: forged.digest(),
            ..lowered.plan.clone()
        };
        assert_eq!(
            check(&forged, &forged_plan).unwrap_err(),
            Error::InvalidBinding(staged.marker.id),
            "{corruption}"
        );
    }
}

#[test]
fn ownership_trace_compound_drop_checks_exact_variant_staging_and_all_child_exits() {
    let (after, output, original, _, _) =
        compound_drop_fixture_with_layout(false, false, l::RES, l::OPT);
    let checked = check(after.unit(), &output).unwrap();
    assert_eq!(
        output.observations[0].marker,
        original.observations[0].marker
    );
    assert!(matches!(
        output.observations[0].point,
        Point::CompoundDrop {
            staged: Some(_),
            ..
        }
    ));
    let commits = checked.commits(after.unit(), MarkerId(0)).unwrap();
    let Point::CompoundDrop {
        staged: Some((_, storage)),
        anchor,
        ..
    } = output.observations[0].point
    else {
        unreachable!()
    };
    let slot = |mut address: AddressId| {
        for _ in 0..after.unit().addresses.len() {
            match after.unit().addresses[address.index()].base {
                AddressBase::Slot(slot) => return slot,
                AddressBase::Address(parent) => address = parent,
                _ => panic!("fixture cleanup has nonlocal storage"),
            }
        }
        panic!("fixture cleanup has a cyclic address")
    };
    let target_slot = slot(storage);
    let Point::CompoundDrop {
        block: entry,
        normal,
        ..
    } = output.observations[0].point
    else {
        unreachable!()
    };
    // Address preparation has its own alternative cleanup of the same slot.
    // Select this action's finalizer through its ordinary control-flow edges.
    let mut reachable = std::collections::BTreeSet::new();
    let mut pending = vec![entry];
    while let Some(block) = pending.pop() {
        if block == normal || !reachable.insert(block) {
            continue;
        }
        for edge in after.unit().terminators[block.index()].normal_edges(&after.unit().pools) {
            pending.push(after.unit().tables.edges[edge.index()].target);
        }
    }
    let groups = reachable
        .iter()
        .flat_map(|block| {
            after.unit().blocks[block.index()]
                .instructions
                .get(&after.unit().instructions)
                .unwrap()
                .iter()
                .filter_map(move |instruction| match instruction {
                    Inst::FinalizeBegin {
                        address,
                        anchor: action,
                    } if slot(*address) == target_slot && *action == anchor => {
                        Some((*block, *address))
                    }
                    _ => None,
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        groups.len(),
        1,
        "one finalizer for the active Resource child"
    );
    let (call_block, child) = groups[0];
    let Terminator::Call {
        normal: Some(success),
        failure,
        ..
    } = after.unit().terminators[call_block.index()]
    else {
        unreachable!()
    };
    let code = after.unit().blocks[call_block.index()]
        .instructions
        .get(&after.unit().instructions)
        .unwrap();
    let guards = code
        .iter()
        .filter_map(|instruction| match instruction {
            Inst::FrameEnter { failure } => Some(*failure),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(guards.len(), 1);
    let targets = [
        success,
        after.unit().tables.failures[failure.index()].cleanup,
        after.unit().tables.failures[guards[0].index()].cleanup,
    ];
    let completions = targets.map(|edge| {
        let block = after.unit().tables.edges[edge.index()].target;
        let code = after.unit().blocks[block.index()]
            .instructions
            .get(&after.unit().instructions)
            .unwrap();
        let ends = code
            .iter()
            .enumerate()
            .filter_map(|(index, instruction)| {
                matches!(instruction, Inst::FinalizeEnd { address, anchor: action }
                if *address == child && *action == anchor)
                .then_some(index)
            })
            .collect::<Vec<_>>();
        assert_eq!(ends.len(), 1);
        Point::Instruction {
            block,
            index: ends[0] as u32,
            moment: Moment::After,
        }
    });
    assert_eq!(
        commits.len(),
        3,
        "normal, call failure and FrameEnter rejection"
    );
    assert!(completions.iter().all(|point| commits.contains(point)));
    for field in 0..3 {
        let mut forged = output.clone();
        let Location::Address(original) = forged.observations[0].marker.token else {
            unreachable!()
        };
        let Point::CompoundDrop {
            anchor,
            staged: Some((at, storage)),
            ..
        } = &mut forged.observations[0].point
        else {
            unreachable!()
        };
        match field {
            0 => *at += 1,
            1 => {
                *storage = original;
            }
            _ => *anchor = CleanupAnchorId(u32::MAX),
        }
        assert!(check(after.unit(), &forged).is_err());
    }
    for wrong_tag in [false, true] {
        let mut forged = after.unit().clone();
        if wrong_tag {
            let mut address = child;
            let projection = loop {
                let info = &forged.addresses[address.index()];
                if let Some(offset) = info
                    .projection
                    .get(&forged.pools.projections)
                    .unwrap()
                    .iter()
                    .position(|step| matches!(step, ProjectionStep::VariantField { .. }))
                {
                    break &mut forged.pools.projections[(info.projection.start as usize) + offset];
                }
                let AddressBase::Address(parent) = info.base else {
                    panic!("child has no variant field")
                };
                address = parent;
            };
            let ProjectionStep::VariantField { variant, .. } = projection else {
                unreachable!()
            };
            *variant = 0;
        } else {
            let Location::Address(original) = output.observations[0].marker.token else {
                unreachable!()
            };
            let switch = forged
                .terminators
                .iter()
                .position(|term| matches!(term, Terminator::Switch { .. }))
                .unwrap();
            let code = forged.blocks[switch].instructions;
            let Inst::Read {
                address,
                mode: ReadMode::Borrow,
                ..
            } = &mut forged.instructions[(code.start + code.len - 1) as usize]
            else {
                unreachable!()
            };
            *address = original;
        }
        let mut stale = output.clone();
        stale.unit = forged.digest();
        assert!(
            check(&forged, &stale).is_err(),
            "wrong selected root or tag: {wrong_tag}"
        );
    }
    let mut missing = after.into_raw();
    let Point::Instruction { block, index, .. } = completions[0] else {
        unreachable!()
    };
    let offset = missing.blocks[block.index()].instructions.start + index;
    missing.instructions[offset as usize] = Inst::FrameLeave;
    let mut stale = output;
    stale.unit = missing.digest();
    assert!(check(&missing, &stale).is_err());
}

#[test]
fn erased_cleanup_drop_preserves_its_marker_and_requires_an_empty_jump_boundary() {
    let (mut before, _) = fixture();
    let address = AddressId(0);
    let block = BlockId(before.blocks.len() as u32);
    let anchor = CleanupAnchorId(before.tables.cleanup_anchors.len() as u32);
    let site = before.tables.source_anchors[0].site;
    before.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: 0,
        action: 0,
        site,
    });
    let mut info = before.blocks[ENTRY.index()].clone();
    info.parameters = PoolRange::EMPTY;
    info.instructions = PoolRange::push(
        &mut before.instructions,
        [Inst::Drop {
            address,
            if_live: false,
            anchor,
            failure: FailureId(0),
        }],
    );
    before.blocks.push(info);
    let edge = EdgeId(before.tables.edges.len() as u32);
    before.tables.edges.push(Edge {
        target: ENTRY,
        arguments: PoolRange::EMPTY,
    });
    before.terminators.push(Terminator::Jump(edge));
    let input = Plan::new(
        &before,
        vec![Observation {
            marker: ObserveOwnership {
                id: MarkerId(0),
                site,
                kind: Kind::Drop,
                token: Location::Address(address),
                owner: Location::Address(address),
            },
            point: Point::Instruction {
                block,
                index: 0,
                moment: Moment::Before,
            },
        }],
    );
    check(&before, &input).unwrap();
    let mut after = before.clone();
    after.blocks[block.index()].instructions = PoolRange::EMPTY;
    let mut output = Plan::new(&after, input.observations.clone());
    output.observations[0].point = Point::EmptyDrop {
        anchor: (&before.tables.cleanup_anchors[anchor.index()]).into(),
        block,
    };
    // The ordinary anchor table contains no action after its drop is erased.
    after.tables.cleanup_anchors.clear();
    output.unit = after.digest();
    let checked = check_transform(
        &before,
        &input,
        &after,
        &output,
        Some(&recipe(&input, &output)),
    )
    .unwrap();
    assert_eq!(
        checked.commits(&after, MarkerId(0)).unwrap(),
        vec![output.observations[0].point]
    );
    let mut nonempty = after.clone();
    nonempty.blocks[block.index()].instructions = before.blocks[block.index()].instructions;
    let nonempty_plan = Plan::new(&nonempty, output.observations.clone());
    assert!(matches!(
        check(&nonempty, &nonempty_plan),
        Err(Error::InvalidBinding(_))
    ));
    let mut returning = after.clone();
    returning.terminators[block.index()] = Terminator::Return(None);
    let returning_plan = Plan::new(&returning, output.observations.clone());
    assert!(matches!(
        check(&returning, &returning_plan),
        Err(Error::InvalidBinding(_))
    ));
    let mut forged = output.clone();
    forged.observations[0].marker.kind = Kind::Create;
    assert!(matches!(
        check(&after, &forged),
        Err(Error::InvalidBinding(_))
    ));
    let mut wrong_action = output.clone();
    if let Point::EmptyDrop { anchor, .. } = &mut wrong_action.observations[0].point {
        anchor.action += 1;
    }
    assert!(matches!(
        check_transform(
            &before,
            &input,
            &after,
            &wrong_action,
            Some(&recipe(&input, &wrong_action))
        ),
        Err(Error::InvalidTransport(_))
    ));
    let missing = Plan::new(&after, vec![]);
    assert_eq!(
        check_transform(
            &before,
            &input,
            &after,
            &missing,
            Some(&recipe(&input, &missing))
        )
        .unwrap_err(),
        Error::MissingMarker(MarkerId(0))
    );
}

#[test]
fn unreachable_markers_keep_history_but_cannot_hide_live_or_failure_reachable_transitions() {
    let (after, plan) = fixture();
    let mut row = plan.observations.last().unwrap().clone();
    let Location::Value(result) = row.marker.token else {
        panic!()
    };
    let mut before = after.clone();
    let dead = BlockId(before.blocks.len() as u32);
    let mut block = before.blocks[ENTRY.index()].clone();
    block.parameters = PoolRange::EMPTY;
    block.instructions = PoolRange::EMPTY;
    before.blocks.push(block);
    before.terminators.push(Terminator::Return(Some(result)));
    row.point = Point::Return(dead);
    let input = Plan::new(&before, vec![row.clone()]);
    row.point = Point::Unreachable {
        origin: before.digest(),
        marker: row.marker.id,
        site: row.marker.site,
    };
    let output = Plan::new(&after, vec![row]);
    let checked = check_transform(
        &before,
        &input,
        &after,
        &output,
        Some(&recipe(&input, &output)),
    )
    .unwrap();
    assert!(
        checked
            .commits(&after, output.observations[0].marker.id)
            .unwrap()
            .is_empty()
    );
    // This history can cross an identity boundary, but can never revive.
    check_transform(
        &after,
        &output,
        &after,
        &output,
        Some(&recipe(&output, &output)),
    )
    .unwrap();
    let mut rewritten = output.clone();
    if let Point::Unreachable { origin, .. } = &mut rewritten.observations[0].point {
        *origin = after.digest();
    }
    let mut changed_after = after.clone();
    changed_after.header.key =
        hirsel_lir::objects::UnitKey(hirsel_syntax::ContentHash::of_bytes(b"next pass"));
    rewritten.unit = changed_after.digest();
    assert!(matches!(
        check_transform(
            &after,
            &output,
            &changed_after,
            &rewritten,
            Some(&recipe(&output, &rewritten))
        ),
        Err(Error::InvalidTransport(_))
    ));
    let mut revived = output.clone();
    revived.observations[0].point = Point::Return(ENTRY);
    assert!(matches!(
        check_transform(
            &after,
            &output,
            &after,
            &revived,
            Some(&recipe(&output, &revived))
        ),
        Err(Error::InvalidTransport(_))
    ));
    for through_failure in [false, true] {
        let mut live = before.clone();
        let edge = EdgeId(live.tables.edges.len() as u32);
        live.tables.edges.push(Edge {
            target: dead,
            arguments: PoolRange::EMPTY,
        });
        if through_failure {
            let failure = live.blocks[ENTRY.index()]
                .instructions
                .get(&live.instructions)
                .unwrap()
                .iter()
                .find_map(Inst::failure)
                .unwrap();
            live.tables.failures[failure.index()].cleanup = edge;
        } else {
            live.terminators[ENTRY.index()] = Terminator::Jump(edge);
        }
        let input = Plan::new(&live, input.observations.clone());
        let mut forged = output.clone();
        if let Point::Unreachable { origin, .. } = &mut forged.observations[0].point {
            *origin = live.digest();
        }
        assert!(
            matches!(
                check_transform(
                    &live,
                    &input,
                    &after,
                    &forged,
                    Some(&recipe(&input, &forged))
                ),
                Err(Error::InvalidTransport(_))
            ),
            "failure={through_failure}"
        );
    }
    let missing = Plan::new(&after, vec![]);
    assert!(matches!(
        check_transform(
            &before,
            &input,
            &after,
            &missing,
            Some(&recipe(&input, &missing))
        ),
        Err(Error::MissingMarker(_))
    ));
    let mut forged = output.clone();
    let mut extra = forged.observations[0].clone();
    extra.marker.id = MarkerId(999);
    if let Point::Unreachable { marker, .. } = &mut extra.point {
        *marker = extra.marker.id;
    }
    forged.observations.push(extra);
    assert!(matches!(
        check_transform(
            &before,
            &input,
            &after,
            &forged,
            Some(&recipe(&input, &forged))
        ),
        Err(Error::ForgedRelocation(_)) | Err(Error::ForgedMarker(_))
    ));
}

#[test]
fn only_an_exact_empty_drop_can_eliminate_its_failure_cleanup_observation() {
    let (mut before, old) = fixture();
    let mut retired = old.observations.last().unwrap().clone();
    let Location::Value(result) = retired.marker.token else {
        panic!()
    };
    let site = retired.marker.site;
    let anchor = CleanupAnchorId(before.tables.cleanup_anchors.len() as u32);
    before.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: 0,
        action: 42,
        site,
    });
    let mut block = before.blocks[ENTRY.index()].clone();
    block.parameters = PoolRange::EMPTY;
    block.instructions = PoolRange::EMPTY;
    let empty = BlockId(before.blocks.len() as u32);
    let normal = BlockId(empty.0 + 1);
    let failed = BlockId(empty.0 + 2);
    before.blocks.extend([block.clone(), block.clone(), block]);
    let leave = EdgeId(before.tables.edges.len() as u32);
    before.tables.edges.push(Edge {
        target: normal,
        arguments: PoolRange::EMPTY,
    });
    let enter = EdgeId(before.tables.edges.len() as u32);
    before.tables.edges.push(Edge {
        target: empty,
        arguments: PoolRange::EMPTY,
    });
    let failed_edge = EdgeId(before.tables.edges.len() as u32);
    before.tables.edges.push(Edge {
        target: failed,
        arguments: PoolRange::EMPTY,
    });
    before.terminators[ENTRY.index()] = Terminator::Jump(enter);
    before.terminators.extend([
        Terminator::Jump(leave),
        Terminator::Return(Some(result)),
        Terminator::Return(Some(result)),
    ]);
    let failure = FailureId(before.tables.failures.len() as u32);
    let mut failure_info = before.tables.failures[0].clone();
    failure_info.cleanup = failed_edge;
    before.tables.failures.push(failure_info);
    let original = before.blocks[ENTRY.index()]
        .instructions
        .get(&before.instructions)
        .unwrap()
        .to_vec();
    let index = original.len() as u32;
    let code = original.iter().cloned().chain([Inst::Drop {
        address: AddressId(0),
        if_live: true,
        anchor,
        failure,
    }]);
    before.blocks[ENTRY.index()].instructions = PoolRange::push(&mut before.instructions, code);
    retired.point = Point::Return(failed);
    let drop = Observation {
        marker: ObserveOwnership {
            id: MarkerId(998),
            site,
            kind: Kind::Drop,
            token: Location::Address(AddressId(0)),
            owner: Location::Address(AddressId(0)),
        },
        point: Point::Instruction {
            block: ENTRY,
            index,
            moment: Moment::IfLive,
        },
    };
    let input = Plan::new(&before, vec![drop, retired]);
    let mut after = before.clone();
    after.blocks[ENTRY.index()].instructions = PoolRange::push(&mut after.instructions, original);
    after.blocks.pop();
    after.terminators.pop();
    let mut rows = input.observations.clone();
    rows[0].point = Point::EmptyDrop {
        anchor: (&before.tables.cleanup_anchors[anchor.index()]).into(),
        block: empty,
    };
    rows[1].point = Point::Unreachable {
        origin: before.digest(),
        marker: rows[1].marker.id,
        site,
    };
    let output = Plan::new(&after, rows);
    check_transform(
        &before,
        &input,
        &after,
        &output,
        Some(&recipe(&input, &output)),
    )
    .unwrap();
    let mut shared_failure = before.clone();
    let other = shared_failure.blocks[ENTRY.index()]
        .instructions
        .get(&shared_failure.instructions)
        .unwrap()
        .iter()
        .find_map(Inst::failure)
        .unwrap();
    shared_failure.tables.failures[other.index()].cleanup = failed_edge;
    let input = Plan::new(&shared_failure, input.observations.clone());
    let mut output = output.clone();
    if let Point::Unreachable { origin, .. } = &mut output.observations[1].point {
        *origin = shared_failure.digest();
    }
    assert!(matches!(
        check_transform(
            &shared_failure,
            &input,
            &after,
            &output,
            Some(&recipe(&input, &output))
        ),
        Err(Error::InvalidTransport(_))
    ));
}

#[test]
fn ownership_trace_uses_the_original_guard_selection_at_the_placement_boundary() {
    use hirsel_lir::stage::Value;
    let mut t = Lir::<Value>::new(&[], None);
    t.builder.unit.header.origin = UnitOrigin::Library;
    let arena = t.plain(l::ARENA);
    t.builder.push(
        ENTRY,
        Inst::ArenaOpen {
            result: arena,
            budget: None,
        },
    );
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
    let original: hirsel_rc::seal_demand::UnitDemand = serde_json::from_value(serde_json::json!({
        "owner": [unit.header.definition, unit.header.instance],
        "original_values": unit.values.len(), "values": [arena.0],
    }))
    .unwrap();
    let plan = Plan::new(&unit, vec![]);
    let options = hirsel_rc::observed::Options {
        placement: &Default::default(),
        proofs: &Default::default(),
        elide: true,
        reuse: &Default::default(),
        finalizers: &Default::default(),
    };
    let plain =
        hirsel_rc::observed::lower_validated(unit.clone(), &plan, &inputs, &options).unwrap();
    assert_eq!(plain.unit.tables.regions.len(), 1);
    assert!(!plain.unit.tables.regions[0].seal_demand);
    let selected = hirsel_rc::observed::lower_validated_with_guard(
        unit.clone(),
        &plan,
        &inputs,
        &options,
        Some(&original),
    )
    .unwrap();
    assert!(selected.unit.tables.regions[0].seal_demand);
    assert_eq!(selected.recipes.len(), 10);
    assert_eq!(selected.recipes[0].before, plan.unit);
    assert_eq!(selected.recipes.last().unwrap().after, selected.plan.unit);
    check(&selected.unit, &selected.plan).unwrap();
    let mut other = unit;
    other.header.instance = support::hash("another guard owner");
    let other_plan = Plan::new(&other, vec![]);
    assert!(
        hirsel_rc::observed::lower_validated_with_guard(
            other,
            &other_plan,
            &inputs,
            &options,
            Some(&original),
        )
        .is_err()
    );
}
