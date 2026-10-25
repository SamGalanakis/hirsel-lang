//! Code selection from original source definitions, independent of candidate
//! LIR, physical callable signatures or profile hashes. No guessed cases.
use super::{ClosureInput, NamedContext, SourceError, provider};
use hirsel_layout::instance::InstanceKey;
use hirsel_syntax::DefPath;
use hirsel_tir::{Body, Callee, InstructionKind, PlaceId, Rvalue, Storage, Terminator};
use hirsel_types::{GenericArgument, UseKind};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Target {
    Definition {
        path: DefPath,
        arguments: Vec<GenericArgument>,
    },
    Closure(u32),
}
#[derive(Clone)]
enum Definition {
    Unknown,
    Alias(u32),
    Code(Target),
}
pub(super) struct Code {
    definitions: Vec<Vec<Definition>>,
    calls: BTreeMap<u32, Callee>,
    indirect: BTreeMap<u32, u32>,
    closures: BTreeMap<u32, ClosureInput>,
    projected_arguments: HashSet<(u32, u32)>,
}
fn root(body: &Body, place: PlaceId) -> Result<usize, SourceError> {
    let place = body
        .places
        .get(place.0 as usize)
        .ok_or(SourceError::Malformed("receipt code source place"))?;
    if place.root.0 as usize >= body.locals.len() {
        return Err(SourceError::Malformed("receipt code source local"));
    }
    Ok(place.root.0 as usize)
}
fn local(body: &Body, place: PlaceId) -> Result<usize, SourceError> {
    let root = root(body, place)?;
    if !body.places[place.0 as usize].projection.is_empty() {
        return Err(SourceError::Unsupported(
            "receipt code selection needs original projected producer expansion",
        ));
    }
    Ok(root)
}
pub(super) fn capture(
    provider: &dyn provider::Provider,
    context: &NamedContext,
) -> Result<Code, SourceError> {
    let (_, original) = provider::original(provider, context.input.base.definition())?;
    if original
        .output()
        .map(|output| output.observation_identity())
        != Some(context.source.entry().source_identity())
        || original.implementation_revision() != context.source.entry().implementation_revision()
    {
        return Err(SourceError::Malformed(
            "receipt code source changed during context assembly",
        ));
    }
    let body = original
        .definition()
        .bodies
        .get(context.input.base.body() as usize)
        .ok_or(SourceError::Malformed("receipt code original body"))?;
    let mut code = Code {
        definitions: vec![vec![]; body.locals.len()],
        calls: BTreeMap::new(),
        indirect: BTreeMap::new(),
        closures: BTreeMap::new(),
        projected_arguments: HashSet::new(),
    };
    for (index, value) in body.locals.iter().enumerate() {
        if matches!(value.storage, Storage::Parameter | Storage::Capture) {
            code.definitions[index].push(Definition::Unknown);
        }
    }
    for (block_index, block) in body.blocks.iter().enumerate() {
        for (statement, instruction) in block.instructions.iter().enumerate() {
            match instruction.kind {
                InstructionKind::Assign { destination, value } => {
                    // Projected non-callable storage does not define code for
                    // the root. Any later projected code read remains refused.
                    let place = body
                        .places
                        .get(destination.0 as usize)
                        .ok_or(SourceError::Malformed("receipt code destination"))?;
                    let target = place.root.0 as usize;
                    let row = code
                        .definitions
                        .get_mut(target)
                        .ok_or(SourceError::Malformed("receipt code destination root"))?;
                    if !place.projection.is_empty() {
                        row.push(Definition::Unknown);
                        continue;
                    }
                    let rvalue = body
                        .rvalues
                        .get(value.0 as usize)
                        .ok_or(SourceError::Malformed("receipt code rvalue"))?;
                    let definition = match rvalue {
                        Rvalue::Use(operand) => {
                            let input = root(body, operand.place)?;
                            if body.places[operand.place.0 as usize].projection.is_empty() {
                                Definition::Alias(input as u32)
                            } else {
                                // A field read need not produce code. Retain an
                                // unknown producer and refuse only if a call
                                // actually depends on it.
                                Definition::Unknown
                            }
                        }
                        Rvalue::FunctionReference {
                            definition,
                            arguments,
                        } => Definition::Code(Target::Definition {
                            path: definition.clone(),
                            arguments: arguments.clone(),
                        }),
                        Rvalue::Closure { body: target, .. } => {
                            if !context.input.actuals.arguments.is_empty()
                                || !context.input.actuals.outer.is_empty()
                            {
                                return Err(SourceError::Unsupported(
                                    "receipt closure graph needs complete enclosing actuals",
                                ));
                            }
                            let input = ClosureInput {
                                base: InstanceKey::new(
                                    context.input.base.definition(),
                                    target.0,
                                    &[],
                                    provider.types(),
                                    None,
                                    None,
                                )
                                .map_err(|error| SourceError::Type(format!("{error:?}")))?,
                                parent: hirsel_tir::BodyId(context.input.base.body()),
                                block: hirsel_tir::BlockId(block_index as u32),
                                statement: statement as u32,
                            };
                            if code.closures.insert(value.0, input).is_some() {
                                return Err(SourceError::Unsupported(
                                    "receipt closure has multiple original assignments",
                                ));
                            }
                            Definition::Code(Target::Closure(value.0))
                        }
                        _ => Definition::Unknown,
                    };
                    row.push(definition);
                }
                InstructionKind::Replace {
                    destination,
                    displaced,
                    ..
                } => {
                    for place in [destination, displaced] {
                        code.definitions[root(body, place)?].push(Definition::Unknown);
                    }
                }
                InstructionKind::Snapshot { destination, .. } => {
                    code.definitions[root(body, destination)?].push(Definition::Unknown);
                }
                _ => (),
            }
        }
        if let Terminator::Call {
            callee,
            arguments,
            returns,
            ..
        } = &block.terminator
        {
            code.calls.insert(block_index as u32, callee.clone());
            if let Callee::Indirect { value, .. } = callee {
                code.indirect
                    .insert(block_index as u32, local(body, value.place)? as u32);
                if value.usage == UseKind::Mut {
                    code.definitions[local(body, value.place)?].push(Definition::Unknown);
                }
            }
            for argument in arguments {
                let place = body
                    .places
                    .get(argument.value.place.0 as usize)
                    .ok_or(SourceError::Malformed("receipt argument place"))?;
                if !place.projection.is_empty() {
                    code.projected_arguments
                        .insert((block_index as u32, argument.parameter.0));
                }
                if argument.value.usage == UseKind::Mut {
                    let place = body
                        .places
                        .get(argument.value.place.0 as usize)
                        .ok_or(SourceError::Malformed("receipt mut argument"))?;
                    code.definitions
                        .get_mut(place.root.0 as usize)
                        .ok_or(SourceError::Malformed("receipt mut argument local"))?
                        .push(Definition::Unknown);
                }
            }
            if let Some(returned) = returns {
                code.definitions[root(body, returned.destination)?].push(Definition::Unknown);
            }
        }
    }
    Ok(code)
}
impl Code {
    pub fn closures(&self) -> impl Iterator<Item = (u32, ClosureInput)> + '_ {
        self.closures
            .iter()
            .map(|(&value, input)| (value, input.clone()))
    }
    pub fn targets(&self, block: u32) -> Result<Vec<Target>, SourceError> {
        let callee = self
            .calls
            .get(&block)
            .ok_or(SourceError::Malformed("receipt code call selector"))?;
        if let Callee::Definition { path, arguments } = callee {
            return Ok(vec![Target::Definition {
                path: path.clone(),
                arguments: arguments.clone(),
            }]);
        }
        let root = *self
            .indirect
            .get(&block)
            .ok_or(SourceError::Malformed("receipt indirect source selector"))?;
        self.local_targets(root)
    }
    pub fn callback_targets(
        &self,
        block: u32,
        parameter: u32,
        local: u32,
    ) -> Result<Vec<Target>, SourceError> {
        if self.projected_arguments.contains(&(block, parameter)) {
            return Err(SourceError::Unsupported(
                "receipt callback needs original projected producer expansion",
            ));
        }
        self.local_targets(local)
    }
    fn local_targets(&self, root: u32) -> Result<Vec<Target>, SourceError> {
        let mut active = HashSet::new();
        let mut done = HashSet::new();
        let mut pending = vec![(root, false)];
        let mut targets = Vec::new();
        while let Some((local, exit)) = pending.pop() {
            if exit {
                active.remove(&local);
                done.insert(local);
                continue;
            }
            if done.contains(&local) {
                continue;
            }
            if !active.insert(local) {
                return Err(SourceError::Unsupported(
                    "receipt code selection needs cyclic producer expansion",
                ));
            }
            pending.push((local, true));
            let definitions = self
                .definitions
                .get(local as usize)
                .filter(|row| !row.is_empty())
                .ok_or(SourceError::Unsupported(
                    "receipt indirect call has no original code producer",
                ))?;
            for definition in definitions {
                match definition {
                    Definition::Unknown => {
                        return Err(SourceError::Unsupported(
                            "receipt indirect call needs complete original code producers",
                        ));
                    }
                    Definition::Alias(input) => pending.push((*input, false)),
                    Definition::Code(target) if !targets.contains(target) => {
                        targets.push(target.clone())
                    }
                    Definition::Code(_) => (),
                }
            }
        }
        if targets.is_empty() {
            return Err(SourceError::Unsupported(
                "receipt indirect call has no original targets",
            ));
        }
        Ok(targets)
    }
}
