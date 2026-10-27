//! Source-context assembly before native instance restoration. This uses the
//! existing context registry and demand solver; it creates no native C proof.
use super::{ReceiptSourceObservation, SourceError, provider, selection};
mod code;
mod operations;
use hirsel_layout::instance::InstanceKey;
use hirsel_lir::sealed_payload_source::ClosedStorageProfile;
use hirsel_rc::instance::{Actuals, SelectionRead, substitute};
use hirsel_rc::sealed_source::{
    SourceCallableEntry,
    contract::{
        Port,
        registry::{Context, ContextId, Registry, Request},
    },
    profile::{self, CallableCase},
};
use hirsel_types::{OperationalFailure, QueryError, TypeId, TypeMemo};
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct NamedInput {
    pub base: InstanceKey,
    pub actuals: Actuals,
}

impl NamedInput {
    fn same(&self, other: &Self) -> bool {
        self.base == other.base
            && self.actuals.binder == other.actuals.binder
            && self.actuals.arguments == other.actuals.arguments
            && self.actuals.outer == other.actuals.outer
    }
}

/// Full port profiles and the original current observation are retained beside
/// one lexical request. Executable source bodies are released after capture.
pub struct NamedContext {
    pub input: NamedInput,
    pub source: ReceiptSourceObservation,
    pub ports: BTreeMap<Port, ClosedStorageProfile>,
}
impl NamedContext {
    pub fn context(&self) -> Context<'_> {
        Context {
            base: &self.input.base,
            entry: self.source.entry(),
            source: self.source.demand(),
            ports: &self.ports,
        }
    }
}

/// Read all original parameter/result types before body release. Full callable
/// cases are requested separately for each port; a union cannot replace them.
pub fn named(
    provider: &dyn provider::Provider,
    input: NamedInput,
    memo: &TypeMemo,
    cases: &mut impl FnMut(&SourceCallableEntry, Port, TypeId) -> Result<Vec<CallableCase>, SourceError>,
) -> Result<NamedContext, SourceError> {
    let (signature, original) = provider::original(provider, input.base.definition())?;
    let source = super::named(
        &original,
        &input.base,
        provider.types(),
        &signature,
        &input.actuals,
        memo,
    )?;
    let ports = profiles(
        provider,
        &source,
        &original.definition().bodies[input.base.body() as usize],
        &input.actuals,
        memo,
        cases,
    )?;
    let context = NamedContext {
        input,
        source,
        ports,
    };
    // Exact complete port coverage/type/source binding is the same operation
    // used by Registry, not a second context identity implementation.
    context.context().canonical_bytes()?;
    Ok(context)
}

fn profiles(
    provider: &dyn provider::Provider,
    source: &ReceiptSourceObservation,
    body: &hirsel_tir::Body,
    actuals: &Actuals,
    memo: &TypeMemo,
    cases: &mut impl FnMut(&SourceCallableEntry, Port, TypeId) -> Result<Vec<CallableCase>, SourceError>,
) -> Result<BTreeMap<Port, ClosedStorageProfile>, SourceError> {
    let mut ports = BTreeMap::new();
    for (port, ty) in body
        .parameters
        .iter()
        .enumerate()
        .map(|(ordinal, local)| {
            (
                Port::Parameter(ordinal as u32),
                body.locals[local.0 as usize].ty,
            )
        })
        .chain([(Port::Result, body.result)])
        .chain(source.entry().captures().iter().map(|capture| {
            (
                Port::Capture(capture.ordinal),
                body.locals[capture.local as usize].ty,
            )
        }))
    {
        let ty = substitute(provider.types(), memo, ty, actuals, true)
            .map_err(|error| SourceError::Type(format!("{error:?}")))?;
        let profile = profile::boundary(
            provider.types(),
            ty,
            &mut |definition| {
                provider
                    .signature(definition.path())
                    .map(Some)
                    .map_err(|error| {
                        QueryError::Operational(OperationalFailure {
                            operation: "receipt source profile declaration".into(),
                            message: format!("{error:?}"),
                        })
                    })
            },
            &mut |ty| cases(source.entry(), port, ty),
        )?;
        ports.insert(port, profile);
    }
    Ok(ports)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosureInput {
    pub base: InstanceKey,
    pub parent: hirsel_tir::BodyId,
    pub block: hirsel_tir::BlockId,
    pub statement: u32,
}

pub struct ClosureContext {
    pub input: ClosureInput,
    pub source: ReceiptSourceObservation,
    pub ports: BTreeMap<Port, ClosedStorageProfile>,
}
impl ClosureContext {
    pub fn context(&self) -> Context<'_> {
        Context {
            base: &self.input.base,
            entry: self.source.entry(),
            source: self.source.demand(),
            ports: &self.ports,
        }
    }
    /// The requested closure producer comes from the caller, outside this saved
    /// record. Re-read the actual source assignment and every original capture.
    pub fn recheck(
        &self,
        provider: &dyn provider::Provider,
        input: ClosureInput,
        cases: &mut impl FnMut(
            &SourceCallableEntry,
            Port,
            TypeId,
        ) -> Result<Vec<CallableCase>, SourceError>,
    ) -> Result<(), SourceError> {
        let current = closure(provider, input, &TypeMemo::new(), cases)?;
        if self.input != current.input
            || self.source.entry() != current.source.entry()
            || self.source.demand().projection() != current.source.demand().projection()
            || self.ports != current.ports
        {
            return Err(SourceError::Malformed(
                "receipt closure differs from current original observations",
            ));
        }
        Ok(())
    }
}

/// Assemble a nongeneric closure from an exact current original assignment.
/// Callable type equality alone cannot select its capture ordinals/profiles.
/// This retains source data without inventing a CT-5/environment recipe.
pub fn closure(
    provider: &dyn provider::Provider,
    input: ClosureInput,
    memo: &TypeMemo,
    cases: &mut impl FnMut(&SourceCallableEntry, Port, TypeId) -> Result<Vec<CallableCase>, SourceError>,
) -> Result<ClosureContext, SourceError> {
    let (_, original) = provider::original(provider, input.base.definition())?;
    let source = super::closure(
        &original,
        &input.base,
        provider.types(),
        input.parent,
        input.block,
        input.statement,
    )?;
    let ports = profiles(
        provider,
        &source,
        &original.definition().bodies[input.base.body() as usize],
        &Actuals::none(),
        memo,
        cases,
    )?;
    let context = ClosureContext {
        input,
        source,
        ports,
    };
    context.context().canonical_bytes()?;
    Ok(context)
}

/// Original call selectors and selection observations, outside semantic keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallObservation {
    pub caller: ContextId,
    pub block: u32,
    pub target: ContextId,
    pub selection: SelectionRead,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationObservation {
    pub caller: ContextId,
    pub block: u32,
    pub operation: hirsel_core_contract::OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallTargets {
    pub caller: ContextId,
    pub block: u32,
    pub targets: Vec<ContextId>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosureObservation {
    pub caller: ContextId,
    pub value: u32,
    pub target: ContextId,
}

pub struct NamedGraph {
    pub targets: Vec<CallTargets>,
    pub closures: Vec<ClosureObservation>,
    pub operations: Vec<OperationObservation>,
    pub calls: Vec<CallObservation>,
    pub contexts: Vec<NamedContext>,
    pub roots: Vec<ContextId>,
    pub requests: Vec<Request>,
}

impl NamedGraph {
    /// Reconstruct the whole graph from current source and the original roots.
    /// Deleting both a target and its call observation cannot hide a dependency.
    /// Requested roots come from the caller, never the saved graph under test.
    /// This checks source data, not native C or metadata custody.
    pub fn recheck(
        &self,
        provider: &dyn provider::SelectionProvider,
        roots: impl IntoIterator<Item = NamedInput>,
        cases: &mut impl FnMut(
            &SourceCallableEntry,
            Port,
            TypeId,
        ) -> Result<Vec<CallableCase>, SourceError>,
    ) -> Result<(), SourceError> {
        let current = named_graph(provider, roots, cases)?;
        if self.targets != current.targets
            || self.closures != current.closures
            || self.operations != current.operations
            || self.calls != current.calls
            || self.roots != current.roots
            || self.contexts.len() != current.contexts.len()
            || self.requests.len() != current.requests.len()
            || self
                .contexts
                .iter()
                .zip(&current.contexts)
                .any(|(before, after)| {
                    !before.input.same(&after.input)
                        || before.source.entry() != after.source.entry()
                        || before.source.demand().projection() != after.source.demand().projection()
                        || before.ports != after.ports
                })
            || self
                .requests
                .iter()
                .zip(&current.requests)
                .any(|(before, after)| {
                    before.ordinary_key() != after.ordinary_key()
                        || before.context_key() != after.context_key()
                        || before.aware_key() != after.aware_key()
                        || before.declaration() != after.declaration()
                        || before.profiles() != after.profiles()
                })
        {
            return Err(SourceError::Malformed(
                "receipt graph differs from current original observations",
            ));
        }
        Ok(())
    }
}

/// Discover original named and local closure call contexts from named roots.
/// Unsupported operations or unresolved incoming callable producers remain unavailable;
/// none becomes a known-empty expansion or an ordinary fallback request.
/// Source-available support for those boundaries belongs in this same factory.
pub fn named_graph(
    provider: &dyn provider::SelectionProvider,
    roots: impl IntoIterator<Item = NamedInput>,
    cases: &mut impl FnMut(&SourceCallableEntry, Port, TypeId) -> Result<Vec<CallableCase>, SourceError>,
) -> Result<NamedGraph, SourceError> {
    let memo = TypeMemo::new();
    let mut registry = Registry::default();
    let mut contexts = Vec::new();
    let mut root_ids = Vec::new();
    let mut observations = Vec::new();
    let mut operations = Vec::new();
    for root in roots {
        let context = named(provider, root, &memo, cases)?;
        let id = registry.register(context.context())?;
        if id.index() == contexts.len() {
            contexts.push(context);
        }
        root_ids.push(id);
    }
    let mut closures_observed = Vec::new();
    let mut call_targets = Vec::new();
    let mut index = 0;
    while index < contexts.len() {
        let context = &contexts[index];
        let id = registry.register(context.context())?;
        let code = code::capture(provider, context)?;
        let mut closures = BTreeMap::new();
        for (value, input) in code.closures() {
            let context = closure(provider, input, &memo, cases)?;
            let context = NamedContext {
                input: NamedInput {
                    base: context.input.base,
                    actuals: Actuals::none(),
                },
                source: context.source,
                ports: context.ports,
            };
            let target = registry.register(context.context())?;
            if target.index() == contexts.len() {
                contexts.push(context);
            }
            closures.insert(value, target);
            closures_observed.push(ClosureObservation {
                caller: id,
                value,
                target,
            });
        }
        let blocks: Vec<_> = contexts[index]
            .source
            .demand()
            .projection()
            .calls()
            .keys()
            .copied()
            .collect();
        let mut calls = BTreeMap::new();
        for block in blocks {
            let context = &contexts[index];
            let call = &context.source.demand().projection().calls()[&block];
            let operation = operations::expand(provider, call)?;
            let mut read_owner = None;
            let mut task_scope = None;
            let scope = match operation {
                Some((operation, operations::Demand::External(demand))) => {
                    calls.insert(
                        block,
                        hirsel_rc::sealed_source::contract::registry::Expansion::External(demand),
                    );
                    operations.push(OperationObservation {
                        caller: id,
                        block,
                        operation,
                    });
                    continue;
                }
                Some((operation, operations::Demand::RegionRead { owner, callback })) => {
                    operations.push(OperationObservation {
                        caller: id,
                        block,
                        operation,
                    });
                    read_owner = Some(owner);
                    Some(callback)
                }
                Some((operation, operations::Demand::ArenaScope { callback })) => {
                    operations.push(OperationObservation {
                        caller: id,
                        block,
                        operation,
                    });
                    Some(callback)
                }
                Some((operation, operations::Demand::TaskCallback { callback, scoped })) => {
                    operations.push(OperationObservation {
                        caller: id,
                        block,
                        operation,
                    });
                    task_scope = Some(scoped);
                    Some(callback)
                }
                None => None,
            };
            let indirect =
                scope.is_some() || matches!(call.callee, hirsel_tir::Callee::Indirect { .. });
            let selected = if let Some(callback) = scope {
                let argument = call
                    .arguments
                    .get(&callback)
                    .ok_or(SourceError::Malformed("receipt arena callback argument"))?;
                code.callback_targets(block, callback, argument.local)?
            } else {
                code.targets(block)?
            };
            let mut targets = Vec::new();
            let mut generated = None;
            for selected in selected {
                match selected {
                    code::Target::Closure(value) => {
                        targets.push(*closures.get(&value).ok_or(SourceError::Malformed(
                            "receipt call has no original closure producer",
                        ))?);
                    }
                    code::Target::Definition { path, arguments } => {
                        let target = selection::definition(
                            provider,
                            &path,
                            &arguments,
                            &contexts[index].input.actuals,
                            &memo,
                        )?;
                        let selection = target.selection;
                        let context = named(
                            provider,
                            NamedInput {
                                base: target.base,
                                actuals: target.actuals,
                            },
                            &memo,
                            cases,
                        )?;
                        let target = registry.register(context.context())?;
                        if target.index() == contexts.len() {
                            contexts.push(context);
                        }
                        if selection.generated && !indirect {
                            generated = Some((selection.clone(), target));
                        }
                        observations.push(CallObservation {
                            caller: id,
                            block,
                            target,
                            selection,
                        });
                        targets.push(target);
                    }
                }
            }
            targets.sort_unstable();
            targets.dedup();
            use hirsel_rc::sealed_source::contract::registry::Expansion;
            let expansion = if let (Some(owner), Some(callback)) = (read_owner, scope) {
                Expansion::RegionRead {
                    owner,
                    callback,
                    targets: targets.clone(),
                }
            } else if let (Some(callback), Some(scoped)) = (scope, task_scope) {
                Expansion::TaskCallback {
                    callback,
                    scoped,
                    targets: targets.clone(),
                }
            } else if let Some(callback) = scope {
                Expansion::ArenaScope {
                    callback,
                    targets: targets.clone(),
                }
            } else if let Some((selection, target)) = generated {
                if targets != [target] {
                    return Err(SourceError::Malformed(
                        "receipt direct generated call has multiple targets",
                    ));
                }
                Expansion::Generated { selection, target }
            } else {
                Expansion::Targets(targets.clone())
            };
            call_targets.push(CallTargets {
                caller: id,
                block,
                targets,
            });
            calls.insert(block, expansion);
        }
        registry.resolve(id, calls, closures)?;
        index += 1;
    }
    Ok(NamedGraph {
        targets: call_targets,
        closures: closures_observed,
        operations,
        calls: observations,
        contexts,
        roots: root_ids,
        requests: registry.solve()?,
    })
}
