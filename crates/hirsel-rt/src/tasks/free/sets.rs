//! Owner-local task sets still have semantic ordinals and phase publication.
//! Completion ranks belong to scopes, so creation and insertion may happen
//! after a child has finished without changing the winner of await_any.

use super::super::*;

#[cfg(test)]
mod release_tests;
#[cfg(test)]
mod tests;

/// # Safety
/// The role is held, scope is live, and out is writable.
pub(crate) unsafe fn create(ctx: *mut TaskCtx, scope: u64, out: *mut u64) -> Stopped<()> {
    super::record::begin(ctx);
    let (scope, (index, generation)) = role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        (scope, s.take_set_slot())
    });
    let row = TaskSet {
        generation,
        scope,
        members: Vec::new(),
    };
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        role::with_held(ctx, |s| {
            assert!(s.sets[index as usize].is_none());
            s.sets[index as usize] = Some(row);
        });
        out.write(
            crate::channels::ENDPOINT | TASK_SET | u64::from(generation) << 32 | u64::from(index),
        );
    })?;
    unsafe { try_yield(&mut *ctx) }
}

fn set(ctx: *mut TaskCtx, scope: u64, word: u64) -> u32 {
    role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        s.set_handle(word, scope)
    })
}

/// # Safety
/// The role is held and the task/set handles belong to the live scope.
pub(crate) unsafe fn add(ctx: *mut TaskCtx, scope: u64, word: u64, task: u64) -> Stopped<()> {
    super::record::begin(ctx);
    let set = role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        s.task_handle(task, scope);
        let set = s.set_handle(word, scope);
        s.set(set).members.reserve(1);
        set
    });
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        role::with_held(ctx, |s| s.set(set).members.push(Some(task)))
    })?;
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// The role is held, the handles are live, and out is writable.
pub(crate) unsafe fn has(
    ctx: *mut TaskCtx,
    scope: u64,
    word: u64,
    index: u64,
    out: *mut u8,
) -> Stopped<()> {
    super::record::begin(ctx);
    let set = set(ctx, scope, word);
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        let has = role::with_held(ctx, |s| {
            usize::try_from(index)
                .ok()
                .and_then(|index| s.set(set).members.get(index))
                .is_some_and(Option::is_some)
        });
        out.write(u8::from(has));
    })?;
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// The role is held, index is an unawaited member, and out is writable.
pub(crate) unsafe fn take(
    ctx: *mut TaskCtx,
    scope: u64,
    word: u64,
    index: u64,
    out: *mut u64,
) -> Stopped<()> {
    super::record::begin(ctx);
    let set = set(ctx, scope, word);
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        let task = role::with_held(ctx, |s| {
            usize::try_from(index)
                .ok()
                .and_then(|index| s.set(set).members.get_mut(index))
                .and_then(Option::take)
                .expect("an unawaited task set member")
        });
        out.write(task);
    })?;
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// The role is held, handles are live, and out is writable.
pub(crate) unsafe fn await_any(
    ctx: *mut TaskCtx,
    scope: u64,
    word: u64,
    out: *mut IndexOption,
    caller_site: u64,
) -> Stopped<()> {
    super::record::begin(ctx);
    let set = set(ctx, scope, word);
    let waiting =
        unsafe { role::with_held(ctx, |s| s.set(set).members.iter().any(Option::is_some)) };
    let member = if waiting {
        super::replay::token(ctx)
            .map(|token| token.await_member(ctx))
            .transpose()?
    } else {
        None
    };
    if let Some(index) = member
        && usize::try_from(index).ok().is_none_or(|index| unsafe {
            role::with_held(ctx, |s| {
                s.set(set).members.get(index).is_none_or(Option::is_none)
            })
        })
    {
        return Err(crate::stop::Stop::Diverged(
            "a recorded await_any member is absent from its set".into(),
        ));
    }
    let index = if waiting {
        let mut entered = false;
        loop {
            let completed = role::with_role(ctx, |s| match member {
                Some(index) => s.member_completed(set, index).then_some(index),
                None => s.first_completed(set),
            });
            if let Some(index) = completed {
                break Some(index);
            }
            if !entered {
                super::record::publish(ctx, Phase::Entered, |_| ())?;
                entered = true;
            }
            unsafe { try_block(&mut *ctx, Wait::Any { set, member }, false, caller_site) }?;
        }
    } else {
        None
    };
    let replay = super::replay::token(ctx);
    let publication = super::record::publish(ctx, Phase::Done, |writer| unsafe {
        if let (Some(writer), Some(index)) = (writer, index) {
            writer.await_any(index);
        } else if let (Some(replay), Some(index)) = (&replay, index) {
            replay.await_any(index)?;
        }
        out.write(IndexOption {
            tag: u32::from(index.is_some()),
            padding: 0,
            index: index.unwrap_or(0),
        });
        Ok(())
    });
    drop(replay);
    publication??;
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// The role is held and word is a live task-set endpoint owned by this task.
pub(crate) unsafe fn close(ctx: *mut TaskCtx, word: u64) -> Stopped<()> {
    super::record::begin(ctx);
    let index = word as u32;
    role::with_role(ctx, |s| {
        let row = s.sets[index as usize].as_ref().expect("a live task set");
        assert_eq!(
            row.generation,
            ((word & !(crate::channels::ENDPOINT | TASK_SET)) >> 32) as u32
        );
        s.set_slots.reserve(1);
    });
    let row = super::record::publish(ctx, Phase::Done, |_| unsafe {
        role::with_held(ctx, |s| s.take_set(index))
    })?;
    drop(row);
    Ok(())
}

/// Reserve linked-child promotion and row recycling before the Done section.
/// The simulation follows the same unlink/promote order as retire_task (the
/// Tokio cancellation-token tree structure), including a parent retired before
/// one of its descendants. No real parent link changes during preparation.
fn prepare_retirement(s: &mut Scheduler, retiring: &[u32]) {
    if retiring.is_empty() {
        return;
    }
    s.reserve_retirements(retiring.len());
    let mut parents: Vec<_> = s
        .tasks
        .iter()
        .map(|row| {
            row.as_ref()
                .and_then(|row| row.parent.map(|(parent, _)| parent))
        })
        .collect();
    let mut links: Vec<_> = s
        .tasks
        .iter()
        .map(|row| {
            row.as_ref()
                .map_or_else(Vec::new, |row| row.children.clone())
        })
        .collect();
    let mut maximum: Vec<_> = links.iter().map(Vec::len).collect();
    for &child in retiring {
        if let Some(parent) = parents[child as usize] {
            let promoted = std::mem::take(&mut links[child as usize]);
            links[parent as usize].retain(|&id| id != child);
            for &id in &promoted {
                parents[id as usize] = Some(parent);
            }
            links[parent as usize].extend(promoted);
            maximum[parent as usize] = maximum[parent as usize].max(links[parent as usize].len());
        }
    }
    for (index, maximum) in maximum.into_iter().enumerate() {
        if let Some(row) = s.tasks[index].as_mut() {
            row.children.reserve(maximum - row.children.len());
        }
    }
}

/// # Safety
/// As for TaskDetachAll/TaskCancelAll. The caller owns the set and every
/// unawaited handle; no table reference survives a call to a child rest.
pub(crate) unsafe fn release(
    ctx: *mut TaskCtx,
    scope: u64,
    word: u64,
    cancel: bool,
    caller_site: u64,
) -> Stopped<()> {
    super::record::begin(ctx);
    let (scope, set, members) = role::with_role(ctx, |s| {
        let scope = s.scope_handle(scope);
        let set = s.set_handle(word, scope);
        let members: Vec<_> = s.set(set).members.iter().flatten().copied().collect();
        if members.is_empty() {
            s.set_slots.reserve(1);
        }
        (scope, set, members)
    });
    if members.is_empty() {
        let row = super::record::publish(ctx, Phase::Done, |_| unsafe {
            role::with_held(ctx, |s| s.take_set(set))
        })?;
        drop(row);
        return unsafe { try_yield(&mut *ctx) };
    }
    let (children, cancellations) = unsafe {
        role::with_held(ctx, |s| {
            let children: Vec<_> = members
                .iter()
                .map(|&word| s.task_handle(word, scope))
                .collect();
            let cancellations: Vec<_> = if cancel {
                children
                    .iter()
                    .map(|&child| (child, s.prepare_cancel(child)))
                    .collect()
            } else {
                Vec::new()
            };
            let tasks = s.tasks.len();
            s.woken.reserve(tasks);
            (children, cancellations)
        })
    };
    super::record::publish(ctx, Phase::Entered, |_| unsafe {
        role::with_held(ctx, |s| {
            for (child, targets) in &cancellations {
                s.cancel_prepared(*child, CANCEL_REQUESTED, targets);
            }
            for &child in &children {
                s.task(child).detached = true;
            }
        })
    })?;
    drop(cancellations);
    drop(children);
    for &word in &members {
        // A preceding called rest may have completed/vacated another member.
        // Native handles are generation-qualified; never reuse its old index.
        let holding = role::with_role(ctx, |s| {
            let child = s.member(word)?;
            (readiness::saved(s, child) && s.task(child).state == State::Holding).then(|| {
                s.task(child).slot = None;
                child
            })
        });
        if let Some(child) = holding {
            unsafe { hand_off(&mut *ctx, child) };
        }
    }
    if cancel && !role::with_role(ctx, |s| unsafe { poll_drained(s, set) }) {
        unsafe { try_block(&mut *ctx, Wait::Drain { set }, false, caller_site) }?;
    }
    let retiring = role::with_role(ctx, |s| {
        let retiring: Vec<_> = members
            .iter()
            .filter_map(|&word| {
                let child = s.member(word)?;
                let row = s.task(child);
                (row.state == State::Done && row.failure.is_none() && readiness::saved(s, child))
                    .then_some(child)
            })
            .collect();
        prepare_retirement(s, &retiring);
        s.set_slots.reserve(1);
        retiring
    });
    let mut retired = Vec::with_capacity(retiring.len());
    let set_row = super::record::publish(ctx, Phase::Done, |_| unsafe {
        role::with_held(ctx, |s| {
            for &child in &retiring {
                s.scope(scope).children.retain(|&id| id != child);
                retired.push(s.retire_task(child));
            }
            s.take_set(set)
        })
    })?;
    drop(set_row);
    drop(retired);
    drop(retiring);
    drop(members);
    unsafe { try_yield(&mut *ctx) }
}
