//! The home bridge (design §6 Io, Home thread; §5 Claim versus withdraw).
//!
//! Non-`Send` Io state (a custom adapter, `Rc` sinks, readiness, the launcher's echo and
//! report state) stays on the admission thread, the home thread. A task on another worker
//! reaches it by posting its request here and waiting for the completion. Each posted request
//! is a record of `hirsel_free::claim` (the protocol `proofs/free` model-checks, the bridge's
//! claim and close included): the home thread claims it by CAS `Prepared -> ClaimedForIssue`
//! before acting on it, and [`Bridge::close`] withdraws every unclaimed one by CAS
//! `Prepared -> Withdrawn`, so after a stop no worker reaches home state again. A request the
//! home thread claimed before the close is in doubt: its completion is quarantined, never
//! delivered, and its buffers are freed by whichever holder leaves last.
//!
//! Deviation from the design's per-worker SPSC slot: one multi-producer queue of records,
//! each with its own completion wait (a futex-backed condition variable). The claim protocol
//! and its record are the same; a slot per worker would save the queue lock, a headroom item
//! the B1 `syscall_serialisation` metric reports.

use crate::io::{Adapter, AdapterError, Completion, Request};
use crate::io_metrics::IoMetrics;
use hirsel_free::claim::{Claim, Requests, Sweep};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// How a bridged request ended for its poster.
#[derive(Debug)]
pub enum Bridged {
    Done(Result<Completion, AdapterError>),
    /// Withdrawn before the home thread claimed it: it was never issued.
    Withdrawn,
    /// Claimed before the bridge closed: issued or not, its completion is quarantined.
    InDoubt,
}

/// A posted request's buffers: owned by its record, shared by poster and table.
#[derive(Debug, Default)]
pub struct Slot {
    request: Mutex<Option<Request>>,
    outcome: Mutex<Option<Bridged>>,
    ended: Condvar,
}

type Record = hirsel_free::claim::Request<Slot>;

/// One run's bridge to its home thread.
#[derive(Default)]
pub struct Bridge {
    requests: Requests<Slot>,
    posted: Mutex<VecDeque<Arc<Record>>>,
    doorbell: Condvar,
    next: AtomicU64,
    closed: AtomicBool,
    metrics: Option<Arc<IoMetrics>>,
}

/// A posted request's claim on its completion.
pub struct Ticket(Arc<Record>);

impl Bridge {
    pub fn with_metrics(metrics: Option<Arc<IoMetrics>>) -> Bridge {
        Bridge {
            metrics,
            ..Bridge::default()
        }
    }

    /// Post `request` for the home thread (any thread). After [`close`](Self::close) it is
    /// withdrawn at once.
    pub fn post(&self, request: Request) -> Ticket {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let record = self.requests.prepare(
            id,
            Slot {
                request: Mutex::new(Some(request)),
                ..Slot::default()
            },
        );
        if record.state() == Claim::Prepared {
            self.posted.lock().unwrap().push_back(record.clone());
            self.doorbell.notify_one();
        }
        Ticket(record)
    }

    /// The home thread: claim and perform every posted request on `adapter`. Returns how
    /// many it performed.
    pub fn serve(&self, adapter: &mut dyn Adapter) -> usize {
        let mut served = 0;
        loop {
            let Some(record) = self.posted.lock().unwrap().pop_front() else {
                return served;
            };
            let slot = record.buffers();
            if !record.claim() {
                // Withdrawn by a close: never issued.
                notify(slot);
                continue;
            }
            let request = slot
                .request
                .lock()
                .unwrap()
                .take()
                .expect("a claimed bridge request has its request");
            let completion = adapter.call(request);
            if let Some(metrics) = &self.metrics {
                metrics.bridged();
            }
            record.finish();
            self.requests.retire(record.id);
            let mut outcome = slot.outcome.lock().unwrap();
            // A completion after the close is quarantined: the poster learns only that it was
            // in doubt, and the completion is dropped here.
            *outcome = Some(if self.requests_closed() {
                Bridged::InDoubt
            } else {
                Bridged::Done(completion)
            });
            slot.ended.notify_all();
            served += 1;
        }
    }

    /// The home thread: wait up to `timeout` for a posted request.
    pub fn wait_posted(&self, timeout: Duration) -> bool {
        let posted = self.posted.lock().unwrap();
        let (posted, _) = self
            .doorbell
            .wait_timeout_while(posted, timeout, |posted| posted.is_empty())
            .unwrap();
        !posted.is_empty()
    }

    fn requests_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// After a stop: withdraw every unclaimed request and report the claimed ones, which are
    /// in doubt. No request posted from now on is issued.
    pub fn close(&self) -> Sweep {
        self.closed.store(true, Ordering::Release);
        let sweep = self.requests.sweep();
        for record in std::mem::take(&mut *self.posted.lock().unwrap()) {
            notify(record.buffers());
        }
        sweep
    }
}

fn notify(slot: &Slot) {
    let _outcome = slot.outcome.lock().unwrap();
    slot.ended.notify_all();
}

impl Ticket {
    /// Wait for the completion (the posting thread).
    pub fn wait(self) -> Bridged {
        let slot = self.0.buffers();
        let mut outcome = slot.outcome.lock().unwrap();
        loop {
            if let Some(outcome) = outcome.take() {
                return outcome;
            }
            match self.0.state() {
                Claim::Withdrawn => return Bridged::Withdrawn,
                // Claimed, or finished with the outcome about to be stored: wait for it.
                Claim::Prepared | Claim::ClaimedForIssue | Claim::Done => {}
            }
            outcome = slot.ended.wait(outcome).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Request as R;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;

    /// A custom adapter with a non-`Send` sink: it lives on the home thread only.
    struct HomeOnly {
        sink: Rc<RefCell<Vec<u8>>>,
        home: std::thread::ThreadId,
    }
    impl Adapter for HomeOnly {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            assert_eq!(
                std::thread::current().id(),
                self.home,
                "home state off home"
            );
            match request {
                R::Print { bytes, .. } => {
                    self.sink.borrow_mut().extend_from_slice(&bytes);
                    Ok(Completion::Unit(Ok(())))
                }
                _ => Err(AdapterError::Unsupported("test".into())),
            }
        }
    }

    fn print(byte: u8) -> Request {
        R::Print {
            stdout: 3,
            bytes: vec![byte],
        }
    }

    #[test]
    fn a_custom_non_send_adapter_and_sink_serve_eight_workers() {
        let bridge = Arc::new(Bridge::with_metrics(Some(Arc::default())));
        let sink = Rc::new(RefCell::new(Vec::new()));
        let mut adapter = HomeOnly {
            sink: sink.clone(),
            home: std::thread::current().id(),
        };
        let workers: Vec<_> = (0..8u8)
            .map(|w| {
                let bridge = bridge.clone();
                std::thread::spawn(move || {
                    for _ in 0..200 {
                        match bridge.post(print(w)).wait() {
                            Bridged::Done(Ok(Completion::Unit(Ok(())))) => {}
                            other => panic!("{other:?}"),
                        }
                    }
                })
            })
            .collect();
        let mut served = 0;
        while served < 1600 {
            bridge.wait_posted(Duration::from_millis(10));
            served += bridge.serve(&mut adapter);
        }
        for worker in workers {
            worker.join().unwrap();
        }
        let mut counts = [0; 8];
        for &b in sink.borrow().iter() {
            counts[usize::from(b)] += 1;
        }
        assert_eq!(counts, [200; 8]);
        let metrics = bridge.metrics.as_ref().unwrap().values();
        assert_eq!(metrics[0], ("syscall_serialisation", 1.0));
    }

    #[test]
    fn a_request_unclaimed_at_the_close_is_withdrawn_and_never_issued() {
        let bridge = Bridge::default();
        let ticket = bridge.post(print(1));
        let sweep = bridge.close();
        assert_eq!(sweep.withdrawn, vec![0]);
        let sink = Rc::new(RefCell::new(Vec::new()));
        let mut adapter = HomeOnly {
            sink: sink.clone(),
            home: std::thread::current().id(),
        };
        assert_eq!(bridge.serve(&mut adapter), 0);
        assert!(matches!(ticket.wait(), Bridged::Withdrawn));
        // Posted after the close: withdrawn at once.
        assert!(matches!(bridge.post(print(2)).wait(), Bridged::Withdrawn));
        assert!(sink.borrow().is_empty());
    }

    /// An adapter whose call reports that it was entered, then waits to be let go.
    struct Gated {
        entered: mpsc::Sender<()>,
        go: mpsc::Receiver<()>,
        calls: Rc<RefCell<u32>>,
    }
    impl Adapter for Gated {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
            *self.calls.borrow_mut() += 1;
            self.entered.send(()).unwrap();
            self.go.recv().unwrap();
            Ok(Completion::Unit(Ok(())))
        }
    }

    #[test]
    fn a_request_claimed_before_the_close_is_in_doubt_and_its_completion_quarantined() {
        // The claim matrix on the bridge: stop after the claim, during the host call, and
        // after the call before the poster saw the completion.
        let bridge = Arc::new(Bridge::default());
        let (entered_tx, entered) = mpsc::channel();
        let (go, go_rx) = mpsc::channel();
        let poster = {
            let bridge = bridge.clone();
            std::thread::spawn(move || bridge.post(print(1)).wait())
        };
        while !bridge.wait_posted(Duration::from_millis(10)) {}
        let closer = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                entered.recv().unwrap();
                let sweep = bridge.close();
                go.send(()).unwrap();
                sweep
            })
        };
        let calls = Rc::new(RefCell::new(0));
        let mut adapter = Gated {
            entered: entered_tx,
            go: go_rx,
            calls: calls.clone(),
        };
        assert_eq!(bridge.serve(&mut adapter), 1);
        let sweep = closer.join().unwrap();
        assert_eq!(sweep.in_doubt, vec![0]);
        assert!(sweep.withdrawn.is_empty());
        assert!(matches!(poster.join().unwrap(), Bridged::InDoubt));
        assert_eq!(*calls.borrow(), 1, "issued exactly once");
    }

    #[test]
    fn exactly_one_of_claim_and_withdraw_wins_under_racing_closes() {
        for _ in 0..200 {
            let bridge = Arc::new(Bridge::default());
            let tickets: Vec<_> = (0..4).map(|i| bridge.post(print(i))).collect();
            let closer = {
                let bridge = bridge.clone();
                std::thread::spawn(move || bridge.close())
            };
            let sink = Rc::new(RefCell::new(Vec::new()));
            let mut adapter = HomeOnly {
                sink: sink.clone(),
                home: std::thread::current().id(),
            };
            let served = bridge.serve(&mut adapter);
            let sweep = closer.join().unwrap();
            let mut done = 0;
            let mut withdrawn = 0;
            for ticket in tickets {
                match ticket.wait() {
                    Bridged::Done(_) | Bridged::InDoubt => done += 1,
                    Bridged::Withdrawn => withdrawn += 1,
                }
            }
            assert_eq!(done, served);
            assert_eq!(withdrawn, sweep.withdrawn.len());
            assert_eq!(done + withdrawn, 4);
            assert_eq!(sink.borrow().len(), served, "only claimed requests issued");
        }
    }
}
