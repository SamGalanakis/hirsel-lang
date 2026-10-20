//! Production PROG-6 admission, recording and replay shared by source and native
//! launch adapters.
use crate::launch::{RecordedRun, Recording, RecordingWriter};
use crate::{
    host_paths::PinnedDirectory, host_stdin::CooperativeStdin, io_real::RealIo, io_sim::SimIo,
    net::NetRule, readiness::Readiness,
};
use hirsel_run_config::{AdmittedRun, HostAuthority, RevocationEvidence, RunRefusal, ValidatedRun};
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
    use hirsel_run_config::RecordingLevel;
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
) -> Result<hirsel_run_config::RecordingLevel, String> {
    use hirsel_run_config::RecordingLevel;
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
fn header(
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
            admitted_at: admitted.evidence().at,
            revocation: admitted.evidence().revocation.clone(),
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
    let (file, header, _) = header(io, admitted, path, outputs)?;
    let writer = RecordingWriter::new(file, &header).map_err(|e| format!("the Io tape: {e}"))?;
    io.record_to(writer);
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
    io.journal_to(Box::new(journal));
    Ok(())
}

/// Simulated Io that serves exactly a recording's events under the run it bound,
/// re-deriving every grant decision from that record. It acquires no host resource.
/// The caller checks the run against the program it launches.
pub fn replay(path: &Path) -> Result<(SimIo, ValidatedRun), RunRefusal> {
    let invalid = |detail: String| RunRefusal::Invalid {
        field: "recording".into(),
        detail,
    };
    let bytes = std::fs::read(path).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    let recording = if crate::journal::is_journal(&bytes) {
        // A durable journal replays only when complete; otherwise recovery names what is in doubt.
        let recovered = crate::journal::recover(&bytes).map_err(|e| invalid(e.to_string()))?;
        if !recovered.complete() {
            return Err(invalid(crate::journal::incomplete(&recovered)));
        }
        recovered.recording
    } else {
        String::from_utf8(bytes)
            .map_err(|_| invalid("bad recording UTF-8".into()))
            .and_then(|text| crate::launch::parse_recording(&text).map_err(invalid))?
    };
    let run = recording.bound_run()?;
    let mut io = SimIo::new(recording.tree, recording.events);
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
    Ok((io, run))
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
        let readiness = (self.always_cooperative
            || record.io.as_ref().is_some_and(|io| io.tasks.is_some()))
        .then(Readiness::default);
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
        self.io = Some(io);
        Ok(())
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
