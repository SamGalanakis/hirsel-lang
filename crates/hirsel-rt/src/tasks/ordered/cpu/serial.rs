//! Caller-owned CPU jobs use their ordinary TaskCode, without a resumable
//! child, a cooperative turn, or a private alternate. The parent retains the
//! lexical scope and the generation-qualified child row until join/discard.
use super::super::{ahead, engine};
use crate::budget::stretch::{Budgets, Settled, Stretch};
use crate::stop::{Stop, Stopped};
use crate::tasks::{self, State as TaskState, Task, TaskCode, handle, role};
use crate::{Header, TaskCtx};

pub(crate) struct State {
    pub(crate) path: Vec<u32>,
    ordinal: u64,
    parent: *mut TaskCtx,
    initial: Settled,
    settled: Settled,
    peak: u64,
    peak_fuel: u64,
    budgets: Budgets,
    issued: u64,
    spent: u64,
    image: Option<(Vec<u128>, usize, u32)>,
    disposed: bool,
    receipt: Option<crate::io::CpuSummary>,
    pub(crate) probe: Option<std::sync::Arc<crate::pool::cpu_scoped::probe::Job>>,
}

pub(crate) fn get(ctx: *mut TaskCtx) -> Option<*mut State> {
    // The running job exclusively owns this cell, including while the heap
    // has a mutable borrow. Meter callbacks touch only this disjoint cell.
    unsafe { (*ctx).ahead.as_ref()?.serial.as_ref().map(|s| s.get()) }
}

pub(crate) fn on(ctx: *mut TaskCtx) -> bool {
    get(ctx).is_some()
}

impl State {
    fn capture(&mut self, word: i64) -> Stopped<()> {
        let spent = self
            .issued
            .saturating_sub(word.max(0) as u64)
            .saturating_add(u64::from(word < 0));
        self.issued = word.max(0) as u64;
        self.spent = self.spent.saturating_add(spent.min(self.settled.fuel));
        if spent > self.settled.fuel {
            self.settled.fuel = 0;
            return Err(Stop::BudgetExceeded(hirsel_run_schema::BudgetKind::Fuel));
        }
        if self.settled.fuel != u64::MAX {
            self.settled.fuel -= spent;
        }
        Ok(())
    }

    // All charges were checked at their serial execution positions. One
    // high-water mark is sufficient to certify this already accepted prefix;
    // replay executes and checks the same serial stream before comparing it.
    fn checked_prefix(&self) -> Stretch {
        let fuel = self.spent;
        let mut charges = Stretch::new(Budgets {
            memory: true,
            ..self.budgets
        });
        charges.fuel_points(self.peak_fuel);
        charges
            .alloc(self.peak)
            .expect("one serial high-water mark");
        let credit = self.initial.level + self.peak - self.settled.level;
        charges.free(credit);
        charges.fuel_points(fuel - self.peak_fuel);
        charges
    }
}

pub(crate) fn refill(ctx: *mut TaskCtx) -> Stopped<()> {
    let word = unsafe { (*ctx).view.compiled.fuel };
    unsafe { (*ctx).ordered_charge_trace.capture(word) };
    let state = unsafe { &mut *get(ctx).unwrap() };
    state.capture(word)?;
    let ordered = unsafe {
        (*ctx)
            .run_ref()
            .ordered_mode
            .load(std::sync::atomic::Ordering::Acquire)
    };
    let grant = if ordered {
        state.settled.fuel.min(crate::budget::FUEL_CHUNK)
    } else {
        unsafe { (*ctx).run_ref().fuel.take() }
            .checked_sub(1)
            .ok_or(Stop::BudgetExceeded(hirsel_run_schema::BudgetKind::Fuel))?
    };
    state.issued = grant;
    unsafe {
        (*ctx).heap().set_fuel(grant as i64);
        (*ctx).ordered_charge_trace.grant(grant as i64);
    }
    Ok(())
}

pub(crate) fn reserve(ctx: *mut TaskCtx, bytes: u64) -> Stopped<()> {
    let word = unsafe { (*ctx).view.compiled.fuel };
    unsafe {
        (*ctx).ordered_charge_trace.capture(word);
        (*ctx).ordered_charge_trace.allocation(bytes);
    }
    let state = unsafe { &mut *get(ctx).unwrap() };
    state.allocate(word, bytes)?;
    unsafe { (*ctx).ordered_allocation.set(bytes) };
    Ok(())
}

impl State {
    fn allocate(&mut self, word: i64, bytes: u64) -> Stopped<()> {
        self.capture(word)?;
        if self.settled.level.saturating_add(bytes) > self.settled.limit {
            return Err(Stop::BudgetExceeded(hirsel_run_schema::BudgetKind::Memory));
        }
        self.settled.level += bytes;
        let high = self.settled.level.saturating_sub(self.initial.level);
        if high > self.peak {
            self.peak = high;
            self.peak_fuel = self.spent;
        }
        Ok(())
    }
}

pub(crate) fn charged(ctx: *mut TaskCtx, bytes: u64) {
    assert_eq!(unsafe { (*ctx).ordered_allocation.replace(0) }, bytes);
    let level = unsafe { (*get(ctx).unwrap()).settled.level };
    unsafe { (*ctx).heap_ref().meter.settled(level) };
}

pub(crate) fn freed(ctx: *mut TaskCtx, bytes: u64) {
    unsafe {
        (*ctx)
            .ordered_charge_trace
            .free((*ctx).view.compiled.fuel, bytes)
    };
    let state = unsafe { &mut *get(ctx).unwrap() };
    // A free cannot stop while its heap is borrowed. Its preceding fuel was
    // checked at allocation/refill; capture at the next safe boundary.
    state
        .capture(unsafe { (*ctx).view.compiled.fuel })
        .expect("a free cannot cross a compiled fuel boundary");
    state.settled.level = state
        .settled
        .level
        .checked_sub(bytes)
        .expect("serial CPU credit");
    unsafe { (*ctx).heap_ref().meter.settled(state.settled.level) };
}

/// The checked ordinary emitter publishes initialized fields, never padding.
/// # Safety
/// The checked worker owns this record and all its retained roots.
pub(crate) unsafe fn publish(
    ctx: *mut TaskCtx,
    summary: *mut ahead::cpu_handoff::Summary,
) -> Stopped<u8> {
    let state = unsafe { &mut *get(ctx).unwrap() };
    let row = unsafe { &mut *summary };
    if row.version != ahead::cpu_handoff::VERSION
        || row.ordinal != state.ordinal
        || row.phase != ahead::cpu_handoff::SUMMARY_PENDING
        || state.receipt.is_some()
    {
        return Ok(ahead::cpu_handoff::NOT_CERTIFIED);
    }
    unsafe {
        (*ctx)
            .ordered_charge_trace
            .capture((*ctx).view.compiled.fuel)
    };
    state.capture(unsafe { (*ctx).view.compiled.fuel })?;
    let program = unsafe { super::super::values::Program::linked(ctx, row.descriptor) }
        .map_err(|e| Stop::Refused(format!("CPU result recipe: {e:?}")))?;
    if program.extent() as u64 != row.value_bytes {
        return Err(Stop::Refused("CPU result recipe extent".into()));
    }
    let value = program
        .encode_with_host(row.value, |_, _| None, |_| None)
        .map_err(|e| Stop::Refused(format!("CPU result encoding: {e:?}")))?;
    state.receipt = Some(crate::io::CpuSummary {
        version: 3,
        retirement: state.ordinal,
        ordinal: state.ordinal,
        value,
        stop: None,
        native: Some(crate::io::CpuNativeSummary {
            descriptor: row.descriptor,
            value_bytes: row.value_bytes,
            retained_count: row.retained_count,
            pieces: crate::io::CpuSummary::pieces(&state.checked_prefix()),
            level: state.settled.level,
            limit: state.settled.limit,
            fuel: state.settled.fuel,
        }),
    });
    row.phase = ahead::cpu_handoff::SUMMARY_RETIRED;
    Ok(ahead::cpu_handoff::ACCEPTED)
}

/// # Safety
/// The generated wrapper owns matching Option images of size bytes.
pub(crate) unsafe fn finish(
    ctx: *mut TaskCtx,
    slot: *mut u8,
    none: *const u8,
    size: u64,
    descriptor: u32,
) -> u8 {
    let state = unsafe { &mut *get(ctx).unwrap() };
    assert!(state.image.is_none());
    let size = usize::try_from(size).expect("bounded CPU result image");
    let mut image = vec![0u128; size.div_ceil(16)];
    unsafe {
        std::ptr::copy_nonoverlapping(slot, image.as_mut_ptr().cast(), size);
        std::ptr::copy_nonoverlapping(none, slot, size);
    }
    state.image = Some((image, size, descriptor));
    crate::STATUS_OK
}

pub(crate) fn child(ctx: *mut TaskCtx, word: u64) -> bool {
    role::with_role(ctx, |s| {
        s.tasks
            .get(word as u32 as usize)
            .and_then(Option::as_ref)
            .is_some_and(|row| {
                row.generation == (word >> 32) as u32
                    && row.cpu_scoped
                    && row.inline_context.is_some()
            })
    })
}

pub(crate) fn selected(ctx: *mut TaskCtx) -> bool {
    if root(ctx).is_some() {
        return true;
    }
    engine::with_driver(ctx, |d| d.cpu_batch.as_ref().is_some_and(|b| b.width == 1))
}

/// The typed CPU preclaim, rather than a private-alternate certificate,
/// identifies the next ordinary callback. No private frame is being entered.
pub(crate) fn pending(ctx: *mut TaskCtx, scope: u64) -> bool {
    if let Some(root) = root(ctx) {
        return unsafe { (*root).scope == Some(scope) && (*root).pending.is_some() };
    }
    if !engine::on(ctx) {
        return false;
    }
    let parent = engine::current(ctx);
    let claimed = engine::with_driver(ctx, |d| {
        d.cpu_batch.as_ref().is_some_and(|batch| {
            batch.width == 1
                && batch
                    .records
                    .iter()
                    .any(|row| row.pending_parent == Some(parent))
        })
    });
    if !claimed {
        return false;
    }
    role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        s.task(parent as u32).scopes.last() == Some(&scope)
    })
}

/// CPU storage is metered from Tasks projection, before captures allocate.
/// # Safety
/// The sole caller owns admission; no task has been published yet.
pub(crate) unsafe fn enable_meter(ctx: *mut TaskCtx) {
    if unsafe { (*ctx).heap_ref().meter.limit() } == 0 {
        unsafe {
            (*ctx).heap_ref().meter.set_limit(u64::MAX);
            for heap in (*ctx).run_ref().heaps.heaps() {
                (*heap).set_metered();
            }
        }
    }
}

/// # Safety
/// The parent owns the CPU scope and unstarted generated closure.
pub(crate) unsafe fn spawn(
    ctx: *mut TaskCtx,
    out: *mut u64,
    scope: u64,
    code: TaskCode,
    environment: *mut Header,
    site: u64,
) -> Stopped<u8> {
    let (child, generation, path, ordinal) = role::with_role(ctx, |s| {
        let parent = s.current;
        let scope = s.scope_handle(scope);
        let ordinal = if let Some(root) = root(ctx) {
            unsafe { (*root).pending.take().expect("CPU preclaim") as usize }
        } else {
            match &s.logical {
                tasks::Logical::Ordered(d) => d
                    .cpu_batch
                    .as_ref()
                    .unwrap()
                    .records
                    .iter()
                    .position(|r| {
                        r.pending_parent
                            == Some(handle(
                                parent,
                                s.tasks[parent as usize].as_ref().unwrap().generation,
                            ))
                    })
                    .unwrap(),
                _ => unreachable!(),
            }
        };
        let mut path = s.task(parent).path.clone();
        path.push(s.task(parent).spawned);
        s.task(parent).spawned += 1;
        let (child, generation) = s.take_task_slot();
        let owned = Box::new(std::cell::UnsafeCell::new(unsafe {
            TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr())
        }));
        let target = owned.get();
        unsafe { (*target).task_id = child };
        let mut row = Task::new(generation, target, Some(scope), path.clone(), None);
        row.inline_context = Some(owned);
        row.code = Some((code, environment, site));
        row.cpu_scoped = true;
        row.state = TaskState::Ready;
        s.tasks[child as usize] = Some(row);
        s.scope(scope).children.push(child);
        s.link_child(parent, child);
        (child, generation, path, ordinal)
    });
    let word = handle(child, generation);
    if let Some(probe) = unsafe { (*ctx).run_ref().cpu_probe.as_ref() } {
        let job = probe.job(word, scope, 1, ordinal as u64, &path);
        role::with_role(ctx, |s| s.task(child).cpu_job = Some(job));
    }
    if root(ctx).is_some() && engine::on(ctx) {
        let parent = engine::current(ctx);
        engine::with_driver(ctx, |d| {
            assert!(d.cpu_batch.as_mut().unwrap().bind(parent, word))
        });
    }
    let complete = if let Some(root) = root(ctx) {
        unsafe {
            (*root).children.push(word);
            (*root).children.len() == (*root).count
        }
    } else {
        let parent = engine::current(ctx);
        engine::with_driver(ctx, |d| {
            let batch = d.cpu_batch.as_mut().unwrap();
            assert!(batch.bind(parent, word));
            batch.records.iter().all(|r| r.task.is_some())
        })
    };
    unsafe { out.write(word) };
    if complete {
        unsafe { run_all(ctx, scope) }?;
    }
    Ok(crate::STATUS_OK)
}

/// # Safety
/// The parent owns every unstarted row in this CPU scope and is at head.
unsafe fn run_all(ctx: *mut TaskCtx, scope: u64) -> Stopped<()> {
    let mut index = 0;
    loop {
        let child = role::with_role(ctx, |s| {
            let scope = s.scope_handle(scope);
            s.scope(scope).children.get(index).copied()
        });
        let Some(child) = child else {
            break;
        };
        if unsafe { run_one(ctx, child) }? {
            break;
        }
        index += 1;
    }
    Ok(())
}

unsafe fn run_one(ctx: *mut TaskCtx, child: u32) -> Stopped<bool> {
    let ordered = engine::on(ctx);
    if ordered {
        engine::serial_cpu_prefix(ctx)?;
    } else {
        unsafe { (*ctx).switch_out() };
    }
    let child_key = role::with_role(ctx, |s| s.key(child).word());
    let (initial, budgets, ordinal, limit) = if let Some(root) = root(ctx) {
        let initial = if ordered {
            engine::with_driver(ctx, |d| d.frontier.as_ref().unwrap().settled)
        } else {
            unsafe {
                Settled {
                    level: (*ctx).heap_ref().meter.held(),
                    limit: match (*ctx).heap_ref().meter.limit() {
                        0 => u64::MAX,
                        n => n,
                    },
                    fuel: (*ctx).run_ref().fuel.remaining(),
                }
            }
        };
        let ordinal = unsafe {
            (*root)
                .children
                .iter()
                .position(|&key| key == child_key)
                .unwrap()
        };
        let limit = unsafe { (*ctx).run_ref().cpu_grant.as_ref().unwrap().max_batch_bytes };
        (
            initial,
            Budgets {
                fuel: initial.fuel != u64::MAX,
                memory: true,
            },
            ordinal,
            limit,
        )
    } else {
        engine::with_driver(ctx, |d| {
            let b = d.cpu_batch.as_ref().unwrap();
            (
                d.frontier.as_ref().unwrap().settled,
                d.budgets,
                b.ordinal(child_key).unwrap(),
                b.limit,
            )
        })
    };
    let (target, code, environment, site, path, probe) = role::with_role(ctx, |s| {
        let row = s.task(child);
        let (code, environment, site) = row.code.unwrap();
        (
            row.ctx,
            code,
            environment,
            site,
            row.path.clone(),
            row.cpu_job.clone(),
        )
    });
    unsafe {
        let mut local = ahead::Local::new(true);
        local.serial = Some(Box::new(std::cell::UnsafeCell::new(State {
            path,
            ordinal: ordinal as u64,
            parent: ctx,
            initial,
            settled: Settled {
                limit: initial.limit.min(limit),
                ..initial
            },
            peak: 0,
            peak_fuel: 0,
            budgets,
            issued: initial.fuel.min(crate::budget::FUEL_CHUNK),
            spent: 0,
            image: None,
            disposed: false,
            receipt: None,
            probe,
        })));
        (*target).ahead = Some(Box::new(local));
        if (*ctx).run_ref().ordered_roots.get().is_some() {
            (*target).roots.trace_creator.path = (*get(target).unwrap()).path.clone();
            (*target).ordered_charge_trace.enable();
        }
        if ordered {
            (*ctx).heap().unbind();
        }
        role::release(ctx);
        if ordered {
            TaskCtx::attach(target, (*ctx).worker_ptr());
            crate::stop::set_running(target);
        } else {
            TaskCtx::switch_in(target, (*ctx).worker_ptr());
        }
        crate::pool::cpu_scoped::probe::resume(target);
        if ordered {
            (*target)
                .heap()
                .set_fuel(initial.fuel.min(crate::budget::FUEL_CHUNK) as i64);
        } else {
            (*get(target).unwrap()).issued = (*target).view.compiled.fuel.max(0) as u64;
        }
        (*target)
            .ordered_charge_trace
            .grant((*target).view.compiled.fuel);
    }
    // Only plain values cross the ordinary generated callback. A Stop can
    // abandon this stack; the row owns all contexts, images and metadata.
    let status = unsafe { code(target, environment, site, site) };
    unsafe { restore(ctx, target, status, child) }
}

unsafe fn restore(
    ctx: *mut TaskCtx,
    target: *mut TaskCtx,
    status: u8,
    child: u32,
) -> Stopped<bool> {
    unsafe {
        crate::pool::cpu_scoped::probe::boundary(target, 0);
        (*target)
            .ordered_charge_trace
            .capture((*target).view.compiled.fuel);
    }
    let capture = unsafe { (&mut *get(target).unwrap()).capture((*target).view.compiled.fuel) };
    unsafe {
        if (*ctx)
            .run_ref()
            .ordered_mode
            .load(std::sync::atomic::Ordering::Acquire)
        {
            (*target).heap().unbind();
            TaskCtx::attach(ctx, (*target).worker_ptr());
            crate::stop::set_running(ctx);
        } else {
            (*target).switch_out();
            TaskCtx::switch_in(ctx, (*target).worker_ptr());
        }
        role::enter(ctx);
    }
    if let Err(stop) = capture {
        record_stop(ctx, target, &stop)?;
        return Err(stop);
    }
    let (mut charges, receipt, initial, settled) = unsafe {
        let state = &mut *get(target).unwrap();
        (
            state.checked_prefix(),
            state.receipt.take(),
            state.initial,
            state.settled,
        )
    };
    trace_prefix(ctx, target, &mut charges, Ok(()));
    if engine::on(ctx) {
        let child_key = role::with_role(ctx, |s| s.key(child).word());
        engine::with_driver(ctx, |d| {
            let state = &mut d.frontier.as_mut().unwrap().settled;
            assert_eq!(*state, initial);
            let batch = d.cpu_batch.as_mut().unwrap();
            batch.settle(child_key, &charges, state, true).unwrap()
        })
        .map_err(Stop::BudgetExceeded)?;
    }
    unsafe { (*ctx).heap_ref().meter.settled(settled.level) };
    if let Some(mut receipt) = receipt {
        let native = receipt.native.as_mut().unwrap();
        native.pieces = crate::io::CpuSummary::pieces(&charges);
        native.level = settled.level;
        native.limit = settled.limit;
        native.fuel = settled.fuel;
        super::record_summary(ctx, &receipt)?;
    }
    role::with_role(ctx, |s| {
        let row = s.task(child);
        row.state = TaskState::Done;
        if status != crate::STATUS_OK {
            row.failure = Some(unsafe { (*target).failure.take_chain(&row.path) });
        }
    });
    if engine::on(ctx) {
        engine::fuel_in(ctx);
    }
    Ok(status != crate::STATUS_OK)
}

/// A denied later preclaim still drains the already accepted input prefix.
/// Run it before the parent allocates its result list or observes a join.
pub(crate) fn finish_prepared(ctx: *mut TaskCtx) -> Stopped<()> {
    if unsafe { (*ctx).failure.active() } {
        return Ok(());
    }
    let scope = role::with_role(ctx, |s| {
        let parent = s.current;
        let scope = *s.task(parent).scopes.last()?;
        let row = s.scopes[scope as usize].as_ref().unwrap();
        // Calls never suspend partway through a batch. A completed child
        // means this prefix has already run, including its first failure.
        (!row.children.is_empty()
            && row.children.iter().all(|&child| {
                let child = s.tasks[child as usize].as_ref().unwrap();
                child.inline_context.is_some() && child.state == TaskState::Ready
            }))
        .then_some(handle(scope, row.generation))
    });
    if let Some(scope) = scope {
        unsafe { run_all(ctx, scope) }?;
    }
    Ok(())
}

/// # Safety
/// The parent owns this child's handle and writable Option destination.
pub(crate) unsafe fn join(
    ctx: *mut TaskCtx,
    scope: u64,
    word: u64,
    slot: *mut u8,
    size: u64,
    site: u64,
) -> Stopped<u8> {
    finish_prepared(ctx)?;
    let (scope, child) = unsafe { tasks::child_handle(ctx, scope, word) };
    let target = role::with_role(ctx, |s| s.task(child).ctx);
    if let Some(state) = get(target) {
        let image = unsafe { (*state).image.take() };
        if let Some((image, expected, _)) = image {
            assert_eq!(expected as u64, size);
            unsafe { (*state).disposed = true };
            if let Some(job) = unsafe { (*state).probe.as_ref() } {
                job.event("transfer");
            }
            unsafe { std::ptr::copy_nonoverlapping(image.as_ptr().cast::<u8>(), slot, expected) };
        }
    }
    unsafe { cleanup(ctx, child, site) }?;
    if let Some(root) = root(ctx) {
        unsafe { (*root).children.retain(|&key| key != word) };
    }
    let (chain, retired) = role::with_role(ctx, |s| {
        let chain = s.task(child).failure.take();
        s.scope(scope).children.retain(|&c| c != child);
        (chain, s.retire_task(child))
    });
    drop(retired);
    if let Some(chain) = chain {
        unsafe { (*ctx).failure.append_chain(chain) };
        Ok(crate::STATUS_FAILURE)
    } else {
        Ok(crate::STATUS_OK)
    }
}

unsafe fn cleanup(ctx: *mut TaskCtx, child: u32, site: u64) -> Stopped<()> {
    let target = role::with_role(ctx, |s| s.task(child).ctx);
    if let Some(state) = get(target) {
        let image = unsafe {
            (*state)
                .image
                .as_mut()
                .map(|(image, _, index)| (image.as_mut_ptr().cast::<u8>(), *index))
        };
        if let Some((value, index)) = image {
            if let Some(job) = unsafe { (*state).probe.as_ref() } {
                job.event("withdrawal");
            }
            let mut cleanup = std::mem::MaybeUninit::uninit();
            unsafe {
                crate::finalize::hirsel_rt_v2_finalize(
                    ctx,
                    index,
                    value,
                    std::ptr::null(),
                    cleanup.as_mut_ptr(),
                    site,
                    site,
                )
            };
            unsafe {
                (*state).image = None;
                (*state).disposed = true;
            };
        } else if !unsafe { (*state).disposed } {
            if let Some(job) = unsafe { (*state).probe.as_ref() } {
                job.event("withdrawal");
            }
            unsafe { (*state).disposed = true };
        }
    }
    let environment = role::with_role(ctx, |s| s.task(child).code.take().map(|(_, env, _)| env));
    if let Some(environment) = environment {
        unsafe {
            crate::release::release_block(&mut *ctx, environment, std::ptr::null(), site, site)
        };
    }
    Ok(())
}

/// # Safety
/// The parent owns the scope. Every direct call has returned before entry.
pub(crate) unsafe fn drain(ctx: *mut TaskCtx, scope: u64, site: u64) -> Stopped<()> {
    let index = role::with_role(ctx, |s| s.scope_handle(scope));
    loop {
        let child = role::with_role(ctx, |s| {
            s.scopes[index as usize]
                .as_ref()
                .unwrap()
                .children
                .iter()
                .rev()
                .copied()
                .find(|&child| {
                    s.tasks[child as usize]
                        .as_ref()
                        .unwrap()
                        .inline_context
                        .is_some()
                })
        });
        let Some(child) = child else {
            break;
        };
        unsafe { cleanup(ctx, child, site) }?;
        if let Some(root) = root(ctx) {
            let word = role::with_role(ctx, |s| s.key(child).word());
            unsafe { (*root).children.retain(|&key| key != word) };
        }
        let (chain, retired) = role::with_role(ctx, |s| {
            let chain = s.task(child).failure.take();
            s.scope(index).children.retain(|&c| c != child);
            (chain, s.retire_task(child))
        });
        drop(retired);
        if let Some(chain) = chain {
            let own = tasks::current_path(ctx);
            unsafe { (*ctx).failure.merge_chain(&own, chain) };
        }
    }
    Ok(())
}

/// Return a stopped direct call to its parent carrier before the ordinary
/// terminal gate runs. The scope still owns the abandoned child's storage.
/// # Safety
/// The child runs synchronously below its live parent's compiled frame.
pub(crate) unsafe fn stop_context(ctx: *mut TaskCtx, stop: &mut Stop) -> *mut TaskCtx {
    let state = unsafe { &mut *get(ctx).unwrap() };
    let parent = state.parent;
    let _ = state.capture(unsafe { (*ctx).view.compiled.fuel });
    let settled = state.settled;
    let failure = unsafe { (*ctx).failure.take_chain(&state.path) };
    let ordered = unsafe {
        (*ctx)
            .run_ref()
            .ordered_mode
            .load(std::sync::atomic::Ordering::Acquire)
    };
    unsafe {
        if ordered {
            (*ctx).heap().unbind();
            TaskCtx::attach(parent, (*ctx).worker_ptr());
            crate::stop::set_running(parent);
        } else {
            (*ctx).switch_out();
            TaskCtx::switch_in(parent, (*ctx).worker_ptr());
        }
        role::enter(parent);
    }
    unsafe { (*parent).failure.append_chain(failure) };
    if ordered {
        engine::with_driver(parent, |d| {
            let frontier = &mut d.frontier.as_mut().unwrap().settled;
            frontier.level = settled.level;
            frontier.fuel = settled.fuel;
        });
        engine::fuel_in(parent);
    }
    unsafe { (*parent).heap_ref().meter.settled(settled.level) };
    if let Err(error) = record_stop(parent, ctx, stop) {
        *stop = error;
    }
    parent
}

fn record_stop(parent: *mut TaskCtx, child: *mut TaskCtx, stop: &Stop) -> Stopped<()> {
    let Stop::BudgetExceeded(kind) = stop else {
        return Ok(());
    };
    let state = unsafe { &mut *get(child).unwrap() };
    state.receipt = None;
    let mut charges = state.checked_prefix();
    let ordinal = state.ordinal;
    trace_prefix(parent, child, &mut charges, Err(*kind));
    let receipt = crate::io::CpuSummary {
        version: 3,
        retirement: ordinal,
        ordinal,
        value: Vec::new(),
        stop: Some(*kind),
        native: None,
    };
    super::record_summary(parent, &receipt)
}

// The optional exact observer follows the live serial stream without
// inventing cooperative child turns or retaining a speculative envelope.
fn trace_prefix(
    parent: *mut TaskCtx,
    child: *mut TaskCtx,
    charges: &mut Stretch,
    result: Result<(), hirsel_run_schema::BudgetKind>,
) {
    let Some(exact) = (unsafe { (*child).ordered_charge_trace.take() }) else {
        return;
    };
    charges.exact = exact;
    let task = role::with_role(parent, |s| s.key(unsafe { (*child).task_id }).word());
    super::super::events::charges::serial(parent, task, charges, result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::stretch::{Event, reference};

    fn state(initial: Settled) -> State {
        State {
            path: vec![0],
            ordinal: 0,
            parent: std::ptr::null_mut(),
            initial,
            settled: initial,
            peak: 0,
            peak_fuel: 0,
            budgets: Budgets {
                fuel: true,
                memory: true,
            },
            issued: initial.fuel,
            spent: 0,
            image: None,
            disposed: false,
            receipt: None,
            probe: None,
        }
    }

    #[test]
    fn serial_live_checks_preserve_the_first_fuel_or_memory_stop() {
        let events = [
            Event::Fuel,
            Event::Alloc(16),
            Event::Fuel,
            Event::Free(8),
            Event::Fuel,
            Event::Alloc(24),
            Event::Fuel,
        ];
        for fuel in 0..=5 {
            for limit in [0, 8, 16, 24, 32, 64] {
                let initial = Settled {
                    level: 0,
                    limit,
                    fuel,
                };
                let mut expected = initial;
                let reference = reference(&mut expected, &events);
                let mut actual = state(initial);
                let mut word = fuel as i64;
                let mut outcome = Ok(());
                for event in events {
                    outcome = match event {
                        Event::Fuel => {
                            word -= 1;
                            actual.capture(word)
                        }
                        Event::Alloc(bytes) => actual.allocate(word, bytes),
                        Event::Free(bytes) => {
                            actual.settled.level -= bytes;
                            Ok(())
                        }
                        Event::Credit(_) => unreachable!(),
                    };
                    if outcome.is_err() {
                        break;
                    }
                }
                assert_eq!(
                    outcome,
                    reference.map_err(Stop::BudgetExceeded),
                    "fuel={fuel} limit={limit}"
                );
                assert_eq!(actual.settled, expected);
                if outcome.is_ok() {
                    let mut replayed = initial;
                    assert_eq!(replayed.settle(&actual.checked_prefix()), Ok(()));
                    assert_eq!(replayed, expected);
                }
            }
        }
    }

    #[test]
    fn unlimited_receipts_retain_the_actual_source_fuel() {
        let initial = Settled {
            level: 0,
            limit: 64,
            fuel: u64::MAX,
        };
        let mut actual = state(initial);
        actual.budgets.fuel = false;
        actual.issued = 7;
        actual.capture(3).unwrap();
        let charges = actual.checked_prefix();
        assert_eq!(charges.fuel_charges(), 4);
        let mut replayed = initial;
        replayed.settle(&charges).unwrap();
        let mut expected = initial;
        reference(&mut expected, &[Event::Fuel; 4]).unwrap();
        assert_eq!(actual.settled, expected);
        assert_eq!(replayed, expected);
    }

    #[test]
    fn a_serial_job_checks_more_highs_than_a_speculative_envelope_can_hold() {
        let initial = Settled {
            level: 0,
            limit: 4096,
            fuel: 4096,
        };
        let mut actual = state(initial);
        let mut events = Vec::new();
        for index in 0..crate::budget::stretch::ENVELOPE_MAX + 1 {
            let word = initial.fuel as i64 - index as i64 - 1;
            actual.allocate(word, 1).unwrap();
            events.extend([Event::Fuel, Event::Alloc(1)]);
        }
        let mut expected = initial;
        reference(&mut expected, &events).unwrap();
        let mut replayed = initial;
        replayed.settle(&actual.checked_prefix()).unwrap();
        assert_eq!(replayed, expected);
    }
    unsafe extern "C" fn ordinary_callback(
        ctx: *mut TaskCtx,
        environment: *mut Header,
        _: u64,
        _: u64,
    ) -> u8 {
        unsafe {
            assert!((*ctx).task_sp.is_null());
            assert!((*ctx).ahead_physical.is_null());
            assert_eq!((*ctx).role_depth, 0);
            assert!(!engine::on(ctx));
            let words = crate::alloc::payload(environment).cast::<[u64; 2]>();
            let [value, mode] = words.read();
            assert_eq!(value, 42, "a callback must never restart");
            assert_ne!(mode, 2, "a younger job ran after the first failure");
            words.write([43, mode]);
            if mode == 1 {
                return crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, 0, 0, 0);
            }
            let mut result = [1u64, value];
            let none = [0u64; 2];
            let status = tasks::hirsel_rt_v2_task_finish(
                ctx,
                result.as_mut_ptr().cast(),
                none.as_ptr().cast(),
                16,
                2,
            );
            assert_eq!(result, none, "TaskFinish transfers the owned Option image");
            status
        }
    }

    #[test]
    fn ordinary_caller_jobs_transfer_results_and_never_restart_after_failure() {
        for failure_first in [false, true] {
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
            let mut run = crate::host::BodyRun::new(1, false);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            unsafe { run.use_ordered_home_dispatch() };
            let outcome = run.run(
                |ctx| unsafe {
                    let mut record = hirsel_run_config::RunConfig::empty(
                        [0; 32],
                        "serial@1/main".into(),
                        hirsel_run_schema::LaunchKind::Main,
                    );
                    record.io = Some(hirsel_run_schema::IoGrants {
                        tasks: Some("tasks.cooperative@1".into()),
                        ..Default::default()
                    });
                    (*ctx).run().deferred_ordered =
                        super::super::super::admission::Admission::prepare(Some(
                            &record.validated().unwrap(),
                        ))
                        .unwrap();
                    (*ctx).run().cpu_grant = Some(hirsel_run_schema::CpuGrant {
                        provider: "cpu.scoped@1".into(),
                        max_workers: 1,
                        max_domains: 1,
                        max_physical_bytes: 256 << 20,
                        max_batch_bytes: 8 << 20,
                    });
                },
                move |ctx| unsafe {
                    let mut status = 0;
                    let tasks_word = 0;
                    let mut scope = 0;
                    let mut handles = [0u64; 2];
                    let control = |action, count, status: &mut u32| {
                        assert_eq!(
                            crate::pool::cpu_scoped::hirsel_rt_v2_cpu_map_scoped(
                                ctx,
                                status,
                                &tasks_word,
                                1,
                                action,
                                count,
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!(*status, 0);
                    };
                    control(4, 0, &mut status);
                    assert_eq!(
                        tasks::hirsel_rt_v2_task_scope_enter(
                            ctx,
                            std::ptr::null_mut(),
                            &mut scope,
                            &tasks_word,
                        ),
                        crate::STATUS_OK
                    );
                    control(2, 2, &mut status);
                    let modes = if failure_first { [1, 2] } else { [0, 1] };
                    for (ordinal, mode) in modes.into_iter().enumerate() {
                        control(5, ordinal as u64, &mut status);
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
                            .write([42, mode]);
                        assert_eq!(
                            tasks::hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut handles[ordinal],
                                &scope,
                                ordinary_callback,
                                environment,
                                0,
                            ),
                            crate::STATUS_OK
                        );
                    }
                    let mut result = [0u64; 2];
                    if !failure_first {
                        assert_eq!(
                            tasks::hirsel_rt_v2_task_join(
                                ctx,
                                0,
                                0,
                                &scope,
                                &mut handles[0],
                                result.as_mut_ptr().cast(),
                                16,
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!(result, [1, 42]);
                        result = [0; 2];
                    }
                    let failed = usize::from(!failure_first);
                    assert_eq!(
                        tasks::hirsel_rt_v2_task_join(
                            ctx,
                            0,
                            0,
                            &scope,
                            &mut handles[failed],
                            result.as_mut_ptr().cast(),
                            16,
                        ),
                        crate::STATUS_FAILURE
                    );
                    assert!((*ctx).failure.active());
                    assert_eq!(
                        tasks::hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 1),
                        crate::STATUS_FAILURE
                    );
                    crate::failure::hirsel_rt_v2_failure_clear(ctx);
                    control(1, 0, &mut status);
                    assert!(!engine::on(ctx));
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }
}

/// The sole caller needs no Ordered image to execute a closed CPU batch.
/// Ordinary Tasks operations still activate the admitted scheduler on demand.
pub(crate) struct Root {
    count: usize,
    pending: Option<u64>,
    children: Vec<u64>,
    pub(crate) scope: Option<u64>,
}

pub(crate) fn root(ctx: *mut TaskCtx) -> Option<*mut Root> {
    unsafe {
        (*ctx)
            .ahead
            .as_ref()?
            .serial_batch
            .as_ref()
            .map(|s| s.get())
    }
}

pub(crate) fn root_start(ctx: *mut TaskCtx, width: u32, action: u32) -> bool {
    root(ctx).is_some()
        || (width == 1
            && action == 4
            && !engine::on(ctx)
            && unsafe { (*ctx).run_ref().deferred_ordered.is_some() })
}

pub(crate) fn control(
    ctx: *mut TaskCtx,
    width: u32,
    action: u32,
    count: u64,
    live: impl FnOnce() -> u32,
) -> Stopped<u32> {
    let value = if engine::on(ctx) {
        super::control(ctx, width, action, count, live)?
    } else {
        match super::receipt(ctx, width, action, count, None)? {
            Some(value) => value,
            None => {
                let value = live();
                super::receipt(ctx, width, action, count, Some(value))?;
                value
            }
        }
    };
    if action == 4 && value == 0 {
        unsafe {
            let local = (*ctx)
                .ahead
                .get_or_insert_with(|| Box::new(ahead::Local::new(true)));
            assert!(local.serial_batch.is_none());
            local.serial_batch = Some(Box::new(std::cell::UnsafeCell::new(Root {
                count: 0,
                pending: None,
                children: Vec::new(),
                scope: None,
            })));
            enable_meter(ctx);
        }
    } else if action == 1 {
        if let Some(root) = root(ctx) {
            assert!(unsafe { (*root).children.is_empty() });
        }
        unsafe { (*ctx).ahead.as_mut().unwrap().serial_batch = None };
    } else if value == 0
        && let Some(root) = root(ctx)
    {
        match action {
            2 => unsafe { (*root).count = usize::try_from(count).expect("bounded CPU count") },
            5 => unsafe { assert!((*root).pending.replace(count).is_none()) },
            _ => {}
        }
    }
    if action == 5 && value != 0 {
        finish_prepared(ctx)?;
    }
    Ok(value)
}

pub(crate) unsafe fn scope_enter(ctx: *mut TaskCtx, out: *mut u64) -> u8 {
    let _role = unsafe { role::Guard::new(ctx) };
    let (slot, generation) = tasks::open_scope(ctx, None);
    let word = handle(slot, generation);
    unsafe {
        (*root(ctx).unwrap()).scope = Some(word);
        out.write(word)
    };
    crate::STATUS_OK
}

pub(crate) fn owns_scope(ctx: *mut TaskCtx, word: u64) -> bool {
    root(ctx).is_some_and(|root| unsafe { (*root).scope == Some(word) })
}

pub(crate) unsafe fn scope_exit(ctx: *mut TaskCtx, scope: u64, site: u64) -> Stopped<u8> {
    let _role = unsafe { role::Guard::new(ctx) };
    unsafe { drain(ctx, scope, site) }?;
    role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        let parent = s.current;
        assert_eq!(s.task(parent).scopes.pop(), Some(scope));
        assert!(s.scope(scope).children.is_empty());
        s.take_scope(scope);
        s.cancel_epoch.changed();
    });
    unsafe { (*root(ctx).unwrap()).scope = None };
    Ok(if unsafe { (*ctx).failure.active() } {
        crate::STATUS_FAILURE
    } else {
        crate::STATUS_OK
    })
}
