#![cfg(feature = "measurement")]
use crate::support;
use hirsel_rt::alloc::{hirsel_rt_v2_dispose, hirsel_rt_v2_reuse};
use hirsel_rt::copy::{hirsel_rt_v2_detach, hirsel_rt_v2_unique_copy};
use hirsel_rt::count::hirsel_rt_v2_dup;
use hirsel_rt::region::{
    hirsel_rt_v2_region_enter, hirsel_rt_v2_region_exit, hirsel_rt_v2_region_reserve,
};
use hirsel_rt::release::{hirsel_rt_v2_release, hirsel_rt_v2_reset};
use hirsel_rt::{class_of, measurement::Counters};
use std::ptr;
use support::*;

#[test]
fn a_shared_pair_copy_reset_and_reuse_has_exact_counts() {
    let ctx = Ctx::production();
    let leaf = leaf(8);
    let pair = layout(16, 2, None, vec![release_step(0, 0), release_step(1, 8)]);
    let child = alloc_block(ctx.0, leaf, 8);
    unsafe {
        hirsel_rt_v2_dup(ctx.0, child);
    }
    let parent = alloc_block(ctx.0, pair, 16);
    write_ref(parent, 0, child);
    write_ref(parent, 8, child);
    let path =
        std::env::temp_dir().join(format!("hirsel-measurement-known-{}", std::process::id()));
    unsafe { &mut *ctx.0 }.set_measurement_output(
        hirsel_rt::host_paths::RecorderDestination::pin(&path)
            .unwrap()
            .open(&[])
            .unwrap(),
    );
    let (mut detached, mut copied, mut token, mut reused, mut fresh) = (
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
    );
    unsafe {
        assert_eq!(hirsel_rt_v2_detach(ctx.0, parent, 0, &mut detached), 0);
        assert_eq!(
            hirsel_rt_v2_unique_copy(ctx.0, parent, ptr::null_mut(), 0, 0, &mut copied),
            0
        );
        assert_eq!(
            hirsel_rt_v2_release(ctx.0, detached, index(ctx.0, pair), ptr::null(), 0, 0),
            0
        );
        assert_eq!(
            hirsel_rt_v2_release(ctx.0, copied, index(ctx.0, pair), ptr::null(), 0, 0),
            0
        );
        assert_eq!(
            hirsel_rt_v2_reset(
                ctx.0,
                parent,
                index(ctx.0, pair),
                ptr::null(),
                0,
                0,
                &mut token
            ),
            0
        );
        assert!(!token.is_null());
        assert_eq!(
            hirsel_rt_v2_reuse(ctx.0, token, index(ctx.0, leaf), 16, 8, &mut reused),
            0
        );
        assert_eq!(
            hirsel_rt_v2_reuse(
                ctx.0,
                ptr::null_mut(),
                index(ctx.0, leaf),
                16,
                8,
                &mut fresh
            ),
            0
        );
        assert_eq!(
            hirsel_rt_v2_release(ctx.0, reused, index(ctx.0, leaf), ptr::null(), 0, 0),
            0
        );
        assert_eq!(
            hirsel_rt_v2_release(ctx.0, fresh, index(ctx.0, leaf), ptr::null(), 0, 0),
            0
        );
        let before_noop = (&*ctx.0).measurements().clone();
        hirsel_rt_v2_dispose(ctx.0, ptr::null_mut());
        assert_eq!((&*ctx.0).measurements(), &before_noop);
    }
    let mut outer = std::mem::MaybeUninit::uninit();
    let mut inner = std::mem::MaybeUninit::uninit();
    let mut out = ptr::null_mut();
    unsafe {
        hirsel_rt_v2_region_enter(ctx.0, outer.as_mut_ptr());
        hirsel_rt_v2_region_reserve(ctx.0, outer.as_mut_ptr(), 8, 8, &mut out);
        hirsel_rt_v2_region_enter(ctx.0, inner.as_mut_ptr());
        hirsel_rt_v2_region_reserve(ctx.0, inner.as_mut_ptr(), 8, 8, &mut out);
        assert_eq!(hirsel_rt_v2_region_exit(ctx.0, inner.as_mut_ptr(), 0, 0), 0);
        assert_eq!(hirsel_rt_v2_region_exit(ctx.0, outer.as_mut_ptr(), 0, 0), 0);
    }
    // Two original blocks + shallow root + deep root/child + null-token fresh = six.
    // Dup: original second child edge + two detach edges + repeated deep-copy edge = four.
    // Drop: two copied pairs (3 each), reset original pair (3), two leaves (1 each) = eleven.
    let mut expected = Counters {
        count_increments: 4,
        count_decrements: 11,
        cow_detaches: 1,
        deep_copies: 1,
        copied_blocks: 3,
        copied_bytes: 48,
        reuse_hits: 1,
        reuse_misses: 1,
        peak_live_bytes: 80,
        region_high_water_bytes: 2 * 65536,
        region_pool_high_water_bytes: 2 * 65536,
        ..Default::default()
    };
    expected.allocations[class_of(16)] = 6;
    expected.frees[class_of(16)] = 6;
    assert_eq!(unsafe { &*ctx.0 }.measurements(), &expected);
    assert_eq!(ctx.stats().live_blocks, 0);
    drop(ctx);
    let report = std::fs::read_to_string(&path).unwrap();
    assert!(report.starts_with("hirsel-measurement 1\nclass 16 6 6\n"));
    assert!(report.contains("count_increments 4\ncount_decrements 11\n"));
    assert!(report.ends_with("arena_high_water_bytes unavailable\n"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn nested_site_scopes_restore_the_caller_and_totals_sum_over_all_sites() {
    use hirsel_rt::measurement::{
        ProfileSite, hirsel_rt_v2_profile_enter, hirsel_rt_v2_profile_leave,
    };
    let ctx = Ctx::production();
    let layout = leaf(8);
    let block = alloc_block(ctx.0, layout, 8);
    let site = |id, line| ProfileSite {
        id,
        line,
        column: 1,
        package: c"known".as_ptr(),
        file: c"known.hir".as_ptr(),
        function: c"known::copy".as_ptr(),
        operation: c"Copy".as_ptr(),
    };
    let (first, second) = (site(1, 2), site(2, 3));
    let path =
        std::env::temp_dir().join(format!("hirsel-sites-nested-{}.json", std::process::id()));
    unsafe { &mut *ctx.0 }.set_profile_output(std::fs::File::create(&path).unwrap());
    unsafe {
        let outer = hirsel_rt_v2_profile_enter(ctx.0, &first);
        for _ in 0..3 {
            hirsel_rt_v2_dup(ctx.0, block);
        }
        let inner = hirsel_rt_v2_profile_enter(ctx.0, &second);
        for _ in 0..5 {
            hirsel_rt_v2_dup(ctx.0, block);
            assert_eq!(
                hirsel_rt_v2_release(ctx.0, block, index(ctx.0, layout), ptr::null(), 0, 0),
                0
            );
        }
        hirsel_rt_v2_profile_leave(ctx.0, inner);
        for _ in 0..3 {
            assert_eq!(
                hirsel_rt_v2_release(ctx.0, block, index(ctx.0, layout), ptr::null(), 0, 0),
                0
            );
        }
        hirsel_rt_v2_profile_leave(ctx.0, outer);
        assert_eq!(
            hirsel_rt_v2_release(ctx.0, block, index(ctx.0, layout), ptr::null(), 0, 0),
            0
        );
    }
    drop(ctx);
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (id, n) in [(1, 3), (2, 5)] {
        let row = report["sites"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .unwrap();
        assert_eq!(row["counts"]["count_increments"], n);
        assert_eq!(row["counts"]["count_decrements"], n);
    }
    assert_eq!(report["totals"]["count_increments"], 8);
    assert_eq!(report["totals"]["count_decrements"], 9);
    for (kind, count) in report["totals"].as_object().unwrap() {
        let sum: u64 = report["sites"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["counts"][kind].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(sum, count.as_u64().unwrap(), "{kind}");
    }
    std::fs::remove_file(path).unwrap();
}
