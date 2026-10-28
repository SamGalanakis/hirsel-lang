//! Demand declaration assembly over source products. This boundary owns no
//! Definition or Value collection and never exports a NativeInput snapshot.
mod metadata;
use super::*;
use hirsel_core_contract::OperationId;
use hirsel_engine::{
    InstanceGraph, NativeDefinitionProduct, NativeDiscoveryCache, NativeSourceRoots, QueryEngine,
    QueryError,
};
use hirsel_rc::instance::{DiscoveryContext, Program};
use hirsel_syntax::DefPath;
use hirsel_types::NameSite;
use std::collections::{BTreeMap, BTreeSet};

/// Root data keeps the reservation of its actual producing graph. Scalar
/// callers own their snapshot; admitted front products share theirs directly.
#[derive(Clone)]
pub enum RootInputs {
    Owned(Box<NativeSourceRoots>),
    Front(Arc<hirsel_engine::FrontNativeProducts>),
}
impl std::ops::Deref for RootInputs {
    type Target = NativeSourceRoots;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(roots) => roots,
            Self::Front(front) => &front.roots,
        }
    }
}
impl RootInputs {
    pub(super) fn owned_mut(&mut self) -> Option<&mut NativeSourceRoots> {
        match self {
            Self::Owned(roots) => Some(roots),
            Self::Front(_) => None,
        }
    }
}

/// Compact current inputs for source/instance misses. Required source roots and
/// GEN-5 remain the engine's policy; native demand does not prune diagnostics.
pub struct NativeDeclarations {
    pub roots: RootInputs,
    pub program: Program,
    pub context: DiscoveryContext,
    pub sources: BTreeMap<DefPath, Arc<NativeDefinitionProduct>>,
    pub source_anchors: BTreeMap<DefPath, Vec<hirsel_span::Anchor>>,
    pub discoveries: Vec<ProductRef>,
    pub constant_definitions: Vec<DefPath>,
}
impl NativeDeclarations {
    /// Reviewed runtime dispositions and ownership from this exact source
    /// inventory. Instance C admission and Ready must use the same manifest.
    pub fn manifest(&self) -> Result<hirsel_core_contract::Manifest, String> {
        crate::native::adapt::manifest_for_operations(self.roots.operations.values())
    }
    /// Join a separately selected source instance with the actual V-owned
    /// original call inventory. This opens no Definition or candidate Value.
    /// The instance recipe supplies the destination and lexical body ordinal.
    pub fn source_operations(
        &self,
        source: &DefPath,
        selected: &hirsel_lir::ir::Symbol,
        body: hirsel_tir::BodyId,
    ) -> Result<hirsel_layout::operation::OperationRegistry, String> {
        let output = self
            .sources
            .get(source)
            .and_then(|source| source.lower.as_ref())
            .ok_or("current verifier-owned source invocation output is unavailable")?;
        let evidence = hirsel_rc::invocations::source_instance_from_output(
            &output.invocations(),
            source,
            &self.program.operations,
            selected,
            body,
        )
        .map_err(str::to_owned)?;
        let mut registry = self.program.operation_registry();
        registry
            .invocations
            .insert(evidence.body.definition.clone(), evidence.invocations);
        registry.invocation_bodies.push(evidence.body);
        Ok(registry)
    }

    /// Demand one original source body and apply the same independent CT-5
    /// uses check as snapshot import. No body enters the declaration registry.
    pub fn body(
        &mut self,
        engine: &mut QueryEngine,
        path: &DefPath,
    ) -> Result<hirsel_tir::DefinitionRef, QueryError> {
        let source = self
            .sources
            .get_mut(path)
            .ok_or_else(|| invalid(format!("missing native source declaration {path}")))?;
        let (definition, output) = engine.native_definition_body_with_output(source)?;
        let violations = hirsel_validate::validate_instance_uses(
            &self.program.types,
            &self.program.instance_bodies,
            path,
            &definition,
        );
        if !violations.is_empty() {
            return Err(invalid(format!("invalid instance bodies: {violations:?}")));
        }
        Arc::make_mut(source).lower = output;
        Ok(definition)
    }

    /// Capture the original Lower snapshot before native recipe restoration.
    /// Only an actual V output supplies its observation. An unavailable source
    /// observation remains None; this does not construct a receipt contract or
    /// infer independently empty call/closure expansion.
    pub fn original_source(
        &mut self,
        engine: &mut QueryEngine,
        path: &DefPath,
    ) -> Result<
        (
            hirsel_tir::DefinitionRef,
            Option<hirsel_rc::sealed_source::SourceInput>,
        ),
        QueryError,
    > {
        let definition = self.body(engine, path)?;
        let source = &self.sources[path];
        let input = source.lower.as_ref().map(|lower| {
            hirsel_rc::sealed_source::SourceInput::capture(
                lower.observation_identity(),
                source.source.implementation_revision,
                definition.clone(),
            )
        });
        Ok((definition, input))
    }

    pub fn collect(
        engine: &mut QueryEngine,
        cache: &mut NativeDiscoveryCache,
        requested: &[DefPath],
    ) -> Result<Self, QueryError> {
        let roots = engine.native_source_roots(requested)?;
        let mut selected: BTreeSet<_> = roots.definitions.iter().cloned().collect();
        // CT-3 removes compile-time-only functions from runtime instance roots,
        // while semantic launch revisions still bind their source declarations.
        let semantic = engine.native_semantic_program(roots.definitions.clone())?;
        selected.extend(semantic.source_bindings().map(|(path, _, _)| path.clone()));
        let mut catalogue = Vec::new();
        for path in &selected {
            catalogue.push(cache.definition(engine, path, &selected)?);
        }
        let source_constants = catalogue
            .iter()
            .filter(|product| product.source.constant.is_some())
            .map(|product| product.source.definition.clone())
            .collect();
        let statics = engine.native_static_inputs(&selected)?;
        let extra = engine.native_static_requirements(&statics, &selected, &source_constants)?;
        Self::from_settled(
            &engine.signature_snapshot(),
            roots,
            catalogue,
            statics,
            extra,
        )
    }

    /// Assemble only completed immutable source products. Source checking,
    /// GEN-5, constants and Lower remain explicit front-query responsibilities.
    /// This ordinary metadata constructor grants no V or native C authority.
    pub fn from_settled(
        signatures: &hirsel_engine::SignatureSnapshot,
        roots: NativeSourceRoots,
        products: impl IntoIterator<Item = hirsel_engine::NativeDiscoveryProduct>,
        statics: hirsel_engine::NativeStaticInputs,
        extra: InstanceGraph,
    ) -> Result<Self, QueryError> {
        Self::assemble(
            signatures,
            RootInputs::Owned(Box::new(roots)),
            products,
            statics,
            extra,
        )
    }

    /// Share exactly the admitted front root and its original reservation.
    /// Declaration assembly still validates every selected product independently.
    pub fn from_front(facts: Arc<hirsel_engine::FrontNativeProducts>) -> Result<Self, QueryError> {
        Self::assemble(
            &facts.signatures,
            RootInputs::Front(facts.clone()),
            facts.catalogue.iter().map(|product| (**product).clone()),
            facts.statics.clone(),
            facts.extra.clone(),
        )
    }

    fn assemble(
        signatures: &hirsel_engine::SignatureSnapshot,
        roots: RootInputs,
        products: impl IntoIterator<Item = hirsel_engine::NativeDiscoveryProduct>,
        statics: hirsel_engine::NativeStaticInputs,
        extra: InstanceGraph,
    ) -> Result<Self, QueryError> {
        if roots
            .definitions
            .windows(2)
            .any(|paths| paths[0] >= paths[1])
        {
            return Err(invalid("noncanonical settled native root selection"));
        }
        let mut catalogue = BTreeMap::new();
        for product in products {
            if catalogue
                .insert(product.source.definition.clone(), product)
                .is_some()
            {
                return Err(invalid("duplicate settled native source declaration"));
            }
        }
        let selected: BTreeSet<_> = roots.definitions.iter().cloned().collect();
        if !selected.iter().all(|path| catalogue.contains_key(path))
            || catalogue.iter().any(|(path, product)| {
                !selected.contains(path)
                    && !compile_time_only(&product.source.signature, signatures)
            })
        {
            return Err(invalid(
                "settled native source catalogue differs from root selection",
            ));
        }
        let environment = &roots.environment;
        // Request environment only. Per-definition/private/body dependencies
        // belong to individual query actions, never a whole-program digest.
        let input = hirsel_lir::stage::InputIdentity {
            interface_version: environment.interface_version,
            digest: ContentHash::of_parts(
                "hirsel.native.environment.v1",
                [
                    environment.interface_version.to_le_bytes().as_slice(),
                    environment.compiler_version.as_bytes(),
                    environment.spec_hash.0.as_slice(),
                    environment.package_manifest_hash.0.as_slice(),
                    environment.builtin_signatures_hash.0.as_slice(),
                    roots.registry_revision.0.as_slice(),
                ],
            ),
        };
        let mut program = Program {
            input,
            types: signatures.types().clone(),
            memo: Arc::new(hirsel_types::TypeMemo::new()),
            definitions: BTreeMap::new(),
            signatures: roots.builtins.clone(),
            revisions: BTreeMap::new(),
            binders: BTreeMap::new(),
            operations: roots
                .operations
                .keys()
                .map(|key| {
                    (
                        key.definition.clone(),
                        OperationId {
                            definition: key.definition.clone(),
                            declaration_digest: key.declaration_digest,
                        },
                    )
                })
                .collect(),
            requirements: Vec::new(),
            owners: Default::default(),
            constants: BTreeMap::new(),
            instance_bodies: roots.instance_bodies.clone(),
        };
        for (path, signature) in &program.signatures {
            crate::native::adapt::add_owner(&mut program.owners, path, signature, None);
        }
        let mut context = DiscoveryContext::default();
        let mut sources = BTreeMap::new();
        let mut discoveries = Vec::new();
        let mut source_anchors = BTreeMap::new();
        let mut graph = InstanceGraph::default();
        for path in &catalogue.keys().cloned().collect::<Vec<_>>() {
            let product = catalogue
                .remove(path)
                .ok_or_else(|| invalid(format!("missing settled native declaration {path}")))?;
            let current = signatures
                .signature(hirsel_syntax::DefId::intern(path))
                .and_then(Result::ok)
                .ok_or_else(|| invalid(format!("missing settled native signature {path}")))?;
            if product.source.definition != *path
                || product.source.signature.definition.path() != path
                || product.source.source.syntax.item.definition != *path
                || !Arc::ptr_eq(current, &product.source.signature)
                || roots
                    .sources
                    .anchor(product.source.source.syntax.anchor)
                    .is_none()
            {
                return Err(invalid(format!(
                    "native declaration differs from its source snapshot: {path}"
                )));
            }
            if selected.contains(path)
                && let Some(declaration) =
                    super::source::source_declaration(&product).map_err(invalid)?
            {
                context.sources.insert(path.clone(), declaration);
            }
            source_anchors.insert(
                path.clone(),
                product.contracts.as_ref().map_or_else(
                    || vec![product.source.source.syntax.anchor],
                    |c| c.source_anchors.clone(),
                ),
            );
            if let Some(product) = product.product {
                discoveries.push(product);
            }
            graph
                .edges
                .extend(product.requirements.edges.iter().cloned());
            context.requirements.insert(
                path.clone(),
                product
                    .requirements
                    .edges
                    .iter()
                    .map(crate::native::adapt::requirement)
                    .collect(),
            );
            let source = product.source;
            program
                .signatures
                .insert(path.clone(), source.signature.clone());
            program
                .revisions
                .insert(path.clone(), source.source.implementation_revision);
            program.binders.insert(
                path.clone(),
                source.source.syntax.owner().site(NameSite::Item),
            );
            crate::native::adapt::add_owner(
                &mut program.owners,
                path,
                &source.signature,
                source.representation.as_ref(),
            );
            if let Some(value) = &source.constant {
                program.constants.insert(path.clone(), value.clone());
            }
            sources.insert(path.clone(), source);
        }
        for edge in &extra.edges {
            context
                .requirements
                .entry(edge.owner.clone())
                .or_default()
                .push(crate::native::adapt::requirement(edge));
        }
        graph.edges.extend(extra.edges);
        if let Some(site) = graph.growing_sites(signatures.types()).first() {
            return Err(invalid(format!(
                "instance expansion grows through an implicit instance at {site:?} (spec #87)"
            )));
        }
        program.constants.extend(statics.constants);
        program.signatures.extend(statics.generated_signatures);
        let executable = context.sources.keys().cloned().collect();
        let violations = hirsel_validate::validate_instance_catalog(
            &program.types,
            &program.instance_bodies,
            &program.signatures,
            &executable,
        );
        if !violations.is_empty() {
            return Err(invalid(format!("invalid instance bodies: {violations:?}")));
        }
        Ok(Self {
            roots,
            program,
            context,
            sources,
            source_anchors,
            discoveries,
            constant_definitions: statics.definitions,
        })
    }
}
fn compile_time_only(
    signature: &hirsel_types::SignatureRef,
    signatures: &hirsel_engine::SignatureSnapshot,
) -> bool {
    matches!(&signature.kind, hirsel_types::SignatureKind::Function(function)
        if function.generics.is_empty()
            && function.parameters.iter().map(|parameter| parameter.ty)
                .chain([function.result])
                .any(|ty| hirsel_types::mentions_compile_time_only(signatures.types(), ty)))
}

fn invalid(message: impl Into<String>) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "native declarations".into(),
        message: message.into(),
    })
}
