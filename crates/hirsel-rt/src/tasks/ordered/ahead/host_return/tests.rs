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
    let mut l = layout(16);
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
    run.config.flags = crate::context::CONFIG_DIAGNOSTIC;
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
            let read = ReadRecipe {
                cleanup_slots: 3,
                ..read_recipe()
            };
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
            (*ctx).heap().set_metered();
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
            assert!(valid_pending(ctx, &n.host));
            (*second).count = 2;
            assert!(!valid_pending(ctx, &n.host));
            assert_eq!((*first).count, 1);
            (*second).count = 1;
            n.host.pending[1].block = first;
            assert!(!valid_pending(ctx, &n.host), "duplicate unique owner");
            n.host.pending[1].block = second;
            n.host.pending[7] = n.host.pending[1];
            assert!(!valid_pending(ctx, &n.host), "slot outside recipe bound");
            n.host.pending[7] = EMPTY;
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
            let meter_ops = (*ctx).budget_counters().meter_ops;
            let running = crate::stop::running();
            crate::stop::set_running(std::ptr::null_mut());
            admit(ctx, &raw mut *n);
            admit(ctx, &raw mut *n);
            crate::stop::set_running(running);
            assert_eq!(
                (*ctx).budget_counters().meter_ops - meter_ops,
                2,
                "each finite owner reaches Meter::credit exactly once"
            );
            assert!(!n.host.active());
            assert_eq!((*ctx).heap_ref().state(first), crate::alloc::STATE_FREE);
            assert_eq!((*ctx).heap_ref().state(second), crate::alloc::STATE_FREE);
            assert_eq!((*ctx).heap_ref().state(immortal), crate::alloc::STATE_LIVE);
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

#[test]
fn inert_cleanup_rejects_malformed_layout_and_holder_metadata() {
    use hirsel_core_contract::handle_scan::*;
    let mut bytes = byte_layout();
    let mut element = layout(1);
    element.align = 1;
    element.flags = crate::LAYOUT_NO_EXTENT;
    let scalar = [VALUE, VALUE_BITS, 1, 0, 0, 0, 0, 0];
    element.scan = scalar.as_ptr();
    element.scan_words = 8;
    let scan = [
        TYPE_EDGE,
        0,
        3,
        0,
        0,
        0,
        0,
        0,
        VALUE,
        VALUE_SEQUENCE,
        16,
        1,
        16,
        3,
        0,
        0,
    ];
    bytes.scan = scan.as_ptr();
    bytes.scan_words = 16;
    let layouts = [&raw const bytes, &raw const element];
    let mut run = crate::host::BodyRun::new(1, false);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 2;
    let outcome = run.run(
        |_| {},
        move |ctx| unsafe {
            let bytes = &mut *(*ctx).descriptor(2).cast_mut();
            let element = &mut *(*ctx).descriptor(3).cast_mut();
            assert!(inert_buffer_layout(ctx, 2));
            let good = *bytes;
            for bad in [
                crate::Layout { magic: 0, ..good },
                crate::Layout { version: 0, ..good },
                crate::Layout { align: 0, ..good },
                crate::Layout { size: 8, ..good },
                crate::Layout {
                    flags: good.flags | crate::LAYOUT_FROZEN,
                    ..good
                },
                crate::Layout {
                    flags: good.flags | crate::LAYOUT_MUTEX_ROOT,
                    ..good
                },
                crate::Layout {
                    step_count: 1,
                    ..good
                },
                crate::Layout {
                    scan_words: u64::MAX,
                    ..good
                },
                crate::Layout {
                    scan_words: 15,
                    ..good
                },
                crate::Layout {
                    scan: std::ptr::null(),
                    ..good
                },
            ] {
                *bytes = bad;
                assert!(!inert_buffer_layout(ctx, 2), "{bytes:?}");
            }
            *bytes = good;
            let mut malformed = scan;
            bytes.scan = malformed.as_ptr();
            for (word, value) in [
                (0, HANDLE),
                (2, u64::MAX),
                (8, UNKNOWN),
                (9, VALUE_HANDLE),
                (11, 8),
                (13, 2),
            ] {
                malformed = scan;
                malformed[word] = value;
                assert!(!inert_buffer_layout(ctx, 2), "{malformed:?}");
            }
            *bytes = good;
            element.step_count = 1;
            assert!(!inert_buffer_layout(ctx, 2));
            assert_eq!(element.step_count, 1);
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
}

#[test]
fn unmetered_cleanup_consumes_each_owner_once_only_after_admission() {
    // Adapt Tokio task_abort.rs:49-108's retained owner / deferred destruction
    // control to Hirsel's canonical physical admission and inert storage.
    let bytes = byte_layout();
    let wrapper = layout(24);
    let layouts = [&bytes as *const _, &wrapper as *const _];
    let mut run = crate::host::BodyRun::new(1, false);
    run.config.flags = crate::context::CONFIG_DIAGNOSTIC;
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 2;
    let outcome = run.run(
        |ctx| unsafe {
            crate::tasks::ordered::engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
        },
        |ctx| unsafe {
            let read = ReadRecipe {
                buffer_descriptor: 2,
                ..read_recipe()
            };
            let reads = [&read as *const _];
            let root = root(&reads);
            let mut state = State::new(&root);
            state.armed = &read;
            state.task = crate::tasks::ordered::engine::current(ctx);
            state.phase = ACTIVE;
            let small = crate::alloc::allocate_sequence(ctx, 2, 1, 1, 1);
            let large = crate::alloc::allocate_sequence(ctx, 2, 8192, 1, 1);
            state.pending[0] = Pending {
                block: small,
                fuel: 91,
                charged: 0,
            };
            state.pending[1] = Pending {
                block: large,
                fuel: 87,
                charged: 0,
            };
            // No native execution is needed: the same custody record is admitted
            // through the real cleanup function, with TLS idle as in native::run.
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 3, 24, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<[u64; 3]>()
                .write([code as usize as u64, 0, 1]);
            let capture = Recipe {
                wrapper_descriptor: 3,
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
            n.host = state;
            flush_logical(ctx, &raw mut *n);
            assert_eq!((*small).count, 1);
            assert_eq!((*large).count, 1);
            assert_eq!((*ctx).heap_ref().state(small), crate::alloc::STATE_LIVE);
            assert_eq!((*ctx).ordered_freed.get(), 0);
            let running = crate::stop::running();
            crate::stop::set_running(std::ptr::null_mut());
            admit(ctx, &raw mut *n);
            admit(ctx, &raw mut *n);
            crate::stop::set_running(running);
            assert!(n.host.pending.iter().all(|p| p.block.is_null()));
            assert_eq!((*ctx).heap_ref().state(small), crate::alloc::STATE_FREE);
            assert_eq!((*ctx).ordered_freed.get(), 0);
            crate::release::release_block(&mut *ctx, env, std::ptr::null(), 0, 0);
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
}

// Lifecycle ownership follows Tokio task_abort.rs:49-108: cancellation first
// marks a retained task, and only its admitted owner destroys task storage.
unsafe fn selected_pending_lifecycle(stop: bool) {
    use std::sync::atomic::Ordering;
    unsafe {
        let wrapper = layout(24);
        let source = layout(8);
        let bytes = byte_layout();
        let layouts = [
            &wrapper as *const _,
            &source as *const _,
            &bytes as *const _,
        ];
        let mut config = crate::Config::new();
        config.flags = crate::context::CONFIG_DIAGNOSTIC;
        config.layouts = layouts.as_ptr();
        config.layout_count = layouts.len() as u32;
        let mut ctx = std::ptr::null_mut();
        assert_eq!(
            crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
            0
        );
        crate::tasks::ordered::engine::enable(
            ctx,
            crate::ordered::Policy::admit(&Default::default(), None),
        );
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
        let native = &raw mut *n;
        (*ctx).ahead_physical = &raw mut n.physical;
        let mut local = Box::new(super::super::Local::new(true));
        local.native = Some(n);
        (*ctx).ahead = Some(local);
        assert!(!(*ctx).test_host_tail);
        let mut first = crate::alloc::allocate_sequence(ctx, 4, 0, 1, 1);
        let second = crate::alloc::allocate_sequence(ctx, 4, 0, 1, 1);
        hirsel_rt_v2_host_arm(ctx, &read, &raw mut first);
        assert!(armed(ctx));
        let request = crate::io::Request::NetRead {
            net: 11,
            stream: 12,
            max: 1,
        };
        let result = Ok(crate::io::Completion::NetBytes(Ok(vec![71])));
        select(ctx, 73, &request, &result);
        assert!(selected(ctx));
        hirsel_rt_v2_host_enter(&raw mut (*native).physical, &read);
        assert!((*native).host.active());
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
        (*ctx).heap().set_metered();
        (*ctx).heap_ref().meter.set_limit(1 << 20);
        (*ctx).heap_ref().meter.ordered((*ctx).run_ptr());
        (*ctx)
            .run_ref()
            .ordered_memory
            .store(true, Ordering::Release);
        (*ctx).ordered_charge_trace.enable();
        (*ctx).ordered_charge_trace.grant(100);
        hirsel_rt_v2_host_defer(&raw mut (*native).physical, 0, first, 91);
        hirsel_rt_v2_host_defer(&raw mut (*native).physical, 1, second, 87);
        if stop {
            (*ctx)
                .run_ref()
                .control
                .request_inside(crate::stop::Stop::Requested);
        } else {
            crate::tasks::cancel_root(ctx, crate::tasks::CANCEL_REQUESTED);
        }
        // Exactly native::run's idle ordering: flush before inspecting stop or
        // attempting canonical admission. Neither branch observes cancellation.
        flush_logical(ctx, native);
        let amount = crate::budget::small_charge((*ctx).heap_ref().capacity(first));
        assert_eq!((*ctx).ordered_freed.get(), 2 * amount);
        let selected = (*native).host.selected.as_ref().unwrap();
        assert_eq!(selected.token, 73);
        assert_eq!(selected.request, request);
        assert_eq!(selected.result, result);
        assert_eq!((*first).count, 1);
        assert_eq!((*second).count, 1);
        assert_eq!((*ctx).heap_ref().state(first), crate::alloc::STATE_LIVE);
        assert_eq!((*ctx).heap_ref().state(second), crate::alloc::STATE_LIVE);
        let meter_ops = (*ctx).budget_counters().meter_ops;
        if stop {
            assert_eq!(
                (*ctx).run_ref().control.stopped(),
                Some(crate::stop::Stop::Requested)
            );
            assert!((*native).host.active());
            assert_eq!((*native).host.pending[0].block, first);
            assert_eq!((*native).host.pending[1].block, second);
            assert_eq!(
                (
                    (*ctx).activations.live(),
                    (*ctx).invocation_depth,
                    (*ctx).invocation_tokens,
                    (*ctx).invocation_site_slots
                ),
                depth
            );
            (*ctx).abandon();
            let audit = (*ctx).exit();
            assert_eq!(
                audit.live_blocks, 3,
                "the two pending buffers and capture stayed owned until terminal reclamation"
            );
            assert_eq!(audit.retained_bytes, 0);
            assert_eq!(
                (*ctx).budget_counters().meter_ops,
                meter_ops,
                "terminal reclamation must not invent admitted free credits"
            );
            // The context still owns the physical record until its teardown;
            // dropping it only destroys Rust custody, with storage now unmapped.
            (*ctx).ahead_physical = std::ptr::null_mut();
            drop((*ctx).ahead.take());
        } else {
            let task = crate::tasks::current_task(ctx);
            assert_eq!(
                crate::tasks::task_canceled(ctx, task),
                Some(crate::tasks::CANCEL_REQUESTED)
            );
            assert!(
                crate::tasks::ordered::engine::driver(ctx)
                    .head_ready(crate::tasks::ordered::engine::current(ctx))
            );
            // At the canonical head, physical admission remains independent of
            // pending cancellation. This idle context has no TLS task.
            admit(ctx, native);
            admit(ctx, native);
            assert!((*native).host.pending.iter().all(|p| p.block.is_null()));
            assert!((*native).host.selected.is_none());
            assert_eq!((*ctx).heap_ref().state(first), crate::alloc::STATE_FREE);
            assert_eq!((*ctx).heap_ref().state(second), crate::alloc::STATE_FREE);
            assert_eq!((*ctx).budget_counters().meter_ops - meter_ops, 2);
            assert_eq!((*ctx).ordered_freed.get(), 2 * amount);
            assert_eq!(
                (
                    (*ctx).activations.live(),
                    (*ctx).invocation_depth,
                    (*ctx).invocation_tokens,
                    (*ctx).invocation_site_slots
                ),
                depth
            );
            assert_eq!(
                crate::tasks::task_canceled(ctx, task),
                Some(crate::tasks::CANCEL_REQUESTED)
            );
            crate::depth::hirsel_rt_v2_depth_leave(ctx);
            (*ctx).heap_ref().meter.set_limit(0);
            crate::release::release_block(&mut *ctx, env, std::ptr::null(), 0, 0);
            (*ctx).ahead_physical = std::ptr::null_mut();
            drop((*ctx).ahead.take());
            let audit = (*ctx).exit();
            assert_eq!(audit.live_blocks, 0);
            assert_eq!(audit.retained_bytes, 0);
        }
        crate::context::hirsel_rt_v2_ctx_destroy(ctx);
    }
}

#[test]
fn canceled_selected_native_retains_two_pending_owners_until_admission() {
    unsafe { selected_pending_lifecycle(false) };
}

#[test]
fn stopped_selected_native_reclaims_two_pending_owners_without_admission() {
    unsafe { selected_pending_lifecycle(true) };
}
