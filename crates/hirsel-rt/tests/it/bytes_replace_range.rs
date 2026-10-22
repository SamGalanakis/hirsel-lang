//! Prepared-storage ABI vectors from Rust slice::copy_within; see NOTICES.
use crate::support::*;
use hirsel_rt::release::hirsel_rt_v2_release;
use hirsel_rt::text::{
    hirsel_rt_v2_bytes_descriptor, hirsel_rt_v2_bytes_replace_range, replacement_fits,
};

#[test]
fn prepared_leaf_preserves_initialized_length_capacity_and_spare_bytes() {
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        let layout = &hirsel_rt_v2_bytes_descriptor.0;
        let mut b = alloc_block(ctx.0, layout, 144);
        write_u64(b, 0, 5);
        write_u64(b, 8, 128);
        unsafe {
            std::ptr::write_bytes(payload(b).add(16), 0xcc, 128);
            std::ptr::copy_nonoverlapping([0u8, 1, 2, 3, 4].as_ptr(), payload(b).add(16), 5);
            let original_owner = b;
            assert_eq!(
                hirsel_rt_v2_bytes_replace_range(ctx.0, &mut b, 1, original_owner, 8, 4),
                0
            );
            assert_eq!(
                std::slice::from_raw_parts(payload(b).add(16), 5),
                &[0, 0, 1, 2, 3]
            );
            assert_eq!(b, original_owner);
            assert_eq!(read_u64(b, 0), 5);
            assert_eq!(read_u64(b, 8), 128);
            assert!(
                std::slice::from_raw_parts(payload(b).add(21), 123)
                    .iter()
                    .all(|&b| b == 0xcc)
            );
            assert_eq!(
                hirsel_rt_v2_bytes_replace_range(ctx.0, &mut b, 0, original_owner, 9, 4),
                0
            );
            assert_eq!(
                std::slice::from_raw_parts(payload(b).add(16), 5),
                &[0, 1, 2, 3, 3]
            );
            for start in [0, 5] {
                assert_eq!(
                    hirsel_rt_v2_bytes_replace_range(
                        ctx.0,
                        &mut b,
                        start,
                        std::ptr::null_mut(),
                        u64::MAX,
                        0
                    ),
                    0
                );
            }
            for (start, len) in [(6, 0), (u64::MAX, 0), (u64::MAX, 1), (4, 2), (5, 1), (0, 6)] {
                assert_eq!(
                    hirsel_rt_v2_bytes_replace_range(
                        ctx.0,
                        &mut b,
                        start,
                        std::ptr::null_mut(),
                        0,
                        len
                    ),
                    1
                );
            }
            assert_eq!(
                std::slice::from_raw_parts(payload(b).add(16), 5),
                &[0, 1, 2, 3, 3]
            );
            assert_eq!(
                hirsel_rt_v2_release(ctx.0, b, (*b).index(), std::ptr::null(), 0, 0),
                0
            );
        }
    }
}

#[test]
fn subtraction_checks_u64_limits_without_pointer_construction() {
    for (len, start, count, valid) in [
        (0, 0, 0, true),
        (5, 5, 0, true),
        (5, 6, 0, false),
        (5, 4, 2, false),
        (u64::MAX, u64::MAX, 0, true),
        (u64::MAX, u64::MAX, 1, false),
        (u64::MAX, u64::MAX - 1, 1, true),
        (u64::MAX, 1, u64::MAX, false),
    ] {
        assert_eq!(replacement_fits(len, start, count), valid);
    }
}

#[test]
fn prepared_same_owner_arena_view_charges_exact_length_before_copy() {
    use hirsel_rt::copy::hirsel_rt_v2_detach;
    use hirsel_rt::count::hirsel_rt_v2_dup;
    use hirsel_rt::region::{
        ALLOC_TRACKED, hirsel_rt_v2_arena_alloc, hirsel_rt_v2_arena_bound,
        hirsel_rt_v2_arena_enter, hirsel_rt_v2_region_exit,
    };
    use hirsel_rt::{Region, STATUS_EXHAUSTED};
    // Source-level from_list cannot freeze an arena List into heap-owned Bytes.
    // Prepare the same ABI interval directly, keeping its arena owner alive.
    for ctx in [Ctx::production(), Ctx::diagnostic()] {
        let layout = &hirsel_rt_v2_bytes_descriptor.0;
        let descriptor = index(ctx.0, layout);
        for allowance in [4, 5] {
            let mut arena = std::mem::MaybeUninit::<Region>::uninit();
            unsafe {
                hirsel_rt_v2_arena_enter(ctx.0, arena.as_mut_ptr());
                let source = hirsel_rt_v2_arena_alloc(
                    ctx.0,
                    arena.as_mut_ptr(),
                    descriptor,
                    144,
                    8,
                    ALLOC_TRACKED,
                );
                write_u64(source, 0, 5);
                write_u64(source, 8, 128);
                std::ptr::copy_nonoverlapping(
                    [0u8, 1, 2, 3, 4].as_ptr(),
                    payload(source).add(16),
                    5,
                );
                // Literal initialization has already been charged; bound just
                // the remaining allowance to isolate replacement's COW charge.
                hirsel_rt_v2_arena_bound(ctx.0, arena.as_mut_ptr(), allowance);
                hirsel_rt_v2_dup(ctx.0, source);
                let mut destination = source;
                let mut prepared = std::ptr::null_mut();
                let status = hirsel_rt_v2_detach(ctx.0, destination, 0, &mut prepared);
                if allowance == 4 {
                    assert_eq!(status, STATUS_EXHAUSTED);
                    assert!(prepared.is_null());
                    assert_eq!((*arena.as_ptr()).remaining, 4);
                } else {
                    assert_eq!(status, 0);
                    destination = prepared;
                    assert_ne!(destination, source);
                    assert_eq!((*arena.as_ptr()).remaining, 0);
                    assert_eq!(
                        hirsel_rt_v2_bytes_replace_range(ctx.0, &mut destination, 1, source, 8, 4),
                        0
                    );
                    assert_eq!(
                        std::slice::from_raw_parts(payload(destination).add(16), 5),
                        &[0, 0, 1, 2, 3]
                    );
                    assert_eq!(read_u64(destination, 0), 5);
                    assert_eq!(read_u64(destination, 8), 128);
                    assert_eq!((*arena.as_ptr()).remaining, 0);
                }
                assert_eq!(
                    std::slice::from_raw_parts(payload(source).add(16), 5),
                    &[0, 1, 2, 3, 4]
                );
                assert_eq!(hirsel_rt_v2_region_exit(ctx.0, arena.as_mut_ptr(), 0, 0), 0);
            }
        }
    }
}
