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
    atomic: AtomicU64,
    root_ctx: AtomicU64,
    cleanup_effect: AtomicU64,
    cleanup_after_report: AtomicBool,
    cleanup_status: AtomicU64,
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
            crate::tasks::ordered::engine::with_driver(ctx, |d| {
                d.head_paused(probe.terminal.load(Ordering::Acquire))
            })
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

pub(crate) fn terminal_run(mode: u64) {
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
                        8,
                        0,
                        8,
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
    probe
        .cleanup_after_report
        .store(probe.reported.load(Ordering::Acquire), Ordering::Release);
    probe
        .cleanup_effect
        .store(unsafe { retired_effect(ctx, probe) }, Ordering::Release);
    let mut canceled = u32::MAX;
    let status = unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) };
    probe.cleanup_status.store(status.into(), Ordering::Release);
    probe.cleaned.store(true, Ordering::Release);
    STATUS_OK
}

// Observe the already-retired image without introducing another scheduling
// point. A marker after atomic_store returns belongs to the child's next turn.
unsafe fn retired_effect(ctx: *mut TaskCtx, probe: &Probe) -> u64 {
    let mut value = 0u64;
    let _role = unsafe { role::Guard::new(ctx) };
    unsafe {
        scheduler(ctx).sync.read(
            probe.atomic.load(Ordering::Acquire),
            (&raw mut value).cast(),
            8,
        );
    }
    value
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
    let result = super::drops::tests::finalized(
        crate::Layout {
            finalizer: Some(returned_cleanup),
            ..environment
        },
        &[],
    );
    let layouts = [&environment as *const crate::Layout, result];
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
                    8,
                    0,
                    8,
                ),
                STATUS_OK
            );
            seen.atomic.store(atomic, Ordering::Release);
            seen.root_ctx.store(ctx as u64, Ordering::Release);
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
        let mut sink_effect = 0;
        let mut sink_home = false;
        let mut sink_before_cleanup = false;
        let mut reports = 0;
        let mut rendered = String::new();
        let mut report = |returned: &crate::exit_report::Returned| {
            let ctx = probe.root_ctx.load(Ordering::Acquire) as *mut TaskCtx;
            sink_effect = unsafe { retired_effect(ctx, &probe) };
            sink_home = std::thread::current().id() == home;
            sink_before_cleanup = !probe.cleaned.load(Ordering::Acquire);
            rendered = unsafe { crate::exit_report::render(returned) };
            reports += 1;
            probe.reported.store(true, Ordering::Release);
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
            // Assert on the launcher stack. Runtime fibers deliberately have
            // no unwind linkage, so a sink panic cannot unwind into libtest.
            assert_eq!(sink_effect, 23, "sink overtook older retired store");
            assert!(sink_home, "report sink is home-affine");
            assert!(sink_before_cleanup, "sink must precede result cleanup");
            assert_eq!(rendered, "error: 42\n");
            assert_eq!(reports, 1, "report exactly once");
            assert_eq!(probe.cleanup_effect.load(Ordering::Acquire), 23);
            assert!(probe.cleanup_after_report.load(Ordering::Acquire));
            assert_eq!(
                probe.cleanup_status.load(Ordering::Acquire),
                STATUS_OK as u64
            );
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
                    0,
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

struct BudgetSuffix {
    wide: bool,
    suffix: AtomicBool,
}

unsafe extern "C" fn budget_suffix_head(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    unsafe {
        let [probe, atomic] = crate::alloc::payload(env).cast::<[u64; 2]>().read();
        let probe = &*(probe as *const BudgetSuffix);
        if probe.wide {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !probe.suffix.load(Ordering::Acquire) {
                assert!(
                    std::time::Instant::now() < until,
                    "younger charge suffix never ran ahead"
                );
                std::thread::yield_now();
            }
        }
        hirsel_rt_v2_atomic_store(ctx, &atomic, (&23u64 as *const u64).cast(), 8);
        let _role = role::Guard::new(ctx);
        crate::stop::check(ctx, engine::at_head(ctx));
        unreachable!("younger budget cut must precede this child's next logical head");
    }
}

#[test]
fn budget_terminal_trace_truncates_actual_younger_suffix_and_accumulates_live_head_charges() {
    budget_terminal_trace_truncates_actual_younger_suffix_and_accumulates_live_head_charges_commit(
        hirsel_run_config::Commit::Shared,
    );
}

#[test]
fn dedicated_commit_preserves_first_budget_stop_and_truncates_younger_suffix() {
    budget_terminal_trace_truncates_actual_younger_suffix_and_accumulates_live_head_charges_commit(
        hirsel_run_config::Commit::Dedicated,
    );
}

pub(crate) fn budget_terminal_trace_truncates_actual_younger_suffix_and_accumulates_live_head_charges_commit(
    commit: hirsel_run_config::Commit,
) {
    use super::events::{CanonicalItem, CanonicalOutcome, Transition};
    use crate::budget::stretch_trace::Charge;
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
    let wide_layout = crate::Layout { size: 64, ..layout };
    let layouts = [&layout as *const crate::Layout, &wide_layout];
    for fuel in [false, true] {
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                for jitter in [false, true] {
                    let probe = Arc::new(BudgetSuffix {
                        wide: width > 1,
                        suffix: AtomicBool::new(false),
                    });
                    let seen = probe.clone();
                    let mut ctx = std::ptr::null_mut();
                    unsafe {
                        let mut config = crate::Config::new();
                        config.layouts = layouts.as_ptr();
                        config.layout_count = layouts.len() as u32;
                        assert_eq!(
                            crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
                            0
                        );
                        let charge = crate::alloc::allocation_charge(ctx, 2, 16, 8);
                        let large = crate::alloc::allocation_charge(ctx, 3, 64, 8);
                        (*ctx).set_budgets(fuel.then_some(10), Some(2 * charge - 1));
                        let mut policy = Policy::admit(&Default::default(), Some(2 * charge - 1));
                        policy.speculation_bytes = 1 << 20;
                        policy.run_ahead = 8;
                        policy.reorder = reorder;
                        policy.commit = commit;
                        engine::enable(ctx, policy);
                        super::events::enable(ctx);
                        crate::pool::start(ctx, width, true);
                        crate::pool::free::enable(ctx);
                        let entry: crate::stop::Entry = Box::new(move |ctx, _| {
                            let mut atomic = 0;
                            assert_eq!(
                                hirsel_rt_v2_atomic_create(
                                    ctx,
                                    std::ptr::null_mut(),
                                    &mut atomic,
                                    (&0u64 as *const u64).cast(),
                                    8,
                                    0,
                                    8,
                                ),
                                STATUS_OK
                            );
                            let mut scope = 0;
                            assert_eq!(
                                hirsel_rt_v2_task_scope_enter(
                                    ctx,
                                    std::ptr::null_mut(),
                                    &mut scope,
                                    &0
                                ),
                                STATUS_OK
                            );
                            let env = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                16,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
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
                                    budget_suffix_head,
                                    env,
                                    0
                                ),
                                STATUS_OK
                            );
                            // A physical width1 dispatcher can still execute
                            // run-ahead continuations. Pin this reference to
                            // its logical head without adding an operation.
                            if !seen.wide {
                                let _role = role::Guard::new(ctx);
                                crate::stop::check(ctx, engine::at_head(ctx));
                            }
                            if jitter {
                                for _ in 0..13 {
                                    std::thread::yield_now();
                                }
                            }
                            if fuel {
                                (*ctx).view.compiled.fuel -= 1;
                            }
                            // Width1 stops at this first allocation. A younger
                            // pure segment at widthN can run its whole suffix.
                            let first = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                16,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            assert_eq!(
                                crate::release::hirsel_rt_v2_release(
                                    ctx,
                                    first,
                                    2,
                                    std::ptr::null(),
                                    0,
                                    0
                                ),
                                STATUS_OK
                            );
                            if fuel {
                                (*ctx).view.compiled.fuel -= 2;
                            }
                            let second = crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                3,
                                64,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            );
                            assert_eq!(
                                crate::release::hirsel_rt_v2_release(
                                    ctx,
                                    second,
                                    3,
                                    std::ptr::null(),
                                    0,
                                    0
                                ),
                                STATUS_OK
                            );
                            assert_eq!(
                                hirsel_rt_v2_atomic_store(
                                    ctx,
                                    &atomic,
                                    (&99u64 as *const u64).cast(),
                                    8
                                ),
                                STATUS_OK
                            );
                            seen.suffix.store(true, Ordering::Release);
                            let _role = role::Guard::new(ctx);
                            crate::stop::check(ctx, engine::at_head(ctx));
                            unreachable!("younger rejected turn cannot resume");
                        });
                        assert_eq!(
                            crate::stop::run_root(ctx, crate::stop::LaunchFrame::bare(entry)),
                            Err(Stop::BudgetExceeded(hirsel_run_schema::BudgetKind::Memory))
                        );
                        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
                        while crate::pool::busy((*ctx).run_ref()) != 0 {
                            assert!(
                                std::time::Instant::now() < until,
                                "budget cut did not quiesce"
                            );
                            std::thread::yield_now();
                        }
                        (*ctx).abandon();
                        let s = (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap();
                        let Logical::Ordered(driver) = &s.logical else {
                            unreachable!()
                        };
                        let log = driver.events.as_ref().unwrap();
                        let rows = log.canonical().unwrap();
                        let end = rows.last().unwrap();
                        assert_eq!(end.item, CanonicalItem::Terminal);
                        assert_eq!(end.transition, Transition::Stop);
                        assert_eq!(
                            end.outcome,
                            CanonicalOutcome::Stopped(
                                hirsel_run_schema::RunOutcome::BudgetExceeded(
                                    hirsel_run_schema::BudgetKind::Memory
                                )
                            )
                        );
                        assert_eq!(
                            end.before.exact,
                            if fuel {
                                vec![Charge::Fuel(1), Charge::Alloc(charge)]
                            } else {
                                vec![Charge::Alloc(charge)]
                            }
                        );
                        assert_eq!(
                            rows.iter()
                                .find(|row| row.op == "task.async" && row.subevent == 0)
                                .unwrap()
                                .before
                                .exact,
                            [Charge::Alloc(charge)],
                            "head allocation must survive its partial settlement"
                        );
                        assert_eq!(log.rejected_budget_stretches.len(), 1);
                        let rejected = &log.rejected_budget_stretches[0];
                        if width > 1 {
                            assert!(probe.suffix.load(Ordering::Acquire));
                            assert!(rejected.exact.contains(&Charge::Alloc(large)));
                            assert!(rejected.exact.contains(&Charge::Free(large)));
                            assert!(
                                rejected
                                    .pieces()
                                    .next()
                                    .unwrap()
                                    .1
                                    .envelope
                                    .last()
                                    .unwrap()
                                    .1
                                    > charge as i64
                            );
                        } else {
                            assert!(!probe.suffix.load(Ordering::Acquire));
                        }
                        let rows = rows.to_vec();
                        if let Some(reference) = &reference {
                            assert_eq!(&rows, reference);
                        } else {
                            reference = Some(rows);
                        }
                        crate::context::hirsel_rt_v2_ctx_destroy(ctx);
                    }
                }
            }
        }
    }
}

#[test]
fn exact_charge_trace_preserves_fuel_before_and_after_a_free_across_head_settlements() {
    exact_charge_trace_preserves_fuel_before_and_after_a_free_across_head_settlements_commit(
        hirsel_run_config::Commit::Shared,
    );
}

#[test]
fn dedicated_commit_preserves_exact_budget_charges_across_partial_head_settlements() {
    exact_charge_trace_preserves_fuel_before_and_after_a_free_across_head_settlements_commit(
        hirsel_run_config::Commit::Dedicated,
    );
}

pub(crate) fn exact_charge_trace_preserves_fuel_before_and_after_a_free_across_head_settlements_commit(
    commit: hirsel_run_config::Commit,
) {
    use crate::budget::stretch_trace::Charge;
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
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        let output = Arc::new(std::sync::Mutex::new(None));
        let seen = output.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                let charge = crate::alloc::allocation_charge(ctx, 2, 16, 8);
                (*ctx).set_budgets(Some(10), Some(charge));
                let mut policy = Policy::admit(&Default::default(), Some(charge));
                policy.commit = commit;
                engine::enable(ctx, policy);
                super::events::enable(ctx);
            },
            move |ctx| unsafe {
                let mut atomic = 0;
                assert_eq!(
                    hirsel_rt_v2_atomic_create(
                        ctx,
                        std::ptr::null_mut(),
                        &mut atomic,
                        (&0u64 as *const u64).cast(),
                        8,
                        0,
                        8,
                    ),
                    STATUS_OK
                );
                let charge = crate::alloc::allocation_charge(ctx, 2, 16, 8);
                let block =
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0);
                (*ctx).view.compiled.fuel -= 1;
                assert_eq!(
                    crate::release::hirsel_rt_v2_release(ctx, block, 2, std::ptr::null(), 0, 0),
                    STATUS_OK
                );
                (*ctx).view.compiled.fuel -= 2;
                assert_eq!(
                    hirsel_rt_v2_atomic_store(ctx, &atomic, (&9u64 as *const u64).cast(), 8),
                    STATUS_OK
                );
                let _role = role::Guard::new(ctx);
                engine::at_head(ctx).unwrap();
                let log = engine::driver(ctx).events.as_ref().unwrap();
                let rows = log.canonical().unwrap();
                let store = rows.last().unwrap();
                assert_eq!(store.op, "atomic.store");
                assert_eq!(
                    store.before.exact,
                    [
                        Charge::Alloc(charge),
                        Charge::Fuel(1),
                        Charge::Free(charge),
                        Charge::Fuel(2)
                    ]
                );
                let piece = store.before.pieces().next().unwrap().1;
                assert_eq!((piece.fuel, piece.net), (3, 0));
                assert_eq!(piece.envelope, [(0, charge as i64)]);
                *seen.lock().unwrap() = Some(rows.to_vec());
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        let rows = output.lock().unwrap().take().unwrap();
        if let Some(reference) = &reference {
            assert_eq!(&rows, reference);
        } else {
            reference = Some(rows);
        }
    }
}

unsafe extern "C" fn blocked_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    unsafe {
        let mut receiver = crate::alloc::payload(env).cast::<u64>().read();
        let mut status = u32::MAX;
        let mut slot = [0u64; 2];
        hirsel_rt_v2_channel_receive(
            ctx,
            0,
            0,
            &mut status,
            &mut receiver,
            slot.as_mut_ptr().cast(),
        );
        unreachable!("blocked child must never resume");
    }
}

#[test]
fn canonical_blocked_cut_uses_logical_wait_targets_after_every_frame_is_saved() {
    use super::events::{CanonicalItem, CanonicalOutcome, Transition};
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
    let env_layout = crate::Layout { size: 8, ..layout };
    let layouts = [&layout as *const crate::Layout, &env_layout];
    for children in [false, true] {
        for known_site in [true, false] {
            let mut reference = None;
            let mut raw_reference = None;
            for width in [1, 2, 4, 8] {
                for reorder in [
                    hirsel_run_config::Reorder::Off,
                    hirsel_run_config::Reorder::Auto,
                ] {
                    for jitter in [false, true] {
                        let mut ctx = std::ptr::null_mut();
                        unsafe {
                            let mut config = crate::Config::new();
                            config.layouts = layouts.as_ptr();
                            config.layout_count = layouts.len() as u32;
                            assert_eq!(
                                crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
                                0
                            );
                            let mut policy = Policy::admit(&Default::default(), None);
                            policy.reorder = reorder;
                            engine::enable(ctx, policy);
                            super::events::enable(ctx);
                            // Perturb only provisional storage identities; no
                            // object is initialized or operation introduced.
                            for _ in 1..width {
                                let _reserved = crate::tasks::role::with_role(ctx, |s| {
                                    s.channels.reserve_open()
                                });
                            }
                            crate::pool::start(ctx, width, true);
                            crate::pool::free::enable(ctx);
                            let forced_saves = Arc::new(AtomicU64::new(0));
                            let observed_saves = forced_saves.clone();
                            let entry: crate::stop::Entry = Box::new(move |ctx, _| {
                                // Force overlapping final saves: hold the first child
                                // after its scheduler callback until every waiter has
                                // saved. Every old callback must defer to this token;
                                // the transition to idle must then recheck CONC-8.
                                if children && width > 1 && jitter {
                                    let (notify_saved, receive) = std::sync::mpsc::channel();
                                    let receive = std::sync::Mutex::new(receive);
                                    let sent = AtomicBool::new(false);
                                    let run_address = (*ctx).run_ptr() as usize;
                                    let held = AtomicBool::new(false);
                                    let observed_saves = observed_saves.clone();
                                    crate::pool::free::test_before_idle(ctx, move |task| {
                                        let run = run_address as *mut crate::context::RunShared;
                                        let (parked, all_saved) = {
                                            let _guard = role::IdleGuard::new(run);
                                            let s = (*(*run).tasks.get()).as_deref().unwrap();
                                            let parked = s.tasks[task as usize]
                                                .as_ref()
                                                .is_some_and(|row| {
                                                    matches!(
                                                        row.wait,
                                                        Some(
                                                            Wait::Receive { .. }
                                                                | Wait::Exit { .. }
                                                        )
                                                    )
                                                });
                                            let all_saved = s.tasks.iter().flatten().count() == 3
                                                && s.tasks
                                                    .iter()
                                                    .flatten()
                                                    .all(|row| row.state == State::Parked)
                                                && s.tasks
                                                    .iter()
                                                    .enumerate()
                                                    .filter(|(_, row)| row.is_some())
                                                    .all(|(task, _)| {
                                                        readiness::saved(s, task as u32)
                                                    });
                                            (parked, all_saved)
                                        };
                                        // This hook follows check_quiet and runs outside
                                        // its role. No old save callback can see the held
                                        // child's notification idle before its verdict.
                                        if all_saved && !sent.swap(true, Ordering::SeqCst) {
                                            observed_saves.fetch_or(2, Ordering::SeqCst);
                                            notify_saved.send(()).unwrap();
                                        }
                                        if task == 1 && parked && !held.swap(true, Ordering::SeqCst)
                                        {
                                            observed_saves.fetch_or(1, Ordering::SeqCst);
                                            receive.lock().unwrap().recv_timeout(std::time::Duration::from_secs(5))
                                                .expect("all final waiters must save before the held token becomes idle");
                                        }
                                    });
                                }
                                let mut scope = 0;
                                if children {
                                    assert_eq!(
                                        hirsel_rt_v2_task_scope_enter(
                                            ctx,
                                            std::ptr::null_mut(),
                                            &mut scope,
                                            &0
                                        ),
                                        STATUS_OK
                                    );
                                }
                                for _ in 0..if children { 2 } else { 1 } {
                                    let mut endpoints = [0u64; 2];
                                    let none = [0u64; 2];
                                    assert_eq!(
                                        hirsel_rt_v2_channel_open(
                                            ctx,
                                            std::ptr::null_mut(),
                                            endpoints.as_mut_ptr(),
                                            &0,
                                            none.as_ptr().cast(),
                                            2
                                        ),
                                        STATUS_OK
                                    );
                                    if children {
                                        let env = crate::alloc::hirsel_rt_v2_alloc(
                                            ctx,
                                            3,
                                            8,
                                            8,
                                            crate::alloc::DOMAIN_HEAP,
                                            0,
                                        );
                                        crate::alloc::payload(env)
                                            .cast::<u64>()
                                            .write(endpoints[1]);
                                        let mut child = 0;
                                        assert_eq!(
                                            hirsel_rt_v2_task_spawn(
                                                ctx,
                                                0,
                                                0,
                                                std::ptr::null_mut(),
                                                &mut child,
                                                &scope,
                                                blocked_child,
                                                env,
                                                0
                                            ),
                                            STATUS_OK
                                        );
                                    } else {
                                        let mut status = u32::MAX;
                                        let mut slot = [0u64; 2];
                                        hirsel_rt_v2_channel_receive(
                                            ctx,
                                            if known_site { 0 } else { 17 },
                                            0,
                                            &mut status,
                                            &mut endpoints[1],
                                            slot.as_mut_ptr().cast(),
                                        );
                                    }
                                }
                                if children {
                                    hirsel_rt_v2_task_scope_exit(
                                        ctx,
                                        if known_site { 0 } else { 17 },
                                        0,
                                        &scope,
                                        0,
                                    );
                                }
                                unreachable!("fully blocked tree cannot return");
                            });
                            let control = crate::host::RunControl::from_inner(
                                (*ctx).run_ref().control.clone(),
                            );
                            let (done, finished) = std::sync::mpsc::channel();
                            let watchdog = std::thread::spawn(move || {
                                if finished
                                    .recv_timeout(std::time::Duration::from_secs(5))
                                    .is_err()
                                {
                                    control.stop();
                                    false
                                } else {
                                    true
                                }
                            });
                            let outcome =
                                crate::stop::run_root(ctx, crate::stop::LaunchFrame::bare(entry));
                            let Err(Stop::Blocked(raw)) = outcome else {
                                panic!("{outcome:?}")
                            };
                            assert_eq!(raw.len(), if children { 3 } else { 1 });
                            (*ctx).abandon();
                            assert_eq!(crate::pool::busy((*ctx).run_ref()), 0);
                            if children && width > 1 && jitter {
                                assert_eq!(
                                    forced_saves.load(Ordering::SeqCst),
                                    3,
                                    "both sides of the forced idle handoff executed"
                                );
                            }
                            let _ = done.send(());
                            assert!(
                                watchdog.join().unwrap(),
                                "blocked cut watchdog: {children}/{known_site}/{width}/{reorder:?}/{jitter}"
                            );

                            let s = (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap();
                            assert!(
                                s.tasks
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, row)| row.is_some())
                                    .all(|(task, _)| readiness::saved(s, task as u32))
                            );
                            let Logical::Ordered(driver) = &s.logical else {
                                unreachable!()
                            };
                            let log = driver.events.as_ref().unwrap();
                            assert_eq!(log.canonical().is_ok(), known_site);
                            if known_site {
                                let rows = log.canonical().unwrap();
                                let end = rows.last().unwrap();
                                assert_eq!(end.item, CanonicalItem::Terminal);
                                assert_eq!(end.transition, Transition::Stop);
                                assert!(end.task.is_empty());
                                assert_eq!((end.subevent, end.version), (0, 1));
                                let CanonicalOutcome::Blocked(waits) = &end.outcome else {
                                    panic!("no blocked snapshot")
                                };
                                assert_eq!(waits.len(), raw.len());
                                for (wait, original) in waits.iter().zip(&raw) {
                                    assert_eq!(wait.task, original.spawn_path);
                                    assert_eq!(wait.kind, original.kind);
                                    assert_eq!(wait.site, original.site);
                                }
                                if children {
                                    assert_eq!(waits[0].kind, "task.scope");
                                    assert_eq!(waits[0].targets.len(), 3);
                                    assert_eq!(waits[1].task, [0]);
                                    assert_eq!(waits[2].task, [1]);
                                    assert_ne!(waits[1].targets, waits[2].targets);
                                } else {
                                    assert_eq!(
                                        waits[0].targets,
                                        [crate::ordered::footprint::ObjectId {
                                            creator: vec![],
                                            ordinal: 0
                                        }]
                                    );
                                }
                                if let Some(reference) = &reference {
                                    assert_eq!(rows, reference);
                                } else {
                                    reference = Some(rows.to_vec());
                                }
                                let targets: Vec<_> = raw
                                    .iter()
                                    .filter(|wait| wait.kind == "channel.receive")
                                    .flat_map(|wait| wait.targets.iter().copied())
                                    .collect();
                                if width == 1 {
                                    raw_reference = Some(targets);
                                } else {
                                    assert_ne!(
                                        Some(&targets),
                                        raw_reference.as_ref(),
                                        "negative control must change physical channel indices"
                                    );
                                }
                            }
                            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
                        }
                    }
                }
            }
        }
    }
}
