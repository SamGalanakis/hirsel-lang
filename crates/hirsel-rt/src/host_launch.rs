//! Production PROG-6 admission shared by source and native launch adapters.
use crate::{
    host_paths::PinnedDirectory, host_stdin::CooperativeStdin, io_real::RealIo, net::NetRule,
    net_host::CooperativeNet, readiness::Readiness,
};
use hirsel_run_config::{HostAuthority, RevocationEvidence, RunConfig, RunRefusal};

pub struct HostGrants {
    /// Present for cooperative admission: host net and stdin park on it.
    readiness: Option<Readiness>,
    io: Option<RealIo>,
    output: Option<Box<dyn std::io::Write>>,
}
impl HostGrants {
    pub fn new(output: impl std::io::Write + 'static) -> Self {
        Self {
            readiness: None,
            io: None,
            output: Some(Box::new(output)),
        }
    }
    /// Interpreter admission for the cooperative scheduler: sockets and stdin are
    /// nonblocking and record their interest in one Readiness. Native callers keep
    /// the default refusal until their own task contexts are implemented.
    pub fn cooperative(output: impl std::io::Write + 'static) -> Self {
        Self {
            readiness: Some(Readiness::default()),
            ..Self::new(output)
        }
    }
    pub fn into_io(self) -> RealIo {
        self.io.expect("successful host admission")
    }
    /// The admitted io and the readiness its cooperative providers share.
    pub fn into_cooperative(self) -> (RealIo, Readiness) {
        let readiness = self.readiness.clone().expect("cooperative admission");
        (self.into_io(), readiness)
    }
}
impl HostAuthority for HostGrants {
    fn authorize(&mut self, record: &RunConfig) -> Result<(), RunRefusal> {
        if record.io.as_ref().is_some_and(|io| io.tasks.is_some()) && self.readiness.is_none() {
            return Err(RunRefusal::Unsupported {
                field: "io.tasks".into(),
            });
        }
        let mounts = record
            .io
            .as_ref()
            .into_iter()
            .flat_map(|io| &io.dirs)
            .map(|mount| {
                PinnedDirectory::open(std::path::Path::new(&mount.host))
                    .map(|pin| (mount.path.clone(), mount.access, pin))
                    .map_err(|error| RunRefusal::Unauthorized {
                        field: format!("io.dirs.{}: {error}", mount.path),
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
            match &self.readiness {
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
        if let Some(readiness) = &self.readiness {
            io.set_net_provider(CooperativeNet::new(readiness.clone()));
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
                io.set_net_script(peers.clone())
                    .map_err(|_| RunRefusal::Unsupported {
                        field: "io.net.peers".into(),
                    })?;
            }
        }
        self.io = Some(io);
        Ok(())
    }
    fn revocation(&mut self, reference: &str) -> Result<RevocationEvidence, RunRefusal> {
        Err(RunRefusal::MissingEvidence {
            reference: reference.into(),
        })
    }
}
