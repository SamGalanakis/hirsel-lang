//! Per-module SCC inference, using the existing body ownership proof.
use super::*;
use hirsel_types::{SignatureRef, UniqueInference, UniqueSummary};
use std::collections::BTreeMap;

struct Assumed<'a> {
    provider: &'a mut dyn SignatureProvider,
    headers: &'a BTreeMap<DefId, SignatureRef>,
    reads: &'a mut BTreeMap<DefId, SignatureRef>,
}
impl SignatureProvider for Assumed<'_> {
    fn signature(&mut self, id: DefId) -> Result<SignatureRef, QueryError> {
        match self.headers.get(&id) {
            Some(header) => {
                self.reads.insert(id, header.clone());
                Ok(header.clone())
            }
            None => self.provider.signature(id),
        }
    }
    fn lookup(
        &mut self,
        context: &FileContext,
        path: &hirsel_syntax::Path,
    ) -> hirsel_types::LookupOutput {
        self.provider.lookup(context, path)
    }
    fn cite_name(&mut self, path: &DefPath) -> Option<hirsel_span::Span> {
        self.provider.cite_name(path)
    }
}

pub(super) fn infer(
    modes: &dyn ModesPhase,
    t: &hirsel_types::TypeInterner,
    m: &hirsel_types::TypeMemo,
    bodies: &[(Arc<TypedBody>, SignatureRef)],
    provider: &mut dyn SignatureProvider,
) -> Result<Vec<(DefId, UniqueInference)>, QueryError> {
    let positions: BTreeMap<_, _> = bodies
        .iter()
        .enumerate()
        .map(|(i, (_, s))| (s.definition, i))
        .collect();
    let mut headers = BTreeMap::new();
    for (_, signature) in bodies {
        let mut header = (**signature).clone();
        let hirsel_types::SignatureKind::Function(f) = &header.kind else {
            continue;
        };
        header.metadata.inferred_unique = UniqueSummary {
            preserves_unique: f
                .parameters
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    (p.passing == hirsel_syntax::PassingMode::Mut)
                        .then_some(hirsel_types::ParamId(i as u32))
                })
                .collect(),
            results: hirsel_types::unique_summary::result_rows(
                t,
                f.result,
                &header.metadata.roles,
                &mut |id| provider.signature(id).ok(),
            )
            .into_iter()
            .map(|(row, _)| row)
            .collect(),
        };
        headers.insert(header.definition, Arc::new(header));
    }
    let graph: Vec<Vec<usize>> = bodies
        .iter()
        .map(|(body, signature)| {
            body.checked
                .resolution
                .expressions
                .iter()
                .filter_map(|(_, name)| {
                    let hirsel_types::ResolvedName::Definition(path) = name else {
                        return None;
                    };
                    hirsel_types::unique_summary::same_module(path, signature.definition.path())
                        .then(|| positions.get(&DefId::intern(path)).copied())
                        .flatten()
                })
                .collect()
        })
        .collect();
    let mut failures = BTreeMap::new();
    for component in hirsel_graph::components(&graph) {
        let cyclic = component.len() > 1 || graph[component[0]].contains(&component[0]);
        let mut previous = BTreeMap::<usize, BTreeMap<DefId, SignatureRef>>::new();
        loop {
            let mut losses = Vec::new();
            for &index in &component {
                let (body, declared) = &bodies[index];
                let signature = headers[&declared.definition].clone();
                if previous.get(&index).is_some_and(|reads| {
                    reads.iter().all(|(id, read)| headers.get(id) == Some(read))
                }) {
                    continue;
                }
                let mut reads = BTreeMap::from([(declared.definition, signature.clone())]);
                let mut assumed = Assumed {
                    provider,
                    headers: &headers,
                    reads: &mut reads,
                };
                let checked = modes.check_modes(
                    t,
                    m,
                    body.clone(),
                    &signature,
                    &mut PhaseProducts::default(),
                    &mut assumed,
                );
                previous.insert(index, reads);
                if let Ok(output) = &checked {
                    failures.insert(
                        declared.definition,
                        output.value.ownership.unique_failures.clone(),
                    );
                }
                let proved = match checked {
                    Ok(output)
                        if !output
                            .diagnostics
                            .iter()
                            .any(|d| d.severity == hirsel_diag::Severity::Error) =>
                    {
                        output.value.ownership.inferred_unique
                    }
                    Ok(_) | Err(QueryError::Diagnostics(_)) => UniqueSummary::default(),
                    Err(error) => return Err(error),
                };
                let mut next = signature.metadata.inferred_unique.clone();
                next.preserves_unique
                    .retain(|p| proved.preserves_unique.contains(p));
                next.results.retain(|p| proved.results.contains(p));
                if next != signature.metadata.inferred_unique {
                    let mut header = (*signature).clone();
                    header.metadata.inferred_unique = next;
                    losses.push((header.definition, Arc::new(header)));
                }
            }
            if losses.is_empty() {
                break;
            }
            headers.extend(losses);
            if !cyclic {
                break;
            }
        }
    }
    Ok(headers
        .into_iter()
        .map(|(id, header)| {
            (
                id,
                UniqueInference {
                    summary: header.metadata.inferred_unique.clone(),
                    failures: failures.remove(&id).unwrap_or_default(),
                },
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Proofs {
        calls: Mutex<BTreeMap<String, usize>>,
    }
    impl ModesPhase for Proofs {
        fn check_modes(
            &self,
            _: &hirsel_types::TypeInterner,
            _: &hirsel_types::TypeMemo,
            body: Arc<TypedBody>,
            signature: &Signature,
            _: &mut PhaseProducts,
            provider: &mut dyn SignatureProvider,
        ) -> Result<QueryOutput<AnalyzedBody>, QueryError> {
            *self
                .calls
                .lock()
                .unwrap()
                .entry(signature.definition.path().0.name.clone())
                .or_default() += 1;
            let mut preserves = signature.definition.path().0.name != "loss";
            for (_, name) in &body.checked.resolution.expressions {
                if let hirsel_types::ResolvedName::Definition(path) = name {
                    let callee = provider.signature(DefId::intern(path))?;
                    preserves &= callee
                        .metadata
                        .inferred_unique
                        .preserves_unique
                        .contains(&hirsel_types::ParamId(0));
                }
            }
            let mut ownership = hirsel_types::OwnershipFacts::default();
            if preserves {
                ownership
                    .inferred_unique
                    .preserves_unique
                    .push(hirsel_types::ParamId(0));
            }
            Ok(QueryOutput {
                value: AnalyzedBody {
                    typed: body,
                    ownership,
                    results: vec![],
                    call_allocations: vec![],
                },
                diagnostics: vec![],
                dependencies: vec![],
            })
        }
    }

    fn prove(
        source: &str,
    ) -> (
        BTreeMap<String, usize>,
        Vec<(String, hirsel_types::UniqueSummary)>,
    ) {
        let mut adapters = crate::compiler_adapters();
        adapters.modes = None;
        let mut engine = QueryEngine::new(default_environment(), adapters);
        let file = engine
            .sources
            .add("proof-inputs.hir", source.into())
            .unwrap();
        let parsed = engine.parse_file(file, &default_context()).unwrap();
        let bodies: Vec<_> = parsed
            .value
            .items
            .iter()
            .map(|syntax| {
                let path = &syntax.item.definition;
                let signature = engine.item_signature(path).unwrap().value;
                let body = Arc::new(engine.check_definition(path).unwrap().value);
                (body, signature)
            })
            .collect();
        let proofs = Proofs {
            calls: Mutex::default(),
        };
        let inferred = infer(
            &proofs,
            &engine.types().clone(),
            &engine.type_memo().clone(),
            &bodies,
            &mut engine,
        )
        .unwrap();
        (
            proofs.calls.into_inner().unwrap(),
            inferred
                .into_iter()
                .map(|(id, proof)| (id.path().0.name.clone(), proof.summary))
                .collect(),
        )
    }

    #[test]
    fn acyclic_proofs_use_the_settled_callee_once() {
        with_compiler_stack(|| {
            let (calls, summaries) = prove(
                "fn caller(mut value: Bytes) { loss(mut value) }\nfn loss(mut value: Bytes) {}\n",
            );
            assert_eq!(
                calls,
                BTreeMap::from([("caller".into(), 1), ("loss".into(), 1)])
            );
            assert!(summaries.iter().all(|(_, summary)| summary.is_empty()));
        });
    }

    #[test]
    fn cyclic_proofs_recheck_changed_observed_summaries_until_settled() {
        with_compiler_stack(|| {
            let (calls, summaries) = prove(
                "fn a(mut value: Bytes) { b(mut value) }\nfn b(mut value: Bytes) { loss(mut value) }\nfn loss(mut value: Bytes) { a(mut value) }\n",
            );
            assert!(summaries.iter().all(|(_, summary)| summary.is_empty()));
            assert!(calls.values().all(|calls| *calls >= 2));
            assert!(calls.values().sum::<usize>() < 12, "{calls:?}");
        });
    }
}
