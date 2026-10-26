//! Per-module SCC inference, using the existing body ownership proof.
use super::*;
use hirsel_types::{SignatureRef, UniqueInference, UniqueSummary};
use std::collections::BTreeMap;

struct Assumed<'a> {
    provider: &'a mut dyn SignatureProvider,
    headers: &'a BTreeMap<DefId, SignatureRef>,
}
impl SignatureProvider for Assumed<'_> {
    fn signature(&mut self, id: DefId) -> Result<SignatureRef, QueryError> {
        match self.headers.get(&id) {
            Some(header) => Ok(header.clone()),
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
    modes: &ModeAdapter,
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
        loop {
            let mut losses = Vec::new();
            for &index in &component {
                let (body, declared) = &bodies[index];
                let signature = headers[&declared.definition].clone();
                let mut assumed = Assumed {
                    provider,
                    headers: &headers,
                };
                let checked = modes.check_modes(
                    t,
                    m,
                    body.clone(),
                    &signature,
                    &mut PhaseProducts::default(),
                    &mut assumed,
                );
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
