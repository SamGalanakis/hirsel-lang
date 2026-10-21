//! Representation and drain controls independent of compiler layout production.
use super::*;
use alloc::*;
use context::*;
use count::*;
use release::*;
use std::cell::Cell;
use std::ptr;

thread_local! { static DRAINS: Cell<usize> = const { Cell::new(0) }; }

/// Test-only path counter: no instrumentation is compiled into runtime archives.
pub(crate) fn count_drain() {
    DRAINS.with(|count| count.set(count.get() + 1));
}

fn step(kind: u32, offset: u64, span: u32) -> Step {
    Step {
        kind,
        field: WHOLE_VALUE,
        variant: 0,
        span,
        offset,
        stride: 0,
        count: 0,
        count_offset: NO_OFFSET,
        layout: ptr::null(),
    }
}
fn layout(size: u64, steps: &[Step], flags: u32) -> Layout {
    Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size,
        align: 8,
        finalizer: None,
        steps: steps.as_ptr(),
        step_count: steps.len() as u64,
        field_count: 0,
        flags,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: u64::MAX,
    }
}
struct Ctx(*mut TaskCtx);
impl Ctx {
    fn new(layouts: &[&Layout], diagnostic: bool) -> Self {
        let table: Vec<_> = layouts.iter().map(|l| *l as *const Layout).collect();
        let config = Config {
            layouts: table.as_ptr(),
            layout_count: table.len() as u32,
            flags: if diagnostic { CONFIG_DIAGNOSTIC } else { 0 },
            ..Config::new()
        };
        let mut ctx = ptr::null_mut();
        assert_eq!(unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) }, 0);
        Self(ctx)
    }
    fn alloc(&self, index: u32, size: u64, align: u64) -> *mut Header {
        unsafe { hirsel_rt_v2_alloc(self.0, index + 2, size, align, DOMAIN_HEAP, 0) }
    }
    fn release(&self, block: *mut Header) {
        assert_eq!(
            unsafe { hirsel_rt_v2_release(self.0, block, (*block).index(), ptr::null(), 0, 0) },
            0
        );
    }
    fn live(&self) -> u64 {
        unsafe { (*self.0).stats().live_blocks }
    }
}
impl Drop for Ctx {
    fn drop(&mut self) {
        unsafe { hirsel_rt_v2_ctx_destroy(self.0) }
    }
}

#[test]
fn cursor_preserves_every_bit_in_both_alignment_pools() {
    let leaf = layout(0, &[], 0);
    let ctx = Ctx::new(&[&leaf], false);
    for align in [8, 16, 32, 65536] {
        let block = ctx.alloc(0, 16, align);
        unsafe {
            for pc in [0, 1, (1 << 24) - 2] {
                for element in [0, 1, (1 << 37) - 1, 1 << 37, (1 << 40) - 1] {
                    let cursor = pc | element << 24;
                    (*block).set_cursor(cursor, true);
                    assert_eq!((*block).cursor(), cursor);
                    assert_eq!((*block).index(), 2);
                    assert_eq!((*block).align(), align);
                    (*block).restore_descriptor();
                }
            }
            hirsel_rt_v2_free(ctx.0, block);
        }
    }
    assert_eq!(ctx.live(), 0);
}

#[test]
fn reciprocal_is_exact_for_every_pool_displacement() {
    for pool in 0..64 {
        let stride =
            (8 + CLASS_SIZES[pool % 32] as u64).next_multiple_of(if pool < 32 { 8 } else { 16 });
        let multiplier = (1u64 << 32).div_ceil(stride);
        for delta in 0..65536 {
            assert_eq!((delta * multiplier) >> 32, delta / stride);
        }
    }
}

#[test]
fn all_class_edges_and_overalignments_keep_payload_at_plus_eight() {
    let leaf = layout(0, &[], 0);
    for diagnostic in [false, true] {
        let ctx = Ctx::new(&[&leaf], diagnostic);
        for align in [1, 2, 4, 8, 16, 32, 64, 4096, 65536, 131072] {
            for &size in &CLASS_SIZES {
                for extent in [0, size as u64, size as u64 + 1] {
                    let block = ctx.alloc(0, extent, align);
                    assert_eq!(payload(block).addr(), block.addr() + 8);
                    assert_eq!(payload(block).addr() % align as usize, 0);
                    assert!(unsafe { (*block).capacity() } >= extent);
                    assert_eq!(unsafe { (*block).align() }, align.max(8));
                    ctx.release(block);
                }
            }
        }
        assert_eq!(ctx.live(), 0);
    }
}

#[test]
fn mixed_simple_wide_shared_children_reset_and_cross_layout_reuse() {
    let steps = [
        Step {
            count: 2,
            stride: 8,
            ..step(STEP_LOOP, 0, 1)
        },
        step(STEP_RELEASE, 0, 0),
    ];
    let wide = layout(16, &steps, LAYOUT_WIDE_CURSOR);
    let leaf = layout(0, &[], 0);
    for diagnostic in [false, true] {
        let ctx = Ctx::new(&[&leaf, &wide], diagnostic);
        let child = ctx.alloc(0, 0, 8);
        unsafe {
            hirsel_rt_v2_dup(ctx.0, child);
        }
        let block = ctx.alloc(1, 16, 8);
        unsafe {
            payload(block).cast::<*mut Header>().write(child);
            payload(block).add(8).cast::<*mut Header>().write(child);
        }
        let mut token = ptr::null_mut();
        assert_eq!(
            unsafe { hirsel_rt_v2_reset(ctx.0, block, 3, ptr::null(), 0, 0, &mut token) },
            0
        );
        assert_eq!(token, block);
        assert_eq!(
            unsafe { payload(token).cast::<*mut Header>().read() },
            child
        );
        assert_eq!(
            unsafe { payload(token).add(8).cast::<*mut Header>().read() },
            child
        );
        let mut reused = ptr::null_mut();
        assert_eq!(
            unsafe { hirsel_rt_v2_reuse(ctx.0, token, 2, 0, 8, &mut reused) },
            0
        );
        ctx.release(reused);
        assert_eq!(ctx.live(), 0);
    }
}

#[cfg(not(feature = "system-allocator"))]
#[test]
fn post_order_free_replays_build_order_including_niche_slots() {
    let steps = [
        step(STEP_NONNULL, 0, 2),
        step(STEP_RELEASE, 8, 0),
        step(STEP_RELEASE, 0, 0),
    ];
    let tree = layout(16, &steps, 0);
    fn build(
        ctx: &Ctx,
        depth: u32,
        order: &mut Vec<*mut Header>,
        root: Option<*mut Header>,
    ) -> *mut Header {
        let node = root.unwrap_or_else(|| ctx.alloc(0, 16, 8));
        order.push(node);
        let left = if depth == 0 {
            ptr::null_mut()
        } else {
            build(ctx, depth - 1, order, None)
        };
        let right = if depth == 0 {
            ptr::null_mut()
        } else {
            build(ctx, depth - 1, order, None)
        };
        unsafe {
            payload(node).cast::<*mut Header>().write(left);
            payload(node).add(8).cast::<*mut Header>().write(right);
        }
        node
    }
    let ctx = Ctx::new(&[&tree], false);
    let mut first = vec![];
    let root = build(&ctx, 8, &mut first, None);
    ctx.release(root);
    // Page-local free lists deliberately defer reuse until untouched storage
    // is exhausted. Keep those fresh blocks live, then replay the exact same
    // construction order from the collected list (the root is freed last).
    let mut fresh = vec![];
    let recycled_root = loop {
        let block = ctx.alloc(0, 16, 8);
        if block == first[0] {
            break block;
        }
        unsafe {
            payload(block).write_bytes(0, 16);
        }
        fresh.push(block);
        assert!(
            fresh.len() < SLAB_SIZE / 16,
            "the page must collect before a new page"
        );
    };
    let mut second = vec![];
    let root = build(&ctx, 8, &mut second, Some(recycled_root));
    assert_eq!(first, second);
    ctx.release(root);
    for block in fresh {
        ctx.release(block);
    }
    assert_eq!(ctx.live(), 0);
}

#[test]
fn full_u32_bound_and_invalid_configuration_are_explicit() {
    let leaf = layout(0, &[], 0);
    let ctx = Ctx::new(&[&leaf], false);
    let block = ctx.alloc(0, 0, 8);
    unsafe {
        // The full width stops below the tracked arena counts (runtime ABI 11).
        (*block).count = crate::count::finite_count_max(0);
        assert_eq!((*block).count, crate::count::COUNT_ARENA_SHARED - 1);
        assert_eq!(
            crate::count::finite_count_max(u32::MAX as u64),
            crate::count::COUNT_ARENA_SHARED - 1
        );
        hirsel_rt_v2_dup(ctx.0, block);
        assert_eq!((*block).count, u32::MAX);
        hirsel_rt_v2_dup(ctx.0, block);
        ctx.release(block);
        assert_eq!((*block).count, u32::MAX);
        assert_ne!((*block).count, 1);
    }
    for max in [1, u32::MAX as u64 + 1, u64::MAX] {
        let config = Config {
            count_max: max,
            ..Config::new()
        };
        let mut invalid = ptr::null_mut();
        assert_eq!(
            unsafe { hirsel_rt_v2_ctx_create(&config, &mut invalid) },
            LAUNCH_INVALID_CONFIG
        );
        assert!(invalid.is_null());
    }
}

#[test]
fn packed_consumed_slots_and_live_bit_guards_survive_reversal() {
    let guarded = [
        Step {
            variant: 0,
            ..step(STEP_LIVE_BIT, 17, 2)
        },
        step(STEP_NONNULL, 1, 1),
        step(STEP_RELEASE, 1, 0),
    ];
    let record = layout(25, &guarded, 0);
    for diagnostic in [false, true] {
        let ctx = Ctx::new(&[&record], diagnostic);
        let leaf = ctx.alloc(0, 25, 8);
        unsafe {
            payload(leaf).add(17).cast::<u64>().write_unaligned(0);
        }
        let parent = ctx.alloc(0, 25, 8);
        let root = ctx.alloc(0, 25, 8);
        for (block, child) in [(parent, leaf), (root, parent)] {
            unsafe {
                payload(block)
                    .add(1)
                    .cast::<*mut Header>()
                    .write_unaligned(child);
                payload(block).add(17).cast::<u64>().write_unaligned(1);
            }
        }
        ctx.release(root);
        assert_eq!(ctx.live(), 0);
    }
}

thread_local! { static EVENTS: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) }; }
unsafe extern "C" fn finalizer(
    ctx: *mut TaskCtx,
    value: *mut u8,
    anchor: u64,
    _operation_site: u64,
) -> u8 {
    unsafe {
        assert_eq!(finalize::hirsel_rt_v2_shielded(ctx), 1);
        let id = value.cast::<u64>().read();
        EVENTS.with(|events| events.borrow_mut().push(id));
        // Reentrant allocation/release must restore the outer drain's work identity.
        let temporary = hirsel_rt_v2_alloc(ctx, 0, 16, 8, DOMAIN_HEAP, 0);
        hirsel_rt_v2_release(ctx, temporary, 0, ptr::null(), anchor, _operation_site);
        if id == 2 {
            failure::hirsel_rt_v2_failure_begin(ctx, 1, 2, anchor, _operation_site)
        } else {
            0
        }
    }
}

#[test]
fn reentrant_failing_finalizers_finish_siblings_and_inline_continuations() {
    let steps = [step(STEP_RELEASE, 16, 0), step(STEP_RELEASE, 8, 0)];
    let record = Layout {
        finalizer: Some(finalizer),
        ..layout(24, &steps, 0)
    };
    let leaf = Layout {
        finalizer: Some(finalizer),
        ..layout(8, &[], 0)
    };
    for diagnostic in [false, true] {
        EVENTS.with(|events| events.borrow_mut().clear());
        let ctx = Ctx::new(&[&leaf, &record], diagnostic);
        let root = ctx.alloc(1, 24, 8);
        unsafe {
            payload(root).cast::<u64>().write(1);
        }
        for (offset, id) in [(8, 3), (16, 2)] {
            let child = ctx.alloc(0, 8, 8);
            unsafe {
                payload(child).cast::<u64>().write(id);
                payload(root).add(offset).cast::<*mut Header>().write(child);
            }
        }
        let mut token = ptr::null_mut();
        assert_eq!(
            unsafe { hirsel_rt_v2_reset(ctx.0, root, 3, ptr::null(), 7, 7, &mut token) },
            1
        );
        assert!(token.is_null());
        assert_eq!(ctx.live(), 0);
        EVENTS.with(|events| assert_eq!(&*events.borrow(), &[1, 2, 3]));
        let mut value = 4u64;
        let mut continuation = std::mem::MaybeUninit::<Cleanup>::uninit();
        assert_eq!(
            unsafe {
                finalize::hirsel_rt_v2_finalize(
                    ctx.0,
                    2,
                    (&raw mut value).cast(),
                    ptr::null(),
                    continuation.as_mut_ptr(),
                    7,
                    7,
                )
            },
            0
        );
        assert_eq!(ctx.live(), 0);
        EVENTS.with(|events| assert_eq!(&*events.borrow(), &[1, 2, 3, 4]));
    }
}

#[test]
fn releasing_pointer_free_storage_never_enters_the_drain() {
    // The byte-sequence descriptor is also the backing storage of String/Bytes.
    // A scalar sequence has no release steps, regardless of its length.
    let scalar = layout(8, &[], LAYOUT_DYNAMIC);
    for diagnostic in [false, true] {
        let ctx = Ctx::new(&[&scalar], diagnostic);
        for count in [0, 1, 64] {
            let block = ctx.alloc(0, 8 + count * 8, 8);
            unsafe {
                payload(block).cast::<u64>().write(count);
                hirsel_rt_v2_dup(ctx.0, block);
            }
            let before = DRAINS.with(Cell::get);
            ctx.release(block);
            assert_eq!(ctx.live(), 1, "a shared release keeps its other holder");
            ctx.release(block);
            assert_eq!(ctx.live(), 0, "the last release frees the storage");
            assert_eq!(
                DRAINS.with(Cell::get),
                before,
                "scalar storage has nothing to drain"
            );
            assert!(unsafe { (*ctx.0).work.is_null() });
        }
    }
}

#[test]
fn a_speculative_turn_defers_root_releases_to_its_retirement() {
    let root = layout(8, &[], LAYOUT_FROZEN);
    let ctx = Ctx::new(&[&root], true);
    let block = ctx.alloc(0, 8, 8);
    unsafe {
        crate::roots::hirsel_rt_v2_root_dup(ctx.0, block);
        (*ctx.0).roots.speculative = true;
        crate::roots::hirsel_rt_v2_root_release(ctx.0, block);
        crate::roots::hirsel_rt_v2_root_release(ctx.0, block);
        assert_eq!(
            crate::roots::count(&*ctx.0, block),
            2,
            "a non-head turn's root releases wait for its retirement"
        );
        crate::roots::retire(&mut *ctx.0);
    }
    assert_eq!(ctx.live(), 0, "retirement applies them in program order");
}
