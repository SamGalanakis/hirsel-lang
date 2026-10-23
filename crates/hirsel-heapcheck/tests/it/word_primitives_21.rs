//! The total construct's failure payload is observable directly here, before Option mapping.
use crate::fixtures::*;
use hirsel_heapcheck::{
    Config,
    build::{Layouts, path},
};
use hirsel_lir::{ir::*, layout_types::*, stage::Rc};

#[test]
fn word_primitives_21_failure_payload_is_zero() {
    let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let c = common(&mut layouts);
    let byte = layouts.scalar(ScalarKind::U8);
    let pointer = layouts.scalar(ScalarKind::Pointer);
    let slice = layouts.record(&[pointer, c.u64, c.u64]);
    {
        let r = layouts.get_mut(slice);
        r.kind = LayoutKind::Slice { element: byte };
        r.scan = vec![ScanStep::SliceOwner { offset: 0 }];
        r.drop = vec![DropStep::Release { offset: 0 }];
        r.copy = CopyRecipe::Dup;
    }
    let sequence = layouts.record(&[c.u64, c.u64]);
    {
        let r = layouts.get_mut(sequence);
        r.kind = LayoutKind::Sequence { element: byte };
        r.size = SizeSpec::Dynamic(vec![
            SizeTerm::Constant(16),
            SizeTerm::ReadLength {
                offset: 0,
                scalar: ScalarKind::U64,
            },
            SizeTerm::Add,
        ]);
    }
    let scalar_layouts: Vec<_> = [ScalarKind::U16, ScalarKind::U32, ScalarKind::U64]
        .into_iter()
        .map(|s| layouts.scalar(s))
        .collect();
    let mut vectors = Vec::new();
    for lanes in [1u8, 3, 8, 16, 17, 32, 64] {
        let vector = layouts.record(&[]);
        let r = layouts.get_mut(vector);
        r.kind = LayoutKind::Simd {
            element: byte,
            lanes: u32::from(lanes),
        };
        r.size = SizeSpec::Fixed(u64::from(lanes));
        r.align = 1;
        vectors.push((lanes, vector));
    }
    let mut f = F::<Rc>::new("main", c);
    let k = f.b.constant(
        ValueRepr::Aggregate,
        slice,
        ConstantValue::Bytes(vec![255; 64]),
    );
    let bytes = f.b.plain(ValueRepr::Aggregate, slice);
    f.inst(Inst::Const {
        result: bytes,
        constant: k,
    });
    let mut expected = String::new();
    for (width, layout) in [BytesWidth::U16, BytesWidth::U32, BytesWidth::U64]
        .into_iter()
        .zip(scalar_layouts)
    {
        for endian in [ByteOrder::Little, ByteOrder::Big] {
            for at in [65 - u64::from(width.bytes()), 64, u64::MAX] {
                let offset = f.int(at.into());
                let result = f.b.plain(ValueRepr::Scalar(width.scalar()), layout);
                let valid = f.b.plain(BOOL, c.bool);
                f.inst(Inst::BytesRead {
                    result,
                    valid,
                    bytes,
                    offset,
                    width,
                    endian,
                });
                f.print(result);
                f.print(valid);
                expected.push_str("0\n0\n");
            }
        }
    }
    for (lanes, layout) in vectors {
        for at in [65 - u64::from(lanes), 64, u64::MAX] {
            let offset = f.int(at.into());
            let result = f.b.plain(ValueRepr::Aggregate, layout);
            let valid = f.b.plain(BOOL, c.bool);
            f.inst(Inst::BytesVectorRead {
                result,
                valid,
                bytes,
                offset,
                lanes,
            });
            let mask = f.b.plain(U64, c.u64);
            f.inst(Inst::ByteVector {
                result: mask,
                op: ByteVectorOp::Bitmask { value: result },
                lanes,
            });
            f.print(mask);
            f.print(valid);
            expected.push_str("0\n0\n");
        }
    }
    f.ret(None);
    let unit = f.finish(&layouts.table);
    let (target, table) = layouts.finish();
    let p = program(target, table, vec![unit]);
    let run = run(&p, &path("main"), &[], &mut Printer, &Config::default());
    let audit = run.result.unwrap();
    assert_eq!(
        String::from_utf8(audit.observation.output).unwrap(),
        expected
    );
}
