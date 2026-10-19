//! Per-field drop flags: edge truth, guarded reads, currency and inline paths.
use crate::support;
use hirsel_core_contract::Passing;
use hirsel_lir::{ir::*, stage::Ready};
use support::*;

#[derive(Clone, Copy)]
enum Mutation {
    None,
    WrongArgument,
    Stale,
    WrongPath,
    WholeFlag,
    UnguardedRead,
}

/// An untouched variant is initialized, but its payload is not necessarily
/// active. A payload flag needs a tag observation on every incoming path.
enum TagObservation {
    EveryPath,
    NoPath,
    SomePaths,
}

fn variant_flag(observation: TagObservation) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[(Passing::Borrow, l::OPT)], None);
    let slot = t.slot(l::OPT, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let whole = t.at(ENTRY, slot, &[], &[]);
    let flag = t.flag(slot);
    let path = [ProjectionStep::VariantField {
        variant: 1,
        field: 0,
    }];
    t.builder.unit.tables.init_flags.last_mut().unwrap().path =
        PoolRange::push(&mut t.builder.unit.pools.projections, path);
    let joined = t.builder.block(BlockKind::Body, &[flag]);
    if !matches!(observation, TagObservation::NoPath) {
        let none = t.builder.block(BlockKind::Body, &[]);
        let some = t.builder.block(BlockKind::Body, &[]);
        let probe = if matches!(observation, TagObservation::SomePaths) {
            let probe = t.builder.block(BlockKind::Body, &[]);
            t.branch(ENTRY, (probe, &[]), (some, &[]));
            probe
        } else {
            ENTRY
        };
        let tag = t.read(probe, whole, version, ReadMode::Borrow, None);
        let invalid = t.builder.block(BlockKind::Body, &[]);
        t.builder.term(invalid, Terminator::Unreachable);
        let no = t.bool_const(none, false);
        let yes = t.bool_const(some, true);
        t.builder.jump(none, joined, &[no]);
        t.builder.jump(some, joined, &[yes]);
        let arms = [
            SwitchArm {
                value: 0,
                edge: t.builder.edge(none, &[]),
            },
            SwitchArm {
                value: 1,
                edge: t.builder.edge(some, &[]),
            },
        ];
        let arms = PoolRange::push(&mut t.builder.unit.pools.switch_arms, arms);
        let otherwise = t.builder.edge(invalid, &[]);
        t.builder.term(
            probe,
            Terminator::Switch {
                value: tag,
                arms,
                otherwise,
            },
        );
    } else {
        let yes = t.bool_const(ENTRY, true);
        t.builder.jump(ENTRY, joined, &[yes]);
    }
    let full = t.builder.block(BlockKind::Body, &[]);
    let done = t.builder.block(BlockKind::Body, &[]);
    t.branch_on(joined, flag, (full, &[]), (done, &[]));
    // Presence proved by a true flag survives forwarding before any address
    // preparation could independently re-establish the active variant.
    let forwarded = t.flag(slot);
    t.builder.unit.tables.init_flags.last_mut().unwrap().path =
        PoolRange::push(&mut t.builder.unit.pools.projections, path);
    let present = t.builder.block(BlockKind::Body, &[forwarded]);
    let yes = t.bool_const(full, true);
    t.builder.jump(full, present, &[yes]);
    let field = t.at(present, slot, &path, &[]);
    t.read(present, field, version, ReadMode::Bits, None);
    t.builder.jump(present, done, &[]);
    t.builder.ret(done, None);
    t.check()
}

#[test]
fn a_variant_payload_flag_requires_observed_presence() {
    assert_eq!(variant_flag(TagObservation::EveryPath), []);
    for observation in [TagObservation::NoPath, TagObservation::SomePaths] {
        let violations = variant_flag(observation);
        assert!(
            violations.iter().any(|v| matches!(v, K::InitFlag { .. })),
            "{violations:?}"
        );
    }
}

fn field_flag(mutation: Mutation) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[(Passing::Take, l::PAY)], None);
    let value = t.plain(l::PAY);
    t.param(value);
    let slot = t.slot(l::PAY, SlotKind::Parameter(0), ROOT);
    let version = t.builder.unit.parameters[0];
    let field = t.at(ENTRY, slot, &[ProjectionStep::Field(0)], &[]);
    let sibling = t.at(ENTRY, slot, &[ProjectionStep::Field(1)], &[]);
    let took = t.builder.block(BlockKind::Body, &[]);
    let kept = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (took, &[]), (kept, &[]));
    t.read(took, field, version, ReadMode::Take, None);
    let absent = t.bool_const(took, matches!(mutation, Mutation::WrongArgument));
    let present = t.bool_const(kept, true);
    let flag = t.flag(slot);
    let path = match mutation {
        Mutation::WholeFlag => vec![],
        Mutation::WrongPath => vec![ProjectionStep::Field(0), ProjectionStep::Field(0)],
        _ => vec![ProjectionStep::Field(0)],
    };
    t.builder.unit.tables.init_flags.last_mut().unwrap().path =
        PoolRange::push(&mut t.builder.unit.pools.projections, path);
    let join = t.builder.block(BlockKind::Body, &[flag]);
    t.builder.jump(took, join, &[absent]);
    t.builder.jump(kept, join, &[present]);
    // The parent remains live, with its independent sibling read after the join.
    t.read(join, sibling, version, ReadMode::Bits, None);
    if matches!(mutation, Mutation::UnguardedRead) {
        t.read(join, field, version, ReadMode::Bits, None);
    }
    let full = t.builder.block(BlockKind::Body, &[]);
    let empty = t.builder.block(BlockKind::Body, &[]);
    t.branch_on(join, flag, (full, &[]), (empty, &[]));
    t.read(full, field, version, ReadMode::Take, None);
    let forwarded = if matches!(mutation, Mutation::Stale) {
        flag
    } else {
        t.bool_const(full, false)
    };
    let after = t.flag(slot);
    t.builder.unit.tables.init_flags.last_mut().unwrap().path = PoolRange::push(
        &mut t.builder.unit.pools.projections,
        [ProjectionStep::Field(0)],
    );
    let end = t.builder.block(BlockKind::Body, &[after]);
    t.builder.jump(full, end, &[forwarded]);
    t.builder.jump(empty, end, &[flag]);
    t.read(end, sibling, version, ReadMode::Take, None);
    t.builder.push(
        end,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(end, None);
    t.check()
}

#[test]
fn field_flags_prove_presence_on_each_edge() {
    assert!(field_flag(Mutation::None).is_empty());
    for mutation in [
        Mutation::WrongArgument,
        Mutation::Stale,
        Mutation::WrongPath,
        Mutation::WholeFlag,
    ] {
        let violations = field_flag(mutation);
        assert!(
            violations.iter().any(|k| matches!(k, K::InitFlag { .. })),
            "{violations:?}"
        );
    }
}

#[test]
fn a_field_guard_permits_the_present_read_and_preserves_its_sibling() {
    assert!(field_flag(Mutation::None).is_empty());
    let violations = field_flag(Mutation::UnguardedRead);
    assert!(
        violations.iter().any(|k| matches!(k, K::MovedPart(_))),
        "{violations:?}"
    );
}

#[test]
fn explicit_finalization_is_a_ready_stage_primitive() {
    use hirsel_lir::{effect::Opcode, stage::Rc};
    let mut t = Lir::<Ready>::new(&[], None);
    let slot = t.slot(l::RES, SlotKind::Local, ROOT);
    let whole = t.at(ENTRY, slot, &[], &[]);
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let n = t.int(ENTRY);
    let symbol = t.symbol(SymbolKind::Type);
    let constructor =
        t.construct_site(ConstructorKind::Record(symbol), l::RES, &[(0, n)], t.heap());
    let value = t.owned(l::RES);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: value,
            construct: constructor,
            failure,
        },
    );
    t.store(ENTRY, whole, value, slot);
    let anchor = t.cleanup_anchor();
    t.builder.push(
        ENTRY,
        Inst::FinalizeBegin {
            address: whole,
            anchor,
        },
    );
    t.builder.push(
        ENTRY,
        Inst::FinalizeEnd {
            address: whole,
            anchor,
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
    let (unit, layouts, manifest) = t.finish();
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        discharge: None,
        operations: &(),
        target: &target,
        layouts: &layouts,
        manifest: &manifest,
    };
    let violations = hirsel_lir::validate::check(&unit, &inputs);
    assert!(violations.is_empty(), "{violations:?}");
    let violations = hirsel_lir::validate::check(&unit.into_stage::<Rc>(), &inputs);
    for op in [Opcode::FinalizeBegin, Opcode::FinalizeEnd] {
        assert!(
            violations
                .iter()
                .any(|v| v.kind == K::FinalizationStage(op)),
            "{violations:?}"
        );
    }
}

fn future_version(live: bool) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[], None);
    let slot = t.slot(l::I64, SlotKind::Local, ROOT);
    let whole = t.at(ENTRY, slot, &[], &[]);
    t.builder.push(ENTRY, Inst::StorageLive { slot });
    let first = t.int(ENTRY);
    let first = t.store(ENTRY, whole, first, slot);
    if !live {
        t.read(ENTRY, whole, first, ReadMode::Take, None);
    }
    let version = t.version_of(slot);
    let flag = t.flag(slot);
    let join = t.builder.block(BlockKind::Body, &[version, flag]);
    let write = t.builder.block(BlockKind::Body, &[]);
    let done = t.builder.block(BlockKind::Body, &[]);
    let value = t.int(write);
    let future = t.store(write, whole, value, slot);
    let empty = t.bool_const(ENTRY, live);
    let full = t.bool_const(write, true);
    t.builder.jump(ENTRY, join, &[future, empty]);
    t.builder.jump(write, join, &[future, full]);
    t.branch_on(join, flag, (done, &[]), (write, &[]));
    t.read(done, whole, version, ReadMode::Take, None);
    t.builder.push(
        done,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(done, None);
    t.check()
}

#[test]
fn a_future_version_marker_on_an_empty_edge_cannot_name_live_content() {
    pair(future_version, |k| matches!(k, K::VersionJoin { .. }));
}

/// The false input has finished field zero's finalizer but still owns its
/// sibling. The guard must recover only the true input's Live phase.
fn phase_flag(repeat: bool) -> Vec<K> {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep};
    let mut t = Lir::<Ready>::new(&[(Passing::Take, l::PAIR)], None);
    let record = &mut t.table.layouts[l::PAIR.0 as usize];
    for field in &mut record.fields {
        field.layout = l::RES;
    }
    record.scan.clear();
    record.copy = CopyRecipe::NotCopyable;
    record.drop = vec![
        DropStep::Child {
            offset: 8,
            layout: l::RES,
        },
        DropStep::Child {
            offset: 0,
            layout: l::RES,
        },
    ];
    let slot = t.slot(l::PAIR, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let first = t.at(ENTRY, slot, &[ProjectionStep::Field(0)], &[]);
    let sibling = t.at(ENTRY, slot, &[ProjectionStep::Field(1)], &[]);
    // These static field preparations cannot fail. Their required failure
    // references name an implementation fault, which has no source cleanup.
    let failures: Vec<_> = t
        .builder
        .unit
        .tables
        .failures
        .iter()
        .map(|f| t.builder.unit.tables.edges[f.cleanup.index()].target)
        .collect();
    for block in failures {
        t.builder.term(
            block,
            Terminator::InvariantFault {
                reason: "static field preparation failed".into(),
            },
        );
    }
    let finished = t.builder.block(BlockKind::Body, &[]);
    let live = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (finished, &[]), (live, &[]));
    let anchor = t.cleanup_anchor();
    t.builder.push(
        finished,
        Inst::FinalizeBegin {
            address: first,
            anchor,
        },
    );
    t.builder.push(
        finished,
        Inst::FinalizeEnd {
            address: first,
            anchor,
        },
    );
    let absent = t.bool_const(finished, false);
    let present = t.bool_const(live, true);
    let flag = t.flag(slot);
    t.builder.unit.tables.init_flags.last_mut().unwrap().path = PoolRange::push(
        &mut t.builder.unit.pools.projections,
        [ProjectionStep::Field(0)],
    );
    let joined = t.builder.block(BlockKind::Body, &[flag]);
    t.builder.jump(finished, joined, &[absent]);
    t.builder.jump(live, joined, &[present]);
    let yes = t.builder.block(BlockKind::Body, &[]);
    let no = t.builder.block(BlockKind::Body, &[]);
    let after = t.builder.block(BlockKind::Body, &[]);
    t.branch_on(joined, flag, (yes, &[]), (no, &[]));
    t.builder.push(
        yes,
        Inst::FinalizeBegin {
            address: first,
            anchor,
        },
    );
    t.builder.push(
        yes,
        Inst::FinalizeEnd {
            address: first,
            anchor,
        },
    );
    if repeat {
        t.builder.push(
            yes,
            Inst::FinalizeBegin {
                address: first,
                anchor,
            },
        );
    }
    t.builder.jump(yes, after, &[]);
    t.builder.jump(no, after, &[]);
    t.builder.push(
        after,
        Inst::FinalizeBegin {
            address: sibling,
            anchor,
        },
    );
    t.builder.push(
        after,
        Inst::FinalizeEnd {
            address: sibling,
            anchor,
        },
    );
    t.builder.push(
        after,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(after, None);
    t.check()
}

#[test]
fn a_true_field_guard_recovers_only_its_true_inputs_finalization_phases() {
    pair(phase_flag, |k| matches!(k, K::FinalizeState(_)));
}

fn forwarded_phase(repeat: bool) -> Vec<K> {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep, ScanStep};
    let mut t = Lir::<Ready>::new(&[(Passing::Take, l::RES)], None);
    let record = &mut t.table.layouts[l::RES.0 as usize];
    record.fields[0].layout = l::BLK;
    record.scan = vec![ScanStep::Counted { offset: 0 }];
    record.drop = vec![DropStep::Finalize, DropStep::Release { offset: 0 }];
    record.copy = CopyRecipe::NotCopyable;
    let slot = t.slot(l::RES, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    let whole = t.at(ENTRY, slot, &[], &[]);
    let child = t.at(ENTRY, slot, &[ProjectionStep::Field(0)], &[]);
    let failures: Vec<_> = t
        .builder
        .unit
        .tables
        .failures
        .iter()
        .map(|f| t.builder.unit.tables.edges[f.cleanup.index()].target)
        .collect();
    for block in failures {
        t.builder.term(
            block,
            Terminator::InvariantFault {
                reason: "static field preparation failed".into(),
            },
        );
    }
    let yes = t.bool_const(ENTRY, true);
    let flag = t.flag(slot);
    let joined = t.builder.block(BlockKind::Body, &[flag]);
    t.builder.jump(ENTRY, joined, &[yes]);
    let full = t.builder.block(BlockKind::Body, &[]);
    let empty = t.builder.block(BlockKind::Body, &[]);
    t.branch_on(joined, flag, (full, &[]), (empty, &[]));
    let anchor = t.cleanup_anchor();
    t.builder.push(
        full,
        Inst::FinalizeBegin {
            address: whole,
            anchor,
        },
    );
    t.builder.push(
        full,
        Inst::FinalizeEnd {
            address: whole,
            anchor,
        },
    );
    let forwarded = t.flag(slot);
    let after = t.builder.block(BlockKind::Body, &[forwarded]);
    t.builder.jump(full, after, &[flag]);
    let ready = t.builder.block(BlockKind::Body, &[]);
    t.branch_on(after, forwarded, (ready, &[]), (empty, &[]));
    if repeat {
        t.builder.push(
            ready,
            Inst::FinalizeBegin {
                address: whole,
                anchor,
            },
        );
    }
    let value = t.read(ready, child, version, ReadMode::Take, None);
    t.release(ready, value, &[]);
    t.builder.push(
        ready,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(ready, None);
    t.builder.push(
        empty,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    t.builder.ret(empty, None);
    t.check()
}

#[test]
fn a_forwarded_flag_keeps_the_finalization_phase_of_its_current_content() {
    pair(forwarded_phase, |k| matches!(k, K::FinalizeState(_)));
}
