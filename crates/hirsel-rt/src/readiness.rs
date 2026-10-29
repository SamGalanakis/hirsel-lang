//! Host readiness for the cooperative schedulers (the interpreter's and the compiled run's).
//! A host operation that would block (a socket, a resolver wakeup, stdin, a process job)
//! records its descriptor interest or deadline under the scheduler's opaque wait token. The
//! interests live in one persistent epoll set and the deadlines in exact ordered indexes, so a
//! scheduling point asks [`Readiness::ready`] for exactly the tokens that are ready, at a cost
//! independent of how many operations are parked, and with no runnable task
//! [`Readiness::wait`] sleeps in one `epoll_wait` until a descriptor is ready or the earliest
//! deadline passes. A timer wait is a deadline with no descriptor interest. A runner call
//! waits for a delivery: its callee's end makes it ready ([`Readiness::deliver`]).
//!
//! The runs of one execution (RUN-12) share one Readiness, so an idle execution sleeps on
//! every run's interests at once. Each run's handle is [`tagged`](Readiness::tagged): its
//! tokens carry the run's tag in bits 48 to 61, and it reports only its own tokens.
//!
//! The epoll layer follows mio's selector (mio a88e5e11 `src/sys/unix/selector/epoll.rs`):
//! descriptors are registered edge-triggered (`interests_to_epoll`, :129-145) and stay
//! registered between operations, so a connection that blocks request after request costs
//! one `epoll_ctl` per park instead of the MOD, ADD and DEL a level-triggered set that drops
//! idle descriptors needs. Deviations, each for a reason of this runtime:
//! - Keyed by descriptor, not by token: an event names a descriptor and is fanned out to
//!   the tokens waiting on it, since a read and a write of one socket park under two tokens
//!   and epoll holds one registration per descriptor.
//! - Re-armed with `EPOLL_CTL_MOD` at every park instead of registered once. Providers own
//!   and close their descriptors (sockets, resolver and process wakeups, a retried
//!   connect's socket) without telling Readiness, so a park cannot know whether its
//!   descriptor number still names the registered file. MOD answers that (ENOENT: it was
//!   closed, so ADD) and, edge-triggered, re-evaluates readiness, so data that arrived while
//!   no token watched the descriptor is reported at once. This takes the place of tokio's
//!   per-resource readiness bits cleared on WouldBlock (`runtime/io/scheduled_io.rs`
//!   `clear_readiness`, :372). Register-once would save that one call per park; the
//!   measurement is in the crate README.
use crate::deadlines::Deadlines;
use crate::id_map::{IdMap, IdSet};
use crate::io::AdapterError;
use crate::net::{NetError, SocketAddr};
use std::collections::VecDeque;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

/// A retained eventfd for execution work whose producer is another executor.
/// As with the driver's own eventfd, notifications coalesce and an early
/// notification remains readable until home drains it.
pub(crate) struct Signal(OwnedFd);
impl Signal {
    pub(crate) fn new() -> Result<Self, AdapterError> {
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(AdapterError::Unsupported("execution work eventfd".into()));
        }
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
    pub(crate) fn fd(&self) -> std::os::fd::RawFd {
        self.0.as_raw_fd()
    }
    pub(crate) fn notify(&self) {
        let one = 1u64;
        loop {
            let n = unsafe { libc::write(self.fd(), (&raw const one).cast(), 8) };
            if n == 8 || (n < 0 && errno() == libc::EAGAIN) {
                return;
            }
            if n < 0 && errno() == libc::EINTR {
                continue;
            }
            crate::fault("execution work eventfd write");
        }
    }
    pub(crate) fn drain(&self) {
        let mut count = 0u64;
        loop {
            let n = unsafe { libc::read(self.fd(), (&raw mut count).cast(), 8) };
            if n == 8 || (n < 0 && errno() == libc::EAGAIN) {
                return;
            }
            if n < 0 && errno() == libc::EINTR {
                continue;
            }
            crate::fault("execution work eventfd read");
        }
    }
}

#[derive(Clone, Default)]
pub struct Readiness(Arc<ReadySlot>, u64, Arc<AtomicU64>);

/// Clones publish into the same driver, even when the first wait comes from
/// another thread. OnceLock follows std's lazy publication protocol; the large
/// registry is absent until an interest or driver configuration needs it.
#[derive(Default)]
struct ReadySlot(std::sync::OnceLock<Box<Mutex<ReadyState>>>);
impl ReadySlot {
    fn get(&self) -> Option<&Mutex<ReadyState>> {
        self.0.get().map(Box::as_ref)
    }
    fn lock(&self) -> std::sync::LockResult<MutexGuard<'_, ReadyState>> {
        self.0.get_or_init(|| Box::new(Mutex::default())).lock()
    }
}
/// The token bits a run's tag occupies: above every interpreter and host wait token's
/// counter, below the host's own token ranges (bits 62 and 63).
const TAG_SHIFT: u32 = 48;
const TAG_MASK: u64 = ((1 << 14) - 1) << TAG_SHIFT;
/// The token a wake descriptor's events carry: no request has it (every tag and host range
/// bit set), so it only ends a wait and is dropped at the next scheduling point.
const WAKE_TOKEN: u64 = u64::MAX;
impl std::fmt::Debug for Readiness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Readiness")
    }
}
#[derive(Default)]
struct ReadyState {
    /// Interrupt a driver that is sleeping while a worker changes its interests.
    wake: Option<OwnedFd>,
    /// Tagged execution runs share a driver: one run must never wait by itself.
    shared: bool,
    virtual_time: Option<u64>,
    provider_deadline: Option<u64>,
    isolated_process_clock: bool,
    pending: IdMap<Pending>,
    /// Tokens whose Pending a provider borrowed since the last scheduling point: their
    /// interest and deadline are registered before the next readiness question.
    touched: IdSet,
    /// Requests that can become ready independently of a real timer. Updated
    /// when the provider releases its PendingGuard, never by a population scan.
    non_timer: usize,
    external_waits: usize,
    /// The epoll set, created at the first descriptor interest.
    epoll: Option<OwnedFd>,
    /// Each watched descriptor's tokens and the events each waits for; the descriptor's
    /// registration is the union. A descriptor no token watches keeps its registration and
    /// loses its row; its events are then ignored.
    fds: IdMap<Vec<(u64, libc::c_short)>>,
    /// Exact per-domain deadlines, tied by registration token.
    timers: Deadlines<Instant>,
    virtual_timers: Deadlines<u64>,
    /// Parked tokens with neither interest nor deadline, or whose descriptor epoll cannot
    /// watch (a regular file): ready at every scheduling point, as a poll(2) of them is.
    always: IdSet,
    /// Tokens the idle wait found ready, for the next scheduling point.
    woken: IdSet,
    /// The descriptors registered by [`Readiness::wake_on`]: they end an idle wait but are
    /// no source of progress of their own.
    wakes: IdSet,
    /// Parked tokens waiting on a scripted world, which no host event readies: each is
    /// ready once a network request completes after it blocked, since that request may
    /// have advanced the script (a write a parked read's peer expects).
    scripted: IdSet,
    /// Level readiness for affected script subscribers, cleared on rearm.
    scripted_ready: IdSet,
    /// The number of network requests completed so far: the progress epoch a scripted
    /// wait compares against the one it blocked at.
    progress: u64,
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
    /// It waits on a scripted world ([`Readiness::scripted`]) since this progress epoch.
    scripted: Option<ScriptedWait>,
    /// What the epoll set and the deadline indexes hold for this token.
    registered: Option<(libc::c_int, libc::c_short)>,
    non_timer: bool,
    external_wait: bool,
    timer: Option<Instant>,
    virtual_timer: Option<u64>,
}
/// A stream may advance after another network effect; an unanswered connection
/// has consumed its attempt and stays pending until cancellation.
#[derive(Clone, Copy)]
enum ScriptedWait {
    Progress(u64),
    UnansweredConnect,
}
/// A provider's mutable request row. Dropping it publishes changed interests to the driver.
pub(crate) struct PendingGuard<'a> {
    state: MutexGuard<'a, ReadyState>,
    token: u64,
}
impl std::ops::Deref for PendingGuard<'_> {
    type Target = Pending;
    fn deref(&self) -> &Pending {
        &self.state.pending[&self.token]
    }
}
impl std::ops::DerefMut for PendingGuard<'_> {
    fn deref_mut(&mut self) -> &mut Pending {
        self.state.pending.get_mut(&self.token).unwrap()
    }
}
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        let progress = self.state.progress;
        let p = self.state.pending.get_mut(&self.token).unwrap();
        let non_timer = p.deadline.is_none()
            || p.interest.is_some()
            || p.delivery
            || p.scripted.is_some()
            || p.process.is_some()
            || p.resolver.is_some();
        let script = p.scripted;
        let external = p.interest.is_some() || p.delivery;
        let was_external = std::mem::replace(&mut p.external_wait, external);
        let was = std::mem::replace(&mut p.non_timer, non_timer);
        if non_timer != was {
            if non_timer {
                self.state.non_timer += 1;
            } else {
                self.state.non_timer -= 1;
            }
        }
        if external != was_external {
            if external {
                self.state.external_waits += 1;
            } else {
                self.state.external_waits -= 1;
            }
        }
        if let Some(ScriptedWait::Progress(since)) = script {
            self.state.scripted.insert(self.token);
            if since < progress {
                self.state.scripted_ready.insert(self.token);
            } else {
                self.state.scripted_ready.remove(&self.token);
            }
        } else {
            self.state.scripted.remove(&self.token);
            self.state.scripted_ready.remove(&self.token);
        }
        self.state.notify_driver();
    }
}
impl Readiness {
    /// Interrupt an idle driver without readying a request. Create the eventfd
    /// before publishing the notification, so a notify before the first wait
    /// cannot disappear. The counter coalesces concurrent notifications.
    pub(crate) fn notify(&self) -> Result<(), AdapterError> {
        let mut state = self.0.lock().unwrap();
        state.driver_wake()?;
        state.notify_driver();
        Ok(())
    }

    /// The handle of run `tag` of this execution (the root is 0) on the same readiness.
    pub fn tagged(&self, tag: u64) -> Self {
        self.mark_shared();
        assert!(
            tag << TAG_SHIFT & !TAG_MASK == 0,
            "a run tag within 14 bits"
        );
        Self(
            self.0.clone(),
            tag << TAG_SHIFT,
            Arc::new(AtomicU64::new(tag << TAG_SHIFT)),
        )
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
    pub(crate) fn mark_shared(&self) {
        self.0.lock().unwrap().shared = true;
    }

    /// No other host source or external stop descriptor needs this driver.
    pub(crate) fn exclusive_socket(&self) -> bool {
        if self.0.get().is_none() {
            return true;
        }
        let state = self.0.lock().unwrap();
        !state.shared
            && state.wakes.is_empty()
            && state.virtual_time.is_none()
            && state.pending.is_empty()
    }

    /// A sole native root may wait on its entered socket without parking a fiber.
    /// Shared execution drivers always decline: another run may need to progress.
    /// Rust a7c37f9f socket/unix.rs:180-229 supplies the poll/deadline/EINTR loop.
    pub(crate) fn wait_current(&self) -> Result<bool, AdapterError> {
        if self.0.get().is_none() {
            return Ok(false);
        }
        let state = self.0.lock().unwrap();
        if state.shared
            || !state.wakes.is_empty()
            || state.virtual_time.is_some()
            || state.pending.len() != 1
        {
            return Ok(false);
        }
        let Some(pending) = state.pending.get(&self.2.load(Ordering::Relaxed)) else {
            return Ok(false);
        };
        let Some((fd, events)) = pending.interest else {
            return Ok(false);
        };
        if pending.resolver.is_some() || pending.process.is_some() {
            return Ok(false);
        }
        let deadline = pending.deadline;
        drop(state);
        let mut row = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        loop {
            let timeout = deadline.map_or(-1, |end| {
                let left = end.saturating_duration_since(Instant::now());
                left.as_millis()
                    .saturating_add(u128::from(left.subsec_nanos() % 1_000_000 != 0))
                    .min(i32::MAX as u128) as i32
            });
            // SAFETY: the live provider owns fd; row is writable through this call.
            let n = unsafe { libc::poll(&mut row, 1, timeout) };
            if n >= 0 {
                return Ok(true);
            }
            if errno() != libc::EINTR {
                return Err(AdapterError::Unsupported("host socket wait".into()));
            }
        }
    }
    /// The entered request waits for a delivery ([`deliver`](Self::deliver)).
    pub(crate) fn await_delivery(&self) {
        self.pending().delivery = true;
    }
    /// The entered request waits for a delivery that another thread announces by making `fd`
    /// readable (a remote runner call's provider), so the idle wait wakes for it. The
    /// delivery itself still arrives through [`deliver`](Self::deliver).
    pub fn await_descriptor(&self, fd: std::os::fd::RawFd) {
        let mut pending = self.pending();
        pending.delivery = true;
        pending.interest = Some((fd, libc::POLLIN));
    }
    /// Wake the idle wait whenever `fd` becomes readable, without readying any request: a
    /// descriptor another thread writes to with news for the run (a runner host cancelling
    /// a callee run), which the run reads at its next scheduling point. A run whose only
    /// remaining sources are such descriptors is still Blocked (CONC-8).
    pub fn wake_on(&self, fd: std::os::fd::RawFd) -> Result<(), AdapterError> {
        let mut state = self.0.lock().unwrap();
        state.wakes.insert(fd as u64);
        let tokens = state.fds.entry(fd as u64).or_default();
        if !tokens.iter().any(|&(token, _)| token == WAKE_TOKEN) {
            tokens.push((WAKE_TOKEN, libc::POLLIN));
        }
        if state.control(fd, libc::POLLIN)? {
            Ok(())
        } else {
            Err(AdapterError::Unsupported(
                "an unwatchable wake descriptor".into(),
            ))
        }
    }
    /// The delivery `token` waits for arrived: it is ready at the next scheduling point.
    pub fn deliver(&self, token: u64) {
        if self.0.get().is_none() {
            return;
        }
        let token = self.tag(token);
        let mut state = self.0.lock().unwrap();
        if state.pending.get(&token).is_some_and(|p| p.delivery) {
            state.woken.insert(token);
        }
        state.notify_driver();
    }
    /// The request blocked under the entered token waits on a scripted world.
    pub(crate) fn scripted(&self) {
        let progress = self.0.lock().unwrap().progress;
        self.pending().scripted = Some(ScriptedWait::Progress(progress));
    }
    /// A provider inspected this scripted candidate and it is still blocked.
    /// Acknowledge the current progress without publishing a false wake; the
    /// next idle wait can then consider the provider's next virtual deadline.
    pub(crate) fn acknowledge_scripted(&self, token: u64) {
        let token = self.tag(token);
        let mut state = self.0.lock().unwrap();
        let progress = state.progress;
        if let Some(pending) = state.pending.get_mut(&token)
            && matches!(pending.scripted, Some(ScriptedWait::Progress(_)))
        {
            pending.scripted = Some(ScriptedWait::Progress(progress));
            state.scripted_ready.remove(&token);
        }
    }
    /// An unbounded Unanswered connect has no readiness source (CONC-8).
    pub(crate) fn scripted_connect(&self) {
        self.pending().scripted = Some(ScriptedWait::UnansweredConnect);
    }
    pub(crate) fn pending_scripted_connect(&self) -> bool {
        if self.0.get().is_none() {
            return false;
        }
        let token = self.2.load(Ordering::Relaxed);
        self.0
            .lock()
            .unwrap()
            .pending
            .get(&token)
            .is_some_and(|pending| {
                matches!(pending.scripted, Some(ScriptedWait::UnansweredConnect))
            })
    }
    /// A network request completed: parked scripted waits are retried at the next
    /// scheduling point.
    pub(crate) fn progressed(&self) {
        if self.0.get().is_none() {
            return;
        }
        let mut state = self.0.lock().unwrap();
        state.progress += 1;
        let state = &mut *state;
        // This world progressed, so every current progress subscriber is
        // affected. No scheduling point walks idle script registrations.
        for token in state.scripted.members_unordered() {
            state.scripted_ready.insert(token);
        }
        state.notify_driver();
    }
    /// Harness-only virtual time: samples are stable while runnable work remains.
    pub(crate) fn virtual_clock(ticks: u64) -> Self {
        let readiness = Self::default();
        readiness.0.lock().unwrap().virtual_time = Some(ticks);
        readiness
    }
    pub(crate) fn isolate_process_clock(&self) {
        self.0.lock().unwrap().isolated_process_clock = true;
    }
    pub(crate) fn provider_deadline(&self, deadline: Option<u64>) {
        if deadline.is_none() && self.0.get().is_none() {
            return;
        }
        self.0.lock().unwrap().provider_deadline = deadline;
    }
    pub(crate) fn virtual_time(&self) -> Option<u64> {
        if self.0.get().is_none() {
            return None;
        }
        self.0.lock().unwrap().virtual_time
    }
    pub(crate) fn pending(&self) -> PendingGuard<'_> {
        let mut state = self.0.lock().unwrap();
        let token = self.2.load(Ordering::Relaxed);
        state.touched.insert(token);
        state.pending.entry(token).or_default();
        PendingGuard { state, token }
    }

    /// Progress already committed by the current operation, without constructing
    /// a Pending row for the usual single successful send.
    pub(crate) fn offset(&self) -> usize {
        if self.0.get().is_none() {
            return 0;
        }
        let state = self.0.lock().unwrap();
        state
            .pending
            .get(&self.2.load(Ordering::Relaxed))
            .map_or(0, |p| p.offset)
    }

    /// Select the opaque operation token before asking RealIo to execute a request.
    pub fn enter(&self, token: u64) {
        self.2.store(self.tag(token), Ordering::Relaxed);
    }
    pub fn finish(&self, token: u64) {
        if self.0.get().is_none() {
            return;
        }
        let token = self.tag(token);
        let mut state = self.0.lock().unwrap();
        // Every side index names a Pending row. A ready syscall creates none.
        let Some(pending) = state.pending.remove(&token) else {
            return;
        };
        state.non_timer -= usize::from(pending.non_timer);
        state.external_waits -= usize::from(pending.external_wait);
        state.timers.remove(token);
        state.virtual_timers.remove(token);
        state.touched.remove(&token);
        state.always.remove(&token);
        state.scripted.remove(&token);
        state.scripted_ready.remove(&token);
        state.woken.remove(&token);
        if let Some((fd, _)) = pending.registered {
            state.unregister(token, fd);
        }
        state.notify_driver();
    }
    /// The task of the request blocked under `token` parked: from the next scheduling point
    /// on, [`ready`](Self::ready) reports the token when its interest is ready.
    pub fn parked(&self, token: u64) {
        if self.0.get().is_none() {
            return;
        }
        let token = self.tag(token);
        let mut state = self.0.lock().unwrap();
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
        state.notify_driver();
    }
    /// After a packet is sent or a write is accepted, the operation must complete
    /// with its own outcome rather than report an effect-free cancellation.
    pub fn cancellable(&self, token: u64) -> bool {
        if self.0.get().is_none() {
            return true;
        }
        self.0
            .lock()
            .unwrap()
            .pending
            .get(&self.tag(token))
            .is_none_or(|p| !p.committed)
    }
    /// A scheduling point: the parked tokens that are ready now (a watched descriptor is
    /// ready, a deadline passed, or the token waits on nothing epoll can watch), in token
    /// order. Never blocks; each ready token's operation is re-issued by the caller.
    pub fn ready(&self) -> Result<Vec<u64>, AdapterError> {
        if self.0.get().is_none() {
            return Ok(Vec::new());
        }
        let mut state = self.0.lock().unwrap();
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
    /// Home's conservative readiness bound. Like Go's timers.check precheck,
    /// readers need not enter the driver before its earliest possible timer.
    /// Decline every request that could become ready independently of time.
    /// wake_on descriptors are readiness-neutral: poll removes WAKE_TOKEN;
    /// stop words are checked separately at the existing runtime entries.
    pub(crate) fn ready_deadline(&self) -> Option<Instant> {
        if self.0.get().is_none() {
            return None;
        }
        let mut state = self.0.lock().unwrap();
        if state.shared
            || state.virtual_time.is_some()
            || !state.woken.is_empty()
            || state.non_timer != 0
        {
            return None;
        }
        state.sync().ok()?;
        // A consumed due entry with a still-live request also declines: a
        // later deadline must never hide a request already ready at home.
        if state.timers.len() != state.pending.len() {
            return None;
        }
        state.timers.next()
    }

    /// Whether [`ready`](Self::ready) would report a token of this run now, without taking
    /// any: what epoll and the timers found waits for that scheduling point.
    pub fn has_ready(&self) -> Result<bool, AdapterError> {
        if self.0.get().is_none() {
            return Ok(false);
        }
        let mut state = self.0.lock().unwrap();
        let woken = Self::poll(&mut state)?;
        state.woken = woken;
        let state = &*state;
        Ok(state
            .woken
            .members_unordered()
            .chain(state.always.members_unordered())
            .any(|t| self.own(t) && state.pending.get(&t).is_some_and(|p| p.parked)))
    }
    /// Every token found ready so far: the idle wait's findings, the scripted waits a
    /// network request completed after, the descriptors epoll reports ready now, and the
    /// passed deadlines.
    fn poll(state: &mut ReadyState) -> Result<IdSet, AdapterError> {
        state.sync()?;
        let mut ready = std::mem::take(&mut state.woken);
        for token in state.progressed_scripts() {
            ready.insert(token);
        }
        if !state.fds.is_empty() {
            state.epoll_events(0, &mut ready)?;
        }
        state.due(&mut ready);
        ready.remove(&WAKE_TOKEN);
        Ok(ready)
    }
    /// A timer wait for CLOCK_MONOTONIC `deadline` nanoseconds under the entered token:
    /// true once it has passed (the token's interest is then finished), otherwise the
    /// deadline is recorded as the token's interest and the caller parks.
    pub(crate) fn timer(&self, deadline: u64) -> Result<bool, AdapterError> {
        if let Some(now) = self.virtual_time() {
            let token = self.2.load(Ordering::Relaxed);
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
        let token = self.2.load(Ordering::Relaxed);
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
        match self.wait_source(false)? {
            WaitSource::Ready => Ok(true),
            WaitSource::Empty => Ok(false),
            WaitSource::Driver(epoll, deadline, _) => self.block(epoll, deadline),
        }
    }

    /// Wait as [`wait`](Self::wait), and also when only wake descriptors remain:
    /// for a waiter whose outstanding event source (an owned host job) notifies
    /// this driver, so its publication and a stop end the same blocking wait.
    pub(crate) fn wait_woken(&self) -> Result<bool, AdapterError> {
        match self.wait_source(true)? {
            WaitSource::Ready => Ok(true),
            WaitSource::Empty => Ok(false),
            WaitSource::Driver(epoll, deadline, _) => self.block(epoll, deadline),
        }
    }

    fn block(&self, epoll: OwnedFd, deadline: Option<Instant>) -> Result<bool, AdapterError> {
        let mut events = [libc::epoll_event { events: 0, u64: 0 }; 256];
        let n = loop {
            let timeout = deadline.map_or(-1, |deadline| {
                let left = deadline.saturating_duration_since(Instant::now());
                left.as_millis()
                    .saturating_add(u128::from(left.subsec_nanos() % 1_000_000 != 0))
                    .min(libc::c_int::MAX as u128) as libc::c_int
            });
            // SAFETY: epoll is owned and events has room for every returned event.
            let n = unsafe {
                libc::epoll_wait(
                    epoll.as_raw_fd(),
                    events.as_mut_ptr(),
                    events.len() as i32,
                    timeout,
                )
            };
            if n >= 0 {
                break n as usize;
            }
            if errno() != libc::EINTR {
                return Err(AdapterError::Unsupported("host readiness epoll".into()));
            }
        };
        let mut state = self.0.lock().unwrap();
        let mut woken = std::mem::take(&mut state.woken);
        state.collect_events(&events[..n], &mut woken);
        state.woken = woken;
        Ok(true)
    }

    pub(crate) fn same_driver(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// One idle wait across independent driver sets. Like Rust's Unix socket
    /// poll loop, snapshot owned descriptors, release all state locks, poll
    /// against the earliest absolute deadline, and retry EINTR without extending
    /// that deadline. epoll descriptors are themselves pollable (epoll(7)).
    pub(crate) fn wait_many(drivers: &[&Self]) -> Result<bool, AdapterError> {
        let mut unique: Vec<&Self> = Vec::new();
        let mut sources = Vec::new();
        let mut external = false;
        for &driver in drivers {
            if unique.iter().any(|other| Arc::ptr_eq(&driver.0, &other.0)) {
                continue;
            }
            unique.push(driver);
            match driver.wait_source(true)? {
                WaitSource::Ready => return Ok(true),
                WaitSource::Empty => {}
                WaitSource::Driver(epoll, deadline, source) => {
                    external |= source;
                    sources.push((driver, epoll, deadline));
                }
            }
        }
        if !external {
            return Ok(false);
        }
        let deadline = sources
            .iter()
            .filter_map(|(_, _, deadline)| *deadline)
            .min();
        let mut fds = sources
            .iter()
            .map(|(_, epoll, _)| libc::pollfd {
                fd: epoll.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect::<Vec<_>>();
        loop {
            let timeout = deadline.map_or(-1, |deadline| {
                let left = deadline.saturating_duration_since(Instant::now());
                left.as_millis()
                    .saturating_add(u128::from(left.subsec_nanos() % 1_000_000 != 0))
                    .min(libc::c_int::MAX as u128) as libc::c_int
            });
            // SAFETY: fds owns this many initialized writable poll rows.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
            if n >= 0 {
                break;
            }
            if errno() != libc::EINTR {
                return Err(AdapterError::Unsupported("host readiness poll".into()));
            }
        }
        for ((driver, _, _), fd) in sources.iter().zip(&fds) {
            if fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(AdapterError::Unsupported(
                    "host readiness descriptor".into(),
                ));
            }
            let mut state = driver.0.lock().unwrap();
            let mut woken = std::mem::take(&mut state.woken);
            state.epoll_events(0, &mut woken)?;
            state.woken = woken;
        }
        Ok(true)
    }

    pub(crate) fn wait_source(&self, include_wake_only: bool) -> Result<WaitSource, AdapterError> {
        if self.0.get().is_none() {
            return Ok(WaitSource::Empty);
        }
        let mut state = self.0.lock().unwrap();
        state.sync()?;
        if state.isolated_process_clock && state.external_waits != 0 {
            return Err(AdapterError::Unsupported(
                "external wait in timed process clock domain".into(),
            ));
        }
        if !state.scripted_ready.is_empty() {
            return Ok(WaitSource::Ready);
        }
        if let Some(now) = state.virtual_time {
            let next = state
                .next_virtual()
                .into_iter()
                .chain(state.provider_deadline)
                .min();
            if let Some(next) = next {
                state.virtual_time = Some(now.max(next));
                return Ok(WaitSource::Ready);
            }
            if !include_wake_only {
                return Ok(WaitSource::Empty);
            }
        }
        if !state.always.is_empty() || !state.woken.is_empty() {
            return Ok(WaitSource::Ready);
        }
        let deadline = if state.virtual_time.is_none() {
            state.next_timer()
        } else {
            None
        };
        let sources = state.virtual_time.is_none()
            && state
                .fds
                .iter_unordered()
                .any(|(fd, _)| !state.wakes.contains(&fd));
        if !sources && deadline.is_none() && !include_wake_only {
            return Ok(WaitSource::Empty);
        }
        state.driver_wake()?;
        let epoll = state
            .epoll
            .as_ref()
            .unwrap()
            .try_clone()
            .map_err(|_| AdapterError::Unsupported("host readiness epoll clone".into()))?;
        Ok(WaitSource::Driver(
            epoll,
            deadline,
            sources || deadline.is_some(),
        ))
    }
}

pub(crate) enum WaitSource {
    Ready,
    Empty,
    Driver(OwnedFd, Option<Instant>, bool),
}

impl ReadyState {
    /// The scripted waits a network request completed after: each may be ready now, and
    /// a re-issue that blocks again waits for the next one.
    fn progressed_scripts(&self) -> Vec<u64> {
        self.scripted_ready.members_unordered().collect()
    }
    /// Register the interest and deadline of every token a provider touched.
    fn sync(&mut self) -> Result<(), AdapterError> {
        let mut touched = std::mem::take(&mut self.touched);
        for token in touched.drain_unordered() {
            let Some(pending) = self.pending.get_mut(&token) else {
                continue;
            };
            let (interest, registered, delivery, scripted) = (
                pending.interest,
                pending.registered,
                pending.delivery,
                pending.scripted.is_some(),
            );
            if pending.timer != pending.deadline {
                self.timers.remove(token);
                if let Some(deadline) = pending.deadline {
                    self.timers.insert(token, deadline);
                }
                pending.timer = pending.deadline;
            }
            if pending.virtual_timer != pending.virtual_deadline {
                self.virtual_timers.remove(token);
                if let Some(deadline) = pending.virtual_deadline {
                    self.virtual_timers.insert(token, deadline);
                }
                pending.virtual_timer = pending.virtual_deadline;
            }
            let watched = interest.is_some() || pending.deadline.is_some();
            let timed = pending.virtual_deadline.is_some();
            if let Some((fd, _)) = registered
                && interest.is_none_or(|(new, _)| new != fd)
            {
                self.unregister(token, fd);
            }
            let mut always = !watched && !timed && !delivery && !scripted;
            if let Some((fd, events)) = interest {
                // Re-armed on every park, even unchanged: a descriptor closed and reopened
                // under the same number left the epoll set when it closed, and an edge that
                // came while no token watched it was dropped; MOD re-reports current readiness.
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
            // The registration stays for the descriptor's next park (mio keeps a source
            // registered until it is dropped). Edge-triggered, an idle descriptor never
            // wakes the set, and closing it removes it from the set.
            self.fds.remove(&(fd as u64));
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
    /// Watch `fd` for `events` (poll(2) bits, which epoll shares), edge-triggered: modify its
    /// registration, adding it when absent. False when epoll cannot watch the descriptor.
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
            events: events as u16 as u32 | libc::EPOLLET as u32,
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
            self.collect_events(&events[..n as usize], ready);
            if (n as usize) < BATCH {
                return Ok(());
            }
            // A full batch: take the rest without waiting.
            timeout = 0;
        }
    }
    fn collect_events(&self, events: &[libc::epoll_event], ready: &mut IdSet) {
        for event in events {
            let fd = event.u64 as libc::c_int;
            if self
                .wake
                .as_ref()
                .is_some_and(|wake| wake.as_raw_fd() == fd)
            {
                let mut value = 0u64;
                // SAFETY: a live eventfd and an eight-byte writable buffer.
                unsafe {
                    libc::read(fd, (&mut value as *mut u64).cast(), 8);
                }
                continue;
            }
            let revents = event.events;
            for (token, wanted) in self.fds.get(&(fd as u64)).into_iter().flatten() {
                let wanted =
                    (*wanted as u16 as u32) | libc::EPOLLERR as u32 | libc::EPOLLHUP as u32;
                if revents & wanted != 0 {
                    ready.insert(*token);
                }
            }
        }
    }
    fn notify_driver(&self) {
        if let Some(wake) = &self.wake {
            let value = 1u64;
            // SAFETY: a live nonblocking eventfd and an eight-byte input buffer.
            unsafe {
                libc::write(wake.as_raw_fd(), (&value as *const u64).cast(), 8);
            }
        }
    }
    fn driver_wake(&mut self) -> Result<(), AdapterError> {
        if self.wake.is_some() {
            return Ok(());
        }
        // SAFETY: no pointers; the fresh descriptor is owned immediately.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(AdapterError::Unsupported("host readiness wake".into()));
        }
        let wake = unsafe { OwnedFd::from_raw_fd(fd) };
        self.control(fd, libc::POLLIN)?;
        self.wake = Some(wake);
        Ok(())
    }
    /// Visit only the inclusive due prefix in the active clock domain.
    fn due(&mut self, ready: &mut IdSet) {
        if let Some(now) = self.virtual_time {
            while let Some((_, token)) = self.virtual_timers.pop_due(now) {
                self.pending.get_mut(&token).unwrap().virtual_timer = None;
                ready.insert(token);
            }
        } else if !self.timers.is_empty() {
            let now = Instant::now();
            while let Some((_, token)) = self.timers.pop_due(now) {
                self.pending.get_mut(&token).unwrap().timer = None;
                ready.insert(token);
            }
        }
    }
    fn next_timer(&mut self) -> Option<Instant> {
        self.timers.next()
    }
    fn next_virtual(&mut self) -> Option<u64> {
        self.virtual_timers.next()
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
    /// The tokens of the waits blocked now.
    pub(crate) fn tokens(&self) -> impl Iterator<Item = u64> + '_ {
        self.rows.iter_unordered().map(|(token, _)| token)
    }
    /// A new scheduling point.
    pub(crate) fn point(&mut self) {
        self.advance(1);
    }
    /// Exact empty points counted by an Ordered timer precheck. Apply before
    /// any wait-log operation, so park/ready/finish retain their old positions.
    pub(crate) fn advance(&mut self, points: u64) {
        self.point += points;
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

/// Readiness vectors ported from mio a88e5e11 `tests/` (file:line per test) and the cases
/// this runtime's deviations add: descriptor-keyed fan-out and the re-arm at every park.
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Duration;

    fn pair() -> (UnixStream, UnixStream) {
        let (watched, peer) = UnixStream::pair().unwrap();
        watched.set_nonblocking(true).unwrap();
        (watched, peer)
    }
    /// The request under `token` would block on `fd` and its task parks.
    fn park(readiness: &Readiness, token: u64, fd: &UnixStream, events: libc::c_short) {
        readiness.enter(token);
        readiness.pending().interest = Some((fd.as_raw_fd(), events));
        readiness.parked(token);
    }
    fn would_block(mut stream: &UnixStream) -> bool {
        stream.read(&mut [0; 64]).unwrap_err().kind() == std::io::ErrorKind::WouldBlock
    }

    /// A wake descriptor ends an idle wait but is no source of its own: with nothing else
    /// to wait on the run is Blocked; a delivery parked on another thread's descriptor is
    /// ready once that thread writes to it.
    #[test]
    fn a_wake_descriptor_wakes_a_wait_without_being_a_source() {
        let readiness = Readiness::default();
        let (cancel, mut canceller) = pair();
        readiness.wake_on(cancel.as_raw_fd()).unwrap();
        assert!(!readiness.wait().unwrap());
        canceller.write_all(&[1]).unwrap();
        assert!(readiness.ready().unwrap().is_empty());
        let (delivered, mut provider) = pair();
        readiness.enter(1);
        readiness.await_descriptor(delivered.as_raw_fd());
        readiness.parked(1);
        assert!(readiness.ready().unwrap().is_empty());
        provider.write_all(&[1]).unwrap();
        assert!(readiness.wait().unwrap());
        assert_eq!(readiness.ready().unwrap(), [1]);
    }

    #[test]
    fn sole_socket_wait_needs_no_epoll_or_parked_row_and_shared_drivers_decline() {
        let (watched, mut peer) = pair();
        let readiness = Readiness::default();
        readiness.enter(7);
        readiness.pending().interest = Some((watched.as_raw_fd(), libc::POLLIN));
        peer.write_all(b"ready").unwrap();
        assert_eq!(readiness.wait_current(), Ok(true));
        assert!(readiness.0.lock().unwrap().epoll.is_none());
        let (wake, _sender) = pair();
        readiness.wake_on(wake.as_raw_fd()).unwrap();
        assert_eq!(readiness.wait_current(), Ok(false));
        readiness
            .0
            .lock()
            .unwrap()
            .wakes
            .remove(&(wake.as_raw_fd() as u64));
        readiness.tagged(0);
        assert_eq!(readiness.wait_current(), Ok(false));
        readiness.finish(7);
    }

    #[test]
    fn sole_socket_wait_honors_its_timeout_and_declines_with_another_pending_wait() {
        let (watched, _peer) = pair();
        let readiness = Readiness::default();
        readiness.enter(7);
        readiness.pending().interest = Some((watched.as_raw_fd(), libc::POLLIN));
        readiness.pending().deadline = Some(Instant::now());
        assert_eq!(readiness.wait_current(), Ok(true));
        readiness.enter(8);
        readiness.pending().interest = Some((watched.as_raw_fd(), libc::POLLIN));
        assert_eq!(readiness.wait_current(), Ok(false));
        readiness.finish(8);
        readiness.finish(7);
    }

    #[test]
    fn timer_bound_declines_mixed_sources_and_tracks_the_earliest_remaining_timer() {
        let readiness = Readiness::default();
        assert_eq!(readiness.ready_deadline(), None);
        let later = Instant::now() + Duration::from_secs(60);
        let earlier = later - Duration::from_secs(10);
        for (token, deadline) in [(1, later), (2, earlier)] {
            readiness.enter(token);
            readiness.pending().deadline = Some(deadline);
            readiness.parked(token);
        }
        assert_eq!(readiness.ready_deadline(), Some(earlier));
        readiness.finish(2);
        assert_eq!(readiness.ready_deadline(), Some(later));
        // Each independent source must invalidate the timer-only conclusion.
        for source in 0..5 {
            readiness.enter(3);
            match source {
                0 => readiness.pending().interest = Some((0, libc::POLLIN)),
                1 => readiness.await_delivery(),
                2 => readiness.scripted(),
                3 => {
                    let _ = readiness.pending();
                }
                4 => {
                    readiness.0.lock().unwrap().woken.insert(1);
                }
                _ => unreachable!(),
            }
            assert_eq!(readiness.ready_deadline(), None, "source {source}");
            readiness.finish(3);
            readiness.0.lock().unwrap().woken.remove(&1);
            assert_eq!(readiness.ready_deadline(), Some(later));
        }
        readiness.tagged(0);
        assert_eq!(readiness.ready_deadline(), None, "shared drivers decline");
        let virtual_clock = Readiness::virtual_clock(0);
        virtual_clock.enter(7);
        assert_eq!(virtual_clock.timer(100), Ok(false));
        assert_eq!(virtual_clock.ready_deadline(), None);
    }

    #[test]
    fn deferred_empty_poll_points_preserve_wait_ordinals_across_park_and_finish() {
        fn log() -> WaitLog {
            let mut log = WaitLog::default();
            log.enter(17);
            log.park(false);
            log.parked(17);
            log
        }
        let (mut eager, mut deferred) = (log(), log());
        for _ in 0..1024 {
            eager.point();
        }
        deferred.advance(1024);
        for log in [&mut eager, &mut deferred] {
            log.enter(23);
            log.park(false);
            log.parked(23);
            log.point();
        }
        assert_eq!(eager.ready(17), Some((0, 1024)));
        assert_eq!(deferred.ready(17), Some((0, 1024)));
        assert_eq!(eager.ready(23), Some((1, 0)));
        assert_eq!(deferred.ready(23), Some((1, 0)));
        eager.finish(17);
        deferred.finish(17);
        for _ in 0..17 {
            eager.point();
        }
        deferred.advance(17);
        assert_eq!(eager.ready(23), deferred.ready(23));
        assert_eq!(eager.token_of(0), deferred.token_of(0));
        assert_eq!(eager.token_of(1), deferred.token_of(1));
    }

    #[test]
    fn a_readable_wake_descriptor_readies_no_token_and_keeps_the_timer_bound() {
        let readiness = Readiness::default();
        let end = Instant::now() + Duration::from_secs(60);
        readiness.enter(7);
        readiness.pending().deadline = Some(end);
        readiness.parked(7);
        let (wake, mut sender) = pair();
        readiness.wake_on(wake.as_raw_fd()).unwrap();
        assert_eq!(readiness.ready_deadline(), Some(end));
        sender.write_all(b"stop").unwrap();
        assert_eq!(readiness.ready_deadline(), Some(end));
        assert_eq!(readiness.ready().unwrap(), Vec::<u64>::new());
        assert_eq!(readiness.ready_deadline(), Some(end));
        assert!(!readiness.has_ready().unwrap());
    }

    /// mio tests/poll.rs:120 readiness_is_reregistered_after_would_block.
    #[test]
    fn readiness_is_reported_again_after_would_block() {
        let readiness = Readiness::default();
        let (mut watched, mut peer) = pair();
        park(&readiness, 1, &watched, libc::POLLIN);
        assert!(readiness.ready().unwrap().is_empty());
        peer.write_all(b"hello").unwrap();
        assert_eq!(readiness.ready().unwrap(), [1]);
        assert_eq!(watched.read(&mut [0; 16]).unwrap(), 5);
        assert!(would_block(&watched));
        // The re-issued request blocked again under its token.
        park(&readiness, 1, &watched, libc::POLLIN);
        assert!(readiness.ready().unwrap().is_empty());
        peer.write_all(b"again").unwrap();
        assert_eq!(readiness.ready().unwrap(), [1]);
    }

    /// mio tests/poll.rs:66 zero_duration_polls_events: a scheduling point never sleeps
    /// but still asks the kernel.
    #[test]
    fn a_scheduling_point_reports_ready_descriptors_without_waiting() {
        let readiness = Readiness::default();
        let streams: Vec<_> = (0..3).map(|_| pair()).collect();
        for (token, (watched, _)) in (1..).zip(&streams) {
            park(&readiness, token, watched, libc::POLLOUT);
        }
        assert_eq!(readiness.ready().unwrap(), [1, 2, 3]);
    }

    /// mio tests/registering.rs:225 registering_after_deregistering: a finished wait keeps
    /// the descriptor registered, and the next wait on it is answered, not stale.
    #[test]
    fn a_descriptor_waits_again_after_its_last_wait_finished() {
        let readiness = Readiness::default();
        let (mut watched, mut peer) = pair();
        park(&readiness, 1, &watched, libc::POLLIN);
        assert!(readiness.ready().unwrap().is_empty());
        readiness.finish(1);
        park(&readiness, 2, &watched, libc::POLLIN);
        assert!(readiness.ready().unwrap().is_empty());
        peer.write_all(b"x").unwrap();
        assert_eq!(readiness.ready().unwrap(), [2]);
        watched.read_exact(&mut [0]).unwrap();
    }

    /// Edge-triggered, an edge that came while no token watched the descriptor was
    /// consumed by a scheduling point; the park re-arms the registration, so data already
    /// waiting is reported (tokio runtime/io/scheduled_io.rs:372 keeps readiness until a
    /// WouldBlock clears it for the same reason).
    #[test]
    fn data_that_arrived_while_unwatched_is_reported_at_the_next_park() {
        let readiness = Readiness::default();
        let (watched, mut peer) = pair();
        let (other, _other_peer) = pair();
        park(&readiness, 1, &watched, libc::POLLIN);
        assert!(readiness.ready().unwrap().is_empty());
        readiness.finish(1);
        // Another descriptor keeps the set in use, so this scheduling point harvests the
        // unwatched descriptor's edge and drops it.
        park(&readiness, 2, &other, libc::POLLIN);
        peer.write_all(b"x").unwrap();
        assert!(readiness.ready().unwrap().is_empty());
        park(&readiness, 3, &watched, libc::POLLIN);
        assert_eq!(readiness.ready().unwrap(), [3]);
    }

    /// mio tests/poll.rs:47 add_then_drop, and a descriptor number reused after close (a
    /// connect retrying the next address under one token): the new file is registered.
    #[test]
    fn a_closed_and_reused_descriptor_number_is_watched_anew() {
        let readiness = Readiness::default();
        let (watched, _first_peer) = pair();
        let number = watched.as_raw_fd();
        park(&readiness, 1, &watched, libc::POLLIN);
        assert!(readiness.ready().unwrap().is_empty());
        // Keep ownership of the descriptor number throughout replacement.
        // Dropping it and expecting socketpair's lowest-free allocation races
        // unrelated tests. dup3 atomically closes the old file and installs
        // the new one at that exact number, so epoll must register it anew.
        let (second, mut second_peer) = pair();
        assert_eq!(
            unsafe { libc::dup3(second.as_raw_fd(), number, libc::O_CLOEXEC) },
            number
        );
        drop(second);
        park(&readiness, 1, &watched, libc::POLLIN);
        assert!(readiness.ready().unwrap().is_empty());
        second_peer.write_all(b"x").unwrap();
        assert_eq!(readiness.ready().unwrap(), [1]);
        readiness.finish(1);
        let (third, mut third_peer) = pair();
        assert_eq!(
            unsafe { libc::dup3(third.as_raw_fd(), number, libc::O_CLOEXEC) },
            number
        );
        drop(third);
        park(&readiness, 2, &watched, libc::POLLIN);
        third_peer.write_all(b"y").unwrap();
        assert_eq!(readiness.ready().unwrap(), [2]);
    }

    /// mio tests/unix_stream.rs:316 unix_stream_shutdown_write, from the reading side: a
    /// peer's close ends a read wait (EPOLLHUP and EPOLLRDHUP are reported with EPOLLIN).
    #[test]
    fn a_peer_close_ends_a_read_wait() {
        let readiness = Readiness::default();
        let (watched, peer) = pair();
        park(&readiness, 1, &watched, libc::POLLIN);
        assert!(readiness.ready().unwrap().is_empty());
        drop(peer);
        assert!(readiness.wait().unwrap());
        assert_eq!(readiness.ready().unwrap(), [1]);
        assert_eq!((&watched).read(&mut [0; 4]).unwrap(), 0);
    }

    /// Keyed by descriptor: a read and a write of one socket park under two tokens, and
    /// each is reported only for its own direction.
    #[test]
    fn a_read_and_a_write_of_one_descriptor_are_reported_apart() {
        let readiness = Readiness::default();
        let (mut watched, mut peer) = pair();
        peer.set_nonblocking(true).unwrap();
        let chunk = [0u8; 4096];
        while watched.write(&chunk).is_ok() {}
        park(&readiness, 1, &watched, libc::POLLOUT);
        park(&readiness, 2, &watched, libc::POLLIN);
        assert!(readiness.ready().unwrap().is_empty());
        peer.write_all(b"x").unwrap();
        assert_eq!(readiness.ready().unwrap(), [2]);
        watched.read_exact(&mut [0]).unwrap();
        readiness.finish(2);
        let mut sink = vec![0u8; 1 << 16];
        while peer.read(&mut sink).is_ok() {}
        assert!(readiness.wait().unwrap());
        assert_eq!(readiness.ready().unwrap(), [1]);
    }

    /// A registration kept for an idle descriptor never wakes the idle wait again: with
    /// data unread and no token watching it, the wait sleeps to the next deadline.
    #[test]
    fn an_unwatched_ready_descriptor_does_not_wake_the_idle_wait() {
        let readiness = Readiness::default();
        let (watched, mut peer) = pair();
        let (other, _other_peer) = pair();
        park(&readiness, 1, &watched, libc::POLLIN);
        assert!(readiness.ready().unwrap().is_empty());
        readiness.finish(1);
        park(&readiness, 2, &other, libc::POLLIN);
        peer.write_all(b"x").unwrap();
        // The first scheduling point takes the unwatched edge.
        assert!(readiness.ready().unwrap().is_empty());
        readiness.enter(3);
        readiness.pending().deadline = Some(Instant::now() + Duration::from_millis(20));
        readiness.parked(3);
        let start = Instant::now();
        assert!(readiness.wait().unwrap());
        assert!(start.elapsed() >= Duration::from_millis(15));
        assert_eq!(readiness.ready().unwrap(), [3]);
        drop(watched);
    }

    /// A scripted wait is ready once a network request completes after it blocked, even
    /// when a scheduling point ran between that progress and its task's park, and a
    /// re-issue that blocks again waits for the next progress.
    #[test]
    fn scripted_waits_keep_progress_made_before_their_park() {
        let readiness = Readiness::default();
        readiness.enter(1);
        readiness.scripted();
        readiness.progressed();
        assert_eq!(readiness.ready().unwrap(), Vec::<u64>::new());
        readiness.parked(1);
        assert_eq!(readiness.ready().unwrap(), [1]);
        readiness.enter(1);
        readiness.scripted();
        assert_eq!(readiness.ready().unwrap(), Vec::<u64>::new());
        assert!(!readiness.wait().unwrap());
        readiness.progressed();
        assert!(readiness.wait().unwrap());
        assert_eq!(readiness.ready().unwrap(), [1]);
    }
}

#[cfg(test)]
mod concurrent_tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn shared_idle_wait_observes_delivery_into_a_driver_without_an_external_source() {
        let delivery = Readiness::default();
        delivery.enter(1);
        delivery.await_delivery();
        delivery.parked(1);
        let timer = Readiness::default();
        timer.enter(2);
        timer.pending().deadline = Some(Instant::now() + Duration::from_secs(5));
        timer.parked(2);
        let worker = delivery.clone();
        let joined = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            worker.deliver(1);
        });
        let start = Instant::now();
        assert!(Readiness::wait_many(&[&delivery, &timer]).unwrap());
        joined.join().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "delivery driver was excluded from the wait"
        );
        assert_eq!(delivery.ready().unwrap(), [1]);
        assert!(timer.ready().unwrap().is_empty());
    }
    #[test]
    fn repeated_wake_registration_keeps_one_interest_and_rearms_readiness() {
        use std::io::Write;
        let driver = Readiness::default();
        let (read, mut write) = UnixStream::pair().unwrap();
        for _ in 0..8 {
            driver.wake_on(read.as_raw_fd()).unwrap();
        }
        assert_eq!(
            driver.0.lock().unwrap().fds[&(read.as_raw_fd() as u64)].len(),
            1
        );
        assert!(
            !driver.wait().unwrap(),
            "wake-only descriptors are not progress sources"
        );
        let (request_read, mut request_write) = UnixStream::pair().unwrap();
        driver.enter(7);
        driver.await_descriptor(request_read.as_raw_fd());
        driver.parked(7);
        write.write_all(b"wake").unwrap();
        request_write.write_all(b"x").unwrap();
        assert!(driver.wait().unwrap());
        assert_eq!(driver.ready().unwrap(), [7]);
    }

    #[test]
    fn shared_idle_wait_uses_the_earliest_deadline_from_either_driver() {
        for reverse in [false, true] {
            let slow = Readiness::default();
            let fast = Readiness::default();
            for (driver, token, delay) in [(&slow, 1, 5000), (&fast, 2, 10)] {
                driver.enter(token);
                driver.pending().deadline = Some(Instant::now() + Duration::from_millis(delay));
                driver.parked(token);
            }
            let start = Instant::now();
            let drivers = if reverse {
                [&fast, &slow]
            } else {
                [&slow, &fast]
            };
            assert!(Readiness::wait_many(&drivers).unwrap());
            assert!(start.elapsed() < Duration::from_secs(1));
            assert_eq!(fast.ready().unwrap(), [2]);
            assert!(slow.ready().unwrap().is_empty());
        }
    }

    #[test]
    fn shared_idle_wait_latches_either_descriptor_and_ignores_wake_only_sources() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        for second in [false, true] {
            let a = Readiness::default();
            let b = Readiness::default();
            let (read, mut write) = UnixStream::pair().unwrap();
            let active = if second { &b } else { &a };
            a.notify().unwrap();
            b.notify().unwrap();
            assert!(!Readiness::wait_many(&[&a, &b]).unwrap());
            active.enter(7);
            active.await_descriptor(read.as_raw_fd());
            active.parked(7);
            // Drain setup wakeups before forcing the descriptor edge.
            assert!(a.ready().unwrap().is_empty());
            assert!(b.ready().unwrap().is_empty());
            write.write_all(b"x").unwrap();
            assert!(Readiness::wait_many(&[&a, &b]).unwrap());
            assert_eq!(active.ready().unwrap(), [7]);
            assert!(
                if second { a.ready() } else { b.ready() }
                    .unwrap()
                    .is_empty()
            );
        }
    }
    #[test]
    fn explicit_driver_notification_survives_before_first_wait_without_readying_a_token() {
        let readiness = Readiness::default();
        readiness.enter(10);
        {
            let mut pending = readiness.pending();
            pending.deadline = Some(Instant::now() + Duration::from_secs(5));
        }
        readiness.parked(10);
        readiness.notify().unwrap();
        let start = Instant::now();
        assert!(readiness.wait().unwrap());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(readiness.ready().unwrap().is_empty());
        readiness.finish(10);
        assert!(
            !readiness.wait().unwrap(),
            "wake-only is not an event source"
        );
    }
    #[test]
    fn sleeping_driver_releases_state_and_observes_worker_delivery() {
        let readiness = Readiness::default();
        readiness.enter(10);
        {
            let mut pending = readiness.pending();
            pending.deadline = Some(Instant::now() + Duration::from_secs(5));
            pending.delivery = true;
        }
        readiness.parked(10);
        let worker = readiness.clone();
        let joined = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            worker.deliver(10);
        });
        let start = Instant::now();
        assert!(readiness.wait().unwrap());
        joined.join().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "driver held the state lock"
        );
        assert_eq!(readiness.ready().unwrap(), [10]);
    }
}

#[cfg(test)]
mod tagged_worker_tests {
    use super::*;
    #[test]
    fn tagged_runs_have_independent_entered_requests_on_one_driver() {
        let readiness = Readiness::default();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|scope| {
            for tag in 0..8 {
                let run = readiness.tagged(tag);
                let barrier = barrier.clone();
                scope.spawn(move || {
                    run.enter(tag + 1);
                    barrier.wait();
                    run.pending().offset = tag as usize + 100;
                    assert_eq!(run.offset(), tag as usize + 100);
                });
            }
        });
        assert_eq!(readiness.0.lock().unwrap().pending.len(), 8);
    }
}

#[cfg(test)]
mod timed_isolation_tests {
    use super::*;
    #[test]
    fn pending_external_delivery_refuses_before_virtual_advance_or_blocked() {
        let readiness = Readiness::virtual_clock(0);
        readiness.isolate_process_clock();
        readiness.enter(7);
        readiness.await_delivery();
        readiness.parked(7);
        readiness.provider_deadline(Some(5));
        assert!(matches!(
            readiness.wait(),
            Err(AdapterError::Unsupported(_))
        ));
        assert_eq!(readiness.virtual_time(), Some(0));
        readiness.finish(7);
        assert!(readiness.wait().unwrap());
        assert_eq!(readiness.virtual_time(), Some(5));
    }
}

#[cfg(test)]
#[path = "readiness/modules.rs"]
mod runtime50;
