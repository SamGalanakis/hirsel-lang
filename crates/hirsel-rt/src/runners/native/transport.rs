//! Owned handoff to home; no worker can reach a non-Send callee adapter.
use super::*;
use std::collections::BTreeSet;

pub(super) struct Spec {
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
    sent: BTreeSet<u64>,
}

pub(super) struct Transport {
    rows: Mutex<Rows>,
    pub io: Arc<crate::io_real::ConcurrentIo>,
    pub signal: Arc<crate::readiness::Signal>,
    pub driver: crate::readiness::Readiness,
    work: crate::readiness::Readiness,
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

    /// A bounded host attempt starts a prepared callee at most once, then the
    /// ordinary provider's delivered completion answers every later retry.
    pub fn issue(&self, token: u64, key: [u8; 32]) {
        let mut rows = self.rows.lock().unwrap();
        if let Some(outcome) = rows.immediate.remove(&key) {
            self.io.deliver(token, outcome);
        }
        if let Some(spec) = rows.prepared.remove(&key) {
            assert!(rows.sent.insert(token));
            rows.queued.push_back((token, key, spec));
            self.signal.notify();
        }
    }

    pub fn take(&self) -> VecDeque<(u64, [u8; 32], Spec)> {
        std::mem::take(&mut self.rows.lock().unwrap().queued)
    }

    pub fn finish(&self, token: u64) {
        self.rows.lock().unwrap().sent.remove(&token);
    }

    pub fn sources(&self) -> Option<crate::readiness::Readiness> {
        (!self.rows.lock().unwrap().sent.is_empty()).then(|| self.work.clone())
    }
}
