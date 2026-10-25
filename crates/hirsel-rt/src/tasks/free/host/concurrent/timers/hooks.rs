//! The deterministic runtime driver's adapter to the concrete scope-timer
//! index (r6 S6): the production [`Service::arm`], `Timers::poll`,
//! `Timers::expire` and [`Service::disarm`] over a virtual-clock provider.
//! The provider answers each bounded `SleepUntil` attempt against the
//! driver's time and records every finish; the index decides the rest.
use super::super::*;
use crate::tasks::registry::ScopeKey;

/// A virtual clock provider: a deadline at or before now is due.
#[derive(Default)]
pub(crate) struct Clock {
    now: AtomicU64,
    /// Blocked attempts by token: the deadlines a provider would watch.
    armed: Mutex<BTreeMap<u64, u64>>,
    finishes: Mutex<Vec<u64>>,
}

impl ConcurrentAdapter for Clock {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        panic!("scope control does not prepare a program host operation");
    }

    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        panic!("a scope timer attempt names its token");
    }

    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        let Request::SleepUntil { deadline, .. } = request else {
            panic!("a scope timer asked for {request:?}");
        };
        if deadline <= self.now.load(Ordering::Acquire) {
            Ok(Completion::Wait(Ok(())))
        } else {
            self.armed.lock().unwrap().insert(token, deadline);
            Err(AdapterError::Blocked)
        }
    }

    fn finish(&self, token: u64) {
        self.armed.lock().unwrap().remove(&token);
        self.finishes.lock().unwrap().push(token);
    }
}

pub(crate) struct Deadlines {
    io: Service,
    clock: Arc<Clock>,
}

impl Deadlines {
    pub(crate) fn new() -> Self {
        let clock = Arc::new(Clock::default());
        let io = Service {
            adapter: clock.clone(),
            live: Default::default(),
            jobs: Default::default(),
            driver: None,
            rows: Default::default(),
            pending: AtomicUsize::new(0),
            opaque: Default::default(),
            timers: Default::default(),
        };
        Self { io, clock }
    }

    pub(crate) fn now(&self) -> u64 {
        self.clock.now.load(Ordering::Acquire)
    }

    /// Home's arm: `Some(token)` is armed, `None` was already due.
    pub(crate) fn arm(&self, owner: ScopeKey, clock: u64, at: u64) -> Option<u64> {
        self.io
            .arm(owner, clock, at)
            .expect("a virtual clock refuses nothing")
    }

    pub(crate) fn disarm(&self, token: u64) {
        self.io.disarm(token);
    }

    pub(crate) fn pending(&self) -> bool {
        self.io.timers.pending()
    }

    /// Whether the provider watches some blocked deadline.
    pub(crate) fn watching(&self) -> bool {
        !self.clock.armed.lock().unwrap().is_empty()
    }

    /// Advance to the earliest watched deadline and poll exactly the tokens
    /// the provider reports ready, as one readiness snapshot. Returns them
    /// with their deadlines, in token order.
    pub(crate) fn step(&self) -> Vec<(u64, u64)> {
        let armed = self.clock.armed.lock().unwrap().clone();
        let Some(&next) = armed.values().min() else {
            return Vec::new();
        };
        let now = next.max(self.now());
        self.clock.now.store(now, Ordering::Release);
        let ready: Vec<(u64, u64)> = armed.into_iter().filter(|&(_, at)| at <= now).collect();
        let tokens = ready.iter().map(|&(token, _)| token).collect();
        self.io
            .timers
            .poll(&*self.clock, &crate::io::Ready::Tokens(tokens))
            .expect("a virtual clock refuses nothing");
        ready
    }

    /// Latched due timers whose expiry `expire` has not consumed.
    pub(crate) fn due(&self) -> usize {
        self.io.timers.0.lock().unwrap().due.len()
    }

    /// Expire every due timer whose scope published its token.
    pub(crate) fn expire(&self, s: &mut Scheduler) -> bool {
        self.io.timers.expire(s)
    }

    pub(crate) fn finishes(&self) -> Vec<u64> {
        self.clock.finishes.lock().unwrap().clone()
    }
}
