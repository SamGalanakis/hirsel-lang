//! Saved-frame acceptance witnesses; atomic probes observe physical execution only.
use super::*;
use crate::tasks::*;
use std::sync::{Arc, atomic::AtomicU32};

fn scalar_environment() -> crate::Layout {
    crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 16,
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

unsafe extern "C" fn unused(_: *mut TaskCtx, _: *mut crate::Header, _: u64, _: u64) -> u8 {
    panic!("refused metadata must not enter either child body");
}

#[test]
fn unsupported_metadata_refuses_before_scope_or_environment_access() {
    for width in [1, 2, 4] {
        for (version, flags) in [
            (0, 0),
            (6, 0),
            (1, 1),
            (2, 1),
            (3, 1),
            (4, 1),
            (5, 1),
            (1, u32::MAX),
        ] {
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let control = run.control();
            let outcome = run.run(
                |ctx| unsafe {
                    super::super::engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                },
                move |ctx| unsafe {
                    let info = SpawnAheadInfo {
                        version,
                        flags,
                        entry: Some(unused),
                    };
                    hirsel_rt_v2_task_spawn_ahead(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        unused,
                        std::ptr::null_mut(),
                        0,
                        &info,
                    )
                },
            );
            assert_eq!(
                outcome.ended,
                Err(crate::stop::Stop::Refused(
                    "unsupported spawn-ahead metadata version or flags".into()
                )),
            );
            let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while control.status().reclamation_pending != 0 {
                assert!(
                    std::time::Instant::now() < until,
                    "refused metadata reclaims its workers"
                );
                std::thread::yield_now();
            }
            assert!(control.status().finished);
        }
    }
}

struct BudgetProbe {
    wide: bool,
    infinite: bool,
    private_prefix: AtomicU32,
    younger: std::sync::atomic::AtomicU64,
}

unsafe extern "C" fn budget_child(
    ctx: *mut TaskCtx,
    environment: *mut crate::Header,
    _: u64,
    _: u64,
) -> u8 {
    unsafe {
        let [address, ordinal] = crate::alloc::payload(environment).cast::<[u64; 2]>().read();
        let probe = &*(address as *const BudgetProbe);
        if ordinal == 0 {
            if probe.wide {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while probe.private_prefix.load(Ordering::Acquire) == 0 {
                    assert!(
                        std::time::Instant::now() < until,
                        "private sibling never executed"
                    );
                    std::thread::yield_now();
                }
                // A published probe alone does not prove that code returned.
                // Wait for the actual private physical owner to acknowledge
                // its save. In the finite case only task_entry's post-body
                // barrier can produce that save; no spawn can retire yet.
                loop {
                    let saved = {
                        let _role = role::Guard::new(ctx);
                        let id = probe.younger.load(Ordering::Acquire);
                        if id == 0 {
                            false
                        } else {
                            let Logical::Ordered(driver) = &scheduler(ctx).logical else {
                                unreachable!()
                            };
                            let row = &driver
                                .reserved
                                .get(&(id as u32))
                                .expect("younger spawn is still unpublished")
                                .task;
                            assert_eq!(row.generation, (id >> 32) as u32);
                            row.dispatch.as_ref().is_some_and(|frame| frame.is_saved())
                        }
                    };
                    if saved {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < until,
                        "private sibling never physically saved"
                    );
                    std::thread::yield_now();
                }
            }
            // The canonical first child exhausts a budget of twenty points.
            for _ in 0..21 {
                (*ctx).view.compiled.fuel -= 1;
                if (*ctx).view.compiled.fuel < 0 {
                    crate::budget::hirsel_rt_v2_fuel_refill(ctx);
                }
            }
            unreachable!("first-child fuel stop must precede completion");
        }
        assert!(private(ctx), "the younger spawn must remain unpublished");
        probe.private_prefix.fetch_add(1, Ordering::Release);
        if probe.infinite {
            loop {
                (*ctx).view.compiled.fuel -= 1;
                if (*ctx).view.compiled.fuel < 0 {
                    crate::budget::hirsel_rt_v2_fuel_refill(ctx);
                }
            }
        }
        crate::STATUS_OK
    }
}

fn stopped_budget_trace(width: u32, infinite: bool) -> super::super::events::Trace {
    let layout = scalar_environment();
    let layouts = [&layout as *const crate::Layout];
    let probe = Arc::new(BudgetProbe {
        wide: width > 1,
        infinite,
        private_prefix: AtomicU32::new(0),
        younger: std::sync::atomic::AtomicU64::new(0),
    });
    let seen = probe.clone();
    let mut run = crate::host::BodyRun::new(width, true);
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    run.fuel = Some(20);
    unsafe { run.use_physical_dispatch() };
    let (outcome, trace) = run.run_traced_after_bounded_stop(
        |ctx| unsafe {
            super::super::engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
            super::super::events::enable(ctx);
        },
        move |ctx| unsafe {
            let mut scope = 0;
            assert_eq!(
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                crate::STATUS_OK
            );
            for ordinal in 0..2 {
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env)
                    .cast::<[u64; 2]>()
                    .write([Arc::as_ptr(&seen) as u64, ordinal]);
                let mut task = 0;
                let info = SpawnAheadInfo {
                    version: 1,
                    flags: 0,
                    entry: Some(budget_child),
                };
                assert_eq!(
                    hirsel_rt_v2_task_spawn_ahead(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut task,
                        &scope,
                        budget_child,
                        env,
                        0,
                        &info,
                    ),
                    crate::STATUS_OK
                );
                if ordinal == 1 {
                    seen.younger.store(task, Ordering::Release);
                }
            }
            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0)
        },
    );
    assert_eq!(
        outcome.ended,
        Err(crate::stop::Stop::BudgetExceeded(
            hirsel_run_schema::BudgetKind::Fuel
        ))
    );
    assert_eq!(
        probe.private_prefix.load(Ordering::Acquire),
        u32::from(width > 1)
    );
    trace.expect("fully saved terminal trace")
}

#[test]
fn fuel_first_stop_with_a_physically_finished_private_sibling_matches_width_one() {
    let reference = stopped_budget_trace(1, false);
    for width in [2, 4] {
        assert_eq!(
            stopped_budget_trace(width, false).bytes(),
            reference.bytes()
        );
    }
}

#[test]
fn fuel_first_stop_with_a_checkpointed_infinite_private_sibling_matches_width_one() {
    let reference = stopped_budget_trace(1, true);
    for width in [2, 4] {
        assert_eq!(stopped_budget_trace(width, true).bytes(), reference.bytes());
    }
}

#[test]
#[cfg(unix)]
fn version_dispatch_does_not_overread_legacy_or_unsupported_prefixes() {
    unsafe extern "C" fn legacy_child(
        _: *mut TaskCtx,
        _: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        crate::STATUS_OK
    }
    unsafe {
        let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
        let mapping = libc::mmap(
            std::ptr::null_mut(),
            page * 2,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        assert_ne!(mapping, libc::MAP_FAILED);
        assert_eq!(
            libc::mprotect(mapping.cast::<u8>().add(page).cast(), page, libc::PROT_NONE),
            0
        );
        for version in [1, 6] {
            // The next byte is inaccessible. v1 has exactly its original16
            // bytes; unsupported metadata has only its8-byte dispatch prefix.
            let bytes = if version == 1 { 16 } else { 8 };
            let info = mapping
                .cast::<u8>()
                .add(page - bytes)
                .cast::<SpawnAheadInfo>();
            std::ptr::addr_of_mut!((*info).version).write(version);
            std::ptr::addr_of_mut!((*info).flags).write(0);
            if version == 1 {
                std::ptr::addr_of_mut!((*info).entry).write(None);
            }
            // The mapping outlives run; pass its address through the Send closure.
            let info_address = info as usize;
            let layout = scalar_environment();
            let layouts = [&layout as *const crate::Layout];
            let mut run = crate::host::BodyRun::new(1, false);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            run.use_physical_dispatch();
            let outcome = run.run(
                |ctx| {
                    super::super::engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    )
                },
                move |ctx| {
                    let info = info_address as *const SpawnAheadInfo;
                    let mut scope = 0;
                    if version == 1 {
                        assert_eq!(
                            hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0
                            ),
                            crate::STATUS_OK
                        );
                    }
                    let environment = if version == 1 {
                        // TaskSpawn has always required an owned environment.
                        // Keep metadata at the exact v1 page boundary.
                        crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            16,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        )
                    } else {
                        std::ptr::null_mut()
                    };
                    let mut child = 0;
                    let status = hirsel_rt_v2_task_spawn_ahead(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        legacy_child,
                        environment,
                        0,
                        info,
                    );
                    assert_eq!(status, crate::STATUS_OK);
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0)
                },
            );
            if version == 1 {
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            } else {
                assert_eq!(
                    outcome.ended,
                    Err(crate::stop::Stop::Refused(
                        "unsupported spawn-ahead metadata version or flags".into()
                    ))
                );
            }
        }
        assert_eq!(libc::munmap(mapping, page * 2), 0);
    }
}
