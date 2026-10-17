use hirsel_diag::{Diagnostic, Label, RuleId};
use hirsel_intern::Interner;
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
    reserved_values: Arc<BTreeMap<PackageId, BTreeSet<String>>>,
    module_heads: Arc<BTreeMap<(PackageId, Vec<String>), BTreeSet<String>>>,
}

pub use hirsel_types::LookupError;

#[cfg(test)]
mod package_view_tests {
    use super::*;

    #[test]
    fn package_views_share_rows_and_keep_reservations_package_scoped() {
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
        assert!(index.reserves_value("left_value"));
        assert!(!index.reserves_value("right_value"));
        assert!(view.reserves_value("right_value"));
        assert!(!view.reserves_value("left_value"));
        assert!(Arc::ptr_eq(&index.entries, &view.entries));
        assert_eq!(index.entry(&left_definition), view.entry(&left_definition));
    }
}

impl ManifestFixture {
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
        let mut reserved_values: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for entry in entries
            .values()
            .filter(|entry| entry.namespace == Namespace::Value)
        {
            reserved_values
                .entry(entry.definition.0.package.clone())
                .or_default()
                .insert(entry.definition.0.name.clone());
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
                reserved_values: Arc::new(reserved_values),
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

    /// Same-package values reserve local names throughout the package, including
    /// other modules. Types and module heads live in distinct namespaces.
    pub fn reserves_value(&self, name: &str) -> bool {
        self.reserved_values
            .get(&self.package)
            .is_some_and(|values| values.contains(name))
    }

    pub fn has_head(&self, context: &FileContext, head: &str) -> bool {
        self.heads.contains_key(head)
            || [vec![], context.modules.clone()].iter().any(|modules| {
                self.module_heads
                    .get(&(context.package.clone(), modules.clone()))
                    .is_some_and(|heads| heads.contains(head))
            })
    }

    pub fn lookup(
        &self,
        context: &FileContext,
        path: &Path,
        symbols: &Interner,
    ) -> Result<DefPath, LookupError> {
        let segments: Option<Vec<_>> = path
            .segments
            .iter()
            .map(|s| symbols.resolve(*s).map(str::to_owned))
            .collect();
        self.lookup_spelling(
            context,
            path.package.as_ref(),
            &segments.ok_or(LookupError::Missing)?,
        )
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
            // Ambiguity is about the head, regardless of which member exists.
            if exposed.len() > 1 {
                return Err(LookupError::Ambiguous);
            }
            exposed
                .iter()
                .map(|(package, prefix)| {
                    let mut module = prefix.clone();
                    module.extend_from_slice(&segments[1..segments.len() - 1]);
                    make(package.clone(), module)
                })
                .collect()
        };
        for candidate in candidates {
            if self.entries.contains_key(&candidate) {
                return self.access(context, &candidate);
            }
        }
        Err(LookupError::Missing)
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
