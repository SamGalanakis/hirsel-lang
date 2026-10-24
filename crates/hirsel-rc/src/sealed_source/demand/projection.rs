//! Finite dependency data captured while the original source body is available.
//! No Body/DefinitionRef, candidate layout, derived instance or source proof is
//! retained. Original type actuals in callees remain session-local query data.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceOperand {
    pub local: u32,
    pub usage: UseKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceCall {
    /// Original target identity/actuals, never selected from candidate LIR.
    pub callee: Callee,
    pub arguments: BTreeMap<u32, SourceOperand>,
    pub result: Option<u32>,
    pub environment: Option<SourceOperand>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceClosure {
    pub body: hirsel_tir::BodyId,
    pub result: u32,
    pub captures: Vec<SourceOperand>,
}

/// Compact original-body projection. Numeric nodes/ports are scoped by the
/// actual original base instance and its independently observed source identity.
/// This is dependency data, not evidence that target expansion is complete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodyProjection {
    pub(super) locals: usize,
    pub(super) parameters: Vec<u32>,
    pub(super) captures: Vec<u32>,
    pub(super) edges: Vec<Vec<usize>>,
    calls: BTreeMap<u32, SourceCall>,
    closures: BTreeMap<u32, Vec<SourceClosure>>,
}

impl BodyProjection {
    pub fn calls(&self) -> &BTreeMap<u32, SourceCall> {
        &self.calls
    }
    pub fn closures(&self) -> &BTreeMap<u32, Vec<SourceClosure>> {
        &self.closures
    }
    pub fn parameters(&self) -> &[u32] {
        &self.parameters
    }
    pub fn captures(&self) -> &[u32] {
        &self.captures
    }
    pub fn local_count(&self) -> usize {
        self.locals
    }

    fn local(&self, local: u32, inventory: bool) -> Result<usize, SourceError> {
        if local as usize >= self.locals {
            return Err(SourceError::Malformed("demand source local"));
        }
        Ok(local as usize * 2 + usize::from(inventory))
    }
    fn result(&self, inventory: bool) -> usize {
        self.locals * 2 + usize::from(inventory)
    }
    fn values(&mut self, destination: u32, input: u32) -> Result<(), SourceError> {
        let a = self.local(destination, false)?;
        let b = self.local(input, false)?;
        self.edges[a].push(b);
        let a = self.local(destination, true)?;
        let b = self.local(input, true)?;
        self.edges[a].push(b);
        self.edges[b].push(a);
        Ok(())
    }

    /// Project one actual original TIR body before restoration/normalization.
    /// Calls/closures keep original selectors; linking independently supplied
    /// targets happens later without retaining this body's executable IR.
    pub fn capture(source: &Body) -> Result<Self, SourceError> {
        let local = |place: PlaceId| {
            let place = source
                .places
                .get(place.0 as usize)
                .ok_or(SourceError::Malformed("demand source place"))?;
            if place.root.0 as usize >= source.locals.len() {
                return Err(SourceError::Malformed("demand source local"));
            }
            Ok(place.root.0)
        };
        let operand = |value: &hirsel_tir::Operand| -> Result<SourceOperand, SourceError> {
            Ok(SourceOperand {
                local: local(value.place)?,
                usage: value.usage,
            })
        };
        let mut out = Self {
            locals: source.locals.len(),
            parameters: source.parameters.iter().map(|local| local.0).collect(),
            captures: source
                .locals
                .iter()
                .enumerate()
                .filter_map(|(i, local)| (local.storage == Storage::Capture).then_some(i as u32))
                .collect(),
            edges: vec![vec![]; source.locals.len() * 2 + 2],
            calls: BTreeMap::new(),
            closures: BTreeMap::new(),
        };
        for &parameter in &out.parameters {
            out.local(parameter, false)?;
        }
        for (block, source_block) in source.blocks.iter().enumerate() {
            for instruction in &source_block.instructions {
                match &instruction.kind {
                    InstructionKind::Assign { destination, value } => {
                        let destination = local(*destination)?;
                        let rvalue = source
                            .rvalues
                            .get(value.0 as usize)
                            .ok_or(SourceError::Malformed("demand source rvalue"))?;
                        let mut inputs = Vec::new();
                        let allocation = match rvalue {
                            Rvalue::Use(value) | Rvalue::Unary { value, .. } => {
                                inputs.push(local(value.place)?);
                                None
                            }
                            Rvalue::Binary { left, right, .. } => {
                                inputs.extend([local(left.place)?, local(right.place)?]);
                                None
                            }
                            Rvalue::Construct {
                                fields,
                                constructor,
                                destination,
                            } => {
                                for field in fields {
                                    inputs.push(local(field.value.place)?);
                                }
                                if let hirsel_tir::Constructor::SortedMap { by } = constructor {
                                    inputs.push(local(by.place)?);
                                }
                                Some(destination)
                            }
                            Rvalue::Closure {
                                body,
                                captures,
                                destination: allocation,
                                ..
                            } => {
                                let captures = captures
                                    .iter()
                                    .map(operand)
                                    .collect::<Result<Vec<_>, _>>()?;
                                inputs.extend(captures.iter().map(|capture| capture.local));
                                let closure = SourceClosure {
                                    body: *body,
                                    result: destination,
                                    captures,
                                };
                                out.closures.entry(value.0).or_default().push(closure);
                                Some(allocation)
                            }
                            Rvalue::Constant(_)
                            | Rvalue::ReadConstant { .. }
                            | Rvalue::FunctionReference { .. }
                            | Rvalue::Size(_)
                            | Rvalue::Shape(_) => None,
                        };
                        for input in inputs {
                            out.values(destination, input)?;
                        }
                        if let Some(Destination::Arena(origin)) = allocation {
                            let arenas = source
                                .places
                                .iter()
                                .enumerate()
                                .filter(|(_, place)| place.binding.as_ref() == Some(origin))
                                .map(|(i, _)| local(PlaceId(i as u32)))
                                .collect::<Result<Vec<_>, _>>()?;
                            if arenas.is_empty() {
                                return Err(SourceError::Malformed(
                                    "demand arena destination lacks source binding",
                                ));
                            }
                            let payload = out.local(destination, false)?;
                            for arena in arenas {
                                let retained = out.local(arena, true)?;
                                out.edges[retained].push(payload);
                            }
                        } else if let Some(Destination::Caller(_)) = allocation {
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
                        out.values(local(*destination)?, local(value.place)?)?;
                        out.values(local(*displaced)?, local(*destination)?)?;
                    }
                    InstructionKind::Snapshot { destination, value } => {
                        out.values(local(*destination)?, local(value.place)?)?;
                    }
                    // Cleanup and lexical liveness never erase retained history.
                    _ => (),
                }
            }
            match &source_block.terminator {
                Terminator::Return { value, .. } => {
                    let input = local(value.place)?;
                    let a = out.result(false);
                    let b = out.local(input, false)?;
                    out.edges[a].push(b);
                    let a = out.result(true);
                    let b = out.local(input, true)?;
                    out.edges[a].push(b);
                    out.edges[b].push(a);
                }
                Terminator::Call {
                    callee,
                    arguments,
                    returns,
                    ..
                } => {
                    let mut actuals = BTreeMap::new();
                    for argument in arguments {
                        if actuals
                            .insert(argument.parameter.0, operand(&argument.value)?)
                            .is_some()
                        {
                            return Err(SourceError::Malformed("demand duplicate source argument"));
                        }
                    }
                    let environment = match callee {
                        Callee::Indirect { value, .. } => Some(operand(value)?),
                        _ => None,
                    };
                    out.calls.insert(
                        block as u32,
                        SourceCall {
                            callee: callee.clone(),
                            arguments: actuals,
                            result: returns
                                .as_ref()
                                .map(|returned| local(returned.destination))
                                .transpose()?,
                            environment,
                        },
                    );
                }
                _ => (),
            }
        }
        // Edges are sets; canonical order makes projection equality independent
        // of repeated source occurrences. Recipes keep occurrence multiplicity.
        for edges in &mut out.edges {
            edges.sort_unstable();
            edges.dedup();
        }
        Ok(out)
    }
}
