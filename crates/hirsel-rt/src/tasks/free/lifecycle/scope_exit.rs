//! Persistent nursery exit, following Trio's `_nested_child_finished`
//! (_run.py:1272-1355 at 0480602b48eda20f10283e7ac04ed01848d7acff).
//! Hirsel retains noncanceling normal joins, reverse child rests, spawn-path
//! failure merging and deadline interest through cleanup. Physical Saved,
//! rather than logical child completion, gates every rest and retirement.
//!
//! The stackful adapter drives this same task-owned record. No context,
//! native-stack output pointer or language result is retained here. Dropping
//! an abandoned task reclaims bookkeeping without executing language cleanup.

use super::*;
use crate::continuation::binding::{self, BoundSite};
use registry::ScopeKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Key {
    owner: TaskKey,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Begin,
    Enter,
    Settle,
    RestReady,
    RestWaiting(TaskKey),
    Disarm,
    Retire,
    Merge,
    Yield,
    YieldWaiting,
    Complete,
}

pub(in crate::tasks) struct Operation {
    key: Key,
    binding: Option<BoundSite>,
    scope: ScopeKey,
    phase: Phase,
    error: u8,
    cancellations: Vec<(u32, Vec<u32>)>,
    children: Vec<TaskKey>,
    /// Number of children not yet visited, traversed from the end.
    remaining: usize,
    own_path: Vec<u32>,
    retired: Vec<(Task, Option<Vec<Chained>>)>,
    retired_scope: Option<Scope>,
    expired: bool,
    status: u8,
}

pub(super) enum Step {
    Admission,
    Cut,
    Wait(Wait),
    Rest(TaskKey),
    Yield,
    Complete { status: u8 },
}

/// Reserve an invocation before popping the lexical scope. The existing
/// stackful recording admission runs before any effect of this exit.
pub(super) fn start(ctx: *mut TaskCtx, word: u64, error: u8) -> Key {
    role::with_role(ctx, |s| {
        let scope = s.scope_handle(word);
        let owner = s.key(s.current);
        let scope = s.scope_key(scope).expect("a live exit scope");
        let row = s.task(owner.slot);
        row.scope_exit_generation = row
            .scope_exit_generation
            .checked_add(1)
            .expect("scope exit invocation generation exhausted");
        let key = Key {
            owner,
            generation: row.scope_exit_generation,
        };
        row.scope_exits.push(Operation {
            key,
            binding: None,
            scope,
            phase: Phase::Begin,
            error,
            cancellations: Vec::new(),
            children: Vec::new(),
            remaining: 0,
            own_path: Vec::new(),
            retired: Vec::new(),
            retired_scope: None,
            expired: false,
            status: STATUS_OK,
        });
        key
    })
}

/// Access only during bounded role-owned work. No borrow of the record can
/// cross recording admission, a home bridge or a physical switch.
fn with<R>(s: &mut Scheduler, key: Key, f: impl FnOnce(&mut Scheduler, &mut Operation) -> R) -> R {
    assert_eq!(s.key(s.current), key.owner, "scope exit owner changed");
    let mut operation = s
        .task(key.owner.slot)
        .scope_exits
        .pop()
        .expect("an owned scope exit");
    assert_eq!(
        operation.key, key,
        "stale or non-innermost scope exit invocation"
    );
    if !matches!(
        operation.phase,
        Phase::Merge | Phase::Yield | Phase::YieldWaiting | Phase::Complete
    ) {
        assert_eq!(
            s.scope_key(operation.scope.slot),
            Some(operation.scope),
            "stale exit scope"
        );
    }
    let result = f(s, &mut operation);
    s.task(key.owner.slot).scope_exits.push(operation);
    result
}

fn inspect<R>(
    ctx: *mut TaskCtx,
    key: Key,
    f: impl FnOnce(&mut Scheduler, &mut Operation) -> R,
) -> R {
    role::with_role(ctx, |s| with(s, key, f))
}

/// Recreate one bounded action from the owned phase. A rejected admission has
/// no effects and returns with every encoder/slot loan released.
fn publish<T>(
    ctx: *mut TaskCtx,
    leave: bool,
    phase: super::super::Phase,
    action: impl FnOnce(Option<&mut hirsel_free::concurrent::Writer<'_, '_>>) -> T,
) -> Stopped<Option<T>> {
    if leave {
        super::super::record::poll_publish(ctx, phase, action)
    } else {
        super::super::record::publish(ctx, phase, action).map(Some)
    }
}

/// Drive committed phases until the adapter must wait, call one child rest,
/// yield, or consume completion. Recording and home bridges remain stackful
/// here; none of their Rust loans may escape as Pending.
/// # Safety
/// The running task owns this invocation and holds the role.
pub(super) unsafe fn drive(
    ctx: *mut TaskCtx,
    key: Key,
    out: Option<*mut u8>,
    leave: bool,
) -> Stopped<Step> {
    loop {
        match inspect(ctx, key, |_, op| op.phase) {
            Phase::Begin => {
                if leave {
                    if !super::super::record::begin_entered(ctx) {
                        return Ok(Step::Cut);
                    }
                } else {
                    super::super::record::begin(ctx);
                }
                inspect(ctx, key, |s, op| {
                    if op.error != 0 || unsafe { (*ctx).failure.active() } {
                        for child in s.scope(op.scope.slot).children.clone() {
                            op.cancellations.push((child, s.prepare_cancel(child)));
                        }
                    }
                    op.phase = Phase::Enter;
                });
            }
            Phase::Enter => {
                if publish(ctx, leave, super::super::Phase::Entered, |_| unsafe {
                    role::with_held(ctx, |s| {
                        with(s, key, |s, op| {
                            assert_eq!(s.task(key.owner.slot).scopes.pop(), Some(op.scope.slot));
                            s.cancel_epoch.changed();
                            for (child, targets) in &op.cancellations {
                                s.cancel_prepared(*child, CANCEL_PARENT, targets);
                            }
                            op.phase = Phase::Settle;
                        })
                    })
                })?
                .is_none()
                {
                    return Ok(Step::Admission);
                }
            }
            Phase::Settle => {
                let wait = inspect(ctx, key, |s, op| {
                    // Release prepared cancellation vectors outside the recorder section.
                    op.cancellations.clear();
                    let children = s.scope(op.scope.slot).children.clone();
                    if !children.iter().all(|&child| {
                        readiness::saved(s, child)
                            && matches!(s.task(child).state, State::Holding | State::Done)
                    }) {
                        return Some(Wait::Exit {
                            scope: op.scope.slot,
                        });
                    }
                    // Membership is sampled after settlement, as in the
                    // original exit, rather than at callback return.
                    op.children = children.into_iter().map(|child| s.key(child)).collect();
                    op.remaining = op.children.len();
                    op.phase = Phase::RestReady;
                    None
                });
                if let Some(wait) = wait {
                    return Ok(Step::Wait(wait));
                }
            }
            Phase::RestReady => {
                let child = inspect(ctx, key, |s, op| {
                    if op.remaining == 0 {
                        op.phase = Phase::Disarm;
                        return None;
                    }
                    let child = op.children[op.remaining - 1];
                    assert_eq!(s.key(child.slot), child, "stale child rest");
                    assert!(readiness::saved(s, child.slot));
                    if take_held(s, child.slot) {
                        op.phase = Phase::RestWaiting(child);
                        Some(child)
                    } else {
                        assert_eq!(s.task(child.slot).state, State::Done);
                        op.remaining -= 1;
                        None
                    }
                });
                if let Some(child) = child {
                    return Ok(Step::Rest(child));
                }
            }
            Phase::RestWaiting(child) => {
                inspect(ctx, key, |s, op| {
                    assert_eq!(s.key(child.slot), child, "stale returning child rest");
                    assert_eq!(s.task(child.slot).state, State::Done);
                    assert!(
                        readiness::saved(s, child.slot),
                        "rest returned before physical save"
                    );
                    op.remaining -= 1;
                    op.phase = Phase::RestReady;
                });
            }
            Phase::Disarm => {
                let token = inspect(ctx, key, |s, op| {
                    op.own_path = s.task(key.owner.slot).path.clone();
                    s.reserve_retirements(op.children.len());
                    s.scope_slots.reserve(1);
                    let mut counts: Vec<_> = s
                        .tasks
                        .iter()
                        .map(|row| row.as_ref().map_or(0, |row| row.children.len()))
                        .collect();
                    let mut capacity = counts.clone();
                    for child in op.children.iter().rev() {
                        assert_eq!(s.key(child.slot), *child);
                        let row = s.task(child.slot);
                        assert_eq!(row.state, State::Done);
                        let parent = row.parent;
                        assert!(readiness::saved(s, child.slot));
                        if let Some((parent, _)) = parent {
                            counts[parent as usize] =
                                counts[parent as usize] - 1 + counts[child.slot as usize];
                            capacity[parent as usize] =
                                capacity[parent as usize].max(counts[parent as usize]);
                        }
                    }
                    for (index, maximum) in capacity.into_iter().enumerate() {
                        if let Some(row) = s.tasks[index].as_mut() {
                            row.children.reserve(maximum - row.children.len());
                        }
                    }
                    op.retired.reserve(op.children.len());
                    s.scope(op.scope.slot)
                        .deadline
                        .as_ref()
                        .and_then(|d| d.token)
                });
                // Keep the timer and all rows owned through every child rest
                // and this home bridge. No table/record loan crosses it.
                if let Some(token) = token {
                    let _segment = unsafe { role::Segment::new(ctx) };
                    unsafe { crate::host_waits::disarm(ctx, token) };
                }
                inspect(ctx, key, |_, op| op.phase = Phase::Retire);
            }
            Phase::Retire => {
                let replay = super::super::replay::token(ctx);
                let publication = publish(ctx, leave, super::super::Phase::Done, |writer| unsafe {
                    role::with_held(ctx, |s| {
                        with(s, key, |s, op| {
                            let mut expired =
                                s.scope(op.scope.slot).deadline.as_ref().map(|d| d.expired);
                            if let Some(value) = expired.as_mut() {
                                if let Some(writer) = writer {
                                    writer.expired(*value);
                                } else if let Some(replay) = &replay {
                                    *value = replay.expired(*value)?;
                                }
                            }
                            for child in op.children.iter().rev() {
                                assert_eq!(s.key(child.slot), *child);
                                let chain = s.task(child.slot).failure.take();
                                op.retired.push((s.retire_task(child.slot), chain));
                            }
                            op.retired_scope = Some(s.take_scope(op.scope.slot));
                            op.expired = expired.unwrap_or(false);
                            if let Some(out) = out {
                                out.write(u8::from(op.expired));
                            }
                            op.phase = Phase::Merge;
                            Ok(())
                        })
                    })
                });
                drop(replay);
                match publication? {
                    Some(result) => result?,
                    None => return Ok(Step::Admission),
                }
            }
            Phase::Merge => {
                // No stop-aware accessor between taking the chains and
                // merging them. Every detached owner stays in the record
                // through the last potentially stopping publication.
                let (scope, retired, path) = inspect(ctx, key, |_, op| {
                    (
                        op.retired_scope.take(),
                        std::mem::take(&mut op.retired),
                        std::mem::take(&mut op.own_path),
                    )
                });
                drop(scope);
                for (row, chain) in retired {
                    if let Some(chain) = chain {
                        unsafe { (*ctx).failure.merge_chain(&path, chain) };
                    }
                    drop(row);
                }
                let status = if unsafe { (*ctx).failure.active() } {
                    STATUS_FAILURE
                } else {
                    STATUS_OK
                };
                inspect(ctx, key, |_, op| {
                    op.status = status;
                    op.phase = Phase::Yield;
                });
            }
            Phase::Yield => {
                inspect(ctx, key, |_, op| op.phase = Phase::YieldWaiting);
                return Ok(Step::Yield);
            }
            Phase::YieldWaiting => panic!("scope exit resumed before its yield returned"),
            Phase::Complete => {
                let status = inspect(ctx, key, |_, op| op.status);
                role::with_role(ctx, |s| {
                    let op = s
                        .task(key.owner.slot)
                        .scope_exits
                        .pop()
                        .expect("completed scope exit");
                    assert_eq!(op.key, key);
                });
                return Ok(Step::Complete { status });
            }
        }
    }
}

pub(super) fn yielded(ctx: *mut TaskCtx, key: Key) {
    inspect(ctx, key, |_, op| {
        assert_eq!(op.phase, Phase::YieldWaiting);
        op.phase = Phase::Complete;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Observations = Arc<Mutex<Vec<(u64, u64)>>>;

    unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        let (id, observations) = unsafe {
            crate::alloc::payload(env)
                .cast::<(u64, Observations)>()
                .read()
        };
        let mut image = [1u64, id];
        let none = [0u64; 2];
        assert_eq!(
            unsafe {
                hirsel_rt_v2_task_finish(
                    ctx,
                    image.as_mut_ptr().cast(),
                    none.as_ptr().cast(),
                    16,
                    u32::MAX,
                )
            },
            STATUS_OK
        );
        assert_eq!(
            image,
            [1, id],
            "discard keeps Some owned by its child's rest"
        );
        role::with_role(ctx, |s| {
            let current = s.current;
            let parent = s.task(current).parent.unwrap().0;
            let child = s.key(s.current);
            let op = s
                .task(parent)
                .scope_exits
                .last()
                .expect("task-owned exit during rest");
            assert_eq!(op.phase, Phase::RestWaiting(child));
            assert_eq!(op.remaining, id as usize + 1);
            assert_eq!(op.children[op.remaining - 1], child);
            let (scope, generation) = (op.scope, op.key.generation);
            assert_eq!(s.scope_key(scope.slot), Some(scope));
            assert!(
                !s.task(parent).scopes.contains(&scope.slot),
                "lexical pop retains exit ownership"
            );
            observations.lock().unwrap().push((generation, id));
        });
        // Give the dispatcher multiple save opportunities while the parent
        // retains its exact RestWaiting cursor.
        for _ in 0..3 {
            let mut canceled = 99;
            unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) };
            assert_eq!(canceled, 0, "normal exit joins without cancellation");
        }
        STATUS_OK
    }

    #[test]
    fn an_owned_exit_keeps_one_reverse_cursor_across_rests_and_scope_slot_reuse() {
        for width in [1, 4] {
            for durable in [false, true] {
                let recording =
                    super::super::super::record::Recording::new(b"owned exit cursor", durable);
                let install = recording.clone();
                let layout = crate::Layout {
                    magic: crate::LAYOUT_MAGIC,
                    version: crate::LAYOUT_VERSION,
                    size: std::mem::size_of::<(u64, Observations)>() as u64,
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
                let layouts = [&raw const layout];
                let observations = Arc::new(Mutex::new(Vec::new()));
                let observed = observations.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        for generation in 1..=2 {
                            let mut scope = 0;
                            hirsel_rt_v2_task_scope_enter(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0,
                            );
                            for id in 0..3 {
                                let env = crate::alloc::hirsel_rt_v2_alloc(
                                    ctx,
                                    2,
                                    layout.size,
                                    8,
                                    crate::alloc::DOMAIN_HEAP,
                                    0,
                                );
                                crate::alloc::payload(env)
                                    .cast::<(u64, Observations)>()
                                    .write((id, observations.clone()));
                                let mut task = 0;
                                hirsel_rt_v2_task_spawn(
                                    ctx,
                                    0,
                                    0,
                                    std::ptr::null_mut(),
                                    &mut task,
                                    &scope,
                                    child,
                                    env,
                                    0,
                                );
                            }
                            let held_result = vec![41u64, 42];
                            assert_eq!(
                                hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                                STATUS_OK
                            );
                            assert_eq!(held_result, [41, 42]);
                            role::with_role(ctx, |s| {
                                assert!(s.task(ROOT).scope_exits.is_empty());
                                assert_eq!(s.task(ROOT).scope_exit_generation, generation);
                            });
                        }
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                assert_eq!(
                    *observed.lock().unwrap(),
                    [(1, 2), (1, 1), (1, 0), (2, 2), (2, 1), (2, 0)]
                );
                let cut = recording
                    .recorder
                    .stop("returned", None)
                    .unwrap()
                    .unwrap()
                    .0;
                assert_eq!(
                    cut.progress[&Vec::new()],
                    hirsel_free::phase::Progress::new(10, super::super::super::Phase::Done)
                );
                assert_eq!(cut.progress.len(), 7);
            }
        }
    }
}

#[cfg(test)]
mod failure_tests {
    //! Port Trio test_run.py:132-170 (0480602b): parent error, child crash,
    //! and two three-step loopers. Check each child's order; Hirsel does not
    //! import Trio's round-robin schedule or exception-group presentation.
    use super::*;
    use std::sync::{Arc, Mutex};

    type Events = Arc<Mutex<Vec<(u64, u64)>>>;

    fn failure(site: u64) -> crate::failure::FailureRecord {
        crate::failure::FailureRecord {
            kind: 1,
            rule: 1,
            site,
            operation_site: site,
        }
    }

    unsafe extern "C" fn looper(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        let (id, mode, events) = unsafe {
            crate::alloc::payload(env)
                .cast::<(u64, u64, Events)>()
                .read()
        };
        for step in 0..3 {
            events.lock().unwrap().push((id, step));
            let mut canceled = 0;
            unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) };
        }
        let mut image = [1u64, id];
        let none = [0u64; 2];
        unsafe {
            hirsel_rt_v2_task_finish(
                ctx,
                image.as_mut_ptr().cast(),
                none.as_ptr().cast(),
                16,
                u32::MAX,
            )
        };
        assert_eq!(image, [1, id]);
        role::with_role(ctx, |s| {
            let current = s.current;
            let parent = s.task(current).parent.unwrap().0;
            let child = s.key(current);
            let op = s.task(parent).scope_exits.last().unwrap();
            assert_eq!(op.phase, Phase::RestWaiting(child));
        });
        if mode == 3 {
            drop(events);
            unsafe { crate::stop::raise(ctx, crate::stop::Stop::Requested) };
        }
        if mode == 2 {
            unsafe { (*ctx).failure.record(failure(10 + id)) };
            STATUS_FAILURE
        } else {
            STATUS_OK
        }
    }

    #[test]
    fn parent_and_child_failures_keep_their_domains_and_spawn_paths() {
        // Parent-only, child-only, combined, owned Error, and interleaving.
        for (parent_fails, children, mode, error) in [
            (true, 0, 0, 0),
            (false, 1, 2, 0),
            (false, 2, 2, 0),
            (true, 2, 2, 0),
            (false, 2, 0, 1),
            (false, 2, 0, 0),
        ] {
            scenario(parent_fails, children, mode, error);
        }
    }

    #[test]
    fn stop_during_a_child_rest_abandons_the_owned_exit_without_its_suffix() {
        scenario(false, 1, 3, 0);
    }

    fn scenario(parent_fails: bool, children: u64, mode: u64, error: u8) {
        for width in [1, 4] {
            let recording =
                super::super::super::record::Recording::new(b"Trio exit vectors", false);
            let install = recording.clone();
            let layout = crate::Layout {
                magic: crate::LAYOUT_MAGIC,
                version: crate::LAYOUT_VERSION,
                size: std::mem::size_of::<(u64, u64, Events)>() as u64,
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
            let layouts = [&raw const layout];
            let events = Arc::new(Mutex::new(Vec::new()));
            let observed = events.clone();
            let suffix = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let observed_suffix = suffix.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                    for id in 0..children {
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            layout.size,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env)
                            .cast::<(u64, u64, Events)>()
                            .write((id, mode, events.clone()));
                        let mut task = 0;
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut task,
                            &scope,
                            looper,
                            env,
                            0,
                        );
                    }
                    let held = vec![41u64, 42];
                    if parent_fails {
                        (*ctx).failure.record(failure(1));
                    }
                    let expected_failure = parent_fails || mode == 2;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, error),
                        u8::from(expected_failure)
                    );
                    assert_eq!(held, [41, 42]);
                    suffix.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let chain = (*ctx).failure.take_chain(&[]);
                    let mut expected = Vec::new();
                    if parent_fails {
                        expected.push((failure(1), vec![]));
                    }
                    if mode == 2 {
                        // Port the baseline merge_chain transcript exactly:
                        // an own primary stays first and receives reverse-rest
                        // causes; without it the earlier child path leads.
                        let order: Vec<_> = if parent_fails {
                            (0..children).rev().collect()
                        } else {
                            (0..children).collect()
                        };
                        for id in order {
                            expected.push((failure(10 + id), vec![id as u32]));
                        }
                    }
                    assert_eq!(chain, expected);
                    STATUS_OK
                },
            );
            if mode == 3 {
                assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
                assert_eq!(
                    observed_suffix.load(std::sync::atomic::Ordering::Relaxed),
                    0
                );
            } else {
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(
                    observed_suffix.load(std::sync::atomic::Ordering::Relaxed),
                    1
                );
            }
            assert_eq!(outcome.pending, 0);
            for id in 0..children {
                assert_eq!(
                    observed
                        .lock()
                        .unwrap()
                        .iter()
                        .filter_map(|&(who, step)| (who == id).then_some(step))
                        .collect::<Vec<_>>(),
                    [0, 1, 2]
                );
            }
            let cut = recording
                .recorder
                .stop(if mode == 3 { "stopped" } else { "returned" }, None)
                .unwrap()
                .unwrap()
                .0;
            assert_eq!(
                cut.progress[&Vec::new()],
                hirsel_free::phase::Progress::new(
                    2 + children,
                    if mode == 3 {
                        super::super::super::Phase::Entered
                    } else {
                        super::super::super::Phase::Done
                    }
                )
            );
        }
    }
}

/// Drive the same installed exit after a certified runtime rebind. The record
/// survives lexical pop and context relocation; resolving the old scope word
/// again after retirement is forbidden.
/// # Safety
/// The native caller consumed its typed BoundSite and retains exact roots and
/// ABI outputs in the compact frame for the whole entered invocation.
pub(super) unsafe fn poll(
    ctx: *mut TaskCtx,
    word: u64,
    error: u8,
    caller_site: u64,
    out: Option<*mut u8>,
    site: BoundSite,
) -> Stopped<u8> {
    let existing = role::with_role(ctx, |s| {
        let me = s.current;
        s.task(me)
            .scope_exits
            .last()
            .and_then(|op| (op.binding == Some(site)).then_some(op.key))
    });
    let key = match existing {
        Some(key) => key,
        None => {
            let key = start(ctx, word, error);
            inspect(ctx, key, |_, op| op.binding = Some(site));
            key
        }
    };
    // The only producer of this flag is this invocation's previous Pending.
    let _ = compact::waited(ctx);
    loop {
        let leave = unsafe { binding::can_leave(ctx, site) }
            && out.is_none_or(|out| compact::contains_frame(ctx, out as usize, 1));
        let step = match unsafe { drive(ctx, key, out, leave) } {
            Err(stop) if leave && super::super::replay::cut_rejection(ctx, &stop) => Step::Cut,
            result => result?,
        };
        match step {
            Step::Admission => return Ok(crate::STATUS_SUSPENDED),
            Step::Cut => {
                unsafe { park_left_at(ctx, Wait::Final, caller_site) }?;
                compact::wait(ctx);
                return Ok(crate::STATUS_SUSPENDED);
            }
            Step::Wait(wait) if leave => {
                #[cfg(feature = "diagnostic")]
                crate::compact_trace::runtime_wait(
                    site.task.slot,
                    site.state,
                    site.kind,
                    "settle",
                    site.invocation,
                );
                unsafe { park_left_at(ctx, wait, caller_site) }?;
                compact::wait(ctx);
                return Ok(crate::STATUS_SUSPENDED);
            }
            Step::Wait(wait) => unsafe { try_block(&mut *ctx, wait, false, caller_site) }?,
            Step::Rest(child) if leave => {
                #[cfg(feature = "diagnostic")]
                crate::compact_trace::runtime_wait(
                    site.task.slot,
                    site.state,
                    site.kind,
                    "rest",
                    site.invocation,
                );
                role::with_role(ctx, |s| {
                    assert_eq!(s.key(child.slot), child);
                    assert!(readiness::saved(s, child.slot));
                    let me = s.current;
                    s.task(me).state = State::Parked;
                    let row = s.task(child.slot);
                    assert_eq!(row.state, State::Holding);
                    assert!(row.handoff.replace(me).is_none());
                    row.state = State::Ready;
                    s.runnable.push_back(child.slot);
                });
                compact::wait(ctx);
                unsafe { super::super::dispatch_ready(ctx) };
                return Ok(crate::STATUS_SUSPENDED);
            }
            Step::Rest(child) => unsafe { hand_off(&mut *ctx, child.slot) },
            Step::Yield => {
                let pending = unsafe { super::super::try_yield_entered(&mut *ctx, leave) }?;
                yielded(ctx, key);
                if pending {
                    return Ok(crate::STATUS_SUSPENDED);
                }
            }
            Step::Complete { status } => {
                unsafe { binding::complete_bound(ctx, site) };
                return Ok(status);
            }
        }
    }
}

#[cfg(test)]
mod compact_tests;
