//! Source-context assembly before native instance restoration. This uses the
//! existing context registry and demand solver; it creates no native C proof.
use super::{ReceiptSourceObservation, SourceError, provider, selection};
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
    let body = &original.definition().bodies[input.base.body() as usize];
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
    {
        let ty = substitute(provider.types(), memo, ty, &input.actuals, true)
            .map_err(|error| SourceError::Type(format!("{error:?}")))?;
        let profile = profile::derive(
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

/// Original call selectors and selection observations, outside semantic keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallObservation {
    pub caller: ContextId,
    pub block: u32,
    pub target: ContextId,
    pub selection: SelectionRead,
}

pub struct NamedGraph {
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
        if self.calls != current.calls
            || self.roots != current.roots
            || self.contexts.len() != current.contexts.len()
            || self.requests.len() != current.requests.len()
            || self
                .contexts
                .iter()
                .zip(&current.contexts)
                .any(|(before, after)| {
                    before.source.entry() != after.source.entry()
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
                })
        {
            return Err(SourceError::Malformed(
                "receipt graph differs from current original observations",
            ));
        }
        Ok(())
    }
}

/// Discover a complete graph of original named source calls. Unsupported
/// operations, indirect targets or closure environments remain unavailable;
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
    for root in roots {
        let context = named(provider, root, &memo, cases)?;
        let id = registry.register(context.context())?;
        if id.index() == contexts.len() {
            contexts.push(context);
        }
        root_ids.push(id);
    }
    let mut index = 0;
    while index < contexts.len() {
        let context = &contexts[index];
        if !context.source.demand().projection().closures().is_empty() {
            return Err(SourceError::Unsupported(
                "receipt factory needs original closure context expansion",
            ));
        }
        let blocks: Vec<_> = context
            .source
            .demand()
            .projection()
            .calls()
            .keys()
            .copied()
            .collect();
        let id = registry.register(context.context())?;
        let mut calls = BTreeMap::new();
        for block in blocks {
            let context = &contexts[index];
            let target = selection::direct(
                provider,
                &context.input.base,
                &context.input.actuals,
                block,
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
            use hirsel_rc::sealed_source::contract::registry::Expansion;
            calls.insert(
                block,
                if selection.generated {
                    Expansion::Generated {
                        selection: selection.clone(),
                        target,
                    }
                } else {
                    Expansion::Targets(vec![target])
                },
            );
            observations.push(CallObservation {
                caller: id,
                block,
                target,
                selection,
            });
        }
        registry.resolve(id, calls, BTreeMap::new())?;
        index += 1;
    }
    Ok(NamedGraph {
        calls: observations,
        contexts,
        roots: root_ids,
        requests: registry.solve()?,
    })
}
