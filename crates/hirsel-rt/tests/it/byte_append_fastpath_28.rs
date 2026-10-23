//! Byte append boundary and ownership controls; Rust Vec fixed append vector
//! (f6c1107f8be1, alloctests/tests/vec.rs:169-180), adapted to bytes and COW.
use crate::support::*;
use hirsel_rt::Header;
use hirsel_rt::count::hirsel_rt_v2_dup;
use hirsel_rt::release::hirsel_rt_v2_release;
use hirsel_rt::text::{allocate_bytes, hirsel_rt_v2_bytes_append, hirsel_rt_v2_bytes_descriptor};

unsafe fn release(ctx: &Ctx, block: *mut Header) {
    assert_eq!(
        unsafe { hirsel_rt_v2_release(ctx.0, block, (*block).index(), std::ptr::null(), 0, 0) },
        0
    );
}
unsafe fn shown<'a>(block: *mut Header) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(payload(block).add(16), read_u64(block, 0) as usize) }
}

#[test]
fn room_full_growth_zero_slices_and_shared_sources() {
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        let layout = &hirsel_rt_v2_bytes_descriptor.0;
        index(ctx.0, layout);
        for (capacity, count, extra) in [
            (16, 5, 5),
            (10, 5, 5),
            (5, 5, 5),
            (16, 5, 0),
            (0, 0, 0),
            (0, 0, 5),
        ] {
            unsafe {
                let mut out = alloc_block(ctx.0, layout, 16 + capacity);
                write_u64(out, 0, count);
                write_u64(out, 8, capacity);
                for i in 0..count {
                    payload(out).add(16 + i as usize).write((i + 1) as u8);
                }
                let src = allocate_bytes(ctx.0, &[0xaa, 6, 7, 8, 9, 0, 0xbb]);
                let before = out;
                let stats = ctx.stats();
                #[cfg(feature = "measurement")]
                let counters = (*ctx.0).measurements().clone();
                assert_eq!(
                    hirsel_rt_v2_bytes_append(ctx.0, &mut out, src.owner, src.start + 1, extra),
                    0
                );
                let expected: Vec<u8> = (1..=count as u8)
                    .chain([6, 7, 8, 9, 0].into_iter().take(extra as usize))
                    .collect();
                assert_eq!(shown(out), expected);
                assert_eq!(shown(src.owner), &[0xaa, 6, 7, 8, 9, 0, 0xbb]);
                if capacity >= count + extra {
                    assert_eq!(out, before);
                    assert_eq!(ctx.stats(), stats);
                    #[cfg(feature = "measurement")]
                    assert_eq!((*ctx.0).measurements(), &counters);
                }
                release(&ctx, out);
                release(&ctx, src.owner);
                assert_eq!(ctx.stats().live_blocks, 0);
            }
        }
        for capacity in [5, 64] {
            unsafe {
                let mut out = alloc_block(ctx.0, layout, 16 + capacity);
                write_u64(out, 0, 5);
                write_u64(out, 8, capacity);
                std::ptr::copy_nonoverlapping([1, 2, 3, 4, 5].as_ptr(), payload(out).add(16), 5);
                let snapshot = out;
                hirsel_rt_v2_dup(ctx.0, snapshot);
                let stats = ctx.stats();
                assert_eq!(
                    hirsel_rt_v2_bytes_append(ctx.0, &mut out, snapshot, 8, 0),
                    0
                );
                assert_eq!(out, snapshot);
                assert_eq!(ctx.stats(), stats);
                assert_eq!(
                    hirsel_rt_v2_bytes_append(ctx.0, &mut out, snapshot, 9, 3),
                    0
                );
                assert_ne!(out, snapshot);
                assert_eq!(shown(out), &[1, 2, 3, 4, 5, 2, 3, 4]);
                assert_eq!(shown(snapshot), &[1, 2, 3, 4, 5]);
                release(&ctx, out);
                release(&ctx, snapshot);
                assert_eq!(ctx.stats().live_blocks, 0);
            }
        }
    }
}

#[test]
fn tracked_budget_and_exhaustion_precede_length_publication() {
    use hirsel_rt::region::{
        ALLOC_TRACKED, hirsel_rt_v2_arena_alloc, hirsel_rt_v2_arena_bound,
        hirsel_rt_v2_arena_enter, hirsel_rt_v2_arena_reserve, hirsel_rt_v2_region_exit,
    };
    use hirsel_rt::{Region, STATUS_EXHAUSTED};
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        let layout = &hirsel_rt_v2_bytes_descriptor.0;
        let descriptor = index(ctx.0, layout);
        for shared in [false, true] {
            for extra_budget in [0, 1] {
                unsafe {
                    let mut arena = std::mem::MaybeUninit::<Region>::uninit();
                    hirsel_rt_v2_arena_enter(ctx.0, arena.as_mut_ptr());
                    let original = hirsel_rt_v2_arena_alloc(
                        ctx.0,
                        arena.as_mut_ptr(),
                        descriptor,
                        80,
                        8,
                        ALLOC_TRACKED,
                    );
                    write_u64(original, 0, 3);
                    write_u64(original, 8, 64);
                    std::ptr::copy_nonoverlapping(b"abc".as_ptr(), payload(original).add(16), 3);
                    if shared {
                        hirsel_rt_v2_dup(ctx.0, original);
                    }
                    let allowance = if shared {
                        3 + extra_budget
                    } else {
                        extra_budget
                    };
                    hirsel_rt_v2_arena_bound(ctx.0, arena.as_mut_ptr(), allowance);
                    let mut out = original;
                    let status = hirsel_rt_v2_arena_reserve(ctx.0, out, 1, 1, 1, &mut out);
                    if extra_budget == 0 {
                        assert_eq!(status, STATUS_EXHAUSTED);
                        assert_eq!(out, original);
                        assert_eq!((*arena.as_ptr()).remaining, allowance);
                    } else {
                        assert_eq!(status, 0);
                        let source = allocate_bytes(ctx.0, b"x");
                        assert_eq!(
                            hirsel_rt_v2_bytes_append(
                                ctx.0,
                                &mut out,
                                source.owner,
                                source.start,
                                1
                            ),
                            0
                        );
                        assert_eq!(shown(out), b"abcx");
                        assert_eq!((*arena.as_ptr()).remaining, 0);
                        release(&ctx, source.owner);
                    }
                    if shared || extra_budget == 0 {
                        assert_eq!(shown(original), b"abc");
                    }
                    assert_eq!(hirsel_rt_v2_region_exit(ctx.0, arena.as_mut_ptr(), 0, 0), 0);
                }
                assert_eq!(ctx.stats().live_blocks, 0);
            }
        }
    }
}
