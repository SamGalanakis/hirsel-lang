use super::super::SpawnAheadInfo;
use super::super::native::Recipe;
use super::*;

unsafe extern "C" fn code(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    crate::STATUS_OK
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
        field_count: 0,
        flags: 0,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: 0,
    }
}
fn byte_layout() -> crate::Layout {
    let mut l = layout(u64::MAX);
    l.flags = crate::LAYOUT_SEQUENCE | crate::LAYOUT_DYNAMIC;
    l
}
fn read_recipe() -> ReadRecipe {
    ReadRecipe {
        buffer_descriptor: 4,
        maximum: 1,
        cleanup_slots: 2,
        flags: 0,
    }
}
fn root(reads: &[*const ReadRecipe]) -> Root {
    Root {
        version: 1,
        flags: 0,
        read_count: reads.len() as u64,
        reads: reads.as_ptr(),
        identity: [39; 32],
    }
}
fn capture_recipe() -> Recipe {
    Recipe {
        wrapper_descriptor: 2,
        source_descriptor: 3,
        wrapper_extent: 24,
        callable_offset: 0,
        live_word_offset: 16,
        source_extent: 8,
        live_mask: 1,
        source_code: code as *const u8,
    }
}

#[test]
fn v3_binds_capture_and_read_metadata_and_refuses_malformed_records() {
    let wrapper = layout(24);
    let source = layout(8);
    let bytes = byte_layout();
    let layouts = [
        &wrapper as *const _,
        &source as *const _,
        &bytes as *const _,
    ];
    let mut run = crate::host::BodyRun::new(1, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = layouts.len() as u32;
    let outcome = run.run(
        |_| {},
        |ctx| unsafe {
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
            let captured =
                crate::alloc::hirsel_rt_v2_alloc(ctx, 3, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env).cast::<[u64; 3]>().write([
                code as usize as u64,
                captured as u64,
                1,
            ]);
            let mut read = read_recipe();
            let reads = [&raw const read];
            let mut root = root(&reads);
            let capture = capture_recipe();
            let mut info = HostInfo {
                prefix: Info {
                    prefix: SpawnAheadInfo {
                        version: 3,
                        flags: 0,
                        entry: Some(code),
                    },
                    stack_bytes: 1024,
                    recipe: &capture,
                },
                host: &root,
            };
            assert!(validate(ctx, env, &info).is_some());
            for bad in [
                ReadRecipe {
                    maximum: 0,
                    ..read_recipe()
                },
                ReadRecipe {
                    maximum: 257,
                    ..read_recipe()
                },
                ReadRecipe {
                    cleanup_slots: 0,
                    ..read_recipe()
                },
                ReadRecipe {
                    cleanup_slots: 9,
                    ..read_recipe()
                },
                ReadRecipe {
                    flags: 1,
                    ..read_recipe()
                },
                ReadRecipe {
                    buffer_descriptor: u32::MAX,
                    ..read_recipe()
                },
                ReadRecipe {
                    buffer_descriptor: 3,
                    ..read_recipe()
                },
            ] {
                read = bad;
                assert!(validate(ctx, env, &info).is_none());
            }
            read = read_recipe();
            // Keep the recipe storage live while validate reads its table pointer.
            assert_eq!(read.maximum, 1);
            for (version, flags, count) in [(2, 0, 1), (1, 1, 1), (1, 0, 0), (1, 0, 65)] {
                root.version = version;
                root.flags = flags;
                root.read_count = count;
                assert!(validate(ctx, env, &info).is_none());
            }
            root.version = 1;
            root.flags = 0;
            root.read_count = 1;
            root.reads = std::ptr::null();
            assert!(validate(ctx, env, &info).is_none());
            info.prefix.prefix.entry = None;
            info.host = std::ptr::dangling();
            assert!(
                validate(ctx, env, &info).is_none(),
                "null entry must not inspect the host suffix"
            );
            crate::release::release_block(&mut *ctx, env, std::ptr::null(), 0, 0);
            crate::release::release_block(&mut *ctx, captured, std::ptr::null(), 0, 0);
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
}

#[test]
fn selected_authority_requires_empty_exclusive_buffer_and_exact_read_maximum() {
    let wrapper = layout(24);
    let source = layout(8);
    let bytes = byte_layout();
    let layouts = [
        &wrapper as *const _,
        &source as *const _,
        &bytes as *const _,
    ];
    let mut run = crate::host::BodyRun::new(1, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = layouts.len() as u32;
    let outcome = run.run(
        |ctx| unsafe {
            crate::tasks::ordered::engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
        },
        |ctx| unsafe {
            let read = read_recipe();
            let reads = [&read as *const _];
            let root = root(&reads);
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<[u64; 3]>()
                .write([code as usize as u64, 0, 1]);
            let capture = Recipe {
                source_descriptor: u32::MAX,
                source_extent: 0,
                ..capture_recipe()
            };
            let info = HostInfo {
                prefix: Info {
                    prefix: SpawnAheadInfo {
                        version: 3,
                        flags: 0,
                        entry: Some(code),
                    },
                    stack_bytes: 1024,
                    recipe: &capture,
                },
                host: &root,
            };
            let cert = validate(ctx, env, &info).unwrap();
            let mut n = Native::new(ctx, env, 0, 1, cert);
            (*ctx).ahead_physical = &raw mut n.physical;
            let mut buffer = crate::alloc::allocate_sequence(ctx, 4, 0, 1, 1);
            (*buffer).count = 2;
            hirsel_rt_v2_host_arm(ctx, &read, &raw mut buffer);
            assert!(!armed(ctx));
            (*buffer).count = 1;
            crate::alloc::payload(buffer).cast::<u64>().write(1);
            hirsel_rt_v2_host_arm(ctx, &read, &raw mut buffer);
            assert!(!armed(ctx));
            crate::alloc::payload(buffer).cast::<u64>().write(0);
            let other = read_recipe();
            hirsel_rt_v2_host_arm(ctx, &other, &raw mut buffer);
            assert!(!armed(ctx), "recipe address is part of the root binding");
            hirsel_rt_v2_host_arm(ctx, &read, &raw mut buffer);
            assert!(armed(ctx));
            assert!(!selected(ctx));
            let wrong = crate::io::Request::NetRead {
                net: 11,
                stream: 12,
                max: 2,
            };
            let value = Ok(crate::io::Completion::NetBytes(Ok(vec![71])));
            select(ctx, 73, &wrong, &value);
            assert!(!selected(ctx));
            hirsel_rt_v2_host_enter(&raw mut n.physical, &read);
            assert!(!n.host.active());
            assert!(!armed(ctx));
            for value in [
                Ok(vec![71]),
                Ok(vec![]),
                Err(crate::net::NetError::TimedOut),
            ] {
                hirsel_rt_v2_host_arm(ctx, &read, &raw mut buffer);
                let request = crate::io::Request::NetRead {
                    net: 11,
                    stream: 12,
                    max: 1,
                };
                select(
                    ctx,
                    91,
                    &request,
                    &Ok(crate::io::Completion::NetBytes(value.clone())),
                );
                assert!(selected(ctx));
                assert_eq!(n.host.selected.as_ref().unwrap().token, 91);
                assert_eq!(n.host.selected.as_ref().unwrap().request, request);
                hirsel_rt_v2_host_enter(&raw mut n.physical, &read);
                assert!(n.host.active());
                assert_eq!(n.physical.points_left, 1024);
                n.host = State::new(&root);
            }
            hirsel_rt_v2_host_arm(ctx, &read, &raw mut buffer);
            select(
                ctx,
                93,
                &crate::io::Request::NetRead {
                    net: 11,
                    stream: 12,
                    max: 1,
                },
                &Ok(crate::io::Completion::NetBytes(Ok(vec![1, 2]))),
            );
            assert!(!selected(ctx));
            (*ctx).ahead_physical = std::ptr::null_mut();
            crate::release::release_block(&mut *ctx, buffer, std::ptr::null(), 0, 0);
            crate::release::release_block(&mut *ctx, env, std::ptr::null(), 0, 0);
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
}

#[test]
fn deferred_byte_frees_keep_old_fuel_words_and_release_only_at_admission() {
    use crate::budget::stretch_trace::Charge;
    use std::sync::atomic::Ordering;
    let wrapper = layout(24);
    let source = layout(8);
    let bytes = byte_layout();
    let layouts = [
        &wrapper as *const _,
        &source as *const _,
        &bytes as *const _,
    ];
    let mut run = crate::host::BodyRun::new(1, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = layouts.len() as u32;
    let outcome = run.run(
        |ctx| unsafe {
            crate::tasks::ordered::engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
        },
        |ctx| unsafe {
            let read = read_recipe();
            let reads = [&read as *const _];
            let root = root(&reads);
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<[u64; 3]>()
                .write([code as usize as u64, 0, 1]);
            let capture = Recipe {
                source_descriptor: u32::MAX,
                source_extent: 0,
                ..capture_recipe()
            };
            let info = HostInfo {
                prefix: Info {
                    prefix: SpawnAheadInfo {
                        version: 3,
                        flags: 0,
                        entry: Some(code),
                    },
                    stack_bytes: 1024,
                    recipe: &capture,
                },
                host: &root,
            };
            let cert = validate(ctx, env, &info).unwrap();
            let mut n = Native::new(ctx, env, 0, 1, cert);
            assert_eq!(
                crate::depth::hirsel_rt_v2_depth_enter_at_site(ctx, 111, 222),
                crate::STATUS_OK
            );
            let depth = (
                (*ctx).activations.live(),
                (*ctx).invocation_depth,
                (*ctx).invocation_tokens,
                (*ctx).invocation_site_slots,
            );
            n.host.armed = &read;
            n.host.phase = ACTIVE;
            n.host.task = crate::tasks::ordered::engine::current(ctx);
            let first = crate::alloc::allocate_sequence(ctx, 4, 1, 1, 1);
            let second = crate::alloc::allocate_sequence(ctx, 4, 1, 1, 1);
            let immortal = crate::alloc::allocate_sequence(ctx, 4, 0, 1, 1);
            crate::alloc::payload(first).add(16).write(71);
            (*immortal).count = crate::count::COUNT_IMMORTAL;
            let heap = (*ctx).heap_ref();
            let amount = crate::budget::small_charge(heap.capacity(first));
            assert_eq!(amount, crate::budget::small_charge(heap.capacity(second)));
            heap.meter.set_limit(1 << 20);
            heap.meter.ordered((*ctx).run_ptr());
            (*ctx)
                .run_ref()
                .ordered_memory
                .store(true, Ordering::Release);
            (*ctx).ordered_charge_trace.enable();
            (*ctx).ordered_charge_trace.grant(100);
            hirsel_rt_v2_host_defer(&raw mut n.physical, 0, first, 91);
            hirsel_rt_v2_host_defer(&raw mut n.physical, 1, second, 87);
            hirsel_rt_v2_host_defer(&raw mut n.physical, 2, immortal, 83);
            assert_eq!((*first).count, 1);
            assert_eq!(crate::alloc::payload(first).add(16).read(), 71);
            assert_eq!((*ctx).ordered_freed.get(), 0);
            flush_logical(ctx, &raw mut *n);
            (*ctx).ordered_charge_trace.capture(80);
            assert_eq!(
                (*ctx).ordered_charge_trace.take().unwrap(),
                vec![
                    Charge::Fuel(9),
                    Charge::Free(amount),
                    Charge::Fuel(4),
                    Charge::Free(amount),
                    Charge::Fuel(7)
                ]
            );
            assert_eq!((*ctx).ordered_freed.get(), 2 * amount);
            assert_eq!(n.host.pending[0].block, first);
            assert_eq!(
                (*first).count,
                1,
                "logical credit retains the physical owner"
            );
            flush_logical(ctx, &raw mut *n);
            assert!((*ctx).ordered_charge_trace.take().unwrap().is_empty());
            let running = crate::stop::running();
            crate::stop::set_running(std::ptr::null_mut());
            admit(ctx, &raw mut *n);
            crate::stop::set_running(running);
            assert!(!n.host.active());
            assert_eq!(
                (
                    (*ctx).activations.live(),
                    (*ctx).invocation_depth,
                    (*ctx).invocation_tokens,
                    (*ctx).invocation_site_slots
                ),
                depth,
                "host admission retains the same task-owned invocation state"
            );
            assert_eq!(
                (*ctx).invocation_sites[depth.1 - 1].assume_init_ref(),
                &[111, 222]
            );
            crate::depth::hirsel_rt_v2_depth_leave(ctx);
            assert!(n.host.pending.iter().all(|p| p.block.is_null()));
            assert_eq!(
                (*ctx).ordered_freed.get(),
                2 * amount,
                "physical release must not duplicate credit"
            );
            assert!((*ctx).ordered_charge_trace.take().unwrap().is_empty());
            (*ctx).heap_ref().meter.set_limit(0);
            (*ctx)
                .run_ref()
                .ordered_memory
                .store(false, Ordering::Release);
            (*ctx).ordered_freed.set(0);
            (*immortal).count = 1;
            crate::release::release_block(&mut *ctx, immortal, std::ptr::null(), 0, 0);
            crate::release::release_block(&mut *ctx, env, std::ptr::null(), 0, 0);
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
}
