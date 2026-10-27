//! Native simulated-place crash boundary. Delegation follows launch::Shared;
//! crash request counting follows driver/network/runners.rs::crash_point and
//! network.rs::host_cancelable. A blocked attempt does not advance the index.
//! Adapter defaults route print and borrowed I/O through this owned call path.
use super::*;
use crate::io::{Adapter, Ready};

pub(super) struct CalleeIo {
    io: Rc<RefCell<RealIo>>,
    crash_after: u32,
    requests: u32,
    stopped: bool,
}
impl CalleeIo {
    pub(super) fn new(io: Rc<RefCell<RealIo>>, crash_after: u32) -> Self {
        Self {
            io,
            crash_after,
            requests: 0,
            stopped: false,
        }
    }
}

impl Adapter for CalleeIo {
    fn keys(&self, request: &Request) -> Option<crate::io_keys::KeySet> {
        self.io.borrow().keys(request)
    }
    fn domains(&self, request: &Request) -> Vec<(crate::io_keys::Key, u64, String)> {
        self.io.borrow().domains(request)
    }
    fn identity(&self, handle: u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)> {
        self.io.borrow().identity(handle)
    }
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        self.io.borrow().preflight(request)
    }
    fn tls(&mut self, engine: &'static dyn crate::net_tls::TlsEngine) {
        self.io.borrow_mut().tls(engine);
    }
    fn net(&mut self, host: crate::net_host::Provider) {
        self.io.borrow_mut().net(host);
    }
    fn process(&mut self, host: fn() -> Box<dyn crate::process::ProcessHost>) {
        self.io.borrow_mut().process(host);
    }
    fn tls_roots(&mut self, stream: u64, pem: &[u8]) {
        self.io.borrow_mut().tls_roots(stream, pem);
    }
    fn tls_identity(&mut self, stream: u64, chain: &[u8], key: &[u8]) {
        self.io.borrow_mut().tls_identity(stream, chain, key);
    }
    fn tls_server(
        &mut self,
        stream: u64,
        identities: &[(&[u8], &[u8])],
        clients: crate::net::ClientAuth<&[u8]>,
        alpn: &[String],
    ) {
        self.io
            .borrow_mut()
            .tls_server(stream, identities, clients, alpn);
    }
    fn replay_stderr(&self) -> Vec<u8> {
        self.io.borrow().replay_stderr()
    }
    fn replay_stdout(&self) -> Vec<u8> {
        self.io.borrow().replay_stdout()
    }

    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        if self.stopped {
            return Err(AdapterError::Stopped);
        }
        if self.requests == self.crash_after {
            // Same boundary as HostLibrary::crash_point: before the selected
            // provider request, publish Stopped and perform no part of it.
            self.io.borrow_mut().call(Request::Stopped)?;
            self.stopped = true;
            return Err(AdapterError::Stopped);
        }
        let result = self.io.borrow_mut().call(request);
        if !matches!(result, Err(AdapterError::Blocked)) {
            self.requests = self.requests.saturating_add(1);
        }
        result
    }
    fn wants_issuer(&self) -> bool {
        self.io.borrow().wants_issuer()
    }
    fn issuer(&mut self, task: &[u32]) {
        self.io.borrow_mut().issuer(task);
    }
    fn enter(&mut self, token: u64) {
        self.io.borrow_mut().enter(token);
    }
    fn finish(&mut self, token: u64) {
        self.io.borrow_mut().finish(token);
    }
    fn cancellable(&self, token: u64) -> bool {
        self.io.borrow().cancellable(token)
    }
    fn cpu_control(
        &mut self,
        width: u32,
        action: u32,
        count: u64,
        value: Option<u32>,
    ) -> Result<Option<u32>, AdapterError> {
        self.io
            .borrow_mut()
            .cpu_control(width, action, count, value)
    }
    fn schedule(&mut self, count: u64, choice: u64) -> Result<u64, AdapterError> {
        self.io.borrow_mut().schedule(count, choice)
    }
    fn parked(&mut self, token: u64) {
        self.io.borrow_mut().parked(token);
    }
    fn injected_wait(&self, request: &Request) -> bool {
        self.io.borrow().injected_wait(request)
    }
    fn telemetry_context(&self, telemetry: u64, span: u64) -> Option<Vec<u8>> {
        self.io.borrow().telemetry_context(telemetry, span)
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        self.io.borrow_mut().ready()
    }
    fn ready_deadline(&self) -> Option<std::time::Instant> {
        self.io.borrow().ready_deadline()
    }
    fn advance_ready_points(&mut self, points: u64) {
        self.io.borrow_mut().advance_ready_points(points);
    }
    fn has_ready(&mut self) -> Result<bool, AdapterError> {
        self.io.borrow_mut().has_ready()
    }
    fn runner_readiness(&mut self) -> Option<crate::readiness::Readiness> {
        self.io.borrow_mut().runner_readiness()
    }
    fn deliver(&mut self, token: u64, completion: hirsel_run_schema::RunnerCompletion) {
        self.io.borrow_mut().deliver(token, completion);
    }
    fn wait_sources(&mut self) -> Result<Option<crate::io::WaitSources>, AdapterError> {
        self.io.borrow_mut().wait_sources()
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        self.io.borrow_mut().wait()
    }
    fn scripted_input(&self) -> bool {
        self.io.borrow().scripted_input()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_attempts_keep_crash_index_and_selected_effect_never_runs() {
        let mut real = RealIo::grant(&std::env::temp_dir()).unwrap();
        real.virtual_clock(0);
        let Completion::ClockGrant(Ok(clock)) =
            real.call(Request::MonotonicClock { io: 1 }).unwrap()
        else {
            panic!("clock grant")
        };
        let Completion::Handle(Ok(stdout)) = real.call(Request::Stdout { io: 1 }).unwrap() else {
            panic!("stdout grant")
        };
        let real = Rc::new(RefCell::new(real));
        let mut io = CalleeIo::new(real.clone(), 1);
        let sleep = Request::SleepUntil {
            clock,
            deadline: 10,
        };
        io.enter(7);
        for _ in 0..2 {
            assert_eq!(io.call(sleep.clone()), Err(AdapterError::Blocked));
            assert_eq!(io.requests, 0);
            io.parked(7);
        }
        assert!(io.wait().unwrap());
        assert_eq!(io.call(sleep), Ok(Completion::Wait(Ok(()))));
        io.finish(7);
        assert_eq!(io.requests, 1);
        // Borrowed output must traverse the same boundary as owned requests.
        for _ in 0..2 {
            assert_eq!(
                io.print(stdout, false, b"must not escape"),
                Err(AdapterError::Stopped)
            );
        }
        let real = real.borrow();
        assert!(real.stdout().is_empty());
        assert_eq!(
            real.tape()
                .iter()
                .filter(|event| event.request == Request::Stopped)
                .count(),
            1
        );
        assert!(
            !real
                .tape()
                .iter()
                .any(|event| matches!(event.request, Request::Print { .. }))
        );
    }
}
