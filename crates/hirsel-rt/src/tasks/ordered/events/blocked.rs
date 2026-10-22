//! The existing fully-saved CONC-8 verdict, projected before row reclamation.
use super::*;
use crate::tasks::{Scheduler, TASK_SET, Wait, handle};

fn snapshot(
    s: &Scheduler,
    log: &Log,
    table: Option<crate::isolate::FailureTable>,
) -> Option<Vec<CanonicalWait>> {
    let mut rows = Vec::new();
    let task_id = |index: u32| {
        let row = s.tasks.get(index as usize)?.as_ref()?;
        log.tasks.get(&handle(index, row.generation)).cloned()
    };
    for (task, wait) in s.parked() {
        let row = s.tasks[task as usize].as_ref()?;
        let site = if row.wait_site == 0 {
            Default::default()
        } else {
            table?.located(row.wait_site)
        };
        let (kind, targets) = match wait {
            Wait::Join { child, .. } => ("task.await", vec![task_id(child)?]),
            Wait::Exit { scope } => {
                let row = s.scopes.get(scope as usize)?.as_ref()?;
                let mut targets = vec![log.scopes.get(&handle(scope, row.generation))?.clone()];
                targets.extend(
                    row.children
                        .iter()
                        .map(|&child| task_id(child))
                        .collect::<Option<Vec<_>>>()?,
                );
                ("task.scope", targets)
            }
            Wait::Receive { .. } | Wait::ReceiveReady { .. } => (
                "channel.receive",
                vec![log.receivers.get(&row.path)?.0.clone()],
            ),
            Wait::Any { set, .. } | Wait::Drain { set } => {
                let set_row = s.sets.get(set as usize)?.as_ref()?;
                let word = crate::channels::ENDPOINT | TASK_SET | handle(set, set_row.generation);
                let mut targets = vec![log.sets.get(&word)?.clone()];
                targets.extend(
                    set_row
                        .members
                        .iter()
                        .flatten()
                        .map(|word| log.tasks.get(word).cloned())
                        .collect::<Option<Vec<_>>>()?,
                );
                (
                    if matches!(wait, Wait::Any { .. }) {
                        "task.await_any"
                    } else {
                        "task.cancel_all"
                    },
                    targets,
                )
            }
            Wait::Gate { gate } => ("foreign.gate", vec![log.gates.get(&gate)?.clone()]),
            Wait::Pool => ("foreign.pool", Vec::new()),
            // Native request tokens and Free-only waits have no admitted E identity.
            Wait::Host { .. } | Wait::Final | Wait::Retry { .. } => return None,
        };
        rows.push(CanonicalWait {
            task: row.path.clone(),
            site,
            kind,
            targets,
        });
    }
    rows.sort_unstable_by(|a, b| a.task.cmp(&b.task));
    let parked: BTreeSet<_> = rows.iter().map(|row| &row.task).collect();
    // A terminal cut can leave entered waits unfinished, but cannot certify
    // any pending operation that is absent from the quiescent wait snapshot.
    if log
        .joins
        .keys()
        .chain(log.scope_exits.keys())
        .chain(log.bulk_detaches.keys())
        .chain(log.cancel_all.keys())
        .chain(log.awaiting.keys())
        .chain(log.receiving.iter())
        .chain(log.gate_waiting.keys())
        .chain(log.arming.iter())
        .any(|path| !parked.contains(path))
    {
        return None;
    }
    Some(rows)
}

pub(super) fn record(s: &mut Scheduler, table: Option<crate::isolate::FailureTable>) {
    let Logical::Ordered(driver) = &s.logical else {
        return;
    };
    let Some(log) = &driver.events else {
        return;
    };
    if log.closed_blocked {
        return;
    }
    let waits = snapshot(s, log, table);
    let retirement = driver.frontier.as_ref().unwrap().retired();
    let before = Stretch::new(driver.budgets);
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let task = Vec::new();
    let next = log.ordinals.entry(task.clone()).or_default();
    let ordinal = *next;
    *next = next
        .checked_add(1)
        .expect("blocked terminal ordinal overflow");
    let Some(waits) = waits else {
        log.unsupported.push(Unsupported {
            task,
            ordinal,
            op: Some("stop"),
        });
        return;
    };
    let boundary = Boundary {
        task,
        ordinal,
        op: Some("stop"),
        transition: Transition::Stop,
        retirement,
        before,
        credit: 0,
        native_outcome: None,
    };
    log.subevents(
        &boundary,
        "stop",
        vec![(CanonicalItem::Terminal, CanonicalOutcome::Blocked(waits))],
    );
    log.boundaries.push(boundary);
    log.closed_blocked = true;
}
