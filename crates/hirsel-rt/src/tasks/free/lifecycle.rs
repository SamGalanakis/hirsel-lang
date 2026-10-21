//! Native Free task-tree boundaries. Allocation and identity reservation
//! precede a phase; its publication installs the observable owner topology.

use super::super::*;

/// # Safety
/// The role is held; handles and the owned closure satisfy TaskSpawn's ABI.
pub(crate) unsafe fn spawn(
    ctx: *mut TaskCtx,
    out: *mut u64,
    scope: u64,
    code: TaskCode,
    environment: *mut Header,
    operation_site: u64,
) -> Stopped<()> {
    super::record::begin(ctx);
    let started = unsafe { (*ctx).run_ref() }
        .worker_metrics
        .as_ref()
        .map(|_| std::time::Instant::now());
    let s = scheduler(ctx) as *mut Scheduler;
    let (scope, current, generation, index, path, canceled) = unsafe {
        let s = &mut *s;
        let scope = s.scope_handle(scope);
        let current = s.current;
        let canceled = s.canceled((*ctx).shield).map(|_| CANCEL_PARENT);
        let parent = s.task(current);
        let mut path = parent.path.clone();
        path.push(parent.spawned);
        parent.children.reserve(1);
        s.scope(scope).children.reserve(1);
        let generation = s.fresh_generation();
        let index = s.vacant_tasks.pop().unwrap_or_else(|| {
            s.tasks.push(None);
            (s.tasks.len() - 1) as u32
        });
        (scope, current, generation, index, path, canceled)
    };
    let stack = unsafe { (*ctx).worker() }
        .idle_stacks
        .pop()
        .unwrap_or_else(Stack::new);
    let child_ctx = stack.context();
    unsafe {
        child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
        (*child_ctx).task_id = index;
        (*child_ctx).task_sp =
            stack.prepare(task_entry, child_ctx.cast(), index as usize as *mut u8);
    }
    let mut task = Task::new(generation, child_ctx, Some(scope), path, canceled);
    task.code = Some((code, environment, operation_site));
    task.stack = Some(stack);
    let log = super::record::prepare_child(ctx);
    super::record::spawn(ctx, log, |log| unsafe {
        if let Some(log) = &log {
            assert_eq!(log.path(), task.path);
        }
        (*child_ctx).recording = log;
        let s = &mut *s;
        let parent = s.task(current);
        parent.spawned = parent
            .spawned
            .checked_add(1)
            .expect("spawn path ordinal overflow");
        assert!(s.tasks[index as usize].is_none());
        s.tasks[index as usize] = Some(task);
        s.scope(scope).children.push(index);
        s.link_child(current, index);
        out.write(handle(index, generation));
    })?;
    if let (Some(started), Some(metrics)) =
        (started, unsafe { (*ctx).run_ref() }.worker_metrics.as_ref())
    {
        metrics.spawn(started.elapsed());
    }
    unsafe {
        crate::pool::free::schedule_saved((*ctx).run_ptr(), index, Owner::Any);
        try_yield(&mut *ctx)
    }
}

/// # Safety
/// The role is held and out is writable for the scope word.
pub(crate) unsafe fn scope_enter(ctx: *mut TaskCtx, out: *mut u64) -> Stopped<()> {
    super::record::begin(ctx);
    let s = scheduler(ctx) as *mut Scheduler;
    let (index, generation, current) = unsafe {
        let s = &mut *s;
        let generation = s.fresh_generation();
        let index = match s.vacant_scopes.pop() {
            Some(index) => index,
            None => {
                s.scopes.push(None);
                (s.scopes.len() - 1) as u32
            }
        };
        let current = s.current;
        s.task(current).scopes.reserve(1);
        (index, generation, current)
    };
    let row = Scope {
        generation,
        owner: current,
        children: Vec::new(),
        canceled: Default::default(),
        deadline: None,
        completions: AtomicU64::new(0),
    };
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        let s = &mut *s;
        assert!(s.scopes[index as usize].is_none());
        s.scopes[index as usize] = Some(row);
        s.task(current).scopes.push(index);
        s.cancel_epoch.changed();
        out.write(handle(index, generation));
    })?;
    unsafe { try_yield(&mut *ctx) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::free::record;

    #[test]
    fn a_spawn_after_the_terminal_cut_cannot_publish_a_handle_or_start_a_child() {
        unsafe extern "C" fn never(_: *mut TaskCtx, _: *mut Header, _: u64, _: u64) -> u8 {
            panic!("a spawn rejected at the terminal cut cannot start");
        }
        let recording = record::Recording::new(b"native spawn terminal", false);
        let install = recording.clone();
        let cutter = recording.clone();
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let mut scope = 0;
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                let cut = cutter.recorder.stop("cut", None).unwrap().unwrap().0;
                let _role = role::Guard::new(ctx);
                let mut out = u64::MAX;
                assert!(spawn(ctx, &mut out, scope, never, std::ptr::null_mut(), 0).is_err());
                assert_eq!(out, u64::MAX);
                let s = scheduler(ctx);
                let index = s.scope_handle(scope);
                assert!(s.scope(index).children.is_empty());
                assert!(s.task(ROOT).children.is_empty());
                assert_eq!(s.task(ROOT).spawned, 0);
                assert!(s.tasks.iter().skip(1).all(Option::is_none));
                assert_eq!(cut.progress.len(), 1);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

    #[test]
    fn a_scope_cannot_join_its_owners_topology_after_a_terminal_cut() {
        let recording = record::Recording::new(b"native scope terminal", false);
        let install = recording.clone();
        let cutter = recording.clone();
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let cut = cutter.recorder.stop("cut", None).unwrap().unwrap().0;
                let _role = role::Guard::new(ctx);
                let mut out = u64::MAX;
                assert!(scope_enter(ctx, &mut out).is_err());
                assert_eq!(out, u64::MAX);
                assert!(scheduler(ctx).task(ROOT).scopes.is_empty());
                assert!(scheduler(ctx).scopes.iter().all(Option::is_none));
                assert_eq!(cut.progress[&Vec::new()].op, 0);
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }
}
