//! A host run's `remote.tls@1` runners (docs/spec/runners.md, Providers). Each runner's
//! provider ([`hirsel_remote::client::Client`]) runs on its own thread with the TLS
//! connection it opens, so the interpreter never blocks on the network: the run hands it
//! each sent call and each forwarded cancellation, and takes back each completion at its
//! next scheduling point. A completion is announced by a byte on a socket the run's
//! readiness watches for every parked remote call, so an idle run wakes for it.
//!
//! The TLS material, the endpoint and the settings are read once, at launch; a runner whose
//! material does not load refuses the launch.
use hirsel_remote::client::{Client, ClientAction, Settings};
use hirsel_remote::frame::Call;
use hirsel_remote::transport::{ClientTls, Command, Event};
use hirsel_remote::wire::Wire;
use hirsel_run_schema::{Endpoint, Roots, RunnerCompletion, RunnerEntry};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

enum Input {
    Submit(u64, Box<Call>, [u8; 32]),
    Cancel(u64, u32),
    /// A frame or the end of connection `generation`.
    Connection(u64, Event),
    Close,
}

struct Runner {
    inputs: Sender<Input>,
    thread: Option<JoinHandle<()>>,
}

/// The run's remote runners, by name.
pub(crate) struct RemoteRunners {
    runners: BTreeMap<String, Runner>,
    completions: Arc<Mutex<Vec<(u64, RunnerCompletion)>>>,
    wake: UnixStream,
}

fn read(path: &str, what: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("io.runners: {what} {path}: {e}"))
}

impl RemoteRunners {
    /// Open a provider for every `remote.tls@1` runner of `entries`; None when there is none.
    pub(crate) fn open(
        entries: &[RunnerEntry],
        instance: [u8; 16],
        wire: impl FnOnce() -> Result<Wire, String>,
    ) -> Result<Option<Self>, String> {
        let remote: Vec<&RunnerEntry> = entries
            .iter()
            .filter(|e| e.provider == "remote.tls@1")
            .collect();
        if remote.is_empty() {
            return Ok(None);
        }
        let wire = Arc::new(wire()?);
        let (wake, signal) = UnixStream::pair().map_err(|e| format!("io.runners: {e}"))?;
        wake.set_nonblocking(true)
            .map_err(|e| format!("io.runners: {e}"))?;
        let completions = Arc::new(Mutex::new(vec![]));
        let mut runners = BTreeMap::new();
        for entry in remote {
            let roots = match entry.roots.as_ref() {
                Some(Roots::Pem { pem }) => Some(read(pem, "roots")?),
                _ => None,
            };
            let identity = match &entry.identity {
                Some(files) => Some((
                    read(&files.chain, "identity")?,
                    read(&files.key, "identity")?,
                )),
                None => None,
            };
            let tls = ClientTls::new(
                entry.server_name.as_deref().unwrap_or_default(),
                roots.as_deref(),
                identity.as_ref().map(|(c, k)| (c.as_slice(), k.as_slice())),
            )
            .map_err(|e| format!("io.runners.{}: {e}", entry.name))?;
            let endpoint = entry
                .endpoint
                .clone()
                .ok_or_else(|| format!("io.runners.{}: no endpoint", entry.name))?;
            let stream = hirsel_run_schema::hash_parts(
                "hirsel.stream.v1",
                [entry.name.as_bytes(), &instance[..]],
            );
            let client = Client::new(
                stream,
                instance,
                Settings {
                    timeout_ns: entry.timeout_ns,
                    reconnect: entry.reconnects(),
                    retry: entry.attempts(),
                    reconcile: entry.reconcile,
                },
            );
            let (inputs, received) = channel();
            let worker = Worker {
                tls,
                endpoint,
                wire: wire.clone(),
                client,
                inputs: inputs.clone(),
                received,
                connection: None,
                generation: 0,
                completions: completions.clone(),
                signal: signal.try_clone().map_err(|e| format!("io.runners: {e}"))?,
                start: Instant::now(),
            };
            let thread = std::thread::Builder::new()
                .name(format!("runner {}", entry.name))
                .spawn(move || worker.run())
                .map_err(|e| format!("io.runners: {e}"))?;
            runners.insert(
                entry.name.clone(),
                Runner {
                    inputs,
                    thread: Some(thread),
                },
            );
        }
        Ok(Some(Self {
            runners,
            completions,
            wake,
        }))
    }
    pub(crate) fn serves(&self, runner: &str) -> bool {
        self.runners.contains_key(runner)
    }
    /// The descriptor a parked remote call's readiness watches.
    pub(crate) fn wake_fd(&self) -> RawFd {
        self.wake.as_raw_fd()
    }
    pub(crate) fn submit(&self, runner: &str, token: u64, call: Call, digest: [u8; 32]) {
        if let Some(r) = self.runners.get(runner) {
            let _ = r.inputs.send(Input::Submit(token, Box::new(call), digest));
        }
    }
    pub(crate) fn cancel(&self, runner: &str, token: u64, cause: u32) {
        if let Some(r) = self.runners.get(runner) {
            let _ = r.inputs.send(Input::Cancel(token, cause));
        }
    }
    /// The completions that arrived since the last take.
    pub(crate) fn take(&self) -> Vec<(u64, RunnerCompletion)> {
        let mut drained = [0u8; 64];
        while matches!((&self.wake).read(&mut drained), Ok(n) if n > 0) {}
        std::mem::take(&mut *self.completions.lock().unwrap())
    }
}
impl Drop for RemoteRunners {
    /// The run ended: each stream's session closes, acknowledging what the run holds.
    fn drop(&mut self) {
        for runner in self.runners.values_mut() {
            let _ = runner.inputs.send(Input::Close);
            if let Some(thread) = runner.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

struct Worker {
    tls: ClientTls,
    endpoint: Endpoint,
    wire: Arc<Wire>,
    client: Client,
    inputs: Sender<Input>,
    received: Receiver<Input>,
    connection: Option<Sender<Command>>,
    generation: u64,
    completions: Arc<Mutex<Vec<(u64, RunnerCompletion)>>>,
    signal: UnixStream,
    start: Instant,
}
impl Worker {
    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
    fn run(mut self) {
        loop {
            let wait = self
                .client
                .deadline()
                .map(|at| Duration::from_nanos(at.saturating_sub(self.now())))
                .unwrap_or(Duration::from_secs(3600));
            let input = match self.received.recv_timeout(wait) {
                Ok(input) => Some(input),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let now = self.now();
            let mut actions = self.client.tick(now);
            match input {
                None => {}
                Some(Input::Submit(token, call, digest)) => {
                    actions.extend(self.client.submit(token, *call, digest))
                }
                Some(Input::Cancel(token, cause)) => {
                    actions.extend(self.client.cancel(token, cause))
                }
                Some(Input::Connection(generation, event)) if generation == self.generation => {
                    actions.extend(match event {
                        Event::Frame(frame) => self.client.frame(frame),
                        Event::Closed => {
                            self.connection = None;
                            self.client.lost()
                        }
                    })
                }
                Some(Input::Connection(..)) => {}
                Some(Input::Close) => {
                    let actions = self.client.close();
                    self.apply(actions);
                    if let Some(connection) = self.connection.take() {
                        let _ = connection.send(Command::Close);
                    }
                    return;
                }
            }
            self.apply(actions);
        }
    }
    fn apply(&mut self, actions: Vec<ClientAction>) {
        let mut queue = std::collections::VecDeque::from(actions);
        while let Some(action) = queue.pop_front() {
            match action {
                ClientAction::Connect => match self.tls.connect(&self.endpoint) {
                    Ok(tls) => {
                        self.generation += 1;
                        let generation = self.generation;
                        let (commands, received) = channel();
                        let inputs = self.inputs.clone();
                        let wire = self.wire.clone();
                        let spawned = std::thread::Builder::new()
                            .name("runner connection".into())
                            .spawn(move || {
                                hirsel_remote::transport::serve(tls, wire, received, |event| {
                                    inputs.send(Input::Connection(generation, event)).is_ok()
                                })
                            });
                        if spawned.is_ok() {
                            self.connection = Some(commands);
                            queue.extend(self.client.connected());
                        } else {
                            queue.extend(self.client.lost());
                        }
                    }
                    Err(_) => queue.extend(self.client.lost()),
                },
                ClientAction::Send(frame) => {
                    if let Some(connection) = &self.connection {
                        let _ = connection.send(Command::Send(frame));
                    }
                }
                ClientAction::Disconnect => {
                    if let Some(connection) = self.connection.take() {
                        let _ = connection.send(Command::Close);
                    }
                }
                ClientAction::Complete(token, completion) => {
                    self.completions.lock().unwrap().push((token, completion));
                    let _ = (&self.signal).write(&[1]);
                }
            }
        }
    }
}

/// The request frame of a sent call: the recorded request and the parts only its digest
/// holds.
pub(crate) fn call(
    request: &hirsel_run_schema::RunnerRequest,
    remote: hirsel_interp::library::runners::RemoteRequest,
) -> Call {
    Call {
        epoch: 0,
        position: 0,
        ordinal: remote.ordinal,
        key: request.key,
        form: request.form,
        path: request.path.clone(),
        fingerprint: remote.fingerprint,
        code: remote.code,
        grants: remote.grants,
        definitions: remote.definitions,
        argument: request.argument.clone(),
        acknowledged: 0,
    }
}

/// The frame codec of a run, from its program's own core types.
pub(crate) fn wire(
    runtime: &hirsel_interp::aggregate::RuntimeTypes,
    limit: u64,
) -> Result<Wire, String> {
    use hirsel_interp::library::runners::{allowlist_schema, core_schema};
    let schema = |r: Result<_, hirsel_interp::Error>| r.map_err(|e| format!("RunFrame: {e:?}"));
    Wire::new(
        &schema(core_schema(runtime, "RunFrame"))?,
        &schema(core_schema(runtime, "GrantSet"))?,
        &schema(allowlist_schema(runtime))?,
        limit,
    )
    .map_err(|e| format!("RunFrame: {e:?}"))
}

/// A runner host's cancellation of a callee run it serves (RUN-9), as the run sees it: the
/// host's thread sets the flag and writes a byte, which wakes the run's idle wait, and the
/// run cancels its root scope with cause `Parent` at its next scheduling point.
pub(crate) struct ExternalCancel {
    flag: Arc<std::sync::atomic::AtomicBool>,
    wake: UnixStream,
    applied: bool,
}
/// The host's side of an [`ExternalCancel`].
#[derive(Clone)]
pub(crate) struct CancelHandle {
    flag: Arc<std::sync::atomic::AtomicBool>,
    signal: Arc<UnixStream>,
}
impl ExternalCancel {
    pub(crate) fn pair() -> std::io::Result<(Self, CancelHandle)> {
        let (wake, signal) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        signal.set_nonblocking(true)?;
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        Ok((
            Self {
                flag: flag.clone(),
                wake,
                applied: false,
            },
            CancelHandle {
                flag,
                signal: Arc::new(signal),
            },
        ))
    }
    pub(crate) fn wake_fd(&self) -> RawFd {
        self.wake.as_raw_fd()
    }
    /// Whether the host cancelled the run since the last look; true once.
    pub(crate) fn take(&mut self) -> bool {
        let mut drained = [0u8; 16];
        while matches!((&self.wake).read(&mut drained), Ok(n) if n > 0) {}
        if self.applied || !self.flag.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        self.applied = true;
        true
    }
}
impl CancelHandle {
    pub(crate) fn cancel(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::Release);
        let _ = (&*self.signal).write(&[1]);
    }
}
