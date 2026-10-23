//! Owned handoff to home; no worker can reach a non-Send callee adapter.
use super::*;
use std::collections::BTreeSet;

pub(super) struct Spec {
    pub request: RunnerRequest,
    pub caller: [u8; 16],
    pub start: Start,
    pub grant: RunnerGrant,
    pub ordinal: u64,
    pub lose_reply: bool,
    pub crash_after: Option<u32>,
}

#[derive(Default)]
struct Rows {
    prepared: BTreeMap<[u8; 32], Spec>,
    immediate: BTreeMap<[u8; 32], RunnerCompletion>,
    queued: VecDeque<(u64, [u8; 32], Spec)>,
    active: BTreeSet<[u8; 32]>,
    outcomes: BTreeMap<[u8; 32], RunnerCompletion>,
    cancels: VecDeque<[u8; 32]>,
    waiters: BTreeMap<[u8; 32], (u32, u32)>,
}

pub(super) struct Transport {
    rows: Mutex<Rows>,
    pub io: Arc<crate::io_real::ConcurrentIo>,
    pub signal: Arc<crate::readiness::Signal>,
    pub driver: crate::readiness::Readiness,
    pub work: crate::readiness::Readiness,
}

impl Transport {
    pub fn new(io: Arc<crate::io_real::ConcurrentIo>) -> Result<Arc<Self>, AdapterError> {
        use crate::io_concurrent::ConcurrentAdapter;
        let signal = Arc::new(crate::readiness::Signal::new()?);
        let work = crate::readiness::Readiness::default();
        work.enter(1);
        work.await_descriptor(signal.fd());
        work.parked(1);
        Ok(Arc::new(Self {
            rows: Default::default(),
            driver: io.readiness().expect("native runner readiness"),
            io,
            signal,
            work,
        }))
    }

    pub fn prepare(&self, key: [u8; 32], spec: Spec) {
        assert!(
            self.rows
                .lock()
                .unwrap()
                .prepared
                .insert(key, spec)
                .is_none()
        );
    }

    pub fn immediate(&self, key: [u8; 32], outcome: RunnerCompletion) {
        assert!(
            self.rows
                .lock()
                .unwrap()
                .immediate
                .insert(key, outcome)
                .is_none()
        );
    }

    /// Sending acknowledges owned handoff, without waiting for a result.
    pub fn issue(&self, token: u64, key: [u8; 32]) -> Result<Completion, AdapterError> {
        let mut rows = self.rows.lock().unwrap();
        if let Some(outcome) = rows.immediate.remove(&key) {
            return Ok(Completion::Runner(outcome));
        }
        let spec = rows
            .prepared
            .remove(&key)
            .ok_or(AdapterError::InvalidCompletion)?;
        assert!(rows.active.insert(key));
        rows.queued.push_back((token, key, spec));
        self.signal.notify();
        Ok(Completion::Ready)
    }

    pub fn poll(&self, key: [u8; 32], cancel: Option<u32>) -> Result<Completion, AdapterError> {
        if cancel.is_some_and(|cause| cause > 3) {
            return Err(AdapterError::InvalidCompletion);
        }
        let mut rows = self.rows.lock().unwrap();
        if !rows.active.contains(&key) {
            return Err(AdapterError::InvalidCompletion);
        }
        if cancel.is_some() {
            rows.cancels.push_back(key);
            self.signal.notify();
        }
        if let Some(outcome) = rows.outcomes.remove(&key) {
            rows.active.remove(&key);
            rows.waiters.remove(&key);
            Ok(Completion::Runner(outcome))
        } else {
            Ok(Completion::Ready)
        }
    }

    pub fn complete(&self, key: [u8; 32], outcome: RunnerCompletion) -> Option<(u32, u32)> {
        let mut rows = self.rows.lock().unwrap();
        assert!(rows.outcomes.insert(key, outcome).is_none());
        rows.waiters.remove(&key)
    }

    pub fn waiter(&self, key: [u8; 32], waiter: (u32, u32)) -> bool {
        let mut rows = self.rows.lock().unwrap();
        if rows.outcomes.contains_key(&key) {
            return false;
        }
        rows.waiters.insert(key, waiter);
        true
    }

    pub fn cancels(&self) -> VecDeque<[u8; 32]> {
        std::mem::take(&mut self.rows.lock().unwrap().cancels)
    }

    pub fn pending(&self) -> bool {
        !self.rows.lock().unwrap().active.is_empty()
    }

    pub fn take(&self) -> VecDeque<(u64, [u8; 32], Spec)> {
        std::mem::take(&mut self.rows.lock().unwrap().queued)
    }

    pub fn sources(&self) -> Option<crate::readiness::Readiness> {
        self.pending().then(|| self.work.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_poll_rejects_unknown_duplicate_and_invalid_cancellation() {
        let real = RealIo::grant(&std::env::temp_dir())
            .unwrap()
            .into_concurrent()
            .unwrap_or_else(|_| panic!("Send runner adapter"));
        let transport = Transport::new(Arc::new(real)).unwrap();
        let key = [7; 32];
        assert_eq!(
            transport.poll(key, None),
            Err(AdapterError::InvalidCompletion)
        );
        transport.rows.lock().unwrap().active.insert(key);
        assert_eq!(
            transport.poll(key, Some(4)),
            Err(AdapterError::InvalidCompletion)
        );
        assert!(transport.cancels().is_empty());
        assert_eq!(transport.poll(key, Some(0)), Ok(Completion::Ready));
        assert_eq!(transport.cancels().into_iter().collect::<Vec<_>>(), [key]);
        let outcome = RunnerCompletion {
            outcome: CallOutcome::Canceled(1),
            callee: None,
        };
        transport.complete(key, outcome.clone());
        assert_eq!(transport.poll(key, None), Ok(Completion::Runner(outcome)));
        assert_eq!(
            transport.poll(key, None),
            Err(AdapterError::InvalidCompletion)
        );
    }
}
