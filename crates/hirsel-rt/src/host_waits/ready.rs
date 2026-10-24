use super::super::*;

struct OneEdge(bool);
impl crate::io::Adapter for OneEdge {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        Ok(Ready::Tokens(if std::mem::take(&mut self.0) {
            vec![7]
        } else {
            vec![]
        }))
    }
}

#[test]
fn home_readiness_retains_only_affected_tokens_and_generation_before_save() {
    let mut binding = Binding::new(OneEdge(true));
    for token in 0..1024 {
        binding.waits.native.insert(
            token,
            Native {
                request: Request::SleepUntil {
                    clock: 17,
                    deadline: 100,
                },
                task: Some(token as u32),
                generation: 41,
                notified: false,
                ready: false,
            },
        );
    }
    binding.waits.native.get_mut(&7).unwrap().task = None;
    assert!(
        native_ready_binding(&mut binding, &[])
            .unwrap()
            .hosts
            .is_empty()
    );
    assert_eq!(
        binding
            .waits
            .native_ready
            .members_unordered()
            .collect::<Vec<_>>(),
        vec![7]
    );
    binding.waits.native.get_mut(&7).unwrap().task = Some(3);
    let ready = native_ready_binding(&mut binding, &[]).unwrap();
    assert_eq!(ready.hosts.len(), 1);
    assert_eq!(ready.hosts[0].token, 7);
    assert_eq!(ready.hosts[0].task, 3);
    assert_eq!(ready.hosts[0].generation, 41);
    assert!(ready.hosts[0].ready);
    assert_eq!(binding.waits.native.len(), 1024);
}

struct CompletedClock(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl crate::io::Adapter for CompletedClock {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        unreachable!()
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert!(matches!(
            request,
            Request::SleepUntil {
                clock: 17,
                deadline: 100
            }
        ));
        Ok(Completion::Wait(Ok(())))
    }
    fn finish(&mut self, _: u64) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[test]
fn home_timer_expiry_is_latched_until_publication_and_removed_on_ack() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let finishes = Arc::new(AtomicUsize::new(0));
    let mut binding = Binding::new(CompletedClock(finishes.clone()));
    binding.waits.armed.insert(
        7,
        (
            42,
            5,
            Request::SleepUntil {
                clock: 17,
                deadline: 100,
            },
        ),
    );
    let expiry = expired_binding(&mut binding, &Ready::Tokens(vec![7])).unwrap();
    assert_eq!(
        expiry,
        vec![ExpiredScope {
            scope: 42,
            generation: 5,
            token: 7
        }]
    );
    // Home may consume the edge before scope/Done publishes its token. An
    // empty later readiness poll must return the same latched expiry.
    assert_eq!(
        expired_binding(&mut binding, &Ready::Tokens(vec![])).unwrap(),
        expiry
    );
    assert_eq!(finishes.load(Ordering::Relaxed), 1);
    assert_eq!(binding.waits.armed.len(), 1);
    acknowledge_expired(&mut binding, &[7]);
    assert!(
        expired_binding(&mut binding, &Ready::Tokens(vec![]))
            .unwrap()
            .is_empty()
    );
    assert!(binding.waits.armed.is_empty());
    assert!(binding.waits.armed_due.is_empty());
    assert_eq!(finishes.load(Ordering::Relaxed), 1);
}
