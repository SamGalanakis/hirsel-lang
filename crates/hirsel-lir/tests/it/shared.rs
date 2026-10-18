//! `Shared[T]` roots (MEM-11, LIR schema revision 27): `Freeze` and `Thaw`
//! are library-only moves between a value and its root, a root is a counted
//! block released at its frozen payload, and a library may test an owned
//! root's count before RC. A library may also read the frozen value in place
//! through a borrowed root (`shared.with`), by borrow only. One hand-built
//! mutation per obligation, each paired with its valid twin.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::effect::Opcode;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::stage::{Rc, Value};
use support::*;

fn library<S>(t: &mut Lir<S>) {
    t.builder.unit.header.origin = UnitOrigin::Library;
}

/// `Freeze` of an owned `value` parameter into a root of `root`, returned.
fn freeze(source: bool, value: LayoutId, root: LayoutId) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Take, value)], Some(root));
    if !source {
        library(&mut t);
    }
    let x = t.owned(value);
    t.param(x);
    let r = t.owned(root);
    t.builder.push(
        ENTRY,
        Inst::Freeze {
            result: r,
            value: x,
        },
    );
    t.builder.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn only_a_library_freezes_a_value_into_its_own_root() {
    pair(
        |m| freeze(m, l::BLK, l::SHARED),
        |k| *k == K::Vocabulary(Opcode::Freeze),
    );
    // The root's frozen record must hold exactly the value's layout.
    pair(
        |m| freeze(false, if m { l::PAIRBLK } else { l::BLK }, l::SHARED),
        |k| *k == K::Type("freeze"),
    );
    pair(
        |m| freeze(false, l::BLK, if m { l::PAIRBLK } else { l::SHARED }),
        |k| *k == K::Type("freeze"),
    );
}

/// `Thaw` of an owned root parameter into a value of `value`, returned.
fn thaw(source: bool, value: LayoutId) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Take, l::SHARED)], Some(value));
    if !source {
        library(&mut t);
    }
    let r = t.owned(l::SHARED);
    t.param(r);
    let x = t.owned(value);
    t.builder.push(
        ENTRY,
        Inst::Thaw {
            result: x,
            value: r,
        },
    );
    t.builder.ret(ENTRY, Some(x));
    t.check()
}

#[test]
fn only_a_library_thaws_a_root_into_its_frozen_value() {
    pair(|m| thaw(m, l::BLK), |k| *k == K::Vocabulary(Opcode::Thaw));
    pair(
        |m| thaw(false, if m { l::PAIRBLK } else { l::BLK }),
        |k| *k == K::Type("thaw"),
    );
}

/// An owned root parameter released, naming `named`, or given another
/// representation.
fn release_root(named: Option<LayoutId>, repr: ValueRepr) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Take, l::SHARED)], None);
    let r = t.value_as(repr, l::SHARED, Holding::Owned);
    t.param(r);
    let failure = t.fails();
    let anchor = t.cleanup_anchor();
    t.builder.push(
        ENTRY,
        Inst::Release {
            value: r,
            layout: named.unwrap_or(l::FROZEN),
            live: MaskRef::All,
            anchor,
            failure,
        },
    );
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_root_is_a_counted_block_released_at_its_frozen_payload() {
    pair(
        |m| release_root(m.then_some(l::BLK), ValueRepr::Block),
        |k| *k == K::Type("released layout"),
    );
    pair(
        |m| {
            release_root(
                None,
                if m {
                    ValueRepr::Aggregate
                } else {
                    ValueRepr::Block
                },
            )
        },
        |k| matches!(k, K::Representation(_)),
    );
}

/// A library's uniqueness test of an owned parameter of `layout`, before RC.
fn early_test(layout: LayoutId) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[(Passing::Take, layout)], Some(layout));
    library(&mut t);
    let x = t.owned(layout);
    t.param(x);
    let last = t.plain(l::BOOL);
    t.builder.push(
        ENTRY,
        Inst::IsUnique {
            result: last,
            value: x,
        },
    );
    t.builder.ret(ENTRY, Some(x));
    t.check()
}

#[test]
fn a_library_may_test_an_owned_root_count_before_rc() {
    // A root's count is the number of roots, which no pass inserts; an
    // ordinary block's count is not yet decided before RC.
    pair(
        |m| early_test(if m { l::BLK } else { l::SHARED }),
        |k| *k == K::Type("library uniqueness ownership"),
    );
}

/// What a unit does through the frozen value of its borrowed root parameter.
#[derive(Clone, Copy, PartialEq)]
enum Through {
    Borrow,
    Take,
    Store,
}

/// `shared.with`'s read: an address of field 0 of the root's `Frozen` payload,
/// prepared, then used as `through` says; a library unit unless `source`.
fn frozen_view(source: bool, through: Through) -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[(Passing::Borrow, l::SHARED), (Passing::Take, l::BLK)],
        None,
    );
    if !source {
        library(&mut t);
    }
    let s = t.slot(l::SHARED, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(s);
    t.param(version);
    let x = t.owned(l::BLK);
    t.param(x);
    let view = t.at(ENTRY, s, &[ProjectionStep::Field(0)], &[Undo::Release(x)]);
    match through {
        Through::Borrow => {
            let loan = t.loan(view, false);
            t.begin(ENTRY, loan);
            t.read(ENTRY, view, version, ReadMode::Borrow, Some(loan));
            t.end(ENTRY, loan);
        }
        Through::Take => {
            let taken = t.read(ENTRY, view, version, ReadMode::Take, None);
            t.release_then(ENTRY, taken, &[Undo::Release(x)]);
        }
        Through::Store => {
            t.store(ENTRY, view, x, s);
            t.builder.ret(ENTRY, None);
            return t.check();
        }
    }
    t.release_then(ENTRY, x, &[]);
    t.builder.ret(ENTRY, None);
    t.check()
}

#[test]
fn only_a_library_reads_a_frozen_value_through_its_borrowed_root() {
    pair(
        |m| frozen_view(m, Through::Borrow),
        |k| matches!(k, K::AddressType(_)),
    );
}

#[test]
fn a_frozen_value_is_never_moved_or_written_through_its_root() {
    let frozen = |k: &K| *k == K::Type("access to a frozen value other than a borrow");
    pair(
        |m| frozen_view(false, if m { Through::Take } else { Through::Borrow }),
        frozen,
    );
    pair(
        |m| frozen_view(false, if m { Through::Store } else { Through::Borrow }),
        frozen,
    );
}
