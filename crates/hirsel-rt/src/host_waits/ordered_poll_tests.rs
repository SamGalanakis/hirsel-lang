use super::*;
use crate::io::Adapter;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

struct Attempts {
    calls: Arc<AtomicU32>,
    finishes: Arc<AtomicU32>,
    cancellable: bool,
}
impl Adapter for Attempts {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
        if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
            Err(AdapterError::Blocked)
        } else {
            Ok(Completion::Wait(Ok(())))
        }
    }
    fn finish(&mut self, _: u64) {
        self.finishes.fetch_add(1, Ordering::Relaxed);
    }
    fn cancellable(&self, _: u64) -> bool {
        self.cancellable
    }
}
fn binding(cancellable: bool) -> (Binding, Arc<AtomicU32>, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let finishes = Arc::new(AtomicU32::new(0));
    let mut binding = Binding::new(Attempts {
        calls: calls.clone(),
        finishes: finishes.clone(),
        cancellable,
    });
    binding.waits.parked.insert(
        7,
        Parked {
            request: Request::SleepUntil {
                clock: 99,
                deadline: 100,
            },
            borrowed: None,
            task: Some(3),
            completion: None,
        },
    );
    (binding, calls, finishes)
}

#[test]
fn ordered_poll_observation_distinguishes_skipped_blocked_and_completed_attempts() {
    let (mut binding, calls, finishes) = binding(true);
    let mut snapshot = None;
    assert!(
        !unsafe { poll_binding_observed(&mut binding, 7, None, false, Some(&mut snapshot)) }
            .unwrap()
    );
    assert!(snapshot.is_none());
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(
        !unsafe { poll_binding_observed(&mut binding, 7, None, true, Some(&mut snapshot)) }
            .unwrap()
    );
    assert_eq!(snapshot, Some(Err(AdapterError::Blocked)));
    assert!(binding.waits.parked[&7].completion.is_none());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(finishes.load(Ordering::Relaxed), 0);
    snapshot = None;
    assert!(
        unsafe { poll_binding_observed(&mut binding, 7, None, true, Some(&mut snapshot)) }.unwrap()
    );
    assert_eq!(snapshot, Some(Ok(Completion::Wait(Ok(())))));
    assert_eq!(binding.waits.parked[&7].completion, snapshot);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(finishes.load(Ordering::Relaxed), 1);
    binding.waits.parked.remove(&7);
    assert_eq!(snapshot, Some(Ok(Completion::Wait(Ok(())))));
}

#[test]
fn ordered_poll_observation_records_cancellation_only_when_adapter_allows_it() {
    for allowed in [false, true] {
        let (mut binding, calls, finishes) = binding(allowed);
        let mut snapshot = None;
        let done =
            unsafe { poll_binding_observed(&mut binding, 7, Some(5), false, Some(&mut snapshot)) }
                .unwrap();
        assert_eq!(done, allowed);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(finishes.load(Ordering::Relaxed), u32::from(allowed));
        assert_eq!(
            snapshot,
            allowed.then_some(Ok(Completion::Wait(Err(crate::io::WaitError::Canceled(5)))))
        );
    }
    // A no-observer caller retains the same skip/attempt/finish behavior.
    let (mut binding, calls, finishes) = binding(true);
    assert!(!unsafe { poll_binding(&mut binding, 7, None, false) }.unwrap());
    assert!(!unsafe { poll_binding(&mut binding, 7, None, true) }.unwrap());
    assert!(unsafe { poll_binding(&mut binding, 7, None, true) }.unwrap());
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(finishes.load(Ordering::Relaxed), 1);
}
