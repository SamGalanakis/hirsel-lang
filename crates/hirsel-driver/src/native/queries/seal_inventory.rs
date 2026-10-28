//! Retain current original inventory independently of restored native recipes.
use crate::native::receipt_source::{factory::NamedGraph, provider::SelectionProvider};
use hirsel_lir::operation::OperationDeclarations;
use hirsel_rc::sealed_source::{SourceDeclarations, SourceExpectation, SourceInput};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type Sources = BTreeMap<(DefPath, ContentHash), SourceExpectation>;

pub(super) fn current(
    provider: &dyn SelectionProvider,
    graph: &NamedGraph,
    program: &hirsel_rc::instance::Program,
    target: &hirsel_lir::TargetSpec,
    selected: impl Fn(&hirsel_layout::instance::InstanceKey) -> bool,
) -> Result<Sources, String> {
    let mut sources = BTreeMap::new();
    let mut entries = BTreeMap::new();
    let mut constructions = BTreeMap::new();
    let entry_parents = graph
        .closures
        .iter()
        .filter(|observation| {
            graph
                .requests
                .get(observation.target.index())
                .and_then(|child| child.aware_key())
                .is_some_and(&selected)
        })
        .map(|observation| observation.caller.index())
        .collect::<BTreeSet<_>>();
    let memo = hirsel_types::TypeMemo::default();
    let owners =
        hirsel_rc::library::owners(&program.types, &program.signatures).over(&program.owners);
    let mut layouts = hirsel_layout::layout::LayoutBuilder::new(target, &owners);
    for (context_index, (request, context)) in
        graph.requests.iter().zip(&graph.contexts).enumerate()
    {
        // A selected child needs its lexical entry even when its parent is
        // not selected for this query. Only demanded bodies acquire inventory.
        let demanded = request.aware_key().is_some_and(&selected);
        if !demanded && !entry_parents.contains(&context_index) {
            continue;
        }
        let key = request.aware_key().unwrap_or(request.ordinary_key());
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
        let substitute = |ty| {
            hirsel_rc::instance::substitute(
                provider.types(),
                &memo,
                ty,
                &context.input.actuals,
                true,
            )
            .map_err(|e| hirsel_rc::sealed_source::SourceError::Type(format!("{e:?}")))
        };
        let expected = demanded
            .then(|| source.derive_request(request, provider.types(), &parameters, substitute))
            .transpose()
            .map_err(|e| format!("inventory original expectation: {e:?}"))?;
        let lexical = match &expected {
            Some(expected) => expected.environments().to_vec(),
            None => source
                .closure_environments(request, provider.types(), &parameters, substitute)
                .map_err(|e| format!("inventory original entry environment: {e:?}"))?,
        };
        let mut environments = Vec::new();
        for environment in &lexical {
            let mut observed = graph.closures.iter().filter(|observed| {
                observed.caller.index() == context_index
                    && request
                        .closures()
                        .get(&observed.value)
                        .is_some_and(|closed| {
                            closed.entry() == environment.entry()
                                && closed.profiles() == environment.profiles()
                        })
            });
            let selected_observation = observed
                .next()
                .ok_or("original environment lacks its selected child context")?;
            if observed.next().is_some() {
                return Err("original environment has ambiguous child contexts".into());
            }
            let observed = selected_observation;
            let child = graph
                .requests
                .get(observed.target.index())
                .ok_or("original environment has a foreign child context")?;
            if !demanded && !child.aware_key().is_some_and(&selected) {
                continue;
            }
            let native = source
                .native_environment(environment, child, provider.types(), &mut layouts)
                .map_err(|error| format!("inventory native environment: {error:?}"))?;
            let child_key = (native.original.definition.clone(), native.instance);
            if entries
                .insert(child_key, native.clone())
                .is_some_and(|previous| previous != native)
            {
                return Err("original child has inconsistent environment contexts".into());
            }
            environments.push(native);
        }
        if let Some(expected) = expected {
            constructions.insert((key.definition().clone(), key.digest()), environments);
            sources.insert((key.definition().clone(), key.digest()), expected);
        }
    }
    for (key, expected) in &mut sources {
        let environments = constructions.remove(key).unwrap_or_default();
        expected
            .bind_native_environments(environments, entries.remove(key))
            .map_err(|error| format!("inventory native environments: {error:?}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::receipt_source::{factory::NamedInput, provider::Front};

    #[test]
    fn callback_failure_entries_survive_independent_child_selection() {
        check_entries("MEM-4-sealed-callback-failure-run.hir");
    }

    #[test]
    fn callback_drop_cause_entries_survive_independent_child_selection() {
        check_entries("MEM-4-sealed-callback-drop-cause-run.hir");
    }

    fn check_entries(name: &str) {
        hirsel_engine::with_compiler_stack(|| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance/MEM")
                .join(name);
            let mut loaded = crate::load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input).unwrap();
            let program = crate::native::adapt::program(&validated);
            let original = &validated.snapshot().definitions[&loaded.entry];
            let base = hirsel_layout::instance::InstanceKey::new(
                &loaded.entry,
                original.entry.0,
                &[],
                &program.types,
                None,
                None,
            )
            .unwrap();
            let provider = Front::new(validated.original_reader().unwrap());
            let (graph, _) = super::super::receipt_requests::reconstruct(
                &provider,
                vec![NamedInput {
                    base,
                    actuals: hirsel_rc::instance::Actuals::none(),
                }],
                &program,
            )
            .unwrap();
            let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
            let all = current(&provider, &graph, &program, &target, |_| true).unwrap();
            let mut checked = BTreeSet::new();
            for observation in &graph.closures {
                let Some(key) = graph.requests[observation.target.index()].aware_key() else {
                    continue;
                };
                if !checked.insert(key.digest()) {
                    continue;
                }
                let address = (key.definition().clone(), key.digest());
                let expected = all[&address].source();
                let entry = expected
                    .environments
                    .as_ref()
                    .unwrap()
                    .entry
                    .as_ref()
                    .unwrap();
                assert_eq!(entry.instance, key.digest());
                let isolated = current(&provider, &graph, &program, &target, |candidate| {
                    candidate == key
                })
                .unwrap();
                let actual = isolated[&address].source();
                actual.check().unwrap();
                assert_eq!(actual.environments, expected.environments);
                assert_eq!(actual.witness, expected.witness);
            }
            assert!(
                !checked.is_empty(),
                "fixture has no receipt-aware child entries"
            );
        });
    }
}
