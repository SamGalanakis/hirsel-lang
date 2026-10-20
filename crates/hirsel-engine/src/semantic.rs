//! Driver-owned syntax snapshots, header isolation and explicit manifest composition.
use crate::*;
use hirsel_types::DeclarationSite;
use std::{cell::RefCell, rc::Rc, sync::Arc};

#[derive(Clone, Debug)]
pub struct ResolverSnapshot {
    pub syntax: Arc<ItemSyntax>,
    pub output: QueryOutput<Resolution>,
}
impl std::ops::Deref for ResolverSnapshot {
    type Target = Resolution;
    fn deref(&self) -> &Self::Target {
        &self.output.value
    }
}

/// These snapshots retain syntax owners and shared resolver data, including failed reads.
/// Native resolver envelopes stay at the adapter boundary; consumers use Resolution.
#[derive(Default)]
pub struct CompilerState {
    pub materialization_environment: Option<QueryEnvironment>,
    pub(crate) materialization_limits: Option<hirsel_interp::Limits>,
    pub(crate) constant_values: BTreeMap<DefPath, crate::comptime::EvaluatedConstant>,
    pub(crate) representations: BTreeMap<DefPath, crate::representation::OwnerRecord>,
    pub mode_diagnostics: Vec<(Diagnostic, String)>,
    /// The frozen declaration index the engine publishes before resolution
    /// and signature normalization read it.
    pub(crate) declarations: Option<Arc<crate::snapshot::Declarations>>,
    pub resolver_outputs: BTreeMap<DefPath, ResolverSnapshot>,
    pub signature_resolutions: BTreeMap<DefPath, ResolverSnapshot>,
}
impl CompilerState {
    fn declarations(&self) -> Arc<crate::snapshot::Declarations> {
        self.declarations
            .clone()
            .expect("the engine publishes its declarations before resolving")
    }
}
fn resolver_expected() -> hirsel_resolve::ExpectedTypes {
    let registry = crate::registry::BuiltinRegistry::new();
    hirsel_resolve::ExpectedTypes {
        available_variants: registry
            .signatures()
            .flat_map(|s| {
                s.types.iter().flat_map(|(_, t)| {
                    if let hirsel_types::TypeKind::Variant(variants) = &t.kind {
                        variants.iter().map(|v| v.name.clone()).collect::<Vec<_>>()
                    } else {
                        vec![]
                    }
                })
            })
            .collect(),
        ..Default::default()
    }
}
pub struct ResolverAdapter {
    pub state: Rc<RefCell<CompilerState>>,
}
impl ResolverPhase for ResolverAdapter {
    fn resolve(
        &self,
        syntax: Arc<ItemSyntax>,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Resolution>, QueryError> {
        let item = &syntax.item;
        let declarations = self.state.borrow().declarations();
        let index = declarations.index(item.definition.0.package.clone());
        let siblings = declarations.siblings(&syntax);
        let result = hirsel_resolve::resolve_view(
            &syntax,
            &siblings,
            hirsel_resolve::ResolveView::Item,
            &index,
            provider,
            &resolver_expected(),
        );
        self.state.borrow_mut().resolver_outputs.insert(
            item.definition.clone(),
            ResolverSnapshot {
                syntax: syntax.clone(),
                output: result.query.clone(),
            },
        );
        if let Some(error) = result.operational_failure {
            return Err(QueryError::Operational(error));
        }
        Ok(result.query)
    }
}
pub struct SignatureAdapter {
    pub state: Rc<RefCell<CompilerState>>,
    pub phases: Rc<dyn Phases>,
}
impl SignaturePhase for SignatureAdapter {
    fn signature_items(
        &self,
        candidates: &[Arc<ItemSyntax>],
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        // Visit the later duplicate first so normalization reports overloading before
        // trying unavailable library types in the first declaration's header.
        let syntax = candidates
            .get(1)
            .or_else(|| candidates.first())
            .ok_or(QueryError::NotImplemented)?;
        self.signature_with_candidates(syntax.clone(), provider, Some(candidates), None)
    }
    fn signature(
        &self,
        syntax: Arc<ItemSyntax>,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        self.signature_with_candidates(syntax, provider, None, None)
    }
    fn staged_signature(
        &self,
        candidates: &[Arc<ItemSyntax>],
        provider: &mut dyn SignatureProvider,
        stage: hirsel_types::GroupStage,
    ) -> Result<Signature, QueryError> {
        let syntax = candidates.first().ok_or(QueryError::NotImplemented)?;
        self.signature_with_candidates(syntax.clone(), provider, Some(candidates), Some(stage))
            .map(|output| output.value)
    }
}
impl SignatureAdapter {
    fn signature_with_candidates(
        &self,
        syntax: Arc<ItemSyntax>,
        provider: &mut dyn SignatureProvider,
        candidates: Option<&[Arc<ItemSyntax>]>,
        stage: Option<hirsel_types::GroupStage>,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        let item = &syntax.item;
        if let Some(owner) = hirsel_check_protocol::fact::fact_owner(&item.definition) {
            let owner = provider.signature(&owner)?;
            return Ok(QueryOutput {
                value: hirsel_check_protocol::fact::fact_signature(&owner)?,
                diagnostics: vec![],
                dependencies: hirsel_check_protocol::fact::fact_dependencies(&owner),
            });
        }
        let context = FileContext {
            package: item.definition.0.package.clone(),
            modules: item.definition.0.modules.clone(),
        };
        let module = self.state.borrow().declarations();
        let index = module.index(context.package.clone());
        let declaration_sites = module.sites(&item.definition);
        let siblings = module.siblings(&syntax);
        // Retained snapshots can be older than edited peers; candidates are in
        // current source order, so only this owner's indexed sites are replaced.
        let declaration_sites: Arc<[DeclarationSite]> =
            candidates.map_or(declaration_sites, |candidates| {
                candidates
                    .iter()
                    .map(|candidate| DeclarationSite {
                        definition: candidate.item.definition.clone(),
                        name_span: candidate.item.name_span,
                    })
                    .collect::<Vec<_>>()
                    .into()
            });
        let duplicate = declaration_sites.len() > 1;
        let resolved = if duplicate {
            // normalize's first step is duplicate validation, needing no type resolution.
            None
        } else {
            Some(hirsel_resolve::resolve_view(
                &syntax,
                &siblings,
                hirsel_resolve::ResolveView::Header,
                &index,
                provider,
                &resolver_expected(),
            ))
        };
        let resolution = resolved
            .as_ref()
            .map(|r| r.query.value.clone())
            .unwrap_or_default();
        if let Some(resolved) = resolved {
            if let Some(error) = &resolved.operational_failure {
                return Err(QueryError::Operational(error.clone()));
            }
            let diagnostics = resolved.query.diagnostics.clone();
            // A provisional signature publishes nothing for its declaration.
            if stage.is_none() {
                self.state.borrow_mut().signature_resolutions.insert(
                    item.definition.clone(),
                    ResolverSnapshot {
                        syntax: syntax.clone(),
                        output: resolved.query,
                    },
                );
            }
            if crate::queries::has_errors(&diagnostics) {
                return Err(QueryError::Diagnostics(diagnostics));
            }
        }
        let output = self.phases.normalize_signature(
            hirsel_check_protocol::header::Input {
                syntax: &syntax,
                context: &context,
                resolution: &resolution,
                metadata: module.metadata(),
                declaration_sites: &declaration_sites,
                stage,
            },
            provider,
        )?;
        if stage.is_some() {
            return Ok(output.public);
        }
        if let Some(owner) = output.owner {
            self.state.borrow_mut().representations.insert(
                item.definition.clone(),
                crate::representation::OwnerRecord {
                    text_hash: syntax.text_hash,
                    representation: owner,
                    dependencies: output.public.dependencies.clone(),
                },
            );
        } else {
            self.state
                .borrow_mut()
                .representations
                .remove(&item.definition);
        }
        Ok(output.public)
    }
}
