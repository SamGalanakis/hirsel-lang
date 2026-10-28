use super::*;
use crate::io::{Adapter, AdapterError, Completion, CpuSummary, Event, Request, Tree};
use crate::tasks::ordered::{cpu, engine, events};
use std::sync::{Arc, Mutex};
struct Capture(Arc<Mutex<Vec<Event>>>);
impl Adapter for Capture {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Err(AdapterError::InvalidCompletion)
    }
    fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
        Err(AdapterError::InvalidCompletion)
    }
    fn cpu_control(
        &mut self,
        width: u32,
        action: u32,
        count: u64,
        outcome: Option<u32>,
    ) -> Result<Option<u32>, AdapterError> {
        if let Some(outcome) = outcome {
            self.0.lock().unwrap().push(Event {
                request: Request::CpuControl {
                    width,
                    action,
                    count,
                    outcome,
                },
                completion: Completion::Ready,
                fault: None,
                resources: None,
            });
        }
        Ok(outcome)
    }
    fn cpu_summary(&mut self, summary: &CpuSummary) -> Result<(), AdapterError> {
        self.0.lock().unwrap().push(Event {
            request: Request::CpuSummary(summary.clone()),
            completion: Completion::Ready,
            fault: None,
            resources: None,
        });
        Ok(())
    }
}

unsafe extern "C" fn unused(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
    crate::STATUS_OK
}

#[unsafe(naked)]
unsafe extern "C" fn publishing(_summary: *mut u8, _native: *mut u8) -> ! {
    core::arch::naked_asm!(
        "push rbx", "mov rbx, rsi", "mov rsi, rdi", "mov rdi, rbx",
        "call {publish}", "test eax, eax", "jne 2f",
        "mov rdi, rbx", "mov esi, 32", "call {handoff}", "2:", "ud2",
        publish = sym hirsel_rt_v2_cpu_ahead_publish,
        handoff = sym super::super::native::hirsel_rt_v2_ahead_handoff,
    );
}

#[test]
fn naked_cpu_summary_returns_to_the_same_call_after_one_head_settlement() {
    let outcome = run_summary(1, None);
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
}

fn run_summary(
    width: u32,
    adapter: Option<Box<dyn crate::io::Adapter>>,
) -> crate::host::BodyOutcome {
    run_prefix(width, adapter, None)
}

fn run_prefix(
    width: u32,
    adapter: Option<Box<dyn crate::io::Adapter>>,
    nonhead: Option<bool>,
) -> crate::host::BodyOutcome {
    let mut run = crate::host::BodyRun::new(width, false);
    run.adapter = adapter;
    let scan = [
        hirsel_core_contract::handle_scan::VALUE,
        hirsel_core_contract::handle_scan::VALUE_BITS,
        8,
        0,
        0,
        0,
        0,
        0,
    ];
    let layout = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: 8,
        align: 8,
        finalizer: None,
        steps: std::ptr::null(),
        step_count: 0,
        field_count: 0,
        flags: 0,
        logical: 0,
        logical_count: std::ptr::null(),
        scan: scan.as_ptr(),
        scan_words: 8,
    };
    let layouts = [&raw const layout];
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    run.run(
        |ctx| unsafe {
            (*ctx).run().cpu_grant = Some(hirsel_run_schema::CpuGrant {
                provider: "cpu.scoped@1".into(),
                max_workers: 4,
                max_domains: 1,
                max_physical_bytes: 2 << 30,
                max_batch_bytes: 1 << 20,
            });
            (*ctx).set_budgets(Some(100), Some(1 << 20));
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
            (*ctx).ahead_physical = &raw mut native.physical;
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_arm(ctx);
            super::super::cpu_worker::hirsel_rt_v2_cpu_ahead_enter((*ctx).ahead_physical);
            let value = 42u64;
            let mut summary = Summary {
                version: VERSION,
                phase: SUMMARY_PENDING,
                ordinal: 0,
                value: (&raw const value).cast(),
                value_bytes: 8,
                descriptor: 2,
                retained_count: 0,
                retained: std::ptr::null(),
            };
            let stack = crate::fiber::Stack::new();
            let sp = stack.prepare(
                publishing,
                (&raw mut summary).cast(),
                (&raw mut *native).cast(),
            );
            let initial = engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled);
            if let Some(nonhead) = nonhead {
                crate::stop::set_running(std::ptr::null_mut());
                crate::tasks::role::enter(ctx);
                let task = engine::current(ctx);
                let head = nonhead.then(|| {
                    engine::with_driver(ctx, |d| {
                        let original = d.frontier.take().unwrap();
                        let mut ahead = crate::ordered::Frontier::new(
                            task.wrapping_add(1 << 32),
                            crate::ordered::Policy::admit(&Default::default(), None),
                            original.settled,
                        );
                        ahead.enqueue(task, crate::ordered::footprint::Held::empty());
                        d.frontier = Some(ahead);
                        original
                    })
                });
                for bytes in [16, 32] {
                    (*ctx).view.compiled.fuel -= 1;
                    let allocation = engine::ahead_allocation(ctx, bytes).unwrap();
                    assert!(matches!(
                        (nonhead, allocation),
                        (true, engine::CpuAllocation::Summarized)
                            | (false, engine::CpuAllocation::Settled)
                    ));
                }
                if let Some(head) = head {
                    engine::with_driver(ctx, |d| d.frontier = Some(head));
                }
                crate::tasks::role::release(ctx);
                crate::stop::set_running(ctx);
            }
            let before_publication =
                engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled);
            (*ctx).view.compiled.fuel -= 3;
            crate::fiber::switch(&raw mut native.physical.idle_sp, sp);
            assert_eq!(native.physical.reason, 16);
            assert_eq!(native.physical.saved_sp, sp.wrapping_sub(32));
            assert_eq!(summary.phase, SUMMARY_PENDING);
            assert_eq!(
                engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled),
                before_publication
            );
            crate::stop::set_running(std::ptr::null_mut());
            crate::tasks::role::enter(ctx);
            assert_eq!(serve(ctx, &raw mut *native), Ok(true));
            assert_eq!(summary.phase, SUMMARY_RETIRED);
            assert!(native.summary.is_null());
            assert_eq!(
                engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled.fuel),
                initial.fuel - if nonhead.is_some() { 5 } else { 3 }
            );
            crate::tasks::role::release(ctx);
            crate::stop::set_running(ctx);
            let saved = native.physical.saved_sp;
            crate::fiber::switch(&raw mut native.physical.idle_sp, saved);
            assert_eq!(native.physical.reason, 32);
            assert_eq!(summary.value.cast::<u64>().read(), 42);
            (*ctx).ahead_physical = std::ptr::null_mut();
            crate::STATUS_OK
        },
    )
}

#[test]
fn settled_native_summary_records_once_and_replays_at_every_runtime_width() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let live = run_summary(4, Some(Box::new(Capture(captured.clone()))));
    assert_eq!(live.ended, Ok(crate::STATUS_OK));
    assert_eq!(live.pending, 0);
    let events = captured.lock().unwrap().clone();
    assert_eq!(
        events.len(),
        3,
        "two admission receipts and exactly one settled prefix"
    );
    let Request::CpuSummary(summary) = &events[2].request else {
        panic!("summary follows its claim")
    };
    assert_eq!(summary.value, 42u64.to_le_bytes());
    assert_eq!(summary.ordinal, 0);
    assert_eq!(summary.retirement, 0);
    assert_eq!(
        summary
            .native
            .as_ref()
            .unwrap()
            .pieces
            .iter()
            .map(|p| p.fuel)
            .sum::<u64>(),
        3
    );
    assert_eq!(summary.native.as_ref().unwrap().fuel, 97);
    assert_eq!(summary.stop, None);
    for width in [1, 2, 4, 8] {
        let replay = run_summary(
            width,
            Some(Box::new(crate::io_sim::SimIo::new(
                Tree::default(),
                events.clone(),
            ))),
        );
        assert_eq!(replay.ended, Ok(crate::STATUS_OK), "replay W{width}");
        assert_eq!(replay.pending, 0);
    }
}

#[test]
fn head_and_nonhead_allocation_prefixes_record_identical_charges_at_every_runtime_width() {
    for width in [1, 2, 4, 8] {
        let mut canonical = None;
        for nonhead in [false, true] {
            let captured = Arc::new(Mutex::new(Vec::new()));
            let live = run_prefix(
                width,
                Some(Box::new(Capture(captured.clone()))),
                Some(nonhead),
            );
            assert_eq!(live.ended, Ok(crate::STATUS_OK));
            assert_eq!(live.pending, 0);
            let events = captured.lock().unwrap().clone();
            assert_eq!(events.len(), 3);
            let Request::CpuSummary(summary) = &events[2].request else {
                panic!("summary follows the claim")
            };
            assert_eq!(summary.native.as_ref().unwrap().pieces.len(), 1);
            assert_eq!(summary.native.as_ref().unwrap().pieces[0].fuel, 5);
            assert_eq!(summary.native.as_ref().unwrap().pieces[0].net, 48);
            assert_eq!(
                summary.native.as_ref().unwrap().pieces[0].envelope,
                [(1, 16), (2, 48)]
            );
            assert_eq!(summary.native.as_ref().unwrap().level, 48);
            assert_eq!(summary.native.as_ref().unwrap().fuel, 95);
            if let Some(canonical) = &canonical {
                assert_eq!(&events, canonical, "W{width}, nonhead {nonhead}");
            } else {
                canonical = Some(events.clone());
            }
            // Serial replay may choose the opposite physical settlement path.
            let replay = run_prefix(
                width,
                Some(Box::new(crate::io_sim::SimIo::new(Tree::default(), events))),
                Some(!nonhead),
            );
            assert_eq!(replay.ended, Ok(crate::STATUS_OK));
            assert_eq!(replay.pending, 0);
        }
    }
}

#[test]
fn compiler_reads_the_original_worker_environment() {
    let mut run = crate::host::BodyRun::new(1, false);
    let outcome = run.run(
        |_| {},
        |ctx| {
            let environment = std::ptr::dangling_mut::<Header>();
            let certificate = super::super::native::Certificate {
                entry: unused,
                stack_bytes: 256,
                host: std::ptr::null(),
                resume: None,
                cpu: true,
                preclaimed: false,
                live: std::ptr::null_mut(),
                mask: 0,
                source: std::ptr::null_mut(),
            };
            let native = Native::new(ctx, environment, 0, certificate);
            let offset = hirsel_lir::runtime_abi::AHEAD_WORKER_ENVIRONMENT_OFFSET as usize;
            // Native's compile-time prefix assertions make byte 40 an initialized
            // pointer. Reject an invalid compiler offset before reading that prefix.
            assert_eq!(offset, 40);
            // SAFETY: the repr(C) physical record starts Native; the asserted prefix
            // stores the constructor's live environment pointer here. No pointee is read.
            let actual = unsafe {
                (&raw const native.physical)
                    .cast::<u8>()
                    .add(offset)
                    .cast::<*mut Header>()
                    .read()
            };
            assert_eq!(actual, environment);
            crate::STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert_eq!(outcome.pending, 0);
}
