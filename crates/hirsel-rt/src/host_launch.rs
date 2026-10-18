//! Production PROG-6 admission, recording and replay shared by source and native
//! launch adapters.
use crate::launch::{RecordedRun, Recording, RecordingWriter};
use crate::{
    host_paths::PinnedDirectory, host_stdin::CooperativeStdin, io_real::RealIo, io_sim::SimIo,
    net::NetRule, net_host::CooperativeNet, readiness::Readiness,
};
use hirsel_run_config::{
    AdmittedRun, HostAuthority, RevocationEvidence, RunConfig, RunRefusal, ValidatedRun,
};
use std::{fs::File, path::Path};

/// Stream every completion of an admitted host run to `path` (DET-1). The header binds
/// the canonical run record, its admission time and the mounts' initial tree, and is
/// durable before entry. The destination must lie outside every granted mount and
/// output file.
pub fn record(
    io: &mut RealIo,
    admitted: &AdmittedRun,
    path: &Path,
    outputs: &[(&str, &File)],
) -> Result<(), String> {
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
    let writer = RecordingWriter::new(file, &header).map_err(|e| format!("the Io tape: {e}"))?;
    io.record_to(writer);
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
    let recording = std::fs::read_to_string(path)
        .map_err(|e| invalid(format!("{}: {e}", path.display())))
        .and_then(|text| crate::launch::parse_recording(&text).map_err(invalid))?;
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

pub struct HostGrants {
    /// Admit cooperative providers even when the record grants no tasks (the interpreter).
    always_cooperative: bool,
    revocations: Option<hirsel_run_config::Revocations>,
    io: Option<RealIo>,
    output: Option<Box<dyn std::io::Write>>,
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
        }
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
    fn authorize(&mut self, record: &RunConfig) -> Result<(), RunRefusal> {
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
        // Use the sole validator even for callers admitting already-built data.
        let run = record.validated()?;
        io.set_projections(run.io_projections());
        if record
            .io
            .as_ref()
            .is_some_and(|grants| grants.stdin.is_some())
        {
            match &readiness {
                Some(readiness) => {
                    io.set_cooperative_stdin(CooperativeStdin::pin(readiness.clone()).map_err(
                        |error| RunRefusal::Unauthorized {
                            field: format!("io.stdin: {error}"),
                        },
                    )?)
                }
                None => io.set_stdin(std::io::stdin()),
            }
        }
        if let Some(readiness) = &readiness {
            io.set_cooperative(readiness.clone());
            io.set_net_provider(CooperativeNet::new(readiness.clone()));
        }
        if let Some(provider) = run.random_provider() {
            io.set_random(provider, run.config().random_seed());
        }
        if record
            .io
            .as_ref()
            .is_some_and(|grants| grants.stderr.is_some())
        {
            io.set_stderr(std::io::stderr());
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
