//! Host-owned launch selection and binding over immutable checked source facts.
//! Reading a tape selects its entry before compiler roots are requested. Binding
//! grants or a recording never needs a retained mutable compiler program.
use crate::{BoundHostRun, HostLaunch, ToolError};
use hirsel_run_schema::{CodeReach, LaunchKind, ProgramRevision};
use hirsel_syntax::DefPath;
use hirsel_types::{Signature, TypeInterner};

pub(crate) struct LaunchSelection {
    entry: Option<DefPath>,
    replay: Option<(hirsel_rt::io_sim::SimIo, hirsel_run_config::ValidatedRun)>,
}

fn refused(error: hirsel_run_schema::RunRefusal) -> ToolError {
    ToolError::Runtime(format!("replay refused: {error}"))
}

impl LaunchSelection {
    pub(crate) fn read(launch: HostLaunch<'_>) -> Result<Self, ToolError> {
        let (entry, replay) =
            match launch {
                HostLaunch::Replay(tape) => {
                    let (io, run) = hirsel_rt::host_launch::replay(tape).map_err(refused)?;
                    let entry = if run.record().launch == LaunchKind::Call {
                        Some(crate::def_path(&run.record().entry).ok_or_else(|| {
                            ToolError::Runtime("replay refused: the callee entry's path".into())
                        })?)
                    } else {
                        None
                    };
                    (entry, Some((io, run)))
                }
                HostLaunch::Callee { run, .. } => (
                    Some(crate::def_path(&run.record().entry).ok_or_else(|| {
                        ToolError::Runtime("launch: the callee entry's path".into())
                    })?),
                    None,
                ),
                HostLaunch::Live { .. } => (None, None),
            };
        Ok(Self { entry, replay })
    }

    pub(crate) fn entry(&self) -> Option<&DefPath> {
        self.entry.as_ref()
    }

    pub(crate) fn requires_replayable(&self, launch: HostLaunch<'_>) -> bool {
        self.replay.is_some()
            || matches!(
                launch,
                HostLaunch::Live {
                    record: Some(_),
                    ..
                } | HostLaunch::Callee {
                    record: Some(_),
                    ..
                }
            )
    }

    pub(crate) fn bind(
        self,
        launch: HostLaunch<'_>,
        types: &TypeInterner,
        signature: &Signature,
        revision: &ProgramRevision,
        code: CodeReach,
    ) -> Result<BoundHostRun, ToolError> {
        let kind = match (&self.replay, launch) {
            (Some((_, run)), _) if run.record().launch == LaunchKind::Call => LaunchKind::Call,
            (_, HostLaunch::Callee { .. }) => LaunchKind::Call,
            _ => hirsel_engine::entry_launch_kind(types, signature).map_err(ToolError::Launch)?,
        };
        match (launch, self.replay) {
            (HostLaunch::Live { config, .. }, _) => {
                let requested = config
                    .read(revision, kind)
                    .map_err(|e| ToolError::Runtime(e.to_string()))?;
                let bound = hirsel_engine::bind_run(
                    types,
                    signature,
                    revision,
                    kind,
                    Some(&requested),
                    code,
                )
                .map_err(ToolError::Launch)?;
                Ok(BoundHostRun::Live(bound))
            }
            (HostLaunch::Callee { run, .. }, _) => {
                let bound =
                    hirsel_engine::bind_run(types, signature, revision, kind, Some(run), code)
                        .map_err(ToolError::Launch)?;
                Ok(BoundHostRun::Live(bound))
            }
            (HostLaunch::Replay(_), Some((io, run))) => {
                let roots = hirsel_engine::entry_roots(types, signature, kind).map_err(refused)?;
                let run = run
                    .bind(&hirsel_run_config::ProgramFacts::of(
                        revision, kind, roots, code,
                    ))
                    .map_err(refused)?
                    .into_run();
                Ok(BoundHostRun::Replay { run, io })
            }
            (HostLaunch::Replay(_), None) => unreachable!("a replay reads its tape first"),
        }
    }
}
