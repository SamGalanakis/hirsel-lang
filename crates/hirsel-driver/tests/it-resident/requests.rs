use hirsel_artifact_store::{Action, Address, Domain, Reference};
use hirsel_driver::resident::{Host, RequestError};
use hirsel_engine::loader_inputs::{InputLimits, LoaderInputs};
use hirsel_engine::requests::{OutputReadiness, RequestEngine, SelectedRequest, observe_inputs};
use hirsel_engine::runtime::{ProductRef, QueryId, QueryRuntime, RequestId};
use hirsel_syntax::ContentHash;
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::task::Poll;

fn product() -> ProductRef {
    ProductRef {
        address: Address(ContentHash::of_bytes(b"address")),
        action: Action(ContentHash::of_bytes(b"complete action")),
        record: Domain {
            kind: 999,
            schema: 1,
        }
        .reference(b"output")
        .unwrap(),
    }
}

/// A transport probe, not a compiler or a dependency/coalescing implementation.
/// Actual task scheduling and graph validation are tested by the shared runtime.
#[derive(Default)]
struct ProbeRuntime {
    next: u64,
    events: Vec<String>,
    replies: VecDeque<Poll<Result<Vec<ProductRef>, Reference>>>,
}
impl QueryRuntime for ProbeRuntime {
    fn request(&mut self, roots: &[QueryId]) -> RequestId {
        self.next += 1;
        assert_eq!(
            roots,
            &[QueryId {
                kind: 1,
                address: product().address
            }]
        );
        self.events.push(format!("request {}", self.next));
        RequestId(self.next)
    }
    fn poll(&mut self, request: RequestId) -> Poll<Result<Vec<ProductRef>, Reference>> {
        self.events.push(format!("poll {}", request.0));
        self.replies.pop_front().unwrap_or(Poll::Pending)
    }
    fn cancel(&mut self, request: RequestId) {
        self.events.push(format!("cancel {}", request.0));
    }
    fn evict_to(&mut self, retained_bytes: u64) {
        self.events.push(format!("evict {retained_bytes}"));
    }
}

struct Ready {
    runtime: Arc<Mutex<ProbeRuntime>>,
    fail: bool,
}
impl OutputReadiness for Ready {
    type Output = Vec<ProductRef>;
    type Error = &'static str;
    fn ready(&mut self, products: &[ProductRef]) -> Result<Self::Output, Self::Error> {
        // The runtime lock must be released before output I/O/callbacks.
        self.runtime.try_lock().unwrap().events.push("ready".into());
        if self.fail {
            Err("missing executable")
        } else {
            Ok(products.to_vec())
        }
    }
}

struct Selector {
    runtime: Arc<Mutex<ProbeRuntime>>,
}
impl RequestEngine for Selector {
    type Request = bool;
    type Error = &'static str;
    type Runtime = ProbeRuntime;
    type Readiness = Ready;
    fn select(&mut self, fail: &bool) -> Result<SelectedRequest<ProbeRuntime, Ready>, Self::Error> {
        self.runtime
            .lock()
            .unwrap()
            .events
            .push("observe/seal/select".into());
        Ok(SelectedRequest {
            runtime: self.runtime.clone(),
            roots: vec![QueryId {
                kind: 1,
                address: product().address,
            }],
            output: Ready {
                runtime: self.runtime.clone(),
                fail: *fail,
            },
        })
    }
}
fn host() -> (Host<Selector>, Arc<Mutex<ProbeRuntime>>) {
    let runtime = Arc::new(Mutex::new(ProbeRuntime::default()));
    (
        Host::new(Selector {
            runtime: runtime.clone(),
        }),
        runtime,
    )
}
fn reply(runtime: &Arc<Mutex<ProbeRuntime>>, result: Poll<Result<Vec<ProductRef>, Reference>>) {
    runtime.lock().unwrap().replies.push_back(result);
}
fn events(runtime: &Arc<Mutex<ProbeRuntime>>) -> Vec<String> {
    runtime.lock().unwrap().events.clone()
}

#[test]
fn pending_request_cancels_once_and_forwards_explicit_eviction() {
    let (mut host, runtime) = host();
    let mut request = host.request(&false).unwrap();
    assert_eq!(request.poll(), Poll::Pending);
    request.evict_to(0);
    request.evict_to(17);
    drop(request);
    assert_eq!(
        events(&runtime),
        [
            "observe/seal/select",
            "request 1",
            "poll 1",
            "evict 0",
            "evict 17",
            "cancel 1"
        ]
    );
}

#[test]
fn shared_runtime_cancellation_does_not_cancel_another_subscription() {
    let (mut host, runtime) = host();
    let abandoned = host.request(&false).unwrap();
    let mut live = host.request(&false).unwrap();
    drop(abandoned);
    reply(&runtime, Poll::Ready(Ok(vec![product()])));
    assert_eq!(live.poll(), Poll::Ready(Ok(vec![product()])));
    drop(live);
    assert_eq!(
        events(&runtime),
        [
            "observe/seal/select",
            "request 1",
            "observe/seal/select",
            "request 2",
            "cancel 1",
            "poll 2",
            "ready"
        ]
    );
}

#[test]
fn host_shutdown_leaves_live_snapshot_owned_by_lease() {
    let (mut host, runtime) = host();
    let mut request = host.request(&false).unwrap();
    drop(host);
    reply(&runtime, Poll::Ready(Ok(vec![product()])));
    assert_eq!(request.poll(), Poll::Ready(Ok(vec![product()])));
    drop(request);
    assert!(!events(&runtime).iter().any(|e| e.starts_with("cancel")));
}

#[test]
fn query_failure_never_materializes_output() {
    let (mut host, runtime) = host();
    let mut request = host.request(&false).unwrap();
    reply(&runtime, Poll::Ready(Err(product().record)));
    assert_eq!(
        request.poll(),
        Poll::Ready(Err(RequestError::Query(product().record)))
    );
    drop(request);
    assert_eq!(
        events(&runtime),
        ["observe/seal/select", "request 1", "poll 1"]
    );
}

#[test]
fn output_readiness_failure_is_not_request_success() {
    let (mut host, runtime) = host();
    let mut request = host.request(&true).unwrap();
    reply(&runtime, Poll::Ready(Ok(vec![product()])));
    assert_eq!(
        request.poll(),
        Poll::Ready(Err(RequestError::Output("missing executable")))
    );
    drop(request);
    assert_eq!(
        events(&runtime),
        ["observe/seal/select", "request 1", "poll 1", "ready"]
    );
}

fn limits() -> InputLimits {
    InputLimits {
        bytes: 64 << 10,
        operations: 100,
    }
}

#[test]
fn capture_retries_racing_discovery_and_keeps_the_complete_input_union() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    let sidecar = tree.path().join("main.hir.json");
    std::fs::write(&source, b"before").unwrap();
    let mut rounds = 0;
    let (result, snapshot) = observe_inputs(limits(), 2, |inputs| -> io::Result<_> {
        rounds += 1;
        let bytes = inputs.read(&source)?;
        assert_eq!(
            inputs.metadata(&sidecar).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        inputs.directory(tree.path())?;
        inputs.canonicalize(&source)?;
        if rounds == 1 {
            std::fs::write(&source, b"after!")?;
        }
        Ok(bytes)
    })
    .unwrap();
    assert_eq!(rounds, 2);
    assert_eq!(&*result.unwrap(), b"after!");
    assert_eq!(&*snapshot.read(&source).unwrap(), b"after!");
    std::fs::write(&sidecar, b"{}").unwrap();
    assert_eq!(
        snapshot.changed_on_disk().len(),
        2,
        "absence and directory reads both change"
    );
    assert_eq!(
        snapshot.metadata(&sidecar).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn stable_discovery_error_still_seals_its_observed_inputs() {
    let tree = tempfile::tempdir().unwrap();
    let missing = tree.path().join("deleted.hir");
    let (result, snapshot) = observe_inputs(limits(), 1, |inputs| inputs.read(&missing)).unwrap();
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
    assert!(snapshot.changed_on_disk().is_empty());
    std::fs::write(&missing, b"now present").unwrap();
    assert_eq!(snapshot.changed_on_disk().len(), 1);
    assert_eq!(
        snapshot.read(&missing).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn unsealable_input_union_refuses_admission_after_bounded_attempts() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"first").unwrap();
    let mut rounds = 0;
    let result = observe_inputs(limits(), 3, |inputs| -> io::Result<_> {
        rounds += 1;
        let bytes = inputs.read(&source)?;
        std::fs::write(&source, format!("version {rounds}"))?;
        Ok(bytes)
    });
    assert!(result.is_err());
    assert_eq!(rounds, 3);
    let result = observe_inputs(limits(), 0, |_| -> io::Result<()> {
        panic!("no attempt allowed")
    });
    assert!(result.is_err());
}

#[test]
fn same_metadata_edit_deletion_and_move_cannot_reuse_old_observation() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"before").unwrap();
    let (_, snapshot) = observe_inputs(limits(), 1, |inputs| inputs.read(&source)).unwrap();
    let modified = std::fs::metadata(&source).unwrap().modified().unwrap();
    std::fs::write(&source, b"after!").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_eq!(snapshot.changed_on_disk().len(), 1);
    std::fs::rename(&source, tree.path().join("moved.hir")).unwrap();
    assert_eq!(snapshot.changed_on_disk().len(), 1);
    assert_eq!(&*snapshot.read(&source).unwrap(), b"before");
    let (deleted, _) = observe_inputs(limits(), 1, |inputs| inputs.read(&source)).unwrap();
    assert_eq!(deleted.unwrap_err().kind(), io::ErrorKind::NotFound);
}

#[test]
fn input_limit_is_a_failure_not_an_empty_successful_snapshot() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"source").unwrap();
    let (result, snapshot) = observe_inputs(
        InputLimits {
            bytes: 0,
            operations: 0,
        },
        1,
        |inputs| inputs.read(&source),
    )
    .unwrap();
    assert!(
        result
            .unwrap_err()
            .get_ref()
            .unwrap()
            .is::<hirsel_engine::loader_inputs::InputLimit>()
    );
    assert_eq!(snapshot.retained_bytes(), 0);
}
