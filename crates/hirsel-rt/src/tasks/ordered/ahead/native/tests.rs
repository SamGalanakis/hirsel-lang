use super::*;

unsafe extern "C" fn code(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    crate::STATUS_OK
}
fn certificate(live: *mut u64) -> Certificate {
    Certificate {
        entry: code,
        stack_bytes: 128,
        host: std::ptr::null(),
        resume: None,
        cpu: false,
        live,
        mask: 1,
        source: std::ptr::null_mut(),
    }
}

#[test]
#[cfg(target_arch = "x86_64")]
fn cpu_tail_requires_its_own_certificate_and_the_canonical_claim_arm() {
    unsafe {
        let config = crate::Config::new();
        let mut ctx = std::ptr::null_mut();
        assert_eq!(
            crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
            crate::STATUS_OK
        );
        let mut mask = 1;
        for cpu in [false, true] {
            let mut cert = certificate(&raw mut mask);
            cert.cpu = cpu;
            let mut native = Native::new(ctx, std::ptr::null_mut(), 0, cert);
            (*ctx).ahead_physical = &raw mut native.physical;
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_enter((*ctx).ahead_physical);
            assert!(!super::super::cpu_worker::armed(ctx));
            assert_eq!(native.cpu_phase, 0);
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_arm(ctx);
            assert_eq!(super::super::cpu_worker::armed(ctx), cpu);
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_enter((*ctx).ahead_physical);
            assert_eq!(native.cpu_phase, if cpu { 2 } else { 0 });
            assert!(!super::super::cpu_worker::armed(ctx));
            if cpu {
                assert_eq!(native.physical.points_left, 1024);
            }
            (*ctx).ahead_physical = std::ptr::null_mut();
        }
        crate::context::hirsel_rt_v2_ctx_destroy(ctx);
    }
}
fn reserved(live: *mut u64, generation: u32) -> super::super::Local {
    let mut local = super::super::Local::new(true);
    local.native = Some(Native::new(
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        0,
        certificate(live),
    ));
    local.reserve(generation);
    local
}
#[test]
pub(crate) fn saved_custody_restores_only_the_mask_once_without_releasing_the_source() {
    let mut image = [code as usize as u64, 0, 1, 0x1234567890abcdef];
    let mut local = reserved(&raw mut image[2], 37);
    image[2] = 0;
    unsafe { local.restore(37) };
    assert_eq!(image, [code as usize as u64, 0, 1, 0x1234567890abcdef]);
    image[2] = 0;
    unsafe { local.restore(37) };
    assert_eq!(image[2], 0, "custody restoration must not run twice");
}
#[test]
pub(crate) fn promotion_adopts_the_actual_moved_mask_and_same_physical_record() {
    let mut mask = 1;
    let mut local = reserved(&raw mut mask, 73);
    let address = &raw const local.native.as_ref().unwrap().physical;
    mask = 0;
    let save = local.adopt(73, std::ptr::dangling_mut(), 5);
    assert_eq!((save.generation, save.fuel, save.descriptor), (73, 5, None));
    let native = local.native.as_ref().unwrap();
    assert_eq!(&raw const native.physical, address);
    assert_eq!(native.physical.points_left, i64::MAX);
    unsafe { local.restore(73) };
    assert_eq!(mask, 0);
}
#[test]
fn promotion_keeps_live_logical_tokens_sites_and_both_allowances_on_the_same_task() {
    let config = crate::Config::new();
    let mut ctx = std::ptr::null_mut();
    // SAFETY: one exclusively owned context; successful entries retire once in
    // nesting order, and the adopted physical record never owns the context.
    unsafe {
        assert_eq!(
            crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
            0
        );
        assert_eq!(
            crate::depth::hirsel_rt_v2_depth_enter_at_site(ctx, 7, 8),
            crate::STATUS_OK
        );
        (*ctx).cleanup_depth = 1;
        assert_eq!(
            crate::depth::hirsel_rt_v2_depth_enter_at_site(ctx, 9, 10),
            crate::STATUS_OK
        );
        let tokens = (*ctx).invocation_tokens;
        let sites = (*ctx).invocation_site_slots;
        let mut mask = 1;
        let mut native = Native::new(ctx, std::ptr::null_mut(), 10, certificate(&raw mut mask));
        let physical = &raw mut native.physical;
        (*ctx).ahead_physical = physical;
        native.adopt();
        assert_eq!((*ctx).ahead_physical, physical);
        assert_eq!(native.ctx, ctx);
        assert_eq!((*ctx).activations.live(), (1, 1));
        assert_eq!((*ctx).invocation_depth, 2);
        assert_eq!((*ctx).invocation_tokens, tokens);
        assert_eq!((*ctx).invocation_site_slots, sites);
        assert_eq!(sites.read().assume_init(), [7, 8]);
        assert_eq!(sites.add(1).read().assume_init(), [9, 10]);
        assert_eq!((*ctx).cleanup_depth, 1);
        (*ctx).cleanup_depth = 0;
        crate::depth::hirsel_rt_v2_depth_leave(ctx);
        assert_eq!((*ctx).activations.live(), (1, 0));
        crate::depth::hirsel_rt_v2_depth_leave(ctx);
        assert_eq!((*ctx).activations.live(), (0, 0));
        (*ctx).ahead_physical = std::ptr::null_mut();
        drop(native);
        crate::context::hirsel_rt_v2_ctx_destroy(ctx);
    }
}
#[test]
pub(crate) fn stale_generation_cannot_restore_or_adopt_custody() {
    let mut mask = 0;
    let mut local = reserved(&raw mut mask, 91);
    for adopt in [false, true] {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if adopt {
                local.adopt(92, std::ptr::null_mut(), 0);
            } else {
                unsafe { local.restore(92) }
            }
        }));
        assert!(result.is_err());
        assert_eq!(mask, 0);
    }
}
fn layout(size: u64) -> crate::Layout {
    crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size,
        align: 8,
        finalizer: None,
        steps: std::ptr::null(),
        step_count: 0,
        field_count: 2,
        flags: 0,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: 0,
    }
}
#[test]
fn v2_capture_recipe_binds_code_live_word_descriptor_extent_and_exclusivity() {
    let wrapper = layout(24);
    let source = layout(8);
    let layouts = [&wrapper as *const _, &source as *const _];
    let mut run = crate::host::BodyRun::new(1, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 2;
    let outcome = run.run(
        |_| {},
        |ctx| unsafe {
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
            let captured =
                crate::alloc::hirsel_rt_v2_alloc(ctx, 3, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(captured).cast::<u64>().write(19);
            let image = crate::alloc::payload(env).cast::<[u64; 3]>();
            image.write([code as usize as u64, captured as u64, 1]);
            let recipe = Recipe {
                wrapper_descriptor: 2,
                source_descriptor: 3,
                wrapper_extent: 24,
                callable_offset: 0,
                live_word_offset: 16,
                source_extent: 8,
                live_mask: 1,
                source_code: code as *const u8,
            };
            let mut info = Info {
                prefix: SpawnAheadInfo {
                    version: 2,
                    flags: 0,
                    entry: Some(code),
                },
                stack_bytes: 128,
                recipe: &recipe,
            };
            assert!(validate(ctx, env, &info).is_some());
            for changed in [
                Recipe {
                    wrapper_descriptor: 3,
                    ..recipe
                },
                Recipe {
                    source_descriptor: 2,
                    ..recipe
                },
                Recipe {
                    wrapper_extent: 16,
                    ..recipe
                },
                Recipe {
                    source_extent: 16,
                    ..recipe
                },
                Recipe {
                    callable_offset: u32::MAX,
                    ..recipe
                },
                Recipe {
                    live_word_offset: 8,
                    ..recipe
                },
                Recipe {
                    live_mask: 3,
                    ..recipe
                },
                Recipe {
                    source_code: std::ptr::null(),
                    ..recipe
                },
            ] {
                info.recipe = &changed;
                assert!(validate(ctx, env, &info).is_none());
            }
            info.recipe = &recipe;
            for bound in [0, 79, 65537, u64::MAX] {
                info.stack_bytes = bound;
                assert!(validate(ctx, env, &info).is_none());
            }
            info.stack_bytes = 128;
            (*env).count = 2;
            assert!(validate(ctx, env, &info).is_none());
            (*env).count = 1;
            (*image)[2] = 0;
            assert!(validate(ctx, env, &info).is_none());
            (*image)[2] = 1;
            (*image)[0] = 0;
            assert!(validate(ctx, env, &info).is_none());
            crate::release::release_block(&mut *ctx, env, std::ptr::null(), 0, 0);
            crate::release::release_block(&mut *ctx, captured, std::ptr::null(), 0, 0);
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
}

#[test]
fn inert_value_metadata_is_admitted_but_owner_and_opaque_scan_programs_are_refused() {
    use hirsel_core_contract::handle_scan::*;
    let bits = [VALUE, VALUE_BITS, 8, 0, 0, 0, 0, 0];
    let mut scalar = layout(8);
    scalar.scan = bits.as_ptr();
    scalar.scan_words = 8;
    let record = [
        TYPE_EDGE,
        0,
        3,
        0,
        0,
        0,
        0,
        0,
        VALUE,
        VALUE_FIELDS,
        16,
        0,
        0,
        0,
        0,
        0,
        VALUE_FIELD,
        0,
        3,
        u64::MAX,
        0,
        0,
        0,
        0,
        VALUE_FIELD,
        8,
        3,
        u64::MAX,
        0,
        0,
        0,
        0,
    ];
    let mut environment = layout(16);
    environment.scan = record.as_ptr();
    environment.scan_words = 32;
    let layouts = [&environment as *const _, &scalar as *const _];
    let mut run = crate::host::BodyRun::new(1, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 2;
    let outcome = run.run(
        |_| {},
        move |ctx| unsafe {
            assert!(inert_source(ctx, 2));
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    for opcode in [
        HANDLE,
        BLOCK,
        ROOT,
        UNKNOWN,
        TYPE_UNKNOWN,
        TYPED_HANDLE,
        INLINE,
        SEQUENCE,
        COLUMN,
    ] {
        let scan = [opcode, 0, 0, 0, 0, 0, 0, 0];
        let mut resource = layout(8);
        resource.scan = scan.as_ptr();
        resource.scan_words = 8;
        let layouts = [&resource as *const _];
        let mut run = crate::host::BodyRun::new(1, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        let outcome = run.run(
            |_| {},
            move |ctx| unsafe {
                assert!(!inert_source(ctx, 2));
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    }
}

#[test]
pub(crate) fn stopped_run_restores_saved_custody_before_reclaiming_its_heap() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let image = layout(24);
    let layouts = [&image as *const _];
    let child_allocation = Arc::new(AtomicUsize::new(0));
    let address = child_allocation.clone();
    let mut run = crate::host::BodyRun::new(1, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    let outcome = run.run(
        |ctx| unsafe {
            super::super::super::engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
        },
        move |ctx| unsafe {
            // A never-started Saved reservation still owns its original mask.
            // The context uses separate owned storage here; Task's Drop owns
            // its contents, and ManuallyDrop below frees only that allocation.
            let child = Box::into_raw(Box::new(std::mem::ManuallyDrop::new(TaskCtx::new(
                std::ptr::from_mut((*ctx).worker()),
                (*ctx).run_ptr(),
            ))))
            .cast::<TaskCtx>();
            address.store(child as usize, Ordering::Release);
            (*child).task_id = 1;
            let environment =
                crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
            let live = crate::alloc::payload(environment).add(16).cast::<u64>();
            live.write(1);
            let mut local = Box::new(super::super::Local::new(true));
            local.native = Some(Native::new(child, environment, 0, certificate(live)));
            local.reserve(73);
            (*child).ahead = Some(local);
            let mut task = crate::tasks::Task::new(73, child, None, vec![1], None);
            // No stack code is entered. This owner simply keeps the context
            // alive through the actual stopped-run heap/row teardown order.
            task.stack = Some(crate::fiber::Stack::new());
            super::super::super::engine::driver(ctx).reserved.insert(
                1,
                super::super::Reserved {
                    task: Box::new(task),
                },
            );
            crate::stop::raise(ctx, crate::stop::Stop::Requested)
        },
    );
    assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
    let child = child_allocation.load(Ordering::Acquire);
    assert_ne!(child, 0);
    // Task::drop already destroyed the context; do not drop its fields again.
    unsafe { drop(Box::from_raw(child as *mut std::mem::ManuallyDrop<TaskCtx>)) };
}
