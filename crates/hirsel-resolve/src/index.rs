use hirsel_diag::{Diagnostic, Label, RuleId};
use hirsel_span::Span;
use hirsel_syntax::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Namespace {
    Type,
    Value,
}

/// Metadata only: indexing never reads a definition's body or infers a signature.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionEntry {
    pub definition: DefPath,
    pub namespace: Namespace,
    pub visibility: Visibility,
    pub span: Span,
}

/// A manifest import exposes a module head, not unqualified values (LANG-4).
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct DependencyExposure {
    pub head: String,
    pub package: PackageId,
    pub modules: Vec<String>,
}

/// Small build-input boundary pending the fixture format in spec issue #4.
/// Dependencies are explicit snapshots; no file discovery or source imports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestFixture {
    pub package: PackageId,
    pub definitions: Vec<DefinitionEntry>,
    pub exposures: Vec<DependencyExposure>,
    /// Covers the entire manifest, including dependency membership and visibility.
    pub revision: ContentHash,
}

#[derive(Clone, Debug)]
pub struct ManifestIndex {
    pub package: PackageId,
    pub revision: ContentHash,
    entries: Arc<BTreeMap<DefPath, DefinitionEntry>>,
    heads: Arc<BTreeMap<String, BTreeSet<(PackageId, Vec<String>)>>>,
    module_heads: Arc<BTreeMap<(PackageId, Vec<String>), BTreeSet<String>>>,
}

pub use hirsel_types::LookupError;

#[cfg(test)]
mod package_view_tests {
    use super::*;

    #[test]
    fn package_views_share_rows_and_keep_lookup_package_scoped() {
        let package = |name: &str| PackageId {
            name: name.into(),
            version: "1".into(),
        };
        let left = package("left");
        let right = package("right");
        let definition = |package, name: &str| {
            DefPath(ItemPath {
                package,
                modules: vec![],
                name: name.into(),
            })
        };
        let entries = [(left.clone(), "left_value"), (right.clone(), "right_value")].map(
            |(package, name)| DefinitionEntry {
                definition: definition(package, name),
                namespace: Namespace::Value,
                visibility: Visibility::Public,
                span: Span::default(),
            },
        );
        let left_definition = entries[0].definition.clone();
        let (index, diagnostics) = ManifestIndex::new(ManifestFixture {
            package: left,
            definitions: entries.into(),
            exposures: vec![],
            revision: ContentHash::default(),
        });
        assert!(diagnostics.is_empty());
        let view = index.for_package(right);
        for (selected, own, foreign) in [
            (&index, "left_value", "right_value"),
            (&view, "right_value", "left_value"),
        ] {
            let context = FileContext {
                package: selected.package.clone(),
                modules: vec![],
            };
            assert!(
                selected
                    .lookup_spelling(&context, None, &[own.into()])
                    .is_ok()
            );
            assert_eq!(
                selected.lookup_spelling(&context, None, &[foreign.into()]),
                Err(LookupError::Missing)
            );
        }
        assert!(Arc::ptr_eq(&index.entries, &view.entries));
        assert_eq!(index.entry(&left_definition), view.entry(&left_definition));
    }
}

impl ManifestFixture {
    /// Select declared dependency signatures for a phase fixture. Their module
    /// heads and visibility use the same index as ordinary source declarations.
    pub fn with_dependency_signatures<'a>(
        mut self,
        signatures: impl IntoIterator<Item = &'a hirsel_types::SignatureRef>,
    ) -> Self {
        for signature in signatures {
            let path = &signature.definition;
            if path.0.package == self.package {
                continue;
            }
            self.definitions.push(DefinitionEntry {
                definition: path.clone(),
                namespace: if matches!(signature.kind, hirsel_types::SignatureKind::Type(_)) {
                    Namespace::Type
                } else {
                    Namespace::Value
                },
                visibility: signature.metadata.visibility,
                span: Span::default(),
            });
            if let Some(head) = path.0.modules.first()
                && !head.starts_with('$')
            {
                self.exposures.push(DependencyExposure {
                    head: head.clone(),
                    package: path.0.package.clone(),
                    modules: vec![head.clone()],
                });
            }
        }
        self
    }

    pub fn from_files<'a>(
        package: PackageId,
        files: impl IntoIterator<Item = &'a SyntaxFile>,
        revision: ContentHash,
    ) -> Self {
        let definitions = files
            .into_iter()
            .flat_map(|file| {
                file.roots.iter().map(|id| {
                    let item = &file.tree.items[id.0 as usize];
                    DefinitionEntry {
                        definition: item.definition.clone(),
                        namespace: if matches!(item.kind, ItemKind::Type { .. }) {
                            Namespace::Type
                        } else {
                            Namespace::Value
                        },
                        visibility: item.visibility,
                        span: item.name_span,
                    }
                })
            })
            .collect();
        Self {
            package,
            definitions,
            exposures: vec![],
            revision,
        }
    }
}

impl ManifestIndex {
    /// Duplicate identity is an error, never repaired using a source position.
    pub fn new(mut fixture: ManifestFixture) -> (Self, Vec<Diagnostic>) {
        fixture.definitions.sort_by(|a, b| {
            a.definition.cmp(&b.definition).then_with(|| {
                (a.span.file, a.span.start, a.span.end).cmp(&(
                    b.span.file,
                    b.span.start,
                    b.span.end,
                ))
            })
        });
        let mut entries: BTreeMap<DefPath, DefinitionEntry> = BTreeMap::new();
        let mut diagnostics = vec![];
        for entry in fixture.definitions {
            let path = &entry.definition.0;
            let segment = |name: &str| !name.is_empty() && !name.contains(['/', '.', '@']);
            if !segment(&path.package.name)
                || path.package.version.is_empty()
                || !segment(&path.name)
                || !path.modules.iter().all(|m| segment(m))
            {
                diagnostics.push(Diagnostic::error(
                    const { RuleId::of("PROG-1") },
                    entry.span,
                    "definition path is not canonical",
                ));
                continue;
            }
            if let Some(first) = entries.get(&entry.definition) {
                let mut diagnostic = Diagnostic::error(
                    const { RuleId::of("LANG-7") },
                    entry.span,
                    "duplicate definition path; overloading is forbidden",
                );
                diagnostic.secondary.push(Label {
                    span: first.span,
                    message: "first definition of this path".into(),
                });
                diagnostics.push(diagnostic);
            } else {
                entries.insert(entry.definition.clone(), entry);
            }
        }
        let mut module_heads: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for definition in entries.keys() {
            for (i, head) in definition.0.modules.iter().enumerate() {
                module_heads
                    .entry((
                        definition.0.package.clone(),
                        definition.0.modules[..i].to_vec(),
                    ))
                    .or_default()
                    .insert(head.clone());
            }
        }
        let mut heads: BTreeMap<String, BTreeSet<(PackageId, Vec<String>)>> = BTreeMap::new();
        for exposure in fixture.exposures {
            heads
                .entry(exposure.head)
                .or_default()
                .insert((exposure.package, exposure.modules));
        }
        (
            Self {
                package: fixture.package,
                revision: fixture.revision,
                entries: Arc::new(entries),
                heads: Arc::new(heads),
                module_heads: Arc::new(module_heads),
            },
            diagnostics,
        )
    }

    /// Select a current package while sharing the same immutable declaration index.
    pub fn for_package(&self, package: PackageId) -> Self {
        Self {
            package,
            ..self.clone()
        }
    }

    /// Select the importing package's explicit module heads while sharing declarations.
    pub fn with_exposures(&self, exposures: &[DependencyExposure]) -> Self {
        let mut heads: BTreeMap<String, BTreeSet<(PackageId, Vec<String>)>> = BTreeMap::new();
        for exposure in exposures {
            heads
                .entry(exposure.head.clone())
                .or_default()
                .insert((exposure.package.clone(), exposure.modules.clone()));
        }
        Self {
            heads: Arc::new(heads),
            ..self.clone()
        }
    }

    pub fn entry(&self, definition: &DefPath) -> Option<&DefinitionEntry> {
        self.entries.get(definition)
    }

    pub fn has_head(&self, context: &FileContext, head: &str) -> bool {
        self.heads.contains_key(head)
            || [vec![], context.modules.clone()].iter().any(|modules| {
                self.module_heads
                    .get(&(context.package.clone(), modules.clone()))
                    .is_some_and(|heads| heads.contains(head))
            })
    }

    pub fn lookup(&self, context: &FileContext, path: &Path) -> Result<DefPath, LookupError> {
        let segments: Vec<_> = path
            .segments
            .iter()
            .map(|s| s.as_str().to_owned())
            .collect();
        self.lookup_spelling(context, path.package.as_ref(), &segments)
    }

    pub fn lookup_spelling(
        &self,
        context: &FileContext,
        package: Option<&PackageId>,
        segments: &[String],
    ) -> Result<DefPath, LookupError> {
        let (name, modules) = segments.split_last().ok_or(LookupError::Missing)?;
        let make = |package: PackageId, modules: Vec<String>| {
            DefPath(ItemPath {
                package,
                modules,
                name: name.clone(),
            })
        };
        let candidates = if let Some(package) = package {
            vec![make(package.clone(), modules.to_vec())]
        } else {
            // Own package wins over dependency exposure, even when it is private.
            let mut relative = context.modules.clone();
            relative.extend_from_slice(modules);
            let own = [
                make(context.package.clone(), relative),
                make(context.package.clone(), modules.to_vec()),
            ];
            if let Some(found) = own.iter().find(|d| self.entries.contains_key(*d)) {
                return self.access(context, found);
            }
            if segments.len() < 2 {
                return Err(LookupError::Missing);
            }
            let Some(exposed) = self.heads.get(&segments[0]) else {
                return Err(LookupError::Missing);
            };
            let candidates: Vec<_> = exposed
                .iter()
                .map(|(package, prefix)| {
                    let mut module = prefix.clone();
                    module.extend_from_slice(&segments[1..segments.len() - 1]);
                    make(package.clone(), module)
                })
                .collect();
            // Only identical core/std module paths share a namespace. Two std
            // majors remain ambiguous even if only one declares this member.
            let merged = exposed.len() == 2
                && exposed.iter().any(|(p, _)| p.package_is("core", "1"))
                && exposed.iter().any(|(p, _)| p.name_is("std"))
                && exposed
                    .iter()
                    .map(|(_, m)| m)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == 1;
            if exposed.len() > 1 && !merged {
                return Err(LookupError::Ambiguous(candidates));
            }
            candidates
        };
        for candidate in &candidates {
            if self.entries.contains_key(candidate) {
                return self.access(context, candidate);
            }
        }
        if package.is_none() {
            Err(LookupError::MissingMember(candidates))
        } else {
            Err(LookupError::Missing)
        }
    }

    pub fn access(
        &self,
        context: &FileContext,
        definition: &DefPath,
    ) -> Result<DefPath, LookupError> {
        if let Some(entry) = self.entries.get(definition)
            && entry.visibility == Visibility::Private
            && context.package != definition.0.package
        {
            return Err(LookupError::Invisible(definition.clone()));
        }
        for (i, module) in definition.0.modules.iter().enumerate() {
            if module == "internal"
                && (context.package != definition.0.package
                    || !context.modules.starts_with(&definition.0.modules[..i]))
            {
                return Err(LookupError::Invisible(definition.clone()));
            }
        }
        Ok(definition.clone())
    }
}

// Name tokens give precise locations even when an item begins with attributes.
// Hand-built trees without a token tape retain their supplied item origin.
