//! Native task effects over the saved-frame dispatcher.
//!
//! Follows the cooperative task tree and Frontier's checked call transitions.
//! Task mutations retire after their preceding stretches. Scope/spawn commands
//! own unpublished rows while their issuing task runs ahead. A
//! holding frame is read only after idle save; completion retires its physical
//! token before publishing Done or returning to a caller. This test-only slice
//! does not yet admit deadlines or host effects.

use super::super::*;
use super::engine;
use crate::ordered::objects::Kind;
use crate::ordered::{Next, Progress};

/// An unpublished native frame. Owning the reserved row removes it from the
/// free list, but neither the task tree nor the logical queue can see it yet.
pub(crate) struct Spawn {
    #[cfg(feature = "ownership-trace")]
    ownership: Option<crate::ownership_trace::SpawnBinding>,
    index: u32,
    owner: u64,
    scope: u64,
    shield: u32,
    task: Option<Task>,
    held: crate::ordered::footprint::Held,
    started: Option<std::time::Instant>,
    run: *mut crate::context::RunShared,
}

impl Spawn {
    pub(super) fn ready(&self, s: &mut Scheduler) -> bool {
        self.task.is_some() || super::ahead::can_promote(s, self.index)
    }

    pub(super) fn install(
        mut self,
        s: &mut Scheduler,
    ) -> (u64, crate::ordered::footprint::Held, Option<u32>) {
        let mut task = self
            .task
            .take()
            .unwrap_or_else(|| super::ahead::take(s, self.index));
        let parent = self.owner as u32;
        assert_eq!(s.task(parent).generation, (self.owner >> 32) as u32);
        let scope = s.scope_handle(self.scope);
        assert_eq!(task.scope, Some(scope));
        let ordinal = task.path.last().copied().expect("a child path");
        assert_eq!(
            s.task(parent).spawned,
            ordinal,
            "retire spawns in owner order"
        );
        s.task(parent).spawned = ordinal.checked_add(1).expect("spawn path ordinal overflow");
        let inherited = s.canceled_task(parent, self.shield);
        task.canceled = inherited.map(|_| CANCEL_PARENT).into();
        task.priority = s.schedule.admit();
        let word = handle(self.index, task.generation);
        assert!(
            s.tasks[self.index as usize].is_none(),
            "a reserved task row"
        );
        #[cfg(feature = "ownership-trace")]
        unsafe {
            crate::ownership_trace::task(&*self.run, word, &task.path);
            crate::ownership_trace::spawned(&*self.run, self.ownership, word);
        };
        s.tasks[self.index as usize] = Some(task);
        s.scope(scope).children.push(self.index);
        s.link_child(parent, self.index);
        if let (Some(started), Some(metrics)) =
            (self.started, unsafe { &*self.run }.worker_metrics.as_ref())
        {
            metrics.spawn(started.elapsed());
        }
        (word, self.held, inherited)
    }
}

/// # Safety
/// As for TaskScopeEnter, with the ordered policy and role installed.
pub(crate) unsafe fn scope_enter(ctx: *mut TaskCtx, out: *mut u64) -> Stopped<u8> {
    let owner = engine::current(ctx);
    let (index, generation) = role::with_role(ctx, |s| s.take_scope_slot());
    let word = handle(index, generation);
    engine::with_driver(ctx, |d| d.objects.inert(Kind::Scope, word));
    engine::submit(
        ctx,
        super::Command::ScopeEnter {
            index,
            generation,
            owner,
        }
        .operation(crate::ordered::footprint::Footprint {
            writes: std::collections::BTreeSet::from([crate::ordered::footprint::Item::Task(
                owner,
            )]),
            ..Default::default()
        }),
    )?;
    unsafe { out.write(word) };
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskScopeEnterUntil; the timer adapter is bound on the home thread.
pub(crate) unsafe fn scope_enter_until(
    ctx: *mut TaskCtx,
    out: *mut u64,
    deadline: *const [u64; 2],
) -> Stopped<u8> {
    let [clock, at] = unsafe { deadline.read() };
    let ((index, generation), progress) = engine::head_canonical(
        ctx,
        "task.scope_enter",
        || {
            (
                open_scope(
                    ctx,
                    Some(ScopeDeadline {
                        token: None,
                        expired: false,
                    }),
                ),
                Next::Keep,
            )
        },
        |&(index, generation)| {
            super::events::deadline_scope(ctx, handle(index, generation), clock, at)
        },
    )?;
    assert!(matches!(progress, Progress::Absorbed(_)));
    // No Frontier/Scheduler borrow spans this home bridge. The row remains
    // owned by this head and cannot be observed by a retirement poll yet.
    let token = unsafe { crate::host_waits::arm(ctx, index, generation, clock, at) }?;
    if let Some(token) = token {
        role::with_role(ctx, |s| {
            s.scope(index).deadline.as_mut().unwrap().token = Some(token);
        });
    }
    super::events::deadline_armed(ctx, index, token.is_some());
    if token.is_none() {
        role::with_role(ctx, |s| super::events::expire(s, index));
    }
    let word = handle(index, generation);
    engine::with_driver(ctx, |d| d.objects.inert(Kind::Scope, word));
    let (_, progress) = engine::head_continuation(ctx, || ((), Next::Yield))?;
    engine::continue_at_head(ctx, progress)?;
    unsafe { out.write(word) };
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskSpawn. The owned environment remains unstarted during its scan.
pub(crate) unsafe fn spawn(
    ctx: *mut TaskCtx,
    out: *mut u64,
    scope: *const u64,
    code: TaskCode,
    environment: *mut Header,
    operation_site: u64,
) -> Stopped<u8> {
    unsafe { spawn_with_entry(ctx, out, scope, code, environment, operation_site, None) }
}

pub(crate) unsafe fn spawn_with_entry(
    ctx: *mut TaskCtx,
    out: *mut u64,
    scope: *const u64,
    code: TaskCode,
    environment: *mut Header,
    operation_site: u64,
    ahead_entry: Option<TaskCode>,
) -> Stopped<u8> {
    unsafe {
        spawn_certified(
            ctx,
            out,
            scope,
            code,
            environment,
            operation_site,
            ahead_entry,
            None,
        )
    }
}

pub(crate) unsafe fn spawn_certified(
    ctx: *mut TaskCtx,
    out: *mut u64,
    scope: *const u64,
    code: TaskCode,
    environment: *mut Header,
    operation_site: u64,
    ahead_entry: Option<TaskCode>,
    certificate: Option<super::ahead::native::Certificate>,
) -> Stopped<u8> {
    let parent = engine::current(ctx);
    let scope = unsafe { scope.read() };
    let ordinal = {
        let committed = role::with_role(ctx, |s| s.task(parent as u32).spawned);
        engine::with_driver(ctx, |d| {
            let next = d.spawn_ordinals.entry(parent).or_insert(committed);
            let ordinal = *next;
            *next = next.checked_add(1).expect("spawn path ordinal overflow");
            ordinal
        })
    };
    let (index, generation, path) = role::with_role(ctx, |s| {
        let mut path = s.task(parent as u32).path.clone();
        path.push(ordinal);
        let (index, generation) = s.take_task_slot();
        (index, generation, path)
    });
    let stack = unsafe { (*ctx).worker() }.idle_stacks.take();
    let child = stack.context();
    unsafe {
        child.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
        (*child).task_id = index;
        if (*ctx).run_ref().ordered_roots.get().is_some() {
            (*child).roots.trace_creator.path = path.clone();
            (*child).ordered_charge_trace.enable();
        }
        (*child).task_sp = stack.prepare(
            task_entry,
            child.cast(),
            handle(index, generation) as usize as *mut u8,
        );
    }
    let wide = crate::pool::cpu_scoped::width(ctx).unwrap_or_else(|| unsafe {
        (*ctx)
            .run_ref()
            .control
            .width
            .load(std::sync::atomic::Ordering::Acquire)
    }) > 1;
    let alternate = ahead_entry.filter(|_| wide && engine::with_driver(ctx, |d| d.private_enabled));
    let mut task = Task::new(generation, child, Some(scope as u32), path, None);
    task.code = Some((alternate.unwrap_or(code), environment, operation_site));
    if alternate.is_some() {
        unsafe { (*child).ahead = Some(Box::new(super::ahead::Local::new(true))) };
    }
    task.stack = Some(stack);
    inherit_cpu(ctx, &mut task);
    // SAFETY: no notification has published this owned environment yet.
    let captures = unsafe {
        crate::ordered::capture::scan(
            ctx,
            crate::alloc::payload(environment),
            (*environment).index(),
        )
    };
    let word = handle(index, generation);
    let (held, plain) = engine::with_driver(ctx, |driver| {
        let held = driver.objects.captured(&captures);
        // Task result types are erased by TaskSpawn's current native ABI. An
        // unknown binding conservatively blocks passes through a captured task.
        driver.objects.unknown(Kind::Task, word);
        driver.objects.obtained(
            driver
                .frontier
                .as_mut()
                .unwrap()
                .held
                .get_mut(&parent)
                .unwrap(),
            Kind::Task,
            word,
        );
        let plain = driver
            .plain_layouts
            .contains(&unsafe { (*environment).index() });
        (held, plain)
    });
    let host = certificate
        .as_ref()
        .is_some_and(|c| !c.host.is_null() || c.cpu);
    let reserve = !host && alternate.is_some() && {
        let safe_capture = !captures.roots
            && !captures.unknown
            && captures.handles.is_empty()
            && captures.typed.is_empty()
            && plain;
        let scope_committed = role::with_role(ctx, |s| {
            s.scopes
                .get(scope as u32 as usize)
                .and_then(Option::as_ref)
                .is_some_and(|row| row.generation == (scope >> 32) as u32)
        });
        let safe_capture = safe_capture
            || (certificate.is_some()
                && !captures.roots
                && !captures.unknown
                && captures.handles.is_empty()
                && captures.typed.is_empty());
        safe_capture
            && scope_committed
            && engine::with_driver(ctx, |d| d.reserved.len()) < super::ahead::MAX_RESERVATIONS
    };
    if let Some(certificate) = certificate {
        if (reserve || (host && alternate.is_some()))
            && task
                .stack
                .as_ref()
                .unwrap()
                .supports_ahead(certificate.stack_bytes)
        {
            let mut native =
                super::ahead::native::Native::new(child, environment, operation_site, certificate);
            unsafe {
                (*child).ahead_physical = &raw mut native.physical;
                (*child).task_sp = task.stack.as_ref().unwrap().prepare(
                    super::ahead::native::start,
                    child.cast(),
                    (&raw mut *native).cast(),
                );
                (*child).ahead.as_mut().unwrap().native = Some(native);
            }
        } else {
            task.code = Some((code, environment, operation_site));
            unsafe {
                (*child).ahead = None;
            }
        }
    }
    let reserve = reserve && unsafe { (*child).ahead.is_some() };
    let task = if reserve {
        unsafe { (*child).ahead.as_ref().unwrap().reserve(generation) };
        let task = super::ahead::Reserved {
            task: Box::new(task),
        };
        engine::with_driver(ctx, |d| d.reserved.insert(index, task));
        None
    } else {
        Some(task)
    };
    let spawn = Spawn {
        #[cfg(feature = "ownership-trace")]
        ownership: unsafe { crate::ownership_trace::take_spawn(ctx) },
        index,
        owner: parent,
        scope,
        shield: unsafe { (*ctx).shield },
        task,
        held,
        started: unsafe { (*ctx).run_ref() }
            .worker_metrics
            .as_ref()
            .map(|_| std::time::Instant::now()),
        run: unsafe { (*ctx).run_ptr() },
    };
    engine::submit(
        ctx,
        super::Command::TaskSpawn {
            reservation: std::cell::RefCell::new(Some(spawn)),
        }
        .operation(crate::ordered::footprint::Footprint {
            writes: std::collections::BTreeSet::from([crate::ordered::footprint::Item::Task(
                parent,
            )]),
            ..Default::default()
        }),
    )?;
    // A committed spawn has already moved out of the private owner map.
    // Only an owned unpublished frame can be advertised by this path.
    if engine::with_driver(ctx, |d| d.reserved.contains_key(&index)) {
        let owner = role::with_role(ctx, |s| {
            super::ahead::physical(s, TaskKey::new(index, generation))
                .expect("owned private reservation")
                .owner
        });
        unsafe {
            crate::pool::free::schedule_saved(
                (*ctx).run_ptr(),
                TaskKey::new(index, generation),
                owner,
            )
        };
    }
    unsafe { out.write(word) };
    Ok(STATUS_OK)
}

/// # Safety
/// As for TaskFinish. The Slot belongs to the current task until idle save.
pub(crate) unsafe fn finish(
    ctx: *mut TaskCtx,
    slot: *mut u8,
    none: *const u8,
    size: u64,
    descriptor: u32,
) -> Stopped<u8> {
    if crate::tasks::compact::held(ctx) {
        // The reissued end of a result held from a saved compact frame: its
        // parent called its rest, as `save_boundary` returns below.
        engine::at_head(ctx)?;
        return Ok(STATUS_OK);
    }
    engine::at_head(ctx)?;
    let me = role::with_role(ctx, |s| s.current);
    unsafe { super::events::task_result(ctx, descriptor, slot) };
    if role::with_role(ctx, |s| s.task(me).detached) {
        // A detached body retains its result for its own cleanup. Its actual
        // end commits after that cleanup, in complete().
        return Ok(STATUS_OK);
    }
    let (parent, progress) = engine::head_effect(ctx, "task.end", || {
        // SAFETY: caller contract.
        let (held, image) = unsafe { Slot::held(slot, none, size) };
        let parent = role::with_role(ctx, |s| {
            let scope = s.task(me).scope.expect("a child has a scope");
            let parent = s.scope(scope).owner;
            let parent = handle(parent, s.task(parent).generation);
            let row = s.task(me);
            row.ended = true;
            row.slot = Some(held);
            row.held_none = Some(image);
            row.state = State::Holding;
            s.order(me);
            parent
        });
        // A waiter's poll cannot inspect this slot until after physical save.
        (parent, Next::Hold)
    })?;
    let task = engine::current(ctx);
    engine::with_driver(ctx, |d| {
        d.frontier
            .as_mut()
            .unwrap()
            .held
            .get_mut(&parent)
            .expect("a live scope owner")
            .holding
            .insert(task)
    });
    if unsafe { crate::tasks::compact::can_hold(ctx, slot, size) } {
        // The result stays in the compact frame; the frame leaves its stack.
        engine::defer_boundary_poll(ctx, progress);
        crate::tasks::compact::hold(ctx);
        return Ok(crate::STATUS_SUSPENDED);
    }
    engine::save_boundary(ctx, progress)?;
    Ok(STATUS_OK)
}

#[derive(Clone, Copy)]
pub(super) enum ResultAction {
    Keep,
    Deliver(*mut u8, u64),
    Discard,
}

#[derive(Clone, Copy)]
pub(super) struct Call {
    /// None resumes the same op after its waiting phase or an earlier call.
    pub(super) begin: Option<&'static str>,
    pub(super) then_yield: bool,
    pub(super) result: ResultAction,
    pub(super) detached: bool,
}

/// Apply the native half of Frontier's call insertion without accessing a
/// running context. Strict owner callbacks already prepared their result;
/// logged detach calls discard it only when this saved rest is selected.
pub(super) fn call_saved(s: &mut Scheduler, caller: u64, child: u64) {
    let parent = caller as u32;
    let index = child as u32;
    assert_eq!(s.task(parent).generation, (caller >> 32) as u32);
    assert_eq!(s.task(index).generation, (child >> 32) as u32);
    assert!(readiness::saved(s, index));
    if s.task(index).state == State::Ready {
        assert_eq!(
            s.task(index).handoff,
            Some(parent),
            "an owner-prepared call"
        );
        return;
    }
    assert_eq!(s.task(index).state, State::Holding);
    assert!(
        s.task(index).detached,
        "only detached calls have no owner callback"
    );
    s.task(index).slot = None;
    s.task(index).state = State::Ready;
    s.task(index).handoff = Some(parent);
    s.task(parent).state = State::Parked;
}

/// A bulk detach's members are processed one at a time, after any earlier
/// member's rest. Running/done members need the same ordered detach effect.
pub(super) fn detach_member(s: &mut Scheduler, caller: u64, child: u64) {
    let Logical::Ordered(driver) = &s.logical else {
        unreachable!()
    };
    if !driver.detaching.contains_key(&caller) {
        return;
    }
    let Some(index) = s.member(child) else {
        super::events::bulk_detach_member(s, caller, child, false);
        return;
    };
    let row = s.task(index);
    row.detached = true;
    let retire = row.state == State::Done && row.failure.is_none();
    super::events::bulk_detach_member(s, caller, child, true);
    if retire {
        assert!(readiness::saved(s, index));
        let scope = s.task(index).scope.expect("a detached child scope");
        s.scope(scope).children.retain(|&c| c != index);
        s.vacate_task(index);
    }
}

/// Execute a holding child's rest as a call, never as a FIFO enqueue.
/// # Safety
/// The child is holding a physically saved frame; the caller owns the role.
pub(super) unsafe fn call_start(ctx: *mut TaskCtx, child: u32, call: Call) -> Stopped<Progress> {
    let effect = || {
        let id = role::with_role(ctx, |s| {
            let me = s.current;
            assert!(readiness::saved(s, child));
            assert_eq!(s.task(child).state, State::Holding);
            match call.result {
                ResultAction::Keep => {}
                ResultAction::Deliver(slot, size) => {
                    let held = s.task(child).slot.take().expect("a holding result");
                    // SAFETY: the child is saved and this callback owns the caller.
                    unsafe { deliver(held, slot, size) };
                }
                ResultAction::Discard => s.task(child).slot = None,
            }
            s.task(me).state = State::Parked;
            let row = s.task(child);
            row.detached |= call.detached;
            row.state = State::Ready;
            row.handoff = Some(me);
            handle(child, row.generation)
        });
        (
            (),
            Next::Call {
                members: vec![id],
                then_yield: call.then_yield,
            },
        )
    };
    let (_, progress) = if let Some(name) = call.begin {
        engine::head_effect(ctx, name, effect)?
    } else {
        engine::head_continuation(ctx, effect)?
    };
    assert!(matches!(progress, Progress::Called(_)));
    Ok(progress)
}

/// # Safety
/// The child's result and calling activation remain live through its rest.
pub(super) unsafe fn call(ctx: *mut TaskCtx, child: u32, call: Call) -> Stopped<()> {
    let progress = unsafe { call_start(ctx, child, call) }?;
    engine::save_boundary(ctx, progress)
}

/// # Safety
/// As for TaskJoin. Only this task's slot or an idle-saved child's slot is used.
pub(crate) unsafe fn join(
    ctx: *mut TaskCtx,
    scope: *const u64,
    task: *mut u64,
    slot: *mut u8,
    size: u64,
    caller_site: u64,
) -> Stopped<u8> {
    super::inop::start(
        ctx,
        super::inop::Operation::Join(super::inop::Join::new(
            unsafe { scope.read() },
            unsafe { task.read() },
            super::inop::StackfulDestination {
                address: slot,
                size,
            },
            caller_site,
        )),
    );
    super::inop::run(ctx, || unsafe { super::inop::drive_join(ctx) })
}

/// # Safety
/// As for TaskScopeExit.
pub(crate) unsafe fn scope_exit(
    ctx: *mut TaskCtx,
    scope: *const u64,
    error: u8,
    caller_site: u64,
) -> Stopped<u8> {
    unsafe { exit_scope(ctx, scope, error, caller_site) }.map(|(status, _)| status)
}

/// # Safety
/// As for TaskScopeExitUntil; the owner's output is never written remotely.
pub(crate) unsafe fn scope_exit_until(
    ctx: *mut TaskCtx,
    scope: *const u64,
    error: u8,
    caller_site: u64,
    out: *mut u8,
) -> Stopped<u8> {
    let (status, expired) = unsafe { exit_scope(ctx, scope, error, caller_site) }?;
    unsafe { out.write(u8::from(expired)) };
    Ok(status)
}

unsafe fn exit_scope(
    ctx: *mut TaskCtx,
    scope: *const u64,
    error: u8,
    caller_site: u64,
) -> Stopped<(u8, bool)> {
    engine::at_head(ctx)?;
    super::events::prepare_scope_exit(ctx, unsafe { scope.read() });
    let ((scope, waited), progress) = engine::head_effect(ctx, "task.scope_exit", || {
        let (scope, waited) = role::with_role(ctx, |s| {
            let scope = s.scope_handle(unsafe { scope.read() });
            let me = s.current;
            assert_eq!(s.task(me).scopes.pop(), Some(scope));
            s.cancel_epoch.changed();
            if error != 0 || unsafe { (*ctx).failure.active() } {
                for child in s.scope(scope).children.clone() {
                    super::events::scope_cancel(s, me, child);
                }
            }
            let waited = !s.settled(scope);
            if waited {
                s.task(me).wait_site = caller_site;
                s.park(Wait::Exit { scope });
            }
            (scope, waited)
        });
        (
            (scope, waited),
            if waited { Next::Park } else { Next::Keep },
        )
    })?;
    if waited {
        engine::save_boundary(ctx, progress)?;
    }
    let children = role::with_role(ctx, |s| s.scope(scope).children.clone());
    for &child in children.iter().rev() {
        if role::with_role(ctx, |s| s.task(child).state) == State::Holding {
            unsafe {
                call(
                    ctx,
                    child,
                    Call {
                        begin: None,
                        then_yield: false,
                        result: ResultAction::Discard,
                        detached: false,
                    },
                )
            }?;
        }
    }
    let (deadline, _) = engine::head_continuation(ctx, || {
        let deadline = role::with_role(ctx, |s| {
            let me = s.current;
            let own = s.task(me).path.clone();
            for child in children {
                assert_eq!(s.task(child).state, State::Done);
                assert!(readiness::saved(s, child));
                if let Some(chain) = s.task(child).failure.take() {
                    unsafe { (*ctx).failure.merge_chain(&own, chain) };
                }
                s.vacate_task(child);
            }
            s.take_scope(scope).deadline
        });
        (deadline, Next::Keep)
    })?;
    if let Some(ScopeDeadline {
        token: Some(token), ..
    }) = &deadline
    {
        unsafe { crate::host_waits::disarm(ctx, *token) };
    }
    let expired = deadline.is_some_and(|d| d.expired);
    let status = if unsafe { (*ctx).failure.active() } {
        STATUS_FAILURE
    } else {
        STATUS_OK
    };
    super::events::complete_scope_exit(ctx, status, expired);
    Ok((status, expired))
}

/// # Safety
/// As for TaskDetach. A successful detached completion retires its row at idle.
pub(crate) unsafe fn detach(ctx: *mut TaskCtx, scope: *const u64, task: *mut u64) -> Stopped<u8> {
    let scope = unsafe { scope.read() };
    let child = unsafe { task.read() };
    engine::submit(
        ctx,
        super::Command::TaskDetach { scope, child }.operation(
            crate::ordered::footprint::Footprint {
                writes: std::collections::BTreeSet::from([crate::ordered::footprint::Item::Task(
                    child,
                )]),
                ..Default::default()
            },
        ),
    )?;
    Ok(STATUS_OK)
}

/// # Safety
/// As for ChannelReceive. Its output image remains owner-local after resume.
pub(crate) unsafe fn receive(
    ctx: *mut TaskCtx,
    receiver: *mut u64,
    message: *mut u8,
    out: *mut u32,
    caller_site: u64,
) -> Stopped<u8> {
    engine::at_head(ctx)?;
    let word = unsafe { receiver.read() };
    let descriptor = engine::carrier_descriptor(ctx, Kind::Receiver, word);
    unsafe { super::events::prepare_receive(ctx, word, descriptor) };
    let me = role::with_role(ctx, |s| s.current);
    let (status, progress) = engine::head_observed(
        ctx,
        "channel.receive",
        || {
            let shield = unsafe { (*ctx).shield };
            let status = role::with_role(ctx, |s| {
                let channel = s.channels.channel(word, false);
                // SAFETY: this head callback owns the receiving task and its output.
                unsafe { receive_ready(s, me, shield, channel, message) }
            });
            (status, Next::Yield)
        },
        |status| {
            unsafe { super::events::receive_immediate(ctx, *status, message) };
            Vec::new()
        },
    )?;
    let status = if let Some(status) = status {
        engine::continue_at_head(ctx, progress)?;
        status
    } else {
        engine::continue_at_head(ctx, progress)?;
        let (_, progress) = engine::head_continuation(ctx, || {
            role::with_role(ctx, |s| {
                let channel = s.channels.channel(word, false);
                s.task(me).wait_site = caller_site;
                s.park(Wait::Receive {
                    channel,
                    slot: message,
                });
            });
            ((), Next::Park)
        })?;
        engine::save_boundary(ctx, progress)?;
        role::with_role(ctx, |s| s.task(me).status)
    };
    if status == crate::channels::DONE {
        // The receive ends its turn before obtained handles may be used. Its
        // next segment refreshes the held set before any following E boundary.
        unsafe { engine::obtained_image(ctx, message, descriptor) };
    }
    // Metadata enumerates initialized fields; no whole-carrier byte slice is
    // formed. CLOSED/CANCELED may leave the output entirely unwritten.
    unsafe { super::events::complete_receive(ctx, status, descriptor, message) };
    unsafe { out.write(status) };
    Ok(STATUS_OK)
}

/// # Safety
/// The child owns its last runtime frame and has dropped its environment.
pub(crate) unsafe fn complete(ctx: &mut TaskCtx, index: u32, status: u8) -> ! {
    unsafe { crate::stop::check(ctx, engine::at_head(ctx)) };
    let ctx_ptr = std::ptr::from_mut(ctx);
    let began_end = role::with_role(ctx_ptr, |s| s.task(index).ended);
    let effect = || {
        role::with_role(ctx_ptr, |s| finish_end(ctx_ptr, s, index, status));
        ((), Next::End)
    };
    let effect = if began_end {
        engine::head_continuation(ctx, effect)
    } else {
        engine::head_effect(ctx, "task.end", effect)
    };
    let (_, progress) = unsafe { crate::stop::check(ctx, effect) };
    #[cfg(feature = "ownership-trace")]
    crate::ownership_trace::task_end(ctx);
    let task = engine::current(ctx);
    engine::with_driver(ctx, |driver| {
        if progress == Progress::Poll {
            driver.defer_poll_until_saved(task);
        } else {
            // The frontier returned into a caller without a scheduling point.
            driver.defer_return_until_saved(task);
        }
        driver.finishing(task);
    });
    if let Some(metrics) = &ctx.run_ref().worker_metrics {
        metrics.complete();
    }
    if ctx.failure.active() || ctx.shield != 0 || !ctx.work.is_null() || !ctx.region.is_null() {
        fault("a task completed with context state left");
    }
    drop(std::mem::take(&mut ctx.failure));
    drop(std::mem::take(&mut ctx.frozen));
    drop(std::mem::take(&mut ctx.isolated));
    drop(std::mem::take(&mut ctx.isolations));
    unsafe { crate::pool::free::finish_current(ctx) }
}

/// The end effect's table work: no TaskCtx bridge, switch or user code.
fn finish_end(ctx: *mut TaskCtx, s: &mut Scheduler, index: u32, status: u8) {
    let row = s.task(index);
    let first_end = !std::mem::replace(&mut row.ended, true);
    let path = row.path.clone();
    let chain = unsafe { (*ctx).failure.take_chain(&path) };
    let failed = status != STATUS_OK;
    if failed != !chain.is_empty() {
        fault("a task's status disagrees with its failure state");
    }
    s.task(index).finishing = true;
    if first_end && !failed {
        s.order(index);
    }
    if failed {
        s.task(index).failure = Some(chain);
        s.order(index);
        let scope = s.task(index).scope.expect("a child has a scope");
        s.cancel_scope(scope, CANCEL_CHILD_FAILED);
        let siblings = s.scope(scope).children.clone();
        s.wake_owner(scope);
        let mut cancellations = Vec::new();
        for sibling in siblings {
            if sibling != index {
                if let Some(events) =
                    super::events::cancel_observed(s, sibling, CANCEL_CHILD_FAILED)
                {
                    cancellations.extend(events);
                }
            }
        }
        cancellations.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        unsafe { super::events::failed_end(ctx, s, index, cancellations) };
    }
}

/// Called under IdleGuard after physical ownership invalidation/publication.
/// No TaskCtx bridge or switch is allowed here.
pub(super) fn after_save(s: &mut Scheduler, task: u32, completed: bool) {
    if completed && s.task(task).finishing {
        let row = s.task(task);
        row.finishing = false;
        row.state = State::Done;
        if let Some(stack) = row.stack.take() {
            // SAFETY: the completed token was invalidated and context state
            // consumed before idle. No other task can yet recycle this row.
            unsafe {
                std::ptr::drop_in_place(row.ctx);
                drop(row.context_home.take());
                crate::pool::free::recycle_stack(stack);
            }
        }
        if let Some(parent) = s.task(task).handoff.take() {
            let parent_id = handle(parent, s.task(parent).generation);
            let still_calling = match &s.logical {
                Logical::Ordered(driver) => {
                    driver.frontier.as_ref().unwrap().state(parent_id)
                        == crate::ordered::State::Parked
                }
                _ => unreachable!(),
            };
            if !still_calling {
                s.task(parent).state = State::Ready;
            }
        }
    }
    if matches!(s.task(task).state, State::Holding | State::Done) {
        s.settle(task);
    }
}

/// The current callback already used this generation for its barrier. No
/// later callback can access the row after this exclusive idle retirement.
pub(super) fn retire_detached(s: &mut Scheduler, task: u32) {
    let row = s.task(task);
    if row.state == State::Done && row.detached && row.failure.is_none() {
        let scope = row.scope.expect("a detached child has a scope");
        s.scope(scope).children.retain(|&c| c != task);
        s.vacate_task(task);
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::ordered::Policy;

    const NONE: [u64; 2] = [0, 0];

    #[derive(Default)]
    struct DetachOrder {
        logged: std::sync::atomic::AtomicBool,
        child: std::sync::atomic::AtomicU64,
        rest_done: std::sync::atomic::AtomicBool,
        child2: std::sync::atomic::AtomicU64,
        set: std::sync::atomic::AtomicU64,
        order: std::sync::Mutex<Vec<u64>>,
    }

    unsafe extern "C" fn detach_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        use std::sync::atomic::Ordering;
        let [probe, mode, atomic] = unsafe { crate::alloc::payload(env).cast::<[u64; 3]>().read() };
        let probe = unsafe { &*(probe as *const DetachOrder) };
        if mode != 1 {
            let mut result = [1u64, 17];
            assert_eq!(
                unsafe {
                    hirsel_rt_v2_task_finish(
                        ctx,
                        result.as_mut_ptr().cast(),
                        NONE.as_ptr().cast(),
                        16,
                        u32::MAX,
                    )
                },
                STATUS_OK
            );
            assert_eq!(result, [1, 17], "discard remains owned by this rest");
            let set = probe.set.load(Ordering::Acquire);
            if set != 0 {
                let _role = unsafe { role::Guard::new(ctx) };
                let s = scheduler(ctx);
                assert!(
                    s.sets[set as u32 as usize].is_some(),
                    "set remains owned through all calls"
                );
                if mode == 2 {
                    let later = probe.child.load(Ordering::Acquire) as u32;
                    assert!(
                        !s.task(later).detached,
                        "later member is untouched during the first rest"
                    );
                    assert_eq!(s.task(later).state, State::Holding);
                }
            }
            let mut canceled = 0;
            assert_eq!(
                unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) },
                STATUS_OK
            );
            let mut value = u64::MAX;
            assert_eq!(
                unsafe { hirsel_rt_v2_atomic_load(ctx, &atomic, (&raw mut value).cast(), 8) },
                STATUS_OK
            );
            assert_eq!(
                value, 0,
                "later caller logs cannot retire inside its child's rest"
            );
            probe.rest_done.store(true, Ordering::Release);
            probe.order.lock().unwrap().push(mode);
        } else {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !probe.logged.load(Ordering::Acquire) {
                assert!(
                    std::time::Instant::now() < until,
                    "younger detach did not run ahead"
                );
                std::thread::yield_now();
            }
            let _role = unsafe { role::Guard::new(ctx) };
            let child = probe.child.load(Ordering::Acquire) as u32;
            let s = scheduler(ctx);
            assert_eq!(s.task(child).state, State::Holding);
            assert!(!s.task(child).detached, "detach mutates only at retirement");
            assert!(s.task(child).slot.is_some());
            assert!(!probe.rest_done.load(Ordering::Acquire));
            let child2 = probe.child2.load(Ordering::Acquire);
            if child2 != 0 {
                assert!(!s.task(child2 as u32).detached);
                assert_eq!(s.task(child2 as u32).state, State::Holding);
            }
        }
        STATUS_OK
    }

    pub(crate) fn run_logged_detach(bulk: bool) {
        use std::sync::{Arc, atomic::Ordering};
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 24,
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
            let probe = Arc::new(DetachOrder::default());
            let seen = probe.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
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
                    let mut held = 0;
                    let mut set = 0;
                    if bulk {
                        assert_eq!(
                            hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope),
                            STATUS_OK
                        );
                        seen.set.store(set, Ordering::Release);
                    }
                    for mode in if bulk { vec![0, 2, 1] } else { vec![0, 1] } {
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            24,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env).cast::<[u64; 3]>().write([
                            Arc::as_ptr(&seen) as u64,
                            mode,
                            atomic,
                        ]);
                        let mut child = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut child,
                                &scope,
                                detach_child,
                                env,
                                0
                            ),
                            STATUS_OK
                        );
                        if mode != 1 {
                            if mode == 0 {
                                held = child;
                                seen.child.store(child, Ordering::Release);
                            } else {
                                seen.child2.store(child, Ordering::Release);
                            }
                            let _role = role::Guard::new(ctx);
                            engine::at_head(ctx).unwrap();
                            assert_eq!(
                                crate::tasks::role::with_role(ctx, |s| s.task(child as u32).state),
                                State::Holding
                            );
                            if mode == 2 {
                                assert_eq!(
                                    hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut child),
                                    STATUS_OK
                                );
                                assert_eq!(
                                    hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut held),
                                    STATUS_OK
                                );
                                engine::at_head(ctx).unwrap();
                            }
                        }
                    }
                    if bulk {
                        assert_eq!(
                            hirsel_rt_v2_task_detach_all(ctx, &scope, &mut set),
                            STATUS_OK
                        );
                    } else {
                        assert_eq!(hirsel_rt_v2_task_detach(ctx, &scope, &mut held), STATUS_OK);
                    }
                    let value = 7u64;
                    assert_eq!(
                        hirsel_rt_v2_atomic_store(ctx, &atomic, (&raw const value).cast(), 8),
                        STATUS_OK
                    );
                    seen.logged.store(true, Ordering::Release);
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                    let mut loaded = 0;
                    assert_eq!(
                        hirsel_rt_v2_atomic_load(ctx, &atomic, (&raw mut loaded).cast(), 8),
                        STATUS_OK
                    );
                    assert_eq!(loaded, 7);
                    let _role = role::Guard::new(ctx);
                    engine::at_head(ctx).unwrap();
                    if bulk {
                        assert!(scheduler(ctx).sets[set as u32 as usize].is_none());
                        assert!(crate::tasks::ordered::engine::with_driver(ctx, |d| d
                            .detaching
                            .is_empty()));
                    }
                    let me = engine::current(ctx);
                    crate::tasks::ordered::engine::with_driver(ctx, |d| d.finishing(me));
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert!(probe.rest_done.load(Ordering::Acquire));
            assert_eq!(
                *probe.order.lock().unwrap(),
                if bulk { vec![2, 0] } else { vec![0] }
            );
        }
    }

    #[test]
    fn logged_native_detach_calls_a_saved_yielding_rest_before_later_caller_logs() {
        run_logged_detach(false);
    }

    #[test]
    fn logged_native_bulk_detach_calls_members_in_add_order_before_closing_the_set() {
        run_logged_detach(true);
    }

    #[derive(Default)]
    struct SpawnOrder {
        issued: std::sync::atomic::AtomicBool,
        observed: std::sync::atomic::AtomicBool,
        children: [std::sync::atomic::AtomicU64; 2],
        paths: std::sync::Mutex<Vec<Vec<u32>>>,
    }

    unsafe extern "C" fn spawning_child(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
        use std::sync::atomic::Ordering;
        let [probe, ordinal, scope] =
            unsafe { crate::alloc::payload(env).cast::<[u64; 3]>().read() };
        let probe = unsafe { &*(probe as *const SpawnOrder) };
        if ordinal == 0 {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !probe.issued.load(Ordering::Acquire) {
                assert!(
                    std::time::Instant::now() < until,
                    "younger spawner could not run ahead"
                );
                std::thread::yield_now();
            }
            let _role = unsafe { role::Guard::new(ctx) };
            let s = scheduler(ctx);
            for child in &probe.children {
                let child = child.load(Ordering::Acquire);
                assert!(
                    s.tasks[child as u32 as usize].is_none(),
                    "spawn has not retired"
                );
            }
            assert_eq!(s.task(0).spawned, 1);
            let scope = s.scope_handle(scope);
            assert_eq!(s.scope(scope).children.len(), 1);
            assert!(
                probe.paths.lock().unwrap().is_empty(),
                "unpublished children never start"
            );
            probe.observed.store(true, Ordering::Release);
        } else {
            let _role = unsafe { role::Guard::new(ctx) };
            let s = scheduler(ctx);
            let me = s.current;
            let path = s.task(me).path.clone();
            assert_eq!(path, vec![ordinal as u32]);
            assert_eq!(
                s.task(me).parent.unwrap().0,
                0,
                "publication uses the originating parent"
            );
            probe.paths.lock().unwrap().push(path);
        }
        STATUS_OK
    }

    #[test]
    fn native_logged_spawns_remain_unpublished_behind_a_running_head_and_keep_owner_paths() {
        use std::sync::{Arc, atomic::Ordering};
        let layout = crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size: 24,
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
            let probe = Arc::new(SpawnOrder::default());
            let seen = probe.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe { engine::enable(ctx, Policy::admit(&Default::default(), None)) },
                move |ctx| unsafe {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        STATUS_OK
                    );
                    for ordinal in 0..3 {
                        let env = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            24,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(env).cast::<[u64; 3]>().write([
                            Arc::as_ptr(&seen) as u64,
                            ordinal,
                            scope,
                        ]);
                        let mut child = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut child,
                                &scope,
                                spawning_child,
                                env,
                                0
                            ),
                            STATUS_OK
                        );
                        if ordinal != 0 {
                            seen.children[ordinal as usize - 1].store(child, Ordering::Release);
                            let _role = role::Guard::new(ctx);
                            assert!(scheduler(ctx).tasks[child as u32 as usize].is_none());
                            assert!(
                                !engine::driver(ctx)
                                    .frontier
                                    .as_ref()
                                    .unwrap()
                                    .queue()
                                    .any(|id| id == child)
                            );
                        }
                    }
                    seen.issued.store(true, Ordering::Release);
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                    let _role = role::Guard::new(ctx);
                    engine::at_head(ctx).unwrap();
                    let me = engine::current(ctx);
                    crate::tasks::ordered::engine::with_driver(ctx, |d| d.finishing(me));
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert!(probe.observed.load(Ordering::Acquire));
            let mut paths = probe.paths.lock().unwrap().clone();
            paths.sort();
            assert_eq!(paths, [vec![1], vec![2]]);
        }
    }

    unsafe extern "C" fn child(ctx: *mut TaskCtx, environment: *mut Header, _: u64, _: u64) -> u8 {
        let [identity, atomic] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 2]>().read() };
        let id = identity & 0xff;
        let discarded = identity >> 63 != 0;
        let local = [id; 32];
        for _ in 0..32 {
            let mut loaded = 0u64;
            assert_eq!(
                unsafe { hirsel_rt_v2_atomic_load(ctx, &atomic, (&raw mut loaded).cast(), 8) },
                STATUS_OK
            );
            assert_eq!(loaded, id);
        }
        let mut result = [1, id * 100];
        assert_eq!(
            unsafe {
                hirsel_rt_v2_task_finish(
                    ctx,
                    result.as_mut_ptr().cast(),
                    NONE.as_ptr().cast(),
                    16,
                    u32::MAX,
                )
            },
            STATUS_OK
        );
        assert_eq!(
            result,
            if discarded { [1, id * 100] } else { NONE },
            "discard keeps the result for its owner; join moves it",
        );
        assert_eq!(local, [id; 32], "a migrated rest retains its owned stack");
        // A rest's ordinary yield permits other turns before the no-poll call
        // return. The caller cannot vacate this row before physical completion.
        let value = id + 1000;
        unsafe { hirsel_rt_v2_atomic_store(ctx, &atomic, (&raw const value).cast(), 8) }
    }

    fn children_run(width: u32, holding_first: bool, consume: u8) {
        children_run_commit(
            width,
            holding_first,
            consume,
            hirsel_run_config::Commit::Shared,
        );
    }

    #[test]
    fn dedicated_commit_preserves_holding_join_scope_exit_and_detach_rests() {
        for width in [1, 8] {
            for (holding, consume) in [(false, 0), (true, 0), (false, 1), (false, 2), (true, 3)] {
                children_run_commit(
                    width,
                    holding,
                    consume,
                    hirsel_run_config::Commit::Dedicated,
                );
            }
        }
    }

    fn children_run_commit(
        width: u32,
        holding_first: bool,
        consume: u8,
        commit: hirsel_run_config::Commit,
    ) {
        use hirsel_core_contract::handle_scan::{TYPED_HANDLE, WORDS};
        let scan = [TYPED_HANDLE, 8, Kind::Atomic.code(), 2, 0, 0, 0, 0];
        assert_eq!(scan.len(), WORDS);
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
            scan: scan.as_ptr(),
            scan_words: scan.len() as u64,
        };
        let layouts = [&environment as *const crate::Layout];
        let mut run = crate::host::BodyRun::new(width, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = layouts.len() as u32;
        // SAFETY: only the installed strict task and E boundaries are used.
        // Environments are dropless, there are no budgets/host/root effects,
        // every child is joined, and root completion is explicit below.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                let mut policy = Policy::admit(&Default::default(), None);
                policy.commit = commit;
                engine::enable(ctx, policy);
            },
            move |ctx| unsafe {
                for _ in 0..3 {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        STATUS_OK
                    );
                    let mut children = Vec::new();
                    for id in 1..=8 {
                        let mut atomic = 0;
                        assert_eq!(
                            hirsel_rt_v2_atomic_create(
                                ctx,
                                std::ptr::null_mut(),
                                &mut atomic,
                                (&raw const id).cast(),
                                8
                            , 0, 8,),
                            STATUS_OK
                        );
                        let environment = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            16,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(environment)
                            .cast::<[u64; 2]>()
                            .write([id | if consume == 0 { 0 } else { 1 << 63 }, atomic]);
                        let mut task = 0;
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut task,
                                &scope,
                                child,
                                environment,
                                0,
                            ),
                            STATUS_OK
                        );
                        children.push((task, atomic, id));
                        if consume == 2 {
                            assert_eq!(hirsel_rt_v2_task_detach(ctx, &scope, &mut task), STATUS_OK);
                        }
                    }
                    if holding_first {
                        // These serial head turns let every child's first half
                        // hold before a join starts its rest.
                        let atomic = children[0].1;
                        for _ in 0..64 {
                            let mut value = 0u64;
                            assert_eq!(
                                hirsel_rt_v2_atomic_load(ctx, &atomic, (&raw mut value).cast(), 8),
                                STATUS_OK
                            );
                        }
                        role::enter(ctx);
                        assert!(
                            children
                                .iter()
                                .all(|&(task, _, _)| crate::tasks::role::with_role(ctx, |s| s.task(task as u32).state)
                                    == State::Holding)
                        );
                        role::release(ctx);
                    }
                    if matches!(consume, 1 | 2) {
                        assert_eq!(hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0), STATUS_OK);
                    }
                    for (mut task, atomic, id) in children.into_iter().rev() {
                        let retired = if holding_first && matches!(consume, 0 | 3) {
                            role::enter(ctx);
                            engine::at_head(ctx).unwrap();
                            let retired = crate::tasks::ordered::engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().retired());
                            role::release(ctx);
                            Some(retired)
                        } else { None };
                        if consume == 0 {
                        let mut result = NONE;
                        assert_eq!(
                            hirsel_rt_v2_task_join(
                                ctx,
                                0,
                                0,
                                &scope,
                                &mut task,
                                result.as_mut_ptr().cast(),
                                16
                            ),
                            STATUS_OK
                        );
                        assert_eq!(result, [1, id * 100]);
                        } else if consume == 3 {
                            assert_eq!(hirsel_rt_v2_task_detach(ctx, &scope, &mut task), STATUS_OK);
                        }
                        if let Some(retired) = retired {
                            role::enter(ctx);
                            // D detach returns at issuance. Observe the same
                            // completed call boundary as the yielding join.
                            engine::at_head(ctx).unwrap();
                            assert_eq!(
                                crate::tasks::ordered::engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().retired()) - retired,
                                2,
                                "only the rest's store and the join yield poll; call return does not",
                            );
                            role::release(ctx);
                        }
                        let mut value = 0u64;
                        assert_eq!(
                            hirsel_rt_v2_atomic_load(ctx, &atomic, (&raw mut value).cast(), 8),
                            STATUS_OK
                        );
                        assert_eq!(value, id + 1000, "join waits for the child's yielding rest");
                    }
                    if !matches!(consume, 1 | 2) {
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                        STATUS_OK
                    );
                    }
                }
                role::enter(ctx);
                engine::at_head(ctx).unwrap();
                assert!(crate::tasks::role::with_role(ctx, |s| s.scopes.iter().all(Option::is_none)));
                let task = engine::current(ctx);
                let driver = engine::driver(ctx);
                assert!(driver.frontier.as_ref().unwrap().retired() > 700);
                driver.finishing(task);
                role::release(ctx);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn native_spawn_park_join_and_call_rests_use_saved_ordered_frames() {
        for width in [2, 4, 8] {
            children_run(width, false, 0);
        }
    }

    #[test]
    fn joining_already_holding_children_yields_only_after_their_rests_return() {
        for width in [2, 4, 8] {
            children_run(width, true, 0);
        }
    }
    #[test]
    fn scope_exit_calls_unjoined_holding_rests_and_waits_for_their_physical_completion() {
        for width in [2, 4, 8] {
            children_run(width, false, 1);
        }
    }

    #[test]
    fn detached_running_and_holding_children_retire_their_own_results_and_rows() {
        for width in [2, 4, 8] {
            children_run(width, false, 2);
            children_run(width, true, 3);
        }
    }

    unsafe extern "C" fn send_child(
        ctx: *mut TaskCtx,
        environment: *mut Header,
        _: u64,
        _: u64,
    ) -> u8 {
        let [mut sender, await_saved] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 2]>().read() };
        if await_saved != 0 {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let waiting = {
                    let _role = unsafe { role::Guard::new(ctx) };
                    let s = scheduler(ctx);
                    matches!(s.task(0).wait, Some(Wait::Receive { .. })) && readiness::saved(s, 0)
                };
                if waiting {
                    break;
                }
                assert!(
                    std::time::Instant::now() < until,
                    "receiver never saved its wait"
                );
                let mut cause = 0;
                assert_eq!(
                    unsafe { hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
                    STATUS_OK
                );
            }
        }
        let mut scope = 0;
        assert_eq!(
            unsafe { hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0) },
            STATUS_OK
        );
        let mut message = [1u64, 77];
        let mut status = u32::MAX;
        assert_eq!(
            unsafe {
                hirsel_rt_v2_channel_send(
                    ctx,
                    &mut status,
                    &mut sender,
                    message.as_mut_ptr().cast(),
                )
            },
            STATUS_OK
        );
        assert_eq!(status, crate::channels::DONE);
        assert_eq!(message, NONE);
        assert_eq!(
            unsafe { hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0) },
            STATUS_OK
        );
        let mut result = [1u64, 9];
        unsafe {
            hirsel_rt_v2_task_finish(ctx, result.as_mut_ptr().cast(), NONE.as_ptr().cast(), 16, 3)
        }
    }

    pub(crate) fn receive_run(
        width: u32,
        sender: bool,
        observing: bool,
        reorder: hirsel_run_config::Reorder,
    ) -> Option<(
        super::super::events::ReceivePollCompletion,
        Vec<super::super::events::CanonicalEvent>,
    )> {
        receive_run_commit(
            width,
            sender,
            observing,
            reorder,
            hirsel_run_config::Commit::Shared,
        )
    }

    #[test]
    fn dedicated_commit_preserves_saved_receive_evidence_and_source_free_blocked_cut() {
        let mut reference = None;
        for width in [1, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let observed = receive_run_commit(
                    width,
                    true,
                    true,
                    reorder,
                    hirsel_run_config::Commit::Dedicated,
                )
                .unwrap();
                if let Some(reference) = &reference {
                    assert_eq!(&observed, reference);
                } else {
                    reference = Some(observed);
                }
                receive_run_commit(
                    width,
                    false,
                    false,
                    reorder,
                    hirsel_run_config::Commit::Dedicated,
                );
            }
        }
    }

    fn receive_run_commit(
        width: u32,
        sender: bool,
        observing: bool,
        reorder: hirsel_run_config::Reorder,
        commit: hirsel_run_config::Commit,
    ) -> Option<(
        super::super::events::ReceivePollCompletion,
        Vec<super::super::events::CanonicalEvent>,
    )> {
        use hirsel_core_contract::handle_scan::*;
        use std::sync::{Arc, Mutex};
        let observed = Arc::new(Mutex::new(None));
        let seen = observed.clone();
        let scan = [TYPED_HANDLE, 0, Kind::Sender.code(), 2, 0, 0, 0, 0];
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
            scan: scan.as_ptr(),
            scan_words: 8,
        };
        let option = [
            VALUE,
            VALUE_VARIANT,
            16,
            0,
            0,
            0,
            2,
            0,
            VALUE_FIELD,
            8,
            4,
            1,
            0,
            0,
            0,
            0,
        ];
        let bits = [VALUE, VALUE_BITS, 8, 0, 0, 0, 0, 0];
        let image = crate::Layout {
            scan: option.as_ptr(),
            scan_words: option.len() as u64,
            ..environment
        };
        let scalar = crate::Layout {
            size: 8,
            scan: bits.as_ptr(),
            scan_words: bits.len() as u64,
            ..environment
        };
        let layouts = [&environment as *const crate::Layout, &image, &scalar];
        let mut run = crate::host::BodyRun::new(width, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = layouts.len() as u32;
        // SAFETY: only supported task/E/receive boundaries; no budgets, host,
        // finalizers or Shared roots. Quiescent endpoint cleanup is explicit.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                let mut policy = Policy::admit(&Default::default(), None);
                policy.reorder = reorder;
                policy.commit = commit;
                engine::enable(ctx, policy);
                if observing {
                    super::super::events::enable(ctx);
                }
            },
            move |ctx| unsafe {
                let mut scope = 0;
                assert_eq!(
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                    STATUS_OK
                );
                let mut endpoints = [0u64; 2];
                assert_eq!(
                    hirsel_rt_v2_channel_open(
                        ctx,
                        std::ptr::null_mut(),
                        endpoints.as_mut_ptr(),
                        &0,
                        NONE.as_ptr().cast(),
                        3
                    ),
                    STATUS_OK
                );
                let mut child = 0;
                if sender {
                    let environment = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        16,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(environment)
                        .cast::<[u64; 2]>()
                        .write([endpoints[0], u64::from(observing)]);
                    assert_eq!(
                        hirsel_rt_v2_task_spawn(
                            ctx,
                            0,
                            0,
                            std::ptr::null_mut(),
                            &mut child,
                            &scope,
                            send_child,
                            environment,
                            0,
                        ),
                        STATUS_OK
                    );
                }
                let mut status = u32::MAX;
                let mut message = NONE;
                assert_eq!(
                    hirsel_rt_v2_channel_receive(
                        ctx,
                        0,
                        0,
                        &mut status,
                        &mut endpoints[1],
                        message.as_mut_ptr().cast()
                    ),
                    STATUS_OK
                );
                assert!(sender, "a blocked receive cannot continue");
                assert_eq!(status, crate::channels::DONE);
                assert_eq!(message, [1, 77]);
                if observing {
                    let _role = role::Guard::new(ctx);
                    let log = engine::driver(ctx).events.as_ref().unwrap();
                    assert_eq!(
                        log.receive_polls.len(),
                        1,
                        "actual saved poll completed receive"
                    );
                    let poll = &log.receive_polls[0];
                    assert_eq!(poll.status, crate::channels::DONE);
                    assert_eq!(poll.sequence, 0);
                    assert_eq!(
                        poll.payload,
                        Some([1u32.to_le_bytes().as_slice(), &77u64.to_le_bytes()].concat())
                    );
                    assert!(log.boundaries.iter().any(|b| b.task == poll.task
                        && b.ordinal == poll.ordinal
                        && b.transition == super::super::events::Transition::Park));
                    let done = log
                        .owner_completions
                        .iter()
                        .find(|done| done.task == poll.task && done.ordinal == poll.ordinal)
                        .unwrap();
                    let super::super::events::OwnerOutcome::Receive {
                        status, payload, ..
                    } = &done.outcome
                    else {
                        panic!("receive owner outcome")
                    };
                    assert_eq!(*status, poll.status);
                    assert_eq!(payload, &poll.payload);
                    let rows: Vec<_> = log
                        .canonical()
                        .expect("supported saved receive prefix")
                        .iter()
                        .filter(|row| {
                            row.op == "channel.receive"
                                && row.task == poll.task
                                && row.ordinal == poll.ordinal
                        })
                        .cloned()
                        .collect();
                    assert_eq!(rows.len(), 5);
                    for (subevent, row) in rows.iter().enumerate() {
                        assert_eq!(row.subevent as usize, subevent);
                    }
                    assert_eq!(rows[0].transition, super::super::events::Transition::Yield);
                    assert_eq!(rows[2].transition, super::super::events::Transition::Park);
                    assert_eq!(rows[4].transition, super::super::events::Transition::Keep);
                    assert_eq!(rows[4].retirement, poll.retirement);
                    assert_eq!(
                        rows[4].outcome,
                        super::super::events::CanonicalOutcome::Received {
                            status: poll.status,
                            payload: poll.payload.clone()
                        }
                    );
                    *seen.lock().unwrap() = Some((poll.clone(), rows));
                }
                let mut result = NONE;
                assert_eq!(
                    hirsel_rt_v2_task_join(
                        ctx,
                        0,
                        0,
                        &scope,
                        &mut child,
                        result.as_mut_ptr().cast(),
                        16
                    ),
                    STATUS_OK
                );
                assert_eq!(result, [1, 9]);
                result.fill(0);
                if observing {
                    let _role = role::Guard::new(ctx);
                    let log = engine::driver(ctx).events.as_ref().unwrap();
                    assert_eq!(
                        log.task_results[&vec![0]],
                        super::super::events::TaskResult {
                            descriptor: 3,
                            payload: Some(
                                [1u32.to_le_bytes().as_slice(), &9u64.to_le_bytes()].concat()
                            ),
                        },
                        "task result remains owned after delivery and row retirement"
                    );
                }
                assert_eq!(
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0),
                    STATUS_OK
                );
                role::enter(ctx);
                engine::at_head(ctx).unwrap();
                crate::tasks::role::with_role(ctx, |s| s.channels.close(endpoints[0]));
                crate::tasks::role::with_role(ctx, |s| s.channels.close(endpoints[1]));
                let task = engine::current(ctx);
                crate::tasks::ordered::engine::with_driver(ctx, |d| d.finishing(task));
                role::release(ctx);
                STATUS_OK
            },
        );
        if sender {
            assert_eq!(outcome.ended, Ok(STATUS_OK));
        } else {
            let Err(Stop::Blocked(waits)) = outcome.ended else {
                panic!("{outcome:?}")
            };
            assert_eq!(waits.len(), 1);
            assert_eq!(waits[0].kind, "channel.receive");
            assert!(waits[0].spawn_path.is_empty());
        }
        assert_eq!(outcome.pending, 0);
        observed.lock().unwrap().clone()
    }

    #[test]
    fn saved_receive_poll_evidence_matches_owner_at_one_and_many_workers() {
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let observed = receive_run(width, true, true, reorder).unwrap();
                if let Some(reference) = &reference {
                    assert_eq!(&observed, reference);
                } else {
                    reference = Some(observed);
                }
            }
        }
    }

    #[test]
    fn a_real_ordered_receive_wakes_from_a_retired_send_and_moves_the_saved_slot() {
        for width in [2, 4, 8] {
            receive_run(width, true, false, hirsel_run_config::Reorder::Auto);
        }
    }

    #[test]
    fn the_last_ordered_receive_reports_blocked_only_after_idle_save_and_poll() {
        for width in [2, 4, 8] {
            receive_run(width, false, false, hirsel_run_config::Reorder::Auto);
        }
    }
}
