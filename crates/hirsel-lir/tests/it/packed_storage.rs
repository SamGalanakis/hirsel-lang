//! Source-free C controls: ordinary storage reserves the largest Packed
//! variant, while a length-dependent flat encoding cannot be stored inline.
use crate::support;

use hirsel_lir::layout_types::*;
use hirsel_lir::stage::Value;
use support::*;

fn field(ordinal: u32, offset: u64, layout: LayoutId) -> FieldLayout {
    FieldLayout {
        ordinal,
        offset,
        layout,
        unaligned: false,
    }
}

fn packed<S: hirsel_lir::stage::Stage>(t: &mut Lir<S>) -> LayoutId {
    let id = LayoutId(t.table.layouts.len() as u32);
    let mut r = t.table.layouts[l::OPT.0 as usize].clone();
    r.kind = LayoutKind::Packed;
    r.size = SizeSpec::Dynamic(vec![SizeTerm::ByTag {
        tag_offset: 0,
        variants: vec![
            (0, vec![SizeTerm::Constant(16)]),
            (1, vec![SizeTerm::Constant(24)]),
        ],
    }]);
    r.tag = Some(TagEncoding {
        offset: 0,
        form: TagForm::Ordinal,
        variants: vec![
            VariantLayout {
                ordinal: 0,
                fields: vec![field(0, 8, l::U64)],
            },
            VariantLayout {
                ordinal: 1,
                fields: vec![field(0, 8, l::U64), field(1, 16, l::U64)],
            },
        ],
    });
    r.abi.scalars.clear();
    t.table.layouts.push(r);
    id
}

fn aggregate(t: &mut Lir<Value>, child: LayoutId) -> LayoutId {
    let id = LayoutId(t.table.layouts.len() as u32);
    let mut r = t.table.layouts[l::PAY.0 as usize].clone();
    r.size = SizeSpec::Fixed(32);
    r.fields = vec![field(0, 0, child), field(1, 24, l::U64)];
    r.abi.scalars.clear();
    t.table.layouts.push(r);
    id
}

fn layout_obligation(k: &K, obligation: &str) -> bool {
    matches!(k, K::Layout(detail) if detail.contains(obligation))
}

#[test]
fn c_reserves_the_maximum_packed_field_extent_for_overlap_and_containment() {
    for overlap in [true, false] {
        pair(
            |bad| {
                let mut t = Lir::<Value>::new(&[], None);
                let child = packed(&mut t);
                let parent = aggregate(&mut t, child);
                if bad {
                    if overlap {
                        t.table.layouts[parent.0 as usize].fields[1].offset = 16;
                    } else {
                        t.table.layouts[parent.0 as usize].size = SizeSpec::Fixed(24);
                    }
                }
                t.ret(ENTRY, None);
                t.check()
            },
            |k| layout_obligation(k, if overlap { "Overlap" } else { "Size" }),
        );
    }
}

#[test]
fn c_reserves_the_maximum_packed_extent_inside_variant_payloads() {
    pair(
        |bad| {
            let mut t = Lir::<Value>::new(&[], None);
            let child = packed(&mut t);
            let mut r = t.table.layouts[l::OPT.0 as usize].clone();
            r.size = SizeSpec::Fixed(40);
            r.tag.as_mut().unwrap().variants[1].fields = vec![
                field(0, 8, child),
                field(1, if bad { 24 } else { 32 }, l::U64),
            ];
            r.abi.scalars.clear();
            t.table.layouts.push(r);
            t.ret(ENTRY, None);
            t.check()
        },
        |k| layout_obligation(k, "Overlap"),
    );
}

#[test]
fn c_requires_the_maximum_packed_stride_in_inline_arrays() {
    pair(
        |bad| {
            let mut t = Lir::<Value>::new(&[], None);
            let packed = packed(&mut t);
            let child = aggregate(&mut t, packed);
            let mut r = t.table.layouts[l::PAY.0 as usize].clone();
            r.kind = LayoutKind::Inline {
                element: child,
                count: 2,
            };
            r.size = SizeSpec::Fixed(if bad { 48 } else { 64 });
            r.fields.clear();
            r.abi.scalars.clear();
            t.table.layouts.push(r);
            t.ret(ENTRY, None);
            t.check()
        },
        |k| layout_obligation(k, "Inline size"),
    );
}

#[test]
fn c_requires_the_maximum_packed_stride_in_sequences() {
    pair(
        |bad| {
            let mut t = Lir::<Value>::new(&[], None);
            let packed = packed(&mut t);
            let child = aggregate(&mut t, packed);
            let mut r = t.table.layouts[l::SEQ.0 as usize].clone();
            r.kind = LayoutKind::Sequence { element: child };
            r.size = SizeSpec::Dynamic(vec![
                SizeTerm::Constant(8),
                SizeTerm::ReadLength {
                    offset: 0,
                    scalar: ScalarKind::U64,
                },
                SizeTerm::Constant(if bad { 24 } else { 32 }),
                SizeTerm::Multiply,
                SizeTerm::Add,
            ]);
            t.table.layouts.push(r);
            t.ret(ENTRY, None);
            t.check()
        },
        |k| layout_obligation(k, "sequence size"),
    );
}

#[test]
fn c_rejects_unbounded_flat_storage_beside_an_accepted_packed_twin() {
    pair(
        |bad| {
            let mut t = Lir::<Value>::new(&[], None);
            let child = packed(&mut t);
            let parent = aggregate(&mut t, child);
            if bad {
                let flat = &t.table.layouts[l::FLAT_TEXT.0 as usize];
                assert_eq!(flat.size.storage(), None);
                t.table.layouts[parent.0 as usize].fields[0].layout = l::FLAT_TEXT;
            }
            t.ret(ENTRY, None);
            t.check()
        },
        |k| layout_obligation(k, "dynamic extent inline"),
    );
}

#[test]
fn c_keeps_direct_dynamic_packed_sequences_unavailable() {
    for inline in [false, true] {
        pair(
            |bad| {
                let mut t = Lir::<Value>::new(&[], None);
                let packed = packed(&mut t);
                let child = if bad {
                    packed
                } else {
                    aggregate(&mut t, packed)
                };
                let stride = if bad { 24 } else { 32 };
                let mut r = t.table.layouts[l::SEQ.0 as usize].clone();
                if inline {
                    r.kind = LayoutKind::Inline {
                        element: child,
                        count: 2,
                    };
                    r.size = SizeSpec::Fixed(2 * stride);
                    r.fields.clear();
                } else {
                    r.kind = LayoutKind::Sequence { element: child };
                    r.size = SizeSpec::Dynamic(vec![
                        SizeTerm::Constant(8),
                        SizeTerm::ReadLength {
                            offset: 0,
                            scalar: ScalarKind::U64,
                        },
                        SizeTerm::Constant(stride),
                        SizeTerm::Multiply,
                        SizeTerm::Add,
                    ]);
                }
                t.table.layouts.push(r);
                t.ret(ENTRY, None);
                t.check()
            },
            |k| layout_obligation(k, "dynamic-extent element"),
        );
    }
}

/// MEM-13: a direct sequence of dynamic Packed elements stores them back to back
/// after a u64 count and a u64 count of used bytes (LIR schema revision 19).
fn sequential<S: hirsel_lir::stage::Stage>(t: &mut Lir<S>, element: LayoutId) -> LayoutId {
    let id = LayoutId(t.table.layouts.len() as u32);
    let mut r = t.table.layouts[l::SEQ.0 as usize].clone();
    r.kind = LayoutKind::Sequential { element };
    r.size = SizeSpec::Dynamic(vec![
        SizeTerm::Constant(16),
        SizeTerm::ReadLength {
            offset: 8,
            scalar: ScalarKind::U64,
        },
        SizeTerm::Add,
    ]);
    r.fields = vec![field(0, 0, l::U64), field(1, 8, l::U64)];
    r.scan.clear();
    r.drop.clear();
    r.copy = CopyRecipe::NotCopyable;
    t.table.layouts.push(r);
    id
}

#[test]
fn c_requires_the_sequential_layout_of_a_direct_packed_sequence() {
    let cases: [(&str, fn(&mut Lir<Value>, LayoutId, LayoutId)); 6] = [
        ("sequential size", |t, s, _| {
            t.table.layouts[s.0 as usize].size = SizeSpec::Dynamic(vec![
                SizeTerm::Constant(16),
                SizeTerm::ReadLength {
                    offset: 0,
                    scalar: ScalarKind::U64,
                },
                SizeTerm::Add,
            ]);
        }),
        ("byte count at 8", |t, s, _| {
            t.table.layouts[s.0 as usize].fields[1].offset = 16;
        }),
        ("dynamic Packed elements", |t, s, _| {
            t.table.layouts[s.0 as usize].kind = LayoutKind::Sequential { element: l::U64 };
        }),
        ("Packed extent", |t, _, e| {
            let SizeSpec::Dynamic(program) = &mut t.table.layouts[e.0 as usize].size else {
                unreachable!()
            };
            let [SizeTerm::ByTag { variants, .. }] = program.as_mut_slice() else {
                unreachable!()
            };
            variants.push((7, vec![SizeTerm::Constant(32)]));
        }),
        // Packed[T] is Flat (MEM-12): an element never holds a resource.
        ("no reference or resource", |t, _, e| {
            t.table.layouts[e.0 as usize].drop = vec![DropStep::Finalize];
        }),
        ("drops nothing", |t, s, e| {
            t.table.layouts[s.0 as usize].drop = vec![DropStep::ElementsReverse {
                count_offset: 0,
                offset: 16,
                stride: 24,
                element: e,
            }];
        }),
    ];
    for (detail, mutate) in cases {
        pair(
            |bad| {
                let mut t = Lir::<Value>::new(&[], None);
                let element = packed(&mut t);
                let s = sequential(&mut t, element);
                if bad {
                    mutate(&mut t, s, element);
                }
                t.ret(ENTRY, None);
                t.check()
            },
            |k| layout_obligation(k, detail),
        );
    }
}

/// A `List[Packed[Choice]]` derived from types, with Choice's `Large` arm holding `extra`.
fn derive_packed_list(
    extra: hirsel_types::Primitive,
) -> Result<(hirsel_lir::layout_types::LayoutTable, LayoutId), hirsel_lir::layout::LayoutError> {
    use hirsel_lir::layout::{LayoutBuilder, Owners};
    use hirsel_types::*;
    let mut store = TypeStore::default();
    let mut add = |kind| {
        store.push(Type {
            sendability_requirement: SendabilityRequirement::None,
            allocation_target: None,
            drop_allocation: DropAllocation::NoAlloc,
            kind,
            modes: Modes::global(),
            erased_captures: None,
            brand: Brand::default(),
        })
    };
    let u = add(TypeKind::Primitive(Primitive::U64));
    let x = add(TypeKind::Primitive(extra));
    let choice = add(TypeKind::Variant(
        [
            Variant {
                name: "Small".into(),
                fields: vec![(None, u)].into(),
            },
            Variant {
                name: "Large".into(),
                fields: vec![(None, u), (None, x), (None, u)].into(),
            },
        ]
        .into_iter()
        .collect(),
    ));
    let packed = add(TypeKind::Builtin {
        kind: BuiltinType::Packed,
        arguments: vec![GenericArgument::Type(choice)].into(),
    });
    let list = add(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(packed)].into(),
    });
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut b = LayoutBuilder::new(&target, &owners);
    let id = b.layout(&store, list)?;
    Ok((b.finish(), id))
}

#[test]
fn derivation_lays_a_direct_dynamic_packed_list_out_sequentially() {
    use hirsel_lir::layout_validate::validate_table;
    let (table, id) = derive_packed_list(hirsel_types::Primitive::U64).unwrap();
    validate_table(&table, &TargetSpec::x86_64_unknown_linux_gnu()).unwrap();
    let LayoutKind::Block { payload } = table.layout(id).unwrap().kind else {
        panic!("a list is a block")
    };
    let record = table.layout(payload).unwrap();
    let LayoutKind::Sequential { element } = record.kind else {
        panic!(
            "a direct dynamic Packed list is sequential: {:?}",
            record.kind
        )
    };
    assert_eq!(
        hirsel_lir::layout::packed_extents(table.layout(element).unwrap()),
        Some(vec![(0, 16), (1, 32)])
    );
    assert!(record.scan.is_empty() && record.drop.is_empty());
    let d = hirsel_lir::rt_program::descriptor(&table, payload).unwrap();
    assert!(d.steps.is_empty());
}

/// The twin: an element holding a reference, which the Flat bound on Packed excludes
/// (MEM-12), has no sequential layout.
#[test]
fn derivation_refuses_a_sequential_element_with_references() {
    let error = derive_packed_list(hirsel_types::Primitive::String).unwrap_err();
    assert!(
        format!("{error:?}").contains("references or resources"),
        "{error:?}"
    );
}

/// What a sequential-append fixture varies (LIR schema revision 19).
#[derive(Clone, Copy, PartialEq)]
enum Append {
    Valid,
    /// The place holds a stride sequence block, not a sequential one.
    NotSequential,
    /// The appended value is not of the element layout.
    OtherElement,
    /// The unit was imported from TIR.
    Source,
}

/// `SeqAppend` of an owned Packed parameter to a `mut` parameter holding a
/// sequential sequence block.
fn append(case: Append) -> Vec<K> {
    use hirsel_core_contract::Passing;
    use hirsel_lir::ir::*;
    use hirsel_lir::stage::Rc;
    let mut t = Lir::<Rc>::new(&[], None);
    let element = packed(&mut t);
    let sequence = sequential(&mut t, element);
    // The fixture's sequence block holds the sequential payload instead.
    if case != Append::NotSequential {
        t.table.layouts[l::SEQBLK.0 as usize].kind = LayoutKind::Block { payload: sequence };
    }
    let list = l::SEQBLK;
    let value = if case == Append::OtherElement {
        l::U64
    } else {
        element
    };
    let abi = t.abi(&[(Passing::Mut, list), (Passing::Take, value)], None);
    t.unit.header.abi = abi;
    if case != Append::Source {
        t.unit.header.origin = UnitOrigin::Library;
    }
    let s = t.slot(list, SlotKind::Parameter(0), ROOT);
    let sv = t.version_of(s);
    t.param(sv);
    let v = t.owned_or_plain(value);
    t.param(v);
    let at = t.at(ENTRY, s, &[], &[]);
    let version = t.version_of(s);
    t.push(
        ENTRY,
        Inst::SeqAppend {
            address: at,
            value: v,
            version,
        },
    );
    t.ret(ENTRY, None);
    t.check()
}

/// C: an append names a sequential block's place and a value of its element
/// layout, and only a library unit appends.
#[test]
fn c_requires_a_sequential_append_of_its_element_in_a_library_unit() {
    let bad = |k: &K| *k == K::Type("sequential append");
    pair(
        |m| {
            append(if m {
                Append::NotSequential
            } else {
                Append::Valid
            })
        },
        bad,
    );
    pair(
        |m| {
            append(if m {
                Append::OtherElement
            } else {
                Append::Valid
            })
        },
        bad,
    );
    pair(
        |m| append(if m { Append::Source } else { Append::Valid }),
        |k| *k == K::Vocabulary(hirsel_lir::effect::Opcode::SeqAppend),
    );
}

/// The end of element `s[i]` of a sequential sequence, located under `s`'s
/// first version, then an append to `s` (`SeqAppend` of a Packed element, or
/// `FlatAppend` of a flat `Item`). Growth may move the elements to a new block
/// and release the old one: a use of the old address reaches the released
/// block, so the twin prepares it again (validator version 7).
fn append_address(flat: bool, mutant: bool) -> Vec<K> {
    use hirsel_core_contract::Passing;
    use hirsel_lir::ir::*;
    use hirsel_lir::stage::Rc;
    let mut t = Lir::<Rc>::new(&[], Some(l::U64));
    let element = if flat { l::FLAT_WRAP } else { packed(&mut t) };
    let sequence = sequential(&mut t, element);
    t.table.layouts[l::SEQBLK.0 as usize].kind = LayoutKind::Block { payload: sequence };
    let mut params = vec![(Passing::Mut, l::SEQBLK), (Passing::Borrow, l::U64)];
    if flat {
        params.extend([(Passing::Borrow, l::SLICE), (Passing::Borrow, l::U64)]);
    } else {
        params.push((Passing::Take, element));
        t.unit.header.origin = UnitOrigin::Library;
    }
    let abi = t.abi(&params, Some(l::U64));
    t.unit.header.abi = abi;
    let s = t.slot(l::SEQBLK, SlotKind::Parameter(0), ROOT);
    let first = t.version_of(s);
    t.param(first);
    let i = t.plain(l::U64);
    t.param(i);
    let whole = t.at(ENTRY, s, &[], &[]);
    let steps = [ProjectionStep::Index(i)];
    let at = t.at(ENTRY, s, &steps, &[]);
    let end = |t: &mut Lir<Rc>, address| {
        let result = t.plain(l::U64);
        t.push(ENTRY, Inst::FlatNext { result, address });
        result
    };
    end(&mut t, at);
    let second = t.version_of(s);
    if flat {
        let text = t.plain(l::SLICE);
        t.param(text);
        let k = t.plain(l::U64);
        t.param(k);
        let symbol = t.symbol(SymbolKind::Type);
        let placement = t.heap();
        let construct = t.construct_site(
            ConstructorKind::Record(symbol),
            l::FLAT_ITEM,
            &[(0, text), (1, k)],
            placement,
        );
        t.push(
            ENTRY,
            Inst::FlatAppend {
                address: whole,
                construct,
                version: second,
            },
        );
    } else {
        let v = t.owned_or_plain(element);
        t.param(v);
        t.push(
            ENTRY,
            Inst::SeqAppend {
                address: whole,
                value: v,
                version: second,
            },
        );
    }
    let again = if mutant {
        at
    } else {
        t.at(ENTRY, s, &steps, &[])
    };
    let r = end(&mut t, again);
    t.ret(ENTRY, Some(r));
    t.check()
}

#[test]
fn an_append_stales_an_element_address_prepared_before_it() {
    crate::invalid_ir::stale_address_only(|m| append_address(false, m));
}

#[test]
fn a_flat_append_stales_an_element_address_prepared_before_it() {
    crate::invalid_ir::stale_address_only(|m| append_address(true, m));
}
