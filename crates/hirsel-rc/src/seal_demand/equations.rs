//! Portable grant-flow equations extracted on an individual Value miss.
//! Solving these ordinary producer facts needs neither another Value body nor
//! a layout table. They select physical capacity, never source seal authority.
use super::*;
use hirsel_syntax::{ContentHash, DefPath};
use serde::{Deserialize, Serialize};

pub type Function = (DefPath, ContentHash);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Target {
    definition: DefPath,
    instance: Option<ContentHash>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Closure {
    value: u32,
    target: Option<Target>,
    captures: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Call {
    target: Option<Target>,
    environment: Option<u32>,
    // Original parameter ordinal, local value ordinal, arena carrier.
    arguments: Vec<(u32, u32, bool)>,
    result: Option<u32>,
}

/// No instructions, source bodies, layout indices or accepted proof tokens.
/// Local value ordinals refer only to this owner's original Value numbering.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalDemandEquations {
    schema: u32,
    owner: Function,
    values: u32,
    parameters: Vec<u32>,
    captures: Vec<u32>,
    flow: Vec<(u32, u32)>,
    closures: Vec<Closure>,
    calls: Vec<Call>,
    returns: Vec<u32>,
    seals: Vec<u32>,
}

impl LocalDemandEquations {
    pub fn owner(&self) -> &Function {
        &self.owner
    }

    /// Extraction is not admission. The caller still owns independent Value,
    /// original source, grant and Ready checks before using any output.
    pub fn extract<S>(unit: &LirUnit<S>) -> Result<Self, String> {
        let mut out = Self {
            schema: 1,
            owner: (unit.header.definition.clone(), unit.header.instance),
            values: u32::try_from(unit.values.len()).map_err(|_| "seal-demand value count")?,
            parameters: unit.parameters.iter().map(|v| v.0).collect(),
            captures: unit.captures.iter().map(|v| v.0).collect(),
            flow: vec![],
            closures: vec![],
            calls: vec![],
            returns: vec![],
            seals: vec![],
        };
        let target = |symbol: SymbolId| -> Result<Target, String> {
            let row = unit
                .tables
                .symbols
                .get(symbol.index())
                .ok_or("seal-demand symbol index")?;
            Ok(Target {
                definition: row.definition.clone(),
                instance: row.instance,
            })
        };
        for inst in &unit.instructions {
            match *inst {
                Inst::SealArena { grant, .. } => out.seals.push(grant.0),
                Inst::Closure {
                    result, closure, ..
                } => {
                    let row = unit
                        .tables
                        .closures
                        .get(closure.index())
                        .ok_or("seal-demand closure index")?;
                    let captures = row
                        .captures
                        .get(&unit.pools.captures)
                        .ok_or("seal-demand capture range")?;
                    out.closures.push(Closure {
                        value: result.0,
                        target: row.code.as_ref().map(|c| target(c.symbol)).transpose()?,
                        captures: captures.iter().map(|c| c.value.0).collect(),
                    });
                }
                Inst::CopyValue { result, value }
                | Inst::Weaken { result, value }
                | Inst::Recast { result, value }
                | Inst::Read {
                    result,
                    reaching: value,
                    ..
                }
                | Inst::Copy {
                    result,
                    reaching: value,
                    ..
                }
                | Inst::Store {
                    version: result,
                    value,
                    ..
                }
                | Inst::Replace {
                    version: result,
                    value,
                    ..
                } => out.flow.push((result.0, value.0)),
                _ => (),
            }
        }
        for edge in &unit.tables.edges {
            let block = unit
                .blocks
                .get(edge.target.index())
                .ok_or("seal-demand block index")?;
            let parameters = block
                .parameters
                .get(&unit.pools.values)
                .ok_or("seal-demand block parameters")?;
            let arguments = edge
                .arguments
                .get(&unit.pools.values)
                .ok_or("seal-demand edge arguments")?;
            if parameters.len() != arguments.len() {
                return Err("seal-demand edge arity".into());
            }
            out.flow.extend(
                parameters
                    .iter()
                    .zip(arguments)
                    .map(|(to, from)| (to.0, from.0)),
            );
        }
        for term in &unit.terminators {
            match *term {
                Terminator::Return(Some(value)) => out.returns.push(value.0),
                Terminator::Call { call, .. } => {
                    let call = unit
                        .tables
                        .calls
                        .get(call.index())
                        .ok_or("seal-demand call index")?;
                    let args = call
                        .arguments
                        .get(&unit.pools.call_arguments)
                        .ok_or("seal-demand call arguments")?;
                    let arguments = args
                        .iter()
                        .map(|arg| {
                            let value = match arg.value {
                                ArgumentValue::Value(v) => v,
                                ArgumentValue::Address { version, .. } => version,
                            };
                            let info = unit
                                .values
                                .get(value.index())
                                .ok_or("seal-demand argument value")?;
                            let ty = unit
                                .types
                                .get(info.ty.index())
                                .ok_or("seal-demand argument type")?;
                            Ok((arg.parameter, value.0, ty.repr == ValueRepr::Arena))
                        })
                        .collect::<Result<_, String>>()?;
                    out.calls.push(Call {
                        target: match call.callee {
                            Callee::Direct(code) | Callee::Bound { code, .. } => {
                                Some(target(code)?)
                            }
                            _ => None,
                        },
                        environment: call.callee.value().map(|v| v.0),
                        arguments,
                        result: call.result.map(|v| v.0),
                    });
                }
                _ => (),
            }
        }
        out.flow.sort_unstable();
        out.flow.dedup();
        out.returns.sort_unstable();
        out.returns.dedup();
        out.seals.sort_unstable();
        out.seals.dedup();
        out.validate()?;
        Ok(out)
    }

    /// Check decoded archive structure before any fixed-point evaluation.
    pub fn validate(&self) -> Result<(), String> {
        let inside = |v: &u32| *v < self.values;
        let canonical = |xs: &[u32]| xs.windows(2).all(|w| w[0] < w[1]);
        if self.schema != 1
            || !self
                .parameters
                .iter()
                .chain(&self.captures)
                .chain(&self.returns)
                .chain(&self.seals)
                .all(inside)
            || !canonical(&self.returns)
            || !canonical(&self.seals)
            || self.flow.windows(2).any(|w| w[0] >= w[1])
            || self
                .flow
                .iter()
                .any(|(to, from)| !inside(to) || !inside(from))
            || self
                .closures
                .iter()
                .any(|c| !inside(&c.value) || !c.captures.iter().all(inside))
            || self.calls.iter().any(|c| {
                c.environment.is_some_and(|v| !inside(&v))
                    || c.result.is_some_and(|v| !inside(&v))
                    || c.arguments.iter().any(|(_, v, _)| !inside(v))
            })
        {
            return Err("invalid seal-demand local equations".into());
        }
        Ok(())
    }
}

/// Least fixed point over portable local equations. Entries are explicit;
/// unreachable generic templates cannot invent demanded guards or refusals.
pub fn solve(locals: &[LocalDemandEquations], entries: &[usize]) -> Result<Demand, String> {
    if entries.iter().any(|&entry| entry >= locals.len()) {
        return Err("seal-demand entry outside the unit graph".into());
    }
    let mut symbols = BTreeMap::new();
    let mut offsets = vec![0usize];
    for (u, local) in locals.iter().enumerate() {
        local.validate()?;
        if symbols.insert(local.owner.clone(), u).is_some() {
            return Err("duplicate seal-demand unit instance".into());
        }
        offsets.push(offsets.last().unwrap() + local.values as usize);
    }
    let mut graph = vec![BTreeSet::new(); *offsets.last().unwrap()];
    let mut origins = vec![BTreeSet::<(usize, usize)>::new(); graph.len()];
    for (u, local) in locals.iter().enumerate() {
        for &(to, from) in &local.flow {
            graph[offsets[u] + to as usize].insert(offsets[u] + from as usize);
        }
        for (c, closure) in local.closures.iter().enumerate() {
            origins[offsets[u] + closure.value as usize].insert((u, c));
        }
    }
    let resolve = |target: &Target| -> Option<usize> {
        if let Some(instance) = target.instance {
            symbols.get(&(target.definition.clone(), instance)).copied()
        } else {
            let mut candidates = symbols.iter().filter(|((p, _), _)| *p == target.definition);
            let (_, &selected) = candidates.next()?;
            candidates.next().is_none().then_some(selected)
        }
    };
    let mut reachable: BTreeSet<_> = entries.iter().copied().collect();
    // No seal anywhere is the ordinary no-op path, including unknown calls.
    if locals.iter().any(|local| !local.seals.is_empty()) {
        loop {
            let mut changed = false;
            let mut unresolved = false;
            for v in 0..graph.len() {
                let discovered: BTreeSet<_> = graph[v]
                    .iter()
                    .flat_map(|&source| origins[source].iter().copied())
                    .collect();
                let before = origins[v].len();
                origins[v].extend(discovered);
                changed |= before != origins[v].len();
            }
            for (u, local) in locals.iter().enumerate() {
                if !reachable.contains(&u) {
                    continue;
                }
                for call in &local.calls {
                    let environments = call
                        .environment
                        .map(|v| origins[offsets[u] + v as usize].clone())
                        .unwrap_or_default();
                    let targets: BTreeSet<_> = if let Some(target) = &call.target {
                        resolve(target).into_iter().collect()
                    } else {
                        environments
                            .iter()
                            .filter_map(|&(from, c)| {
                                locals[from].closures[c].target.as_ref().and_then(&resolve)
                            })
                            .collect()
                    };
                    unresolved |=
                        targets.is_empty() && call.arguments.iter().any(|(_, _, arena)| *arena);
                    for target in targets {
                        changed |= reachable.insert(target);
                        let callee = &locals[target];
                        for &(parameter, input, _) in &call.arguments {
                            let formal = *callee
                                .parameters
                                .get(parameter as usize)
                                .ok_or("seal-demand parameter arity")?;
                            changed |= graph[offsets[target] + formal as usize]
                                .insert(offsets[u] + input as usize);
                        }
                        if let Some(result) = call.result {
                            for &value in &callee.returns {
                                changed |= graph[offsets[u] + result as usize]
                                    .insert(offsets[target] + value as usize);
                            }
                        }
                        for &(from, c) in &environments {
                            let captures = &locals[from].closures[c].captures;
                            if captures.len() != callee.captures.len() {
                                return Err("seal-demand capture arity".into());
                            }
                            for (&formal, &actual) in callee.captures.iter().zip(captures) {
                                changed |= graph[offsets[target] + formal as usize]
                                    .insert(offsets[from] + actual as usize);
                            }
                        }
                    }
                }
            }
            if !changed {
                if unresolved {
                    return Err("seal demand needs a complete arena call target".into());
                }
                break;
            }
        }
    }
    let mut seen = vec![false; graph.len()];
    let mut pending = VecDeque::new();
    for (u, local) in locals.iter().enumerate() {
        if reachable.contains(&u) {
            pending.extend(local.seals.iter().map(|&v| offsets[u] + v as usize));
        }
    }
    while let Some(v) = pending.pop_front() {
        if seen[v] {
            continue;
        }
        seen[v] = true;
        pending.extend(graph[v].iter().copied());
    }
    Ok(Demand {
        values: locals
            .iter()
            .enumerate()
            .map(|(u, local)| {
                (0..local.values)
                    .filter(|&v| seen[offsets[u] + v as usize])
                    .map(ValueId)
                    .collect()
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests;
