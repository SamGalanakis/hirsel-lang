//! Owned native command submission. The frontier is temporarily moved out of
//! Scheduler for bounded object work, then restored before any host bridge or
//! physical suspension. A committing task never borrows another task's ctx.

use std::collections::BTreeSet;

use crate::TaskCtx;
use crate::budget::stretch::{Budgets, Settled, Stretch};
use crate::ordered::footprint::{Footprint, Held, Item, ObjectId, TaskId};
use crate::ordered::objects::{Kind, Registry};
use crate::ordered::{Frontier, Offer, Operation, Policy, Progress};
use crate::stop::{Stop, Stopped};

use super::super::{Logical, handle, scheduler};
use super::driver::Driver;
use super::{Command, Objects, Outcome, Returned};

pub(crate) fn on(ctx: *mut TaskCtx) -> bool {
    matches!(scheduler(ctx).logical, Logical::Ordered(_))
}

fn driver(ctx: *mut TaskCtx) -> &'static mut Driver {
    let Logical::Ordered(driver) = &mut scheduler(ctx).logical else {
        panic!("ordered submission without an ordered driver")
    };
    driver
}

fn current(ctx: *mut TaskCtx) -> TaskId {
    let s = scheduler(ctx);
    let task = s.current;
    handle(task, s.task(task).generation)
}

/// Opt-in for supported native-boundary tests. General admission is not wired.
/// # Safety
/// Admission only, with physical dispatch installed and before root start.
/// The body uses only supported ordered boundaries and explicit dispatch end;
/// no budgeted allocation, fuel, root release, task/host or terminal effect.
/// No native object exists before this admission.
pub(crate) unsafe fn enable(ctx: *mut TaskCtx, policy: Policy) {
    let mut objects = Registry::default();
    objects.install_types(unsafe { crate::ordered::capture::types(ctx) });
    let root = current(ctx);
    let mut driver = Driver::new(
        root,
        policy,
        Settled {
            level: 0,
            limit: u64::MAX,
            fuel: u64::MAX,
        },
        objects,
    );
    driver
        .frontier
        .as_mut()
        .unwrap()
        .held
        .insert(root, Held::empty());
    assert_eq!(driver.select(), vec![root]);
    scheduler(ctx).logical = Logical::Ordered(driver);
}

fn empty() -> Stretch {
    Stretch::new(Budgets {
        fuel: false,
        memory: false,
    })
}

/// All callbacks here are bounded internal object work. Objects::deferred
/// forbids a poll until this function restores the frontier and ends borrows.
fn with_frontier<T>(
    ctx: *mut TaskCtx,
    action: impl FnOnce(&mut Frontier<Command, Outcome>, &mut Objects<'_>) -> T,
) -> T {
    let mut frontier = driver(ctx)
        .frontier
        .take()
        .expect("nonreentrant ordered commit");
    let mut records = Vec::new();
    let mut record = |task, _, outcome: &Outcome, _| records.push((task, outcome.returned()));
    let mut backend = unsafe { Objects::deferred(ctx, &mut record) };
    let result = action(&mut frontier, &mut backend);
    let driver = driver(ctx);
    assert!(driver.frontier.replace(frontier).is_none());
    for (task, returned) in records {
        let ordinal = driver.recorded.entry(task).or_default();
        if driver.waiting.get(&task) == Some(ordinal) {
            assert!(driver.returned.insert(task, returned).is_none());
            driver.waiting.remove(&task);
        }
        *ordinal += 1;
    }
    result
}

/// Exactly one retirement poll. No driver/scheduler reference crosses it.
fn poll(ctx: *mut TaskCtx) -> Stopped<()> {
    assert!(
        !std::mem::replace(&mut driver(ctx).host_poll, true),
        "one poll owner"
    );
    unsafe { super::super::poll_parked(ctx) }?;
    let ready = {
        let s = scheduler(ctx);
        let ready = std::mem::take(&mut s.runnable);
        ready
            .into_iter()
            .map(|task| handle(task, s.task(task).generation))
            .collect()
    };
    let driver = driver(ctx);
    driver.frontier.as_mut().unwrap().complete_poll(ready);
    driver.host_poll = false;
    Ok(())
}

fn notify(ctx: *mut TaskCtx) {
    let selected = driver(ctx).select();
    for id in selected {
        let task = id as u32;
        let owner = {
            let row = scheduler(ctx).task(task);
            assert_eq!(row.generation, (id >> 32) as u32);
            row.owner
        };
        unsafe { crate::pool::free::schedule_saved((*ctx).run_ptr(), task, owner) };
    }
}

fn advance(ctx: *mut TaskCtx) -> Stopped<()> {
    if driver(ctx).host_poll {
        return Ok(());
    }
    loop {
        let progress =
            with_frontier(ctx, |f, backend| f.absorb(backend)).map_err(Stop::BudgetExceeded)?;
        match progress {
            Progress::Poll => poll(ctx)?,
            Progress::Absorbed(_) | Progress::Called(_) | Progress::Retired => {}
            Progress::NeedSegment(_) | Progress::Idle => break,
            Progress::Stopped => return Err(Stop::Failed),
        }
    }
    notify(ctx);
    Ok(())
}

fn accepted(ctx: *mut TaskCtx, task: TaskId, waiting: bool) {
    let driver = driver(ctx);
    let ordinal = driver.submitted.entry(task).or_default();
    if waiting {
        assert!(driver.waiting.insert(task, *ordinal).is_none());
    }
    *ordinal += 1;
}

fn suspend(ctx: *mut TaskCtx, task: TaskId, head_only: bool) {
    driver(ctx).suspending(task, head_only);
    unsafe { crate::pool::free::suspend_current(ctx, false) };
    driver(ctx).enter(task, crate::pool::compute_index().unwrap_or(0));
}

/// The owning runtime entry keeps its carriers live while a pending command
/// waits. The accepted command owns its input bytes, so no saved slot is read
/// or written by a different committing task.
pub(super) fn submit(
    ctx: *mut TaskCtx,
    mut operation: Operation<Command>,
) -> Stopped<Option<Returned>> {
    let task = current(ctx);
    driver(ctx).enter(task, crate::pool::compute_index().unwrap_or(0));
    loop {
        advance(ctx)?;
        let head = driver(ctx).frontier.as_ref().unwrap().head();
        if head == Some(task) {
            accepted(ctx, task, true);
            let progress =
                with_frontier(ctx, |f, backend| f.apply_head(empty(), operation, backend))
                    .map_err(Stop::BudgetExceeded)?;
            if progress == Progress::Poll {
                poll(ctx)?;
            }
            advance(ctx)?;
            return Ok(Some(
                driver(ctx)
                    .returned
                    .remove(&task)
                    .expect("head outcome recorded"),
            ));
        }
        let (offer, retained, result) = with_frontier(ctx, |f, backend| {
            let (offer, retained) = f.offer(task, empty(), operation, backend);
            let result = (offer == Offer::Early).then(|| f.early_result(task).unwrap().returned());
            (offer, retained, result)
        });
        match offer {
            Offer::Early => {
                accepted(ctx, task, false);
                return Ok(result);
            }
            Offer::Logged => {
                accepted(ctx, task, false);
                return Ok(None);
            }
            Offer::Pending => {
                accepted(ctx, task, true);
                loop {
                    suspend(ctx, task, false);
                    advance(ctx)?;
                    if let Some(returned) = driver(ctx).returned.remove(&task) {
                        return Ok(Some(returned));
                    }
                }
            }
            Offer::Full => {
                operation = retained.expect("unaccepted command retained").1;
                suspend(ctx, task, true);
            }
            Offer::Stopped => return Err(Stop::Failed),
        }
    }
}

pub(super) fn footprint(ctx: *mut TaskCtx, kind: Kind, word: u64, write: bool) -> Footprint {
    driver(ctx)
        .objects
        .footprint(kind, word, write)
        .unwrap_or_else(|| {
            // Missing native binding is a conservative head/log boundary, never
            // an empty footprint that could certify an early commit.
            Footprint {
                writes: BTreeSet::from([Item::Host]),
                ..Footprint::default()
            }
        })
}

fn bind_atomic(ctx: *mut TaskCtx, word: u64) -> Footprint {
    let task = current(ctx);
    let path = super::super::current_path(ctx);
    let driver = driver(ctx);
    let ordinal = *driver.submitted.get(&task).unwrap_or(&0);
    let object = ObjectId {
        creator: path,
        ordinal,
    };
    driver
        .objects
        .bind_erased(Kind::Atomic, word, object.clone(), BTreeSet::new());
    let frontier = driver.frontier.as_mut().unwrap();
    frontier
        .held
        .get_mut(&task)
        .expect("a live owner")
        .objects
        .insert(object);
    frontier.carries = driver.objects.carries.clone();
    driver.objects.footprint(Kind::Atomic, word, true).unwrap()
}

/// # Safety
/// The role owns this entry; image and output match the atomic ABI.
pub(crate) unsafe fn atomic_create(
    ctx: *mut TaskCtx,
    out: *mut u64,
    image: *const u8,
    size: u64,
) -> Stopped<()> {
    let reservation = scheduler(ctx).sync.reserve_image();
    let word = reservation.word();
    let footprint = bind_atomic(ctx, word);
    let image = unsafe { std::slice::from_raw_parts(image, size as usize) }.to_vec();
    submit(
        ctx,
        Command::AtomicCreate {
            reservation: std::cell::RefCell::new(Some(reservation)),
            image,
        }
        .operation(footprint),
    )?;
    unsafe { out.write(word) };
    Ok(())
}

/// # Safety
/// As for the atomic load ABI; all bytes remain owned by this entry.
pub(crate) unsafe fn atomic_load(
    ctx: *mut TaskCtx,
    word: u64,
    slot: *mut u8,
    size: u64,
) -> Stopped<()> {
    let op = Command::AtomicLoad {
        word,
        bytes: size as usize,
    }
    .operation(footprint(ctx, Kind::Atomic, word, false));
    let returned = submit(ctx, op)?.expect("a value command cannot be logged without a result");
    assert_eq!(returned.image.len(), size as usize);
    unsafe { std::ptr::copy_nonoverlapping(returned.image.as_ptr(), slot, size as usize) };
    Ok(())
}

/// # Safety
/// As for the atomic store ABI; the image is copied before submission.
pub(crate) unsafe fn atomic_store(
    ctx: *mut TaskCtx,
    word: u64,
    image: *const u8,
    size: u64,
) -> Stopped<()> {
    let image = unsafe { std::slice::from_raw_parts(image, size as usize) }.to_vec();
    let op =
        Command::AtomicStore { word, image }.operation(footprint(ctx, Kind::Atomic, word, true));
    submit(ctx, op)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_atomic_entries_submit_retire_and_return_owned_results() {
        let mut run = crate::host::BodyRun::new(8, false);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { enable(ctx, Policy::admit(&Default::default(), None)) },
            |ctx| {
                unsafe {
                    let mut word = 0;
                    let initial = 17u64;
                    assert_eq!(
                        super::super::super::hirsel_rt_v2_atomic_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut word,
                            (&raw const initial).cast(),
                            8
                        ),
                        crate::STATUS_OK
                    );
                    for expected in 18..=33u64 {
                        assert_eq!(
                            super::super::super::hirsel_rt_v2_atomic_store(
                                ctx,
                                &word,
                                (&raw const expected).cast(),
                                8
                            ),
                            crate::STATUS_OK
                        );
                        let mut result = 0u64;
                        assert_eq!(
                            super::super::super::hirsel_rt_v2_atomic_load(
                                ctx,
                                &word,
                                (&raw mut result).cast(),
                                8
                            ),
                            crate::STATUS_OK
                        );
                        assert_eq!(result, expected);
                    }
                    super::super::super::role::enter(ctx);
                    let task = current(ctx);
                    let driver = driver(ctx);
                    assert_eq!(driver.frontier.as_ref().unwrap().retired(), 33);
                    assert_eq!(driver.submitted[&task], 33);
                    assert_eq!(driver.recorded[&task], 33);
                    assert!(driver.returned.is_empty());
                    assert!(driver.waiting.is_empty());
                    driver.finishing(task);
                    super::super::super::role::release(ctx);
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }
}
