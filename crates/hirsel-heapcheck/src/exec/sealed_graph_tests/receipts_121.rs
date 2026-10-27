use super::*;
use hirsel_lir::sealed_payload::*;

#[test]
fn h_receipt_121_structural_fields_and_active_variant_have_rejecting_twins() {
    for variant in [false, true] {
        let mut x = fixture();
        let number = LayoutId(0);
        let mut layouts = Layouts {
            target: x.p.target.clone(),
            table: x.p.layouts,
        };
        let layout = if variant {
            layouts.variant(&[&[], &[number, number]])
        } else {
            layouts.record(&[number, number])
        };
        x.p.layouts = layouts.table;
        let unit = &mut x.p.units[0];
        let ty = LirTypeId(unit.types.len() as u32);
        unit.types.push(LirType {
            layout: number,
            repr: ValueRepr::Scalar(ScalarKind::U64),
        });
        let values = [
            ValueId(unit.values.len() as u32),
            ValueId(unit.values.len() as u32 + 1),
        ];
        unit.values.extend(values.map(|_| ValueInfo {
            ty,
            holding: Holding::Plain,
        }));
        let fields = PoolRange::push(
            &mut unit.pools.fields,
            values
                .into_iter()
                .enumerate()
                .map(|(field, value)| FieldValue {
                    field: field as u32,
                    value,
                }),
        );
        let mut site = ConstructSite {
            arena_no_drop: false,
            tracked: false,
            constructor: if variant {
                ConstructorKind::Variant {
                    definition: SymbolId(0),
                    variant: 1,
                }
            } else {
                ConstructorKind::Tuple
            },
            layout,
            fields,
            destination: DestinationClass::Heap,
            placement: None,
            anchor: SourceAnchorId(0),
        };
        let binding = PayloadBinding {
            values: values.to_vec(),
            recipe: PayloadRecipe {
                source: unit.tables.source_anchors[0].clone(),
                operation: PayloadOperation::Allocate {
                    destination: PayloadDestination::Heap,
                },
                inputs: [11, 12]
                    .into_iter()
                    .enumerate()
                    .map(|(place, n)| PayloadOperand {
                        place: place as u32,
                        ty: T::Primitive(P::U64),
                        producer: PayloadProducer::Literal(PayloadLiteral::Integer(n)),
                        capture_slot: None,
                    })
                    .collect(),
            },
        };
        let c = config(&mut x.p, 2, &[], false);
        let mut ops = NoOperations;
        let mut m = Machine::new(&x.p, &c, &mut ops);
        let mut f = m.frame(0, Role::Entry, None);
        for (value, bits) in values.into_iter().zip([11, 12]) {
            m.define_fresh(
                &mut f,
                value,
                Val::Bits {
                    scalar: ScalarKind::U64,
                    bits,
                },
            )
            .unwrap();
        }
        let mut image = Extent::new(m.size(layout).unwrap());
        let row = &x.p.layouts.layouts[layout.0 as usize];
        let declared = if variant {
            let tag = row.tag.as_ref().unwrap();
            image.write_bytes(tag.offset, &1u32.to_le_bytes());
            &tag.variants[1].fields
        } else {
            &row.fields
        };
        for (field, bits) in declared.iter().zip([11u64, 12]) {
            image.write_bytes(field.offset, &bits.to_le_bytes());
        }
        let check = |m: &Machine<'_, Rc>, site: &ConstructSite, image: &Extent| {
            m.check_receipt_construction(
                &f,
                &[site],
                &binding,
                layout,
                &Val::Image(image.clone()),
                None,
            )
        };
        check(&m, &site, &image).unwrap();
        let mut swapped = image.clone();
        swapped.write_bytes(declared[0].offset, &12u64.to_le_bytes());
        swapped.write_bytes(declared[1].offset, &11u64.to_le_bytes());
        assert_eq!(
            check(&m, &site, &swapped).unwrap_err().kind,
            V::TypeConfusion
        );
        if variant {
            site.constructor = ConstructorKind::Variant {
                definition: SymbolId(0),
                variant: 0,
            };
            assert_eq!(check(&m, &site, &image).unwrap_err().kind, V::TypeConfusion);
        }
    }
}

#[test]
fn h_receipt_121_box_checks_the_whole_payload_with_a_repaired_control() {
    let mut x = fixture();
    let number = LayoutId(0);
    let mut layouts = Layouts {
        target: x.p.target.clone(),
        table: x.p.layouts,
    };
    let layout = layouts.block(number);
    layouts.get_mut(layout).kind = LayoutKind::Box { payload: number };
    x.p.layouts = layouts.table;
    let unit = &mut x.p.units[0];
    let ty = LirTypeId(unit.types.len() as u32);
    unit.types.push(LirType {
        layout: number,
        repr: ValueRepr::Scalar(ScalarKind::U64),
    });
    let value = ValueId(unit.values.len() as u32);
    unit.values.push(ValueInfo {
        ty,
        holding: Holding::Plain,
    });
    let fields = PoolRange::push(&mut unit.pools.fields, [FieldValue { field: 0, value }]);
    let site = ConstructSite {
        arena_no_drop: false,
        tracked: false,
        constructor: ConstructorKind::Box,
        layout: number,
        fields,
        destination: DestinationClass::Heap,
        placement: None,
        anchor: SourceAnchorId(0),
    };
    let binding = PayloadBinding {
        values: vec![value],
        recipe: PayloadRecipe {
            source: unit.tables.source_anchors[0].clone(),
            operation: PayloadOperation::Allocate {
                destination: PayloadDestination::Heap,
            },
            inputs: vec![PayloadOperand {
                place: 0,
                ty: T::Primitive(P::U64),
                producer: PayloadProducer::Literal(PayloadLiteral::Integer(11)),
                capture_slot: None,
            }],
        },
    };
    let c = config(&mut x.p, 2, &[], false);
    let mut ops = NoOperations;
    let mut m = Machine::new(&x.p, &c, &mut ops);
    let mut f = m.frame(0, Role::Entry, None);
    m.define_fresh(
        &mut f,
        value,
        Val::Bits {
            scalar: ScalarKind::U64,
            bits: 11,
        },
    )
    .unwrap();
    let ptr = m.heap_alloc(number, 8, 8).unwrap();
    for bits in [11u64, 12, 11] {
        m.ledger
            .get_mut(ptr.alloc)
            .extent
            .write_bytes(0, &bits.to_le_bytes());
        let checked =
            m.check_receipt_construction(&f, &[&site], &binding, layout, &Val::Ptr(ptr), None);
        if bits == 11 {
            checked.unwrap();
        } else {
            assert_eq!(checked.unwrap_err().kind, V::TypeConfusion);
        }
    }
}
