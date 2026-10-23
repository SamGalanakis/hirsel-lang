//! Native source products are demanded independently. A descriptor lookup
//! revalidates ordinary Lower reads and V's own source envelope before any
//! Definition materialization. Source/private and native proof domains differ.
use crate::*;
use hirsel_syntax::{DefId, ExprId};
use hirsel_tir::{DefinitionRef, Materialized};
use hirsel_types::{SignatureKind, SignatureRef};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Source-only observations for one selected declaration. The opaque lower
/// token can certify only V's exact source envelope, never native Value/Ready.
#[derive(Clone, Debug)]
pub struct NativeDefinitionProduct {
    pub definition: DefPath,
    pub signature: SignatureRef,
    pub signature_dependencies: Vec<SignatureDependency>,
    pub source: NativeSource,
    pub summary: Arc<PublishedSummary>,
    pub summary_identity: Option<ContentHash>,
    pub lower: Option<hirsel_validate::VerifiedDefinitionOutput>,
    pub audit: Option<SourceAudit>,
    pub materialized: Vec<(ExprId, Materialized)>,
    pub representation: Option<RepresentationRecord>,
    pub constant: Option<Materialized>,
}

/// Compact root inputs. This owns no source Definition or native Value table.
/// Required source GEN-5/check roots remain selected by existing source policy.
#[derive(Clone, Debug)]
pub struct NativeSourceRoots {
    pub definitions: Vec<DefPath>,
    pub environment: QueryEnvironment,
    pub registry_revision: ContentHash,
    pub operations: BTreeMap<OperationKey, OperationRecord>,
    pub builtins: BTreeMap<DefPath, SignatureRef>,
    pub sources: SourceMap,
    pub foreign_links: Vec<LinkInput>,
    pub foreign_unreplayable: bool,
    pub foreign_threads: BTreeMap<hirsel_syntax::PackageId, hirsel_types::ForeignThreads>,
}

/// Miss-only expansion of one declaration. The source body stays shared for
/// native import, while requirements can be published independently of it.
pub struct NativeDefinitionExpansion {
    pub body: Option<DefinitionRef>,
    pub requirements: InstanceGraph,
}

impl QueryEngine {
    /// Select compact roots and declaration/launch metadata. No Lower or Value
    /// body is admitted merely to decide an instance/object/Link hit.
    pub fn native_source_roots(
        &mut self,
        roots: &[DefPath],
    ) -> Result<NativeSourceRoots, QueryError> {
        self.prepare_staged_instances()?;
        let paths = if roots.is_empty() {
            self.native_paths()?
        } else {
            self.native_paths_from(roots.iter().cloned())?
        };
        let diagnostics = crate::generic_graph::check(self)?;
        if crate::queries::has_errors(&diagnostics) {
            return Err(QueryError::Diagnostics(diagnostics));
        }
        let registry = registry::BuiltinRegistry::shared();
        Ok(NativeSourceRoots {
            definitions: paths.into_iter().collect(),
            environment: self.environment.clone(),
            registry_revision: registry.revision(),
            operations: registry.operation_inventory(),
            builtins: self.native_builtin_signatures(),
            sources: self.sources.clone(),
            foreign_links: self.foreign_links.clone(),
            foreign_unreplayable: self.foreign_unreplayable,
            foreign_threads: self.foreign_threads.clone(),
        })
    }

    /// Demand only the exact declaration/source observations needed by a native
    /// query. A missing source receipt is a miss; depth/fuel refusal propagates.
    pub fn native_definition_product(
        &mut self,
        path: &DefPath,
    ) -> Result<NativeDefinitionProduct, QueryError> {
        let signature = self.item_signature(path)?;
        if crate::queries::has_errors(&signature.diagnostics) {
            return Err(QueryError::Diagnostics(signature.diagnostics));
        }
        let syntax = self.parse_definition(path)?.value;
        let executable = crate::queries::generation::executable(&syntax);
        let summary = self.definition_summary(DefId::intern(path))?;
        let summary_identity = self.definition_summary_identity(DefId::intern(path))?;
        let lower = if executable {
            self.verified_lower_output(path)?
        } else {
            None
        };
        let (audit, materialized) = if executable {
            let products = self.source_audit_products(path)?;
            (Some(products.audit), products.materialized)
        } else {
            let diagnostics = self.definition_diagnostics(path)?;
            if crate::queries::has_errors(&diagnostics) {
                return Err(QueryError::Diagnostics(diagnostics));
            }
            (None, Vec::new())
        };
        let representation = if matches!(signature.value.kind, SignatureKind::Type(_)) {
            match self.owner_representation(path, path) {
                Ok(record) => Some(record),
                Err(QueryError::NotImplemented) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let constant = if matches!(signature.value.kind, SignatureKind::Const { .. }) {
            Some(self.materialize_value(path)?)
        } else {
            None
        };
        Ok(NativeDefinitionProduct {
            definition: path.clone(),
            signature: signature.value,
            signature_dependencies: signature.dependencies,
            source: NativeSource {
                implementation_revision: syntax.text_hash,
                syntax,
                generated: self.generated_source(path).cloned(),
            },
            summary,
            summary_identity,
            lower,
            audit,
            materialized,
            representation,
            constant,
        })
    }

    /// Miss-only source materialization. A current exact verifier-owned envelope
    /// rebinds without lowering/V; otherwise the ordinary Lower query computes
    /// and independently verifies its artifact. No native proof is minted here.
    pub fn native_definition_body(
        &mut self,
        product: &NativeDefinitionProduct,
    ) -> Result<DefinitionRef, QueryError> {
        self.check_native_definition_product(product)?;
        let path = &product.definition;
        if let Some(output) = self.verified_lower_output(path)?
            && product.lower.as_ref().is_some_and(|old| old == &output)
            && let Some(definition) = self.materialize_verified_lower_output(&output)
        {
            return Ok(definition);
        }
        let lowered = self.lower_to_tir(path)?;
        if crate::queries::has_errors(&lowered.diagnostics) {
            return Err(QueryError::Diagnostics(lowered.diagnostics));
        }
        Ok(lowered.value)
    }

    fn check_native_definition_product(
        &mut self,
        product: &NativeDefinitionProduct,
    ) -> Result<(), QueryError> {
        let path = &product.definition;
        let current = self.parse_definition(path)?.value;
        let signature = self.item_signature(path)?;
        if crate::queries::has_errors(&signature.diagnostics) {
            return Err(QueryError::Diagnostics(signature.diagnostics));
        }
        if current.text_hash != product.source.implementation_revision
            || signature.value.fingerprint != product.signature.fingerprint
        {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "native definition product".into(),
                message: "source declaration changed during native demand".into(),
            }));
        }
        Ok(())
    }

    /// Discover native implicit instances for one owner without exporting or
    /// traversing other source bodies. Source GEN-5 remains a separate check.
    pub fn native_definition_expansion(
        &mut self,
        product: &NativeDefinitionProduct,
        selected: &std::collections::BTreeSet<DefPath>,
    ) -> Result<NativeDefinitionExpansion, QueryError> {
        self.check_native_definition_product(product)?;
        let body = if crate::queries::generation::executable(&product.source.syntax) {
            Some(self.native_definition_body(product)?)
        } else {
            None
        };
        let types = self.types.clone();
        let mut binder = |callee: &DefPath| {
            if !selected.contains(callee) {
                return Ok(None);
            }
            let syntax = self.parse_definition(callee)?.value;
            Ok(Some(syntax.owner().site(hirsel_types::NameSite::Item)))
        };
        let requirements =
            crate::generic_graph::native_definition(&types, product, body.as_deref(), &mut binder)?;
        Ok(NativeDefinitionExpansion { body, requirements })
    }
}
