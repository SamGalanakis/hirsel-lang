//! Runtime complement to the compiled Item/Batch fixture. Rust's nonoverlap
//! discipline (core/src/ptr/mod.rs:490-567 at f6c1107f8be1, MIT OR Apache-2.0)
//! supplies the range check; Hirsel supplies the header and region contract.
use crate::support::*;
use hirsel_rt::copy::hirsel_rt_v2_unique_copy;
use hirsel_rt::region::*;
use hirsel_rt::release::hirsel_rt_v2_release;
use hirsel_rt::{HEADER_SIZE, Header, Region};
use std::{mem::MaybeUninit, ptr};

fn disjoint(a: usize, na: usize, b: usize, nb: usize) -> bool {
    match (a.checked_add(na), b.checked_add(nb)) {
        (Some(end_a), Some(end_b)) => end_a <= b || end_b <= a,
        _ => false,
    }
}

fn bytes(h: *mut Header, len: usize) -> Vec<u8> {
    assert_eq!(payload(h).addr(), h.addr() + HEADER_SIZE);
    assert!(unsafe { (*h).capacity() } >= len as u64);
    unsafe { std::slice::from_raw_parts(payload(h), len) }.to_vec()
}

fn copy(ctx: &Ctx, source: *mut Header) -> *mut Header {
    let mut out = ptr::null_mut();
    assert_eq!(
        unsafe { hirsel_rt_v2_unique_copy(ctx.0, source, ptr::null_mut(), 0, 0, &mut out) },
        0
    );
    out
}

fn release(ctx: &Ctx, h: *mut Header) {
    assert_eq!(
        unsafe { hirsel_rt_v2_release(ctx.0, h, (*h).index(), ptr::null(), 0, 0) },
        0
    );
}

#[test]
fn flat_native_copy_oracle_17_runtime_extents_and_lifetimes() {
    // A handwritten Item("é🐑",7), including an unaligned fixed field at 14.
    let expected = [
        6, 0, 0, 0, 0, 0, 0, 0, 0xc3, 0xa9, 0xf0, 0x9f, 0x90, 0x91, 7, 0, 0, 0, 0, 0, 0, 0,
    ];
    for regional in [false, true] {
        for original_first in [false, true] {
            let ctx = Ctx::diagnostic();
            let shape = leak(hirsel_rt::Layout {
                align: 1,
                flags: hirsel_rt::LAYOUT_DYNAMIC,
                scan_words: 0,
                ..*leaf(16)
            });
            let index = index(ctx.0, shape);
            let mut region = MaybeUninit::<Region>::uninit();
            let mut source = alloc_block(ctx.0, shape, expected.len() as u64);
            if regional {
                release(&ctx, source);
                unsafe { hirsel_rt_v2_region_enter(ctx.0, region.as_mut_ptr()) };
                let mut storage = ptr::null_mut();
                assert_eq!(
                    unsafe {
                        hirsel_rt_v2_region_reserve(ctx.0, region.as_mut_ptr(), 46, 8, &mut storage)
                    },
                    0
                );
                source = unsafe { storage.add(16) }.cast();
                unsafe {
                    hirsel_rt_v2_region_owner(ctx.0, region.as_mut_ptr(), source, index, 22, 8)
                };
                assert_eq!(unsafe { (*source).domain() }, hirsel_rt::DOMAIN_REGION);
                assert_eq!(ctx.stats().active_regions, 1);
            }
            unsafe { ptr::copy_nonoverlapping(expected.as_ptr(), payload(source), expected.len()) };
            let a = copy(&ctx, source);
            let b = copy(&ctx, source);
            for (i, h) in [source, a, b].into_iter().enumerate() {
                assert_eq!(bytes(h, 22), expected);
                for other in [source, a, b].into_iter().take(i) {
                    assert!(disjoint(h.addr(), 30, other.addr(), 30));
                }
            }
            unsafe {
                payload(a).add(14).cast::<u64>().write_unaligned(11);
                payload(b).add(14).cast::<u64>().write_unaligned(19);
            }
            let mut ea = expected;
            ea[14] = 11;
            let mut eb = expected;
            eb[14] = 19;
            assert_eq!(bytes(source, 22), expected);
            assert_eq!(bytes(a, 22), ea);
            assert_eq!(bytes(b, 22), eb);
            let destroy_source = || {
                if regional {
                    assert_eq!(
                        unsafe { hirsel_rt_v2_region_exit(ctx.0, region.as_mut_ptr(), 0, 0) },
                        0
                    );
                } else {
                    release(&ctx, source);
                }
            };
            if original_first {
                let mut destroy_source = destroy_source;
                destroy_source();
                assert_eq!(bytes(a, 22), ea);
                assert_eq!(bytes(b, 22), eb);
                release(&ctx, b);
                assert_eq!(bytes(a, 22), ea);
                release(&ctx, a);
            } else {
                release(&ctx, a);
                assert_eq!(bytes(b, 22), eb);
                release(&ctx, b);
                assert_eq!(bytes(source, 22), expected);
                let mut destroy_source = destroy_source;
                destroy_source();
            }
            let stats = ctx.stats();
            assert_eq!(
                (
                    stats.live_blocks,
                    stats.live_bytes,
                    stats.active_regions,
                    stats.region_active_bytes
                ),
                (0, 0, 0, 0)
            );
        }
    }
}

#[test]
fn flat_native_copy_oracle_17_range_controls() {
    assert!(disjoint(100, 22, 122, 22));
    assert!(!disjoint(100, 22, 100, 22));
    assert!(!disjoint(100, 22, 108, 22));
    assert!(!disjoint(usize::MAX - 2, 22, 100, 22));
}
