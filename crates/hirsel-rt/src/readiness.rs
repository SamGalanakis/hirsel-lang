//! Host readiness for the cooperative schedulers (the interpreter's and the compiled run's).
//! A host operation that would block (a socket, a resolver wakeup, stdin, a process job)
//! records its descriptor interest or deadline under the scheduler's opaque wait token. The
//! interests live in one persistent epoll set and the deadlines in a timer heap, so a
//! scheduling point asks [`Readiness::ready`] for exactly the tokens that are ready, at a cost
//! independent of how many operations are parked, and with no runnable task
//! [`Readiness::wait`] sleeps in one `epoll_wait` until a descriptor is ready or the earliest
//! deadline passes. A timer wait is a deadline with no descriptor interest. A runner call
//! waits for a delivery: its callee's end makes it ready ([`Readiness::deliver`]).
//!
//! The runs of one execution (RUN-12) share one Readiness, so an idle execution sleeps on
//! every run's interests at once. Each run's handle is [`tagged`](Readiness::tagged): its
//! tokens carry the run's tag in bits 48 to 61, and it reports only its own tokens.
use crate::id_map::{IdMap, IdSet};
use crate::io::AdapterError;
use crate::net::{NetError, SocketAddr};
use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

#[derive(Clone, Default)]
pub struct Readiness(Rc<RefCell<ReadyState>>, u64);
/// The token bits a run's tag occupies: above every interpreter and host wait token's
/// counter, below the host's own token ranges (bits 62 and 63).
const TAG_SHIFT: u32 = 48;
const TAG_MASK: u64 = ((1 << 14) - 1) << TAG_SHIFT;
#[derive(Default)]
struct ReadyState {
    current: u64,
    virtual_time: Option<u64>,
    pending: IdMap<Pending>,
    /// Tokens whose Pending a provider borrowed since the last scheduling point: their
    /// interest and deadline are registered before the next readiness question.
    touched: IdSet,
    /// The epoll set, created at the first descriptor interest.
    epoll: Option<OwnedFd>,
    /// Each registered descriptor's tokens and the events each waits for; the descriptor's
    /// registration is the union.
    fds: IdMap<Vec<(u64, libc::c_short)>>,
    /// Deadlines by time, then token; an entry whose token no longer has that deadline is
    /// stale and skipped.
    timers: BinaryHeap<Reverse<(Instant, u64)>>,
    virtual_timers: BinaryHeap<Reverse<(u64, u64)>>,
    /// Parked tokens with neither interest nor deadline, or whose descriptor epoll cannot
    /// watch (a regular file): ready at every scheduling point, as a poll(2) of them is.
    always: IdSet,
    /// Tokens the idle wait found ready, for the next scheduling point.
    woken: IdSet,
}
/// One parked operation's interest plus the progress a retried network
/// operation keeps across readiness waits (CooperativeNet).
#[derive(Default)]
pub(crate) struct Pending {
    pub(crate) interest: Option<(libc::c_int, libc::c_short)>,
    pub(crate) deadline: Option<Instant>,
    virtual_deadline: Option<u64>,
    pub(crate) resolver: Option<(Receiver<Result<Vec<SocketAddr>, NetError>>, UnixStream)>,
    pub(crate) addresses: Option<VecDeque<SocketAddr>>,
    pub(crate) connecting: Option<OwnedFd>,
    pub(crate) last_error: Option<NetError>,
    pub(crate) offset: usize,
    pub(crate) committed: bool,
    /// A host process operation running on its helper thread; dropping it kills the child.
    pub(crate) process: Option<crate::process::HostJob>,
    /// Its task parked ([`Readiness::parked`]): only then is it reported ready.
    parked: bool,
    /// It waits for a delivery, not for a descriptor or deadline.
    delivery: bool,
    /// What the epoll set and the timer heaps hold for this token.
    registered: Option<(libc::c_int, libc::c_short)>,
    timer: Option<Instant>,
    virtual_timer: Option<u64>,
}
impl Readiness {
    /// The handle of run `tag` of this execution (the root is 0) on the same readiness.
    pub fn tagged(&self, tag: u64) -> Self {
        assert!(
            tag << TAG_SHIFT & !TAG_MASK == 0,
            "a run tag within 14 bits"
        );
        Self(self.0.clone(), tag << TAG_SHIFT)
    }
    /// `token` under this run's tag. Idempotent, so a tagged token passes through.
    fn tag(&self, token: u64) -> u64 {
        debug_assert!(
            token & TAG_MASK & !self.1 == 0,
            "a wait token within its tag bits"
        );
        token | self.1
    }
    fn own(&self, token: u64) -> bool {
        token & TAG_MASK == self.1
    }
    /// The entered request waits for a delivery ([`deliver`](Self::deliver)).
    pub(crate) fn await_delivery(&self) {
        self.pending().delivery = true;
    }
    /// The delivery `token` waits for arrived: it is ready at the next scheduling point.
    pub fn deliver(&self, token: u64) {
        let token = self.tag(token);
        let mut state = self.0.borrow_mut();
        if state.pending.get(&token).is_some_and(|p| p.delivery) {
            state.woken.insert(token);
        }
    }
    /// Harness-only virtual time: samples are stable while runnable work remains.
    pub(crate) fn virtual_clock(ticks: u64) -> Self {
        let readiness = Self::default();
        readiness.0.borrow_mut().virtual_time = Some(ticks);
        readiness
    }
    pub(crate) fn virtual_time(&self) -> Option<u64> {
        self.0.borrow().virtual_time
    }
    pub(crate) fn pending(&self) -> std::cell::RefMut<'_, Pending> {
        std::cell::RefMut::map(self.0.borrow_mut(), |state| {
            let token = state.current;
            state.touched.insert(token);
            state.pending.entry(token).or_default()
        })
    }

    /// Select the opaque operation token before asking RealIo to execute a request.
    pub fn enter(&self, token: u64) {
        self.0.borrow_mut().current = self.tag(token);
    }
    pub fn finish(&self, token: u64) {
        let token = self.tag(token);
        let mut state = self.0.borrow_mut();
        state.touched.remove(&token);
        state.always.remove(&token);
        state.woken.remove(&token);
        if let Some(pending) = state.pending.remove(&token)
            && let Some((fd, _)) = pending.registered
        {
            state.unregister(token, fd);
        }
    }
    /// The task of the request blocked under `token` parked: from the next scheduling point
    /// on, [`ready`](Self::ready) reports the token when its interest is ready.
    pub fn parked(&self, token: u64) {
        let token = self.tag(token);
        let mut state = self.0.borrow_mut();
        let virtual_time = state.virtual_time;
        let Some(pending) = state.pending.get_mut(&token) else {
            return;
        };
        pending.parked = true;
        // A deadline that passed before the park left the heap unreported.
        let due = match (virtual_time, pending.virtual_deadline) {
            (Some(now), Some(deadline)) => now >= deadline,
            _ => pending.deadline.is_some_and(|d| Instant::now() >= d),
        };
        if due {
            state.woken.insert(token);
        }
    }
    /// After a packet is sent or a write is accepted, the operation must complete
    /// with its own outcome rather than report an effect-free cancellation.
    pub fn cancellable(&self, token: u64) -> bool {
        self.0
            .borrow()
            .pending
            .get(&self.tag(token))
            .is_none_or(|p| !p.committed)
    }
    /// A scheduling point: the parked tokens that are ready now (a watched descriptor is
    /// ready, a deadline passed, or the token waits on nothing epoll can watch), in token
    /// order. Never blocks; each ready token's operation is re-issued by the caller.
    pub fn ready(&self) -> Result<Vec<u64>, AdapterError> {
        let mut state = self.0.borrow_mut();
        let woken = Self::poll(&mut state)?;
        let others: Vec<u64> = woken
            .members_unordered()
            .filter(|t| !self.own(*t))
            .collect();
        let mut ready: Vec<u64> = woken.members_unordered().filter(|t| self.own(*t)).collect();
        ready.extend(state.always.members_unordered().filter(|t| self.own(*t)));
        ready.sort_unstable();
        ready.dedup();
        let (mut parked, waiting): (Vec<u64>, Vec<u64>) = ready
            .into_iter()
            .filter(|token| state.pending.contains_key(token))
            .partition(|token| state.pending[token].parked);
        // The idle wait's findings for a request whose task has not parked yet stay, and so
        // do the other runs' findings.
        let waiting: Vec<u64> = waiting
            .into_iter()
            .filter(|t| !state.always.contains(t))
            .collect();
        for token in waiting.into_iter().chain(others) {
            state.woken.insert(token);
        }
        parked.sort_unstable();
        Ok(parked.into_iter().map(|t| t & !TAG_MASK).collect())
    }
    /// Whether [`ready`](Self::ready) would report a token of this run now, without taking
    /// any: what epoll and the timers found waits for that scheduling point.
    pub fn has_ready(&self) -> Result<bool, AdapterError> {
        let mut state = self.0.borrow_mut();
        let woken = Self::poll(&mut state)?;
        state.woken = woken;
        let state = &*state;
        Ok(state
            .woken
            .members_unordered()
            .chain(state.always.members_unordered())
            .any(|t| self.own(t) && state.pending.get(&t).is_some_and(|p| p.parked)))
    }
    /// Every token found ready so far: the idle wait's findings, the descriptors epoll
    /// reports ready now, and the passed deadlines.
    fn poll(state: &mut ReadyState) -> Result<IdSet, AdapterError> {
        state.sync()?;
        let mut ready = std::mem::take(&mut state.woken);
        if !state.fds.is_empty() {
            state.epoll_events(0, &mut ready)?;
        }
        state.due(&mut ready);
        Ok(ready)
    }
    /// A timer wait for CLOCK_MONOTONIC `deadline` nanoseconds under the entered token:
    /// true once it has passed (the token's interest is then finished), otherwise the
    /// deadline is recorded as the token's interest and the caller parks.
    pub(crate) fn timer(&self, deadline: u64) -> Result<bool, AdapterError> {
        if let Some(now) = self.virtual_time() {
            let token = self.0.borrow().current;
            if now >= deadline {
                self.finish(token);
                return Ok(true);
            }
            self.pending().virtual_deadline = Some(deadline);
            return Ok(false);
        }
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: now is writable; CLOCK_MONOTONIC is the clock std::time::Instant reads.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0 {
            return Err(AdapterError::Unsupported("host monotonic clock".into()));
        }
        let ticks = (now.tv_sec as u64) * 1_000_000_000 + now.tv_nsec as u64;
        let token = self.0.borrow().current;
        if ticks >= deadline {
            self.finish(token);
            return Ok(true);
        }
        let left = std::time::Duration::from_nanos(deadline - ticks);
        self.pending().deadline = Some(Instant::now() + left);
        Ok(false)
    }
    /// Called only with no runnable task: sleep until a parked token may be ready. A false
    /// result means this provider has no external readiness source (e.g. the remaining waits
    /// are scripted).
    pub fn wait(&self) -> Result<bool, AdapterError> {
        let mut state = self.0.borrow_mut();
        state.sync()?;
        if let Some(now) = state.virtual_time {
            let next = state.next_virtual();
            if let Some(next) = next {
                state.virtual_time = Some(now.max(next));
                return Ok(true);
            }
            return Ok(false);
        }
        if !state.always.is_empty() || !state.woken.is_empty() {
            return Ok(true);
        }
        let deadline = state.next_timer();
        // No descriptor and no timer: nothing external can make progress. With only
        // timers left, the wait below sleeps until the earliest is due.
        if state.fds.is_empty() && deadline.is_none() {
            return Ok(false);
        }
        let timeout = deadline.map_or(-1, |d| {
            let left = d.saturating_duration_since(Instant::now());
            left.as_millis()
                .saturating_add(u128::from(!left.is_zero()))
                .min(libc::c_int::MAX as u128) as libc::c_int
        });
        let mut woken = std::mem::take(&mut state.woken);
        if state.fds.is_empty() {
            // SAFETY: a plain sleep; no descriptor is involved. A signal restarts it.
            while unsafe { libc::poll(std::ptr::null_mut(), 0, timeout) } < 0 {
                if errno() != libc::EINTR {
                    return Err(AdapterError::Unsupported("host readiness poll".into()));
                }
            }
        } else {
            state.epoll_events(timeout, &mut woken)?;
        }
        state.woken = woken;
        Ok(true)
    }
}

impl ReadyState {
    /// Register the interest and deadline of every token a provider touched.
    fn sync(&mut self) -> Result<(), AdapterError> {
        let mut touched = std::mem::take(&mut self.touched);
        for token in touched.drain_unordered() {
            let Some(pending) = self.pending.get_mut(&token) else {
                continue;
            };
            let (interest, registered, delivery) =
                (pending.interest, pending.registered, pending.delivery);
            let mut timers = Vec::new();
            if let Some(deadline) = pending.deadline
                && pending.timer != Some(deadline)
            {
                pending.timer = Some(deadline);
                timers.push(deadline);
            }
            let mut virtual_timer = None;
            if let Some(deadline) = pending.virtual_deadline
                && pending.virtual_timer != Some(deadline)
            {
                pending.virtual_timer = Some(deadline);
                virtual_timer = Some(deadline);
            }
            let watched = interest.is_some() || pending.deadline.is_some();
            let timed = pending.virtual_deadline.is_some();
            for deadline in timers {
                self.timers.push(Reverse((deadline, token)));
            }
            if let Some(deadline) = virtual_timer {
                self.virtual_timers.push(Reverse((deadline, token)));
            }
            if let Some((fd, _)) = registered
                && interest.is_none_or(|(new, _)| new != fd)
            {
                self.unregister(token, fd);
            }
            let mut always = !watched && !timed && !delivery;
            if let Some((fd, events)) = interest {
                // Re-registered on every park, even unchanged: a descriptor closed and
                // reopened under the same number left the epoll set when it closed.
                let entry = self.fds.entry(fd as u64).or_default();
                match entry.iter_mut().find(|(t, _)| *t == token) {
                    Some(row) => row.1 = events,
                    None => entry.push((token, events)),
                }
                let union = entry.iter().fold(0, |all, (_, e)| all | *e);
                if self.control(fd, union)? {
                    self.pending.get_mut(&token).unwrap().registered = Some((fd, events));
                } else {
                    // A descriptor epoll cannot watch is always ready, as poll(2) reports it.
                    self.forget(token, fd);
                    self.pending.get_mut(&token).unwrap().registered = None;
                    always = true;
                }
            }
            if always {
                self.always.insert(token);
            } else {
                self.always.remove(&token);
            }
        }
        Ok(())
    }
    /// Drop `token` from descriptor `fd`'s watchers and update or remove its registration.
    fn unregister(&mut self, token: u64, fd: libc::c_int) {
        if let Some(pending) = self.pending.get_mut(&token) {
            pending.registered = None;
        }
        let Some(rows) = self.fds.get_mut(&(fd as u64)) else {
            return;
        };
        rows.retain(|(t, _)| *t != token);
        if rows.is_empty() {
            self.fds.remove(&(fd as u64));
            if let Some(epoll) = &self.epoll {
                // A descriptor already closed left the set by itself (ENOENT or EBADF).
                // SAFETY: plain epoll control call on our own set.
                unsafe {
                    libc::epoll_ctl(
                        epoll.as_raw_fd(),
                        libc::EPOLL_CTL_DEL,
                        fd,
                        std::ptr::null_mut(),
                    )
                };
            }
        } else {
            let union = rows.iter().fold(0, |all, (_, e)| all | *e);
            // An unwatchable or closed descriptor cannot be registered; its remaining
            // tokens keep their rows and the next park re-registers them.
            let _ = self.control(fd, union);
        }
    }
    fn forget(&mut self, token: u64, fd: libc::c_int) {
        if let Some(rows) = self.fds.get_mut(&(fd as u64)) {
            rows.retain(|(t, _)| *t != token);
            if rows.is_empty() {
                self.fds.remove(&(fd as u64));
            }
        }
    }
    /// Watch `fd` for `events` (poll(2) bits, which epoll shares): modify its registration,
    /// adding it when absent. False when epoll cannot watch the descriptor.
    fn control(&mut self, fd: libc::c_int, events: libc::c_short) -> Result<bool, AdapterError> {
        let epoll = match &self.epoll {
            Some(epoll) => epoll.as_raw_fd(),
            None => {
                // SAFETY: plain epoll creation; the descriptor is owned at once.
                let created = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
                if created == -1 {
                    return Err(AdapterError::Unsupported("host readiness epoll".into()));
                }
                // SAFETY: a fresh descriptor owned by nobody else.
                self.epoll = Some(unsafe { OwnedFd::from_raw_fd(created) });
                created
            }
        };
        let mut event = libc::epoll_event {
            events: events as u16 as u32,
            u64: fd as u64,
        };
        for op in [libc::EPOLL_CTL_MOD, libc::EPOLL_CTL_ADD] {
            // SAFETY: event is a valid epoll_event; fd is a live descriptor its provider holds.
            if unsafe { libc::epoll_ctl(epoll, op, fd, &mut event) } == 0 {
                return Ok(true);
            }
            match errno() {
                libc::ENOENT if op == libc::EPOLL_CTL_MOD => continue,
                libc::EPERM => return Ok(false),
                _ => return Err(AdapterError::Unsupported("host readiness epoll".into())),
            }
        }
        unreachable!()
    }
    /// Add the tokens of descriptors epoll reports ready within `timeout` milliseconds.
    fn epoll_events(
        &mut self,
        timeout: libc::c_int,
        ready: &mut IdSet,
    ) -> Result<(), AdapterError> {
        const BATCH: usize = 256;
        let epoll = self.epoll.as_ref().unwrap().as_raw_fd();
        let mut events = [libc::epoll_event { events: 0, u64: 0 }; BATCH];
        let mut timeout = timeout;
        loop {
            // SAFETY: events holds BATCH writable entries.
            let n = unsafe { libc::epoll_wait(epoll, events.as_mut_ptr(), BATCH as i32, timeout) };
            if n < 0 {
                if errno() == libc::EINTR {
                    continue;
                }
                return Err(AdapterError::Unsupported("host readiness epoll".into()));
            }
            for event in &events[..n as usize] {
                let fd = event.u64 as libc::c_int;
                let revents = event.events;
                for (token, wanted) in self.fds.get(&(fd as u64)).into_iter().flatten() {
                    let wanted =
                        (*wanted as u16 as u32) | libc::EPOLLERR as u32 | libc::EPOLLHUP as u32;
                    if revents & wanted != 0 {
                        ready.insert(*token);
                    }
                }
            }
            if (n as usize) < BATCH {
                return Ok(());
            }
            // A full batch: take the rest without waiting.
            timeout = 0;
        }
    }
    /// Add the tokens whose deadline has passed.
    fn due(&mut self, ready: &mut IdSet) {
        if let Some(now) = self.virtual_time {
            while let Some(&Reverse((deadline, token))) = self.virtual_timers.peek() {
                if deadline > now {
                    break;
                }
                self.virtual_timers.pop();
                if let Some(pending) = self.pending.get_mut(&token)
                    && pending.virtual_timer == Some(deadline)
                {
                    pending.virtual_timer = None;
                    ready.insert(token);
                }
            }
            return;
        }
        if self.timers.is_empty() {
            return;
        }
        let now = Instant::now();
        while let Some(&Reverse((deadline, token))) = self.timers.peek() {
            if deadline > now {
                break;
            }
            self.timers.pop();
            if let Some(pending) = self.pending.get_mut(&token)
                && pending.timer == Some(deadline)
            {
                pending.timer = None;
                ready.insert(token);
            }
        }
    }
    /// The earliest live deadline, dropping stale heap entries.
    fn next_timer(&mut self) -> Option<Instant> {
        while let Some(&Reverse((deadline, token))) = self.timers.peek() {
            if self
                .pending
                .get(&token)
                .is_some_and(|p| p.timer == Some(deadline))
            {
                return Some(deadline);
            }
            self.timers.pop();
        }
        None
    }
    fn next_virtual(&mut self) -> Option<u64> {
        while let Some(&Reverse((deadline, token))) = self.virtual_timers.peek() {
            if self
                .pending
                .get(&token)
                .is_some_and(|p| p.virtual_timer == Some(deadline))
            {
                return Some(deadline);
            }
            self.virtual_timers.pop();
        }
        None
    }
}

fn errno() -> libc::c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The deterministic identity of cooperative waits, kept alike by live recording (`RealIo`)
/// and replay (`SimIo`): the request that first blocks under a token gets the run's next
/// creation ordinal, and each wait counts the scheduling points it waited through since its
/// task parked or it was last ready. Tapes carry `blocked` completions and, at a scheduling point,
/// one `ready <ordinal> <points>` event per wait it re-issues, in ordinal order (DET-1).
#[derive(Clone, Debug, Default)]
pub(crate) struct WaitLog {
    current: u64,
    next: u64,
    /// Scheduling points with a parked wait so far.
    point: u64,
    rows: IdMap<WaitRow>,
    /// Each row's token by its ordinal.
    ordinals: IdMap<u64>,
}
#[derive(Clone, Debug)]
pub(crate) struct WaitRow {
    pub(crate) ordinal: u64,
    /// The scheduling point at which its task parked or it was last ready.
    since: u64,
    /// Its task parked.
    pub(crate) parked: bool,
    pub(crate) committed: bool,
}
impl WaitLog {
    pub(crate) fn enter(&mut self, token: u64) {
        self.current = token;
    }
    pub(crate) fn finish(&mut self, token: u64) {
        if let Some(row) = self.rows.remove(&token) {
            self.ordinals.remove(&row.ordinal);
        }
    }
    pub(crate) fn current(&self) -> u64 {
        self.current
    }
    /// The entered request blocked: its row, created with the next ordinal the first time.
    pub(crate) fn park(&mut self, committed: bool) -> &mut WaitRow {
        let (next, point) = (&mut self.next, self.point);
        let row = self.rows.entry(self.current).or_insert_with(|| {
            *next += 1;
            WaitRow {
                ordinal: *next - 1,
                since: point,
                parked: false,
                committed,
            }
        });
        row.committed = committed;
        self.ordinals.insert(row.ordinal, self.current);
        row
    }
    /// The task of `token`'s blocked request parked at this point.
    pub(crate) fn parked(&mut self, token: u64) {
        let point = self.point;
        if let Some(row) = self.rows.get_mut(&token) {
            row.since = point;
            row.parked = true;
        }
    }
    pub(crate) fn rows_of(&self, token: u64) -> Option<&WaitRow> {
        self.rows.get(&token)
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    /// A new scheduling point.
    pub(crate) fn point(&mut self) {
        self.point += 1;
    }
    /// The scheduling points `token`'s wait has waited through before this one.
    pub(crate) fn points(&self, token: u64) -> Option<u64> {
        let row = self.rows.get(&token)?;
        Some(self.point - row.since - 1)
    }
    /// The points `token`'s wait will have waited through at the next scheduling point.
    pub(crate) fn next_points(&self, token: u64) -> Option<u64> {
        Some(self.point - self.rows.get(&token)?.since)
    }
    /// `token`'s wait is ready at this scheduling point: its `ready` event's wait and points.
    pub(crate) fn ready(&mut self, token: u64) -> Option<(u64, u64)> {
        let points = self.points(token)?;
        let point = self.point;
        let row = self.rows.get_mut(&token)?;
        row.since = point;
        Some((row.ordinal, points))
    }
    /// The parked token of wait `ordinal`.
    pub(crate) fn token_of(&self, ordinal: u64) -> Option<u64> {
        self.ordinals.get(&ordinal).copied()
    }
}
