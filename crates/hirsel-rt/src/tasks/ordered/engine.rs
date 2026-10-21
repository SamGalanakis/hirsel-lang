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
/// no budgets, fuel, root release, host or terminal effects. Native task tests
/// use only nondetached children, supported joins and empty scope exits, with
/// dropless closure environments; no task sets or deadlines are admitted.
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
    let mut result = None;
    let progress = with_frontier(ctx, |frontier, objects| {
        let mut backend = HeadEffect {
            objects,
            effect: Some(effect),
            result: &mut result,
        };
        frontier.apply_head(
            empty(),
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
    let mut result = None;
    let progress = with_frontier(ctx, |frontier, backend| {
        frontier.continue_head(empty(), backend, || {
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
                    // Quiescent destruction is explicit test cleanup; native
                    // ordered drop boundaries remain a separate integration.
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
                    assert_eq!(driver.submitted[&task], 9);
                    assert_eq!(driver.recorded[&task], 9);
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
