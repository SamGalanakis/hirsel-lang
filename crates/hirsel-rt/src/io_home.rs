//! Home-affine adapters behind a run-owned facade. The scheduler carries one
//! run token; the facade's paths and wait table migrate with it, while every
//! adapter method is served on admission. No worker owns or destroys the adapter.
//! This also covers callbacks that cannot migrate their enclosing C frames.
use crate::context::RunShared;
use crate::io::*;

pub(crate) struct HomeAdapter {
    pub(crate) adapter: *mut dyn Adapter,
    pub(crate) run: *mut RunShared,
}
impl HomeAdapter {
    fn with<T>(&self, f: impl FnOnce(&mut dyn Adapter) -> T) -> T {
        let (mut f, mut out) = (Some(f), None);
        // SAFETY: the home loop alone dereferences the adapter. Its claim/withdraw
        // gate closes before admission drops the adapter. The posting fiber waits
        // until home finishes (or withdraws) the call, keeping its buffers live.
        let result = unsafe {
            let ctx = crate::stop::running();
            // On home, call_home executes directly. Off home, `ctx` is this
            // fiber's context and remains valid across the synchronous bridge.
            if ctx.is_null() {
                out = f.take().map(|f| f(&mut *self.adapter));
                Ok(())
            } else {
                crate::pool::call_home_in(self.run, &mut || {
                    out = f.take().map(|f| f(&mut *self.adapter));
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
        self.with(|adapter| adapter.call(request))
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

    fn schedule(&mut self, _count: u64, choice: u64) -> Result<u64, AdapterError> {
        self.with(|adapter| adapter.schedule(_count, choice))
    }

    fn injected_wait(&self, _request: &Request) -> bool {
        self.with(|adapter| adapter.injected_wait(_request))
    }

    fn telemetry_context(&self, _telemetry: u64, _span: u64) -> Option<Vec<u8>> {
        self.with(|adapter| adapter.telemetry_context(_telemetry, _span))
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
