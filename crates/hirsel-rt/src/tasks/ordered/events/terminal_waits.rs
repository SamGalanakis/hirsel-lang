//! Complete known saved prefixes at the selected global stop, after workers
//! have left. Keep canonical() strict: abandonment is an explicit observation,
//! never a fabricated result or a blanket exception for unfinished operations.
use super::*;
use crate::tasks::{Scheduler, State, TASK_SET, Wait, handle, readiness};

pub(super) fn awaits(s: &Scheduler, log: &Log) -> Result<Subevents, &'static str> {
    log.awaiting
        .iter()
        .map(|(path, pending)| {
            let (index, row) = s
                .tasks
                .iter()
                .enumerate()
                .filter_map(|(index, row)| row.as_ref().map(|row| (index, row)))
                .find(|(_, row)| &row.path == path)
                .ok_or("missing terminal await owner")?;
            if !readiness::saved(s, index as u32) {
                return Err("terminal await owner is not saved");
            }
            let set = pending.set_word as u32;
            match row.wait {
                Some(Wait::Any { set: actual, .. }) if actual == set => {}
                // The exact published await owns its continuation across the
                // initial Yield, before installing Any, and after a readiness
                // wake, before observing Selected. Neither path returns a
                // result; pending evidence and the latest ordinal prove that.
                None if row.state == State::Ready => {}
                _ => return Err("terminal await owner has no matching saved continuation"),
            }
            let set_row = s
                .sets
                .get(set as usize)
                .and_then(Option::as_ref)
                .ok_or("missing terminal await set")?;
            let set_word = crate::channels::ENDPOINT | TASK_SET | handle(set, set_row.generation);
            let scope_row = s
                .scopes
                .get(set_row.scope as usize)
                .and_then(Option::as_ref)
                .ok_or("missing terminal await scope")?;
            let scope_word = handle(set_row.scope, scope_row.generation);
            if set_word != pending.set_word
                || scope_word != pending.scope_word
                || log.sets.get(&set_word) != Some(&pending.set)
                || log.scopes.get(&scope_word) != Some(&pending.scope)
            {
                return Err("terminal await identity mismatch");
            }
            let members = set_row
                .members
                .iter()
                .enumerate()
                .filter_map(|(index, word)| word.map(|word| (index, word)))
                .map(|(index, word)| {
                    let row = s.tasks.get(word as u32 as usize)?.as_ref()?;
                    if row.generation != (word >> 32) as u32 || row.scope != Some(set_row.scope) {
                        return None;
                    }
                    log.tasks.get(&word).cloned().map(|id| (index as u64, id))
                })
                .collect::<Option<Vec<_>>>()
                .ok_or("missing terminal await member identity")?;
            if members != pending.members {
                return Err("terminal await members changed");
            }
            let ordinal = log
                .ordinals
                .get(path)
                .and_then(|n| n.checked_sub(1))
                .ok_or("missing terminal await ordinal")?;
            if log.last_ops.get(path) != Some(&"task.await_any")
                || log.canonical.iter().any(|event| {
                    event.task == *path
                        && event.ordinal == ordinal
                        && matches!(event.outcome, CanonicalOutcome::Selected { .. })
                })
                || !log.canonical.iter().any(|event| {
                    event.task == *path
                        && event.ordinal == ordinal
                        && event.item == CanonicalItem::Object(pending.set.clone())
                        && event.outcome
                            == CanonicalOutcome::Awaiting {
                                members: members.clone(),
                            }
                })
            {
                return Err("missing published terminal await prefix");
            }
            Ok((
                CanonicalItem::Object(pending.set.clone()),
                CanonicalOutcome::AwaitAbandoned {
                    task: path.clone(),
                    ordinal,
                },
            ))
        })
        .collect()
}

/// Trace::capture owns IdleGuard and has already proved zero running workers.
/// The original terminal charge/failure and its position remain authoritative.
pub(super) fn close(s: &mut Scheduler, stop: &crate::stop::Stop) -> Result<(), &'static str> {
    let Logical::Ordered(driver) = &s.logical else {
        return Ok(());
    };
    let Some(log) = &driver.events else {
        return Ok(());
    };
    let Some(terminal) = log.canonical.last() else {
        return Ok(());
    };
    if terminal.item != CanonicalItem::Terminal
        || terminal.outcome != CanonicalOutcome::Stopped(stop.clone().outcome())
    {
        return Ok(());
    }
    // Validate the whole await snapshot before mutating any evidence. Host
    // phases then use the same strict validation as task-local terminal cuts.
    let mut events = awaits(s, log)?;
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    events.extend(log.terminal_hosts()?);
    if events.is_empty() {
        return Ok(());
    }
    let mut terminal = log.canonical.pop().unwrap();
    let boundary = Boundary {
        task: terminal.task.clone(),
        ordinal: terminal.ordinal,
        op: Some(terminal.op),
        transition: terminal.transition.clone(),
        retirement: terminal.retirement,
        before: terminal.before.clone(),
        credit: terminal.credit,
        native_outcome: None,
    };
    // Insert before the existing terminal without charging again, consuming a
    // new ordinal, or manufacturing a second version of the terminal item.
    let key = (terminal.task.clone(), terminal.ordinal);
    *log.next_subevent.get_mut(&key).unwrap() = terminal.subevent;
    log.subevents(&boundary, terminal.op, events);
    let next = log.next_subevent.get_mut(&key).unwrap();
    terminal.subevent = *next;
    *next = next.checked_add(1).expect("terminal subevent overflow");
    log.canonical.push(terminal);
    log.awaiting.clear();
    Ok(())
}
