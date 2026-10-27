//! Conservative whole-value receipt demand over original source instances.
//!
//! Each local has payload and retained-inventory selectors. All original
//! definitions of a local are unioned, including CFG alternatives and committed
//! mutations on entered failure. This deliberately widens projections and
//! versions; it never erases an obligation at a join or specialization limit.
//! Cycles use hirsel_graph's iterative reachability, with no iteration cutoff.
//!
//! Call/closure expansion is independent source-query input, not candidate LIR.
//! This analysis produces requirements only: it neither validates those inputs
//! nor constructs a ReceiptContract or a source-observation authority.
use super::SourceError;
use hirsel_tir::{Body, Callee, InstructionKind, PlaceId, Rvalue, Storage, Terminator};
use hirsel_types::{Destination, UseKind};
use std::collections::{BTreeMap, BTreeSet};

/// Original block -> complete source call expansion. An absent row is unknown,
/// including when the candidate would otherwise have no demanded output.
pub struct DemandBody<'a> {
    pub body: &'a Body,
    pub calls: BTreeMap<u32, CallDemand>,
    /// Original closure RvalueId -> original specialized body in this program.
    pub closures: BTreeMap<u32, usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallDemand {
    /// All possible original targets at this call, including indirect targets.
    /// No clone-budget truncation is permitted in this independently built set.
    Bodies(Vec<usize>),
    /// Reviewed arena scope supplies the callback's Arena parameter. The
    /// callback operand carries the scope's internal journal requirement;
    /// it is not a source Arena value or a physical environment recipe.
    ArenaScope { callback: u32, targets: Vec<usize> },
    /// Reviewed task callbacks receive either a fresh Scope or no parameter.
    /// Captures and results retain provenance across the task boundary.
    TaskCallback {
        callback: u32,
        scoped: bool,
        targets: Vec<usize>,
    },
    /// A reviewed Region or Shared read lends the owner's payload to the callback; it
    /// creates no Arena grant and does not transfer the owner's inventory.
    RegionRead {
        owner: u32,
        callback: u32,
        targets: Vec<usize>,
    },
    /// Source operation expansion, including foreign reconstruction/adapters.
    /// An empty expansion means independently known empty, never unavailable.
    External(ExternalDemand),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExternalDemand {
    /// Unconditional payload requirements (for example, a seal root).
    pub parameters: Vec<u32>,
    /// Consumed grants whose complete retained histories are required.
    pub inventory: Vec<u32>,
    /// Original inputs contributing to the ordinary result's provenance.
    pub result: Vec<u32>,
    /// Mutable output parameter -> contributing original input parameters.
    /// These dependencies apply on entered failure as well as normal return.
    pub mutations: Vec<(u32, Vec<u32>)>,
    /// Destination grant parameter -> retained payload input parameters.
    /// Includes scalar/unit-returning allocation and mutation operations.
    pub retained: Vec<(u32, Vec<u32>)>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Requirements {
    pub payload_locals: BTreeSet<u32>,
    pub inventory_locals: BTreeSet<u32>,
    pub result: bool,
    pub result_inventory: bool,
}

mod projection;
pub use projection::{BodyProjection, SourceCall, SourceClosure, SourceOperand};

/// Per-instance projection plus independently selected original target expansion.
/// Absence remains unavailable; caller-selected/unknown targets never become
/// an empty contract just because the executable body has been released.
pub struct CompactBody<'a> {
    pub source: &'a BodyProjection,
    pub calls: &'a BTreeMap<u32, CallDemand>,
    pub closures: &'a BTreeMap<u32, usize>,
}

struct Graph<'a> {
    bodies: &'a [CompactBody<'a>],
    offsets: Vec<usize>,
    edges: Vec<Vec<usize>>,
}

impl Graph<'_> {
    fn local(&self, body: usize, local: u32, inventory: bool) -> Result<usize, SourceError> {
        if local as usize >= self.bodies[body].source.locals {
            return Err(SourceError::Malformed("demand source local"));
        }
        Ok(self.offsets[body] + local as usize * 2 + usize::from(inventory))
    }
    fn result(&self, body: usize, inventory: bool) -> usize {
        self.offsets[body] + self.bodies[body].source.locals * 2 + usize::from(inventory)
    }
    fn edge(&mut self, from: usize, to: usize) {
        self.edges[from].push(to);
    }
    fn both(&mut self, a: usize, b: usize) {
        self.edge(a, b);
        self.edge(b, a);
    }
    fn values(&mut self, body: usize, destination: u32, input: u32) -> Result<(), SourceError> {
        let a = self.local(body, destination, false)?;
        let b = self.local(body, input, false)?;
        self.edge(a, b);
        let a = self.local(body, destination, true)?;
        let b = self.local(body, input, true)?;
        self.both(a, b);
        Ok(())
    }
    fn target(&self, target: usize) -> Result<(), SourceError> {
        if target >= self.bodies.len() {
            return Err(SourceError::Malformed("demand source target"));
        }
        Ok(())
    }
    fn scan(&mut self, body: usize) -> Result<(), SourceError> {
        let bodies = self.bodies;
        let source = bodies[body].source;
        for (from, edges) in source.edges.iter().enumerate() {
            for &to in edges {
                self.edge(self.offsets[body] + from, self.offsets[body] + to);
            }
        }
        for (value, instances) in source.closures() {
            for closure in instances {
                let &target = bodies[body]
                    .closures
                    .get(value)
                    .ok_or(SourceError::Unsupported(
                        "demand needs original closure instance",
                    ))?;
                self.target(target)?;
                let captures = &bodies[target].source.captures;
                if captures.len() != closure.captures.len() {
                    return Err(SourceError::Malformed("demand source capture arity"));
                }
                for (&local, capture) in captures.iter().zip(&closure.captures) {
                    let input = self.local(body, capture.local, false)?;
                    let formal = self.local(target, local, false)?;
                    self.edge(formal, input);
                    let environment = self.local(body, closure.result, false)?;
                    self.edge(formal, environment);
                    if capture.usage == UseKind::Mut {
                        self.edge(input, formal);
                    }
                    let input = self.local(body, capture.local, true)?;
                    let formal = self.local(target, local, true)?;
                    self.both(input, formal);
                }
            }
        }
        for (block, call) in source.calls() {
            let expansion = bodies[body]
                .calls
                .get(block)
                .ok_or(SourceError::Unsupported(
                    "demand needs complete original call expansion",
                ))?;
            let actual = |ordinal: u32| {
                call.arguments
                    .get(&ordinal)
                    .ok_or(SourceError::Malformed("demand source argument selector"))
            };
            match expansion {
                CallDemand::ArenaScope { callback, targets }
                | CallDemand::TaskCallback {
                    callback, targets, ..
                }
                | CallDemand::RegionRead {
                    callback, targets, ..
                } => {
                    if targets.is_empty() || call.environment.is_some() {
                        return Err(SourceError::Unsupported(
                            "demand arena scope needs complete original callbacks",
                        ));
                    }
                    let callback = actual(*callback)?;
                    for &target in targets {
                        self.target(target)?;
                        let parameters = bodies[target].source.parameters.as_slice();
                        let expected = match expansion {
                            CallDemand::TaskCallback { scoped, .. } => usize::from(*scoped),
                            _ => 1,
                        };
                        if parameters.len() != expected {
                            return Err(SourceError::Malformed("demand arena callback arity"));
                        }
                        // A demanded grant journal must reach the source scope
                        // call even when the callback has no captured payload.
                        if let CallDemand::RegionRead { owner, .. } = expansion {
                            let root = self.local(target, parameters[0], false)?;
                            let owner = self.local(body, actual(*owner)?.local, false)?;
                            self.edge(root, owner);
                        } else if matches!(expansion, CallDemand::ArenaScope { .. }) {
                            let grant = self.local(target, parameters[0], true)?;
                            let scope = self.local(body, callback.local, false)?;
                            self.edge(grant, scope);
                        }
                        for &local in &bodies[target].source.captures {
                            let formal = self.local(target, local, false)?;
                            let input = self.local(body, callback.local, false)?;
                            self.edge(formal, input);
                            if callback.usage == UseKind::Mut {
                                self.edge(input, formal);
                            }
                            let formal = self.local(target, local, true)?;
                            let input = self.local(body, callback.local, true)?;
                            self.both(formal, input);
                        }
                        if let Some(returned) = call.result {
                            let output = self.local(body, returned, false)?;
                            self.edge(output, self.result(target, false));
                            let output = self.local(body, returned, true)?;
                            self.both(output, self.result(target, true));
                        }
                    }
                }
                CallDemand::Bodies(targets) => {
                    if targets.is_empty() {
                        return Err(SourceError::Unsupported(
                            "demand has no original call targets",
                        ));
                    }
                    for &target in targets {
                        self.target(target)?;
                        let parameters = &bodies[target].source.parameters;
                        if parameters.len() != call.arguments.len() {
                            return Err(SourceError::Malformed("demand source call arity"));
                        }
                        for (ordinal, &local) in parameters.iter().enumerate() {
                            let value = actual(ordinal as u32)?;
                            let formal = self.local(target, local, false)?;
                            let input = self.local(body, value.local, false)?;
                            self.edge(formal, input);
                            if value.usage == UseKind::Mut {
                                self.edge(input, formal);
                            }
                            let formal = self.local(target, local, true)?;
                            let input = self.local(body, value.local, true)?;
                            self.both(formal, input);
                        }
                        for &local in &bodies[target].source.captures {
                            let environment = call.environment.as_ref().ok_or(
                                SourceError::Malformed("demand direct target has captures"),
                            )?;
                            let formal = self.local(target, local, false)?;
                            let input = self.local(body, environment.local, false)?;
                            self.edge(formal, input);
                            if environment.usage == UseKind::Mut {
                                self.edge(input, formal);
                            }
                            let formal = self.local(target, local, true)?;
                            let input = self.local(body, environment.local, true)?;
                            self.both(formal, input);
                        }
                        if let Some(returned) = call.result {
                            let output = self.local(body, returned, false)?;
                            self.edge(output, self.result(target, false));
                            let output = self.local(body, returned, true)?;
                            self.both(output, self.result(target, true));
                        }
                    }
                }
                CallDemand::External(summary) => {
                    if call.environment.is_some() {
                        return Err(SourceError::Unsupported(
                            "demand needs indirect source targets and environment",
                        ));
                    }
                    for &p in &summary.parameters {
                        let input = self.local(body, actual(p)?.local, false)?;
                        self.edge(0, input);
                    }
                    for &p in &summary.inventory {
                        let input = self.local(body, actual(p)?.local, true)?;
                        self.edge(0, input);
                    }
                    if let Some(returned) = call.result {
                        for &p in &summary.result {
                            self.values(body, returned, actual(p)?.local)?;
                        }
                    } else if !summary.result.is_empty() {
                        return Err(SourceError::Malformed(
                            "demand result on diverging source call",
                        ));
                    }
                    for (output, inputs) in &summary.mutations {
                        let output = actual(*output)?;
                        if output.usage != UseKind::Mut {
                            return Err(SourceError::Malformed(
                                "demand mutation lacks mut source argument",
                            ));
                        }
                        for &input in inputs {
                            self.values(body, output.local, actual(input)?.local)?;
                        }
                    }
                    for (grant, inputs) in &summary.retained {
                        let inventory = self.local(body, actual(*grant)?.local, true)?;
                        for &input in inputs {
                            let payload = self.local(body, actual(input)?.local, false)?;
                            self.edge(inventory, payload);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Analyze only captured dependency data. No executable body/DefinitionRef is
/// retained while assembling or solving the complete source instance graph.
/// Source/V observations and complete target expansions remain independent
/// query-owner inputs; this function never constructs authority or a contract.
pub fn analyze_compact(bodies: &[CompactBody<'_>]) -> Result<Vec<Requirements>, SourceError> {
    let mut offsets = Vec::new();
    let mut nodes = 1;
    for source in bodies {
        offsets.push(nodes);
        nodes += source.source.locals * 2 + 2;
    }
    let mut graph = Graph {
        bodies,
        offsets,
        edges: vec![Vec::new(); nodes],
    };
    for body in 0..bodies.len() {
        graph.scan(body)?;
    }
    let demanded = hirsel_graph::reachable(&graph.edges, 0);
    Ok(bodies
        .iter()
        .enumerate()
        .map(|(body, source)| {
            let mut requirements = Requirements::default();
            for local in 0..source.source.locals {
                if demanded[graph
                    .local(body, local as u32, false)
                    .expect("scanned source local")]
                {
                    requirements.payload_locals.insert(local as u32);
                }
                if demanded[graph
                    .local(body, local as u32, true)
                    .expect("scanned source local")]
                {
                    requirements.inventory_locals.insert(local as u32);
                }
            }
            requirements.result = demanded[graph.result(body, false)];
            requirements.result_inventory = demanded[graph.result(body, true)];
            requirements
        })
        .collect())
}

/// Convenience adapter while an original body closure is already borrowed.
/// Both entry points use the same projection and compact fixed-point solver.
pub fn analyze(bodies: &[DemandBody<'_>]) -> Result<Vec<Requirements>, SourceError> {
    let projections = bodies
        .iter()
        .map(|body| BodyProjection::capture(body.body))
        .collect::<Result<Vec<_>, _>>()?;
    let compact = bodies
        .iter()
        .zip(&projections)
        .map(|(body, source)| CompactBody {
            source,
            calls: &body.calls,
            closures: &body.closures,
        })
        .collect::<Vec<_>>();
    analyze_compact(&compact)
}

#[cfg(test)]
mod tests;
