//! Local schema/operand controls; these do not authorize a receipt producer.
use super::*;
use crate::ir::*;
use crate::operation::{NativeType, Primitive};
use hirsel_codec::snapshot_struct;
use std::hash::{Hash, Hasher};

mod legacy {
    use super::*;
    #[derive(Hash, Default)]
    pub struct Pools {
        pub values: Vec<ValueId>,
        pub addresses: Vec<AddressId>,
        pub projections: Vec<ProjectionStep>,
        pub switch_arms: Vec<SwitchArm>,
        pub call_arguments: Vec<CallArgument>,
        pub fields: Vec<FieldValue>,
        pub captures: Vec<CaptureValue>,
        pub mask_words: Vec<u64>,
    }
    snapshot_struct!(Pools {
        values,
        addresses,
        projections,
        switch_arms,
        call_arguments,
        fields,
        captures,
        mask_words
    });
}
fn hash(value: &impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}
#[test]
fn empty_operand_pool_preserves_legacy_bytes_and_hash() {
    let old = legacy::Pools {
        values: vec![ValueId(3)],
        mask_words: vec![7],
        ..Default::default()
    };
    let pool = Pools {
        values: old.values.clone(),
        mask_words: old.mask_words.clone(),
        ..Default::default()
    };
    let bytes = hirsel_codec::to_text(&old);
    assert_eq!(hirsel_codec::to_text(&pool), bytes);
    assert_eq!(hash(&old), hash(&pool));
    assert_eq!(hirsel_codec::from_text::<Pools>(&bytes).unwrap(), pool);
    let mut present = pool.clone();
    present.receipt_inputs.push(ReceiptInput {
        original: ValueId(3),
        receipt: Some(ValueId(4)),
    });
    assert_ne!(hash(&pool), hash(&present));
    assert_ne!(hirsel_codec::to_text(&present), bytes);
    assert_eq!(
        hirsel_codec::from_text::<Pools>(&hirsel_codec::to_text(&present)).unwrap(),
        present
    );
}
#[test]
fn producer_roundtrips_and_walks_every_local_reference() {
    let inst = Inst::PayloadReceipt {
        arena: crate::ir::OptionalValueId::NONE,
        result: ValueId(9),
        value: ValueId(3),
        shape: SealDescriptorId(4),
        binding: PayloadBindingId(6),
        inputs: PoolRange::new(2, 3),
    };
    let text = hirsel_codec::to_text(&inst);
    assert_eq!(hirsel_codec::from_text::<Inst>(&text).unwrap(), inst);
    let mut refs = Vec::new();
    inst.visit(&mut |r| refs.push(r));
    assert!(refs.contains(&Ref::Value(ValueId(9))));
    assert!(refs.contains(&Ref::Value(ValueId(3))));
    assert!(refs.contains(&Ref::SealDescriptor(SealDescriptorId(4))));
    assert!(refs.contains(&Ref::PayloadBinding(PayloadBindingId(6))));
    assert!(
        refs.iter()
            .any(|r| matches!(r, Ref::ReceiptInputs(range) if range.start == 2 && range.len == 3))
    );
    assert_eq!(Inst::defs(&inst), vec![ValueId(9)]);
    assert!(crate::stage::StageKind::Value.admits(inst.opcode()));
    assert!(crate::stage::StageKind::Ready.admits(inst.opcode()));
}
#[test]
fn original_binding_ordinal_and_input_order_cannot_be_substituted() {
    let binding = PayloadBinding {
        recipe: PayloadRecipe {
            source: SourceAnchor {
                body: 0,
                block: 0,
                statement: 0,
                site: SiteRange {
                    source: 0,
                    start: 0,
                    end: 1,
                },
            },
            operation: PayloadOperation::Boundary,
            inputs: [0, 1]
                .into_iter()
                .map(|place| PayloadOperand {
                    place,
                    ty: NativeType::Primitive(Primitive::U64),
                    producer: PayloadProducer::Parameter(place),
                    capture_slot: None,
                })
                .collect(),
        },
        values: vec![ValueId(1), ValueId(2)],
    };
    let inputs = [
        ReceiptInput {
            original: ValueId(1),
            receipt: None,
        },
        ReceiptInput {
            original: ValueId(2),
            receipt: Some(ValueId(8)),
        },
    ];
    assert!(binding.check_receipt_inputs(&inputs).is_ok());
    assert!(
        binding
            .check_receipt_inputs(&[inputs[1], inputs[0]])
            .is_err()
    );
    assert!(binding.check_receipt_inputs(&inputs[..1]).is_err());
    let tables = SideTables {
        seal_payloads: vec![
            PayloadTable {
                descriptor: SealDescriptorId(5),
                bindings: vec![binding.clone()],
            },
            PayloadTable {
                descriptor: SealDescriptorId(7),
                bindings: vec![binding.clone()],
            },
        ],
        ..Default::default()
    };
    assert_eq!(
        tables.payload_binding(PayloadBindingId(1)),
        Some((SealDescriptorId(7), &binding))
    );
    assert!(tables.payload_binding(PayloadBindingId(2)).is_none());
}
