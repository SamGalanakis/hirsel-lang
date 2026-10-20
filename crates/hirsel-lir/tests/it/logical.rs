//! MEM-6 logical sizes read from layouts agree with the same table read from
//! types (`hirsel_type_analysis::logical`), across nominals, generics and niches.
use hirsel_layout::layout::{LayoutBuilder, OwnerRepresentation, Owners};
use hirsel_lir::layout::{LogicalCharge, block_charge, logical_size};
use hirsel_lir::layout_types::{LayoutKind, TargetSpec};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_tir::RepresentationRecord;
use hirsel_type_analysis::logical::{LogicalNominal, logical_size as type_size};
use hirsel_types::testing::Types as Ty;
use hirsel_types::*;

fn representation(
    name: &str,
    generics: u32,
    shape: impl FnOnce(&mut Ty) -> TypeId,
) -> RepresentationRecord {
    let mut t = Ty::default();
    let private_shape = shape(&mut t);
    RepresentationRecord {
        definition: DefPath::core(name),
        representation_revision: ContentHash::of_bytes(name.as_bytes()),
        generics: (0..generics)
            .map(|i| GenericParameter::Type {
                name: format!("T{i}"),
                bounds: Vec::new().into(),
            })
            .collect(),
        types: t.store,
        private_shape,
        layout_promises: Layout::default(),
        resource: false,
        finalizer: None,
        unsafe_ffi_package: false,
    }
}

#[test]
fn layout_and_type_logical_sizes_agree() {
    let records = [
        representation("Tree", 0, |t| {
            let me = t.named(&DefPath::core("Tree"), vec![]);
            let child = t.boxed(me);
            t.variant(&[&[], &[child, child]])
        }),
        representation("Pair", 1, |t| {
            let first = t.param(0);
            let second = t.p(Primitive::U64);
            t.record(&[first, second])
        }),
    ];
    let mut owners = Owners::default();
    for record in &records {
        owners.nominals.insert(
            record.definition.clone(),
            OwnerRepresentation::Defined(record.clone()),
        );
    }
    let resolve = |path: &DefPath| {
        records
            .iter()
            .find(|r| r.definition == *path)
            .map(|r| LogicalNominal::Representation {
                types: &r.types,
                shape: r.private_shape,
            })
    };
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut b = LayoutBuilder::new(&target, &owners);
    let mut t = Ty::default();
    let byte = t.p(Primitive::U8);
    let short = t.p(Primitive::U16);
    let word = t.p(Primitive::U64);
    let wide = t.p(Primitive::I128);
    let float = t.p(Primitive::F32);
    let flag = t.p(Primitive::Bool);
    let text = t.p(Primitive::String);
    let unit = t.unit();
    let pair_tuple = t.tuple(&[byte, word]);
    let texts = t.list(text);
    let tree = t.named(&DefPath::core("Tree"), vec![]);
    let boxed_tree = t.boxed(tree);
    let pair = t.named(&DefPath::core("Pair"), vec![GenericArgument::Type(short)]);
    let boxed_pair = t.boxed(pair);
    let inline = t.inline(float, 3);
    let either = t.variant(&[&[flag], &[wide, text], &[]]);
    let nested = t.record(&[pair, either, inline, texts]);
    let cases = [
        (byte, 1),
        (word, 8),
        (wide, 16),
        (unit, 0),
        (text, 8),
        (pair_tuple, 9),
        (texts, 8),
        (tree, 20),
        (boxed_tree, 8),
        (pair, 10),
        (boxed_pair, 8),
        (inline, 12),
        (either, 4 + 24),
        (nested, 10 + 28 + 12 + 8),
    ];
    let layouts: Vec<_> = cases
        .iter()
        .map(|(ty, _)| b.layout(&t.store, *ty).unwrap())
        .collect();
    let table = b.finish();
    for ((ty, expected), layout) in cases.iter().zip(&layouts) {
        assert_eq!(type_size(&t.store, *ty, &resolve), Ok(*expected), "{ty:?}");
        assert_eq!(logical_size(&table, *layout), Some(*expected), "{ty:?}");
    }
    // A list block charges its elements; a box block its payload.
    let list = &table.layout(layouts[6]).unwrap().kind;
    let LayoutKind::Block { payload } = *list else {
        panic!("a List is a block reference")
    };
    assert_eq!(
        block_charge(&table, payload),
        Some(LogicalCharge::PerElement(8))
    );
    let LayoutKind::Box { payload } = table.layout(layouts[8]).unwrap().kind else {
        panic!("a box is a block reference")
    };
    assert_eq!(
        block_charge(&table, payload),
        Some(LogicalCharge::Fixed(20))
    );
    assert_eq!(LogicalCharge::PerElement(8).bytes(3), 24);
}
