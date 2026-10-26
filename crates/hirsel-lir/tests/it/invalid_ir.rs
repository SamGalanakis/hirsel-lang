//! Structural, flow and stage obligations: arity and types, SSA dominance,
//! edges and block kinds, addresses and versions, initialization, regions,
//! placement, vocabulary and the Ready discharge checks. One hand-built
//! mutation per obligation, each paired with its valid twin.
use crate::support;

use hirsel_core_contract::{Disposition, Passing, Unavailable};
use hirsel_lir::abi_types::CallableShape;
use hirsel_lir::effect::Opcode;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, ScalarKind};
use hirsel_lir::stage::{Placed, Rc, Ready, Reuse, Stage, Value};
use hirsel_lir::validate::{self, Inputs};
use support::*;

const TAKE_BLK: (Passing, LayoutId) = (Passing::Take, l::BLK);
const BORROW_BLK: (Passing, LayoutId) = (Passing::Borrow, l::BLK);

/// Borrow a parameter, materialize a credit and move it into a call.
fn dup_and_move<S: Stage>() -> Vec<K> {
    let mut t = Lir::<S>::new(&[BORROW_BLK], None);
    let x = t.plain(l::BLK);
    t.param(x);
    t.builder.push(ENTRY, Inst::Dup { value: x });
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.builder.propagate();
    t.call(
        ENTRY,
        &[TAKE_BLK],
        None,
        &[ArgumentValue::Value(x)],
        (n, &[]),
        fail,
    );
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn each_stage_admits_only_its_vocabulary() {
    pair(
        |mutant| {
            if mutant {
                dup_and_move::<Value>()
            } else {
                dup_and_move::<Rc>()
            }
        },
        |k| *k == K::Vocabulary(Opcode::Dup),
    );
    // Ready has lowered every semantic drop.
    pair(
        |mutant| {
            if mutant {
                drop_on_exit::<Ready>()
            } else {
                drop_on_exit::<Reuse>()
            }
        },
        |k| *k == K::Vocabulary(Opcode::Drop),
    );
}

/// A borrowed parameter's content copied out and returned.
fn copy_out<S: Stage>(layout: LayoutId) -> Vec<K> {
    let mut t = Lir::<S>::new(&[(Passing::Borrow, layout)], Some(layout));
    let s = t.slot(layout, SlotKind::Parameter(0), ROOT);
    let pv = t.version_of(s);
    t.param(pv);
    let a = t.at(ENTRY, s, &[], &[]);
    let result = t.owned_or_plain(layout);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Copy {
            result,
            address: a,
            reaching: pv,
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn ready_copies_only_a_dup_or_fields_recipe() {
    // A record of two counted fields is copied by the Ready primitive; a
    // plain copy has its own spelling, `Read Bits`.
    pair(
        |mutant| {
            if mutant {
                copy_out::<Ready>(l::I64)
            } else {
                copy_out::<Ready>(l::PAIR)
            }
        },
        |k| *k == K::Vocabulary(Opcode::Copy),
    );
    assert_eq!(copy_out::<Ready>(l::BLK), []);
    assert_eq!(copy_out::<Rc>(l::I64), []);
}

/// An owned parameter released whole at Ready.
fn release_whole(layout: LayoutId) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[(Passing::Take, layout)], None);
    let x = t.owned(layout);
    t.param(x);
    // Exercise release vocabulary at a valid cleanup anchor; a resource's
    // unrelated early-destruction rejection must not mask the finalizer rule.
    let cleanup = t.cleanup_exit(&[Undo::Release(x)], None);
    t.builder.jump(ENTRY, cleanup, &[]);
    t.check()
}

#[test]
fn an_inline_release_reaches_no_finalizer() {
    // A record of two counted fields is released inline by its drop program;
    // a resource with a finalizer is finalized explicitly, never released.
    pair(
        |mutant| release_whole(if mutant { l::RES } else { l::PAIR }),
        |k| *k == K::Type("released layout"),
    );
    assert_eq!(release_whole(l::SLICE), []);
    assert_eq!(release_whole(l::BLK), []);
    // A counted block owns its payload's finalization; this is distinct from
    // releasing an inline resource before its explicit finalizer.
    assert_eq!(release_whole(l::RESBLK), []);
}

#[test]
fn inline_release_does_not_finalize_borrowed_fields() {
    use hirsel_lir::layout_types::{CopyRecipe, DropStep, ScanStep};

    pair(
        |owned_resource| {
            let mut t = Lir::<Ready>::new(&[(Passing::Take, l::PAIR)], None);
            let record = &mut t.table.layouts[l::PAIR.0 as usize];
            record.fields[1].layout = l::RES;
            // Like a closure borrowing its scope and owning a capture:
            // only the counted field is destroyed with this value.
            record.scan = vec![ScanStep::Counted { offset: 0 }];
            record.drop = vec![DropStep::Release { offset: 0 }];
            if owned_resource {
                record.copy = CopyRecipe::NotCopyable;
                record.drop.insert(
                    0,
                    DropStep::Child {
                        offset: 8,
                        layout: l::RES,
                    },
                );
            }
            assert_eq!(
                hirsel_lir::layout::field_is_borrowed(
                    &t.table,
                    &t.table.layouts[l::PAIR.0 as usize],
                    &t.table.layouts[l::PAIR.0 as usize].fields[1],
                ),
                !owned_resource
            );
            let x = t.owned(l::PAIR);
            t.param(x);
            let cleanup = t.cleanup_exit(&[Undo::Release(x)], None);
            t.builder.jump(ENTRY, cleanup, &[]);
            t.check()
        },
        |k| *k == K::Type("released layout"),
    );
}

fn drop_on_exit<S: Stage>() -> Vec<K> {
    let mut t = Lir::<S>::new(&[TAKE_BLK], None);
    let s = t.slot(l::BLK, SlotKind::Parameter(0), ROOT);
    let pv = t.version_of(s);
    t.param(pv);
    let exit = t.cleanup_exit(&[Undo::Drop(s)], None);
    t.builder.jump(ENTRY, exit, &[]);
    t.check()
}

/// A value defined on one arm is used after the join unless it is passed.
fn dominance(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::I64));
    let p = (!mutant).then(|| t.plain(l::I64));
    let join = t.builder.block(BlockKind::Body, &Vec::from_iter(p));
    let yes = t.builder.block(BlockKind::Body, &[]);
    let no = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (yes, &[]), (no, &[]));
    let a = t.int(yes);
    let b = t.int(no);
    t.builder
        .jump(yes, join, &if mutant { vec![] } else { vec![a] });
    t.builder
        .jump(no, join, &if mutant { vec![] } else { vec![b] });
    t.builder.ret(join, Some(p.unwrap_or(a)));
    t.check()
}

#[test]
fn a_use_must_be_dominated_by_its_definition() {
    pair(dominance, |k| matches!(k, K::NotDominated(_)));
}

#[test]
fn a_value_defined_twice_or_never_is_rejected() {
    let mut t = Lir::<Rc>::new(&[], Some(l::I64));
    let a = t.int(ENTRY);
    let constant = ConstantId(0);
    t.builder.push(
        ENTRY,
        Inst::Const {
            result: a,
            constant,
        },
    );
    t.plain(l::I64);
    t.builder.ret(ENTRY, Some(a));
    let violations = t.check();
    assert!(violations.contains(&K::Redefined(a)), "{violations:?}");
    assert!(
        violations.contains(&K::Undefined(ValueId(1))),
        "{violations:?}"
    );
}

/// Block arguments match their parameters in number and type.
fn edge_arguments(mutant: u8) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::I64));
    let p = t.plain(l::I64);
    let join = t.builder.block(BlockKind::Body, &[p]);
    let a = t.int(ENTRY);
    let flag = t.boolean(ENTRY);
    match mutant {
        0 => t.builder.jump(ENTRY, join, &[a]),
        1 => t.builder.jump(ENTRY, join, &[]),
        _ => t.builder.jump(ENTRY, join, &[flag]),
    }
    t.builder.ret(join, Some(p));
    t.check()
}

#[test]
fn block_arguments_match_parameters_in_arity_and_type() {
    pair(
        |m| edge_arguments(u8::from(m)),
        |k| *k == K::Arity("edge arguments"),
    );
    pair(
        |m| edge_arguments(if m { 2 } else { 0 }),
        |k| *k == K::Type("edge argument"),
    );
}

/// Call arguments against the callee's declared ABI.
fn call_shape(mutant: u8) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.builder.propagate();
    let declared = [TAKE_BLK];
    let abi = t.abi(&declared, None);
    let symbol = t.symbol(SymbolKind::Function);
    let i = t.int(ENTRY);
    let (params, args): (Vec<_>, Vec<_>) = match mutant {
        0 => (vec![TAKE_BLK], vec![ArgumentValue::Value(x)]),
        1 => (vec![], vec![]),
        2 => (vec![(Passing::Take, l::I64)], vec![ArgumentValue::Value(i)]),
        _ => (vec![BORROW_BLK], vec![ArgumentValue::Value(x)]),
    };
    t.call_to(
        ENTRY,
        Callee::Direct(symbol),
        abi,
        &params,
        None,
        &args,
        (n, &[]),
        fail,
    );
    if mutant != 0 {
        t.release(n, x, &[]);
    }
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn call_arguments_match_the_callee_declaration() {
    pair(
        |m| call_shape(u8::from(m)),
        |k| *k == K::Arity("call arguments"),
    );
    pair(
        |m| call_shape(if m { 2 } else { 0 }),
        |k| *k == K::Type("call argument"),
    );
    pair(
        |m| call_shape(if m { 3 } else { 0 }),
        |k| *k == K::CallShape("argument passing"),
    );
}

/// `Unreachable` only as the otherwise target of an exhaustive tag switch.
fn unreachable(mutant: bool) -> Vec<K> {
    exhaustive_switch(l::OPT, mutant)
}

fn exhaustive_switch(variant: LayoutId, mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Borrow, variant)], None);
    let v = t.plain(variant);
    t.param(v);
    let none = t.builder.block(BlockKind::Body, &[]);
    let some = t.builder.block(BlockKind::Body, &[]);
    let never = t.builder.block(BlockKind::Body, &[]);
    t.builder.ret(none, None);
    t.builder.ret(some, None);
    t.builder.term(never, Terminator::Unreachable);
    let mut arms = vec![SwitchArm {
        value: 0,
        edge: t.builder.edge(none, &[]),
    }];
    if !mutant {
        arms.push(SwitchArm {
            value: 1,
            edge: t.builder.edge(some, &[]),
        });
    }
    let otherwise = t.builder.edge(never, &[]);
    let arms = PoolRange::push(&mut t.builder.unit.pools.switch_arms, arms);
    t.builder.term(
        ENTRY,
        Terminator::Switch {
            value: v,
            arms,
            otherwise,
        },
    );
    // Without its arm, `some` is unreachable in the mutant too.
    t.check()
        .into_iter()
        .filter(|k| !matches!(k, K::UnreachableBlock(_)))
        .collect()
}

#[test]
fn unreachable_needs_an_exhaustive_tag_switch() {
    pair(unreachable, |k| *k == K::UnprovedUnreachable);
}

/// The validators read a variant's ordinals, never its tag bytes: a
/// null-pointer niche switches on the same two ordinals as a u32 tag.
#[test]
fn a_niche_variant_switches_on_its_ordinals_like_a_tagged_one() {
    pair(
        |mutant| exhaustive_switch(l::NICHE, mutant),
        |k| *k == K::UnprovedUnreachable,
    );
}

/// An owned niche block's dataful field is a place behind the niche's tag
/// check; a variant the layout lacks is not.
fn niche_field(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::NICHEBLK)], None);
    let p = t.owned(l::NICHEBLK);
    t.param(p);
    let variant = if mutant { 2 } else { 1 };
    let at = t.address(
        AddressBase::Value(p),
        &[ProjectionStep::VariantField { variant, field: 1 }],
        l::I64,
    );
    t.prepare(ENTRY, at, &[p]);
    t.read(ENTRY, at, p, ReadMode::Bits, None);
    t.release(ENTRY, p, &[]);
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_niche_field_is_a_place_of_its_dataful_variant() {
    pair(niche_field, |k| matches!(k, K::AddressType(_)));
}

#[test]
fn unreachable_blocks_and_unused_edges_are_rejected() {
    let mut t = Lir::<Rc>::new(&[], None);
    let dead = t.builder.block(BlockKind::Body, &[]);
    t.builder.ret(ENTRY, None);
    t.builder.ret(dead, None);
    t.builder.edge(ENTRY, &[]);
    let violations = t.check();
    assert!(
        violations.contains(&K::EdgeUse(EdgeId(0))),
        "{violations:?}"
    );
    let mut t = Lir::<Rc>::new(&[], None);
    let dead = t.builder.block(BlockKind::Body, &[]);
    t.builder.ret(ENTRY, None);
    t.builder.ret(dead, None);
    assert_eq!(t.check(), vec![K::UnreachableBlock(dead)]);
}

/// Failure edges enter failure cleanup, which never resumes ordinary code.
fn failure_kinds(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], None);
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.builder.block(
        if mutant {
            BlockKind::Body
        } else {
            BlockKind::FailureCleanup
        },
        &[],
    );
    t.builder.term(
        fail,
        if mutant {
            Terminator::Return(None)
        } else {
            Terminator::Propagate
        },
    );
    t.call(ENTRY, &[], None, &[], (n, &[]), fail);
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn failure_edges_enter_failure_cleanup() {
    pair(failure_kinds, |k| {
        *k == K::BlockKind("failure path transfer")
    });
}

/// An address's declared type is the layout its projection reaches, and
/// every use is dominated by its preparation.
fn address(mutant: u8) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Borrow, l::PAIRBLK)], None);
    let u = t.plain(l::PAIRBLK);
    t.param(u);
    let layout = if mutant == 1 { l::I64 } else { l::BLK };
    let a = t.address(AddressBase::Value(u), &[ProjectionStep::Field(0)], layout);
    if mutant != 2 {
        t.prepare(ENTRY, a, &[]);
    }
    let r = t.plain(layout);
    t.builder.push(
        ENTRY,
        Inst::Read {
            result: r,
            address: a,
            reaching: u,
            mode: ReadMode::Borrow,
        },
    );
    t.builder.ret(ENTRY, None);
    // The borrowed view is plain here: no loan names it.
    t.check()
        .into_iter()
        .filter(|k| !matches!(k, K::Holding(_)))
        .collect()
}

#[test]
fn addresses_are_typed_by_their_projection_and_prepared_before_use() {
    pair(
        |m| address(u8::from(m)),
        |k| *k == K::AddressType(AddressId(0)),
    );
    pair(
        |m| address(if m { 2 } else { 0 }),
        |k| *k == K::AddressUnprepared(AddressId(0)),
    );
}

/// An origin event names a place without accessing it, so its address needs
/// no preparation (an index or tag check runs where the place is accessed);
/// a read of the same unprepared address is rejected.
fn origin_names(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Borrow, l::PAIRBLK)], None);
    let u = t.plain(l::PAIRBLK);
    t.param(u);
    let a = t.address(AddressBase::Value(u), &[ProjectionStep::Field(0)], l::BLK);
    if mutant {
        let r = t.plain(l::BLK);
        t.builder.push(
            ENTRY,
            Inst::Read {
                result: r,
                address: a,
                reaching: u,
                mode: ReadMode::Borrow,
            },
        );
    } else {
        t.builder.push(
            ENTRY,
            Inst::BindOrigin {
                source: a,
                place: a,
                kind: OriginKind::Alias,
            },
        );
        t.builder.push(ENTRY, Inst::EndOrigin { place: a });
    }
    t.builder.ret(ENTRY, None);
    t.check()
        .into_iter()
        .filter(|k| !matches!(k, K::Holding(_)))
        .collect()
}

#[test]
fn an_origin_event_names_an_unprepared_address() {
    pair(origin_names, |k| *k == K::AddressUnprepared(AddressId(0)));
}

/// The mutant reports only a stale prepared address: without the rule C
/// would accept it.
#[track_caller]
pub(crate) fn stale_address_only(case: impl Fn(bool) -> Vec<K>) {
    pair(&case, |k| matches!(k, K::StaleAddress { .. }));
    let mutant = case(true);
    assert!(
        mutant.iter().all(|k| matches!(k, K::StaleAddress { .. })),
        "the mutation fails another rule too: {mutant:#?}"
    );
}

/// A tag-checked `x.Some.0`, prepared under `x`'s first version, and a
/// replace of `x`. Reading through the address before the replace is
/// accepted; after it, the address may name another variant's bytes
/// (validator version 6).
fn variant_address_version(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[(Passing::Take, l::OPT), (Passing::Take, l::OPT)],
        Some(l::I64),
    );
    let s = t.slot(l::OPT, SlotKind::Parameter(0), ROOT);
    let first = t.version_of(s);
    t.param(first);
    let other = t.plain(l::OPT);
    t.param(other);
    let d = t.slot(l::OPT, SlotKind::Displaced, ROOT);
    let whole = t.at(ENTRY, s, &[], &[]);
    let field = t.at(
        ENTRY,
        s,
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        &[],
    );
    let early = (!mutant).then(|| t.read(ENTRY, field, first, ReadMode::Bits, None));
    t.builder.push(ENTRY, Inst::StorageLive { slot: d });
    let second = t.version_of(s);
    let displaced_version = t.version_of(d);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Replace {
            address: whole,
            value: other,
            displaced: d,
            version: second,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: d,
            if_live: false,
        },
    );
    let r = match early {
        Some(r) => r,
        None => t.read(ENTRY, field, second, ReadMode::Bits, None),
    };
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn a_dynamic_address_is_used_under_its_preparation_s_reaching_version() {
    stale_address_only(variant_address_version);
}

/// A write through the prepared `x.Some.0` itself lies below its tag check:
/// the variant stays, so the address stays placed under the new version.
#[test]
fn a_write_below_every_check_keeps_the_address_placed() {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::OPT)], Some(l::I64));
    let s = t.slot(l::OPT, SlotKind::Parameter(0), ROOT);
    let first = t.version_of(s);
    t.param(first);
    let field = t.at(
        ENTRY,
        s,
        &[ProjectionStep::VariantField {
            variant: 1,
            field: 0,
        }],
        &[],
    );
    t.read(ENTRY, field, first, ReadMode::Take, None);
    let n = t.int(ENTRY);
    let second = t.store(ENTRY, field, n, s);
    let r = t.read(ENTRY, field, second, ReadMode::Bits, None);
    t.builder.ret(ENTRY, Some(r));
    assert_eq!(t.check(), []);
}

/// `x.Payload.0` of a Box slot, prepared under `x`'s first version. The
/// replace moves the old block to a displaced slot, which is then dropped:
/// a read through the address after it would reach the released block.
fn payload_address_version(mutant: bool) -> Vec<K> {
    use hirsel_lir::layout_types::LayoutKind;
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::BLK), (Passing::Take, l::BLK)], None);
    t.table.layouts[l::BLK.0 as usize].kind = LayoutKind::Box { payload: l::PAY };
    let s = t.slot(l::BLK, SlotKind::Parameter(0), ROOT);
    let first = t.version_of(s);
    t.param(first);
    let other = t.owned(l::BLK);
    t.param(other);
    let d = t.slot(l::BLK, SlotKind::Displaced, ROOT);
    let drop_s = [Undo::Drop(s)];
    let before = [Undo::Drop(s), Undo::Release(other)];
    let whole = t.at(ENTRY, s, &[], &before);
    let field = t.at(
        ENTRY,
        s,
        &[ProjectionStep::Payload, ProjectionStep::Field(0)],
        &before,
    );
    let early = (!mutant).then(|| t.read(ENTRY, field, first, ReadMode::Bits, None));
    t.builder.push(ENTRY, Inst::StorageLive { slot: d });
    let second = t.version_of(s);
    let displaced_version = t.version_of(d);
    let failure = {
        let target = t.cleanup(&drop_s);
        t.failure(target)
    };
    t.builder.push(
        ENTRY,
        Inst::Replace {
            address: whole,
            value: other,
            displaced: d,
            version: second,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    let old = t.at(ENTRY, d, &[], &[Undo::Drop(d), Undo::Drop(s)]);
    let anchor = t.cleanup_anchor();
    let failure = {
        let target = t.cleanup(&drop_s);
        t.failure(target)
    };
    t.builder.push(
        ENTRY,
        Inst::Drop {
            address: old,
            if_live: false,
            anchor,
            failure,
        },
    );
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: d,
            if_live: false,
        },
    );
    if early.is_none() {
        t.read(ENTRY, field, second, ReadMode::Bits, None);
    }
    let exit = t.cleanup_exit(&drop_s, None);
    t.builder.jump(ENTRY, exit, &[]);
    t.check()
}

#[test]
fn a_payload_address_goes_stale_when_its_root_is_replaced() {
    stale_address_only(payload_address_version);
}

/// `x.Some.0` prepared before a branch whose one arm replaces `x`. After the
/// join `x` reaches through the version join, not the prepared version, so
/// the address must be prepared again there.
fn joined_address_version(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[(Passing::Take, l::OPT), (Passing::Take, l::OPT)],
        Some(l::I64),
    );
    let s = t.slot(l::OPT, SlotKind::Parameter(0), ROOT);
    let first = t.version_of(s);
    t.param(first);
    let other = t.plain(l::OPT);
    t.param(other);
    let d = t.slot(l::OPT, SlotKind::Displaced, ROOT);
    let whole = t.at(ENTRY, s, &[], &[]);
    let steps = [ProjectionStep::VariantField {
        variant: 1,
        field: 0,
    }];
    let field = t.at(ENTRY, s, &steps, &[]);
    let merged = t.version_of(s);
    let join = t.builder.block(BlockKind::Body, &[merged]);
    let yes = t.builder.block(BlockKind::Body, &[]);
    let no = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (yes, &[]), (no, &[]));
    t.builder.push(yes, Inst::StorageLive { slot: d });
    let second = t.version_of(s);
    let displaced_version = t.version_of(d);
    let failure = t.fails();
    t.builder.push(
        yes,
        Inst::Replace {
            address: whole,
            value: other,
            displaced: d,
            version: second,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    t.builder.push(
        yes,
        Inst::StorageDead {
            slot: d,
            if_live: false,
        },
    );
    t.builder.jump(yes, join, &[second]);
    t.builder.jump(no, join, &[first]);
    let at = if mutant {
        field
    } else {
        t.at(join, s, &steps, &[])
    };
    let r = t.read(join, at, merged, ReadMode::Bits, None);
    t.builder.ret(join, Some(r));
    t.check()
}

#[test]
fn a_store_on_one_branch_stales_an_address_prepared_before_the_join() {
    stale_address_only(joined_address_version);
}

/// `s[i]` of a library unit's `mut` sequence parameter, prepared under `s`'s
/// first version, then a `SeqReserve` (or `SeqSetLen`) of `s`. A reserve may
/// move the elements to a new block and release the old one, and a shorter
/// length leaves `i` past the count: a read through the old address reaches
/// a released block or an unset slot, so the twin prepares it again.
fn sequence_write_address(set_len: bool, mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[
            (Passing::Mut, l::SEQBLK),
            (Passing::Borrow, l::U64),
            (Passing::Borrow, l::U64),
        ],
        Some(l::I64),
    );
    t.builder.unit.header.origin = UnitOrigin::Library;
    let s = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let first = t.version_of(s);
    t.param(first);
    let n = t.plain(l::U64);
    t.param(n);
    let i = t.plain(l::U64);
    t.param(i);
    let whole = t.at(ENTRY, s, &[], &[]);
    let steps = [ProjectionStep::Index(i)];
    let element = t.at(ENTRY, s, &steps, &[]);
    t.read(ENTRY, element, first, ReadMode::Bits, None);
    let second = t.version_of(s);
    t.builder.push(
        ENTRY,
        if set_len {
            Inst::SeqSetLen {
                address: whole,
                len: n,
                version: second,
            }
        } else {
            Inst::SeqReserve {
                address: whole,
                capacity: n,
                version: second,
            }
        },
    );
    let at = if mutant {
        element
    } else {
        t.at(ENTRY, s, &steps, &[])
    };
    let r = t.read(ENTRY, at, second, ReadMode::Bits, None);
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn a_reserve_stales_an_element_address_prepared_before_it() {
    stale_address_only(|m| sequence_write_address(false, m));
}

#[test]
fn a_length_write_stales_an_element_address_prepared_before_it() {
    stale_address_only(|m| sequence_write_address(true, m));
}

/// A local slot stored, then replaced; reads name the reaching version.
fn reaching_version(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::I64));
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    let d = t.slot(l::I64, SlotKind::Displaced, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    t.builder.push(ENTRY, Inst::StorageLive { slot: d });
    let a = t.at(ENTRY, s, &[], &[]);
    let one = t.int(ENTRY);
    let first = t.store(ENTRY, a, one, s);
    let two = t.int(ENTRY);
    let second = t.version_of(s);
    let displaced_version = t.version_of(d);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Replace {
            address: a,
            value: two,
            displaced: d,
            version: second,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    let r = t.read(
        ENTRY,
        a,
        if mutant { first } else { second },
        ReadMode::Bits,
        None,
    );
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: d,
            if_live: false,
        },
    );
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn a_read_names_its_storage_s_reaching_version() {
    pair(reaching_version, |k| matches!(k, K::StaleVersion { .. }));
}

/// A replace's displaced content is read under the replace's
/// `displaced_version`; naming the version the slot held before (`first`, the
/// replaced slot's own) is stale. Before LIR schema revision 5 the displaced
/// slot had no reaching version at all, so no read of it could validate.
fn displaced_reaching_version(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::I64));
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    let d = t.slot(l::I64, SlotKind::Displaced, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    t.builder.push(ENTRY, Inst::StorageLive { slot: d });
    let a = t.at(ENTRY, s, &[], &[]);
    let ad = t.at(ENTRY, d, &[], &[]);
    let one = t.int(ENTRY);
    let first = t.store(ENTRY, a, one, s);
    let two = t.int(ENTRY);
    let second = t.version_of(s);
    let displaced_version = t.version_of(d);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Replace {
            address: a,
            value: two,
            displaced: d,
            version: second,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    let old = t.read(
        ENTRY,
        ad,
        if mutant { first } else { displaced_version },
        ReadMode::Take,
        None,
    );
    let new = t.read(ENTRY, a, second, ReadMode::Take, None);
    let sum = t.value(l::I64, Holding::Plain);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Binary {
            result: sum,
            op: BinaryOp::Add,
            left: old,
            right: new,
            failure,
        },
    );
    for slot in [d, s] {
        t.builder.push(
            ENTRY,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
    }
    t.builder.ret(ENTRY, Some(sum));
    t.check()
}

#[test]
fn a_read_of_displaced_content_names_the_displaced_version() {
    pair(displaced_reaching_version, |k| {
        matches!(k, K::StaleVersion { .. })
    });
}

/// The displaced version is typed at the displaced slot's type.
fn displaced_version_type(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], None);
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    let d = t.slot(l::I64, SlotKind::Displaced, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    t.builder.push(ENTRY, Inst::StorageLive { slot: d });
    let a = t.at(ENTRY, s, &[], &[]);
    let one = t.int(ENTRY);
    t.store(ENTRY, a, one, s);
    let two = t.int(ENTRY);
    let second = t.version_of(s);
    // A plain name of another type (a block's) names no content of `d`.
    let displaced_version = if mutant {
        t.value(l::BLK, Holding::Plain)
    } else {
        t.version_of(d)
    };
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Replace {
            address: a,
            value: two,
            displaced: d,
            version: second,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    for slot in [d, s] {
        t.builder.push(
            ENTRY,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
    }
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_displaced_version_is_a_plain_name_of_the_displaced_slot_s_type() {
    pair(displaced_version_type, |k| {
        *k == K::Type("displaced version")
    });
}

/// One arm replaces a slot's content; the join merges the reaching versions
/// in a block parameter. Passing the version the replace superseded is a
/// join argument that no longer reaches.
fn version_join(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::I64));
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    let d = t.slot(l::I64, SlotKind::Displaced, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[]);
    let zero = t.int(ENTRY);
    let v0 = t.store(ENTRY, a, zero, s);
    let merged = t.version_of(s);
    let join = t.builder.block(BlockKind::Body, &[merged]);
    let yes = t.builder.block(BlockKind::Body, &[]);
    let no = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (yes, &[]), (no, &[]));
    t.builder.push(yes, Inst::StorageLive { slot: d });
    let one = t.int(yes);
    let v1 = t.version_of(s);
    let displaced_version = t.version_of(d);
    let failure = t.fails();
    t.builder.push(
        yes,
        Inst::Replace {
            address: a,
            value: one,
            displaced: d,
            version: v1,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    t.builder.push(
        yes,
        Inst::StorageDead {
            slot: d,
            if_live: false,
        },
    );
    t.builder.jump(yes, join, &[if mutant { v0 } else { v1 }]);
    t.builder.jump(no, join, &[v0]);
    let r = t.read(join, a, merged, ReadMode::Bits, None);
    t.builder.push(
        join,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.builder.ret(join, Some(r));
    t.check()
}

#[test]
fn a_version_join_merges_the_versions_of_its_predecessors() {
    pair(version_join, |k| matches!(k, K::VersionJoin { .. }));
}

/// A loop with two entries: blocks A and B each replace a slot's content and
/// pass the new version along every edge. The analysis converges over the
/// irreducible cycle; the mutation passes a version that no longer reaches.
fn two_entry_loop(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::I64));
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    let d = t.slot(l::I64, SlotKind::Displaced, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[]);
    let zero = t.int(ENTRY);
    let v0 = t.store(ENTRY, a, zero, s);
    let pa = t.version_of(s);
    let pb = t.version_of(s);
    let pe = t.version_of(s);
    let block_a = t.builder.block(BlockKind::Body, &[pa]);
    let block_b = t.builder.block(BlockKind::Body, &[pb]);
    let exit = t.builder.block(BlockKind::Body, &[pe]);
    t.branch(ENTRY, (block_a, &[v0]), (block_b, &[v0]));
    let replace = |t: &mut Lir<Rc>, b: BlockId| {
        t.builder.push(b, Inst::StorageLive { slot: d });
        let value = t.int(b);
        let version = t.version_of(s);
        let displaced_version = t.version_of(d);
        let failure = t.fails();
        t.builder.push(
            b,
            Inst::Replace {
                address: a,
                value,
                displaced: d,
                version,
                displaced_version,
                if_live: false,
                failure,
            },
        );
        t.builder.push(
            b,
            Inst::StorageDead {
                slot: d,
                if_live: false,
            },
        );
        version
    };
    let va = replace(&mut t, block_a);
    let vb = replace(&mut t, block_b);
    t.branch(block_a, (block_b, &[va]), (exit, &[va]));
    t.branch(
        block_b,
        (block_a, &[if mutant { v0 } else { vb }]),
        (exit, &[vb]),
    );
    let r = t.read(exit, a, pe, ReadMode::Bits, None);
    t.builder.push(
        exit,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.builder.ret(exit, Some(r));
    t.check()
}

#[test]
fn analyses_converge_on_a_loop_with_two_entries() {
    pair(two_entry_loop, |k| matches!(k, K::VersionJoin { .. }));
}

/// Owned credits around the same two-entry loop: a block moved out on one
/// path through the cycle is a credit join mismatch at its entries.
fn two_entry_credits(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let block_a = t.builder.block(BlockKind::Body, &[]);
    let block_b = t.builder.block(BlockKind::Body, &[]);
    let exit = t.builder.block(BlockKind::Body, &[]);
    t.branch(ENTRY, (block_a, &[]), (block_b, &[]));
    if mutant {
        let y = t.new_block(block_a, &[Undo::Release(x)]);
        t.release(block_a, x, &[y]);
        t.release(block_a, y, &[]);
    }
    t.branch(block_a, (block_b, &[]), (exit, &[]));
    t.branch(block_b, (block_a, &[]), (exit, &[]));
    t.release(exit, x, &[]);
    t.builder.ret(exit, None);
    t.check()
}

#[test]
fn credits_must_agree_at_every_entry_of_a_loop_with_two_entries() {
    pair(two_entry_credits, |k| matches!(k, K::JoinCredits { .. }));
}

/// Storage is live and initialized before a read, and dead before a store.
fn initialization(mutant: u8) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::I64));
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    if mutant != 1 {
        t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    }
    let a = t.at(ENTRY, s, &[], &[]);
    let one = t.int(ENTRY);
    let mut v = t.store(ENTRY, a, one, s);
    if mutant == 3 {
        let two = t.int(ENTRY);
        v = t.store(ENTRY, a, two, s);
    }
    if mutant == 2 {
        // Moving the content out leaves the slot uninitialized.
        t.read(ENTRY, a, v, ReadMode::Take, None);
    }
    let r = t.read(ENTRY, a, v, ReadMode::Bits, None);
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.builder.ret(ENTRY, Some(r));
    t.check()
        .into_iter()
        .filter(|k| !matches!(k, K::StaleVersion { .. }))
        .collect()
}

#[test]
fn storage_is_live_and_initialized_before_a_read_and_dead_before_a_store() {
    pair(
        |m| initialization(u8::from(m)),
        |k| *k == K::DeadStorage(SlotId(0)),
    );
    pair(
        |m| initialization(if m { 2 } else { 0 }),
        |k| *k == K::Uninitialized(SlotId(0)),
    );
    pair(
        |m| initialization(if m { 3 } else { 0 }),
        |k| *k == K::StoreToLive(SlotId(0)),
    );
}

/// An owned block left in a local at exit leaks.
fn storage_leak(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[TAKE_BLK], None);
    let x = t.owned(l::BLK);
    t.param(x);
    let s = t.slot(l::BLK, SlotKind::Local, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[Undo::Release(x)]);
    t.store(ENTRY, a, x, s);
    if mutant {
        t.builder.ret(ENTRY, None);
    } else {
        let exit = t.cleanup_exit(&[Undo::Drop(s)], None);
        t.builder.jump(ENTRY, exit, &[]);
    }
    t.check()
}

#[test]
fn owned_storage_is_released_before_exit() {
    pair(storage_leak, |k| *k == K::LeakedStorage(SlotId(0)));
}

/// A borrowed parameter's storage is never moved out of.
fn borrowed_storage(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[BORROW_BLK], None);
    let s = t.slot(l::BLK, SlotKind::Parameter(0), ROOT);
    let pv = t.version_of(s);
    t.param(pv);
    let a = t.at(ENTRY, s, &[], &[]);
    let mode = if mutant {
        ReadMode::Take
    } else {
        ReadMode::Borrow
    };
    let loan = t.loan(a, false);
    t.begin(ENTRY, loan);
    let v = t.read(ENTRY, a, pv, mode, Some(loan));
    t.end(ENTRY, loan);
    if mutant {
        t.release(ENTRY, v, &[]);
    }
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_borrowed_parameter_s_storage_is_never_moved() {
    pair(borrowed_storage, |k| *k == K::BorrowedStorage(SlotId(0)));
}

/// Loans begin inactive, end active, cover what they lend and are closed at
/// every exit.
fn loans(mutant: u8) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::PAIRBLK)], None);
    let u = t.owned(l::PAIRBLK);
    t.param(u);
    let f0 = t.through(ENTRY, u, &[ProjectionStep::Field(0)], &[Undo::Release(u)]);
    let f1 = t.through(ENTRY, u, &[ProjectionStep::Field(1)], &[Undo::Release(u)]);
    let loan = t.loan(if mutant == 3 { f1 } else { f0 }, false);
    if mutant != 1 {
        t.begin(ENTRY, loan);
    }
    let w = t.read(ENTRY, f0, u, ReadMode::Borrow, Some(loan));
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.cleanup(&[Undo::End(loan), Undo::Release(u)]);
    t.call(
        ENTRY,
        &[BORROW_BLK],
        None,
        &[ArgumentValue::Value(w)],
        (n, &[]),
        fail,
    );
    if mutant != 2 {
        t.end(n, loan);
    }
    t.release(n, u, &[]);
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn loans_follow_their_state_machine_and_cover_their_views() {
    pair(|m| loans(u8::from(m)), |k| *k == K::LoanState(LoanId(0)));
    pair(
        |m| loans(if m { 2 } else { 0 }),
        |k| *k == K::LoanOpenAtExit(LoanId(0)),
    );
    pair(
        |m| loans(if m { 3 } else { 0 }),
        |k| *k == K::LoanCoverage(ValueId(1)),
    );
}

/// A compiler-local region: an uncounted construction is used only while its
/// region is entered.
fn region(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Placed>::new(&[], None);
    t.builder.unit.tables.regions.push(RegionInstance {
        scope: ROOT,
        anchor: SourceAnchorId(0),
        seal_demand: false,
    });
    let r = RegionInstanceId(0);
    let mark = t.owned(l::PTR);
    t.builder.push(
        ENTRY,
        Inst::RegionEnter {
            result: mark,
            region: r,
        },
    );
    let one = t.int(ENTRY);
    let two = t.int(ENTRY);
    let symbol = t.symbol(SymbolKind::Type);
    let site = t.construct_site(
        ConstructorKind::Record(symbol),
        l::PAY,
        &[(0, one), (1, two)],
        Some(Placement::Fixed(StorageOwner::LocalRegion(r))),
    );
    t.builder.unit.tables.constructs[site.index()].destination = DestinationClass::Region;
    let v = t.value_as(ValueRepr::Uncounted, l::BLK, Holding::Plain);
    let exit_failure = t.fails();
    let leave = |t: &mut Lir<Placed>, b: BlockId, failure| {
        t.builder.push(
            b,
            Inst::RegionExit {
                mark,
                region: r,
                failure,
            },
        );
    };
    let failed = t.builder.block(BlockKind::FailureCleanup, &[]);
    let inner = t.fails();
    leave(&mut t, failed, inner);
    t.builder.term(failed, Terminator::Propagate);
    let failure = t.failure(failed);
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: v,
            construct: site,
            failure,
        },
    );
    let n = t.builder.block(BlockKind::Body, &[]);
    if mutant {
        leave(&mut t, ENTRY, exit_failure);
        let fail = t.builder.propagate();
        t.call(
            ENTRY,
            &[(Passing::Borrow, l::BLK)],
            None,
            &[ArgumentValue::Value(v)],
            (n, &[]),
            fail,
        );
        t.builder.ret(n, None);
    } else {
        let failed = t.builder.block(BlockKind::FailureCleanup, &[]);
        let inner = t.fails();
        leave(&mut t, failed, inner);
        t.builder.term(failed, Terminator::Propagate);
        t.call(
            ENTRY,
            &[(Passing::Borrow, l::BLK)],
            None,
            &[ArgumentValue::Value(v)],
            (n, &[]),
            failed,
        );
        leave(&mut t, n, exit_failure);
        t.builder.ret(n, None);
    }
    t.check()
}

#[test]
fn a_region_value_is_used_only_while_its_region_is_entered() {
    pair(region, |k| matches!(k, K::UseAfterRegionExit(_)));
}

/// Placement exists exactly from the Placed stage on.
fn placement<S: Stage>(placed: bool) -> Vec<K> {
    let mut t = Lir::<S>::new(&[], Some(l::BLK));
    let one = t.int(ENTRY);
    let two = t.int(ENTRY);
    let symbol = t.symbol(SymbolKind::Type);
    let placement = placed.then_some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0))));
    let site = t.construct_site(
        ConstructorKind::Record(symbol),
        l::PAY,
        &[(0, one), (1, two)],
        placement,
    );
    let v = t.owned(l::BLK);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: v,
            construct: site,
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(v));
    t.check()
}

#[test]
fn placement_exists_exactly_from_the_placed_stage() {
    pair(
        |m| placement::<Placed>(!m),
        |k| *k == K::Placement("allocation without a storage owner"),
    );
    pair(placement::<Value>, |k| {
        *k == K::Placement("placement before the Placed stage")
    });
}

/// Ready: no inferred call destination and no operation without a native
/// implementation.
fn ready_call(destination: Option<DestinationClass>, native: Disposition) -> Vec<K> {
    let mut t = Lir::<Ready>::new(&[], None);
    t.manifest = manifest(native);
    // A caller destination receives the call's result.
    let result = destination.as_ref().map(|_| l::I64);
    let abi = t.abi(&[], result);
    t.table.fn_abis[abi.0 as usize].destination = destination.is_some();
    let signature = super::operation_abi::ready_signature(destination.is_some());
    t.declare_operation(signature.0, signature.1, abi);
    let result = t.table.fn_abis[abi.0 as usize]
        .result
        .as_ref()
        .map(|r| r.layout);
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.builder.propagate();
    t.call_to(
        ENTRY,
        Callee::Operation(OperationRef(0)),
        abi,
        &[],
        result,
        &[],
        (n, &[]),
        fail,
    );
    t.builder.unit.tables.calls[0].destination = destination;
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn ready_has_no_unresolved_destination_or_unsupported_operation() {
    let native = || Disposition::RuntimePrimitive("hirsel_rt_op".into());
    pair(
        |m| {
            ready_call(
                Some(if m {
                    DestinationClass::Inferred
                } else {
                    DestinationClass::Heap
                }),
                native(),
            )
        },
        |k| *k == K::UnresolvedDestination,
    );
    pair(
        |m| {
            ready_call(
                None,
                if m {
                    Disposition::Unavailable(Unavailable::NotImplemented)
                } else {
                    native()
                },
            )
        },
        |k| *k == K::UnsupportedOperation(OperationRef(0)),
    );
}

/// The validators seal accepted units with a receipt binding the unit,
/// layout table and manifest digests; rejected units come back unchanged.
#[test]
fn accepted_units_are_sealed_and_rejected_units_returned() {
    let build = |release: bool| {
        let mut t = Lir::<Rc>::new(&[TAKE_BLK], None);
        let x = t.owned(l::BLK);
        t.param(x);
        if release {
            t.release(ENTRY, x, &[]);
        }
        t.builder.ret(ENTRY, None);
        t.finish()
    };
    let (unit, table, manifest) = build(true);
    let digest = unit.digest();
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let sealed = validate::validate(unit, &inputs).expect("accepted");
    assert_eq!(sealed.receipt().raw().unit, digest);
    assert_eq!(sealed.receipt().raw().key, sealed.unit().header.key);
    assert_eq!(
        sealed.receipt().raw().validator_version,
        hirsel_lir::VALIDATOR_VERSION
    );
    let (unit, table, manifest) = build(false);
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let rejected = validate::validate(unit.clone(), &inputs).expect_err("rejected");
    assert_eq!(rejected.unit, unit);
    assert_eq!(rejected.violations.len(), 1);
    // A stale table version is a schema rejection before ownership checks.
    let mut stale = table.clone();
    stale.layout_version += 1;
    let target = hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &(),
        target: &target,
        layouts: &stale,
        manifest: &manifest,
    };
    let rejected = validate::validate(unit, &inputs).expect_err("stale");
    assert!(matches!(rejected.violations[0].kind, K::Schema(_)));
    // A table that fails its independent layout validation is rejected before
    // any unit check reads it.
    let mut t = Lir::<Rc>::new(&[], None);
    t.builder.ret(ENTRY, None);
    t.table.layouts[l::PAY.0 as usize].align = 3;
    let violations = t.check();
    assert!(!violations.is_empty());
    assert!(
        violations.iter().all(|k| matches!(k, K::Layout(_))),
        "{violations:?}"
    );
}

/// A unit with loans, views, a call, a join, a loop and cleanup.
fn rich() -> (
    LirUnit<Rc>,
    hirsel_lir::layout_types::LayoutTable,
    hirsel_core_contract::Manifest,
) {
    let mut t = Lir::<Rc>::new(
        &[(Passing::Take, l::PAIRBLK), (Passing::Mut, l::PAIRBLK)],
        None,
    );
    let u = t.owned(l::PAIRBLK);
    t.param(u);
    let p = t.slot(l::PAIRBLK, SlotKind::Parameter(1), ROOT);
    let pv = t.version_of(p);
    t.param(pv);
    let au = t.through(ENTRY, u, &[ProjectionStep::Field(0)], &[Undo::Release(u)]);
    let loan = t.loan(au, false);
    t.begin(ENTRY, loan);
    let w = t.read(ENTRY, au, u, ReadMode::Borrow, Some(loan));
    let header = t.builder.block(BlockKind::Body, &[]);
    let fail = t.cleanup(&[Undo::End(loan), Undo::Release(u)]);
    t.call(
        ENTRY,
        &[BORROW_BLK],
        None,
        &[ArgumentValue::Value(w)],
        (header, &[]),
        fail,
    );
    let body = t.builder.block(BlockKind::Body, &[]);
    let after = t.builder.block(BlockKind::Body, &[]);
    t.branch(header, (body, &[]), (after, &[]));
    t.builder.jump(body, header, &[]);
    t.end(after, loan);
    let aw = t.at(after, p, &[ProjectionStep::Field(1)], &[Undo::Release(u)]);
    let ploan = t.loan(aw, false);
    t.begin(after, ploan);
    let v = t.read(after, aw, pv, ReadMode::Borrow, Some(ploan));
    t.builder.push(after, Inst::Dup { value: v });
    t.end(after, ploan);
    let exit = t.cleanup_exit(&[Undo::Release(v), Undo::Release(u)], None);
    t.builder.jump(after, exit, &[]);
    t.finish()
}

/// Corrupting rows, holdings, edges, block kinds and terminators of a valid
/// unit yields violations, never a panic or a hang.
#[test]
fn corrupted_units_are_rejected_without_panicking() {
    let (unit, table, manifest) = rich();
    let run = |unit: &LirUnit<Rc>| {
        let inputs = Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &(),
            target: &hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu(),
            layouts: &table,
            manifest: &manifest,
        };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            validate::check(unit, &inputs)
        }))
        .unwrap_or_else(|_| panic!("validation panicked on {unit:#?}"))
    };
    assert_eq!(run(&unit), vec![]);
    let mut variants = Vec::new();
    for i in 0..unit.instructions.len() {
        for j in i + 1..unit.instructions.len() {
            let mut u = unit.clone();
            u.instructions.swap(i, j);
            variants.push((
                format!(
                    "swap {i} {j}: {:?} / {:?}",
                    unit.instructions[i], unit.instructions[j]
                ),
                u,
            ));
        }
    }
    for v in 0..unit.values.len() {
        for holding in [Holding::Plain, Holding::Owned, Holding::Borrowed(LoanId(0))] {
            let mut u = unit.clone();
            u.values[v].holding = holding;
            variants.push((format!("value {v} {holding:?}"), u));
        }
    }
    for e in 0..unit.tables.edges.len() {
        for b in 0..unit.blocks.len() {
            let mut u = unit.clone();
            u.tables.edges[e].target = BlockId(b as u32);
            variants.push((format!("edge {e} -> {b}"), u));
        }
    }
    for b in 0..unit.blocks.len() {
        for kind in [
            BlockKind::Body,
            BlockKind::Cleanup,
            BlockKind::FailureCleanup,
        ] {
            let mut u = unit.clone();
            u.blocks[b].kind = kind;
            variants.push((format!("block {b} {kind:?}"), u));
        }
        for term in [
            Terminator::Return(None),
            Terminator::Propagate,
            Terminator::Unreachable,
        ] {
            let mut u = unit.clone();
            u.terminators[b] = term.clone();
            variants.push((format!("block {b} {term:?}"), u));
        }
    }
    let mut rejected = 0;
    for (what, u) in &variants {
        if *u != unit && !run(u).is_empty() {
            rejected += 1;
        } else if *u != unit && std::env::var_os("HIRSEL_SHOW_MUTANTS").is_some() {
            eprintln!("accepted: {what}");
        }
    }
    // What stays accepted is benign: swaps of two releases of one value that
    // differ only in anchor, a body block marked as exit cleanup (or back),
    // and edges retargeted between blocks entered in the same state.
    assert!(
        rejected * 100 >= variants.len() * 85,
        "{rejected} of {} rejected",
        variants.len()
    );
    // A borrowed read moved above its loan's beginning is rejected.
    let mut swapped = unit.clone();
    let begin = swapped
        .instructions
        .iter()
        .position(|i| matches!(i, Inst::BeginLoan { .. }))
        .unwrap();
    swapped.instructions.swap(begin, begin + 1);
    assert!(
        run(&swapped)
            .iter()
            .any(|v| matches!(v.kind, K::OutsideLoan(_)))
    );
}

/// Where a `mut` call fixture names a stale version of the storage it passed.
#[derive(Clone, Copy, PartialEq)]
enum MutRead {
    /// Every read names the call's write-back version.
    Valid,
    /// The normal successor reads the pre-call version.
    StaleOnNormal,
    /// The failure cleanup reads the pre-call version.
    StaleOnFailure,
    /// The write-back version is read before the call defines it.
    BeforeCall,
    /// The write-back version holds a credit.
    OwnedVersion,
}

/// A local passed `mut` to a call, then read on both of the call's edges.
fn mut_call_version(case: MutRead) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::I64));
    let s = t.slot(l::I64, SlotKind::Local, ROOT);
    t.builder.push(ENTRY, Inst::StorageLive { slot: s });
    let a = t.at(ENTRY, s, &[], &[]);
    let one = t.int(ENTRY);
    let before = t.store(ENTRY, a, one, s);
    let after = if case == MutRead::OwnedVersion {
        t.owned(l::I64)
    } else {
        t.version_of(s)
    };
    if case == MutRead::BeforeCall {
        t.read(ENTRY, a, after, ReadMode::Bits, None);
    }
    let n = t.builder.block(BlockKind::Body, &[]);
    let fb = t.builder.block(BlockKind::FailureCleanup, &[]);
    t.read(
        fb,
        a,
        if case == MutRead::StaleOnFailure {
            before
        } else {
            after
        },
        ReadMode::Bits,
        None,
    );
    t.builder.term(fb, Terminator::Propagate);
    t.call(
        ENTRY,
        &[(Passing::Mut, l::I64)],
        None,
        &[ArgumentValue::Address {
            address: a,
            version: after,
        }],
        (n, &[]),
        fb,
    );
    let r = t.read(
        n,
        a,
        if case == MutRead::StaleOnNormal {
            before
        } else {
            after
        },
        ReadMode::Bits,
        None,
    );
    t.builder.push(
        n,
        Inst::StorageDead {
            slot: s,
            if_live: false,
        },
    );
    t.builder.ret(n, Some(r));
    t.check()
}

#[test]
fn a_mut_call_defines_a_new_version_on_both_edges() {
    let case = |stale| move |m| mut_call_version(if m { stale } else { MutRead::Valid });
    pair(case(MutRead::StaleOnNormal), |k| {
        matches!(k, K::StaleVersion { .. })
    });
    pair(case(MutRead::StaleOnFailure), |k| {
        matches!(k, K::StaleVersion { .. })
    });
    pair(case(MutRead::BeforeCall), |k| {
        matches!(k, K::NotDominated(_))
    });
    pair(case(MutRead::OwnedVersion), |k| {
        *k == K::Type("mut call version")
    });
}

/// Where a closure body's capture fixture goes wrong.
#[derive(Clone, Copy, PartialEq)]
enum CaptureRead {
    Valid,
    /// The read names a value that is not the environment content's version.
    Unversioned,
    /// After a replace, the read still names the entry version.
    StaleAfterReplace,
    /// The capture slot has no entry in `LirUnit::captures`.
    Missing,
    /// The capture version has another type than its slot.
    WrongType,
}

/// A closure body reads its capture slot, replaces its content and reads it
/// again; every read names the reaching version.
fn capture_version(case: CaptureRead) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], Some(l::I64));
    let c = t.slot(l::I64, SlotKind::Capture(0), ROOT);
    let d = t.slot(l::I64, SlotKind::Displaced, ROOT);
    let entry = t.builder.unit.captures[0];
    match case {
        CaptureRead::Missing => t.builder.unit.captures.clear(),
        CaptureRead::WrongType => t.builder.unit.captures[0] = t.plain(l::BOOL),
        _ => {}
    }
    let a = t.at(ENTRY, c, &[], &[]);
    let one = t.int(ENTRY);
    let first = t.read(
        ENTRY,
        a,
        if case == CaptureRead::Unversioned {
            one
        } else {
            entry
        },
        ReadMode::Bits,
        None,
    );
    t.builder.push(ENTRY, Inst::StorageLive { slot: d });
    let replaced = t.version_of(c);
    let displaced_version = t.version_of(d);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Replace {
            address: a,
            value: first,
            displaced: d,
            version: replaced,
            displaced_version,
            if_live: false,
            failure,
        },
    );
    let r = t.read(
        ENTRY,
        a,
        if case == CaptureRead::StaleAfterReplace {
            entry
        } else {
            replaced
        },
        ReadMode::Bits,
        None,
    );
    t.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot: d,
            if_live: false,
        },
    );
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn capture_reads_are_version_checked_like_any_slot() {
    let case = |bad| move |m| capture_version(if m { bad } else { CaptureRead::Valid });
    pair(case(CaptureRead::Unversioned), |k| {
        *k == K::Type("reaching operand is not a version")
    });
    pair(case(CaptureRead::StaleAfterReplace), |k| {
        matches!(k, K::StaleVersion { .. })
    });
    pair(case(CaptureRead::Missing), |k| {
        *k == K::Arity("capture slots")
    });
    pair(case(CaptureRead::WrongType), |k| {
        *k == K::Type("capture version")
    });
}

/// Where a String literal fixture goes wrong.
#[derive(Clone, Copy, PartialEq)]
enum Literal {
    /// A Plain slice over static storage.
    Valid,
    /// A slice literal holding a credit.
    OwnedSlice,
    /// String bytes at an aggregate layout that is not a slice.
    NotSlice,
}

/// A String literal produced as a slice over static storage.
fn string_literal(case: Literal) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[], None);
    let layout = if case == Literal::NotSlice {
        l::PAY
    } else {
        l::SLICE
    };
    let ty = t.ty(ValueRepr::Aggregate, layout);
    t.builder.unit.tables.constants.push(ConstantData {
        ty,
        value: ConstantValue::String("hirsel".into()),
    });
    let constant = ConstantId(t.builder.unit.tables.constants.len() as u32 - 1);
    let holding = if case == Literal::OwnedSlice {
        Holding::Owned
    } else {
        Holding::Plain
    };
    let result = t.value_as(ValueRepr::Aggregate, layout, holding);
    t.builder.push(ENTRY, Inst::Const { result, constant });
    if case == Literal::OwnedSlice {
        t.release(ENTRY, result, &[]);
    }
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_string_literal_is_a_plain_slice_over_static_storage() {
    let case = |bad| move |m| string_literal(if m { bad } else { Literal::Valid });
    pair(case(Literal::OwnedSlice), |k| matches!(k, K::Holding(_)));
    pair(case(Literal::NotSlice), |k| *k == K::Type("constant"));
}

/// Where a layout-descriptor literal goes wrong.
#[derive(Clone, Copy, PartialEq)]
enum Descriptor {
    /// A `u32` naming a layout of the table.
    Valid,
    /// The registry index at a `u64`.
    Wide,
    /// A layout outside the table.
    Unknown,
}

/// A layout's runtime descriptor index (`ConstantValue::Descriptor`), produced and dropped.
fn descriptor_literal(case: Descriptor) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[], None);
    let (repr, layout) = if case == Descriptor::Wide {
        (ValueRepr::Scalar(ScalarKind::U64), l::U64)
    } else {
        (ValueRepr::Scalar(ScalarKind::U32), l::U32)
    };
    let ty = t.ty(repr, layout);
    let described = if case == Descriptor::Unknown {
        LayoutId(u32::MAX)
    } else {
        l::PAY
    };
    t.builder.unit.tables.constants.push(ConstantData {
        ty,
        value: ConstantValue::Descriptor(described),
    });
    let constant = ConstantId(t.builder.unit.tables.constants.len() as u32 - 1);
    let result = t.value_as(repr, layout, Holding::Plain);
    t.builder.push(ENTRY, Inst::Const { result, constant });
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_descriptor_literal_is_a_u32_naming_a_layout_of_the_table() {
    let case = |bad| move |m| descriptor_literal(if m { bad } else { Descriptor::Valid });
    pair(case(Descriptor::Wide), |k| *k == K::Type("constant"));
    pair(case(Descriptor::Unknown), |_| true);
}

/// A closure over one copied i64, produced at `layout` and returned.
fn closure_at(layout: LayoutId) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[], Some(layout));
    let x = t.int(ENTRY);
    let code = t.symbol(SymbolKind::Function);
    let abi = t.abi_shaped(CallableShape::Closure, &[], None);
    let captures = PoolRange::push(
        &mut t.builder.unit.pools.captures,
        [CaptureValue {
            value: x,
            usage: CaptureUse::Copy,
        }],
    );
    t.builder.unit.tables.closures.push(ClosureSite {
        receipts: None,
        scoped: false,
        code: Some(ClosureCode {
            symbol: code,
            abi: abi,
        }),
        environment: l::ENV,
        captures,
        destination: DestinationClass::Heap,
        placement: None,
        anchor: SourceAnchorId(0),
    });
    let closure = ClosureId(t.builder.unit.tables.closures.len() as u32 - 1);
    let result = t.owned(layout);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Closure {
            result,
            closure,
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn a_closure_is_its_specialized_or_its_erased_callable() {
    assert_eq!(closure_at(l::CLOSURE), Vec::new());
    // The erased callable of the site's multiplicity; the environment stays
    // named by the site.
    pair(
        |m| closure_at(if m { l::ERASED_ONCE } else { l::ERASED }),
        |k| *k == K::Type("closure"),
    );
}

/// A call through a callee ABI that diverges or returns, built with or
/// without a normal edge (LIR schema revision 19).
fn call_divergence(callee_diverges: bool, returning: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[], None);
    let fail = t.builder.propagate();
    let abi = t.abi(&[], None);
    t.table.fn_abis[abi.0 as usize].diverges = callee_diverges;
    if returning {
        let n = t.builder.block(BlockKind::Body, &[]);
        let symbol = t.symbol(SymbolKind::Function);
        t.call_to(
            ENTRY,
            Callee::Direct(symbol),
            abi,
            &[],
            None,
            &[],
            (n, &[]),
            fail,
        );
        t.builder.ret(n, None);
    } else {
        t.diverging_call(ENTRY, abi, fail);
    }
    t.check()
}

#[test]
fn exactly_the_calls_of_diverging_callees_diverge() {
    assert_eq!(call_divergence(true, false), vec![]);
    assert_eq!(call_divergence(false, true), vec![]);
    // Never erased to unit: a successor after a callee that cannot return.
    assert_eq!(
        call_divergence(true, true),
        vec![K::CallShape("returning call of a diverging callee")]
    );
    // A callee that returns cannot end its caller's frame.
    assert_eq!(
        call_divergence(false, false),
        vec![K::CallShape("diverging call of a returning callee")]
    );
}

/// A function whose own ABI diverges never returns normally.
#[test]
fn a_diverging_function_has_no_normal_return() {
    pair(
        |mutant| {
            let mut t = Lir::<Rc>::new(&[], None);
            let fail = t.builder.propagate();
            let own = t.builder.unit.header.abi;
            t.table.fn_abis[own.0 as usize].diverges = true;
            if mutant {
                t.builder.ret(ENTRY, None);
            } else {
                let callee = t.abi(&[], None);
                t.table.fn_abis[callee.0 as usize].diverges = true;
                t.diverging_call(ENTRY, callee, fail);
            }
            t.check()
        },
        |k| *k == K::Type("returned value"),
    );
}

/// A `FunctionRef` is a callable or, at a pointer scalar, a definition's C callback
/// address (PROG-4); no other scalar.
fn function_ref(repr: ValueRepr, layout: LayoutId) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[], None);
    let symbol = t.builder.symbol(SymbolKind::Function, path("visit"));
    let f = t.value_as(repr, layout, Holding::Plain);
    t.builder
        .push(ENTRY, Inst::FunctionRef { result: f, symbol });
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_function_reference_is_a_callable_or_a_callback_address() {
    use hirsel_lir::layout_types::ScalarKind;
    let pointer = ValueRepr::Scalar(ScalarKind::Pointer);
    assert_eq!(function_ref(pointer, l::PTR), vec![]);
    let integer = function_ref(ValueRepr::Scalar(ScalarKind::I64), l::I64);
    assert_eq!(integer, vec![K::Type("function reference")]);
}

#[test]
fn a_tracked_body_requires_a_physical_caller_site_carrier() {
    pair(
        |mutant| {
            let mut t = Lir::<Ready>::new(&[], None);
            t.builder.unit.header.track_caller = true;
            let own = t.builder.unit.header.abi;
            t.table.fn_abis[own.0 as usize].caller_site = !mutant;
            t.builder.ret(ENTRY, None);
            t.check()
        },
        |k| *k == K::Type("caller metadata"),
    );
}
