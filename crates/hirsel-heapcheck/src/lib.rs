//! The independent checked post-RC heap executor.
//! See README.md: invariants and allowed dependencies.
//!
//! [`run`] executes generated Rc/Reuse/Ready LIR over a separate ledger of
//! checked abstract allocations, then performs the terminal audit. It never
//! calls `hirsel-rt` and never reuses the interpreter's value or checked-mode
//! machinery. A [`Violation`] is a defect in the executed code; a language
//! failure is an ordinary observed outcome that is audited like a success.
pub mod build;
mod exec;
pub mod ledger;
pub mod observe;
pub mod program;
mod scalar;
mod snapshot;
pub mod violation;

pub use exec::{rc_trace, rc_trace_with_config};
pub use ledger::{Counters, Event, Exemption, History, Trace};
pub use observe::{Observation, Observed};
pub use program::{
    Arg, Config, INJECTED_RULE, NoOperations, OpOutcome, OpValue, OperationDomain, Operations,
    Program,
};
pub use violation::{Violation, ViolationKind};

use hirsel_lir::stage::Stage;
use hirsel_syntax::DefPath;

/// A successful terminal audit: what the program observably did, and every
/// named exemption that explains what it retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Audit {
    pub observation: Observation,
    pub exemptions: History<Exemption>,
}

/// One execution. `output` is what the program printed before it finished
/// or was stopped, so a test can show a run fails even when its output matches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Run {
    pub output: Vec<u8>,
    /// Failure-capable sites executed (the fault-injection domain).
    pub sites: u64,
    /// Executor steps completed or attempted, for diagnostic-localisation controls.
    pub steps: u64,
    pub trace: Trace,
    /// Instrumentation only: no check reads them.
    pub counters: Counters,
    pub result: Result<Audit, Violation>,
}

impl Run {
    /// The violation, for tests that expect one.
    pub fn violation(&self) -> Option<ViolationKind> {
        self.result.as_ref().err().map(|v| v.kind)
    }
}

/// Execute `entry` with harness `args`, then audit: observe the result, drop
/// harness roots, drain cleanup, and require zero unexplained allocations,
/// holders, tokens, open resources and handles, and live regions.
pub fn run<S: Stage>(
    program: &Program<S>,
    entry: &DefPath,
    args: &[Arg],
    ops: &mut dyn Operations,
    config: &Config,
) -> Run {
    let mut machine = exec::Machine::new(program, config, ops);
    let result = (|| {
        program.check()?;
        let entry = program.entry(entry)?;
        machine.start(entry, args)?;
        machine.run()?;
        let (observation, exemptions) = machine.finish()?;
        Ok(Audit {
            observation,
            exemptions,
        })
    })()
    .map_err(|v: Box<Violation>| *v);
    let result = blocked(result, &machine.output);
    Run {
        output: machine.output.clone(),
        sites: machine.sites,
        steps: machine.steps,
        trace: machine.ledger.trace.clone(),
        counters: machine.ledger.counters,
        result,
    }
}

/// CONC-8: reporting Blocked runs no user code, cancellation or destruction, so the blocked
/// tasks keep their storage and no terminal audit applies: a Blocked violation is the run's
/// observation.
fn blocked(result: Result<Audit, Violation>, output: &[u8]) -> Result<Audit, Violation> {
    match result {
        Err(v) if v.kind == ViolationKind::Blocked => Ok(Audit {
            observation: Observation {
                schema_version: observe::OBSERVATION_SCHEMA_VERSION,
                returned: None,
                output: output.to_vec(),
                failures: Vec::new(),
                blocked: true,
            },
            exemptions: Default::default(),
        }),
        result => result,
    }
}

/// A clean run, then one run per failure-capable site with that site failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Campaign {
    pub clean: Run,
    pub injected: Vec<(u64, Run)>,
}

impl Campaign {
    /// The first run whose audit failed, with its injected site.
    pub fn first_violation(&self) -> Option<(Option<u64>, &Violation)> {
        if let Err(v) = &self.clean.result {
            return Some((None, v));
        }
        self.injected
            .iter()
            .find_map(|(site, run)| run.result.as_ref().err().map(|v| (Some(*site), v)))
    }
}

/// `run` at Ready, the stage the driver executes, instantiated here once
/// rather than in every caller.
pub fn run_ready(
    program: &Program<hirsel_lir::stage::Ready>,
    entry: &DefPath,
    args: &[Arg],
    ops: &mut dyn Operations,
    config: &Config,
) -> Run {
    run(program, entry, args, ops, config)
}

/// Fault-inject at each deterministic failure-capable site the clean run
/// executed. Each injected run takes that site's failure edge and must pass
/// the terminal audit on the failure path.
pub fn fault_campaign<S: Stage>(
    program: &Program<S>,
    entry: &DefPath,
    args: &[Arg],
    ops: &mut dyn FnMut() -> Box<dyn Operations>,
    config: &Config,
) -> Campaign {
    let mut model = ops();
    if !config.reexecute_campaign
        && config.inject.is_none()
        && model.domain() == OperationDomain::Simulation
        && model.fork_simulation().is_some()
    {
        let campaign = exec::fork_campaign(program, entry, args, &mut *model, config);
        if config.compare_campaigns {
            let reference = fault_campaign(
                program,
                entry,
                args,
                ops,
                &Config {
                    reexecute_campaign: true,
                    compare_campaigns: false,
                    ..config.clone()
                },
            );
            assert_eq!(campaign, reference, "fork and replay campaigns disagree");
        }
        return campaign;
    }
    let clean = run(program, entry, args, &mut *model, config);
    let injected = (0..clean.sites)
        .map(|site| {
            let config = Config {
                inject: Some(site),
                ..config.clone()
            };
            (site, run(program, entry, args, &mut *ops(), &config))
        })
        .collect();
    Campaign { clean, injected }
}

/// `fault_campaign` at Ready, instantiated here once.
pub fn fault_campaign_ready(
    program: &Program<hirsel_lir::stage::Ready>,
    entry: &DefPath,
    args: &[Arg],
    ops: &mut dyn FnMut() -> Box<dyn Operations>,
    config: &Config,
) -> Campaign {
    fault_campaign(program, entry, args, ops, config)
}
