//! Production PROG-6 admission shared by source and native launch adapters.
use crate::{host_paths::PinnedDirectory, io_real::RealIo, net::NetRule};
use hirsel_run_config::{HostAuthority, RevocationEvidence, RunConfig, RunRefusal};

pub struct HostGrants {
    io: Option<RealIo>,
    output: Option<Box<dyn std::io::Write>>,
}
impl HostGrants {
    pub fn new(output: impl std::io::Write + 'static) -> Self {
        Self {
            io: None,
            output: Some(Box::new(output)),
        }
    }
    pub fn into_io(self) -> RealIo {
        self.io.expect("successful host admission")
    }
}
impl HostAuthority for HostGrants {
    fn authorize(&mut self, record: &RunConfig) -> Result<(), RunRefusal> {
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
