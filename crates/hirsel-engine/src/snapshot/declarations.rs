//! The declaration index: every indexed item by file and by definition,
//! updated per changed file (design §1(b), "Snapshot").
//!
//! Indexing a file updates the rows of exactly the definitions it owned or now
//! owns and the item lists of the files those definitions stand in. Each row
//! keeps its candidates' header hashes and `DeclarationCandidates`, the input
//! every query keyed on the definition reads: H(count, then (file path, header
//! hash) per candidate in source order). The module input resolution and
//! signature normalization read (`Declarations`) is frozen from the rows once
//! per revision, when a row or an explicit input changed.
use crate::*;
use hirsel_resolve::{
    DefinitionEntry, DependencyExposure, ManifestFixture, ManifestIndex, Namespace,
};
use hirsel_span::{Anchor, Span};
use hirsel_syntax::{ItemKind, PackageId, TypeBody, Visibility};
use hirsel_types::{DeclarationSite, DefinitionMetadata, Metadata};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

/// The explicit build inputs of module resolution, beside the indexed items.
#[derive(Clone, Debug, Default)]
pub struct ModuleInputs {
    /// Module heads every package sees.
    pub exposures: Vec<DependencyExposure>,
    /// Ordinary dependency heads scoped to their importing package.
    pub package_exposures: BTreeMap<PackageId, Vec<DependencyExposure>>,
    pub signature_metadata: Metadata,
    pub manifest_revision: ContentHash,
}

/// One definition's declarations.
#[derive(Clone, Debug)]
struct Row {
    /// The first candidate's entry, as resolution indexes it.
    entry: DefinitionEntry,
    /// The last candidate's metadata (main's first-entry, last-metadata rule).
    metadata: DefinitionMetadata,
    sites: Arc<[DeclarationSite]>,
    /// Each candidate's header hash, in occurrence order.
    headers: Arc<[ContentHash]>,
    /// `DeclarationCandidates(d)`.
    candidates: ContentHash,
    /// This row's part of the manifest identity hash.
    identity: [u64; 4],
}

impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        fn site(s: &DeclarationSite) -> (&DefPath, Span) {
            (&s.definition, s.name_span)
        }
        self.entry == other.entry
            && (self.metadata.public, self.metadata.alias)
                == (other.metadata.public, other.metadata.alias)
            && self.sites.iter().map(site).eq(other.sites.iter().map(site))
            && self.headers == other.headers
            && self.candidates == other.candidates
    }
}

/// A row as the std image holds it.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct RowImage {
    definition: DefPath,
    types: bool,
    visibility: Visibility,
    span: Span,
    metadata: DefinitionMetadata,
    sites: Vec<DeclarationSite>,
    headers: Vec<ContentHash>,
    candidates: ContentHash,
    identity: [u64; 4],
}
impl RowImage {
    pub(crate) fn definition(&self) -> &DefPath {
        &self.definition
    }
    /// Each candidate's name site, in occurrence order.
    pub(crate) fn sites(&self) -> &[DeclarationSite] {
        &self.sites
    }
}
impl From<&Row> for RowImage {
    fn from(row: &Row) -> Self {
        Self {
            definition: row.entry.definition.clone(),
            types: row.entry.namespace == Namespace::Type,
            visibility: row.entry.visibility,
            span: row.entry.span,
            metadata: row.metadata.clone(),
            sites: row.sites.to_vec(),
            headers: row.headers.to_vec(),
            candidates: row.candidates,
            identity: row.identity,
        }
    }
}
impl From<RowImage> for Row {
    fn from(row: RowImage) -> Self {
        Self {
            entry: DefinitionEntry {
                definition: row.definition,
                namespace: if row.types {
                    Namespace::Type
                } else {
                    Namespace::Value
                },
                visibility: row.visibility,
                span: row.span,
            },
            metadata: row.metadata,
            sites: row.sites.into(),
            headers: row.headers.into(),
            candidates: row.candidates,
            identity: row.identity,
        }
    }
}

/// The module input resolution and signature normalization read, frozen.
pub struct Declarations {
    rows: Arc<FrozenRows>,
    files: BTreeMap<FileId, Arc<[Arc<ItemSyntax>]>>,
    anchors: HashMap<Anchor, FileId>,
}
/// What the rows and the explicit inputs alone decide: frozen again only when
/// a row or an input changes, so a body edit, which moves no row, reuses it.
struct FrozenRows {
    index: ManifestIndex,
    package_indexes: BTreeMap<PackageId, ManifestIndex>,
    metadata: Metadata,
    sites: BTreeMap<DefPath, Arc<[DeclarationSite]>>,
}
impl Declarations {
    /// `package`'s resolution index.
    pub fn index(&self, package: PackageId) -> ManifestIndex {
        self.rows
            .package_indexes
            .get(&package)
            .unwrap_or(&self.rows.index)
            .for_package(package)
    }
    pub fn metadata(&self) -> &Metadata {
        &self.rows.metadata
    }
    /// The items of `syntax`'s file in position order; a derived item reads
    /// its source's.
    pub fn siblings(&self, syntax: &ItemSyntax) -> Arc<[Arc<ItemSyntax>]> {
        self.anchors
            .get(&syntax.anchor)
            .and_then(|file| self.files.get(file))
            .cloned()
            .unwrap_or_else(|| Arc::from([]))
    }
    /// Every declaration site of `path`, in occurrence order.
    pub fn sites(&self, path: &DefPath) -> Arc<[DeclarationSite]> {
        self.rows
            .sites
            .get(path)
            .cloned()
            .unwrap_or_else(|| Arc::from([]))
    }
}

/// The index of every indexed item.
#[derive(Clone, Default)]
pub struct DeclarationIndex {
    /// Changes to signature inputs, independent of source body revisions.
    revision: u64,
    rows: BTreeMap<DefPath, Row>,
    files: BTreeMap<FileId, Arc<[Arc<ItemSyntax>]>>,
    anchors: HashMap<Anchor, FileId>,
    /// The FileIds listing items, by source path: each indexing of a
    /// source adds a snapshot at a new FileId.
    paths: HashMap<std::path::PathBuf, std::collections::BTreeSet<FileId>>,
    /// The sum of every row's identity part, which a row change updates.
    identity: [u64; 4],
    inputs: ModuleInputs,
    frozen: OnceLock<Arc<Declarations>>,
    /// The rows' part of `frozen`, which outlives a change of files only.
    frozen_rows: OnceLock<Arc<FrozenRows>>,
}

/// A CT-5 instance's generated item (`instance$<hash>`, generation.rs): its
/// own source, never indexed, so making one leaves the module input alone.
fn generated(path: &DefPath) -> bool {
    path.0.name.starts_with("instance$")
}
/// Where an item stands: its file, the file's path and its anchor's base.
fn place(sources: &SourceMap, syntax: &ItemSyntax) -> (FileId, std::path::PathBuf, u32) {
    let (file, base) = sources
        .anchor(syntax.anchor)
        .expect("an indexed item's anchor is placed");
    (file, sources.file(file).unwrap().path.clone(), base)
}
/// The package-relative path of the source an item stands in: candidates
/// hash it rather than where the package lies, so persisted reads of them
/// hold wherever the package is read from.
pub(crate) fn logical(sources: &SourceMap, syntax: &ItemSyntax) -> Vec<u8> {
    let (file, _) = sources
        .anchor(syntax.anchor)
        .expect("an indexed item's anchor is placed");
    sources
        .file(file)
        .unwrap()
        .logical_path()
        .as_os_str()
        .as_encoded_bytes()
        .to_vec()
}
/// An item's header: its text up to its body, positioned in its item.
pub(crate) fn header_hash(sources: &SourceMap, syntax: &ItemSyntax) -> ContentHash {
    let header = crate::queries::header_span(syntax);
    ContentHash::of_parts(
        "hirsel.signature.header.v2",
        [
            sources.slice(header).unwrap_or_default().as_bytes(),
            header.start.to_le_bytes().as_slice(),
        ],
    )
}
fn lanes(hash: ContentHash) -> [u64; 4] {
    std::array::from_fn(|i| u64::from_le_bytes(hash.0[i * 8..i * 8 + 8].try_into().unwrap()))
}

impl DeclarationIndex {
    /// Index `file`'s items again after `affected` definitions changed:
    /// `items` holds every definition's candidates in occurrence order.
    pub(crate) fn update(
        &mut self,
        file: FileId,
        affected: &std::collections::BTreeSet<DefPath>,
        items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
        sources: &SourceMap,
    ) {
        // The files whose item lists can change: this one, and each file a
        // changed definition's candidate stands in.
        // A source indexed again stands at a new FileId; its earlier
        // snapshots list nothing now.
        let mut files = self.snapshots(&sources.file(file).unwrap().path);
        files.insert(file);
        for path in affected {
            for candidate in items.get(path).into_iter().flatten() {
                files.insert(place(sources, candidate).0);
            }
        }
        for &changed in &files {
            let listed: Vec<Arc<ItemSyntax>> = if changed == file {
                affected
                    .iter()
                    .flat_map(|path| items.get(path).into_iter().flatten())
                    .filter(|candidate| place(sources, candidate).0 == file)
                    .cloned()
                    .collect()
            } else {
                // The same items, each as the definition now holds it.
                self.files
                    .get(&changed)
                    .into_iter()
                    .flat_map(|old| old.iter())
                    .filter_map(|old| {
                        items
                            .get(&old.item.definition)?
                            .iter()
                            .find(|c| c.anchor == old.anchor)
                            .cloned()
                    })
                    .collect()
            };
            self.set_file(changed, listed, sources);
        }
        for path in affected {
            self.set_row(path, items.get(path).map(Vec::as_slice), sources);
        }
    }
    /// `image`'s index, built over the same items at the same FileIds,
    /// under this index's explicit inputs.
    pub(crate) fn adopt(&mut self, image: &DeclarationIndex) {
        let inputs = std::mem::take(&mut self.inputs);
        let revision = self.revision + 1;
        *self = Self {
            inputs,
            revision,
            frozen: OnceLock::new(),
            frozen_rows: OnceLock::new(),
            ..image.clone()
        };
    }
    /// `path`'s row as the std image holds it.
    pub(crate) fn row_image(&self, path: &DefPath) -> Option<RowImage> {
        self.rows.get(path).map(RowImage::from)
    }
    /// Set `path`'s row from an image, whose items stand in sources not
    /// indexed here (`place_file` lists them once parsed).
    pub(crate) fn adopt_row(&mut self, path: &DefPath, row: impl Into<RowImage>) {
        let row: Row = Row::from(row.into());
        self.replace_row(path, Some(row));
    }
    /// Every row of `image`, added to this index.
    pub(crate) fn adopt_rows(&mut self, image: &DeclarationIndex) {
        for (path, row) in &image.rows {
            self.replace_row(path, Some(row.clone()));
        }
    }
    /// List `file`'s items, which an image's rows already describe: a source
    /// of an image package parsed on demand.
    pub(crate) fn place_file(
        &mut self,
        file: FileId,
        listed: Vec<Arc<ItemSyntax>>,
        sources: &SourceMap,
    ) {
        self.set_file(file, listed, sources);
    }
    /// `path`'s declaration sites, in occurrence order.
    pub(crate) fn sites(&self, path: &DefPath) -> &[DeclarationSite] {
        self.rows.get(path).map_or(&[], |row| &row.sites)
    }
    /// The paths with rows.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &DefPath> {
        self.rows.keys()
    }
    /// Index every item afresh, except generated instances.
    pub(crate) fn rebuild(
        &mut self,
        items: &BTreeMap<DefPath, Vec<Arc<ItemSyntax>>>,
        sources: &SourceMap,
    ) {
        let inputs = std::mem::take(&mut self.inputs);
        let revision = self.revision + 1;
        *self = Self {
            inputs,
            revision,
            ..Self::default()
        };
        let mut files: BTreeMap<FileId, Vec<Arc<ItemSyntax>>> = BTreeMap::new();
        for (path, candidates) in items.iter().filter(|(path, _)| !generated(path)) {
            for candidate in candidates {
                files
                    .entry(place(sources, candidate).0)
                    .or_default()
                    .push(candidate.clone());
            }
            self.set_row(path, Some(candidates), sources);
        }
        for (file, listed) in files {
            self.set_file(file, listed, sources);
        }
    }
    /// `file`'s items, given in DefPath then occurrence order, by position.
    fn set_file(&mut self, file: FileId, mut listed: Vec<Arc<ItemSyntax>>, sources: &SourceMap) {
        listed.sort_by(|a, b| {
            a.item
                .definition
                .cmp(&b.item.definition)
                .then(a.occurrence.cmp(&b.occurrence))
        });
        listed.sort_by_key(|item| place(sources, item).2);
        if let Some(old) = self.files.get(&file) {
            for item in old.iter() {
                if self.anchors.get(&item.anchor) == Some(&file) {
                    self.anchors.remove(&item.anchor);
                }
            }
        }
        for item in &listed {
            self.anchors.insert(item.anchor, file);
        }
        let path = sources.file(file).unwrap().path.clone();
        let snapshots = self.paths.entry(path.clone()).or_default();
        if listed.is_empty() {
            snapshots.remove(&file);
            if snapshots.is_empty() {
                self.paths.remove(&path);
            }
        } else {
            snapshots.insert(file);
        }
        let changed = self.files.get(&file).is_none_or(|old| {
            old.len() != listed.len() || old.iter().zip(&listed).any(|(a, b)| !Arc::ptr_eq(a, b))
        });
        if listed.is_empty() {
            self.files.remove(&file);
        } else {
            self.files.insert(file, listed.into());
        }
        if changed {
            self.frozen = OnceLock::new();
        }
    }
    fn set_row(
        &mut self,
        path: &DefPath,
        candidates: Option<&[Arc<ItemSyntax>]>,
        sources: &SourceMap,
    ) {
        let row = candidates.filter(|c| !c.is_empty()).map(|candidates| {
            work::count(work::Counter::ModuleDeclarationRows);
            let first = &candidates[0].item;
            let last = &candidates[candidates.len() - 1].item;
            let metadata = DefinitionMetadata {
                public: last.visibility != Visibility::Private,
                alias: matches!(
                    last.kind,
                    ItemKind::Type {
                        body: TypeBody::Alias(_),
                        ..
                    }
                ),
            };
            let headers: Arc<[ContentHash]> =
                candidates.iter().map(|c| header_hash(sources, c)).collect();
            let mut parts = vec![(candidates.len() as u64).to_le_bytes().to_vec()];
            for (candidate, header) in candidates.iter().zip(headers.iter()) {
                parts.push(logical(sources, candidate));
                parts.push(header.0.to_vec());
            }
            let kind = match &first.kind {
                ItemKind::Function { .. } => 0,
                ItemKind::Const { .. } => 1,
                ItemKind::Type {
                    body: TypeBody::Alias(_),
                    ..
                } => 2,
                ItemKind::Type { .. } => 3,
            };
            let public = first.visibility != Visibility::Private;
            Row {
                entry: DefinitionEntry {
                    definition: first.definition.clone(),
                    namespace: if matches!(first.kind, ItemKind::Type { .. }) {
                        Namespace::Type
                    } else {
                        Namespace::Value
                    },
                    visibility: first.visibility,
                    span: first.name_span,
                },
                metadata,
                sites: candidates
                    .iter()
                    .map(|c| DeclarationSite {
                        definition: c.item.definition.clone(),
                        name_span: c.item.name_span,
                    })
                    .collect(),
                headers,
                candidates: ContentHash::of_parts(
                    "hirsel.declaration-candidates.v1",
                    parts.iter().map(Vec::as_slice),
                ),
                identity: lanes(ContentHash::of_parts(
                    "hirsel.manifest.identity.v2",
                    [path.fingerprint().0.as_slice(), &[u8::from(public), kind]],
                )),
            }
        });
        self.replace_row(path, row);
    }
    fn replace_row(&mut self, path: &DefPath, row: Option<Row>) {
        let old = match &row {
            Some(row) => self.rows.insert(path.clone(), row.clone()),
            None => self.rows.remove(path),
        };
        if old == row {
            return;
        }
        self.revision += 1;
        if let Some(old) = &old {
            for (sum, part) in self.identity.iter_mut().zip(old.identity) {
                *sum = sum.wrapping_sub(part);
            }
        }
        if let Some(row) = &row {
            for (sum, part) in self.identity.iter_mut().zip(row.identity) {
                *sum = sum.wrapping_add(part);
            }
        }
        self.frozen = OnceLock::new();
        self.frozen_rows = OnceLock::new();
    }
    /// The FileIds whose items stand in snapshots of the source at `path`.
    pub(crate) fn snapshots(&self, path: &std::path::Path) -> std::collections::BTreeSet<FileId> {
        self.paths.get(path).cloned().unwrap_or_default()
    }
    /// Current items of one logical source, including derived fact helpers.
    /// Consult the existing source-to-snapshot index, never all program items.
    pub(crate) fn source_definitions(&self, path: &std::path::Path) -> Vec<DefPath> {
        self.paths
            .get(path)
            .into_iter()
            .flatten()
            .flat_map(|file| {
                self.files
                    .get(file)
                    .into_iter()
                    .flat_map(|items| items.iter())
            })
            .map(|item| item.item.definition.clone())
            .collect()
    }
    /// The manifest identity: every definition's path, visibility and kind.
    pub fn identity(&self) -> ContentHash {
        let bytes: Vec<u8> = self
            .identity
            .iter()
            .flat_map(|lane| lane.to_le_bytes())
            .collect();
        ContentHash::of_parts("hirsel.manifest.identities.v2", [bytes.as_slice()])
    }
    /// `DeclarationCandidates(path)`, or None for a definition with no item.
    pub fn candidates(&self, path: &DefPath) -> Option<ContentHash> {
        self.rows.get(path).map(|row| row.candidates)
    }
    /// The header hashes of `path`'s candidates, in occurrence order.
    pub fn headers(&self, path: &DefPath) -> Option<&[ContentHash]> {
        self.rows.get(path).map(|row| &*row.headers)
    }
    /// The frozen module input, built after a row or an input changed.
    pub fn frozen(&self) -> Arc<Declarations> {
        self.frozen
            .get_or_init(|| {
                Arc::new(Declarations {
                    rows: self
                        .frozen_rows
                        .get_or_init(|| Arc::new(self.freeze()))
                        .clone(),
                    files: self.files.clone(),
                    anchors: self.anchors.clone(),
                })
            })
            .clone()
    }
    pub fn inputs(&self) -> &ModuleInputs {
        &self.inputs
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    /// The explicit inputs, to change; the next freeze reads them.
    pub fn inputs_mut(&mut self) -> &mut ModuleInputs {
        self.revision += 1;
        self.frozen = OnceLock::new();
        self.frozen_rows = OnceLock::new();
        &mut self.inputs
    }
    fn freeze(&self) -> FrozenRows {
        let mut metadata = self.inputs.signature_metadata.clone();
        let roles = crate::registry::nominal_roles();
        metadata.unsafe_capability = roles.unsafe_capability;
        metadata.result_type = roles.result_type;
        metadata.tasks_capability = roles.tasks_capability;
        metadata.scope_capability = roles.scope_capability;
        metadata.heap = roles.heap;
        // Source declaration sites have one indexed representation below.
        metadata.declarations.clear();
        let mut entries = Vec::with_capacity(self.rows.len());
        for (path, row) in &self.rows {
            work::count(work::Counter::ModuleDeclarationRows);
            metadata
                .definitions
                .insert(path.clone(), row.metadata.clone());
            entries.push(row.entry.clone());
        }
        let registry = crate::registry::BuiltinRegistry::new();
        // Index core declarations alongside source metadata, so merged heads
        // resolve members through the same lookup and visibility path.
        entries.extend(registry.signatures().filter_map(|signature| {
            if signature
                .definition
                .path()
                .0
                .modules
                .iter()
                .any(|m| m.starts_with('$'))
            {
                return None;
            }
            Some(DefinitionEntry {
                definition: signature.definition.path().clone(),
                namespace: if matches!(signature.kind, hirsel_types::SignatureKind::Type(_)) {
                    Namespace::Type
                } else {
                    Namespace::Value
                },
                visibility: Visibility::Public,
                span: Span::default(),
            })
        }));
        let core_heads: std::collections::BTreeSet<_> = registry
            .signatures()
            .filter_map(|s| s.definition.path().0.modules.first().cloned())
            .filter(|head| !head.starts_with('$'))
            .collect();
        let complete_heads = |mut heads: Vec<DependencyExposure>| {
            for head in &core_heads {
                if !heads
                    .iter()
                    .any(|e| &e.head == head && !e.package.name_is("std"))
                {
                    heads.push(DependencyExposure {
                        head: head.clone(),
                        package: DefPath::core("Result").0.package,
                        modules: vec![head.clone()],
                    });
                }
            }
            heads
        };
        let index = ManifestIndex::new(ManifestFixture {
            package: crate::default_context().package,
            definitions: entries,
            exposures: complete_heads(self.inputs.exposures.clone()),
            revision: self.inputs.manifest_revision,
        })
        .0;
        let package_indexes = self
            .inputs
            .package_exposures
            .iter()
            .map(|(package, exposures)| {
                let mut heads = exposures.clone();
                heads.extend(self.inputs.exposures.iter().cloned());
                (
                    package.clone(),
                    index
                        .for_package(package.clone())
                        .with_exposures(&complete_heads(heads)),
                )
            })
            .collect();
        FrozenRows {
            index,
            package_indexes,
            metadata,
            sites: self
                .rows
                .iter()
                .map(|(path, row)| (path.clone(), row.sites.clone()))
                .collect(),
        }
    }
}
use hirsel_intern::work;
