use super::*;
use crate::build::Layouts;
use crate::{Config, NoOperations};
use hirsel_lir::layout_types::{FieldLayout, SizeTerm, TargetSpec};
use hirsel_lir::operation::NativeType;
use hirsel_lir::stage::Rc;
use hirsel_lir::structural::{Shape, ShapeGraph};

fn layouts() -> Layouts {
    Layouts::new(TargetSpec::x86_64_unknown_linux_gnu())
}
fn plan(layouts: &Layouts, root: LayoutId, nodes: Vec<Shape>) -> AccessPlan {
    AccessPlan::derive(
        NativeType::Tuple(vec![]),
        ShapeGraph { root: 0, nodes },
        &layouts.table,
        root,
    )
    .unwrap()
}
fn inspect(layouts: Layouts, check: impl FnOnce(&mut Machine<'_, Rc>)) {
    let (target, layouts) = layouts.finish();
    let program = Program::<Rc> {
        target,
        layouts,
        units: vec![],
        statics: vec![],
        invocations: Default::default(),
        invocation_bodies: Vec::new(),
        pretransfer_adapters: Vec::new(),
    };
    let config = Config::default();
    let mut ops = NoOperations;
    check(&mut Machine::new(&program, &config, &mut ops));
}
fn root<'e>(plan: &AccessPlan, image: &'e Extent) -> Projection<'e> {
    Projection {
        node: plan.root,
        location: Location::Stored(At::Image(image, 0)),
    }
}
fn scalar(machine: &Machine<'_, Rc>, p: Projection<'_>) -> u128 {
    machine.bits_at(stored(p.location).unwrap(), 8).unwrap()
}
fn alloc(machine: &mut Machine<'_, Rc>, layout: LayoutId, bytes: &[u8]) -> Ptr {
    let ptr = machine.ledger.alloc(
        Owner::Heap,
        Some(layout),
        bytes.len() as u64,
        bytes.len() as u64,
        8,
        Some(Count::Finite(1)),
    );
    machine
        .ledger
        .get_mut(ptr.alloc)
        .extent
        .write_bytes(0, bytes);
    ptr
}

#[test]
fn sealed_regions_33_projection_shared_stop_and_depth_follow_are_separate() {
    let mut layouts = layouts();
    let number = layouts.scalar(ScalarKind::U64);
    let shared = layouts.shared(number);
    let plan = plan(&layouts, shared, vec![Shape::Transparent(1), Shape::Leaf]);
    inspect(layouts, |machine| {
        // Stopping must not inspect an unread frozen payload (or acquire credit).
        let empty = Extent::new(8);
        let stopped = machine
            .projection_view(&plan, root(&plan, &empty), SharedTraversal::Stop)
            .unwrap();
        assert!(stopped.shared && stopped.children.is_empty());
        assert!(
            machine
                .projection_view(&plan, root(&plan, &empty), SharedTraversal::Follow)
                .is_err()
        );
        let frozen = machine.projection_payload(shared).unwrap();
        let ptr = alloc(machine, frozen, &91u64.to_le_bytes());
        let image = machine.projection_image(&plan, &Val::Ptr(ptr)).unwrap();
        let followed = machine
            .projection_view(&plan, root(&plan, &image), SharedTraversal::Follow)
            .unwrap();
        assert!(followed.shared);
        let frozen = machine
            .projection_view(&plan, followed.children[0], SharedTraversal::Follow)
            .unwrap();
        assert_eq!(scalar(machine, frozen.children[0]), 91);
        assert_eq!(machine.ledger.get(ptr.alloc).count, Some(Count::Finite(1)));
    });
}

#[test]
fn sealed_regions_33_projection_active_variant_ignores_inactive_payload() {
    let mut layouts = layouts();
    let number = layouts.scalar(ScalarKind::U64);
    let variant = layouts.variant(&[&[], &[number]]);
    let plan = plan(
        &layouts,
        variant,
        vec![Shape::Variant(vec![vec![], vec![1]]), Shape::Leaf],
    );
    let mut image = Extent::new(16);
    image.write_bytes(0, &0u32.to_le_bytes());
    inspect(layouts, |machine| {
        let view = machine
            .projection_view(&plan, root(&plan, &image), SharedTraversal::Follow)
            .unwrap();
        assert_eq!(view.ordinal, Some(0));
        assert!(view.children.is_empty());
        image.write_bytes(0, &1u32.to_le_bytes());
        image.write_bytes(8, &42u64.to_le_bytes());
        let view = machine
            .projection_view(&plan, root(&plan, &image), SharedTraversal::Follow)
            .unwrap();
        assert_eq!(view.ordinal, Some(1));
        assert_eq!(scalar(machine, view.children[0]), 42);
    });
}

#[test]
fn sealed_regions_33_projection_soa_preserves_virtual_wrappers_and_capacity() {
    let mut layouts = layouts();
    let number = layouts.scalar(ScalarKind::U64);
    let row = layouts.record(&[number, number]);
    let columns = layouts.record(&[]);
    layouts.get_mut(columns).kind = LayoutKind::SoaColumns { element: row };
    let list = layouts.block(columns);
    let plan = plan(
        &layouts,
        list,
        vec![
            Shape::Sequence(1),
            Shape::Distinct(2),
            Shape::Fields(vec![3, 3]),
            Shape::Leaf,
        ],
    );
    inspect(layouts, |machine| {
        // Capacity 8, two live rows: second column starts at 16 + 8*8.
        let mut bytes = vec![0; 144];
        for (offset, value) in [(0, 2u64), (8, 8), (16, 11), (24, 12), (80, 21), (88, 22)] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        let ptr = alloc(machine, columns, &bytes);
        let image = machine.projection_image(&plan, &Val::Ptr(ptr)).unwrap();
        let sequence = machine
            .projection_view(&plan, root(&plan, &image), SharedTraversal::Follow)
            .unwrap();
        assert_eq!(sequence.count, Some(2));
        for (index, projection) in sequence.children.into_iter().enumerate() {
            assert!(matches!(projection.location, Location::Columns { .. }));
            let distinct = machine
                .projection_view(&plan, projection, SharedTraversal::Follow)
                .unwrap();
            assert!(matches!(distinct.node, Node::Checked { .. }));
            assert_eq!(distinct.logical, 1);
            let fields = machine
                .projection_view(&plan, distinct.children[0], SharedTraversal::Follow)
                .unwrap();
            assert_eq!(fields.logical, 2);
            assert_eq!(scalar(machine, fields.children[0]), 11 + index as u128);
            assert_eq!(scalar(machine, fields.children[1]), 21 + index as u128);
        }
        machine
            .ledger
            .get_mut(ptr.alloc)
            .extent
            .write_bytes(0, &9u64.to_le_bytes());
        assert_eq!(
            machine
                .projection_view(&plan, root(&plan, &image), SharedTraversal::Follow)
                .err()
                .unwrap()
                .kind,
            V::OutOfExtent
        );
    });
}

#[test]
fn sealed_regions_33_projection_byte_view_preserves_owner_offset_and_length() {
    let mut layouts = layouts();
    let byte = layouts.scalar(ScalarKind::U8);
    let storage = layouts.record(&[]);
    layouts.get_mut(storage).kind = LayoutKind::Sequence { element: byte };
    let owner = layouts.block(storage);
    let number = layouts.scalar(ScalarKind::U64);
    let slice = layouts.record(&[owner, number, number]);
    layouts.get_mut(slice).kind = LayoutKind::Slice { element: byte };
    let plan = plan(&layouts, slice, vec![Shape::Leaf]);
    inspect(layouts, |machine| {
        let mut bytes = vec![0; 21];
        bytes[16..].copy_from_slice(b"abcde");
        let ptr = alloc(machine, storage, &bytes);
        let image = machine.slice_image(slice, ptr, 10, 2).unwrap();
        let view = machine
            .projection_view(&plan, root(&plan, &image), SharedTraversal::Follow)
            .unwrap();
        let span = view.bytes.unwrap();
        assert_eq!(span.len, 2);
        assert_eq!(machine.projection_bytes(span).unwrap(), b"cd");
        assert!(matches!(span.at, At::Heap(p) if p.alloc == ptr.alloc && p.offset == 18));
        assert!(view.children.is_empty());
    });
}

#[test]
fn sealed_regions_33_projection_has_no_structural_depth_policy() {
    let mut layouts = layouts();
    let mut layout = layouts.scalar(ScalarKind::U64);
    let depth = 1090;
    let mut shapes = vec![Shape::Leaf];
    for _ in 0..depth {
        layout = layouts.record(&[layout]);
        shapes.insert(0, Shape::Fields(vec![0]));
    }
    for (index, shape) in shapes.iter_mut().take(depth).enumerate() {
        *shape = Shape::Fields(vec![(index + 1) as u32]);
    }
    let plan = plan(&layouts, layout, shapes);
    inspect(layouts, |machine| {
        let mut image = Extent::new(8);
        image.write_bytes(0, &7u64.to_le_bytes());
        let mut projection = root(&plan, &image);
        let mut visited = 0;
        loop {
            let view = machine
                .projection_view(&plan, projection, SharedTraversal::Follow)
                .unwrap();
            visited += 1;
            let Some(next) = view.children.first() else {
                break;
            };
            projection = *next;
        }
        assert_eq!(visited, depth + 1);
        assert_eq!(scalar(machine, projection), 7);
    });
}

#[test]
fn sealed_regions_33_projection_packed_dynamic_fields_keep_framing_offsets() {
    let mut layouts = layouts();
    let byte = layouts.scalar(ScalarKind::U8);
    let text = layouts.record(&[]);
    {
        let row = layouts.get_mut(text);
        row.kind = LayoutKind::Flat;
        row.size = SizeSpec::Dynamic(vec![
            SizeTerm::Constant(8),
            SizeTerm::ReadLength {
                offset: 0,
                scalar: ScalarKind::U64,
            },
            SizeTerm::Add,
        ]);
        row.fields = vec![FieldLayout {
            ordinal: 0,
            offset: 8,
            layout: byte,
            unaligned: false,
        }];
    }
    let number = layouts.scalar(ScalarKind::U64);
    let record = layouts.record(&[]);
    {
        let row = layouts.get_mut(record);
        row.kind = LayoutKind::Packed;
        row.align = 1;
        row.fields = vec![
            FieldLayout {
                ordinal: 0,
                offset: 0,
                layout: text,
                unaligned: true,
            },
            FieldLayout {
                ordinal: 1,
                offset: 8,
                layout: number,
                unaligned: true,
            },
        ];
    }
    let plan = plan(
        &layouts,
        record,
        vec![Shape::Fields(vec![1, 1]), Shape::Leaf],
    );
    inspect(layouts, |machine| {
        let mut image = Extent::new(19);
        image.write_bytes(0, &3u64.to_le_bytes());
        image.write_bytes(8, b"abc");
        image.write_bytes(11, &42u64.to_le_bytes());
        let view = machine
            .projection_view(&plan, root(&plan, &image), SharedTraversal::Follow)
            .unwrap();
        let text = machine
            .projection_view(&plan, view.children[0], SharedTraversal::Follow)
            .unwrap();
        assert_eq!(
            machine.projection_bytes(text.bytes.unwrap()).unwrap(),
            b"abc"
        );
        assert_eq!(scalar(machine, view.children[1]), 42);
    });
}

#[test]
fn sealed_regions_33_projection_map_tombstones_have_no_payload_occurrences() {
    let mut layouts = layouts();
    let number = layouts.scalar(ScalarKind::U64);
    let pair = layouts.record(&[number, number]);
    let entry = layouts.variant(&[&[], &[pair], &[]]);
    let storage = layouts.record(&[]);
    layouts.get_mut(storage).kind = LayoutKind::Sequence { element: entry };
    let entries = layouts.block(storage);
    let container = layouts.record(&[entries]);
    let map = layouts.block(container);
    let plan = plan(
        &layouts,
        map,
        vec![Shape::Map { key: 1, value: 1 }, Shape::Leaf],
    );
    inspect(layouts, |machine| {
        let mut bytes = vec![0; 88]; // start 16, stride 24, three entry slots
        bytes[0..8].copy_from_slice(&3u64.to_le_bytes());
        bytes[16..20].copy_from_slice(&2u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&1u32.to_le_bytes());
        bytes[48..56].copy_from_slice(&17u64.to_le_bytes());
        bytes[56..64].copy_from_slice(&23u64.to_le_bytes());
        bytes[64..68].copy_from_slice(&0u32.to_le_bytes());
        let table = alloc(machine, storage, &bytes);
        let outer = alloc(machine, container, &[0; 8]);
        machine.ledger.get_mut(outer.alloc).extent.put_cell(
            0,
            8,
            Cell::Data {
                ptr: table,
                counted: true,
                borrow: None,
            },
        );
        let image = machine.projection_image(&plan, &Val::Ptr(outer)).unwrap();
        let view = machine
            .projection_view(&plan, root(&plan, &image), SharedTraversal::Follow)
            .unwrap();
        assert_eq!(view.count, Some(1));
        assert_eq!(view.children.len(), 2);
        assert_eq!(scalar(machine, view.children[0]), 17);
        assert_eq!(scalar(machine, view.children[1]), 23);
    });
}
