//! Owner-local task sets still have semantic ordinals and phase publication.
//! Completion ranks belong to scopes, so creation and insertion may happen
//! after a child has finished without changing the winner of await_any.

use super::super::*;

#[cfg(test)]
mod tests;

/// # Safety
/// The role is held, scope is live, and out is writable.
pub(crate) unsafe fn create(ctx: *mut TaskCtx, scope: u64, out: *mut u64) -> Stopped<()> {
    super::record::begin(ctx);
    let s = scheduler(ctx) as *mut Scheduler;
    let scope = unsafe { (*s).scope_handle(scope) };
    let generation = unsafe { (*s).fresh_generation() } & 0x3fff_ffff;
    let index = unsafe { (*s).vacant_sets.pop() }.unwrap_or_else(|| unsafe {
        (*s).sets.push(None);
        ((*s).sets.len() - 1) as u32
    });
    let row = TaskSet {
        generation,
        scope,
        members: Vec::new(),
    };
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        assert!((&(*s).sets)[index as usize].is_none());
        (&mut (*s).sets)[index as usize] = Some(row);
        out.write(
            crate::channels::ENDPOINT | TASK_SET | u64::from(generation) << 32 | u64::from(index),
        );
    })?;
    unsafe { try_yield(&mut *ctx) }
}

fn set(ctx: *mut TaskCtx, scope: u64, word: u64) -> u32 {
    let s = scheduler(ctx);
    let scope = s.scope_handle(scope);
    s.set_handle(word, scope)
}

/// # Safety
/// The role is held and the task/set handles belong to the live scope.
pub(crate) unsafe fn add(ctx: *mut TaskCtx, scope: u64, word: u64, task: u64) -> Stopped<()> {
    super::record::begin(ctx);
    let s = scheduler(ctx) as *mut Scheduler;
    let scope = unsafe { (*s).scope_handle(scope) };
    unsafe { (*s).task_handle(task, scope) };
    let set = unsafe { (*s).set_handle(word, scope) };
    unsafe { (*s).set(set).members.reserve(1) };
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        (*s).set(set).members.push(Some(task))
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
    let s = scheduler(ctx) as *mut Scheduler;
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        let has = usize::try_from(index)
            .ok()
            .and_then(|index| (*s).set(set).members.get(index))
            .is_some_and(Option::is_some);
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
    let s = scheduler(ctx) as *mut Scheduler;
    super::record::publish(ctx, Phase::Done, |_| unsafe {
        let task = usize::try_from(index)
            .ok()
            .and_then(|index| (*s).set(set).members.get_mut(index))
            .and_then(Option::take)
            .expect("an unawaited task set member");
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
    let waiting = scheduler(ctx).set(set).members.iter().any(Option::is_some);
    let index = if waiting {
        let mut entered = false;
        loop {
            if let Some(index) = scheduler(ctx).first_completed(set) {
                break Some(index);
            }
            if !entered {
                super::record::publish(ctx, Phase::Entered, |_| ())?;
                entered = true;
            }
            unsafe {
                try_block(
                    &mut *ctx,
                    Wait::Any { set, member: None },
                    false,
                    caller_site,
                )
            }?;
        }
    } else {
        None
    };
    super::record::publish(ctx, Phase::Done, |writer| unsafe {
        if let (Some(writer), Some(index)) = (writer, index) {
            writer.await_any(index);
        }
        out.write(IndexOption {
            tag: u32::from(index.is_some()),
            padding: 0,
            index: index.unwrap_or(0),
        });
    })?;
    unsafe { try_yield(&mut *ctx) }
}

/// # Safety
/// The role is held and word is a live task-set endpoint owned by this task.
pub(crate) unsafe fn close(ctx: *mut TaskCtx, word: u64) -> Stopped<()> {
    super::record::begin(ctx);
    let s = scheduler(ctx) as *mut Scheduler;
    let index = word as u32;
    let row = unsafe {
        (&(*s).sets)[index as usize]
            .as_ref()
            .expect("a live task set")
    };
    assert_eq!(
        row.generation,
        ((word & !(crate::channels::ENDPOINT | TASK_SET)) >> 32) as u32
    );
    unsafe { (*s).vacant_sets.reserve(1) };
    let row = super::record::publish(ctx, Phase::Done, |_| unsafe {
        let row = (&mut (*s).sets)[index as usize].take().unwrap();
        (*s).vacant_sets.push(index);
        row
    })?;
    drop(row);
    Ok(())
}
