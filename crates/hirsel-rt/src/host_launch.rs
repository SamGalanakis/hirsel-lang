//! Production PROG-6 admission, recording and replay shared by source and native
//! launch adapters.
use crate::launch::{RecordedRun, Recording, RecordingWriter};
use crate::{
    host_paths::PinnedDirectory, host_stdin::CooperativeStdin, io_real::RealIo, io_sim::SimIo,
    net::NetRule, readiness::Readiness,
};
use hirsel_run_config::{AdmittedRun, HostAuthority, RevocationEvidence, ValidatedRun};
use hirsel_run_schema::RunRefusal;
use std::{fs::File, path::Path};

/// Record an admitted host run at the level its record selects (determinism.md, Recording
/// levels), to `path` when the launch names a destination. Absent, the level follows the
/// launch: a destination records the tape. `record` or `durable` without a destination, or
/// `none` with one, refuse before entry.
pub fn journal(
    io: &mut RealIo,
    admitted: &AdmittedRun,
    path: Option<&Path>,
    outputs: &[(&str, &File)],
) -> Result<(), String> {
    use hirsel_run_schema::RecordingLevel;
    match (recording_level(admitted.run(), path.is_some())?, path) {
        (RecordingLevel::Record, Some(path)) => record(io, admitted, path, outputs),
        (RecordingLevel::Durable, Some(path)) => durable(io, admitted, path, outputs),
        _ => Ok(()),
    }
}

/// The level a launch records `run` at, with or without a recording destination; a level
/// and a destination that disagree refuse.
pub fn recording_level(
    run: &ValidatedRun,
    destination: bool,
) -> Result<hirsel_run_schema::RecordingLevel, String> {
    use hirsel_run_schema::RecordingLevel;
    match (run.record().recording, destination) {
        (None, false) => Ok(RecordingLevel::None),
        (None, true) => Ok(RecordingLevel::Record),
        (Some(RecordingLevel::None), true) => {
            Err("the run record selects recording none, but the launch names a recording".into())
        }
        (Some(level @ (RecordingLevel::Record | RecordingLevel::Durable)), false) => Err(format!(
            "the run record selects recording {}, which needs a recording destination",
            if level == RecordingLevel::Record {
                "record"
            } else {
                "durable"
            }
        )),
        (Some(level), _) => Ok(level),
    }
}

/// The recording's header: it binds the canonical run record, its admission time and the
/// mounts' initial tree. The destination must lie outside every granted mount and output
/// file.
pub(crate) fn header(
    io: &mut RealIo,
    admitted: &AdmittedRun,
    path: &Path,
    outputs: &[(&str, &File)],
) -> Result<(File, Recording, std::path::PathBuf), String> {
    let path = std::path::absolute(path).map_err(|e| format!("the Io tape: {e}"))?;
    let file = io
        .prepare_recording_with_files(&path, outputs)
        .map_err(|e| e.to_string())?;
    let tree = io
        .snapshot()
        .map_err(|e| format!("initial recording tree: {e:?}"))?;
    let run = admitted.run();
    let config = run.config();
    let header = Recording {
        arguments: run.record().args.clone().unwrap_or_default(),
        run_identity: run.identity(),
        hash_seed: config.hash_seed(),
        random_seed: config.random_seed(),
        test_seed: config.test_seed(),
        binding: Some(RecordedRun {
            canonical: run.canonical().to_vec(),
            placement: Some(run.record().placement.clone().unwrap_or_default()),
            admitted_at: admitted.evidence().at,
            revocation: admitted.evidence().revocation.clone(),
            instance: admitted.instance(),
        }),
        tree,
        events: vec![],
    };
    Ok((file, header, path))
}

/// Stream every completion of an admitted host run to `path` (DET-1), the header flushed
/// before entry.
pub fn record(
    io: &mut RealIo,
    admitted: &AdmittedRun,
    path: &Path,
    outputs: &[(&str, &File)],
) -> Result<(), String> {
    let (file, header, path) = header(io, admitted, path, outputs)?;
    let writer = RecordingWriter::new(file, &header).map_err(|e| format!("the Io tape: {e}"))?;
    timing_journal(
        io,
        Box::new(crate::journal::TapeJournal(writer)),
        path,
        outputs,
    );
    Ok(())
}

/// Journal an admitted host run durably to `path`: the header, the file and its directory
/// entry are durable before entry.
pub fn durable(
    io: &mut RealIo,
    admitted: &AdmittedRun,
    path: &Path,
    outputs: &[(&str, &File)],
) -> Result<(), String> {
    let (file, header, path) = header(io, admitted, path, outputs)?;
    let failed = |e: std::io::Error| format!("the durable journal: {e}");
    let journal = crate::journal::DurableJournal::create(file, &header).map_err(failed)?;
    journal.file().sync_all().map_err(failed)?;
    let parent = path.parent().unwrap_or(Path::new("/"));
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(failed)?;
    timing_journal(io, Box::new(journal), path, outputs);
    Ok(())
}

/// Attach diagnostic observation without changing the recording protocol. Reuse
/// the recorder's pinned destination checks: sidecars cannot alias a granted
/// output or enter a program-readable mount. Failure disables diagnostic timing.
fn timing_journal(
    io: &mut RealIo,
    journal: Box<dyn crate::journal::Journal + Send>,
    path: std::path::PathBuf,
    outputs: &[(&str, &File)],
) {
    let timing = crate::observation_timing::capture_id().and_then(|capture| {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(".timing.json");
        let mut protected = outputs.to_vec();
        // Opening the tape itself protects against hardlink aliases as well.
        let tape = File::open(&path).ok()?;
        protected.push(("recording", &tape));
        let output = io
            .prepare_recording_with_files(Path::new(&sidecar), &protected)
            .ok()?;
        Some((capture, output))
    });
    if let Some((capture, output)) = timing {
        io.send_journal_to(Box::new(crate::observation_timing::TimingJournal::new(
            journal, path, output, capture,
        )));
    } else {
        io.send_journal_to(journal);
    }
}

/// Simulated Io that serves exactly a recording's events under the run it bound,
/// re-deriving every grant decision from that record. It acquires no host resource.
/// The caller checks the run against the program it launches.
pub fn replay(path: &Path) -> Result<(SimIo, ValidatedRun), RunRefusal> {
    let bytes = std::fs::read(path).map_err(|e| RunRefusal::Invalid {
        field: "recording".into(),
        detail: e.to_string(),
    })?;
    let mut free = None;
    let (recording, binary) = if bytes.starts_with(hirsel_free::tape::MAGIC) {
        let header = crate::free_launch::header(&bytes).map_err(|detail| RunRefusal::Invalid {
            field: "recording".into(),
            detail,
        })?;
        free = Some(bytes);
        (header, None)
    } else if crate::journal::is_journal(&bytes) {
        (read_recording(path)?, None)
    } else {
        crate::tape_binary::replay(bytes).map_err(|detail| RunRefusal::Invalid {
            field: "recording".into(),
            detail,
        })?
    };
    let run = recording.bound_run()?;
    let instance = recording.binding.as_ref().and_then(|b| b.instance);
    let mut io = match binary {
        Some(tape) => SimIo::from_binary(recording.tree, tape),
        None => SimIo::new(recording.tree, recording.events),
    };
    io.free_tape = free;
    io.instance = instance;
    io.set_projections(run.io_projections());
    io.set_mounts(
        run.record()
            .io
            .iter()
            .flat_map(|io| &io.dirs)
            .map(|mount| (mount.path.clone(), mount.access)),
    )?;
    io.set_env(run.record().env.clone().unwrap_or_default());
    let (monotonic, wall) = run.clock_grants();
    io.clock_grants(monotonic, wall);
    if let Some(grant) = run.net_grant() {
        io.set_net_rules(NetRule::from_config(grant));
        if let Some(peers) = &grant.peers {
            io.set_net_script(peers.clone(), grant.listeners.clone())
                .map_err(|_| RunRefusal::Unsupported {
                    field: "io.net.peers".into(),
                })?;
        }
    }
    if let Some(grant) = run.process_grant() {
        io.set_process_grant(grant);
    }
    if let Some(grant) = run.signals_grant() {
        io.set_signal_kinds(grant.bits());
    }
    Ok((io, run))
}

/// A complete recording at `path`: a tape, or a durable journal that recovers complete.
pub fn read_recording(path: &Path) -> Result<Recording, RunRefusal> {
    let invalid = |detail: String| RunRefusal::Invalid {
        field: "recording".into(),
        detail,
    };
    let bytes = std::fs::read(path).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    if crate::journal::is_journal(&bytes) {
        // A durable journal replays only when complete; otherwise recovery names what is in doubt.
        let recovered = crate::journal::recover(&bytes).map_err(|e| invalid(e.to_string()))?;
        if !recovered.complete() {
            return Err(invalid(crate::journal::incomplete(&recovered)));
        }
        Ok(recovered.recording)
    } else {
        crate::launch::parse_recording_bytes(&bytes).map_err(invalid)
    }
}

/// The admitted `io.signals` source: the process's signals, which one run holds at a time,
/// or the grant's script.
pub(crate) fn signal_source(
    grant: &hirsel_run_schema::SignalsGrant,
) -> Result<crate::signals::SignalSource, RunRefusal> {
    let unauthorized = |detail: String| RunRefusal::Unauthorized {
        field: format!("io.signals: {detail}"),
    };
    match &grant.script {
        Some(script) => crate::signals::SignalSource::script(grant.bits(), script)
            .map_err(|e| unauthorized(e.to_string())),
        None => match crate::signals::ProcessSignals::take(grant.bits()) {
            Ok(Some(signals)) => Ok(crate::signals::SignalSource::Host(signals)),
            Ok(None) => Err(unauthorized(
                "another run of this process holds its signals".into(),
            )),
            Err(error) => Err(unauthorized(error.to_string())),
        },
    }
}

/// The process's standard input, read by the run that holds it.
struct HeldStdin {
    input: std::io::Stdin,
    _token: crate::host::ProcessStdin,
}
impl std::io::Read for HeldStdin {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(buffer)
    }
}

pub struct HostGrants {
    /// Admit cooperative providers even when the record grants no tasks (the interpreter).
    always_cooperative: bool,
    /// The readiness the run's providers park on, when its execution shares one (RUN-12).
    readiness: Option<Readiness>,
    revocations: Option<hirsel_run_config::Revocations>,
    io: Option<RealIo>,
    output: Option<Box<dyn std::io::Write>>,
    stderr: Option<Box<dyn std::io::Write>>,
}
impl HostGrants {
    /// Native admission: a record that grants `io.tasks` gets the cooperative providers, so a
    /// request that would block parks its task (`crate::host_waits`); without tasks host
    /// requests block the thread.
    pub fn new(output: impl std::io::Write + 'static) -> Self {
        Self {
            always_cooperative: false,
            readiness: None,
            revocations: None,
            io: None,
            output: Some(Box::new(output)),
            stderr: None,
        }
    }
    /// The stream a granted `io.stderr` writes to, instead of the process's standard error.
    pub fn with_stderr(mut self, stderr: impl std::io::Write + 'static) -> Self {
        self.stderr = Some(Box::new(stderr));
        self
    }
    /// Interpreter admission for the cooperative scheduler: sockets and stdin are
    /// nonblocking and record their interest in one Readiness, with or without tasks.
    pub fn cooperative(output: impl std::io::Write + 'static) -> Self {
        Self {
            always_cooperative: true,
            ..Self::new(output)
        }
    }
    /// Interpreter admission of a callee run (RUN-12): cooperative providers that park on
    /// `readiness`, its caller's, under the callee's own tag.
    pub fn sharing(output: impl std::io::Write + 'static, readiness: Readiness) -> Self {
        readiness.mark_shared();
        Self {
            readiness: Some(readiness),
            ..Self::cooperative(output)
        }
    }
    /// Read the explicit host evidence exactly once, before admission.
    pub fn read_revocations(&mut self, path: &Path) -> Result<(), RunRefusal> {
        self.revocations = Some(hirsel_run_config::Revocations::read_path(path)?);
        Ok(())
    }
    pub fn into_io(self) -> RealIo {
        self.io.expect("successful host admission")
    }
}
impl HostAuthority for HostGrants {
    fn authorize(&mut self, run: &hirsel_run_config::ValidatedRun) -> Result<(), RunRefusal> {
        let record = run.record();
        // Cooperative providers share one readiness, which the RealIo polls and records.
        let readiness = self.readiness.clone().or_else(|| {
            (self.always_cooperative || record.io.as_ref().is_some_and(|io| io.tasks.is_some()))
                .then(Readiness::default)
        });
        let mounts = record
            .io
            .as_ref()
            .into_iter()
            .flat_map(|io| &io.dirs)
            .map(|mount| {
                PinnedDirectory::open(std::path::Path::new(&mount.host))
                    .map(|pin| (mount.path.clone(), mount.access, pin))
                    .map_err(|error| RunRefusal::Unauthorized {
                        field: format!("io.dirs.{} ({}): {error}", mount.path, mount.host),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut io =
            RealIo::from_pinned_mounts(mounts, self.output.take().expect("single admission"))?;
        io.set_projections(run.io_projections());
        if record
            .io
            .as_ref()
            .is_some_and(|grants| grants.stdin.is_some())
        {
            // One run at a time holds the process's standard input.
            let token =
                crate::host::ProcessStdin::take().ok_or_else(|| RunRefusal::Unauthorized {
                    field: "io.stdin: another run of this process holds its standard input".into(),
                })?;
            match &readiness {
                Some(readiness) => io.set_cooperative_stdin(
                    CooperativeStdin::pin(readiness.clone())
                        .map_err(|error| RunRefusal::Unauthorized {
                            field: format!("io.stdin: {error}"),
                        })?
                        .holding(token),
                ),
                None => io.set_stdin(HeldStdin {
                    input: std::io::stdin(),
                    _token: token,
                }),
            }
        }
        if let Some(readiness) = &readiness {
            io.set_cooperative_net(readiness.clone());
        }
        if let Some(provider) = run.random_provider() {
            io.set_random(provider, run.config().random_seed());
        }
        if let Some((provider, seed, queue)) = run.telemetry_provider() {
            io.set_telemetry(crate::telemetry::Capture::of(provider, seed, queue));
        }
        if record
            .io
            .as_ref()
            .is_some_and(|grants| grants.stderr.is_some())
        {
            match self.stderr.take() {
                Some(stderr) => io.set_stderr(stderr),
                None => io.set_stderr(std::io::stderr()),
            }
        }
        io.set_env(record.env.clone().unwrap_or_default());
        let (monotonic, wall) = run.clock_grants();
        io.clock_grants(monotonic, wall);
        if let Some(grant) = run.net_grant() {
            io.set_net_rules(NetRule::from_config(grant));
            if let Some(peers) = &grant.peers {
                io.set_net_script(peers.clone(), grant.listeners.clone())
                    .map_err(|_| RunRefusal::Unsupported {
                        field: "io.net.peers".into(),
                    })?;
            }
        }
        if let Some(grant) = run.process_grant() {
            io.set_process_grant(grant);
        }
        if let Some(grant) = run.signals_grant() {
            io.set_signal_grant(grant)?;
        }
        self.io = Some(io);
        Ok(())
    }
    fn run_instance(&mut self) -> Result<[u8; 16], RunRefusal> {
        crate::io::RandomSource::System
            .draw(16)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| RunRefusal::Unauthorized {
                field: "io.runners: the host's secure generator is unavailable".into(),
            })
    }
    fn revocation(&mut self, reference: &str) -> Result<RevocationEvidence, RunRefusal> {
        self.revocations
            .as_ref()
            .map(|list| list.evidence(reference))
            .ok_or_else(|| RunRefusal::MissingEvidence {
                reference: reference.into(),
            })
    }
}
