//! PROG-1: explicit source packages. No directory discovery or std fallback.
mod dependency_card;
pub use dependency_card::render_dependency_cards;

use crate::*;
use hirsel_resolve::DependencyExposure;
use hirsel_syntax::PackageId;
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// A manifest-selected declaration graph, including libraries without an entry.
/// Loading parses and indexes sources; it never checks or evaluates a body.
pub struct PackageDeclarations {
    pub package: PackageId,
    pub engine: QueryEngine,
    pub files: Vec<(FileId, FileContext)>,
}
impl PackageDeclarations {
    /// Whether a declaration is visible from outside every selected package.
    pub fn is_exported(&self, definition: &DefPath) -> bool {
        let viewer = FileContext {
            // This reserved spelling cannot name a source package.
            package: PackageId {
                name: "$doc".into(),
                version: "0".into(),
            },
            modules: vec![],
        };
        self.engine
            .declarations
            .frozen()
            .index(self.package.clone())
            .access(&viewer, definition)
            .is_ok()
    }
    /// Resolve a header reference using the importing package's selected aliases
    /// and the compiler's ordinary PROG-2 access rules.
    pub fn lookup(
        &self,
        context: &FileContext,
        path: &hirsel_syntax::Path,
    ) -> Result<DefPath, hirsel_types::LookupError> {
        let index = self
            .engine
            .declarations
            .frozen()
            .index(context.package.clone());
        match index.lookup(context, path) {
            Err(hirsel_types::LookupError::Missing)
                if path.package.is_none() && path.segments.len() == 1 =>
            {
                let definition = registry::BuiltinRegistry::new()
                    .lookup_name(path)
                    .ok_or(hirsel_types::LookupError::Missing)?;
                index.access(context, &definition)
            }
            result => result,
        }
    }
}

pub(crate) fn load_package_declarations(
    compiler: &Compiler,
    manifest: &Path,
) -> Result<PackageDeclarations, ToolError> {
    let mut graph = Graph::default();
    let package = graph.visit(manifest, None)?;
    let mut engine = QueryEngine::new(default_environment(), compiler.adapters());
    engine.declarations.inputs_mut().package_exposures = graph
        .packages
        .iter()
        .map(|(id, p)| (id.clone(), p.exposures.clone()))
        .collect();
    let mut files = Vec::new();
    let mut assigned = BTreeSet::new();
    for selected in graph.packages.values() {
        for (source, context) in &selected.sources {
            if !assigned.insert(source.clone()) {
                return Err(invalid("one source path has more than one FileContext"));
            }
            let file = engine
                .sources
                .add_logical(
                    source,
                    &selected.logical_sources[source],
                    crate::tools::read_source(source)?,
                )
                .ok_or(ToolError::SourceTooLarge)?;
            engine
                .reparse_file(file, context)
                .map_err(|error| ToolError::Query {
                    error,
                    phase: None,
                    sources: engine.sources.clone(),
                })?;
            files.push((file, context.clone()));
        }
    }
    Ok(PackageDeclarations {
        package,
        engine,
        files,
    })
}

/// A selected program, with every source indexed before any header is queried.
pub struct LoadedProgram {
    pub engine: QueryEngine,
    pub files: Vec<(FileId, FileContext)>,
    pub entry: DefPath,
    pub primary: FileId,
    /// The selected packages marked `unsafe ffi`, in package order (PROG-4).
    pub unsafe_ffi: Vec<UnsafeFfiPackage>,
    /// Why a dependency package is checked from source rather than prechecked,
    /// for `--verbose`; a precheck changes only how long checking takes.
    pub precheck_notes: Vec<String>,
    packages: Vec<crate::package_cache::PackageRevision>,
    cache: PackageCache,
    identity: ContentHash,
    indexed_revision: u64,
}
impl LoadedProgram {
    /// Check every package of the program, lowering what `lowering` selects:
    /// the diagnostics do not depend on it. The root package lowers under
    /// `lowering`, each dependency only what compile-time evaluation executes,
    /// and `Reachable` then lowers the dependency bodies the root reaches.
    pub fn check(&mut self, lowering: LoweringPolicy) -> Result<CheckOutput, QueryError> {
        self.check_with(lowering, |_, _| Ok(()))
    }
    /// `check`, calling `checked` with each file that checked from source
    /// complete and with no diagnostics, as soon as it has.
    fn check_with(
        &mut self,
        lowering: LoweringPolicy,
        mut checked: impl FnMut(&mut QueryEngine, FileId) -> Result<(), QueryError>,
    ) -> Result<CheckOutput, QueryError> {
        // The program check's own steps; per-definition checking and
        // lowering, GEN-5 and publication below are their own intervals.
        let _interval = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::S0);
        let mut diagnostics = Vec::new();
        let mut completion = CheckCompletion::Complete;
        let unchanged = self.indexed_revision == self.engine.package_cache_revision();
        // Closure identity covers content; ordinary query receipts additionally
        // revalidate selected definitions and negative lookups outside that closure.
        for package in &mut self.packages {
            package.reused = unchanged
                && package.artifact.as_ref().is_some_and(|a| {
                    a.environment.interface_version == self.engine.environment.interface_version
                        && a.environment.compiler_version
                            == self.engine.environment.compiler_version
                        && a.environment.spec_hash == self.engine.environment.spec_hash
                        && a.environment.builtin_signatures_hash
                            == self.engine.environment.builtin_signatures_hash
                        && a.limits == self.engine.comptime_limits
                })
                && if package.proven {
                    self.engine.prove_package(&package.package);
                    true
                } else {
                    self.engine.package_snapshot_valid(&package.package)
                };
        }
        let mut preflight = BTreeMap::new();
        for (file, context) in &self.files {
            // A prechecked package checked complete and clean (`Precheck`).
            if self.engine.prechecked(&context.package) {
                continue;
            }
            if let Some(package) = self
                .packages
                .iter()
                .find(|p| p.reused && p.package == context.package)
            {
                let index = package.files.iter().position(|(f, _)| f == file).unwrap();
                let errors = package.diagnostics.as_ref().unwrap().preflight[index].clone();
                preflight.insert(*file, errors.clone());
                diagnostics.extend(errors);
                continue;
            }
            match crate::tools::association_diagnostics(&mut self.engine, *file, context) {
                Ok(errors) | Err(QueryError::Diagnostics(errors)) => {
                    preflight.insert(*file, errors.clone());
                    diagnostics.extend(errors);
                }
                Err(error) => return Err(error),
            }
        }
        if crate::queries::has_errors(&diagnostics) {
            return Ok(CheckOutput {
                diagnostics: select_diagnostics(diagnostics, Some(&self.engine.sources)),
                completion,
                sources: self.engine.sources.clone(),
                unsafe_ffi: self.unsafe_ffi.clone(),
            });
        }
        let mut outputs = BTreeMap::new();
        let root = self.entry.0.package.clone();
        let policy = |package: &PackageId| {
            if *package == root {
                lowering
            } else {
                LoweringPolicy::Comptime
            }
        };
        for (file, context) in &self.files {
            if self.engine.prechecked(&context.package) {
                continue;
            }
            if let Some(package) = self.packages.iter().find(|p| {
                p.reused
                    && p.package == context.package
                    && p.artifact
                        .as_ref()
                        .unwrap()
                        .check_complete(policy(&p.package))
            }) {
                let index = package.files.iter().position(|(f, _)| f == file).unwrap();
                let artifact = package.artifact.as_ref().unwrap();
                diagnostics.extend(package.diagnostics.as_ref().unwrap().files[index].clone());
                self.engine.last_phase = artifact.last_phase;
                continue;
            }
            let parallel =
                hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Parallel);
            let output = crate::tools::check_file_lowering(
                &mut self.engine,
                *file,
                context,
                policy(&context.package),
            )?;
            drop(parallel);
            if output.completion != CheckCompletion::Complete {
                completion = output.completion;
            } else if output.diagnostics.is_empty() {
                checked(&mut self.engine, *file)?;
            }
            diagnostics.extend(output.diagnostics.iter().cloned());
            // Publication needs each file's verdict, not its copy of the sources.
            outputs.insert(*file, (output.completion, output.diagnostics));
        }
        // The root package's dead error arms (LANG-8) are warnings of the
        // files declaring them, so a reused package keeps them with its verdict.
        if !self.packages.iter().any(|p| p.reused && p.package == root)
            && !crate::queries::has_errors(&diagnostics)
            && completion == CheckCompletion::Complete
        {
            let files: Vec<_> = self
                .files
                .iter()
                .filter(|(_, c)| c.package == root)
                .map(|(file, _)| *file)
                .collect();
            for warning in self.engine.dead_arm_warnings(&files) {
                if let Some((_, file_diagnostics)) =
                    outputs.get_mut(&self.engine.sources.resolve(warning.primary).file)
                {
                    file_diagnostics.push(warning.clone());
                }
                diagnostics.push(warning);
            }
        }
        // GEN-5 crosses package boundaries. Its program verdict is always
        // selected for this program, including when every dependency body is
        // already checked. Unchanged file checks share that program verdict.
        if self.packages.iter().any(|p| p.reused) {
            match crate::generic_graph::check(&mut self.engine) {
                Ok(errors) | Err(QueryError::Diagnostics(errors)) => diagnostics.extend(errors),
                Err(error) => return Err(error),
            }
        }
        // Dependencies lower only what the program's own package reaches.
        if lowering == LoweringPolicy::Reachable
            && !crate::queries::has_errors(&diagnostics)
            && completion == CheckCompletion::Complete
        {
            let roots = self
                .files
                .iter()
                .filter(|(_, c)| c.package == root)
                .flat_map(|(file, _)| self.engine.file_definitions(*file))
                .collect::<Vec<_>>();
            match self.engine.lower_reachable(roots) {
                Ok(()) => {}
                Err(QueryError::Diagnostics(errors)) => diagnostics.extend(errors),
                Err(QueryError::NotImplemented) => {
                    let phase = self
                        .engine
                        .unavailable
                        .or(self.engine.last_phase)
                        .unwrap_or(QueryKind::Lower);
                    self.engine.unavailable = Some(phase);
                    completion = CheckCompletion::Incomplete { phase };
                }
                Err(error) => return Err(error),
            }
        }
        let _publication =
            hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Merge);
        for package in self.packages.iter().filter(|p| {
            unchanged
                && self.cache.enabled()
                && (!p.reused
                    || !p
                        .artifact
                        .as_ref()
                        .unwrap()
                        .check_complete(policy(&p.package)))
        }) {
            let Some(file_outputs) = package
                .files
                .iter()
                .map(|(f, _)| outputs.get(f))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if file_outputs
                .iter()
                .any(|(completion, _)| *completion != CheckCompletion::Complete)
            {
                continue;
            }
            let local: Vec<Vec<_>> = file_outputs
                .iter()
                .zip(&package.files)
                .map(|((_, diagnostics), (f, _))| {
                    diagnostics
                        .iter()
                        .filter(|d| {
                            self.engine.sources.resolve(d.primary).file == *f
                                && d.rule.as_str() != "GEN-5"
                        })
                        .cloned()
                        .collect()
                })
                .collect();
            let lowered = [LoweringPolicy::Reachable, LoweringPolicy::Comptime]
                .into_iter()
                .find(|&p| self.engine.package_lowered(&package.files, p));
            let Some(snapshot) = self
                .engine
                .package_snapshot(&package.package, &package.closure)
            else {
                continue;
            };
            let mut ordinals = BTreeMap::<_, usize>::new();
            let files: Vec<_> = self
                .files
                .iter()
                .filter(|(_, c)| package.closure.contains(&c.package))
                .map(|(file, c)| {
                    let index = ordinals.entry(c.package.clone()).or_default();
                    *index += 1;
                    (*file, c.package.clone(), *index - 1)
                })
                .collect();
            let sites = self.engine.anchor_sites(&files);
            self.cache.insert(
                package.key,
                crate::package_cache::Artifact {
                    snapshot,
                    sites,
                    preflight: package
                        .files
                        .iter()
                        .map(|(f, _)| preflight.get(f).cloned().unwrap_or_default())
                        .collect(),
                    files: local,
                    lowered,
                    last_phase: self.engine.last_phase,
                    environment: self.engine.environment.clone(),
                    limits: self.engine.comptime_limits,
                },
            );
        }
        Ok(CheckOutput {
            diagnostics: select_diagnostics(diagnostics, Some(&self.engine.sources)),
            completion,
            sources: self.engine.sources.clone(),
            unsafe_ffi: self.unsafe_ffi.clone(),
        })
    }
    pub fn check_for_execution(&mut self) -> Result<(), ToolError> {
        let result = self.check(LoweringPolicy::Reachable).and_then(|output| {
            if crate::queries::has_errors(&output.diagnostics) {
                Err(QueryError::Diagnostics(output.diagnostics))
            } else if output.completion != CheckCompletion::Complete {
                Err(QueryError::NotImplemented)
            } else {
                Ok(())
            }
        });
        result.map_err(|error| self.error(error))?;
        if !self.unsafe_ffi.is_empty() {
            let entry = self.entry.clone();
            let foreign = self.engine.foreign_use(&entry).map_err(|e| self.error(e))?;
            self.engine.foreign_unreplayable = !foreign.unmodeled.is_empty();
        }
        Ok(())
    }
    /// The escape from capability checking the entry reaches, if any (CAP-6).
    pub fn unsafe_reach(&mut self) -> Result<Option<crate::UnsafeReach>, ToolError> {
        let entry = self.entry.clone();
        self.engine.unsafe_reach(&entry).map_err(|e| self.error(e))
    }
    /// Write the precheck of `package`, a dependency of this program, which must
    /// check complete with no diagnostics at all (`Precheck`), to `out`. Each
    /// definition's record is computed as soon as its source checks and goes
    /// straight to `spill`, and the source's checked bodies are then released,
    /// so a precheck holds one source's products at a time, not the package's.
    pub fn precheck(
        &mut self,
        package: &PackageId,
        out: &mut impl std::io::Write,
        spill: &mut (impl std::io::Read + std::io::Write + std::io::Seek),
    ) -> Result<(), ToolError> {
        let name = format!("{}@{}", package.name, package.version);
        let revision = self.packages.iter().find(|p| &p.package == package);
        let key = revision
            .map(|r| crate::Precheck::key(self.identity, r.key, self.engine.comptime_limits));
        let mut writer = revision.map(|revision| {
            let files: Vec<_> = revision.files.iter().map(|(f, _)| *f).collect();
            crate::queries::PrecheckWriter::new(
                package.clone(),
                revision.closure.clone(),
                &files,
                std::io::BufWriter::new(&mut *spill),
            )
        });
        let mut released = false;
        let output = self
            .check_with(LoweringPolicy::Comptime, |engine, file| match &mut writer {
                Some(writer) if writer.has_file(file) => {
                    let paths = engine.file_definitions(file);
                    engine.precheck_definitions(writer, paths)?;
                    // The first source's GEN-5 verdict checked every body of the
                    // program. The package's other bodies are checked again with
                    // their own sources rather than all held until then.
                    if !released {
                        released = true;
                        engine.release_unwritten(writer);
                    }
                    Ok(())
                }
                _ => Ok(()),
            })
            .map_err(|error| self.error(error))?;
        if !output.diagnostics.is_empty() {
            return Err(self.error(QueryError::Diagnostics(output.diagnostics)));
        }
        if output.completion != CheckCompletion::Complete {
            return Err(ToolError::Runtime(format!(
                "precheck: {name} does not check completely"
            )));
        }
        if self.unsafe_ffi.iter().any(|p| &p.package == package) {
            return Err(ToolError::Runtime(format!(
                "precheck: {name} is an unsafe ffi package"
            )));
        }
        let (Some(writer), Some(key)) = (writer, key) else {
            return Err(ToolError::Runtime(format!(
                "precheck: {name} is not a dependency"
            )));
        };
        let head = self
            .engine
            .finish_precheck(writer, key)
            .map_err(|error| self.error(error))?;
        out.write_all(&head)?;
        spill.rewind()?;
        std::io::copy(spill, out)?;
        Ok(())
    }
    /// The reference interpreter calls no C: a program whose entry reaches an
    /// extern refuses before entry (PROG-4).
    pub fn refuse_foreign_interpretation(&mut self) -> Result<(), ToolError> {
        // Externs check only inside `unsafe ffi` packages.
        if self.unsafe_ffi.is_empty() {
            return Ok(());
        }
        let entry = self.entry.clone();
        let foreign = self.engine.foreign_use(&entry).map_err(|e| self.error(e))?;
        match foreign.externs.first() {
            Some(extern_) => Err(ToolError::Runtime(format!(
                "launch: the reference interpreter does not call C, and {extern_} is an \
                 extern \"c\" function; build or run the program natively (PROG-4)"
            ))),
            None => Ok(()),
        }
    }
    /// Recording or replaying a run whose entry reaches an extern through anything but
    /// a deterministic wrapper is a launch refusal naming that use (PROG-4).
    pub fn refuse_unreplayable(&mut self) -> Result<(), ToolError> {
        if self.unsafe_ffi.is_empty() {
            return Ok(());
        }
        let entry = self.entry.clone();
        let foreign = self.engine.foreign_use(&entry).map_err(|e| self.error(e))?;
        match foreign.unmodeled.first_key_value() {
            Some((user, extern_)) => Err(ToolError::Runtime(format!(
                "launch: {user} calls the extern {extern_} and is not a wrapper with a \
                 deterministic model, so this run cannot be recorded or replayed (PROG-4)"
            ))),
            None => Ok(()),
        }
    }
    pub fn error(&self, error: QueryError) -> ToolError {
        ToolError::Query {
            error,
            phase: self.engine.unavailable,
            sources: self.engine.sources.clone(),
        }
    }
}

/// Preserve a package corpus input for replay after moving its artifact directory.
/// Returns false for a standalone input, whose source alone is its complete fixture.
pub fn bundle_program(path: &Path, source: &str, dir: &Path) -> Result<bool, ToolError> {
    let sidecar = path.with_extension("hir.json");
    if !sidecar.try_exists()? {
        return Ok(false);
    }
    let primary = std::fs::canonicalize(path)?;
    let mut graph = Graph::default();
    let root = graph.visit(&sidecar, None)?;
    std::fs::create_dir_all(dir)?;
    let manifests: BTreeMap<_, _> = graph
        .packages
        .iter()
        .map(|(id, p)| {
            let name = if id == &root {
                "source.hir.json".into()
            } else {
                format!("package-{}-{}.json", id.name, id.version)
            };
            (p.path.clone(), name)
        })
        .collect();
    for (id, p) in &graph.packages {
        let mut value: Value = serde_json::from_slice(&p.manifest).unwrap();
        for (index, (file, _)) in p.sources.iter().enumerate() {
            let name = if file == &primary {
                "source.hir".into()
            } else {
                format!("source-{}-{}-{index}.hir", id.name, id.version)
            };
            let text = if file == &primary {
                source.into()
            } else {
                crate::tools::read_source(file)?
            };
            std::fs::write(dir.join(&name), text)?;
            value["sources"][index]["path"] = name.into();
        }
        for dependency in value["dependencies"].as_array_mut().unwrap() {
            let target = std::fs::canonicalize(
                p.path
                    .parent()
                    .unwrap()
                    .join(dependency["manifest"].as_str().unwrap()),
            )?;
            dependency["manifest"] = manifests[&target].clone().into();
        }
        std::fs::write(
            dir.join(&manifests[&p.path]),
            serde_json::to_vec_pretty(&value).unwrap(),
        )?;
    }
    Ok(true)
}
/// Load with explicit artifact storage, or `PackageCache::disabled()` for a fresh oracle.
pub(crate) fn load_program_source_with_cache(
    compiler: &Compiler,
    path: &Path,
    replacement: Option<&str>,
    cache: &PackageCache,
    package_only: bool,
) -> Result<LoadedProgram, ToolError> {
    let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::S0);
    let manifest = if path.extension().is_some_and(|e| e == "json") {
        Some(path.to_path_buf())
    } else {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(".json");
        let sidecar = PathBuf::from(sidecar);
        match std::fs::metadata(&sidecar) {
            Ok(_) => Some(sidecar),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        }
    };
    let Some(manifest) = manifest else {
        let (mut engine, file) = compiler.load(path)?;
        if let Some(text) = replacement {
            engine.sources = SourceMap::default();
            let file = engine
                .sources
                .add(path, text.into())
                .ok_or(ToolError::SourceTooLarge)?;
            return Ok(LoadedProgram {
                engine,
                files: vec![(file, default_context())],
                entry: entry_path(&default_context()),
                primary: file,
                unsafe_ffi: vec![],
                precheck_notes: vec![],
                packages: vec![],
                cache: cache.clone(),
                identity: compiler.identity(),
                indexed_revision: 0,
            });
        }
        return Ok(LoadedProgram {
            engine,
            files: vec![(file, default_context())],
            entry: entry_path(&default_context()),
            primary: file,
            unsafe_ffi: vec![],
            precheck_notes: vec![],
            packages: vec![],
            cache: cache.clone(),
            identity: compiler.identity(),
            indexed_revision: 0,
        });
    };
    let mut graph = Graph::default();
    let package = graph.visit(&manifest, None)?;
    let root = graph.packages.get(&package).unwrap();
    let entry = if package_only {
        entry_path(
            &root
                .sources
                .first()
                .ok_or_else(|| invalid("test package has no sources"))?
                .1,
        )
    } else {
        root.entry
            .clone()
            .ok_or_else(|| invalid("launch manifest requires entry"))?
    };
    let primary_path = if path.extension().is_some_and(|e| e == "json") {
        root.sources
            .iter()
            .find(|(_, c)| c.modules == entry.0.modules)
            .map(|(p, _)| p.clone())
            .ok_or_else(|| invalid("entry module has no source"))?
    } else {
        std::fs::canonicalize(path)?
    };
    if !root.sources.iter().any(|(p, _)| p == &primary_path) {
        return Err(invalid(
            "sidecar must assign its source to the root package",
        ));
    }
    let selection = |id: &PackageId| {
        ContentHash::of_parts(
            "hirsel.package.selection.v1",
            [
                id.name.as_bytes(),
                id.version.as_bytes(),
                graph.packages[id].manifest.as_slice(),
            ],
        )
    };
    let manifest_hash = |packages: &mut dyn Iterator<Item = &PackageId>| {
        let parts: Vec<_> = packages.map(selection).collect();
        ContentHash::of_parts(
            "hirsel.package.graph.v1",
            parts.iter().map(|h| h.0.as_slice()),
        )
    };
    let mut environment = default_environment();
    environment.package_manifest_hash = manifest_hash(&mut graph.packages.keys());
    let mut files = Vec::new();
    let mut texts = Vec::new();
    let mut assigned = BTreeMap::new();
    for p in graph.packages.values() {
        for (source, context) in &p.sources {
            if assigned.insert(source.clone(), context.clone()).is_some() {
                return Err(invalid("one source path has more than one FileContext"));
            }
            let text = if source == &primary_path && replacement.is_some() {
                replacement.unwrap().to_string()
            } else {
                // Preserve the standalone UTF-8 diagnostic policy and original bytes.
                crate::tools::read_source(source)?
            };
            files.push((source.clone(), context.clone()));
            texts.push(text);
        }
    }
    // Dependencies key on their own closure; the program package on the graph.
    let dependencies: Vec<_> = graph.packages.keys().filter(|id| **id != package).collect();
    let manifests: BTreeMap<_, _> = dependencies
        .iter()
        .map(|id| {
            let closure = graph.closure(id);
            ((*id).clone(), manifest_hash(&mut closure.iter()))
        })
        .collect();
    let limits = hirsel_interp::Limits::default();
    let mut revisions = vec![];
    for id in &dependencies {
        let key = graph.revision(id, &environment, &files, &texts);
        let cached = cache.select(key, limits);
        revisions.push(crate::package_cache::PackageRevision {
            package: (*id).clone(),
            key,
            closure: graph.closure(id),
            files: vec![],
            artifact: cached.as_ref().and_then(|p| p.artifact.clone()),
            lowerings: cached.map(|p| p.lowerings),
            reused: false,
            proven: false,
            diagnostics: None,
        });
    }
    let unsafe_ffi: Vec<UnsafeFfiPackage> = graph
        .packages
        .iter()
        .filter_map(|(id, p)| {
            let (link, headers_checked, wrappers) = p.unsafe_ffi.clone()?;
            Some(UnsafeFfiPackage {
                package: id.clone(),
                link,
                headers_checked,
                wrappers,
            })
        })
        .collect();
    let unsafe_ffi_metadata: BTreeMap<_, _> = unsafe_ffi
        .iter()
        .map(|p| {
            (
                p.package.clone(),
                hirsel_types::UnsafeFfi {
                    wrappers: p.wrappers.clone(),
                },
            )
        })
        .collect();
    let input = crate::queries::ImageInput {
        unsafe_ffi: unsafe_ffi_metadata.clone(),
        environment: QueryEnvironment {
            package_manifest_hash: ContentHash::of_bytes(b"hirsel.package-image.v1"),
            ..environment.clone()
        },
        limits,
        manifests: manifests.clone(),
        exposures: dependencies
            .iter()
            .map(|id| ((*id).clone(), graph.packages[*id].exposures.clone()))
            .collect(),
        files: files
            .iter()
            .zip(&texts)
            .map(|((path, context), text)| {
                let dependency = context.package != package;
                (
                    path.clone(),
                    context.clone(),
                    dependency.then(|| text.clone()),
                )
            })
            .collect(),
        packages: revisions
            .iter()
            .map(|r| crate::queries::ImagePackage {
                package: r.package.clone(),
                closure: r.closure.clone(),
                artifact: r.artifact.clone(),
                lowerings: r.lowerings.clone(),
            })
            .collect(),
    };
    let image = crate::queries::PackageImage::obtain(cache, &input, || compiler.adapters());
    let mut engine = compiler.engine(environment);
    engine.comptime_limits = limits;
    engine.package_manifests = manifests;
    {
        let inputs = engine.declarations.inputs_mut();
        inputs.package_exposures = graph
            .packages
            .iter()
            .map(|(id, p)| (id.clone(), p.exposures.clone()))
            .collect();
        inputs.signature_metadata.unsafe_ffi = unsafe_ffi_metadata;
    }
    let mut primary = None;
    let mut ids = Vec::new();
    for ((source, context), text) in files.iter().zip(texts) {
        let file = engine
            .sources
            .add_logical(
                source,
                &graph.packages[&context.package].logical_sources[source],
                text,
            )
            .ok_or(ToolError::SourceTooLarge)?;
        if source == &primary_path {
            primary = Some(file);
        }
        ids.push((file, context.clone()));
    }
    engine.foreign_links = unsafe_ffi
        .iter()
        .flat_map(|p| p.link.iter().cloned())
        .collect();
    engine.install_image(&image);
    let mut program = LoadedProgram {
        engine,
        files: ids,
        entry,
        primary: primary.ok_or_else(|| invalid("missing primary source"))?,
        unsafe_ffi,
        precheck_notes: vec![],
        packages: vec![],
        cache: cache.clone(),
        identity: compiler.identity(),
        indexed_revision: 0,
    };
    // Dependency files come parsed from the image. One that failed there is
    // parsed again here, so its failure stops loading in file order with the
    // same engine state as any other parse failure.
    for (file, context) in &program.files {
        if context.package != package && program.engine.parsed_in_image(*file) {
            continue;
        }
        program
            .engine
            .reparse_file(*file, context)
            .map_err(|e| program.error(e))?;
    }
    // A declared wrapper names an ordinary function of its own package.
    for package in &program.unsafe_ffi {
        let functions: BTreeMap<_, _> = program
            .files
            .iter()
            .filter(|(_, c)| c.package == package.package)
            .flat_map(|(file, _)| program.engine.file_functions(*file))
            .collect();
        for wrapper in package.wrappers.keys() {
            if functions.get(wrapper) != Some(&false) {
                return Err(invalid(&format!(
                    "unsafe_ffi wrapper {wrapper} is not a non-extern function of {}@{}",
                    package.package.name, package.package.version
                )));
            }
        }
    }
    for mut revision in revisions {
        revision.proven = image.proven(&revision.package);
        revision.files = program
            .files
            .iter()
            .filter(|(_, c)| c.package == revision.package)
            .cloned()
            .collect();
        program.packages.push(revision);
    }
    if let Some(toolchain) = cache.prechecks() {
        for revision in &program.packages {
            let key = crate::Precheck::key(program.identity, revision.key, limits);
            let candidates: Vec<_> = toolchain
                .prechecks
                .iter()
                .filter(|s| s.package() == &revision.package)
                .collect();
            let name = format!("{}@{}", revision.package.name, revision.package.version);
            match candidates.iter().find(|s| s.matches(key)) {
                Some(precheck) if graph.packages[&revision.package].unsafe_ffi.is_none() => {
                    let files = revision.files.iter().map(|(f, _)| *f).collect();
                    program.engine.adopt_precheck((*precheck).clone(), files);
                }
                _ if !candidates.is_empty() => program.precheck_notes.push(format!(
                    "the precheck for {name} was checked under another compiler or other sources; checking {name}"
                )),
                _ => {
                    if let Some(missing) = &toolchain.missing {
                        program.precheck_notes.push(format!("{missing}; checking {name}"));
                    }
                }
            }
        }
    }
    if crate::package_cache::reporting_prechecks() {
        for note in &program.precheck_notes {
            eprintln!("hirsel: {note}");
        }
    }
    let sites: BTreeMap<_, _> = graph
        .packages
        .iter()
        .flat_map(|(id, p)| {
            p.sources
                .iter()
                .enumerate()
                .map(|(index, (source, _))| {
                    let file = program
                        .files
                        .iter()
                        .find(|(f, _)| &program.engine.sources.file(*f).unwrap().path == source)
                        .unwrap()
                        .0;
                    ((id.clone(), index), file)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    program.indexed_revision = program.engine.package_cache_revision();
    // An artifact's diagnostics name its publisher's anchors: placing them in
    // this program's SourceMap locates them; stored artifacts stay immutable.
    for p in &mut program.packages {
        if let Some(artifact) = &p.artifact {
            program.engine.place_anchors(&artifact.sites, &sites);
            p.diagnostics = Some(crate::package_cache::PackageDiagnostics {
                preflight: artifact.preflight.clone(),
                files: artifact.files.clone(),
            });
        }
        if let Some(lowerings) = &p.lowerings {
            let source_sites = graph
                .packages
                .iter()
                .filter(|(id, _)| p.closure.contains(*id))
                .flat_map(|(id, package)| {
                    package
                        .sources
                        .iter()
                        .enumerate()
                        .map(|(index, _)| (sites[&(id.clone(), index)], id.clone(), index))
                        .collect::<Vec<_>>()
                })
                .collect();
            program.engine.register_package_publication(
                p.package.clone(),
                p.closure.clone(),
                source_sites,
                lowerings.clone(),
            );
        }
    }
    Ok(program)
}

/// The manifest is canonical; the source retains its declared spelling (so a
/// symlink does not leak its physical target into the artifact). Their relationship
/// enters artifacts; no host root becomes part of a source name. This also
/// supports sources outside the manifest directory and absolute manifest entries.
fn logical_source_path(source: &Path, manifest: &Path, package: &PackageId) -> PathBuf {
    let base = manifest.parent().unwrap();
    let common = base
        .ancestors()
        .find(|ancestor| source.starts_with(ancestor))
        .unwrap();
    let mut relative = PathBuf::new();
    for _ in base.strip_prefix(common).unwrap().components() {
        relative.push("..");
    }
    relative.push(source.strip_prefix(common).unwrap());
    PathBuf::from(format!("{}@{}", package.name, package.version)).join(relative)
}

struct SelectedPackage {
    path: PathBuf,
    manifest: Vec<u8>,
    sources: Vec<(PathBuf, FileContext)>,
    logical_sources: BTreeMap<PathBuf, PathBuf>,
    exposures: Vec<DependencyExposure>,
    entry: Option<DefPath>,
    dependencies: Vec<PackageId>,
    unsafe_ffi: Option<ForeignManifest>,
}
/// `unsafe_ffi`: link inputs, headers_checked and wrappers (PROG-4).
type ForeignManifest = (
    Vec<LinkInput>,
    bool,
    BTreeMap<DefPath, hirsel_types::ForeignWrapper>,
);
#[derive(Default)]
struct Graph {
    packages: BTreeMap<PackageId, SelectedPackage>,
    active: Vec<PathBuf>,
}
impl Graph {
    fn closure(&self, id: &PackageId) -> BTreeSet<PackageId> {
        let mut result = BTreeSet::new();
        let mut todo = vec![id.clone()];
        while let Some(id) = todo.pop() {
            if result.insert(id.clone()) {
                todo.extend(self.packages[&id].dependencies.clone());
            }
        }
        result
    }
    fn revision(
        &self,
        id: &PackageId,
        environment: &QueryEnvironment,
        files: &[(PathBuf, FileContext)],
        texts: &[String],
    ) -> ContentHash {
        let mut parts = vec![
            environment.interface_version.to_le_bytes().to_vec(),
            environment.compiler_version.as_bytes().to_vec(),
            environment.spec_hash.0.to_vec(),
            environment.builtin_signatures_hash.0.to_vec(),
        ];
        for id in self.closure(id) {
            let package = &self.packages[&id];
            parts.extend([
                id.name.as_bytes().to_vec(),
                id.version.as_bytes().to_vec(),
                ContentHash::of_bytes(&package.manifest).0.to_vec(),
            ]);
            for (source, context) in &package.sources {
                let index = files
                    .iter()
                    .position(|(path, c)| c == context && path == source)
                    .unwrap();
                parts.push(ContentHash::of_bytes(texts[index].as_bytes()).0.to_vec());
            }
        }
        ContentHash::of_parts("hirsel.checked-package.v1", parts.iter().map(Vec::as_slice))
    }
    fn visit(&mut self, path: &Path, expected: Option<&PackageId>) -> Result<PackageId, ToolError> {
        let path = std::fs::canonicalize(path)?;
        if self.active.contains(&path) {
            return Err(invalid("cyclic package dependency"));
        }
        let bytes = std::fs::read(&path)?;
        let value: UniqueJson = serde_json::from_slice(&bytes)
            .map_err(|e| invalid(&format!("{}: {e}", path.display())))?;
        let object = object_value(
            &value.0,
            &[
                "schema",
                "package",
                "sources",
                "dependencies",
                "entry",
                "unsafe_ffi",
            ],
        )?;
        if object.get("schema").and_then(Value::as_u64) != Some(1) {
            return Err(invalid("package schema must be 1"));
        }
        let package = package_value(required(object, "package")?)?;
        if package.package_is("core", "1") {
            return Err(invalid("core@1 is compiler-owned"));
        }
        if expected.is_some_and(|e| e != &package) {
            return Err(invalid(
                "selected package version does not match dependency",
            ));
        }
        if let Some(old) = self.packages.get(&package) {
            if old.path != path {
                return Err(invalid(
                    "two implementation revisions selected for one package",
                ));
            }
            return Ok(package);
        }
        self.active.push(path.clone());
        let base = path.parent().unwrap();
        let mut logical_sources = BTreeMap::new();
        let sources = array(required(object, "sources")?)?
            .iter()
            .map(|s| {
                let s = object_value(s, &["path", "module"])?;
                let declared = Path::new(string(required(s, "path")?)?);
                let source = std::fs::canonicalize(base.join(declared))?;
                // Absolute entries may name the same directory through an alias
                // such as /tmp. Compare canonical paths in that case; relative
                // entries retain their declared names, including symlinks.
                let logical = if declared.is_absolute() {
                    source.clone()
                } else {
                    base.join(declared)
                };
                logical_sources.insert(
                    source.clone(),
                    logical_source_path(&logical, &path, &package),
                );
                let modules = modules(required(s, "module")?)?;
                Ok((
                    source,
                    FileContext {
                        package: package.clone(),
                        modules,
                    },
                ))
            })
            .collect::<Result<Vec<_>, ToolError>>()?;
        if sources.is_empty() {
            return Err(invalid("package requires at least one source"));
        }
        let entry = object
            .get("entry")
            .map(|e| -> Result<DefPath, ToolError> {
                let e = object_value(e, &["module", "name"])?;
                Ok(DefPath(hirsel_syntax::ItemPath {
                    package: package.clone(),
                    modules: modules(required(e, "module")?)?,
                    name: identifier(required(e, "name")?)?,
                }))
            })
            .transpose()?;
        let mut exposures = Vec::new();
        let mut dependencies = Vec::new();
        for d in array(required(object, "dependencies")?)? {
            let d = object_value(d, &["manifest", "package", "expose"])?;
            let expected = package_value(required(d, "package")?)?;
            let selected = self.visit(
                &base.join(string(required(d, "manifest")?)?),
                Some(&expected),
            )?;
            // Every foreign library of a program is chosen by its root (PROG-4).
            if self.active.len() > 1 && self.packages[&selected].unsafe_ffi.is_some() {
                return Err(invalid(&format!(
                    "only the root launch manifest may select the unsafe ffi package {}@{}",
                    selected.name, selected.version
                )));
            }
            dependencies.push(selected.clone());
            // Module heads are source conveniences; package/version identity is
            // still pinned by this dependency's manifest selection.
            for (_, context) in &self.packages[&selected].sources {
                if let Some(head) = context.modules.first() {
                    exposures.push(DependencyExposure {
                        head: head.clone(),
                        package: selected.clone(),
                        modules: vec![head.clone()],
                    });
                }
            }
            for exposure in array(required(d, "expose")?)? {
                let e = object_value(exposure, &["head", "module"])?;
                exposures.push(DependencyExposure {
                    head: identifier(required(e, "head")?)?,
                    package: selected.clone(),
                    modules: modules(required(e, "module")?)?,
                });
            }
        }
        if package.name_is("std") {
            for (_, context) in &sources {
                if let Some(head) = context.modules.first() {
                    exposures.push(DependencyExposure {
                        head: head.clone(),
                        package: package.clone(),
                        modules: vec![head.clone()],
                    });
                }
            }
        }
        let unsafe_ffi = object
            .get("unsafe_ffi")
            .map(|v| foreign_manifest(v, base, &package))
            .transpose()?;
        self.active.pop();
        self.packages.insert(
            package.clone(),
            SelectedPackage {
                path,
                manifest: bytes,
                sources,
                logical_sources,
                exposures,
                entry,
                dependencies,
                unsafe_ffi,
            },
        );
        Ok(package)
    }
}
/// Parse `{"link": [..], "headers_checked": b, "wrappers": [..]}` (PROG-4).
fn foreign_manifest(
    v: &Value,
    base: &Path,
    package: &PackageId,
) -> Result<ForeignManifest, ToolError> {
    let o = object_value(v, &["link", "headers_checked", "wrappers"])?;
    let link = array(required(o, "link")?)?
        .iter()
        .map(|l| {
            let l = string(l)?;
            if l.contains('/') || [".a", ".o", ".so"].iter().any(|s| l.ends_with(s)) {
                Ok(LinkInput::File(std::fs::canonicalize(base.join(l))?))
            } else if l
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
                && !l.starts_with('-')
            {
                Ok(LinkInput::Library(l.into()))
            } else {
                Err(invalid(&format!("invalid link library {l:?}")))
            }
        })
        .collect::<Result<Vec<_>, ToolError>>()?;
    let headers_checked = required(o, "headers_checked")?
        .as_bool()
        .ok_or_else(|| invalid("headers_checked must be a boolean"))?;
    let mut wrappers = BTreeMap::new();
    for w in array(required(o, "wrappers")?)? {
        let w = object_value(w, &["module", "name", "kind", "model"])?;
        let definition = DefPath(hirsel_syntax::ItemPath {
            package: package.clone(),
            modules: modules(required(w, "module")?)?,
            name: identifier(required(w, "name")?)?,
        });
        let kind = match string(required(w, "kind")?)? {
            "pure" => hirsel_types::WrapperKind::Pure,
            "handle" => hirsel_types::WrapperKind::Handle,
            "effect" => hirsel_types::WrapperKind::Effect,
            other => return Err(invalid(&format!("unknown wrapper kind {other:?}"))),
        };
        let deterministic = match string(required(w, "model")?)? {
            "deterministic" => true,
            "none" => false,
            other => return Err(invalid(&format!("unknown wrapper model {other:?}"))),
        };
        let wrapper = hirsel_types::ForeignWrapper {
            kind,
            deterministic,
        };
        if wrappers.insert(definition.clone(), wrapper).is_some() {
            return Err(invalid(&format!("duplicate wrapper {definition}")));
        }
    }
    Ok((link, headers_checked, wrappers))
}
fn invalid(message: &str) -> ToolError {
    ToolError::Runtime(format!("package manifest: {message}"))
}
fn object_value<'a>(v: &'a Value, keys: &[&str]) -> Result<&'a Map<String, Value>, ToolError> {
    let o = v.as_object().ok_or_else(|| invalid("expected object"))?;
    if let Some(k) = o.keys().find(|k| !keys.contains(&k.as_str())) {
        return Err(invalid(&format!("unknown field {k}")));
    }
    Ok(o)
}
fn required<'a>(o: &'a Map<String, Value>, k: &str) -> Result<&'a Value, ToolError> {
    o.get(k)
        .ok_or_else(|| invalid(&format!("missing field {k}")))
}
fn array(v: &Value) -> Result<&Vec<Value>, ToolError> {
    v.as_array().ok_or_else(|| invalid("expected array"))
}
fn string(v: &Value) -> Result<&str, ToolError> {
    v.as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("expected nonempty string"))
}
fn identifier(v: &Value) -> Result<String, ToolError> {
    let s = string(v)?;
    if !s
        .bytes()
        .enumerate()
        .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit())
    {
        return Err(invalid("noncanonical identifier"));
    }
    let mut sources = SourceMap::default();
    let file = sources.add("manifest identifier", s.into()).unwrap();
    let tokens = hirsel_lexer::lex(file, &sources);
    if !tokens.diagnostics.is_empty()
        || !matches!(
            tokens.tokens.first().map(|t| &t.kind),
            Some(hirsel_syntax::TokenKind::Ident(_))
        )
    {
        return Err(invalid("reserved keyword cannot be a manifest name"));
    }
    Ok(s.into())
}
fn modules(v: &Value) -> Result<Vec<String>, ToolError> {
    array(v)?.iter().map(identifier).collect()
}
fn package_value(v: &Value) -> Result<PackageId, ToolError> {
    let o = object_value(v, &["name", "version"])?;
    let version = string(required(o, "version")?)?;
    if version.parse::<u64>().is_err() || version != version.parse::<u64>().unwrap().to_string() {
        return Err(invalid(
            "package version must be a canonical decimal u64 major",
        ));
    }
    Ok(PackageId {
        name: identifier(required(o, "name")?)?,
        version: version.into(),
    })
}

// serde_json::Value alone permits duplicate object keys by overwriting them.
// Package selection must reject ambiguous input instead.
struct UniqueJson(Value);
impl<'de> serde::Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!("duplicate field {key}")));
                    }
                    out.insert(key, map.next_value::<UniqueJson>()?.0);
                }
                Ok(UniqueJson(Value::Object(out)))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = seq.next_element::<UniqueJson>()? {
                    out.push(v.0);
                }
                Ok(UniqueJson(Value::Array(out)))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<UniqueJson, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueJson(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::Null))
            }
        }
        d.deserialize_any(Visitor)
    }
}
