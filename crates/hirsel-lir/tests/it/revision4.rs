//! LIR schema revision 4: flat values
//! constructed from ordinary values, a String part viewed at its source type,
//! and the length and elements of a flat sequence. Every check is a
//! hand-built mutation paired with its accepted twin.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, LayoutKind, SizeSpec, SizeTerm};
use hirsel_lir::stage::Rc;
use support::*;

/// Construct the flat `record` (a block result of `block`) from borrowed
/// parameters of `fields`' layouts, in field order.
fn construct_flat(record: LayoutId, block: LayoutId, fields: &[LayoutId]) -> Vec<K> {
    let params: Vec<_> = fields.iter().map(|&f| (Passing::Borrow, f)).collect();
    let mut t = Lir::<Rc>::new(&params, Some(block));
    let values: Vec<(u32, ValueId)> = fields
        .iter()
        .enumerate()
        .map(|(i, &f)| {
            let v = t.plain(f);
            (i as u32, t.param(v))
        })
        .collect();
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let site = t.construct_site(ConstructorKind::Record(symbol), record, &values, placement);
    let result = t.owned(block);
    let failure = t.fails();
    t.push(
        ENTRY,
        Inst::Construct {
            result,
            construct: site,
            failure,
        },
    );
    t.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn a_flat_string_field_is_constructed_from_a_byte_slice() {
    pair(
        |mutant| {
            let text = if mutant { l::U64 } else { l::SLICE };
            construct_flat(l::FLAT_ITEM, l::FLAT_ITEM_BLK, &[text, l::U64])
        },
        |k| *k == K::Type("constructed fields"),
    );
}

#[test]
fn a_flat_record_field_is_constructed_from_a_block_of_its_bytes() {
    pair(
        |mutant| {
            let inner = if mutant { l::BLK } else { l::FLAT_ITEM_BLK };
            construct_flat(l::FLAT_OUTER, l::FLAT_OUTER_BLK, &[inner, l::U64])
        },
        |k| *k == K::Type("constructed fields"),
    );
}

#[test]
fn a_flat_list_field_rejects_an_unrelated_record_block() {
    pair(
        |mutant| {
            if mutant {
                construct_flat(l::FLAT_ITEMS, l::FLAT_ITEMS_BLK, &[l::BLK])
            } else {
                construct_flat(l::FLAT_ITEM, l::FLAT_ITEM_BLK, &[l::SLICE, l::U64])
            }
        },
        |k| *k == K::Type("constructed fields"),
    );
}

/// `Outer(inner: x, k)` from an owned `Item` block `x`: the construction only
/// reads the value it encodes, so `x` is released after it. The mutant never
/// releases `x`, which therefore leaks; were the field consumed, the valid
/// twin's release would have no credit.
fn encode_owned(release: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[(Passing::Take, l::FLAT_ITEM_BLK), (Passing::Borrow, l::U64)],
        Some(l::FLAT_OUTER_BLK),
    );
    let x = t.owned(l::FLAT_ITEM_BLK);
    t.param(x);
    let k = t.plain(l::U64);
    t.param(k);
    let symbol = t.symbol(SymbolKind::Type);
    let placement = t.heap();
    let site = t.construct_site(
        ConstructorKind::Record(symbol),
        l::FLAT_OUTER,
        &[(0, x), (1, k)],
        placement,
    );
    let result = t.owned(l::FLAT_OUTER_BLK);
    let target = t.releasing(&[x]);
    let failure = t.failure(target);
    t.push(
        ENTRY,
        Inst::Construct {
            result,
            construct: site,
            failure,
        },
    );
    if release {
        t.release(ENTRY, x, &[result]);
    }
    t.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn a_flat_construction_reads_what_it_encodes() {
    pair(|mutant| encode_owned(!mutant), |k| matches!(k, K::Leak(_)));
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Valid,
    /// The scalar `n`, which has no view: it is its own place.
    Scalar,
    /// A result that is no byte slice.
    NotSlice,
    /// After the loan ended.
    Ended,
    /// A plain result, not a view under a loan.
    Plain,
    /// Under a loan on `n`, which does not cover `text`.
    Uncovered,
}

/// `text` of a borrowed flat `Item` viewed as a String under a loan on it.
fn view(case: View) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Borrow, l::FLAT_ITEM_BLK)], None);
    let x = t.plain(l::FLAT_ITEM_BLK);
    t.param(x);
    let field = if case == View::Scalar { 1 } else { 0 };
    let at = t.through(ENTRY, x, &[ProjectionStep::Field(field)], &[]);
    let lent = if case == View::Uncovered {
        t.through(ENTRY, x, &[ProjectionStep::Field(1)], &[])
    } else {
        at
    };
    let loan = t.loan(lent, false);
    t.begin(ENTRY, loan);
    if case == View::Ended {
        t.end(ENTRY, loan);
    }
    let layout = if case == View::NotSlice {
        l::U64
    } else {
        l::SLICE
    };
    let result = if case == View::Plain {
        t.plain(layout)
    } else {
        t.borrowed(layout, loan)
    };
    t.push(
        ENTRY,
        Inst::FlatView {
            result,
            address: at,
        },
    );
    if case != View::Ended {
        t.end(ENTRY, loan);
    }
    t.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_flat_view_shows_a_string_encoding_as_a_byte_slice() {
    pair(
        |mutant| view(if mutant { View::Scalar } else { View::Valid }),
        |k| *k == K::Type("flat view"),
    );
    pair(
        |mutant| view(if mutant { View::NotSlice } else { View::Valid }),
        |k| *k == K::Type("flat view"),
    );
}

/// An ordinary byte slice needs its counted sequence payload for a new owner.
fn byte_storage(t: &mut Lir<Rc>) {
    let mut storage = t.table.layouts[l::SEQ.0 as usize].clone();
    storage.kind = LayoutKind::Sequence { element: l::U8 };
    storage.size = SizeSpec::Dynamic(vec![
        SizeTerm::Constant(16),
        SizeTerm::ReadLength {
            offset: 0,
            scalar: hirsel_lir::layout_types::ScalarKind::U64,
        },
        SizeTerm::Constant(1),
        SizeTerm::Multiply,
        SizeTerm::Add,
    ]);
    t.table.layouts.push(storage);
}

/// Source-free materialization of a prefixed field. The valid controls
/// fail the former same-layout-only FlatCopy rule; each mutant has an
/// unrelated result layout which cannot encode the addressed bytes.
#[test]
fn a_flat_copy_materializes_only_the_encoding_source_layout() {
    for (owner, result) in [
        (l::FLAT_ITEM_BLK, l::SLICE),
        (l::FLAT_OUTER_BLK, l::FLAT_ITEM_BLK),
    ] {
        pair(
            |mutant| {
                let result_layout = if mutant {
                    if result == l::SLICE {
                        l::FLAT_ITEM_BLK
                    } else {
                        l::SLICE
                    }
                } else {
                    result
                };
                let mut t = Lir::<Rc>::new(&[(Passing::Borrow, owner)], Some(result_layout));
                byte_storage(&mut t);
                let x = t.plain(owner);
                t.param(x);
                let at = t.through(ENTRY, x, &[ProjectionStep::Field(0)], &[]);
                let layout = t.unit.types[t.unit.addresses[at.index()].ty.index()].layout;
                let value = t.owned(result_layout);
                let failure = t.fails();
                t.push(
                    ENTRY,
                    Inst::FlatCopy {
                        result: value,
                        address: at,
                        layout,
                        failure,
                    },
                );
                t.ret(ENTRY, Some(value));
                t.check()
            },
            |k| *k == K::Type("flat copy"),
        );
    }
}

#[test]
fn a_flat_copy_of_bytes_requires_ordinary_sequence_storage() {
    pair(
        |mutant| {
            let mut t = Lir::<Rc>::new(&[(Passing::Borrow, l::FLAT_ITEM_BLK)], Some(l::SLICE));
            if !mutant {
                byte_storage(&mut t);
            }
            let x = t.plain(l::FLAT_ITEM_BLK);
            t.param(x);
            let at = t.through(ENTRY, x, &[ProjectionStep::Field(0)], &[]);
            let layout = t.unit.types[t.unit.addresses[at.index()].ty.index()].layout;
            let value = t.owned(l::SLICE);
            let failure = t.fails();
            t.push(
                ENTRY,
                Inst::FlatCopy {
                    result: value,
                    address: at,
                    layout,
                    failure,
                },
            );
            t.ret(ENTRY, Some(value));
            t.check()
        },
        |k| *k == K::Type("flat copy"),
    );
}

#[test]
fn a_flat_view_borrows_the_flat_value_under_a_covering_loan() {
    pair(
        |mutant| view(if mutant { View::Ended } else { View::Valid }),
        |k| matches!(k, K::OutsideLoan(_)),
    );
    pair(
        |mutant| view(if mutant { View::Plain } else { View::Valid }),
        |k| matches!(k, K::Holding(_)),
    );
    pair(
        |mutant| view(if mutant { View::Uncovered } else { View::Valid }),
        |k| matches!(k, K::LoanCoverage(_)),
    );
}

/// The element count of `steps` inside a borrowed flat `Items`, as `result`.
fn length(steps: &[ProjectionStep], result: LayoutId) -> Vec<K> {
    let mut t = Lir::<Rc>::new(&[(Passing::Borrow, l::FLAT_ITEMS_BLK)], Some(result));
    let x = t.plain(l::FLAT_ITEMS_BLK);
    t.param(x);
    let at = t.through(ENTRY, x, steps, &[]);
    let r = t.plain(result);
    t.push(
        ENTRY,
        Inst::FlatLength {
            result: r,
            address: at,
        },
    );
    t.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn a_flat_length_counts_a_flat_sequence() {
    pair(
        |mutant| {
            let steps: &[ProjectionStep] = if mutant {
                &[]
            } else {
                &[ProjectionStep::Field(0)]
            };
            length(steps, l::U64)
        },
        |k| *k == K::Type("flat length"),
    );
    pair(
        |mutant| {
            length(
                &[ProjectionStep::Field(0)],
                if mutant { l::I64 } else { l::U64 },
            )
        },
        |k| *k == K::Type("flat length"),
    );
}

/// `n` of element `i` of a borrowed flat `Items`' list, read as bits. The
/// mutant indexes the `Items` record itself, which is no sequence.
fn element(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Rc>::new(
        &[
            (Passing::Borrow, l::FLAT_ITEMS_BLK),
            (Passing::Borrow, l::U64),
        ],
        Some(l::U64),
    );
    let x = t.plain(l::FLAT_ITEMS_BLK);
    t.param(x);
    let i = t.plain(l::U64);
    t.param(i);
    let steps: Vec<ProjectionStep> = if mutant {
        vec![ProjectionStep::Index(i)]
    } else {
        vec![
            ProjectionStep::Field(0),
            ProjectionStep::Index(i),
            ProjectionStep::Field(0),
            ProjectionStep::Field(1),
        ]
    };
    // A reading that indexed any flat record would reach its first field.
    let layout = if mutant { l::FLAT_LIST } else { l::U64 };
    let at = t.address(AddressBase::Value(x), &steps, layout);
    t.prepare(ENTRY, at, &[]);
    if mutant {
        t.ret(ENTRY, Some(i));
    } else {
        let r = t.plain(l::U64);
        t.push(
            ENTRY,
            Inst::Read {
                result: r,
                address: at,
                reaching: x,
                mode: ReadMode::Bits,
            },
        );
        t.ret(ENTRY, Some(r));
    }
    t.check()
}

#[test]
fn an_element_of_a_flat_sequence_is_a_place_at_its_cursor() {
    pair(element, |k| matches!(k, K::AddressType(_)));
}

/// A store into `n` of a `mut` flat `Item`, or of element `i` of a flat
/// `Items`' list: the element is a variable part, fixed at construction
/// (MEM-12), whatever field of it is written.
fn element_write(in_list: bool) -> Vec<K> {
    let layout = if in_list { l::FLAT_ITEMS } else { l::FLAT_ITEM };
    let mut t = Lir::<Rc>::new(&[(Passing::Mut, layout), (Passing::Borrow, l::U64)], None);
    let s = t.slot(layout, SlotKind::Parameter(0), ROOT);
    let sv = t.version_of(s);
    t.param(sv);
    let i = t.plain(l::U64);
    t.param(i);
    let steps: Vec<ProjectionStep> = if in_list {
        vec![
            ProjectionStep::Field(0),
            ProjectionStep::Index(i),
            ProjectionStep::Field(0),
            ProjectionStep::Field(1),
        ]
    } else {
        vec![ProjectionStep::Field(1)]
    };
    let at = t.at(ENTRY, s, &steps, &[]);
    let v = t.read(ENTRY, at, sv, ReadMode::Take, None);
    t.store(ENTRY, at, v, s);
    t.ret(ENTRY, None);
    t.check()
}

#[test]
fn a_field_of_a_flat_list_element_is_never_rewritten() {
    pair(element_write, |k| {
        *k == K::Type("write into a variable part of a flat value")
    });
}
