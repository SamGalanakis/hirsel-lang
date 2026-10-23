use super::*;
use crate::context::{
    CONFIG_DIAGNOSTIC, Config, Stats, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy,
    hirsel_rt_v2_ctx_stats, hirsel_rt_v2_worker_collect, hirsel_rt_v2_worker_create,
    hirsel_rt_v2_worker_retire,
};
use crate::region::{
    ALLOC_TRACKED, TRACKED_REGION_BACK, hirsel_rt_v2_arena_bound, hirsel_rt_v2_region_hold,
    hirsel_rt_v2_region_schedule,
};
use crate::release::{
    LAYOUT_FROZEN, LAYOUT_MAGIC, LAYOUT_VERSION, Layout, NO_OFFSET, STEP_RELEASE, Step, WHOLE_VALUE,
};
use crate::sealed_region::{drop_owner, root};

fn release(offset: u64, slice: bool) -> Step {
    Step {
        kind: STEP_RELEASE,
        field: WHOLE_VALUE,
        variant: 0,
        span: 0,
        offset,
        stride: if slice { 24 } else { 0 },
        count: 0,
        count_offset: NO_OFFSET,
        layout: ptr::null(),
    }
}
fn layout(size: u64, steps: &[Step]) -> Layout {
    Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size,
        align: 8,
        finalizer: None,
        steps: steps.as_ptr(),
        step_count: steps.len() as u64,
        field_count: 1,
        logical: 0,
        logical_count: ptr::null(),
        scan: ptr::null(),
        scan_words: u64::MAX,
        flags: 0,
    }
}
fn with_context(test: impl FnOnce(*mut TaskCtx)) {
    let pair_steps = [release(0, false), release(8, false)];
    let root_steps = [release(0, false)];
    let view_steps = [release(0, true), release(24, true)];
    let layouts = [
        layout(8, &[]),          // 2 plain/leaf
        layout(16, &pair_steps), // 3 pair block
        layout(8, &root_steps),  // 4 root holder
        Layout {
            flags: LAYOUT_FROZEN,
            ..layout(8, &[])
        }, // 5 Shared
        layout(48, &view_steps), // 6 two byte views
        layout(0, &[]),          // 7 zero-size block
    ];
    let pointers: Vec<_> = layouts.iter().map(ptr::from_ref).collect();
    let config = Config {
        flags: CONFIG_DIAGNOSTIC,
        layouts: pointers.as_ptr(),
        layout_count: pointers.len() as u32,
        ..Config::new()
    };
    let mut ctx = ptr::null_mut();
    assert_eq!(unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) }, 0);
    test(ctx);
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
}
unsafe fn enter(ctx: *mut TaskCtx) -> Box<Region> {
    let mut region = Box::new(MaybeUninit::<Region>::uninit());
    unsafe {
        hirsel_rt_v2_region_enter(ctx, region.as_mut_ptr());
        Box::from_raw(Box::into_raw(region).cast())
    }
}
unsafe fn stats(ctx: *mut TaskCtx) -> Stats {
    let mut stats = Stats::default();
    unsafe { hirsel_rt_v2_ctx_stats(ctx, &mut stats) };
    stats
}
unsafe fn heap(ctx: *mut TaskCtx, index: u32, size: u64) -> *mut Header {
    unsafe { crate::alloc::hirsel_rt_v2_alloc(ctx, index, size, 8, crate::DOMAIN_HEAP, 0) }
}
fn patch(offset: usize, target: usize) -> Patch {
    Patch {
        offset,
        target: BlockId(target),
    }
}
fn block(source: *mut Header, descriptor: u32, extent: usize, placement: Placement) -> Block {
    Block {
        source,
        descriptor,
        extent,
        align: 8,
        placement,
        patches: Vec::new(),
    }
}
fn root_plan(source: *mut u8, descriptor: u32, extent: usize, charge: u64) -> Plan {
    Plan {
        root: Root {
            descriptor,
            source,
            extent,
            align: 8,
            patches: Vec::new(),
        },
        blocks: Vec::new(),
        shared: Vec::new(),
        charge: Charge::Bytes(charge),
    }
}

#[test]
fn native_transaction_33_heap_diamond_evacuates_once_and_leaves_external_alias() {
    with_context(|ctx| unsafe {
        let mut arena = enter(ctx);
        let leaf = heap(ctx, 2, 8);
        payload(leaf).cast::<[u8; 8]>().write(*b"abc\0\0\0\0\0");
        crate::count::hirsel_rt_v2_dup(ctx, leaf);
        let parent = heap(ctx, 3, 16);
        payload(parent)
            .cast::<[*mut Header; 2]>()
            .write([leaf, leaf]);
        crate::count::hirsel_rt_v2_dup(ctx, parent);
        let mut source = parent;
        let mut plan = root_plan((&raw mut source).cast(), 4, 8, 22);
        plan.root.patches.push(patch(0, 0));
        let mut pair = block(parent, 3, 16, Placement::Evacuate);
        pair.patches = vec![patch(0, 1), patch(8, 1)];
        plan.blocks = vec![pair, block(leaf, 2, 8, Placement::Evacuate)];
        let owner = prepare_commit(ctx, &mut *arena, &plan).unwrap();
        assert_eq!((*parent).count, 1); // the independent external alias
        assert_eq!((*leaf).count, 2);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
        drop(arena);
        let moved = root(owner).cast::<*mut Header>().read();
        assert_ne!(moved, parent);
        crate::count::check_live_block(&*ctx, moved);
        let children = payload(moved).cast::<[*mut Header; 2]>().read();
        assert_eq!(children[0], children[1]);
        assert_ne!(children[0], leaf);
        crate::count::check_live_block(&*ctx, children[0]);
        assert_eq!(&*payload(children[0]).cast::<[u8; 3]>(), b"abc");
        // Changing independent source bytes does not change the sealed graph.
        payload(leaf).write(b'z');
        assert_eq!(payload(children[0]).read(), b'a');
        assert_eq!(
            crate::release::release_block(&mut *ctx, parent, ptr::null(), 0, 0),
            0
        );
        assert_eq!(stats(ctx).live_blocks, 0);
        drop_owner(ctx, owner);
        assert_eq!(stats(ctx).active_regions, 0);
    });
}

#[test]
fn native_transaction_33_retained_dead_and_live_arena_roots_charge_six_and_adopt() {
    for live_root in [false, true] {
        with_context(|ctx| unsafe {
            let mut arena = enter(ctx);
            let leaf = heap(ctx, 2, 8);
            payload(leaf).cast::<[u8; 8]>().write(*b"abc\0\0\0\0\0");
            crate::count::hirsel_rt_v2_dup(ctx, leaf);
            let parent = hirsel_rt_v2_arena_alloc(ctx, &mut *arena, 3, 16, 8, ALLOC_TRACKED);
            payload(parent)
                .cast::<[*mut Header; 2]>()
                .write([leaf, leaf]);
            hirsel_rt_v2_region_schedule(ctx, &mut *arena, 3, payload(parent));
            let mut source = if live_root {
                parent.addr() as u64
            } else {
                7u64
            };
            let mut plan = root_plan(
                (&raw mut source).cast(),
                if live_root { 4 } else { 2 },
                8,
                6,
            );
            if live_root {
                plan.root.patches.push(patch(0, 0));
            }
            let mut pair = block(parent, 3, 16, Placement::Adopt);
            pair.patches = vec![patch(0, 1), patch(8, 1)];
            plan.blocks = vec![pair, block(leaf, 2, 8, Placement::Evacuate)];
            hirsel_rt_v2_arena_bound(ctx, &mut *arena, 5);
            let cleanup = arena.cleanup;
            assert_eq!(
                prepare_commit(ctx, &mut *arena, &plan),
                Err(Error::Exhausted)
            );
            assert_eq!(arena.cleanup, cleanup);
            assert_eq!(arena.remaining, 5);
            assert_eq!((*leaf).count, 2);
            hirsel_rt_v2_arena_bound(ctx, &mut *arena, 6);
            let owner = prepare_commit(ctx, &mut *arena, &plan).unwrap();
            assert_eq!(arena.remaining, 0);
            assert!(arena.cleanup.is_null());
            assert_eq!((*parent).count, crate::COUNT_IMMORTAL);
            assert!(
                parent
                    .cast::<u8>()
                    .sub(TRACKED_REGION_BACK as usize)
                    .cast::<*mut Region>()
                    .read()
                    .is_null()
            );
            assert_eq!(stats(ctx).live_blocks, 0);
            assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
            drop(arena);
            crate::count::check_live_block(&*ctx, parent);
            if live_root {
                assert_eq!(root(owner).cast::<*mut Header>().read(), parent);
            } else {
                assert_eq!(root(owner).cast::<u64>().read(), 7);
            }
            let child = payload(parent).cast::<*mut Header>().read();
            assert_eq!(&*payload(child).cast::<[u8; 3]>(), b"abc");
            drop_owner(ctx, owner);
        });
    }
}

#[test]
fn native_transaction_33_virtual_shared_credits_survive_and_rollback_every_checkpoint() {
    let mut events = Vec::new();
    // First execute the actual successful preparation to enumerate checkpoints.
    for fail_at in std::iter::once(None).chain((0..6).map(Some)) {
        with_context(|ctx| unsafe {
            let mut arena = enter(ctx);
            let shared = heap(ctx, 5, 8);
            payload(shared).cast::<u64>().write(0);
            for _ in 0..2 {
                crate::roots::hirsel_rt_v2_root_dup(ctx, shared);
                hirsel_rt_v2_region_hold(ctx, &mut *arena, shared);
            }
            let mut source = 7u64;
            let mut plan = root_plan((&raw mut source).cast(), 2, 8, 0);
            // Elided semantic fields still own two independent root credits.
            plan.shared = vec![shared, shared];
            let cleanup = arena.cleanup;
            let mut index = 0;
            let result = run(ctx, &mut *arena, &plan, &mut |event| {
                if fail_at.is_none() {
                    events.push(event);
                }
                let fail = fail_at == Some(index);
                index += 1;
                if fail {
                    Err(Error::Interrupted)
                } else {
                    Ok(())
                }
            });
            if fail_at.is_some() {
                assert_eq!(result, Err(Error::Interrupted));
                assert_eq!(arena.cleanup, cleanup);
                assert_eq!((*ctx).region, ptr::from_mut(&mut *arena));
                assert_eq!(crate::roots::count(&*ctx, shared), 3);
            } else {
                let owner = result.unwrap();
                assert_eq!(crate::roots::count(&*ctx, shared), 3);
                assert!(arena.cleanup.is_null());
                drop_owner(ctx, owner);
                assert_eq!(crate::roots::count(&*ctx, shared), 1);
            }
            assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
            assert_eq!(crate::roots::count(&*ctx, shared), 1);
            crate::roots::hirsel_rt_v2_root_release(ctx, shared);
            assert_eq!(stats(ctx).live_blocks, 0);
        });
    }
    assert_eq!(
        events,
        vec![
            Event::Owner,
            Event::Snapshot,
            Event::Extension,
            Event::Shared(0),
            Event::Shared(1),
            Event::Ready
        ]
    );
}

#[test]
fn native_transaction_33_copy_interruption_keeps_source_cells_and_credits() {
    for fail_at in [Event::Block(0), Event::Block(1), Event::Ready] {
        with_context(|ctx| unsafe {
            let mut arena = enter(ctx);
            let leaf = heap(ctx, 2, 8);
            payload(leaf).cast::<u64>().write(71);
            let parent = heap(ctx, 4, 8);
            payload(parent).cast::<*mut Header>().write(leaf);
            let mut source = parent;
            let mut plan = root_plan((&raw mut source).cast(), 4, 8, 16);
            plan.root.patches = vec![patch(0, 0)];
            let mut parent_plan = block(parent, 4, 8, Placement::Evacuate);
            parent_plan.patches = vec![patch(0, 1)];
            plan.blocks = vec![parent_plan, block(leaf, 2, 8, Placement::Evacuate)];
            assert_eq!(
                run(ctx, &mut *arena, &plan, &mut |event| {
                    if event == fail_at {
                        Err(Error::Interrupted)
                    } else {
                        Ok(())
                    }
                }),
                Err(Error::Interrupted)
            );
            assert_eq!(source, parent);
            assert_eq!(payload(parent).cast::<*mut Header>().read(), leaf);
            assert_eq!((*parent).count, 1);
            assert_eq!((*leaf).count, 1);
            assert_eq!(payload(leaf).cast::<u64>().read(), 71);
            assert_eq!(stats(ctx).live_blocks, 2);
            assert_eq!(stats(ctx).active_regions, 1);
            crate::release::release_block(&mut *ctx, parent, ptr::null(), 0, 0);
            assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
            assert_eq!(stats(ctx).live_blocks, 0);
        });
    }
}

#[test]
fn native_transaction_33_distinct_empty_blocks_and_overlapping_views() {
    with_context(|ctx| unsafe {
        let mut arena = enter(ctx);
        let left = heap(ctx, 7, 0);
        let right = heap(ctx, 7, 0);
        let mut source = [left, right];
        let mut plan = root_plan(source.as_mut_ptr().cast(), 3, 16, 0);
        plan.root.patches = vec![patch(0, 0), patch(8, 1)];
        plan.blocks = vec![
            block(left, 7, 0, Placement::Evacuate),
            block(right, 7, 0, Placement::Evacuate),
        ];
        let owner = prepare_commit(ctx, &mut *arena, &plan).unwrap();
        let moved = root(owner).cast::<[*mut Header; 2]>().read();
        assert_ne!(moved[0], moved[1]);
        assert_ne!(moved[0], left);
        assert_ne!(moved[1], right);
        drop_owner(ctx, owner);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);

        let mut arena = enter(ctx);
        let leaf = heap(ctx, 0, 21);
        payload(leaf).cast::<u64>().write(5);
        payload(leaf).add(8).cast::<u64>().write(5);
        ptr::copy_nonoverlapping(b"abcde".as_ptr(), payload(leaf).add(16), 5);
        crate::count::hirsel_rt_v2_dup(ctx, leaf);
        let mut source = [
            crate::views::Slice {
                owner: leaf,
                start: 8,
                length: 3,
            },
            crate::views::Slice {
                owner: leaf,
                start: 9,
                length: 2,
            },
        ];
        let mut plan = root_plan(source.as_mut_ptr().cast(), 6, 48, 5);
        plan.root.patches = vec![patch(0, 0), patch(24, 0)];
        plan.blocks = vec![block(leaf, 0, 21, Placement::Evacuate)];
        let owner = prepare_commit(ctx, &mut *arena, &plan).unwrap();
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
        let moved = root(owner).cast::<[crate::views::Slice; 2]>().read();
        assert_eq!(moved[0].owner, moved[1].owner);
        assert_eq!((moved[0].start, moved[0].length), (8, 3));
        assert_eq!((moved[1].start, moved[1].length), (9, 2));
        assert_eq!(moved[0].text(), "abc");
        assert_eq!(moved[1].text(), "bc");
        drop_owner(ctx, owner);
        assert_eq!(stats(ctx).live_blocks, 0);
    });
}

#[test]
fn native_transaction_33_overflow_invalid_plan_and_nested_named_arena() {
    with_context(|ctx| unsafe {
        let mut outer = enter(ctx);
        let mut inner = enter(ctx);
        let mut value = 71u64;
        let mut plan = root_plan((&raw mut value).cast(), 2, 8, 0);
        plan.charge = Charge::Bytes(u64::MAX).plus(Charge::Bytes(1));
        assert_eq!(
            prepare_commit(ctx, &mut *outer, &plan),
            Err(Error::Exhausted)
        );
        assert_eq!(stats(ctx).active_regions, 2);
        plan.charge = Charge::Bytes(0);
        plan.root.patches.push(patch(0, 0));
        assert_eq!(
            prepare_commit(ctx, &mut *outer, &plan),
            Err(Error::InvalidPlan)
        );
        plan.root.patches.clear();
        let owner = prepare_commit(ctx, &mut *outer, &plan).unwrap();
        assert_eq!((*ctx).region, ptr::from_mut(&mut *inner));
        let block = hirsel_rt_v2_arena_alloc(ctx, &mut *inner, 2, 8, 8, 0);
        payload(block).cast::<u64>().write(29);
        assert_eq!(payload(block).cast::<u64>().read(), 29);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *inner, 0, 0), 0);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *outer, 0, 0), 0);
        drop(inner);
        drop(outer);
        assert_eq!(root(owner).cast::<u64>().read(), 71);
        drop_owner(ctx, owner);
    });
}

#[test]
fn native_transaction_33_receiver_reads_copied_headers_after_source_worker_exit() {
    with_context(|run| unsafe {
        let source = hirsel_rt_v2_worker_create(run);
        let receiver = hirsel_rt_v2_worker_create(run);
        let mut arena = enter(source);
        let leaf = heap(source, 2, 8);
        payload(leaf).cast::<u64>().write(83);
        let mut value = leaf;
        let mut plan = root_plan((&raw mut value).cast(), 4, 8, 8);
        plan.root.patches.push(patch(0, 0));
        plan.blocks.push(block(leaf, 2, 8, Placement::Evacuate));
        let owner = prepare_commit(source, &mut *arena, &plan).unwrap();
        assert_eq!(hirsel_rt_v2_region_exit(source, &mut *arena, 0, 0), 0);
        drop(arena);
        drop(plan);
        hirsel_rt_v2_worker_retire(source);
        let owner = owner.addr();
        let receiver = receiver.addr();
        std::thread::spawn(move || {
            let owner = owner as *mut SealedRegion;
            let receiver = receiver as *mut TaskCtx;
            let leaf = root(owner).cast::<*mut Header>().read();
            crate::count::check_live_block(&*receiver, leaf);
            assert_eq!(payload(leaf).cast::<u64>().read(), 83);
            drop_owner(receiver, owner);
            hirsel_rt_v2_worker_retire(receiver);
        })
        .join()
        .unwrap();
        hirsel_rt_v2_worker_collect(run);
        assert!(
            (*run)
                .run_ref()
                .heaps
                .heaps()
                .all(|heap| (*heap).regions.owned_bytes() == 0)
        );
    });
}

#[test]
fn native_transaction_33_semantic_heap_in_compiler_region_still_charges_and_copies() {
    with_context(|ctx| unsafe {
        let mut compiler = enter(ctx);
        let physical = hirsel_rt_v2_arena_alloc(ctx, &mut *compiler, 2, 8, 8, 0);
        payload(physical).cast::<u64>().write(97);
        let mut arena = enter(ctx);
        let mut source = physical;
        let mut plan = root_plan((&raw mut source).cast(), 4, 8, 8);
        plan.root.patches.push(patch(0, 0));
        plan.blocks.push(block(physical, 2, 8, Placement::Evacuate));
        hirsel_rt_v2_arena_bound(ctx, &mut *arena, 7);
        assert_eq!(
            prepare_commit(ctx, &mut *arena, &plan),
            Err(Error::Exhausted)
        );
        hirsel_rt_v2_arena_bound(ctx, &mut *arena, 8);
        let owner = prepare_commit(ctx, &mut *arena, &plan).unwrap();
        assert_eq!(arena.remaining, 0);
        let moved = root(owner).cast::<*mut Header>().read();
        assert_ne!(moved, physical);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *arena, 0, 0), 0);
        assert_eq!(hirsel_rt_v2_region_exit(ctx, &mut *compiler, 0, 0), 0);
        drop(arena);
        drop(compiler);
        crate::count::check_live_block(&*ctx, moved);
        assert_eq!(payload(moved).cast::<u64>().read(), 97);
        drop_owner(ctx, owner);
    });
}
