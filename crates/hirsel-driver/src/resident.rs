//! Request lifetimes over the shared query runtime. A fresh host uses exactly
//! the same selection and completion path. Hot products, mappings, admission,
//! dependency validation and deterministic transactions belong to the engine.
use hirsel_engine::requests::{OutputReadiness, RequestEngine, SelectedRequest};
use hirsel_engine::runtime::{QueryRuntime, RequestId};
use std::sync::{Arc, Mutex, Weak};
use std::task::Poll;

mod compilation_session;
pub use compilation_session::CompilationSession;
mod observed_session;
pub use observed_session::ObservedSession;

/// Transport-owned host. Keeping this alive retains the engine's shared handles,
/// without retaining a second program, verdict, output or invalidation cache.
pub struct Host<E: RequestEngine> {
    engine: E,
    /// Transport subscriptions only. Weak ownership cannot pin semantic products.
    active: Vec<Weak<Subscription<E::Runtime>>>,
}

impl<E: RequestEngine> Host<E> {
    pub fn new(engine: E) -> Self {
        Self {
            engine,
            active: Vec::new(),
        }
    }

    pub fn request(
        &mut self,
        request: &E::Request,
    ) -> Result<RequestLease<E::Runtime, E::Readiness>, E::Error> {
        let SelectedRequest {
            runtime,
            roots,
            output,
        } = self.engine.select(request)?;
        let id = runtime
            .lock()
            .expect("query runtime poisoned")
            .request(&roots);
        let subscription = Arc::new(Subscription {
            runtime,
            active: Mutex::new(Some(id)),
        });
        self.active
            .retain(|weak| weak.upgrade().is_some_and(|s| s.is_active()));
        self.active.push(Arc::downgrade(&subscription));
        Ok(RequestLease {
            subscription,
            output,
            completed: false,
        })
    }
}

impl<E: RequestEngine> Drop for Host<E> {
    fn drop(&mut self) {
        for subscription in self.active.iter().filter_map(Weak::upgrade) {
            subscription.cancel();
        }
    }
}

struct Subscription<R: QueryRuntime> {
    runtime: Arc<Mutex<R>>,
    active: Mutex<Option<RequestId>>,
}
impl<R: QueryRuntime> Subscription<R> {
    fn is_active(&self) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
    fn cancel(&self) {
        if let Some(id) = self.active.lock().unwrap_or_else(|e| e.into_inner()).take() {
            self.runtime
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .cancel(id);
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum RequestError<E> {
    /// The owning transport host shut down before query completion.
    Cancelled,
    /// Runtime-owned immutable diagnostics. A failed query never runs readiness.
    Query(hirsel_engine::store::Reference),
    Output(E),
}

/// A queued/live consumer owns this lease until output readiness or cancellation.
/// Dropping it cancels only this request; the shared runtime owns coalesced work.
#[must_use = "dropping a request lease cancels the request"]
pub struct RequestLease<R: QueryRuntime, O> {
    subscription: Arc<Subscription<R>>,
    output: O,
    completed: bool,
}

impl<R: QueryRuntime, O: OutputReadiness> RequestLease<R, O> {
    /// The transport drives this when work can advance. After Ready the lease
    /// is terminal, like a completed Future; polling it again is a caller error.
    pub fn poll(&mut self) -> Poll<Result<O::Output, RequestError<O::Error>>> {
        assert!(!self.completed, "request already completed");
        let mut active = self
            .subscription
            .active
            .lock()
            .expect("request subscription poisoned");
        let Some(id) = *active else {
            self.completed = true;
            return Poll::Ready(Err(RequestError::Cancelled));
        };
        let result = self
            .subscription
            .runtime
            .lock()
            .expect("query runtime poisoned")
            .poll(id);
        match result {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                active.take();
                drop(active);
                self.completed = true;
                Poll::Ready(match result {
                    Ok(products) => self.output.ready(&products).map_err(RequestError::Output),
                    Err(diagnostics) => Err(match self.output.failed(diagnostics) {
                        Some(error) => RequestError::Output(error),
                        None => RequestError::Query(diagnostics),
                    }),
                })
            }
        }
    }

    /// Forward the host's explicit allowance to the one retention authority.
    /// Live products/mappings remain owned by the runtime and its consumers.
    pub fn evict_to(&mut self, retained_bytes: u64) {
        self.subscription
            .runtime
            .lock()
            .expect("query runtime poisoned")
            .evict_to(retained_bytes);
    }
}

impl<R: QueryRuntime, O> Drop for RequestLease<R, O> {
    fn drop(&mut self) {
        self.subscription.cancel();
    }
}

impl<O: OutputReadiness> RequestLease<hirsel_engine::runtime::AdmittedRuntime, O> {
    /// Complete a synchronous host operation while leaving request admission,
    /// cancellation and shutdown available to other users of this runtime.
    /// The engine's completion latch never consumes the terminal product;
    /// ordinary polling still owns diagnostics and output readiness.
    pub fn wait(mut self) -> Result<O::Output, RequestError<O::Error>> {
        let id = *self
            .subscription
            .active
            .lock()
            .expect("request subscription poisoned");
        if let Some(id) = id {
            let completion = self
                .subscription
                .runtime
                .lock()
                .expect("query runtime poisoned")
                .completion();
            completion.wait(id);
        }
        match self.poll() {
            Poll::Ready(result) => result,
            Poll::Pending => unreachable!("query completion woke before its request settled"),
        }
    }
}
/// Restore the engine-selected linked output through its configured provider.
/// The descriptor is resolved and output readiness checked on every request;
/// this adapter owns neither an executable cache nor a linker.
pub struct ExecutableOutput {
    pub store: Arc<dyn hirsel_engine::store::ResultStore>,
    pub destination: std::path::PathBuf,
}
impl OutputReadiness for ExecutableOutput {
    type Output = std::path::PathBuf;
    type Error = String;
    fn ready(
        &mut self,
        products: &[hirsel_engine::runtime::ProductRef],
    ) -> Result<Self::Output, String> {
        let [product] = products else {
            return Err("executable request requires exactly one linked root".into());
        };
        let linked =
            crate::native::object_queries::linked_product(self.store.as_ref(), product.clone())?;
        crate::native::object_queries::materialize_executable(
            self.store.as_ref(),
            &linked,
            &self.destination,
        )?;
        Ok(self.destination.clone())
    }
}

#[cfg(test)]
mod compiler_controls;
