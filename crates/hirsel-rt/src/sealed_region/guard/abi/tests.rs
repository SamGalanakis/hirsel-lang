use super::*;
use crate::context::{
    CONFIG_DIAGNOSTIC, Config, Stats, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
    hirsel_rt_v2_ctx_stats,
};
use crate::release::{LAYOUT_MAGIC, LAYOUT_VERSION, Layout};
use crate::sealed_region::receipt::abi::hirsel_rt_v2_receipt_release;
use crate::sealed_region::{hirsel_rt_v2_sealed_release, hirsel_rt_v2_sealed_root};

fn layout(size: u64, align: u64) -> Layout {
    Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size,
        align,
        finalizer: None,
        steps: ptr::null(),
        step_count: 0,
        field_count: 0,
        flags: 0,
        logical: 0,
        logical_count: ptr::null(),
        scan: ptr::null(),
        scan_words: 0,
    }
}
fn context(diagnostic: bool, test: impl FnOnce(*mut TaskCtx)) {
    let layouts = [
        layout(0, 1),
        layout(8, 8),
        layout(1, 1),
        layout(16, 16),
        Layout {
            scan_words: u64::MAX,
            ..layout(8, 8)
        },
    ];
    let descriptors: Vec<_> = layouts.iter().map(ptr::from_ref).collect();
    let config = Config {
        flags: if diagnostic { CONFIG_DIAGNOSTIC } else { 0 },
        layouts: descriptors.as_ptr(),
        layout_count: descriptors.len() as u32,
        ..Config::new()
    };
    let mut ctx = ptr::null_mut();
    assert_eq!(
        unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) },
        STATUS_OK
    );
    test(ctx);
    let mut stats = Stats::default();
    unsafe { hirsel_rt_v2_ctx_stats(ctx, &mut stats) };
    assert_eq!((stats.active_regions, stats.live_blocks), (0, 0));
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
}

#[test]
fn native_guard_entry_33_seals_original_bits_created_before_scope_and_reads_after_exit() {
    for diagnostic in [false, true] {
        for (index, size) in [(2, 0), (3, 8), (4, 1), (5, 16)] {
            context(diagnostic, |ctx| unsafe {
                let mut value = 73u128;
                let source = if size == 0 {
                    ptr::null()
                } else {
                    ptr::from_ref(&value).cast::<c_void>()
                };
                let receipt = hirsel_rt_v2_receipt_bits(ctx, index, source);
                assert!(!receipt.is_null());
                let mut guard = MaybeUninit::<ReceiptArenaGuard<'static>>::uninit();
                let raw = guard.as_mut_ptr().cast();
                hirsel_rt_v2_receipt_region_enter(ctx, raw);
                let base = ptr::addr_of_mut!((*guard.as_mut_ptr()).base);
                region::hirsel_rt_v2_arena_bound(ctx, base, 0);
                let bound = (*base).bound;
                let mut owner = ptr::without_provenance_mut(1);
                assert_eq!(
                    hirsel_rt_v2_seal_prepare_commit(
                        ctx, raw, source, index, receipt, 17, &mut owner
                    ),
                    STATUS_OK
                );
                assert!(!owner.is_null());
                assert_eq!(((*base).bound, (*base).remaining), (bound, 0));
                assert_eq!(hirsel_rt_v2_receipt_region_exit(ctx, raw, 0, 0), STATUS_OK);
                assert!((*ctx).region.is_null());
                hirsel_rt_v2_receipt_release(receipt);
                value = 0;
                std::hint::black_box(value);
                for _ in 0..2 {
                    let borrowed = hirsel_rt_v2_sealed_root(ctx, owner).cast::<u8>();
                    let expected = 73u128.to_ne_bytes();
                    assert_eq!(
                        std::slice::from_raw_parts(borrowed, size),
                        &expected[..size]
                    );
                }
                hirsel_rt_v2_sealed_release(ctx, owner);
            });
        }
    }
}

#[test]
fn native_guard_entry_33_preserves_ordinary_prefix_nested_scopes_and_disarm_after_owner_drop() {
    assert_eq!(size_of::<Region>(), 64);
    assert_eq!(size_of::<ReceiptArenaGuard<'_>>(), 80);
    context(true, |ctx| unsafe {
        #[repr(C)]
        struct Ordinary {
            region: MaybeUninit<Region>,
            canary: u64,
        }
        let mut ordinary = Ordinary {
            region: MaybeUninit::uninit(),
            canary: 0xaabbccddeeff0011,
        };
        region::hirsel_rt_v2_region_enter(ctx, ordinary.region.as_mut_ptr());
        assert_eq!(
            region::hirsel_rt_v2_region_exit(ctx, ordinary.region.as_mut_ptr(), 0, 0),
            STATUS_OK
        );
        assert_eq!(ordinary.canary, 0xaabbccddeeff0011);
        let value = 7u64;
        let receipt = hirsel_rt_v2_receipt_bits(ctx, 3, ptr::from_ref(&value).cast());
        let mut outer = MaybeUninit::<ReceiptArenaGuard<'static>>::uninit();
        let mut inner = MaybeUninit::<ReceiptArenaGuard<'static>>::uninit();
        hirsel_rt_v2_receipt_region_enter(ctx, outer.as_mut_ptr().cast());
        hirsel_rt_v2_receipt_region_enter(ctx, inner.as_mut_ptr().cast());
        let mut owner = ptr::null_mut();
        assert_eq!(
            hirsel_rt_v2_seal_prepare_commit(
                ctx,
                outer.as_mut_ptr().cast(),
                ptr::from_ref(&value).cast(),
                3,
                receipt,
                0,
                &mut owner
            ),
            STATUS_OK
        );
        assert_eq!((*ctx).region, ptr::addr_of_mut!((*inner.as_mut_ptr()).base));
        hirsel_rt_v2_sealed_release(ctx, owner);
        assert_eq!(
            hirsel_rt_v2_receipt_region_exit(ctx, inner.as_mut_ptr().cast(), 0, 0),
            STATUS_OK
        );
        assert_eq!(
            hirsel_rt_v2_receipt_region_exit(ctx, outer.as_mut_ptr().cast(), 0, 0),
            STATUS_OK
        );
        hirsel_rt_v2_receipt_release(receipt);
        // A demanded scope that never seals still closes ordinary backing.
        hirsel_rt_v2_receipt_region_enter(ctx, outer.as_mut_ptr().cast());
        region::hirsel_rt_v2_arena_alloc(
            ctx,
            ptr::addr_of_mut!((*outer.as_mut_ptr()).base),
            3,
            8,
            8,
            0,
        );
        assert_eq!(
            hirsel_rt_v2_receipt_region_exit(ctx, outer.as_mut_ptr().cast(), 0, 0),
            STATUS_OK
        );
    });
}

#[test]
fn native_guard_entry_33_invalid_source_contracts_refuse_before_publication() {
    const CHILD: &str = "HIRSEL_NATIVE_GUARD_33_BAD";
    if let Ok(case) = std::env::var(CHILD) {
        context(true, |ctx| unsafe {
            let value = 7u64;
            let source = ptr::from_ref(&value).cast();
            let receipt = hirsel_rt_v2_receipt_bits(ctx, 3, source);
            let mut guard = MaybeUninit::<ReceiptArenaGuard<'static>>::uninit();
            let raw = guard.as_mut_ptr().cast();
            hirsel_rt_v2_receipt_region_enter(ctx, raw);
            let mut owner = ptr::without_provenance_mut(1);
            match case.as_str() {
                "ordinary" => {
                    let mut ordinary = MaybeUninit::<Region>::uninit();
                    hirsel_rt_v2_seal_prepare_commit(
                        ctx,
                        ordinary.as_mut_ptr().cast(),
                        source,
                        3,
                        receipt,
                        0,
                        &mut owner,
                    );
                }
                "closed" => {
                    hirsel_rt_v2_receipt_region_exit(ctx, raw, 0, 0);
                    hirsel_rt_v2_seal_prepare_commit(ctx, raw, source, 3, receipt, 0, &mut owner);
                }
                "duplicate" => hirsel_rt_v2_receipt_region_enter(ctx, raw),
                "image" => {
                    let other = 8u64;
                    hirsel_rt_v2_seal_prepare_commit(
                        ctx,
                        raw,
                        ptr::from_ref(&other).cast(),
                        3,
                        receipt,
                        0,
                        &mut owner,
                    );
                }
                "storage" => {
                    region::hirsel_rt_v2_arena_alloc(
                        ctx,
                        ptr::addr_of_mut!((*guard.as_mut_ptr()).base),
                        3,
                        8,
                        8,
                        0,
                    );
                    hirsel_rt_v2_seal_prepare_commit(ctx, raw, source, 3, receipt, 0, &mut owner);
                }
                "scan" => {
                    hirsel_rt_v2_receipt_bits(ctx, 6, source);
                }
                "null" => {
                    hirsel_rt_v2_seal_prepare_commit(
                        ctx,
                        raw,
                        source,
                        3,
                        ptr::null(),
                        0,
                        &mut owner,
                    );
                }
                "consumed" => {
                    hirsel_rt_v2_seal_prepare_commit(ctx, raw, source, 3, receipt, 0, &mut owner);
                    hirsel_rt_v2_sealed_release(ctx, owner);
                    hirsel_rt_v2_seal_prepare_commit(ctx, raw, source, 3, receipt, 0, &mut owner);
                }
                "foreign" => context(true, |foreign| {
                    hirsel_rt_v2_seal_prepare_commit(
                        foreign, raw, source, 3, receipt, 0, &mut owner,
                    );
                }),
                _ => panic!("unknown case"),
            }
            panic!("bad native contract was accepted");
        });
        return;
    }
    for (case, message) in [
        ("ordinary", "foreign or closed demanded arena guard"),
        ("closed", "foreign or closed demanded arena guard"),
        ("duplicate", "demanded arena guard entered twice"),
        ("image", "invalid or incomplete seal plan contract"),
        ("storage", "invalid or incomplete seal plan contract"),
        ("scan", "certified pointer-free descriptor"),
        ("null", "null seal root receipt"),
        ("consumed", "invalid or incomplete seal plan contract"),
        ("foreign", "foreign or closed demanded arena guard"),
    ] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sealed_region::guard::abi::tests::native_guard_entry_33_invalid_source_contracts_refuse_before_publication", "--nocapture"])
            .env(CHILD, case).output().unwrap();
        assert!(!output.status.success(), "{case}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{case}: {output:?}"
        );
    }
}

#[test]
fn native_guard_entry_33_forged_false_hint_refuses_the_entered_ordinary_record() {
    const CHILD: &str = "HIRSEL_NATIVE_GUARD_33_FORGED_FALSE";
    if std::env::var_os(CHILD).is_some() {
        context(true, |ctx| unsafe {
            let value = 7u64;
            let source = ptr::from_ref(&value).cast();
            let receipt = hirsel_rt_v2_receipt_bits(ctx, 3, source);
            // Exactly what a forged false hint emits: a valid, entered ordinary
            // 64-byte record, with no journal extension or registry entry.
            let mut ordinary = MaybeUninit::<Region>::uninit();
            region::hirsel_rt_v2_region_enter(ctx, ordinary.as_mut_ptr());
            let mut owner = ptr::null_mut();
            hirsel_rt_v2_seal_prepare_commit(
                ctx,
                ordinary.as_mut_ptr().cast(),
                source,
                3,
                receipt,
                0,
                &mut owner,
            );
            panic!("forged false hint published an owner");
        });
        return;
    }
    let output=std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "sealed_region::guard::abi::tests::native_guard_entry_33_forged_false_hint_refuses_the_entered_ordinary_record", "--nocapture"])
        .env(CHILD,"1").output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("foreign or closed demanded arena guard"),
        "{output:?}"
    );
}
