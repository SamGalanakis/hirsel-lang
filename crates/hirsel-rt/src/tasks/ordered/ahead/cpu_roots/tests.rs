use super::*;
use crate::alloc;
use crate::budget::stretch::{Budgets, Stretch};
use crate::pool::cpu_scoped::ledger::Ledger;
use crate::tasks::ordered::ahead::{Local, cpu_allocation, cpu_withdrawal, native};
use crate::tasks::ordered::{cpu, driver, engine, events};
use crate::tasks::{Logical, Task, handle, role};

unsafe extern "C" fn unused(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    crate::STATUS_OK
}

#[unsafe(naked)]
unsafe extern "C" fn checkpoint(_unused: *mut u8, _native: *mut u8) -> ! {
    core::arch::naked_asm!(
        "push rbx", "mov rbx, rsi", "mov rdi, rbx", "mov esi, 1", "call {handoff}", "ud2",
        handoff = sym native::hirsel_rt_v2_ahead_handoff,
    );
}

#[unsafe(naked)]
unsafe extern "C" fn allocating(_unused: *mut u8, _native: *mut u8) -> ! {
    core::arch::naked_asm!(
        "push rbx", "mov rbx, rsi", "mov rdi, rbx",
        "xor esi, esi", "mov edx, 4097", "mov ecx, 8", "call {allocate}",
        "mov qword ptr [rax + 8], 112", "mov rdi, rbx", "mov esi, 64", "call {handoff}", "ud2",
        allocate = sym cpu_allocation::hirsel_rt_v2_cpu_ahead_alloc,
        handoff = sym native::hirsel_rt_v2_ahead_handoff,
    );
}

unsafe fn make_native(ctx: *mut TaskCtx, input: *mut Header, mapper: *mut Header) -> Box<Native> {
    unsafe {
        Native::new(
            ctx,
            input,
            0,
            native::Certificate {
                entry: unused,
                stack_bytes: 256,
                host: std::ptr::null(),
                resume: None,
                cpu: true,
                preclaimed: true,
                live: std::ptr::null_mut(),
                mask: 1,
                source: mapper,
            },
        )
    }
}

unsafe fn allocate(ctx: *mut TaskCtx, owner: Owner, extent: u64) -> *mut Header {
    unsafe {
        // Fixture storage is prepared on idle, before either saved mapper
        // runs. The test supplies its logical prefix separately below.
        let running = crate::stop::running();
        crate::stop::set_running(std::ptr::null_mut());
        alloc::cpu_preflight(ctx, 0, extent, 8).unwrap();
        let block = alloc::alloc_prepared(ctx, 0, extent, 8, alloc::DOMAIN_HEAP, 0);
        alloc::cpu_allocation_done(ctx);
        crate::stop::set_running(running);
        alloc::payload(block).cast::<u64>().write(112);
        (*ctx)
            .heap_ref()
            .meter
            .cpu
            .get()
            .unwrap()
            .blocks
            .lock()
            .unwrap()
            .adopt(owner, block);
        block
    }
}

unsafe fn held(ctx: *mut TaskCtx) -> u64 {
    let heap = unsafe { (*ctx).heap_ref() };
    heap.backing_bytes() + heap.slab_count * alloc::SLAB_SIZE as u64 * u64::from(heap.diagnostic)
}

fn withdrawal(published: bool, diagnostic: bool) {
    let grant = hirsel_run_schema::CpuGrant {
        provider: "cpu.scoped@1".into(),
        max_workers: 4,
        max_domains: 1,
        max_physical_bytes: 2 << 30,
        max_batch_bytes: 2 << 20,
    };
    let ledger = Ledger::new(&grant);
    let _topology = ledger.topology(4).unwrap();
    let configured = ledger.clone();
    let setup_grant = grant.clone();
    let mut run = crate::host::BodyRun::new(1, false);
    if diagnostic {
        run.config.flags |= crate::context::CONFIG_DIAGNOSTIC;
    }
    let outcome = run.run(
        move |ctx| unsafe {
            (*ctx).run().cpu_grant = Some(setup_grant);
            (*ctx).set_budgets(Some(100), Some(grant.max_batch_bytes));
            (*ctx).heap_ref().meter.cpu.set(configured).ok().unwrap();
            engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
            events::enable(ctx);
        },
        move |ctx| unsafe {
            engine::fuel_in(ctx);
            assert_eq!(cpu::control(ctx, 4, 2, 2, || 0), Ok(0));
            assert_eq!(cpu::control(ctx, 4, 3, 0, || 0), Ok(0));
            let younger = handle(1, 7);
            let other = crate::context::hirsel_rt_v2_worker_create(ctx);
            (*other).task_id = 1;
            role::with_role(ctx, |s| {
                assert_eq!(s.tasks.len(), 1);
                s.tasks
                    .push(Some(Task::new(7, other, Some(0), vec![0], None)));
                let Logical::Ordered(d) = &mut s.logical else {
                    unreachable!()
                };
                d.cpu_batch.as_mut().unwrap().claim(younger, 4, 1);
                d.enqueue(younger, crate::ordered::footprint::Held::empty());
                assert!(d.select().contains(&younger));
                d.claimed(younger, 1);
            });
            let owner = Owner {
                task: younger,
                ordinal: 1,
            };
            let input = allocate(other, owner, 16);
            let mapper = allocate(other, owner, 16);
            let local = allocate(other, owner, 16);
            let result = allocate(other, owner, 16);
            let external = allocate(other, owner, 16);
            crate::count::hirsel_rt_v2_dup(other, external);
            // The ordinal gives back its credit. A holder on another context
            // still owns the block and its slab at the physical save.
            assert_eq!(
                crate::release::release_block(&mut *other, external, std::ptr::null(), 0, 0),
                0
            );
            let discarded = allocate(other, owner, 2048);
            let discarded_generation = ledger
                .blocks
                .lock()
                .unwrap()
                .retain(owner, &[discarded])
                .unwrap();
            assert_eq!(
                crate::release::release_block(&mut *other, discarded, std::ptr::null(), 0, 0),
                0
            );
            assert!(!ledger.blocks.lock().unwrap().valid(&discarded_generation));
            let before = held(other);
            let live_generation = ledger.blocks.lock().unwrap().owned(owner);
            assert_eq!(live_generation.len(), 5);
            let roots = [result, external];
            let value = 42u64;
            let mut summary = cpu_handoff::Summary {
                version: cpu_handoff::VERSION,
                phase: cpu_handoff::SUMMARY_PUBLISHED,
                ordinal: 1,
                value: (&raw const value).cast(),
                value_bytes: 8,
                descriptor: 0,
                retained_count: 2,
                retained: roots.as_ptr(),
            };
            let mut growth = cpu_handoff::Growth {
                version: cpu_handoff::VERSION,
                kind: cpu_handoff::GROW_BLOCK,
                ordinal: 1,
                block: local,
                region: std::ptr::null_mut(),
                index: 0,
                flags: 0,
                extent: 32,
                align: 8,
                keep: 8,
                element_size: 0,
                element_align: 0,
                capacity: 0,
            };
            let mut child_native = make_native(other, input, mapper);
            if published {
                child_native.summary = &raw mut summary;
            } else {
                child_native.growth = &raw mut growth;
            }
            let child_stack = crate::fiber::Stack::new();
            let child_sp = child_stack.prepare(
                checkpoint,
                std::ptr::null_mut(),
                (&raw mut *child_native).cast(),
            );
            (*other).ahead = Some(Box::new(Local::new(true)));
            (*other).ahead.as_mut().unwrap().native = Some(child_native);
            let child_native = &raw mut **(*other).ahead.as_mut().unwrap().native.as_mut().unwrap();
            (*other).ahead_physical = &raw mut (*child_native).physical;
            let budgets = Budgets {
                fuel: true,
                memory: true,
            };
            let mut local_prefix = Stretch::new(budgets);
            local_prefix.fuel_points(2);
            local_prefix.alloc(60).unwrap();
            engine::with_driver(ctx, |d| {
                if published {
                    d.cpu_batch
                        .as_mut()
                        .unwrap()
                        .publish(younger, local_prefix.clone(), &raw mut summary)
                        .unwrap();
                } else {
                    d.stretches.insert(younger, local_prefix.clone());
                }
            });
            // Consume all remaining physical capacity, without changing any
            // semantic budget. Only the saving owner's discarded slab can
            // make room for the head's exact pending allocation.
            let blocker = ledger.reserve_heap(grant.max_batch_bytes - before).unwrap();
            let mut head_native = make_native(ctx, std::ptr::null_mut(), std::ptr::null_mut());
            (*ctx).ahead_physical = &raw mut head_native.physical;
            let head_stack = crate::fiber::Stack::new();
            let sp = head_stack.prepare(
                allocating,
                std::ptr::null_mut(),
                (&raw mut *head_native).cast(),
            );
            crate::fiber::switch(&raw mut head_native.physical.idle_sp, sp);
            let exact_call = head_native.physical.saved_sp;
            assert_eq!(head_native.physical.reason, cpu_allocation::REASON);
            crate::stop::set_running(std::ptr::null_mut());
            role::enter(ctx);
            assert_eq!(cpu_allocation::serve(ctx, &raw mut *head_native), Ok(false));
            assert!(
                !cpu_withdrawal::acknowledged(ctx),
                "a running owner cannot acknowledge roots"
            );
            let charged = engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled);
            crate::fiber::switch(&raw mut (*child_native).physical.idle_sp, child_sp);
            // A physical switch-out folds the old view before after_save.
            // The next switch-in must load the collector's updated counters.
            (*other).heap().unbind();
            role::with_role(ctx, |s| {
                let Logical::Ordered(d) = &mut s.logical else {
                    unreachable!()
                };
                d.suspending(younger, true);
                let ready = driver::after_save(s, 1, false, true, true);
                assert!(ready.iter().all(|(task, _)| task.word() != younger));
            });
            assert!(
                held(other) < before,
                "a discarded ordinal must return its unreachable backing"
            );
            TaskCtx::attach(other, (*other).worker_ptr());
            assert_eq!(
                (*other).view.compiled.idle_slabs,
                0,
                "the resumed heap view contains no reclaimed idle pages"
            );
            assert_eq!(
                held(ctx),
                0,
                "reclamation runs through the saving heap owner"
            );
            assert!(
                ledger.blocks.lock().unwrap().valid(&live_generation),
                "every external holder remains live"
            );
            assert_eq!((*external).count, 1);
            assert_eq!(alloc::payload(external).cast::<u64>().read(), 112);
            let saved = (*child_native).saved_roots.as_ref().unwrap();
            assert_eq!(saved.input, input);
            assert_eq!(saved.mapper, mapper);
            assert_eq!(
                saved.allocations.len(),
                5,
                "parent-frame locals and results are bound at every save"
            );
            assert!(cpu_withdrawal::acknowledged(ctx));
            assert_eq!(head_native.physical.saved_sp, exact_call);
            engine::with_driver(ctx, |d| {
                assert!(
                    !d.stretches.contains_key(&younger),
                    "local prefixes leave the driver during withdrawal"
                );
            });
            if published {
                assert_eq!(summary.phase, cpu_handoff::SUMMARY_WITHDRAWN);
            }
            for _ in 0..3 {
                assert!(cpu_withdrawal::acknowledged(ctx));
                assert_eq!(
                    engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                    charged
                );
            }
            assert_eq!(cpu_allocation::serve(ctx, &raw mut *head_native), Ok(true));
            assert_eq!(
                engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                charged,
                "the head retry charges once"
            );
            if published {
                assert_eq!(summary.phase, cpu_handoff::SUMMARY_PUBLISHED);
            } else {
                assert_eq!(
                    engine::with_driver(ctx, |d| d.stretches[&younger].clone()),
                    local_prefix
                );
            }
            if !published {
                assert_eq!(
                    crate::release::release_block(&mut *ctx, external, std::ptr::null(), 0, 0),
                    0
                );
            }
            saved.check(other, &*child_native, owner);
            let block = head_native.allocation.result;
            role::release(ctx);
            crate::stop::set_running(ctx);
            crate::fiber::switch(&raw mut head_native.physical.idle_sp, exact_call);
            assert_eq!(head_native.physical.reason, 64);
            assert_eq!(alloc::payload(block).cast::<u64>().read(), 112);
            drop(blocker);
            for pointer in [input, mapper, local, result] {
                assert_eq!(
                    crate::release::release_block(&mut *other, pointer, std::ptr::null(), 0, 0),
                    0
                );
            }
            // A different heap returns the surviving external credit remotely.
            if published {
                assert_eq!(
                    crate::release::release_block(&mut *ctx, external, std::ptr::null(), 0, 0),
                    0
                );
            }
            (*other).heap().idle_cap = 0;
            (*other).heap().trim();
            assert_eq!(
                held(other),
                0,
                "remote returns are collected by their heap owner"
            );
            assert_eq!(
                crate::release::release_block(&mut *ctx, block, std::ptr::null(), 0, 0),
                0
            );
            (*ctx).heap().idle_cap = 0;
            (*ctx).heap().trim();
            (*ctx).ahead_physical = std::ptr::null_mut();
            (*other).ahead_physical = std::ptr::null_mut();
            role::with_role(ctx, |s| s.tasks[1] = None);
            crate::context::hirsel_rt_v2_worker_retire(other);
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
}

#[test]
fn acknowledged_local_cpu_withdrawal_reclaims_only_unreachable_owner_storage() {
    for diagnostic in [false, true] {
        withdrawal(false, diagnostic);
    }
}

#[test]
fn acknowledged_published_cpu_withdrawal_keeps_inputs_locals_results_and_external_holders() {
    for diagnostic in [false, true] {
        withdrawal(true, diagnostic);
    }
}

#[test]
fn saved_input_children_remain_bound_after_their_container_and_a_sibling_are_consumed() {
    // The input's blocks are ordinary pre-existing storage, not allocations
    // made through the mapper's CPU helpers. No payload is reread after take.
    unsafe {
        let config = crate::Config::new();
        let mut ctx = std::ptr::null_mut();
        assert_eq!(
            crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
            0
        );
        let grant = hirsel_run_schema::CpuGrant {
            provider: "cpu.scoped@1".into(),
            max_workers: 4,
            max_domains: 1,
            max_physical_bytes: 2 << 30,
            max_batch_bytes: 2 << 20,
        };
        let ledger = Ledger::new(&grant);
        (*ctx)
            .heap_ref()
            .meter
            .cpu
            .set(ledger.clone())
            .ok()
            .unwrap();
        let input = crate::text::allocate_bytes_list(ctx, &[vec![1, 2], vec![3, 4]]);
        let first = alloc::payload(input).add(16).cast::<*mut Header>().read();
        let second = alloc::payload(input).add(40).cast::<*mut Header>().read();
        let mut native = make_native(ctx, input, std::ptr::null_mut());
        assert_eq!(native.input_blocks.len(), 3);
        // Take both initialized element credits, then discard the empty input
        // container and one child. The other is now a mapper/helper local.
        alloc::payload(input).cast::<u64>().write(0);
        (*ctx).heap().free(input);
        assert_eq!(
            crate::release::release_block(&mut *ctx, first, std::ptr::null(), 0, 0),
            0
        );
        let owner = Owner {
            task: handle(1, 7),
            ordinal: 1,
        };
        let saved = Saved::bind(ctx, &native, owner);
        assert!(saved.allocations.is_empty());
        assert_eq!(
            saved.inputs.len(),
            1,
            "a decomposed input's live local keeps its generation"
        );
        assert!(ledger.blocks.lock().unwrap().valid_observed(&saved.inputs));
        assert_eq!(
            crate::release::release_block(&mut *ctx, second, std::ptr::null(), 0, 0),
            0
        );
        assert!(!ledger.blocks.lock().unwrap().valid_observed(&saved.inputs));
        native.environment = std::ptr::null_mut();
        assert!(Saved::bind(ctx, &native, owner).inputs.is_empty());
        crate::context::hirsel_rt_v2_ctx_destroy(ctx);
    }
}

#[test]
fn cpu_save_returns_idle_region_segments_and_keeps_active_region_storage() {
    for diagnostic in [false, true] {
        unsafe {
            let mut config = crate::Config::new();
            if diagnostic {
                config.flags |= crate::context::CONFIG_DIAGNOSTIC;
            }
            let mut ctx = std::ptr::null_mut();
            assert_eq!(
                crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
                0
            );
            let grant = hirsel_run_schema::CpuGrant {
                provider: "cpu.scoped@1".into(),
                max_workers: 4,
                max_domains: 1,
                max_physical_bytes: 2 << 30,
                max_batch_bytes: 2 << 20,
            };
            let ledger = Ledger::new(&grant);
            let _topology = ledger.topology(4).unwrap();
            (*ctx)
                .heap_ref()
                .meter
                .cpu
                .set(ledger.clone())
                .ok()
                .unwrap();
            let mut active = std::mem::MaybeUninit::<crate::region::Region>::uninit();
            let mut discarded = std::mem::MaybeUninit::<crate::region::Region>::uninit();
            crate::region::hirsel_rt_v2_region_enter(ctx, active.as_mut_ptr());
            let mut data = std::ptr::null_mut();
            assert_eq!(
                crate::region::hirsel_rt_v2_region_reserve(
                    ctx,
                    active.as_mut_ptr(),
                    40_000,
                    8,
                    &mut data
                ),
                0
            );
            data.cast::<u64>().write(112);
            crate::region::hirsel_rt_v2_region_enter(ctx, discarded.as_mut_ptr());
            let mut unused = std::ptr::null_mut();
            assert_eq!(
                crate::region::hirsel_rt_v2_region_reserve(
                    ctx,
                    discarded.as_mut_ptr(),
                    40_000,
                    8,
                    &mut unused
                ),
                0
            );
            assert_eq!(
                crate::region::hirsel_rt_v2_region_exit(ctx, discarded.as_mut_ptr(), 0, 0),
                0
            );
            assert!((*ctx).pool().idle_bytes > 0);
            assert_eq!((*ctx).pool().active_count, 1);
            let native = make_native(ctx, std::ptr::null_mut(), std::ptr::null_mut());
            (*ctx).ahead = Some(Box::new(Local::new(true)));
            (*ctx).ahead.as_mut().unwrap().native = Some(native);
            let native = &raw mut **(*ctx).ahead.as_mut().unwrap().native.as_mut().unwrap();
            (*ctx).ahead_physical = &raw mut (*native).physical;
            let stack = crate::fiber::Stack::new();
            let sp = stack.prepare(checkpoint, std::ptr::null_mut(), native.cast());
            crate::fiber::switch(&raw mut (*native).physical.idle_sp, sp);
            let owner = Owner {
                task: 0,
                ordinal: 0,
            };
            save(ctx, owner);
            assert_eq!(
                (*ctx).pool().idle_bytes,
                0,
                "a CPU save returns discarded regional backing"
            );
            assert_eq!(
                (*ctx).pool().active_count,
                1,
                "active regions remain lent to their owners"
            );
            assert_eq!(data.cast::<u64>().read(), 112);
            assert!(ledger.reserve_heap(grant.max_batch_bytes).is_err());
            assert_eq!(
                crate::region::hirsel_rt_v2_region_exit(ctx, active.as_mut_ptr(), 0, 0),
                0
            );
            save(ctx, owner);
            assert_eq!((*ctx).pool().idle_bytes, 0);
            assert!(
                ledger.reserve_heap(grant.max_batch_bytes).is_ok(),
                "all unreachable regional backing is reclaimed"
            );
            (*ctx).ahead_physical = std::ptr::null_mut();
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        }
    }
}
