//! Transport immutable package query results; mutable adapter state stays engine-local.
use super::*;
/// Evidence is engine-local and ends at the next indexed or explicit input edit.
pub(super) struct PackageProof {
    revision: u64,
    environment: QueryEnvironment,
    limits: hirsel_interp::Limits,
    manifest: Option<ContentHash>,
}
impl PackageProof {
    fn new(engine: &QueryEngine) -> Self {
        Self {
            revision: engine.source_revision,
            environment: engine.environment.clone(),
            limits: engine.comptime_limits,
            manifest: Some(engine.declarations.inputs().manifest_revision),
        }
    }
    pub(super) fn matches(&self, engine: &QueryEngine) -> bool {
        self.revision == engine.source_revision
            && self.environment == engine.environment
            && self.limits == engine.comptime_limits
            && self.manifest == Some(engine.declarations.inputs().manifest_revision)
    }
}

pub(super) struct PackagePublication {
    proof: PackageProof,
    closure: BTreeSet<hirsel_syntax::PackageId>,
    /// The closure's sources: each FileId with its (package, manifest ordinal).
    files: Arc<[(FileId, hirsel_syntax::PackageId, usize)]>,
    lowerings: crate::package_cache::SharedLowerings,
}

/// Graphs retain exact TypeId provenance and syntax arena identity. There is no
/// Rc, RefCell, interpreter, or mutable adapter state in a published artifact.
pub(crate) struct PackageSnapshot {
    sources: Vec<PackageSource>,
    generated: BTreeMap<DefPath, GeneratedSource>,
    signatures: BTreeMap<DefPath, Arc<Cached<hirsel_types::SignatureRef>>>,
    resolutions: BTreeMap<DefPath, Arc<Cached<Resolution>>>,
    checked: BTreeMap<DefPath, Arc<Cached<TypedBody>>>,
    modes: BTreeMap<DefPath, Arc<Cached<AnalyzedBody>>>,
    lowered: BTreeMap<DefPath, Arc<Cached<hirsel_tir::DefinitionRef>>>,
    resolver_outputs: BTreeMap<DefPath, ResolverSnapshot>,
    signature_resolutions: BTreeMap<DefPath, ResolverSnapshot>,
    representations: BTreeMap<DefPath, crate::representation::OwnerRecord>,
    constants: BTreeMap<DefPath, crate::comptime::EvaluatedConstant>,
    mode_diagnostics: Vec<(Diagnostic, String)>,
}
/// Generated text is placed relative to its declaration's manifest source,
/// never its old host root.
struct PackageSource {
    /// The publisher's FileId, which the snapshot's `generated` entries name.
    file: FileId,
    /// An anchor in the declaration's source, which the adopter has placed.
    origin: hirsel_span::Anchor,
    suffix: String,
    text: String,
    /// The anchors standing in this source, with their bases.
    anchors: Vec<(hirsel_span::Anchor, u32)>,
}
impl PackageSnapshot {
    pub(crate) fn contains_lowering(&self, path: &DefPath) -> bool {
        self.lowered.contains_key(path)
    }
}
impl QueryEngine {
    pub(crate) fn package_cache_revision(&self) -> u64 {
        self.source_revision
    }
    pub(crate) fn package_snapshot(
        &self,
        package: &hirsel_syntax::PackageId,
        closure: &BTreeSet<hirsel_syntax::PackageId>,
    ) -> Option<PackageSnapshot> {
        self.selected_package_snapshot(package, closure, None)
    }
    fn selected_package_snapshot(
        &self,
        package: &hirsel_syntax::PackageId,
        closure: &BTreeSet<hirsel_syntax::PackageId>,
        definition: Option<&DefPath>,
    ) -> Option<PackageSnapshot> {
        let owned = |p: &DefPath| &p.0.package == package && definition.is_none_or(|d| d == p);
        macro_rules! retain {
            ($rows:expr) => {
                if let Some(path) = definition {
                    $rows
                        .get(path)
                        .map(|c| (path.clone(), c.clone()))
                        .into_iter()
                        .collect()
                } else {
                    $rows
                        .iter()
                        .filter(|(p, _)| owned(p))
                        .map(|(p, c)| (p.clone(), c.clone()))
                        .collect()
                }
            };
        }
        let inside = |p: &DefPath| {
            closure.contains(&p.0.package)
                || p.0.package == hirsel_syntax::DefPath::core("Result").0.package
        };
        let safe = |reads: &DependencyReads, executed: &ExecutionReads| {
            reads.operational_failure.is_none()
                && reads.signatures.iter().all(|s| inside(&s.definition))
                && reads.failed_signatures.iter().all(&inside)
                && executed
                    .bodies
                    .keys()
                    .chain(executed.signatures.keys())
                    .all(&inside)
        };
        // #4 permits versioned reads outside the declared dependency closure. Such
        // packages use ordinary checking rather than an incomplete artifact key.
        use std::ops::Bound::{Included, Unbounded};
        let bounds = definition.map_or((Unbounded, Unbounded), |p| {
            (
                Included((p.clone(), QueryKind::Lex)),
                Included((p.clone(), QueryKind::Lower)),
            )
        });
        for ((path, _), reads) in self.reads.range(bounds) {
            if owned(path) && !safe(reads, &ExecutionReads::default()) {
                return None;
            }
        }
        let lowered: BTreeMap<_, _> = retain!(self.lowered);
        for c in lowered.values() {
            if !safe(&c.reads, &c.executed) {
                return None;
            }
        }
        let state = self.adapters.compiler_state.as_ref()?.borrow();
        let mut constants: BTreeMap<_, _> = retain!(state.constant_values);
        constants.retain(|_, v| safe(&DependencyReads::default(), &v.executed));
        let owner_span = definition.and_then(|p| {
            self.items
                .get(p)?
                .first()
                .map(|s| self.sources.resolve(s.item.origin.span))
        });
        let mut snapshot = PackageSnapshot {
            sources: vec![],
            generated: retain!(self.generated),
            signatures: retain!(self.signatures),
            resolutions: retain!(self.resolutions),
            checked: retain!(self.checked),
            modes: retain!(self.modes),
            lowered,
            resolver_outputs: retain!(state.resolver_outputs),
            signature_resolutions: retain!(state.signature_resolutions),
            representations: retain!(state.representations),
            constants,
            mode_diagnostics: state
                .mode_diagnostics
                .iter()
                .filter(|(d, _)| {
                    let primary = self.sources.resolve(d.primary);
                    if let Some(span) = owner_span {
                        return span.file == primary.file
                            && span.start <= primary.start
                            && primary.end <= span.end;
                    }
                    self.parsed
                        .get(&primary.file)
                        .is_some_and(|(c, _, _)| &c.package == package)
                })
                .cloned()
                .collect(),
        };
        // A generated body's syntax mixes the written header's tokens with
        // tokens in its generated source. Both sources must travel with it.
        for g in snapshot.generated.values() {
            let source = self.sources.file(g.file)?;
            let (origin, _) = self.sources.anchor(g.site.anchor)?;
            let written = self.sources.file(origin)?.path.display().to_string();
            let suffix = source
                .path
                .display()
                .to_string()
                .strip_prefix(&written)
                .filter(|suffix| suffix.starts_with('#'))?
                .to_owned();
            snapshot.sources.push(PackageSource {
                file: g.file,
                origin: g.site.anchor,
                suffix,
                text: source.text().into(),
                anchors: self
                    .sources
                    .anchors()
                    .filter(|(_, file, _)| *file == g.file)
                    .map(|(anchor, _, base)| (anchor, base))
                    .collect(),
            });
        }
        Some(snapshot)
    }
    pub(crate) fn register_package_publication(
        &mut self,
        package: hirsel_syntax::PackageId,
        closure: BTreeSet<hirsel_syntax::PackageId>,
        files: Vec<(FileId, hirsel_syntax::PackageId, usize)>,
        lowerings: crate::package_cache::SharedLowerings,
    ) {
        self.package_publications.insert(
            package,
            PackagePublication {
                proof: PackageProof::new(self),
                closure,
                files: files.into(),
                lowerings,
            },
        );
    }
    /// Where this engine's anchors in `files` stand, by source.
    pub(crate) fn anchor_sites(
        &self,
        files: &[(FileId, hirsel_syntax::PackageId, usize)],
    ) -> Arc<[crate::package_cache::AnchorSite]> {
        let sources: BTreeMap<FileId, (&hirsel_syntax::PackageId, usize)> = files
            .iter()
            .map(|(file, package, index)| (*file, (package, *index)))
            .collect();
        self.sources
            .anchors()
            .filter_map(|(anchor, file, base)| {
                let (package, index) = sources.get(&file)?;
                Some((anchor, (*package).clone(), *index, base))
            })
            .collect()
    }
    /// Place another engine's anchors at this engine's FileId for each source.
    pub(crate) fn place_anchors(
        &mut self,
        sites: &[crate::package_cache::AnchorSite],
        files: &BTreeMap<(hirsel_syntax::PackageId, usize), FileId>,
    ) {
        for (anchor, package, index, base) in sites {
            if let Some(file) = files.get(&(package.clone(), *index)) {
                self.sources.move_anchor(*anchor, *file, *base);
            }
        }
    }
    pub(super) fn publish_lowering(&self, definition: &DefPath) {
        let Some(publication) = self.package_publications.get(&definition.0.package) else {
            return;
        };
        // Index, manifest, environment and limit edits end publication authority,
        // just as they end the engine-local package validity proof.
        if !publication.proof.matches(self) {
            return;
        }
        let Some(node) = self.lowered.get(definition) else {
            return;
        };
        if !node
            .result
            .as_ref()
            .is_ok_and(|o| !has_errors(&o.diagnostics))
        {
            return;
        }
        if publication
            .lowerings
            .lock()
            .unwrap()
            .contains_key(definition)
        {
            return;
        }
        let Some(snapshot) = self.selected_package_snapshot(
            &definition.0.package,
            &publication.closure,
            Some(definition),
        ) else {
            return;
        };
        // Build outside the lock. Readers retain immutable Arc nodes, and
        // concurrent clients add distinct definitions without replacing peers.
        publication
            .lowerings
            .lock()
            .unwrap()
            .entry(definition.clone())
            .or_insert_with(|| {
                Arc::new(crate::package_cache::PublishedLowering {
                    snapshot,
                    sites: self.anchor_sites(&publication.files),
                })
            });
    }
    /// Merge a published snapshot, whose anchors the caller has placed.
    pub(super) fn adopt_package_snapshot(&mut self, snapshot: &PackageSnapshot) {
        // Old cache IDs may collide with client IDs: transport renumbers the
        // nodes before merging into the fresh engine. It runs no phase.
        let adapters = PhaseAdapters {
            compiler_state: Some(Default::default()),
            ..Default::default()
        };
        let mut transport = QueryEngine::new(self.environment.clone(), adapters);
        transport.package_manifests = self.package_manifests.clone();
        transport.comptime_limits = self.comptime_limits;
        transport.cache_sequence = self.cache_sequence;
        let mut placed = BTreeMap::new();
        for source in &snapshot.sources {
            let (origin, _) = self
                .sources
                .anchor(source.origin)
                .expect("generated source's manifest placement");
            let origin = self.sources.file(origin).unwrap();
            let path = format!("{}{}", origin.path.display(), source.suffix);
            let logical = format!("{}{}", origin.logical_path().display(), source.suffix);
            let file = self
                .sources
                .add_logical(path, logical, source.text.clone())
                .expect("cached generated sources fit the source map");
            for &(anchor, base) in &source.anchors {
                self.sources.move_anchor(anchor, file, base);
            }
            placed.insert(source.file, file);
        }
        transport.generated = snapshot.generated.clone();
        for g in transport.generated.values_mut() {
            if let Some(file) = placed.get(&g.file) {
                g.file = *file;
            }
        }
        macro_rules! adopt {
            ($field:ident) => {
                for (path, node) in &snapshot.$field {
                    let mut cached = (**node).clone();
                    transport.cache_sequence += 1;
                    cached.id = transport.cache_sequence;
                    transport.$field.insert(path.clone(), Arc::new(cached));
                }
            };
        }
        adopt!(signatures);
        adopt!(resolutions);
        adopt!(checked);
        adopt!(modes);
        adopt!(lowered);
        {
            let mut state = transport
                .adapters
                .compiler_state
                .as_ref()
                .unwrap()
                .borrow_mut();
            state.resolver_outputs = snapshot.resolver_outputs.clone().into_iter().collect();
            state.signature_resolutions =
                snapshot.signature_resolutions.clone().into_iter().collect();
            state.representations = snapshot.representations.clone().into_iter().collect();
            state.constant_values = snapshot.constants.clone().into_iter().collect();
            state.mode_diagnostics = snapshot.mode_diagnostics.clone();
        }
        self.cache_sequence = transport.cache_sequence;
        self.generated.append(&mut transport.generated);
        macro_rules! merge {
            ($field:ident) => {
                for c in transport.$field.values() {
                    self.reads
                        .insert((c.key.definition.clone(), c.key.kind), c.reads.clone());
                }
                self.$field
                    .extend(transport.$field.iter().map(|(p, c)| (p.clone(), c.clone())));
            };
        }
        merge!(signatures);
        merge!(resolutions);
        merge!(checked);
        merge!(modes);
        merge!(lowered);
        let mut state = self.adapters.compiler_state.as_ref().unwrap().borrow_mut();
        let transport_state = transport.adapters.compiler_state.take().unwrap();
        let mut source = transport_state.borrow_mut();
        macro_rules! append {
            ($field:ident) => {
                state
                    .$field
                    .extend(source.$field.iter().map(|(p, v)| (p.clone(), v.clone())));
            };
        }
        append!(resolver_outputs);
        append!(signature_resolutions);
        append!(representations);
        append!(constant_values);
        for diagnostic in source.mode_diagnostics.drain(..) {
            if !state.mode_diagnostics.contains(&diagnostic) {
                state.mode_diagnostics.push(diagnostic);
            }
        }
        state.materialization_environment = Some(self.environment.clone());
        state.materialization_limits = Some(self.comptime_limits);
    }
    /// Install the package proof a package image established for this package.
    pub(crate) fn prove_package(&mut self, package: &hirsel_syntax::PackageId) {
        self.package_proofs
            .insert(package.clone(), PackageProof::new(self));
    }
    pub(crate) fn package_snapshot_valid(&mut self, package: &hirsel_syntax::PackageId) -> bool {
        let valid = self.with_validation_walk(|engine| {
            let paths: Vec<_> = engine
                .signatures
                .keys()
                .filter(|p| &p.0.package == package)
                .cloned()
                .collect();
            for p in paths {
                let before = engine.recompute_count(&p, QueryKind::Signature);
                let _ = engine.signature(&p);
                if before != engine.recompute_count(&p, QueryKind::Signature) {
                    return false;
                }
            }
            macro_rules! validate {
                ($field:ident) => {
                    for c in engine
                        .$field
                        .values()
                        .filter(|c| &c.key.definition.0.package == package)
                        .cloned()
                        .collect::<Vec<_>>()
                    {
                        let Ok(key) = engine.key(&c.key.definition, c.key.kind) else {
                            return false;
                        };
                        if !engine.valid(&c, &key) {
                            return false;
                        }
                    }
                };
            }
            validate!(resolutions);
            validate!(checked);
            validate!(modes);
            validate!(lowered);
            true
        });
        if valid {
            self.package_proofs
                .insert(package.clone(), PackageProof::new(self));
        }
        valid
    }
}
