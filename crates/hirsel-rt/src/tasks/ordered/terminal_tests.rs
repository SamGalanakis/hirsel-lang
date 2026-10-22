//! Terminal publication uses the same saved owner/head boundary as H ops.

use super::super::*;
use super::engine;
use crate::ordered::Policy;
use crate::stop::Stop;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Default)]
struct Probe {
    attempted: AtomicBool,
    terminal: AtomicU64,
    older_effect: AtomicBool,
    mode: AtomicU64,
    reported: AtomicBool,
    cleaned: AtomicBool,
}

unsafe extern "C" fn older(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let [probe, atomic] = unsafe { crate::alloc::payload(env).cast::<[u64; 2]>().read() };
    let probe = unsafe { &*(probe as *const Probe) };
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !probe.attempted.load(Ordering::Acquire) {
        assert!(
            std::time::Instant::now() < until,
            "terminal task never ran ahead"
        );
        std::thread::yield_now();
    }
    loop {
        let pending = {
            let _role = unsafe { role::Guard::new(ctx) };
            engine::driver(ctx).head_paused(probe.terminal.load(Ordering::Acquire))
        };
        if pending {
            break;
        }
        assert!(
            std::time::Instant::now() < until,
            "terminal task never saved at its head gate"
        );
        std::thread::yield_now();
    }
    match probe.mode.load(Ordering::Acquire) {
        2 => unsafe {
            // The live environment already fills the budget. This older
            // allocation precedes the younger UB in the serial charge stream.
            crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
            unreachable!("older allocation must exhaust memory");
        },
        3 => {
            // A host request is allowed to interrupt a pending terminal gate.
            unsafe { (*ctx).run_ref().control.request(Stop::Requested) };
        }
        _ => {}
    }
    // The younger task has already reached its fatal condition, but the
    // serial reference still performs this older scheduling-point effect.
    let value = 23u64;
    assert_eq!(
        unsafe { hirsel_rt_v2_atomic_store(ctx, &atomic, (&value as *const u64).cast(), 8) },
        STATUS_OK
    );
    probe.older_effect.store(true, Ordering::Release);
    STATUS_OK
}

fn terminal_run(mode: u64) {
    let layout = crate::Layout {
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
    };
    let layouts = [&layout as *const crate::Layout];
    for width in [2, 4, 8] {
        let probe = Arc::new(Probe::default());
        probe.mode.store(mode, Ordering::Release);
        let seen = probe.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                static FAILURES: [u8; 40] = {
                    let mut bytes = [0; 40];
                    bytes[0] = 1; // One site with code zero and empty names.
                    bytes[16] = 7;
                    bytes
                };
                (*ctx).run().failure_table = crate::isolate::FailureTable::new(&FAILURES);
                if mode == 2 {
                    let charge = crate::alloc::allocation_charge(ctx, 2, 16, 8);
                    (*ctx).set_budgets(None, Some(charge));
                }
                engine::enable(ctx, Policy::admit(&Default::default(), None));
            },
            move |ctx| unsafe {
                let mut atomic = 0;
                assert_eq!(
                    hirsel_rt_v2_atomic_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut atomic,
                        (&0u64 as *const u64).cast(),
                        8
                    ),
                    STATUS_OK
                );
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    STATUS_OK
                );
                let env =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                crate::alloc::payload(env)
                    .cast::<[u64; 2]>()
                    .write([Arc::as_ptr(&seen) as u64, atomic]);
                let mut child = 0;
                assert_eq!(
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        older,
                        env,
                        0
                    ),
                    STATUS_OK
                );
                {
                    let _role = role::Guard::new(ctx);
                    seen.terminal.store(engine::current(ctx), Ordering::Release);
                }
                seen.attempted.store(true, Ordering::Release);
                match mode {
                    0 => crate::stop::raise(ctx, Stop::Refused("terminal fixture".into())),
                    4 => STATUS_OK,
                    5 => crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 0, 0, 0),
                    _ => crate::hirsel_rt_v2_unchecked_add_overflow(ctx, 0),
                }
            },
        );
        match mode {
            0 => assert_eq!(outcome.ended, Err(Stop::Refused("terminal fixture".into()))),
            1 => {
                let Err(Stop::UndefinedBehavior(diagnostic)) = &outcome.ended else {
                    panic!("{outcome:?}");
                };
                assert_eq!(diagnostic.site.line, 7);
                assert!(diagnostic.message.contains("int.unchecked_add at line 7"));
            }
            2 => assert_eq!(
                outcome.ended,
                Err(Stop::BudgetExceeded(hirsel_run_schema::BudgetKind::Memory))
            ),
            3 => assert_eq!(outcome.ended, Err(Stop::Requested)),
            4 => assert_eq!(outcome.ended, Ok(STATUS_OK)),
            5 => assert_eq!(outcome.ended, Ok(STATUS_FAILURE)),
            _ => unreachable!(),
        }
        assert_eq!(
            probe.older_effect.load(Ordering::Acquire),
            !matches!(mode, 2 | 3),
            "wrong retired prefix at width {width}"
        );
    }
}

#[test]
fn younger_terminal_condition_waits_for_the_older_retired_prefix() {
    terminal_run(0);
}

#[test]
fn younger_unchecked_add_ub_waits_for_the_older_retired_prefix() {
    terminal_run(1);
}

#[test]
fn older_budget_stop_precedes_a_younger_pending_ub() {
    terminal_run(2);
}

#[test]
fn external_stop_interrupts_a_younger_pending_terminal_gate() {
    terminal_run(3);
}

#[test]
fn root_success_waits_for_its_final_turn_to_retire_before_publication() {
    terminal_run(4);
}

#[test]
fn root_failure_waits_for_its_final_turn_to_retire_before_publication() {
    terminal_run(5);
}

unsafe extern "C" fn returned_cleanup(ctx: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    let probe = unsafe { &*(value.cast::<u64>().read() as *const Probe) };
    assert!(
        probe.reported.load(Ordering::Acquire),
        "sink must precede result cleanup"
    );
    assert!(
        probe.older_effect.load(Ordering::Acquire),
        "cleanup overtook older turn"
    );
    let mut canceled = u32::MAX;
    assert_eq!(
        unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) },
        STATUS_OK
    );
    probe.cleaned.store(true, Ordering::Release);
    STATUS_OK
}

#[test]
fn returned_error_sink_and_yielding_cleanup_follow_older_turns_on_the_production_launch_frame() {
    let environment = crate::Layout {
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
    };
    let result = crate::Layout {
        finalizer: Some(returned_cleanup),
        ..environment
    };
    let layouts = [&environment as *const crate::Layout, &result];
    let node = crate::exit_report::Node {
        kind: 2,
        a: 8,
        b: 0,
        c: 0,
        text: std::ptr::null(),
        text_len: 0,
        fields: std::ptr::null(),
        field_count: 0,
    };
    for width in [2, 4, 8] {
        let probe = Arc::new(Probe::default());
        let seen = probe.clone();
        let node_address = std::ptr::from_ref(&node).addr();
        let entry: crate::stop::Entry = Box::new(move |ctx, words| unsafe {
            let mut atomic = 0;
            assert_eq!(
                hirsel_rt_v2_atomic_create(
                    ctx,
                    std::ptr::null_mut(),
                    &mut atomic,
                    (&0u64 as *const u64).cast(),
                    8
                ),
                STATUS_OK
            );
            let mut scope = 0;
            assert_eq!(
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                STATUS_OK
            );
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env)
                .cast::<[u64; 2]>()
                .write([Arc::as_ptr(&seen) as u64, atomic]);
            let mut child = 0;
            assert_eq!(
                hirsel_rt_v2_task_spawn(
                    ctx,
                    0,
                    0,
                    std::ptr::null_mut(),
                    &mut child,
                    &scope,
                    older,
                    env,
                    0
                ),
                STATUS_OK
            );
            {
                let _role = role::Guard::new(ctx);
                seen.terminal.store(engine::current(ctx), Ordering::Release);
                // This is the production returned-result registration, with
                // storage owned by LaunchFrame, not the returning entry stack.
                let value = (*words).result.as_mut_ptr().cast::<u8>();
                value
                    .cast::<[u64; 2]>()
                    .write([Arc::as_ptr(&seen) as u64, 42]);
                crate::exit_report::hirsel_rt_v2_exit_report(
                    ctx,
                    value,
                    3,
                    8,
                    node_address as *const crate::exit_report::Node,
                );
            }
            seen.attempted.store(true, Ordering::Release);
            STATUS_OK
        });
        let home = std::thread::current().id();
        let mut report = |returned: &crate::exit_report::Returned| {
            assert_eq!(
                std::thread::current().id(),
                home,
                "report sink is home-affine"
            );
            assert!(
                probe.older_effect.load(Ordering::Acquire),
                "sink overtook older turn"
            );
            assert!(!probe.cleaned.load(Ordering::Acquire));
            assert_eq!(
                unsafe { crate::exit_report::render(returned) },
                "error: 42\n"
            );
            assert!(
                !probe.reported.swap(true, Ordering::AcqRel),
                "report exactly once"
            );
        };
        let mut config = crate::Config::new();
        config.layouts = layouts.as_ptr();
        config.layout_count = layouts.len() as u32;
        let mut ctx = std::ptr::null_mut();
        unsafe {
            assert_eq!(
                crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
                0
            );
            engine::enable(ctx, Policy::admit(&Default::default(), None));
            super::events::enable(ctx);
            // Only the semantic error field is initialized-value evidence;
            // the fixture's probe pointer at offset0 never enters the trace.
            super::events::install_value(
                ctx,
                3,
                super::values::Program::new(
                    vec![
                        super::values::Node::Fields {
                            extent: 16,
                            fields: vec![super::values::Field { offset: 8, node: 1 }],
                        },
                        super::values::Node::Bits(8),
                    ],
                    0,
                )
                .unwrap(),
            );
            crate::pool::start(ctx, width, true);
            crate::pool::free::enable(ctx);
            let frame = crate::stop::LaunchFrame::new(entry, 0, 0, [0; 5], &mut report);
            assert_eq!(crate::stop::run_root(ctx, frame), Ok(STATUS_OK));
            assert!(probe.reported.load(Ordering::Acquire));
            assert!(probe.cleaned.load(Ordering::Acquire));
            {
                let _role = role::Guard::new(ctx);
                let log = engine::driver(ctx).events.as_ref().unwrap();
                let ends: Vec<_> = log
                    .canonical
                    .iter()
                    .filter(|row| row.item == super::events::CanonicalItem::Root)
                    .collect();
                assert_eq!(ends.len(), 1);
                assert_eq!(ends[0].transition, super::events::Transition::End);
                assert_eq!(
                    ends[0].outcome,
                    super::events::CanonicalOutcome::RootEnd {
                        status: STATUS_OK,
                        result: Some(42u64.to_le_bytes().to_vec()),
                        failures: Vec::new(),
                    }
                );
            }
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        }
    }
}

/// Observe after actual stop publication and worker shutdown. The launcher
/// retains the run; no stopped entry or parked TaskCtx is sampled for evidence.
fn stopped_trace(
    width: u32,
    reorder: hirsel_run_config::Reorder,
    jitter: bool,
    stop: Stop,
) -> (Vec<super::events::CanonicalEvent>, bool) {
    stopped_trace_with_failures(width, reorder, jitter, stop, None)
}

fn stopped_trace_with_failures(
    width: u32,
    reorder: hirsel_run_config::Reorder,
    jitter: bool,
    stop: Stop,
    failure_metadata: Option<bool>,
) -> (Vec<super::events::CanonicalEvent>, bool) {
    let mut ctx = std::ptr::null_mut();
    unsafe {
        assert_eq!(
            crate::context::hirsel_rt_v2_ctx_create(&crate::Config::new(), &mut ctx),
            0
        );
        let mut policy = Policy::admit(&Default::default(), None);
        policy.reorder = reorder;
        if failure_metadata == Some(true) {
            static TABLE: [u8; 72] = {
                let mut bytes = [0; 72];
                bytes[0] = 2;
                bytes[8] = 1;
                bytes[16] = 17;
                bytes[20] = 3;
                bytes[40] = 2;
                bytes[48] = 23;
                bytes[52] = 5;
                bytes
            };
            (*ctx).run().failure_table = crate::isolate::FailureTable::new(&TABLE);
        }
        engine::enable(ctx, policy);
        super::events::enable(ctx);
        crate::pool::start(ctx, width, true);
        crate::pool::free::enable(ctx);
        let expected = stop.clone();
        let entry: crate::stop::Entry = Box::new(move |ctx, _| {
            let mut atomic = 0;
            assert_eq!(
                hirsel_rt_v2_atomic_create(
                    ctx,
                    std::ptr::null_mut(),
                    &mut atomic,
                    (&0u64 as *const u64).cast(),
                    8,
                ),
                STATUS_OK
            );
            if jitter {
                for _ in 0..13 {
                    std::thread::yield_now();
                }
            }
            assert_eq!(
                hirsel_rt_v2_atomic_store(ctx, &atomic, (&23u64 as *const u64).cast(), 8,),
                STATUS_OK
            );
            let mut value = 0u64;
            assert_eq!(
                hirsel_rt_v2_atomic_load(ctx, &atomic, (&mut value as *mut u64).cast(), 8),
                STATUS_OK
            );
            assert_eq!(value, 23);
            if failure_metadata.is_some() {
                crate::failure::hirsel_rt_v2_failure_begin(
                    ctx,
                    1,
                    hirsel_core_contract::RUNTIME_RULE_BASE,
                    1,
                    2,
                );
                (*ctx).failure.append_chain(vec![(
                    crate::failure::FailureRecord {
                        kind: 2,
                        rule: hirsel_core_contract::RUNTIME_RULE_BASE + 1,
                        site: 2,
                        operation_site: 1,
                    },
                    vec![4, 2],
                )]);
            }
            crate::stop::raise(ctx, stop.clone());
        });
        assert_eq!(
            crate::stop::run_root(ctx, crate::stop::LaunchFrame::bare(entry)),
            Err(expected)
        );
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while crate::pool::busy((*ctx).run_ref()) != 0 {
            assert!(
                std::time::Instant::now() < until,
                "stopped root did not save"
            );
            std::thread::yield_now();
        }
        (*ctx).abandon();
        let s = (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap();
        let Logical::Ordered(driver) = &s.logical else {
            unreachable!()
        };
        let log = driver.events.as_ref().unwrap();
        let result = (log.canonical.clone(), log.canonical().is_ok());
        crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        result
    }
}

#[test]
fn canonical_terminal_causes_follow_the_actual_stop_cut_across_widths() {
    use super::events::{CanonicalItem, CanonicalOutcome, Transition};
    use hirsel_run_schema::{BudgetKind, Environmental, ExecutionDiagnostic, SignalKind, Site};
    let stops = [
        Stop::Refused("terminal fixture".into()),
        Stop::Diverged("missing fixture input".into()),
        Stop::Environmental(Environmental::CodecLimit),
        Stop::Environmental(Environmental::Output("closed fixture sink".into())),
        Stop::Signaled(SignalKind::Terminate),
        Stop::BudgetExceeded(BudgetKind::Memory),
        Stop::BudgetExceeded(BudgetKind::Fuel),
        Stop::UndefinedBehavior(ExecutionDiagnostic {
            rule: "INT-UB".into(),
            site: Site {
                file: "terminal.hir".into(),
                line: 7,
                ..Default::default()
            },
            message: "unchecked addition overflow".into(),
        }),
    ];
    for stop in stops {
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                for jitter in [false, true] {
                    let (rows, complete) = stopped_trace(width, reorder, jitter, stop.clone());
                    assert!(complete);
                    assert_eq!(rows.len(), 4);
                    let end = rows.last().unwrap();
                    assert_eq!(end.item, CanonicalItem::Terminal);
                    assert_eq!(
                        end.outcome,
                        CanonicalOutcome::Stopped(stop.clone().outcome())
                    );
                    assert_eq!(end.transition, Transition::Stop);
                    assert_eq!(
                        (end.ordinal, end.subevent, end.version, end.retirement),
                        (3, 0, 1, 3)
                    );
                    assert_eq!(
                        rows[2].outcome,
                        CanonicalOutcome::Scalar(23u64.to_le_bytes().to_vec())
                    );
                    if let Some(reference) = &reference {
                        assert_eq!(&rows, reference);
                    } else {
                        reference = Some(rows);
                    }
                }
            }
        }
    }
}

#[test]
fn canonical_terminal_rejects_unmapped_blocked_targets() {
    for width in [1, 2, 4, 8] {
        let (rows, complete) = stopped_trace(
            width,
            hirsel_run_config::Reorder::Auto,
            false,
            Stop::Blocked(vec![hirsel_run_schema::Wait {
                spawn_path: vec![],
                site: Default::default(),
                kind: "receive".into(),
                targets: vec![0xfeed],
            }]),
        );
        assert!(!complete);
        assert_eq!(rows.len(), 3, "numeric wait target must never be certified");
    }
}

#[test]
fn canonical_failed_stop_owns_normalized_primary_causes_and_spawn_paths() {
    use super::events::{CanonicalItem, CanonicalOutcome, Transition};
    for metadata in [None, Some(false), Some(true)] {
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                for jitter in [false, true] {
                    let (rows, complete) =
                        stopped_trace_with_failures(width, reorder, jitter, Stop::Failed, metadata);
                    assert_eq!(complete, metadata == Some(true));
                    if complete {
                        assert_eq!(rows.len(), 4);
                        let end = rows.last().unwrap();
                        assert_eq!(end.item, CanonicalItem::Terminal);
                        assert_eq!(end.transition, Transition::Stop);
                        assert_eq!(
                            (end.ordinal, end.subevent, end.version, end.retirement),
                            (3, 0, 1, 3)
                        );
                        let CanonicalOutcome::Failures(chain) = &end.outcome else {
                            panic!("missing terminal failure chain");
                        };
                        assert_eq!(chain.len(), 2);
                        assert_eq!(chain[0].1, Vec::<u32>::new());
                        assert_eq!(chain[1].1, [4, 2]);
                        assert_eq!(chain[0].0.rule, hirsel_core_contract::RUNTIME_RULES[0]);
                        assert_eq!(chain[1].0.rule, hirsel_core_contract::RUNTIME_RULES[1]);
                        assert_eq!((chain[0].0.site.line, chain[0].0.site.column), (17, 3));
                        assert_eq!((chain[1].0.site.line, chain[1].0.site.column), (23, 5));
                        assert_eq!(chain[0].0.operation.as_ref(), Some(&chain[1].0.site));
                        assert_eq!(chain[1].0.operation.as_ref(), Some(&chain[0].0.site));
                    } else {
                        assert_eq!(
                            rows.len(),
                            3,
                            "missing chain/table cannot certify an empty failure"
                        );
                    }
                    if let Some(reference) = &reference {
                        assert_eq!(&rows, reference);
                    } else {
                        reference = Some(rows);
                    }
                }
            }
        }
    }
}
