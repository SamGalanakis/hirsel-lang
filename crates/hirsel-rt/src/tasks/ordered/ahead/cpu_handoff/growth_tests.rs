use super::growth::serve;
use super::*;
use crate::alloc;
use crate::pool::cpu_scoped::ledger::Ledger;
use crate::tasks::ordered::{cpu, engine, events};

unsafe extern "C" fn unused(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    crate::STATUS_OK
}

#[unsafe(naked)]
unsafe extern "C" fn growing(_request: *mut u8, _native: *mut u8) -> ! {
    core::arch::naked_asm!(
        "push rbx", "mov rbx, rsi", "mov rsi, rdi", "mov rdi, rbx",
        "call {grow}", "cmp eax, [rbx + {status}]", "jne 2f",
        "mov rdi, rbx", "mov esi, 64", "call {handoff}", "2:", "ud2",
        grow = sym hirsel_rt_v2_cpu_ahead_grow,
        status = const std::mem::offset_of!(Native, growth_status),
        handoff = sym super::super::native::hirsel_rt_v2_ahead_handoff,
    );
}

#[derive(Clone, Copy)]
enum Refusal {
    Ordinal,
    Version,
    Region,
}

unsafe fn heap_backing(ctx: *mut TaskCtx) -> u64 {
    let heap = unsafe { (*ctx).heap_ref() };
    // The CPU ledger includes each diagnostic slab's shadow allocation.
    heap.backing_bytes() + heap.slab_count * alloc::SLAB_SIZE as u64 * u64::from(heap.diagnostic)
}

fn run_growth(sequence: bool, refuse: bool, repeat: bool, refusal: Option<Refusal>) {
    run_growth_with_sharing(sequence, refuse, repeat, refusal, false, false, None);
}

fn run_growth_with_sharing(
    sequence: bool,
    refuse: bool,
    repeat: bool,
    refusal: Option<Refusal>,
    shared: bool,
    holders: bool,
    kind: Option<u32>,
) {
    let untracked = kind == Some(CHECK_UNTRACKED_SEQUENCE);
    let copy = kind == Some(COPY_SEQUENCE);
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
    run.config.flags |= crate::context::CONFIG_DIAGNOSTIC;
    let outcome = run.run(
        move |ctx| unsafe {
            (*ctx).run().cpu_grant = Some(setup_grant.clone());
            (*ctx).set_budgets(Some(100), Some(setup_grant.max_batch_bytes));
            (*ctx).heap().idle_cap = 0;
            (*ctx).heap_ref().meter.cpu.set(configured).ok().unwrap();
            engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
            events::enable(ctx);
        },
        move |ctx| unsafe {
            engine::fuel_in(ctx);
            assert_eq!(cpu::control(ctx, 4, 2, 1, || 0), Ok(0));
            assert_eq!(cpu::control(ctx, 4, 3, 0, || 0), Ok(0));
            let child = holders.then(|| {
                let child = alloc::hirsel_rt_v2_alloc(ctx, 0, 16, 8, alloc::DOMAIN_HEAP, 0);
                alloc::payload(child).cast::<[u64; 2]>().write([8, 112]);
                crate::count::hirsel_rt_v2_dup(ctx, child);
                child
            });
            let index = if holders {
                crate::text::bytes_list_index(ctx)
            } else {
                0
            };
            let old = alloc::hirsel_rt_v2_alloc(
                ctx,
                index,
                if copy {
                    65536
                } else if holders {
                    64
                } else {
                    32
                },
                8,
                alloc::DOMAIN_HEAP,
                0,
            );
            let input = if let Some(child) = child {
                vec![2, 2, child.addr() as u64, 8, 8, child.addr() as u64, 8, 8]
            } else {
                vec![1, 2, 112, 0]
            };
            std::ptr::copy_nonoverlapping(input.as_ptr(), alloc::payload(old).cast(), input.len());
            if shared {
                crate::count::hirsel_rt_v2_dup(ctx, old);
            }
            let mut live = 1;
            let certificate = super::super::native::Certificate {
                entry: unused,
                stack_bytes: 256,
                host: std::ptr::null(),
                resume: None,
                cpu: true,
                preclaimed: false,
                live: &raw mut live,
                mask: 1,
                source: std::ptr::null_mut(),
            };
            let mut native = Native::new(ctx, std::ptr::null_mut(), 0, certificate);
            let task = engine::current(ctx);
            native.cpu_claim = engine::with_driver(ctx, |driver| {
                driver
                    .cpu_batch
                    .as_ref()
                    .and_then(|batch| batch.identity(task))
            });
            (*ctx).ahead_physical = &raw mut native.physical;
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_arm(ctx);
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_enter((*ctx).ahead_physical);
            let mut request = Growth {
                version: if matches!(refusal, Some(Refusal::Version)) {
                    VERSION + 1
                } else {
                    VERSION
                },
                kind: if matches!(refusal, Some(Refusal::Region)) {
                    ALLOC_REGION
                } else if let Some(kind) = kind {
                    kind
                } else if sequence {
                    GROW_SEQUENCE
                } else {
                    GROW_BLOCK
                },
                ordinal: u64::from(matches!(refusal, Some(Refusal::Ordinal))),
                block: old,
                // These fields belong only to ALLOC_REGION in the landed ABI.
                region: std::ptr::dangling_mut(),
                index: if kind.is_some() { index } else { u32::MAX },
                flags: u32::MAX,
                extent: 65536,
                align: 8,
                keep: 16,
                element_size: if holders { 24 } else { 8 },
                element_align: 8,
                capacity: 8192,
            };
            let stack = crate::fiber::Stack::new();
            let sp = stack.prepare(
                growing,
                (&raw mut request).cast(),
                (&raw mut *native).cast(),
            );
            let held = refuse.then(|| {
                if copy {
                    // A large copy needs new backing. Fill the ledger's exact
                    // remaining capacity without approximating allocator metadata.
                    let mut held = Vec::new();
                    let mut bytes = grant.max_batch_bytes;
                    while bytes != 0 {
                        if let Ok(reservation) = ledger.reserve_heap(bytes) {
                            held.push(reservation);
                        }
                        bytes >>= 1;
                    }
                    held
                } else {
                    vec![
                        ledger
                            .reserve_heap(grant.max_batch_bytes - heap_backing(ctx))
                            .unwrap(),
                    ]
                }
            });
            crate::fiber::switch(&raw mut native.physical.idle_sp, sp);
            assert_eq!(native.physical.reason, 32);
            assert_eq!(native.physical.saved_sp, sp.wrapping_sub(32));
            crate::stop::set_running(std::ptr::null_mut());
            crate::tasks::role::enter(ctx);
            if kind.is_some() {
                super::super::cpu_worker::admit(&raw mut *native);
                assert_eq!(native.cpu_phase, 3);
            }
            assert_eq!(serve(ctx, &raw mut *native), Ok(!refuse));
            if refusal.is_some() {
                assert_eq!(native.growth_status, u32::from(NOT_CERTIFIED));
                assert_eq!(request.block, old);
                assert_eq!(
                    std::slice::from_raw_parts(alloc::payload(old).cast::<u64>(), input.len()),
                    input.as_slice()
                );
                assert_eq!((*ctx).heap_ref().live().0, 1 + u64::from(holders));
                crate::tasks::role::release(ctx);
                crate::stop::set_running(ctx);
                let saved = native.physical.saved_sp;
                crate::fiber::switch(&raw mut native.physical.idle_sp, saved);
                assert_eq!(native.physical.reason, 64);
                assert!(native.growth.is_null());
                (*ctx).ahead_physical = std::ptr::null_mut();
                (*ctx).heap().free(old);
                (*ctx).heap().trim();
                assert_eq!((*ctx).heap_ref().live().0, 0);
                return crate::STATUS_OK;
            }
            if refuse {
                // Observe settled state at an explicit budget boundary. The
                // accepted allocation may still belong to a private lease.
                assert_eq!(engine::host_segment_barrier(ctx), Ok(true));
                let state = engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled);
                for _ in 0..3 {
                    assert_eq!(request.block, old, "refusal retains the input owner");
                    assert_eq!(
                        std::slice::from_raw_parts(alloc::payload(old).cast::<u64>(), input.len()),
                        input.as_slice()
                    );
                    assert_eq!((*ctx).heap_ref().live().0, 1 + u64::from(holders));
                    assert_eq!(serve(ctx, &raw mut *native), Ok(false));
                    assert_eq!(
                        engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                        state
                    );
                }
                drop(held);
                assert_eq!(serve(ctx, &raw mut *native), Ok(true));
            }
            let fresh = request.block;
            if untracked {
                assert_eq!(fresh, old, "ArenaPush must not relocate a heap owner");
                assert_eq!(native.growth_status, u32::from(ACCEPTED));
                assert_eq!(
                    std::slice::from_raw_parts(alloc::payload(old).cast::<u64>(), input.len()),
                    input.as_slice()
                );
                crate::tasks::role::release(ctx);
                crate::stop::set_running(ctx);
                crate::fiber::switch(&raw mut native.physical.idle_sp, native.physical.saved_sp);
                assert_eq!(native.physical.reason, 64);
                assert!(native.growth.is_null());
                (*ctx).ahead_physical = std::ptr::null_mut();
                (*ctx).heap().free(old);
                (*ctx).heap().trim();
                engine::at_head(ctx).unwrap();
                assert_eq!((*ctx).heap_ref().live().0, 0);
                return crate::STATUS_OK;
            }
            assert_ne!(fresh, old);
            if copy {
                assert_eq!(
                    (*old).count,
                    crate::count::COUNT_UNIQUE,
                    "a shallow copy borrows the original credit"
                );
            }
            assert_eq!(
                (*ctx).heap_ref().live().0,
                if shared || copy { 2 } else { 1 } + u64::from(holders),
                "unique growth reclaims its input; shared growth retains the other holder"
            );
            if shared {
                assert_eq!(
                    (*old).count,
                    1,
                    "detach returns exactly the caller's credit"
                );
                assert_eq!(
                    std::slice::from_raw_parts(alloc::payload(old).cast::<u64>(), input.len()),
                    input.as_slice()
                );
            }
            assert_eq!(alloc::payload(fresh).cast::<u64>().read(), input[0]);
            if sequence && !copy {
                assert_eq!(alloc::payload(fresh).add(16).cast::<u64>().read(), input[2]);
                let exact =
                    alloc::sequence_slots((*ctx).heap_ref(), fresh, 16, request.element_size);
                assert!(exact >= request.capacity);
                assert_eq!(alloc::payload(fresh).add(8).cast::<u64>().read(), exact);
            } else {
                assert_eq!(alloc::payload(fresh).add(8).cast::<u64>().read(), 2);
            }
            crate::tasks::role::release(ctx);
            crate::stop::set_running(ctx);
            let saved = native.physical.saved_sp;
            crate::fiber::switch(&raw mut native.physical.idle_sp, saved);
            assert_eq!(native.physical.reason, 64);
            assert!(native.growth.is_null());
            if repeat {
                let sp = stack.prepare(
                    growing,
                    (&raw mut request).cast(),
                    (&raw mut *native).cast(),
                );
                crate::fiber::switch(&raw mut native.physical.idle_sp, sp);
                crate::stop::set_running(std::ptr::null_mut());
                crate::tasks::role::enter(ctx);
                assert_eq!(serve(ctx, &raw mut *native), Ok(true));
                assert_eq!(request.block, fresh);
                assert_eq!(
                    (*ctx).heap_ref().live().0,
                    if shared || copy { 2 } else { 1 } + u64::from(holders),
                    "a resumed no-op must not free the result twice"
                );
                crate::tasks::role::release(ctx);
                crate::stop::set_running(ctx);
                let saved = native.physical.saved_sp;
                crate::fiber::switch(&raw mut native.physical.idle_sp, saved);
                assert_eq!(native.physical.reason, 64);
            }
            if let Some(child) = child {
                assert_eq!(
                    (*child).count,
                    4,
                    "detach retains every child holder exactly once"
                );
                assert_eq!(
                    crate::release::hirsel_rt_v2_release(ctx, old, index, std::ptr::null(), 0, 0),
                    0
                );
                assert_eq!(
                    (*child).count,
                    2,
                    "the copy retains both child credits after the original dies"
                );
                assert_eq!(alloc::payload(child).add(8).cast::<u64>().read(), 112);
                assert_eq!(
                    crate::release::hirsel_rt_v2_release(ctx, fresh, index, std::ptr::null(), 0, 0),
                    0
                );
            } else {
                (*ctx).heap().free(fresh);
                if shared || copy {
                    assert_eq!(
                        std::slice::from_raw_parts(alloc::payload(old).cast::<u64>(), input.len()),
                        input.as_slice()
                    );
                    (*ctx).heap().free(old);
                }
            }
            (*ctx).heap().trim();
            assert_eq!((*ctx).heap_ref().live().0, 0);
            assert_eq!((*ctx).heap_ref().backing_bytes(), 0);
            (*ctx).ahead_physical = std::ptr::null_mut();
            engine::at_head(ctx).unwrap();
            assert_eq!(
                engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled.level),
                0
            );
            (*ctx).ahead_physical = std::ptr::null_mut();
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
}

#[test]
fn saved_growth_reclaims_real_old_blocks_without_leaking() {
    for sequence in [false, true] {
        run_growth(sequence, false, false, None);
    }
}

#[test]
fn repeated_growth_calls_do_not_free_the_returned_block_twice() {
    for sequence in [false, true] {
        run_growth(sequence, false, true, None);
    }
}

#[test]
fn refused_growth_keeps_the_same_input_and_charge_until_backing_is_available() {
    for sequence in [false, true] {
        run_growth(sequence, true, true, None);
    }
}

#[test]
fn native_growth_refuses_wrong_claims_versions_and_regions_without_consuming_the_input() {
    for refusal in [Refusal::Ordinal, Refusal::Version, Refusal::Region] {
        run_growth(false, false, false, Some(refusal));
    }
}

#[test]
fn shared_flat_sequence_growth_keeps_other_holders_and_retries_once() {
    for refuse in [false, true] {
        run_growth_with_sharing(true, refuse, true, None, true, false, None);
    }
}

fn run_region(refuse: bool, diagnostic: bool, tracked: bool, growth: Option<u64>) {
    run_region_storage(refuse, diagnostic, tracked, growth, false);
}

fn run_region_storage(
    refuse: bool,
    diagnostic: bool,
    tracked: bool,
    growth: Option<u64>,
    raw: bool,
) {
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
            (*ctx).run().cpu_grant = Some(setup_grant.clone());
            (*ctx).set_budgets(Some(100), Some(setup_grant.max_batch_bytes));
            (*ctx).heap_ref().meter.cpu.set(configured).ok().unwrap();
            (*ctx).heap().idle_cap = 0;
            engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
            events::enable(ctx);
        },
        move |ctx| unsafe {
            engine::fuel_in(ctx);
            assert_eq!(cpu::control(ctx, 4, 2, 1, || 0), Ok(0));
            assert_eq!(cpu::control(ctx, 4, 3, 0, || 0), Ok(0));
            let mut region = std::mem::MaybeUninit::<Region>::uninit();
            let region = region.as_mut_ptr();
            crate::region::hirsel_rt_v2_region_enter(ctx, region);
            let mut live = 1;
            let certificate = super::super::native::Certificate {
                entry: unused,
                stack_bytes: 256,
                host: std::ptr::null(),
                resume: None,
                cpu: true,
                preclaimed: false,
                live: &raw mut live,
                mask: 1,
                source: std::ptr::null_mut(),
            };
            let mut native = Native::new(ctx, std::ptr::null_mut(), 0, certificate);
            let task = engine::current(ctx);
            native.cpu_claim = engine::with_driver(ctx, |driver| {
                driver
                    .cpu_batch
                    .as_ref()
                    .and_then(|batch| batch.identity(task))
            });
            (*ctx).ahead_physical = &raw mut native.physical;
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_arm(ctx);
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_enter((*ctx).ahead_physical);
            let mut request = Growth {
                version: VERSION,
                kind: if raw { ALLOC_RAW_REGION } else { ALLOC_REGION },
                ordinal: 0,
                block: std::ptr::null_mut(),
                region,
                index: if growth.is_some() {
                    crate::text::bytes_list_index(ctx)
                } else {
                    0
                },
                flags: if tracked {
                    crate::region::ALLOC_TRACKED
                } else {
                    0
                },
                extent: 64,
                align: 64,
                keep: 0,
                element_size: 0,
                element_align: 0,
                capacity: 0,
            };
            let stack = crate::fiber::Stack::new();
            let sp = stack.prepare(
                growing,
                (&raw mut request).cast(),
                (&raw mut *native).cast(),
            );
            let held = refuse.then(|| ledger.reserve_heap(grant.max_batch_bytes).unwrap());
            crate::fiber::switch(&raw mut native.physical.idle_sp, sp);
            assert_eq!(native.physical.reason, 32);
            assert_eq!(native.physical.saved_sp, sp.wrapping_sub(32));
            crate::stop::set_running(std::ptr::null_mut());
            crate::tasks::role::enter(ctx);
            assert_eq!(serve(ctx, &raw mut *native), Ok(!refuse));
            assert_eq!(engine::host_segment_barrier(ctx), Ok(true));
            let state = engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled);
            assert_eq!(state.level, crate::region::SEGMENT_SIZE as u64);
            if refuse {
                for _ in 0..3 {
                    assert!(request.block.is_null());
                    assert!((*region).segment.is_null());
                    assert_eq!(serve(ctx, &raw mut *native), Ok(false));
                    assert_eq!(
                        engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                        state
                    );
                }
                drop(held);
                assert_eq!(serve(ctx, &raw mut *native), Ok(true));
                assert_eq!(
                    engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                    state
                );
            }
            let block = request.block;
            assert!(!block.is_null());
            let storage = if raw {
                block.cast::<u8>()
            } else {
                alloc::payload(block)
            };
            assert_eq!(storage.addr() % 64, 0);
            std::ptr::write_bytes(storage, 0x79, 64);
            assert_eq!(*storage.add(63), 0x79);
            if !raw {
                assert_eq!(alloc::payload(block).addr() % 64, 0);
                assert_eq!(
                    (*block).count,
                    if tracked {
                        crate::COUNT_ARENA_UNIQUE
                    } else {
                        crate::COUNT_IMMORTAL
                    }
                );
            }
            let task = engine::current(ctx);
            let owner = engine::with_driver(ctx, |d| {
                d.cpu_batch.as_ref().unwrap().block_owner(task).unwrap()
            });
            let retained = ledger
                .blocks
                .lock()
                .unwrap()
                .retain(owner, &[block])
                .unwrap();
            assert_eq!(retained.len(), usize::from(!raw));
            assert!(
                raw || !ledger.blocks.lock().unwrap().region_owned(
                    crate::pool::cpu_scoped::ledger::blocks::Owner {
                        task: owner.task + (1 << 32),
                        ..owner
                    },
                    region.addr()
                )
            );
            crate::tasks::role::release(ctx);
            crate::stop::set_running(ctx);
            crate::fiber::switch(&raw mut native.physical.idle_sp, native.physical.saved_sp);
            assert_eq!(native.physical.reason, 64);
            assert!(native.growth.is_null());
            let mut generations = retained.clone();
            if let Some(extent) = growth {
                // Two holder slots name the same child. Moving them must neither
                // add nor lose credits, and only the fresh cleanup may release them.
                let child = alloc::hirsel_rt_v2_alloc(ctx, 0, 16, 8, alloc::DOMAIN_HEAP, 0);
                alloc::payload(child).cast::<[u64; 2]>().write([8, 170]);
                crate::count::hirsel_rt_v2_dup(ctx, child);
                alloc::payload(block).cast::<[u64; 8]>().write([
                    2,
                    2,
                    child.addr() as u64,
                    8,
                    8,
                    child.addr() as u64,
                    8,
                    8,
                ]);
                crate::region::hirsel_rt_v2_region_schedule(
                    ctx,
                    region,
                    (*block).index(),
                    alloc::payload(block),
                );
                request.kind = GROW_BLOCK;
                request.keep = 64;
                request.extent = extent;
                // GROW_BLOCK derives the region and descriptor from the old block.
                request.region = std::ptr::dangling_mut();
                let sp = stack.prepare(
                    growing,
                    (&raw mut request).cast(),
                    (&raw mut *native).cast(),
                );
                let held = (refuse && extent > 65536 - 1024).then(|| {
                    ledger
                        .reserve_heap(
                            grant.max_batch_bytes
                                - crate::region::SEGMENT_SIZE as u64
                                - heap_backing(ctx),
                        )
                        .unwrap()
                });
                assert_eq!(engine::host_segment_barrier(ctx), Ok(true));
                let before_growth =
                    engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled.level);
                let before_segments = (*ctx).pool().active_bytes();
                crate::fiber::switch(&raw mut native.physical.idle_sp, sp);
                crate::stop::set_running(std::ptr::null_mut());
                crate::tasks::role::enter(ctx);
                let blocked = held.is_some();
                assert_eq!(serve(ctx, &raw mut *native), Ok(!blocked));
                assert_eq!(engine::host_segment_barrier(ctx), Ok(true));
                let charged = engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled);
                if blocked {
                    for _ in 0..3 {
                        assert_eq!(request.block, block);
                        assert_eq!(alloc::payload(block).cast::<u64>().read(), 2);
                        assert_eq!((*child).count, 2);
                        assert!(ledger.blocks.lock().unwrap().valid(&retained));
                        assert_eq!(serve(ctx, &raw mut *native), Ok(false));
                        assert_eq!(
                            engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                            charged,
                            "tracked growth charges exactly once across backing refusals"
                        );
                    }
                    drop(held);
                    assert_eq!(serve(ctx, &raw mut *native), Ok(true));
                    assert_eq!(
                        engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                        charged
                    );
                }
                assert_eq!(
                    charged.level - before_growth,
                    (*ctx).pool().active_bytes() - before_segments,
                    "growth admits exactly the bytes in its added region segments"
                );
                let fresh = request.block;
                assert_ne!(fresh, block);
                assert_eq!(native.growth_status, u32::from(ACCEPTED));
                assert_eq!(crate::region::tracked_region(fresh), region);
                assert_eq!((*fresh).count, crate::COUNT_ARENA_UNIQUE);
                assert_eq!(alloc::payload(fresh).addr() % 64, 0);
                assert_eq!(alloc::payload(block).cast::<u64>().read(), 0);
                assert_eq!(alloc::payload(fresh).cast::<u64>().read(), 2);
                assert_eq!(
                    (*child).count,
                    2,
                    "tracked growth moves both holder credits"
                );
                let fresh_generation = ledger
                    .blocks
                    .lock()
                    .unwrap()
                    .retain(owner, &[fresh])
                    .unwrap();
                assert_eq!(fresh_generation.len(), 1);
                assert_ne!(fresh_generation, retained);
                for wrong in [
                    crate::pool::cpu_scoped::ledger::blocks::Owner {
                        task: owner.task + (1 << 32),
                        ..owner
                    },
                    crate::pool::cpu_scoped::ledger::blocks::Owner {
                        ordinal: owner.ordinal + 1,
                        ..owner
                    },
                ] {
                    assert!(
                        ledger
                            .blocks
                            .lock()
                            .unwrap()
                            .retain(wrong, &[fresh])
                            .is_none(),
                        "tracked growth retains its exact task generation and ordinal"
                    );
                }
                generations.extend(fresh_generation);
                assert!(ledger.blocks.lock().unwrap().valid(&generations));
                crate::tasks::role::release(ctx);
                crate::stop::set_running(ctx);
                crate::fiber::switch(&raw mut native.physical.idle_sp, native.physical.saved_sp);
                assert_eq!(native.physical.reason, 64);
                // The second request fits and must neither allocate nor charge.
                let sp = stack.prepare(
                    growing,
                    (&raw mut request).cast(),
                    (&raw mut *native).cast(),
                );
                crate::fiber::switch(&raw mut native.physical.idle_sp, sp);
                crate::stop::set_running(std::ptr::null_mut());
                crate::tasks::role::enter(ctx);
                assert_eq!(serve(ctx, &raw mut *native), Ok(true));
                assert_eq!(request.block, fresh);
                assert_eq!(
                    engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                    charged
                );
                crate::tasks::role::release(ctx);
                crate::stop::set_running(ctx);
                crate::fiber::switch(&raw mut native.physical.idle_sp, native.physical.saved_sp);
            }
            (*ctx).ahead_physical = std::ptr::null_mut();
            assert_eq!(
                crate::region::hirsel_rt_v2_region_exit(ctx, region, 0, 0),
                0
            );
            for generation in &generations {
                assert!(
                    !ledger
                        .blocks
                        .lock()
                        .unwrap()
                        .valid(std::slice::from_ref(generation)),
                    "region exit revokes every tracked allocation generation"
                );
            }
            assert_eq!(
                (*ctx).heap_ref().live().0,
                0,
                "region cleanup releases the moved child holders"
            );
            assert!(
                ledger
                    .blocks
                    .lock()
                    .unwrap()
                    .region_owned(owner, region.addr())
            );
            (*ctx).pool().trim(0);
            engine::at_head(ctx).unwrap();
            assert_eq!(
                engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled.level),
                0
            );
            assert!(ledger.reserve_heap(grant.max_batch_bytes).is_ok());
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
}

#[test]
fn saved_region_allocation_preserves_refused_inputs_charges_once_and_revokes_on_exit() {
    for refuse in [false, true] {
        for diagnostic in [false, true] {
            for tracked in [false, true] {
                run_region(refuse, diagnostic, tracked, None);
            }
        }
    }
}

#[test]
fn shared_holder_sequence_growth_retains_children_across_refusal_and_original_release() {
    for refuse in [false, true] {
        run_growth_with_sharing(true, refuse, true, None, true, true, None);
    }
}

#[test]
fn saved_tracked_growth_moves_holders_charges_once_and_revokes_every_generation_on_exit() {
    for diagnostic in [false, true] {
        for refuse in [false, true] {
            for extent in [256, 65184, 65536] {
                run_region(refuse, diagnostic, true, Some(extent));
            }
        }
    }
}

#[test]
fn saved_raw_region_preserves_headerless_storage_across_backing_refusal() {
    for diagnostic in [false, true] {
        for refuse in [false, true] {
            run_region_storage(refuse, diagnostic, false, None, true);
        }
    }
}

#[test]
fn saved_arena_push_preserves_heap_inputs_and_refuses_wrong_claims() {
    for refusal in [None, Some(Refusal::Ordinal), Some(Refusal::Version)] {
        run_growth_with_sharing(
            true,
            false,
            false,
            refusal,
            false,
            false,
            Some(CHECK_UNTRACKED_SEQUENCE),
        );
    }
}

#[test]
fn saved_shallow_sequence_copy_preserves_the_borrowed_original_credit() {
    for refuse in [false, true] {
        for holders in [false, true] {
            run_growth_with_sharing(
                true,
                refuse,
                false,
                None,
                false,
                holders,
                Some(COPY_SEQUENCE),
            );
        }
    }
}
