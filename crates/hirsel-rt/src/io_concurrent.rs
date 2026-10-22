//! Io any worker may call directly (design §6 Io, `ConcurrentAdapter`).
//!
//! A [`ConcurrentAdapter`] is `Send + Sync` and lives in the run's shared state, so a task
//! issues its host request on its own worker instead of through the home bridge
//! ([`crate::io_bridge`]). It declares each request's logical [keys](crate::io_keys); [`Issuer`]
//! takes the request's positions (its entered phase), waits until the epochs they depend on
//! have ended, makes the one bounded host call, and ends the positions when the call returns
//! or registers its wait, so no key is held across a wait. Free mode's sections, claim and
//! recording wrap these steps (lane F1); ordered mode issues at retirement in turn order and
//! needs none of it.

use crate::io::{Adapter, AdapterError, Completion, Request};
use crate::io_keys::{KeySet, KeyTable, Positions};
use crate::io_metrics::IoMetrics;
use std::sync::{Arc, Mutex};

/// Io callable from any thread.
pub trait ConcurrentAdapter: Send + Sync {
    /// Admission policy: unknown storage may alias any namespace resource.
    /// This remains fixed for the admitted service's lifetime.
    fn namespace_coupled(&self) -> bool {
        false
    }
    /// The logical keys of `request`'s issue transition. The default couples every request
    /// under one key, as for any adapter that declares none.
    fn keys(&self, _request: &Request) -> KeySet {
        KeySet::coupled()
    }
    /// Auxiliary domains, named by a prior handle's resource and a stable label.
    /// Aliases retain that resource's domain; raw key numbers are never names.
    fn domains(&self, _request: &Request) -> Vec<(crate::io_keys::Key, u64, String)> {
        Vec::new()
    }
    /// Checked identity of an initialized result or initial grant. Unknown is not coupled.
    fn identity(&self, _handle: u64) -> Option<(crate::io_keys::Key, crate::io_keys::Kind)> {
        None
    }
    /// Full process issue domains, including inherited sinks not yet projected
    /// as handles. Only checked process grants may supply this metadata.
    fn process_keys(&self, _process: u64) -> Option<KeySet> {
        None
    }
    /// Checked port allocation domain for this Net world. Aliased/narrowed
    /// grants of one world share it; only actual Net grants may supply it.
    fn net_ports(&self, _net: u64) -> Option<crate::io_keys::Key> {
        None
    }
    fn tls(&self, _engine: &'static dyn crate::net_tls::TlsEngine) {}
    fn tls_roots(&self, _stream: u64, _pem: &[u8]) {}
    fn tls_identity(&self, _stream: u64, _chain: &[u8], _key: &[u8]) {}
    fn tls_server(
        &self,
        _stream: u64,
        _identities: &[(&[u8], &[u8])],
        _clients: crate::net::ClientAuth<&[u8]>,
        _alpn: &[String],
    ) {
    }
    /// Enter a request-owned wait token and perform exactly one bounded attempt.
    fn call_at(&self, _token: u64, request: Request) -> Result<Completion, AdapterError> {
        self.call(request)
    }
    /// One bounded attempt and the exact externally written output prefix, when
    /// available. Non-output calls return None. An unobserved failed print must
    /// not be represented as either zero bytes or the full requested output.
    fn call_observed_at(
        &self,
        token: u64,
        request: Request,
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        (self.call_at(token, request), None)
    }
    fn finish(&self, _token: u64) {}
    fn parked(&self, _token: u64) {}
    fn cancellable(&self, _token: u64) -> bool {
        true
    }
    fn ready(&self) -> Result<crate::io::Ready, AdapterError> {
        Ok(crate::io::Ready::Tokens(Vec::new()))
    }
    fn readiness(&self) -> Option<crate::readiness::Readiness> {
        None
    }
    /// Bounded idle preparation, with every source exposed for a shared wait.
    fn wait_sources(&self) -> Result<crate::io::WaitSources, AdapterError> {
        Ok(crate::io::WaitSources::Drivers(
            self.readiness().into_iter().collect(),
        ))
    }
    /// Check argument validity and authority without an effect or tape event.
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError>;
    /// One bounded host call.
    fn call(&self, request: Request) -> Result<Completion, AdapterError>;
}

/// A `Send` [`Adapter`] behind one lock: every request on the coupled key.
pub struct Coupled<A>(Mutex<A>);

impl<A: Adapter + Send> Coupled<A> {
    pub fn new(adapter: A) -> Coupled<A> {
        Coupled(Mutex::new(adapter))
    }
    pub fn into_inner(self) -> A {
        self.0.into_inner().unwrap()
    }
}

impl<A: Adapter + Send> ConcurrentAdapter for Coupled<A> {
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        let mut adapter = self.0.lock().unwrap();
        adapter.enter(token);
        adapter.call(request)
    }
    fn finish(&self, token: u64) {
        self.0.lock().unwrap().finish(token);
    }
    fn parked(&self, token: u64) {
        self.0.lock().unwrap().parked(token);
    }
    fn cancellable(&self, token: u64) -> bool {
        self.0.lock().unwrap().cancellable(token)
    }
    fn ready(&self) -> Result<crate::io::Ready, AdapterError> {
        self.0.lock().unwrap().ready()
    }
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        self.0.lock().unwrap().preflight(request)
    }
    fn call(&self, request: Request) -> Result<Completion, AdapterError> {
        self.0.lock().unwrap().call(request)
    }
}

/// Issues requests on a [`ConcurrentAdapter`] in key order.
pub struct Issuer {
    adapter: Arc<dyn ConcurrentAdapter>,
    keys: KeyTable,
    metrics: Option<Arc<IoMetrics>>,
}

impl Issuer {
    pub fn new(adapter: Arc<dyn ConcurrentAdapter>, metrics: Option<Arc<IoMetrics>>) -> Issuer {
        Issuer {
            adapter,
            keys: KeyTable::with_metrics(metrics.clone()),
            metrics,
        }
    }
    /// The entered phase: `request`'s positions, taken in key order.
    pub fn enter(&self, request: &Request) -> Positions {
        self.keys.take(&self.adapter.keys(request))
    }
    /// Wait for `positions`' predecessors, make the call, end the positions.
    pub fn issue(
        &self,
        positions: Positions,
        request: Request,
    ) -> Result<Completion, AdapterError> {
        self.keys.wait(&positions);
        let result = {
            let _in_flight = self.metrics.as_ref().map(|metrics| metrics.call());
            self.adapter.call(request)
        };
        positions.end();
        result
    }
    /// [`enter`](Self::enter) then [`issue`](Self::issue).
    pub fn request(&self, request: Request) -> Result<Completion, AdapterError> {
        let positions = self.enter(&request);
        self.issue(positions, request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Request as R;
    use crate::io_keys::{Key, Mode};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    /// Files keyed by their handle; each write records how many calls were in flight.
    #[derive(Default)]
    struct Files {
        in_flight: AtomicU32,
        overlapped: AtomicU32,
        order: Mutex<Vec<(u64, u8)>>,
    }
    impl ConcurrentAdapter for Files {
        fn keys(&self, request: &Request) -> KeySet {
            match request {
                R::FileWrite { file, .. } => KeySet::new(vec![
                    (Key(*file as u32 + 10), Mode::Exclusive),
                    (crate::io_keys::NAMESPACE, Mode::Shared),
                ]),
                _ => KeySet::new(vec![(crate::io_keys::NAMESPACE, Mode::Exclusive)]),
            }
        }
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&self, request: Request) -> Result<Completion, AdapterError> {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            if now > 1 {
                self.overlapped.fetch_add(1, Ordering::SeqCst);
            }
            std::thread::sleep(Duration::from_micros(200));
            if let R::FileWrite { file, bytes, .. } = &request {
                self.order.lock().unwrap().push((*file, bytes[0]));
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(Completion::Size(Ok(1)))
        }
    }

    #[test]
    fn writes_to_independent_files_overlap_and_each_files_writes_stay_in_order() {
        let files = Arc::new(Files::default());
        let metrics = Arc::new(IoMetrics::default());
        let issuer = Arc::new(Issuer::new(files.clone(), Some(metrics.clone())));
        let workers: Vec<_> = (0..8u64)
            .map(|file| {
                let issuer = issuer.clone();
                std::thread::spawn(move || {
                    for i in 0..20u8 {
                        issuer
                            .request(R::FileWrite {
                                directory: 1,
                                file,
                                bytes: vec![i],
                            })
                            .unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(files.overlapped.load(Ordering::SeqCst) > 0, "no overlap");
        for file in 0..8 {
            let order: Vec<u8> = files
                .order
                .lock()
                .unwrap()
                .iter()
                .filter(|(f, _)| *f == file)
                .map(|&(_, b)| b)
                .collect();
            assert_eq!(order, (0..20).collect::<Vec<_>>());
        }
        let values = metrics.values();
        assert!(values[3].1 > 1.0, "{values:?}");
    }

    #[test]
    fn a_namespace_mutation_runs_alone() {
        let files = Arc::new(Files::default());
        let issuer = Arc::new(Issuer::new(files.clone(), None));
        let workers: Vec<_> = (0..8u64)
            .map(|file| {
                let issuer = issuer.clone();
                let files = files.clone();
                std::thread::spawn(move || {
                    for i in 0..10u8 {
                        let request = if i % 3 == 0 {
                            R::Remove {
                                directory: 1,
                                path: format!("{file}"),
                            }
                        } else {
                            R::FileWrite {
                                directory: 1,
                                file,
                                bytes: vec![i],
                            }
                        };
                        let positions = issuer.enter(&request);
                        let exclusive = matches!(request, R::Remove { .. });
                        issuer.keys.wait(&positions);
                        if exclusive {
                            assert_eq!(files.in_flight.load(Ordering::SeqCst), 0);
                        }
                        let _ = files.call(request);
                        positions.end();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn a_coupled_custom_adapter_serialises_every_request() {
        struct Counter(u32);
        impl Adapter for Counter {
            fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
                Ok(None)
            }
            fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
                self.0 += 1;
                Ok(Completion::Unit(Ok(())))
            }
        }
        let adapter = Arc::new(Coupled::new(Counter(0)));
        assert_eq!(
            adapter.keys(&R::Print {
                stdout: 1,
                bytes: vec![]
            }),
            KeySet::coupled()
        );
        let issuer = Arc::new(Issuer::new(adapter.clone(), None));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let issuer = issuer.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        issuer
                            .request(R::Print {
                                stdout: 1,
                                bytes: vec![],
                            })
                            .unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        drop(issuer);
        assert_eq!(Arc::into_inner(adapter).unwrap().into_inner().0, 800);
    }
}
