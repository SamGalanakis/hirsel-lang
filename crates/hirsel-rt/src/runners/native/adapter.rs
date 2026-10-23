//! Retain the existing host provider's identity, keys and bounded attempt
//! machinery. Runner calls add only their home-owned callee handoff.
use super::*;
use crate::io_concurrent::ConcurrentAdapter;
use crate::io_keys::{Key, KeySet, Kind};

struct Adapter(Arc<Transport>);

pub(crate) fn wrap(slot: &Arc<Slot>) -> Arc<dyn ConcurrentAdapter> {
    Arc::new(Adapter(slot.transport.clone()))
}

impl ConcurrentAdapter for Adapter {
    fn namespace_coupled(&self) -> bool {
        self.0.io.namespace_coupled()
    }
    fn keys(&self, r: &Request) -> KeySet {
        self.0.io.keys(r)
    }
    fn domains(&self, r: &Request) -> Vec<(Key, u64, String)> {
        self.0.io.domains(r)
    }
    fn identity(&self, h: u64) -> Option<(Key, Kind)> {
        self.0.io.identity(h)
    }
    fn process_keys(&self, h: u64) -> Option<KeySet> {
        self.0.io.process_keys(h)
    }
    fn net_ports(&self, h: u64) -> Option<Key> {
        self.0.io.net_ports(h)
    }
    fn tls(&self, engine: &'static dyn crate::net_tls::TlsEngine) {
        self.0.io.tls(engine)
    }
    fn tls_roots(&self, h: u64, pem: &[u8]) {
        self.0.io.tls_roots(h, pem)
    }
    fn tls_identity(&self, h: u64, chain: &[u8], key: &[u8]) {
        self.0.io.tls_identity(h, chain, key)
    }
    fn tls_server(
        &self,
        h: u64,
        ids: &[(&[u8], &[u8])],
        clients: crate::net::ClientAuth<&[u8]>,
        alpn: &[String],
    ) {
        self.0.io.tls_server(h, ids, clients, alpn)
    }
    fn preflight(&self, r: &Request) -> Result<Option<Completion>, AdapterError> {
        self.0.io.preflight(r)
    }
    fn call(&self, r: Request) -> Result<Completion, AdapterError> {
        self.call_at(0, r)
    }
    fn call_at(&self, token: u64, r: Request) -> Result<Completion, AdapterError> {
        match &r {
            Request::RunnerCall(request) => self.0.issue(token, request.key),
            Request::RunnerPoll { key, cancel } => self.0.poll(*key, *cancel),
            _ => self.0.io.call_at(token, r),
        }
    }
    fn call_observed_at(
        &self,
        token: u64,
        r: Request,
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        if matches!(r, Request::RunnerCall(_) | Request::RunnerPoll { .. }) {
            (self.call_at(token, r), None)
        } else {
            self.0.io.call_observed_at(token, r)
        }
    }
    fn finish(&self, token: u64) {
        self.0.io.finish(token)
    }
    fn parked(&self, token: u64) {
        self.0.io.parked(token)
    }
    fn cancellable(&self, token: u64) -> bool {
        self.0.io.cancellable(token)
    }
    fn ready(&self) -> Result<crate::io::Ready, AdapterError> {
        self.0.io.ready()
    }
    fn readiness(&self) -> Option<crate::readiness::Readiness> {
        self.0.io.readiness()
    }
    fn wait_sources(&self) -> Result<crate::io::WaitSources, AdapterError> {
        match self.0.io.wait_sources()? {
            crate::io::WaitSources::Ready => Ok(crate::io::WaitSources::Ready),
            crate::io::WaitSources::Drivers(mut drivers) => {
                drivers.extend(self.0.sources());
                Ok(crate::io::WaitSources::Drivers(drivers))
            }
        }
    }
}
