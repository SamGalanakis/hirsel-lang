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

pub enum CallDemand {
    /// All possible original targets at this call, including indirect targets.
    /// No clone-budget truncation is permitted in this independently built set.
    Bodies(Vec<usize>),
    /// Source operation expansion, including foreign reconstruction/adapters.
    /// An empty expansion means independently known empty, never unavailable.
    External(ExternalDemand),
}

#[derive(Default)]
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

struct Graph<'a> {
    bodies: &'a [DemandBody<'a>],
    offsets: Vec<usize>,
    edges: Vec<Vec<usize>>,
}

impl Graph<'_> {
    fn local(&self, body: usize, local: usize, inventory: bool) -> Result<usize, SourceError> {
        if local >= self.bodies[body].body.locals.len() {
            return Err(SourceError::Malformed("demand source local"));
        }
        Ok(self.offsets[body] + local * 2 + usize::from(inventory))
    }
    fn place(&self, body: usize, place: PlaceId, inventory: bool) -> Result<usize, SourceError> {
        let place = self.bodies[body]
            .body
            .places
            .get(place.0 as usize)
            .ok_or(SourceError::Malformed("demand source place"))?;
        self.local(body, place.root.0 as usize, inventory)
    }
    fn result(&self, body: usize, inventory: bool) -> usize {
        self.offsets[body] + self.bodies[body].body.locals.len() * 2 + usize::from(inventory)
    }
    fn edge(&mut self, from: usize, to: usize) {
        self.edges[from].push(to);
    }
    fn both(&mut self, a: usize, b: usize) {
        self.edge(a, b);
        self.edge(b, a);
    }
    fn values(
        &mut self,
        body: usize,
        destination: PlaceId,
        input: PlaceId,
    ) -> Result<(), SourceError> {
        let a = self.place(body, destination, false)?;
        let b = self.place(body, input, false)?;
        self.edge(a, b);
        // An Arena token may be forwarded inside a whole-value aggregate or
        // projected view. Inventory identity must follow both aliases.
        let a = self.place(body, destination, true)?;
        let b = self.place(body, input, true)?;
        self.both(a, b);
        Ok(())
    }
    fn capture_locals(&self, body: usize) -> Vec<usize> {
        self.bodies[body]
            .body
            .locals
            .iter()
            .enumerate()
            .filter_map(|(i, l)| (l.storage == Storage::Capture).then_some(i))
            .collect()
    }
    fn target(&self, target: usize) -> Result<(), SourceError> {
        if target >= self.bodies.len() {
            return Err(SourceError::Malformed("demand source target"));
        }
        Ok(())
    }
    fn scan(&mut self, body: usize) -> Result<(), SourceError> {
        let bodies = self.bodies;
        let source = bodies[body].body;
        for block in &source.blocks {
            for instruction in &block.instructions {
                match &instruction.kind {
                    InstructionKind::Assign { destination, value } => {
                        let rvalue = source
                            .rvalues
                            .get(value.0 as usize)
                            .ok_or(SourceError::Malformed("demand source rvalue"))?;
                        let mut inputs = Vec::new();
                        let allocation = match rvalue {
                            Rvalue::Use(value) | Rvalue::Unary { value, .. } => {
                                inputs.push(value.place);
                                None
                            }
                            Rvalue::Binary { left, right, .. } => {
                                inputs.extend([left.place, right.place]);
                                None
                            }
                            Rvalue::Construct {
                                fields,
                                constructor,
                                destination,
                            } => {
                                inputs.extend(fields.iter().map(|f| f.value.place));
                                if let hirsel_tir::Constructor::SortedMap { by } = constructor {
                                    inputs.push(by.place);
                                }
                                Some(destination)
                            }
                            Rvalue::Closure {
                                captures,
                                destination: allocation,
                                ..
                            } => {
                                let &target = bodies[body].closures.get(&value.0).ok_or(
                                    SourceError::Unsupported(
                                        "demand needs original closure instance",
                                    ),
                                )?;
                                self.target(target)?;
                                let locals = self.capture_locals(target);
                                if locals.len() != captures.len() {
                                    return Err(SourceError::Malformed(
                                        "demand source capture arity",
                                    ));
                                }
                                for (&local, capture) in locals.iter().zip(captures) {
                                    let input = self.place(body, capture.place, false)?;
                                    let formal = self.local(target, local, false)?;
                                    self.edge(formal, input);
                                    let environment = self.place(body, *destination, false)?;
                                    self.edge(formal, environment);
                                    if capture.usage == UseKind::Mut {
                                        self.edge(input, formal);
                                    }
                                    let input = self.place(body, capture.place, true)?;
                                    let formal = self.local(target, local, true)?;
                                    self.both(input, formal);
                                    inputs.push(capture.place);
                                }
                                Some(allocation)
                            }
                            Rvalue::Constant(_)
                            | Rvalue::ReadConstant { .. }
                            | Rvalue::FunctionReference { .. }
                            | Rvalue::Size(_)
                            | Rvalue::Shape(_) => None,
                        };
                        for &input in &inputs {
                            self.values(body, *destination, input)?;
                        }
                        if let Some(Destination::Arena(origin)) = allocation {
                            // Whole-source matching includes all aliases of this
                            // exact binder. No type-name or physical-domain guess.
                            let arenas: Vec<_> = source
                                .places
                                .iter()
                                .enumerate()
                                .filter(|(_, p)| p.binding.as_ref() == Some(origin))
                                .map(|(i, _)| PlaceId(i as u32))
                                .collect();
                            if arenas.is_empty() {
                                return Err(SourceError::Malformed(
                                    "demand arena destination lacks source binding",
                                ));
                            }
                            for arena in arenas {
                                let retained = self.place(body, arena, true)?;
                                let payload = self.place(body, *destination, false)?;
                                self.edge(retained, payload);
                            }
                        } else if let Some(Destination::Caller(_)) = allocation {
                            // Caller-selected storage requires the actual source
                            // destination mapping, not an invented heap placement.
                            return Err(SourceError::Unsupported(
                                "demand needs caller destination expansion",
                            ));
                        }
                    }
                    InstructionKind::Replace {
                        destination,
                        value,
                        displaced,
                        ..
                    } => {
                        self.values(body, *destination, value.place)?;
                        self.values(body, *displaced, *destination)?;
                    }
                    InstructionKind::Snapshot { destination, value } => {
                        self.values(body, *destination, value.place)?;
                    }
                    // Binding metadata, loans, cleanup and liveness do not
                    // remove source-history requirements. In particular a Drop
                    // cannot erase a retained semantic allocation.
                    _ => (),
                }
            }
        }
        for (block, source_block) in source.blocks.iter().enumerate() {
            match &source_block.terminator {
                Terminator::Return { value, .. } => {
                    let output = self.result(body, false);
                    let input = self.place(body, value.place, false)?;
                    self.edge(output, input);
                    let output = self.result(body, true);
                    let input = self.place(body, value.place, true)?;
                    self.both(output, input);
                }
                Terminator::Call {
                    callee,
                    arguments,
                    returns,
                    ..
                } => {
                    let expansion =
                        bodies[body]
                            .calls
                            .get(&(block as u32))
                            .ok_or(SourceError::Unsupported(
                                "demand needs complete original call expansion",
                            ))?;
                    let mut actuals = BTreeMap::new();
                    for argument in arguments {
                        if actuals
                            .insert(argument.parameter.0, &argument.value)
                            .is_some()
                        {
                            return Err(SourceError::Malformed("demand duplicate source argument"));
                        }
                    }
                    let actual = |ordinal: u32| {
                        actuals
                            .get(&ordinal)
                            .copied()
                            .ok_or(SourceError::Malformed("demand source argument selector"))
                    };
                    match expansion {
                        CallDemand::Bodies(targets) => {
                            if targets.is_empty() {
                                return Err(SourceError::Unsupported(
                                    "demand has no original call targets",
                                ));
                            }
                            for &target in targets {
                                self.target(target)?;
                                let parameters = &bodies[target].body.parameters;
                                if parameters.len() != actuals.len() {
                                    return Err(SourceError::Malformed("demand source call arity"));
                                }
                                for (ordinal, local) in parameters.iter().enumerate() {
                                    let value = actual(ordinal as u32)?;
                                    let formal = self.local(target, local.0 as usize, false)?;
                                    let input = self.place(body, value.place, false)?;
                                    self.edge(formal, input);
                                    if value.usage == UseKind::Mut {
                                        self.edge(input, formal);
                                    }
                                    let formal = self.local(target, local.0 as usize, true)?;
                                    let input = self.place(body, value.place, true)?;
                                    self.both(formal, input);
                                }
                                for local in self.capture_locals(target) {
                                    let Callee::Indirect { value, .. } = callee else {
                                        return Err(SourceError::Malformed(
                                            "demand direct target has captures",
                                        ));
                                    };
                                    let formal = self.local(target, local, false)?;
                                    let environment = self.place(body, value.place, false)?;
                                    self.edge(formal, environment);
                                    if value.usage == UseKind::Mut {
                                        self.edge(environment, formal);
                                    }
                                    let formal = self.local(target, local, true)?;
                                    let environment = self.place(body, value.place, true)?;
                                    self.both(formal, environment);
                                }
                                if let Some(returned) = returns {
                                    let output = self.place(body, returned.destination, false)?;
                                    self.edge(output, self.result(target, false));
                                    let output = self.place(body, returned.destination, true)?;
                                    self.both(output, self.result(target, true));
                                }
                            }
                        }
                        CallDemand::External(summary) => {
                            if matches!(callee, Callee::Indirect { .. }) {
                                return Err(SourceError::Unsupported(
                                    "demand needs indirect source targets and environment",
                                ));
                            }
                            for &p in &summary.parameters {
                                let input = self.place(body, actual(p)?.place, false)?;
                                self.edge(0, input);
                            }
                            for &p in &summary.inventory {
                                let input = self.place(body, actual(p)?.place, true)?;
                                self.edge(0, input);
                            }
                            if let Some(returned) = returns {
                                for &p in &summary.result {
                                    self.values(body, returned.destination, actual(p)?.place)?;
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
                                    self.values(body, output.place, actual(input)?.place)?;
                                }
                            }
                            for (grant, inputs) in &summary.retained {
                                let inventory = self.place(body, actual(*grant)?.place, true)?;
                                for &input in inputs {
                                    let payload = self.place(body, actual(input)?.place, false)?;
                                    self.edge(inventory, payload);
                                }
                            }
                        }
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }
}

/// Least conservative whole-value fixed point for this complete source closure.
/// Each body is one base source instance; all demanded uses share its union.
/// No source/V identity, spans, physical layouts or derived keys enter output.
/// Unavailable call/closure/destination expansion is an error, never empty.
pub fn analyze(bodies: &[DemandBody<'_>]) -> Result<Vec<Requirements>, SourceError> {
    let mut offsets = Vec::new();
    let mut nodes = 1; // sole root for all independently supplied sink effects
    for source in bodies {
        offsets.push(nodes);
        nodes += source.body.locals.len() * 2 + 2;
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
            for local in 0..source.body.locals.len() {
                if demanded[graph
                    .local(body, local, false)
                    .expect("scanned source local")]
                {
                    requirements.payload_locals.insert(local as u32);
                }
                if demanded[graph
                    .local(body, local, true)
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

#[cfg(test)]
mod tests;
