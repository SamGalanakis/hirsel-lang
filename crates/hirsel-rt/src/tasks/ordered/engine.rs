//! Owned native command submission. The frontier is temporarily moved out of
//! Scheduler for bounded object work, then restored before any host bridge or
//! physical suspension. A committing task never borrows another task's ctx.

use std::collections::BTreeSet;

use crate::TaskCtx;
use crate::budget::stretch::{Budgets, Settled, Stretch};
use crate::ordered::footprint::{Footprint, Held, Item, ObjectId, TaskId};
use crate::ordered::objects::{Kind, Registry};
use crate::ordered::{Backend, Frontier, Next, Offer, Operation, Policy, Progress, Receipt};
use crate::stop::{Stop, Stopped};

use super::super::{Logical, handle, scheduler};
use super::driver::Driver;
use super::{Command, Objects, Outcome, Returned};

pub(crate) fn on(ctx: *mut TaskCtx) -> bool {
    matches!(scheduler(ctx).logical, Logical::Ordered(_))
}

pub(super) fn driver(ctx: *mut TaskCtx) -> &'static mut Driver {
    let Logical::Ordered(driver) = &mut scheduler(ctx).logical else {
        panic!("ordered submission without an ordered driver")
    };
    driver
}

pub(super) fn current(ctx: *mut TaskCtx) -> TaskId {
    let s = scheduler(ctx);
    let task = s.current;
    handle(task, s.task(task).generation)
}

/// Opt-in for supported native-boundary tests. General admission is not wired.
/// # Safety
/// Admission only, with physical dispatch installed and before root start.
/// The body uses only supported ordered boundaries and explicit dispatch end;
/// no host or terminal effects. Native task tests
/// use supported task/receive boundaries and dropless closure environments;
/// no deadlines or host-bearing finalizers are admitted. Resource finalizers
/// use only installed task/E/drop boundaries. Shared-root releases log owned
/// credits; mutex roots and yielding resource drops remain strict-head.
/// No native object exists before this admission.
pub(crate) unsafe fn enable(ctx: *mut TaskCtx, policy: Policy) {
    unsafe {
        (*ctx).run_ref().ordered_allowance.store(
            policy.speculation_bytes,
            std::sync::atomic::Ordering::Release,
        )
    };
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
    unsafe { (*ctx).heap_ref().meter.ordered((*ctx).run_ptr()) };
    unsafe {
        (*ctx)
            .run_ref()
            .ordered_mode
            .store(true, std::sync::atomic::Ordering::Release)
    };
}

/// Switch-in/out run under the scheduler role. Leases belong to a task, never
/// to its worker, and ordered fuel never comes out of the shared Free pool.
/// These projections deliberately avoid scheduler(ctx): switch-out can be part
/// of stop abandonment, where observing the requested stop would recurse.
fn fuel_state(ctx: *mut TaskCtx) -> (TaskId, &'static mut Driver) {
    // An outer compiled entry drops its temporary guard before raising a
    // stop. Abandonment still needs exclusive policy access for switch-out.
    // enter acquires the role without observing the run's stop word.
    unsafe { super::super::role::enter(ctx) };
    let s = unsafe {
        (*(*(*ctx).run_ptr()).tasks.get())
            .as_deref_mut()
            .expect("an admitted ordered scheduler")
    };
    let index = unsafe { (*ctx).task_id };
    let task = handle(index, s.task(index).generation);
    let Logical::Ordered(driver) = &mut s.logical else {
        panic!("ordered switch without an ordered policy")
    };
    (task, driver)
}

pub(crate) fn fuel_in(ctx: *mut TaskCtx) {
    let initial = unsafe { (*ctx).run_ref().fuel.remaining() };
    let memory_limit = unsafe { (*ctx).heap_ref().meter.limit() };
    let memory_level = unsafe { (*ctx).heap_ref().meter.held() };
    let (task, driver) = fuel_state(ctx);
    driver.enter(task, crate::pool::compute_index().unwrap_or(0));
    let frontier = driver.frontier.as_mut().expect("restored fuel frontier");
    if !driver.fuel_initialized {
        frontier.settled.fuel = initial;
        frontier.settled.limit = if memory_limit == 0 {
            u64::MAX
        } else {
            memory_limit
        };
        frontier.settled.level = memory_level;
        driver.memory = memory_limit != 0;
        driver.budgets = Budgets {
            fuel: initial != u64::MAX,
            memory: driver.memory,
        };
        unsafe {
            (*ctx)
                .run_ref()
                .ordered_memory
                .store(driver.memory, std::sync::atomic::Ordering::Release)
        };
        driver.fuel_initialized = true;
    }
    let fuel = driver
        .stretches
        .get(&task)
        .map_or(0, Stretch::fuel_charges)
        .saturating_add(frontier.logged_fuel(task));
    let room = frontier.settled.fuel.saturating_sub(fuel);
    let grant = room.min(crate::budget::FUEL_CHUNK);
    driver.fuel_grants.insert(task, grant);
    let level = frontier.settled.level;
    unsafe { (*ctx).heap_ref().meter.settled(level) };
    unsafe { (*ctx).heap().set_fuel(grant as i64) };
}

fn capture_fuel(ctx: *mut TaskCtx) {
    let word = unsafe { (*ctx).view.compiled.fuel };
    let remaining = word.max(0) as u64;
    let (task, driver) = fuel_state(ctx);
    let budgets = driver.budgets;
    let freed = unsafe { (*ctx).ordered_freed.replace(0) };
    if freed != 0 {
        driver
            .stretches
            .entry(task)
            .or_insert_with(|| Stretch::new(budgets))
            .free(freed);
        // A free may be older settled storage; only currently reserved
        // speculative bytes can replenish the speculative allowance.
        if let Some(frontier) = &mut driver.frontier {
            frontier.credit_speculative(task, freed);
        }
    }
    let grant = driver
        .fuel_grants
        .insert(task, remaining)
        .unwrap_or(remaining);
    let consumed = grant
        .saturating_sub(remaining)
        .saturating_add(u64::from(word < 0));
    if consumed != 0 {
        driver
            .stretches
            .entry(task)
            .or_insert_with(|| Stretch::new(budgets))
            .fuel_points(consumed);
    }
    // A negative word denotes the failed point already counted above. Clear
    // it before a pause, so saving this frame cannot charge that point twice.
    if word < 0 {
        unsafe { (*ctx).heap().set_fuel(0) };
    }
}

pub(crate) fn fuel_out(ctx: *mut TaskCtx) {
    capture_fuel(ctx);
    let (task, driver) = fuel_state(ctx);
    driver.fuel_grants.remove(&task);
    unsafe { (*ctx).heap().set_fuel(0) };
}

fn stretch(ctx: *mut TaskCtx) -> Stretch {
    capture_fuel(ctx);
    let (task, driver) = fuel_state(ctx);
    driver
        .stretches
        .remove(&task)
        .unwrap_or_else(|| Stretch::new(driver.budgets))
}

/// Before taking a worker heap/pool borrow. A denied reservation leaves the
/// request and logical budget untouched; only head promotion can resume it.
pub(crate) fn reserve_allocation(ctx: *mut TaskCtx, bytes: u64) -> Stopped<()> {
    let task = current(ctx);
    driver(ctx).enter(task, crate::pool::compute_index().unwrap_or(0));
    capture_fuel(ctx);
    advance(ctx)?;
    let memory = driver(ctx).memory;
    let budgets = driver(ctx).budgets;
    let envelope_full = memory
        && driver(ctx)
            .stretches
            .get(&task)
            .is_some_and(|stretch| stretch.allocation_pauses(bytes));
    if envelope_full {
        at_head(ctx)?;
    }
    let reserved = {
        let driver = driver(ctx);
        let frontier = driver
            .frontier
            .as_mut()
            .expect("restored allocation frontier");
        let fuel = frontier
            .logged_fuel(task)
            .saturating_add(driver.stretches.get(&task).map_or(0, Stretch::fuel_charges));
        frontier.reserve(task, bytes, fuel)
    };
    if reserved.is_err() {
        at_head(ctx)?;
    }
    if memory {
        let head = driver(ctx).frontier.as_ref().unwrap().head() == Some(task);
        if head {
            let before = stretch(ctx);
            let frontier = driver(ctx).frontier.as_mut().unwrap();
            frontier
                .settled
                .settle(&before)
                .map_err(Stop::BudgetExceeded)?;
            let mut allocation = Stretch::new(budgets);
            allocation
                .alloc(bytes)
                .expect("one allocation fits an envelope");
            frontier
                .settled
                .settle(&allocation)
                .map_err(Stop::BudgetExceeded)?;
        } else {
            driver(ctx)
                .stretches
                .entry(task)
                .or_insert_with(|| Stretch::new(budgets))
                .alloc(bytes)
                .expect("preflighted allocation envelope");
        }
        assert_eq!(
            unsafe { (*ctx).ordered_allocation.replace(bytes) },
            0,
            "one upcoming heap/pool charge"
        );
        let level = driver(ctx).frontier.as_ref().unwrap().settled.level;
        unsafe { (*ctx).heap_ref().meter.settled(level) };
    }
    Ok(())
}

/// These callbacks execute with a worker heap/pool borrowed. They touch only
/// the originating task's disjoint cells: no policy, role, stop or switch.
pub(crate) fn memory_charged(ctx: *mut TaskCtx, bytes: u64) {
    assert_eq!(
        unsafe { (*ctx).ordered_allocation.replace(0) },
        bytes,
        "ordered allocation must preflight its exact charge"
    );
}

pub(crate) fn memory_freed(ctx: *mut TaskCtx, bytes: u64) {
    unsafe {
        (*ctx)
            .ordered_freed
            .set((*ctx).ordered_freed.get().saturating_add(bytes))
    };
}

/// A refill is a physical budget boundary, never a semantic operation or poll.
/// An exhausted younger segment waits for promotion before exposing its stop.
pub(crate) fn fuel_refill(ctx: *mut TaskCtx) -> Stopped<()> {
    capture_fuel(ctx);
    advance(ctx)?;
    let task = current(ctx);
    let must_wait = {
        let driver = driver(ctx);
        let frontier = driver.frontier.as_ref().unwrap();
        let consumed = frontier
            .logged_fuel(task)
            .saturating_add(driver.stretches.get(&task).map_or(0, Stretch::fuel_charges));
        frontier.head() != Some(task) && consumed > frontier.settled.fuel
    };
    if must_wait {
        at_head(ctx)?;
    }
    if driver(ctx).frontier.as_ref().unwrap().head() == Some(task) {
        let before = stretch(ctx);
        driver(ctx)
            .frontier
            .as_mut()
            .unwrap()
            .settled
            .settle(&before)
            .map_err(Stop::BudgetExceeded)?;
    }
    fuel_in(ctx);
    Ok(())
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
    let spawned = std::mem::take(&mut backend.spawned);
    drop(backend);
    let driver = driver(ctx);
    assert!(driver.frontier.replace(frontier).is_none());
    for (task, held) in spawned {
        driver.enqueue(task, held);
    }
    for (task, returned) in records {
        let ordinal = driver.recorded.entry(task).or_default();
        if driver.waiting.get(&task) == Some(ordinal) {
            assert!(driver.returned.insert(task, returned).is_none());
            driver.waiting.remove(&task);
        }
        *ordinal += 1;
    }
    let level = driver.frontier.as_ref().unwrap().settled.level;
    unsafe { (*ctx).heap_ref().meter.settled(level) };
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
    if driver(ctx).host_poll || driver(ctx).poll_deferred() {
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

/// Wait before any task-tree mutation. This gate keeps a later task's native
/// arguments on its own stack until its existing log has retired at the head.
pub(super) fn at_head(ctx: *mut TaskCtx) -> Stopped<()> {
    let task = current(ctx);
    driver(ctx).enter(task, crate::pool::compute_index().unwrap_or(0));
    loop {
        advance(ctx)?;
        if driver(ctx).frontier.as_ref().unwrap().head() == Some(task) {
            let before = stretch(ctx);
            driver(ctx)
                .frontier
                .as_mut()
                .unwrap()
                .settled
                .settle(&before)
                .map_err(Stop::BudgetExceeded)?;
            fuel_in(ctx);
            return Ok(());
        }
        suspend(ctx, task, true);
    }
}

struct HeadEffect<'a, 'b, F, T> {
    objects: &'a mut Objects<'b>,
    effect: Option<F>,
    result: &'a mut Option<T>,
}

impl<F: FnOnce() -> (T, Next), T> Backend<Command> for HeadEffect<'_, '_, F, T> {
    type Result = Outcome;

    fn commit(&mut self, _: TaskId, op: &Operation<Command>, early: bool) -> Receipt<Outcome> {
        assert!(!early, "a native task effect runs only at the head");
        assert!(matches!(op.payload, Command::Head { .. }));
        let (result, next) = self.effect.take().expect("head callback committed once")();
        *self.result = Some(result);
        Receipt {
            result: Outcome::image(0, Vec::new()),
            next,
            credit: 0,
        }
    }
    fn record(&mut self, task: TaskId, op: &Operation<Command>, result: &Outcome, retirement: u64) {
        self.objects.record(task, op, result, retirement);
    }
    fn publish(&mut self, _: TaskId, _: &Operation<Command>, _: &Outcome) {
        unreachable!("strict head effect cannot have an early receipt")
    }
    fn call(&mut self, caller: TaskId, child: TaskId) {
        self.objects.call(caller, child);
    }
    fn defer_poll(&self) -> bool {
        true
    }
    fn poll(&mut self) -> Vec<TaskId> {
        unreachable!("restore policy before a native retirement poll")
    }
}

/// Execute the actual bounded native effect after stretch settlement. Nothing
/// crosses a suspension here; only its owner supplies this immediate callback.
/// The returned progress belongs to the caller, which installs any newly
/// committed child before completing the retirement poll.
pub(super) fn head_effect<T>(
    ctx: *mut TaskCtx,
    name: &'static str,
    effect: impl FnOnce() -> (T, Next),
) -> Stopped<(T, Progress)> {
    at_head(ctx)?;
    let task = current(ctx);
    accepted(ctx, task, false);
    let before = stretch(ctx);
    let mut result = None;
    let progress = with_frontier(ctx, |frontier, objects| {
        let mut backend = HeadEffect {
            objects,
            effect: Some(effect),
            result: &mut result,
        };
        frontier.apply_head(
            before,
            Command::Head { name }.operation(Footprint::default()),
            &mut backend,
        )
    })
    .map_err(Stop::BudgetExceeded)?;
    Ok((result.expect("head effect completed"), progress))
}

/// Resume the same waiting/end op after a child call or park. This has no
/// new accepted command and therefore consumes no operation ordinal.
pub(super) fn head_continuation<T>(
    ctx: *mut TaskCtx,
    effect: impl FnOnce() -> (T, Next),
) -> Stopped<(T, Progress)> {
    at_head(ctx)?;
    let before = stretch(ctx);
    let mut result = None;
    let progress = with_frontier(ctx, |frontier, backend| {
        frontier.continue_head(before, backend, || {
            let (value, next) = effect();
            result = Some(value);
            next
        })
    })
    .map_err(Stop::BudgetExceeded)?;
    Ok((result.expect("head continuation completed"), progress))
}

/// A strict native task op returns when its next serial continuation is head.
pub(super) fn continue_at_head(ctx: *mut TaskCtx, progress: Progress) -> Stopped<()> {
    if progress == Progress::Poll {
        poll(ctx)?;
    }
    at_head(ctx)
}

/// A park or holding publication precedes physical save. The generic idle
/// callback finishes its poll and may only then select a logical continuation.
pub(super) fn save_boundary(ctx: *mut TaskCtx, progress: Progress) -> Stopped<()> {
    let task = current(ctx);
    if progress == Progress::Poll {
        driver(ctx).defer_poll_until_saved(task);
    }
    suspend(ctx, task, false);
    at_head(ctx)
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
    let mut before = stretch(ctx);
    loop {
        advance(ctx)?;
        let head = driver(ctx).frontier.as_ref().unwrap().head();
        if head == Some(task) {
            accepted(ctx, task, true);
            let progress =
                with_frontier(ctx, |f, backend| f.apply_head(before, operation, backend))
                    .map_err(Stop::BudgetExceeded)?;
            if progress == Progress::Poll {
                poll(ctx)?;
            }
            advance(ctx)?;
            let result = driver(ctx)
                .returned
                .remove(&task)
                .expect("head outcome recorded");
            fuel_in(ctx);
            return Ok(Some(result));
        }
        let (offer, retained, result) = with_frontier(ctx, |f, backend| {
            let (offer, retained) = f.offer(task, before, operation, backend);
            let result = (offer == Offer::Early).then(|| f.early_result(task).unwrap().returned());
            (offer, retained, result)
        });
        match offer {
            Offer::Early => {
                accepted(ctx, task, false);
                fuel_in(ctx);
                return Ok(result);
            }
            Offer::Logged => {
                accepted(ctx, task, false);
                fuel_in(ctx);
                return Ok(None);
            }
            Offer::Pending => {
                accepted(ctx, task, true);
                loop {
                    suspend(ctx, task, false);
                    advance(ctx)?;
                    if let Some(returned) = driver(ctx).returned.remove(&task) {
                        fuel_in(ctx);
                        return Ok(Some(returned));
                    }
                }
            }
            Offer::Full => {
                (before, operation) = retained.expect("unaccepted command retained");
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

#[derive(Clone)]
pub(super) struct Carrier {
    descriptor: u32,
    none: Vec<u8>,
}

fn bind(ctx: *mut TaskCtx, bindings: &[(Kind, u64, BTreeSet<u32>)]) -> Footprint {
    let task = current(ctx);
    let path = super::super::current_path(ctx);
    let driver = driver(ctx);
    let ordinal = *driver.submitted.get(&task).unwrap_or(&0);
    let object = ObjectId {
        creator: path,
        ordinal,
    };
    for (kind, word, obtains) in bindings {
        driver
            .objects
            .bind_erased(*kind, *word, object.clone(), obtains.clone());
    }
    let frontier = driver.frontier.as_mut().unwrap();
    let held = frontier.held.get_mut(&task).expect("a live owner");
    held.objects.insert(object.clone());
    for (_, _, obtains) in bindings {
        held.obtains.extend(obtains);
    }
    frontier.carries = driver.objects.carries.clone();
    Footprint {
        writes: BTreeSet::from([Item::Object(object)]),
        ..Footprint::default()
    }
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
    let footprint = bind(ctx, &[(Kind::Atomic, word, BTreeSet::new())]);
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

pub(super) fn is_sender(ctx: *mut TaskCtx, word: u64) -> bool {
    driver(ctx).carriers.contains_key(&(Kind::Sender, word))
}

pub(super) fn sender_close(ctx: *mut TaskCtx, word: u64) -> Stopped<()> {
    let footprint = footprint(ctx, Kind::Sender, word, true);
    submit(
        ctx,
        Command::SenderClose { sender: word }.operation(footprint),
    )?;
    Ok(())
}

pub(super) fn carrier_descriptor(ctx: *mut TaskCtx, kind: Kind, word: u64) -> u32 {
    driver(ctx).carriers[&(kind, word)].descriptor
}

/// # Safety
/// The caller owns a live output image matching the linked descriptor.
pub(super) unsafe fn obtained_image(ctx: *mut TaskCtx, image: *const u8, descriptor: u32) {
    let captures = unsafe { crate::ordered::capture::scan(ctx, image, descriptor) };
    let task = current(ctx);
    let driver = driver(ctx);
    let obtained = driver.objects.captured(&captures);
    let held = driver
        .frontier
        .as_mut()
        .unwrap()
        .held
        .get_mut(&task)
        .unwrap();
    held.objects.extend(obtained.objects);
    held.obtains.extend(obtained.obtains);
    held.unknown |= obtained.unknown;
}

/// # Safety
/// The descriptor describes the readable None image for this admitted run.
unsafe fn carrier(ctx: *mut TaskCtx, descriptor: u32, none: *const u8) -> Carrier {
    let layout = unsafe { (*ctx).descriptor(descriptor) };
    let element = unsafe { crate::channels::Element::new(descriptor, layout, none) };
    Carrier {
        descriptor,
        none: element.none.into_vec(),
    }
}

/// # Safety
/// As for the channel.open ABI. Both endpoint words are owner-local outputs.
pub(crate) unsafe fn channel_open(
    ctx: *mut TaskCtx,
    out: *mut u64,
    none: *const u8,
    descriptor: u32,
) -> Stopped<()> {
    let carrier = unsafe { carrier(ctx, descriptor, none) };
    let reservation = scheduler(ctx).channels.reserve_open();
    let (sender, receiver) = reservation.words();
    let footprint = bind(
        ctx,
        &[
            (Kind::Sender, sender, BTreeSet::new()),
            (Kind::Receiver, receiver, BTreeSet::from([descriptor])),
        ],
    );
    for (kind, word) in [(Kind::Sender, sender), (Kind::Receiver, receiver)] {
        driver(ctx).carriers.insert((kind, word), carrier.clone());
    }
    submit(
        ctx,
        Command::ChannelOpen {
            reservation: std::cell::RefCell::new(Some(reservation)),
            descriptor,
            none: carrier.none,
        }
        .operation(footprint),
    )?;
    unsafe {
        out.write(sender);
        out.add(1).write(receiver);
    }
    Ok(())
}

/// # Safety
/// As for sender.dup. Source and output are owner-local handle words.
pub(crate) unsafe fn sender_dup(ctx: *mut TaskCtx, sender: u64, out: *mut u64) -> Stopped<()> {
    let reservation = scheduler(ctx).channels.reserve_dup();
    let word = reservation.word();
    let footprint = footprint(ctx, Kind::Sender, sender, true);
    let driver = driver(ctx);
    driver.objects.alias(Kind::Sender, sender, word);
    let carrier = driver.carriers[&(Kind::Sender, sender)].clone();
    driver.carriers.insert((Kind::Sender, word), carrier);
    submit(
        ctx,
        Command::SenderDup {
            reservation: std::cell::RefCell::new(Some(reservation)),
            sender,
        }
        .operation(footprint),
    )?;
    unsafe { out.write(word) };
    Ok(())
}

/// # Safety
/// As for channel.send; the writable Some image stays owned by this entry.
pub(crate) unsafe fn channel_send(
    ctx: *mut TaskCtx,
    sender: u64,
    message: *mut u8,
    out: *mut u32,
) -> Stopped<()> {
    let size = driver(ctx).carriers[&(Kind::Sender, sender)].none.len();
    let image = unsafe { std::slice::from_raw_parts(message, size) }.to_vec();
    let operation = Command::Send {
        sender,
        image: std::cell::RefCell::new(image),
    }
    .operation(footprint(ctx, Kind::Sender, sender, true));
    let returned = submit(ctx, operation)?.expect("send requires its outcome");
    unsafe {
        std::ptr::copy_nonoverlapping(returned.image.as_ptr(), message, size);
        out.write(returned.status);
    }
    Ok(())
}

/// # Safety
/// As for mutex.create. Wrapper allocation is owner-local; budgets are not
/// admitted by this test-only policy until allocation hooks are installed.
pub(crate) unsafe fn mutex_create(
    ctx: *mut TaskCtx,
    out: *mut u64,
    payload: *mut u8,
    none: *const u8,
    descriptor: u32,
    wrapper: u32,
) -> Stopped<()> {
    let carrier = unsafe { carrier(ctx, descriptor, none) };
    let image = unsafe { std::slice::from_raw_parts(payload, carrier.none.len()) }.to_vec();
    let reservation = scheduler(ctx).sync.reserve_mutex();
    let word = reservation.word();
    let footprint = bind(ctx, &[(Kind::Mutex, word, BTreeSet::from([descriptor]))]);
    driver(ctx)
        .carriers
        .insert((Kind::Mutex, word), carrier.clone());
    let block = unsafe {
        crate::alloc::hirsel_rt_v2_alloc(ctx, wrapper, 8, 8, crate::alloc::DOMAIN_HEAP, 0)
    };
    unsafe { block.add(1).cast::<u64>().write(word) };
    submit(
        ctx,
        Command::MutexCreate {
            reservation: std::cell::RefCell::new(Some(reservation)),
            descriptor,
            none: carrier.none.clone(),
            image: std::cell::RefCell::new(image),
        }
        .operation(footprint),
    )?;
    unsafe {
        std::ptr::copy_nonoverlapping(carrier.none.as_ptr(), payload, carrier.none.len());
        out.write(block as u64);
    }
    Ok(())
}

/// # Safety
/// As for mutex.take: the None carrier and status are owner-local outputs.
pub(crate) unsafe fn mutex_take(
    ctx: *mut TaskCtx,
    word: u64,
    payload: *mut u8,
    out: *mut u32,
) -> Stopped<()> {
    let none = driver(ctx).carriers[&(Kind::Mutex, word)].none.clone();
    let operation =
        Command::MutexTake { word, none }.operation(footprint(ctx, Kind::Mutex, word, true));
    let returned = submit(ctx, operation)?.expect("take requires its outcome");
    if returned.status == 0 {
        let descriptor = carrier_descriptor(ctx, Kind::Mutex, word);
        // MutexTake keeps the turn, so its obtained payload handles must
        // constrain younger passes before any later op or finalizer runs.
        unsafe { obtained_image(ctx, returned.image.as_ptr(), descriptor) };
    }
    unsafe {
        std::ptr::copy_nonoverlapping(returned.image.as_ptr(), payload, returned.image.len());
        out.write(returned.status);
    }
    Ok(())
}

/// # Safety
/// As for mutex.put: payload is an owned Some carrier, restored to None.
pub(crate) unsafe fn mutex_put(ctx: *mut TaskCtx, word: u64, payload: *mut u8) -> Stopped<()> {
    let none = driver(ctx).carriers[&(Kind::Mutex, word)].none.clone();
    let image = unsafe { std::slice::from_raw_parts(payload, none.len()) }.to_vec();
    submit(
        ctx,
        Command::MutexPut {
            word,
            image: std::cell::RefCell::new(image),
        }
        .operation(footprint(ctx, Kind::Mutex, word, true)),
    )?;
    unsafe { std::ptr::copy_nonoverlapping(none.as_ptr(), payload, none.len()) };
    Ok(())
}

/// # Safety
/// The caller owns a dead mutex. Only this owner appends the returned chain.
pub(crate) unsafe fn mutex_cause(ctx: *mut TaskCtx, word: u64) -> Stopped<()> {
    let returned = submit(
        ctx,
        Command::MutexCause { word }.operation(footprint(ctx, Kind::Mutex, word, false)),
    )?
    .expect("cause requires its outcome");
    unsafe { (*ctx).failure.append_chain(returned.chain) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_native_region_credits_and_first_stop_match_warm_and_cold_pools() {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        for width in [2, 4, 8] {
            for warm in [false, true] {
                let layout = image_layout(16);
                let layouts = [&layout as *const crate::Layout];
                let allocated = Arc::new(AtomicU64::new(0));
                let seen = allocated.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe {
                        let limit = crate::region::SEGMENT_SIZE as u64;
                        (*ctx).set_budgets(None, Some(limit));
                        enable(ctx, Policy::admit(&Default::default(), Some(limit)));
                    },
                    move |ctx| unsafe {
                        if warm {
                            for _ in 0..2 {
                                let mut region =
                                    std::mem::MaybeUninit::<crate::region::Region>::uninit();
                                crate::region::hirsel_rt_v2_arena_enter(ctx, region.as_mut_ptr());
                                crate::region::hirsel_rt_v2_arena_alloc(
                                    ctx,
                                    region.as_mut_ptr(),
                                    2,
                                    16,
                                    8,
                                    0,
                                );
                                seen.fetch_add(1, Ordering::Relaxed);
                                assert_eq!(
                                    crate::region::hirsel_rt_v2_region_exit(
                                        ctx,
                                        region.as_mut_ptr(),
                                        0,
                                        0
                                    ),
                                    0
                                );
                            }
                        }
                        let mut outer = std::mem::MaybeUninit::<crate::region::Region>::uninit();
                        crate::region::hirsel_rt_v2_arena_enter(ctx, outer.as_mut_ptr());
                        crate::region::hirsel_rt_v2_arena_alloc(
                            ctx,
                            outer.as_mut_ptr(),
                            2,
                            16,
                            8,
                            0,
                        );
                        seen.fetch_add(1, Ordering::Relaxed);
                        let mut inner = std::mem::MaybeUninit::<crate::region::Region>::uninit();
                        crate::region::hirsel_rt_v2_arena_enter(ctx, inner.as_mut_ptr());
                        crate::region::hirsel_rt_v2_arena_alloc(
                            ctx,
                            inner.as_mut_ptr(),
                            2,
                            16,
                            8,
                            0,
                        );
                        panic!("a second live canonical region segment must stop");
                    },
                );
                assert_eq!(
                    outcome.ended,
                    Err(Stop::BudgetExceeded(hirsel_run_schema::BudgetKind::Memory))
                );
                assert_eq!(allocated.load(Ordering::Relaxed), if warm { 3 } else { 1 });
            }
        }
    }

    #[test]
    fn ordered_native_memory_and_fuel_stop_in_request_order_and_frees_restore_room() {
        use hirsel_run_schema::BudgetKind;
        for width in [2, 4, 8] {
            for case in 0..3 {
                let layout = image_layout(16);
                let layouts = [&layout as *const crate::Layout];
                let mut run = crate::host::BodyRun::new(width, true);
                run.config.layouts = layouts.as_ptr();
                run.config.layout_count = 1;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        let charge = crate::alloc::allocation_charge(ctx, 2, 16, 8);
                        (*ctx).set_budgets(
                            Some(1),
                            Some(if case == 2 { charge } else { 2 * charge - 1 }),
                        );
                        enable(ctx, Policy::admit(&Default::default(), Some(charge)));
                    },
                    move |ctx| unsafe {
                        let allocate = || {
                            crate::alloc::hirsel_rt_v2_alloc(
                                ctx,
                                2,
                                16,
                                8,
                                crate::alloc::DOMAIN_HEAP,
                                0,
                            )
                        };
                        let first = allocate();
                        if case == 1 {
                            charge(ctx);
                            charge(ctx);
                        }
                        if case == 2 {
                            assert_eq!(
                                crate::release::hirsel_rt_v2_release(
                                    ctx,
                                    first,
                                    2,
                                    std::ptr::null(),
                                    0,
                                    0
                                ),
                                0
                            );
                        }
                        let second = allocate();
                        assert_eq!(case, 2, "the second request should stop");
                        assert_eq!(
                            crate::release::hirsel_rt_v2_release(
                                ctx,
                                second,
                                2,
                                std::ptr::null(),
                                0,
                                0
                            ),
                            0
                        );
                        let _role = super::super::super::role::Guard::new(ctx);
                        at_head(ctx).unwrap();
                        assert_eq!(driver(ctx).frontier.as_ref().unwrap().settled.level, 0);
                        assert_eq!((*ctx).heap_ref().meter.held(), 0);
                        assert_eq!((*ctx).ordered_allocation.get(), 0);
                        let task = current(ctx);
                        driver(ctx).finishing(task);
                        0
                    },
                );
                assert_eq!(
                    outcome.ended,
                    match case {
                        0 => Err(Stop::BudgetExceeded(BudgetKind::Memory)),
                        1 => Err(Stop::BudgetExceeded(BudgetKind::Fuel)),
                        _ => Ok(0),
                    },
                    "width {width}, case {case}"
                );
            }
        }
    }

    unsafe fn charge(ctx: *mut TaskCtx) {
        unsafe {
            (*ctx).view.compiled.fuel -= 1;
            if (*ctx).view.compiled.fuel < 0 {
                crate::budget::hirsel_rt_v2_fuel_refill(ctx);
            }
        }
    }

    #[derive(Default)]
    struct FuelOrder {
        start: std::sync::atomic::AtomicBool,
        older_retired: std::sync::atomic::AtomicBool,
        younger_passed: std::sync::atomic::AtomicBool,
        younger: std::sync::atomic::AtomicU64,
    }

    unsafe extern "C" fn fuel_child(
        ctx: *mut TaskCtx,
        environment: *mut crate::Header,
        _: u64,
        _: u64,
    ) -> u8 {
        use super::super::super::*;
        use std::sync::atomic::Ordering;
        let [probe, mode, atomic] =
            unsafe { crate::alloc::payload(environment).cast::<[u64; 3]>().read() };
        let probe = unsafe { &*(probe as *const FuelOrder) };
        if matches!(mode, 0 | 10) {
            while !probe.start.load(Ordering::Acquire) {
                let mut cause = 0;
                assert_eq!(
                    unsafe { hirsel_rt_v2_task_canceled(ctx, &mut cause, &0) },
                    0
                );
            }
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let id = probe.younger.load(Ordering::Acquire);
                let paused = {
                    let _role = unsafe { role::Guard::new(ctx) };
                    id != 0 && driver(ctx).head_paused(id)
                };
                if paused {
                    break;
                }
                assert!(
                    std::time::Instant::now() < until,
                    "younger exhausted segment did not save"
                );
                std::thread::yield_now();
            }
            assert!(!probe.younger_passed.load(Ordering::Acquire));
            if mode == 0 {
                unsafe { charge(ctx) };
            }
            let _role = unsafe { role::Guard::new(ctx) };
            let value = 7u64;
            assert_eq!(
                unsafe { hirsel_rt_v2_atomic_store(ctx, &atomic, (&raw const value).cast(), 8) },
                0
            );
            if mode == 0 {
                assert!(driver(ctx).frontier.as_ref().unwrap().settled.fuel <= 19);
            }
            probe.older_retired.store(true, Ordering::Release);
        } else {
            {
                let _role = unsafe { role::Guard::new(ctx) };
                probe.younger.store(current(ctx), Ordering::Release);
            }
            if matches!(mode, 5 | 6 | 7) {
                if mode == 6 {
                    unsafe {
                        charge(ctx);
                        charge(ctx);
                    }
                }
                let block = unsafe {
                    crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 16, 8, crate::alloc::DOMAIN_HEAP, 0)
                };
                if mode == 7 {
                    assert_eq!(
                        unsafe {
                            crate::release::hirsel_rt_v2_release(
                                ctx,
                                block,
                                2,
                                std::ptr::null(),
                                0,
                                0,
                            )
                        },
                        0
                    );
                }
                unsafe {
                    charge(ctx);
                    charge(ctx);
                }
                probe.younger_passed.store(true, Ordering::Release);
                panic!("combined-budget exhausted segment must stop on promotion");
            } else if matches!(mode, 8 | 9) {
                let mut blocks = Vec::new();
                for _ in 0..=crate::budget::stretch::ENVELOPE_MAX {
                    unsafe { charge(ctx) };
                    blocks.push(unsafe {
                        crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            24,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        )
                    });
                }
                if mode == 9 {
                    let _role = unsafe { role::Guard::new(ctx) };
                    let task = current(ctx);
                    let d = driver(ctx);
                    assert!(!d.budgets.fuel);
                    assert_eq!(
                        d.stretches[&task].pieces().next().unwrap().1.envelope.len(),
                        1
                    );
                }
                for block in blocks {
                    assert_eq!(
                        unsafe {
                            crate::release::hirsel_rt_v2_release(
                                ctx,
                                block,
                                2,
                                std::ptr::null(),
                                0,
                                0,
                            )
                        },
                        0
                    );
                }
                if mode == 8 {
                    probe.younger_passed.store(true, Ordering::Release);
                }
            } else if mode >= 3 {
                unsafe {
                    if mode == 3 {
                        let block = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            256,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(block)
                            .cast::<[u64; 3]>()
                            .write([0; 3]);
                        assert_eq!(
                            crate::release::hirsel_rt_v2_release(
                                ctx,
                                block,
                                2,
                                std::ptr::null(),
                                0,
                                0
                            ),
                            0
                        );
                    } else {
                        let mut region = std::mem::MaybeUninit::<crate::region::Region>::uninit();
                        crate::region::hirsel_rt_v2_region_enter(ctx, region.as_mut_ptr());
                        crate::region::hirsel_rt_v2_arena_alloc(
                            ctx,
                            region.as_mut_ptr(),
                            2,
                            256,
                            8,
                            0,
                        );
                        assert_eq!(
                            crate::region::hirsel_rt_v2_region_exit(ctx, region.as_mut_ptr(), 0, 0),
                            0
                        );
                    }
                }
                probe.younger_passed.store(true, Ordering::Release);
            } else {
                let remaining = if mode == 2 {
                    for _ in 0..5 {
                        unsafe { charge(ctx) };
                    }
                    let value = 3u64;
                    assert_eq!(
                        unsafe {
                            hirsel_rt_v2_atomic_store(ctx, &atomic, (&raw const value).cast(), 8)
                        },
                        0
                    );
                    16
                } else {
                    21
                };
                // Include fuel in earlier logged turns when bounding this segment.
                // Without that term the logged variant would return past point21.
                for _ in 0..remaining {
                    unsafe { charge(ctx) };
                }
                probe.younger_passed.store(true, Ordering::Release);
                panic!("the younger's exhausted point must stop at promotion");
            }
        }
        let mut result = [1u64, 0];
        assert_eq!(
            unsafe {
                hirsel_rt_v2_task_finish(
                    ctx,
                    result.as_mut_ptr().cast(),
                    [0u64; 2].as_ptr().cast(),
                    16,
                )
            },
            0
        );
        if mode == 9 {
            probe.younger_passed.store(true, Ordering::Release);
        }
        0
    }

    fn younger_budget(modes: &[u64]) {
        use super::super::super::*;
        use hirsel_core_contract::handle_scan::TYPED_HANDLE;
        use std::sync::{Arc, atomic::Ordering};
        for (width, mode) in [2, 4, 8]
            .into_iter()
            .flat_map(|width| modes.iter().copied().map(move |mode| (width, mode)))
        {
            let probe = Arc::new(FuelOrder::default());
            let seen = probe.clone();
            let scan = [TYPED_HANDLE, 16, Kind::Atomic.code(), 2, 0, 0, 0, 0];
            let environment = crate::Layout {
                scan: scan.as_ptr(),
                scan_words: scan.len() as u64,
                ..image_layout(24)
            };
            let layouts = [&environment as *const crate::Layout];
            let mut run = crate::host::BodyRun::new(width, true);
            run.config.layouts = layouts.as_ptr();
            run.config.layout_count = 1;
            if mode < 5 {
                run.fuel = Some(20);
            }
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    let memory = if mode >= 5 {
                        let environment = crate::alloc::allocation_charge(ctx, 2, 24, 8);
                        let small = crate::alloc::allocation_charge(ctx, 2, 16, 8);
                        let limit = if matches!(mode, 8 | 9) {
                            environment * (crate::budget::stretch::ENVELOPE_MAX as u64 + 4)
                        } else {
                            2 * environment + small - 1
                        };
                        (*ctx).set_budgets(
                            if mode == 9 {
                                None
                            } else if mode == 8 {
                                Some(1_000_000)
                            } else {
                                Some(1)
                            },
                            Some(limit),
                        );
                        Some(limit)
                    } else {
                        None
                    };
                    let mut policy = Policy::admit(&Default::default(), memory);
                    if matches!(mode, 3 | 4) {
                        policy.speculation_bytes = 0;
                    } else if mode >= 5 {
                        policy.speculation_bytes = 1024 * 1024;
                    }
                    enable(ctx, policy);
                },
                move |ctx| unsafe {
                    let mut scope = 0;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0),
                        0
                    );
                    let mut atomic = 0;
                    let zero = 0u64;
                    assert_eq!(
                        hirsel_rt_v2_atomic_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut atomic,
                            (&raw const zero).cast(),
                            8
                        ),
                        0
                    );
                    let mut last_child = 0;
                    for mode in [if mode >= 5 { 10 } else { 0 }, mode] {
                        let environment = crate::alloc::hirsel_rt_v2_alloc(
                            ctx,
                            2,
                            24,
                            8,
                            crate::alloc::DOMAIN_HEAP,
                            0,
                        );
                        crate::alloc::payload(environment)
                            .cast::<[u64; 3]>()
                            .write([Arc::as_ptr(&seen) as u64, mode, atomic]);
                        if !matches!(mode, 0 | 10) {
                            seen.start.store(true, Ordering::Release);
                        }
                        let mut child = 0;
                        // Spawn may suspend, so publish the younger id from its own entry
                        // instead of retaining a spawner-side table reference.
                        assert_eq!(
                            hirsel_rt_v2_task_spawn(
                                ctx,
                                0,
                                0,
                                std::ptr::null_mut(),
                                &mut child,
                                &scope,
                                fuel_child,
                                environment
                            ),
                            0
                        );
                        last_child = child;
                        if matches!(mode, 0 | 10) {
                            // Spawn now returns at issuance. Setup must regain
                            // the head before publishing the second child's
                            // diagnostic start flag, so the older child cannot
                            // hold retirement ahead of that child's spawn.
                            let _role = role::Guard::new(ctx);
                            at_head(ctx).unwrap();
                        }
                    }
                    let mut result = [0u64; 2];
                    assert_eq!(
                        hirsel_rt_v2_task_join(
                            ctx,
                            0,
                            0,
                            &scope,
                            &mut last_child,
                            result.as_mut_ptr().cast(),
                            16
                        ),
                        0
                    );
                    assert!(
                        matches!(mode, 3 | 4 | 8 | 9),
                        "the younger fuel stop must precede root continuation"
                    );
                    assert_eq!(hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0), 0);
                    let _role = role::Guard::new(ctx);
                    let task = current(ctx);
                    driver(ctx).finishing(task);
                    0
                },
            );
            assert_eq!(
                outcome.ended,
                if matches!(mode, 3 | 4 | 8 | 9) {
                    Ok(0)
                } else {
                    Err(Stop::BudgetExceeded(if matches!(mode, 5 | 7) {
                        hirsel_run_schema::BudgetKind::Memory
                    } else {
                        hirsel_run_schema::BudgetKind::Fuel
                    }))
                }
            );
            assert!(probe.older_retired.load(Ordering::Acquire));
            assert_eq!(
                probe.younger_passed.load(Ordering::Acquire),
                matches!(mode, 3 | 4 | 8 | 9)
            );
        }
    }

    #[test]
    fn ordered_native_younger_fuel_exhaustion_waits_for_the_older_retiring_turn() {
        younger_budget(&[1, 2]);
    }

    #[test]
    fn ordered_native_heap_and_region_allowance_pauses_resume_only_after_head_promotion() {
        younger_budget(&[3, 4]);
    }

    #[test]
    fn ordered_native_speculative_envelopes_choose_the_first_memory_or_fuel_stop() {
        younger_budget(&[5, 6, 7]);
    }

    #[test]
    fn ordered_native_envelope_overflow_saves_before_borrowing_the_heap() {
        younger_budget(&[8, 9]);
    }

    #[test]
    fn ordered_native_pure_fuel_loop_observes_external_stop_without_reentering_abandonment() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        for width in [2, 8] {
            let started = Arc::new(AtomicBool::new(false));
            let seen = started.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let control = run.control();
            let stopper = std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !started.load(Ordering::Acquire) {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "ordered loop did not start"
                    );
                    std::thread::yield_now();
                }
                control.stop();
            });
            let outcome = run.run(
                |ctx| unsafe { enable(ctx, Policy::admit(&Default::default(), None)) },
                move |ctx| {
                    seen.store(true, Ordering::Release);
                    loop {
                        unsafe { charge(ctx) };
                    }
                },
            );
            stopper.join().unwrap();
            assert_eq!(outcome.ended, Err(Stop::Requested));
        }
    }

    #[test]
    fn ordered_native_fuel_exhaustion_is_exact_across_chunks_and_worker_widths() {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        for width in [2, 4, 8] {
            for limit in [
                0,
                1,
                crate::budget::FUEL_CHUNK,
                crate::budget::FUEL_CHUNK + 3,
            ] {
                let charged = Arc::new(AtomicU64::new(0));
                let seen = charged.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.fuel = Some(limit);
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    |ctx| unsafe { enable(ctx, Policy::admit(&Default::default(), None)) },
                    move |ctx| loop {
                        unsafe { charge(ctx) };
                        seen.fetch_add(1, Ordering::Relaxed);
                    },
                );
                assert_eq!(
                    outcome.ended,
                    Err(Stop::BudgetExceeded(hirsel_run_schema::BudgetKind::Fuel))
                );
                assert_eq!(
                    charged.load(Ordering::Relaxed),
                    limit,
                    "width {width}, limit {limit}"
                );
            }
        }
    }

    #[test]
    fn ordered_native_boundaries_settle_partial_chunks_without_fuel_operations_or_polls() {
        for width in [2, 4, 8] {
            let mut run = crate::host::BodyRun::new(width, true);
            run.fuel = Some(11);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |ctx| unsafe { enable(ctx, Policy::admit(&Default::default(), None)) },
                |ctx| unsafe {
                    let mut word = 0;
                    let zero = 0u64;
                    assert_eq!(
                        crate::tasks::hirsel_rt_v2_atomic_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut word,
                            (&raw const zero).cast(),
                            8
                        ),
                        0
                    );
                    for consumed in [3, 8] {
                        for _ in 0..consumed {
                            charge(ctx);
                        }
                        let value = consumed;
                        assert_eq!(
                            crate::tasks::hirsel_rt_v2_atomic_store(
                                ctx,
                                &word,
                                (&raw const value).cast(),
                                8
                            ),
                            0
                        );
                        let _role = crate::tasks::role::Guard::new(ctx);
                        let driver = driver(ctx);
                        let frontier = driver.frontier.as_ref().unwrap();
                        assert_eq!(frontier.settled.fuel, if consumed == 3 { 8 } else { 0 });
                        assert_eq!(frontier.retired(), if consumed == 3 { 2 } else { 3 });
                        assert_eq!(driver.recorded[&current(ctx)], frontier.retired());
                    }
                    charge(ctx);
                    panic!("the twelfth point must stop");
                },
            );
            assert_eq!(
                outcome.ended,
                Err(Stop::BudgetExceeded(hirsel_run_schema::BudgetKind::Fuel))
            );
        }
    }

    #[test]
    fn ordered_unbounded_native_fuel_refills_preserve_semantic_ordinals() {
        let mut run = crate::host::BodyRun::new(4, true);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { enable(ctx, Policy::admit(&Default::default(), None)) },
            |ctx| unsafe {
                for _ in 0..(3 * crate::budget::FUEL_CHUNK + 5) {
                    charge(ctx);
                }
                let _role = crate::tasks::role::Guard::new(ctx);
                let task = current(ctx);
                let driver = driver(ctx);
                assert_eq!(driver.frontier.as_ref().unwrap().settled.fuel, u64::MAX);
                assert_eq!(driver.frontier.as_ref().unwrap().retired(), 0);
                assert!(driver.submitted.is_empty());
                assert!(driver.recorded.is_empty());
                driver.finishing(task);
                0
            },
        );
        assert_eq!(outcome.ended, Ok(0));
    }

    fn image_layout(size: u64) -> crate::Layout {
        crate::Layout {
            magic: crate::LAYOUT_MAGIC,
            version: crate::LAYOUT_VERSION,
            size,
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
        }
    }

    #[test]
    fn native_channel_and_mutex_entries_preserve_owned_carriers_and_aliases() {
        let image = image_layout(16);
        let close = crate::Step {
            kind: crate::STEP_CLOSE_HANDLE,
            field: 0,
            variant: 0,
            span: 0,
            offset: 0,
            stride: 0,
            count: 0,
            count_offset: 0,
            layout: std::ptr::null(),
        };
        let wrapper = crate::Layout {
            flags: crate::LAYOUT_FROZEN | crate::LAYOUT_MUTEX_ROOT,
            steps: &close,
            step_count: 1,
            field_count: 1,
            ..image_layout(8)
        };
        let layouts = [&image as *const crate::Layout, &wrapper];
        let mut run = crate::host::BodyRun::new(8, false);
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = layouts.len() as u32;
        // SAFETY: only supported E entries run until explicitly quiescent
        // fixture cleanup; no other tasks or observable host services exist.
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe { enable(ctx, Policy::admit(&Default::default(), None)) },
            |ctx| {
                use super::super::super::*;
                unsafe {
                    let none = [0u64; 2];
                    let mut endpoints = [0u64; 2];
                    assert_eq!(
                        hirsel_rt_v2_channel_open(
                            ctx,
                            std::ptr::null_mut(),
                            endpoints.as_mut_ptr(),
                            &0,
                            none.as_ptr().cast(),
                            2
                        ),
                        0
                    );
                    let mut duplicate = 0;
                    assert_eq!(
                        hirsel_rt_v2_sender_dup(
                            ctx,
                            std::ptr::null_mut(),
                            &mut duplicate,
                            &0,
                            &endpoints[0]
                        ),
                        0
                    );
                    let mut message = [1u64, 55];
                    let mut status = u32::MAX;
                    assert_eq!(
                        hirsel_rt_v2_channel_send(
                            ctx,
                            &mut status,
                            &mut duplicate,
                            message.as_mut_ptr().cast()
                        ),
                        0
                    );
                    assert_eq!(status, crate::channels::DONE);
                    assert_eq!(message, none);
                    role::enter(ctx);
                    let channel = scheduler(ctx).channels.channel(endpoints[1], false);
                    let mut received = [0u64; 2];
                    assert_eq!(
                        scheduler(ctx)
                            .channels
                            .receive(channel, received.as_mut_ptr().cast()),
                        Ok(true)
                    );
                    assert_eq!(received, [1, 55]);
                    scheduler(ctx).channels.close(endpoints[1]);
                    role::release(ctx);
                    message = [1, 66];
                    assert_eq!(
                        hirsel_rt_v2_channel_send(
                            ctx,
                            &mut status,
                            &mut duplicate,
                            message.as_mut_ptr().cast()
                        ),
                        0
                    );
                    assert_eq!(status, crate::channels::CLOSED);
                    assert_eq!(message, [1, 66], "closed send returns ownership unchanged");
                    role::enter(ctx);
                    scheduler(ctx).channels.close(endpoints[0]);
                    scheduler(ctx).channels.close(duplicate);
                    // Close the inert endpoints as quiescent fixture cleanup;
                    // receiver/drop entry integration follows later.
                    role::release(ctx);

                    let mut payload = [1u64, 77];
                    let mut mutex = 0;
                    assert_eq!(
                        hirsel_rt_v2_mutex_create(
                            ctx,
                            std::ptr::null_mut(),
                            &mut mutex,
                            payload.as_mut_ptr().cast(),
                            none.as_ptr().cast(),
                            2,
                            3
                        ),
                        0
                    );
                    assert_eq!(payload, none);
                    assert_eq!(
                        hirsel_rt_v2_mutex_take(
                            ctx,
                            &mut status,
                            &mutex,
                            payload.as_mut_ptr().cast()
                        ),
                        0
                    );
                    assert_eq!(status, 0);
                    assert_eq!(payload, [1, 77]);
                    payload[1] = 88;
                    assert_eq!(
                        hirsel_rt_v2_mutex_put(ctx, &mutex, payload.as_mut_ptr().cast()),
                        0
                    );
                    assert_eq!(payload, none);
                    assert_eq!(
                        hirsel_rt_v2_mutex_take(
                            ctx,
                            &mut status,
                            &mutex,
                            payload.as_mut_ptr().cast()
                        ),
                        0
                    );
                    assert_eq!(payload, [1, 88]);
                    // Failure fixture while the owner holds the empty mutex;
                    // MutexCause must apply the chain to this continuation.
                    role::enter(ctx);
                    let word = mutex_word(&mutex);
                    let failure = crate::FailureRecord {
                        kind: 1,
                        rule: 2,
                        site: 3,
                        operation_site: 4,
                    };
                    scheduler(ctx).sync.kill(word, vec![(failure, Vec::new())]);
                    role::release(ctx);
                    assert_eq!(hirsel_rt_v2_mutex_cause(ctx, &mutex), 0);
                    assert!((*ctx).failure.active());
                    (*ctx).failure.clear();
                    // Last wrapper release now retires root.release and the
                    // cell's drop.trivial through their strict head callbacks.
                    assert_eq!(
                        crate::roots::hirsel_rt_v2_root_drop(
                            ctx,
                            mutex as *mut crate::Header,
                            0,
                            0
                        ),
                        0
                    );
                    role::enter(ctx);
                    let task = current(ctx);
                    let driver = driver(ctx);
                    assert_eq!(driver.submitted[&task], 11);
                    assert_eq!(driver.recorded[&task], 11);
                    assert!(driver.returned.is_empty());
                    driver.finishing(task);
                    role::release(ctx);
                }
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }

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
