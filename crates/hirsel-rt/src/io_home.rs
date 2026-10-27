//! Home-affine adapters behind a run-owned facade. The scheduler carries one
//! run token; the facade's paths and wait table migrate with it, while every
//! adapter method is served on admission. No worker owns or destroys the adapter.
//! This also covers callbacks that cannot migrate their enclosing C frames.
use crate::context::RunShared;
use crate::io::*;

thread_local! {
    static POLLING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A saved request's poll owns its own evidence, even when width-one home
/// still has the suspended caller's task installed in thread-local storage.
/// Poll callbacks do not suspend; restore attribution before returning to code.
pub(crate) struct Poll {
    previous: bool,
    _local: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl Poll {
    pub(crate) fn new() -> Self {
        Self {
            previous: POLLING.with(|polling| polling.replace(true)),
            _local: std::marker::PhantomData,
        }
    }
}

impl Drop for Poll {
    fn drop(&mut self) {
        POLLING.with(|polling| polling.set(self.previous));
    }
}

pub(crate) struct Snapshot {
    pub(crate) result: Result<Completion, AdapterError>,
    pub(crate) discovery: Option<Result<crate::host_waits::Discovery, &'static str>>,
    pub(crate) output: Result<Option<Vec<u8>>, &'static str>,
}

pub(crate) struct HomeAdapter {
    pub(crate) adapter: *mut dyn Adapter,
    pub(crate) run: *mut RunShared,
}
impl HomeAdapter {
    /// Home only. Ordered serializes the poll owner with every adapter call;
    /// drain its empty points before a method can change wait-log ordinals.
    unsafe fn invoke<T>(&self, f: impl FnOnce(&mut dyn Adapter) -> T) -> T {
        let adapter = unsafe { &mut *self.adapter };
        if !unsafe {
            (*self.run)
                .ordered_mode
                .load(std::sync::atomic::Ordering::Acquire)
        } {
            return f(adapter);
        }
        let hint = unsafe { &(*self.run).timer_hint };
        hint.publish(None);
        let points = hint.take_points();
        if points != 0 {
            adapter.advance_ready_points(points);
        }
        let out = f(adapter);
        hint.publish(adapter.ready_deadline());
        out
    }

    fn observer(&self) -> Option<u32> {
        let ctx = crate::stop::running();
        // Home poll callbacks already have their own source-free observer.
        if ctx.is_null()
            || unsafe { (*self.run).ordered_roots.get().is_none() }
            || POLLING.with(std::cell::Cell::get)
        {
            return None;
        }
        Some(unsafe { (*ctx).task_id })
    }

    fn retain(
        &self,
        task: u32,
        adapter: &dyn Adapter,
        request: &Request,
        result: &Result<Completion, AdapterError>,
        count: Option<u64>,
    ) {
        let declared = adapter.keys(request).is_some();
        let bytes = match request {
            Request::Print { bytes, .. } | Request::PrintStderr { bytes, .. } => {
                Some(bytes.as_slice())
            }
            _ => None,
        };
        let snapshot = Snapshot {
            result: crate::tasks::ordered::events::owned_host_result(result),
            discovery: declared.then(|| crate::host_waits::snapshot(adapter, result)),
            output: crate::tasks::ordered::events::retained_output(bytes, result, count),
        };
        assert!(
            unsafe { (*self.run).ordered_home_results.lock().unwrap() }
                .insert(task, snapshot)
                .is_none()
        );
    }

    fn with<T>(&self, f: impl FnOnce(&mut dyn Adapter) -> T) -> T {
        self.bridge(|| unsafe { self.invoke(f) })
    }

    fn bridge<T>(&self, f: impl FnOnce() -> T) -> T {
        let (mut f, mut out) = (Some(f), None);
        // SAFETY: the home loop alone dereferences the adapter. Its claim/withdraw
        // gate closes before admission drops the adapter. The posting fiber waits
        // until home finishes (or withdraws) the call, keeping its buffers live.
        let result = unsafe {
            let ctx = crate::stop::running();
            // On home, call_home executes directly. Off home, `ctx` is this
            // fiber's context and remains valid across the synchronous bridge.
            if ctx.is_null() {
                out = f.take().map(|f| f());
                Ok(())
            } else {
                crate::pool::call_home_in(self.run, &mut || {
                    out = f.take().map(|f| f());
                })
            }
        };
        // Release owned request/completion buffers before abandoning a stopped fiber.
        if let Err(stop) = result {
            drop(out);
            drop(f);
            unsafe { crate::stop::raise(crate::stop::running(), stop) };
        }
        out.unwrap_or_else(|| crate::fault("a home adapter call returned no value"))
    }
}

impl Adapter for HomeAdapter {
    fn batches_issue(&self) -> bool {
        crate::pool::on_pool() || crate::pool::compute_index().is_some()
    }

    fn issue_batch(
        &mut self,
        request: Request,
        token: u64,
        issuer: &[u32],
    ) -> Result<Completion, AdapterError> {
        let task = self.observer();
        let observed = task.map(|_| request.clone());
        self.bridge(|| unsafe {
            // Keep each existing invocation boundary, including deferred
            // timer-point draining and hint publication, within one loan.
            if self.invoke(|adapter| adapter.wants_issuer()) {
                self.invoke(|adapter| adapter.issuer(issuer));
            }
            self.invoke(|adapter| adapter.enter(token));
            self.invoke(|adapter| {
                let result = adapter.call(request);
                if let Some(task) = task {
                    self.retain(task, adapter, observed.as_ref().unwrap(), &result, None);
                }
                result
            })
        })
    }

    fn scope_timer(&mut self, request: Request) -> Result<Completion, AdapterError> {
        // Scope timers are observed by deadline_armed/expire, rather than by
        // the issuing task's ordinary host request/result transfer slot.
        self.with(|adapter| adapter.scope_timer(request))
    }

    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        self.with(|adapter| adapter.preflight(request))
    }

    fn tls(&mut self, _engine: &'static dyn crate::net_tls::TlsEngine) {
        self.with(|adapter| adapter.tls(_engine))
    }

    fn net(&mut self, _host: crate::net_host::Provider) {
        self.with(|adapter| adapter.net(_host))
    }

    fn process(&mut self, _host: fn() -> Box<dyn crate::process::ProcessHost>) {
        self.with(|adapter| adapter.process(_host))
    }

    fn tls_roots(&mut self, _stream: u64, _pem: &[u8]) {
        self.with(|adapter| adapter.tls_roots(_stream, _pem))
    }

    fn tls_identity(&mut self, _stream: u64, _chain: &[u8], _key: &[u8]) {
        self.with(|adapter| adapter.tls_identity(_stream, _chain, _key))
    }

    fn tls_server(
        &mut self,
        _stream: u64,
        _identities: &[(&[u8], &[u8])],
        _clients: crate::net::ClientAuth<&[u8]>,
        _alpn: &[String],
    ) {
        self.with(|adapter| adapter.tls_server(_stream, _identities, _clients, _alpn))
    }

    fn replay_stdout(&self) -> Vec<u8> {
        self.with(|adapter| adapter.replay_stdout())
    }

    fn replay_stderr(&self) -> Vec<u8> {
        self.with(|adapter| adapter.replay_stderr())
    }

    fn keys(&self, request: &Request) -> Option<crate::io_keys::KeySet> {
        self.with(|adapter| adapter.keys(request))
    }
    fn domains(&self, request: &Request) -> Vec<(crate::io_keys::Key, u64, String)> {
        self.with(|adapter| adapter.domains(request))
    }
    fn identity(&self, handle: u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)> {
        self.with(|adapter| adapter.identity(handle))
    }
    fn print_observed(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        let task = self.observer();
        self.with(|adapter| {
            let result = adapter.print_observed(handle, stderr, bytes);
            if let Some(task) = task {
                let request = if stderr {
                    Request::PrintStderr {
                        stderr: handle,
                        bytes: bytes.to_vec(),
                    }
                } else {
                    Request::Print {
                        stdout: handle,
                        bytes: bytes.to_vec(),
                    }
                };
                self.retain(task, adapter, &request, &result.0, result.1);
            }
            result
        })
    }
    fn print(
        &mut self,
        handle: u64,
        stderr: bool,
        bytes: &[u8],
    ) -> Result<Completion, AdapterError> {
        self.with(|adapter| adapter.print(handle, stderr, bytes))
    }

    fn net_io(
        &mut self,
        _token: u64,
        _net: u64,
        _stream: u64,
        _inline: bool,
        _buffer: &mut NetBuffer<'_>,
    ) -> Option<Result<NetResult, AdapterError>> {
        self.with(|adapter| adapter.net_io(_token, _net, _stream, _inline, _buffer))
    }

    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        let this = &*self;
        let task = self.observer();
        let observed = task.map(|_| request.clone());
        this.with(move |adapter| {
            let result = adapter.call(request);
            if let Some(task) = task {
                this.retain(task, adapter, observed.as_ref().unwrap(), &result, None);
            }
            result
        })
    }

    fn wants_issuer(&self) -> bool {
        self.with(|adapter| adapter.wants_issuer())
    }

    fn issuer(&mut self, _task: &[u32]) {
        self.with(|adapter| adapter.issuer(_task))
    }

    fn enter(&mut self, _token: u64) {
        self.with(|adapter| adapter.enter(_token))
    }

    fn finish(&mut self, _token: u64) {
        self.with(|adapter| adapter.finish(_token))
    }

    fn parked(&mut self, _token: u64) {
        self.with(|adapter| adapter.parked(_token))
    }

    fn cancellable(&self, _token: u64) -> bool {
        self.with(|adapter| adapter.cancellable(_token))
    }

    fn cpu_control(
        &mut self,
        width: u32,
        action: u32,
        count: u64,
        value: Option<u32>,
    ) -> Result<Option<u32>, AdapterError> {
        self.with(|adapter| adapter.cpu_control(width, action, count, value))
    }
    fn schedule(&mut self, _count: u64, choice: u64) -> Result<u64, AdapterError> {
        self.with(|adapter| adapter.schedule(_count, choice))
    }

    fn injected_wait(&self, _request: &Request) -> bool {
        self.with(|adapter| adapter.injected_wait(_request))
    }

    fn telemetry_context(&self, _telemetry: u64, _span: u64) -> Option<Vec<u8>> {
        self.with(|adapter| adapter.telemetry_context(_telemetry, _span))
    }

    fn isolated_clock(&self) -> bool {
        self.with(|adapter| adapter.isolated_clock())
    }
    fn process_site(&mut self, child: u64, site: u64, operation_site: u64, path: &[u32]) {
        self.with(|adapter| adapter.process_site(child, site, operation_site, path))
    }
    fn process_sources(&self) -> bool {
        self.with(|adapter| adapter.process_sources())
    }
    fn process_batch(&mut self, canceled: &[u64]) -> Result<Ready, AdapterError> {
        self.with(|adapter| adapter.process_batch(canceled))
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        self.with(|adapter| adapter.ready())
    }

    fn has_ready(&mut self) -> Result<bool, AdapterError> {
        self.with(|adapter| adapter.has_ready())
    }

    fn runner_readiness(&mut self) -> Option<crate::readiness::Readiness> {
        // Local runners execute on admission at width 1. Readiness contains
        // Rc state and must never be returned to a compute or pool thread.
        if crate::pool::on_pool() || crate::pool::compute_index().is_some() {
            return None;
        }
        self.with(|adapter| adapter.runner_readiness())
    }

    fn deliver(&mut self, _token: u64, _completion: hirsel_run_schema::RunnerCompletion) {
        self.with(|adapter| adapter.deliver(_token, _completion))
    }

    fn wait_sources(&mut self) -> Result<Option<WaitSources>, AdapterError> {
        self.with(|adapter| adapter.wait_sources())
    }

    fn wait(&mut self) -> Result<bool, AdapterError> {
        self.with(|adapter| adapter.wait())
    }

    fn scripted_input(&self) -> bool {
        self.with(|adapter| adapter.scripted_input())
    }

    fn wake_on(&mut self, _fd: std::os::fd::RawFd) -> bool {
        self.with(|adapter| adapter.wake_on(_fd))
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Host {
        home: std::thread::ThreadId,
        events: Arc<Mutex<Vec<String>>>,
    }
    impl Host {
        fn event(&self, event: String) {
            assert_eq!(std::thread::current().id(), self.home);
            self.events.lock().unwrap().push(event);
        }
    }
    impl Adapter for Host {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn wants_issuer(&self) -> bool {
            self.event("wants".into());
            true
        }
        fn issuer(&mut self, path: &[u32]) {
            self.event(format!("issuer:{path:?}"));
        }
        fn enter(&mut self, token: u64) {
            self.event(format!("enter:{token}"));
        }
        fn finish(&mut self, token: u64) {
            self.event(format!("finish:{token}"));
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            assert!(matches!(
                request,
                Request::SleepUntil {
                    clock: 99,
                    deadline: 100
                }
            ));
            self.event("call".into());
            Ok(Completion::Wait(Ok(())))
        }
    }

    #[test]
    fn batched_metadata_and_effect_keep_the_original_order_on_home_through_shared() {
        for width in [1, 2, 4] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let host = crate::launch::Shared(std::rc::Rc::new(std::cell::RefCell::new(Host {
                home: std::thread::current().id(),
                events: events.clone(),
            })));
            let mut run = crate::host::BodyRun::new(width, false);
            run.adapter = Some(Box::new(host));
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                |_| {},
                |ctx| unsafe {
                    let binding = Binding::get(ctx);
                    binding
                        .adapter
                        .issue_batch(
                            Request::SleepUntil {
                                clock: 99,
                                deadline: 100,
                            },
                            7,
                            &[1, 2],
                        )
                        .unwrap();
                    binding.adapter.finish(7);
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert_eq!(
                *events.lock().unwrap(),
                ["wants", "issuer:[1, 2]", "enter:7", "call", "finish:7"]
            );
        }
    }
}
