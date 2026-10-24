use super::*;
use crate::Header;
use crate::codec::*;
use std::ptr;

fn program(nodes: &[Vec<u64>]) -> Vec<u64> {
    let mut words = vec![CODEC_VERSION, nodes.len() as u64, 0, 0, 0, 0, NO_NODE];
    words.resize(7 + nodes.len(), 0);
    for (index, node) in nodes.iter().enumerate() {
        words[7 + index] = words.len() as u64;
        words.extend_from_slice(node);
    }
    words
}
fn children<'p, 'v>(cursor: Cursor<'p, 'v>) -> Vec<Cursor<'p, 'v>> {
    let Value::Children { children, .. } = cursor.read() else {
        panic!("expected children")
    };
    children
}
fn integer(cursor: Cursor<'_, '_>) -> u64 {
    let Value::Scalar { bytes, .. } = cursor.read() else {
        panic!("expected scalar")
    };
    match bytes.len() {
        1 => u64::from(bytes[0]),
        2 => u64::from(u16::from_ne_bytes(bytes.try_into().unwrap())),
        4 => u64::from(u32::from_ne_bytes(bytes.try_into().unwrap())),
        8 => u64::from_ne_bytes(bytes.try_into().unwrap()),
        _ => panic!("unexpected integer width"),
    }
}
unsafe fn image<T>(value: &T) -> &[u8] {
    // Test values below have explicit initialized fields with no padding.
    unsafe { std::slice::from_raw_parts(ptr::from_ref(value).cast(), size_of::<T>()) }
}
#[repr(C)]
struct Block<T> {
    header: Header,
    data: T,
}
fn block<T>(data: T) -> Block<T> {
    // Deliberately unrelated descriptor/count words. Access is borrowed from
    // its validated program, not Header.domain/index/count interpretation.
    Block {
        header: Header {
            count: 17,
            descriptor: u32::MAX,
        },
        data,
    }
}
fn owner<T>(block: &Block<T>) -> *mut Header {
    ptr::from_ref(&block.header).cast_mut()
}

#[test]
fn fields_and_scalar_bits_keep_actual_typed_addresses() {
    let access = Access::parse(&program(&[
        vec![NODE_FIELDS, 2, 0, 1, 8, 2],
        vec![NODE_SCALAR, 4],
        vec![NODE_SCALAR, 11],
    ]));
    let mut value = [0u8; 16];
    value[..8].copy_from_slice(&19u64.to_ne_bytes());
    value[8..12].copy_from_slice(&0x80000000u32.to_ne_bytes());
    let root = unsafe { access.borrow(&value) };
    assert_eq!(root.node(), 0);
    assert_eq!(root.address(), Some(value.as_ptr()));
    let fields = children(root);
    assert_eq!(fields.len(), 2);
    assert_eq!(integer(fields[0]), 19);
    assert_eq!(fields[1].address(), Some(unsafe { value.as_ptr().add(8) }));
    let Value::Scalar { code, bytes } = fields[1].read() else {
        panic!("expected float bits")
    };
    assert_eq!(code, 11);
    assert_eq!(bytes, 0x80000000u32.to_ne_bytes());
    let snapshot = fields[1];
    assert_eq!(snapshot.address(), fields[1].address());
}

#[test]
fn box_and_checked_wrappers_remain_explicit_without_running_checks_or_counts() {
    let access = Access::parse(&program(&[
        vec![NODE_CHECKED, 999, 1],
        vec![NODE_BOX, 77, 8, 8, 2],
        vec![NODE_SCALAR, 4],
    ]));
    let source = block(31u64);
    let holder = owner(&source);
    let root = unsafe { access.borrow(image(&holder)) };
    let checked = children(root);
    assert_eq!(checked.len(), 1);
    assert_eq!(checked[0].node(), 1);
    assert_eq!(checked[0].address(), root.address());
    let boxed = children(checked[0]);
    assert_eq!(boxed.len(), 1);
    assert_eq!(boxed[0].node(), 2);
    assert_eq!(boxed[0].address(), Some(ptr::from_ref(&source.data).cast()));
    assert_eq!(integer(boxed[0]), 31);
    assert_eq!(source.header.count, 17);
}

#[test]
fn variants_visit_only_active_fields_and_null_niches_never_follow_box() {
    let access = Access::parse(&program(&[
        vec![NODE_VARIANT, 0, 0, 0, 2, 1, 8, 1, 1, 16, 1, 24, 24],
        vec![NODE_SCALAR, 4],
    ]));
    for tag in [0u64, 1] {
        let mut value = [0u8; 24];
        value[..4].copy_from_slice(&(tag as u32).to_ne_bytes());
        value[8..16].copy_from_slice(&11u64.to_ne_bytes());
        value[16..24].copy_from_slice(&22u64.to_ne_bytes());
        let root = unsafe { access.borrow(&value) };
        let Value::Children {
            ordinal, children, ..
        } = root.read()
        else {
            panic!("expected variant")
        };
        assert_eq!(ordinal, Some(tag as u32));
        assert_eq!(children.len(), 1);
        assert_eq!(integer(children[0]), if tag == 0 { 11 } else { 22 });
    }
    let access = Access::parse(&program(&[
        vec![NODE_VARIANT, 1, 0, 0, 2, 0, 1, 0, 1, 8, 8],
        vec![NODE_BOX, 2, 8, 8, 2],
        vec![NODE_SCALAR, 4],
    ]));
    let source = block(9u64);
    for holder in [ptr::null_mut(), owner(&source)] {
        let root = unsafe { access.borrow(image(&holder)) };
        let Value::Children {
            ordinal, children, ..
        } = root.read()
        else {
            panic!("expected niche")
        };
        if holder.is_null() {
            assert_eq!(ordinal, Some(0));
            assert!(children.is_empty());
        } else {
            assert_eq!(ordinal, Some(1));
            assert_eq!(integer(self::children(children[0])[0]), 9);
        }
    }
    assert_eq!(source.header.count, 17);
}

#[test]
fn framing_flat_extents_and_repeated_elements_use_borrowed_child_locations() {
    let access = Access::parse(&program(&[
        vec![NODE_FRAMED, 1],
        vec![NODE_FLAT_FIELDS, 2, 2, 3],
        vec![NODE_FLAT_TEXT, 0],
        vec![NODE_SCALAR, 4],
    ]));
    let mut value = [0u8; 27];
    value[..8].copy_from_slice(&19u64.to_ne_bytes());
    value[8..16].copy_from_slice(&3u64.to_ne_bytes());
    value[16..19].copy_from_slice(b"abc");
    value[19..].copy_from_slice(&7u64.to_ne_bytes());
    let root = unsafe { access.borrow(&value) };
    let framed = children(root);
    assert_eq!(framed[0].node(), 1);
    assert_eq!(framed[0].address(), Some(unsafe { value.as_ptr().add(8) }));
    let fields = children(framed[0]);
    assert!(matches!(fields[0].read(), Value::Bytes(b"abc")));
    assert_eq!(integer(fields[1]), 7);
    assert_eq!(fields[1].address(), Some(unsafe { value.as_ptr().add(19) }));
    let access = Access::parse(&program(&[
        vec![NODE_REPEAT, 2, 8, 1],
        vec![NODE_SCALAR, 4],
    ]));
    let values = [5u64, 6];
    let root = unsafe { access.borrow(image(&values)) };
    assert_eq!(
        children(root).into_iter().map(integer).collect::<Vec<_>>(),
        vec![5, 6]
    );
    // Two variable-width flat texts, lengths1/3, behind an explicit count.
    let access = Access::parse(&program(&[
        vec![NODE_FLAT_SEQUENCE, 16, 1, 1],
        vec![NODE_FLAT_TEXT, 0],
    ]));
    let mut sequence = [0u8; 36];
    sequence[..8].copy_from_slice(&28u64.to_ne_bytes());
    sequence[8..16].copy_from_slice(&2u64.to_ne_bytes());
    sequence[16..24].copy_from_slice(&1u64.to_ne_bytes());
    sequence[24] = b'a';
    sequence[25..33].copy_from_slice(&3u64.to_ne_bytes());
    sequence[33..].copy_from_slice(b"bcd");
    let root = unsafe { access.borrow(&sequence) };
    let values = children(root);
    assert!(matches!(values[0].read(), Value::Bytes(b"a")));
    assert!(matches!(values[1].read(), Value::Bytes(b"bcd")));
}

#[test]
fn columns_preserve_virtual_rows_and_actual_noncontiguous_field_addresses() {
    let access = Access::parse(&program(&[
        vec![NODE_COLUMNS, 2, 8, 16, 12, 1, 2, 0, 8, 3, 8, 4, 4],
        vec![NODE_CHECKED, 999, 2],
        vec![NODE_FIELDS, 2, 0, 3, 8, 4],
        vec![NODE_SCALAR, 4],
        vec![NODE_SCALAR, 3],
    ]));
    let mut data = [0u8; 40];
    data[..8].copy_from_slice(&2u64.to_ne_bytes());
    data[8..16].copy_from_slice(&2u64.to_ne_bytes());
    data[16..24].copy_from_slice(&11u64.to_ne_bytes());
    data[24..32].copy_from_slice(&22u64.to_ne_bytes());
    data[32..36].copy_from_slice(&33u32.to_ne_bytes());
    data[36..].copy_from_slice(&44u32.to_ne_bytes());
    let source = block(data);
    let holder = owner(&source);
    let root = unsafe { access.borrow(image(&holder)) };
    let rows = children(root);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.address().is_none()));
    let row = children(rows[1]);
    assert_eq!(row[0].node(), 2);
    assert!(row[0].address().is_none());
    let fields = children(row[0]);
    assert_eq!(integer(fields[0]), 22);
    assert_eq!(integer(fields[1]), 44);
    assert_ne!(
        fields[1].address(),
        Some(unsafe { fields[0].address().unwrap().add(8) })
    );
    assert_eq!(source.header.count, 17);
    let empty = Access::parse(&program(&[
        vec![NODE_COLUMNS, 2, 8, 16, 0, 1, 0],
        vec![NODE_FIELDS, 0],
    ]));
    let source = block([2u64, 2]);
    let holder = owner(&source);
    let root = unsafe { empty.borrow(image(&holder)) };
    let rows = children(root);
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert!(row.address().is_none());
        assert!(children(row).is_empty());
    }
}

#[test]
fn keyed_storage_skips_dead_slots_without_consulting_private_index_or_counts() {
    #[repr(C)]
    struct Container {
        header: Header,
        entries: *mut Header,
        index: *mut Header,
    }
    let entries = block([3u64, 3, 1, 7, 70, 0, 999, 999, 1, 9, 90]);
    let index = block(0u64);
    let container = Container {
        header: Header {
            count: 23,
            descriptor: u32::MAX,
        },
        entries: owner(&entries),
        index: owner(&index),
    };
    let holder = ptr::from_ref(&container.header).cast_mut();
    let access = Access::parse(&program(&[
        vec![
            NODE_MAP, 2, 16, 8, 0, 8, 3, 8, 16, 24, 4, 8, 8, 1, 8, 1, 16, 0, 0, 0, 0, 0,
        ],
        vec![NODE_SCALAR, 4],
    ]));
    let root = unsafe { access.borrow(image(&holder)) };
    let Value::Children {
        count,
        set,
        children,
        ..
    } = root.read()
    else {
        panic!("expected map")
    };
    assert_eq!(count, Some(2));
    assert!(!set);
    assert_eq!(
        children.into_iter().map(integer).collect::<Vec<_>>(),
        vec![7, 70, 9, 90]
    );
    assert_eq!(container.header.count, 23);
    assert_eq!(entries.header.count, 17);
    assert_eq!(index.header.count, 17);
}

#[test]
fn text_views_sequences_and_paths_are_borrowed_without_adapter_calls() {
    #[repr(C)]
    struct Text {
        owner: *mut Header,
        start: u64,
        length: u64,
    }
    #[repr(C)]
    struct ByteStorage {
        count: u64,
        capacity: u64,
        bytes: [u8; 8],
    }
    let source = block(ByteStorage {
        count: 8,
        capacity: 8,
        bytes: *b"abcdefgh",
    });
    let text = Text {
        owner: owner(&source),
        start: crate::alloc::sequence_slice_start(1) + 2,
        length: 3,
    };
    let access = Access::parse(&program(&[vec![NODE_TEXT, 0, 0, 8, 16, 2]]));
    let root = unsafe { access.borrow(image(&text)) };
    assert!(matches!(root.read(), Value::Bytes(b"cde")));
    assert_eq!(source.header.count, 17);
    let source = block([2u64, 2, 7, 8]);
    let holder = owner(&source);
    let access = Access::parse(&program(&[
        vec![NODE_SEQUENCE, 2, 8, 16, 8, 1],
        vec![NODE_SCALAR, 4],
    ]));
    let root = unsafe { access.borrow(image(&holder)) };
    assert_eq!(
        children(root).into_iter().map(integer).collect::<Vec<_>>(),
        vec![7, 8]
    );
    assert_eq!(source.header.count, 17);
    let path = 42u64;
    let access = Access::parse(&program(&[vec![NODE_PATH]]));
    assert!(matches!(
        unsafe { access.borrow(image(&path)) }.read(),
        Value::Path(42)
    ));
}

#[test]
fn mechanical_wrapper_cursor_adds_no_structural_depth_limit_or_host_recursion() {
    std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(|| {
            let depth = hirsel_core_contract::depth::STRUCTURAL_LIMIT as usize + 1;
            let mut nodes = (0..depth)
                .map(|i| vec![NODE_CHECKED, 999, (i + 1) as u64])
                .collect::<Vec<_>>();
            nodes.push(vec![NODE_SCALAR, 4]);
            let access = Access::parse(&program(&nodes));
            let value = 7u64;
            let mut cursor = unsafe { access.borrow(image(&value)) };
            let mut visited = 0;
            loop {
                match cursor.read() {
                    Value::Children { mut children, .. } => {
                        assert_eq!(children.len(), 1);
                        cursor = children.pop().unwrap();
                        visited += 1;
                    }
                    Value::Scalar { .. } => {
                        assert_eq!(integer(cursor), 7);
                        break;
                    }
                    _ => panic!("unexpected leaf"),
                }
            }
            assert_eq!(visited, depth);
        })
        .unwrap()
        .join()
        .unwrap();
}
