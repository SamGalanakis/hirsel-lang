//! Run-local observations for M0b's existing policy. Timing affects eligibility
//! only through Frontier::tune; it never changes the four early-commit rules,
//! placement, semantic operations, budgets, or recording/replay inputs.
use std::collections::BTreeMap;
use std::time::Instant;

use crate::multicore_policy::{AUTO_WINDOW_TURNS, AutoSample};
use crate::ordered::footprint::TaskId;
use crate::ordered::{Frontier, Offer};

#[derive(Default)]
struct Segment {
    started: Option<Instant>,
    after_early: bool,
}

pub(super) struct Tuning {
    enabled: bool,
    probing: bool,
    next: u64,
    sample: AutoSample,
    segments: u64,
    running: BTreeMap<TaskId, Segment>,
    pub(super) windows: u64,
    pub(super) fallbacks: u64,
    pub(super) probes: u64,
    pub(super) early: u64,
}

impl Tuning {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            probing: true,
            next: AUTO_WINDOW_TURNS,
            sample: AutoSample::default(),
            segments: 0,
            running: BTreeMap::new(),
            windows: 0,
            fallbacks: 0,
            probes: 1,
            early: 0,
        }
    }

    pub(super) fn start(&mut self, task: TaskId) {
        if self.enabled {
            self.running
                .entry(task)
                .or_default()
                .started
                .get_or_insert_with(Instant::now);
        }
    }

    /// Called at semantic boundaries and before physical save. A suspended
    /// interval is never counted as useful work. Credit only the continuation
    /// enabled by an actual early result, while its owner is still non-head.
    /// Preemption can perturb timings, so the verdict remains host policy.
    pub(super) fn end(&mut self, task: TaskId, nonhead: bool) {
        let Some(segment) = self.running.get_mut(&task) else {
            return;
        };
        if let Some(started) = segment.started.take() {
            let ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            self.segments = self.segments.saturating_add(1);
            self.sample.segment_ns = self.sample.segment_ns.saturating_add(ns);
            if nonhead && segment.after_early {
                self.sample.saved_ns = self.sample.saved_ns.saturating_add(ns);
            }
        }
        segment.after_early = false;
    }

    pub(super) fn clock(&self) -> Option<Instant> {
        self.enabled.then(Instant::now)
    }

    pub(super) fn offer(&mut self, task: TaskId, offer: Offer, overhead_ns: u64) {
        if !self.enabled {
            return;
        }
        self.sample.probes = self.sample.probes.saturating_add(1);
        self.sample.overhead_ns = self.sample.overhead_ns.saturating_add(overhead_ns);
        if offer == Offer::Early {
            self.sample.eligible = self.sample.eligible.saturating_add(1);
            self.early = self.early.saturating_add(1);
            self.running.entry(task).or_default().after_early = true;
        }
    }

    pub(super) fn retire<P, R>(&mut self, frontier: &mut Frontier<P, R>) {
        if !self.enabled || frontier.retired() < self.next {
            return;
        }
        self.windows = self.windows.saturating_add(1);
        self.next = frontier.retired().saturating_add(AUTO_WINDOW_TURNS);
        if self.probing {
            let mut sample = self.sample;
            sample.segment_ns /= self.segments.max(1);
            frontier.tune(sample);
            self.probing = crate::multicore_policy::reorder(sample);
            if !self.probing {
                self.fallbacks = self.fallbacks.saturating_add(1);
            }
        } else {
            // One bounded recovery window after one strict window, including
            // when the shape offered no eligible work during strict execution.
            frontier.probe();
            self.probing = true;
            self.probes = self.probes.saturating_add(1);
        }
        self.sample = AutoSample::default();
        self.segments = 0;
    }

    pub(super) fn finished(&mut self, task: TaskId) {
        self.running.remove(&task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::stretch::{Budgets, Settled, Stretch};
    use crate::ordered::{Backend, Next, Operation, Policy, Receipt};
    use hirsel_rt_operations::descriptors::op_descriptor;

    struct Objects;
    impl Backend<()> for Objects {
        type Result = ();
        fn commit(&mut self, _: TaskId, _: &Operation<()>, _: bool) -> Receipt<()> {
            Receipt {
                result: (),
                next: Next::Yield,
                credit: 0,
            }
        }
        fn record(&mut self, _: TaskId, _: &Operation<()>, _: &(), _: u64) {}
        fn publish(&mut self, _: TaskId, _: &Operation<()>, _: &()) {}
        fn poll(&mut self) -> Vec<TaskId> {
            vec![]
        }
    }
    fn frontier() -> Frontier<(), ()> {
        let mut frontier = Frontier::new(
            0,
            Policy::admit(&Default::default(), None),
            Settled {
                level: 0,
                limit: u64::MAX,
                fuel: u64::MAX,
            },
        );
        frontier
            .held
            .insert(0, crate::ordered::footprint::Held::empty());
        frontier.enqueue(1, crate::ordered::footprint::Held::empty());
        frontier
    }
    fn window(tuning: &mut Tuning, frontier: &mut Frontier<(), ()>) {
        for _ in 0..AUTO_WINDOW_TURNS {
            frontier
                .apply_head(
                    Stretch::new(Budgets {
                        fuel: false,
                        memory: false,
                    }),
                    Operation {
                        descriptor: op_descriptor("atomic.store").unwrap(),
                        footprint: Default::default(),
                        payload: (),
                    },
                    &mut Objects,
                )
                .unwrap();
        }
        tuning.retire(frontier);
    }

    #[test]
    fn bounded_windows_decline_short_conflicting_and_losing_shapes_then_recover() {
        let mut tuning = Tuning::new(true);
        let mut frontier = frontier();
        let operation = Operation {
            descriptor: op_descriptor("atomic.store").unwrap(),
            footprint: Default::default(),
            payload: (),
        };
        for sample in [
            AutoSample {
                segment_ns: 1,
                probes: 100,
                eligible: 100,
                saved_ns: 100_000,
                overhead_ns: 1,
            },
            AutoSample {
                segment_ns: 100_000,
                probes: 100,
                eligible: 24,
                saved_ns: 100_000,
                overhead_ns: 1,
            },
            AutoSample {
                segment_ns: 100_000,
                probes: 100,
                eligible: 100,
                saved_ns: 100,
                overhead_ns: 100,
            },
        ] {
            tuning.sample = sample;
            tuning.segments = 1;
            window(&mut tuning, &mut frontier);
            assert!(!frontier.eligible(1, &operation));
            let fallbacks = tuning.fallbacks;
            tuning.retire(&mut frontier);
            assert_eq!(
                tuning.fallbacks, fallbacks,
                "no repeated decision in one window"
            );
            window(&mut tuning, &mut frontier);
            assert!(frontier.eligible(1, &operation), "bounded recovery probe");
        }
        tuning.sample = AutoSample {
            segment_ns: 100_000,
            probes: 100,
            eligible: 100,
            saved_ns: 100_000,
            overhead_ns: 100,
        };
        tuning.segments = 1;
        window(&mut tuning, &mut frontier);
        assert!(frontier.eligible(1, &operation));
        assert_eq!((tuning.windows, tuning.fallbacks, tuning.probes), (7, 3, 4));
    }

    #[test]
    fn useful_time_requires_an_early_continuation_and_omits_saved_intervals() {
        let mut tuning = Tuning::new(true);
        tuning.offer(1, Offer::Early, 7);
        tuning.start(1);
        tuning.running.get_mut(&1).unwrap().started =
            Some(Instant::now() - std::time::Duration::from_millis(1));
        tuning.end(1, true);
        assert!(tuning.sample.saved_ns >= 1_000_000);
        let saved = tuning.sample.saved_ns;
        let segments = tuning.segments;
        tuning.end(1, true);
        assert_eq!(
            tuning.segments, segments,
            "saved time cannot be sampled twice"
        );
        tuning.offer(1, Offer::Pending, 11);
        tuning.start(1);
        tuning.end(1, true);
        tuning.offer(1, Offer::Early, 13);
        tuning.start(1);
        tuning.end(1, false);
        assert_eq!(
            tuning.sample.saved_ns, saved,
            "head and pending work earn no parallel credit"
        );
        assert_eq!(
            (
                tuning.sample.probes,
                tuning.sample.eligible,
                tuning.sample.overhead_ns
            ),
            (3, 2, 31)
        );
        tuning.finished(1);
        assert!(tuning.running.is_empty());
    }

    #[test]
    fn explicit_off_retains_no_timing_state_or_recovery_probes() {
        let mut tuning = Tuning::new(false);
        assert!(tuning.clock().is_none());
        tuning.start(1);
        tuning.offer(1, Offer::Early, 100);
        tuning.end(1, true);
        window(&mut tuning, &mut frontier());
        assert!(tuning.running.is_empty());
        assert_eq!((tuning.windows, tuning.early), (0, 0));
    }

    #[test]
    fn native_auto_samples_drive_bounded_fallback_and_recovery_without_trace_changes() {
        use crate::tasks::{self, ordered::engine, role};
        use hirsel_run_config::{Commit, Reorder};
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for commit in [Commit::Shared, Commit::Dedicated] {
                for reorder in [Reorder::Off, Reorder::Auto] {
                    let mut run = crate::host::BodyRun::new(width, true);
                    unsafe { run.use_physical_dispatch() };
                    let (outcome, trace) = run.run_traced(
                        |ctx| unsafe {
                            let mut policy = Policy::admit(&Default::default(), None);
                            policy.commit = commit;
                            policy.reorder = reorder;
                            engine::enable(ctx, policy);
                            crate::tasks::ordered::events::enable(ctx);
                        },
                        move |ctx| unsafe {
                            let mut atomic = 0;
                            tasks::hirsel_rt_v2_atomic_create(
                                ctx,
                                std::ptr::null_mut(),
                                &mut atomic,
                                (&0u64 as *const u64).cast(),
                                8,
                            );
                            for value in 0..256u64 {
                                tasks::hirsel_rt_v2_atomic_store(
                                    ctx,
                                    &atomic,
                                    (&raw const value).cast(),
                                    8,
                                );
                            }
                            let _guard = role::Guard::new(ctx);
                            let tuning = &engine::driver(ctx).tuning;
                            if reorder == Reorder::Auto {
                                assert!(tuning.windows >= 3);
                                assert!(
                                    tuning.fallbacks >= 2,
                                    "no parallel work is a losing window"
                                );
                                assert!(tuning.probes >= 2);
                                assert!(tuning.segments > 0);
                            } else {
                                assert_eq!(tuning.windows, 0);
                            }
                            crate::STATUS_OK
                        },
                    );
                    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                    assert_eq!(outcome.pending, 0);
                    let trace = trace
                        .expect("complete native tuning trace")
                        .bytes()
                        .to_vec();
                    if let Some(reference) = &reference {
                        assert_eq!(&trace, reference);
                    } else {
                        reference = Some(trace);
                    }
                }
            }
        }
    }

    unsafe extern "C" fn conflicting_child(
        ctx: *mut crate::TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        let word = unsafe { crate::alloc::payload(environment).cast::<u64>().read() };
        for value in 0..256u64 {
            unsafe {
                crate::tasks::hirsel_rt_v2_atomic_store(ctx, &word, (&raw const value).cast(), 8)
            };
        }
        let none = [0u64; 2];
        let mut slot = [1u64, 0];
        unsafe {
            crate::tasks::hirsel_rt_v2_task_finish(
                ctx,
                slot.as_mut_ptr().cast(),
                none.as_ptr().cast(),
                16,
                0,
            )
        }
    }

    #[test]
    fn actual_shared_object_siblings_measure_conflict_and_bounded_recovery() {
        use crate::tasks::{self, ordered::engine, role};
        use hirsel_core_contract::handle_scan::{TYPED_HANDLE, WORDS};
        let scan = [
            TYPED_HANDLE,
            0,
            crate::ordered::objects::Kind::Atomic.code(),
            2,
            0,
            0,
            0,
            0,
        ];
        assert_eq!(scan.len(), WORDS);
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
            scan_words: scan.len() as u64,
        };
        let layouts = [&layout as *const crate::Layout];
        for width in [2, 4, 8] {
            for commit in [
                hirsel_run_config::Commit::Shared,
                hirsel_run_config::Commit::Dedicated,
            ] {
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(|ctx| unsafe {
                    let mut policy = Policy::admit(&Default::default(), None);
                    policy.commit = commit;
                    engine::enable(ctx, policy);
                }, move |ctx| unsafe {
                    let mut scope = 0;
                    tasks::hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                    let mut word = 0;
                    tasks::hirsel_rt_v2_atomic_create(ctx, std::ptr::null_mut(), &mut word,
                        (&0u64 as *const u64).cast(), 8);
                    for _ in 0..2 {
                        let environment = crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
                        crate::alloc::payload(environment).cast::<u64>().write(word);
                        let mut task = 0;
                        tasks::hirsel_rt_v2_task_spawn(ctx, 0, 0, std::ptr::null_mut(), &mut task, &scope, conflicting_child, environment, 0);
                    }
                    tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
                    let _guard = role::Guard::new(ctx);
                    let tuning = &engine::driver(ctx).tuning;
                    assert!(tuning.windows >= 6);
                    assert!(tuning.fallbacks >= 3);
                    assert!(tuning.probes >= 3);
                    assert_eq!(tuning.early, 0, "an earlier live carrier prevents early mutation");
                    assert!(tuning.running.len() <= 1, "completed child timing rows retire");
                    eprintln!("width={width} commit={commit:?} actual-conflict windows={} fallback={} probes={} early={}", tuning.windows, tuning.fallbacks, tuning.probes, tuning.early);
                    crate::STATUS_OK
                });
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(outcome.pending, 0);
            }
        }
    }
}
