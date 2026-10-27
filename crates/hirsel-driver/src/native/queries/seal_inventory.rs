//! Retain current original inventory independently of restored native recipes.
use crate::native::receipt_source::{factory::NamedGraph, provider::SelectionProvider};
use hirsel_lir::operation::OperationDeclarations;
use hirsel_rc::sealed_source::{SourceDeclarations, SourceExpectation, SourceInput};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::BTreeMap;

pub(super) type Sources = BTreeMap<(DefPath, ContentHash), SourceExpectation>;

pub(super) fn current(
    provider: &dyn SelectionProvider,
    graph: &NamedGraph,
    program: &hirsel_rc::instance::Program,
    selected: impl Fn(&hirsel_layout::instance::InstanceKey) -> bool,
) -> Result<Sources, String> {
    let mut sources = BTreeMap::new();
    let memo = hirsel_types::TypeMemo::default();
    for (request, context) in graph.requests.iter().zip(&graph.contexts) {
        // Only demanded instances acquire original inventory obligations. An
        // ordinary source function retains its existing native query contract.
        let Some(key) = request.aware_key() else {
            continue;
        };
        if !selected(key) {
            continue;
        }
        if request.ordinary_key() != &context.input.base {
            return Err("inventory context differs from original request".into());
        }
        let original = provider
            .original(key.definition())
            .map_err(|e| format!("inventory original source: {e:?}"))?;
        let output = original
            .output()
            .ok_or("inventory needs current original V output")?;
        let nominals = program
            .signatures
            .iter()
            .filter(|(_, signature)| matches!(signature.kind, hirsel_types::SignatureKind::Type(_)))
            .map(|(path, _)| {
                provider
                    .signature(path)
                    .map(|signature| (hirsel_syntax::DefId::intern(path), signature))
                    .map_err(|e| format!("inventory original nominal declaration: {e:?}"))
            })
            .collect::<Result<_, _>>()?;
        let source = SourceInput::capture(
            output.observation_identity(),
            original.implementation_revision(),
            original.definition().clone(),
        )
        .with_nominals(nominals);
        let parameters = request
            .declaration()
            .entry()
            .parameters()
            .iter()
            .map(|p| p.ordinal)
            .collect::<Vec<_>>();
        let expected = source
            .derive_request(request, provider.types(), &parameters, |ty| {
                hirsel_rc::instance::substitute(
                    provider.types(),
                    &memo,
                    ty,
                    &context.input.actuals,
                    true,
                )
                .map_err(|e| hirsel_rc::sealed_source::SourceError::Type(format!("{e:?}")))
            })
            .map_err(|e| format!("inventory original expectation: {e:?}"))?;
        sources.insert((key.definition().clone(), key.digest()), expected);
    }
    for operation in &graph.operations {
        let path = &operation.operation.definition;
        if !path.is_core("arena.seal") && !path.is_core("region.with") {
            continue;
        }
        let context = &graph.contexts[operation.caller.index()];
        let call = &context.source.demand().projection().calls()[&operation.block];
        let hirsel_tir::Callee::Definition { arguments, .. } = &call.callee else {
            return Err("sealed original operation".into());
        };
        let arguments = arguments
            .iter()
            .map(|arg| match arg {
                hirsel_types::GenericArgument::Type(ty) => hirsel_rc::instance::substitute(
                    provider.types(),
                    &memo,
                    *ty,
                    &context.input.actuals,
                    true,
                )
                .map(hirsel_types::GenericArgument::Type)
                .map_err(|e| format!("sealed original actual: {e:?}")),
                _ => Err("sealed original non-type actual".into()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let expected =
            hirsel_rc::library::sealed_region::source_expectation(program, path, arguments)
                .map_err(|e| e.to_string())?;
        sources.insert((path.clone(), expected.witness().instance), expected);
    }
    Ok(sources)
}

pub(super) fn declarations<D: OperationDeclarations>(
    ordinary: D,
    sources: &Sources,
) -> SourceDeclarations<D> {
    let mut declarations = SourceDeclarations::overlay(ordinary);
    for source in sources.values() {
        // Each map key is computed from this expectation in current().
        declarations
            .insert(source.clone())
            .expect("unique original inventory instance");
    }
    declarations
}
