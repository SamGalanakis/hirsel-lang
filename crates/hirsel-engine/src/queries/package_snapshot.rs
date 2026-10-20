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
            manifest: engine
                .adapters
                .compiler_state
                .as_ref()
                .map(|s| s.borrow().manifest_revision),
        }
    }
    pub(super) fn matches(&self, engine: &QueryEngine) -> bool {
        self.revision == engine.source_revision
            && self.environment == engine.environment
            && self.limits == engine.comptime_limits
            && self.manifest
                == engine
                    .adapters
                    .compiler_state
                    .as_ref()
                    .map(|s| s.borrow().manifest_revision)
    }
}

pub(super) struct PackagePublication {
    proof: PackageProof,
    closure: BTreeSet<hirsel_syntax::PackageId>,
    sites: Arc<[(FileId, hirsel_syntax::PackageId, usize, u32)]>,
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
/// Generated text is placed relative to a manifest source, never its old host root.
struct PackageSource {
    file: FileId,
    origin: FileId,
    suffix: String,
    text: String,
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
                .map(|s| s.file.tree.items[s.item.0 as usize].origin.span)
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
                    if let Some(span) = owner_span {
                        return span.file == d.primary.file
                            && span.start <= d.primary.start
                            && d.primary.end <= span.end;
                    }
                    self.parsed
                        .get(&d.primary.file)
                        .is_some_and(|(c, _, _)| &c.package == package)
                })
                .cloned()
                .collect(),
        };
        // A generated body's syntax mixes the written header's tokens with
        // tokens in its generated source. Both sources must travel with it.
        let mut files = BTreeSet::new();
        let mut owners = BTreeSet::new();
        let mut syntax = |file: &Arc<SyntaxFile>| {
            if !owners.insert(Arc::as_ptr(file) as usize) {
                return;
            }
            files.insert(file.file);
            files.extend(file.tokens.iter().map(|t| t.span.file));
        };
        for c in snapshot.checked.values() {
            if let Ok(output) = &c.result {
                syntax(&output.value.file);
            }
        }
        for c in snapshot.modes.values() {
            if let Ok(output) = &c.result {
                syntax(&output.value.typed.file);
            }
        }
        for s in snapshot
            .resolver_outputs
            .values()
            .chain(snapshot.signature_resolutions.values())
        {
            syntax(&s.file);
        }
        for g in snapshot.generated.values() {
            files.extend([g.file, g.site.file]);
        }
        for file in files {
            if self.parsed.contains_key(&file) {
                continue;
            }
            let source = self.sources.file(file)?;
            let path = source.path.display().to_string();
            let (origin, suffix) = self.parsed.keys().find_map(|origin| {
                let written = self.sources.file(*origin)?.path.display().to_string();
                let suffix = path.strip_prefix(&written)?;
                suffix
                    .starts_with('#')
                    .then(|| (*origin, suffix.to_owned()))
            })?;
            snapshot.sources.push(PackageSource {
                file,
                origin,
                suffix,
                text: source.text().into(),
            });
        }
        Some(snapshot)
    }
    pub(crate) fn register_package_publication(
        &mut self,
        package: hirsel_syntax::PackageId,
        closure: BTreeSet<hirsel_syntax::PackageId>,
        sites: Vec<(FileId, hirsel_syntax::PackageId, usize, u32)>,
        lowerings: crate::package_cache::SharedLowerings,
    ) {
        self.package_publications.insert(
            package,
            PackagePublication {
                proof: PackageProof::new(self),
                closure,
                sites: sites.into(),
                lowerings,
            },
        );
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
                    sites: publication.sites.clone(),
                })
            });
    }
    pub(super) fn adopt_package_snapshot(
        &mut self,
        snapshot: &PackageSnapshot,
        moves: &[(FileId, FileId, u32)],
    ) {
        // Old IDs may collide with client IDs. Rebase only the transported nodes
        // before merging into the fresh engine; Arc::make_mut detaches changed sites.
        // Transport only rebases cached artifacts and compiler state; it runs no phase.
        let adapters = PhaseAdapters {
            compiler_state: Some(Default::default()),
            ..Default::default()
        };
        let mut transport = QueryEngine::new(self.environment.clone(), adapters);
        transport.package_manifests = self.package_manifests.clone();
        transport.comptime_limits = self.comptime_limits;
        transport.cache_sequence = self.cache_sequence;
        let mut moves = moves.to_vec();
        for source in &snapshot.sources {
            let origin = moves
                .iter()
                .find(|(old, _, _)| *old == source.origin)
                .expect("generated source's manifest placement")
                .1;
            let path = format!(
                "{}{}",
                self.sources.file(origin).unwrap().path.display(),
                source.suffix
            );
            let file = self
                .sources
                .add(path, source.text.clone())
                .expect("cached generated sources fit the source map");
            moves.push((source.file, file, source.text.len() as u32));
        }
        transport.generated = snapshot.generated.clone();
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
        transport.rebase_package_snapshot(&moves);
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
    pub(crate) fn rebase_package_snapshot(&mut self, moves: &[(FileId, FileId, u32)]) {
        self.rebase_positions(
            &moves
                .iter()
                .filter(|(old, new, _)| old != new)
                .map(|&(old, new, len)| {
                    let before = Span::new(old, 0, len).unwrap();
                    positions::Movement {
                        old: before,
                        new_file: new,
                        delta: 0,
                        whole: (before, Span::new(new, 0, len).unwrap()),
                    }
                })
                .collect::<Vec<_>>(),
        );
    }
}
