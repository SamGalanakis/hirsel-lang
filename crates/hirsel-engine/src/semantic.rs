//! Driver-owned syntax snapshots, header isolation and explicit manifest composition.
use crate::*;
use hirsel_types::DeclarationSite;
use std::sync::Arc;

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

/// Exact ordinary archive already recomputed and compared in this engine.
/// This is comparison metadata, with no retained checked or analyzed body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerifiedMode {
    pub key: crate::store::EntryKey,
    pub reads: hirsel_syntax::ContentHash,
    pub payload: hirsel_syntax::ContentHash,
}

/// Products a single phase invocation publishes with its memo. The sink is
/// local to the invocation, including when normalization returns diagnostics.
#[derive(Clone, Debug, Default)]
pub struct PhaseProducts {
    /// A validated durable Signature already owns the complete publication.
    /// Its lazy local projection may omit syntax-backed auxiliary products and
    /// must never be serialized back over that original record.
    pub(crate) signature_persisted: bool,
    pub(crate) verified_mode: Option<VerifiedMode>,
    pub resolution: Option<ResolverSnapshot>,
    pub(crate) representation: Option<crate::representation::OwnerRecord>,
    pub mode_diagnostics: Vec<(Diagnostic, String)>,
}
fn resolver_expected() -> hirsel_resolve::ExpectedTypes {
    hirsel_resolve::ExpectedTypes {
        available_variants: crate::registry::variant_names().clone(),
        ..Default::default()
    }
}
pub struct ResolverAdapter;
impl ResolverPhase for ResolverAdapter {
    fn resolve(
        &self,
        t: &hirsel_types::TypeInterner,
        _: &hirsel_types::TypeMemo,
        syntax: Arc<ItemSyntax>,
        declarations: &Declarations,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Resolution>, QueryError> {
        let item = &syntax.item;
        let index = declarations.index(item.definition.0.package.clone());
        let siblings = declarations.siblings(&syntax);
        let result = hirsel_resolve::resolve_view(
            &syntax,
            &siblings,
            hirsel_resolve::ResolveView::Item,
            &index,
            t,
            provider,
            &resolver_expected(),
        );
        products.resolution = Some(ResolverSnapshot {
            syntax: syntax.clone(),
            output: result.query.clone(),
        });
        if let Some(error) = result.operational_failure {
            return Err(QueryError::Operational(error));
        }
        Ok(result.query)
    }
}
pub struct SignatureAdapter {
    pub phases: Arc<dyn Phases>,
}
impl SignaturePhase for SignatureAdapter {
    fn signature_items(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        candidates: &[Arc<ItemSyntax>],
        declarations: &Declarations,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        // Visit the later duplicate first so normalization reports overloading before
        // trying unavailable library types in the first declaration's header.
        let syntax = candidates
            .get(1)
            .or_else(|| candidates.first())
            .ok_or(QueryError::NotImplemented)?;
        self.signature_with_candidates(
            t,
            m,
            syntax.clone(),
            declarations,
            products,
            provider,
            Some(candidates),
            None,
        )
    }
    fn signature(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        syntax: Arc<ItemSyntax>,
        declarations: &Declarations,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        self.signature_with_candidates(t, m, syntax, declarations, products, provider, None, None)
    }
    fn staged_signature(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        candidates: &[Arc<ItemSyntax>],
        declarations: &Declarations,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
        stage: hirsel_types::GroupStage,
    ) -> Result<Signature, QueryError> {
        let syntax = candidates.first().ok_or(QueryError::NotImplemented)?;
        self.signature_with_candidates(
            t,
            m,
            syntax.clone(),
            declarations,
            products,
            provider,
            Some(candidates),
            Some(stage),
        )
        .map(|output| output.value)
    }
}
impl SignatureAdapter {
    fn signature_with_candidates(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        syntax: Arc<ItemSyntax>,
        declarations: &Declarations,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
        candidates: Option<&[Arc<ItemSyntax>]>,
        stage: Option<hirsel_types::GroupStage>,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        let item = &syntax.item;
        if let Some(owner) = hirsel_check_protocol::fact::fact_owner(&item.definition) {
            let owner = provider.signature(DefId::intern(&owner))?;
            return Ok(QueryOutput {
                value: hirsel_check_protocol::fact::fact_signature(t, &owner)?,
                diagnostics: vec![],
                dependencies: hirsel_check_protocol::fact::fact_dependencies(&owner),
            });
        }
        if let Some(signature) = crate::dictionaries::literal_signature(t, m, &syntax, provider)? {
            return Ok(signature);
        }
        let context = FileContext {
            package: item.definition.0.package.clone(),
            modules: item.definition.0.modules.clone(),
        };
        let module = declarations;
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
                t,
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
                products.resolution = Some(ResolverSnapshot {
                    syntax: syntax.clone(),
                    output: resolved.query,
                });
            }
            if crate::queries::has_errors(&diagnostics) {
                return Err(QueryError::Diagnostics(diagnostics));
            }
        }
        let normalize =
            hirsel_intern::timings::phase(hirsel_intern::timings::Phase::SignatureNormalize);
        let output = self.phases.normalize_signature(
            hirsel_check_protocol::header::Input {
                types: t,
                memo: m,
                syntax: &syntax,
                context: &context,
                resolution: &resolution,
                metadata: module.metadata(),
                declaration_sites: &declaration_sites,
                stage,
            },
            provider,
        )?;
        drop(normalize);
        if stage.is_some() {
            return Ok(output.public);
        }
        products.representation = output
            .owner
            .map(|owner| crate::representation::OwnerRecord {
                view: Default::default(),
                text_hash: syntax.text_hash,
                representation: owner,
                dependencies: output.public.dependencies.clone(),
            });
        Ok(output.public)
    }
}
