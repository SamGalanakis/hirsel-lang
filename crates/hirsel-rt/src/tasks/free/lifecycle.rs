//! Native Free task-tree boundaries. Allocation and identity reservation
//! precede a phase; its publication installs the observable owner topology.

use super::super::*;

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
