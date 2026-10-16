//! Driver-owned syntax snapshots, header isolation and explicit manifest composition.
use crate::*;
use hirsel_resolve::{DefinitionEntry, ManifestFixture, ManifestIndex, Namespace};
use hirsel_syntax::{ItemKind, TypeBody, Visibility};
use hirsel_types::{DeclarationSite, DefinitionMetadata, Metadata};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc};

#[derive(Clone, Debug)]
pub struct ResolverSnapshot {
    pub file: Arc<SyntaxFile>,
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
    pub(crate) constant_values:
        crate::layered::Layered<DefPath, crate::comptime::EvaluatedConstant>,
    pub(crate) representations:
        crate::layered::Layered<DefPath, crate::representation::OwnerRecord>,
    pub mode_diagnostics: Vec<(Diagnostic, String)>,
    module_declarations: BTreeMap<DefPath, ModuleDeclaration>,
    module_input: RefCell<Option<Rc<ModuleInput>>>,
    pub manifest_revision: ContentHash,
    pub resolver_outputs: crate::layered::Layered<DefPath, ResolverSnapshot>,
    pub signature_resolutions: crate::layered::Layered<DefPath, ResolverSnapshot>,
    /// Additional explicit build inputs; callers must update the manifest revision.
    pub exposures: Vec<hirsel_resolve::DependencyExposure>,
    /// Ordinary dependency heads scoped to their importing package.
    pub package_exposures:
        BTreeMap<hirsel_syntax::PackageId, Vec<hirsel_resolve::DependencyExposure>>,
    pub signature_metadata: Metadata,
}
struct ModuleDeclaration {
    entry: DefinitionEntry,
    metadata: DefinitionMetadata,
    sites: Rc<[DeclarationSite]>,
}
struct ModuleInput {
    index: ManifestIndex,
    metadata: Metadata,
    package_indexes: BTreeMap<hirsel_syntax::PackageId, ManifestIndex>,
}
impl CompilerState {
    pub(crate) fn set_declarations(
        &mut self,
        declarations: impl IntoIterator<Item = (Arc<SyntaxFile>, ItemId)>,
    ) {
        let mut declarations: Vec<_> = declarations.into_iter().collect();
        declarations.sort_by_key(|(file, id)| {
            (file.file, file.tree.items[id.0 as usize].origin.span.start)
        });
        let mut rows: BTreeMap<
            DefPath,
            (DefinitionEntry, DefinitionMetadata, Vec<DeclarationSite>),
        > = BTreeMap::new();
        for (file, id) in declarations {
            hirsel_intern::work::count(hirsel_intern::work::Counter::ModuleDeclarationRows);
            let item = &file.tree.items[id.0 as usize];
            let metadata = DefinitionMetadata {
                public: item.visibility != Visibility::Private,
                alias: matches!(
                    item.kind,
                    ItemKind::Type {
                        body: TypeBody::Alias(_),
                        ..
                    }
                ),
            };
            let row = rows.entry(item.definition.clone()).or_insert_with(|| {
                (
                    DefinitionEntry {
                        definition: item.definition.clone(),
                        namespace: if matches!(item.kind, ItemKind::Type { .. }) {
                            Namespace::Type
                        } else {
                            Namespace::Value
                        },
                        visibility: item.visibility,
                        span: item.name_span,
                    },
                    metadata.clone(),
                    vec![],
                )
            });
            // Preserve the old first-entry/last-metadata policy for duplicates.
            row.1 = metadata;
            row.2.push(DeclarationSite {
                definition: item.definition.clone(),
                name_span: item.name_span,
            });
        }
        self.module_declarations = rows
            .into_iter()
            .map(|(path, (entry, metadata, sites))| {
                (
                    path,
                    ModuleDeclaration {
                        entry,
                        metadata,
                        sites: sites.into(),
                    },
                )
            })
            .collect();
        *self.module_input.get_mut() = None;
    }
    fn module_input(&self) -> Rc<ModuleInput> {
        if let Some(input) = self.module_input.borrow().as_ref()
            && input.index.revision == self.manifest_revision
        {
            return input.clone();
        }
        let mut metadata = self.signature_metadata.clone();
        let roles = crate::registry::nominal_roles();
        metadata.unsafe_capability = roles.unsafe_capability;
        metadata.result_type = roles.result_type;
        metadata.tasks_capability = roles.tasks_capability;
        metadata.scope_capability = roles.scope_capability;
        metadata.heap = roles.heap;
        // Source declaration sites have one indexed representation below.
        metadata.declarations.clear();
        let mut entries = vec![];
        for (path, row) in &self.module_declarations {
            for _ in row.sites.iter() {
                hirsel_intern::work::count(hirsel_intern::work::Counter::ModuleDeclarationRows);
            }
            metadata
                .definitions
                .insert(path.clone(), row.metadata.clone());
            entries.push(row.entry.clone());
        }
        let core_heads: std::collections::BTreeSet<_> = crate::registry::BuiltinRegistry::new()
            .signatures()
            .filter_map(|s| s.definition.0.modules.first().cloned())
            .filter(|head| !head.starts_with('$'))
            .collect();
        let complete_heads = |mut heads: Vec<hirsel_resolve::DependencyExposure>| {
            for head in &core_heads {
                if !heads.iter().any(|e| &e.head == head) {
                    heads.push(hirsel_resolve::DependencyExposure {
                        head: head.clone(),
                        package: crate::registry::core_path("Result").0.package,
                        modules: vec![head.clone()],
                    });
                }
            }
            heads
        };
        let index = ManifestIndex::new(ManifestFixture {
            package: crate::default_context().package,
            definitions: entries,
            exposures: complete_heads(self.exposures.clone()),
            revision: self.manifest_revision,
        })
        .0;
        let package_indexes = self
            .package_exposures
            .iter()
            .map(|(package, exposures)| {
                let mut heads = exposures.clone();
                heads.extend(self.exposures.iter().cloned());
                (
                    package.clone(),
                    index
                        .for_package(package.clone())
                        .with_exposures(&complete_heads(heads)),
                )
            })
            .collect();
        let input = Rc::new(ModuleInput {
            index,
            package_indexes,
            metadata,
        });
        *self.module_input.borrow_mut() = Some(input.clone());
        input
    }
    pub(crate) fn index(&self, package: hirsel_syntax::PackageId) -> ManifestIndex {
        let input = self.module_input();
        input
            .package_indexes
            .get(&package)
            .unwrap_or(&input.index)
            .for_package(package)
    }
    fn declaration_sites(&self, path: &DefPath) -> Rc<[DeclarationSite]> {
        self.module_declarations
            .get(path)
            .map_or_else(|| Rc::from([]), |row| row.sites.clone())
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
        file: Arc<SyntaxFile>,
        id: ItemId,
        symbols: &Interner,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Resolution>, QueryError> {
        let item = &file.tree.items[id.0 as usize];
        let index = self.state.borrow().index(item.definition.0.package.clone());
        let result = hirsel_resolve::resolve_view(
            &file,
            hirsel_resolve::ResolveView::Item(id),
            symbols,
            &index,
            provider,
            &resolver_expected(),
        );
        self.state.borrow_mut().resolver_outputs.insert(
            item.definition.clone(),
            ResolverSnapshot {
                file: file.clone(),
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
        candidates: &[(Arc<SyntaxFile>, ItemId)],
        symbols: &Interner,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        // Visit the later duplicate first so normalization reports overloading before
        // trying unavailable library types in the first declaration's header.
        let (file, id) = candidates
            .get(1)
            .or_else(|| candidates.first())
            .ok_or(QueryError::NotImplemented)?;
        self.signature_with_candidates(file.clone(), *id, symbols, provider, Some(candidates))
    }
    fn signature(
        &self,
        file: Arc<SyntaxFile>,
        id: ItemId,
        symbols: &Interner,
        provider: &mut dyn SignatureProvider,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        self.signature_with_candidates(file, id, symbols, provider, None)
    }
}
impl SignatureAdapter {
    fn signature_with_candidates(
        &self,
        file: Arc<SyntaxFile>,
        id: ItemId,
        symbols: &Interner,
        provider: &mut dyn SignatureProvider,
        candidates: Option<&[(Arc<SyntaxFile>, ItemId)]>,
    ) -> Result<QueryOutput<Signature>, QueryError> {
        let item = &file.tree.items[id.0 as usize];
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
        let (index, module, declaration_sites) = {
            let state = self.state.borrow();
            (
                state.index(context.package.clone()),
                state.module_input(),
                state.declaration_sites(&item.definition),
            )
        };
        // Retained snapshots can be older than edited peers; candidates are in
        // current source order, so only this owner's indexed sites are replaced.
        let declaration_sites: Rc<[DeclarationSite]> =
            candidates.map_or(declaration_sites, |candidates| {
                candidates
                    .iter()
                    .map(|(file, id)| {
                        let item = &file.tree.items[id.0 as usize];
                        DeclarationSite {
                            definition: item.definition.clone(),
                            name_span: item.name_span,
                        }
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
                &file,
                hirsel_resolve::ResolveView::Header(id),
                symbols,
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
            self.state.borrow_mut().signature_resolutions.insert(
                item.definition.clone(),
                ResolverSnapshot {
                    file: file.clone(),
                    output: resolved.query,
                },
            );
            if crate::queries::has_errors(&diagnostics) {
                return Err(QueryError::Diagnostics(diagnostics));
            }
        }
        let output = self.phases.normalize_signature(
            hirsel_check_protocol::header::Input {
                tree: &file.tree,
                item: id,
                symbols,
                context: &context,
                resolution: &resolution,
                metadata: &module.metadata,
                declaration_sites: &declaration_sites,
            },
            provider,
        )?;
        if let Some(owner) = output.owner {
            self.state.borrow_mut().representations.insert(
                item.definition.clone(),
                crate::representation::OwnerRecord {
                    text_hash: item.text_hash,
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

#[cfg(test)]
mod module_input_tests {
    use super::*;

    #[test]
    fn explicit_manifest_revision_refreshes_shared_inputs() {
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), engine_only_adapters());
            let context = default_context();
            let file = engine
                .sources
                .add("manifest.hir", "fn f() {}".into())
                .unwrap();
            engine.parse_file(file, &context).unwrap();
            let state = engine.adapters.compiler_state.as_ref().unwrap();
            let first = state.borrow().module_input();
            assert!(!first.index.has_head(&context, "external"));
            assert!(Rc::ptr_eq(&first, &state.borrow().module_input()));
            {
                let mut state = state.borrow_mut();
                state.exposures.push(hirsel_resolve::DependencyExposure {
                    head: "external".into(),
                    package: hirsel_syntax::PackageId {
                        name: "dependency".into(),
                        version: "1".into(),
                    },
                    modules: vec![],
                });
                state.manifest_revision = ContentHash::of_bytes(b"updated explicit inputs");
            }
            let second = state.borrow().module_input();
            assert!(second.index.has_head(&context, "external"));
            assert!(!first.index.has_head(&context, "external"));
            assert!(Rc::ptr_eq(&second, &state.borrow().module_input()));
        });
    }
}
