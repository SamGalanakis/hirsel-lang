//! Installation and entry yield owned by a certified scope invocation.
use super::*;
use crate::continuation::binding::{self, BoundSite};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Begin,
    Prepare,
    Install,
    Arm,
    Publish,
    Yield,
    Complete,
}

pub(in crate::tasks) struct Entry {
    binding: BoundSite,
    phase: Stage,
    deadline: Option<[u64; 2]>,
    scope: Option<(u32, u32, Scope)>,
    installed: Option<(u32, u32)>,
    token: Option<u64>,
}

/// The output is reconstructed on each drive from its checked frame place;
/// no address into an abandoned native stack is retained.
pub(super) unsafe fn poll(
    ctx: *mut TaskCtx,
    out: *mut u64,
    deadline: Option<[u64; 2]>,
    site: BoundSite,
) -> Stopped<u8> {
    let result = unsafe { drive(ctx, out, deadline, site) };
    match result {
        Err(stop)
            if super::super::replay::cut_rejection(ctx, &stop)
                && unsafe { binding::can_leave(ctx, site) }
                && compact::contains_frame(ctx, out as usize, 8) =>
        {
            unsafe { park_left_at(ctx, Wait::Final, site.anchor) }?;
            compact::wait(ctx);
            Ok(crate::STATUS_SUSPENDED)
        }
        result => result,
    }
}

unsafe fn drive(
    ctx: *mut TaskCtx,
    out: *mut u64,
    deadline: Option<[u64; 2]>,
    site: BoundSite,
) -> Stopped<u8> {
    role::with_role(ctx, |s| {
        let current = s.current;
        let row = s.task(current);
        if row
            .scope_entries
            .last()
            .is_some_and(|op| op.binding == site)
        {
            assert_eq!(row.scope_entries.last().unwrap().deadline, deadline);
        } else {
            row.scope_entries.push(Entry {
                binding: site,
                phase: Stage::Begin,
                deadline,
                scope: None,
                installed: None,
                token: None,
            });
        }
    });
    let _ = compact::waited(ctx);
    loop {
        let phase = inspect(ctx, site, |_, op| op.phase);
        let leave = unsafe { binding::can_leave(ctx, site) }
            && compact::contains_frame(ctx, out as usize, 8);
        match phase {
            Stage::Begin => {
                if leave {
                    if !super::super::record::begin_entered(ctx) {
                        unsafe { park_left(ctx, Wait::Final) }?;
                        compact::wait(ctx);
                        return Ok(crate::STATUS_SUSPENDED);
                    }
                } else {
                    super::super::record::begin(ctx);
                }
                inspect(ctx, site, |_, op| op.phase = Stage::Prepare);
            }
            Stage::Prepare => {
                let row = prepare_scope(
                    ctx,
                    deadline.map(|_| ScopeDeadline {
                        token: None,
                        expired: false,
                    }),
                );
                inspect(ctx, site, |_, op| {
                    op.scope = Some(row);
                    op.phase = Stage::Install;
                });
            }
            Stage::Install => {
                // Plain entry combines installation and output in Done.
                if deadline.is_none() {
                    if publish(ctx, leave, Phase::Done, |_| unsafe {
                        role::with_held(ctx, |s| {
                            with(s, site, |s, op| {
                                let (index, current, row) =
                                    op.scope.take().expect("prepared scope");
                                let generation = row.generation;
                                install_scope(s, index, current, row);
                                out.write(handle(index, generation));
                                op.installed = Some((index, generation));
                                op.phase = Stage::Yield;
                            })
                        })
                    })?
                    .is_none()
                    {
                        return Ok(crate::STATUS_SUSPENDED);
                    }
                } else {
                    if publish(ctx, leave, Phase::Entered, |_| unsafe {
                        role::with_held(ctx, |s| {
                            with(s, site, |s, op| {
                                let (index, current, row) =
                                    op.scope.take().expect("prepared deadline scope");
                                let generation = row.generation;
                                install_scope(s, index, current, row);
                                op.installed = Some((index, generation));
                                op.phase = Stage::Arm;
                            })
                        })
                    })?
                    .is_none()
                    {
                        return Ok(crate::STATUS_SUSPENDED);
                    }
                }
            }
            Stage::Arm => {
                let (index, generation) = inspect(ctx, site, |_, op| op.installed.unwrap());
                let [clock, at] = deadline.expect("deadline entry");
                let token = if super::super::replay::on(ctx) {
                    None
                } else {
                    unsafe { crate::host_waits::arm(ctx, index, generation, clock, at) }?
                };
                inspect(ctx, site, |_, op| {
                    op.token = token;
                    op.phase = Stage::Publish;
                });
            }
            Stage::Publish => {
                let replay = super::super::replay::on(ctx);
                if publish(ctx, leave, Phase::Done, |_| unsafe {
                    role::with_held(ctx, |s| {
                        with(s, site, |s, op| {
                            let (index, generation) = op.installed.unwrap();
                            match op.token {
                                Some(token) => {
                                    s.scope(index).deadline.as_mut().unwrap().token = Some(token)
                                }
                                None if !replay => s.expire(index),
                                None => {}
                            }
                            out.write(handle(index, generation));
                            op.phase = Stage::Yield;
                        })
                    })
                })?
                .is_none()
                {
                    return Ok(crate::STATUS_SUSPENDED);
                }
            }
            Stage::Yield => {
                let pending = unsafe { super::super::try_yield_entered(&mut *ctx, leave) }?;
                inspect(ctx, site, |_, op| op.phase = Stage::Complete);
                if pending {
                    return Ok(crate::STATUS_SUSPENDED);
                }
            }
            Stage::Complete => {
                role::with_role(ctx, |s| {
                    let current = s.current;
                    let op = s
                        .task(current)
                        .scope_entries
                        .pop()
                        .expect("completed scope entry");
                    assert_eq!(op.binding, site);
                });
                unsafe { binding::complete_bound(ctx, site) };
                return Ok(STATUS_OK);
            }
        }
    }
}

fn with<R>(
    s: &mut Scheduler,
    site: BoundSite,
    action: impl FnOnce(&mut Scheduler, &mut Entry) -> R,
) -> R {
    assert_eq!(s.key(s.current), site.task);
    let current = s.current;
    let mut op = s
        .task(current)
        .scope_entries
        .pop()
        .expect("owned scope entry");
    assert_eq!(op.binding, site);
    let result = action(s, &mut op);
    s.task(current).scope_entries.push(op);
    result
}

fn inspect<R>(
    ctx: *mut TaskCtx,
    site: BoundSite,
    action: impl FnOnce(&mut Scheduler, &mut Entry) -> R,
) -> R {
    role::with_role(ctx, |s| with(s, site, action))
}

fn publish<T>(
    ctx: *mut TaskCtx,
    leave: bool,
    phase: Phase,
    action: impl FnOnce(Option<&mut hirsel_free::concurrent::Writer<'_, '_>>) -> T,
) -> Stopped<Option<T>> {
    if leave {
        super::super::record::poll_publish(ctx, phase, action)
    } else {
        super::super::record::publish(ctx, phase, action).map(Some)
    }
}
