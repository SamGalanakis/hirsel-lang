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
mod retained;

/// The explicit build inputs of module resolution, beside the indexed items.
#[derive(Clone, Debug, Default)]
pub struct ModuleInputs {
    /// Module heads every package sees.
    pub exposures: Vec<DependencyExposure>,
    /// Ordinary dependency heads scoped to their importing package.
    pub package_exposures: BTreeMap<PackageId, Vec<DependencyExposure>>,
    /// Complete selected direct dependencies, including unexposed packages.
    /// V uses the transitive closure, independently of source lookup aliases.
    pub package_dependencies: BTreeMap<PackageId, Vec<PackageId>>,
    pub signature_metadata: Metadata,
    pub manifest_revision: ContentHash,
}

/// One immutable declaration row. Snapshots share it; owned declaration sites
/// are constructed only when a consumer asks.
#[derive(Debug)]
struct Row {
    data: DeclarationRow,
    sites: OnceLock<Arc<[DeclarationSite]>>,
}
impl std::ops::Deref for Row {
    type Target = DeclarationRow;
    fn deref(&self) -> &DeclarationRow {
        &self.data
    }
}
impl Row {
    fn entry(&self) -> DefinitionEntry {
        DefinitionEntry {
            definition: self.definition.path().clone(),
            namespace: if self.types {
                Namespace::Type
            } else {
                Namespace::Value
            },
            visibility: self.visibility,
            span: self.span,
        }
    }
    fn sites(&self) -> &Arc<[DeclarationSite]> {
        hirsel_intern::timings::count_by(hirsel_intern::timings::Count::DeclarationSiteReads, 1);
        self.sites.get_or_init(|| {
            hirsel_intern::timings::count_by(
                hirsel_intern::timings::Count::DeclarationSiteMaterializations,
                self.site_spans.len() as u64,
            );
            self.site_spans
                .iter()
                .map(|&name_span| DeclarationSite {
                    definition: self.definition.path().clone(),
                    name_span,
                })
                .collect()
        })
    }
}
impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        self.definition == other.definition
            && self.types == other.types
            && self.visibility == other.visibility
            && self.span == other.span
            && (self.metadata.public, self.metadata.alias)
                == (other.metadata.public, other.metadata.alias)
            && self.site_spans == other.site_spans
            && self.headers == other.headers
            && self.candidates == other.candidates
    }
}

/// Syntax-free declaration inputs. A file's parse product can retain these
/// heads without retaining token tapes, expression arenas or type tables.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DeclarationHead {
    pub(crate) definition: DefId,
    pub(crate) kind: u8,
    pub(crate) visibility: Visibility,
    pub(crate) name: Span,
    pub(crate) header: ContentHash,
    pub(crate) body: ContentHash,
}
impl DeclarationHead {
    pub(crate) fn capture(sources: &SourceMap, syntax: &ItemSyntax) -> Self {
        let item = &syntax.item;
        Self {
            definition: DefId::intern(&item.definition),
            kind: match &item.kind {
                ItemKind::Function { .. } => 0,
                ItemKind::Const { .. } => 1,
                ItemKind::Type {
                    body: TypeBody::Alias(_),
                    ..
                } => 2,
                ItemKind::Type { .. } => 3,
                ItemKind::Signature { .. } => 4,
                ItemKind::Implementation { .. } => 5,
            },
            visibility: item.visibility,
            name: item.name_span,
            header: header_hash(sources, syntax),
            body: syntax.text_hash,
        }
    }
}

/// Compact ordinary declaration inputs. A site's definition is supplied by
/// the row; its ordered candidate spans are materialized only on demand.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DeclarationRow {
    definition: DefId,
    types: bool,
    visibility: Visibility,
    span: Span,
    metadata: DefinitionMetadata,
    site_spans: Vec<Span>,
    headers: Vec<ContentHash>,
    candidates: ContentHash,
    identity: [u64; 4],
}
impl DeclarationRow {
    pub(crate) fn definition(&self) -> &DefPath {
        self.definition.path()
    }
    /// Each candidate's name span, in occurrence order.
    pub(crate) fn site_spans(&self) -> &[Span] {
        &self.site_spans
    }
}
impl From<DeclarationRow> for Row {
    fn from(image: DeclarationRow) -> Self {
        Self {
            data: image,
            sites: OnceLock::new(),
        }
    }
}

/// The module input resolution and signature normalization read, frozen.
pub(crate) trait DeclarationSiblings: Send + Sync {
    fn siblings(&self, syntax: &ItemSyntax) -> Result<Arc<[Arc<ItemSyntax>]>, crate::QueryError>;
}
pub struct Declarations {
    archived_siblings: Option<Arc<dyn DeclarationSiblings>>,
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
    sites: BTreeMap<DefId, Arc<Row>>,
}
impl Declarations {
    pub(crate) fn name_set(
        &self,
        package: crate::memo::PackageKey,
        module: crate::memo::ModuleKey,
    ) -> ContentHash {
        let start = DefPath(hirsel_syntax::ItemPath {
            package: package.id(),
            modules: if module.0.as_str().is_empty() {
                vec![]
            } else {
                module.0.as_str().split('.').map(str::to_owned).collect()
            },
            name: String::new(),
        });
        let first = DefId::intern(&start);
        // DefId orders by DefPath, so this visits only the selected module.
        let mut parts = Vec::new();
        for (id, row) in self.rows.sites.range(first..) {
            let path = id.path();
            if path.0.package != start.0.package || path.0.modules != start.0.modules {
                break;
            }
            if hirsel_types::specialization_parts(path).is_some() {
                continue;
            }
            parts.push(path.0.name.as_bytes());
            parts.push(row.candidates.0.as_slice());
        }
        ContentHash::of_parts("hirsel.read.module-names.v2", parts)
    }
    pub(crate) fn with_archived_siblings(&self, source: Arc<dyn DeclarationSiblings>) -> Arc<Self> {
        Arc::new(Self {
            rows: self.rows.clone(),
            files: BTreeMap::new(),
            anchors: HashMap::new(),
            archived_siblings: Some(source),
        })
    }
    pub(crate) fn try_siblings(
        &self,
        syntax: &ItemSyntax,
    ) -> Result<Arc<[Arc<ItemSyntax>]>, crate::QueryError> {
        match &self.archived_siblings {
            Some(source) => source.siblings(syntax),
            None => Ok(self.siblings(syntax)),
        }
    }
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
            .get(&DefId::intern(path))
            .map(|row| row.sites().clone())
            .unwrap_or_else(|| Arc::from([]))
    }
}

/// The index of every indexed item.
#[derive(Clone, Default)]
pub struct DeclarationIndex {
    /// Changes to signature inputs, independent of source body revisions.
    revision: u64,
    rows: BTreeMap<DefId, Arc<Row>>,
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
    /// Place a demanded source file's materialized items under its indexed rows.
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
        self.rows
            .get(&DefId::intern(path))
            .map_or(&[], |row| row.sites().as_ref())
    }
    /// The paths with rows.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &DefPath> {
        self.rows.keys().map(|id| id.path())
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
        let heads: Vec<_> = candidates
            .into_iter()
            .flatten()
            .map(|syntax| {
                (
                    DeclarationHead::capture(sources, syntax),
                    logical(sources, syntax),
                )
            })
            .collect();
        self.set_heads(path, &heads);
    }
    /// Rebuild one row from ordinary parse heads in candidate order. No syntax
    /// materialization or semantic/type decoding is required by the namespace.
    pub(crate) fn set_heads(&mut self, path: &DefPath, heads: &[(DeclarationHead, Vec<u8>)]) {
        let row = heads.first().map(|(first, _)| {
            work::count(work::Counter::ModuleDeclarationRows);
            let last = &heads.last().unwrap().0;
            let metadata = DefinitionMetadata {
                public: last.visibility != Visibility::Private,
                alias: last.kind == 2,
            };
            let mut parts = vec![(heads.len() as u64).to_le_bytes().to_vec()];
            for (head, logical) in heads {
                parts.push(logical.clone());
                parts.push(head.header.0.to_vec());
            }
            let public = first.visibility != Visibility::Private;
            Arc::new(Row::from(DeclarationRow {
                definition: first.definition,
                types: first.kind >= 2,
                visibility: first.visibility,
                span: first.name,
                metadata,
                site_spans: heads.iter().map(|(head, _)| head.name).collect(),
                headers: heads.iter().map(|(head, _)| head.header).collect(),
                candidates: ContentHash::of_parts(
                    "hirsel.declaration-candidates.v1",
                    parts.iter().map(Vec::as_slice),
                ),
                identity: lanes(ContentHash::of_parts(
                    "hirsel.manifest.identity.v2",
                    [
                        path.fingerprint().0.as_slice(),
                        &[u8::from(public), first.kind],
                    ],
                )),
            }))
        });
        self.replace_row(path, row);
    }
    fn replace_row(&mut self, path: &DefPath, row: Option<Arc<Row>>) {
        let old = match &row {
            Some(row) => self.rows.insert(DefId::intern(path), row.clone()),
            None => self.rows.remove(&DefId::intern(path)),
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
        self.rows
            .get(&DefId::intern(path))
            .map(|row| row.candidates)
    }
    /// The header hashes of `path`'s candidates, in occurrence order.
    pub fn headers(&self, path: &DefPath) -> Option<&[ContentHash]> {
        self.rows.get(&DefId::intern(path)).map(|row| &*row.headers)
    }
    /// Header rows and source placement retained by an observation-only copy.
    /// No item arenas or frozen full-syntax sibling lists participate.
    pub(crate) fn observation_bytes(&self) -> Option<u64> {
        let mut bytes = std::mem::size_of::<Self>() as u64;
        for row in self.rows.values() {
            bytes = bytes
                .checked_add(
                    (std::mem::size_of::<Row>() + 8 * std::mem::size_of::<usize>()) as u64,
                )?
                .checked_add((row.site_spans.capacity() * std::mem::size_of::<Span>()) as u64)?
                .checked_add(
                    (row.headers.capacity() * std::mem::size_of::<ContentHash>()) as u64,
                )?;
            if let Some(sites) = row.sites.get() {
                bytes = bytes
                    .checked_add((sites.len() * std::mem::size_of::<DeclarationSite>()) as u64)?;
            }
        }
        bytes = bytes.checked_add(
            (self.anchors.capacity() * (std::mem::size_of::<(Anchor, FileId)>() + 1) + 16) as u64,
        )?;
        for (path, files) in &self.paths {
            bytes = bytes.checked_add(path.capacity() as u64)?.checked_add(
                (std::mem::size_of::<std::path::PathBuf>()
                    + files.len()
                        * (std::mem::size_of::<FileId>() + 6 * std::mem::size_of::<usize>()))
                    as u64,
            )?;
        }
        Some(bytes)
    }
    pub(crate) fn observations(&self) -> Self {
        Self {
            revision: self.revision,
            rows: self.rows.clone(),
            files: BTreeMap::new(),
            anchors: self.anchors.clone(),
            paths: self.paths.clone(),
            identity: self.identity,
            inputs: self.inputs.clone(),
            frozen: OnceLock::new(),
            frozen_rows: OnceLock::new(),
        }
    }
    /// The frozen module input, built after a row or an input changed.
    pub fn frozen(&self) -> Arc<Declarations> {
        self.frozen
            .get_or_init(|| {
                Arc::new(Declarations {
                    archived_siblings: None,
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
                .insert(path.path().clone(), row.metadata.clone());
            entries.push(row.entry());
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
            sites: self.rows.clone(),
        }
    }
}
use hirsel_intern::work;

#[cfg(test)]
#[path = "declarations_tests.rs"]
mod tests;
