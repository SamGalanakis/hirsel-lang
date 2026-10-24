//! SCC evaluation of independently reconstructed callable storage equations.
//!
//! Source and V supply their own executable transfer, not one another's facts.
//! The finite path domain comes from consumed owned projections. Extending a
//! recursive path beyond that domain retains a dependency at its prefix rather
//! than spending an iteration/depth budget or inventing heap independence.
use hirsel_types::storage::{StorageBehavior, StorageResult, StorageSelector};
use hirsel_types::{OwnedTypePart, SharedPath};
use std::collections::BTreeSet;

mod actuals;
pub mod program;

/// Prefix-closed input and result paths of one callable's source payloads.
/// An input selector retains its actual parameter/capture identity throughout.
#[derive(Clone, Debug)]
pub struct StorageDomain {
    pub inputs: BTreeSet<StorageSelector>,
    pub results: BTreeSet<SharedPath<OwnedTypePart>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageSolveError {
    DomainCount,
    InvalidCallee(usize),
    InputOutsideDomain(StorageSelector),
    ResultOutsideDomain(SharedPath<OwnedTypePart>),
}

/// The SCC coordinator also carries finite callable-target facts that are
/// proof observations rather than fields of a semantic storage profile.
pub fn fixed_point<T, E>(
    calls: &[Vec<usize>],
    mut values: Vec<T>,
    mut transfer: impl FnMut(usize, &[T]) -> Result<T, E>,
    mut join: impl FnMut(usize, &mut T, T) -> bool,
) -> Result<Vec<T>, E> {
    for component in hirsel_graph::components(calls) {
        loop {
            let mut changed = false;
            for &body in &component {
                let next = transfer(body, &values)?;
                changed |= join(body, &mut values[body], next);
            }
            if !changed {
                break;
            }
        }
    }
    Ok(values)
}

impl StorageDomain {
    /// Finite projection prefixes actually consumed or constructed by this
    /// body. Recursive extensions collapse to their longest represented prefix.
    pub fn for_body(body: &hirsel_types::storage::graph::StorageBody) -> Self {
        use hirsel_types::storage::graph::*;
        let mut paths = BTreeSet::from([SharedPath::new()]);
        let mut add = |path: &SharedPath<OwnedTypePart>| {
            let mut prefix = SharedPath::new();
            for part in path {
                prefix.push(part.clone());
                paths.insert(prefix.clone());
            }
        };
        for path in &body.paths {
            // Profiles and sealed roots have their own relative namespace;
            // every suffix is still only a finite domain member, never code.
            for offset in 0..=path.len() {
                add(&path.iter().skip(offset).cloned().collect());
            }
        }
        for input in &body.inputs {
            add(&input.place.parts);
            for path in &input.callables {
                add(path);
            }
        }
        for node in &body.nodes {
            match &node.operation {
                StorageOp::Assign(assignments) => {
                    for (destination, value) in assignments {
                        add(&destination.parts);
                        match value {
                            StorageValue::Copy { value, .. }
                            | StorageValue::Unique { value, .. } => add(&value.parts),
                            StorageValue::Construct { fields, placement } => {
                                for (path, value) in fields {
                                    add(path);
                                    add(&value.parts);
                                }
                                if let StoragePlacement::Sources(places) = placement {
                                    for place in places {
                                        add(&place.parts);
                                    }
                                }
                            }
                            StorageValue::Callable { captures, .. } => {
                                for capture in captures {
                                    add(&capture.parts);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                StorageOp::Call {
                    callee,
                    arguments,
                    destination,
                    ..
                } => {
                    if let StorageCallee::Value(value) = callee {
                        add(&value.parts);
                    }
                    if let StorageCallee::ScopeCallback { result_path, .. } = callee {
                        add(result_path);
                    }
                    match callee {
                        StorageCallee::MapInsert | StorageCallee::MapGet { .. } => {
                            add(&vec![OwnedTypePart::Elements, OwnedTypePart::Field(0)].into());
                            add(&vec![OwnedTypePart::Elements, OwnedTypePart::Field(1)].into());
                            if let StorageCallee::MapGet { result_path, .. } = callee {
                                add(result_path);
                            }
                        }
                        _ => {}
                    }
                    if let StorageCallee::Declared {
                        allocations,
                        results,
                        ..
                    } = callee
                    {
                        for selector in allocations {
                            add(&selector.parts);
                        }
                        for result in results {
                            add(&result.parts);
                            add(&result.source.parts);
                        }
                    }
                    for (_, place) in arguments {
                        add(&place.parts);
                    }
                    if let Some(place) = destination {
                        add(&place.parts);
                    }
                }
                StorageOp::Return(value) => add(&value.parts),
                _ => {}
            }
        }
        Self {
            inputs: body
                .inputs
                .iter()
                .flat_map(|input| {
                    paths.iter().map(move |parts| StorageSelector {
                        input: input.input.clone(),
                        parts: parts.clone(),
                    })
                })
                .collect(),
            results: paths,
        }
    }
    pub(super) fn selector(
        &self,
        value: &StorageSelector,
    ) -> Result<StorageSelector, StorageSolveError> {
        self.inputs
            .iter()
            .filter(|candidate| {
                candidate.input == value.input && value.parts.starts_with(candidate.parts.iter())
            })
            .max_by_key(|candidate| candidate.parts.len())
            .cloned()
            .ok_or_else(|| StorageSolveError::InputOutsideDomain(value.clone()))
    }

    /// A collapsed dependency applies to every owned child of its prefix.
    /// Consumers must not treat it as evidence that a projected child is empty.
    pub fn normalize(&self, value: StorageBehavior) -> Result<StorageBehavior, StorageSolveError> {
        let StorageBehavior::Known {
            allocations,
            results,
        } = value
        else {
            return Ok(StorageBehavior::Unknown);
        };
        let allocations = allocations
            .iter()
            .map(|value| self.selector(value))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let results = results
            .iter()
            .map(|value| {
                let parts = self
                    .results
                    .iter()
                    .filter(|candidate| value.parts.starts_with(candidate.iter()))
                    .max_by_key(|candidate| candidate.len())
                    .cloned()
                    .ok_or_else(|| StorageSolveError::ResultOutsideDomain(value.parts.clone()))?;
                Ok(StorageResult {
                    parts,
                    source: self.selector(&value.source)?,
                })
            })
            .collect::<Result<BTreeSet<_>, StorageSolveError>>()?;
        Ok(StorageBehavior::Known {
            allocations: allocations.into_iter().collect(),
            results: results.into_iter().collect(),
        })
    }
}

/// Least fixed point in callee-first SCC order. The transfer is independently
/// supplied by each checker. Every growing result adds a member of a finite
/// normalized domain, or becomes Unknown; there is no iteration cutoff.
pub fn solve(
    calls: &[Vec<usize>],
    domains: &[StorageDomain],
    mut transfer: impl FnMut(usize, &[StorageBehavior]) -> StorageBehavior,
) -> Result<Vec<StorageBehavior>, StorageSolveError> {
    if calls.len() != domains.len() {
        return Err(StorageSolveError::DomainCount);
    }
    if let Some(callee) = calls
        .iter()
        .flatten()
        .find(|callee| **callee >= calls.len())
    {
        return Err(StorageSolveError::InvalidCallee(*callee));
    }
    fixed_point(
        calls,
        vec![StorageBehavior::empty(); calls.len()],
        |body, summaries| domains[body].normalize(transfer(body, summaries)),
        |_, current, next| current.join(&next),
    )
}
