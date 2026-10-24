//! `hirsel serve`: a runner host for `remote.tls@1` (docs/spec/runners.md RUN-13). It
//! listens where its host configuration says, admits each TLS client as the caller its
//! certificate (or its absence) names, checks every arriving call (RUN-8) against the
//! callee entries its program defines, keeps each caller's call streams exactly once
//! ([`hirsel_remote::host::Host`]), and runs each admitted call as its own callee run on
//! this host, recorded to its own tape when the configuration records callees (RUN-10).
//!
//! Connections run on their own threads; the callee runs run one at a time on the calling
//! thread, which owns the program. A cancellation a caller forwards reaches the running
//! callee through its [`ExternalCancel`](crate::network::ExternalCancel).
use crate::network::{CancelHandle, ExternalCancel};
use crate::{HostLaunch, ToolError};
use hirsel_remote::frame::Call;
use hirsel_remote::host::{
    ABANDONED, Action, Caller, Host, Limits, Place, Refusal, RunId, SessionId, Start,
};
use hirsel_remote::transport::{Command, Event, HostTls, Listener};
use hirsel_remote::wire::Wire;
use hirsel_run_config::host::HostConfig;
use hirsel_run_schema::{CallForm, CallOutcome, CalleeFacts, CalleeOutcome, CodeReach, IoGrants};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

const DENIED: u32 = 0;
const VERSION_MISMATCH: u32 = 1;
const REJECTED: u32 = 4;

/// The callee entries this host serves, and its place policy.
struct Served {
    entries: BTreeMap<String, (CalleeFacts, CallForm)>,
    /// `admit.safe@1`: a callee that reaches an extern or `Unsafe` operation is refused.
    safe_only: bool,
}
impl Place for Served {
    /// RUN-8 checks 3 and 4, for a caller the session already admitted (check 2).
    fn admit(&mut self, caller: &Caller, call: &Call) -> Result<IoGrants, Refusal> {
        let refuse = |reason, missing| Err(Refusal { reason, missing });
        let Some((facts, form)) = self.entries.get(&call.path) else {
            return refuse(VERSION_MISMATCH, vec![call.code]);
        };
        if facts.code != call.code || facts.fingerprint != call.fingerprint {
            return refuse(VERSION_MISMATCH, vec![call.code]);
        }
        let listed = |allowlist: &Option<Vec<String>>| {
            allowlist
                .as_ref()
                .is_none_or(|paths| paths.contains(&call.path))
        };
        if *form != call.form
            || !listed(&caller.definitions)
            || !listed(&call.definitions)
            || (self.safe_only && facts.reach != CodeReach::Safe)
        {
            return refuse(DENIED, vec![]);
        }
        match hirsel_run_schema::narrow(&caller.grants, &call.grants) {
            Some(grants) => Ok(grants),
            None => refuse(DENIED, vec![]),
        }
    }
}

struct Shared {
    host: Host,
    place: Served,
    sessions: BTreeMap<SessionId, Sender<Command>>,
    cancels: BTreeMap<RunId, CancelHandle>,
    next_session: SessionId,
    starts: Sender<(Start, ExternalCancel)>,
}
impl Shared {
    fn dispatch(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Send(session, frame) => {
                    if let Some(connection) = self.sessions.get(&session) {
                        let _ = connection.send(Command::Send(frame));
                    }
                }
                Action::Close(session) => {
                    if let Some(connection) = self.sessions.get(&session) {
                        let _ = connection.send(Command::Close);
                    }
                }
                Action::Start(start) => match ExternalCancel::pair() {
                    Ok((cancel, handle)) => {
                        self.cancels.insert(start.run, handle);
                        let _ = self.starts.send((start, cancel));
                    }
                    Err(error) => {
                        eprintln!("hirsel serve: a callee's cancellation channel: {error}");
                        let actions =
                            self.host
                                .ended(start.run, CallOutcome::InDoubt(ABANDONED), None);
                        self.dispatch(actions);
                    }
                },
                Action::Cancel(run, _) => {
                    if let Some(handle) = self.cancels.get(&run) {
                        handle.cancel();
                    }
                }
            }
        }
    }
}

fn host_error(what: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::Runtime(format!("hirsel serve: {what}: {error}"))
}
fn read(path: &str, what: &str) -> Result<Vec<u8>, ToolError> {
    std::fs::read(path).map_err(|e| host_error(what, format!("{path}: {e}")))
}

/// Serve `program`'s callee entries under the host configuration at `config` until the
/// process ends. `ready` is told the listening endpoint once connections are accepted.
pub fn serve(
    program: &Path,
    config: &Path,
    verify: hirsel_engine::Verify,
    ready: &mut dyn FnMut(&str),
) -> Result<(), ToolError> {
    let bytes = std::fs::read(config).map_err(|e| {
        host_error(
            "the host configuration",
            format!("{}: {e}", config.display()),
        )
    })?;
    let config =
        HostConfig::validate(&bytes).map_err(|e| host_error("the host configuration", e))?;
    // The callee runs run one at a time on this thread, which owns the program.
    if config.runs() != 1 {
        return Err(host_error(
            "host.runs",
            "only 1 callee run at a time is supported",
        ));
    }
    let mut session = crate::session::Session::open(program, verify);
    let initial = session.request(crate::requests::CompileOperation::Tir { definition: None })?;
    let crate::requests::CompileResult::Tir(input) = &initial.result else {
        return Err(host_error(
            "compiler",
            "TIR request returned another operation",
        ));
    };
    crate::tir_execution::refuse_foreign_interpretation(input)?;
    let entries = input.entries.clone();
    let runtime = input.runtime.clone();
    let wire = Arc::new(
        crate::network::remote_wire(&runtime, config.frame_bytes())
            .map_err(|e| host_error("the frame codec", e))?,
    );
    let fresh: [u8; 16] = hirsel_rt::io::RandomSource::System
        .draw(16)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| host_error("the incarnation", "the secure generator is unavailable"))?;
    let journal: Box<dyn hirsel_remote::journal::Journal> = match &config.journal {
        None => Box::new(hirsel_remote::journal::NoJournal::new(fresh)),
        Some(entry) => Box::new(
            hirsel_remote::journal::DirJournal::open(
                Path::new(entry.path.as_deref().unwrap_or_default()),
                fresh,
            )
            .map_err(|e| host_error("the call journal", e))?,
        ),
    };
    let host = Host::new(
        journal,
        Limits {
            runs: config.runs(),
            queue: config.queue(),
        },
    )
    .map_err(|e| host_error("the call journal", e))?;
    let clients = match &config.clients {
        Some(roots) => Some(read(&roots.pem, "host.clients")?),
        None => None,
    };
    let tls = Arc::new(
        HostTls::new(
            &read(&config.identity.chain, "host.identity")?,
            &read(&config.identity.key, "host.identity")?,
            clients.as_deref(),
        )
        .map_err(|e| host_error("host.identity", e))?,
    );
    let listener = Listener::bind(&config.listen).map_err(|e| host_error("host.listen", e))?;
    let (starts, started) = channel();
    let shared = Arc::new(Mutex::new(Shared {
        host,
        place: Served {
            entries: entries.clone(),
            safe_only: config.admission.is_some(),
        },
        sessions: BTreeMap::new(),
        cancels: BTreeMap::new(),
        next_session: 1,
        starts,
    }));
    let config = Arc::new(config);
    {
        let (shared, config, wire) = (shared.clone(), config.clone(), wire.clone());
        std::thread::Builder::new()
            .name("runner host".into())
            .spawn(move || accept(listener, tls, shared, config, wire))
            .map_err(|e| host_error("the listener", e))?;
    }
    ready(&match &config.listen {
        hirsel_run_schema::Endpoint::Unix(path) => path.clone(),
        hirsel_run_schema::Endpoint::Tcp { address, service } => format!("{address}:{service}"),
    });
    for (start, cancel) in started {
        let run = start.run;
        let (end, callee) = run_callee(&mut session, &entries, &config, &start, cancel);
        let mut shared = shared.lock().unwrap();
        shared.cancels.remove(&run);
        let actions = shared.host.ended(run, end, callee);
        shared.dispatch(actions);
    }
    Ok(())
}

/// Accept connections until the listener fails; each runs on its own thread.
fn accept(
    listener: Listener,
    tls: Arc<HostTls>,
    shared: Arc<Mutex<Shared>>,
    config: Arc<HostConfig>,
    wire: Arc<Wire>,
) {
    loop {
        let socket = match listener.accept() {
            Ok(socket) => socket,
            Err(error) => {
                eprintln!("hirsel serve: accept: {error}");
                continue;
            }
        };
        let (tls, shared, config, wire) =
            (tls.clone(), shared.clone(), config.clone(), wire.clone());
        let _ = std::thread::Builder::new()
            .name("runner session".into())
            .spawn(move || {
                // A failed handshake is the client's: it never becomes a session.
                let Ok((tls, peer)) = tls.accept(socket) else {
                    return;
                };
                let caller = config
                    .caller(peer.as_ref().map(|p| &p.digest), |name| {
                        peer.as_ref().is_some_and(|p| p.names(name))
                    })
                    .map(|entry| Caller {
                        name: entry.name.clone(),
                        grants: entry.grants.clone(),
                        definitions: entry.definitions.clone(),
                    });
                let (commands, received) = channel();
                let session = {
                    let mut shared = shared.lock().unwrap();
                    let session = shared.next_session;
                    shared.next_session += 1;
                    shared.sessions.insert(session, commands);
                    shared.host.connected(session, caller);
                    session
                };
                let digests = wire.clone();
                hirsel_remote::transport::serve(tls, wire, received, |event| {
                    let mut guard = shared.lock().unwrap();
                    let shared = &mut *guard;
                    let actions = match event {
                        Event::Frame(frame) => {
                            shared
                                .host
                                .frame(session, frame, &mut shared.place, &*digests)
                        }
                        Event::Closed => {
                            shared.sessions.remove(&session);
                            shared.host.disconnected(session)
                        }
                    };
                    shared.dispatch(actions);
                    true
                });
            });
    }
}

/// Run one admitted call as its own callee run (RUN-10): its outcome, and the callee run's
/// instance when it ran.
fn run_callee(
    session: &mut crate::session::Session,
    entries: &BTreeMap<String, (CalleeFacts, CallForm)>,
    config: &HostConfig,
    start: &Start,
    cancel: ExternalCancel,
) -> (CallOutcome, Option<[u8; 16]>) {
    let call = &start.call;
    let Some((facts, _)) = entries.get(&call.path) else {
        return (CallOutcome::NotRun(VERSION_MISMATCH), None);
    };
    let selected = crate::def_path(&call.path).and_then(|definition| {
        session
            .request_applied(crate::requests::CompileOperation::Tir {
                definition: Some(definition),
            })
            .ok()
    });
    let Some(crate::requests::CompileResult::Tir(input)) =
        selected.as_ref().map(|selected| &selected.result)
    else {
        return (CallOutcome::NotRun(REJECTED), None);
    };
    // Check 8 uses the selected immutable callee's parameter schema.
    let decodes = crate::call_schemas(&input.signature, &input.runtime)
        .ok()
        .and_then(|(_, argument)| {
            hirsel_interp::library::runners::decode(&argument, &call.argument, &input.runtime).ok()
        });
    if decodes.is_none() {
        return (CallOutcome::NotRun(REJECTED), None);
    }
    let key = hirsel_run_schema::call_key(&start.instance, call.ordinal);
    let cause = hirsel_run_schema::Cause {
        instance: start.instance,
        ordinal: call.ordinal,
        digest: start.digest,
    };
    let seed = |name: &str| hirsel_run_schema::callee_seed(&key, name);
    let mut record = hirsel_run_config::RunConfig::empty(
        facts.program,
        call.path.clone(),
        hirsel_run_schema::LaunchKind::Call,
    );
    record.call = Some(hirsel_run_schema::CallRecord::new(
        call.form,
        &cause,
        &call.argument,
    ));
    record.inputs.hash_seed = seed("hash");
    record.inputs.random_seed = seed("random");
    record.inputs.test_seed = seed("test");
    record.io = (call.form == CallForm::Call).then(|| start.grants.clone());
    record.admission = config
        .admission
        .as_ref()
        .map(|place| hirsel_run_schema::Admission {
            not_before: None,
            expires: None,
            revocation: None,
            code: place.code.clone(),
        });
    let callee = hirsel_run_schema::callee_instance(&key);
    let run = match record.validated() {
        Ok(run) => run,
        Err(error) => {
            eprintln!("hirsel serve: {}: callee admission: {error}", call.path);
            return (CallOutcome::NotRun(REJECTED), None);
        }
    };
    // Each callee run records to its own tape, named by its call key.
    let tape = config
        .tapes
        .as_ref()
        .map(|dir| PathBuf::from(dir).join(format!("{}.tape", hirsel_run_schema::hex(&key))));
    let launch = HostLaunch::Callee {
        run: &run,
        record: tape.as_deref(),
    };
    let result = crate::launch_binding::LaunchSelection::read(launch).and_then(|selection| {
        crate::tir_execution::run_tir_on_host(
            input,
            launch,
            selection,
            None,
            None,
            Some(cancel),
            false,
        )
    });
    match result {
        Ok((_, Some(CalleeOutcome::Ok(bytes)))) => (CallOutcome::Ok(bytes), Some(callee)),
        Ok((_, Some(CalleeOutcome::Err(bytes)))) => (CallOutcome::Failed(bytes), Some(callee)),
        Ok((_, Some(CalleeOutcome::Failed(bytes)))) => (CallOutcome::Faulted(bytes), Some(callee)),
        // A result beyond the codec's budgets cannot cross.
        Ok((hirsel_run_schema::RunOutcome::Returned(_), None)) => {
            (CallOutcome::NotRun(REJECTED), Some(callee))
        }
        Ok((outcome, None)) => {
            eprintln!(
                "hirsel serve: {}: the callee ended without an outcome: {outcome:?}",
                call.path
            );
            (CallOutcome::InDoubt(ABANDONED), Some(callee))
        }
        Err(error) => {
            eprintln!("hirsel serve: {}: {error:?}", call.path);
            (CallOutcome::InDoubt(ABANDONED), Some(callee))
        }
    }
}
