//! Equality evidence for the conservative FlatFormatV0/SER-2 common subset.
use hirsel_lir::layout::{LayoutBuilder, Owners};
use hirsel_lir::layout_types::{LayoutKind, SizeSpec, TargetSpec};
use hirsel_lir::layout_validate::validate_table;
use hirsel_types::wire::{Codec, Error, Limits, Value, ValueId, ValueNode};
use hirsel_types::*;

#[test]
fn canonical_fixed_flat_bytes_equal_the_codec_on_both_native_targets() {
    let mut types = TypeStore::default();
    let flag = types.push(Type::boolean());
    let word = types.push(Type {
        kind: TypeKind::Primitive(Primitive::U32),
        ..Type::unit()
    });
    let lane = types.push(Type {
        kind: TypeKind::Primitive(Primitive::U16),
        ..Type::unit()
    });
    let pair = types.push(Type {
        kind: TypeKind::Builtin {
            kind: BuiltinType::Inline,
            arguments: vec![
                GenericArgument::Type(lane),
                GenericArgument::Size(Size::Constant(2)),
            ]
            .into(),
        },
        ..Type::unit()
    });
    let record = types.push(Type {
        kind: TypeKind::Record(
            vec![
                Field {
                    name: "flag".into(),
                    ty: flag,
                    growable: false,
                },
                Field {
                    name: "word".into(),
                    ty: word,
                    growable: false,
                },
                Field {
                    name: "pair".into(),
                    ty: pair,
                    growable: false,
                },
            ]
            .into(),
        ),
        ..Type::unit()
    });
    let schema = serializable_schema(&types, record, &GenericDeclarations::default(), &mut |_| {
        Ok(None)
    })
    .unwrap()
    .unwrap();
    let codec = Codec::new(&schema).unwrap();
    let data = Value {
        root: ValueId(5),
        nodes: vec![
            ValueNode::Bool(true),
            ValueNode::Unsigned(0x12345678),
            ValueNode::Unsigned(0x1234),
            ValueNode::Unsigned(0x5678),
            ValueNode::Sequence(vec![ValueId(2), ValueId(3)]),
            ValueNode::Fields(vec![ValueId(0), ValueId(1), ValueId(4)]),
        ],
    };
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let owners = Owners::default();
        let mut builder = LayoutBuilder::new(&target, &owners);
        let flat = builder.flat(&types, record).unwrap();
        let ordinary = builder.layout(&types, record).unwrap();
        let table = builder.finish();
        validate_table(&table, &target).unwrap();
        let layout = table.layout(flat).unwrap();
        assert_eq!(layout.kind, LayoutKind::Flat);
        assert_eq!(layout.align, 1);
        assert_eq!(layout.size, SizeSpec::Fixed(9));
        assert_eq!(codec.fixed_flat_extent(), Some(9));
        assert_eq!(
            layout.fields.iter().map(|f| f.offset).collect::<Vec<_>>(),
            [0, 1, 5]
        );
        let mut bytes = vec![0; 9];
        bytes[layout.fields[0].offset as usize] = 1;
        let offset = layout.fields[1].offset as usize;
        bytes[offset..offset + 4].copy_from_slice(&0x12345678u32.to_le_bytes());
        let offset = layout.fields[2].offset as usize;
        bytes[offset..offset + 2].copy_from_slice(&0x1234u16.to_le_bytes());
        bytes[offset + 2..offset + 4].copy_from_slice(&0x5678u16.to_le_bytes());
        assert_eq!(
            codec.encode_payload(&data, Limits::default()).unwrap(),
            bytes
        );
        assert_eq!(
            codec.encode_fixed_flat(&bytes, Limits::default()).unwrap(),
            codec.encode(&data, Limits::default()).unwrap()
        );
        // Ordinary storage has padding and no format equality proof.
        let SizeSpec::Fixed(n) = table.layout(ordinary).unwrap().size else {
            panic!("fixed ordinary record")
        };
        assert_ne!(n, bytes.len() as u64);
        assert_eq!(
            codec.encode_fixed_flat(&vec![0; n as usize], Limits::default()),
            Err(Error::Schema)
        );
    }
}
