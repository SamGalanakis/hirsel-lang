//! Optional integration of exact charge witnesses with retired event positions.
use super::*;
use crate::budget::stretch::Settled;
use hirsel_run_schema::BudgetKind;

pub(in crate::tasks::ordered) fn terminal_recorded(ctx: *mut TaskCtx) -> bool {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap() };
    matches!(&s.logical, Logical::Ordered(driver) if driver.events.as_ref().is_some_and(|log| log.budget_terminal))
}

pub(in crate::tasks::ordered) fn partial(
    ctx: *mut TaskCtx,
    task: TaskId,
    before: &Stretch,
    result: Result<(), BudgetKind>,
) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !super::enabled(s) {
        return;
    }
    let path = s.tasks[task as u32 as usize].as_ref().unwrap().path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        return;
    };
    let limit = driver
        .cpu_batch
        .as_ref()
        .and_then(|batch| batch.limit(task))
        .unwrap_or(u64::MAX);
    let Some(log) = &mut driver.events else {
        return;
    };
    let Some(observer) = &mut log.charges else {
        return;
    };
    assert_eq!(
        observer.settle_with_limit(task, before, limit),
        result,
        "exact head charge prefix"
    );
    if let Err(kind) = result {
        #[cfg(test)]
        log.rejected_budget_stretches.push(before.clone());
        let before = observer.take(task);
        let retirement = driver.frontier.as_ref().unwrap().retired();
        emit(log, path, retirement, before, kind);
    }
}

pub(in crate::tasks::ordered) fn stopped(
    ctx: *mut TaskCtx,
    task: TaskId,
    before: &Stretch,
    retirement: u64,
    kind: BudgetKind,
) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !super::enabled(s) {
        return;
    }
    let path = s.tasks[task as u32 as usize].as_ref().unwrap().path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        return;
    };
    let Some(log) = &mut driver.events else {
        return;
    };
    let Some(observer) = &mut log.charges else {
        return;
    };
    assert_eq!(
        observer.settle(task, before),
        Err(kind),
        "exact rejected stretch prefix"
    );
    #[cfg(test)]
    log.rejected_budget_stretches.push(before.clone());
    let before = observer.take(task);
    emit(log, path, retirement, before, kind);
}

fn emit(log: &mut Log, task: Vec<u32>, retirement: u64, before: Stretch, kind: BudgetKind) {
    assert!(!log.budget_terminal, "one selected budget cut");
    log.budget_terminal = true;
    let next = log.ordinals.entry(task.clone()).or_default();
    let ordinal = *next;
    *next = next.checked_add(1).expect("terminal ordinal overflow");
    log.last_ops.insert(task.clone(), "stop");
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
        vec![(
            CanonicalItem::Terminal,
            CanonicalOutcome::Stopped(hirsel_run_schema::RunOutcome::BudgetExceeded(kind)),
        )],
    );
    log.boundaries.push(boundary);
}

pub(in crate::tasks::ordered) fn retirement(
    ctx: *mut TaskCtx,
    task: TaskId,
    before: &Stretch,
    credit: u64,
) -> Option<Stretch> {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let Logical::Ordered(driver) = &mut s.logical else {
        return None;
    };
    let log = driver.events.as_mut()?;
    let observer = log.charges.as_mut()?;
    assert_eq!(
        observer.settle(task, before),
        Ok(()),
        "retired exact charges must fit"
    );
    observer.credit(credit);
    unsafe { (*ctx).ordered_charge_trace.committed_credit(credit) };
    Some(observer.take(task))
}

pub(in crate::tasks::ordered) fn initialize(
    log: &mut Log,
    state: Settled,
    budgets: crate::budget::stretch::Budgets,
) {
    assert!(
        log.charges
            .replace(crate::budget::stretch_trace::Observer::new(state, budgets))
            .is_none()
    );
}
