//! Small immutable tasks exercising the real scheduler through the host adapter.
//! These are lifecycle controls, not compiler/verification/performance evidence.
use hirsel_artifact_store::{Action, Address, Domain};
use hirsel_driver::resident::{Host, RequestError};
use hirsel_engine::loader_inputs::{FrozenInputs, InputLimits, LoaderInputs};
use hirsel_engine::requests::{OutputReadiness, RequestEngine, SelectedRequest, observe_inputs};
use hirsel_engine::runtime::{
    AdmittedRuntime, MemoryWeight, ProcessBudget, ProductRef, QueryContext, QueryFactory, QueryId,
    QueryPoll, QueryTask, RuntimeErrors,
};
use hirsel_engine::store::{MemStore, ResultStore};
use hirsel_intern::jobs::Jobs;
use hirsel_syntax::ContentHash;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::task::Poll;
use std::time::{Duration, Instant};

const ALLOWANCE: u64 = (128 << 20) + (1 << 20);
fn root() -> QueryId {
    QueryId {
        kind: 123,
        address: Address(ContentHash::of_bytes(b"stable input root")),
    }
}
fn errors() -> RuntimeErrors {
    let r = Domain {
        kind: 999,
        schema: 1,
    }
    .reference(b"diagnostic")
    .unwrap();
    RuntimeErrors {
        resource: r,
        cycle: r,
        cancelled: r,
        protocol: r,
        panicked: r,
    }
}
struct Task {
    product: ProductRef,
    polls: Arc<AtomicUsize>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        root()
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            retained: 128,
            ..Default::default()
        }
    }
    fn poll(&mut self, _: &mut dyn QueryContext) -> QueryPoll {
        self.polls.fetch_add(1, Ordering::SeqCst);
        QueryPoll::Complete(self.product.clone())
    }
}
struct Factory {
    product: ProductRef,
    polls: Arc<AtomicUsize>,
}
impl QueryFactory for Factory {
    fn create(&self, _: QueryId) -> Result<Box<dyn QueryTask>, hirsel_artifact_store::Reference> {
        Ok(Box::new(Task {
            product: self.product.clone(),
            polls: self.polls.clone(),
        }))
    }
}
struct Readiness {
    store: Arc<dyn ResultStore>,
}
impl OutputReadiness for Readiness {
    type Output = Vec<u8>;
    type Error = String;
    fn ready(&mut self, products: &[ProductRef]) -> Result<Vec<u8>, String> {
        let [product] = products else {
            return Err("wrong root count".into());
        };
        self.store
            .get_content(
                product
                    .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                    .map_err(|e| e.to_string())?
                    .reference(),
            )
            .map(|b| b.to_vec())
            .ok_or_else(|| "missing product".into())
    }
}
struct Engine {
    store: Arc<dyn ResultStore>,
    budget: Arc<ProcessBudget>,
    polls: Arc<AtomicUsize>,
    current: Option<(FrozenInputs, u8, Arc<Mutex<AdmittedRuntime>>)>,
}
impl RequestEngine for Engine {
    type Request = (PathBuf, u8);
    type Error = String;
    type Runtime = AdmittedRuntime;
    type Readiness = Readiness;
    fn select(
        &mut self,
        (path, policy): &Self::Request,
    ) -> Result<SelectedRequest<AdmittedRuntime, Readiness>, String> {
        let (bytes, inputs) = observe_inputs(
            InputLimits {
                bytes: 65536,
                operations: 100,
            },
            2,
            |c| c.read(path),
        )
        .map_err(|_| "input race")?;
        let bytes = bytes.map_err(|e| e.to_string())?;
        let same = self
            .current
            .as_ref()
            .is_some_and(|(old, p, _)| old == &inputs && p == policy);
        if !same {
            let mut parts = inputs.identity_parts();
            parts.push(vec![*policy]);
            let action = Action(ContentHash::of_parts(
                "hirsel.resident.fixture.action.v1",
                parts.iter().map(Vec::as_slice),
            ));
            let product = crate::fixture_product(
                &*self.store,
                root().address,
                action,
                self.store
                    .put_content(
                        Domain {
                            kind: 123,
                            schema: 1,
                        },
                        &bytes,
                    )
                    .unwrap(),
            );
            let reservation = self.budget.reserve(ALLOWANCE).map_err(|e| e.to_string())?;
            let mut runtime = AdmittedRuntime::new(Jobs::new(1), reservation, errors())
                .map_err(|e| e.to_string())?;
            runtime
                .register(
                    root().kind,
                    Arc::new(Factory {
                        product,
                        polls: self.polls.clone(),
                    }),
                )
                .unwrap();
            self.current = Some((inputs, *policy, Arc::new(Mutex::new(runtime))));
        }
        Ok(SelectedRequest {
            runtime: self.current.as_ref().unwrap().2.clone(),
            roots: vec![root()],
            output: Readiness {
                store: self.store.clone(),
            },
        })
    }
}
fn engine(
    budget: Arc<ProcessBudget>,
    store: Arc<dyn ResultStore>,
    polls: Arc<AtomicUsize>,
) -> Engine {
    Engine {
        budget,
        store,
        polls,
        current: None,
    }
}
fn complete<
    R: hirsel_engine::runtime::QueryRuntime,
    O: OutputReadiness<Output = Vec<u8>, Error = String>,
>(
    request: &mut hirsel_driver::resident::RequestLease<R, O>,
) -> Result<Vec<u8>, RequestError<String>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match request.poll() {
            Poll::Ready(result) => return result,
            Poll::Pending => {
                assert!(Instant::now() < deadline, "runtime request stalled");
                std::thread::yield_now();
            }
        }
    }
}
#[test]
fn actual_runtime_coalesces_sixteen_identical_requests_and_cancel_preserves_live_consumers() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"snapshot").unwrap();
    let budget = ProcessBudget::new(2 * ALLOWANCE);
    let store = Arc::new(MemStore::default());
    let polls = Arc::new(AtomicUsize::new(0));
    let mut host = Host::new(engine(budget.clone(), store, polls.clone()));
    let mut requests = (0..16)
        .map(|_| host.request(&(source.clone(), 0)).unwrap())
        .collect::<Vec<_>>();
    drop(requests.remove(0));
    for request in &mut requests {
        assert_eq!(complete(request).unwrap(), b"snapshot");
    }
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    drop(requests);
    drop(host);
    assert_eq!(budget.reserved(), 0);
}
#[test]
fn actual_runtime_keeps_old_revision_and_refuses_cap_multiplication() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"before").unwrap();
    let budget = ProcessBudget::new(2 * ALLOWANCE);
    let store = Arc::new(MemStore::default());
    let polls = Arc::new(AtomicUsize::new(0));
    let mut host = Host::new(engine(budget.clone(), store, polls));
    let mut before = host.request(&(source.clone(), 0)).unwrap();
    std::fs::write(&source, b"after!").unwrap();
    let mut after = host.request(&(source.clone(), 0)).unwrap();
    assert_eq!(budget.reserved(), 2 * ALLOWANCE);
    std::fs::write(&source, b"third!").unwrap();
    assert!(host.request(&(source.clone(), 0)).is_err());
    assert_eq!(complete(&mut before).unwrap(), b"before");
    assert_eq!(complete(&mut after).unwrap(), b"after!");
    drop(before);
    drop(after);
    drop(host);
    assert_eq!(budget.reserved(), 0);
}
#[test]
fn actual_runtime_policy_change_restart_and_zero_retention_use_ordinary_products() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"payload").unwrap();
    let budget = ProcessBudget::new(2 * ALLOWANCE);
    let store = Arc::new(MemStore::default());
    let polls = Arc::new(AtomicUsize::new(0));
    let mut host = Host::new(engine(budget.clone(), store.clone(), polls.clone()));
    let mut request = host.request(&(source.clone(), 0)).unwrap();
    assert_eq!(complete(&mut request).unwrap(), b"payload");
    request.evict_to(0);
    drop(request);
    let mut request = host.request(&(source.clone(), 1)).unwrap();
    assert_eq!(complete(&mut request).unwrap(), b"payload");
    drop(request);
    drop(host);
    assert_eq!(budget.reserved(), 0);
    let mut fresh = Host::new(engine(budget.clone(), store, polls.clone()));
    let mut request = fresh.request(&(source, 1)).unwrap();
    assert_eq!(complete(&mut request).unwrap(), b"payload");
    assert_eq!(polls.load(Ordering::SeqCst), 3);
    drop(request);
    drop(fresh);
    assert_eq!(budget.reserved(), 0);
}

#[test]
fn blocking_completion_preserves_coalescing_and_checks_ordinary_output() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"shared output").unwrap();
    let budget = ProcessBudget::new(2 * ALLOWANCE);
    let store = Arc::new(MemStore::default());
    let polls = Arc::new(AtomicUsize::new(0));
    let mut host = Host::new(engine(budget.clone(), store, polls.clone()));
    let first = host.request(&(source.clone(), 0)).unwrap();
    let second = host.request(&(source, 0)).unwrap();
    assert_eq!(first.wait().unwrap(), b"shared output");
    assert_eq!(second.wait().unwrap(), b"shared output");
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    drop(host);
    assert_eq!(budget.reserved(), 0);
}

#[test]
fn blocking_completion_after_host_shutdown_reports_cancelled() {
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"cancelled output").unwrap();
    let budget = ProcessBudget::new(2 * ALLOWANCE);
    let store = Arc::new(MemStore::default());
    let mut host = Host::new(engine(budget.clone(), store, Arc::new(AtomicUsize::new(0))));
    let request = host.request(&(source, 0)).unwrap();
    drop(host);
    assert_eq!(request.wait(), Err(RequestError::Cancelled));
    assert_eq!(budget.reserved(), 0);
}

/// Test-only immutable selector: reads the descriptor, never mutable disk.
struct FrozenEngine {
    store: Arc<dyn ResultStore>,
    budget: Arc<ProcessBudget>,
}
impl RequestEngine for FrozenEngine {
    type Request = hirsel_driver::requests::CompileRequest;
    type Error = String;
    type Runtime = AdmittedRuntime;
    type Readiness = Readiness;
    fn select(
        &mut self,
        request: &Self::Request,
    ) -> Result<SelectedRequest<AdmittedRuntime, Readiness>, String> {
        let bytes = request
            .inputs
            .read(&request.path)
            .map_err(|e| e.to_string())?;
        let parts = request.inputs.identity_parts();
        let product = crate::fixture_product(
            &*self.store,
            root().address,
            Action(ContentHash::of_parts(
                "hirsel.resident.explicit-input-fixture.v1",
                parts.iter().map(Vec::as_slice),
            )),
            self.store
                .put_content(
                    Domain {
                        kind: 123,
                        schema: 1,
                    },
                    &bytes,
                )
                .unwrap(),
        );
        let mut runtime = AdmittedRuntime::new(
            Jobs::new(1),
            self.budget.reserve(ALLOWANCE).map_err(|e| e.to_string())?,
            errors(),
        )
        .map_err(|e| e.to_string())?;
        runtime
            .register(
                root().kind,
                Arc::new(Factory {
                    product,
                    polls: Arc::new(AtomicUsize::new(0)),
                }),
            )
            .unwrap();
        Ok(SelectedRequest {
            runtime: Arc::new(Mutex::new(runtime)),
            roots: vec![root()],
            output: Readiness {
                store: self.store.clone(),
            },
        })
    }
}

fn check_operation() -> hirsel_driver::requests::CompileOperation {
    hirsel_driver::requests::CompileOperation::Check {
        lowering: hirsel_engine::LoweringPolicy::Comptime,
    }
}

#[test]
fn compilation_session_delivers_explicit_bytes_without_mutating_live_requests_or_reading_disk() {
    use hirsel_driver::resident::CompilationSession;
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    std::fs::write(&source, b"before").unwrap();
    let (_, inputs) = observe_inputs(
        InputLimits {
            bytes: 65536,
            operations: 100,
        },
        1,
        |capture| capture.read(&source),
    )
    .unwrap();
    let budget = ProcessBudget::new(2 * ALLOWANCE);
    let mut session = CompilationSession::new(
        FrozenEngine {
            store: Arc::new(MemStore::default()),
            budget: budget.clone(),
        },
        source.clone(),
        inputs,
    );
    let before = session.request(check_operation()).unwrap();
    std::fs::write(&source, b"newer disk state").unwrap();
    let edits = [(source.clone(), Arc::<[u8]>::from(b"delivered".as_slice()))]
        .into_iter()
        .collect();
    assert_eq!(session.apply_sources(&edits, 65536).unwrap(), 1);
    assert_eq!(session.apply_sources(&edits, 65536).unwrap(), 1);
    let after = session.request(check_operation()).unwrap();
    assert_eq!(before.wait().unwrap(), b"before");
    assert_eq!(after.wait().unwrap(), b"delivered");
    assert_eq!(session.revision(), 1);
    let accepted = session.inputs().clone();
    assert!(session.apply_sources(&edits, 1).is_err());
    let unknown = [(
        tree.path().join("unknown.hir"),
        Arc::<[u8]>::from(b"unknown".as_slice()),
    )]
    .into_iter()
    .collect();
    assert!(session.apply_sources(&unknown, 65536).is_err());
    assert_eq!(session.inputs(), &accepted);
    assert_eq!(session.revision(), 1);
    drop(session);
    assert_eq!(budget.reserved(), 0);
}

#[test]
fn compilation_session_replaces_complete_observations_and_keeps_missing_inputs_watched() {
    use hirsel_driver::resident::CompilationSession;
    let tree = tempfile::tempdir().unwrap();
    let source = tree.path().join("main.hir");
    let alias = tree.path().join("alias.hir");
    let sidecar = tree.path().join("main.hir.json");
    std::fs::write(&source, b"original").unwrap();
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    let limits = InputLimits {
        bytes: 65536,
        operations: 100,
    };
    let (_, inputs) = observe_inputs(limits, 1, |capture| -> std::io::Result<()> {
        capture.read(&alias)?;
        capture.read(&source)?;
        capture.canonicalize(&alias)?;
        assert_eq!(
            capture.read(&sidecar).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        Ok(())
    })
    .unwrap();
    let budget = ProcessBudget::new(2 * ALLOWANCE);
    let mut session = CompilationSession::new(
        FrozenEngine {
            store: Arc::new(MemStore::default()),
            budget: budget.clone(),
        },
        alias.clone(),
        inputs,
    );
    assert_eq!(session.entry(), alias);
    let watched = session
        .watched()
        .map(std::path::Path::to_path_buf)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        watched,
        [source.clone(), alias.clone(), sidecar.clone()]
            .into_iter()
            .collect()
    );
    let old = session.request(check_operation()).unwrap();
    std::fs::remove_file(&source).unwrap();
    let (missing, inputs) = observe_inputs(limits, 1, |capture| capture.read(&alias)).unwrap();
    assert_eq!(missing.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    assert_eq!(session.replace_inputs(inputs), 1);
    assert!(session.request(check_operation()).is_err());
    assert_eq!(old.wait().unwrap(), b"original");
    assert!(session.watched().any(|path| path == alias));
    drop(session);
    assert_eq!(budget.reserved(), 0);
}
