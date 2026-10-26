use super::*;
use crate::codec::*;
use crate::io_real::borrowed_net_tests::allocation_calls;
fn program(nodes: &[Vec<u64>]) -> Vec<u64> {
    let mut words = vec![CODEC_VERSION, nodes.len() as u64, 0, 0, 0, 0, NO_NODE];
    words.resize(7 + nodes.len(), 0);
    for (i, node) in nodes.iter().enumerate() {
        words[7 + i] = words.len() as u64;
        words.extend_from_slice(node);
    }
    words
}
thread_local! { static DIGEST: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

unsafe fn check(p: &[u64], edges: &[u8], a: *const u8, c: *const u8) {
    unsafe {
        let before = allocation_calls();
        let admitted = crate::codec::structural::preflight(p, edges, a);
        assert_eq!(allocation_calls(), before);
        if !admitted {
            return;
        }
        let before = allocation_calls();
        let forward = crate::codec::value_access::compare(p, std::ptr::null_mut(), a, c);
        let reverse = crate::codec::value_access::compare(p, std::ptr::null_mut(), c, a);
        assert!(matches!(
            (forward, reverse),
            (0, 0) | (1, 2) | (2, 1) | (3, 3)
        ));
        let hash = crate::codec::value_access::hash(p, std::ptr::null_mut(), a, 17);
        DIGEST.with(|digest| digest.set(digest.get().rotate_left(7) ^ hash ^ u64::from(forward)));
        assert_eq!(allocation_calls(), before);
    }
}
#[repr(C)]
struct Block<T> {
    header: Header,
    data: T,
}
fn block<T>(data: T) -> Block<T> {
    Block {
        header: Header {
            count: 17,
            descriptor: 0,
        },
        data,
    }
}
fn ptr<T>(v: &T) -> *const u8 {
    std::ptr::from_ref(v).cast()
}
fn owner<T>(b: &Block<T>) -> *mut Header {
    std::ptr::from_ref(&b.header).cast_mut()
}
#[test]
fn physical_formats_and_structural_operations_allocate_nothing() {
    unsafe {
        DIGEST.with(|digest| digest.set(0));
        for code in 0..=13 {
            let p = program(&[vec![NODE_SCALAR, code]]);
            let a = if code == 0 { 1u128 } else { u128::MAX };
            let c = 1u128;
            check(&p, &[0], ptr(&a), ptr(&c));
        }
        for depth in [0, 1, 1087, 1088, 1089, 4096] {
            let mut nodes = (0..depth)
                .map(|i| vec![NODE_CHECKED, 0, i + 1])
                .collect::<Vec<_>>();
            nodes.push(vec![NODE_FIELDS, 0]);
            let p = program(&nodes);
            let edges = (0..=depth).map(|i| u8::from(i < depth)).collect::<Vec<_>>();
            check(&p, &edges, std::ptr::null(), std::ptr::null());
            check(
                &p,
                &vec![0; nodes.len()],
                std::ptr::null(),
                std::ptr::null(),
            );
        }
        let p = program(&[vec![NODE_FIELDS, 2, 0, 1, 8, 1], vec![NODE_SCALAR, 4]]);
        for a in [[1u64, 2], [1, 3], [2, 1]] {
            for c in [[1u64, 2], [1, 3], [2, 1]] {
                check(&p, &[1, 0], ptr(&a), ptr(&c));
            }
        }
        let p = program(&[
            vec![NODE_VARIANT, 0, 0, 0, 2, 1, 8, 1, 1, 16, 1, 24, 24],
            vec![NODE_SCALAR, 4],
        ]);
        for a in [[0u64, 2, 3], [1, 2, 3]] {
            for c in [[0u64, 2, 3], [1, 2, 3]] {
                check(&p, &[1, 0], ptr(&a), ptr(&c));
            }
        }
        let p = program(&[
            vec![NODE_FRAMED, 1],
            vec![NODE_FLAT_FIELDS, 2, 2, 3],
            vec![NODE_FLAT_TEXT, 0],
            vec![NODE_SCALAR, 4],
        ]);
        let mut a = [0u8; 27];
        a[..8].copy_from_slice(&19u64.to_ne_bytes());
        a[8..16].copy_from_slice(&3u64.to_ne_bytes());
        a[16..19].copy_from_slice(b"abc");
        a[19..].copy_from_slice(&7u64.to_ne_bytes());
        check(&p, &[0, 1, 0, 0], ptr(&a), ptr(&a));
        let p = program(&[vec![NODE_SEQUENCE, 0, 8, 16, 8, 1], vec![NODE_SCALAR, 4]]);
        for n in 0u64..=4 {
            let a = block([n, 4, 1, 2, 3, 4]);
            let a = owner(&a);
            check(&p, &[1, 0], ptr(&a), ptr(&a));
        }
        let p = program(&[
            vec![NODE_COLUMNS, 2, 8, 16, 12, 1, 2, 0, 8, 3, 8, 4, 4],
            vec![NODE_CHECKED, 999, 2],
            vec![NODE_FIELDS, 2, 0, 3, 8, 4],
            vec![NODE_SCALAR, 4],
            vec![NODE_SCALAR, 3],
        ]);
        let mut a = [0u8; 40];
        a[..8].copy_from_slice(&2u64.to_ne_bytes());
        a[8..16].copy_from_slice(&2u64.to_ne_bytes());
        a[16..24].copy_from_slice(&11u64.to_ne_bytes());
        a[24..32].copy_from_slice(&22u64.to_ne_bytes());
        a[32..36].copy_from_slice(&33u32.to_ne_bytes());
        a[36..].copy_from_slice(&44u32.to_ne_bytes());
        let a = block(a);
        let a = owner(&a);
        check(&p, &[1, 1, 1, 0, 0], ptr(&a), ptr(&a));
        for set in [false, true] {
            let p = program(&[
                vec![
                    if set { NODE_SET } else { NODE_MAP },
                    2,
                    16,
                    8,
                    0,
                    8,
                    3,
                    8,
                    16,
                    24,
                    4,
                    8,
                    8,
                    1,
                    8,
                    if set { NO_NODE } else { 1 },
                    16,
                    0,
                    0,
                    0,
                    0,
                    0,
                ],
                vec![NODE_SCALAR, 4],
            ]);
            let entries = block([3u64, 3, 1, 9, 90, 0, 999, 999, 1, 7, 70]);
            let container = block([owner(&entries), std::ptr::null_mut()]);
            let a = owner(&container);
            check(&p, &[1, 0], ptr(&a), ptr(&a));
        }

        // Pinned against the pre-cutover owning walker on the identical vectors.
        DIGEST.with(|digest| assert_eq!(digest.get(), 531002500612344498));
    }
}
